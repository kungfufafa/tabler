//! Logging in: the tiberius `Config` of a profile, the TCP connection (through the SSH tunnel's
//! listener when there is one, SQL Browser for a named instance, one Azure redirect) and the
//! statements every session starts with.
//!
//! Encryption follows the profile's SSL mode, like the other drivers: Disable encrypts the login
//! only (SQL Server always does that much), Prefer encrypts if the server can, Require and the
//! Verify modes always. Prefer and Require take any certificate, as PostgreSQL's modes of those
//! names do (a SQL Server's default certificate is self-signed); the Verify modes check it
//! against the system's roots plus the profile's CA file, under the server's name (or the
//! "hostname in certificate" setting). No mode falls back to an unencrypted connection when TLS
//! fails. Strict (TDS 8.0) starts TLS before the first packet.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use ::tiberius::error::Error;
use ::tiberius::{AuthMethod, Client, Config, EncryptionLevel, SqlBrowser};
use futures::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use super::conn::Conn;
use super::errors::{Phase, map_error};
use super::params::{MssqlAuth, MssqlParams};
use crate::drivers::ConnectTarget;
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::sqlgen::dialect::tsql::SESSION_SETUP;
use crate::types::profile::{ConnectionProfile, DEFAULT_CONNECT_TIMEOUT_SECS, SslMode};

/// The client over a TCP connection.
pub type TdsClient = Client<Compat<TcpStream>>;

/// Where the TCP connection goes and which server it reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// What the socket connects to: the server, or the tunnel's local listener.
    pub connect_host: String,
    pub connect_port: u16,
    /// The server itself: the login names it (Azure SQL routes by it), the certificate must be
    /// issued to it, and integrated authentication asks a ticket for it.
    pub server_host: String,
    pub server_port: u16,
    /// The profile set the port, rather than taking the default.
    pub port_given: bool,
    /// Through an SSH tunnel: SQL Browser (UDP) and redirects to other hosts cannot be followed.
    pub tunnelled: bool,
}

/// The encryption level of an SSL mode, and whether the certificate is checked.
pub fn encryption(ssl: SslMode, strict: bool) -> (EncryptionLevel, bool) {
    let verify = matches!(ssl, SslMode::VerifyCa | SslMode::VerifyFull);
    let level = match ssl {
        _ if strict => EncryptionLevel::Strict,
        SslMode::Disable => EncryptionLevel::Off,
        SslMode::Prefer => EncryptionLevel::On,
        SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => EncryptionLevel::Required,
    };
    (level, verify)
}

fn trimmed(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

/// The login method of `auth`, with the profile's user and the password (or token).
pub fn auth_method(
    auth: MssqlAuth,
    user: Option<&str>,
    password: Option<&str>,
) -> Result<AuthMethod, AppError> {
    let user = trimmed(user);
    let need_user = || {
        user.ok_or_else(|| AppError::InvalidInput("SQL Server: the user name is missing".into()))
    };
    match auth {
        MssqlAuth::SqlServer => Ok(AuthMethod::sql_server(
            need_user()?,
            password.unwrap_or_default(),
        )),
        MssqlAuth::EntraToken => {
            let token = trimmed(password).ok_or_else(|| {
                AppError::InvalidInput(
                    "SQL Server: the Microsoft Entra ID token is missing (set the password, or a \
                     command that prints one)"
                        .into(),
                )
            })?;
            Ok(AuthMethod::aad_token(token))
        }
        #[cfg(windows)]
        MssqlAuth::Windows => Ok(AuthMethod::windows(
            need_user()?,
            password.unwrap_or_default(),
        )),
        #[cfg(windows)]
        MssqlAuth::Integrated => Ok(AuthMethod::Integrated),
        #[cfg(not(windows))]
        MssqlAuth::Windows | MssqlAuth::Integrated => Err(AppError::Unsupported(
            "Windows and integrated authentication work in the Windows build of Tabler only".into(),
        )),
    }
}

/// The tiberius configuration of a profile. Never panics: tiberius panics when a certificate
/// bypass meets a CA file, which the modes here never combine.
pub fn build_config(
    profile: &ConnectionProfile,
    params: &MssqlParams,
    password: Option<&str>,
    endpoint: &Endpoint,
    connect_timeout: Duration,
) -> Result<Config, AppError> {
    params.check()?;
    let mut config = Config::new();
    config.host(&endpoint.server_host);
    if let Some(instance) = named_instance(params, endpoint) {
        // The port comes from SQL Browser.
        config.instance_name(instance);
    } else {
        config.port(endpoint.server_port);
    }
    if let Some(db) = trimmed(profile.database.as_deref()) {
        config.database(db);
    }
    config.application_name(params.application_name());
    config.authentication(auth_method(params.auth, profile.user.as_deref(), password)?);

    let (level, verify) = encryption(profile.ssl, params.strict);
    config.encryption(level);
    if verify {
        if let Some(ca) = trimmed(profile.ssl_ca_path.as_deref()) {
            config.trust_cert_ca(ca);
        }
    } else {
        config.trust_cert();
    }
    if let Some(name) = params.hostname_in_certificate() {
        config.hostname_in_certificate(name);
    }
    match (
        trimmed(profile.options.ssl_cert_path.as_deref()),
        trimmed(profile.options.ssl_key_path.as_deref()),
    ) {
        (Some(cert), Some(key)) => config.client_certificate(cert, key),
        (None, None) => {}
        _ => {
            return Err(AppError::InvalidInput(
                "SQL Server: a client certificate needs both its certificate and its key file"
                    .into(),
            ));
        }
    }

    config.readonly(params.read_only_intent);
    config.multi_subnet_failover(params.multi_subnet_failover);
    if let Some(size) = params.packet_size {
        config.packet_size(size);
    }
    config.handshake_timeout(Some(connect_timeout));
    session_options(&mut config);
    Ok(config)
}

/// How every session reads, the fake server's in the tests included.
pub fn session_options(config: &mut Config) {
    // tiberius cuts any server round-trip at 30 s by default; the app has its own query time
    // out, which cancels on the server.
    config.command_timeout(None);
    // nvarchar under a collation without supplementary characters is UCS-2 and can hold an
    // unpaired surrogate: it reads as U+FFFD, where strict decoding fails the whole result.
    config.lossy_utf16_decoding(true);
}

/// The named instance to look up through SQL Browser: only when no port was given, and never
/// over a tunnel, which carries no UDP.
pub fn named_instance<'a>(params: &'a MssqlParams, endpoint: &Endpoint) -> Option<&'a str> {
    params
        .instance()
        .filter(|_| !endpoint.port_given && !endpoint.tunnelled)
}

impl Endpoint {
    /// Where a target's connection goes: the SSH tunnel's listener when there is one, and the
    /// profile's server behind it.
    pub fn of(target: &ConnectTarget<'_>) -> Endpoint {
        let profile = target.profile;
        Endpoint {
            connect_host: target.host(),
            connect_port: target.port(),
            server_host: trimmed(profile.host.as_deref())
                .unwrap_or("localhost")
                .to_string(),
            server_port: profile
                .port
                .or_else(|| profile.default_port())
                .unwrap_or(1433),
            port_given: profile.port.is_some(),
            tunnelled: target.host_override.is_some(),
        }
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

/// The profile's startup commands (Advanced ▸ Bootstrap commands), sent as one batch.
pub fn startup_sql(profile: &ConnectionProfile) -> Option<&str> {
    trimmed(profile.options.startup_sql.as_deref())
}

/// Opens the connections of one session: each logs in with the profile's settings (the password
/// command's output while it is fresh) and runs the session setup and startup commands.
pub struct Dial {
    pub profile: ConnectionProfile,
    pub params: MssqlParams,
    pub endpoint: Endpoint,
    pub timeout: Duration,
    /// The password typed or stored; a Command password comes from `token` instead.
    pub password: Option<String>,
    pub token: Option<Arc<CommandPassword>>,
}

impl Dial {
    pub async fn conn(&self) -> Result<Conn, AppError> {
        match &self.token {
            None => self.once(self.password.as_deref()).await,
            Some(token) => {
                with_token(
                    token,
                    |password| async move { self.once(Some(&password)).await },
                )
                .await
            }
        }
    }

    async fn once(&self, password: Option<&str>) -> Result<Conn, AppError> {
        let config = build_config(
            &self.profile,
            &self.params,
            password,
            &self.endpoint,
            self.timeout,
        )?;
        let mut client = connect(config, &self.params, &self.endpoint, self.timeout).await?;
        ready(&mut client, startup_sql(&self.profile)).await?;
        Ok(Conn::spawn(client))
    }
}

/// Runs `dial` with the Command password (an Entra token expires within the hour): the one it
/// printed while it is fresh, and once more with a new one when the server refused it.
pub async fn with_token<T, F, Fut>(token: &CommandPassword, dial: F) -> Result<T, AppError>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T, AppError>>,
{
    match dial(token.password().await?).await {
        Err(AppError::Auth(message)) => {
            log::info!("SQL Server refused the command's password ({message}); running it again");
            token.invalidate().await;
            dial(token.password().await?).await
        }
        other => other,
    }
}

/// Connects and logs in, following one redirect of the server (Azure SQL's gateway).
pub async fn connect(
    config: Config,
    params: &MssqlParams,
    endpoint: &Endpoint,
    timeout: Duration,
) -> Result<TdsClient, AppError> {
    let tcp = tcp_stream(&config, params, endpoint, timeout).await?;
    let client = match Client::connect(config.clone(), tcp.compat_write()).await {
        Err(Error::Routing { host, port }) => {
            if endpoint.tunnelled {
                return Err(AppError::Connect(format!(
                    "SQL Server redirected the connection to {host}:{port}, which the SSH tunnel \
                     does not reach"
                )));
            }
            let mut config = config;
            config.host(&host);
            config.port(port);
            let routed = Endpoint {
                connect_host: host,
                connect_port: port,
                port_given: true,
                ..endpoint.clone()
            };
            let tcp = tcp_stream(&config, params, &routed, timeout).await?;
            Client::connect(config, tcp.compat_write())
                .await
                .map_err(|e| map_error(e, Phase::Connect, None))?
        }
        other => other.map_err(|e| map_error(e, Phase::Connect, None))?,
    };
    Ok(client)
}

/// What every new connection runs after the login: the session setup, then the profile's startup
/// commands.
pub async fn ready<S>(client: &mut Client<S>, startup: Option<&str>) -> Result<(), AppError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    setup(client).await?;
    if let Some(sql) = startup {
        let run = async { client.simple_query(sql).await?.into_results().await };
        run.await
            .map_err(|e| match map_error(e, Phase::Connect, Some(sql)) {
                AppError::Db { message, .. } => AppError::Connect(format!(
                    "SQL Server: the startup commands failed: {message}"
                )),
                other => other,
            })?;
    }
    Ok(())
}

async fn tcp_stream(
    config: &Config,
    params: &MssqlParams,
    endpoint: &Endpoint,
    timeout: Duration,
) -> Result<TcpStream, AppError> {
    let timed_out = || {
        AppError::Connect(format!(
            "SQL Server: no answer from {}:{} within {} s",
            endpoint.connect_host,
            endpoint.connect_port,
            timeout.as_secs()
        ))
    };
    let tcp = if named_instance(params, endpoint).is_some() {
        tokio::time::timeout(timeout, TcpStream::connect_named(config))
            .await
            .map_err(|_| timed_out())?
            .map_err(|e| map_error(e, Phase::Connect, None))?
    } else {
        let addr = (endpoint.connect_host.as_str(), endpoint.connect_port);
        let connecting = async {
            if params.multi_subnet_failover {
                connect_any(addr).await
            } else {
                TcpStream::connect(addr).await
            }
        };
        tokio::time::timeout(timeout, connecting)
            .await
            .map_err(|_| timed_out())?
            .map_err(|e| {
                AppError::Connect(format!(
                    "SQL Server: cannot reach {}:{}: {e}",
                    endpoint.connect_host, endpoint.connect_port
                ))
            })?
    };
    tcp.set_nodelay(true)
        .map_err(|e| AppError::Connect(format!("SQL Server: {e}")))?;
    Ok(tcp)
}

/// Multi-subnet failover: every address of the name at once, the first to answer wins.
async fn connect_any(addr: (&str, u16)) -> std::io::Result<TcpStream> {
    let addrs: Vec<_> = tokio::net::lookup_host(addr).await?.collect();
    if addrs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the host name has no address",
        ));
    }
    let attempts = addrs.into_iter().map(|a| Box::pin(TcpStream::connect(a)));
    futures::future::select_ok(attempts).await.map(|(s, _)| s)
}

/// The statements every session starts with (`dialect::tsql::SESSION_SETUP`).
pub async fn setup<S>(client: &mut Client<S>) -> Result<(), AppError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    client
        .simple_query(SESSION_SETUP)
        .await
        .map_err(|e| map_error(e, Phase::Connect, Some(SESSION_SETUP)))?
        .into_results()
        .await
        .map_err(|e| map_error(e, Phase::Connect, Some(SESSION_SETUP)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::sqlserver::decode::{Bytes, decode_row};
    use crate::drivers::sqlserver::testing::{self, Cell, Reply, Request, Ty, result};
    use crate::types::profile::Driver;
    use crate::types::value::Value;

    fn profile(ssl: SslMode) -> ConnectionProfile {
        let mut p: ConnectionProfile = serde_json::from_value(serde_json::json!({
            "id": "6f9619ff-8b86-d011-b42d-00c04fc964ff", "name": "ms", "color": null,
            "group": null, "driver": "sqlserver", "host": "db.example.com", "port": null,
            "user": "sa", "database": "shop", "sslCaPath": null, "sqlitePath": null, "ssh": null,
            "lastConnectedAt": null
        }))
        .expect("a profile");
        p.ssl = ssl;
        assert_eq!(p.driver, Driver::Sqlserver);
        p
    }

    fn endpoint() -> Endpoint {
        Endpoint {
            connect_host: "127.0.0.1".into(),
            connect_port: 50_000,
            server_host: "db.example.com".into(),
            server_port: 1433,
            port_given: false,
            tunnelled: true,
        }
    }

    #[test]
    fn ssl_modes_map_to_encryption_levels() {
        assert_eq!(
            encryption(SslMode::Disable, false),
            (EncryptionLevel::Off, false)
        );
        assert_eq!(
            encryption(SslMode::Prefer, false),
            (EncryptionLevel::On, false)
        );
        assert_eq!(
            encryption(SslMode::Require, false),
            (EncryptionLevel::Required, false)
        );
        assert_eq!(
            encryption(SslMode::VerifyCa, false),
            (EncryptionLevel::Required, true)
        );
        assert_eq!(
            encryption(SslMode::VerifyFull, false),
            (EncryptionLevel::Required, true)
        );
        assert_eq!(
            encryption(SslMode::Prefer, true),
            (EncryptionLevel::Strict, false)
        );
        assert_eq!(
            encryption(SslMode::VerifyFull, true),
            (EncryptionLevel::Strict, true)
        );
    }

    #[test]
    fn configs_build_without_panicking() {
        let timeout = Duration::from_secs(5);
        let params = MssqlParams {
            instance: Some("SQLEXPRESS".into()),
            ..MssqlParams::default()
        };
        // Every mode, with and without a CA file: the bypass and the CA never meet.
        for ssl in [
            SslMode::Disable,
            SslMode::Prefer,
            SslMode::Require,
            SslMode::VerifyCa,
            SslMode::VerifyFull,
        ] {
            let mut p = profile(ssl);
            p.ssl_ca_path = Some("ca.pem".into());
            let config = build_config(&p, &params, Some("pw"), &endpoint(), timeout).unwrap();
            assert!(format!("{config:?}").contains("db.example.com"));
            assert!(config.get_lossy_utf16_decoding());
        }
        // The login names the profile's database (Switch Database opens a session from the
        // profile with the other one), or none: the login's default.
        for (database, logs_in) in [
            (Some("shop"), r#"database: Some("shop")"#),
            (Some(" archive "), r#"database: Some("archive")"#),
            (Some("  "), "database: None"),
            (None, "database: None"),
        ] {
            let mut p = profile(SslMode::Prefer);
            p.database = database.map(str::to_string);
            let config = build_config(&p, &params, Some("pw"), &endpoint(), timeout).unwrap();
            assert!(format!("{config:?}").contains(logs_in), "{database:?}");
        }
        // Over a tunnel the instance is not looked up: the port is the server's.
        assert_eq!(named_instance(&params, &endpoint()), None);
        let direct = Endpoint {
            tunnelled: false,
            ..endpoint()
        };
        assert_eq!(named_instance(&params, &direct), Some("SQLEXPRESS"));
        let given = Endpoint {
            port_given: true,
            ..direct
        };
        assert_eq!(named_instance(&params, &given), None);
    }

    #[test]
    fn logins_need_what_their_method_needs() {
        let p = profile(SslMode::Prefer);
        let timeout = Duration::from_secs(5);
        let mut no_user = p.clone();
        no_user.user = Some("  ".into());
        assert!(matches!(
            build_config(
                &no_user,
                &MssqlParams::default(),
                Some("pw"),
                &endpoint(),
                timeout
            ),
            Err(AppError::InvalidInput(_))
        ));
        let entra = MssqlParams {
            auth: MssqlAuth::EntraToken,
            ..MssqlParams::default()
        };
        assert!(build_config(&no_user, &entra, Some("eyJ0"), &endpoint(), timeout).is_ok());
        assert!(build_config(&no_user, &entra, None, &endpoint(), timeout).is_err());
        let mut half_cert = p.clone();
        half_cert.options.ssl_cert_path = Some("client.pem".into());
        assert!(
            build_config(
                &half_cert,
                &MssqlParams::default(),
                None,
                &endpoint(),
                timeout
            )
            .is_err()
        );
        half_cert.options.ssl_key_path = Some("client.key".into());
        assert!(
            build_config(
                &half_cert,
                &MssqlParams::default(),
                None,
                &endpoint(),
                timeout
            )
            .is_ok()
        );
        let integrated = MssqlParams {
            auth: MssqlAuth::Integrated,
            ..MssqlParams::default()
        };
        assert_eq!(
            build_config(&no_user, &integrated, None, &endpoint(), timeout).is_ok(),
            cfg!(windows)
        );
    }

    #[test]
    fn endpoints_follow_the_tunnel_and_the_profile() {
        let p = profile(SslMode::Prefer);
        let direct = Endpoint::of(&ConnectTarget {
            profile: &p,
            password: None,
            host_override: None,
        });
        assert_eq!(
            direct,
            Endpoint {
                connect_host: "db.example.com".into(),
                connect_port: 1433,
                tunnelled: false,
                ..endpoint()
            }
        );
        let tunnel = Endpoint::of(&ConnectTarget {
            profile: &p,
            password: None,
            host_override: Some(("127.0.0.1".into(), 50_000)),
        });
        assert_eq!(tunnel, endpoint());
        let mut q = p.clone();
        q.port = Some(14330);
        q.options.connect_timeout_secs = Some(5);
        q.options.startup_sql = Some("  \n".into());
        let given = Endpoint::of(&ConnectTarget {
            profile: &q,
            password: None,
            host_override: None,
        });
        assert!(given.port_given && given.connect_port == 14330 && given.server_port == 14330);
        assert_eq!(connect_timeout(&q), Duration::from_secs(5));
        assert_eq!(connect_timeout(&p), Duration::from_secs(60));
        assert_eq!(startup_sql(&q), None);
    }

    /// An Entra token expires within the hour: a refused one is fetched again, once.
    #[tokio::test]
    async fn a_refused_token_is_fetched_again_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let fetched = Arc::new(AtomicUsize::new(0));
        let token = {
            let fetched = Arc::clone(&fetched);
            CommandPassword::with_fetch(Duration::from_secs(3000), move || {
                let n = fetched.fetch_add(1, Ordering::SeqCst) + 1;
                Box::pin(async move { Ok(format!("tok-{n}")) })
            })
            .seeded(Some("tok-0"))
        };
        let tried = std::sync::Mutex::new(Vec::new());
        let got = with_token(&token, |password: String| {
            tried.lock().unwrap().push(password.clone());
            async move {
                if password == "tok-0" {
                    Err(AppError::Auth("Login failed".into()))
                } else {
                    Ok(password)
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(got, "tok-1");
        assert_eq!(*tried.lock().unwrap(), ["tok-0", "tok-1"]);
        // Another failure is not the token's: the command does not run again.
        let down = with_token(&token, |_| async {
            Err::<(), _>(AppError::Connect("down".into()))
        })
        .await;
        assert!(matches!(down, Err(AppError::Connect(_))));
        // A second refusal is the answer.
        let refused = with_token(&token, |_| async {
            Err::<(), _>(AppError::Auth("Login failed".into()))
        })
        .await;
        assert!(matches!(refused, Err(AppError::Auth(_))));
        assert_eq!(fetched.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn ready_sets_the_session_up_then_runs_the_startup_commands() {
        let (mut client, seen) = testing::client(testing::ok).await.unwrap();
        ready(&mut client, Some("SET LANGUAGE us_english"))
            .await
            .unwrap();
        assert_eq!(
            seen.requests()[1..],
            [
                Request::Batch(SESSION_SETUP.into()),
                Request::Batch("SET LANGUAGE us_english".into())
            ]
        );
    }

    const UCS2: &str = "SELECT name FROM legacy";

    /// `a`, a lone high surrogate, `b`: UCS-2 data that is not UTF-16.
    fn ucs2(r: &Request) -> Reply {
        match r {
            Request::Batch(s) if s == UCS2 => Reply::Tokens(result(
                &[("name", Ty::NVarChar)],
                &[vec![Cell::Units(&[0x61, 0xD800, 0x62])]],
                false,
            )),
            _ => testing::ok(r),
        }
    }

    async fn read_ucs2<S>(client: &mut Client<S>) -> Result<Vec<::tiberius::Row>, Error>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send,
    {
        client.simple_query(UCS2).await?.into_first_result().await
    }

    #[tokio::test]
    async fn an_unpaired_surrogate_reads_as_a_replacement_character() {
        let (mut client, _) = testing::client(ucs2).await.unwrap();
        let mut rows = read_ucs2(&mut client).await.unwrap();
        let row = rows.pop().expect("a row");
        assert_eq!(
            decode_row(row, Bytes::Preview),
            [Value::Text("a\u{FFFD}b".into())]
        );
        // Strict decoding, tiberius's default, would fail the whole result.
        let (io, _) = testing::serve(ucs2);
        let mut strict = testing::config();
        strict.lossy_utf16_decoding(false);
        let mut client = Client::connect(strict, io).await.unwrap();
        assert!(read_ucs2(&mut client).await.is_err());
    }
}
