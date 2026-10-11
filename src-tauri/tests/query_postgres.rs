//! SQL editor execution against PostgreSQL. Runs only when `TABLER_TEST_PG_URL` is set, e.g.
//! `TABLER_TEST_PG_URL=postgres://apriansyahrs@127.0.0.1:5432/tabler_dev cargo test --test query_postgres`.

// `AppError` carries the full server diagnostic (same allowance as the library).
#![allow(clippy::result_large_err)]

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tabler_lib::commands::query::{explain_inner, run_sql_inner};
use tabler_lib::drivers::{
    CancelHandle, CancelSlot, CollectSink, ConnectTarget, DbSession, RowSink, connect,
    request_cancel,
};
use tabler_lib::error::AppError;
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::query::{
    ExplainMode, QueryEvent, ResultColumn, RunOptions, StatementKind, StatementResult,
};
use tabler_lib::types::schema::ValueKind;
use tabler_lib::types::value::Value;

/// Parses `postgres://user[:pass]@host[:port]/db` into a profile plus the password.
fn profile_from_url(url: &str) -> (ConnectionProfile, Option<String>) {
    let rest = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))
        .expect("TABLER_TEST_PG_URL must start with postgres://");
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
        name: "pg-test".into(),
        color: None,
        group: None,
        driver: Driver::Postgres,
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

async fn open() -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
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
    .expect("connect");
    Some(session)
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

fn rows_in(events: &[QueryEvent], index: usize) -> Vec<Vec<Value>> {
    events
        .iter()
        .filter_map(|e| match e {
            QueryEvent::Rows { index: i, rows, .. } if *i == index => Some(rows.clone()),
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

/// Waits until a statement has put its live cancel handle into the slot.
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
    sets: u32,
    stop_after: Option<u64>,
}

impl RowSink for CountSink {
    fn begin_set(&mut self, _columns: Vec<ResultColumn>) {
        self.sets += 1;
    }
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
        "select 1; select 2; update users set name = name where false;",
        RunOptions::default(),
    )
    .await;
    assert_eq!(results.len(), 3, "{results:?}");
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(results[0].kind, StatementKind::Rows);
    assert_eq!(results[1].kind, StatementKind::Rows);
    assert_eq!(results[2].kind, StatementKind::Affected);
    assert_eq!(results[2].affected, Some(0));
    assert_eq!(rows_in(&events, 0), vec![vec![Value::Int(1)]]);
    assert_eq!(rows_in(&events, 1), vec![vec![Value::Int(2)]]);
    assert_eq!(results[0].columns[0].kind, ValueKind::Int);
    assert_eq!(results[0].columns[0].data_type, "int4");
    assert!(matches!(events.last(), Some(QueryEvent::Done { .. })));
}

#[tokio::test]
async fn decodes_described_types_and_collects_notices() {
    let Some(session) = open().await else {
        return;
    };
    let mut sink = CollectSink::default();
    let outcome = session
        .run_statement(
            "select 1::int4 as a, 'x'::text as b, now() as c, array[1,2] as d, null::int as e, \
             '{\"k\": 1}'::jsonb as f, 9007199254740993::int8 as g, 1.50::numeric as h",
            100,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(outcome.kind, StatementKind::Rows);
    let set = &sink.sets[0];
    let kinds: Vec<_> = set.columns.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        [
            ValueKind::Int,
            ValueKind::Text,
            ValueKind::TimestampTz,
            ValueKind::Array,
            ValueKind::Int,
            ValueKind::Json,
            ValueKind::BigInt,
            ValueKind::Decimal
        ]
    );
    let row = &set.rows[0];
    assert_eq!(row[0], Value::Int(1));
    assert_eq!(row[1], Value::Text("x".into()));
    assert!(matches!(row[2], Value::TimestampTz(_)));
    assert_eq!(row[3], Value::Array(vec![Value::Int(1), Value::Int(2)]));
    assert_eq!(row[4], Value::Null);
    assert_eq!(row[5], Value::Json("{\"k\": 1}".into()));
    assert_eq!(row[6], Value::BigInt("9007199254740993".into()));
    assert_eq!(row[7], Value::Decimal("1.50".into()));

    let outcome = session
        .run_statement(
            "do $$ begin raise notice 'tabler notice %', 7; end $$",
            100,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(outcome.kind, StatementKind::Empty);
    assert!(
        outcome
            .notices
            .iter()
            .any(|n| n == "NOTICE: tabler notice 7"),
        "{:?}",
        outcome.notices
    );
}

#[tokio::test]
async fn reports_syntax_errors_with_position() {
    let Some(session) = open().await else {
        return;
    };
    let (results, events) = run(
        session.as_ref(),
        "select 1;\nselect * frm users;\nselect 3;",
        RunOptions::default(),
    )
    .await;
    assert_eq!(results.len(), 2, "stops at the first error");
    let err = results[1].error.as_ref().expect("error");
    assert_eq!(err.code, "DB_ERROR");
    assert_eq!(err.sqlstate.as_deref(), Some("42601"));
    assert_eq!(err.statement_index, Some(1));
    let pos = err.position.as_ref().expect("position");
    assert_eq!((pos.line, pos.column, pos.offset), (2, 10, 19));
    assert!(matches!(
        events[events.len() - 2],
        QueryEvent::Failed { index: 1, .. }
    ));
}

#[tokio::test]
async fn cancels_a_running_statement() {
    let Some(session) = open().await else {
        return;
    };
    let slot = CancelSlot::default();
    let task = {
        let session = session.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut events = Vec::new();
            let results = {
                let mut emit = |e: QueryEvent| events.push(e);
                run_sql_inner(
                    session.as_ref(),
                    "select pg_sleep(30); select 2",
                    &RunOptions::default(),
                    &slot,
                    &mut emit,
                )
                .await
            };
            (results, events)
        })
    };
    wait_armed(&slot).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let cancelled_at = Instant::now();
    let handle = request_cancel(&slot).expect("a live handle");
    session.cancel(handle).await.expect("cancel");
    let (results, _events) = task.await.unwrap();
    assert!(
        cancelled_at.elapsed() < Duration::from_secs(2),
        "took {:?}",
        cancelled_at.elapsed()
    );
    assert_eq!(results.len(), 1, "the batch stops after a cancel");
    assert_eq!(results[0].error.as_ref().unwrap().code, "CANCELLED");

    // The editor connection is still usable.
    assert_eq!(
        single(session.as_ref(), "select 5").await.unwrap(),
        vec![vec![Value::Int(5)]]
    );
    // Nothing to cancel is not an error.
    session
        .cancel(CancelHandle::Requested)
        .await
        .expect("no-op cancel");
}

#[tokio::test]
async fn tracks_transactions_on_the_editor() {
    let Some(session) = open().await else {
        return;
    };
    assert_eq!(session.in_transaction(), Some(false));
    single(session.as_ref(), "BEGIN").await.unwrap();
    single(session.as_ref(), "select 1").await.unwrap();
    assert_eq!(session.in_transaction(), Some(true));
    // A failing statement inside the transaction reports its own error, not 25P02.
    let err = single(session.as_ref(), "select * from tabler_missing_x7q")
        .await
        .unwrap_err();
    match err {
        AppError::Db { sqlstate, .. } => assert_eq!(sqlstate.as_deref(), Some("42P01")),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(session.in_transaction(), Some(true));
    single(session.as_ref(), "ROLLBACK").await.unwrap();
    assert_eq!(session.in_transaction(), Some(false));

    // Session state persists between runs (dedicated editor connection). The temp table only
    // exists inside the rolled-back transaction.
    let table = format!(
        "tabler_tx_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let (results, _) = run(
        session.as_ref(),
        &format!("begin; create temp table {table} (a int); insert into {table} values (1);"),
        RunOptions::default(),
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(session.in_transaction(), Some(true));
    let (results, events) = run(
        session.as_ref(),
        &format!("select count(*) from {table}; rollback;"),
        RunOptions::default(),
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(rows_in(&events, 0), vec![vec![Value::Int(1)]]);
    assert_eq!(session.in_transaction(), Some(false));
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
    let (results, events) = run(
        session.as_ref(),
        "select generate_series(1, 20000) as n",
        options.clone(),
    )
    .await;
    let r = &results[0];
    assert!(r.error.is_none(), "{r:?}");
    assert!(r.truncated);
    assert_eq!(r.row_count, 1000);
    let rows = rows_in(&events, 0);
    assert_eq!(rows.len(), 1000);
    assert_eq!(rows[999], vec![Value::Int(1000)]);

    // The server-side cancel used to stop the statement must not hit the next one.
    let started = Instant::now();
    assert_eq!(
        single(session.as_ref(), "select pg_sleep(0.3), 42")
            .await
            .unwrap()[0][1],
        Value::Int(42)
    );
    assert!(started.elapsed() >= Duration::from_millis(300));

    // A huge result is stopped server-side rather than drained.
    let started = Instant::now();
    let (results, _) = run(
        session.as_ref(),
        "select generate_series(1, 200000000) as n",
        options.clone(),
    )
    .await;
    assert!(results[0].truncated);
    assert_eq!(results[0].row_count, 1000);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(
        single(session.as_ref(), "select 8").await.unwrap(),
        vec![vec![Value::Int(8)]]
    );

    // Inside a transaction the cap is rolled back to a savepoint: the transaction goes on.
    single(session.as_ref(), "begin").await.unwrap();
    let (results, _) = run(
        session.as_ref(),
        "select generate_series(1, 20000)",
        options,
    )
    .await;
    assert!(results[0].truncated);
    assert_eq!(results[0].row_count, 1000);
    assert_eq!(
        single(session.as_ref(), "select 7").await.unwrap(),
        vec![vec![Value::Int(7)]]
    );
    assert_eq!(session.in_transaction(), Some(true));
    single(session.as_ref(), "rollback").await.unwrap();
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
    assert_eq!(sink.sets, 1);

    // With parameters (extended protocol).
    let mut sink = CountSink::default();
    let n = session
        .stream_query(
            "select id::text from events_big where id <= $1::int8",
            &[Value::Int(1234)],
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(n, 1234);

    // A sink that stops early (cancelled export) ends the stream; the pool stays healthy.
    for params in [vec![], vec![Value::Int(0)]] {
        let sql = if params.is_empty() {
            "select * from events_big"
        } else {
            "select id::text from events_big where id > $1::int8"
        };
        let mut sink = CountSink {
            stop_after: Some(1),
            ..CountSink::default()
        };
        let n = session
            .stream_query(sql, &params, &mut sink, CancelSlot::default())
            .await
            .unwrap();
        assert!(n <= 500, "stopped after the first chunk, got {n}");
        for _ in 0..4 {
            session.ping().await.expect("pool usable");
            assert_eq!(session.query("select 1", &[]).await.unwrap().rows.len(), 1);
        }
    }
}

#[tokio::test]
async fn explains_statements() {
    let Some(session) = open().await else {
        return;
    };
    let plan = explain_inner(
        session.as_ref(),
        "select * from users where id = 1;",
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    let text = plan.text.expect("text plan");
    assert!(text.contains("users"), "{text}");
    assert_eq!(plan.columns.len(), 1);

    let plan = explain_inner(
        session.as_ref(),
        "select count(*) from events_big",
        ExplainMode::Analyze,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert!(plan.text.unwrap().contains("actual time"));

    let options = RunOptions {
        explain: Some(ExplainMode::Plan),
        ..RunOptions::default()
    };
    let (results, _) = run(session.as_ref(), "select 1;\nselect * frm users", options).await;
    assert_eq!(results[0].kind, StatementKind::Explain);
    let err = results[1].error.as_ref().expect("error");
    let pos = err.position.as_ref().expect("position");
    assert_eq!((pos.line, pos.column), (2, 10));
}

#[tokio::test]
async fn rejects_meta_commands_and_client_copy() {
    let Some(session) = open().await else {
        return;
    };
    let (results, _) = run(
        session.as_ref(),
        "\\dt\nselect 1;",
        RunOptions {
            stop_on_error: false,
            ..RunOptions::default()
        },
    )
    .await;
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].error.as_ref().unwrap().code, "UNSUPPORTED");
    assert!(results[1].error.is_none());
    let err = single(session.as_ref(), "copy users to stdout")
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Unsupported(_)));
    single(session.as_ref(), "select 1").await.unwrap();
}

#[tokio::test]
async fn reports_a_lost_editor_connection() {
    let Some(session) = open().await else {
        return;
    };
    let pid = match &single(session.as_ref(), "select pg_backend_pid()")
        .await
        .unwrap()[0][0]
    {
        Value::Int(n) => *n,
        other => panic!("unexpected {other:?}"),
    };
    session
        .execute("select pg_terminate_backend($1::int)", &[Value::Int(pid)])
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let err = single(session.as_ref(), "select 1").await.unwrap_err();
    assert!(matches!(err, AppError::NotConnected), "{err:?}");
}

fn suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

fn count_of(rows: &[Vec<Value>]) -> i64 {
    match rows.first().and_then(|r| r.first()) {
        Some(Value::Int(n)) => *n,
        other => panic!("unexpected {other:?}"),
    }
}

/// The row cap must never make a statement silently lose its side effects: statements that are
/// not plain reads run to completion, and a read that the server had to cancel says so.
#[tokio::test]
async fn row_cap_never_silently_drops_side_effects() {
    let Some(session) = open().await else {
        return;
    };
    let s = suffix();
    let (log, f_sql, f_srf, tgt) = (
        format!("tabler_log_{s}"),
        format!("tabler_f_{s}"),
        format!("tabler_srf_{s}"),
        format!("tabler_tgt_{s}"),
    );
    let setup = format!(
        "create table {log} (v int);\n\
         create table {tgt} (v int);\n\
         create function {f_sql}(x int) returns int volatile language sql as \
           $$ insert into {log} values (x); select x $$;\n\
         create function {f_srf}() returns setof int language plpgsql as \
           $$ begin insert into {log} values (-1); return query select generate_series(1, 300000); end $$;"
    );
    let (results, _) = run(session.as_ref(), &setup, RunOptions::default()).await;
    let cleanup = format!(
        "drop function if exists {f_sql}(int); drop function if exists {f_srf}(); \
         drop table if exists {log}; drop table if exists {tgt};"
    );
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
        let capped = RunOptions {
            max_rows: 100,
            ..RunOptions::default()
        };

        // A read calling a writing function is cancelled at the cap: reported, not silent.
        for sql in [
            format!("select {f_sql}(g) from generate_series(1, 300000) g"),
            format!("select * from {f_srf}()"),
        ] {
            let (results, events) = run(session.as_ref(), &sql, capped.clone()).await;
            let r = &results[0];
            assert!(r.error.is_none(), "{r:?}");
            assert!(r.truncated);
            assert_eq!(rows_in(&events, 0).len(), 100);
            assert!(
                r.notices.iter().any(|n| n.contains("row limit")),
                "{sql}: {:?}",
                r.notices
            );
        }
        // …and it really was rolled back, as the notice says.
        let rows = single(session.as_ref(), &format!("select count(*) from {log}"))
            .await
            .unwrap();
        assert_eq!(count_of(&rows), 0);

        // DML with RETURNING runs to completion and reports what it did.
        let (results, events) = run(
            session.as_ref(),
            &format!("insert into {tgt} select generate_series(1, 20000) returning v"),
            capped.clone(),
        )
        .await;
        let r = &results[0];
        assert!(r.error.is_none(), "{r:?}");
        assert!(r.truncated);
        assert_eq!(rows_in(&events, 0).len(), 100);
        assert_eq!(r.affected, Some(20000));
        assert!(r.notices.is_empty(), "{:?}", r.notices);
        let rows = single(session.as_ref(), &format!("select count(*) from {tgt}"))
            .await
            .unwrap();
        assert_eq!(count_of(&rows), 20000);

        // A statement that fails after the cap (not cancelled: it has side effects) reports the
        // failure instead of a truncated success.
        single(session.as_ref(), "begin").await.unwrap();
        let (results, _) = run(
            session.as_ref(),
            &format!(
                "insert into {tgt} select 1 / (g - 15000) from generate_series(1, 20000) g \
                 returning v"
            ),
            capped,
        )
        .await;
        single(session.as_ref(), "rollback").await.unwrap();
        let err = results[0].error.as_ref().expect("the division by zero");
        assert_eq!(err.sqlstate.as_deref(), Some("22012"), "{err:?}");
        assert_eq!(
            single(session.as_ref(), "select 3").await.unwrap(),
            vec![vec![Value::Int(3)]]
        );
    }))
    .await;
    // Clean up even when an assertion failed.
    let _ = single(session.as_ref(), "rollback").await;
    let (results, _) = run(session.as_ref(), &cleanup, RunOptions::default()).await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// A cancel request that failed (network…) can be sent again while the statement still runs.
#[tokio::test]
async fn retries_a_cancel_that_did_not_go_through() {
    let Some(session) = open().await else {
        return;
    };
    let slot = CancelSlot::default();
    let task = {
        let session = session.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut emit = |_e: QueryEvent| {};
            run_sql_inner(
                session.as_ref(),
                "select pg_sleep(30)",
                &RunOptions::default(),
                &slot,
                &mut emit,
            )
            .await
        })
    };
    wait_armed(&slot).await;
    // The first request is lost (as when `DbSession::cancel` fails).
    drop(request_cancel(&slot).expect("a live handle"));
    let cancelled_at = Instant::now();
    let handle = request_cancel(&slot).expect("the live handle again");
    session.cancel(handle).await.expect("cancel");
    let results = task.await.unwrap();
    assert!(cancelled_at.elapsed() < Duration::from_secs(2));
    assert_eq!(results[0].error.as_ref().unwrap().code, "CANCELLED");
    assert_eq!(slot.cancels_in_flight(), 0);
}

/// Closing a session (disconnect, database switch) stops the editor statement instead of
/// waiting for it.
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
                .run_statement("select pg_sleep(30)", 10, &mut sink, slot)
                .await
        })
    };
    wait_armed(&slot).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    session.close().await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
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

/// `;` inside parentheses does not end a PostgreSQL statement (multi-action rules).
#[tokio::test]
async fn runs_multi_action_rules() {
    let Some(session) = open().await else {
        return;
    };
    let s = suffix();
    let (t, a, b) = (
        format!("tabler_rt_{s}"),
        format!("tabler_ra_{s}"),
        format!("tabler_rb_{s}"),
    );
    let sql = format!(
        "create temp table {t} (v int); create temp table {a} (v int); create temp table {b} (v int);\n\
         create rule tabler_r_{s} as on insert to {t} do also (insert into {a} values (1); insert into {b} values (2));\n\
         insert into {t} values (0);\n\
         select (select count(*) from {a}) + (select count(*) from {b});\n\
         drop table {t}, {a}, {b};"
    );
    let (results, events) = run(session.as_ref(), &sql, RunOptions::default()).await;
    assert_eq!(results.len(), 7, "{results:?}");
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(rows_in(&events, 5), vec![vec![Value::Int(2)]]);
}

fn data_type(results: &[StatementResult], index: usize) -> String {
    results[index]
        .columns
        .first()
        .map(|c| c.data_type.clone())
        .unwrap_or_default()
}

/// `DISCARD ALL` / `DEALLOCATE ALL` also drop the type-lookup statements tokio-postgres keeps
/// prepared on the connection; describing a custom type afterwards must still work.
#[tokio::test]
async fn describes_custom_types_after_prepared_statements_are_dropped() {
    let Some(session) = open().await else {
        return;
    };
    let s = suffix();
    let types: Vec<String> = (1..=5).map(|i| format!("tabler_e{i}_{s}")).collect();
    let setup: String = types
        .iter()
        .enumerate()
        .map(|(i, t)| format!("create type {t} as enum ('v{i}');\n"))
        .collect();
    let (results, _) = run(session.as_ref(), &setup, RunOptions::default()).await;
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
        let (e1, e2, e3, e4, e5) = (&types[0], &types[1], &types[2], &types[3], &types[4]);

        let (results, _) = run(
            session.as_ref(),
            &format!("select 'v0'::{e1} as v; discard all; select 'v1'::{e2} as v;"),
            RunOptions::default(),
        )
        .await;
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
        assert_eq!(data_type(&results, 2), *e2);

        let (results, _) = run(
            session.as_ref(),
            &format!("deallocate all; select 'v2'::{e3} as v;"),
            RunOptions::default(),
        )
        .await;
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
        assert_eq!(data_type(&results, 1), *e3);

        // Inside a transaction a failed type lookup would abort the transaction.
        let (results, _) = run(
            session.as_ref(),
            &format!("begin; deallocate prepare all; select 'v3'::{e4} as v; select 1; rollback;"),
            RunOptions::default(),
        )
        .await;
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
        assert_eq!(data_type(&results, 2), *e4);

        // Dropped where the editor cannot see it: the statement still runs, as text.
        let (results, events) = run(
            session.as_ref(),
            &format!("do $$ begin execute 'deallocate all'; end $$; select 'v4'::{e5} as v;"),
            RunOptions::default(),
        )
        .await;
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
        assert_eq!(rows_in(&events, 1), vec![vec![Value::Text("v4".into())]]);
        assert!(
            results[1]
                .notices
                .iter()
                .any(|n| n.contains("column types")),
            "{:?}",
            results[1].notices
        );
    }))
    .await;
    let cleanup: String = types
        .iter()
        .map(|t| format!("drop type if exists {t};\n"))
        .collect();
    let _ = single(session.as_ref(), "rollback").await;
    let (results, _) = run(session.as_ref(), &cleanup, RunOptions::default()).await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// Logs `<label> <event> [index]` for every event of a run.
async fn logged_run(
    session: &dyn DbSession,
    sql: &str,
    label: &str,
    log: &std::sync::Mutex<Vec<String>>,
) -> Vec<StatementResult> {
    let slot = CancelSlot::default();
    let mut emit = |e: QueryEvent| {
        let v = serde_json::to_value(&e).unwrap();
        let index = v["data"]["index"].as_u64().map(|i| format!(" {i}"));
        log.lock().unwrap().push(format!(
            "{label} {}{}",
            v["event"].as_str().unwrap(),
            index.unwrap_or_default()
        ));
    };
    run_sql_inner(session, sql, &RunOptions::default(), &slot, &mut emit).await
}

/// Two runs on one connection (two editor tabs) must not interleave their statements: B's
/// UPDATE must not land inside A's `BEGIN … ROLLBACK` and be rolled back with it.
#[tokio::test]
async fn concurrent_runs_do_not_interleave() {
    let Some(session) = open().await else {
        return;
    };
    let t = format!("tabler_cc_{}", suffix());
    let (results, _) = run(
        session.as_ref(),
        &format!("create table {t} (id int, flag int); insert into {t} values (1, 0);"),
        RunOptions::default(),
    )
    .await;
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
        let log = std::sync::Mutex::new(Vec::new());
        let run_a = logged_run(
            session.as_ref(),
            "begin; select pg_sleep(1); rollback;",
            "A",
            &log,
        );
        let sql_b = format!("update {t} set flag = 1");
        let run_b = async {
            for _ in 0..500 {
                if log.lock().unwrap().iter().any(|l| l == "A finished 0") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            logged_run(session.as_ref(), &sql_b, "B", &log).await
        };
        let (a, b) = tokio::join!(run_a, run_b);
        assert!(a.iter().all(|r| r.error.is_none()), "{a:?}");
        assert!(b[0].error.is_none(), "{b:?}");
        assert_eq!(b[0].affected, Some(1));
        let log = log.into_inner().unwrap();
        let a_done = log.iter().position(|l| l == "A done").unwrap();
        let b_started = log.iter().position(|l| l == "B started 0").unwrap();
        assert!(a_done < b_started, "B ran inside A's run: {log:?}");
        assert_eq!(
            single(session.as_ref(), &format!("select flag from {t}"))
                .await
                .unwrap(),
            vec![vec![Value::Int(1)]]
        );
    }))
    .await;
    single(session.as_ref(), &format!("drop table if exists {t}"))
        .await
        .unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// Inside a transaction a capped read is stopped on the server (under a savepoint) instead of
/// being read to the end, and the transaction stays usable.
#[tokio::test]
async fn caps_rows_inside_a_transaction_without_draining() {
    let Some(session) = open().await else {
        return;
    };
    let t = format!("tabler_txcap_{}", suffix());
    let capped = RunOptions {
        max_rows: 1000,
        ..RunOptions::default()
    };
    let (results, _) = run(
        session.as_ref(),
        &format!("begin; create temp table {t} (v int); insert into {t} values (1);"),
        RunOptions::default(),
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    let started = Instant::now();
    let (results, events) = run(
        session.as_ref(),
        "select generate_series(1, 200000000) as n",
        capped.clone(),
    )
    .await;
    let r = &results[0];
    assert!(r.error.is_none(), "{r:?}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "took {:?}",
        started.elapsed()
    );
    assert!(r.truncated);
    assert_eq!(rows_in(&events, 0).len(), 1000);
    assert!(
        r.notices.iter().any(|n| n.contains("row limit")),
        "{:?}",
        r.notices
    );
    // The transaction is intact: its earlier work is still there and it accepts statements.
    assert_eq!(session.in_transaction(), Some(true));
    assert_eq!(
        single(session.as_ref(), &format!("select count(*) from {t}"))
            .await
            .unwrap(),
        vec![vec![Value::Int(1)]]
    );
    // A read that completes under the cap leaves no savepoint behind.
    let (results, _) = run(session.as_ref(), "select 1; select 2;", capped).await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert!(
        single(session.as_ref(), "release savepoint tabler_row_cap")
            .await
            .is_err()
    );
    single(session.as_ref(), "rollback").await.unwrap();
    assert_eq!(session.in_transaction(), Some(false));
}

/// `COMMIT AND CHAIN` outside a transaction block fails and opens nothing.
#[tokio::test]
async fn a_failed_chain_opens_no_transaction() {
    let Some(session) = open().await else {
        return;
    };
    assert_eq!(session.in_transaction(), Some(false));
    let err = single(session.as_ref(), "commit and chain")
        .await
        .unwrap_err();
    match err {
        AppError::Db { sqlstate, .. } => assert_eq!(sqlstate.as_deref(), Some("25P01")),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(session.in_transaction(), Some(false));
    // …so a capped read is still cancelled on the server rather than drained.
    let (results, _) = run(
        session.as_ref(),
        "select generate_series(1, 30000000)",
        RunOptions {
            max_rows: 100,
            ..RunOptions::default()
        },
    )
    .await;
    assert!(
        results[0].notices.iter().any(|n| n.contains("row limit")),
        "{:?}",
        results[0].notices
    );
}

/// A cancel requested while the statement is being described must stop it even when the cancel
/// signal never reaches the server (it would be ignored by an idle backend).
#[tokio::test]
async fn a_cancel_during_describe_stops_the_statement() {
    let Some(session) = open().await else {
        return;
    };
    let Some(locker) = open().await else {
        return;
    };
    let t = format!("tabler_lk_{}", suffix());
    single(session.as_ref(), &format!("create table {t} (v int)"))
        .await
        .unwrap();
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        // Another connection locks the table: describing an INSERT on it waits for the lock.
        let (results, _) = run(
            locker.as_ref(),
            &format!("begin; lock table {t} in access exclusive mode;"),
            RunOptions::default(),
        )
        .await;
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
        let slot = CancelSlot::default();
        let task = {
            let session = session.clone();
            let slot = slot.clone();
            let sql = format!("insert into {t} values (1) returning v");
            tokio::spawn(async move {
                let mut sink = CollectSink::default();
                session.run_statement(&sql, 10, &mut sink, slot).await
            })
        };
        wait_armed(&slot).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        // The request is made, but its signal is lost (never sent).
        drop(request_cancel(&slot).expect("a live handle"));
        single(locker.as_ref(), "rollback").await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the statement ended")
            .unwrap();
        assert!(matches!(result, Err(AppError::Cancelled)), "{result:?}");
        let rows = single(session.as_ref(), &format!("select count(*) from {t}"))
            .await
            .unwrap();
        assert_eq!(count_of(&rows), 0, "the INSERT did not run");
    }))
    .await;
    let _ = single(locker.as_ref(), "rollback").await;
    single(session.as_ref(), &format!("drop table if exists {t}"))
        .await
        .unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// `UPDATE` / `DELETE` inside a string literal do not make a read look like a write: the capped
/// read is still cancelled on the server rather than drained.
#[tokio::test]
async fn keywords_in_literals_do_not_prevent_the_cap_cancel() {
    let Some(session) = open().await else {
        return;
    };
    let capped = RunOptions {
        max_rows: 1000,
        ..RunOptions::default()
    };
    for sql in [
        "select n, 'UPDATE' as op from generate_series(1, 5000000) n",
        "select n from generate_series(1, 5000000) n where 'DELETE' <> 'x' -- insert",
    ] {
        let (results, _) = run(session.as_ref(), sql, capped.clone()).await;
        let r = &results[0];
        assert!(r.error.is_none(), "{r:?}");
        assert!(r.truncated);
        assert!(
            r.notices.iter().any(|n| n.contains("row limit")),
            "{sql}: {:?}",
            r.notices
        );
    }
}
