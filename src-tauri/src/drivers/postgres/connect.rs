//! Connection setup: `tokio_postgres::Config` from a profile, TLS (rustls), the deadpool worker
//! pool, the dedicated editor client and per-connection session initialisation.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use deadpool_postgres::{
    ClientWrapper, Hook, HookError, ManagerConfig, Metrics, Pool, RecyclingMethod,
};
use rustls::client::WantsClientCert;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, ConfigBuilder, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::{Client, Config};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::drivers::ConnectTarget;
use crate::error::AppError;
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
pub struct ClientIdentity {
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

/// Loads the profile's SSL key / cert pair (PEM). Both empty: no client certificate; only one of
/// them: an error (the server could not use it).
pub fn client_identity(
    key_path: Option<&str>,
    cert_path: Option<&str>,
) -> Result<Option<ClientIdentity>, AppError> {
    use rustls::pki_types::pem::PemObject as _;
    let key_path = key_path.map(str::trim).filter(|p| !p.is_empty());
    let cert_path = cert_path.map(str::trim).filter(|p| !p.is_empty());
    let (key_path, cert_path) = match (key_path, cert_path) {
        (None, None) => return Ok(None),
        (Some(k), Some(c)) => (k, c),
        (Some(_), None) => {
            return Err(AppError::InvalidInput(
                "SSL: choose the client certificate (Cert…) that goes with the key".into(),
            ));
        }
        (None, Some(_)) => {
            return Err(AppError::InvalidInput(
                "SSL: choose the private key (Key…) of the client certificate".into(),
            ));
        }
    };
    let read = |path: &str| {
        std::fs::read(crate::tunnel::expand_tilde(path))
            .map_err(|e| AppError::Connect(format!("cannot read {path}: {e}")))
    };
    let pem = read(cert_path)?;
    let certs = CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Connect(format!("invalid PEM in {cert_path}: {e:?}")))?;
    if certs.is_empty() {
        return Err(AppError::Connect(format!(
            "no certificate found in {cert_path}"
        )));
    }
    let key = PrivateKeyDer::from_pem_slice(&read(key_path)?).map_err(|e| {
        AppError::Connect(format!(
            "no usable private key in {key_path} (PEM, unencrypted): {e:?}"
        ))
    })?;
    Ok(Some(ClientIdentity { certs, key }))
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

async fn connect_once(
    config: &Config,
    tls: &PgTls,
    timeout: Duration,
) -> Result<(Client, EditorConnection), ConnectFailure> {
    match tokio::time::timeout(timeout, config.connect(tls.clone())).await {
        Ok(Ok(pair)) => Ok(pair),
        Ok(Err(e)) if is_tls_handshake_error(&e) => {
            Err(ConnectFailure::TlsHandshake(AppError::from(e)))
        }
        Ok(Err(e)) => Err(ConnectFailure::Other(AppError::from(e))),
        Err(_) => Err(ConnectFailure::Other(AppError::Timeout)),
    }
}

/// Server notices (`RAISE NOTICE`, warnings…) received on the editor connection, oldest first.
pub type NoticeBuffer = Arc<std::sync::Mutex<Vec<String>>>;

/// Notices kept between two statements; older ones are dropped.
const MAX_NOTICES: usize = 1000;

fn format_notice(n: &tokio_postgres::error::DbError) -> String {
    let mut text = format!("{}: {}", n.severity(), n.message());
    if let Some(detail) = n.detail() {
        text.push_str("\nDETAIL: ");
        text.push_str(detail);
    }
    if let Some(hint) = n.hint() {
        text.push_str("\nHINT: ");
        text.push_str(hint);
    }
    text
}

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
                Some(Ok(tokio_postgres::AsyncMessage::Notice(notice))) => {
                    let mut buf = buffer.lock().unwrap_or_else(|p| p.into_inner());
                    if buf.len() >= MAX_NOTICES {
                        buf.remove(0);
                    }
                    buf.push(format_notice(&notice));
                }
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
/// handshake failure is retried without TLS (as libpq does). Returns the config actually used so
/// the pool shares the same transport.
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
    let (client, connection) = match connect_once(&config, tls, timeout).await {
        Ok(pair) => pair,
        Err(ConnectFailure::TlsHandshake(e)) if prefer => {
            log::info!(
                "postgres: TLS handshake failed with sslmode=prefer ({e}); retrying without TLS"
            );
            config.ssl_mode(tokio_postgres::config::SslMode::Disable);
            connect_once(&config, tls, timeout).await?
        }
        Err(e) => return Err(e.into()),
    };
    let (lost, task, notices) = spawn_connection(connection);
    client.batch_execute(init_sql).await?;
    Ok((config, client, lost, task, notices))
}

/// Builds the worker pool; every new connection runs the session init batch.
pub fn build_pool(config: Config, tls: PgTls, init_sql: String) -> Result<Pool, AppError> {
    let manager = deadpool_postgres::Manager::from_config(
        config,
        tls,
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
