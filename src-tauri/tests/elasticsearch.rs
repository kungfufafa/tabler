//! Elasticsearch and OpenSearch against a real server (the elasticsearch lane's suite). Built only
//! with `drv-elasticsearch` (`required-features` in Cargo.toml) and run with
//! `node dev/scripts/lane.mjs elasticsearch it`, which loads `.env`. Each test skips itself while
//! `TABLER_TEST_ELASTICSEARCH_URL` is unset, and is `#[ignore]`d so that a plain
//! `cargo test --all-features` never reaches a server.
//!
//! `TABLER_TEST_ELASTICSEARCH_URL=elasticsearch://elastic:changeme@127.0.0.1:9200` (plain HTTP;
//! add `?ssl=true` for HTTPS without a certificate check). The suite works in an index of its own
//! (`tabler_it_<random>`) and deletes it at the end.

#![cfg(feature = "drv-elasticsearch")]

mod common;

use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use tabler_lib::drivers::native::NativeSession;
use tabler_lib::drivers::{CancelSlot, ConnectTarget, RowSink, Session, open, request_cancel};
use tabler_lib::error::AppError;
use tabler_lib::types::browse::{BrowseRequest, RowKey};
use tabler_lib::types::caps::Feature;
use tabler_lib::types::mutate::{ApplyRequest, StagedChange};
use tabler_lib::types::profile::{ConnectionProfile, SslMode};
use tabler_lib::types::query::{ResultColumn, RunOptions};
use tabler_lib::types::schema::TableRef;
use tabler_lib::types::value::Value;

const VAR: &str = "TABLER_TEST_ELASTICSEARCH_URL";
const DOCS: usize = 25;

#[derive(Default)]
struct Rows(Vec<Vec<Value>>);

impl RowSink for Rows {
    fn begin_set(&mut self, _columns: Vec<ResultColumn>) {}
    fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        self.0.extend(rows);
        ControlFlow::Continue(())
    }
}

fn test_profile(url: &str, read_only: bool) -> (ConnectionProfile, Option<String>) {
    let (mut profile, password) = common::profile_from_url(url);
    profile.ssl = if url.contains("ssl=true") {
        SslMode::Require
    } else {
        SslMode::Disable
    };
    profile.options.read_only = read_only;
    (profile, password)
}

async fn session(url: &str, read_only: bool) -> Arc<dyn NativeSession> {
    common::install_crypto();
    let (profile, password) = test_profile(url, read_only);
    match open(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap_or_else(|e| panic!("{VAR}: connect failed: {e}"))
    {
        Session::Native(s) => s,
        Session::Sql(_) => panic!("Elasticsearch opens a native session"),
    }
}

fn table(name: &str) -> TableRef {
    TableRef {
        schema: None,
        name: name.into(),
    }
}

fn ids_of(page: &tabler_lib::types::browse::BrowsePage) -> Vec<String> {
    let i = page.columns.iter().position(|c| c.name == "_id").unwrap();
    page.rows
        .iter()
        .map(|r| r[i].as_text().unwrap_or_default())
        .collect()
}

fn pk(row: &[Value], columns: &[tabler_lib::types::schema::ColumnMeta]) -> RowKey {
    RowKey::Pk(
        columns
            .iter()
            .zip(row)
            .filter(|(c, _)| c.is_pk)
            .map(|(c, v)| (c.name.clone(), v.clone()))
            .collect(),
    )
}

#[tokio::test]
#[ignore = "set TABLER_TEST_ELASTICSEARCH_URL"]
async fn real_server() {
    let Some(url) = common::url(VAR) else {
        return;
    };
    let s = session(&url, false).await;
    let info = s.server_info().await.unwrap();
    println!("connected to {}", info.version);
    s.ping().await.unwrap();
    let index = format!(
        "tabler_it_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let created = s
        .call(
            "createIndex",
            serde_json::json!({
                "name": index,
                "body": r#"{"mappings":{"properties":{"n":{"type":"long"},"big":{"type":"long"},"name":{"type":"text","fields":{"keyword":{"type":"keyword"}}},"at":{"type":"date"},"tags":{"type":"keyword"}}}}"#,
            }),
        )
        .await;
    created.unwrap_or_else(|e| panic!("create {index}: {e}"));
    let result = std::panic::AssertUnwindSafe(exercise(s.clone(), &url, &index));
    let outcome = futures::FutureExt::catch_unwind(result).await;
    let _ = s
        .call("deleteIndex", serde_json::json!({ "index": index }))
        .await;
    s.close().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn exercise(s: Arc<dyn NativeSession>, url: &str, index: &str) {
    let caps = s.capabilities();
    assert!(caps.has(Feature::Editor) && caps.has(Feature::InsertRows));

    // Inserts through the grid's commit: one `_bulk`, every change reported.
    let meta = s.table_meta(&table(index)).await.unwrap();
    let changes: Vec<StagedChange> = (0..DOCS)
        .map(|i| StagedChange::Insert {
            row_id: format!("r{i}"),
            values: [
                ("_id".to_string(), Value::Text(format!("d{i:02}"))),
                ("n".to_string(), Value::Int(i as i64)),
                (
                    "big".to_string(),
                    Value::BigInt("9007199254740993".to_string()),
                ),
                ("name".to_string(), Value::Text(format!("doc {i}"))),
                (
                    "at".to_string(),
                    Value::TimestampTz("2024-05-01T10:00:00.000Z".into()),
                ),
                ("tags".to_string(), Value::Json(r#"["a","b"]"#.into())),
            ]
            .into_iter()
            .collect(),
            exprs: None,
        })
        .collect();
    let req = ApplyRequest {
        table: table(index),
        changes,
        returning: true,
    };
    let plan = s.plan_changes(&meta, &req).unwrap();
    let out = s.apply_changes(&meta, &req, plan).await;
    assert!(out.success, "{:?}", out.error);

    // The sidebar, the structure, the grid paged by cursor (sorted by `n`).
    let tables = s.list_tables("").await.unwrap();
    assert!(tables.iter().any(|t| t.name == index));
    let meta = s.table_meta(&table(index)).await.unwrap();
    let mut cursor = None;
    let mut seen = Vec::new();
    let mut first_page = None;
    loop {
        let req = BrowseRequest {
            table: table(index),
            columns: None,
            filter: None,
            sort: vec![tabler_lib::types::browse::SortSpec {
                column: "n".into(),
                dir: tabler_lib::types::browse::SortDir::Asc,
                nulls: None,
            }],
            limit: 10,
            offset: 0,
            cursor: cursor.clone(),
        };
        let page = s.browse(&meta, &req, CancelSlot::default()).await.unwrap();
        seen.extend(ids_of(&page));
        if first_page.is_none() {
            first_page = Some(page.clone());
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    let expected: Vec<String> = (0..DOCS).map(|i| format!("d{i:02}")).collect();
    assert_eq!(seen, expected);
    let page = first_page.unwrap();
    let col = |name: &str| page.columns.iter().position(|c| c.name == name).unwrap();
    assert_eq!(
        page.rows[0][col("big")],
        Value::BigInt("9007199254740993".into())
    );
    assert_eq!(
        page.rows[0][col("at")],
        Value::TimestampTz("2024-05-01T10:00:00.000Z".into())
    );
    assert_eq!(
        page.rows[0][col("tags")],
        Value::Json(r#"["a","b"]"#.into())
    );

    // Counts and one cell.
    let count = s
        .count(&meta, None, true, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(count.count, DOCS as i64);
    let key0 = pk(&page.rows[0], &page.columns);
    assert_eq!(
        s.fetch_cell(&meta, &key0, "name").await.unwrap(),
        Value::Text("doc 0".into())
    );

    // A commit with one failing change: row 1 read at an old `_seq_no` meets a conflict.
    let mut stale = BTreeMap::new();
    if let RowKey::Pk(k) = pk(&page.rows[1], &page.columns) {
        stale = k;
    }
    stale.insert("_seq_no".into(), Value::Int(9_999));
    let req = ApplyRequest {
        table: table(index),
        changes: vec![
            StagedChange::Update {
                row_id: "a".into(),
                key: key0,
                changes: [("name".to_string(), Value::Text("edited".into()))]
                    .into_iter()
                    .collect(),
                exprs: None,
            },
            StagedChange::Delete {
                row_id: "b".into(),
                key: RowKey::Pk(stale),
            },
        ],
        returning: true,
    };
    let plan = s.plan_changes(&meta, &req).unwrap();
    let out = s.apply_changes(&meta, &req, plan).await;
    assert!(!out.success);
    assert!(out.results[0].error.is_none());
    assert_eq!(
        out.results[1].error.as_ref().and_then(|e| e.vendor_code),
        Some(409)
    );

    // The console: a search, and SQL where the cluster has it.
    let mut rows = Rows::default();
    let ran = s
        .run_console(
            &format!("GET /{index}/_search\n{{\"size\": 3}}"),
            100,
            &mut rows,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(ran.row_count, 3);
    let mut rows = Rows::default();
    match s
        .run_console(
            &format!("SELECT n FROM \"{index}\" ORDER BY n"),
            7,
            &mut rows,
            CancelSlot::default(),
        )
        .await
    {
        Ok(out) => assert_eq!((out.row_count, out.truncated), (7, true)),
        Err(e) => println!("no SQL on this cluster: {e}"),
    }

    // A cancel and a time out: a health request that waits for more nodes than the cluster has
    // (a wait for a status is met by any better one, so it returns at once).
    let wait = "GET /_cluster/health?wait_for_nodes=1000&timeout=30s";
    let slot = CancelSlot::default();
    let run = {
        let (s, slot) = (s.clone(), slot.clone());
        tokio::spawn(async move {
            let mut rows = Rows::default();
            s.run_console(wait, 10, &mut rows, slot).await
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    if let Some(handle) = request_cancel(&slot) {
        s.cancel(handle).await.unwrap();
    }
    assert!(matches!(run.await.unwrap(), Err(AppError::Cancelled)));
    let options = RunOptions {
        timeout_secs: 1,
        ..RunOptions::default()
    };
    let results = tabler_lib::commands::console::run_console_inner(
        s.as_ref(),
        wait,
        &options,
        &CancelSlot::default(),
        &mut |_| {},
    )
    .await;
    assert_eq!(
        results[0].error.as_ref().map(|e| e.code.as_str()),
        Some(tabler_lib::error::QUERY_TIMEOUT_CODE)
    );

    // Running Queries lists the cluster's tasks (Serverless has no task API).
    if caps.has(Feature::ProcessList) {
        let processes = s.list_processes().await.unwrap().unwrap();
        assert!(processes.iter().any(|p| p.current));
    }
    // An object operation takes one index, never a pattern.
    assert!(
        s.call(
            "deleteIndex",
            serde_json::json!({ "index": format!("{index}*") })
        )
        .await
        .is_err()
    );

    // Read-only: every write path refuses.
    let ro = session(url, true).await;
    let meta = ro.table_meta(&table(index)).await.unwrap();
    assert!(ro.plan_changes(&meta, &req).is_err());
    let mut rows = Rows::default();
    for write in [
        format!("DELETE /{index}"),
        // A write once a decoding proxy has passed the path on.
        format!("GET /{index}%2F_flush"),
    ] {
        assert!(
            ro.run_console(&write, 10, &mut rows, CancelSlot::default())
                .await
                .is_err(),
            "{write}"
        );
    }
    assert!(
        ro.call("truncate", serde_json::json!({ "index": index }))
            .await
            .is_err()
    );
    ro.close().await;
    assert_eq!(
        s.count(&meta, None, true, CancelSlot::default())
            .await
            .unwrap()
            .count,
        DOCS as i64,
        "the conflicting delete removed nothing"
    );
}
