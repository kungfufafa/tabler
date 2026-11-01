//! Opening the session's connections: the TCP socket (to the SSH tunnel's port when there is
//! one), TLS by the profile's SSL mode after Vertica's TLS request, the login with the password
//! or OAuth token (a Command's output run again once when the server refuses it), and the setup
//! every connection runs before the session uses it: the profile's time zone, on a read-only
//! session the read-only transaction mode, then its startup commands, which a read-only session
//! checks first and the server then holds to read-only too.
//!
//! A cancel goes over a plain socket of its own, as vertica-python sends it: the request carries
//! only the session's backend key.

use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use super::auth::Credential;
use super::conn::{self, Connection, Flow, StartupOptions};
use super::params::{DEFAULT_CLIENT_LABEL, VerticaAuth, VerticaParams};
use super::session::refused_read_only;
use super::wire::BackendKey;
use crate::drivers::{ConnectTarget, trace_sql};
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::sqlgen::dialect::{SqlDialect, vertica};
use crate::sqlgen::sql::{redact, split_statements};
use crate::types::profile::{
    ConnectionProfile, DEFAULT_CONNECT_TIMEOUT_SECS, Driver, PasswordMode, SslMode,
};

const DIALECT: SqlDialect = SqlDialect::Vertica;

/// A socket, plain or in TLS.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

pub type Stream = Box<dyn Io>;

/// The TLS a connection negotiated, for Session Details.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsInfo {
    pub version: Option<String>,
    pub cipher: Option<String>,
}

/// One logged-in, set-up connection.
pub struct Conn {
    pub inner: Connection<Stream>,
    /// None: the connection is not encrypted.
    pub tls: Option<TlsInfo>,
}

impl Conn {
    pub fn key(&self) -> Option<BackendKey> {
        self.inner.key()
    }

    pub fn is_broken(&self) -> bool {
        self.inner.is_broken()
    }
}

/// Where the password comes from.
enum Secret {
    Fixed(String),
    /// A Command's output, run again when it is older than the descriptor's TTL or refused.
    Command(Arc<CommandPassword>),
}

/// TLS for a connection: the config and the name the certificate is checked for.
struct Tls {
    config: Arc<rustls::ClientConfig>,
    name: ServerName<'static>,
}

/// Everything a new connection of the session needs.
pub struct Dial {
    host: String,
    port: u16,
    ssl: SslMode,
    tls: Option<Tls>,
    options: StartupOptions,
    auth: VerticaAuth,
    secret: Secret,
    timeout: Duration,
    /// What each connection runs after the login, in order.
    setup: Vec<String>,
}

/// Why opening a connection failed: the server refused the credential (a Command password is
/// then run again once), or anything else.
fn refused_login(e: &AppError) -> bool {
    matches!(e, AppError::Auth(_))
}

impl Dial {
    /// The dial of `target`: its address (the tunnel's when there is one), TLS, credential and
    /// setup, checked. `read_only` makes each connection read-only on the server and refuses
    /// startup commands that write.
    pub fn new(
        target: &ConnectTarget<'_>,
        params: &VerticaParams,
        read_only: bool,
    ) -> Result<Dial, AppError> {
        let profile = target.profile;
        let user = profile
            .user
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .ok_or_else(|| AppError::InvalidInput("User is required".into()))?;
        let database = profile
            .database
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .ok_or_else(|| AppError::InvalidInput("Database is required".into()))?;
        let tls = match profile.ssl {
            SslMode::Disable => None,
            mode => {
                let identity = crate::tls_files::load_client_certificate(
                    profile.options.ssl_key_path.as_deref(),
                    profile.options.ssl_cert_path.as_deref(),
                )?;
                let ca = profile
                    .ssl_ca_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty());
                let config = crate::tls_client::rustls_config(mode, ca, identity, &[], None)?;
                Some(Tls {
                    config: Arc::new(config),
                    name: crate::tls_client::server_name(&target.tls_host())?,
                })
            }
        };
        let ttl = Duration::from_secs(Driver::Vertica.descriptor().command_ttl_secs.into());
        let secret = match CommandPassword::for_profile_ttl(profile, target.password, ttl) {
            Some(command) => Secret::Command(command),
            None => Secret::Fixed(match profile.options.password_mode() {
                PasswordMode::None => String::new(),
                _ => target.password.unwrap_or_default().to_string(),
            }),
        };
        let mut options = StartupOptions::new(user, database);
        options.client_label = params
            .client_label()
            .unwrap_or(DEFAULT_CLIENT_LABEL)
            .to_string();
        options.workload = params.workload().map(str::to_string);
        Ok(Dial {
            host: target.host(),
            port: target.port(),
            ssl: profile.ssl,
            tls,
            options,
            auth: params.auth,
            secret,
            timeout: connect_timeout(profile),
            setup: setup_statements(profile, read_only)?,
        })
    }

    /// A new connection, in autocommit or not, set up; within the connect timeout.
    pub async fn open(&self, autocommit: bool) -> Result<Conn, AppError> {
        let secret = self.password(false).await?;
        match self.attempt(autocommit, &secret).await {
            Err(e) if refused_login(&e) && matches!(self.secret, Secret::Command(_)) => {
                // The token may have expired before its TTL: one more run of the command.
                let secret = self.password(true).await?;
                self.attempt(autocommit, &secret).await
            }
            other => other,
        }
    }

    async fn password(&self, refresh: bool) -> Result<String, AppError> {
        match &self.secret {
            Secret::Fixed(p) => Ok(p.clone()),
            Secret::Command(command) => {
                if refresh {
                    command.invalidate().await;
                }
                command.password().await
            }
        }
    }

    fn credential(&self, secret: &str) -> Credential {
        match self.auth {
            VerticaAuth::Password => Credential::Password(secret.to_string()),
            VerticaAuth::OAuth => Credential::OAuthToken(secret.trim().to_string()),
        }
    }

    async fn attempt(&self, autocommit: bool, secret: &str) -> Result<Conn, AppError> {
        let credential = self.credential(secret);
        let opening = async {
            let (stream, tls) = self.stream().await?;
            let mut options = self.options.clone();
            options.autocommit = autocommit;
            let encrypted = tls.is_some();
            let inner = Connection::startup(stream, &options, &credential, encrypted).await?;
            let mut conn = Conn { inner, tls };
            self.set_up(&mut conn, autocommit).await?;
            Ok(conn)
        };
        match tokio::time::timeout(self.timeout, opening).await {
            Ok(result) => result,
            Err(_) => Err(AppError::Connect(format!(
                "{}:{}: no answer within {} s",
                self.host,
                self.port,
                self.timeout.as_secs()
            ))),
        }
    }

    async fn socket(&self) -> Result<TcpStream, AppError> {
        let socket = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|e| AppError::Connect(format!("{}:{}: {e}", self.host, self.port)))?;
        let _ = socket.set_nodelay(true);
        Ok(socket)
    }

    /// The socket, in TLS when the SSL mode asks for it and the server agrees (`prefer` goes on
    /// without when it does not; the stricter modes fail).
    async fn stream(&self) -> Result<(Stream, Option<TlsInfo>), AppError> {
        let mut socket = self.socket().await?;
        let Some(tls) = &self.tls else {
            return Ok((Box::new(socket), None));
        };
        if !conn::negotiate_tls(&mut socket).await? {
            if self.ssl == SslMode::Prefer {
                return Ok((Box::new(socket), None));
            }
            return Err(AppError::Connect(
                "The Vertica server does not accept TLS connections (choose SSL mode \
                 \u{201c}disable\u{201d} or \u{201c}prefer\u{201d}, or enable TLS on the server)"
                    .into(),
            ));
        }
        let connector = tokio_rustls::TlsConnector::from(Arc::clone(&tls.config));
        let stream = connector
            .connect(tls.name.clone(), socket)
            .await
            .map_err(|e| AppError::Connect(format!("Vertica TLS: {e}")))?;
        let (_, session) = stream.get_ref();
        let info = TlsInfo {
            version: session
                .protocol_version()
                .and_then(|v| v.as_str())
                .map(|v| v.replace('_', ".")),
            cipher: session
                .negotiated_cipher_suite()
                .map(|s| format!("{:?}", s.suite())),
        };
        Ok((Box::new(stream), Some(info)))
    }

    /// The setup of a new connection: autocommit as asked (when the server did not take it
    /// from the startup message), then `setup`.
    async fn set_up(&self, conn: &mut Conn, autocommit: bool) -> Result<(), AppError> {
        let reported = conn.inner.hello().parameters.get("auto_commit").cloned();
        let mut statements: Vec<&str> = Vec::new();
        if let Some(value) = reported
            && value.eq_ignore_ascii_case("on") != autocommit
        {
            statements.push(if autocommit {
                vertica::AUTOCOMMIT_ON
            } else {
                vertica::AUTOCOMMIT_OFF
            });
        }
        statements.extend(self.setup.iter().map(String::as_str));
        for sql in statements {
            conn.inner
                .simple(sql, &mut |_| Flow::Continue)
                .await
                .map_err(|e| match e {
                    // A startup command may carry a credential (`ALTER SESSION SET AWSAuth`),
                    // and the server may quote it back: both are masked.
                    AppError::Db { message, .. } => {
                        let shown = trace_sql(DIALECT, sql, &[]);
                        AppError::Connect(format!(
                            "Vertica session setup ({}): {}",
                            shown.text,
                            shown.scrub(message)
                        ))
                    }
                    other => other,
                })?;
        }
        Ok(())
    }

    /// Asks the server to stop what the session `key` runs, over a new plain socket.
    pub async fn cancel(&self, key: BackendKey) -> Result<(), AppError> {
        let request = async {
            let socket = self.socket().await?;
            conn::send_cancel(socket, key).await
        };
        tokio::time::timeout(self.timeout, request)
            .await
            .unwrap_or(Err(AppError::Timeout))
    }
}

/// The profile's Advanced ▸ Connect timeout, else the app's default.
pub fn connect_timeout(profile: &ConnectionProfile) -> Duration {
    Duration::from_secs(
        profile
            .options
            .connect_timeout_secs
            .filter(|s| *s > 0)
            .map_or(DEFAULT_CONNECT_TIMEOUT_SECS, u64::from),
    )
}

/// What every connection of `profile` runs after the login: the time zone, the read-only
/// transaction mode when the session is read-only, then the startup commands.
pub fn setup_statements(
    profile: &ConnectionProfile,
    read_only: bool,
) -> Result<Vec<String>, AppError> {
    let mut out = Vec::new();
    if let Some(zone) = profile
        .options
        .timezone
        .as_deref()
        .map(str::trim)
        .filter(|z| !z.is_empty())
    {
        out.push(vertica::set_time_zone(zone));
    }
    // Read-only on the server before any startup command: one the classifier lets through
    // still meets the server's refusal.
    if read_only {
        out.push(vertica::READ_ONLY_SESSION.to_string());
    }
    if let Some(text) = profile.options.startup_sql.as_deref() {
        for st in split_statements(DIALECT, text) {
            let sql = st.sql.trim();
            if sql.is_empty() {
                continue;
            }
            // What the session refuses elsewhere (a write, `NEXTVAL`), it refuses here too.
            if read_only && refused_read_only(sql) {
                // The message reaches the UI and MCP clients: credentials in it are masked.
                return Err(AppError::InvalidInput(format!(
                    "Startup command \u{201c}{}\u{201d}: it writes (or is not known to only \
                     read), and the connection is read-only",
                    redact(DIALECT, sql).sql
                )));
            }
            out.push(sql.to_string());
        }
    }
    Ok(out)
}
