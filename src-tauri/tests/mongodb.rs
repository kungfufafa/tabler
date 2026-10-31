//! MongoDB against a real server (docs/drivers/mongodb.md). Built only with `drv-mongodb`
//! (`required-features` in Cargo.toml) and run with `node dev/scripts/lane.mjs mongodb it`, which
//! loads `.env`. Each test skips itself while `TABLER_TEST_MONGODB_URL` is unset, and is
//! `#[ignore]`d so that a plain `cargo test --all-features` never reaches a server.
//!
//! The URL is a connection string (`mongodb://user:password@host:27017/?authSource=admin`, or
//! `mongodb+srv://…`); its password goes to the password field and the rest to the profile's
//! `uri`, so its options (TLS, replica set) apply. Every test works in a scratch database of its
//! own, `tabler_it_<id>`, dropped at the end.

#![cfg(feature = "drv-mongodb")]

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tabler_lib::commands::console::run_console_inner;
use tabler_lib::drivers::native::NativeSession;
use tabler_lib::drivers::{CancelSlot, CollectSink, ConnectTarget, Session, open, request_cancel};
use tabler_lib::error::AppError;
use tabler_lib::types::browse::{BrowseRequest, RowKey};
use tabler_lib::types::caps::Feature;
use tabler_lib::types::mutate::ApplyRequest;
use tabler_lib::types::profile::ConnectionProfile;
use tabler_lib::types::query::{QueryEvent, RunOptions, StatementKind};
use tabler_lib::types::schema::TableRef;
use tabler_lib::types::value::Value;

const VAR: &str = "TABLER_TEST_MONGODB_URL";

/// The profile of `url` (its password apart) and the password; `read_only` sets the profile's.
fn profile(url: &str, read_only: bool) -> (ConnectionProfile, Option<String>) {
    let (scheme, rest) = url.split_once("://").expect("a mongodb:// URL");
    let (user, password, tail) = match rest.split_once('@') {
        Some((auth, tail)) if !auth.contains('/') => {
            let (u, p) = auth.split_once(':').unwrap_or((auth, ""));
            (
                Some(decode(u)),
                Some(decode(p)).filter(|p| !p.is_empty()),
                tail,
            )
        }
        _ => (None, None, rest),
    };
    let profile = serde_json::from_value(json!({
        "id": uuid::Uuid::new_v4(), "name": "mongodb-test", "driver": "mongodb",
        "host": null, "user": user, "ssl": "prefer",
        "options": {
            "connectTimeoutSecs": 10, "readOnly": read_only,
            "driverParams": { "uri": format!("{scheme}://{tail}") },
        },
        "sortOrder": 0, "createdAt": "", "updatedAt": "",
    }))
    .expect("a profile");
    (profile, password)
}

fn decode(s: &str) -> String {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A session of the server in `VAR`, or `None` (skipped) when it is unset.
async fn session(read_only: bool) -> Option<Arc<dyn NativeSession>> {
    let url = common::url(VAR)?;
    common::install_crypto();
    let (profile, password) = profile(&url, read_only);
    let session = open(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap_or_else(|e| panic!("{VAR}: connect failed: {e}"));
    match session {
        Session::Native(n) => Some(n),
        Session::Sql(_) => panic!("MongoDB opened a SQL session"),
    }
}

/// A scratch database with `items` holding `n` documents `{_id: i, n: i * 10}`.
async fn scratch(s: &dyn NativeSession, n: i64) -> String {
    let db = format!("tabler_it_{}", uuid::Uuid::new_v4().simple());
    let docs: Vec<String> = (1..=n)
        .map(|i| format!("{{_id: {i}, n: {}}}", i * 10))
        .collect();
    run(
        s,
        &format!(
            "db.getSiblingDB('{db}').items.insertMany([{}])",
            docs.join(", ")
        ),
        10,
    )
    .await;
    db
}

async fn drop_scratch(s: &dyn NativeSession, db: &str) {
    s.call("dropDatabase", json!({ "database": db }))
        .await
        .unwrap();
}

async fn run(s: &dyn NativeSession, text: &str, max_rows: usize) -> (StatementKind, CollectSink) {
    let mut sink = CollectSink::default();
    let out = s
        .run_console(text, max_rows, &mut sink, CancelSlot::default())
        .await
        .unwrap_or_else(|e| panic!("{text}: {e}"));
    (out.kind, sink)
}

fn browse(table: &TableRef, limit: u32, offset: u64) -> BrowseRequest {
    serde_json::from_value(json!({
        "table": table, "columns": null, "filter": null, "limit": limit, "offset": offset,
        "sort": [{ "column": "_id", "dir": "asc", "nulls": null }],
    }))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "set TABLER_TEST_MONGODB_URL"]
async fn connects_lists_browses_counts_and_fetches() {
    let Some(s) = session(false).await else {
        return;
    };
    let info = s.server_info().await.unwrap();
    assert!(!info.version.is_empty());
    s.ping().await.unwrap();
    assert!(s.capabilities().has(Feature::EditRows));
    let db = scratch(s.as_ref(), 25).await;
    assert!(
        s.list_databases()
            .await
            .unwrap()
            .iter()
            .any(|d| d.name == db)
    );
    let tables = s.list_tables(&db).await.unwrap();
    assert_eq!(tables[0].name, "items");
    let table = TableRef::new(Some(&db), "items");
    let meta = s.table_meta(&table).await.unwrap();
    assert_eq!(meta.primary_key, ["_id"]);
    let page = s
        .browse(&meta, &browse(&table, 10, 20), CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 5);
    assert_eq!(page.rows[0][0], Value::Int(21));
    let count = s
        .count(&meta, None, true, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((count.count, count.exact), (25, true));
    let key = RowKey::Pk(BTreeMap::from([("_id".to_string(), Value::Int(7))]));
    assert_eq!(
        s.fetch_cell(&meta, &key, "n").await.unwrap(),
        Value::Int(70)
    );
    assert!(s.definition(&table).await.unwrap().is_some());
    drop_scratch(s.as_ref(), &db).await;
    s.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "set TABLER_TEST_MONGODB_URL"]
async fn commits_and_reports_the_failing_change() {
    let Some(s) = session(false).await else {
        return;
    };
    let db = scratch(s.as_ref(), 3).await;
    let table = TableRef::new(Some(&db), "items");
    let meta = s.table_meta(&table).await.unwrap();
    s.browse(&meta, &browse(&table, 10, 0), CancelSlot::default())
        .await
        .unwrap();
    let int = |n: i64| json!(Value::Int(n));
    let req: ApplyRequest = serde_json::from_value(json!({
        "table": table,
        "changes": [
            { "op": "update", "rowId": "a", "key": { "kind": "pk", "value": { "_id": int(1) } },
              "changes": { "n": int(11) } },
            { "op": "delete", "rowId": "b", "key": { "kind": "pk", "value": { "_id": int(99) } } },
        ],
    }))
    .unwrap();
    let plan = s.plan_changes(&meta, &req).unwrap();
    let result = s.apply_changes(&meta, &req, plan).await;
    assert!(!result.success);
    assert!(result.results[1].error.is_some());
    let key = RowKey::Pk(BTreeMap::from([("_id".to_string(), Value::Int(1))]));
    let n = s.fetch_cell(&meta, &key, "n").await.unwrap();
    // A replica set rolls the transaction back; a standalone server keeps the first change.
    if s.capabilities().data.transactions == tabler_lib::types::caps::TxMode::Atomic {
        assert_eq!(n, Value::Int(10));
    } else {
        assert_eq!(n, Value::Int(11));
    }
    drop_scratch(s.as_ref(), &db).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "set TABLER_TEST_MONGODB_URL"]
async fn the_console_runs_cancels_and_times_out() {
    let Some(s) = session(false).await else {
        return;
    };
    let db = scratch(s.as_ref(), 5).await;
    let (kind, sink) = run(
        s.as_ref(),
        &format!("db.getSiblingDB('{db}').items.find()"),
        2,
    )
    .await;
    assert_eq!(kind, StatementKind::Rows);
    assert_eq!(sink.sets[0].rows.len(), 2);
    // A read that takes a second per document on the server is cancelled through killOp.
    let slow = format!(
        "db.getSiblingDB('{db}').items.find({{$where: 'function() {{ var e = Date.now() + 1000; \
         while (Date.now() < e) {{}} return true; }}'}})"
    );
    let slot = CancelSlot::default();
    let run_slow = {
        let (s, slow, slot) = (s.clone(), slow.clone(), slot.clone());
        tokio::spawn(async move {
            let mut sink = CollectSink::default();
            s.run_console(&slow, 100, &mut sink, slot).await
        })
    };
    while !slot.is_armed() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let handle = request_cancel(&slot).unwrap();
    s.cancel(handle).await.unwrap();
    let err = run_slow.await.unwrap().unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
    // The query time out cancels a statement still running.
    let options = RunOptions {
        timeout_secs: 1,
        ..RunOptions::default()
    };
    let mut events = Vec::new();
    let results = run_console_inner(
        s.as_ref(),
        &slow,
        &options,
        &CancelSlot::default(),
        &mut |e: QueryEvent| events.push(e),
    )
    .await;
    assert_eq!(results[0].error.as_ref().unwrap().code, "QUERY_TIMEOUT");
    drop_scratch(s.as_ref(), &db).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "set TABLER_TEST_MONGODB_URL"]
async fn a_read_only_session_refuses_writes() {
    let Some(writer) = session(false).await else {
        return;
    };
    let db = scratch(writer.as_ref(), 2).await;
    let Some(s) = session(true).await else {
        return;
    };
    let table = TableRef::new(Some(&db), "items");
    let meta = s.table_meta(&table).await.unwrap();
    let req: ApplyRequest = serde_json::from_value(json!({
        "table": table,
        "changes": [{ "op": "delete", "rowId": "a",
            "key": { "kind": "pk", "value": { "_id": json!(Value::Int(1)) } } }],
    }))
    .unwrap();
    assert!(s.plan_changes(&meta, &req).is_err());
    let mut sink = CollectSink::default();
    let insert = format!("db.getSiblingDB('{db}').items.insertOne({{a: 1}})");
    assert!(
        s.run_console(&insert, 10, &mut sink, CancelSlot::default())
            .await
            .is_err()
    );
    assert!(
        s.call("dropDatabase", json!({ "database": db }))
            .await
            .is_err()
    );
    let count = s
        .count(&meta, None, true, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(count.count, 2);
    drop_scratch(writer.as_ref(), &db).await;
}
