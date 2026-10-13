//! MySQL TLS against the local server's auto-generated certificates (MySQL 8+ creates `ca.pem`,
//! `server-cert.pem` and `client-cert.pem` / `client-key.pem` in its data directory; Homebrew's is
//! `/opt/homebrew/var/mysql`, or set `TABLER_TEST_MYSQL_DATADIR`). Needs `TABLER_TEST_MYSQL_URL`
//! (`mysql://user[:password]@host[:port]/database`); skips itself otherwise.

// Same allowance as the library: `AppError` carries full server diagnostics.
#![allow(clippy::result_large_err)]

use std::path::PathBuf;

use tabler_lib::drivers::{ConnectTarget, connect};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};

const ENV: &str = "TABLER_TEST_MYSQL_URL";

struct Server {
    user: String,
    password: Option<String>,
    port: u16,
    datadir: PathBuf,
}

fn server() -> Option<Server> {
    let Ok(url) = std::env::var(ENV) else {
        println!("skipped: {ENV} unset");
        return None;
    };
    let rest = url.strip_prefix("mysql://")?;
    let (creds, hostdb) = rest.rsplit_once('@').unwrap_or(("root", rest));
    let (user, password) = match creds.split_once(':') {
        Some((u, p)) => (u.to_string(), Some(p.to_string())),
        None => (creds.to_string(), None),
    };
    let hostport = hostdb.split('/').next().unwrap_or(hostdb);
    let port = hostport
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse().ok())
        .unwrap_or(3306);
    let datadir = std::env::var("TABLER_TEST_MYSQL_DATADIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/opt/homebrew/var/mysql"));
    let readable = ["ca.pem", "client-cert.pem", "client-key.pem"]
        .iter()
        .all(|f| std::fs::File::open(datadir.join(f)).is_ok());
    if !readable {
        println!(
            "skipped: no readable MySQL certificates in {}",
            datadir.display()
        );
        return None;
    }
    Some(Server {
        user,
        password,
        port,
        datadir,
    })
}

fn tls_profile(s: &Server, ssl: SslMode) -> ConnectionProfile {
    let file = |f: &str| Some(s.datadir.join(f).to_string_lossy().into_owned());
    ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "conn2 tls".into(),
        color: None,
        group: None,
        driver: Driver::Mysql,
        // An IP: the auto-generated server certificate lists no IP (nor any host name).
        host: Some("127.0.0.1".into()),
        port: Some(s.port),
        user: Some(s.user.clone()),
        database: None,
        ssl,
        ssl_ca_path: file("ca.pem"),
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions {
            connect_timeout_secs: Some(10),
            ssl_key_path: file("client-key.pem"),
            ssl_cert_path: file("client-cert.pem"),
            ..ProfileOptions::default()
        },
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    }
}

#[tokio::test]
async fn conn2_mysql_client_certificate_and_verify_ca() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let Some(s) = server() else { return };
    let open = |p: ConnectionProfile| {
        let password = s.password.clone();
        async move {
            connect(ConnectTarget {
                profile: &p,
                password: password.as_deref(),
                host_override: None,
            })
            .await
        }
    };

    // REQUIRED with the server's client certificate: the parsed PEM buffers reach the server.
    let session = match open(tls_profile(&s, SslMode::Require)).await {
        Ok(session) => session,
        Err(e) if e.to_string().contains("SSL is required") || e.to_string().contains("TLS") => {
            println!("skipped: the server does not offer TLS ({e})");
            return;
        }
        Err(e) => panic!("REQUIRED with a client certificate: {e}"),
    };
    let cipher = session
        .query("SHOW SESSION STATUS LIKE 'Ssl_cipher'", &[])
        .await
        .unwrap();
    let row = cipher.rows.first().expect("Ssl_cipher");
    assert!(!format!("{:?}", row[1]).contains("\"\""), "no TLS: {row:?}");
    session.close().await;

    // VERIFY-CA to an IP the certificate does not list: refused with what to use instead.
    let err = match open(tls_profile(&s, SslMode::VerifyCa)).await {
        Ok(session) => {
            session.close().await;
            panic!("VERIFY-CA should fail the (driver's) name check for 127.0.0.1");
        }
        Err(e) => e.to_string(),
    };
    println!("VERIFY-CA to 127.0.0.1: {err}");
    assert!(
        err.contains("VERIFY-IDENTITY") && err.contains("REQUIRED"),
        "{err}"
    );
    assert!(err.contains("127.0.0.1"), "{err}");
}
