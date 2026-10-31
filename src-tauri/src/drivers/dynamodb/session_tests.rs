//! The session against the in-memory DynamoDB of `testing`: connect (over a socket, through the
//! entry point), the sidebar, grid pages with cursors, counts, cells, commits with a failing
//! change, the console with its row cap, cancel and time out, the caps, and read-only refusals on
//! every write path.

use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::attr::AttrType;
use super::auth;
use super::client::{Client, RetryPolicy};
use super::entry;
use super::session::{DynamoSession, FILTER_OPS, capabilities};
use super::testing::FakeDynamo;
use crate::commands::console::run_console_inner;
use crate::commands::query::{QUERY_TIMEOUT_CODE, with_timeout_by};
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::common::aws::credentials::CredentialProvider;
use crate::drivers::native::{CallKind, NativeSession, TableSource};
use crate::drivers::registry::UtilInput;
use crate::drivers::{CancelSlot, ConnectTarget, RowSink, Session, request_cancel};
use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::native::dynamo::{self as b, KeySchema};
use crate::testkit::FakeServer;
use crate::types::browse::{
    BrowseRequest, Conjunction, FilterGroup, FilterNode, FilterOp, RowKey, SortDir, SortSpec,
};
use crate::types::caps::{Affected, CountMode, Feature, Paging, TxMode};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::{ApplyRequest, StagedChange};
use crate::types::profile::ConnectionProfile;
use crate::types::query::{ResultColumn, RunOptions, StatementKind};
use crate::types::schema::{TableMeta, TableRef};
use crate::types::value::Value;

const REGION: &str = "us-east-1";

/// `users`: partition key `org` (S), sort key `id` (N), five items of every kind of value.
fn users() -> Arc<FakeDynamo> {
    let fake = FakeDynamo::new();
    fake.create_table("users", ("org", AttrType::S), Some(("id", AttrType::N)));
    fake.create_table("audit", ("at", AttrType::S), None);
    fake.describe_with(
        "users",
        "AttributeDefinitions",
        json!([
            {"AttributeName": "org", "AttributeType": "S"},
            {"AttributeName": "id", "AttributeType": "N"},
            {"AttributeName": "name", "AttributeType": "S"},
        ]),
    );
    fake.describe_with(
        "users",
        "GlobalSecondaryIndexes",
        json!([{
            "IndexName": "by-name",
            "KeySchema": [{"AttributeName": "name", "KeyType": "HASH"}],
            "Projection": {"ProjectionType": "KEYS_ONLY"},
            "IndexStatus": "ACTIVE",
        }]),
    );
    for (id, name, extra) in [
        (1, "Ann", json!({"big": {"N": "9007199254740993"}})),
        (2, "Bob", json!({"price": {"N": "19.99"}})),
        (3, "Cy", json!({"at": {"S": "2026-10-30T09:15:00.123456Z"}})),
        (
            4,
            "Di",
            json!({"doc": {"M": {"a": {"L": [{"N": "1"}, {"S": "x"}]}}}}),
        ),
        (5, "Ed", json!({})),
    ] {
        let mut item =
            json!({"org": {"S": "acme"}, "id": {"N": id.to_string()}, "name": {"S": name}});
        for (k, v) in extra.as_object().unwrap() {
            item[k] = v.clone();
        }
        fake.put("users", item);
    }
    fake
}

fn client(fake: &Arc<FakeDynamo>) -> Client {
    Client::new(
        fake.clone(),
        "http://localhost:8000/",
        REGION,
        Arc::new(CredentialProvider::fixed(auth::local(None))),
    )
    .unwrap()
    .with_retry(RetryPolicy {
        max_attempts: 3,
        base: Duration::ZERO,
        cap: Duration::ZERO,
    })
}

fn session(fake: &Arc<FakeDynamo>, read_only: bool) -> DynamoSession {
    DynamoSession::new(
        client(fake),
        super::params::DynamoParams::default(),
        REGION,
        true,
        read_only,
    )
}

fn users_ref() -> TableRef {
    TableRef::new(Some(REGION), "users")
}

fn request(limit: u32, cursor: Option<String>, filter: Option<FilterGroup>) -> BrowseRequest {
    BrowseRequest {
        table: users_ref(),
        columns: None,
        filter,
        sort: Vec::new(),
        limit,
        offset: 0,
        cursor,
    }
}

fn key(org: &str, id: i64) -> RowKey {
    RowKey::Pk(BTreeMap::from([
        ("org".to_string(), Value::Text(org.into())),
        ("id".to_string(), Value::Int(id)),
    ]))
}

fn update(id: i64, column: &str, v: Value) -> StagedChange {
    StagedChange::Update {
        row_id: format!("r{id}"),
        key: key("acme", id),
        changes: BTreeMap::from([(column.to_string(), v)]),
        exprs: None,
    }
}

fn apply_request(changes: Vec<StagedChange>) -> ApplyRequest {
    ApplyRequest {
        table: users_ref(),
        changes,
        returning: false,
    }
}

/// Rows a console command streamed.
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

fn profile(params: serde_json::Value) -> ConnectionProfile {
    serde_json::from_value(json!({
        "id": uuid::Uuid::new_v4(),
        "name": "dynamo",
        "driver": "dynamodb",
        "database": REGION,
        "options": { "driverParams": params, "connectTimeoutSecs": 5 },
    }))
    .unwrap()
}

/// Connect, test and the server's identity: a Local profile reaches the fake over HTTP through
/// the entry point, proves itself with `DescribeEndpoints` (or one table name where the server
/// has none), and an MCP process gets no session.
#[tokio::test]
async fn connects_over_http_and_pings_for_free() {
    let fake = users();
    let server = FakeServer::start(vec![fake.route()]).await;
    let p = profile(json!({"auth": "local", "endpoint": server.url("/")}));
    entry::validate(&p).unwrap();
    let target = || ConnectTarget {
        profile: &p,
        password: None,
        host_override: None,
    };
    let Session::Native(store) = entry::open(target()).await.unwrap() else {
        panic!("a native session")
    };
    assert_eq!(fake.actions(), ["DescribeEndpoints"]);
    let info = store.server_info().await.unwrap();
    assert!(info.version.starts_with("DynamoDB API"), "{}", info.version);
    assert_eq!(info.current_database.as_deref(), Some(REGION));
    assert_eq!(info.current_user.as_deref(), Some(auth::LOCAL_KEY_ID));
    assert_eq!(store.capabilities(), capabilities(REGION));
    let tables = store.list_tables(REGION).await.unwrap();
    assert_eq!(
        tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["audit", "users"]
    );
    store.close().await;
    // Closed: what is still asked of it stops at once.
    assert!(matches!(
        store.list_tables(REGION).await,
        Err(AppError::Cancelled)
    ));

    // A server without DescribeEndpoints (DynamoDB Local) is asked for a table name instead.
    let before = fake.actions().len();
    fake.fail_next(
        "DescribeEndpoints",
        400,
        "UnknownOperationException",
        "not supported",
    );
    let Session::Native(local) = entry::open(target()).await.unwrap() else {
        panic!("a native session")
    };
    local.ping().await.unwrap();
    assert_eq!(
        &fake.actions()[before..],
        ["DescribeEndpoints", "ListTables", "ListTables"]
    );
    assert_eq!(fake.requests().last().unwrap().1, json!({"Limit": 1}));

    // Refused credentials fail the connect.
    fake.check_signatures("AKIDEXAMPLE", "secret");
    match entry::open(target()).await {
        Err(AppError::Auth(m)) => assert!(m.contains("UnrecognizedClientException"), "{m}"),
        other => panic!("{:?}", other.err()),
    }
    match entry::open_as(target(), true).await {
        Err(AppError::Unsupported(m)) => assert!(m.contains("MCP"), "{m}"),
        other => panic!("{:?}", other.err()),
    }
}

#[test]
fn profiles_are_checked_and_the_form_calls_read() {
    let ok = profile(json!({"auth": "local"}));
    entry::validate(&ok).unwrap();
    let mut bad_region = ok.clone();
    bad_region.database = Some("US East".into());
    assert!(entry::validate(&bad_region).is_err());
    // An access key without its ID is saved; the connect says what is missing.
    entry::validate(&profile(json!({"auth": "accessKey"}))).unwrap();
    let command = profile(json!({"auth": "command"}));
    assert!(entry::validate(&command).is_err());
    let mut command = command;
    command.options.password_mode = Some(crate::types::profile::PasswordMode::Command);
    command.options.password_command = Some("aws configure export-credentials".into());
    entry::validate(&command).unwrap();
    assert!(entry::validate(&profile(json!({"sampleSize": 0}))).is_err());
    assert!(entry::validate(&profile(json!({"endpoint": "ftp://x"}))).is_err());

    assert_eq!(entry::util_kind("regions"), CallKind::Read);
    assert_eq!(entry::util_kind("awsProfiles"), CallKind::Read);
    assert_eq!(entry::util_kind("CreateTable"), CallKind::Write);
}

#[tokio::test]
async fn the_form_lists_regions_and_refuses_unknown_calls() {
    let p = profile(json!({}));
    let input = || UtilInput {
        profile: &p,
        secret: None,
    };
    let regions = entry::util(input(), "regions", json!({})).await.unwrap();
    assert!(regions.as_array().unwrap().contains(&json!("eu-central-1")));
    assert!(matches!(
        entry::util(input(), "dropEverything", json!({})).await,
        Err(AppError::Unsupported(_))
    ));
    assert!(
        entry::util(input(), "awsProfiles", json!({}))
            .await
            .unwrap()
            .is_array()
    );
}

/// What the caps grant is what the session does: cursor pages, costly counts, every filter
/// operator listed (and none other), edits applied in transactions with exact counts, only
/// Tabler's classifier for read-only, no keep-alive, no MCP.
#[test]
fn caps_grant_what_is_implemented() {
    let caps = capabilities(REGION);
    assert_eq!(
        caps.features,
        [
            Feature::Editor,
            Feature::EditRows,
            Feature::InsertRows,
            Feature::DeleteRows,
            Feature::ExactCount,
            Feature::StructureView,
            Feature::ViewDefinition,
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );
    for absent in [
        Feature::Mcp,
        Feature::CancelOnServer,
        Feature::EditWithoutKey,
        Feature::ServerExpressions,
        Feature::SwitchDatabase,
        Feature::TableDdl,
        Feature::StructureEdit,
    ] {
        assert!(!caps.has(absent), "{absent:?}");
    }
    assert_eq!(caps.data.paging, Paging::Cursor);
    assert_eq!(caps.data.count, CountMode::Costly);
    assert_eq!(caps.data.transactions, TxMode::Batch);
    assert_eq!(caps.data.affected, Affected::Exact);
    assert!(caps.data.sort && caps.data.raw_filter.is_none());
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    assert!(!caps.safety.hardened);
    assert_eq!(caps.session.keepalive_secs, None);
    assert_eq!(caps.namespaces.default_schema.as_deref(), Some(REGION));
    assert_eq!(caps.namespaces.labels.database, "Region");

    // Every operator offered builds a filter; the others are refused.
    let keys = KeySchema {
        hash: ("org".into(), AttrType::S),
        range: Some(("id".into(), AttrType::N)),
    };
    let values = |op: FilterOp| match op {
        FilterOp::IsNull | FilterOp::IsNotNull => vec![],
        FilterOp::Between | FilterOp::NotBetween => {
            vec![Value::Text("a".into()), Value::Text("b".into())]
        }
        FilterOp::In | FilterOp::NotIn => vec![Value::Text("a".into()), Value::Text("b".into())],
        _ => vec![Value::Text("a%".into())],
    };
    let all = [
        FilterOp::Eq,
        FilterOp::Ne,
        FilterOp::Gt,
        FilterOp::Lt,
        FilterOp::Ge,
        FilterOp::Le,
        FilterOp::Like,
        FilterOp::NotLike,
        FilterOp::In,
        FilterOp::NotIn,
        FilterOp::IsNull,
        FilterOp::IsNotNull,
        FilterOp::Between,
        FilterOp::NotBetween,
        FilterOp::Contains,
        FilterOp::NotContains,
        FilterOp::ContainsCase,
        FilterOp::NotContainsCase,
        FilterOp::HasPrefix,
        FilterOp::HasSuffix,
        FilterOp::HasPrefixCase,
        FilterOp::HasSuffixCase,
        FilterOp::Ilike,
        FilterOp::JsonContains,
        FilterOp::JsonNotContains,
    ];
    for op in all {
        let filter = FilterGroup {
            conjunction: Conjunction::And,
            items: vec![FilterNode::Condition {
                column: "name".into(),
                op,
                values: values(op),
            }],
        };
        let built = b::read_request(&b::ReadSpec {
            table: "users",
            index: None,
            keys: &keys,
            columns: &[],
            filter: Some(&filter),
            descending: false,
            consistent: false,
            consumed_capacity: false,
        });
        assert_eq!(built.is_ok(), FILTER_OPS.contains(&op), "{op:?}: {built:?}");
    }
}

/// The sidebar and the structure: the region, every table, a table's columns from its
/// description and a sample, its definition, and completions without reading any table.
#[tokio::test]
async fn the_catalog_in_the_sidebar_s_terms() {
    let fake = users();
    fake.set_page(1);
    let s = session(&fake, false);
    assert_eq!(s.list_databases().await.unwrap()[0].name, REGION);
    assert_eq!(s.list_schemas().await.unwrap()[0].name, REGION);
    assert_eq!(s.list_tables(REGION).await.unwrap().len(), 2);
    let meta = s.table_meta(&users_ref()).await.unwrap();
    assert_eq!(meta.primary_key, ["org", "id"]);
    assert_eq!(meta.row_estimate, Some(5));
    assert!(meta.indexes.iter().any(|i| i.name == "by-name"));
    let definition = s.definition(&users_ref()).await.unwrap().unwrap();
    assert!(definition.starts_with("CreateTable {"), "{definition}");
    assert!(definition.contains("\"KeySchema\""), "{definition}");
    let scans = fake.actions().iter().filter(|a| *a == "Scan").count();
    let completions = s.completion_metadata().await.unwrap();
    let tables = &completions.schemas[REGION];
    assert!(tables["users"].contains(&"name".to_string()));
    assert!(tables["audit"].is_empty());
    assert_eq!(
        fake.actions().iter().filter(|a| *a == "Scan").count(),
        scans
    );
    // The app's own requests are in the trace for the console log.
    let traced: Vec<String> = s.drain_trace().into_iter().map(|t| t.sql).collect();
    assert!(
        traced.iter().any(|t| t.starts_with("DescribeTable ")),
        "{traced:?}"
    );
}

/// Grid pages through the session: cursors to the end, values in their types (2^53 + 1 a
/// BigInt, a decimal its text, a timestamp text, a map JSON), sort only on the sort key of a
/// Query, counts (the estimate, or every page counted), cells whole.
#[tokio::test]
async fn grid_pages_counts_and_cells() {
    let fake = users();
    let s = session(&fake, false);
    let meta = s.table_meta(&users_ref()).await.unwrap();
    let p1 = s
        .browse(&meta, &request(3, None, None), CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(p1.rows.len(), 3);
    let p2 = s
        .browse(
            &meta,
            &request(3, p1.next_cursor.clone(), None),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(p2.rows.len(), 2);
    assert_eq!(p2.next_cursor, None);
    let col = |page: &crate::types::browse::BrowsePage, name: &str| {
        page.columns.iter().position(|c| c.name == name).unwrap()
    };
    assert_eq!(
        p1.rows[0][col(&p1, "big")],
        Value::BigInt("9007199254740993".into())
    );
    assert_eq!(
        p1.rows[1][col(&p1, "price")],
        Value::Decimal("19.99".into())
    );
    assert_eq!(
        p1.rows[2][col(&p1, "at")],
        Value::Text("2026-10-30T09:15:00.123456Z".into())
    );
    assert!(matches!(&p2.rows[0][col(&p2, "doc")], Value::Json(_)));
    assert!(super::attr::is_missing(&p1.rows[0][col(&p1, "price")]));

    // Sorted by the sort key, descending, in a Query; another column is refused.
    let mut sorted = request(
        10,
        None,
        Some(FilterGroup {
            conjunction: Conjunction::And,
            items: vec![FilterNode::Condition {
                column: "org".into(),
                op: FilterOp::Eq,
                values: vec![Value::Text("acme".into())],
            }],
        }),
    );
    sorted.sort = vec![SortSpec {
        column: "id".into(),
        dir: SortDir::Desc,
        nulls: None,
    }];
    let p = s
        .browse(&meta, &sorted, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(p.rows[0][col(&p, "id")], Value::Int(5));
    assert!(p.sql.starts_with("Query {"), "{}", p.sql);
    sorted.sort[0].column = "name".into();
    assert!(matches!(
        s.browse(&meta, &sorted, CancelSlot::default()).await,
        Err(AppError::Unsupported(_))
    ));

    let estimate = s
        .count(&meta, None, false, CancelSlot::default())
        .await
        .unwrap();
    assert!(!estimate.exact);
    let exact = s
        .count(&meta, None, true, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((exact.count, exact.exact), (5, true));
    let cell = s.fetch_cell(&meta, &key("acme", 4), "doc").await.unwrap();
    assert!(matches!(cell, Value::Json(j) if j.contains("\"x\"")));
    assert!(matches!(
        s.fetch_cell(&meta, &key("acme", 99), "doc").await,
        Err(AppError::NotFound(_))
    ));
}

/// A name that is a reserved word or holds expression syntax, and a value that looks like one,
/// reach DynamoDB only as `#n` / `:v` placeholders.
#[tokio::test]
async fn names_and_values_are_never_inlined() {
    let fake = users();
    let s = session(&fake, false);
    let meta = s.table_meta(&users_ref()).await.unwrap();
    let evil = "name) OR attribute_exists(org";
    let filter = FilterGroup {
        conjunction: Conjunction::And,
        items: vec![FilterNode::Condition {
            column: evil.into(),
            op: FilterOp::Eq,
            values: vec![Value::Text(":v0 OR size(org) > :v0".into())],
        }],
    };
    let page = s
        .browse(
            &meta,
            &request(10, None, Some(filter)),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert!(page.rows.is_empty());
    let (_, body) = fake.requests().last().cloned().unwrap();
    let expression = body["FilterExpression"].as_str().unwrap();
    assert!(!expression.contains("attribute_exists(org"), "{expression}");
    assert!(!expression.contains("size("), "{expression}");
    let names = body["ExpressionAttributeNames"].as_object().unwrap();
    assert!(names.values().any(|n| n == &json!(evil)), "{body}");
    let values = body["ExpressionAttributeValues"].as_object().unwrap();
    assert!(
        values
            .values()
            .any(|v| v == &json!({"S": ":v0 OR size(org) > :v0"})),
        "{body}"
    );
}

/// A commit with one failing change: the transaction applies none of it and says which change
/// failed; without it everything applies. One change is one conditional request.
#[tokio::test]
async fn a_commit_with_one_failing_change() {
    let fake = users();
    let s = session(&fake, false);
    let meta = s.table_meta(&users_ref()).await.unwrap();
    s.browse(&meta, &request(10, None, None), CancelSlot::default())
        .await
        .unwrap();
    // Someone else renames Bob after the grid loaded him.
    fake.put(
        "users",
        json!({"org": {"S": "acme"}, "id": {"N": "2"}, "name": {"S": "Robert"}}),
    );
    let req = apply_request(vec![
        update(1, "name", Value::Text("Anna".into())),
        update(2, "name", Value::Text("Bobby".into())),
        StagedChange::Delete {
            row_id: "r5".into(),
            key: key("acme", 5),
        },
    ]);
    let plan = s.plan_changes(&meta, &req).unwrap();
    assert_eq!(plan.preview.len(), 3);
    assert!(plan.preview[0].1.starts_with("UpdateItem {"));
    let out = s.apply_changes(&meta, &req, plan).await;
    assert!(!out.success);
    assert!(out.results.iter().all(|r| r.error.is_some()));
    let failed = out.results[1].error.as_ref().unwrap();
    assert!(
        failed.message.contains("someone else"),
        "{}",
        failed.message
    );
    assert!(
        out.results[0]
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("not applied"),
    );
    assert_eq!(fake.actions().last().unwrap(), "TransactWriteItems");
    assert_eq!(fake.items("users").len(), 5);

    // The two good ones apply together; the commit forgot what it wrote, so a second edit of
    // Ann is guarded by her key alone until the grid loads her again.
    let good = apply_request(vec![req.changes[0].clone(), req.changes[2].clone()]);
    let plan = s.plan_changes(&meta, &good).unwrap();
    let out = s.apply_changes(&meta, &good, plan).await;
    assert!(out.success, "{:?}", out.error);
    assert_eq!(
        out.results.iter().map(|r| r.affected).collect::<Vec<_>>(),
        [1, 1]
    );
    assert_eq!(fake.items("users").len(), 4);
    let again = apply_request(vec![update(1, "name", Value::Text("Ann".into()))]);
    let plan = s.plan_changes(&meta, &again).unwrap();
    assert!(
        !plan.preview[0].1.contains("\"Anna\""),
        "{}",
        plan.preview[0].1
    );
    let out = s.apply_changes(&meta, &again, plan).await;
    assert!(out.success);
    assert_eq!(fake.actions().last().unwrap(), "UpdateItem");
}

/// The console through the editor's runner: PartiQL pages up to the row cap, a write's count (none
/// for a DELETE, which succeeds on a missing key), an API request's reply, an error that stops
/// the run.
#[tokio::test]
async fn console_runs_through_the_editor() {
    let fake = users();
    fake.set_page(2);
    let s = session(&fake, false);
    let text = "SELECT * FROM \"users\";\n\
                UPDATE \"users\" SET name = 'x' WHERE org = 'acme' AND id = 1;\n\
                DELETE FROM \"users\" WHERE org = 'acme' AND id = 99;\n\
                DescribeTable {\"TableName\": \"users\"}";
    assert_eq!(s.split_console(text).len(), 4);
    assert_eq!(s.classify_console("SELECT * FROM t"), StatementClass::Read);
    assert_eq!(s.classify_console(text), StatementClass::Write);
    let options = RunOptions {
        max_rows: 4,
        ..RunOptions::default()
    };
    let mut events = Vec::new();
    let results = run_console_inner(&s, text, &options, &CancelSlot::default(), &mut |e| {
        events.push(e)
    })
    .await;
    assert_eq!(results.len(), 4);
    assert_eq!(
        (results[0].kind, results[0].row_count, results[0].truncated),
        (StatementKind::Rows, 4, true)
    );
    assert_eq!(
        (results[1].kind, results[1].affected),
        (StatementKind::Affected, Some(1))
    );
    assert_eq!(
        (results[2].kind, results[2].affected),
        (StatementKind::Empty, None)
    );
    assert_eq!(results[3].columns[0].name, "reply");
    assert!(results.iter().all(|r| r.error.is_none()));
    // The console's commands are not in the app's trace: the editor logs them.
    assert!(s.drain_trace().is_empty());

    let mut sink = Rows::default();
    let out = s
        .run_console(
            "Scan {\"TableName\": \"users\", \"Limit\": 2}",
            100,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!((out.kind, out.row_count), (StatementKind::Rows, 2));
    assert!(sink.columns.iter().any(|c| c.name == "org"));
    assert!(out.notices[0].contains("ExclusiveStartKey"));
    let failed = run_console_inner(
        &s,
        "SELECT * FROM \"nope\"; SELECT * FROM \"users\"",
        &RunOptions::default(),
        &CancelSlot::default(),
        &mut |_| {},
    )
    .await;
    // The run stops at the first error.
    assert_eq!(failed.len(), 1);
    assert!(failed[0].error.is_some());
}

/// A cancel stops a grid page in flight; the query time out cancels a console command and a
/// grid read through the session and reports a time out.
#[tokio::test]
async fn cancel_and_time_out() {
    let fake = users();
    let s = Arc::new(session(&fake, false));
    let meta = s.table_meta(&users_ref()).await.unwrap();
    fake.set_delay(Duration::from_secs(30));

    let slot = CancelSlot::default();
    let (canceller, stopper) = (slot.clone(), s.clone());
    tokio::spawn(async move {
        while !canceller.is_armed() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if let Some(handle) = request_cancel(&canceller) {
            stopper.cancel(handle).await.unwrap();
        }
    });
    let started = std::time::Instant::now();
    let out = s.browse(&meta, &request(5, None, None), slot).await;
    assert!(matches!(out, Err(AppError::Cancelled)), "{out:?}");
    assert!(started.elapsed() < Duration::from_secs(10));

    let options = RunOptions {
        timeout_secs: 1,
        ..RunOptions::default()
    };
    let results = run_console_inner(
        s.as_ref(),
        "SELECT * FROM \"users\"; SELECT * FROM \"audit\"",
        &options,
        &CancelSlot::default(),
        &mut |_| {},
    )
    .await;
    assert_eq!(
        results[0].error.as_ref().map(|e| e.code.as_str()),
        Some(QUERY_TIMEOUT_CODE)
    );
    assert_eq!(results.len(), 1, "nothing runs after a time out");

    let slot = CancelSlot::default();
    let (out, expired) = with_timeout_by(
        |handle| s.cancel(handle),
        &slot,
        Some(Duration::from_millis(100)),
        s.count(&meta, None, true, slot.clone()),
    )
    .await;
    assert!(
        expired && matches!(out, Err(AppError::Cancelled)),
        "{out:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(20));
}

/// A read-only profile's session refuses every write before anything is sent: edits, console
/// writes (PartiQL, API writes, a statement action carrying a write), and calls that are not
/// reads. Reads still work.
#[tokio::test]
async fn read_only_refuses_every_write_path() {
    let fake = users();
    let s = session(&fake, true);
    let meta = s.table_meta(&users_ref()).await.unwrap();
    s.browse(&meta, &request(10, None, None), CancelSlot::default())
        .await
        .unwrap();
    let sent = fake.requests().len();

    let req = apply_request(vec![update(1, "name", Value::Text("x".into()))]);
    assert_eq!(
        s.plan_changes(&meta, &req).err().map(|e| e.to_string()),
        Some(READ_ONLY_MESSAGE.to_string())
    );
    // A plan made elsewhere is refused too.
    let other = session(&fake, false);
    let plan = other.plan_changes(&meta, &req).unwrap();
    let out = s.apply_changes(&meta, &req, plan).await;
    assert!(!out.success);
    assert_eq!(
        out.results[0].error.as_ref().unwrap().message,
        READ_ONLY_MESSAGE
    );

    for write in [
        "UPDATE \"users\" SET name = 'x' WHERE org = 'acme' AND id = 1",
        "DELETE FROM \"users\" WHERE org = 'acme' AND id = 1",
        "PutItem {\"TableName\": \"users\", \"Item\": {\"org\": {\"S\": \"a\"}, \"id\": {\"N\": \"9\"}}}",
        "DeleteTable {\"TableName\": \"users\"}",
        "ExecuteStatement {\"Statement\": \"DELETE FROM users WHERE org = 'acme' AND id = 1\"}",
        "ExecuteTransaction {\"TransactStatements\": [{\"Statement\": \"SELECT * FROM users\"}, {\"Statement\": \"DELETE FROM users WHERE org = 'a' AND id = 1\"}]}",
        "SELECT * FROM users -- x\r DELETE FROM users WHERE org = 'a' AND id = 1",
        "BatchExecuteStatement {\"Statements\": [{\"Statement\": \"SELECT * FROM users\", \"statement\": \"DELETE FROM users\"}]}",
        "Frobnicate {}",
    ] {
        let mut sink = Rows::default();
        let out = s
            .run_console(write, 100, &mut sink, CancelSlot::default())
            .await;
        assert_eq!(
            out.err().map(|e| e.to_string()),
            Some(READ_ONLY_MESSAGE.to_string()),
            "{write}"
        );
    }
    assert_eq!(fake.requests().len(), sent, "a refused write sent nothing");
    assert_eq!(s.call_kind("UpdateTable"), CallKind::Write);
    assert!(matches!(
        s.call("UpdateTable", json!({})).await,
        Err(AppError::Unsupported(_))
    ));
    assert_eq!(fake.requests().len(), sent);

    // Reads go on: the console's reads and the read calls.
    let mut sink = Rows::default();
    s.run_console(
        "SELECT * FROM \"users\"",
        100,
        &mut sink,
        CancelSlot::default(),
    )
    .await
    .unwrap();
    assert_eq!(sink.rows.len(), 5);
    assert_eq!(s.call_kind("tableInfo"), CallKind::Read);
    let info = s
        .call("tableInfo", json!({"table": "users"}))
        .await
        .unwrap();
    assert_eq!(info["table"]["TableName"], json!("users"));
}

/// The table info panel's call, and pages of a secondary index (not remembered for edits).
#[tokio::test]
async fn table_info_and_index_pages() {
    let fake = users();
    fake.set_ttl("users", "expiresAt");
    let s = session(&fake, false);
    let info = s
        .call("tableInfo", json!({"table": "users"}))
        .await
        .unwrap();
    assert_eq!(info["timeToLive"]["AttributeName"], json!("expiresAt"));
    // The fake has no point-in-time recovery: the panel shows none.
    assert_eq!(info["continuousBackups"], json!(null));
    assert_eq!(s.call_kind("browseIndex"), CallKind::Read);
    let args = json!({
        "table": "users",
        "index": "nope",
        "request": serde_json::to_value(request(10, None, None)).unwrap(),
    });
    assert!(matches!(
        s.call("browseIndex", args).await,
        Err(AppError::NotFound(_))
    ));
    let args = json!({
        "table": "users",
        "index": "by-name",
        "request": serde_json::to_value(request(10, None, None)).unwrap(),
    });
    // The fake refuses index reads: the request names the index.
    assert!(s.call("browseIndex", args).await.is_err());
    let (action, body) = fake.requests().last().cloned().unwrap();
    assert_eq!(action, "Scan");
    assert_eq!(body["IndexName"], json!("by-name"));
    // Consistent reads: a local index has them, a global one never (even one sharing the
    // table's partition key, a common shape): DynamoDB refuses ConsistentRead on it.
    fake.describe_with(
        "users",
        "GlobalSecondaryIndexes",
        json!([{
            "IndexName": "by-org-name",
            "KeySchema": [{"AttributeName": "org", "KeyType": "HASH"}, {"AttributeName": "name", "KeyType": "RANGE"}],
            "Projection": {"ProjectionType": "ALL"},
        }]),
    );
    fake.describe_with(
        "users",
        "LocalSecondaryIndexes",
        json!([{
            "IndexName": "by-org-name-local",
            "KeySchema": [{"AttributeName": "org", "KeyType": "HASH"}, {"AttributeName": "name", "KeyType": "RANGE"}],
            "Projection": {"ProjectionType": "ALL"},
        }]),
    );
    let consistent = DynamoSession::new(
        client(&fake),
        super::params::DynamoParams {
            consistent_reads: true,
            ..super::params::DynamoParams::default()
        },
        REGION,
        true,
        false,
    );
    for (index, wanted) in [("by-org-name", None), ("by-org-name-local", Some(true))] {
        let args = json!({
            "table": "users",
            "index": index,
            "request": serde_json::to_value(request(10, None, None)).unwrap(),
        });
        let _ = consistent.call("browseIndex", args).await;
        let (_, body) = fake.requests().last().cloned().unwrap();
        assert_eq!(body["IndexName"], json!(index));
        assert_eq!(
            body.get("ConsistentRead"),
            wanted.map(serde_json::Value::Bool).as_ref(),
            "{index}"
        );
    }
    // Key columns of the meta must be typed S, N or B.
    let mut meta: TableMeta = s.table_meta(&users_ref()).await.unwrap();
    meta.columns[0].data_type = "M".into();
    assert!(matches!(
        s.browse(&meta, &request(1, None, None), CancelSlot::default())
            .await,
        Err(AppError::InvalidInput(_))
    ));
}

/// TLS of an endpoint of the profile's own follows the profile: verified with its CA file
/// (refused without it), with the client certificate the server asks for. AWS's endpoints are
/// always verified, whatever the form's mode; requests are never retried by the HTTP client.
#[tokio::test]
async fn tls_follows_the_profile_for_its_own_endpoint() {
    use crate::testkit::tls::{self, TlsServer};
    use crate::types::profile::SslMode;
    use std::io::Write as _;

    crate::testkit::install_crypto();
    let server = TlsServer::requiring_client_cert();
    let ca = tls::ca_file();
    let mut cert = tempfile::NamedTempFile::new().unwrap();
    cert.write_all(tls::CLIENT_CERT_PEM.as_bytes()).unwrap();
    let mut key = tempfile::NamedTempFile::new().unwrap();
    key.write_all(tls::CLIENT_KEY_PEM.as_bytes()).unwrap();
    let endpoint = format!("https://localhost:{}/", server.addr().port());
    let mut p = profile(json!({"auth": "local", "endpoint": endpoint}));
    p.ssl = SslMode::VerifyFull;
    p.ssl_ca_path = Some(ca.path().to_string_lossy().into_owned());
    p.options.ssl_cert_path = Some(cert.path().to_string_lossy().into_owned());
    p.options.ssl_key_path = Some(key.path().to_string_lossy().into_owned());
    let open = |p: &ConnectionProfile| {
        let p = p.clone();
        async move {
            entry::open(ConnectTarget {
                profile: &p,
                password: None,
                host_override: None,
            })
            .await
        }
    };
    // The handshake and the signed request get through; the test server's reply is not
    // DynamoDB's.
    match open(&p).await {
        Err(AppError::Db { message, .. }) => assert!(message.contains("DescribeEndpoints")),
        other => panic!("{:?}", other.err()),
    }
    let seen = server.requests();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].client_cert.is_some());
    assert_eq!(
        seen[0].header("x-amz-target"),
        Some("DynamoDB_20120810.DescribeEndpoints")
    );
    // Without the CA file the server's certificate is refused.
    p.ssl_ca_path = None;
    assert!(matches!(open(&p).await, Err(AppError::Connect(_))));
    assert_eq!(server.requests().len(), 1);

    let mut aws = profile(json!({}));
    aws.ssl = SslMode::Require;
    let regional = super::session::http_options(&aws, false).unwrap();
    assert_eq!(regional.tls.mode, SslMode::VerifyFull);
    let own = super::session::http_options(&aws, true).unwrap();
    assert_eq!(own.tls.mode, SslMode::Require);
    assert_eq!(own.retry, crate::drivers::common::http::RetryPolicy::NONE);
    assert_eq!(own.request_timeout, Some(super::session::REQUEST_TIMEOUT));
}

/// Which endpoints follow the form's TLS mode: only one the form names (or DynamoDB Local's) off
/// AWS's domains. The region's endpoint, AWS's other hosts (FIPS, VPC, dual-stack) and an AWS
/// profile's `endpoint_url` are verified in full: a server whose certificate the system (or the
/// profile's CA file) does not trust gets nothing, not even the session token.
#[tokio::test]
async fn aws_endpoints_are_always_verified() {
    use super::params::DynamoParams;
    use super::session::{endpoint, http_options};
    use super::transport::HttpTransport;
    use crate::drivers::common::http::{Credential, HttpClient};
    use crate::testkit::tls::{self, TlsServer};
    use crate::types::profile::SslMode;

    let params = |v: serde_json::Value| -> DynamoParams { serde_json::from_value(v).unwrap() };
    let regional = endpoint(&params(json!({})), REGION, None).unwrap();
    assert_eq!(
        (regional.url.as_str(), regional.custom, regional.form_tls),
        ("https://dynamodb.us-east-1.amazonaws.com/", false, false)
    );
    for aws in [
        "https://dynamodb-fips.us-east-1.amazonaws.com",
        "vpce-1a2b.dynamodb.us-east-1.vpce.amazonaws.com",
        "https://dynamodb.us-east-1.api.aws",
    ] {
        let e = endpoint(&params(json!({"endpoint": aws})), REGION, None).unwrap();
        assert!(e.custom && !e.form_tls, "{aws}");
    }
    let own = endpoint(
        &params(json!({"endpoint": "https://localhost:4566"})),
        REGION,
        Some("https://ignored.example"),
    )
    .unwrap();
    assert_eq!(
        (own.url.as_str(), own.custom, own.form_tls),
        ("https://localhost:4566/", true, true)
    );
    let local = endpoint(&params(json!({"auth": "local"})), REGION, None).unwrap();
    assert!(local.custom && local.form_tls);

    // An AWS profile's endpoint_url: verified with the profile's CA file, whatever its mode.
    crate::testkit::install_crypto();
    let server = TlsServer::start();
    let url = format!("https://localhost:{}", server.addr().port());
    let from_config = endpoint(&params(json!({"auth": "profile"})), REGION, Some(&url)).unwrap();
    assert!(from_config.custom && !from_config.form_tls);
    let mut p = profile(json!({"auth": "profile"}));
    p.ssl = SslMode::Prefer;
    let call = |p: &ConnectionProfile| {
        let (p, e) = (p.clone(), from_config.clone());
        async move {
            let target = ConnectTarget {
                profile: &p,
                password: None,
                host_override: None,
            };
            let options = http_options(&p, e.form_tls).unwrap();
            let http = HttpClient::for_target(&target, &e.url, options, Credential::None).unwrap();
            let creds = crate::drivers::common::aws::credentials::Credentials::static_keys(
                "AKID",
                "secret",
                Some("SESSIONTOKEN"),
            )
            .unwrap();
            Client::new(
                Arc::new(HttpTransport::new(http)),
                &e.url,
                REGION,
                Arc::new(CredentialProvider::fixed(creds)),
            )
            .unwrap()
            .call("ListTables", &json!({}), None)
            .await
        }
    };
    assert!(matches!(call(&p).await, Err(AppError::Connect(_))));
    assert!(server.requests().is_empty(), "nothing reached it");
    let ca = tls::ca_file();
    p.ssl_ca_path = Some(ca.path().to_string_lossy().into_owned());
    // Trusted through the CA file: the request gets there (the test server is not DynamoDB).
    let _ = call(&p).await;
    assert_eq!(server.requests().len(), 1);
}

/// A server that does not answer within the request time out is given up (`Timeout`), and the
/// request is not sent again.
#[tokio::test]
async fn a_silent_server_times_out() {
    use super::transport::HttpTransport;
    use crate::drivers::common::http::{Credential, HttpClient, HttpEndpoint};
    use crate::testkit::{Reply, Route};

    crate::testkit::install_crypto();
    let server = FakeServer::start(vec![
        Route::new("POST", "/*").reply(Reply::ok().delay(Duration::from_secs(30))),
    ])
    .await;
    let endpoint = server.url("/");
    let mut options = super::session::http_options(&profile(json!({})), true).unwrap();
    options.request_timeout = Some(Duration::from_millis(200));
    let http = HttpClient::new(
        HttpEndpoint::new(&endpoint, None).unwrap(),
        options,
        Credential::None,
    )
    .unwrap();
    let c = Client::new(
        Arc::new(HttpTransport::new(http)),
        &endpoint,
        REGION,
        Arc::new(CredentialProvider::fixed(auth::local(None))),
    )
    .unwrap();
    let started = std::time::Instant::now();
    let out = c.call("ListTables", &json!({}), None).await;
    assert!(matches!(out, Err(AppError::Timeout)), "{out:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(server.requests().len(), 1);
}
