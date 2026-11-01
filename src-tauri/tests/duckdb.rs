//! DuckDB against a real database file: the duckdb lane's suite (docs/drivers/duckdb.md). Built
//! only with `drv-duckdb` (`required-features` in Cargo.toml) and run with
//! `node dev/scripts/lane.mjs duckdb it`, which loads `.env`. Each test skips itself while its
//! variable is unset, and is `#[ignore]`d so that a plain `cargo test --all-features` leaves the
//! file alone.
//!
//! `TABLER_TEST_DUCKDB_PATH` is a DuckDB file holding the lane's sample
//! (`tests/fixtures/duckdb/sample.sql`; `pnpm dev:duckdb:build` writes `dev/fixtures/sample.duckdb`).
//! The suite works on a copy in a temp folder, so the file itself is never changed.

#![cfg(feature = "drv-duckdb")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tabler_lib::commands::browse::{browse_page, fetch_cell_inner};
use tabler_lib::commands::query::{query_limited, run_sql_inner};
use tabler_lib::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, TxStatement, connect, request_cancel,
};
use tabler_lib::error::AppError;
use tabler_lib::types::browse::{BrowseRequest, RowKey};
use tabler_lib::types::caps::Feature;
use tabler_lib::types::driver::ReadOnlyMode;
use tabler_lib::types::profile::ConnectionProfile;
use tabler_lib::types::query::{RunOptions, StatementKind, StatementResult};
use tabler_lib::types::schema::TableRef;
use tabler_lib::types::value::Value;

const VAR: &str = "TABLER_TEST_DUCKDB_PATH";

/// A read that keeps the engine busy for many seconds.
const SLOW: &str = "SELECT count(*) FROM range(100000000000) r(i) WHERE i % 7 = 3";

/// A copy of the file in `VAR` in a new temp folder; None (skipped) while it is unset.
fn copy_of_file() -> Option<(tempfile::TempDir, PathBuf)> {
    let src = common::url(VAR)?;
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("it.duckdb");
    std::fs::copy(&src, &path).unwrap_or_else(|e| panic!("{VAR}: copy {src}: {e}"));
    let wal = PathBuf::from(format!("{src}.wal"));
    if wal.exists() {
        std::fs::copy(&wal, dir.path().join("it.duckdb.wal")).expect("copy the WAL");
    }
    Some((dir, path))
}

fn profile_of(path: &Path, read_only: bool) -> ConnectionProfile {
    let (mut profile, _) = common::profile_from_url("sqlite://x");
    profile.driver = tabler_lib::types::profile::Driver::Duckdb;
    profile.sqlite_path = Some(path.to_string_lossy().into_owned());
    profile.options.read_only = read_only;
    profile
}

async fn open(path: &Path, read_only: bool) -> Arc<dyn DbSession> {
    let profile = profile_of(path, read_only);
    connect(ConnectTarget {
        profile: &profile,
        password: None,
        host_override: None,
    })
    .await
    .unwrap_or_else(|e| panic!("{VAR}: connect failed: {e}"))
}

async fn run(session: &dyn DbSession, sql: &str) -> Vec<StatementResult> {
    let options = RunOptions {
        max_rows: 2,
        ..RunOptions::default()
    };
    let mut emit = |_| {};
    run_sql_inner(session, sql, &options, &CancelSlot::default(), &mut emit).await
}

#[tokio::test]
#[ignore = "set TABLER_TEST_DUCKDB_PATH"]
async fn real_file() {
    let Some((dir, path)) = copy_of_file() else {
        return;
    };
    let s = open(&path, false).await;

    // Connect, list and structure.
    let info = s.server_info().await.unwrap();
    assert!(info.version.contains("v1."), "{}", info.version);
    let schemas = s.list_schemas().await.unwrap();
    assert!(schemas.iter().any(|x| x.name == "main"));
    let tables = s.list_tables("main").await.unwrap();
    assert!(tables.iter().any(|x| x.name == "orders"));
    let t = TableRef::new(Some("main"), "vals");
    s.execute(
        "CREATE TABLE vals (id INTEGER PRIMARY KEY, big BIGINT, d DECIMAL(10, 2), \
         ts TIMESTAMP, s STRUCT(a INTEGER))",
        &[],
    )
    .await
    .unwrap();
    s.execute(
        "INSERT INTO vals VALUES (1, 9007199254740993, 1.50, TIMESTAMP '2026-10-30 12:00:00', \
         {'a': 1}), (2, 2, 2, NULL, NULL), (3, 3, 3, NULL, NULL)",
        &[],
    )
    .await
    .unwrap();
    let meta = s.table_meta(&t).await.unwrap();
    assert_eq!(meta.primary_key, ["id"]);
    let caps = s.capabilities();
    assert!(caps.has(Feature::EditRows) && caps.has(Feature::CancelOnServer));
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    assert!(
        s.table_ddl(&t)
            .await
            .unwrap()
            .unwrap()
            .contains("PRIMARY KEY")
    );

    // Browse with paging (values exact), count, fetch.
    let page = |offset| BrowseRequest {
        table: t.clone(),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit: 2,
        offset,
        cursor: None,
    };
    let first = browse_page(s.as_ref(), &meta, &page(0), None)
        .await
        .unwrap();
    assert_eq!(first.rows.len(), 2);
    let second = browse_page(s.as_ref(), &meta, &page(2), None)
        .await
        .unwrap();
    assert_eq!(second.rows.len(), 1);
    let a = &first.rows[0];
    assert_eq!(a[1], Value::BigInt("9007199254740993".into()));
    assert_eq!(a[2], Value::Decimal("1.50".into()));
    assert_eq!(a[3], Value::Timestamp("2026-10-30 12:00:00".into()));
    assert!(
        matches!(&a[4], Value::Json(j) if j.contains("\"a\"")),
        "{:?}",
        a[4]
    );
    let counted = s.query("SELECT count(*) FROM vals", &[]).await.unwrap();
    assert_eq!(counted.rows, [[Value::Int(3)]]);
    let key = RowKey::Pk([("id".to_string(), Value::Int(2))].into());
    let cell = fetch_cell_inner(s.as_ref(), &meta, "big", &key)
        .await
        .unwrap();
    assert_eq!(cell, Value::Int(2));

    // A commit with one failing change: the whole transaction is rolled back, and the error
    // names the failed statement.
    let update = "UPDATE vals SET big = $1 WHERE id = $2";
    let failed = s
        .transaction(vec![
            TxStatement::new(update, vec![Value::Int(20), Value::Int(2)]).expecting(1),
            TxStatement::new(update, vec![Value::Int(40), Value::Int(4)]).expecting(1),
        ])
        .await
        .unwrap_err();
    assert!(!matches!(failed, AppError::Cancelled), "{failed:?}");
    let still = s
        .query("SELECT big FROM vals WHERE id = 2", &[])
        .await
        .unwrap();
    assert_eq!(still.rows, [[Value::Int(2)]]);

    // The editor: a transaction lasts in its connection, a read is capped.
    let results = run(
        s.as_ref(),
        "BEGIN TRANSACTION; SELECT * FROM range(10); ROLLBACK",
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(results[1].kind, StatementKind::Rows);
    assert!(results[1].truncated);

    // A cancel and a time out, both interrupting the engine.
    let slot = CancelSlot::default();
    let mut sink = CollectSink::default();
    let statement = s.run_statement(SLOW, 10, &mut sink, slot.clone());
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Some(handle) = request_cancel(&slot) {
            s.cancel(handle).await.unwrap();
        }
    };
    let (result, ()) = tokio::join!(statement, cancel);
    assert!(matches!(result, Err(AppError::Cancelled)), "{result:?}");
    let limited = query_limited(s.as_ref(), SLOW, &[], Some(Duration::from_millis(500))).await;
    assert!(
        matches!(limited, Err(AppError::QueryTimeout { .. })),
        "{limited:?}"
    );
    s.close().await;

    // Read-only: the engine refuses writes and files, and the session says so.
    let ro = open(&path, true).await;
    let caps = ro.capabilities();
    assert!(caps.safety.hardened);
    assert_eq!(caps.safety.read_only, ReadOnlyMode::Server);
    assert!(ro.execute("DELETE FROM vals", &[]).await.is_err());
    let csv = dir.path().join("x.csv");
    std::fs::write(&csv, "a\n1\n").unwrap();
    let scan = format!(
        "SELECT * FROM '{}'",
        csv.to_string_lossy().replace('\\', "/")
    );
    assert!(ro.query(&scan, &[]).await.is_err());
    assert_eq!(
        ro.query("SELECT count(*) FROM vals", &[])
            .await
            .unwrap()
            .rows,
        [[Value::Int(3)]]
    );
    ro.close().await;
    drop(dir);
}
