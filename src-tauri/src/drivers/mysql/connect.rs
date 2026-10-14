//! Connection setup: `OptsBuilder` from the profile, TLS policy, "Use socket", connect timeout,
//! session initialisation, the dedicated editor connection and the worker pool.

use std::path::Path;
use std::time::Duration;

use mysql_async::prelude::Queryable;
use mysql_async::{
    ClientIdentity, Conn, DriverError, Error as MyError, IoError, Opts, OptsBuilder, Pool,
    PoolConstraints, PoolOpts, Row, SslOpts,
};

use super::decode::text_of;
use crate::drivers::ConnectTarget;
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, PasswordMode, SslMode};

const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 15;
/// Worker pool size (browsing + introspection); the editor connection is separate.
const POOL_MAX: usize = 4;
/// Idle pooled connections are recycled after this long.
const CONN_TTL: Duration = Duration::from_secs(600);
const TCP_KEEPALIVE: Duration = Duration::from_secs(60);
/// MySQL 8+: make `TABLE_ROWS` & co. fresh instead of up-to-a-day stale. Unknown on MariaDB.
const STATS_EXPIRY_SQL: &str = "SET SESSION information_schema_stats_expiry = 0";
/// Where "Use socket" looks when no socket path is set, in order: the macOS installer's (and
/// Homebrew's link), Debian / Ubuntu packages', Homebrew's data directory.
pub const SOCKET_CANDIDATES: [&str; 3] = [
    "/tmp/mysql.sock",
    "/var/run/mysqld/mysqld.sock",
    "/opt/homebrew/var/mysql/mysql.sock",
];

/// The socket file a "Use socket" profile connects through (`None` without it): the path typed in
/// the form, else the first of the usual places that exists (backups and restores use it too).
pub fn resolve_socket(profile: &ConnectionProfile) -> Result<Option<String>, AppError> {
    socket_path(
        &ConnectTarget {
            profile,
            password: None,
            host_override: None,
        },
        &SOCKET_CANDIDATES,
    )
}

/// Whether the password may go to the server in clear text (the `mysql_clear_password`
/// authentication that RDS / Aurora / Cloud SQL IAM authentication uses): only for a password
/// from a command ("Command Line (IAM authentication)"), and only over TLS or a local socket.
pub fn cleartext_password_allowed(profile: &ConnectionProfile, tls: bool, socket: bool) -> bool {
    profile.options.password_mode() == PasswordMode::Command && (tls || socket)
}

fn is_cleartext_refused(e: &MyError) -> bool {
    matches!(e, MyError::Driver(DriverError::CleartextPluginDisabled))
}

/// What a connect reports when the server asks for the password in clear text and the driver
/// refused to send it (see [`cleartext_password_allowed`]).
fn cleartext_refused(profile: &ConnectionProfile) -> AppError {
    AppError::Connect(
        if profile.options.password_mode() == PasswordMode::Command {
            "IAM authentication sends the token in clear text: turn on SSL".into()
        } else {
            "The server asks for the password in clear text (mysql_clear_password authentication), \
         which Tabler sends only for a Command Line (IAM authentication) password over SSL or a \
         socket"
                .into()
        },
    )
}

fn connect_error(profile: &ConnectionProfile, e: MyError) -> AppError {
    if is_cleartext_refused(&e) {
        cleartext_refused(profile)
    } else {
        e.into()
    }
}

/// The socket file of a "Use socket" profile (`None` without it): the path typed in the form,
/// else the first of `candidates` that exists. A tunnelled connection cannot use one.
fn socket_path(
    target: &ConnectTarget<'_>,
    candidates: &[&str],
) -> Result<Option<String>, AppError> {
    let profile = target.profile;
    if !profile.uses_socket() {
        return Ok(None);
    }
    if target.host_override.is_some() {
        return Err(AppError::InvalidInput(
            "A MySQL socket connection cannot go through an SSH tunnel yet".into(),
        ));
    }
    let typed = profile
        .options
        .socket_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());
    match typed {
        Some(path) => {
            let path = crate::tunnel::expand_tilde(path);
            if !path.exists() {
                return Err(AppError::Connect(format!(
                    "MySQL socket not found: {} (is the server running?)",
                    path.display()
                )));
            }
            Ok(Some(path.to_string_lossy().into_owned()))
        }
        None => candidates
            .iter()
            .find(|c| Path::new(c).exists())
            .map(|c| Some((*c).to_string()))
            .ok_or_else(|| {
                AppError::Connect(format!(
                    "No MySQL socket found at {}; enter the socket path (SELECT @@socket shows it)",
                    candidates.join(", ")
                ))
            }),
    }
}

pub(super) struct Opened {
    pub pool: Pool,
    /// Options the pool connects with (for one-off side connections, e.g. `KILL QUERY`).
    pub pool_opts: Opts,
    /// Dedicated SQL-editor connection (session state must persist between statements).
    pub editor: Conn,
    pub is_mariadb: bool,
}

/// Session statements run on every new connection (pool and editor).
fn setup_statements(profile: &ConnectionProfile) -> Vec<String> {
    let mut out = vec!["SET NAMES utf8mb4".to_string()];
    if let Some(tz) = profile
        .options
        .timezone
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        out.push(format!(
            "SET SESSION time_zone = '{}'",
            tz.replace('\\', "\\\\").replace('\'', "''")
        ));
    }
    if profile.options.read_only {
        // Standard syntax; equivalent to `transaction_read_only = 1` and accepted by MariaDB too.
        out.push("SET SESSION TRANSACTION READ ONLY".to_string());
    }
    if let Some(sql) = profile
        .options
        .startup_sql
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push(sql.to_string());
    }
    out
}

/// The profile's TLS client certificate (Key… / Cert…), parsed here (PEM only, the key
/// unencrypted PKCS#8 / PKCS#1 / SEC1, the client certificate first; see `tls_files`) so a wrong
/// file fails with an error naming it, then handed to the driver as PEM buffers. Both empty:
/// none; only one of them: an error.
fn client_identity(profile: &ConnectionProfile) -> Result<Option<ClientIdentity>, AppError> {
    let Some(cert) = crate::tls_files::load_client_certificate(
        profile.options.ssl_key_path.as_deref(),
        profile.options.ssl_cert_path.as_deref(),
    )?
    else {
        return Ok(None);
    };
    Ok(Some(ClientIdentity::new(
        cert.certs_pem().into(),
        cert.key_pem().into(),
    )))
}

/// Trusts the CA file (CA Cert…) plus the operating system's roots (the macOS Keychain, as for
/// PostgreSQL); the driver adds its bundled webpki roots itself.
fn with_verify_roots(opts: SslOpts, profile: &ConnectionProfile) -> SslOpts {
    let mut roots = Vec::new();
    if let Some(ca) = profile
        .ssl_ca_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        roots.push(crate::tunnel::expand_tilde(ca).into());
    }
    let native: String = crate::tls_files::native_roots()
        .iter()
        .map(|c| crate::tls_files::pem_block("CERTIFICATE", c))
        .collect();
    if !native.is_empty() {
        roots.push(native.into_bytes().into());
    }
    opts.with_root_certs(roots)
}

fn ssl_opts(target: &ConnectTarget<'_>) -> Result<Option<SslOpts>, AppError> {
    let profile = target.profile;
    let opts = match profile.ssl {
        SslMode::Disable => return Ok(None),
        // Encrypt without authenticating the server (self-signed certs are common).
        SslMode::Prefer | SslMode::Require => SslOpts::default()
            .with_danger_accept_invalid_certs(true)
            .with_danger_skip_domain_validation(true),
        // mysql_async 0.37.1 cannot skip the name check with rustls 0.23.45 (it looks for the
        // text "NotValidForName", which rustls no longer prints), so VERIFY-CA checks the name
        // too: against the profile host, as VERIFY-IDENTITY does (see `name_check_error`).
        SslMode::VerifyCa => with_verify_roots(SslOpts::default(), profile)
            .with_danger_skip_domain_validation(true)
            .with_danger_tls_hostname_override(Some(target.tls_host())),
        // Through an SSH tunnel the TCP host is 127.0.0.1; verify against the profile host.
        SslMode::VerifyFull => with_verify_roots(SslOpts::default(), profile)
            .with_danger_tls_hostname_override(Some(target.tls_host())),
    };
    Ok(Some(opts.with_client_identity(client_identity(profile)?)))
}

/// The server certificate is not valid for the host name (rustls' "certificate not valid for
/// name …").
fn is_name_mismatch(e: &MyError) -> bool {
    let text = crate::error::error_chain(e);
    text.contains("not valid for name") || text.contains("NotValidForName")
}

/// What VERIFY-CA reports when the certificate does not list the host: the driver checks the name
/// even in this mode (see `ssl_opts`), so say so and what to use instead.
fn name_check_error(host: &str, e: &MyError) -> AppError {
    let text = crate::error::error_chain(e);
    // The driver wraps the TLS error in "Input/output error: " twice: quote the TLS part.
    let detail = text
        .find("invalid peer certificate")
        .map_or(text.as_str(), |i| &text[i..]);
    AppError::Connect(format!(
        "VERIFY-CA: the server certificate is not valid for \u{201c}{host}\u{201d}, and the MySQL \
         driver cannot skip the host name check. Use VERIFY-IDENTITY with a host name the \
         certificate lists, or REQUIRED to encrypt without verifying the server ({detail})"
    ))
}

fn build_opts(
    target: &ConnectTarget<'_>,
    socket: Option<&str>,
    ssl: Option<SslOpts>,
    setup: Vec<String>,
) -> Opts {
    let profile = target.profile;
    let constraints = PoolConstraints::new(0, POOL_MAX).expect("0 <= POOL_MAX");
    // IAM authentication: the token goes in clear text, over the encrypted transport only.
    let cleartext = cleartext_password_allowed(profile, ssl.is_some(), socket.is_some());
    OptsBuilder::default()
        .ip_or_hostname(target.host())
        .tcp_port(target.port())
        // "Use socket": host and port are ignored.
        .socket(socket)
        .user(profile.user.clone().filter(|u| !u.is_empty()))
        .pass(target.password)
        .db_name(profile.database.clone().filter(|d| !d.is_empty()))
        // Never let the server redirect a localhost TCP connection to its Unix socket: the
        // address may be a tunnel listener whose socket path does not exist here.
        .prefer_socket(false)
        // UPDATE with identical values still reports the matched row as affected.
        .client_found_rows(true)
        .tcp_keepalive(Some(TCP_KEEPALIVE))
        .conn_ttl(Some(CONN_TTL))
        .pool_opts(PoolOpts::default().with_constraints(constraints))
        .setup(setup)
        .ssl_opts(ssl)
        .enable_cleartext_plugin(cleartext)
        .into()
}

fn is_tls_error(e: &MyError) -> bool {
    match e {
        MyError::Io(IoError::Tls(_)) => true,
        MyError::Driver(DriverError::NoClientSslFlagFromServer) => true,
        other => {
            let text = other.to_string().to_ascii_lowercase();
            text.contains("tls") || text.contains("ssl")
        }
    }
}

async fn connect_once(opts: Opts, timeout: Duration) -> Result<Result<Conn, MyError>, AppError> {
    tokio::time::timeout(timeout, Conn::new(opts))
        .await
        .map_err(|_| AppError::Timeout)
}

pub fn is_mariadb_version(version: &str) -> bool {
    version.to_ascii_lowercase().contains("mariadb")
}

/// `"MySQL 9.6.0"` / `"MariaDB 11.8.2"` from the raw `VERSION()` string.
pub fn display_version(raw: &str, is_mariadb: bool) -> String {
    let number = raw.split('-').next().unwrap_or(raw).trim();
    let number = if number.is_empty() {
        raw.trim()
    } else {
        number
    };
    format!("{} {number}", if is_mariadb { "MariaDB" } else { "MySQL" })
}

/// Opens the editor connection (validating credentials/TLS) and builds the worker pool.
pub(super) async fn open(target: &ConnectTarget<'_>) -> Result<Opened, AppError> {
    let profile = target.profile;
    let timeout = Duration::from_secs(
        profile
            .options
            .connect_timeout_secs
            .map(u64::from)
            .filter(|s| *s > 0)
            .unwrap_or(DEFAULT_CONNECT_TIMEOUT_SECS),
    );
    let mut setup = setup_statements(profile);
    let socket = socket_path(target, &SOCKET_CANDIDATES)?;
    let socket = socket.as_deref();
    // A local socket is a secure transport for the server (no TLS needed, and the driver cannot
    // run TLS over one), as for the mysql client.
    let mut ssl = match socket {
        Some(_) => None,
        None => ssl_opts(target)?,
    };

    let mut editor = match connect_once(
        build_opts(target, socket, ssl.clone(), setup.clone()),
        timeout,
    )
    .await?
    {
        Ok(conn) => conn,
        Err(e) if profile.ssl == SslMode::Prefer && ssl.is_some() && is_tls_error(&e) => {
            log::warn!("mysql: TLS handshake failed ({e}); retrying without TLS");
            ssl = None;
            connect_once(build_opts(target, socket, None, setup.clone()), timeout)
                .await?
                .map_err(|e| connect_error(profile, e))?
        }
        Err(e) if profile.ssl == SslMode::VerifyCa && is_name_mismatch(&e) => {
            return Err(name_check_error(&target.tls_host(), &e));
        }
        Err(e) => return Err(connect_error(profile, e)),
    };

    let version: Option<Row> = editor.query_first("SELECT VERSION()").await?;
    let version = version
        .and_then(|mut r| r.take::<mysql_async::Value, _>(0))
        .and_then(text_of)
        .unwrap_or_default();
    let is_mariadb = is_mariadb_version(&version);
    log::debug!("mysql: connected to {version} (conn id {})", editor.id());

    // Only pooled connections inherit the statement once it is known to work here.
    if !is_mariadb {
        match editor.query_drop(STATS_EXPIRY_SQL).await {
            Ok(()) => setup.push(STATS_EXPIRY_SQL.to_string()),
            Err(e) => log::debug!("mysql: {STATS_EXPIRY_SQL} rejected: {e}"),
        }
    }

    let pool_opts = build_opts(target, socket, ssl, setup);
    let pool = Pool::new(pool_opts.clone());
    Ok(Opened {
        pool,
        pool_opts,
        editor,
        is_mariadb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::profile::{Driver, ProfileOptions};

    fn profile() -> ConnectionProfile {
        ConnectionProfile {
            id: uuid::Uuid::nil(),
            name: "t".into(),
            color: None,
            group: None,
            driver: Driver::Mysql,
            host: Some("db.example.com".into()),
            port: Some(3307),
            user: Some("u".into()),
            database: Some("d".into()),
            ssl: SslMode::Disable,
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
    fn version_display() {
        assert_eq!(display_version("9.6.0", false), "MySQL 9.6.0");
        assert_eq!(
            display_version("11.8.2-MariaDB-ubu2404", true),
            "MariaDB 11.8.2"
        );
        assert_eq!(display_version("8.0.36-28", false), "MySQL 8.0.36");
        assert!(is_mariadb_version(
            "10.11.6-MariaDB-1:10.11.6+maria~ubu2204"
        ));
        assert!(!is_mariadb_version("9.6.0"));
    }

    #[test]
    fn session_setup_statements() {
        let mut p = profile();
        assert_eq!(setup_statements(&p), ["SET NAMES utf8mb4"]);
        p.options.timezone = Some("Asia/Jakarta".into());
        p.options.read_only = true;
        p.options.startup_sql = Some("  SET SESSION sql_mode = 'ANSI'  ".into());
        assert_eq!(
            setup_statements(&p),
            [
                "SET NAMES utf8mb4",
                "SET SESSION time_zone = 'Asia/Jakarta'",
                "SET SESSION TRANSACTION READ ONLY",
                "SET SESSION sql_mode = 'ANSI'",
            ]
        );
        p.options.timezone = Some("it's".into());
        assert!(
            setup_statements(&p)
                .iter()
                .any(|s| s == "SET SESSION time_zone = 'it''s'")
        );
    }

    #[test]
    fn opts_follow_profile_and_tunnel_override() {
        let p = profile();
        let direct = ConnectTarget {
            profile: &p,
            password: Some("pw"),
            host_override: None,
        };
        let opts = build_opts(&direct, None, ssl_opts(&direct).unwrap(), vec![]);
        assert_eq!(opts.ip_or_hostname(), "db.example.com");
        assert_eq!(opts.tcp_port(), 3307);
        assert_eq!(opts.user(), Some("u"));
        assert_eq!(opts.pass(), Some("pw"));
        assert_eq!(opts.db_name(), Some("d"));
        assert!(opts.client_found_rows());
        assert!(!opts.prefer_socket());
        assert!(opts.ssl_opts().is_none());
        assert_eq!(opts.pool_opts().constraints().max(), POOL_MAX);

        let tunnelled = ConnectTarget {
            profile: &p,
            password: None,
            host_override: Some(("127.0.0.1".into(), 45_000)),
        };
        let opts = build_opts(&tunnelled, None, None, vec![]);
        assert_eq!(opts.ip_or_hostname(), "127.0.0.1");
        assert_eq!(opts.tcp_port(), 45_000);
        assert_eq!(opts.pass(), None);
        assert_eq!(opts.socket(), None);
    }

    #[test]
    fn use_socket_path_typed_or_detected() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("mysql.sock");
        std::fs::write(&sock, "").unwrap();
        let sock = sock.to_string_lossy().into_owned();
        let missing = dir.path().join("none.sock").to_string_lossy().into_owned();
        let mut p = profile();
        let target = |p: &ConnectionProfile, candidates: &[&str]| {
            socket_path(
                &ConnectTarget {
                    profile: p,
                    password: None,
                    host_override: None,
                },
                candidates,
            )
        };
        // Off: TCP.
        p.options.socket_path = Some(sock.clone());
        assert_eq!(target(&p, &[]).unwrap(), None);
        // A typed path is used as is (and must exist).
        p.options.use_socket = Some(true);
        assert_eq!(target(&p, &[]).unwrap().as_deref(), Some(sock.as_str()));
        p.options.socket_path = Some(missing.clone());
        let err = target(&p, &[]).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
        // Empty: the first existing candidate.
        p.options.socket_path = Some("  ".into());
        assert_eq!(
            target(&p, &[missing.as_str(), sock.as_str()])
                .unwrap()
                .as_deref(),
            Some(sock.as_str())
        );
        let err = target(&p, &[missing.as_str()]).unwrap_err();
        assert!(err.to_string().contains("No MySQL socket found"), "{err}");
        // Never through a tunnel.
        let tunnelled = ConnectTarget {
            profile: &p,
            password: None,
            host_override: Some(("127.0.0.1".into(), 1)),
        };
        assert_eq!(
            socket_path(&tunnelled, &[sock.as_str()])
                .unwrap_err()
                .code(),
            "INVALID_INPUT"
        );
        // The socket goes into the options; host and port are ignored by the driver.
        let direct = ConnectTarget {
            profile: &p,
            password: None,
            host_override: None,
        };
        let opts = build_opts(&direct, Some(sock.as_str()), None, vec![]);
        assert_eq!(opts.socket(), Some(sock.as_str()));
        assert!(opts.ssl_opts().is_none());
    }

    #[test]
    fn ssl_modes() {
        let mut p = profile();
        p.ssl_ca_path = Some("/tmp/ca.pem".into());
        let target = |p: &ConnectionProfile| -> Option<SslOpts> {
            ssl_opts(&ConnectTarget {
                profile: p,
                password: None,
                host_override: Some(("127.0.0.1".into(), 1)),
            })
            .unwrap()
        };
        p.ssl = SslMode::Disable;
        assert!(target(&p).is_none());
        p.ssl = SslMode::Prefer;
        let s = target(&p).unwrap();
        assert!(s.accept_invalid_certs() && s.skip_domain_validation());
        p.ssl = SslMode::Require;
        let s = target(&p).unwrap();
        assert!(s.accept_invalid_certs() && s.skip_domain_validation());
        p.ssl = SslMode::VerifyCa;
        let s = target(&p).unwrap();
        assert!(!s.accept_invalid_certs() && s.skip_domain_validation());
        // The CA file first, then the system's roots (one PEM buffer, when it has any).
        let first = format!("{:?}", s.root_certs().first());
        assert!(
            first.contains("Path") && first.contains("ca.pem"),
            "{first}"
        );
        assert!(s.root_certs().len() <= 2);
        // The name is checked against the profile host, also through a tunnel.
        assert_eq!(s.tls_hostname_override(), Some("db.example.com"));
        p.ssl = SslMode::VerifyFull;
        let s = target(&p).unwrap();
        assert!(!s.accept_invalid_certs() && !s.skip_domain_validation());
        assert_eq!(s.tls_hostname_override(), Some("db.example.com"));
        assert!(s.client_identity().is_none());
    }

    #[tokio::test]
    async fn ssl_client_identity() {
        let mut p = profile();
        p.ssl = SslMode::Require;
        let opts = |p: &ConnectionProfile| {
            ssl_opts(&ConnectTarget {
                profile: p,
                password: None,
                host_override: None,
            })
        };
        p.options.ssl_key_path = Some("/somewhere/client.key".into());
        let err = opts(&p).unwrap_err();
        assert!(err.to_string().contains("Cert…"), "{err}");
        // Missing files fail before connecting, naming the file; disabled SSL ignores them.
        p.options.ssl_cert_path = Some("/nonexistent/cert.pem".into());
        assert!(
            opts(&p)
                .unwrap_err()
                .to_string()
                .contains("/nonexistent/cert.pem")
        );
        p.ssl = SslMode::Disable;
        assert!(opts(&p).unwrap().is_none());

        let Some(m) = crate::tls_files::tests::material() else {
            return;
        };
        p.ssl = SslMode::Require;
        // The parsed chain and key go to the driver as PEM buffers.
        p.options.ssl_key_path = Some(m.path("client-pkcs1.key"));
        p.options.ssl_cert_path = Some(m.path("client-chain.crt"));
        let s = opts(&p).unwrap().unwrap();
        let identity = s.client_identity().unwrap();
        assert!(format!("{:?}", identity.cert_chain()).starts_with("Buf"));
        let chain = identity.cert_chain();
        let chain = chain.read().await.unwrap();
        assert_eq!(
            String::from_utf8_lossy(&chain)
                .matches("-----BEGIN CERTIFICATE-----")
                .count(),
            2
        );
        let key = identity.priv_key();
        let key = key.read().await.unwrap();
        assert!(String::from_utf8_lossy(&key).starts_with("-----BEGIN RSA PRIVATE KEY-----"));
        // Formats the driver would misread are refused with the file's name.
        for (key, needle) in [
            (
                "client-encrypted.key",
                "encrypted private keys are not supported",
            ),
            ("client-key.der", "DER"),
        ] {
            p.options.ssl_key_path = Some(m.path(key));
            let err = opts(&p).unwrap_err().to_string();
            assert!(err.contains(key) && err.contains(needle), "{err}");
        }
    }

    /// Regression: "Command Line (IAM authentication)" profiles could not log in to RDS / Aurora
    /// / Cloud SQL IAM users, which authenticate with `mysql_clear_password`.
    #[test]
    fn iam_tokens_may_go_in_clear_text_over_tls_or_a_socket_only() {
        let mut p = profile();
        let opts_with = |p: &ConnectionProfile, ssl: Option<SslOpts>, socket: Option<&str>| {
            let target = ConnectTarget {
                profile: p,
                password: Some("token"),
                host_override: None,
            };
            build_opts(&target, socket, ssl, vec![])
        };
        let tls = Some(SslOpts::default());
        // Keychain passwords never.
        assert!(!opts_with(&p, tls.clone(), None).enable_cleartext_plugin());
        p.options.password_mode = Some(PasswordMode::Command);
        p.options.password_command = Some("aws rds generate-db-auth-token".into());
        assert!(opts_with(&p, tls.clone(), None).enable_cleartext_plugin());
        assert!(opts_with(&p, None, Some("/tmp/mysql.sock")).enable_cleartext_plugin());
        // Plain TCP (SSL off, or PREFERRED that fell back to no TLS): refused.
        assert!(!opts_with(&p, None, None).enable_cleartext_plugin());
        assert!(cleartext_password_allowed(&p, true, false));
        assert!(!cleartext_password_allowed(&p, false, false));

        // The driver's refusal is explained.
        let refused = MyError::Driver(DriverError::CleartextPluginDisabled);
        let msg = connect_error(&p, refused).to_string();
        assert_eq!(
            msg,
            "IAM authentication sends the token in clear text: turn on SSL"
        );
        p.options.password_mode = None;
        let refused = MyError::Driver(DriverError::CleartextPluginDisabled);
        assert!(
            connect_error(&p, refused)
                .to_string()
                .contains("clear text")
        );
        let other = connect_error(&p, MyError::Other("boom".into()));
        assert!(!other.to_string().contains("clear text"));
    }

    #[test]
    fn verify_ca_name_mismatch_is_explained() {
        let e = MyError::Other("invalid peer certificate: certificate not valid for name \"127.0.0.1\"; certificate is only valid for DnsName(\"localhost\")".into());
        assert!(is_name_mismatch(&e));
        let msg = name_check_error("127.0.0.1", &e).to_string();
        assert!(
            msg.contains("VERIFY-IDENTITY") && msg.contains("REQUIRED"),
            "{msg}"
        );
        assert!(msg.contains("not valid for name"), "{msg}");
        assert!(!is_name_mismatch(&MyError::Other("Access denied".into())));
    }
}
