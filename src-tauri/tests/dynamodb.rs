//! DynamoDB against a real server (docs/drivers/dynamodb.md). Built only with `drv-dynamodb`
//! (`required-features` in Cargo.toml) and run with `node dev/scripts/lane.mjs dynamodb it`, which
//! loads `.env`. Each test skips itself while `TABLER_TEST_DYNAMODB_URL` is unset, and is
//! `#[ignore]`d so that a plain `cargo test --all-features` never reaches a server (AWS bills the
//! requests).
//!
//! `TABLER_TEST_DYNAMODB_URL=dynamodb://<key id>[:<secret>]@<host>[:<port>]/<region>`:
//! - DynamoDB Local or another emulator at `http://<host>:<port>/` (`dynamodb://local@localhost:8000/us-east-1`
//!   signs with Local's dummy keys);
//! - AWS itself with the host `aws` (`dynamodb://AKIA…:<secret>@aws/eu-west-1`).
//!
//! Each test creates its own table (`tabler_it_…`, on demand) and deletes it at the end; a test
//! that fails leaves its table behind.

#![cfg(feature = "drv-dynamodb")]

mod common;

use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tabler_lib::commands::safety::READ_ONLY_MESSAGE;
use tabler_lib::drivers::native::NativeSession;
use tabler_lib::drivers::{CancelSlot, ConnectTarget, RowSink, Session, open, request_cancel};
use tabler_lib::error::AppError;
use tabler_lib::types::browse::{BrowseRequest, RowKey};
use tabler_lib::types::mutate::{ApplyRequest, StagedChange};
use tabler_lib::types::profile::{ConnectionProfile, DriverParams};
use tabler_lib::types::query::{ResultColumn, StatementKind};
use tabler_lib::types::schema::{TableMeta, TableRef};
use tabler_lib::types::value::Value;

const VAR: &str = "TABLER_TEST_DYNAMODB_URL";

/// The profile of the URL (see the module's doc), and its secret.
fn profile(url: &str, read_only: bool) -> (ConnectionProfile, Option<String>) {
    let (mut p, password) = common::profile_from_url(url);
    let host = p.host.take().unwrap_or_default();
    let port = p.port.take();
    let mut params = serde_json::Map::new();
    if host != "aws" {
        let port = port.unwrap_or(8000);
        params.insert("endpoint".into(), json!(format!("http://{host}:{port}/")));
    }
    let local = p.user.as_deref() == Some("local");
    params.insert(
        "auth".into(),
        json!(if local { "local" } else { "accessKey" }),
    );
    p.options.driver_params = Some(DriverParams(params));
    p.options.read_only = read_only;
    (p, password)
}

/// A session of the server in `TABLER_TEST_DYNAMODB_URL`; `None` (skipped) when it is unset.
async fn connect(read_only: bool) -> Option<Arc<dyn NativeSession>> {
    let url = common::url(VAR)?;
    common::install_crypto();
    let (p, password) = profile(&url, read_only);
    let session = open(ConnectTarget {
        profile: &p,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap_or_else(|e| panic!("{VAR}: connect failed: {e}"));
    match session {
        Session::Native(s) => Some(s),
        Session::Sql(_) => panic!("DynamoDB opens a native session"),
    }
}

#[derive(Default)]
struct Rows {
    columns: Vec<ResultColumn>,
    rows: Vec<Vec<Value>>,
}

impl RowSink for Rows {
    fn begin_set(&mut self, columns: Vec<ResultColumn>) {
        self.columns = columns;
    }
    fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        self.rows.extend(rows);
        ControlFlow::Continue(())
    }
}

/// Runs one console command and returns its rows (or its error's text).
async fn run(s: &dyn NativeSession, command: &str) -> Result<Rows, String> {
    let mut rows = Rows::default();
    s.run_console(command, 1000, &mut rows, CancelSlot::default())
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// A new table `tabler_it_…` (partition key `pk` S, sort key `n` N) holding five items, active.
async fn create_table(s: &dyn NativeSession) -> String {
    let name = format!("tabler_it_{}", uuid::Uuid::new_v4().simple());
    let request = json!({
        "TableName": name,
        "AttributeDefinitions": [
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "n", "AttributeType": "N"},
        ],
        "KeySchema": [
            {"AttributeName": "pk", "KeyType": "HASH"},
            {"AttributeName": "n", "KeyType": "RANGE"},
        ],
        "BillingMode": "PAY_PER_REQUEST",
    });
    run(s, &format!("CreateTable {request}")).await.unwrap();
    for _ in 0..120 {
        let reply = run(s, &format!("DescribeTable {}", json!({"TableName": name})))
            .await
            .unwrap();
        if matches!(&reply.rows[0][0], Value::Json(j) if j.contains("\"ACTIVE\"")) {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let items = [
        json!({"pk": {"S": "a"}, "n": {"N": "1"}, "big": {"N": "9007199254740993"}}),
        json!({"pk": {"S": "a"}, "n": {"N": "2"}, "price": {"N": "19.99"}}),
        json!({"pk": {"S": "a"}, "n": {"N": "3"}, "at": {"S": "2026-10-30T09:15:00Z"}}),
        json!({"pk": {"S": "a"}, "n": {"N": "4"}, "doc": {"M": {"x": {"L": [{"N": "1"}]}}}}),
        json!({"pk": {"S": "b"}, "n": {"N": "1"}, "name": {"S": "bee"}}),
    ];
    for item in items {
        let put = json!({"TableName": name, "Item": item});
        run(s, &format!("PutItem {put}")).await.unwrap();
    }
    name
}

async fn drop_table(s: &dyn NativeSession, name: &str) {
    run(s, &format!("DeleteTable {}", json!({"TableName": name})))
        .await
        .unwrap();
}

fn table(name: &str, meta: &TableMeta) -> TableRef {
    TableRef::new(meta.table.schema.as_deref(), name)
}

fn request(t: &TableRef, limit: u32, cursor: Option<String>) -> BrowseRequest {
    BrowseRequest {
        table: t.clone(),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit,
        offset: 0,
        cursor,
    }
}

fn key(pk: &str, n: i64) -> RowKey {
    RowKey::Pk(BTreeMap::from([
        ("pk".to_string(), Value::Text(pk.into())),
        ("n".to_string(), Value::Int(n)),
    ]))
}

/// Connect, the sidebar, grid pages to the end, the count, a cell, a commit with one failing
/// change, the console, a cancel and a time out, on one table.
#[tokio::test]
#[ignore = "set TABLER_TEST_DYNAMODB_URL"]
async fn browse_edit_and_console() {
    let Some(s) = connect(false).await else {
        return;
    };
    s.ping().await.unwrap();
    let info = s.server_info().await.unwrap();
    assert!(info.version.starts_with("DynamoDB"), "{}", info.version);
    let name = create_table(s.as_ref()).await;
    let region = s.list_schemas().await.unwrap()[0].name.clone();
    let tables = s.list_tables(&region).await.unwrap();
    assert!(tables.iter().any(|t| t.name == name));
    let meta = s
        .table_meta(&TableRef::new(Some(&region), &name))
        .await
        .unwrap();
    assert_eq!(meta.primary_key, ["pk", "n"]);
    let t = table(&name, &meta);
    let definition = s.definition(&t).await.unwrap().unwrap();
    assert!(definition.starts_with("CreateTable {"));

    // Pages of two to the end.
    let mut cursor = None;
    let mut seen = 0;
    let mut values = Vec::new();
    for _ in 0..10 {
        let page = s
            .browse(&meta, &request(&t, 2, cursor), CancelSlot::default())
            .await
            .unwrap();
        seen += page.rows.len();
        for (i, c) in page.columns.iter().enumerate() {
            for row in &page.rows {
                values.push((c.name.clone(), row[i].clone()));
            }
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen, 5);
    let has = |name: &str, v: Value| values.iter().any(|(n, x)| n == name && *x == v);
    assert!(has("big", Value::BigInt("9007199254740993".into())));
    assert!(has("price", Value::Decimal("19.99".into())));
    assert!(has("at", Value::Text("2026-10-30T09:15:00Z".into())));
    let exact = s
        .count(&meta, None, true, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((exact.count, exact.exact), (5, true));
    let cell = s.fetch_cell(&meta, &key("a", 4), "doc").await.unwrap();
    assert!(matches!(cell, Value::Json(_)), "{cell:?}");

    // A commit with one failing change (an insert of a key that exists) applies none of it.
    let req = ApplyRequest {
        table: t.clone(),
        changes: vec![
            StagedChange::Update {
                row_id: "u".into(),
                key: key("b", 1),
                changes: BTreeMap::from([("name".to_string(), Value::Text("wasp".into()))]),
                exprs: None,
            },
            StagedChange::Insert {
                row_id: "i".into(),
                values: BTreeMap::from([
                    ("pk".to_string(), Value::Text("a".into())),
                    ("n".to_string(), Value::Int(1)),
                ]),
                exprs: None,
            },
        ],
        returning: false,
    };
    // The grid loaded `b` above, so the update is guarded by its loaded name.
    let plan = s.plan_changes(&meta, &req).unwrap();
    let out = s.apply_changes(&meta, &req, plan).await;
    assert!(!out.success);
    assert!(out.results.iter().all(|r| r.error.is_some()));
    assert_eq!(
        s.fetch_cell(&meta, &key("b", 1), "name").await.unwrap(),
        Value::Text("bee".into())
    );
    let ok = ApplyRequest {
        changes: vec![req.changes[0].clone()],
        ..req.clone()
    };
    let plan = s.plan_changes(&meta, &ok).unwrap();
    assert!(s.apply_changes(&meta, &ok, plan).await.success);
    assert_eq!(
        s.fetch_cell(&meta, &key("b", 1), "name").await.unwrap(),
        Value::Text("wasp".into())
    );

    // The console: a PartiQL read, a write's count.
    let rows = run(
        s.as_ref(),
        &format!("SELECT * FROM \"{name}\" WHERE \"pk\" = 'a'"),
    )
    .await
    .unwrap();
    assert_eq!(rows.rows.len(), 4);
    let mut sink = Rows::default();
    let out = s
        .run_console(
            &format!("UPDATE \"{name}\" SET \"name\" = 'moth' WHERE \"pk\" = 'b' AND \"n\" = 1"),
            10,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!((out.kind, out.affected), (StatementKind::Affected, Some(1)));
    // A DELETE of a key that does not exist succeeds and deletes nothing: no count is claimed.
    let out = s
        .run_console(
            &format!("DELETE FROM \"{name}\" WHERE \"pk\" = 'zz' AND \"n\" = 9"),
            10,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!((out.kind, out.affected), (StatementKind::Empty, None));

    // A cancel before the read starts sends nothing.
    let slot = CancelSlot::default();
    let _ = request_cancel(&slot);
    assert!(matches!(
        s.count(&meta, None, true, slot).await,
        Err(AppError::Cancelled)
    ));
    // The editor's time out: once the count is in flight (its first poll sends the request and
    // cannot see the reply), the cancel the time out sends stops it at once, and the session
    // goes on.
    let slot = CancelSlot::default();
    let counting = s.count(&meta, None, true, slot.clone());
    tokio::pin!(counting);
    let early = tokio::select! {
        biased;
        out = &mut counting => Some(out),
        () = std::future::ready(()) => None,
    };
    assert!(early.is_none(), "{early:?}");
    let handle = request_cancel(&slot).expect("the count armed its cancel");
    s.cancel(handle).await.unwrap();
    let out = tokio::time::timeout(Duration::from_secs(5), counting)
        .await
        .expect("the cancel stops the count at once");
    assert!(matches!(out, Err(AppError::Cancelled)), "{out:?}");
    s.ping().await.unwrap();

    drop_table(s.as_ref(), &name).await;
    s.close().await;
}

/// The refusal of a read-only session (`commands::safety::READ_ONLY_MESSAGE`).
fn is_read_only(e: &AppError) -> bool {
    matches!(e, AppError::InvalidInput(m) if m == READ_ONLY_MESSAGE)
}

/// A read-only profile's session refuses every write before it is sent (the console's, a
/// commit's plan and a commit planned elsewhere), with the read-only refusal; the table is
/// untouched and its reads work.
#[tokio::test]
#[ignore = "set TABLER_TEST_DYNAMODB_URL"]
async fn read_only_refuses_writes() {
    let Some(writer) = connect(false).await else {
        return;
    };
    let Some(s) = connect(true).await else {
        return;
    };
    let name = create_table(writer.as_ref()).await;
    let region = s.list_schemas().await.unwrap()[0].name.clone();
    let meta = s
        .table_meta(&TableRef::new(Some(&region), &name))
        .await
        .unwrap();
    for write in [
        format!("DELETE FROM \"{name}\" WHERE pk = 'a' AND n = 1"),
        format!("UPDATE \"{name}\" SET price = 1 WHERE pk = 'a' AND n = 1"),
        format!("DeleteTable {}", json!({"TableName": name})),
        format!(
            "PutItem {}",
            json!({"TableName": name, "Item": {"pk": {"S": "z"}, "n": {"N": "9"}}})
        ),
    ] {
        let mut sink = Rows::default();
        match s
            .run_console(&write, 10, &mut sink, CancelSlot::default())
            .await
        {
            Err(e) => assert!(is_read_only(&e), "{write}: {e}"),
            Ok(out) => panic!("{write}: {out:?}"),
        }
    }
    let req = ApplyRequest {
        table: table(&name, &meta),
        changes: vec![StagedChange::Delete {
            row_id: "d".into(),
            key: key("a", 1),
        }],
        returning: false,
    };
    match s.plan_changes(&meta, &req) {
        Err(e) => assert!(is_read_only(&e), "{e}"),
        Ok(_) => panic!("a read-only session planned a commit"),
    }
    // A plan made by a writable session is refused too, change by change.
    let plan = writer.plan_changes(&meta, &req).unwrap();
    let out = s.apply_changes(&meta, &req, plan).await;
    assert!(!out.success);
    assert!(
        out.results.iter().all(|r| r.affected == 0
            && r.error
                .as_ref()
                .is_some_and(|e| e.message == READ_ONLY_MESSAGE)),
        "{:?}",
        out.results
    );

    // Nothing reached the server: the table, its five items and the deleted one's values are
    // there, the put item is not.
    let exact = s
        .count(&meta, None, true, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(exact.count, 5);
    assert_eq!(
        s.fetch_cell(&meta, &key("a", 1), "big").await.unwrap(),
        Value::BigInt("9007199254740993".into())
    );
    assert!(matches!(
        s.fetch_cell(&meta, &key("a", 2), "price").await,
        Ok(Value::Decimal(ref d)) if d == "19.99"
    ));
    assert!(matches!(
        s.fetch_cell(&meta, &key("z", 9), "pk").await,
        Err(AppError::NotFound(_))
    ));
    drop_table(writer.as_ref(), &name).await;
}
