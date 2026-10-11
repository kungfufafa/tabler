//! Connection setup: `OptsBuilder` from the profile, TLS policy, connect timeout, session
//! initialisation, the dedicated editor connection and the worker pool.

use std::path::PathBuf;
use std::time::Duration;

use mysql_async::prelude::Queryable;
use mysql_async::{
    Conn, DriverError, Error as MyError, IoError, Opts, OptsBuilder, Pool, PoolConstraints,
    PoolOpts, Row, SslOpts,
};

use super::decode::text_of;
use crate::drivers::ConnectTarget;
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, SslMode};

const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 15;
/// Worker pool size (browsing + introspection); the editor connection is separate.
const POOL_MAX: usize = 4;
/// Idle pooled connections are recycled after this long.
const CONN_TTL: Duration = Duration::from_secs(600);
const TCP_KEEPALIVE: Duration = Duration::from_secs(60);
/// MySQL 8+: make `TABLE_ROWS` & co. fresh instead of up-to-a-day stale. Unknown on MariaDB.
const STATS_EXPIRY_SQL: &str = "SET SESSION information_schema_stats_expiry = 0";

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

fn ssl_opts(target: &ConnectTarget<'_>) -> Option<SslOpts> {
    let profile = target.profile;
    let roots: Vec<_> = profile
        .ssl_ca_path
        .iter()
        .filter(|p| !p.trim().is_empty())
        .map(|p| PathBuf::from(p).into())
        .collect();
    match profile.ssl {
        SslMode::Disable => None,
        // Encrypt without authenticating the server (self-signed certs are common).
        SslMode::Prefer | SslMode::Require => Some(
            SslOpts::default()
                .with_danger_accept_invalid_certs(true)
                .with_danger_skip_domain_validation(true),
        ),
        SslMode::VerifyCa => Some(
            SslOpts::default()
                .with_root_certs(roots)
                .with_danger_skip_domain_validation(true),
        ),
        // Through an SSH tunnel the TCP host is 127.0.0.1; verify against the profile host.
        SslMode::VerifyFull => Some(
            SslOpts::default()
                .with_root_certs(roots)
                .with_danger_tls_hostname_override(Some(target.tls_host())),
        ),
    }
}

fn build_opts(target: &ConnectTarget<'_>, ssl: Option<SslOpts>, setup: Vec<String>) -> Opts {
    let profile = target.profile;
    let constraints = PoolConstraints::new(0, POOL_MAX).expect("0 <= POOL_MAX");
    OptsBuilder::default()
        .ip_or_hostname(target.host())
        .tcp_port(target.port())
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
    let mut ssl = ssl_opts(target);

    let mut editor =
        match connect_once(build_opts(target, ssl.clone(), setup.clone()), timeout).await? {
            Ok(conn) => conn,
            Err(e) if profile.ssl == SslMode::Prefer && ssl.is_some() && is_tls_error(&e) => {
                log::warn!("mysql: TLS handshake failed ({e}); retrying without TLS");
                ssl = None;
                connect_once(build_opts(target, None, setup.clone()), timeout).await??
            }
            Err(e) => return Err(e.into()),
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

    let pool_opts = build_opts(target, ssl, setup);
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
        let opts = build_opts(&direct, ssl_opts(&direct), vec![]);
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
        let opts = build_opts(&tunnelled, None, vec![]);
        assert_eq!(opts.ip_or_hostname(), "127.0.0.1");
        assert_eq!(opts.tcp_port(), 45_000);
        assert_eq!(opts.pass(), None);
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
        assert_eq!(s.root_certs().len(), 1);
        p.ssl = SslMode::VerifyFull;
        let s = target(&p).unwrap();
        assert!(!s.accept_invalid_certs() && !s.skip_domain_validation());
        assert_eq!(s.tls_hostname_override(), Some("db.example.com"));
    }
}
