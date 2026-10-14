//! The statements the app runs for its own purposes that do not go through the SQL editor's
//! logging (the Explain command and the transaction it wraps around an Analyze, a SQL dump
//! import) are recorded in the session's trace, so the console log shows what really ran; the SQL
//! editor's own runs are not (the renderer logs those with the text the user submitted, and a
//! second record would log every editor statement twice).
//!
//! SQLite needs no server; PostgreSQL is gated on `TABLER_TEST_PG_URL` (a skip notice, then a
//! pass, when unset). Objects are named `fx3_trace_<random>` and dropped at the end.

use std::path::Path;
use std::sync::Arc;

use tabler_lib::commands::query::{explain_inner, run_sql_inner};
use tabler_lib::drivers::{CancelSlot, ConnectTarget, DbSession, connect};
use tabler_lib::import::run_sql_import;
use tabler_lib::types::browse::{TraceCategory, TracedSql};
use tabler_lib::types::import::SqlImportRequest;
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::query::{ExplainMode, RunOptions};
use tokio_util::sync::CancellationToken;

fn profile(driver: Driver) -> ConnectionProfile {
    ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "trace-runs-test".into(),
        color: None,
        group: None,
        driver,
        host: None,
        port: None,
        user: None,
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

async fn sqlite_session(path: &Path) -> Arc<dyn DbSession> {
    let mut profile = profile(Driver::Sqlite);
    profile.ssl = SslMode::Disable;
    profile.sqlite_path = Some(path.to_string_lossy().into_owned());
    connect(ConnectTarget {
        profile: &profile,
        password: None,
        host_override: None,
    })
    .await
    .expect("connect")
}

async fn postgres_session() -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return None;
    };
    let rest = url.split_once("://").map(|(_, r)| r).expect("a URL");
    let (auth, hostdb) = rest.rsplit_once('@').expect("user@host");
    let (user, password) = match auth.split_once(':') {
        Some((u, p)) => (u.to_string(), Some(p.to_string())),
        None => (auth.to_string(), None),
    };
    let (hostport, db) = hostdb.split_once('/').expect("host/db");
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (hostport.to_string(), None),
    };
    let mut profile = profile(Driver::Postgres);
    profile.host = Some(host);
    profile.port = port;
    profile.user = Some(user);
    profile.database = Some(db.split('?').next().unwrap_or("").to_string());
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
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

fn texts(trace: &[TracedSql]) -> Vec<&str> {
    trace.iter().map(|t| t.sql.as_str()).collect()
}

async fn editor_run(s: &dyn DbSession, sql: &str, options: &RunOptions) {
    let mut emit = |_| {};
    let results = run_sql_inner(s, sql, options, &CancelSlot::default(), &mut emit).await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:#?}");
}

#[tokio::test]
async fn a_sql_dump_import_records_the_statements_it_ran() {
    let dir = tempfile::tempdir().unwrap();
    let s = sqlite_session(&dir.path().join("dump.sqlite")).await;
    let dump = dir.path().join("fx3_dump.sql");
    std::fs::write(
        &dump,
        "CREATE TABLE fx3_trace_t (id INTEGER PRIMARY KEY, note TEXT);\n\
         INSERT INTO fx3_trace_t VALUES (1, 'one');\n\
         INSERT INTO fx3_trace_t VALUES (1, 'duplicate');\n\
         INSERT INTO fx3_trace_t VALUES (2, 'two');\n",
    )
    .unwrap();
    s.drain_trace();

    let report = run_sql_import(
        s.as_ref(),
        &SqlImportRequest {
            import_id: "fx3-trace".into(),
            path: dump.to_string_lossy().into_owned(),
            stop_on_error: false,
        },
        CancelSlot::default(),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!((report.executed, report.failed), (3, 1));

    let trace = s.drain_trace();
    assert_eq!(trace.len(), 4, "{:#?}", texts(&trace));
    assert_eq!(
        trace[0].sql,
        "CREATE TABLE fx3_trace_t (id INTEGER PRIMARY KEY, note TEXT)"
    );
    assert_eq!(trace[0].category, TraceCategory::Structure);
    assert_eq!(trace[1].sql, "INSERT INTO fx3_trace_t VALUES (1, 'one')");
    assert_eq!(
        (
            trace[1].category,
            trace[1].affected,
            trace[1].error.as_deref()
        ),
        (TraceCategory::Data, Some(1), None)
    );
    assert!(trace[2].error.as_deref().unwrap().contains("UNIQUE"));
    assert_eq!(trace[3].affected, Some(1));
    assert!(s.drain_trace().is_empty());
}

#[tokio::test]
async fn the_editor_path_records_nothing_but_the_explain_command_does() {
    let dir = tempfile::tempdir().unwrap();
    let s = sqlite_session(&dir.path().join("explain.sqlite")).await;
    editor_run(
        s.as_ref(),
        "CREATE TABLE fx3_trace_t (a int); INSERT INTO fx3_trace_t VALUES (1)",
        &RunOptions::default(),
    )
    .await;
    editor_run(
        s.as_ref(),
        "SELECT * FROM fx3_trace_t",
        &RunOptions {
            explain: Some(ExplainMode::Plan),
            ..RunOptions::default()
        },
    )
    .await;
    assert!(
        s.drain_trace().is_empty(),
        "the renderer logs the editor's runs itself"
    );

    let plan = explain_inner(
        s.as_ref(),
        "SELECT * FROM fx3_trace_t WHERE a = 1;",
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert!(plan.text.is_some());
    let trace = s.drain_trace();
    assert_eq!(
        texts(&trace),
        ["EXPLAIN QUERY PLAN SELECT * FROM fx3_trace_t WHERE a = 1"],
        "the statement that really ran, not the one the user typed"
    );
    assert_eq!(trace[0].category, TraceCategory::Data);
    assert!(trace[0].rows.is_some() && trace[0].error.is_none());
}

#[tokio::test]
async fn postgres_explain_analyze_records_its_transaction() {
    let Some(s) = postgres_session().await else {
        return;
    };
    let table = format!(
        "fx3_trace_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    s.execute(&format!("CREATE TABLE {table} (a int)"), &[])
        .await
        .unwrap();
    s.execute(&format!("INSERT INTO {table} VALUES (1)"), &[])
        .await
        .unwrap();
    s.drain_trace();

    let sql = format!("UPDATE {table} SET a = 2");
    explain_inner(
        s.as_ref(),
        &sql,
        ExplainMode::Analyze,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    let trace = s.drain_trace();
    let shown = texts(&trace);
    // Opened, run, rolled back: what really ran, in order.
    let begin = shown
        .iter()
        .position(|t| *t == "BEGIN")
        .unwrap_or_else(|| panic!("{shown:#?}"));
    let explain = shown
        .iter()
        .position(|t| t.starts_with("EXPLAIN (") && t.ends_with(&sql))
        .unwrap_or_else(|| panic!("{shown:#?}"));
    let rollback = shown
        .iter()
        .position(|t| *t == "ROLLBACK")
        .unwrap_or_else(|| panic!("{shown:#?}"));
    assert!(begin < explain && explain < rollback, "{shown:#?}");
    assert!(trace.iter().all(|t| t.error.is_none()), "{trace:#?}");

    // The rollback happened: the row is untouched.
    let a = s
        .query(&format!("SELECT a::text FROM {table}"), &[])
        .await
        .unwrap();
    assert_eq!(a.rows[0][0].as_text().as_deref(), Some("1"));

    // The editor's own EXPLAIN ANALYZE is not recorded.
    s.drain_trace();
    editor_run(
        s.as_ref(),
        &sql,
        &RunOptions {
            explain: Some(ExplainMode::Analyze),
            ..RunOptions::default()
        },
    )
    .await;
    assert!(s.drain_trace().is_empty());

    s.execute(&format!("DROP TABLE {table}"), &[])
        .await
        .unwrap();
    s.close().await;
}
