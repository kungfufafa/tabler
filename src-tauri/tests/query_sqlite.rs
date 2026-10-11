//! SQL editor execution against SQLite. Gated on `TABLER_TEST_SQLITE_PATH` (the fixture database,
//! relative to the repository root); every test works on a temporary copy.

// `AppError` carries the full server diagnostic (same allowance as the library).
#![allow(clippy::result_large_err)]

use std::ops::ControlFlow;
use std::path::PathBuf;
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

const ENV: &str = "TABLER_TEST_SQLITE_PATH";

/// Opens a session on a temporary copy of the fixture (the fixture itself is never modified).
async fn open() -> Option<(Arc<dyn DbSession>, tempfile::TempDir)> {
    let Ok(rel) = std::env::var(ENV) else {
        println!("skipped: {ENV} unset");
        return None;
    };
    let fixture = {
        let p = PathBuf::from(&rel);
        if p.is_absolute() {
            p
        } else {
            // Relative to the repository root (the parent of src-tauri).
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(p)
        }
    };
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("sample.sqlite");
    std::fs::copy(&fixture, &copy).unwrap_or_else(|e| panic!("copy {}: {e}", fixture.display()));
    let profile = ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "sqlite-test".into(),
        color: None,
        group: None,
        driver: Driver::Sqlite,
        host: None,
        port: None,
        user: None,
        database: None,
        ssl: SslMode::Disable,
        ssl_ca_path: None,
        sqlite_path: Some(copy.to_string_lossy().into_owned()),
        ssh: None,
        options: ProfileOptions::default(),
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    };
    let session = connect(ConnectTarget {
        profile: &profile,
        password: None,
        host_override: None,
    })
    .await
    .expect("open sqlite");
    Some((session, dir))
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
    let Some((session, _dir)) = open().await else {
        return;
    };
    let t = format!("tabler_q_{}", suffix());
    let (results, events) = run(
        session.as_ref(),
        &format!(
            "select 1; select count(*) from users;\n\
             create table {t} (a integer primary key, b text);\n\
             insert into {t} (b) values ('x'), ('y');\n\
             update users set name = name where 1 = 0;\n\
             select * from {t} order by a;\n\
             drop table {t};"
        ),
        RunOptions::default(),
    )
    .await;
    assert_eq!(results.len(), 7, "{results:?}");
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    let kinds: Vec<_> = results.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [
            StatementKind::Rows,
            StatementKind::Rows,
            StatementKind::Empty,
            StatementKind::Affected,
            StatementKind::Affected,
            StatementKind::Rows,
            StatementKind::Empty
        ]
    );
    assert_eq!(rows_in(&events, 1), vec![vec![Value::Int(61)]]);
    assert_eq!(results[3].affected, Some(2));
    assert_eq!(results[3].last_insert_id.as_deref(), Some("2"));
    assert_eq!(results[4].affected, Some(0));
    assert_eq!(rows_in(&events, 5).len(), 2);
    assert_eq!(results[5].span.line, 5);
}

#[tokio::test]
async fn reports_errors_with_position() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    let (results, _) = run(
        session.as_ref(),
        "select 1;\nselect * frm users;",
        RunOptions::default(),
    )
    .await;
    let err = results[1].error.as_ref().expect("error");
    assert_eq!(err.code, "DB_ERROR");
    assert_eq!(err.statement_index, Some(1));
    let pos = err.position.as_ref().expect("position");
    assert_eq!((pos.line, pos.column), (2, 10));

    let err = single(session.as_ref(), "select * from tabler_missing_q")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no such table"), "{err}");
}

#[tokio::test]
async fn interrupts_a_long_statement() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    let slot = CancelSlot::default();
    let task = {
        let session = session.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut events = Vec::new();
            let mut emit = |e: QueryEvent| events.push(e);
            run_sql_inner(
                session.as_ref(),
                "with recursive c(x) as (select 1 union all select x + 1 from c where x < 2000000000) \
                 select count(*) from c; select 2",
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
    session.cancel(handle).await.unwrap();
    let results = task.await.unwrap();
    assert!(
        cancelled_at.elapsed() < Duration::from_secs(2),
        "took {:?}",
        cancelled_at.elapsed()
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].error.as_ref().unwrap().code, "CANCELLED");
    assert_eq!(
        single(session.as_ref(), "select 3").await.unwrap(),
        vec![vec![Value::Int(3)]]
    );
}

#[tokio::test]
async fn tracks_transactions() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    assert_eq!(session.in_transaction(), Some(false));
    single(session.as_ref(), "BEGIN").await.unwrap();
    assert_eq!(session.in_transaction(), Some(true));
    single(session.as_ref(), "delete from users").await.unwrap();
    // The worker connection does not see the uncommitted delete.
    let rs = session
        .query("select count(*) from users", &[])
        .await
        .unwrap();
    assert_eq!(rs.rows, vec![vec![Value::Int(61)]]);
    single(session.as_ref(), "ROLLBACK").await.unwrap();
    assert_eq!(session.in_transaction(), Some(false));
    assert_eq!(
        single(session.as_ref(), "select count(*) from users")
            .await
            .unwrap(),
        vec![vec![Value::Int(61)]]
    );
}

#[tokio::test]
async fn caps_and_streams_rows() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    let (results, events) = run(
        session.as_ref(),
        "select * from events_big",
        RunOptions {
            max_rows: 1000,
            ..RunOptions::default()
        },
    )
    .await;
    assert!(results[0].truncated);
    assert_eq!(results[0].row_count, 1000);
    assert_eq!(rows_in(&events, 0).len(), 1000);

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
            "select id from events_big where id <= ?1",
            &[Value::Int(77)],
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(n, 77);

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
    assert!(n < 100_000, "{n}");
    session.ping().await.unwrap();
}

#[tokio::test]
async fn explains_and_keeps_routine_bodies_whole() {
    let Some((session, _dir)) = open().await else {
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
    let text = plan.text.expect("plan text");
    assert!(text.contains("users"), "{text}");

    let log = format!("tabler_log_{}", suffix());
    let trg = format!("tabler_trg_{}", suffix());
    let (results, _) = run(
        session.as_ref(),
        &format!(
            "create table {log} (msg text);\n\
             create trigger {trg} after update on users begin\n\
               insert into {log} values (case when new.name = old.name then 'same' else 'changed' end);\n\
               insert into {log} values ('second');\n\
             end;\n\
             update users set name = name where id = (select min(id) from users);\n\
             select count(*) from {log};\n\
             drop trigger {trg};\n\
             drop table {log};"
        ),
        RunOptions::default(),
    )
    .await;
    assert_eq!(results.len(), 6, "{results:?}");
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(results[2].affected, Some(1));
}

const LONG: &str = "with recursive c(x) as (select 1 union all select x + 1 from c where x < 2000000000) \
                    select count(*) from c";

/// Cancelling a run that still waits behind another statement on the same connection must not
/// interrupt that other statement (the interrupt handle is per connection).
#[tokio::test]
async fn cancelling_a_queued_run_spares_the_running_one() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    let spawn_run = |sql: &'static str, slot: CancelSlot| {
        let session = session.clone();
        tokio::spawn(async move {
            let mut emit = |_e: QueryEvent| {};
            run_sql_inner(
                session.as_ref(),
                sql,
                &RunOptions::default(),
                &slot,
                &mut emit,
            )
            .await
        })
    };
    let slot_a = CancelSlot::default();
    let task_a = spawn_run(LONG, slot_a.clone());
    wait_armed(&slot_a).await;
    let slot_b = CancelSlot::default();
    let task_b = spawn_run("select 42", slot_b.clone());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!slot_b.is_armed(), "a queued run exposes no live handle");
    if let Some(handle) = request_cancel(&slot_b) {
        session.cancel(handle).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !task_a.is_finished(),
        "the running statement was interrupted"
    );

    let handle = request_cancel(&slot_a).expect("the running statement's handle");
    session.cancel(handle).await.unwrap();
    let a = task_a.await.unwrap();
    assert_eq!(a[0].error.as_ref().unwrap().code, "CANCELLED");
    let b = task_b.await.unwrap();
    assert_eq!(b[0].error.as_ref().unwrap().code, "CANCELLED", "{b:?}");

    // Worker connection: cancelling a queued export spares the browse query running there.
    let browse = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .query(
                    "with recursive c(x) as (select 1 union all select x + 1 from c \
                     where x < 3000000) select count(*) from c",
                    &[],
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let export_slot = CancelSlot::default();
    let export = {
        let session = session.clone();
        let slot = export_slot.clone();
        tokio::spawn(async move {
            let mut sink = CountSink::default();
            session.stream_query("select 1", &[], &mut sink, slot).await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!browse.is_finished(), "pick a longer browse query");
    if let Some(handle) = request_cancel(&export_slot) {
        session.cancel(handle).await.unwrap();
    }
    let rows = browse
        .await
        .unwrap()
        .expect("the browse query was not interrupted");
    assert_eq!(rows.rows, vec![vec![Value::Int(3_000_000)]]);
    assert!(matches!(export.await.unwrap(), Err(AppError::Cancelled)));
}

/// Closing the session interrupts the statement running on it (a long write would otherwise
/// keep the file locked after a disconnect) and refuses queued work.
#[tokio::test]
async fn close_interrupts_the_running_statement() {
    let Some((session, _dir)) = open().await else {
        return;
    };
    let slot = CancelSlot::default();
    let task = {
        let session = session.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut sink = CollectSink::default();
            session.run_statement(LONG, 10, &mut sink, slot).await
        })
    };
    wait_armed(&slot).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    session.close().await;
    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("the statement stopped")
        .unwrap();
    assert!(result.is_err(), "{result:?}");
    let err = single(session.as_ref(), "select 1").await.unwrap_err();
    assert!(matches!(err, AppError::NotConnected), "{err:?}");
}
