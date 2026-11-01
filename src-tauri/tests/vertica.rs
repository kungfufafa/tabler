//! Vertica against a real server: the vertica lane's suite (docs/drivers/vertica.md). Built only
//! with `drv-vertica` (`required-features` in Cargo.toml) and run with
//! `node dev/scripts/lane.mjs vertica it`, which loads `.env`. Each test skips itself while
//! `TABLER_TEST_VERTICA_URL` (`vertica://dbadmin:password@host:5433/VMart`) is unset, and is
//! `#[ignore]`d so that a plain `cargo test --all-features` never reaches a server.
//!
//! The tests make their own tables (`tabler_it_<uuid>` in the user's default schema) and drop
//! them. Besides what the scripted server proves, they check what only a server can: the count of
//! DML's `OUTPUT` row, the catalog queries' columns, EXPORT_OBJECTS with quoted names, the grid's
//! SQL (NULL ordering, `IS NOT DISTINCT FROM`), the server's read-only mode, its cancel and time
//! out.

#![cfg(feature = "drv-vertica")]
#![allow(clippy::result_large_err)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use tabler_lib::commands::browse::{browse_page, fetch_cell_inner};
use tabler_lib::commands::query::query_limited;
use tabler_lib::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, StatementOutcome, TxStatement, connect,
    request_cancel,
};
use tabler_lib::error::AppError;
use tabler_lib::sqlgen::dialect::SqlDialect;
use tabler_lib::sqlgen::narrow::Narrowing;
use tabler_lib::sqlgen::sql::{self, TableTarget};
use tabler_lib::types::browse::{BrowseRequest, RowKey};
use tabler_lib::types::caps::Feature;
use tabler_lib::types::driver::ReadOnlyMode;
use tabler_lib::types::mutate::StagedChange;
use tabler_lib::types::query::StatementKind;
use tabler_lib::types::schema::{TableMeta, TableRef};
use tabler_lib::types::value::Value;

const VAR: &str = "TABLER_TEST_VERTICA_URL";
const BIG: &str = "9007199254740993";

/// A name for this run's objects.
fn scratch(what: &str) -> String {
    format!("tabler_it_{what}_{}", uuid::Uuid::new_v4().simple())
}

/// One statement on the editor, its rows kept.
async fn run(s: &dyn DbSession, sql: &str) -> Result<(StatementOutcome, CollectSink), AppError> {
    let mut sink = CollectSink::default();
    let outcome = s
        .run_statement(sql, 10_000, &mut sink, CancelSlot::default())
        .await?;
    Ok((outcome, sink))
}

async fn drop_table(s: &dyn DbSession, name: &str) {
    if let Err(e) = run(s, &format!("DROP TABLE IF EXISTS \"{name}\" CASCADE")).await {
        println!("dropping {name}: {e}");
    }
}

/// A table `name` (id INT primary key enabled, name VARCHAR, big INT) with three rows.
async fn make_table(s: &dyn DbSession, name: &str) -> TableMeta {
    run(
        s,
        &format!(
            "CREATE TABLE \"{name}\" (id INT NOT NULL, name VARCHAR(40) UNIQUE ENABLED, big INT, \
             PRIMARY KEY (id) ENABLED)"
        ),
    )
    .await
    .unwrap();
    for (id, n) in [(1, "a"), (2, "b"), (3, "c")] {
        let (out, _) = run(
            s,
            &format!("INSERT INTO \"{name}\" VALUES ({id}, '{n}', {BIG})"),
        )
        .await
        .unwrap();
        assert_eq!(out.kind, StatementKind::Affected);
        assert_eq!(out.affected, Some(1), "the OUTPUT row's count");
    }
    s.table_meta(&TableRef {
        schema: None,
        name: name.into(),
    })
    .await
    .unwrap()
}

fn update(s: &dyn DbSession, meta: &TableMeta, id: i64, name: &str) -> TxStatement {
    let narrowing = Narrowing::for_caps(&s.capabilities());
    let target = TableTarget {
        table: &meta.table,
        columns: &meta.columns,
    };
    let change = StagedChange::Update {
        row_id: id.to_string(),
        key: RowKey::Pk([("id".to_string(), Value::Int(id))].into()),
        changes: [("name".to_string(), Value::Text(name.into()))].into(),
        exprs: None,
    };
    let (built, expect) =
        sql::change_on(&narrowing, SqlDialect::Vertica, &target, &change, false).unwrap();
    let mut st = TxStatement::new(built.sql, built.params);
    st.expect_affected = expect;
    st
}

#[tokio::test]
#[ignore = "set TABLER_TEST_VERTICA_URL"]
async fn connects_lists_and_reports_the_server() {
    let Some((s, _)) = common::open(VAR).await else {
        return;
    };
    let info = s.server_info().await.unwrap();
    assert!(
        info.version.contains("Vertica") || info.version.contains("OpenText"),
        "{}",
        info.version
    );
    let dbs = s.list_databases().await.unwrap();
    assert!(dbs.iter().any(|d| d.is_current), "{dbs:?}");
    let schemas = s.list_schemas().await.unwrap();
    assert!(schemas.iter().any(|x| x.name == "v_catalog"), "{schemas:?}");
    s.ping().await.unwrap();
    let caps = s.capabilities();
    assert!(caps.has(Feature::Editor) && caps.has(Feature::CancelOnServer));
    assert_eq!(caps.safety.read_only, ReadOnlyMode::Server);
    let processes = s.list_processes().await.unwrap().unwrap();
    assert!(processes.iter().any(|p| p.current), "{processes:?}");
    let completion = s.completion_metadata().await.unwrap();
    assert!(completion.default_schema.is_some());
    s.close().await;
}

#[tokio::test]
#[ignore = "set TABLER_TEST_VERTICA_URL"]
async fn browses_counts_fetches_and_commits() {
    let Some((s, _)) = common::open(VAR).await else {
        return;
    };
    let name = scratch("grid");
    let meta = make_table(s.as_ref(), &name).await;
    assert_eq!(meta.primary_key, ["id"]);
    assert_eq!(meta.columns.len(), 3);
    assert!(
        meta.indexes.iter().any(|i| i.unique),
        "the unique constraint: {:?}",
        meta.indexes
    );
    let tables = s
        .list_tables(meta.table.schema.as_deref().unwrap())
        .await
        .unwrap();
    assert!(tables.iter().any(|t| t.name == name));
    let ddl = s.table_ddl(&meta.table).await.unwrap().unwrap();
    assert!(ddl.contains("CREATE TABLE"), "{ddl}");

    let req = BrowseRequest {
        table: meta.table.clone(),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit: 2,
        offset: 1,
        cursor: None,
    };
    let page = browse_page(s.as_ref(), &meta, &req, None).await.unwrap();
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.rows[0][2], Value::BigInt(BIG.into()));

    let narrowing = Narrowing::for_caps(&s.capabilities());
    let count = sql::count_on(
        &narrowing,
        SqlDialect::Vertica,
        &meta.table,
        &meta.columns,
        None,
    )
    .unwrap();
    let rows = s.query(&count.sql, &count.params).await.unwrap();
    assert_eq!(rows.rows, [[Value::Int(3)]]);
    let key = RowKey::Pk([("id".to_string(), Value::Int(2))].into());
    assert_eq!(
        fetch_cell_inner(s.as_ref(), &meta, "name", &key)
            .await
            .unwrap(),
        Value::Text("b".into())
    );

    // A commit whose second change breaks the unique constraint leaves both rows as they were.
    let err = s
        .transaction(vec![
            update(s.as_ref(), &meta, 1, "z"),
            update(s.as_ref(), &meta, 2, "c"),
        ])
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AppError::Db {
                statement_index: Some(1),
                ..
            }
        ),
        "{err:?}"
    );
    let (_, sink) = run(
        s.as_ref(),
        &format!("SELECT name FROM \"{name}\" WHERE id = 1"),
    )
    .await
    .unwrap();
    assert_eq!(sink.sets[0].rows, [[Value::Text("a".into())]]);
    let done = s
        .transaction(vec![update(s.as_ref(), &meta, 1, "y")])
        .await
        .unwrap();
    assert_eq!(done[0].affected, 1);

    drop_table(s.as_ref(), &name).await;
    s.close().await;
}

#[tokio::test]
#[ignore = "set TABLER_TEST_VERTICA_URL"]
async fn the_editor_keeps_its_transaction_and_caps_its_rows() {
    let Some((s, _)) = common::open(VAR).await else {
        return;
    };
    let name = scratch("editor");
    make_table(s.as_ref(), &name).await;
    run(s.as_ref(), "BEGIN").await.unwrap();
    run(s.as_ref(), &format!("DELETE FROM \"{name}\" WHERE id = 3"))
        .await
        .unwrap();
    assert_eq!(s.in_transaction(), Some(true));
    run(s.as_ref(), "ROLLBACK").await.unwrap();
    assert_eq!(s.in_transaction(), Some(false));
    let mut sink = CollectSink::default();
    let out = s
        .run_statement(
            "SELECT * FROM v_catalog.columns",
            5,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert!(out.truncated);
    assert_eq!(sink.sets[0].rows.len(), 5);
    run(s.as_ref(), "SELECT 1").await.unwrap();
    drop_table(s.as_ref(), &name).await;
    s.close().await;
}

#[tokio::test]
#[ignore = "set TABLER_TEST_VERTICA_URL"]
async fn cancel_and_time_out_stop_the_statement_on_the_server() {
    let Some((s, _)) = common::open(VAR).await else {
        return;
    };
    let slot = CancelSlot::default();
    let running = {
        let s = Arc::clone(&s);
        let slot = slot.clone();
        tokio::spawn(async move {
            s.run_statement("SELECT SLEEP(60)", 10, &mut CollectSink::default(), slot)
                .await
        })
    };
    let mut handle = None;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle = request_cancel(&slot);
        if handle.is_some() {
            break;
        }
    }
    s.cancel(handle.expect("the statement armed the slot"))
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(20), running)
        .await
        .expect("the cancel returns")
        .unwrap();
    assert!(matches!(outcome, Err(AppError::Cancelled)), "{outcome:?}");
    let timed = query_limited(
        s.as_ref(),
        "SELECT SLEEP(60)",
        &[],
        Some(Duration::from_millis(500)),
    )
    .await;
    // SLEEP calls a side-effect function: on a writable session the read runs and times out.
    assert!(
        matches!(timed, Err(AppError::QueryTimeout { .. })),
        "{timed:?}"
    );
    s.ping().await.unwrap();
    s.close().await;
}

#[tokio::test]
#[ignore = "set TABLER_TEST_VERTICA_URL"]
async fn a_read_only_profile_is_refused_writes_by_the_server() {
    let Some(url) = common::url(VAR) else {
        return;
    };
    common::install_crypto();
    let (mut profile, password) = common::profile_from_url(&url);
    profile.options.read_only = true;
    let s = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap();
    let name = scratch("ro");
    // The session refuses before sending…
    let refused = run(s.as_ref(), &format!("CREATE TABLE \"{name}\" (a INT)")).await;
    assert!(
        matches!(refused, Err(AppError::InvalidInput(_))),
        "{refused:?}"
    );
    // …and the server would too: its transaction mode is read only.
    let (_, sink) = run(s.as_ref(), "SHOW TRANSACTION_READ_ONLY").await.unwrap();
    let setting = sink.sets[0].rows[0]
        .get(1)
        .and_then(Value::as_text)
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(["on", "true"].contains(&setting.as_str()), "{setting}");
    s.close().await;
}
