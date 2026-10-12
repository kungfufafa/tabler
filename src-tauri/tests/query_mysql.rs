//! SQL editor execution against MySQL / MariaDB. Gated on `TABLER_TEST_MYSQL_URL`
//! (`mysql://user[:pass]@host:port/db`); every test prints a skip notice and passes when unset.

// `AppError` carries the full server diagnostic (same allowance as the library).
#![allow(clippy::result_large_err)]

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tabler_lib::commands::query::{explain_inner, run_sql_inner};
use tabler_lib::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, RowSink, connect, request_cancel,
};
use tabler_lib::error::AppError;
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::query::{
    ExplainMode, QueryEvent, ResultColumn, RunOptions, StatementKind, StatementResult,
};
use tabler_lib::types::value::Value;

const ENV: &str = "TABLER_TEST_MYSQL_URL";

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

/// Parses `mysql://user[:pass]@host:port/db` into a profile plus the password.
fn profile_from_url(url: &str) -> (ConnectionProfile, Option<String>) {
    let rest = url
        .strip_prefix("mysql://")
        .unwrap_or_else(|| panic!("{ENV} must start with mysql://"));
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
        Some((h, d)) => (h, Some(d.to_string()).filter(|d| !d.is_empty())),
        None => (hostdb, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (hostport.to_string(), None),
    };
    let profile = ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "mysql-test".into(),
        color: None,
        group: None,
        driver: Driver::Mysql,
        host: Some(host),
        port,
        user,
        database: db,
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
    };
    (profile, pass)
}

async fn open() -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var(ENV) else {
        println!("skipped: {ENV} unset");
        return None;
    };
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let (profile, password) = profile_from_url(&url);
    let session = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap_or_else(|e| panic!("connect failed: {e}"));
    Some(session)
}

fn suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

async fn run(
    session: &dyn DbSession,
    sql: &str,
    options: RunOptions,
) -> (Vec<StatementResult>, Vec<QueryEvent>) {
    let slot = CancelSlot::default();
    let mut events = Vec::new();
    let results = {
        let mut emit = |e: QueryEvent| events.push(e);
        run_sql_inner(session, sql, &options, &slot, &mut emit).await
    };
    (results, events)
}

fn rows_in(events: &[QueryEvent], index: usize, set: u32) -> Vec<Vec<Value>> {
    events
        .iter()
        .filter_map(|e| match e {
            QueryEvent::Rows {
                index: i,
                set: s,
                rows,
            } if *i == index && *s == set => Some(rows.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

async fn single(session: &dyn DbSession, sql: &str) -> Result<Vec<Vec<Value>>, AppError> {
    let mut sink = CollectSink::default();
    session
        .run_statement(sql, 10_000, &mut sink, CancelSlot::default())
        .await?;
    Ok(sink
        .sets
        .into_iter()
        .next()
        .map(|s| s.rows)
        .unwrap_or_default())
}

async fn wait_armed(slot: &CancelSlot) {
    for _ in 0..500 {
        if slot.is_armed() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the statement never started");
}

#[derive(Default)]
struct CountSink {
    rows: u64,
    stop_after: Option<u64>,
}

impl RowSink for CountSink {
    fn begin_set(&mut self, _columns: Vec<ResultColumn>) {}
    fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        self.rows += rows.len() as u64;
        match self.stop_after {
            Some(n) if self.rows >= n => ControlFlow::Break(()),
            _ => ControlFlow::Continue(()),
        }
    }
}

#[tokio::test]
async fn runs_multiple_statements() {
    let Some(session) = open().await else {
        return;
    };
    let (results, events) = run(
        session.as_ref(),
        "select 1; select 'two' as t; update users set name = name where 1 = 0;",
        RunOptions::default(),
    )
    .await;
    assert_eq!(results.len(), 3, "{results:?}");
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(results[0].kind, StatementKind::Rows);
    assert_eq!(results[1].kind, StatementKind::Rows);
    assert_eq!(results[2].kind, StatementKind::Affected);
    assert_eq!(results[2].affected, Some(0));
    assert_eq!(rows_in(&events, 0, 0), vec![vec![Value::Int(1)]]);
    assert_eq!(
        rows_in(&events, 1, 0),
        vec![vec![Value::Text("two".into())]]
    );

    // Warnings come back as notices.
    let mut sink = CollectSink::default();
    let outcome = session
        .run_statement(
            "select cast('12abc' as signed) as n",
            10,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert!(
        outcome.notices.iter().any(|n| n.contains("1292")),
        "{:?}",
        outcome.notices
    );
}

#[tokio::test]
async fn reports_syntax_errors_with_position() {
    let Some(session) = open().await else {
        return;
    };
    let (results, _) = run(
        session.as_ref(),
        "select 1;\nselect *\n  frm users;\nselect 3;",
        RunOptions::default(),
    )
    .await;
    assert_eq!(results.len(), 2);
    let err = results[1].error.as_ref().expect("error");
    assert_eq!(err.vendor_code, Some(1064));
    assert_eq!(err.statement_index, Some(1));
    let pos = err.position.as_ref().expect("position");
    // "near 'frm users' at line 2" of the statement = line 3, column 3 of the editor text.
    assert_eq!((pos.line, pos.column), (3, 3), "{err:?}");
}

#[tokio::test]
async fn returns_every_result_set_of_a_call() {
    let Some(session) = open().await else {
        return;
    };
    let name = format!("tabler_p_{}", suffix());
    let sql = format!(
        "DELIMITER $$\n\
         CREATE PROCEDURE {name}()\n\
         BEGIN\n\
           SELECT 1 AS a;\n\
           SELECT 2 AS b, 3 AS c UNION ALL SELECT 4, 5;\n\
         END$$\n\
         DELIMITER ;\n\
         CALL {name}();\n\
         DROP PROCEDURE {name};"
    );
    let (results, events) = run(session.as_ref(), &sql, RunOptions::default()).await;
    // Clean up even when assertions below fail.
    let _ = single(
        session.as_ref(),
        &format!("DROP PROCEDURE IF EXISTS {name}"),
    )
    .await;
    assert_eq!(results.len(), 3, "{results:?}");
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    let call = &results[1];
    assert_eq!(call.kind, StatementKind::Rows);
    assert_eq!(call.result_sets, 2);
    assert_eq!(call.row_count, 3);
    assert_eq!(call.columns.len(), 1);
    assert_eq!(rows_in(&events, 1, 0), vec![vec![Value::Int(1)]]);
    assert_eq!(
        rows_in(&events, 1, 1),
        vec![
            vec![Value::Int(2), Value::Int(3)],
            vec![Value::Int(4), Value::Int(5)]
        ]
    );
    let column_sets: Vec<(u32, usize)> = events
        .iter()
        .filter_map(|e| match e {
            QueryEvent::Columns {
                index: 1,
                set,
                columns,
            } => Some((*set, columns.len())),
            _ => None,
        })
        .collect();
    assert_eq!(column_sets, [(0, 1), (1, 2)]);
}

#[tokio::test]
async fn cancels_a_running_statement() {
    let Some(session) = open().await else {
        return;
    };
    for sql in [
        "select sleep(30)",
        "select count(*) from events_big where sleep(30) = 0",
    ] {
        let slot = CancelSlot::default();
        let task = {
            let session = session.clone();
            let slot = slot.clone();
            let sql = sql.to_string();
            tokio::spawn(async move {
                let mut events = Vec::new();
                let mut emit = |e: QueryEvent| events.push(e);
                run_sql_inner(
                    session.as_ref(),
                    &format!("{sql}; select 2"),
                    &RunOptions::default(),
                    &slot,
                    &mut emit,
                )
                .await
            })
        };
        wait_armed(&slot).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let cancelled_at = Instant::now();
        let handle = request_cancel(&slot).expect("a live handle");
        session.cancel(handle).await.expect("cancel");
        let results = task.await.unwrap();
        assert!(
            cancelled_at.elapsed() < Duration::from_secs(2),
            "{sql}: took {:?}",
            cancelled_at.elapsed()
        );
        assert_eq!(results.len(), 1, "{sql}: {results:?}");
        assert_eq!(
            results[0].error.as_ref().unwrap().code,
            "CANCELLED",
            "{sql}"
        );
        // The editor connection is still usable, and no stale kill hits the next statement.
        assert_eq!(
            single(session.as_ref(), "select count(*) from events_big")
                .await
                .unwrap(),
            vec![vec![Value::Int(100_000)]]
        );
    }
}

#[tokio::test]
async fn caps_rows_and_keeps_the_editor_usable() {
    let Some(session) = open().await else {
        return;
    };
    let options = RunOptions {
        max_rows: 1000,
        ..RunOptions::default()
    };
    let started = Instant::now();
    let (results, events) = run(
        session.as_ref(),
        "select * from events_big a cross join (select 1 union all select 2) b",
        options,
    )
    .await;
    let r = &results[0];
    assert!(r.error.is_none(), "{r:?}");
    assert!(r.truncated);
    assert_eq!(r.row_count, 1000);
    assert_eq!(rows_in(&events, 0, 0).len(), 1000);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        single(session.as_ref(), "select count(*) from events_big")
            .await
            .unwrap(),
        vec![vec![Value::Int(100_000)]]
    );
    assert_eq!(
        single(session.as_ref(), "select 42").await.unwrap(),
        vec![vec![Value::Int(42)]]
    );
}

#[tokio::test]
async fn tracks_transactions_and_session_state() {
    let Some(session) = open().await else {
        return;
    };
    let table = format!("tabler_tx_{}", suffix());
    assert_eq!(session.in_transaction(), Some(false));
    single(
        session.as_ref(),
        &format!("create temporary table {table} (a int)"),
    )
    .await
    .unwrap();
    single(session.as_ref(), "begin").await.unwrap();
    assert_eq!(session.in_transaction(), Some(true));
    let mut sink = CollectSink::default();
    let outcome = session
        .run_statement(
            &format!("insert into {table} values (1), (2)"),
            10,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(outcome.kind, StatementKind::Affected);
    assert_eq!(outcome.affected, Some(2));
    single(session.as_ref(), "rollback").await.unwrap();
    assert_eq!(session.in_transaction(), Some(false));
    assert_eq!(
        single(session.as_ref(), &format!("select count(*) from {table}"))
            .await
            .unwrap(),
        vec![vec![Value::Int(0)]]
    );
    single(session.as_ref(), &format!("drop temporary table {table}"))
        .await
        .unwrap();
}

#[tokio::test]
async fn streams_queries_on_workers() {
    let Some(session) = open().await else {
        return;
    };
    let mut sink = CountSink::default();
    let n = session
        .stream_query(
            "select * from events_big",
            &[],
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(n, 100_000);
    assert_eq!(sink.rows, 100_000);

    let mut sink = CountSink::default();
    let n = session
        .stream_query(
            "select id from events_big where id <= ?",
            &[Value::Int(321)],
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(n, 321);

    let mut sink = CountSink {
        stop_after: Some(1),
        ..CountSink::default()
    };
    let n = session
        .stream_query(
            "select * from events_big",
            &[],
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert!(n <= 500, "{n}");
    for _ in 0..5 {
        assert_eq!(
            session
                .query("select count(*) from events_big", &[])
                .await
                .unwrap()
                .rows,
            vec![vec![Value::Int(100_000)]]
        );
    }
}

#[tokio::test]
async fn explains_statements() {
    let Some(session) = open().await else {
        return;
    };
    let plan = explain_inner(
        session.as_ref(),
        "select * from users where id = 1",
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    let text = plan.text.expect("text plan");
    assert!(!text.is_empty());
    if !session.is_mariadb() {
        assert!(text.contains("->"), "{text}");
    }
    let plan = explain_inner(
        session.as_ref(),
        "select count(*) from users",
        ExplainMode::Analyze,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert!(plan.text.is_some());
}

fn count_of(rows: &[Vec<Value>]) -> i64 {
    match rows.first().and_then(|r| r.first()) {
        Some(Value::Int(n)) => *n,
        other => panic!("unexpected {other:?}"),
    }
}

/// Stopping at the row cap must not abort the rest of a procedure or batch: their later writes
/// still run (no KILL), later result sets still arrive.
#[tokio::test]
async fn row_cap_does_not_abort_procedures_or_batches() {
    let Some(session) = open().await else {
        return;
    };
    let s = suffix();
    let (log, proc_name, func) = (
        format!("tabler_log_{s}"),
        format!("tabler_p_{s}"),
        format!("tabler_f_{s}"),
    );
    let setup = format!(
        "CREATE TABLE {log} (id INT) ENGINE=InnoDB;\n\
         DELIMITER $$\n\
         CREATE PROCEDURE {proc_name}()\n\
         BEGIN\n\
           SELECT a.*, REPEAT('a', 100) AS pad FROM events_big a;\n\
           INSERT INTO {log} VALUES (1);\n\
           SELECT 7 AS after_insert;\n\
         END$$\n\
         CREATE FUNCTION {func}(x INT) RETURNS INT DETERMINISTIC MODIFIES SQL DATA\n\
         BEGIN\n\
           INSERT INTO {log} VALUES (x);\n\
           RETURN x;\n\
         END$$\n\
         DELIMITER ;"
    );
    let (results, _) = run(session.as_ref(), &setup, RunOptions::default()).await;
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
        let capped = RunOptions {
            max_rows: 100,
            ..RunOptions::default()
        };

        let (results, events) = run(
            session.as_ref(),
            &format!("CALL {proc_name}()"),
            capped.clone(),
        )
        .await;
        let r = &results[0];
        assert!(r.error.is_none(), "{r:?}");
        assert!(r.truncated);
        assert_eq!(r.result_sets, 2, "{r:?}");
        assert_eq!(rows_in(&events, 0, 0).len(), 100);
        assert_eq!(rows_in(&events, 0, 1), vec![vec![Value::Int(7)]]);
        let rows = single(session.as_ref(), &format!("SELECT COUNT(*) FROM {log}"))
            .await
            .unwrap();
        assert_eq!(count_of(&rows), 1, "the procedure's INSERT ran");

        // A custom-DELIMITER batch sent as one statement.
        let (results, _) = run(
            session.as_ref(),
            &format!(
                "DELIMITER $$\nSELECT * FROM events_big; INSERT INTO {log} VALUES (2)$$\nDELIMITER ;"
            ),
            capped.clone(),
        )
        .await;
        assert!(results[0].error.is_none(), "{results:?}");
        assert!(results[0].truncated);
        let rows = single(
            session.as_ref(),
            &format!("SELECT COUNT(*) FROM {log} WHERE id = 2"),
        )
        .await
        .unwrap();
        assert_eq!(count_of(&rows), 1, "the batch's INSERT ran");

        // A read calling a writing function is still killed at the cap, but says so.
        let (results, _) = run(
            session.as_ref(),
            &format!("SELECT {func}(id + 1000) FROM events_big"),
            capped,
        )
        .await;
        let r = &results[0];
        assert!(r.error.is_none(), "{r:?}");
        assert!(r.truncated);
        assert!(
            r.notices.iter().any(|n| n.contains("row limit")),
            "{:?}",
            r.notices
        );
        assert_eq!(
            single(session.as_ref(), "select 42").await.unwrap(),
            vec![vec![Value::Int(42)]]
        );
    }))
    .await;
    for sql in [
        format!("DROP PROCEDURE IF EXISTS {proc_name}"),
        format!("DROP FUNCTION IF EXISTS {func}"),
        format!("DROP TABLE IF EXISTS {log}"),
    ] {
        single(session.as_ref(), &sql).await.unwrap();
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// A KILL that is slow to arrive (pool busy) must not abort the next run's statement.
#[tokio::test]
async fn a_late_kill_does_not_hit_the_next_statement() {
    let Some(session) = open().await else {
        return;
    };
    // Keep the worker pool busy so the KILL has to wait for a connection.
    let blockers: Vec<_> = (0..4)
        .map(|_| {
            let session = session.clone();
            tokio::spawn(async move { session.query("SELECT SLEEP(3)", &[]).await })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let slot = CancelSlot::default();
    let task = {
        let session = session.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut emit = |_e: QueryEvent| {};
            run_sql_inner(
                session.as_ref(),
                "select sleep(1)",
                &RunOptions::default(),
                &slot,
                &mut emit,
            )
            .await
        })
    };
    wait_armed(&slot).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let handle = request_cancel(&slot).expect("a live handle");
    let canceller = {
        let session = session.clone();
        tokio::spawn(async move { session.cancel(handle).await })
    };
    let first = task.await.unwrap();
    assert_eq!(first[0].error.as_ref().unwrap().code, "CANCELLED");
    assert_eq!(slot.cancels_in_flight(), 0, "the run waited for its KILL");

    // The next run (nobody cancels it) must run to its end.
    let started = Instant::now();
    let (results, events) = run(
        session.as_ref(),
        "select sleep(1.5) as s",
        RunOptions::default(),
    )
    .await;
    assert!(results[0].error.is_none(), "{results:?}");
    assert_eq!(rows_in(&events, 0, 0), vec![vec![Value::Int(0)]]);
    assert!(started.elapsed() >= Duration::from_millis(1400));
    canceller.await.unwrap().expect("kill");
    for b in blockers {
        let _ = b.await;
    }
}

/// A server-side drop of the editor connection shows up as `NotConnected` and fails `ping`, so
/// the connection is reported lost instead of every run failing with CONNECT_FAILED.
#[tokio::test]
async fn reports_a_lost_editor_connection() {
    let Some(session) = open().await else {
        return;
    };
    let id = count_of(
        &single(session.as_ref(), "SELECT CONNECTION_ID()")
            .await
            .unwrap(),
    );
    session.ping().await.expect("healthy");
    session
        .execute(&format!("KILL CONNECTION {id}"), &[])
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let err = single(session.as_ref(), "SELECT 1").await.unwrap_err();
    assert!(matches!(err, AppError::NotConnected), "{err:?}");
    let err = single(session.as_ref(), "SELECT 1").await.unwrap_err();
    assert!(matches!(err, AppError::NotConnected), "{err:?}");
    assert!(session.ping().await.is_err(), "ping reports the loss");
}

/// Nothing runs on the editor between the user's statements that would change `ROW_COUNT()`.
#[tokio::test]
async fn warnings_do_not_reset_row_count() {
    let Some(session) = open().await else {
        return;
    };
    let table = format!("tabler_rc_{}", suffix());
    let (results, events) = run(
        session.as_ref(),
        &format!(
            "create temporary table {table} (a int primary key);\n\
             insert ignore into {table} values (1), (1), (2);\n\
             select row_count();\n\
             drop temporary table {table};"
        ),
        RunOptions::default(),
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(results[1].affected, Some(2));
    assert!(
        results[1].notices.iter().any(|n| n.contains("1 warning")),
        "{:?}",
        results[1].notices
    );
    assert_eq!(rows_in(&events, 2, 0), vec![vec![Value::Int(2)]]);
}

/// The error marker points at the offending token even when its text occurs earlier.
#[tokio::test]
async fn positions_errors_on_the_offending_token() {
    let Some(session) = open().await else {
        return;
    };
    for (sql, offset) in [
        ("select id from users limit 10 10", 30),
        ("select 1 from dual where (1) and 1)", 34),
    ] {
        let (results, _) = run(session.as_ref(), sql, RunOptions::default()).await;
        let err = results[0].error.as_ref().expect("syntax error");
        assert_eq!(err.vendor_code, Some(1064));
        assert_eq!(
            err.position.as_ref().unwrap().offset,
            offset,
            "{sql}: {err:?}"
        );
    }
}

/// Closing a session (disconnect, database switch) ends the editor statement instead of waiting
/// for it.
#[tokio::test]
async fn close_does_not_wait_for_the_running_statement() {
    let Some(session) = open().await else {
        return;
    };
    let slot = CancelSlot::default();
    let task = {
        let session = session.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut sink = CollectSink::default();
            session
                .run_statement("select sleep(30)", 10, &mut sink, slot)
                .await
        })
    };
    wait_armed(&slot).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    session.close().await;
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "close took {:?}",
        started.elapsed()
    );
    let result = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("the statement stopped")
        .unwrap();
    assert!(result.is_err(), "{result:?}");
    let err = single(session.as_ref(), "select 1").await.unwrap_err();
    assert!(matches!(err, AppError::NotConnected), "{err:?}");
}

/// Plan-only EXPLAIN must never execute anything: a custom-DELIMITER block holding several
/// statements is refused instead of being wrapped (the server would run the INSERT too).
#[tokio::test]
async fn explain_refuses_a_delimiter_block_with_several_statements() {
    let Some(session) = open().await else {
        return;
    };
    let t = format!("tabler_ex_{}", suffix());
    single(session.as_ref(), &format!("CREATE TABLE {t} (id INT)"))
        .await
        .unwrap();
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        let sql = format!("DELIMITER //\nSELECT * FROM {t}; INSERT INTO {t} VALUES (42) //");
        let err = explain_inner(
            session.as_ref(),
            &sql,
            ExplainMode::Plan,
            &CancelSlot::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)), "{err:?}");
        let (results, _) = run(
            session.as_ref(),
            &sql,
            RunOptions {
                explain: Some(ExplainMode::Plan),
                ..RunOptions::default()
            },
        )
        .await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].error.as_ref().unwrap().code, "INVALID_INPUT");
        let rows = single(session.as_ref(), &format!("SELECT COUNT(*) FROM {t}"))
            .await
            .unwrap();
        assert_eq!(count_of(&rows), 0, "nothing was inserted");
        // A single statement in a DELIMITER block is still explained.
        let plan = explain_inner(
            session.as_ref(),
            &format!("DELIMITER //\nSELECT * FROM {t} //"),
            ExplainMode::Plan,
            &CancelSlot::default(),
        )
        .await
        .unwrap();
        assert!(plan.text.unwrap_or_default().contains(&t));
    }))
    .await;
    single(session.as_ref(), &format!("DROP TABLE IF EXISTS {t}"))
        .await
        .unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// `KILL QUERY` only cuts a procedure's `SLEEP` short; the server completes the CALL and its
/// INSERT is committed. The result must say so, not report a cancelled (failed) statement.
#[tokio::test]
async fn a_call_that_completes_despite_a_cancel_keeps_its_outcome() {
    let Some(session) = open().await else {
        return;
    };
    let s = suffix();
    let (t, p) = (format!("tabler_cl_{s}"), format!("tabler_clp_{s}"));
    single(
        session.as_ref(),
        &format!("CREATE TABLE {t} (id INT) ENGINE=InnoDB"),
    )
    .await
    .unwrap();
    single(
        session.as_ref(),
        &format!(
            "CREATE PROCEDURE {p}() BEGIN INSERT INTO {t} VALUES (1); SELECT SLEEP(3) AS s; END"
        ),
    )
    .await
    .unwrap();
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        let slot = CancelSlot::default();
        let task = {
            let session = session.clone();
            let slot = slot.clone();
            let sql = format!("CALL {p}()");
            tokio::spawn(async move {
                let mut emit = |_e: QueryEvent| {};
                run_sql_inner(
                    session.as_ref(),
                    &sql,
                    &RunOptions::default(),
                    &slot,
                    &mut emit,
                )
                .await
            })
        };
        wait_armed(&slot).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let handle = request_cancel(&slot).expect("a live handle");
        session.cancel(handle).await.expect("cancel");
        let results = task.await.unwrap();
        let r = &results[0];
        assert!(r.error.is_none(), "{r:?}");
        assert!(
            r.notices.iter().any(|n| n.contains("Cancel was requested")),
            "{:?}",
            r.notices
        );
        let rows = single(session.as_ref(), &format!("SELECT COUNT(*) FROM {t}"))
            .await
            .unwrap();
        assert_eq!(count_of(&rows), 1, "the CALL's INSERT is committed");
    }))
    .await;
    for sql in [
        format!("DROP PROCEDURE IF EXISTS {p}"),
        format!("DROP TABLE IF EXISTS {t}"),
    ] {
        single(session.as_ref(), &sql).await.unwrap();
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// DDL with a syntax error fails before MySQL's implicit commit: the transaction stays open.
#[tokio::test]
async fn ddl_with_a_syntax_error_keeps_the_transaction_open() {
    let Some(session) = open().await else {
        return;
    };
    let s = suffix();
    let (t, bad) = (format!("tabler_dt_{s}"), format!("tabler_dtb_{s}"));
    single(
        session.as_ref(),
        &format!("CREATE TABLE {t} (id INT) ENGINE=InnoDB"),
    )
    .await
    .unwrap();
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        single(session.as_ref(), "BEGIN").await.unwrap();
        single(session.as_ref(), &format!("INSERT INTO {t} VALUES (1)"))
            .await
            .unwrap();
        let err = single(session.as_ref(), &format!("CREATE TABLE {bad} (id int,)"))
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                AppError::Db {
                    vendor_code: Some(1064),
                    ..
                }
            ),
            "{err:?}"
        );
        assert_eq!(session.in_transaction(), Some(true));
        single(session.as_ref(), "ROLLBACK").await.unwrap();
        assert_eq!(session.in_transaction(), Some(false));
        let rows = single(session.as_ref(), &format!("SELECT COUNT(*) FROM {t}"))
            .await
            .unwrap();
        assert_eq!(count_of(&rows), 0, "the INSERT was still uncommitted");
    }))
    .await;
    let _ = single(session.as_ref(), "ROLLBACK").await;
    for sql in [
        format!("DROP TABLE IF EXISTS {t}"),
        format!("DROP TABLE IF EXISTS {bad}"),
    ] {
        single(session.as_ref(), &sql).await.unwrap();
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// `UPDATE` inside a string literal does not make a capped read look like a write: it is still
/// killed at the cap (with the notice) rather than drained.
#[tokio::test]
async fn keywords_in_literals_do_not_prevent_the_cap_kill() {
    let Some(session) = open().await else {
        return;
    };
    let (results, _) = run(
        session.as_ref(),
        "SELECT a.id, 'UPDATE' AS op FROM events_big a CROSS JOIN events_big b \
         WHERE b.id <= 100 -- delete",
        RunOptions {
            max_rows: 1000,
            ..RunOptions::default()
        },
    )
    .await;
    let r = &results[0];
    assert!(r.error.is_none(), "{r:?}");
    assert!(r.truncated);
    assert!(
        r.notices.iter().any(|n| n.contains("row limit")),
        "{:?}",
        r.notices
    );
    assert_eq!(
        single(session.as_ref(), "select 42").await.unwrap(),
        vec![vec![Value::Int(42)]]
    );
}

async fn count_rows(session: &dyn DbSession, table: &str) -> i64 {
    count_of(
        &single(session, &format!("select count(*) from {table}"))
            .await
            .unwrap(),
    )
}

/// EXPLAIN ANALYZE executes the statement (a SELECT calling a function that writes commits
/// that write under autocommit): it must run in a transaction that is rolled back.
#[tokio::test]
async fn explain_analyze_never_keeps_changes() {
    let Some(session) = open().await else {
        return;
    };
    let s = session.as_ref();
    let x = suffix();
    let (t, log, f) = (
        format!("tabler_xa_{x}"),
        format!("tabler_xa_log_{x}"),
        format!("tabler_xa_f_{x}"),
    );
    let (results, _) = run(
        s,
        &format!(
            "create temporary table {t} (a int) engine=InnoDB; \
             insert into {t} values (1), (2), (3); \
             create table {log} (a int) engine=InnoDB;"
        ),
        RunOptions::default(),
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    // DETERMINISTIC: binary logging refuses a writing function without it.
    let created = single(
        s,
        &format!(
            "create function {f}() returns int deterministic modifies sql data \
             begin insert into {log} values (1); return 1; end"
        ),
    )
    .await;
    assert_eq!(s.in_transaction(), Some(false));

    explain_inner(
        s,
        &format!("DELETE FROM {t}"),
        ExplainMode::Analyze,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert_eq!(count_rows(s, &t).await, 3);
    assert_eq!(s.in_transaction(), Some(false));

    let options = RunOptions {
        explain: Some(ExplainMode::Analyze),
        stop_on_error: false,
        ..RunOptions::default()
    };
    let (results, _) = run(
        s,
        &format!("delete from {t} where a = 1; update {t} set a = a + 10;"),
        options,
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(count_rows(s, &t).await, 3);
    let sum = single(s, &format!("select cast(sum(a) as signed) from {t}"))
        .await
        .unwrap();
    assert_eq!(count_of(&sum), 6);

    // A read whose function writes: the write is rolled back too.
    match created {
        Ok(_) => {
            explain_inner(
                s,
                &format!("select {f}()"),
                ExplainMode::Analyze,
                &CancelSlot::default(),
            )
            .await
            .unwrap();
            assert_eq!(count_rows(s, &log).await, 0);
            assert_eq!(s.in_transaction(), Some(false));
            single(s, &format!("drop function {f}")).await.unwrap();
        }
        Err(e) => println!("writing function not created ({e}); that part is skipped"),
    }
    single(s, &format!("drop table {log}")).await.unwrap();
    single(s, &format!("drop temporary table {t}"))
        .await
        .unwrap();
}

/// Inside the user's own transaction the analyze runs under a savepoint (never START
/// TRANSACTION, which would commit the user's work): the transaction stays open with its INSERT.
#[tokio::test]
async fn explain_analyze_inside_a_transaction_keeps_it_open() {
    let Some(session) = open().await else {
        return;
    };
    let s = session.as_ref();
    let t = format!("tabler_xt_{}", suffix());
    single(
        s,
        &format!("create temporary table {t} (a int) engine=InnoDB"),
    )
    .await
    .unwrap();
    single(s, &format!("insert into {t} values (1), (2), (3)"))
        .await
        .unwrap();
    let (results, _) = run(
        s,
        &format!("BEGIN; INSERT INTO {t} VALUES (4);"),
        RunOptions::default(),
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(s.in_transaction(), Some(true));

    explain_inner(
        s,
        &format!("DELETE FROM {t}"),
        ExplainMode::Analyze,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert_eq!(s.in_transaction(), Some(true));
    assert_eq!(
        count_rows(s, &t).await,
        4,
        "the INSERT is intact, the DELETE undone"
    );

    single(s, "ROLLBACK").await.unwrap();
    assert_eq!(s.in_transaction(), Some(false));
    assert_eq!(count_rows(s, &t).await, 3, "ROLLBACK removed the INSERT");

    // With autocommit off every statement runs in an implicit transaction (which START
    // TRANSACTION would commit): the analyze keeps the user's pending INSERT pending.
    let (results, _) = run(
        s,
        &format!("SET autocommit = 0; INSERT INTO {t} VALUES (5);"),
        RunOptions::default(),
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    explain_inner(
        s,
        &format!("DELETE FROM {t}"),
        ExplainMode::Analyze,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert_eq!(count_rows(s, &t).await, 4);
    single(s, "ROLLBACK").await.unwrap();
    assert_eq!(count_rows(s, &t).await, 3, "the INSERT was still pending");
    single(s, "SET autocommit = 1").await.unwrap();
    single(s, &format!("drop temporary table {t}"))
        .await
        .unwrap();
}
