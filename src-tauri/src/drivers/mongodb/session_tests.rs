//! The session against `FakeMongo` served over the wire: the `mongodb` crate connects, signs in,
//! lists, browses, counts, edits, runs the console, cancels and times out as it would against a
//! server.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ::mongodb::bson::{DateTime, doc};
use serde_json::json;

use super::session::{MongoSession, OpTag};
use super::testing::FakeMongo;
use crate::commands::console::run_console_inner;
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::native::{CallKind, NativePlan, NativeSession, TableSource};
use crate::drivers::{CancelSlot, CollectSink, ConnectTarget, request_cancel};
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::native::mongo::{self, LoadedTypes};
use crate::types::admin::KillScope;
use crate::types::browse::{
    BrowseRequest, Conjunction, FilterGroup, FilterNode, FilterOp, RowIdentity, RowKey,
};
use crate::types::caps::{CountMode, Feature, Paging, RawFilter, SafeConfirm, SchemaMode, TxMode};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::ApplyRequest;
use crate::types::profile::ConnectionProfile;
use crate::types::query::{QueryEvent, RunOptions, StatementKind};
use crate::types::schema::{TableKind, TableMeta, TableRef};
use crate::types::value::Value;

fn profile(port: u16, extra: serde_json::Value) -> ConnectionProfile {
    let mut p = json!({ "host": "127.0.0.1", "port": port, "database": "hr",
        "options": { "connectTimeoutSecs": 5 } });
    if let (Some(p), Some(extra)) = (p.as_object_mut(), extra.as_object()) {
        for (k, v) in extra {
            p.insert(k.clone(), v.clone());
        }
    }
    super::params::tests::profile(p)
}

async fn open_with(
    f: &FakeMongo,
    extra: serde_json::Value,
    password: Option<&str>,
    command: Option<Arc<CommandPassword>>,
) -> Result<MongoSession, AppError> {
    let addr = f.serve().await;
    let p = profile(addr.port(), extra);
    let target = ConnectTarget {
        profile: &p,
        password,
        host_override: None,
    };
    MongoSession::open_with(target, command).await
}

async fn open(f: &FakeMongo) -> MongoSession {
    open_with(f, json!({}), None, None).await.unwrap()
}

/// `hr.people`: three people (int `_id`s), and `hr.adults`, a view of the older two.
fn people() -> FakeMongo {
    let f = FakeMongo::new();
    f.seed(
        "hr",
        "people",
        vec![
            doc! { "_id": 1, "name": "Ann", "age": 31 },
            doc! { "_id": 2, "name": "Bob", "age": 25 },
            doc! { "_id": 3, "name": "Cy", "age": 40, "tags": ["a", "b"] },
        ],
    );
    f.run(
        "hr",
        &doc! { "create": "adults", "viewOn": "people", "pipeline": [{ "$match": { "age": { "$gte": 30 } } }] },
    );
    f
}

fn table(name: &str) -> TableRef {
    TableRef::new(Some("hr"), name)
}

fn request(name: &str, value: serde_json::Value) -> BrowseRequest {
    let mut base =
        json!({ "table": table(name), "columns": null, "filter": null, "limit": 10, "offset": 0 });
    if let (Some(b), Some(v)) = (base.as_object_mut(), value.as_object()) {
        for (k, v) in v {
            b.insert(k.clone(), v.clone());
        }
    }
    serde_json::from_value(base).unwrap()
}

fn id_key(id: i64) -> RowKey {
    RowKey::Pk(BTreeMap::from([("_id".to_string(), Value::Int(id))]))
}

fn condition(column: &str, op: FilterOp, values: Vec<Value>) -> FilterGroup {
    FilterGroup {
        conjunction: Conjunction::And,
        items: vec![FilterNode::Condition {
            column: column.into(),
            op,
            values,
        }],
    }
}

async fn console(
    s: &MongoSession,
    text: &str,
    max_rows: usize,
) -> (Result<StatementKind, AppError>, CollectSink, Vec<String>) {
    let mut sink = CollectSink::default();
    let out = s
        .run_console(text, max_rows, &mut sink, CancelSlot::default())
        .await;
    let notices = out.as_ref().map(|o| o.notices.clone()).unwrap_or_default();
    (out.map(|o| o.kind), sink, notices)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connects_lists_and_says_what_it_offers() {
    let f = people();
    f.seed("shop", "orders", vec![doc! { "_id": 1 }]);
    let s = open(&f).await;
    let info = s.server_info().await.unwrap();
    assert_eq!(info.version, "8.0.0-fake");
    assert_eq!(info.current_database.as_deref(), Some("hr"));

    let caps = s.capabilities();
    for f in [
        Feature::StructureView,
        Feature::Editor,
        Feature::CancelOnServer,
        Feature::EditRows,
        Feature::InsertRows,
        Feature::DeleteRows,
        Feature::ServerExpressions,
        Feature::ExactCount,
        Feature::ViewDefinition,
        Feature::SwitchDatabase,
        Feature::ProcessList,
        Feature::KillProcess,
    ] {
        assert!(caps.has(f), "{f:?}");
    }
    for f in [
        Feature::EditWithoutKey,
        Feature::Returning,
        Feature::StructureEdit,
        Feature::TableDdl,
        Feature::Explain,
        Feature::Mcp,
        Feature::ExportTables,
    ] {
        assert!(!caps.has(f), "{f:?}");
    }
    assert_eq!(caps.namespaces.schemas, SchemaMode::DatabaseIsSchema);
    assert_eq!(caps.namespaces.labels.tables, "Collections");
    assert_eq!(caps.namespaces.default_schema.as_deref(), Some("hr"));
    assert_eq!(caps.data.paging, Paging::Offset);
    assert_eq!(caps.data.count, CountMode::Estimate);
    assert_eq!(caps.data.raw_filter, Some(RawFilter::Json));
    assert!(!caps.data.filter_ops.contains(&FilterOp::JsonContains));
    // A standalone server has no transactions: one change after the other.
    assert_eq!(caps.data.transactions, TxMode::Sequential);
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    assert_eq!(caps.safety.confirm, SafeConfirm::Password);
    assert_eq!(caps.session.keepalive_secs, Some(30));

    let dbs: Vec<String> = s
        .list_databases()
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert!(dbs.contains(&"hr".to_string()) && dbs.contains(&"shop".to_string()));
    let schemas = s.list_schemas().await.unwrap();
    assert_eq!(schemas.len(), 1);
    assert!(schemas[0].is_default && schemas[0].name == "hr");
    let tables = s.list_tables("hr").await.unwrap();
    let kinds: Vec<(&str, TableKind)> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(
        kinds,
        [("adults", TableKind::View), ("people", TableKind::Table)]
    );
    assert_eq!(s.list_tables("shop").await.unwrap()[0].name, "orders");
    s.ping().await.unwrap();
    // What the session sent is in its trace, as the console would read it back.
    let traced: Vec<String> = s.drain_trace().into_iter().map(|t| t.sql).collect();
    assert!(
        traced
            .iter()
            .any(|t| t.starts_with("db.getSiblingDB(\"hr\").runCommand({\"listCollections\"")),
        "{traced:?}"
    );
    // Keep-alive pings stay out of it.
    assert!(!traced.iter().any(|t| t.contains("\"ping\"")), "{traced:?}");

    // The structure: `_id` first as the key, a view without one, the definition with indexes.
    let meta = s.table_meta(&table("people")).await.unwrap();
    let names: Vec<&str> = meta.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["_id", "age", "name", "tags"]);
    assert_eq!(meta.primary_key, ["_id"]);
    assert_eq!(meta.row_estimate, Some(3));
    let view = s.table_meta(&table("adults")).await.unwrap();
    assert!(view.primary_key.is_empty());
    assert_eq!(view.kind, TableKind::View);
    f.run("hr", &doc! { "createIndexes": "people", "indexes": [{ "key": { "name": 1 }, "name": "name_1", "unique": true }] });
    let def = s.definition(&table("people")).await.unwrap().unwrap();
    assert_eq!(
        def,
        "db.createCollection(\"people\");\ndb.getCollection(\"people\").createIndex({\"name\": 1}, {\"name\": \"name_1\", \"unique\": true});"
    );
    let completion = s.completion_metadata().await.unwrap();
    assert_eq!(
        completion.schemas["hr"]["people"],
        ["_id", "age", "name", "tags"]
    );
    assert_eq!(completion.default_schema.as_deref(), Some("hr"));
    f.seed("hr", "later", vec![doc! { "_id": 1 }]);
    let completion = s.completion_metadata().await.unwrap();
    // A collection whose structure was not read yet has no fields to offer.
    assert!(completion.schemas["hr"]["later"].is_empty());

    s.close().await;
    assert!(matches!(s.ping().await, Err(AppError::NotConnected)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browses_by_page_counts_and_fetches_cells() {
    let f = FakeMongo::new();
    f.seed(
        "hr",
        "nums",
        (1..=25)
            .map(|i| doc! { "_id": i, "n": i64::from(i) * 10, "even": i % 2 == 0 })
            .collect(),
    );
    let s = open(&f).await;
    let meta = s.table_meta(&table("nums")).await.unwrap();
    let sort = json!([{ "column": "n", "dir": "desc", "nulls": null }]);
    let page = s
        .browse(
            &meta,
            &request("nums", json!({ "sort": sort, "offset": 20 })),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(page.identity, RowIdentity::Pk);
    assert_eq!(page.pk_columns, ["_id"]);
    let ids: Vec<&Value> = page.rows.iter().map(|r| &r[0]).collect();
    assert_eq!(
        ids,
        [
            &Value::Int(5),
            &Value::Int(4),
            &Value::Int(3),
            &Value::Int(2),
            &Value::Int(1)
        ]
    );
    assert!(
        page.sql.contains("\"sort\": {\"n\": -1, \"_id\": 1}"),
        "{}",
        page.sql
    );
    assert_eq!(page.next_cursor, None);

    // The filter bar: values are values, never operators.
    let filtered = BrowseRequest {
        filter: Some(condition("n", FilterOp::Ge, vec![Value::Int(200)])),
        ..request("nums", json!({}))
    };
    assert_eq!(
        s.browse(&meta, &filtered, CancelSlot::default())
            .await
            .unwrap()
            .rows
            .len(),
        6
    );
    let injected = BrowseRequest {
        filter: Some(condition(
            "n",
            FilterOp::Eq,
            vec![Value::Text("{\"$gt\": 0}".into())],
        )),
        ..request("nums", json!({}))
    };
    assert!(
        s.browse(&meta, &injected, CancelSlot::default())
            .await
            .unwrap()
            .rows
            .is_empty()
    );

    let count = s
        .count(&meta, None, false, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((count.count, count.exact), (25, false));
    let count = s
        .count(&meta, None, true, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((count.count, count.exact), (25, true));
    let even = condition("even", FilterOp::Eq, vec![Value::Bool(true)]);
    let count = s
        .count(&meta, Some(&even), false, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((count.count, count.exact), (12, true));

    assert_eq!(
        s.fetch_cell(&meta, &id_key(7), "n").await.unwrap(),
        Value::Int(70)
    );
    assert_eq!(
        s.fetch_cell(&meta, &id_key(7), "nope").await.unwrap(),
        super::values::missing()
    );
    let err = s.fetch_cell(&meta, &id_key(99), "n").await.unwrap_err();
    assert!(matches!(err, AppError::NotFound(_)), "{err:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn values_keep_their_types_in_the_grid() {
    let f = FakeMongo::new();
    let oid = ::mongodb::bson::oid::ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap();
    f.seed(
        "hr",
        "kinds",
        vec![
            doc! {
                "_id": oid,
                "big": 9_007_199_254_740_993_i64,
                "small": 5_i64,
                "price": super::values::parse_decimal("1.10").unwrap(),
                "at": DateTime::from_millis(1_714_557_600_000),
                "nested": { "a": [1, { "b": "c" }] },
            },
            doc! { "_id": "text id" },
        ],
    );
    let s = open(&f).await;
    let meta = s.table_meta(&table("kinds")).await.unwrap();
    let page = s
        .browse(&meta, &request("kinds", json!({})), CancelSlot::default())
        .await
        .unwrap();
    let col = |name: &str| meta.columns.iter().position(|c| c.name == name).unwrap();
    let row = &page.rows[0];
    assert_eq!(
        row[col("_id")],
        Value::other("objectId", "65f1a2b3c4d5e6f708192a3b")
    );
    assert_eq!(row[col("big")], Value::BigInt("9007199254740993".into()));
    assert_eq!(row[col("small")], Value::Int(5));
    assert_eq!(row[col("price")], Value::Decimal("1.10".into()));
    assert_eq!(
        row[col("at")],
        Value::TimestampTz("2024-05-01T10:00:00.000Z".into())
    );
    assert_eq!(
        row[col("nested")],
        Value::Json("{\"a\": [1, {\"b\": \"c\"}]}".into())
    );
    // A field the document lacks is missing, not null; a string `_id` stays a string.
    let second = &page.rows[1];
    assert_eq!(second[col("big")], super::values::missing());
    assert_eq!(second[col("_id")], Value::Text("text id".into()));
}

fn changes(meta: &TableMeta, list: serde_json::Value) -> ApplyRequest {
    serde_json::from_value(json!({ "table": meta.table, "changes": list })).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_standalone_server_applies_changes_one_after_the_other() {
    let f = people();
    let s = open(&f).await;
    let meta = s.table_meta(&table("people")).await.unwrap();
    s.browse(&meta, &request("people", json!({})), CancelSlot::default())
        .await
        .unwrap();
    let int = |n: i64| json!(Value::Int(n));
    let req = changes(
        &meta,
        json!([
            { "op": "update", "rowId": "a", "key": { "kind": "pk", "value": { "_id": int(1) } },
              "changes": { "age": json!(Value::Float(32.0)) } },
            { "op": "delete", "rowId": "b", "key": { "kind": "pk", "value": { "_id": int(9) } } },
            { "op": "insert", "rowId": "c", "values": { "_id": int(4), "name": json!(Value::Text("Di".into())) } },
        ]),
    );
    let plan = s.plan_changes(&meta, &req).unwrap();
    let preview: Vec<String> = plan.preview.iter().map(|(_, t)| t.clone()).collect();
    assert_eq!(
        preview[0],
        "db.runCommand({\"update\": \"people\", \"updates\": [{\"q\": {\"_id\": 1}, \"u\": {\"$set\": {\"age\": 32}}, \"multi\": false}], \"ordered\": true})"
    );
    let result = s.apply_changes(&meta, &req, plan).await;
    assert!(!result.success);
    assert_eq!(result.sql, preview);
    let r = &result.results;
    assert_eq!((r[0].affected, r[0].error.is_none()), (1, true));
    assert!(
        r[1].error
            .as_ref()
            .unwrap()
            .message
            .starts_with("no document matched")
    );
    assert_eq!(
        r[2].error.as_ref().unwrap().message,
        "not applied: an earlier change failed"
    );
    let docs = f.docs("hr", "people");
    // The Int32 stayed an Int32; the insert after the failure was not sent.
    assert_eq!(docs[0], doc! { "_id": 1, "name": "Ann", "age": 32 });
    assert_eq!(docs.len(), 3);
    assert!(f.commands_named("insert").is_empty());

    // Views cannot be edited.
    let view = s.table_meta(&table("adults")).await.unwrap();
    let err = s
        .plan_changes(&view, &changes(&view, json!([])))
        .err()
        .unwrap();
    assert!(err.to_string().contains("cannot be edited"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_expression_sets_the_servers_date() {
    let f = people();
    let s = open(&f).await;
    let meta = s.table_meta(&table("people")).await.unwrap();
    let req = changes(
        &meta,
        json!([{ "op": "update", "rowId": "a", "key": { "kind": "pk", "value": { "_id": json!(Value::Int(2)) } },
            "changes": {}, "exprs": { "seen": "currentTimestamp" } }]),
    );
    let plan = s.plan_changes(&meta, &req).unwrap();
    assert!(
        plan.preview[0]
            .1
            .contains("\"$currentDate\": {\"seen\": {\"$type\": \"date\"}}")
    );
    let result = s.apply_changes(&meta, &req, plan).await;
    assert!(result.success, "{:?}", result.error);
    assert!(matches!(
        f.docs("hr", "people")[1].get("seen"),
        Some(::mongodb::bson::Bson::DateTime(_))
    ));
    assert!(s.capabilities().data.server_exprs.len() == 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replica_set_commits_in_one_transaction() {
    let f = people();
    f.hello_with(doc! { "setName": "rs0", "isWritablePrimary": true });
    let s = open_with(
        &f,
        json!({ "options": { "connectTimeoutSecs": 5,
        "driverParams": { "replicaSet": "rs0", "directConnection": true } } }),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(s.capabilities().data.transactions, TxMode::Atomic);
    let meta = s.table_meta(&table("people")).await.unwrap();
    let int = |n: i64| json!(Value::Int(n));
    let failing = changes(
        &meta,
        json!([
            { "op": "delete", "rowId": "a", "key": { "kind": "pk", "value": { "_id": int(2) } } },
            { "op": "delete", "rowId": "b", "key": { "kind": "pk", "value": { "_id": int(9) } } },
        ]),
    );
    let plan = s.plan_changes(&meta, &failing).unwrap();
    let result = s.apply_changes(&meta, &failing, plan).await;
    assert!(!result.success);
    assert!(result.results[0].error.is_none() && result.results[0].affected == 0);
    assert!(result.results[1].error.is_some());
    // Rolled back: Bob is still there.
    assert_eq!(f.docs("hr", "people").len(), 3);
    assert_eq!(f.transactions(), (0, 1));
    let good = changes(
        &meta,
        json!([{ "op": "delete", "rowId": "a", "key": { "kind": "pk", "value": { "_id": int(2) } } }]),
    );
    let plan = s.plan_changes(&meta, &good).unwrap();
    let result = s.apply_changes(&meta, &good, plan).await;
    assert!(result.success, "{:?}", result.error);
    assert_eq!(f.docs("hr", "people").len(), 2);
    assert_eq!(f.transactions(), (1, 1));

    // A commit that failed without the server saying it aborted (a write concern error): it
    // may have been applied, and is said to be unknown.
    let next = changes(
        &meta,
        json!([{ "op": "delete", "rowId": "c", "key": { "kind": "pk", "value": { "_id": int(3) } } }]),
    );
    f.fail_next("commitTransaction", 64, "waiting for replication timed out");
    let plan = s.plan_changes(&meta, &next).unwrap();
    let result = s.apply_changes(&meta, &next, plan).await;
    assert!(!result.success);
    let error = result.error.unwrap();
    assert_eq!(
        error.message,
        "The commit's outcome is unknown, its changes may have been applied: waiting for \
         replication timed out"
    );
    assert!(error.hint.unwrap().starts_with("Refresh the data"));
    // One the server aborted (a transient transaction error): nothing was applied.
    f.fail_next_labelled(
        "commitTransaction",
        251,
        "transaction aborted",
        &["TransientTransactionError"],
    );
    let next = changes(
        &meta,
        json!([{ "op": "delete", "rowId": "d", "key": { "kind": "pk", "value": { "_id": int(1) } } }]),
    );
    let plan = s.plan_changes(&meta, &next).unwrap();
    let result = s.apply_changes(&meta, &next, plan).await;
    let error = result.error.unwrap();
    assert_eq!(
        (error.message.as_str(), error.hint),
        ("transaction aborted", None)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_console_runs_statements_with_a_row_cap() {
    let f = people();
    let s = open(&f).await;
    let split: Vec<String> = s
        .split_console("db.people.find()\n  .sort({_id: 1}); use shop\nshow collections")
        .into_iter()
        .map(|st| st.sql)
        .collect();
    assert_eq!(
        split,
        [
            "db.people.find()\n  .sort({_id: 1})",
            "use shop",
            "show collections"
        ]
    );
    assert_eq!(s.classify_console("db.people.find()"), StatementClass::Read);
    assert_eq!(
        s.classify_console("db.people.drop()"),
        StatementClass::Write
    );
    assert_eq!(s.classify_console("let x = 1"), StatementClass::Write);

    let (kind, sink, _) = console(&s, "db.people.find().sort({_id: 1})", 2).await;
    assert_eq!(kind.unwrap(), StatementKind::Rows);
    assert_eq!(sink.sets[0].rows.len(), 2);
    let names: Vec<&str> = sink.sets[0]
        .columns
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["_id", "age", "name"]);
    let mut capped = CollectSink::default();
    let out = s
        .run_console("db.people.find()", 2, &mut capped, CancelSlot::default())
        .await
        .unwrap();
    assert!(out.truncated && out.row_count == 2);
    let out = s
        .run_console("db.people.find()", 3, &mut capped, CancelSlot::default())
        .await
        .unwrap();
    assert!(!out.truncated && out.row_count == 3);

    let (_, sink, _) = console(&s, "db.people.countDocuments({age: {$gt: 26}})", 100).await;
    assert_eq!(sink.sets[0].rows, [[Value::Int(2)]]);
    let out = s
        .run_console(
            "db.people.insertOne({name: 'Di'})",
            100,
            &mut capped,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!((out.kind, out.affected), (StatementKind::Affected, Some(1)));
    let (_, _, notices) = console(&s, "db.people.updateMany({}, {$set: {seen: true}})", 10).await;
    assert_eq!(notices, ["matched 4, modified 4, upserted 0"]);
    let (kind, _, notices) = console(&s, "use shop", 10).await;
    assert_eq!(kind.unwrap(), StatementKind::Empty);
    assert_eq!(notices, ["switched to db shop"]);
    f.seed("shop", "carts", vec![doc! { "_id": 1 }]);
    let (_, sink, _) = console(&s, "show collections", 10).await;
    assert_eq!(sink.sets[0].rows, [[Value::Text("carts".into())]]);
    // A parse error says where.
    let (kind, _, _) = console(&s, "db.people.find({a: })", 10).await;
    let err = kind.unwrap_err().to_string();
    assert!(err.contains("(line 1, column"), "{err}");
    // A bulkWrite stops at its failing command and says so.
    console(&s, "use hr", 10).await.0.unwrap();
    let (kind, _, _) = console(
        &s,
        "db.people.bulkWrite([{insertOne: {document: {_id: 50}}}, {deleteOne: {filter: {_id: 2}}},          {insertOne: {document: {_id: 1}}}])",
        10,
    )
    .await;
    let err = kind.unwrap_err().to_string();
    assert!(
        err.starts_with("bulkWrite: command 3 of 3 failed (2 documents written before)"),
        "{err}"
    );

    // Fields only later documents have are named, not dropped without a word.
    f.seed(
        "hr",
        "wide",
        (0..205)
            .map(|i| {
                if i == 204 {
                    doc! { "_id": i, "late": true }
                } else {
                    doc! { "_id": i }
                }
            })
            .collect(),
    );
    let (_, sink, notices) = console(&s, "db.wide.find()", 1000).await;
    assert_eq!(sink.sets[0].rows.len(), 205);
    assert_eq!(
        notices,
        ["Fields that are not in the first 200 documents are not shown: late"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_without_sessions_is_read_and_cancelled_by_comment() {
    let f = people();
    f.hello_with(doc! { "logicalSessionTimeoutMinutes": ::mongodb::bson::Bson::Null });
    let s = Arc::new(open(&f).await);
    let meta = s.table_meta(&table("people")).await.unwrap();
    let page = s
        .browse(
            &meta,
            &request("people", json!({ "limit": 2 })),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 2);
    let (_, sink, _) = console(&s, "db.people.find().batchSize(1)", 10).await;
    assert_eq!(sink.sets[0].rows.len(), 3);
    f.delay("count", Duration::from_secs(10));
    let slot = CancelSlot::default();
    let count = {
        let (s, meta, slot) = (s.clone(), meta.clone(), slot.clone());
        tokio::spawn(async move { s.count(&meta, None, false, slot).await })
    };
    // Cancelled once it runs on the server, so the kill finds it however loaded the machine.
    for _ in 0..1000 {
        if slot.is_armed() && f.running() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    s.cancel(request_cancel(&slot).unwrap()).await.unwrap();
    let err = count.await.unwrap().unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
    let killed = f.commands_named("aggregate");
    let listing = killed
        .iter()
        .find(|c| format!("{c}").contains("\"$currentOp\""))
        .unwrap();
    // No session to match: the operation is found by its comment alone.
    assert!(!format!("{listing}").contains("lsid.id"), "{listing}");
    assert_eq!(f.commands_named("killOp").len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pages_of_large_documents_come_whole() {
    let f = FakeMongo::new();
    let text = "x".repeat(2000);
    f.seed(
        "hr",
        "logs",
        (0..10)
            .map(|i| doc! { "_id": i, "text": text.clone() })
            .collect(),
    );
    // About two documents to a batch, as 16 MiB holds about 300 documents of 56 KB.
    f.batch_bytes(5000);
    let s = open(&f).await;
    let meta = s.table_meta(&table("logs")).await.unwrap();
    let mut ids = Vec::new();
    for offset in [0_u64, 4, 8] {
        let req = request(
            "logs",
            json!({ "limit": 4, "offset": offset,
            "sort": [{ "column": "_id", "dir": "asc", "nulls": null }] }),
        );
        let page = s.browse(&meta, &req, CancelSlot::default()).await.unwrap();
        ids.extend(page.rows.into_iter().map(|r| r[0].clone()));
    }
    // Every document once: no page came back short, so no skip passed over any.
    assert_eq!(ids, (0..10).map(Value::Int).collect::<Vec<_>>());
    assert!(f.commands_named("getMore").len() >= 3);
    assert_eq!(f.open_cursors(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_kills_the_operation_and_a_time_out_stops_the_run() {
    let f = people();
    let s = Arc::new(open(&f).await);
    let meta = s.table_meta(&table("people")).await.unwrap();
    f.delay("find", Duration::from_secs(10));
    let slot = CancelSlot::default();
    let browse = {
        let (s, meta, slot) = (s.clone(), meta.clone(), slot.clone());
        tokio::spawn(async move { s.browse(&meta, &request("people", json!({})), slot).await })
    };
    // Cancelled once it runs on the server, so the kill finds it however loaded the machine.
    for _ in 0..1000 {
        if slot.is_armed() && f.running() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let handle = request_cancel(&slot).expect("the browse armed its cancel");
    s.cancel(handle).await.unwrap();
    let err = tokio::time::timeout(Duration::from_secs(5), browse)
        .await
        .expect("the browse stops at once")
        .unwrap()
        .unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
    // On the server, by its comment and its session.
    assert_eq!(f.commands_named("killOp").len(), 1);
    let listing = &f.commands_named("aggregate");
    let matched = listing
        .iter()
        .find(|c| format!("{c}").contains("\"$currentOp\""))
        .unwrap();
    assert!(format!("{matched}").contains("lsid.id"), "{matched}");

    // The console's run under the query time out: cancelled and reported as timed out.
    let options = RunOptions {
        timeout_secs: 1,
        ..RunOptions::default()
    };
    let mut events = Vec::new();
    let results = run_console_inner(
        s.as_ref(),
        "db.people.find()",
        &options,
        &CancelSlot::default(),
        &mut |e: QueryEvent| events.push(e),
    )
    .await;
    assert_eq!(results[0].error.as_ref().unwrap().code, "QUERY_TIMEOUT");
    assert_eq!(f.commands_named("killOp").len(), 2);
    // maxTimeMS on the server: a time out of the server's own.
    let (kind, _, _) = console(&s, "db.people.find().maxTimeMS(50)", 10).await;
    assert!(matches!(kind, Err(AppError::Timeout)), "{kind:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_that_stalls_the_kill_does_not_hold_a_timed_out_run() {
    let f = people();
    let s = open(&f).await;
    // The run's find and the kill's `$currentOp` listing both hang on the server.
    f.delay("find", Duration::from_secs(60));
    f.delay("aggregate", Duration::from_secs(60));
    let options = RunOptions {
        timeout_secs: 1,
        ..RunOptions::default()
    };
    let started = std::time::Instant::now();
    let results = tokio::time::timeout(
        Duration::from_secs(10),
        run_console_inner(
            &s,
            "db.people.find()",
            &options,
            &CancelSlot::default(),
            &mut |_: QueryEvent| {},
        ),
    )
    .await
    .expect("the kill gives up and the run returns");
    assert_eq!(results[0].error.as_ref().unwrap().code, "QUERY_TIMEOUT");
    // The time out, then at most the kill's limit.
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    assert!(f.commands_named("killOp").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_cancel_leaves_the_next_operations_alone() {
    let f = people();
    let s = Arc::new(open(&f).await);
    let client = s.client();
    let tag = OpTag::new(s.start_session(&client).await.unwrap());
    assert!(tag.lsid.is_some(), "the server has sessions");
    let slot = CancelSlot::default();
    let first = {
        let (s, client, tag, slot) = (s.clone(), client.clone(), tag.clone(), slot.clone());
        tokio::spawn(async move {
            // Not (or no longer) on the server when the cancel comes: the kill loop keeps
            // looking for it by its session.
            let op = async {
                let _session = tag.session().await;
                std::future::pending::<Result<(), AppError>>().await
            };
            s.cancellable(&slot, &client, &tag, op).await
        })
    };
    drop(tag);
    for _ in 0..500 {
        if slot.is_armed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let handle = request_cancel(&slot).expect("the operation armed its cancel");
    let cancel = {
        let s = s.clone();
        tokio::spawn(async move { s.cancel(handle).await })
    };
    let err = first.await.unwrap().unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
    tokio::time::sleep(Duration::from_millis(30)).await;
    // Two writes nobody cancels, each in a session from the crate's pool (which hands out the
    // session returned last), on the server while the kill loop still lists operations.
    f.delay("update", Duration::from_secs(1));
    let write = |id: i32, age: i32| {
        let s = s.clone();
        tokio::spawn(async move {
            let text = format!("db.people.updateOne({{_id: {id}}}, {{$set: {{age: {age}}}}})");
            console(&s, &text, 10).await.0
        })
    };
    let (w1, w2) = (write(1, 99), write(2, 98));
    let (w1, w2) = (w1.await.unwrap(), w2.await.unwrap());
    cancel.await.unwrap().unwrap();
    assert!(w1.is_ok() && w2.is_ok(), "{w1:?} {w2:?}");
    assert!(f.commands_named("killOp").is_empty());
    let ages: Vec<_> = f
        .docs("hr", "people")
        .iter()
        .filter_map(|d| d.get_i32("age").ok())
        .collect();
    assert_eq!(ages, [99, 98, 40]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_refuses_every_write_path() {
    let f = people();
    let s = open_with(
        &f,
        json!({ "options": { "readOnly": true, "connectTimeoutSecs": 5 } }),
        None,
        None,
    )
    .await
    .unwrap();
    let meta = s.table_meta(&table("people")).await.unwrap();
    let req = changes(
        &meta,
        json!([{ "op": "delete", "rowId": "a", "key": { "kind": "pk", "value": { "_id": json!(Value::Int(1)) } } }]),
    );
    let err = s.plan_changes(&meta, &req).err().unwrap();
    assert_eq!(err.to_string(), READ_ONLY_MESSAGE);
    // A plan made elsewhere is refused too.
    let edits = mongo::plan_changes(
        &meta.table,
        &meta.columns,
        &LoadedTypes::default(),
        &req.changes,
    )
    .unwrap();
    let plan = NativePlan {
        preview: edits.preview(None),
        ops: Box::new(edits),
    };
    let result = s.apply_changes(&meta, &req, plan).await;
    assert_eq!(result.error.unwrap().message, READ_ONLY_MESSAGE);
    for text in [
        "db.people.insertOne({a: 1})",
        "db.people.aggregate([{$out: 'x'}])",
        "db.runCommand({dropDatabase: 1})",
    ] {
        let (kind, _, _) = console(&s, text, 10).await;
        assert_eq!(kind.unwrap_err().to_string(), READ_ONLY_MESSAGE, "{text}");
    }
    // What the parser cannot read is refused as well (as a parse error).
    assert!(
        console(&s, "db.people.find().forEach(printjson)", 10)
            .await
            .0
            .is_err()
    );
    console(&s, "db.people.find()", 10).await.0.unwrap();
    let err = s
        .call(
            "dropCollection",
            json!({ "database": "hr", "collection": "people" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), READ_ONLY_MESSAGE);
    s.call(
        "collectionStats",
        json!({ "database": "hr", "collection": "people" }),
    )
    .await
    .unwrap();
    let killed = s
        .kill_processes(&["1".to_string()], KillScope::Query)
        .await
        .unwrap();
    assert!(!killed[0].ok);
    // Nothing that writes reached the server.
    for name in ["insert", "delete", "drop", "dropDatabase", "killOp"] {
        assert!(f.commands_named(name).is_empty(), "{name}");
    }
    assert_eq!(f.docs("hr", "people").len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_queries_are_listed_and_killed() {
    let f = people();
    let s = Arc::new(open(&f).await);
    let meta = s.table_meta(&table("people")).await.unwrap();
    f.delay("find", Duration::from_secs(10));
    let browse = {
        let (s, meta) = (s.clone(), meta.clone());
        tokio::spawn(async move {
            s.browse(&meta, &request("people", json!({})), CancelSlot::default())
                .await
        })
    };
    let mut rows = Vec::new();
    for _ in 0..500 {
        rows = s.list_processes().await.unwrap().unwrap();
        if !rows.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let row = rows.first().expect("the slow find is listed");
    assert_eq!(row.database.as_deref(), Some("hr"));
    assert!(
        row.query
            .as_deref()
            .unwrap()
            .contains("\"find\": \"people\"")
    );
    assert!(row.own && !row.current);
    assert_eq!(row.duration_ms, Some(1));
    let outcome = s
        .kill_processes(std::slice::from_ref(&row.id), KillScope::Connection)
        .await
        .unwrap();
    assert!(!outcome[0].ok);
    let outcome = s
        .kill_processes(std::slice::from_ref(&row.id), KillScope::Query)
        .await
        .unwrap();
    assert!(outcome[0].ok, "{outcome:?}");
    let err = browse.await.unwrap().unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calls_change_collections_and_say_what_they_run() {
    let f = people();
    let s = open(&f).await;
    assert_eq!(s.call_kind("collectionStats"), CallKind::Read);
    assert_eq!(s.call_kind("dropCollection"), CallKind::Write);
    assert_eq!(s.call_kind("anything"), CallKind::Write);
    let preview = s
        .call(
            "preview",
            json!({ "method": "truncateCollection", "args": { "database": "hr", "collection": "people" } }),
        )
        .await
        .unwrap();
    assert_eq!(
        preview["command"],
        "db.getSiblingDB(\"hr\").runCommand({\"delete\": \"people\", \"deletes\": [{\"q\": {}, \"limit\": 0}]})"
    );
    let stats = s
        .call(
            "collectionStats",
            json!({ "database": "hr", "collection": "people" }),
        )
        .await
        .unwrap();
    assert_eq!(stats["storageStats"]["count"], 3);
    s.call(
        "createIndex",
        json!({ "database": "hr", "collection": "people", "keys": "{name: 1}", "options": "{unique: true}" }),
    )
    .await
    .unwrap();
    let def = s.definition(&table("people")).await.unwrap().unwrap();
    assert!(def.contains("createIndex({\"name\": 1}"), "{def}");
    s.call(
        "dropIndex",
        json!({ "database": "hr", "collection": "people", "name": "name_1" }),
    )
    .await
    .unwrap();
    s.call(
        "renameCollection",
        json!({ "database": "hr", "collection": "people", "to": "staff" }),
    )
    .await
    .unwrap();
    let out = s
        .call(
            "truncateCollection",
            json!({ "database": "hr", "collection": "staff" }),
        )
        .await
        .unwrap();
    assert_eq!(out["reply"]["n"], 3);
    assert!(f.docs("hr", "staff").is_empty());
    s.call(
        "dropCollection",
        json!({ "database": "hr", "collection": "staff" }),
    )
    .await
    .unwrap();
    let err = s
        .call(
            "dropCollection",
            json!({ "database": "hr", "collection": "staff" }),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AppError::Db {
                vendor_code: Some(26),
                ..
            }
        ),
        "{err:?}"
    );
    let err = s.call("frobnicate", json!({})).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported: MongoDB: unknown call \u{201c}frobnicate\u{201d}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotated_command_password_signs_in_again() {
    let f = people();
    f.require_plain("u", "p1");
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = runs.clone();
    let command = CommandPassword::with_fetch(Duration::from_secs(600), move || {
        counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok("p2".to_string()) })
    })
    .seeded(Some("p1"));
    let extra = json!({ "user": "u", "options": { "connectTimeoutSecs": 5, "passwordMode": "command",
        "passwordCommand": "print-password", "driverParams": { "authMechanism": "plain" } } });
    let s = open_with(&f, extra, Some("p1"), Some(Arc::new(command)))
        .await
        .unwrap();
    s.list_tables("hr").await.unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    // Nobody types a password a command prints: Safe Mode asks for the passcode.
    assert_eq!(s.capabilities().safety.confirm, SafeConfirm::Passcode);
    // The secret rotates and the server restarts: the old password no longer signs in.
    f.require_plain("u", "p2");
    f.drop_connections();
    let _ = s.ping().await;
    s.list_tables("hr").await.unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(f.sign_ins().last().map(String::as_str), Some("plain:p2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oidc_token_is_fetched_again_when_the_server_asks() {
    let f = people();
    f.accept_tokens(&["tok-0"]);
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = runs.clone();
    let command = CommandPassword::with_fetch(Duration::from_secs(600), move || {
        let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move { Ok(format!("tok-{n}")) })
    })
    .seeded(Some("tok-0"));
    let extra = json!({ "options": { "connectTimeoutSecs": 5, "passwordMode": "command",
        "passwordCommand": "print-token", "driverParams": { "authMechanism": "oidc" } } });
    let s = open_with(&f, extra, Some("tok-0"), Some(Arc::new(command)))
        .await
        .unwrap();
    s.list_tables("hr").await.unwrap();
    f.accept_tokens(&["tok-1"]);
    f.reauth_next();
    s.list_tables("hr").await.unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(f.sign_ins().last().map(String::as_str), Some("oidc:tok-1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_credentials_and_mcp_are_refused() {
    let f = people();
    f.require_plain("u", "right");
    let extra = json!({ "user": "u", "options": { "connectTimeoutSecs": 5,
        "driverParams": { "authMechanism": "plain" } } });
    let err = open_with(&f, extra, Some("wrong"), None)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, AppError::Auth(_)), "{err:?}");
    let p = profile(1, json!({}));
    let target = ConnectTarget {
        profile: &p,
        password: None,
        host_override: None,
    };
    let err = MongoSession::open(target, true).await.err().unwrap();
    assert_eq!(
        err.to_string(),
        "unsupported: MongoDB is not available to MCP clients in this Tabler"
    );
}
