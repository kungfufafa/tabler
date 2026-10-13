//! MySQL "Use socket" against a real server: set `TABLER_TEST_MYSQL_URL`
//! (`mysql://user[:password]@host[:port]/database`) to a server whose Unix socket is reachable
//! here (the local Homebrew MySQL listens on `/tmp/mysql.sock`). Objects are prefixed `conn2_`
//! and dropped afterwards.

// Same allowance as the library: `AppError` carries full server diagnostics.
#![allow(clippy::result_large_err)]

use tabler_lib::appdb::AppDb;
use tabler_lib::commands::profiles::run_connection_test;
use tabler_lib::drivers::{ConnectTarget, connect};
use tabler_lib::secrets::{Backend, Secrets};
use tabler_lib::state::AppState;
use tabler_lib::types::profile::{
    ConnectionProfile, Driver, PasswordMode, ProfileInput, ProfileOptions, SshAuth, SshConfig,
    SslMode,
};
use tabler_lib::types::value::Value;

const ENV: &str = "TABLER_TEST_MYSQL_URL";

/// User, password and database of `mysql://user[:password]@host[:port]/database` (the host is
/// not used: the connection goes through the socket).
fn credentials(url: &str) -> (String, Option<String>, Option<String>) {
    let rest = url.strip_prefix("mysql://").expect("a mysql:// URL");
    let (creds, hostdb) = rest.rsplit_once('@').unwrap_or(("root", rest));
    let (user, password) = match creds.split_once(':') {
        Some((u, p)) => (u.to_string(), Some(p.to_string())),
        None => (creds.to_string(), None),
    };
    let db = hostdb
        .split_once('/')
        .map(|(_, d)| d.to_string())
        .filter(|d| !d.is_empty());
    (user, password, db)
}

/// A socket profile whose host and port point nowhere, so only the socket can work.
fn socket_profile(user: &str, db: Option<String>, socket: Option<&str>) -> ConnectionProfile {
    ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "conn2 socket".into(),
        color: None,
        group: None,
        driver: Driver::Mysql,
        host: Some("unreachable.invalid".into()),
        port: Some(1),
        user: Some(user.into()),
        database: db,
        // Preferred TLS would be tried over TCP; a socket never uses it.
        ssl: SslMode::Prefer,
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions {
            connect_timeout_secs: Some(10),
            use_socket: Some(true),
            socket_path: socket.map(str::to_string),
            ..ProfileOptions::default()
        },
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::Text(s) => s.clone(),
        Value::Int(i) => i.to_string(),
        other => format!("{other:?}"),
    }
}

#[tokio::test]
async fn conn2_mysql_connects_through_its_socket() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let Ok(url) = std::env::var(ENV) else {
        println!("skipped: {ENV} unset");
        return;
    };
    let (user, password, db) = credentials(&url);

    // Auto-detected (empty path): the first usual socket that exists.
    let detected = socket_profile(&user, db.clone(), None);
    let session = match connect(ConnectTarget {
        profile: &detected,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    {
        Ok(s) => s,
        Err(e) if e.to_string().contains("No MySQL socket found") => {
            println!("skipped: no local MySQL socket ({e})");
            return;
        }
        Err(e) => panic!("socket connect failed: {e}"),
    };
    let socket = session
        .query("SELECT @@socket", &[])
        .await
        .unwrap()
        .rows
        .first()
        .map(|r| text(&r[0]))
        .unwrap();
    assert!(socket.ends_with(".sock"), "{socket}");

    // The pool works through the socket too: a conn2_ table round trip.
    session
        .execute("DROP TABLE IF EXISTS conn2_socket_probe", &[])
        .await
        .unwrap();
    session
        .execute("CREATE TABLE conn2_socket_probe (id INT PRIMARY KEY)", &[])
        .await
        .unwrap();
    session
        .execute("INSERT INTO conn2_socket_probe (id) VALUES (1), (2)", &[])
        .await
        .unwrap();
    let count = session
        .query("SELECT COUNT(*) FROM conn2_socket_probe", &[])
        .await
        .unwrap();
    assert_eq!(text(&count.rows[0][0]), "2");
    session
        .execute("DROP TABLE conn2_socket_probe", &[])
        .await
        .unwrap();
    // MySQL 8+ tells how the session came in.
    if let Ok(kind) = session
        .query(
            "SELECT CONNECTION_TYPE FROM performance_schema.threads \
             WHERE PROCESSLIST_ID = CONNECTION_ID()",
            &[],
        )
        .await
        && let Some(row) = kind.rows.first()
    {
        assert_eq!(text(&row[0]), "Socket");
    }
    session.close().await;

    // The path the server reports, typed in the form.
    let typed = socket_profile(&user, db.clone(), Some(&socket));
    let session = connect(ConnectTarget {
        profile: &typed,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap();
    session.ping().await.unwrap();
    session.close().await;

    // Test connection: works, and refuses a socket over SSH before trying anything.
    let appdb = AppDb::open_in_memory().unwrap();
    let state = AppState::new(appdb.clone(), Secrets::new(Backend::File, appdb));
    let mut profile = typed.clone();
    profile.options.password_mode = Some(if password.is_some() {
        PasswordMode::Keychain
    } else {
        PasswordMode::None
    });
    let input = |profile: ConnectionProfile| ProfileInput {
        profile,
        password: password.clone(),
        ssh_password: None,
        ssh_passphrase: None,
        clear_password: false,
    };
    let result = run_connection_test(&state, input(profile.clone()))
        .await
        .unwrap();
    assert!(result.ok, "{}", result.message);
    assert!(result.server_version.unwrap().starts_with("M"));

    profile.ssh = Some(SshConfig {
        host: "bastion.invalid".into(),
        port: 22,
        user: "x".into(),
        auth: SshAuth::Agent,
        key_path: None,
        local_port: None,
        keepalive_secs: None,
    });
    let err = run_connection_test(&state, input(profile))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "INVALID_INPUT");
    assert!(err.to_string().contains("SSH"), "{err}");
}
