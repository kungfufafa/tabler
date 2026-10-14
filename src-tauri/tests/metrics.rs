//! Metrics Board object queries against real servers: `{{variables}}` bound per dialect, the
//! read-only rule, the row cap. Gated per driver on `TABLER_TEST_PG_URL`, `TABLER_TEST_MYSQL_URL`
//! and `TABLER_TEST_SQLITE_PATH` (a skip notice when unset). Only the write-timeout tests create
//! anything: a table named `mqh_write_timeout` on the PostgreSQL / MySQL server (dropped again)
//! and a table in the copy of the SQLite fixture.

#![allow(clippy::result_large_err)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tabler_lib::drivers::{CancelSlot, CollectSink, ConnectTarget, DbSession, connect};
use tabler_lib::metrics::{self, MAX_ROWS};
use tabler_lib::types::metrics::{MetricsQuery, MetricsResult};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::value::Value;

fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn base_profile(driver: Driver) -> ConnectionProfile {
    ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "metrics-test".into(),
        color: None,
        group: None,
        driver,
        host: None,
        port: None,
        user: None,
        database: None,
        ssl: SslMode::Disable,
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions {
            connect_timeout_secs: Some(10),
            ..ProfileOptions::default()
        },
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    }
}

/// `scheme://user[:pass]@host[:port]/db` as a profile plus the password.
fn profile_from_url(driver: Driver, url: &str) -> (ConnectionProfile, Option<String>) {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let (creds, hostdb) = match rest.rsplit_once('@') {
        Some((c, h)) => (Some(c), h),
        None => (None, rest),
    };
    let (user, pass) = match creds {
        Some(c) => match c.split_once(':') {
            Some((u, p)) => (Some(pct_decode(u)), Some(pct_decode(p))),
            None => (Some(pct_decode(c)), None),
        },
        None => (None, None),
    };
    let (hostport, db) = match hostdb.split_once('/') {
        Some((h, d)) => (
            h,
            Some(d.split('?').next().unwrap_or("").to_string()).filter(|d| !d.is_empty()),
        ),
        None => (hostdb, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (hostport.to_string(), None),
    };
    let mut profile = base_profile(driver);
    profile.host = Some(host);
    profile.port = port;
    profile.user = user;
    profile.database = db;
    if driver == Driver::Postgres {
        profile.ssl = SslMode::Prefer;
    }
    (profile, pass)
}

async fn open_server(driver: Driver, env: &str) -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var(env) else {
        println!("skipped: {env} unset");
        return None;
    };
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let (profile, password) = profile_from_url(driver, &url);
    Some(
        connect(ConnectTarget {
            profile: &profile,
            password: password.as_deref(),
            host_override: None,
        })
        .await
        .expect("connect"),
    )
}

async fn open_sqlite() -> Option<(Arc<dyn DbSession>, tempfile::TempDir)> {
    const ENV: &str = "TABLER_TEST_SQLITE_PATH";
    let Ok(rel) = std::env::var(ENV) else {
        println!("skipped: {ENV} unset");
        return None;
    };
    let fixture = {
        let p = PathBuf::from(&rel);
        if p.is_absolute() {
            p
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(p)
        }
    };
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("sample.sqlite");
    std::fs::copy(&fixture, &copy).unwrap_or_else(|e| panic!("copy {}: {e}", fixture.display()));
    let mut profile = base_profile(Driver::Sqlite);
    profile.sqlite_path = Some(copy.to_string_lossy().into_owned());
    let session = connect(ConnectTarget {
        profile: &profile,
        password: None,
        host_override: None,
    })
    .await
    .expect("open sqlite");
    Some((session, dir))
}

fn query(parts: &[&str], values: Vec<Value>) -> MetricsQuery {
    MetricsQuery {
        parts: parts.iter().map(|s| s.to_string()).collect(),
        values,
        allow_writes: false,
        max_rows: 0,
        timeout_secs: None,
    }
}

async fn run(session: &dyn DbSession, q: &MetricsQuery) -> MetricsResult {
    let prepared = metrics::prepare(session.dialect(), q).expect("prepare");
    metrics::run(
        session,
        &prepared,
        metrics::row_cap(q.max_rows),
        CancelSlot::default(),
    )
    .await
    .expect("run")
}

#[tokio::test]
async fn postgres_binds_literals_typed_by_context() {
    let Some(pg) = open_pg().await else { return };
    // A text value against an integer: the literal takes the column's type. numeric decodes.
    let r = run(
        pg.as_ref(),
        &query(
            &[
                "SELECT x AS n, (x * 1.5)::numeric AS amount FROM generate_series(1, 10) x WHERE x > ",
                " AND ",
                " IS NOT NULL ORDER BY x",
            ],
            vec![Value::Text("7".into()), Value::Text("it's \\ fine".into())],
        ),
    )
    .await;
    assert_eq!(r.rows.len(), 3);
    assert_eq!(r.rows[0][0], Value::Int(8));
    assert_eq!(r.rows[0][1], Value::Decimal("12.0".into()));
    assert!(!r.truncated);
    // uuid and date contexts.
    let r = run(
        pg.as_ref(),
        &query(
            &[
                "SELECT ",
                "::date + 1 AS d, gen_random_uuid() <> ",
                " AS other",
            ],
            vec![
                Value::Date("2026-01-31".into()),
                Value::Text("00000000-0000-0000-0000-000000000000".into()),
            ],
        ),
    )
    .await;
    assert_eq!(r.rows[0][0], Value::Date("2026-02-01".into()));
    assert_eq!(r.rows[0][1], Value::Bool(true));
}

#[tokio::test]
async fn postgres_caps_rows_and_refuses_writes() {
    let Some(pg) = open_pg().await else { return };
    let r = run(
        pg.as_ref(),
        &query(&["SELECT generate_series(1, 20000) AS n"], vec![]),
    )
    .await;
    assert_eq!(r.rows.len(), MAX_ROWS);
    assert!(r.truncated);
    let write = query(&["CREATE TABLE metrics_never (id int)"], vec![]);
    assert!(metrics::prepare(pg.dialect(), &write).is_err());
}

async fn open_pg() -> Option<Arc<dyn DbSession>> {
    open_server(Driver::Postgres, "TABLER_TEST_PG_URL").await
}

#[tokio::test]
async fn mysql_binds_parameters() {
    let Some(my) = open_server(Driver::Mysql, "TABLER_TEST_MYSQL_URL").await else {
        return;
    };
    let r = run(
        my.as_ref(),
        &query(
            &["SELECT ", " + 1 AS n, CONCAT('%', ", ", '%') AS pattern"],
            vec![Value::Int(41), Value::Text("o'k".into())],
        ),
    )
    .await;
    assert_eq!(r.rows[0][0].as_text().as_deref(), Some("42"));
    assert_eq!(r.rows[0][1], Value::Text("%o'k%".into()));
    let write = query(&["DELETE FROM metrics_never"], vec![]);
    assert!(metrics::prepare(my.dialect(), &write).is_err());
}

#[tokio::test]
async fn sqlite_binds_numbered_parameters() {
    let Some((lite, _dir)) = open_sqlite().await else {
        return;
    };
    let r = run(
        lite.as_ref(),
        &query(
            &["SELECT ", " * 2 AS n, ", " AS t, ", " + ", " AS sum"],
            vec![
                Value::Int(21),
                Value::Text("x".into()),
                Value::Int(1),
                Value::Int(2),
            ],
        ),
    )
    .await;
    assert_eq!(r.rows[0][0], Value::Int(42));
    assert_eq!(r.rows[0][1], Value::Text("x".into()));
    assert_eq!(r.rows[0][2], Value::Int(3));
    let write = query(&["DELETE FROM metrics_never"], vec![]);
    assert!(metrics::prepare(lite.dialect(), &write).is_err());
}

/// Runs `q` under a one second limit; returns the error and how long it took.
async fn run_limited(session: &dyn DbSession, q: &MetricsQuery) -> (String, Duration) {
    let prepared = metrics::prepare(session.dialect(), q).expect("prepare");
    let started = Instant::now();
    let err = metrics::run_with_timeout(
        session,
        &prepared,
        metrics::row_cap(q.max_rows),
        CancelSlot::default(),
        Duration::from_secs(1),
    )
    .await
    .expect_err("the query outlives its timeout");
    (err.to_string(), started.elapsed())
}

#[tokio::test]
async fn postgres_cancels_a_read_past_its_timeout() {
    let Some(pg) = open_pg().await else { return };
    // pg_sleep is refused unless the object allows data changes: this object does.
    let slow = MetricsQuery {
        allow_writes: true,
        ..query(&["SELECT pg_sleep(60)"], vec![])
    };
    let (message, took) = run_limited(pg.as_ref(), &slow).await;
    assert_eq!(message, "Query timed out after 1 s");
    assert!(took < Duration::from_secs(20), "{took:?}");
    // The worker connection is usable again straight away.
    let r = run(pg.as_ref(), &query(&["SELECT 1 AS one"], vec![])).await;
    assert_eq!(r.rows[0][0], Value::Int(1));
}

#[tokio::test]
async fn postgres_refuses_functions_that_change_the_server() {
    let Some(pg) = open_pg().await else { return };
    for sql in [
        "SELECT pg_sleep(0)",
        "SELECT pg_advisory_lock(7770001)",
        "SELECT pg_try_advisory_lock_shared(7770002)",
        "SELECT pg_try_advisory_xact_lock_shared(7770003)",
        "SELECT pg_advisory_unlock_shared(7770004)",
        "SELECT pg_terminate_backend(pg_backend_pid())",
        "SELECT setval('metrics_never_seq', 1)",
    ] {
        let err = metrics::prepare(pg.dialect(), &query(&[sql], vec![])).unwrap_err();
        assert_eq!(err.to_string(), metrics::SIDE_EFFECT_REFUSED, "{sql}");
    }
}

/// A call written as a PostgreSQL Unicode-escaped identifier is still a call: the server runs
/// `U&"pg_s\006ceep"(0)` as `pg_sleep(0)` (so the object must not run it without "Allow data
/// changes"), and the same with the escape character set by a `UESCAPE` clause.
#[tokio::test]
async fn postgres_refuses_a_call_written_as_a_unicode_escaped_name() {
    let Some(pg) = open_pg().await else { return };
    let plain = r#"SELECT U&"pg_s\006ceep"(0) AS slept"#;
    let sleeps = [
        plain,
        r#"SELECT U&"pg_s!006ceep" UESCAPE '!'(0) AS slept"#,
        r#"SELECT U&"pg_s!006ceep" UESCAPE $$!$$(0) AS slept"#,
        r#"SELECT U&"pg_s!006ceep" UESCAPE E'!'(0) AS slept"#,
        r#"SELECT pg_catalog.U&"pg_s\006ceep"(0) AS slept"#,
    ];
    for sql in sleeps {
        // The server accepts the call and runs it ...
        let allowed = MetricsQuery {
            allow_writes: true,
            ..query(&[sql], vec![])
        };
        let prepared = metrics::prepare(pg.dialect(), &allowed).expect("prepare");
        let ran = metrics::run(
            pg.as_ref(),
            &prepared,
            metrics::row_cap(allowed.max_rows),
            CancelSlot::default(),
        )
        .await;
        ran.unwrap_or_else(|e| panic!("{sql}: {e}"));
        // ... so the object does not run it unless it may change data.
        assert!(
            metrics::prepare(pg.dialect(), &query(&[sql], vec![])).is_err(),
            "{sql}"
        );
    }
    // The server returns the one row for the plain escape.
    let allowed = MetricsQuery {
        allow_writes: true,
        ..query(&[plain], vec![])
    };
    assert_eq!(run(pg.as_ref(), &allowed).await.rows.len(), 1);
    for sql in [
        r#"SELECT U&"set\0076al"('metrics_never_seq', 1)"#,
        r#"SELECT U&"pg_terminate_backend"(pg_backend_pid())"#,
        r#"SELECT U&"pg_advisory_l\006fck"(7770005)"#,
    ] {
        let err = metrics::prepare(pg.dialect(), &query(&[sql], vec![])).unwrap_err();
        assert_eq!(err.to_string(), metrics::SIDE_EFFECT_REFUSED, "{sql}");
    }
}

/// PostgreSQL ends a `--` comment at a lone CR and reads every non-ASCII character as part of a
/// name, so `€$$` opens no dollar quote: the server runs the call that follows, and the object
/// must not run it without "Allow data changes".
#[tokio::test]
async fn postgres_refuses_a_call_hidden_by_a_carriage_return_or_a_non_ascii_name() {
    let Some(pg) = open_pg().await else { return };
    let sleeps = [
        "SELECT 1 AS one --x\r, pg_sleep(0) AS slept",
        "SELECT 1 AS €$$, pg_sleep(0) AS a$$",
    ];
    for sql in sleeps {
        // The server runs both columns, the call included ...
        let allowed = MetricsQuery {
            allow_writes: true,
            ..query(&[sql], vec![])
        };
        let result = run(pg.as_ref(), &allowed).await;
        assert_eq!(result.columns.len(), 2, "{sql}");
        // ... so a board that may not change data refuses it.
        let err = metrics::prepare(pg.dialect(), &query(&[sql], vec![])).unwrap_err();
        assert_eq!(err.to_string(), metrics::SIDE_EFFECT_REFUSED, "{sql}");
    }
    // The CR ends the comment: `+ 1` is code.
    let sum = MetricsQuery {
        allow_writes: true,
        ..query(&["SELECT 1 --x\r+ 1 AS v"], vec![])
    };
    let result = run(pg.as_ref(), &sum).await;
    assert_eq!(
        format!("{:?}", result.rows[0][0]),
        format!("{:?}", Value::Int(2))
    );
    // A data-changing statement behind a CR is refused, not run.
    for sql in [
        "WITH x AS (-- c\r DELETE FROM fx3_metrics_never RETURNING 1) SELECT count(*) AS n FROM x",
        "SELECT 1 AS €$$; DELETE FROM fx3_metrics_never; SELECT 3 AS a$$",
    ] {
        assert!(
            metrics::prepare(pg.dialect(), &query(&[sql], vec![])).is_err(),
            "{sql}"
        );
    }
}

/// A MySQL executable comment is code to the server, so the closing `*/` and a version number
/// glued to the body of the comment must not hide a call or an `INTO OUTFILE`.
#[tokio::test]
async fn mysql_refuses_a_call_hidden_by_an_executable_comment() {
    let Some(my) = open_server(Driver::Mysql, "TABLER_TEST_MYSQL_URL").await else {
        return;
    };
    for sql in [
        "SELECT SLEEP/*!*/(0) AS slept",
        "SELECT /*!50000 SLEEP*/(0) AS slept",
        "SELECT /*!50000SLEEP*/(0) AS slept",
        "SELECT /*!SLEEP*//**/(0) AS slept",
    ] {
        // The server runs the call (one row, 0) ...
        let allowed = MetricsQuery {
            allow_writes: true,
            ..query(&[sql], vec![])
        };
        let r = run(my.as_ref(), &allowed).await;
        assert_eq!(r.rows.len(), 1, "{sql}");
        assert_eq!(r.rows[0][0].as_text().as_deref(), Some("0"), "{sql}");
        // ... so the object does not run it unless it may change data.
        let err = metrics::prepare(my.dialect(), &query(&[sql], vec![])).unwrap_err();
        assert_eq!(err.to_string(), metrics::SIDE_EFFECT_REFUSED, "{sql}");
    }
    // `/*!50000INTO*/ OUTFILE` is a SELECT ... INTO OUTFILE for the server: it answers with a
    // file error (never a syntax error), and for a read-only object it is refused as a write.
    // The directory does not exist, so no file is ever written.
    let outfile = "SELECT 1 /*!50000INTO*/ OUTFILE '/mqh_no_such_dir/out.txt'";
    let err = metrics::prepare(my.dialect(), &query(&[outfile], vec![])).unwrap_err();
    assert_eq!(err.to_string(), metrics::WRITE_REFUSED);
    let allowed = MetricsQuery {
        allow_writes: true,
        ..query(&[outfile], vec![])
    };
    let prepared = metrics::prepare(my.dialect(), &allowed).expect("prepare");
    let ran = metrics::run(
        my.as_ref(),
        &prepared,
        metrics::row_cap(allowed.max_rows),
        CancelSlot::default(),
    )
    .await
    .expect_err("INTO OUTFILE into a missing directory fails");
    assert!(
        !ran.to_string().contains("error in your SQL syntax"),
        "the server read it as INTO OUTFILE: {ran}"
    );
}

#[tokio::test]
async fn mysql_cancels_a_read_past_its_timeout() {
    let Some(my) = open_server(Driver::Mysql, "TABLER_TEST_MYSQL_URL").await else {
        return;
    };
    let slow = MetricsQuery {
        allow_writes: true,
        ..query(&["SELECT SLEEP(60)"], vec![])
    };
    let (message, took) = run_limited(my.as_ref(), &slow).await;
    assert_eq!(message, "Query timed out after 1 s");
    assert!(took < Duration::from_secs(20), "{took:?}");
    let r = run(my.as_ref(), &query(&["SELECT 1 AS one"], vec![])).await;
    assert_eq!(r.rows[0][0].as_text().as_deref(), Some("1"));
    for sql in ["SELECT GET_LOCK('metrics_never', 1)", "SELECT SLEEP(1)"] {
        let err = metrics::prepare(my.dialect(), &query(&[sql], vec![])).unwrap_err();
        assert_eq!(err.to_string(), metrics::SIDE_EFFECT_REFUSED, "{sql}");
    }
}

#[tokio::test]
async fn sqlite_cancels_a_read_past_its_timeout() {
    let Some((lite, _dir)) = open_sqlite().await else {
        return;
    };
    let endless = query(
        &["WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r) SELECT max(n) FROM r"],
        vec![],
    );
    let (message, took) = run_limited(lite.as_ref(), &endless).await;
    assert_eq!(message, "Query timed out after 1 s");
    assert!(took < Duration::from_secs(20), "{took:?}");
    let r = run(lite.as_ref(), &query(&["SELECT 1 AS one"], vec![])).await;
    assert_eq!(r.rows[0][0], Value::Int(1));
}

// ---- writes have a time limit too --------------------------------------------------------------

const WRITE_TABLE: &str = "mqh_write_timeout";

/// Runs `sql` on the session's editor connection, which keeps its transaction open between runs.
async fn on_editor(session: &dyn DbSession, sql: &str) {
    let mut sink = CollectSink::default();
    session
        .run_statement(sql, 100, &mut sink, CancelSlot::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// A board write that waits for a row lock, which the editor connection's open transaction holds,
/// is cancelled when its limit passes: the object gets the timeout, the write never happens (not
/// even once the lock is released), and the worker connection is usable again.
async fn a_write_blocked_on_a_lock_is_cancelled_at_its_limit(session: &dyn DbSession) {
    let table = WRITE_TABLE;
    for sql in [
        format!("DROP TABLE IF EXISTS {table}"),
        format!("CREATE TABLE {table} (id int PRIMARY KEY, v int)"),
        format!("INSERT INTO {table} (id, v) VALUES (1, 0)"),
    ] {
        session.execute(&sql, &[]).await.expect(&sql);
    }
    on_editor(session, "BEGIN").await;
    on_editor(session, &format!("UPDATE {table} SET v = v WHERE id = 1")).await;

    let write = MetricsQuery {
        allow_writes: true,
        ..query(
            &[&format!("UPDATE {table} SET v = 99 WHERE id = 1")],
            vec![],
        )
    };
    let (message, took) = run_limited(session, &write).await;

    on_editor(session, "ROLLBACK").await;
    // Were the write only abandoned, it would go through now that the lock is free.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let value = run(
        session,
        &query(&[&format!("SELECT v FROM {table} WHERE id = 1")], vec![]),
    )
    .await;
    // The worker connection that ran the write is back in the pool and works.
    let again = run(session, &query(&["SELECT 1 AS one"], vec![])).await;
    session
        .execute(&format!("DROP TABLE {table}"), &[])
        .await
        .expect("drop the table");

    assert_eq!(message, "Query timed out after 1 s");
    assert!(took < Duration::from_secs(20), "{took:?}");
    assert_eq!(value.rows[0][0].as_text().as_deref(), Some("0"));
    assert_eq!(again.rows[0][0].as_text().as_deref(), Some("1"));
}

#[tokio::test]
async fn postgres_cancels_a_write_past_its_timeout() {
    let Some(pg) = open_pg().await else { return };
    a_write_blocked_on_a_lock_is_cancelled_at_its_limit(pg.as_ref()).await;
}

#[tokio::test]
async fn mysql_cancels_a_write_past_its_timeout() {
    let Some(my) = open_server(Driver::Mysql, "TABLER_TEST_MYSQL_URL").await else {
        return;
    };
    a_write_blocked_on_a_lock_is_cancelled_at_its_limit(my.as_ref()).await;
}

#[tokio::test]
async fn sqlite_cancels_a_write_past_its_timeout() {
    let Some((lite, _dir)) = open_sqlite().await else {
        return;
    };
    lite.execute(
        &format!("CREATE TABLE {WRITE_TABLE} (id INTEGER PRIMARY KEY, v INTEGER)"),
        &[],
    )
    .await
    .expect("create the table");
    // An INSERT whose source never ends, so it never gets to insert.
    let endless = MetricsQuery {
        allow_writes: true,
        ..query(
            &[&format!(
                "INSERT INTO {WRITE_TABLE} (v) WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r) SELECT max(n) FROM r"
            )],
            vec![],
        )
    };
    let (message, took) = run_limited(lite.as_ref(), &endless).await;
    assert_eq!(message, "Query timed out after 1 s");
    assert!(took < Duration::from_secs(20), "{took:?}");
    let count = run(
        lite.as_ref(),
        &query(&[&format!("SELECT count(*) FROM {WRITE_TABLE}")], vec![]),
    )
    .await;
    assert_eq!(count.rows[0][0], Value::Int(0));
    // The worker connection is usable again.
    let r = run(lite.as_ref(), &query(&["SELECT 1 AS one"], vec![])).await;
    assert_eq!(r.rows[0][0], Value::Int(1));
}
