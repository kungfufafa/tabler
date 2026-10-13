//! Verification against real servers that the other integration tests do not cover (see
//! docs/verify-servers.md for the compose services, the certificates and the variables):
//!
//! - A. TLS client certificates through Tabler's own connect path: PostgreSQL `cert` /
//!   `clientcert=verify-*` (`pg-ssl`) and MySQL `REQUIRE X509` / `REQUIRE SUBJECT` with
//!   `require_secure_transport` (`mysql-ssl`): key formats (PKCS#8, PKCS#1, SEC1, encrypted,
//!   DER, PKCS#12), chain order, host-name checks of `verify-full` against an IP host.
//! - B. MySQL value-change notes / warnings as CSV import errors (`mysql_warning_guard`) on
//!   MariaDB and MySQL.
//! - C. The SQL-dump import's own connection through the profile's SSH tunnel (mirrors
//!   `commands::import::dump_session`): it works, and its forwarded connections are gone once the
//!   import closes it while the connection's tunnel stays up.
//!
//! Every test skips itself when its variables are unset. Objects created on a server are
//! `verify_*` tables, dropped at the end.

// Same allowance as the library: `AppError` carries full server diagnostics.
#![allow(clippy::result_large_err)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use tabler_lib::appdb::AppDb;
use tabler_lib::drivers::{CancelSlot, ConnectTarget, DbSession, connect};
use tabler_lib::error::AppError;
use tabler_lib::import::{run_csv_import, run_sql_import};
use tabler_lib::secrets::{Backend, Secrets};
use tabler_lib::state::AppState;
use tabler_lib::tunnel::{
    AppHostKeys, TunnelAuth, TunnelHandle, TunnelStatus, open_tunnel, trust_host_key,
};
use tabler_lib::types::import::{
    CsvFormat, CsvImportReport, CsvImportRequest, ImportTarget, InsertMode, SqlImportRequest,
    TextEncoding,
};
use tabler_lib::types::profile::{ConnectionProfile, SshAuth, SshConfig};
use tabler_lib::types::schema::TableRef;
use tabler_lib::types::value::Value;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

// ----- shared helpers ---------------------------------------------------------------------------

fn install_crypto() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
}

/// The repository root (relative paths in the variables are relative to it).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn repo_path(p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    if path.is_absolute() {
        path
    } else {
        repo_root().join(path)
    }
}

/// `scheme://user[:pass]@host[:port]/db`.
#[derive(Debug, Clone)]
struct DbUrl {
    driver: &'static str,
    user: Option<String>,
    password: Option<String>,
    host: String,
    port: Option<u16>,
    db: Option<String>,
}

fn parse_url(url: &str) -> DbUrl {
    let (driver, rest) = if let Some(r) = url.strip_prefix("postgres://") {
        ("postgres", r)
    } else if let Some(r) = url.strip_prefix("postgresql://") {
        ("postgres", r)
    } else if let Some(r) = url.strip_prefix("mysql://") {
        ("mysql", r)
    } else {
        panic!("unsupported URL {url}");
    };
    let (auth, hostdb) = rest
        .rsplit_once('@')
        .map_or((None, rest), |(a, h)| (Some(a), h));
    let (user, password) = match auth.map(|a| a.split_once(':').unwrap_or((a, ""))) {
        Some((u, p)) => (
            Some(u.to_string()),
            Some(p.to_string()).filter(|p| !p.is_empty()),
        ),
        None => (None, None),
    };
    let (hostport, db) = hostdb.split_once('/').unwrap_or((hostdb, ""));
    let (host, port) = hostport
        .rsplit_once(':')
        .map_or((hostport, None), |(h, p)| (h, p.parse::<u16>().ok()));
    DbUrl {
        driver,
        user,
        password,
        host: host.to_string(),
        port,
        db: db
            .split('?')
            .next()
            .filter(|d| !d.is_empty())
            .map(String::from),
    }
}

/// TLS settings of a profile (`ssl` is the kebab-case `SslMode`).
#[derive(Debug, Clone, Default)]
struct Tls {
    ssl: &'static str,
    ca: Option<String>,
    key: Option<String>,
    cert: Option<String>,
}

/// Only what a session needs; the rest defaults (robust to new profile fields).
fn profile(url: &DbUrl, tls: &Tls) -> ConnectionProfile {
    serde_json::from_value(serde_json::json!({
        "id": Uuid::new_v4(),
        "name": "verify",
        "driver": url.driver,
        "host": url.host,
        "port": url.port,
        "user": url.user,
        "database": url.db,
        "ssl": if tls.ssl.is_empty() { "disable" } else { tls.ssl },
        "sslCaPath": tls.ca,
        "options": {
            "connectTimeoutSecs": 10,
            "sslKeyPath": tls.key,
            "sslCertPath": tls.cert,
        },
    }))
    .expect("profile")
}

async fn open_session(
    profile: &ConnectionProfile,
    password: Option<&str>,
    host_override: Option<(String, u16)>,
) -> Result<Arc<dyn DbSession>, AppError> {
    tokio::time::timeout(
        CONNECT_TIMEOUT,
        connect(ConnectTarget {
            profile,
            password,
            host_override,
        }),
    )
    .await
    .unwrap_or(Err(AppError::Timeout))
}

/// A plain (no TLS) session for the URL in `env`, or `None` (skipped) when it is unset.
async fn open_env(env: &str) -> Option<(Arc<dyn DbSession>, DbUrl)> {
    let Ok(url) = std::env::var(env) else {
        println!("skipped: {env} unset");
        return None;
    };
    install_crypto();
    let url = parse_url(&url);
    let p = profile(&url, &Tls::default());
    let s = open_session(&p, url.password.as_deref(), None)
        .await
        .unwrap_or_else(|e| panic!("connect {env}: {e}"));
    Some((s, url))
}

fn text(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Int(n) => n.to_string(),
        Value::Text(s)
        | Value::BigInt(s)
        | Value::Decimal(s)
        | Value::Date(s)
        | Value::Timestamp(s)
        | Value::Enum(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

async fn scalar(s: &dyn DbSession, sql: &str) -> Value {
    s.query(sql, &[]).await.expect(sql).rows.remove(0).remove(0)
}

/// One-line form of an error: `[CODE] message`.
fn describe(e: &AppError) -> String {
    format!("[{}] {}", e.code(), e.to_string().replace('\n', " | "))
}

// ----- SSH (compose `ssh` service) --------------------------------------------------------------

struct SshEnv {
    cfg: SshConfig,
    password: Option<String>,
}

impl SshEnv {
    fn from_env() -> Option<SshEnv> {
        let host = std::env::var("TABLER_TEST_SSH_HOST").ok()?;
        let port = std::env::var("TABLER_TEST_SSH_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(22);
        let user = std::env::var("TABLER_TEST_SSH_USER").unwrap_or_else(|_| "tabler".into());
        let key = std::env::var("TABLER_TEST_SSH_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .map(|k| repo_path(&k).display().to_string());
        let password = std::env::var("TABLER_TEST_SSH_PASSWORD").ok();
        let auth = if key.is_some() && password.is_none() {
            SshAuth::Key
        } else {
            SshAuth::Password
        };
        Some(SshEnv {
            cfg: SshConfig {
                host,
                port,
                user,
                auth,
                key_path: key,
                local_port: None,
                keepalive_secs: Some(10),
            },
            password,
        })
    }

    fn auth(&self) -> TunnelAuth {
        TunnelAuth {
            password: self.password.clone(),
            ..TunnelAuth::default()
        }
    }
}

fn test_state() -> AppState {
    let appdb = AppDb::open_in_memory().unwrap();
    AppState::new(appdb.clone(), Secrets::new(Backend::File, appdb))
}

/// Opens the compose SSH tunnel to `host:port` (as the SSH server sees it), trusting the
/// server's host key the way "Accept" in the app does (never touching `~/.ssh/known_hosts`).
async fn compose_tunnel(
    state: &AppState,
    ssh: &SshEnv,
    profile_id: Uuid,
    host: &str,
    port: u16,
) -> TunnelHandle {
    let attempt = || async {
        let trust = AppHostKeys::for_state(state, profile_id).with_known_hosts(None);
        tokio::time::timeout(
            CONNECT_TIMEOUT,
            open_tunnel(&ssh.cfg, ssh.auth(), host.to_string(), port, &trust),
        )
        .await
        .expect("SSH tunnel timed out")
    };
    match attempt().await {
        Ok(t) => t,
        Err(AppError::HostKey { .. }) => {
            let pending = state
                .pending_host_keys
                .lock()
                .unwrap()
                .get(&profile_id)
                .cloned()
                .expect("pending host key");
            trust_host_key(&state.appdb, &pending).unwrap();
            attempt().await.expect("SSH tunnel after trusting the key")
        }
        Err(e) => panic!("SSH tunnel: {}", describe(&e)),
    }
}

// ----- A. TLS client certificates ----------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Expect {
    Ok,
    /// Fails, with one of these substrings in the message (empty: any message).
    Err(&'static [&'static str]),
}

struct Case {
    label: &'static str,
    user: &'static str,
    password: Option<&'static str>,
    /// `None`: the URL's host.
    host: Option<&'static str>,
    ssl: &'static str,
    ca: bool,
    key: Option<&'static str>,
    cert: Option<&'static str>,
    expect: Expect,
}

const fn case(
    label: &'static str,
    user: &'static str,
    ssl: &'static str,
    key: Option<&'static str>,
    cert: Option<&'static str>,
    expect: Expect,
) -> Case {
    Case {
        label,
        user,
        password: None,
        host: None,
        ssl,
        ca: true,
        key,
        cert,
        expect,
    }
}

fn cert_dir() -> PathBuf {
    repo_path(
        &std::env::var("TABLER_TEST_SSL_CERT_DIR").unwrap_or_else(|_| "dev/certs".to_string()),
    )
}

fn cert_file(name: &str) -> String {
    cert_dir().join(name).display().to_string()
}

/// What the server says about the session: user and TLS state.
async fn tls_probe(s: &dyn DbSession, driver: &str) -> String {
    if driver == "postgres" {
        let rs = s
            .query(
                "SELECT current_user::text, s.version::text, s.client_dn::text \
                 FROM pg_stat_ssl s WHERE s.pid = pg_backend_pid()",
                &[],
            )
            .await;
        match rs {
            Ok(rs) => {
                let row = &rs.rows[0];
                format!(
                    "user={} tls={} client_dn={}",
                    text(&row[0]),
                    text(&row[1]),
                    text(&row[2])
                )
            }
            Err(e) => format!("probe failed: {}", describe(&e)),
        }
    } else {
        let user = s
            .query("SELECT CURRENT_USER()", &[])
            .await
            .map(|rs| text(&rs.rows[0][0]))
            .unwrap_or_else(|e| describe(&e));
        let tls = s
            .query("SHOW SESSION STATUS LIKE 'Ssl_version'", &[])
            .await
            .map(|rs| rs.rows.first().map(|r| text(&r[1])).unwrap_or_default())
            .unwrap_or_else(|e| describe(&e));
        format!("user={user} tls={tls}")
    }
}

async fn run_case(url: &DbUrl, c: &Case, host_override: Option<(String, u16)>) -> (bool, String) {
    let mut u = url.clone();
    u.user = Some(c.user.to_string());
    if let Some(h) = c.host {
        u.host = h.to_string();
    }
    let tls = Tls {
        ssl: c.ssl,
        ca: c.ca.then(|| cert_file("ca.crt")),
        key: c.key.map(cert_file),
        cert: c.cert.map(cert_file),
    };
    let p = profile(&u, &tls);
    let tunnelled = host_override.is_some();
    let started = Instant::now();
    let outcome = open_session(&p, c.password, host_override).await;
    let ms = started.elapsed().as_millis();
    match (outcome, c.expect) {
        (Ok(s), expect) => {
            let probe = tls_probe(s.as_ref(), url.driver).await;
            s.close().await;
            (
                matches!(expect, Expect::Ok),
                format!("OK ({ms} ms) {probe}"),
            )
        }
        (Err(e), expect) => {
            let msg = describe(&e);
            let pass = match expect {
                Expect::Ok => false,
                Expect::Err(needles) => {
                    needles.is_empty() || needles.iter().any(|n| msg.contains(n))
                }
            };
            // tokio-postgres' Display is only the error kind: show what the cause would say.
            let generic = msg.ends_with("] error performing TLS handshake")
                || msg.ends_with("] error connecting to server")
                || msg.ends_with("] error communicating with the server");
            let cause = if url.driver == "postgres" && generic && !tunnelled {
                pg_raw_cause(&u, c)
                    .await
                    .map(|c| format!(" || raw cause via 127.0.0.1: {c}"))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            (pass, format!("ERR {msg}{cause}"))
        }
    }
}

/// The same `verify-full` connection made with tokio-postgres + rustls directly, pinned to the
/// IPv4 address (`hostaddr`), returning the error with its whole `source()` chain. `None` when
/// the case is not `verify-full` or its files are not PEM.
async fn pg_raw_cause(u: &DbUrl, c: &Case) -> Option<String> {
    use rustls::pki_types::pem::PemObject as _;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    if c.ssl != "verify-full" {
        return None;
    }
    let mut roots = rustls::RootCertStore::empty();
    if c.ca {
        for cert in CertificateDer::pem_file_iter(cert_file("ca.crt")).ok()? {
            roots.add(cert.ok()?).ok()?;
        }
    }
    let builder = rustls::ClientConfig::builder().with_root_certificates(roots);
    let config = match (c.key, c.cert) {
        (Some(k), Some(crt)) => {
            let chain = CertificateDer::pem_file_iter(cert_file(crt))
                .ok()?
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            let key = PrivateKeyDer::from_pem_file(cert_file(k)).ok()?;
            builder.with_client_auth_cert(chain, key).ok()?
        }
        _ => builder.with_no_client_auth(),
    };
    let mut cfg = tokio_postgres::Config::new();
    cfg.host(&u.host)
        .hostaddr(std::net::IpAddr::from([127, 0, 0, 1]))
        .port(u.port.unwrap_or(5432))
        .user(c.user)
        .dbname(u.db.as_deref().unwrap_or("postgres"))
        .ssl_mode(tokio_postgres::config::SslMode::Require)
        .connect_timeout(Duration::from_secs(10));
    if let Some(pw) = c.password {
        cfg.password(pw);
    }
    let tls = tokio_postgres_rustls::MakeRustlsConnect::new(config);
    match cfg.connect(tls).await {
        Ok(_) => Some("(connects)".into()),
        Err(e) => {
            let mut text = e.to_string();
            let mut source = std::error::Error::source(&e);
            while let Some(s) = source {
                text.push_str(": ");
                text.push_str(&s.to_string());
                source = s.source();
            }
            Some(text.replace('\n', " "))
        }
    }
}

/// Runs the cases, prints every outcome, then fails listing the unexpected ones.
async fn run_cases(
    title: &str,
    url: &DbUrl,
    cases: &[Case],
    tunnel: Option<(&Case, (String, u16))>,
) {
    let mut failures = Vec::new();
    println!("== {title}");
    for c in cases {
        let (pass, outcome) = run_case(url, c, None).await;
        println!(
            "{} {:<58} {}",
            if pass { "pass" } else { "FAIL" },
            c.label,
            outcome
        );
        if !pass {
            failures.push(format!(
                "{}: expected {:?}, got {outcome}",
                c.label, c.expect
            ));
        }
    }
    if let Some((c, addr)) = tunnel {
        let (pass, outcome) = run_case(url, c, Some(addr)).await;
        println!(
            "{} {:<58} {}",
            if pass { "pass" } else { "FAIL" },
            c.label,
            outcome
        );
        if !pass {
            failures.push(format!(
                "{}: expected {:?}, got {outcome}",
                c.label, c.expect
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{title}: {} unexpected outcome(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

const KEY: Option<&str> = Some("client.key");
const CHAIN: Option<&str> = Some("client-chain.crt");
const ANY_ERR: Expect = Expect::Err(&[]);

/// `pg-ssl`: `tabler_cert` (`cert`), `tabler_ca` (password + `clientcert=verify-ca`),
/// `tabler_full` (password + `clientcert=verify-full`). The URL host should be `localhost` (the
/// server certificate has no IP SAN).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_postgres_client_certificates() {
    let Ok(raw) = std::env::var("TABLER_TEST_PG_SSL_URL") else {
        println!("skipped: TABLER_TEST_PG_SSL_URL unset");
        return;
    };
    install_crypto();
    let url = parse_url(&raw);
    let cert_only = "tabler_cert";
    #[rustfmt::skip]
    let cases = [
        case("verify-full, PKCS#8 key + leaf,intermediate chain", cert_only, "verify-full", KEY, CHAIN, Expect::Ok),
        case("verify-full, PKCS#1 key", cert_only, "verify-full", Some("client-pkcs1.key"), CHAIN, Expect::Ok),
        case("verify-full, EC (SEC1) key", cert_only, "verify-full", Some("client-ec.key"), Some("client-ec.crt"), Expect::Ok),
        case("verify-full, key+chain in one PEM for both fields", cert_only, "verify-full", Some("client-combined.pem"), Some("client-combined.pem"), Expect::Ok),
        case("require (no CA), client cert", cert_only, "require", KEY, CHAIN, Expect::Ok),
        case("prefer, client cert", cert_only, "prefer", KEY, CHAIN, Expect::Ok),
        case("verify-ca, client cert", cert_only, "verify-ca", KEY, CHAIN, Expect::Ok),
        // Server cannot build the chain to its CA without the intermediate.
        case("leaf only (no intermediate in the cert file)", cert_only, "verify-full", KEY, Some("client.crt"), ANY_ERR),
        case("chain in the wrong order (intermediate first)", cert_only, "verify-full", KEY, Some("client-chain-reversed.crt"), Expect::Err(&["SSL client certificate", "key"])),
        case("encrypted PKCS#8 key", cert_only, "verify-full", Some("client-encrypted.key"), CHAIN, Expect::Err(&["client-encrypted.key"])),
        case("DER (binary) PKCS#8 key", cert_only, "verify-full", Some("client-key.der"), CHAIN, Expect::Err(&["client-key.der"])),
        case("PKCS#12 bundle in both fields", cert_only, "verify-full", Some("client.p12"), Some("client.p12"), Expect::Err(&["client.p12"])),
        case("no client certificate", cert_only, "verify-full", None, None, Expect::Err(&["requires a valid client certificate"])),
        case("certificate of another CN (cert auth)", cert_only, "verify-full", Some("other.key"), Some("other.crt"), Expect::Err(&["certificate authentication failed"])),
        case("sslmode disable (server: hostssl only)", cert_only, "disable", None, None, Expect::Err(&["no encryption"])),
        Case {
            ca: false,
            ..case("verify-full, no CA file (native roots only)", cert_only, "verify-full", KEY, CHAIN, ANY_ERR)
        },
        Case {
            host: Some("127.0.0.1"),
            ..case("verify-full to 127.0.0.1 (cert has no IP SAN)", cert_only, "verify-full", KEY, CHAIN, Expect::Err(&["not valid for name", "NotValidForName"]))
        },
        Case {
            host: Some("127.0.0.1"),
            ..case("verify-ca to 127.0.0.1 (name not checked)", cert_only, "verify-ca", KEY, CHAIN, Expect::Ok)
        },
        Case {
            password: Some("tabler"),
            ..case("password + clientcert=verify-ca, other CN", "tabler_ca", "verify-full", Some("other.key"), Some("other.crt"), Expect::Ok)
        },
        Case {
            password: Some("tabler"),
            ..case("password + clientcert=verify-ca, no cert", "tabler_ca", "verify-full", None, None, Expect::Err(&["requires a valid client certificate"]))
        },
        Case {
            password: Some("tabler"),
            ..case("password + clientcert=verify-full, matching CN", "tabler_full", "verify-full", Some("full.key"), Some("full.crt"), Expect::Ok)
        },
        Case {
            password: Some("tabler"),
            ..case("password + clientcert=verify-full, other CN", "tabler_full", "verify-full", Some("other.key"), Some("other.crt"), Expect::Err(&["authentication failed"]))
        },
    ];
    // Through the compose SSH server to the `pg-ssl` service: SNI / name check use the profile
    // host (`pg-ssl`), not the tunnel's 127.0.0.1.
    let state = test_state();
    let ssh = SshEnv::from_env();
    let tunnel = match &ssh {
        Some(ssh) => Some(compose_tunnel(&state, ssh, Uuid::new_v4(), "pg-ssl", 5432).await),
        None => None,
    };
    let tunnelled = Case {
        host: Some("pg-ssl"),
        ..case(
            "verify-full via SSH tunnel to host pg-ssl",
            cert_only,
            "verify-full",
            KEY,
            CHAIN,
            Expect::Ok,
        )
    };
    let through_ssh = tunnel.as_ref().map(|t| (&tunnelled, t.connect_addr()));
    run_cases("PostgreSQL client certificates", &url, &cases, through_ssh).await;
    if let Some(t) = &tunnel {
        t.close().await;
    }
}

/// `mysql-ssl` (require_secure_transport=ON): `tabler_x509` (REQUIRE X509), `tabler_subject`
/// (REQUIRE SUBJECT '/O=Tabler Dev/CN=tabler_cert'), `tabler_pw` (password, REQUIRE SSL).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mysql_client_certificates() {
    let Ok(raw) = std::env::var("TABLER_TEST_MYSQL_SSL_URL") else {
        println!("skipped: TABLER_TEST_MYSQL_SSL_URL unset");
        return;
    };
    install_crypto();
    let url = parse_url(&raw);
    let x509 = "tabler_x509";
    #[rustfmt::skip]
    let cases = [
        case("verify-full, PKCS#8 key + leaf,intermediate chain", x509, "verify-full", KEY, CHAIN, Expect::Ok),
        case("verify-full, PKCS#1 key", x509, "verify-full", Some("client-pkcs1.key"), CHAIN, Expect::Ok),
        case("verify-full, EC (SEC1) key", x509, "verify-full", Some("client-ec.key"), Some("client-ec.crt"), Expect::Ok),
        case("verify-full, key+chain in one PEM for both fields", x509, "verify-full", Some("client-combined.pem"), Some("client-combined.pem"), Expect::Ok),
        case("require (no CA), client cert", x509, "require", KEY, CHAIN, Expect::Ok),
        case("prefer, client cert", x509, "prefer", KEY, CHAIN, Expect::Ok),
        case("verify-ca, client cert", x509, "verify-ca", KEY, CHAIN, Expect::Ok),
        case("leaf only (no intermediate in the cert file)", x509, "verify-full", KEY, Some("client.crt"), ANY_ERR),
        case("chain in the wrong order (intermediate first)", x509, "verify-full", KEY, Some("client-chain-reversed.crt"), Expect::Err(&["SSL client certificate"])),
        case("encrypted PKCS#8 key", x509, "verify-full", Some("client-encrypted.key"), CHAIN, Expect::Err(&["client-encrypted.key"])),
        case("DER (binary) PKCS#8 key", x509, "verify-full", Some("client-key.der"), CHAIN, Expect::Err(&["client-key.der"])),
        case("PKCS#12 bundle in both fields", x509, "verify-full", Some("client.p12"), Some("client.p12"), Expect::Err(&["client.p12"])),
        case("no client certificate (REQUIRE X509)", x509, "verify-full", None, None, Expect::Err(&["Access denied"])),
        case("sslmode disable (require_secure_transport)", x509, "disable", None, None, Expect::Err(&["Access denied", "insecure transport"])),
        case("REQUIRE SUBJECT, matching cert", "tabler_subject", "verify-full", KEY, CHAIN, Expect::Ok),
        case("REQUIRE SUBJECT, other cert", "tabler_subject", "verify-full", Some("other.key"), Some("other.crt"), Expect::Err(&["Access denied"])),
        Case {
            ca: false,
            ..case("verify-full, no CA file (built-in + system roots)", x509, "verify-full", KEY, CHAIN, ANY_ERR)
        },
        Case {
            host: Some("127.0.0.1"),
            ..case("verify-full to 127.0.0.1 (cert has no IP SAN)", x509, "verify-full", KEY, CHAIN, Expect::Err(&["not valid for name", "NotValidForName"]))
        },
        Case {
            host: Some("127.0.0.1"),
            // mysql_async 0.37.1 cannot skip the name check with rustls 0.23.45: refused, saying
            // what to use instead (docs/verify-servers.md, A.3).
            ..case("verify-ca to 127.0.0.1 (driver checks the name: explained)", x509, "verify-ca", KEY, CHAIN, Expect::Err(&["VERIFY-IDENTITY"]))
        },
        Case {
            password: Some("tabler"),
            ..case("password, REQUIRE SSL, no client cert", "tabler_pw", "verify-full", None, None, Expect::Ok)
        },
    ];
    let state = test_state();
    let ssh = SshEnv::from_env();
    let tunnel = match &ssh {
        Some(ssh) => Some(compose_tunnel(&state, ssh, Uuid::new_v4(), "mysql-ssl", 3306).await),
        None => None,
    };
    let tunnelled = Case {
        host: Some("mysql-ssl"),
        ..case(
            "verify-full via SSH tunnel to host mysql-ssl",
            x509,
            "verify-full",
            KEY,
            CHAIN,
            Expect::Ok,
        )
    };
    let through_ssh = tunnel.as_ref().map(|t| (&tunnelled, t.connect_addr()));
    run_cases("MySQL client certificates", &url, &cases, through_ssh).await;
    if let Some(t) = &tunnel {
        t.close().await;
    }
}

// ----- B. MySQL value changes as CSV import errors -----------------------------------------------

fn csv_request(
    path: &str,
    table: &TableRef,
    mapping: &[&str],
    mode: InsertMode,
) -> CsvImportRequest {
    CsvImportRequest {
        import_id: format!("verify-{}", Uuid::new_v4().simple()),
        path: path.into(),
        format: CsvFormat {
            delimiter: Some(",".into()),
            quote: "\"".into(),
            encoding: Some(TextEncoding::Utf8),
            header: true,
        },
        target: ImportTarget::Existing {
            table: table.clone(),
        },
        mapping: mapping.iter().map(|m| Some(m.to_string())).collect(),
        empty_as_null: true,
        mode,
    }
}

async fn import_csv(
    s: &dyn DbSession,
    dir: &std::path::Path,
    table: &TableRef,
    csv: &str,
    mapping: &[&str],
    mode: InsertMode,
) -> CsvImportReport {
    let path = dir.join(format!("{}.csv", Uuid::new_v4().simple()));
    std::fs::write(&path, csv).unwrap();
    let meta = s.table_meta(table).await.expect("table meta");
    let req = csv_request(&path.to_string_lossy(), table, mapping, mode);
    run_csv_import(s, &req, Some(&meta), |_| {}, CancellationToken::new())
        .await
        .expect("import")
}

fn summary(r: &CsvImportReport) -> String {
    match &r.failure {
        Some(f) => format!(
            "failure rows {:?}..{:?} line {:?}: {}",
            f.first_row, f.last_row, f.line, f.message
        ),
        None => format!(
            "ok: {} imported, {} skipped {:?}",
            r.rows_imported,
            r.rows_skipped,
            r.skipped
                .iter()
                .map(|s| format!("{s:?}"))
                .collect::<Vec<_>>()
        ),
    }
}

async fn csv_value_changes(env: &str) {
    let Some((s, url)) = open_env(env).await else {
        return;
    };
    // A second session outside strict mode (bootstrap command), where MySQL cuts / zeroes values
    // with a warning instead of an error.
    let mut lax_profile = profile(&url, &Tls::default());
    lax_profile.options.startup_sql = Some("SET SESSION sql_mode = ''".into());
    let lax = open_session(&lax_profile, url.password.as_deref(), None)
        .await
        .unwrap_or_else(|e| panic!("connect {env} (sql_mode ''): {}", describe(&e)));
    let version = s.server_info().await.map(|i| i.version).unwrap_or_default();
    let name = format!("verify_csv_{}", Uuid::new_v4().simple());
    let schema = text(&scalar(s.as_ref(), "SELECT DATABASE()").await);
    let table = TableRef::new(Some(&schema), &name);
    let q = format!("`{name}`");
    s.execute(
        &format!("CREATE TABLE {q} (id INT PRIMARY KEY, d DECIMAL(10,2), day DATE)"),
        &[],
    )
    .await
    .unwrap();
    let steps =
        std::panic::AssertUnwindSafe(csv_steps(s.as_ref(), lax.as_ref(), &table, &q, &version));
    let outcome = futures::FutureExt::catch_unwind(steps).await;
    s.execute(&format!("DROP TABLE IF EXISTS {q}"), &[])
        .await
        .unwrap();
    s.close().await;
    lax.close().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn csv_steps(
    s: &dyn DbSession,
    lax: &dyn DbSession,
    table: &TableRef,
    q: &str,
    version: &str,
) {
    let dir = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();
    let mut check = |label: &str, ok: bool, detail: String| {
        println!("{} {label:<52} {detail}", if ok { "pass" } else { "FAIL" });
        if !ok {
            failures.push(format!("{label}: {detail}"));
        }
    };
    println!("== CSV value changes on {version}");
    let count = |sql: String| async move { text(&scalar(s, &sql).await) };

    // A DECIMAL rounded to its scale (a note on MySQL and MariaDB alike).
    let r = import_csv(
        s,
        dir.path(),
        table,
        "id,d\n1,1.5\n2,1.239\n3,2\n",
        &["id", "d"],
        InsertMode::StopOnError,
    )
    .await;
    let rows = count(format!("SELECT COUNT(*) FROM {q}")).await;
    let ok = r.failure.as_ref().is_some_and(|f| {
        f.first_row == Some(2) && f.message.ends_with("MySQL would store a changed value")
    }) && rows == "0";
    check(
        "decimal 1.239 -> DECIMAL(10,2), stop on error",
        ok,
        format!("{} (rows in table: {rows})", summary(&r)),
    );

    // A date that would lose its time part.
    let r = import_csv(
        s,
        dir.path(),
        table,
        "id,day\n1,2026-01-02\n2,2026-01-02 10:20:30\n",
        &["id", "day"],
        InsertMode::StopOnError,
    )
    .await;
    let rows = count(format!("SELECT COUNT(*) FROM {q}")).await;
    let ok = r.failure.as_ref().is_some_and(|f| f.first_row == Some(2)) && rows == "0";
    check(
        "datetime -> DATE (time dropped), stop on error",
        ok,
        format!("{} (rows in table: {rows})", summary(&r)),
    );

    // Values that fit go in.
    let r = import_csv(
        s,
        dir.path(),
        table,
        "id,d,day\n1,1.5,2026-01-02\n50,2,2026-02-03\n",
        &["id", "d", "day"],
        InsertMode::StopOnError,
    )
    .await;
    check(
        "clean rows",
        r.failure.is_none() && r.rows_imported == 2,
        summary(&r),
    );

    // ON DUPLICATE KEY UPDATE (MySQL warns 1287 about VALUES(); not an error).
    let r = import_csv(
        s,
        dir.path(),
        table,
        "id,d\n1,2.25\n1,3.5\n",
        &["id", "d"],
        InsertMode::UpdateOnDuplicate,
    )
    .await;
    let d1 = count(format!("SELECT CAST(d AS CHAR) FROM {q} WHERE id = 1")).await;
    check(
        "update on duplicate, clean",
        r.failure.is_none() && d1 == "3.50",
        format!("{} (d of id 1: {d1})", summary(&r)),
    );

    // A rounding value through ON DUPLICATE KEY UPDATE / REPLACE must not be stored changed.
    let r = import_csv(
        s,
        dir.path(),
        table,
        "id,d\n1,4.125\n",
        &["id", "d"],
        InsertMode::UpdateOnDuplicate,
    )
    .await;
    let d1 = count(format!("SELECT CAST(d AS CHAR) FROM {q} WHERE id = 1")).await;
    check(
        "update on duplicate, 4.125 rounds",
        (r.failure.is_some() || r.rows_skipped == 1) && d1 == "3.50",
        format!("{} (d of id 1: {d1})", summary(&r)),
    );
    let r = import_csv(
        s,
        dir.path(),
        table,
        "id,day\n50,2026-03-04 05:06:07\n",
        &["id", "day"],
        InsertMode::Replace,
    )
    .await;
    let day = count(format!("SELECT CAST(day AS CHAR) FROM {q} WHERE id = 50")).await;
    check(
        "replace, datetime -> DATE",
        (r.failure.is_some() || r.rows_skipped == 1) && day == "2026-02-03",
        format!("{} (day of id 50: {day})", summary(&r)),
    );

    // The guard must not leave the session's sql_mode changed.
    let mode = count("SELECT @@SESSION.sql_mode".to_string()).await;
    check(
        "sql_mode afterwards",
        !mode.contains("tabler-import-warning"),
        mode,
    );

    // Outside strict mode: text into a DECIMAL (zeroed) and an out-of-range value (clipped).
    let lax_mode = text(&scalar(lax, "SELECT @@SESSION.sql_mode").await);
    for (label, csv, id) in [
        ("sql_mode '', 'abc' -> DECIMAL(10,2)", "id,d\n60,abc\n", 60),
        (
            "sql_mode '', 123456789012 -> DECIMAL(10,2)",
            "id,d\n61,123456789012\n",
            61,
        ),
    ] {
        let r = import_csv(
            lax,
            dir.path(),
            table,
            csv,
            &["id", "d"],
            InsertMode::StopOnError,
        )
        .await;
        let n = count(format!("SELECT COUNT(*) FROM {q} WHERE id = {id}")).await;
        check(
            label,
            r.failure.is_some() && n == "0",
            format!("{} (session sql_mode '{lax_mode}', rows: {n})", summary(&r)),
        );
    }

    assert!(
        failures.is_empty(),
        "{} unexpected outcome(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn b_mariadb_csv_value_changes() {
    csv_value_changes("TABLER_TEST_MARIADB_URL").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn b_mysql_csv_value_changes() {
    csv_value_changes("TABLER_TEST_MYSQL_URL").await;
}

// ----- C. SQL-dump import connection through the SSH tunnel --------------------------------------

/// This process's ESTABLISHED TCP sockets on the tunnel's local port: (driver side, tunnel side).
fn tunnel_sockets(port: u16) -> (usize, usize) {
    let out = std::process::Command::new("lsof")
        .args([
            "-nP",
            "-a",
            "-p",
            &std::process::id().to_string(),
            &format!("-iTCP:{port}"),
            "-sTCP:ESTABLISHED",
            "-Fn",
        ])
        .output()
        .expect("lsof");
    let text = String::from_utf8_lossy(&out.stdout);
    let driver_side = format!(":{port}");
    let tunnel_side = format!(":{port}->");
    let names: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix('n')).collect();
    (
        names.iter().filter(|n| n.ends_with(&driver_side)).count(),
        names.iter().filter(|n| n.contains(&tunnel_side)).count(),
    )
}

fn listening(port: u16) -> bool {
    let out = std::process::Command::new("lsof")
        .args([
            "-nP",
            "-a",
            "-p",
            &std::process::id().to_string(),
            &format!("-iTCP:{port}"),
            "-sTCP:LISTEN",
            "-t",
        ])
        .output()
        .expect("lsof");
    !out.stdout.is_empty()
}

async fn wait_for_sockets(port: u16, want: (usize, usize), within: Duration) -> (usize, usize) {
    let started = Instant::now();
    loop {
        let now = tunnel_sockets(port);
        if now == want || started.elapsed() > within {
            return now;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Connections the server sees from Tabler's clients (PostgreSQL: `application_name`; MySQL: the
/// user's sessions from outside the server's own container).
async fn server_connections(s: &dyn DbSession, mysql: bool) -> String {
    let sql = if mysql {
        "SELECT COUNT(*) FROM information_schema.PROCESSLIST \
         WHERE USER = SUBSTRING_INDEX(CURRENT_USER(), '@', 1) \
           AND HOST NOT LIKE 'localhost%' AND HOST NOT LIKE '127.0.0.1%'"
    } else {
        "SELECT count(*) FROM pg_stat_activity WHERE application_name = 'tabler'"
    };
    text(&scalar(s, sql).await)
}

async fn dump_over_ssh(env: &str) {
    let Ok(raw) = std::env::var(env) else {
        println!("skipped: {env} unset");
        return;
    };
    let Some(ssh) = SshEnv::from_env() else {
        println!("skipped: TABLER_TEST_SSH_HOST unset");
        return;
    };
    install_crypto();
    let url = parse_url(&raw);
    let mysql = url.driver == "mysql";
    let default_port = if mysql { 3306 } else { 5432 };
    let mut p = profile(&url, &Tls::default());
    p.ssh = Some(ssh.cfg.clone());
    let state = test_state();
    let tunnel = compose_tunnel(
        &state,
        &ssh,
        p.id,
        &url.host,
        url.port.unwrap_or(default_port),
    )
    .await;
    let status = tunnel.subscribe();
    let port = tunnel.local_port();
    let main = open_session(&p, url.password.as_deref(), Some(tunnel.connect_addr()))
        .await
        .unwrap_or_else(|e| panic!("connect through the tunnel: {}", describe(&e)));
    let current_db = main
        .server_info()
        .await
        .ok()
        .and_then(|i| i.current_database);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let baseline = tunnel_sockets(port);
    let server_before = server_connections(main.as_ref(), mysql).await;
    println!(
        "== SQL dump over SSH ({env}), tunnel port {port}; baseline sockets {baseline:?}, server-side connections {server_before}"
    );

    let name = format!("verify_dump_{}", Uuid::new_v4().simple());
    let dump = if mysql {
        format!(
            "SET NAMES utf8mb4;\nSET @OLD_FOREIGN_KEY_CHECKS=@@FOREIGN_KEY_CHECKS, FOREIGN_KEY_CHECKS=0;\n\
             CREATE TABLE `{name}` (id INT PRIMARY KEY, name VARCHAR(10));\n\
             LOCK TABLES `{name}` WRITE;\nINSERT INTO `{name}` VALUES (1,'a'),(2,'b');\nUNLOCK TABLES;\n\
             SET FOREIGN_KEY_CHECKS=@OLD_FOREIGN_KEY_CHECKS;\n"
        )
    } else {
        format!(
            "SET client_min_messages = warning;\nSET search_path = public;\n\
             CREATE TABLE \"{name}\" (id int PRIMARY KEY, name text);\n\
             BEGIN;\nINSERT INTO \"{name}\" VALUES (1,'a'),(2,'b');\nCOMMIT;\n"
        )
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dump.sql");
    std::fs::write(&path, &dump).unwrap();
    let req = SqlImportRequest {
        import_id: format!("verify-dump-{}", Uuid::new_v4().simple()),
        path: path.to_string_lossy().into_owned(),
        stop_on_error: true,
    };

    // What `commands::import::dump_session` does: the profile at its current database, through
    // the connection's tunnel (`connect_addr`), bounded by the connect timeout.
    let mut dump_profile = p.clone();
    if let Some(db) = current_db {
        dump_profile.database = Some(db);
    }
    assert!(tunnel.is_up());
    let session = open_session(
        &dump_profile,
        url.password.as_deref(),
        Some(tunnel.connect_addr()),
    )
    .await
    .unwrap_or_else(|e| panic!("dump session: {}", describe(&e)));
    let report = run_sql_import(
        session.as_ref(),
        &req,
        CancelSlot::default(),
        |_| {},
        CancellationToken::new(),
    )
    .await;
    let during = tunnel_sockets(port);
    let server_during = server_connections(main.as_ref(), mysql).await;
    let close_started = Instant::now();
    let closed_in_time = tokio::time::timeout(Duration::from_secs(5), session.close())
        .await
        .is_ok();
    let close_ms = close_started.elapsed().as_millis();
    drop(session);
    let after = wait_for_sockets(port, baseline, Duration::from_secs(10)).await;
    let mut server_after = server_connections(main.as_ref(), mysql).await;
    let waited = Instant::now();
    while server_after != server_before && waited.elapsed() < Duration::from_secs(10) {
        tokio::time::sleep(Duration::from_millis(200)).await;
        server_after = server_connections(main.as_ref(), mysql).await;
    }
    println!("   import: {report:?}");
    println!(
        "   sockets during {during:?}, after close {after:?} (close {close_ms} ms, in time: {closed_in_time}); server-side connections during {server_during}, after {server_after}"
    );

    // The import's rows are visible on the connection's own session; the tunnel is untouched.
    let rows = text(
        &scalar(
            main.as_ref(),
            &format!(
                "SELECT COUNT(*) FROM {}",
                if mysql {
                    format!("`{name}`")
                } else {
                    format!("\"{name}\"")
                }
            ),
        )
        .await,
    );
    let tunnel_unchanged = !status.has_changed().unwrap_or(true) && tunnel.is_up();
    main.execute(
        &if mysql {
            format!("DROP TABLE IF EXISTS `{name}`")
        } else {
            format!("DROP TABLE IF EXISTS \"{name}\"")
        },
        &[],
    )
    .await
    .unwrap();
    main.close().await;
    tunnel.close().await;
    let after_tunnel = wait_for_sockets(port, (0, 0), Duration::from_secs(5)).await;
    let state_after = tunnel.state();
    let event = status.has_changed().unwrap_or(true);
    let still_listening = listening(port);
    let port_free = std::net::TcpListener::bind(("127.0.0.1", port)).is_ok();
    println!(
        "   rows {rows}; tunnel up after import: {tunnel_unchanged}; after tunnel.close(): sockets {after_tunnel:?}, status {:?} (event: {event}), listening {still_listening}, port free {port_free}",
        state_after.status
    );

    let report = report.expect("dump import");
    assert_eq!((report.failed, report.stopped), (0, false), "{report:?}");
    assert_eq!(rows, "2");
    assert!(
        during.0 > baseline.0,
        "the dump session did not use the tunnel: {during:?}"
    );
    assert!(
        closed_in_time,
        "closing the dump session took more than 5 s"
    );
    assert_eq!(
        after, baseline,
        "dump session sockets left on the tunnel port"
    );
    assert_eq!(
        server_after, server_before,
        "the dump session's server connections stayed open"
    );
    assert!(
        tunnel_unchanged,
        "the connection's tunnel changed state during the import"
    );
    assert_eq!(state_after.status, TunnelStatus::Down);
    assert!(event, "no tunnel status change after close");
    assert_eq!(after_tunnel, (0, 0));
    assert!(!still_listening && port_free, "tunnel port still bound");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c_postgres_dump_over_ssh() {
    dump_over_ssh("TABLER_TEST_SSH_PG_URL").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c_mysql_dump_over_ssh() {
    dump_over_ssh("TABLER_TEST_SSH_MYSQL_URL").await;
}
