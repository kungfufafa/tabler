//! Cassandra and ScyllaDB against a real server: the cassandra lane's suite
//! (docs/drivers/cassandra.md). Built only with `drv-cassandra` (`required-features` in
//! Cargo.toml) and run with `node dev/scripts/lane.mjs cassandra it`, which loads `.env`. Each
//! test skips itself while `TABLER_TEST_CASSANDRA_URL` (`cassandra://user:password@host:9042/
//! keyspace`, the keyspace existing) is unset, and is `#[ignore]`d so that a plain `cargo test
//! --all-features` never reaches a server.
//!
//! The tests make their own tables (`tabler_it_<uuid>` in the URL's keyspace) and drop them.
//! Besides what the scripted node proves, they check what only a server can: the scylla
//! driver's paging state, bound values typed by the server's markers, lightweight transactions'
//! `[applied]`, schema agreement after DDL, and the driver's own time out.

#![cfg(feature = "drv-cassandra")]
#![allow(clippy::result_large_err)]

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tabler_lib::commands::query::query_limited;
use tabler_lib::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, StatementOutcome, TxStatement, connect,
    request_cancel,
};
use tabler_lib::error::AppError;
use tabler_lib::sqlgen::narrow::Narrowing;
use tabler_lib::sqlgen::sql::{self, TableTarget};
use tabler_lib::types::browse::{BrowseRequest, RowKey};
use tabler_lib::types::mutate::StagedChange;
use tabler_lib::types::query::StatementKind;
use tabler_lib::types::schema::{TableMeta, TableRef};
use tabler_lib::types::value::Value;

const VAR: &str = "TABLER_TEST_CASSANDRA_URL";
const BIG: i64 = (1 << 53) + 1;

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
    if let Err(e) = run(s, &format!("DROP TABLE IF EXISTS \"{name}\"")).await {
        println!("dropping {name}: {e}");
    }
}

fn keyspace(profile: &tabler_lib::types::profile::ConnectionProfile) -> String {
    profile.database.clone().expect("the URL names a keyspace")
}

/// The statement the grid's commit sends for one change.
fn planned(s: &dyn DbSession, meta: &TableMeta, change: &StagedChange) -> TxStatement {
    let narrowing = Narrowing::for_caps(&s.capabilities());
    let target = TableTarget {
        table: &meta.table,
        columns: &meta.columns,
    };
    let (built, expect) =
        sql::change_on(&narrowing, s.dialect(), &target, change, false).expect("a change");
    let mut st = TxStatement::new(built.sql, built.params);
    st.expect_affected = expect;
    st
}

fn key(id: i64) -> RowKey {
    RowKey::Pk(BTreeMap::from([
        ("id".to_string(), Value::Int(id)),
        ("seq".to_string(), Value::Int(0)),
    ]))
}

#[tokio::test]
#[ignore = "set TABLER_TEST_CASSANDRA_URL"]
async fn connects_lists_and_reads_structure_and_values() {
    let Some((s, profile)) = common::open(VAR).await else {
        return;
    };
    let ks = keyspace(&profile);
    let info = s.server_info().await.unwrap();
    println!("server: {}", info.version);
    assert!(info.version.starts_with("Cassandra") || info.version.starts_with("ScyllaDB"));
    s.ping().await.unwrap();
    assert!(
        s.list_databases()
            .await
            .unwrap()
            .iter()
            .any(|d| d.name == ks)
    );
    let name = scratch("values");
    run(
        s.as_ref(),
        &format!(
            "CREATE TABLE \"{name}\" (id int, seq int, big bigint, n varint, d decimal, \
             ts timestamp, l list<int>, m map<text, int>, PRIMARY KEY ((id), seq)) WITH \
             CLUSTERING ORDER BY (seq DESC)"
        ),
    )
    .await
    .unwrap();
    // Schema agreement was awaited: the new table is listed at once.
    let tables = s.list_tables(&ks).await.unwrap();
    assert!(tables.iter().any(|t| t.name == name));
    let meta = s
        .table_meta(&TableRef::new(Some(&ks), &name))
        .await
        .unwrap();
    assert_eq!(meta.primary_key, ["id", "seq"]);
    assert!(meta.ddl.unwrap_or_default().contains("CLUSTERING ORDER BY"));
    run(
        s.as_ref(),
        &format!(
            "INSERT INTO \"{name}\" (id, seq, big, n, d, ts, l, m) VALUES (1, 0, {BIG}, \
             -123456789012345678901234567890, -0.05, '2024-05-01 10:00:00.123+0000', [1, 2], \
             {{'a': 1}})"
        ),
    )
    .await
    .unwrap();
    let (out, sink) = run(
        s.as_ref(),
        &format!("SELECT big, n, d, ts, l, m FROM \"{name}\""),
    )
    .await
    .unwrap();
    assert_eq!(out.kind, StatementKind::Rows);
    let row = &sink.sets[0].rows[0];
    assert_eq!(row[0], Value::BigInt(BIG.to_string()));
    assert_eq!(
        row[1],
        Value::BigInt("-123456789012345678901234567890".into())
    );
    assert_eq!(row[2], Value::Decimal("-0.05".into()));
    assert_eq!(
        row[3],
        Value::TimestampTz("2024-05-01 10:00:00.123+0000".into())
    );
    assert_eq!(row[4], Value::Json("[1, 2]".into()));
    assert_eq!(row[5], Value::Json("{\"a\": 1}".into()));
    drop_table(s.as_ref(), &name).await;
}

#[tokio::test]
#[ignore = "set TABLER_TEST_CASSANDRA_URL"]
async fn the_grid_pages_counts_fetches_and_commits_by_key() {
    let Some((s, profile)) = common::open(VAR).await else {
        return;
    };
    let ks = keyspace(&profile);
    let name = scratch("grid");
    run(
        s.as_ref(),
        &format!("CREATE TABLE \"{name}\" (id int, seq int, val text, PRIMARY KEY ((id), seq))"),
    )
    .await
    .unwrap();
    for id in 1..=5 {
        run(
            s.as_ref(),
            &format!("INSERT INTO \"{name}\" (id, seq, val) VALUES ({id}, 0, 'v{id}')"),
        )
        .await
        .unwrap();
    }
    let meta = s
        .table_meta(&TableRef::new(Some(&ks), &name))
        .await
        .unwrap();
    let source = s.table_source().expect("Cassandra pages its own rows");
    let mut seen = 0;
    let mut cursor = None;
    for _ in 0..10 {
        let page = source
            .browse(
                &meta,
                &BrowseRequest {
                    table: meta.table.clone(),
                    columns: None,
                    filter: None,
                    sort: Vec::new(),
                    limit: 2,
                    offset: 0,
                    cursor: cursor.clone(),
                },
                CancelSlot::default(),
            )
            .await
            .unwrap();
        seen += page.rows.len();
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen, 5);
    let count = source
        .count(&meta, None, true, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((count.count, count.exact), (5, true));
    let cell = source.fetch_cell(&meta, &key(3), "val").await.unwrap();
    assert_eq!(cell, Value::Text("v3".into()));

    // The second change addresses a row that is not there: its LWT does not apply.
    let change = |id: i64, val: &str| StagedChange::Update {
        row_id: format!("r{id}"),
        key: key(id),
        changes: BTreeMap::from([("val".to_string(), Value::Text(val.into()))]),
        exprs: None,
    };
    let err = s
        .transaction(vec![
            planned(s.as_ref(), &meta, &change(1, "one")),
            planned(s.as_ref(), &meta, &change(99, "none")),
            planned(s.as_ref(), &meta, &change(2, "two")),
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
    assert_eq!(
        source.fetch_cell(&meta, &key(1), "val").await.unwrap(),
        Value::Text("one".into())
    );
    assert_eq!(
        source.fetch_cell(&meta, &key(2), "val").await.unwrap(),
        Value::Text("v2".into())
    );
    drop_table(s.as_ref(), &name).await;
}

#[tokio::test]
#[ignore = "set TABLER_TEST_CASSANDRA_URL"]
async fn the_editor_caps_cancels_and_times_out() {
    let Some((s, _)) = common::open(VAR).await else {
        return;
    };
    run(s.as_ref(), "CONSISTENCY ONE").await.unwrap();
    let mut sink = CollectSink::default();
    let out = s
        .run_statement(
            "SELECT keyspace_name, table_name, column_name FROM system_schema.columns",
            3,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!((out.row_count, out.truncated), (3, true));
    // A run cancelled before it starts never runs.
    let slot = CancelSlot::default();
    let _ = request_cancel(&slot);
    assert!(matches!(
        s.run_statement(
            "SELECT * FROM system.local",
            10,
            &mut CollectSink::default(),
            slot
        )
        .await,
        Err(AppError::Cancelled)
    ));
    let limited = query_limited(
        s.as_ref(),
        "SELECT * FROM system_schema.columns",
        &[],
        Some(Duration::from_micros(1)),
    )
    .await;
    assert!(
        matches!(limited, Err(AppError::QueryTimeout { .. })),
        "{limited:?}"
    );
    s.ping().await.unwrap();
}

#[tokio::test]
#[ignore = "set TABLER_TEST_CASSANDRA_URL"]
async fn read_only_refuses_writes() {
    let Some(url) = common::url(VAR) else {
        return;
    };
    common::install_crypto();
    let (mut profile, password) = common::profile_from_url(&url);
    profile.options.read_only = true;
    let s: Arc<dyn DbSession> = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap();
    let name = scratch("ro");
    for sql in [
        format!("CREATE TABLE \"{name}\" (id int PRIMARY KEY)"),
        format!("INSERT INTO \"{name}\" (id) VALUES (1)"),
        "BEGIN BATCH DELETE FROM x WHERE id = 1; APPLY BATCH".to_string(),
    ] {
        assert!(run(s.as_ref(), &sql).await.is_err(), "{sql}");
        assert!(s.execute(&sql, &[]).await.is_err(), "{sql}");
    }
    let ks = keyspace(&profile);
    assert!(
        !s.list_tables(&ks)
            .await
            .unwrap()
            .iter()
            .any(|t| t.name == name),
        "nothing was created"
    );
    run(s.as_ref(), "SELECT * FROM system.local").await.unwrap();
}
