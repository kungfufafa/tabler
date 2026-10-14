//! Connection setup: `tokio_postgres::Config` from a profile, TLS (rustls), the deadpool worker
//! pool, the dedicated editor client and per-connection session initialisation.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use deadpool_postgres::{
    ClientWrapper, Hook, HookError, ManagerConfig, Metrics, Pool, RecyclingMethod,
};
use futures::future::BoxFuture;
use rustls::client::WantsClientCert;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ConfigBuilder, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::{Client, Config};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::drivers::ConnectTarget;
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::types::profile::{ProfileOptions, SslMode};

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// TLS connector that pins SNI / certificate verification to the profile's real host name.
///
/// tokio-postgres hands `MakeTlsConnect::make_tls_connect` the host it is *connecting* to. When
/// the connection goes through an SSH tunnel that host is `127.0.0.1` (the tunnel listener), so
/// `MakeRustlsConnect` would send the wrong SNI and `verify-full` would fail the name check.
/// This wrapper ignores the passed host and always uses `ConnectTarget::tls_host()`, which keeps
/// the plain `Config::connect` path (no `connect_raw` needed) while every mode ends up with the
/// right name.
#[derive(Clone)]
pub struct PgTls {
    inner: MakeRustlsConnect,
    sni_host: String,
}

impl<S> MakeTlsConnect<S> for PgTls
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Stream = <MakeRustlsConnect as MakeTlsConnect<S>>::Stream;
    type TlsConnect = <MakeRustlsConnect as MakeTlsConnect<S>>::TlsConnect;
    type Error = <MakeRustlsConnect as MakeTlsConnect<S>>::Error;

    fn make_tls_connect(&mut self, _hostname: &str) -> Result<Self::TlsConnect, Self::Error> {
        <MakeRustlsConnect as MakeTlsConnect<S>>::make_tls_connect(&mut self.inner, &self.sni_host)
    }
}

/// Accepts any server certificate (`prefer` / `require`: encryption without authentication).
#[derive(Debug)]
struct NoVerify;

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Verifies the certificate chain against the trust roots but skips the host-name check
/// (`verify-ca`).
#[derive(Debug)]
struct VerifyChainOnly {
    roots: RootCertStore,
}

impl ServerCertVerifier for VerifyChainOnly {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let cert = rustls::server::ParsedCertificate::try_from(end_entity)?;
        rustls::client::verify_server_cert_signed_by_trust_anchor(
            &cert,
            &self.roots,
            intermediates,
            now,
            provider().signature_verification_algorithms.all,
        )?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The process-wide crypto provider (installed by `lib.rs::run` / the tests).
fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
}

/// Native roots plus the profile's extra CA bundle, if any.
fn root_store(ca_path: Option<&str>) -> Result<RootCertStore, AppError> {
    use rustls::pki_types::pem::PemObject as _;
    let mut roots = RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for err in &native.errors {
        log::warn!("native certificate store: {err}");
    }
    roots.add_parsable_certificates(native.certs);
    if let Some(path) = ca_path.filter(|p| !p.trim().is_empty()) {
        let pem = std::fs::read(path)
            .map_err(|e| AppError::Connect(format!("cannot read CA file {path}: {e}")))?;
        let mut added = 0usize;
        for cert in CertificateDer::pem_slice_iter(&pem) {
            let cert =
                cert.map_err(|e| AppError::Connect(format!("invalid PEM in {path}: {e:?}")))?;
            roots
                .add(cert)
                .map_err(|e| AppError::Connect(format!("invalid certificate in {path}: {e}")))?;
            added += 1;
        }
        if added == 0 {
            return Err(AppError::Connect(format!(
                "no certificates found in CA file {path}"
            )));
        }
    }
    if roots.is_empty() {
        return Err(AppError::Connect(
            "no trusted root certificates available".into(),
        ));
    }
    Ok(roots)
}

/// A TLS client certificate (chain) and its private key, for servers that authenticate clients.
pub type ClientIdentity = crate::tls_files::ClientCertificate;

/// Loads the profile's SSL key / cert pair (PEM; see `tls_files`). Both empty: no client
/// certificate; only one of them: an error (the server could not use it).
pub fn client_identity(
    key_path: Option<&str>,
    cert_path: Option<&str>,
) -> Result<Option<ClientIdentity>, AppError> {
    crate::tls_files::load_client_certificate(key_path, cert_path)
}

fn with_identity(
    builder: ConfigBuilder<ClientConfig, WantsClientCert>,
    identity: Option<ClientIdentity>,
) -> Result<ClientConfig, AppError> {
    match identity {
        None => Ok(builder.with_no_client_auth()),
        Some(ClientIdentity { certs, key }) => builder
            .with_client_auth_cert(certs, key)
            .map_err(|e| AppError::Connect(format!("SSL client certificate: {e}"))),
    }
}

fn client_config(
    mode: SslMode,
    ca_path: Option<&str>,
    identity: Option<ClientIdentity>,
) -> Result<ClientConfig, AppError> {
    let builder = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| AppError::Connect(format!("tls: {e}")))?;
    let builder = match mode {
        SslMode::Disable | SslMode::Prefer | SslMode::Require => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify)),
        SslMode::VerifyCa => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(VerifyChainOnly {
                roots: root_store(ca_path)?,
            })),
        SslMode::VerifyFull => builder.with_root_certificates(root_store(ca_path)?),
    };
    with_identity(builder, identity)
}

/// Builds the TLS connector for a target. `Disable` still gets a (never used) connector so the
/// pool and the editor share one concrete type; its client certificate is not loaded.
pub fn make_tls(target: &ConnectTarget<'_>) -> Result<PgTls, AppError> {
    let profile = target.profile;
    let identity = if profile.ssl == SslMode::Disable {
        None
    } else {
        client_identity(
            profile.options.ssl_key_path.as_deref(),
            profile.options.ssl_cert_path.as_deref(),
        )?
    };
    let config = client_config(profile.ssl, profile.ssl_ca_path.as_deref(), identity)?;
    Ok(PgTls {
        inner: MakeRustlsConnect::new(config),
        sni_host: target.tls_host(),
    })
}

/// Translates a profile into a `tokio_postgres::Config`.
pub fn make_config(target: &ConnectTarget<'_>) -> Config {
    let profile = target.profile;
    let mut config = Config::new();
    config.host(target.host());
    config.port(target.port());
    if let Some(user) = profile.user.as_deref().filter(|u| !u.is_empty()) {
        config.user(user);
    }
    if let Some(password) = target.password {
        config.password(password);
    }
    config.dbname(
        profile
            .database
            .as_deref()
            .filter(|d| !d.is_empty())
            .unwrap_or("postgres"),
    );
    config.application_name("tabler");
    config.connect_timeout(connect_timeout(&profile.options));
    config.ssl_mode(match profile.ssl {
        SslMode::Disable => tokio_postgres::config::SslMode::Disable,
        SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
        SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => {
            tokio_postgres::config::SslMode::Require
        }
    });
    config
}

pub fn connect_timeout(options: &ProfileOptions) -> Duration {
    options
        .connect_timeout_secs
        .filter(|s| *s > 0)
        .map(|s| Duration::from_secs(s as u64))
        .unwrap_or(DEFAULT_CONNECT_TIMEOUT)
}

/// The statements run on every new server connection (pool and editor alike).
pub fn session_init_sql(options: &ProfileOptions) -> String {
    let mut sql = String::from(
        "SET DateStyle = 'ISO, YMD'; SET IntervalStyle = 'postgres'; \
         SET extra_float_digits = 3; SET client_encoding = 'UTF8';",
    );
    if let Some(tz) = options.timezone.as_deref().filter(|t| !t.trim().is_empty()) {
        sql.push_str(&format!(
            " SET TIME ZONE '{}';",
            tz.trim().replace('\'', "''")
        ));
    }
    if options.read_only {
        sql.push_str(" SET default_transaction_read_only = on;");
    }
    if let Some(extra) = options
        .startup_sql
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        sql.push(' ');
        sql.push_str(extra.trim());
        if !extra.trim_end().ends_with(';') {
            sql.push(';');
        }
    }
    sql
}

fn is_tls_handshake_error(err: &tokio_postgres::Error) -> bool {
    err.as_db_error().is_none()
        && err
            .to_string()
            .starts_with("error performing TLS handshake")
}

type EditorConnection = tokio_postgres::Connection<
    tokio_postgres::Socket,
    <PgTls as MakeTlsConnect<tokio_postgres::Socket>>::Stream,
>;

/// Why a connection attempt failed; `TlsHandshake` is the only case `prefer` may retry.
enum ConnectFailure {
    TlsHandshake(AppError),
    Other(AppError),
}

impl From<ConnectFailure> for AppError {
    fn from(f: ConnectFailure) -> AppError {
        match f {
            ConnectFailure::TlsHandshake(e) | ConnectFailure::Other(e) => e,
        }
    }
}

/// How telling a failed attempt is: the server's own answer (a FATAL from pg_hba, the
/// certificate check, a wrong password…) over a TLS failure over not reaching the server.
fn specificity(err: &tokio_postgres::Error) -> u8 {
    if err.as_db_error().is_some() {
        3
    } else if is_tls_handshake_error(err) {
        2
    } else if err.to_string().starts_with("error connecting to server") {
        0
    } else {
        1
    }
}

/// The addresses to try one at a time: those of a host name that resolves to more than one
/// (`localhost` → `::1`, `127.0.0.1`). tokio-postgres tries them itself but reports only the last
/// error, so `::1`'s "connection refused" would hide the FATAL the server sent on `127.0.0.1`.
/// Empty: connect as configured (an IP, a socket, one address, or a name that does not resolve,
/// which tokio-postgres reports itself).
async fn addresses_to_try(config: &Config) -> Vec<std::net::IpAddr> {
    let ([tokio_postgres::config::Host::Tcp(host)], []) =
        (config.get_hosts(), config.get_hostaddrs())
    else {
        return Vec::new();
    };
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Vec::new();
    }
    let port = config.get_ports().first().copied().unwrap_or(5432);
    let Ok(resolved) = tokio::net::lookup_host((host.as_str(), port)).await else {
        return Vec::new();
    };
    let mut ips = Vec::new();
    for addr in resolved {
        if !ips.contains(&addr.ip()) {
            ips.push(addr.ip());
        }
    }
    if ips.len() > 1 { ips } else { Vec::new() }
}

fn failure(err: tokio_postgres::Error) -> ConnectFailure {
    if is_tls_handshake_error(&err) {
        ConnectFailure::TlsHandshake(AppError::from(err))
    } else {
        ConnectFailure::Other(AppError::from(err))
    }
}

/// Connects with `config`. A host name with several addresses is tried address by address
/// (`hostaddr`, the name still used for TLS), reporting the most telling error; the address that
/// worked is pinned in `config` (the editor's cancel requests go there; the pool resolves the
/// name itself, see [`pool_config`]).
async fn connect_once(
    config: &mut Config,
    tls: &PgTls,
    timeout: Duration,
) -> Result<(Client, EditorConnection), ConnectFailure> {
    let addrs = addresses_to_try(config).await;
    if addrs.is_empty() {
        return match tokio::time::timeout(timeout, config.connect(tls.clone())).await {
            Ok(Ok(pair)) => Ok(pair),
            Ok(Err(e)) => Err(failure(e)),
            Err(_) => Err(ConnectFailure::Other(AppError::Timeout)),
        };
    }
    let deadline = tokio::time::Instant::now() + timeout;
    let mut best: Option<tokio_postgres::Error> = None;
    for ip in addrs {
        let mut one = config.clone();
        one.hostaddr(ip);
        match tokio::time::timeout_at(deadline, one.connect(tls.clone())).await {
            Ok(Ok(pair)) => {
                *config = one;
                return Ok(pair);
            }
            Ok(Err(e)) => {
                log::debug!("postgres: {ip}: {}", crate::error::error_chain(&e));
                if best
                    .as_ref()
                    .is_none_or(|b| specificity(&e) > specificity(b))
                {
                    best = Some(e);
                }
            }
            Err(_) => break,
        }
    }
    match best {
        Some(e) => Err(failure(e)),
        None => Err(ConnectFailure::Other(AppError::Timeout)),
    }
}

/// Server notices (`RAISE NOTICE`, warnings…) received on the editor connection (see
/// `editor::NoticeHub`: the running statement forwards them as they arrive).
pub type NoticeBuffer = Arc<super::editor::NoticeHub>;

/// Spawns the connection task; `lost` flips to `true` once it ends (error or orderly close).
/// Notices are collected into the returned buffer. The abort handle ends the task (and closes
/// the socket) without waiting for the client.
fn spawn_connection(
    mut connection: EditorConnection,
) -> (Arc<AtomicBool>, tokio::task::AbortHandle, NoticeBuffer) {
    use futures::StreamExt as _;
    let lost = Arc::new(AtomicBool::new(false));
    let notices = NoticeBuffer::default();
    let flag = lost.clone();
    let buffer = notices.clone();
    // Flags the connection as lost however the task ends (also when it is aborted).
    struct LostOnDrop(Arc<AtomicBool>);
    impl Drop for LostOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let task = tokio::spawn(async move {
        let _lost = LostOnDrop(flag);
        let mut messages = futures::stream::poll_fn(move |cx| connection.poll_message(cx));
        loop {
            match messages.next().await {
                Some(Ok(tokio_postgres::AsyncMessage::Notice(notice))) => buffer.push(&notice),
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    log::warn!("postgres editor connection ended: {e}");
                    break;
                }
                None => break,
            }
        }
    });
    (lost, task.abort_handle(), notices)
}

/// Opens the dedicated editor client and runs the session init on it. With `prefer`, a TLS
/// handshake failure is retried without TLS (as libpq does). Returns the config actually used
/// (pinned to the address that worked) so the pool shares the same transport (see
/// [`pool_config`]) and cancel requests reach the editor's server.
pub async fn open_editor(
    mut config: Config,
    tls: &PgTls,
    init_sql: &str,
    timeout: Duration,
    prefer: bool,
) -> Result<
    (
        Config,
        Client,
        Arc<AtomicBool>,
        tokio::task::AbortHandle,
        NoticeBuffer,
    ),
    AppError,
> {
    let (client, connection) = match connect_once(&mut config, tls, timeout).await {
        Ok(pair) => pair,
        Err(ConnectFailure::TlsHandshake(e)) if prefer => {
            log::info!(
                "postgres: TLS handshake failed with sslmode=prefer ({e}); retrying without TLS"
            );
            config.ssl_mode(tokio_postgres::config::SslMode::Disable);
            connect_once(&mut config, tls, timeout).await?
        }
        Err(e) => return Err(e.into()),
    };
    let (lost, task, notices) = spawn_connection(connection);
    client.batch_execute(init_sql).await?;
    Ok((config, client, lost, task, notices))
}

/// The worker pool's connect settings: the profile's (`original`) with the transport the editor
/// ended up using (`prefer` may have fallen back to no TLS), but not the address the editor was
/// pinned to: the pool resolves the host name for every new connection, so a load balancer or a
/// failover that moves the server to another address does not break every later pool connection.
pub fn pool_config(original: &Config, editor: &Config) -> Config {
    let mut config = original.clone();
    config.ssl_mode(editor.get_ssl_mode());
    config
}

/// Opens the pool's connections. With a password command ("Command Line (IAM authentication)")
/// each new connection asks `token` for the password, which runs the command again once its last
/// output is older than the TTL (IAM tokens expire; open connections stay as they are).
struct PoolConnect {
    tls: PgTls,
    token: Option<Arc<CommandPassword>>,
}

impl deadpool_postgres::Connect for PoolConnect {
    fn connect(
        &self,
        pg_config: &Config,
    ) -> BoxFuture<'_, Result<(Client, tokio::task::JoinHandle<()>), tokio_postgres::Error>> {
        let mut config = pg_config.clone();
        let tls = self.tls.clone();
        Box::pin(async move {
            if let Some(token) = &self.token {
                match token.password().await {
                    Ok(password) => {
                        config.password(password);
                    }
                    // `PgSession::client` reports it if the connection fails; until then the
                    // previous token may still be accepted.
                    Err(e) => log::warn!("postgres: the password command failed: {e}"),
                }
            }
            let (client, connection) = config.connect(tls).await?;
            let task = tokio::spawn(async move {
                if let Err(e) = connection.await {
                    log::debug!("postgres: a pool connection ended: {e}");
                }
            });
            Ok((client, task))
        })
    }
}

/// Builds the worker pool; every new connection runs the session init batch. `token`: the
/// password source of a profile whose password comes from a command (see [`PoolConnect`]).
pub fn build_pool(
    config: Config,
    tls: PgTls,
    init_sql: String,
    token: Option<Arc<CommandPassword>>,
) -> Result<Pool, AppError> {
    let manager = deadpool_postgres::Manager::from_connect(
        config,
        PoolConnect { tls, token },
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    let init: Arc<str> = Arc::from(init_sql);
    Pool::builder(manager)
        .max_size(4)
        .post_create(Hook::async_fn(
            move |client: &mut ClientWrapper, _metrics: &Metrics| {
                let init = init.clone();
                Box::pin(async move {
                    client
                        .batch_execute(&init)
                        .await
                        .map_err(HookError::Backend)
                })
            },
        ))
        .build()
        .map_err(|e| AppError::Internal(format!("postgres pool: {e}")))
}

/// Maps pool errors onto `AppError`.
pub fn pool_error(err: deadpool_postgres::PoolError) -> AppError {
    use deadpool_postgres::PoolError as P;
    match err {
        P::Backend(e) => AppError::from(e),
        P::PostCreateHook(HookError::Backend(e)) => AppError::from(e),
        P::PostCreateHook(HookError::Message(m)) => AppError::Connect(m.into_owned()),
        P::Closed => AppError::NotConnected,
        P::Timeout(_) => AppError::Timeout,
        P::NoRuntimeSpecified => AppError::Internal("postgres pool: no runtime".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::profile::{ConnectionProfile, Driver};

    fn profile() -> ConnectionProfile {
        ConnectionProfile {
            id: uuid::Uuid::nil(),
            name: "t".into(),
            color: None,
            group: None,
            driver: Driver::Postgres,
            host: Some("db.example.com".into()),
            port: Some(5433),
            user: Some("alice".into()),
            database: None,
            ssl: SslMode::Prefer,
            ssl_ca_path: None,
            sqlite_path: None,
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    #[test]
    fn builds_config_from_profile_and_tunnel_override() {
        let p = profile();
        let target = ConnectTarget {
            profile: &p,
            password: Some("pw"),
            host_override: Some(("127.0.0.1".into(), 50000)),
        };
        let cfg = make_config(&target);
        assert_eq!(cfg.get_ports(), &[50000]);
        assert!(matches!(
            cfg.get_hosts()[0],
            tokio_postgres::config::Host::Tcp(ref h) if h == "127.0.0.1"
        ));
        assert_eq!(cfg.get_user(), Some("alice"));
        assert_eq!(cfg.get_password(), Some(&b"pw"[..]));
        assert_eq!(cfg.get_dbname(), Some("postgres"));
        assert_eq!(cfg.get_application_name(), Some("tabler"));
        assert_eq!(cfg.get_connect_timeout(), Some(&DEFAULT_CONNECT_TIMEOUT));
        assert_eq!(cfg.get_ssl_mode(), tokio_postgres::config::SslMode::Prefer);
        // SNI stays on the real host even though we connect to the tunnel.
        assert_eq!(target.tls_host(), "db.example.com");
    }

    #[test]
    fn session_init_includes_options() {
        let mut opts = ProfileOptions::default();
        assert!(session_init_sql(&opts).starts_with("SET DateStyle = 'ISO, YMD';"));
        opts.timezone = Some("Asia/Jakarta".into());
        opts.read_only = true;
        opts.startup_sql = Some("SET search_path = app".into());
        let sql = session_init_sql(&opts);
        assert!(sql.contains("SET TIME ZONE 'Asia/Jakarta';"));
        assert!(sql.contains("SET default_transaction_read_only = on;"));
        assert!(sql.ends_with("SET search_path = app;"));
        opts.timezone = Some("it's".into());
        assert!(session_init_sql(&opts).contains("SET TIME ZONE 'it''s';"));
    }

    #[test]
    fn tls_configs_build_for_every_mode() {
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok();
        for mode in [SslMode::Disable, SslMode::Prefer, SslMode::Require] {
            assert!(client_config(mode, None, None).is_ok());
        }
        // Verify modes need roots; the native store normally has some.
        let _ = client_config(SslMode::VerifyFull, None, None);
        assert!(client_config(SslMode::VerifyCa, Some("/nonexistent/ca.pem"), None).is_err());
    }

    /// A "server" on `[::1]` only that answers every startup with a pg_hba FATAL; `127.0.0.1`
    /// refuses. With `localhost`, the FATAL must be what is reported, whatever the address order.
    #[tokio::test]
    async fn the_servers_fatal_wins_over_another_address_refusing() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let Ok(listener) = tokio::net::TcpListener::bind("[::1]:0").await else {
            return; // no IPv6 loopback here
        };
        let port = listener.local_addr().unwrap().port();
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
            return; // the same port is taken on IPv4: cannot set the scene up
        }
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 512];
                let _ = socket.read(&mut buf).await;
                let mut fields = Vec::new();
                for (code, value) in [
                    (b'S', "FATAL"),
                    (b'V', "FATAL"),
                    (b'C', "28000"),
                    (
                        b'M',
                        "no pg_hba.conf entry for host \"::1\", user \"u\", database \"d\", no encryption",
                    ),
                ] {
                    fields.push(code);
                    fields.extend_from_slice(value.as_bytes());
                    fields.push(0);
                }
                fields.push(0);
                let mut msg = vec![b'E'];
                msg.extend_from_slice(&(fields.len() as i32 + 4).to_be_bytes());
                msg.extend_from_slice(&fields);
                let _ = socket.write_all(&msg).await;
            }
        });
        let mut p = profile();
        p.host = Some("localhost".into());
        p.port = Some(port);
        p.user = Some("u".into());
        p.database = Some("d".into());
        p.ssl = SslMode::Disable;
        let target = ConnectTarget {
            profile: &p,
            password: None,
            host_override: None,
        };
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok();
        let tls = make_tls(&target).unwrap();
        let mut config = make_config(&target);
        let err = match connect_once(&mut config, &tls, Duration::from_secs(5)).await {
            Ok(_) => panic!("the fake server never accepts"),
            Err(f) => AppError::from(f),
        };
        assert!(err.to_string().contains("no encryption"), "{err}");
    }

    /// What [`fake_pg`] saw: the passwords of the connections it accepted, and the cancel
    /// requests (`(pid, secret key)`) it received.
    #[derive(Clone, Default)]
    struct FakePg {
        passwords: Arc<std::sync::Mutex<Vec<String>>>,
        cancels: Arc<std::sync::Mutex<Vec<(i32, i32)>>>,
    }

    /// A minimal PostgreSQL server on 127.0.0.1: asks every connection for a cleartext password,
    /// accepts it, answers every simple query with an empty result and records cancel requests.
    async fn fake_pg() -> (u16, FakePg) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = FakePg::default();
        let state = seen.clone();
        tokio::spawn(async move {
            let mut pid = 100;
            while let Ok((socket, _)) = listener.accept().await {
                pid += 1;
                tokio::spawn(serve_fake_pg(socket, state.clone(), pid));
            }
        });
        (port, seen)
    }

    async fn serve_fake_pg(
        mut socket: tokio::net::TcpStream,
        seen: FakePg,
        pid: i32,
    ) -> std::io::Result<()> {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        async fn body(socket: &mut tokio::net::TcpStream) -> std::io::Result<Vec<u8>> {
            let len = socket.read_i32().await?;
            let mut body = vec![0u8; usize::try_from(len - 4).unwrap_or(0)];
            socket.read_exact(&mut body).await?;
            Ok(body)
        }
        let int = |b: &[u8]| i32::from_be_bytes(b[..4].try_into().unwrap());
        loop {
            let startup = body(&mut socket).await?;
            match int(&startup) {
                // CancelRequest: record it; closing the socket tells the client it was handled.
                80_877_102 => {
                    seen.cancels
                        .lock()
                        .unwrap()
                        .push((int(&startup[4..]), int(&startup[8..])));
                    return Ok(());
                }
                // SSLRequest / GSSENCRequest: not supported.
                80_877_103 | 80_877_104 => socket.write_all(b"N").await?,
                _ => break,
            }
        }
        // AuthenticationCleartextPassword, then the PasswordMessage.
        socket.write_all(&[b'R', 0, 0, 0, 8, 0, 0, 0, 3]).await?;
        let _tag = socket.read_u8().await?;
        let password = body(&mut socket).await?;
        let password = password.strip_suffix(&[0]).unwrap_or(&password);
        seen.passwords
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(password).into_owned());
        // AuthenticationOk, BackendKeyData, ReadyForQuery.
        let mut out = vec![b'R', 0, 0, 0, 8, 0, 0, 0, 0, b'K', 0, 0, 0, 12];
        out.extend(pid.to_be_bytes());
        out.extend(42i32.to_be_bytes());
        out.extend([b'Z', 0, 0, 0, 5, b'I']);
        socket.write_all(&out).await?;
        loop {
            let tag = socket.read_u8().await?;
            let _ = body(&mut socket).await?;
            match tag {
                b'X' => return Ok(()),
                // EmptyQueryResponse, ReadyForQuery.
                b'Q' => {
                    socket
                        .write_all(&[b'I', 0, 0, 0, 4, b'Z', 0, 0, 0, 5, b'I'])
                        .await?
                }
                _ => {}
            }
        }
    }

    fn fake_target(p: &mut ConnectionProfile, port: u16) {
        p.host = Some("127.0.0.1".into());
        p.port = Some(port);
        p.ssl = SslMode::Disable;
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok();
    }

    /// Regression: the password of a "Command Line (IAM authentication)" profile was frozen into
    /// the pool's config, so pool connections opened after the token expired failed to log in.
    #[tokio::test]
    async fn pool_connections_get_a_fresh_password_once_it_is_old() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (port, seen) = fake_pg().await;
        let mut p = profile();
        fake_target(&mut p, port);
        let target = ConnectTarget {
            profile: &p,
            password: Some("tok-0"),
            host_override: None,
        };
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = runs.clone();
        let token = CommandPassword::with_fetch(Duration::from_millis(300), move || {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move { Ok(format!("tok-{n}")) })
        })
        .seeded(Some("tok-0"));
        let pool = build_pool(
            make_config(&target),
            make_tls(&target).unwrap(),
            "SET x = 1".into(),
            Some(Arc::new(token)),
        )
        .unwrap();
        let first = pool.get().await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        // `first` is checked out: a new connection, with the command run again.
        let second = pool.get().await.unwrap();
        drop(first);
        // An idle connection is reused as it is.
        let third = pool.get().await.unwrap();
        assert_eq!(*seen.passwords.lock().unwrap(), ["tok-0", "tok-1"]);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        drop((second, third));

        // Without a password command the profile's password is used throughout.
        let plain = build_pool(
            make_config(&target),
            make_tls(&target).unwrap(),
            "SET x = 1".into(),
            None,
        )
        .unwrap();
        let _a = plain.get().await.unwrap();
        let _b = plain.get().await.unwrap();
        assert_eq!(
            *seen.passwords.lock().unwrap(),
            ["tok-0", "tok-1", "tok-0", "tok-0"]
        );
    }

    #[test]
    fn the_pool_resolves_the_host_name_itself() {
        let p = profile();
        let target = ConnectTarget {
            profile: &p,
            password: None,
            host_override: None,
        };
        let original = make_config(&target);
        let mut editor = original.clone();
        editor.hostaddr("10.0.0.7".parse().unwrap());
        editor.ssl_mode(tokio_postgres::config::SslMode::Disable);
        let pool = pool_config(&original, &editor);
        assert!(pool.get_hostaddrs().is_empty());
        assert!(matches!(
            pool.get_hosts()[0],
            tokio_postgres::config::Host::Tcp(ref h) if h == "db.example.com"
        ));
        // The transport the editor ended up with (prefer → no TLS) is kept.
        assert_eq!(
            pool.get_ssl_mode(),
            tokio_postgres::config::SslMode::Disable
        );
    }

    /// Regression: cancel requests dialled the host name, which may lead to another server than
    /// the editor's (several addresses); they go to the editor's pinned address now, and to the
    /// name when that address cannot be reached.
    #[tokio::test]
    async fn cancel_requests_go_to_the_pinned_address() {
        let (port, seen) = fake_pg().await;
        let mut p = profile();
        fake_target(&mut p, port);
        let target = ConnectTarget {
            profile: &p,
            password: Some("pw"),
            host_override: None,
        };
        let tls = make_tls(&target).unwrap();
        // A name that does not resolve: only the pinned address leads to the server.
        let mut config = Config::new();
        config
            .host("tabler-pinned.invalid")
            .hostaddr("127.0.0.1".parse().unwrap())
            .port(port)
            .user("u")
            .password("pw")
            .dbname("d")
            .ssl_mode(tokio_postgres::config::SslMode::Disable);
        let (client, connection) = config.connect(tls.clone()).await.unwrap();
        tokio::spawn(connection);
        super::super::editor::cancel_and_wait(&client.cancel_token(), &tls, &config)
            .await
            .unwrap();
        assert_eq!(seen.cancels.lock().unwrap().len(), 1);

        // The pinned address is gone: the host name is tried.
        let mut moved = make_config(&target);
        moved.hostaddr("::1".parse().unwrap());
        if std::net::TcpListener::bind(("::1", port)).is_ok() {
            super::super::editor::cancel_and_wait(&client.cancel_token(), &tls, &moved)
                .await
                .unwrap();
            assert_eq!(seen.cancels.lock().unwrap().len(), 2);
        }
    }

    #[test]
    fn client_identity_needs_both_files() {
        assert!(client_identity(None, Some(" ")).unwrap().is_none());
        let err = client_identity(Some("/k.pem"), None).err().unwrap();
        assert_eq!(err.code(), "INVALID_INPUT");
        assert!(err.to_string().contains("Cert…"), "{err}");
        let err = client_identity(None, Some("/c.pem")).err().unwrap();
        assert!(err.to_string().contains("Key…"), "{err}");
        let err = client_identity(Some("/nonexistent/k.pem"), Some("/nonexistent/c.pem"))
            .err()
            .unwrap();
        assert!(err.to_string().contains("cannot read"), "{err}");

        // A file without PEM blocks is refused with its path.
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.pem");
        std::fs::write(&junk, "hello").unwrap();
        let junk = junk.to_string_lossy();
        let err = client_identity(Some(&junk), Some(&junk)).err().unwrap();
        assert!(err.to_string().contains("junk.pem"), "{err}");

        // Disabled SSL never loads (or fails on) the client files.
        let mut p = profile();
        p.ssl = SslMode::Disable;
        p.options.ssl_key_path = Some("/nonexistent/k.pem".into());
        p.options.ssl_cert_path = Some("/nonexistent/c.pem".into());
        let target = ConnectTarget {
            profile: &p,
            password: None,
            host_override: None,
        };
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok();
        assert!(make_tls(&target).is_ok());
        p.ssl = SslMode::Require;
        let target = ConnectTarget {
            profile: &p,
            password: None,
            host_override: None,
        };
        assert!(make_tls(&target).is_err());
    }
}
