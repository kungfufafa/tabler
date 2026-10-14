//! MySQL TLS against the local server's auto-generated certificates (MySQL 8+ creates `ca.pem`,
//! `server-cert.pem` and `client-cert.pem` / `client-key.pem` in its data directory; Homebrew's is
//! `/opt/homebrew/var/mysql`, or set `TABLER_TEST_MYSQL_DATADIR`). Needs `TABLER_TEST_MYSQL_URL`
//! (`mysql://user[:password]@host[:port]/database`); skips itself otherwise.
//!
//! The auto-generated server certificate has a CN and no SAN (not even an IP), so connecting to
//! `127.0.0.1` is the VERIFY-CA scenario: the chain verifies against `ca.pem`, the name never
//! does.

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
    assert_tls_cipher(&session).await;
    session.close().await;

    // VERIFY-CA to an IP the certificate does not list: the chain is checked against ca.pem, the
    // name is not (it used to fail like VERIFY-IDENTITY and tell the user to switch to REQUIRED).
    let session = match open(tls_profile(&s, SslMode::VerifyCa)).await {
        Ok(session) => session,
        Err(e) => panic!("VERIFY-CA to 127.0.0.1 with the server's own CA must connect: {e}"),
    };
    assert_tls_cipher(&session).await;
    session.close().await;

    // The same without the client certificate.
    let mut no_identity = tls_profile(&s, SslMode::VerifyCa);
    no_identity.options.ssl_key_path = None;
    no_identity.options.ssl_cert_path = None;
    let session = open(no_identity)
        .await
        .unwrap_or_else(|e| panic!("VERIFY-CA without a client certificate: {e}"));
    assert_tls_cipher(&session).await;
    session.close().await;

    // VERIFY-IDENTITY still checks the name against the profile host.
    let err = match open(tls_profile(&s, SslMode::VerifyFull)).await {
        Ok(session) => {
            session.close().await;
            panic!("VERIFY-IDENTITY to 127.0.0.1 must fail: the certificate does not list it");
        }
        Err(e) => e.to_string(),
    };
    println!("VERIFY-IDENTITY to 127.0.0.1: {err}");
    assert!(
        err.contains("not valid for name") || err.contains("NotValidForName"),
        "{err}"
    );

    // VERIFY-CA with a CA that did not sign the server certificate: skipping the name check must
    // not skip the chain check.
    let other = tempfile::tempdir().unwrap();
    let other_ca = other.path().join("other-ca.pem");
    std::fs::write(&other_ca, OTHER_CA).unwrap();
    let mut wrong_ca = tls_profile(&s, SslMode::VerifyCa);
    wrong_ca.ssl_ca_path = Some(other_ca.to_string_lossy().into_owned());
    wrong_ca.options.ssl_key_path = None;
    wrong_ca.options.ssl_cert_path = None;
    let err = match open(wrong_ca).await {
        Ok(session) => {
            session.close().await;
            panic!("VERIFY-CA with a CA that signed nothing must fail");
        }
        Err(e) => e.to_string(),
    };
    println!("VERIFY-CA with the wrong CA: {err}");
    assert!(
        err.contains("UnknownIssuer") || err.to_ascii_lowercase().contains("unknown"),
        "{err}"
    );
    assert!(!err.contains("not valid for name"), "{err}");
}

/// The session runs over TLS: the server reports a cipher.
async fn assert_tls_cipher(session: &std::sync::Arc<dyn tabler_lib::drivers::DbSession>) {
    let cipher = session
        .query("SHOW SESSION STATUS LIKE 'Ssl_cipher'", &[])
        .await
        .unwrap();
    let row = cipher.rows.first().expect("Ssl_cipher");
    assert!(!format!("{:?}", row[1]).contains("\"\""), "no TLS: {row:?}");
}

/// A self-signed CA (CA:TRUE) that signed nothing the local server uses.
const OTHER_CA: &str = "-----BEGIN CERTIFICATE-----
MIIBwDCCAWegAwIBAgIURfHEhJApwuhePFxIqNpNudj6ljAwCgYIKoZIzj0EAwIw
LTErMCkGA1UEAwwiVGFibGVyIHRlc3QgQ0EgdGhhdCBzaWduZWQgbm90aGluZzAg
Fw0yNjEwMTQwMjQ3MjZaGA8yMTI2MDkyMDAyNDcyNlowLTErMCkGA1UEAwwiVGFi
bGVyIHRlc3QgQ0EgdGhhdCBzaWduZWQgbm90aGluZzBZMBMGByqGSM49AgEGCCqG
SM49AwEHA0IABG+n2DeQNjPNYUES9mBr9t2qtJB/9zKu4Rdkgyc1XoGOmDMqfBGe
K8We8ErwImbzspfi4IM0YaSls/vWO7V8bpSjYzBhMB0GA1UdDgQWBBTjq2MOKQGh
WyxzAZJdCMIhbPCOWzAfBgNVHSMEGDAWgBTjq2MOKQGhWyxzAZJdCMIhbPCOWzAP
BgNVHRMBAf8EBTADAQH/MA4GA1UdDwEB/wQEAwIBBjAKBggqhkjOPQQDAgNHADBE
AiBBTOIYyxHX957KmPtpmY1cl+zVv4ag9Hp6MbSZGsOG2wIgMlUHFyf0RQohNmwV
xJ7YC1jS8F5MsgGdwHIJ8u0j7iI=
-----END CERTIFICATE-----
";
