//! conn3a: connection options against real servers. Runs only when `TABLER_TEST_PG_URL` /
//! `TABLER_TEST_MYSQL_URL` are set (see .env.example). Objects are prefixed `conn3a_` and dropped
//! afterwards.
//! - PostgreSQL: extra parameters (startup `application_name` / `options`, server settings),
//!   Disable channel binding, connection encoding.
//! - MySQL: connection encoding (`SET NAMES`), LOAD DATA LOCAL INFILE limited to allowed files.

// `AppError` carries the full server diagnostic (same allowance as the library).
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use tabler_lib::drivers::{ConnectTarget, DbSession, connect};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::value::Value;

/// `scheme://user[:pass]@host[:port]/db` into a profile plus the password.
fn profile_from_url(url: &str, driver: Driver) -> (ConnectionProfile, Option<String>) {
    let rest = url.split_once("://").map(|(_, r)| r).expect("a URL");
    let (auth, hostdb) = match rest.rsplit_once('@') {
        Some((a, h)) => (Some(a), h),
        None => (None, rest),
    };
    let (user, password) = match auth {
        Some(a) => match a.split_once(':') {
            Some((u, p)) => (Some(u.to_string()), Some(p.to_string())),
            None => (Some(a.to_string()), None),
        },
        None => (None, None),
    };
    let (hostport, db) = match hostdb.split_once('/') {
        Some((h, d)) => (h, Some(d.split('?').next().unwrap_or("").to_string())),
        None => (hostdb, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (hostport.to_string(), None),
    };
    let profile = ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "conn3a".into(),
        color: None,
        group: None,
        driver,
        host: Some(host),
        port,
        user,
        database: db.filter(|d| !d.is_empty()),
        ssl: SslMode::Prefer,
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions::default(),
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    };
    (profile, password)
}

async fn open(profile: &ConnectionProfile, password: Option<&str>) -> Arc<dyn DbSession> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    connect(ConnectTarget {
        profile,
        password,
        host_override: None,
    })
    .await
    .expect("connect")
}

async fn text(session: &Arc<dyn DbSession>, sql: &str) -> String {
    let rs = session.query(sql, &[]).await.expect(sql);
    match &rs.rows[0][0] {
        Value::Text(s) => s.clone(),
        Value::Int(i) => i.to_string(),
        other => format!("{other:?}"),
    }
}

#[tokio::test]
async fn conn3a_postgres_parameters_channel_binding_and_encoding() {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return;
    };
    let (mut p, password) = profile_from_url(&url, Driver::Postgres);
    p.options.pg_params = Some(
        "application_name=conn3a_app options='-c geqo=off' statement_timeout=1234 \
         search_path='conn3a_s, public' jit=off"
            .into(),
    );
    p.options.pg_disable_channel_binding = Some(true);
    p.options.encoding = Some("LATIN1".into());
    let session = open(&p, password.as_deref()).await;
    // Worker pool connections (query) and the editor get the same settings.
    assert_eq!(text(&session, "SHOW application_name").await, "conn3a_app");
    assert_eq!(text(&session, "SHOW geqo").await, "off");
    assert_eq!(text(&session, "SHOW statement_timeout").await, "1234ms");
    assert_eq!(text(&session, "SHOW search_path").await, "conn3a_s, public");
    assert_eq!(text(&session, "SHOW jit").await, "off");
    assert_eq!(text(&session, "SHOW client_encoding").await, "LATIN1");
    // Tabler reads and writes UTF-8 only: a session with another encoding is read-only, so an
    // edit cannot be stored mis-encoded (the server refuses it).
    assert_eq!(
        text(&session, "SHOW default_transaction_read_only").await,
        "on"
    );
    session
        .execute("DROP TABLE IF EXISTS conn3a_encoded", &[])
        .await
        .expect_err("a LATIN1 session is read-only");
    session
        .execute("CREATE TABLE conn3a_encoded (v text)", &[])
        .await
        .expect_err("a LATIN1 session is read-only");
    session.close().await;
    // The same profile with UTF-8 (or no encoding) stays writable.
    p.options.encoding = Some("UTF8".into());
    let utf8 = open(&p, password.as_deref()).await;
    assert_eq!(
        text(&utf8, "SHOW default_transaction_read_only").await,
        "off"
    );
    utf8.close().await;

    // A bad setting fails the connect with the server's message.
    p.options.pg_params = Some("conn3a_no_such_setting=1".into());
    p.options.encoding = None;
    let err = connect(ConnectTarget {
        profile: &p,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .err()
    .expect("an unknown setting is refused");
    assert!(err.to_string().contains("conn3a_no_such_setting"), "{err}");
}

#[tokio::test]
async fn conn3a_mysql_encoding_and_local_infile() {
    let Ok(url) = std::env::var("TABLER_TEST_MYSQL_URL") else {
        println!("skipped: TABLER_TEST_MYSQL_URL unset");
        return;
    };
    let (mut p, password) = profile_from_url(&url, Driver::Mysql);
    p.options.encoding = Some("latin1".into());
    let latin1 = open(&p, password.as_deref()).await;
    assert_eq!(
        text(&latin1, "SELECT @@character_set_client").await,
        "latin1"
    );
    assert_eq!(
        text(&latin1, "SELECT @@character_set_results").await,
        "latin1"
    );
    // Tabler reads and writes UTF-8 only: a session with another encoding is read-only, so an
    // edit cannot be stored mis-encoded (the server refuses it).
    assert_eq!(
        text(&latin1, "SELECT @@session.transaction_read_only").await,
        "1"
    );
    latin1
        .execute("CREATE TABLE conn3a_encoded (v VARCHAR(10))", &[])
        .await
        .expect_err("a latin1 session is read-only");
    latin1.close().await;
    // UTF-8 (or no encoding) stays writable.
    p.options.encoding = Some("utf8mb4".into());
    let utf8 = open(&p, password.as_deref()).await;
    assert_eq!(
        text(&utf8, "SELECT @@session.transaction_read_only").await,
        "0"
    );
    utf8.close().await;

    p.options.encoding = None;
    p.options.mysql_local_infile = Some(true);
    let session = open(&p, password.as_deref()).await;

    // LOAD DATA LOCAL INFILE: only when the server allows it (local_infile is off by default
    // since MySQL 8; the test never changes server settings).
    let server_allows = text(&session, "SELECT @@GLOBAL.local_infile").await == "1";
    if server_allows {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("conn3a.csv");
        std::fs::write(&file, "1,a\n2,b\n").unwrap();
        let file = std::fs::canonicalize(&file).unwrap();
        session
            .execute("DROP TABLE IF EXISTS conn3a_infile", &[])
            .await
            .unwrap();
        session
            .execute("CREATE TABLE conn3a_infile (id INT, v VARCHAR(10))", &[])
            .await
            .unwrap();
        let load = format!(
            "LOAD DATA LOCAL INFILE '{}' INTO TABLE conn3a_infile FIELDS TERMINATED BY ','",
            file.display()
        );
        // Not allowed yet, and no approver in tests: refused.
        assert!(session.execute(&load, &[]).await.is_err());
        // Approved for this connection (its profile name) only.
        tabler_lib::local_infile::allow("conn3a", [file.clone()]);
        session.execute(&load, &[]).await.unwrap();
        assert_eq!(
            text(&session, "SELECT COUNT(*) FROM conn3a_infile").await,
            "2"
        );
        session
            .execute("DROP TABLE conn3a_infile", &[])
            .await
            .unwrap();
    } else {
        println!("LOAD DATA LOCAL INFILE part skipped: the server has local_infile = OFF");
    }
    session.close().await;
}
