//! The driver's parts together against the in-memory DynamoDB of `testing`: the catalog, grid
//! pages and cursors, counts, cells, the console, edits with their guards and transactions,
//! retries, and the signed wire protocol over a socket.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone as _, Utc};
use serde_json::json;

use super::attr::{AttrType, Item, item_from_json};
use super::auth;
use super::browse::{self, GridOptions, Outcome};
use super::client::{Client, RetryPolicy};
use super::testing::{FakeDynamo, http_transport};
use crate::drivers::common::aws::credentials::{CredentialProvider, Credentials, Fetch};
use crate::error::AppError;
use crate::sqlgen::native::dynamo::{self as b, ConsoleCommand, KeySchema, LoadedItems};
use crate::testkit::FakeServer;
use crate::types::browse::{
    BrowsePage, BrowseRequest, Conjunction, FilterGroup, FilterNode, FilterOp, RowKey, SortDir,
    SortSpec,
};
use crate::types::mutate::StagedChange;
use crate::types::schema::TableMeta;
use crate::types::schema::TableRef;
use crate::types::value::Value;

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 29, 10, 0, 0).unwrap()
}

fn fast() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 4,
        base: Duration::ZERO,
        cap: Duration::ZERO,
    }
}

fn local() -> Arc<CredentialProvider> {
    Arc::new(CredentialProvider::fixed(auth::local(None)))
}

fn client(fake: &Arc<FakeDynamo>) -> Client {
    Client::new(fake.clone(), "http://localhost:8000/", "us-east-1", local())
        .unwrap()
        .with_retry(fast())
        .with_clock(Arc::new(at))
}

/// A grid page of the `users` table, its items remembered in `loaded` as the session does.
async fn grid_page(
    c: &Client,
    meta: &TableMeta,
    req: &BrowseRequest,
    opts: &GridOptions,
    loaded: &mut LoadedItems,
) -> Result<BrowsePage, AppError> {
    let (page, items) = browse::browse(c, meta, &keys(), None, req, opts, None).await?;
    for item in &items {
        loaded.record(&keys(), item);
    }
    Ok(page)
}

/// `users`: partition key `org` (S), sort key `id` (N), five items.
fn users() -> Arc<FakeDynamo> {
    let fake = FakeDynamo::new();
    fake.create_table("users", ("org", AttrType::S), Some(("id", AttrType::N)));
    fake.set_ttl("users", "expiresAt");
    for (org, id, name, zip) in [
        ("acme", 1, "Ann", "02134"),
        ("acme", 2, "Bob", "10001"),
        ("acme", 3, "Cy", "94105"),
        ("beta", 1, "Di", "60601"),
        ("beta", 2, "Ed", "73301"),
    ] {
        fake.put(
            "users",
            json!({"org": {"S": org}, "id": {"N": id.to_string()}, "name": {"S": name}, "zip": {"S": zip}}),
        );
    }
    fake.put(
        "users",
        json!({"org": {"S": "beta"}, "id": {"N": "3"}, "name": {"S": "Flo"}, "tags": {"SS": ["x"]}, "big": {"N": "12345678901234567890"}}),
    );
    fake
}

fn keys() -> KeySchema {
    KeySchema {
        hash: ("org".into(), AttrType::S),
        range: Some(("id".into(), AttrType::N)),
    }
}

fn request(limit: u32, cursor: Option<String>, filter: Option<FilterGroup>) -> BrowseRequest {
    BrowseRequest {
        table: TableRef::new(Some("us-east-1"), "users"),
        columns: None,
        filter,
        sort: Vec::new(),
        limit,
        offset: 0,
        cursor,
    }
}

fn opts(page_requests: u32) -> GridOptions {
    GridOptions {
        consistent: false,
        consumed_capacity: true,
        page_requests,
        max_bytes: browse::Budget::DEFAULT.bytes,
    }
}

fn eq(column: &str, v: Value) -> FilterNode {
    FilterNode::Condition {
        column: column.into(),
        op: FilterOp::Eq,
        values: vec![v],
    }
}

#[tokio::test]
async fn tables_are_listed_page_by_page() {
    let fake = FakeDynamo::new();
    for n in ["a", "b", "c", "d", "e"] {
        fake.create_table(n, ("id", AttrType::S), None);
    }
    fake.set_page(2);
    let names = browse::list_tables(&client(&fake), None).await.unwrap();
    assert_eq!(names, ["a", "b", "c", "d", "e"]);
    let starts: Vec<_> = fake
        .requests()
        .iter()
        .map(|(_, body)| body.get("ExclusiveStartTableName").cloned())
        .collect();
    assert_eq!(starts, [None, Some(json!("b")), Some(json!("d"))]);
}

#[tokio::test]
async fn table_meta_from_the_description_and_a_sample() {
    let fake = users();
    let c = client(&fake);
    let (meta, desc) = browse::table_meta(&c, "users", 100, None).await.unwrap();
    assert_eq!(desc.keys, keys());
    let names: Vec<&str> = meta.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["org", "id", "name", "zip", "big", "tags"]);
    assert_eq!(meta.row_estimate, Some(6));
    assert!(
        meta.comment
            .as_deref()
            .unwrap()
            .contains("TTL on expiresAt")
    );
    assert_eq!(meta.table.schema.as_deref(), Some("us-east-1"));
    assert!(meta.ddl.as_deref().unwrap().starts_with("CreateTable {"));
    assert!(matches!(
        browse::describe(&c, "nope", None).await,
        Err(AppError::Db { detail: Some(ref d), .. }) if d == "ResourceNotFoundException"
    ));
}

#[tokio::test]
async fn grid_pages_follow_the_cursor() {
    let fake = users();
    let c = client(&fake);
    let (meta, _) = browse::table_meta(&c, "users", 100, None).await.unwrap();
    let mut loaded = LoadedItems::default();
    let p1 = grid_page(&c, &meta, &request(4, None, None), &opts(5), &mut loaded)
        .await
        .unwrap();
    assert_eq!(p1.rows.len(), 4);
    assert_eq!(p1.pk_columns, ["org", "id"]);
    assert!(p1.sql.starts_with("Scan {"));
    let cursor = p1.next_cursor.clone().expect("a cursor");
    let p2 = grid_page(
        &c,
        &meta,
        &request(4, Some(cursor), None),
        &opts(5),
        &mut loaded,
    )
    .await
    .unwrap();
    assert_eq!(p2.rows.len(), 2);
    assert_eq!(p2.next_cursor, None);
    assert_eq!(loaded.len(), 6);
    // Values keep their types; attributes an item lacks are missing, not NULL.
    let col = |name: &str| p2.columns.iter().position(|c| c.name == name).unwrap();
    let flo = &p2.rows[1];
    assert_eq!(flo[col("name")], Value::Text("Flo".into()));
    assert_eq!(
        flo[col("big")],
        Value::BigInt("12345678901234567890".into())
    );
    assert_eq!(
        flo[col("tags")],
        Value::Array(vec![Value::Text("x".into())])
    );
    assert!(super::attr::is_missing(&flo[col("zip")]));
    assert_eq!(p1.rows[0][col("zip")], Value::Text("02134".into()));
    // Only the columns asked for.
    let mut only = request(1, None, None);
    only.columns = Some(vec!["name".into(), "org".into(), "nope".into()]);
    let p = grid_page(&c, &meta, &only, &opts(5), &mut loaded)
        .await
        .unwrap();
    assert_eq!(
        p.columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["name", "org"]
    );
    assert_eq!(p.rows[0].len(), 2);
    assert!(matches!(
        grid_page(
            &c,
            &meta,
            &request(1, Some("bogus!".into()), None),
            &opts(5),
            &mut loaded
        )
        .await,
        Err(AppError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn filtered_pages_take_several_requests_and_stop_with_a_cursor() {
    let fake = users();
    let c = client(&fake);
    let (meta, _) = browse::table_meta(&c, "users", 100, None).await.unwrap();
    let mut loaded = LoadedItems::default();
    let only_ed = FilterGroup {
        conjunction: Conjunction::And,
        items: vec![eq("name", Value::Text("Ed".into()))],
    };
    let before = fake.requests().len();
    // Two items per request, at most two requests: Ed (the fifth item) is not reached.
    let p = grid_page(
        &c,
        &meta,
        &request(2, None, Some(only_ed.clone())),
        &opts(2),
        &mut loaded,
    )
    .await
    .unwrap();
    assert!(p.rows.is_empty());
    assert_eq!(fake.requests().len() - before, 2);
    let p = grid_page(
        &c,
        &meta,
        &request(2, p.next_cursor, Some(only_ed)),
        &opts(2),
        &mut loaded,
    )
    .await
    .unwrap();
    assert_eq!(p.rows.len(), 1);
}

#[tokio::test]
async fn a_partition_key_filter_queries_and_sorts() {
    let fake = users();
    let c = client(&fake);
    let (meta, _) = browse::table_meta(&c, "users", 100, None).await.unwrap();
    let mut loaded = LoadedItems::default();
    let mut req = request(
        10,
        None,
        Some(FilterGroup {
            conjunction: Conjunction::And,
            items: vec![eq("org", Value::Text("acme".into()))],
        }),
    );
    req.sort = vec![SortSpec {
        column: "id".into(),
        dir: SortDir::Desc,
        nulls: None,
    }];
    let p = grid_page(&c, &meta, &req, &opts(5), &mut loaded)
        .await
        .unwrap();
    assert!(p.sql.starts_with("Query {"), "{}", p.sql);
    let ids: Vec<&Value> = p.rows.iter().map(|r| &r[1]).collect();
    assert_eq!(ids, [&Value::Int(3), &Value::Int(2), &Value::Int(1)]);
    // A sort a Scan cannot serve is refused.
    let mut scan = request(10, None, None);
    scan.sort = req.sort.clone();
    assert!(matches!(
        grid_page(&c, &meta, &scan, &opts(5), &mut loaded).await,
        Err(AppError::Unsupported(_))
    ));
}

/// DynamoDB refuses a Query whose filter names a key attribute: such filters are a Scan, and
/// the fake refuses what DynamoDB refuses.
#[tokio::test]
async fn a_query_never_filters_a_key() {
    let fake = users();
    let c = client(&fake);
    let (meta, _) = browse::table_meta(&c, "users", 100, None).await.unwrap();
    let mut loaded = LoadedItems::default();
    let cmp = |column: &str, op: FilterOp, v: i64| FilterNode::Condition {
        column: column.into(),
        op,
        values: vec![Value::Int(v)],
    };
    let and = |items: Vec<FilterNode>| FilterGroup {
        conjunction: Conjunction::And,
        items,
    };
    let acme = || eq("org", Value::Text("acme".into()));
    let cases = [
        // A second sort key condition.
        (
            and(vec![
                acme(),
                cmp("id", FilterOp::Gt, 1),
                cmp("id", FilterOp::Lt, 3),
            ]),
            vec![("acme", 2)],
        ),
        // A second partition key condition.
        (
            and(vec![acme(), eq("org", Value::Text("beta".into()))]),
            vec![],
        ),
        // An empty string, which DynamoDB refuses as a key condition's value.
        (and(vec![eq("org", Value::Text(String::new()))]), vec![]),
        // Any column next to the partition key: the key columns are columns too.
        (
            and(vec![
                acme(),
                FilterNode::AnyColumn {
                    op: FilterOp::Eq,
                    values: vec![Value::Text("3".into())],
                },
            ]),
            vec![("acme", 3)],
        ),
    ];
    for (filter, want) in cases {
        let p = grid_page(
            &c,
            &meta,
            &request(10, None, Some(filter.clone())),
            &opts(5),
            &mut loaded,
        )
        .await
        .unwrap();
        assert!(p.sql.starts_with("Scan {"), "{}", p.sql);
        let got: Vec<(Value, Value)> = p
            .rows
            .iter()
            .map(|r| (r[0].clone(), r[1].clone()))
            .collect();
        let want: Vec<(Value, Value)> = want
            .iter()
            .map(|(o, i)| (Value::Text((*o).into()), Value::Int(*i)))
            .collect();
        assert_eq!(got, want, "{filter:?}");
        let n = browse::count(&c, &meta, &keys(), Some(&filter), false, false, None)
            .await
            .unwrap();
        assert_eq!(n.count, want.len() as i64, "{filter:?}");
    }
    // One sort key condition and a filter of other attributes stay a Query.
    let one = and(vec![
        acme(),
        cmp("id", FilterOp::Ge, 2),
        eq("name", Value::Text("Cy".into())),
    ]);
    let p = grid_page(
        &c,
        &meta,
        &request(10, None, Some(one)),
        &opts(5),
        &mut loaded,
    )
    .await
    .unwrap();
    assert!(p.sql.starts_with("Query {"), "{}", p.sql);
    assert_eq!(p.rows.len(), 1);
    let names = json!({"#o": "org", "#i": "id", "#n": "name"});
    let values = json!({
        ":a": {"S": "acme"}, ":b": {"N": "1"}, ":c": {"N": "3"}, ":d": {"S": "Cy"}, ":e": {"S": ""}
    });
    for (kc, filter) in [
        ("#o = :e", None),
        ("#o = :a AND #i > :b", Some("#i < :c")),
        ("#o = :a", Some("#o = :a")),
        ("#o = :a", Some("(#n = :d OR #i = :b)")),
        ("#o = :a AND #i > :b AND #i < :c", None),
        ("#o = :a AND #o = :a", None),
        ("#i > :b", None),
        ("#o = :a OR #i > :b", None),
        ("#o > :a", None),
        ("#o = :a AND #i <> :b", None),
        ("#o = :a AND #n = :d", None),
        ("#o = :a AND NOT #i = :b", None),
    ] {
        let mut body = json!({
            "TableName": "users",
            "KeyConditionExpression": kc,
            "ExpressionAttributeNames": names,
            "ExpressionAttributeValues": values,
        });
        if let Some(f) = filter {
            body["FilterExpression"] = json!(f);
        }
        let err = c.call("Query", &body, None).await.unwrap_err().to_string();
        assert!(
            err.contains("ValidationException")
                || err.contains("not supported")
                || err.contains("primary key")
                || err.contains("empty string"),
            "{kc} {filter:?}: {err}"
        );
    }
    for kc in [
        "#o = :a",
        "#o = :a AND #i = :b",
        "#o = :a AND #i BETWEEN :b AND :c",
        "#o = :a AND #i >= :b",
    ] {
        let body = json!({
            "TableName": "users",
            "KeyConditionExpression": kc,
            "FilterExpression": "#n = :d",
            "ExpressionAttributeNames": names,
            "ExpressionAttributeValues": values,
        });
        c.call("Query", &body, None).await.unwrap();
    }
}

#[tokio::test]
async fn counts_estimate_or_scan_everything() {
    let fake = users();
    let c = client(&fake);
    let (meta, _) = browse::table_meta(&c, "users", 100, None).await.unwrap();
    let est = browse::count(&c, &meta, &keys(), None, false, false, None)
        .await
        .unwrap();
    assert_eq!((est.count, est.exact), (6, false));
    let exact = browse::count(&c, &meta, &keys(), None, true, false, None)
        .await
        .unwrap();
    assert_eq!((exact.count, exact.exact), (6, true));
    let beta = FilterGroup {
        conjunction: Conjunction::And,
        items: vec![eq("org", Value::Text("beta".into()))],
    };
    let filtered = browse::count(&c, &meta, &keys(), Some(&beta), false, false, None)
        .await
        .unwrap();
    assert_eq!((filtered.count, filtered.exact), (3, true));
    let (action, body) = fake.requests().last().cloned().unwrap();
    assert_eq!(action, "Query");
    assert_eq!(body["Select"], json!("COUNT"));
}

#[tokio::test]
async fn cells_load_whole_values() {
    let fake = users();
    let c = client(&fake);
    let key = RowKey::Pk(BTreeMap::from([
        ("org".to_string(), Value::Text("beta".into())),
        ("id".to_string(), Value::Int(3)),
    ]));
    assert_eq!(
        browse::fetch_cell(&c, "users", &keys(), &key, "big", true, None)
            .await
            .unwrap(),
        Value::BigInt("12345678901234567890".into())
    );
    assert!(super::attr::is_missing(
        &browse::fetch_cell(&c, "users", &keys(), &key, "zip", false, None)
            .await
            .unwrap()
    ));
    let gone = RowKey::Pk(BTreeMap::from([
        ("org".to_string(), Value::Text("zeta".into())),
        ("id".to_string(), Value::Int(1)),
    ]));
    assert!(matches!(
        browse::fetch_cell(&c, "users", &keys(), &gone, "name", false, None).await,
        Err(AppError::NotFound(_))
    ));
}

#[tokio::test]
async fn console_runs_partiql_pages_and_api_requests() {
    let fake = users();
    fake.set_page(2);
    let c = client(&fake);
    let sel = b::parse_command("SELECT * FROM \"users\"").unwrap();
    let out = browse::run_command(&c, &sel, 100, None).await.unwrap();
    assert_eq!(out.rows.len(), 6);
    let names: Vec<&str> = out.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(&names[..2], ["id", "name"]);
    assert_eq!(out.columns[0].data_type, "N");
    assert_eq!(out.affected, None);
    let capped = browse::run_command(&c, &sel, 3, None).await.unwrap();
    assert_eq!(capped.rows.len(), 3);
    assert!(capped.truncated);
    let scan = b::parse_command("Scan {\"TableName\": \"users\", \"Limit\": 2}").unwrap();
    let out = browse::run_command(&c, &scan, 100, None).await.unwrap();
    assert_eq!(out.rows.len(), 2);
    assert!(out.notices[0].contains("ExclusiveStartKey"));
    let describe = b::parse_command("DescribeTable {\"TableName\": \"users\"}").unwrap();
    let out = browse::run_command(&c, &describe, 100, None).await.unwrap();
    assert_eq!(out.columns[0].name, "reply");
    assert!(matches!(&out.rows[0][0], Value::Json(j) if j.contains("\"TableName\":\"users\"")));
    let bad = ConsoleCommand::Partiql("SELEC nonsense".into());
    assert!(browse::run_command(&c, &bad, 100, None).await.is_err());
}

fn update(org: &str, id: i64, changes: &[(&str, Value)]) -> StagedChange {
    StagedChange::Update {
        row_id: format!("{org}{id}"),
        key: RowKey::Pk(BTreeMap::from([
            ("org".to_string(), Value::Text(org.into())),
            ("id".to_string(), Value::Int(id)),
        ])),
        changes: changes
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        exprs: None,
    }
}

async fn loaded_page(c: &Client) -> (TableMeta, LoadedItems) {
    let (meta, _) = browse::table_meta(c, "users", 100, None).await.unwrap();
    let mut loaded = LoadedItems::default();
    grid_page(c, &meta, &request(10, None, None), &opts(5), &mut loaded)
        .await
        .unwrap();
    (meta, loaded)
}

fn item(fake: &FakeDynamo, org: &str, id: &str) -> Option<Item> {
    fake.items("users").into_iter().find(|i| {
        i.get("org") == Some(&super::attr::Attr::S(org.into()))
            && i.get("id") == Some(&super::attr::Attr::N(id.into()))
    })
}

#[tokio::test]
async fn edits_keep_types_and_stop_at_a_changed_item() {
    let fake = users();
    let c = client(&fake);
    let (meta, loaded) = loaded_page(&c).await;
    // Someone else renames Bob after the grid loaded him.
    fake.put(
        "users",
        json!({"org": {"S": "acme"}, "id": {"N": "2"}, "name": {"S": "Robert"}, "zip": {"S": "10001"}}),
    );
    let changes = vec![
        update("acme", 1, &[("zip", Value::Text("02135".into()))]),
        update("acme", 2, &[("name", Value::Text("Bobby".into()))]),
        update("acme", 3, &[("name", Value::Text("Cyrus".into()))]),
    ];
    let plan = b::plan_changes("users", &keys(), &meta.columns, &loaded, &changes, "tok").unwrap();
    let out = browse::apply_each(&c, &plan, None).await;
    assert!(matches!(out[0].1, Outcome::Applied));
    match &out[1].1 {
        Outcome::Failed(e) => assert!(
            e.to_string().contains("changed or deleted by someone else"),
            "{e}"
        ),
        other => panic!("{other:?}"),
    }
    assert!(matches!(out[2].1, Outcome::NotApplied(_)));
    assert_eq!(
        out[2].1.error().unwrap(),
        "not applied: an earlier change failed"
    );
    // The zip stayed a string with its leading zero; Robert was not overwritten; Cy untouched.
    assert_eq!(
        item(&fake, "acme", "1").unwrap()["zip"],
        super::attr::Attr::S("02135".into())
    );
    assert_eq!(
        item(&fake, "acme", "2").unwrap()["name"],
        super::attr::Attr::S("Robert".into())
    );
    assert_eq!(
        item(&fake, "acme", "3").unwrap()["name"],
        super::attr::Attr::S("Cy".into())
    );
}

#[tokio::test]
async fn transactions_apply_all_or_report_each_reason() {
    let fake = users();
    let c = client(&fake);
    let (meta, loaded) = loaded_page(&c).await;
    let insert_dup = StagedChange::Insert {
        row_id: "n1".into(),
        values: BTreeMap::from([
            ("org".to_string(), Value::Text("acme".into())),
            ("id".to_string(), Value::Int(1)),
        ]),
        exprs: None,
    };
    let changes = vec![
        update("acme", 3, &[("name", Value::Text("Cyrus".into()))]),
        insert_dup,
        StagedChange::Delete {
            row_id: "d".into(),
            key: RowKey::Pk(BTreeMap::from([
                ("org".to_string(), Value::Text("beta".into())),
                ("id".to_string(), Value::Int(1)),
            ])),
        },
    ];
    let plan = b::plan_changes("users", &keys(), &meta.columns, &loaded, &changes, "tok").unwrap();
    let out = browse::apply_atomic(&c, &plan, None).await.unwrap();
    assert!(
        matches!(out[0].1, Outcome::NotApplied(ref w) if w.contains("item 2 failed")),
        "{:?}",
        out[0]
    );
    assert!(
        matches!(out[1].1, Outcome::Failed(ref e) if e.to_string().contains("someone else")),
        "{:?}",
        out[1]
    );
    assert!(matches!(out[2].1, Outcome::NotApplied(_)));
    assert_eq!(
        item(&fake, "acme", "3").unwrap()["name"],
        super::attr::Attr::S("Cy".into())
    );
    assert!(item(&fake, "beta", "1").is_some());

    // Without the failing insert, all of it applies at once.
    let ok = vec![changes[0].clone(), changes[2].clone()];
    let plan = b::plan_changes("users", &keys(), &meta.columns, &loaded, &ok, "tok2").unwrap();
    let out = browse::apply_atomic(&c, &plan, None).await.unwrap();
    assert!(out.iter().all(|(_, o)| matches!(o, Outcome::Applied)));
    assert_eq!(
        item(&fake, "acme", "3").unwrap()["name"],
        super::attr::Attr::S("Cyrus".into())
    );
    assert!(item(&fake, "beta", "1").is_none());
}

#[tokio::test]
async fn a_changed_key_moves_the_item_in_one_transaction() {
    let fake = users();
    let c = client(&fake);
    let (meta, loaded) = loaded_page(&c).await;
    let changes = vec![update("acme", 3, &[("id", Value::Int(30))])];
    let plan = b::plan_changes("users", &keys(), &meta.columns, &loaded, &changes, "tok").unwrap();
    let out = browse::apply_each(&c, &plan, None).await;
    assert!(matches!(out[0].1, Outcome::Applied));
    assert!(item(&fake, "acme", "3").is_none());
    let moved = item(&fake, "acme", "30").unwrap();
    assert_eq!(moved["name"], super::attr::Attr::S("Cy".into()));
    assert_eq!(moved["zip"], super::attr::Attr::S("94105".into()));
    assert_eq!(
        fake.actions().last().map(String::as_str),
        Some("TransactWriteItems")
    );
}

#[tokio::test]
async fn throttling_and_server_errors_are_retried() {
    let fake = users();
    let c = client(&fake);
    fake.fail_next(
        "Scan",
        400,
        "ProvisionedThroughputExceededException",
        "slow down",
    );
    fake.fail_next("Scan", 500, "InternalServerError", "oops");
    let items = browse::sample(&c, "users", 10, None).await.unwrap();
    assert_eq!(items.len(), 6);
    assert_eq!(fake.actions(), ["Scan", "Scan", "Scan"]);
    // A write is not repeated after a server error, but a transaction with a token is.
    fake.fail_next("PutItem", 500, "InternalServerError", "oops");
    let put = b::put_new(
        "users",
        &keys(),
        &item_from_json(&json!({"org": {"S": "new"}, "id": {"N": "1"}})).unwrap(),
    );
    assert!(c.call("PutItem", &put.body, None).await.is_err());
    assert!(item(&fake, "new", "1").is_none());
    fake.fail_next("TransactWriteItems", 500, "InternalServerError", "oops");
    let tx = json!({"TransactItems": [{"Put": put.body}], "ClientRequestToken": "t-1"});
    c.call("TransactWriteItems", &tx, None).await.unwrap();
    assert!(item(&fake, "new", "1").is_some());
}

#[tokio::test]
async fn expired_tokens_are_fetched_again() {
    let fake = users();
    fake.expire_token("old");
    let fetch: Fetch =
        Arc::new(|| Box::pin(async { Credentials::static_keys("ASIA", "s", Some("new")) }));
    let c = Client::new(
        fake.clone(),
        "http://localhost:8000/",
        "us-east-1",
        Arc::new(CredentialProvider::fetching(
            fetch,
            Some(Credentials::static_keys("ASIA", "s", Some("old")).unwrap()),
        )),
    )
    .unwrap()
    .with_retry(fast());
    assert_eq!(browse::list_tables(&c, None).await.unwrap(), ["users"]);
    assert_eq!(fake.actions(), ["ListTables", "ListTables"]);
}

/// The whole signed protocol over a socket: the session's transport (the shared `HttpClient`)
/// to the fake server, which checks each signature against the secret and its own clock.
#[tokio::test]
async fn signed_requests_over_http() {
    let fake = users();
    fake.check_signatures("AKIDEXAMPLE", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY");
    let server = FakeServer::start(vec![fake.route()]).await;
    let endpoint = server.url("/");
    let signed = |secret: &str, token: Option<&str>| {
        Client::new(
            http_transport(&endpoint),
            &endpoint,
            "us-east-1",
            Arc::new(CredentialProvider::fixed(
                Credentials::static_keys("AKIDEXAMPLE", secret, token).unwrap(),
            )),
        )
        .unwrap()
        .with_retry(fast())
    };
    let good = signed("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", Some("session"));
    assert_eq!(browse::list_tables(&good, None).await.unwrap(), ["users"]);
    let (meta, _) = browse::table_meta(&good, "users", 10, None).await.unwrap();
    assert_eq!(meta.primary_key, ["org", "id"]);
    let sent = &server.requests()[0];
    assert_eq!(
        sent.header("x-amz-target"),
        Some("DynamoDB_20120810.ListTables")
    );
    assert_eq!(sent.header("x-amz-security-token"), Some("session"));
    assert_eq!(
        sent.header("content-type"),
        Some("application/x-amz-json-1.0")
    );

    let bad = signed("wrong", None);
    match browse::list_tables(&bad, None).await {
        Err(AppError::Auth(m)) => assert!(m.contains("InvalidSignatureException"), "{m}"),
        other => panic!("{other:?}"),
    }

    // A clock 20 minutes behind the server's: corrected from the reply's Date, once.
    fake.set_clock(Utc::now() + chrono::Duration::minutes(20));
    let skewed = good.with_clock(Arc::new(Utc::now));
    assert_eq!(browse::list_tables(&skewed, None).await.unwrap(), ["users"]);
}

#[tokio::test]
async fn cancel_stops_a_grid_page() {
    let fake = users();
    for _ in 0..10 {
        fake.fail_next("Scan", 400, "ThrottlingException", "slow");
    }
    let c = Client::new(fake.clone(), "http://localhost:8000/", "us-east-1", local())
        .unwrap()
        .with_retry(RetryPolicy {
            max_attempts: 20,
            base: Duration::from_secs(30),
            cap: Duration::from_secs(30),
        });
    let token = tokio_util::sync::CancellationToken::new();
    let stop = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        stop.cancel();
    });
    let out = browse::sample(&c, "users", 10, Some(&token)).await;
    assert!(matches!(out, Err(AppError::Cancelled)), "{out:?}");
}

#[tokio::test]
async fn a_page_with_more_items_than_its_limit_is_refused() {
    use super::transport::{HttpRequest, HttpResponse, Transport};
    struct Generous;
    #[async_trait::async_trait]
    impl Transport for Generous {
        async fn send(&self, _req: HttpRequest) -> Result<HttpResponse, AppError> {
            Ok(HttpResponse::json(
                200,
                &json!({"Items": [{"id": {"S": "a"}}, {"id": {"S": "b"}}, {"id": {"S": "c"}}]}),
            ))
        }
    }
    let c = Client::new(
        Arc::new(Generous),
        "http://localhost:8000/",
        "local",
        local(),
    )
    .unwrap();
    let k = KeySchema {
        hash: ("id".into(), AttrType::S),
        range: None,
    };
    let read = b::read_request(&b::ReadSpec {
        table: "t",
        index: None,
        keys: &k,
        columns: &[],
        filter: None,
        descending: false,
        consistent: false,
        consumed_capacity: false,
    })
    .unwrap();
    assert!(matches!(
        browse::read_page(&c, &read, 2, None, 5, usize::MAX, None).await,
        Err(AppError::Db { .. })
    ));
    assert_eq!(
        browse::read_page(&c, &read, 3, None, 5, usize::MAX, None)
            .await
            .unwrap()
            .items
            .len(),
        3
    );
}

/// Replies with `replies` in turn (the last one again), whatever is asked.
struct Canned(std::sync::Mutex<Vec<serde_json::Value>>);

#[async_trait::async_trait]
impl super::transport::Transport for Canned {
    async fn send(
        &self,
        _req: super::transport::HttpRequest,
    ) -> Result<super::transport::HttpResponse, AppError> {
        let mut replies = self.0.lock().unwrap();
        let reply = if replies.len() > 1 {
            replies.remove(0)
        } else {
            replies[0].clone()
        };
        Ok(super::transport::HttpResponse::json(200, &reply))
    }
}

fn canned(replies: Vec<serde_json::Value>) -> Client {
    Client::new(
        Arc::new(Canned(std::sync::Mutex::new(replies))),
        "http://localhost:8000/",
        "local",
        local(),
    )
    .unwrap()
}

/// A PartiQL INSERT or UPDATE that succeeds changed one item (DynamoDB refuses a duplicate key
/// and a missing one); a DELETE of a missing key succeeds too, so only the item it returned
/// proves a deletion.
#[tokio::test]
async fn partiql_writes_count_only_what_their_reply_proves() {
    let delete = b::parse_command("DELETE FROM \"users\" WHERE org = 'acme' AND id = 99").unwrap();
    let out = browse::run_command(&client(&users()), &delete, 100, None)
        .await
        .unwrap();
    assert!(out.write && out.columns.is_empty());
    assert_eq!(out.affected, None);
    let returning =
        b::parse_command("DELETE FROM \"users\" WHERE org = 'acme' AND id = 1 RETURNING ALL OLD *")
            .unwrap();
    let old = canned(vec![
        json!({"Items": [{"org": {"S": "acme"}, "id": {"N": "1"}}]}),
    ]);
    let out = browse::run_command(&old, &returning, 100, None)
        .await
        .unwrap();
    assert_eq!((out.affected, out.rows.len()), (Some(1), 1));
    let none = canned(vec![json!({"Items": []})]);
    let out = browse::run_command(&none, &returning, 100, None)
        .await
        .unwrap();
    assert_eq!(out.affected, None);
    for one in [
        "UPDATE \"users\" SET a = 1 WHERE org = 'acme' AND id = 1",
        "INSERT INTO \"users\" VALUE {'org': 'acme', 'id': 7}",
    ] {
        let cmd = b::parse_command(one).unwrap();
        let out = browse::run_command(&none, &cmd, 100, None).await.unwrap();
        assert_eq!((out.write, out.affected), (true, Some(1)), "{one}");
    }
    let select = b::parse_command("SELECT * FROM \"users\"").unwrap();
    let out = browse::run_command(&none, &select, 100, None)
        .await
        .unwrap();
    assert_eq!((out.write, out.affected), (false, None));
}

/// Counts that would overflow (or go negative) are a server's error, not a wrapped number.
#[tokio::test]
async fn a_count_that_overflows_is_refused() {
    let (meta, _) = browse::table_meta(&client(&users()), "users", 10, None)
        .await
        .unwrap();
    let next = json!({"org": {"S": "acme"}, "id": {"N": "1"}});
    for last in [5, -3] {
        let c = canned(vec![
            json!({"Count": i64::MAX - 1, "LastEvaluatedKey": next}),
            json!({"Count": last}),
        ]);
        match browse::count(&c, &meta, &keys(), None, true, false, None).await {
            Err(e @ AppError::Db { .. }) => assert!(e.to_string().contains("Count"), "{e}"),
            other => panic!("{other:?}"),
        }
    }
    let c = canned(vec![
        json!({"Count": 2, "LastEvaluatedKey": next}),
        json!({"Count": 3}),
    ]);
    let n = browse::count(&c, &meta, &keys(), None, true, false, None)
        .await
        .unwrap();
    assert_eq!((n.count, n.exact), (5, true));
}

/// A reply body past its action's cap (a hostile or broken server streaming on) is refused once
/// it passes the cap, before it fills memory; a `BatchGetItem` has room for DynamoDB's 16 MB.
#[tokio::test]
async fn a_reply_past_the_size_cap_is_refused() {
    use super::transport::max_reply_bytes;
    use crate::testkit::{Piece, Reply, Route};
    let page = max_reply_bytes(Some("Scan"));
    assert_eq!(page, 8 << 20);
    assert_eq!(max_reply_bytes(Some("BatchGetItem")), 32 << 20);
    let mib = vec![b' '; 1 << 20];
    let pieces = (0..=page >> 20).map(|_| Piece::Data(mib.clone())).collect();
    let server = FakeServer::start(vec![
        Route::new("POST", "/*").reply(Reply::new(200).chunked(pieces)),
    ])
    .await;
    let endpoint = server.url("/");
    let c = Client::new(http_transport(&endpoint), &endpoint, "local", local())
        .unwrap()
        .with_retry(fast());
    match c.call("Scan", &json!({}), None).await {
        Err(e @ AppError::Db { .. }) => assert!(e.to_string().contains("larger than 8 MB"), "{e}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        c.call("BatchGetItem", &json!({}), None).await.unwrap(),
        json!({})
    );
}

/// A reply of many small objects (each a tree node once parsed: 100 times its text) is refused
/// before it is parsed, while a full grid page of real items is read.
#[tokio::test]
async fn a_reply_too_big_once_parsed_is_refused() {
    use super::transport::{MAX_REPLY_MEMORY, max_reply_bytes, parsed_size};
    let tiny: Vec<serde_json::Value> = (0..200_000).map(|_| json!({"a": {"NULL": true}})).collect();
    let tiny = json!({ "Items": tiny });
    let text = tiny.to_string();
    assert!(text.len() < max_reply_bytes(Some("Scan")));
    assert!(parsed_size(text.as_bytes()) > MAX_REPLY_MEMORY);
    match canned(vec![tiny]).call("Scan", &json!({}), None).await {
        Err(e @ AppError::Db { .. }) => assert!(e.to_string().contains("too many values"), "{e}"),
        other => panic!("{other:?}"),
    }
    let mut deep = json!(0);
    for _ in 0..60 {
        deep = json!({ "": deep });
    }
    // One-entry maps take a tree node each, far more than their text.
    let deep: Vec<serde_json::Value> = (0..3_000).map(|_| deep.clone()).collect();
    let deep = json!({ "Items": deep });
    let text = deep.to_string();
    assert!(text.len() < max_reply_bytes(Some("Scan")));
    assert!(parsed_size(text.as_bytes()) > MAX_REPLY_MEMORY);
    match canned(vec![deep]).call("Scan", &json!({}), None).await {
        Err(e @ AppError::Db { .. }) => assert!(e.to_string().contains("too many values"), "{e}"),
        other => panic!("{other:?}"),
    }

    // 1,000 items of 50 attributes, nested maps among them.
    let item: serde_json::Map<String, serde_json::Value> = (0..50)
        .map(|i| {
            let v = if i % 5 == 0 {
                json!({"M": {"k": {"M": {"n": {"N": "1"}}}}})
            } else {
                json!({"S": format!("value {i}")})
            };
            (format!("attribute_{i}"), v)
        })
        .collect();
    let page = json!({ "Items": vec![serde_json::Value::Object(item); 1_000] });
    assert!(parsed_size(page.to_string().as_bytes()) < MAX_REPLY_MEMORY);
    let reply = canned(vec![page])
        .call("Scan", &json!({}), None)
        .await
        .unwrap();
    assert_eq!(reply["Items"].as_array().map(Vec::len), Some(1_000));
}

/// Items of `n` keys, each with an attribute of its own (a sparse table).
fn sparse(n: usize) -> serde_json::Value {
    let items: Vec<serde_json::Value> = (0..n)
        .map(|i| {
            let mut item = serde_json::Map::new();
            item.insert("pk".into(), json!({"S": format!("k{i}")}));
            item.insert(format!("a{i}"), json!({"N": "1"}));
            serde_json::Value::Object(item)
        })
        .collect();
    json!(items)
}

/// A sparse result shows at most `MAX_COLUMNS` attributes and the rows its cells hold, with
/// notices, rather than rows times attributes of missing cells (PartiQL and API replies alike).
#[tokio::test]
async fn sparse_console_results_are_capped() {
    use super::catalog::MAX_COLUMNS;
    let n = MAX_COLUMNS + 100;
    let reply = json!({"Items": sparse(n)});
    for command in ["Scan {\"TableName\": \"t\"}", "SELECT * FROM \"t\""] {
        let cmd = b::parse_command(command).unwrap();
        let out = browse::run_command(&canned(vec![reply.clone()]), &cmd, 10_000, None)
            .await
            .unwrap();
        assert_eq!(out.columns.len(), MAX_COLUMNS, "{command}");
        assert_eq!(out.columns[0].name, "a0");
        assert_eq!(out.rows.len(), n);
        assert!(out.rows.iter().all(|r| r.len() == MAX_COLUMNS));
        assert!(!out.truncated);
        assert!(
            out.notices.iter().any(|m| m.contains("101 are not shown")),
            "{:?}",
            out.notices
        );
        // The cells hold 4 rows of 500 attributes.
        let small = browse::Budget {
            bytes: usize::MAX,
            cells: 4 * MAX_COLUMNS + 7,
        };
        let out =
            browse::run_command_within(&canned(vec![reply.clone()]), &cmd, 10_000, small, None)
                .await
                .unwrap();
        assert_eq!((out.rows.len(), out.truncated), (4, true), "{command}");
        assert!(
            out.notices.iter().any(|m| m.contains("first 4 items")),
            "{:?}",
            out.notices
        );
    }
}

/// A statement whose pages go on stops once a budget of item bytes is in (DynamoDB's pages are
/// 1 MB; a hostile server's can be 8 MB each), and so does a grid page.
#[tokio::test]
async fn reads_stop_at_their_byte_budget() {
    let big = json!({"pk": {"S": "x".repeat(1000)}});
    let endless = json!({"Items": [big], "NextToken": "more"});
    let each = super::attr::item_size(&item_from_json(&big).unwrap());
    assert!(each > 1000, "{each}");
    let small = browse::Budget {
        bytes: 5 * each - 1,
        cells: browse::Budget::DEFAULT.cells,
    };
    let select = b::parse_command("SELECT * FROM \"t\"").unwrap();
    let out = browse::run_command_within(&canned(vec![endless]), &select, 1000, small, None)
        .await
        .unwrap();
    assert_eq!((out.rows.len(), out.truncated), (5, true));
    assert!(out.notices.iter().any(|m| m.starts_with("Stopped after")));

    let k = KeySchema {
        hash: ("pk".into(), AttrType::S),
        range: None,
    };
    let read = b::read_request(&b::ReadSpec {
        table: "t",
        index: None,
        keys: &k,
        columns: &[],
        filter: None,
        descending: false,
        consistent: false,
        consumed_capacity: false,
    })
    .unwrap();
    let pages = canned(vec![
        json!({"Items": [big], "LastEvaluatedKey": {"pk": {"S": "x"}}}),
    ]);
    let page = browse::read_page(&pages, &read, 100, None, 50, 3 * each - 1, None)
        .await
        .unwrap();
    assert_eq!((page.items.len(), page.requests), (3, 3));
    assert!(page.next.is_some());
}

/// A ListTables page longer than asked for, a name longer than DynamoDB's, or more pages than
/// a full region's are refused: a hostile server cannot make the listing grow without end.
#[tokio::test]
async fn table_listings_stay_within_a_region() {
    use super::catalog::MAX_TABLE_PAGES;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let names: Vec<String> = (0..101).map(|i| format!("t{i}")).collect();
    let long_page = canned(vec![json!({"TableNames": names})]);
    match browse::list_tables(&long_page, None).await {
        Err(e) => assert!(e.to_string().contains("Limit of 100"), "{e}"),
        Ok(n) => panic!("{}", n.len()),
    }
    let long_name = canned(vec![json!({"TableNames": ["x".repeat(256)]})]);
    match browse::list_tables(&long_name, None).await {
        Err(e) => assert!(e.to_string().contains("256 characters"), "{e}"),
        Ok(n) => panic!("{}", n.len()),
    }

    struct Endless(AtomicUsize);
    #[async_trait::async_trait]
    impl super::transport::Transport for Endless {
        async fn send(
            &self,
            _req: super::transport::HttpRequest,
        ) -> Result<super::transport::HttpResponse, AppError> {
            let n = self.0.fetch_add(1, Ordering::SeqCst);
            let name = format!("t{n}");
            Ok(super::transport::HttpResponse::json(
                200,
                &json!({"TableNames": [name], "LastEvaluatedTableName": name}),
            ))
        }
    }
    let endless = Arc::new(Endless(AtomicUsize::new(0)));
    let c = Client::new(endless.clone(), "http://localhost:8000/", "local", local()).unwrap();
    match browse::list_tables(&c, None).await {
        Err(e) => assert!(e.to_string().contains("more than 10000 tables"), "{e}"),
        Ok(n) => panic!("{}", n.len()),
    }
    assert_eq!(endless.0.load(Ordering::SeqCst), MAX_TABLE_PAGES);
}
