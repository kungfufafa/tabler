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
use crate::conn_options::{MYSQL_DEFAULT_CHARSET as DEFAULT_CHARSET, mysql_charset as charset};
use crate::drivers::ConnectTarget;
use crate::error::AppError;
use crate::sqlgen::conn_options as opt_sql;
use crate::types::profile::{
    ConnectionProfile, DEFAULT_CONNECT_TIMEOUT_SECS, MysqlVersion, PasswordMode, ProfileOptions,
    SslMode,
};

/// Worker pool size (browsing + introspection); the editor connection is separate.
const POOL_MAX: usize = 4;
/// Idle pooled connections are recycled after this long.
const CONN_TTL: Duration = Duration::from_secs(600);
const TCP_KEEPALIVE: Duration = Duration::from_secs(60);
/// MySQL 8+: make `TABLE_ROWS` & co. fresh instead of up-to-a-day stale. Unknown on MariaDB.
const STATS_EXPIRY_SQL: &str = "SET SESSION information_schema_stats_expiry = 0";
/// Where "Use socket" looks when no socket path is set, in order (the first that exists wins):
/// the macOS installer's (and Homebrew's link), Debian / Ubuntu packages', Apple-silicon
/// Homebrew's data directory, Intel Homebrew's, MAMP's, XAMPP's, MacPorts' (MySQL 8 and 5.7),
/// the RPM packages' and the systemd `/run` of current Linux distributions.
pub const SOCKET_CANDIDATES: [&str; 10] = [
    "/tmp/mysql.sock",
    "/var/run/mysqld/mysqld.sock",
    "/opt/homebrew/var/mysql/mysql.sock",
    "/usr/local/var/mysql/mysql.sock",
    "/Applications/MAMP/tmp/mysql/mysql.sock",
    "/Applications/XAMPP/xamppfiles/var/mysql/mysql.sock",
    "/opt/local/var/run/mysql8/mysqld.sock",
    "/opt/local/var/run/mysql57/mysqld.sock",
    "/var/lib/mysql/mysql.sock",
    "/run/mysqld/mysqld.sock",
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
/// authentication of RDS / Aurora / Cloud SQL IAM authentication, PAM and LDAP): for a password
/// from a command ("Command Line (IAM authentication)") or with Other options ▸ Enable
/// CLEARTEXT_PLUGIN, and only where the transport protects it: TLS (`tls`), or a connection that
/// is private anyway (`private`): a local Unix socket, or an SSH tunnel, which encrypts the TCP
/// connection to the server (a PAM / LDAP user on a bastion-only server, SSL mode DISABLED).
/// Plain TCP without any of them stays refused.
pub fn cleartext_password_allowed(profile: &ConnectionProfile, tls: bool, private: bool) -> bool {
    cleartext_wanted(profile) && (tls || private)
}

/// The profile sends its password in clear text when the server asks (see above).
fn cleartext_wanted(profile: &ConnectionProfile) -> bool {
    profile.options.password_mode() == PasswordMode::Command
        || profile.options.mysql_cleartext_plugin == Some(true)
}

fn is_cleartext_refused(e: &MyError) -> bool {
    matches!(e, MyError::Driver(DriverError::CleartextPluginDisabled))
}

/// What a connect reports when the server asks for the password in clear text and the driver
/// refused to send it (see [`cleartext_password_allowed`]).
fn cleartext_refused(profile: &ConnectionProfile) -> AppError {
    AppError::Connect(
        if profile.options.password_mode() == PasswordMode::Command {
            "IAM authentication sends the token in clear text: turn on SSL (or connect through an \
             SSH tunnel)"
                .into()
        } else if cleartext_wanted(profile) {
            "Enable CLEARTEXT_PLUGIN sends the password in clear text: turn on SSL, use a socket \
             or connect through an SSH tunnel"
                .into()
        } else {
            "The server asks for the password in clear text (mysql_clear_password authentication): \
             turn on Other options \u{25b8} Enable CLEARTEXT_PLUGIN (over SSL, a socket or an SSH \
             tunnel)"
                .into()
        },
    )
}

/// The server certificate does not list the host: rustls says "certificate not valid for name
/// ...", Secure Transport (the Old MySQL variant) "A host name mismatch has occurred".
fn is_name_mismatch(e: &MyError) -> bool {
    let text = crate::error::error_chain(e);
    text.contains("not valid for name")
        || text.contains("NotValidForName")
        || text.contains("host name mismatch")
}

/// What VERIFY-IDENTITY reports when the certificate does not list the host: what to do instead
/// (VERIFY-CA checks the chain only).
fn name_mismatch_error(profile: &ConnectionProfile, e: &MyError) -> AppError {
    let text = crate::error::error_chain(e);
    // The driver wraps the TLS error in "Input/output error: " twice: quote the TLS part.
    let detail = text
        .find("invalid peer certificate")
        .map_or(text.as_str(), |i| &text[i..]);
    let host = profile.host.as_deref().unwrap_or_default();
    AppError::Connect(format!(
        "VERIFY-IDENTITY: the server certificate does not list \u{201c}{host}\u{201d}. Connect \
         with a host name it lists, or use VERIFY-CA to check the certificate chain without the \
         name ({detail})"
    ))
}

/// The TLS handshake failed over the protocol version: what a TLS 1.0 / 1.1-only server (MySQL
/// 5.5 / 5.6) answers a TLS 1.2 client hello with.
fn is_tls_version_error(e: &MyError) -> bool {
    let text = crate::error::error_chain(e).to_ascii_lowercase();
    text.contains("protocolversion")
        || text.contains("protocol version")
        || text.contains("peer is incompatible")
}

fn connect_error(profile: &ConnectionProfile, e: MyError) -> AppError {
    if is_cleartext_refused(&e) {
        cleartext_refused(profile)
    } else if profile.ssl == SslMode::VerifyFull && is_name_mismatch(&e) {
        name_mismatch_error(profile, &e)
    } else if !is_old(profile) && is_tls_version_error(&e) {
        AppError::Connect(format!(
            "{}. The server may offer only TLS 1.0 / 1.1: choose Ver. \u{25b8} Old MySQL (5.x), \
             which can talk to such servers on macOS",
            crate::error::error_chain(&e)
        ))
    } else {
        e.into()
    }
}

/// A "Use socket" profile over SSH: the tunnel forwards the server's socket (see `socket_path`).
fn socket_over_ssh(target: &ConnectTarget<'_>) -> bool {
    target.profile.uses_socket() && target.host_override.is_some()
}

/// The socket file of a "Use socket" profile (`None` without it): the path typed in the form,
/// else the first of `candidates` that exists. Over SSH (`host_override`) the tunnel forwards the
/// socket on the SSH server to its local TCP port, so the driver connects over TCP.
fn socket_path(
    target: &ConnectTarget<'_>,
    candidates: &[&str],
) -> Result<Option<String>, AppError> {
    let profile = target.profile;
    if !profile.uses_socket() {
        return Ok(None);
    }
    if target.host_override.is_some() {
        return Ok(None);
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
    // Checked by `open` before connecting.
    let mut out = vec![opt_sql::mysql_set_names(
        charset(profile).unwrap_or(DEFAULT_CHARSET),
    )];
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
    // Also a session whose encoding is not UTF-8: Tabler reads and writes text as UTF-8, so the
    // server must refuse the writes that would be stored mis-encoded.
    if crate::conn_options::effective_read_only(profile) {
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
/// PostgreSQL); the driver adds its bundled webpki roots itself. The platform TLS library of the
/// Old MySQL variant trusts the system's roots on its own (`system_roots` false).
fn with_verify_roots(opts: SslOpts, profile: &ConnectionProfile, system_roots: bool) -> SslOpts {
    let mut roots = Vec::new();
    if let Some(ca) = profile
        .ssl_ca_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        roots.push(crate::tunnel::expand_tilde(ca).into());
    }
    let native: String = if system_roots {
        crate::tls_files::native_roots()
            .iter()
            .map(|c| crate::tls_files::pem_block("CERTIFICATE", c))
            .collect()
    } else {
        String::new()
    };
    if !native.is_empty() {
        roots.push(native.into_bytes().into());
    }
    opts.with_root_certs(roots)
}

/// The profile is for an old MySQL server ("Ver." ▸ Old MySQL).
fn is_old(profile: &ConnectionProfile) -> bool {
    profile.options.mysql_version == Some(MysqlVersion::Old)
}

/// Whether the TLS handshake of `profile` goes through the platform's TLS library (TLS 1.0 and
/// up) instead of rustls (TLS 1.2 and up): the Old MySQL variant, whose servers (5.5 / 5.6) often
/// offer nothing newer than TLS 1.1. Secure Transport cannot take the PEM client certificate, so
/// a profile with one stays on rustls. Only macOS builds have the platform path (the `legacy-tls`
/// feature of the vendored driver); elsewhere the option is ignored.
fn uses_legacy_tls(profile: &ConnectionProfile, has_client_identity: bool) -> bool {
    is_old(profile) && !has_client_identity
}

fn ssl_opts(target: &ConnectTarget<'_>) -> Result<Option<SslOpts>, AppError> {
    let profile = target.profile;
    if profile.ssl == SslMode::Disable {
        return Ok(None);
    }
    let identity = client_identity(profile)?;
    let legacy = uses_legacy_tls(profile, identity.is_some());
    let opts = match profile.ssl {
        SslMode::Disable => return Ok(None),
        // Encrypt without authenticating the server (self-signed certs are common).
        SslMode::Prefer | SslMode::Require => SslOpts::default()
            .with_danger_accept_invalid_certs(true)
            .with_danger_skip_domain_validation(true),
        // VERIFY-CA: the chain (CA file, system roots) and the validity period are checked, the
        // host name is not: the vendored driver skips only the name verdict (see
        // vendor/mysql_async/Cargo.toml). The profile host still goes out as the TLS server name
        // (SNI), also through a tunnel.
        SslMode::VerifyCa => with_verify_roots(SslOpts::default(), profile, !legacy)
            .with_danger_skip_domain_validation(true)
            .with_danger_tls_hostname_override(Some(target.tls_host())),
        // Through an SSH tunnel the TCP host is 127.0.0.1; verify against the profile host.
        SslMode::VerifyFull => with_verify_roots(SslOpts::default(), profile, !legacy)
            .with_danger_tls_hostname_override(Some(target.tls_host())),
    };
    Ok(Some(
        opts.with_client_identity(identity).with_legacy_tls(legacy),
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
    // IAM authentication: the token goes in clear text, over the encrypted transport only: TLS,
    // a socket, or an SSH tunnel (`host_override` is its local end).
    let cleartext = cleartext_password_allowed(
        profile,
        ssl.is_some(),
        socket.is_some() || socket_over_ssh(target) || target.host_override.is_some(),
    );
    // LOAD DATA LOCAL INFILE: the files the user allows (none without the option).
    let infile = (profile.options.mysql_local_infile == Some(true))
        .then(|| crate::local_infile::InfileGate::new(profile.name.clone()));
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
        // Old MySQL: servers with `old_passwords` still switch to the pre-4.1 `mysql_old_password`
        // login; the driver refuses it unless `secure_auth` is off. Every other variant keeps it on.
        .secure_auth(!is_old(profile))
        .local_infile_handler(infile)
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

/// The connect timeout of the profile: Advanced ▸ Connect timeout, else the default (60 s).
pub fn connect_timeout(options: &ProfileOptions) -> Duration {
    Duration::from_secs(
        options
            .connect_timeout_secs
            .map(u64::from)
            .filter(|s| *s > 0)
            .unwrap_or(DEFAULT_CONNECT_TIMEOUT_SECS),
    )
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
    let timeout = connect_timeout(&profile.options);
    charset(profile)?;
    let mut setup = setup_statements(profile);
    let socket = socket_path(target, &SOCKET_CANDIDATES)?;
    let socket = socket.as_deref();
    // A local socket is a secure transport for the server (no TLS needed, and the driver cannot
    // run TLS over one), as for the mysql client; so is the server's socket reached over SSH.
    let mut ssl = match socket {
        Some(_) => None,
        None if socket_over_ssh(target) => None,
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
        assert_eq!(setup_statements(&p), ["SET NAMES 'utf8mb4'"]);
        p.options.timezone = Some("Asia/Jakarta".into());
        p.options.read_only = true;
        p.options.startup_sql = Some("  SET SESSION sql_mode = 'ANSI'  ".into());
        assert_eq!(
            setup_statements(&p),
            [
                "SET NAMES 'utf8mb4'",
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
        // Over SSH the tunnel forwards the server's socket: the driver connects over TCP to it,
        // without TLS (SSH encrypts it), and a clear-text password may go as over a local socket.
        let tunnelled = ConnectTarget {
            profile: &p,
            password: None,
            host_override: Some(("127.0.0.1".into(), 1)),
        };
        assert_eq!(socket_path(&tunnelled, &[sock.as_str()]).unwrap(), None);
        assert!(socket_over_ssh(&tunnelled));
        let opts = build_opts(&tunnelled, None, None, vec![]);
        assert_eq!(opts.socket(), None);
        assert_eq!(opts.tcp_port(), 1);
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
        assert!(
            msg.starts_with("IAM authentication sends the token in clear text: turn on SSL"),
            "{msg}"
        );
        assert!(msg.contains("SSH tunnel"), "{msg}");
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
    fn connection_encoding_replaces_utf8mb4() {
        let mut p = profile();
        p.options.encoding = Some(" latin1 ".into());
        assert_eq!(setup_statements(&p)[0], "SET NAMES 'latin1'");
        p.options.encoding = Some("latin1; DROP TABLE x".into());
        assert_eq!(charset(&p).unwrap_err().code(), "INVALID_INPUT");
        p.options.encoding = Some("".into());
        assert_eq!(charset(&p).unwrap(), "utf8mb4");
    }

    #[test]
    fn a_non_utf8_encoding_makes_the_session_read_only() {
        const READ_ONLY: &str = "SET SESSION TRANSACTION READ ONLY";
        let mut p = profile();
        assert!(!setup_statements(&p).iter().any(|s| s == READ_ONLY));
        p.options.encoding = Some("utf8mb4".into());
        assert!(!setup_statements(&p).iter().any(|s| s == READ_ONLY));
        for enc in ["latin1", "LATIN1", "sjis", "gbk"] {
            p.options.encoding = Some(enc.into());
            let sql = setup_statements(&p);
            assert_eq!(sql[0], format!("SET NAMES '{enc}'"));
            assert!(sql.iter().any(|s| s == READ_ONLY), "{enc}: {sql:?}");
        }
        // The option alone still does it, with the default encoding.
        p.options.encoding = None;
        p.options.read_only = true;
        assert!(setup_statements(&p).iter().any(|s| s == READ_ONLY));
    }

    #[test]
    fn other_options_cleartext_plugin_and_local_infile() {
        let mut p = profile();
        let opts_with = |p: &ConnectionProfile, ssl: Option<SslOpts>, socket: Option<&str>| {
            let target = ConnectTarget {
                profile: p,
                password: Some("pw"),
                host_override: None,
            };
            build_opts(&target, socket, ssl, vec![])
        };
        let tls = Some(SslOpts::default());
        assert!(!opts_with(&p, tls.clone(), None).enable_cleartext_plugin());
        p.options.mysql_cleartext_plugin = Some(true);
        assert!(opts_with(&p, tls.clone(), None).enable_cleartext_plugin());
        assert!(opts_with(&p, None, Some("/tmp/mysql.sock")).enable_cleartext_plugin());
        // Never over plain TCP: the refusal says how to fix it.
        assert!(!opts_with(&p, None, None).enable_cleartext_plugin());
        let msg = connect_error(&p, MyError::Driver(DriverError::CleartextPluginDisabled));
        assert!(msg.to_string().contains("turn on SSL"), "{msg}");
        p.options.mysql_cleartext_plugin = None;
        let msg = connect_error(&p, MyError::Driver(DriverError::CleartextPluginDisabled));
        assert!(msg.to_string().contains("Enable CLEARTEXT_PLUGIN"), "{msg}");

        // LOAD DATA LOCAL INFILE: a handler only with the option.
        assert!(opts_with(&p, None, None).local_infile_handler().is_none());
        p.options.mysql_local_infile = Some(true);
        assert!(opts_with(&p, None, None).local_infile_handler().is_some());
    }

    #[test]
    fn socket_candidates_cover_the_usual_installs_and_the_first_existing_wins() {
        // The first three keep their order (they were the whole list).
        assert_eq!(
            SOCKET_CANDIDATES[..3],
            [
                "/tmp/mysql.sock",
                "/var/run/mysqld/mysqld.sock",
                "/opt/homebrew/var/mysql/mysql.sock"
            ]
        );
        for usual in [
            // Intel Homebrew, MAMP, XAMPP, MacPorts, RPM packages, systemd's /run.
            "/usr/local/var/mysql/mysql.sock",
            "/Applications/MAMP/tmp/mysql/mysql.sock",
            "/Applications/XAMPP/xamppfiles/var/mysql/mysql.sock",
            "/opt/local/var/run/mysql8/mysqld.sock",
            "/opt/local/var/run/mysql57/mysqld.sock",
            "/var/lib/mysql/mysql.sock",
            "/run/mysqld/mysqld.sock",
        ] {
            assert!(SOCKET_CANDIDATES.contains(&usual), "{usual}");
        }
        // Absolute and distinct.
        let mut seen = std::collections::HashSet::new();
        for c in SOCKET_CANDIDATES {
            assert!(c.starts_with('/') && seen.insert(c), "{c}");
        }

        // The message lists every place that was tried.
        let dir = tempfile::tempdir().unwrap();
        let missing: Vec<String> = (0..3)
            .map(|i| dir.path().join(format!("m{i}.sock")).display().to_string())
            .collect();
        let mut p = profile();
        p.options.use_socket = Some(true);
        let err = socket_path(
            &ConnectTarget {
                profile: &p,
                password: None,
                host_override: None,
            },
            &[&missing[0], &missing[1], &missing[2]],
        )
        .unwrap_err()
        .to_string();
        for m in &missing {
            assert!(err.contains(m.as_str()), "{err}");
        }
        // And the real list, checked against this machine: the answer is one of the candidates
        // or the error that names them all.
        match resolve_socket(&p) {
            Ok(Some(found)) => assert!(SOCKET_CANDIDATES.contains(&found.as_str()), "{found}"),
            Ok(None) => panic!("use_socket is on"),
            Err(e) => assert!(
                e.to_string().contains("/usr/local/var/mysql/mysql.sock"),
                "{e}"
            ),
        }
    }

    #[test]
    fn old_mysql_accepts_the_pre_4_1_login_the_other_variants_refuse() {
        let mut p = profile();
        let secure_auth = |p: &ConnectionProfile| {
            let target = ConnectTarget {
                profile: p,
                password: Some("pw"),
                host_override: None,
            };
            build_opts(&target, None, None, vec![]).secure_auth()
        };
        assert!(secure_auth(&p), "no version chosen: Default");
        p.options.mysql_version = Some(MysqlVersion::Old);
        assert!(!secure_auth(&p), "Old MySQL allows mysql_old_password");
        for version in [
            MysqlVersion::Default,
            MysqlVersion::Mariadb,
            MysqlVersion::Singlestore,
        ] {
            p.options.mysql_version = Some(version);
            assert!(secure_auth(&p), "{version:?}");
        }
    }

    #[test]
    fn only_old_mysql_uses_the_platform_tls_library_and_not_with_a_client_certificate() {
        let mut p = profile();
        p.ssl = SslMode::Require;
        let legacy = |p: &ConnectionProfile| {
            ssl_opts(&ConnectTarget {
                profile: p,
                password: None,
                host_override: None,
            })
            .unwrap()
            .unwrap()
            .legacy_tls()
        };
        for version in [
            None,
            Some(MysqlVersion::Default),
            Some(MysqlVersion::Mariadb),
            Some(MysqlVersion::Singlestore),
        ] {
            p.options.mysql_version = version;
            assert!(!legacy(&p), "{version:?}");
        }
        p.options.mysql_version = Some(MysqlVersion::Old);
        for mode in [
            SslMode::Prefer,
            SslMode::Require,
            SslMode::VerifyCa,
            SslMode::VerifyFull,
        ] {
            p.ssl = mode;
            assert!(legacy(&p), "{mode:?}");
        }
        // DISABLED has no TLS at all.
        p.ssl = SslMode::Disable;
        assert!(
            ssl_opts(&ConnectTarget {
                profile: &p,
                password: None,
                host_override: None,
            })
            .unwrap()
            .is_none()
        );
        // Secure Transport takes no PEM client certificate: that profile stays on rustls.
        assert!(uses_legacy_tls(&p, false));
        assert!(!uses_legacy_tls(&p, true));
    }

    /// The Old variant verifies like the others: its roots are the CA file only (the platform
    /// library trusts the system's roots itself), the others also carry the system's.
    #[test]
    fn verify_modes_of_the_old_variant_do_not_copy_the_system_roots() {
        let mut p = profile();
        p.ssl = SslMode::VerifyCa;
        p.ssl_ca_path = Some("/tmp/ca.pem".into());
        let roots = |p: &ConnectionProfile| {
            ssl_opts(&ConnectTarget {
                profile: p,
                password: None,
                host_override: None,
            })
            .unwrap()
            .unwrap()
            .root_certs()
            .len()
        };
        p.options.mysql_version = Some(MysqlVersion::Old);
        assert_eq!(roots(&p), 1);
        p.options.mysql_version = None;
        assert!(roots(&p) >= 1);
    }

    #[test]
    fn a_clear_text_password_may_go_through_an_ssh_tunnel() {
        let mut p = profile();
        p.options.mysql_cleartext_plugin = Some(true);
        let opts = |p: &ConnectionProfile, ssl: Option<SslOpts>, tunnelled: bool| {
            let target = ConnectTarget {
                profile: p,
                password: Some("pw"),
                host_override: tunnelled.then(|| ("127.0.0.1".to_string(), 45_000)),
            };
            build_opts(&target, None, ssl, vec![]).enable_cleartext_plugin()
        };
        // SSL DISABLED, TCP through the tunnel: SSH encrypts it, so it is allowed.
        assert!(opts(&p, None, true));
        // Direct TCP without TLS stays refused; with TLS it is allowed.
        assert!(!opts(&p, None, false));
        assert!(opts(&p, Some(SslOpts::default()), false));
        // Not asked for: never, tunnel or not.
        p.options.mysql_cleartext_plugin = None;
        assert!(!opts(&p, None, true));

        // The decision itself: TLS, or a private transport (socket, SSH tunnel).
        p.options.mysql_cleartext_plugin = Some(true);
        assert!(cleartext_password_allowed(&p, false, true));
        assert!(cleartext_password_allowed(&p, true, false));
        assert!(!cleartext_password_allowed(&p, false, false));
        // IAM authentication follows the same rule.
        p.options.mysql_cleartext_plugin = None;
        p.options.password_mode = Some(PasswordMode::Command);
        assert!(cleartext_password_allowed(&p, false, true));
        assert!(!cleartext_password_allowed(&p, false, false));
        // The refusal tells about the tunnel.
        let msg = connect_error(&p, MyError::Driver(DriverError::CleartextPluginDisabled));
        assert!(msg.to_string().contains("SSH tunnel"), "{msg}");
    }

    #[test]
    fn the_connect_timeout_defaults_to_60_seconds() {
        let mut options = ProfileOptions::default();
        assert_eq!(connect_timeout(&options), Duration::from_secs(60));
        assert_eq!(DEFAULT_CONNECT_TIMEOUT_SECS, 60);
        options.connect_timeout_secs = Some(0);
        assert_eq!(connect_timeout(&options), Duration::from_secs(60));
        options.connect_timeout_secs = Some(25);
        assert_eq!(connect_timeout(&options), Duration::from_secs(25));
    }

    #[test]
    fn a_name_mismatch_under_verify_identity_points_to_verify_ca() {
        let rustls = MyError::Other(
            "invalid peer certificate: certificate not valid for name \"127.0.0.1\"; certificate is not valid for any names"
                .into(),
        );
        let secure_transport =
            MyError::Other("TLS 1.0 / 1.1 handshake: A host name mismatch has occurred.".into());
        let mut p = profile();
        p.ssl = SslMode::VerifyFull;
        for e in [rustls, secure_transport] {
            assert!(is_name_mismatch(&e));
        }
        let rustls = MyError::Other(
            "Input/output error: invalid peer certificate: certificate not valid for name \"db.example.com\"; certificate is only valid for DnsName(\"other\")"
                .into(),
        );
        let msg = connect_error(&p, rustls).to_string();
        assert!(
            msg.contains("VERIFY-IDENTITY")
                && msg.contains("VERIFY-CA")
                && msg.contains("db.example.com"),
            "{msg}"
        );
        assert!(
            msg.contains("not valid for name"),
            "the TLS detail is kept: {msg}"
        );
        // Other modes and other failures are reported as they are.
        p.ssl = SslMode::VerifyCa;
        let msg =
            connect_error(&p, MyError::Other("certificate not valid for name".into())).to_string();
        assert!(!msg.contains("VERIFY-IDENTITY"), "{msg}");
        p.ssl = SslMode::VerifyFull;
        let msg = connect_error(&p, MyError::Other("Access denied".into())).to_string();
        assert!(!msg.contains("VERIFY-IDENTITY"), "{msg}");
    }

    #[test]
    fn a_tls_version_refusal_points_to_old_mysql() {
        let mut p = profile();
        p.ssl = SslMode::Require;
        for text in [
            "Input/output error: received fatal alert: ProtocolVersion",
            "peer is incompatible: ServerTlsVersionIsDisabledByOurConfig",
            "error: protocol version not supported",
        ] {
            let msg = connect_error(&p, MyError::Other(text.into())).to_string();
            assert!(msg.contains("Old MySQL (5.x)"), "{text}: {msg}");
            assert!(msg.contains(text), "the cause is kept: {msg}");
        }
        // The Old variant already speaks TLS 1.0 / 1.1: nothing to suggest there.
        p.options.mysql_version = Some(MysqlVersion::Old);
        let msg = connect_error(
            &p,
            MyError::Other("received fatal alert: ProtocolVersion".into()),
        )
        .to_string();
        assert!(!msg.contains("Old MySQL (5.x)"), "{msg}");
        // Other failures are reported as they are.
        p.options.mysql_version = None;
        let msg = connect_error(&p, MyError::Other("Access denied".into())).to_string();
        assert!(!msg.contains("Old MySQL (5.x)"), "{msg}");
    }
}
