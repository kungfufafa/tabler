//! The session against the scripted BigQuery (`testing::FakeBigquery`): connect and sign in
//! (over the wire, through `entry::open`), the sidebar, free and query-backed pages with their
//! cursors, counts, cells, a commit with a failing change, the editor and its BigQuery session,
//! cancel and timeouts, and read-only refusals on every write path.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::client::Client;
use super::entry;
use super::jobs::{DatasetReference, QueryOptions};
use super::params::{BigqueryParams, DEFAULT_MAXIMUM_BYTES_BILLED};
use super::session::{BigquerySession, DRY_RUN, QUERY_BROWSE_LIMIT, SessionConfig};
use super::testing::{
    FakeBigquery, FakeReply, error_reply, job_inserted, results, rows, sa_key_json, schema,
    still_running, test_key,
};
use super::transport::HttpResponse;
use crate::drivers::native::CallKind;
use crate::drivers::registry::UtilInput;
use crate::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, Session, StatementOutcome, TxStatement,
    request_cancel,
};
use crate::error::AppError;
use crate::testkit::{FakeServer, Reply, Route};
use crate::types::admin::KillScope;
use crate::types::browse::{
    BrowseRequest, Conjunction, FilterGroup, FilterNode, FilterOp, RowKey, SortDir, SortSpec,
};
use crate::types::caps::{CountMode, Feature, Paging, SchemaMode, TxMode};
use crate::types::driver::ReadOnlyMode;
use crate::types::objects::{RoutineKind, RoutineRef};
use crate::types::profile::{ConnectionProfile, Driver, DriverParams, PasswordMode};
use crate::types::query::StatementKind;
use crate::types::schema::{TableKind, TableRef};
use crate::types::value::Value;

const BASE: &str = "https://bigquery.example/bigquery/v2";
const P: &str = "/bigquery/v2/projects/demo-proj";
const JOB: &str = "tabler_fake";

fn config(read_only: bool) -> SessionConfig {
    SessionConfig {
        options: QueryOptions {
            location: Some("US".into()),
            default_dataset: Some(DatasetReference {
                project_id: "demo-proj".into(),
                dataset_id: "shop".into(),
            }),
            maximum_bytes_billed: Some(1 << 30),
            use_query_cache: true,
            ..QueryOptions::default()
        },
        extra_projects: vec!["bigquery-public-data".into()],
        read_only,
        user: Some("tabler@demo-proj.iam.gserviceaccount.com".into()),
    }
}

fn session(fake: &FakeBigquery, read_only: bool) -> BigquerySession {
    let client = Client::new(fake.transport(), BASE, "demo-proj");
    BigquerySession::new(Arc::new(client), config(read_only))
}

fn path(rest: &str) -> String {
    format!("{P}{rest}")
}

/// `shop.orders`: an INT64 key, a NUMERIC, a TIMESTAMP and a repeated RECORD; 3 rows.
fn orders_table(kind: &str) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({
            "tableReference": {"projectId": "demo-proj", "datasetId": "shop", "tableId": "orders"},
            "type": kind,
            "schema": {"fields": [
                {"name": "id", "type": "INTEGER", "mode": "REQUIRED"},
                {"name": "total", "type": "NUMERIC"},
                {"name": "at", "type": "TIMESTAMP"},
                {"name": "items", "type": "RECORD", "mode": "REPEATED", "fields": [
                    {"name": "sku", "type": "STRING"}, {"name": "qty", "type": "INTEGER"}]}
            ]},
            "numRows": "3",
            "tableConstraints": {"primaryKey": {"columns": ["id"]}},
            "view": if kind == "VIEW" { json!({"query": "SELECT 1 AS id"}) } else { json!(null) }
        }),
    )
}

fn order_row(id: &str, total: &str) -> Vec<serde_json::Value> {
    vec![
        json!(id),
        json!(total),
        json!("1704164645000000"),
        json!([{"v": {"f": [{"v": "a"}, {"v": "2"}]}}]),
    ]
}

fn data_page(cells: &[Vec<serde_json::Value>], token: Option<&str>) -> HttpResponse {
    let mut v = json!({"kind": "bigquery#tableDataList", "totalRows": "3", "rows": rows(cells)});
    if let Some(t) = token {
        v["pageToken"] = t.into();
    }
    HttpResponse::json(200, &v)
}

fn inserted() -> HttpResponse {
    job_inserted("demo-proj", JOB, "US")
}

/// A `jobs.insert` reply of a job that runs in BigQuery session `id`.
fn inserted_in(id: &str) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({"jobReference": {"projectId": "demo-proj", "jobId": JOB, "location": "US"},
            "status": {"state": "RUNNING"},
            "statistics": {"sessionInfo": {"sessionId": id}}}),
    )
}

fn dml_done(n: u64) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({"jobComplete": true, "numDmlAffectedRows": n.to_string(),
            "jobReference": {"projectId": "demo-proj", "jobId": JOB, "location": "US"}}),
    )
}

fn empty_done() -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({"jobComplete": true,
            "jobReference": {"projectId": "demo-proj", "jobId": JOB, "location": "US"}}),
    )
}

fn failed_insert(reason: &str, message: &str) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({"jobReference": {"projectId": "demo-proj", "jobId": JOB, "location": "US"},
            "status": {"state": "DONE", "errorResult": {"reason": reason, "message": message},
                "errors": [{"reason": reason, "message": message}]}}),
    )
}

fn inserts(fake: &FakeBigquery) -> Vec<serde_json::Value> {
    fake.requests()
        .into_iter()
        .filter(|r| r.path() == path("/jobs") && r.method == super::transport::Method::Post)
        .map(|r| r.json())
        .collect()
}

fn job_sql(body: &serde_json::Value) -> String {
    body["configuration"]["query"]["query"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn request(columns: Option<Vec<String>>) -> BrowseRequest {
    BrowseRequest {
        table: TableRef::new(Some("shop"), "orders"),
        columns,
        filter: None,
        sort: Vec::new(),
        limit: 2,
        offset: 0,
        cursor: None,
    }
}

fn eq_filter(column: &str, v: Value) -> FilterGroup {
    FilterGroup {
        conjunction: Conjunction::And,
        items: vec![FilterNode::Condition {
            column: column.into(),
            op: FilterOp::Eq,
            values: vec![v],
        }],
    }
}

#[tokio::test]
async fn server_info_ping_and_caps() {
    let fake = FakeBigquery::new();
    fake.on(
        "GET",
        &path("/datasets"),
        HttpResponse::json(200, &json!({"datasets": []})),
    );
    let s = session(&fake, false);
    let info = s.server_info().await.unwrap();
    assert_eq!(info.version, "BigQuery");
    assert_eq!(info.current_database.as_deref(), Some("demo-proj"));
    assert_eq!(
        info.current_user.as_deref(),
        Some("tabler@demo-proj.iam.gserviceaccount.com")
    );
    // A ping is one free metadata call, no job.
    s.ping().await.unwrap();
    let sent = fake.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].param("maxResults").as_deref(), Some("1"));

    let caps = s.capabilities();
    assert_eq!(
        caps.features,
        [
            Feature::Editor,
            Feature::CancelOnServer,
            Feature::ExactCount,
            Feature::StructureView,
            Feature::TableDdl,
            Feature::ViewDefinition,
            Feature::Routines,
            Feature::ProcessList,
            Feature::KillProcess,
        ]
    );
    for off in [
        Feature::EditRows,
        Feature::InsertRows,
        Feature::DeleteRows,
        Feature::Explain,
        Feature::SwitchDatabase,
        Feature::ExportTables,
        Feature::Mcp,
    ] {
        assert!(!caps.has(off), "{off:?}");
    }
    assert_eq!(caps.data.paging, Paging::Cursor);
    assert_eq!(caps.data.count, CountMode::Costly);
    assert_eq!(caps.data.transactions, TxMode::Atomic);
    assert_eq!(caps.namespaces.schemas, SchemaMode::Schemas);
    assert_eq!(caps.namespaces.default_schema.as_deref(), Some("shop"));
    assert_eq!(caps.namespaces.labels.schema, "Dataset");
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    assert_eq!(caps.session.keepalive_secs, None);
    assert!(!caps.session.ping_billed);
    assert!(s.table_source().is_some());

    // Every operator the caps offer builds.
    let meta = {
        fake.on(
            "GET",
            &path("/datasets/shop/tables/orders"),
            orders_table("TABLE"),
        );
        s.table_meta(&TableRef::new(Some("shop"), "orders"))
            .await
            .unwrap()
    };
    for op in caps.data.filter_ops {
        let values = match op {
            FilterOp::IsNull | FilterOp::IsNotNull => vec![],
            FilterOp::Between | FilterOp::NotBetween => vec![Value::Int(1), Value::Int(2)],
            _ => vec![Value::Int(1)],
        };
        let filter = FilterGroup {
            conjunction: Conjunction::And,
            items: vec![FilterNode::Condition {
                column: "total".into(),
                op,
                values,
            }],
        };
        crate::sqlgen::sql::count(
            super::session::DIALECT,
            &meta.table,
            &meta.columns,
            Some(&filter),
        )
        .unwrap_or_else(|e| panic!("{op:?}: {e}"));
    }
}

async fn orders_meta(fake: &FakeBigquery, s: &BigquerySession) -> crate::types::schema::TableMeta {
    fake.on(
        "GET",
        &path("/datasets/shop/tables/orders"),
        orders_table("TABLE"),
    );
    s.table_meta(&TableRef::new(Some("shop"), "orders"))
        .await
        .unwrap()
}

#[tokio::test]
async fn the_sidebar_lists_projects_datasets_tables_and_structure() {
    let fake = FakeBigquery::new();
    fake.on(
        "GET",
        "/bigquery/v2/projects",
        HttpResponse::json(
            200,
            &json!({"projects": [{"projectReference": {"projectId": "other"}}]}),
        ),
    );
    fake.on(
        "GET",
        &path("/datasets"),
        HttpResponse::json(
            200,
            &json!({"datasets": [
                {"datasetReference": {"projectId": "demo-proj", "datasetId": "shop"}},
                {"datasetReference": {"projectId": "demo-proj", "datasetId": "logs"}}]}),
        ),
    );
    fake.on(
        "GET",
        &path("/datasets/shop/tables"),
        HttpResponse::json(
            200,
            &json!({"tables": [
                {"tableReference": {"projectId": "demo-proj", "datasetId": "shop", "tableId": "orders"}, "type": "TABLE"},
                {"tableReference": {"projectId": "demo-proj", "datasetId": "shop", "tableId": "v"}, "type": "VIEW"}]}),
        ),
    );
    fake.on(
        "GET",
        &path("/datasets/logs/tables"),
        HttpResponse::json(200, &json!({})),
    );
    fake.on(
        "GET",
        &path("/datasets/shop/tables/orders"),
        orders_table("TABLE"),
    );
    fake.on(
        "GET",
        &path("/datasets/shop/tables/v"),
        orders_table("VIEW"),
    );
    let s = session(&fake, false);
    let dbs: Vec<String> = s
        .list_databases()
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(dbs, ["demo-proj", "bigquery-public-data", "other"]);
    let schemas = s.list_schemas().await.unwrap();
    assert_eq!(schemas[0].name, "logs");
    assert!(schemas[1].is_default);
    let tables = s.list_tables("shop").await.unwrap();
    assert_eq!(tables[1].kind, TableKind::View);
    let meta = s.table_meta(&TableRef::new(None, "orders")).await.unwrap();
    assert_eq!(meta.primary_key, ["id"]);
    assert_eq!(meta.row_estimate, Some(3));
    assert_eq!(meta.columns.len(), 4);
    assert_eq!(
        s.view_definition(&TableRef::new(Some("shop"), "v"))
            .await
            .unwrap()
            .as_deref(),
        Some("SELECT 1 AS id")
    );
    // Completion from the free calls only: the default dataset's columns, the others' tables.
    let c = s.completion_metadata().await.unwrap();
    assert_eq!(c.default_schema.as_deref(), Some("shop"));
    assert_eq!(c.schemas["shop"]["orders"], ["id", "total", "at", "items"]);
    assert!(c.schemas["logs"].is_empty());
    assert!(inserts(&fake).is_empty(), "nothing billed");
}

#[tokio::test]
async fn a_plain_page_is_read_for_free_and_the_next_by_its_cursor() {
    let fake = FakeBigquery::new();
    let s = session(&fake, false);
    let meta = orders_meta(&fake, &s).await;
    fake.on_seq(
        "GET",
        &path("/datasets/shop/tables/orders/data"),
        vec![
            data_page(
                &[
                    order_row("9007199254740993", "12.50"),
                    order_row("2", "0.000000001"),
                ],
                Some("p2"),
            ),
            data_page(&[order_row("3", "1")], None),
        ],
    );
    let source = s.table_source().unwrap();
    let first = source
        .browse(&meta, &request(None), CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(first.rows.len(), 2);
    // 2^53 + 1 stays exact, NUMERIC is its text, a TIMESTAMP is text, nested values are JSON.
    assert_eq!(first.rows[0][0], Value::BigInt("9007199254740993".into()));
    assert_eq!(first.rows[0][1], Value::Decimal("12.50".into()));
    assert_eq!(
        first.rows[0][2],
        Value::TimestampTz("2024-01-02 03:04:05+00".into())
    );
    assert!(
        matches!(&first.rows[0][3], Value::Json(j) if j.contains("\"sku\":\"a\"")),
        "{:?}",
        first.rows[0][3]
    );
    assert_eq!(first.rows[1][0], Value::Int(2));
    assert!(first.sql.contains("tabledata.list"), "{}", first.sql);
    let cursor = first.next_cursor.clone().expect("a next page");
    let mut next = request(None);
    next.cursor = Some(cursor.clone());
    let second = source
        .browse(&meta, &next, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(
        second.rows,
        vec![vec![
            Value::Int(3),
            Value::Decimal("1".into()),
            Value::TimestampTz("2024-01-02 03:04:05+00".into()),
            second.rows[0][3].clone(),
        ]]
    );
    assert_eq!(second.next_cursor, None);
    let reads = fake.requests_to("/data");
    assert_eq!(reads[0].param("maxResults").as_deref(), Some("2"));
    assert_eq!(reads[0].param("pageToken"), None);
    assert_eq!(reads[1].param("pageToken").as_deref(), Some("p2"));
    assert_eq!(
        reads[0].param("formatOptions.useInt64Timestamp").as_deref(),
        Some("true")
    );
    assert!(inserts(&fake).is_empty(), "no job, nothing billed");

    // Columns asked for, in their order.
    let picked = source
        .browse(
            &meta,
            &request(Some(vec!["total".into(), "id".into()])),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(picked.columns[0].name, "total");
    assert_eq!(picked.rows[0][1], Value::Int(3));

    // A cursor of another page, and an offset without a cursor, are refused.
    let mut sorted = request(None);
    sorted.sort = vec![SortSpec {
        column: "id".into(),
        dir: SortDir::Desc,
        nulls: None,
    }];
    sorted.cursor = Some(cursor);
    let e = source
        .browse(&meta, &sorted, CancelSlot::default())
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::InvalidInput(_)), "{e:?}");
    let mut skip = request(None);
    skip.offset = 10;
    assert!(
        source
            .browse(&meta, &skip, CancelSlot::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_sorted_or_filtered_page_is_one_job_whose_pages_follow_for_free() {
    let fake = FakeBigquery::new();
    let s = session(&fake, false);
    let meta = orders_meta(&fake, &s).await;
    fake.on("POST", &path("/jobs"), inserted());
    let cols = schema(&[("id", "INTEGER"), ("total", "NUMERIC")]);
    fake.on_seq(
        "GET",
        &path("/queries/*"),
        vec![
            results(
                cols.clone(),
                rows(&[vec![json!("9"), json!("1.5")], vec![json!("8"), json!("2")]]),
                3,
                Some("q2"),
            ),
            results(cols, rows(&[vec![json!("7"), json!(null)]]), 3, None),
        ],
    );
    let mut req = request(Some(vec!["id".into(), "total".into()]));
    req.sort = vec![SortSpec {
        column: "id".into(),
        dir: SortDir::Desc,
        nulls: None,
    }];
    req.filter = Some(eq_filter("total", Value::Decimal("1.5".into())));
    let source = s.table_source().unwrap();
    let first = source
        .browse(&meta, &req, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(first.rows[0][0], Value::Int(9));
    let jobs = inserts(&fake);
    assert_eq!(jobs.len(), 1);
    let sql = job_sql(&jobs[0]);
    assert!(sql.contains("FROM `shop`.`orders`"), "{sql}");
    assert!(sql.contains("ORDER BY `id` DESC"), "{sql}");
    assert!(
        sql.contains(&format!("LIMIT {QUERY_BROWSE_LIMIT}")),
        "{sql}"
    );
    let q = &jobs[0]["configuration"]["query"];
    assert_eq!(q["maximumBytesBilled"], "1073741824");
    assert_eq!(q["queryParameters"][0]["parameterType"]["type"], "NUMERIC");
    assert!(
        first.sql.contains("1.5"),
        "the shown SQL inlines the values"
    );
    let polls = fake.requests_to(&format!("/queries/{JOB}"));
    assert_eq!(polls[0].param("maxResults").as_deref(), Some("2"));

    req.cursor = first.next_cursor.clone();
    let second = source
        .browse(&meta, &req, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(second.rows, vec![vec![Value::Int(7), Value::Null]]);
    assert_eq!(second.next_cursor, None);
    assert_eq!(inserts(&fake).len(), 1, "the next page bills nothing");
    let polls = fake.requests_to(&format!("/queries/{JOB}"));
    assert_eq!(
        polls.last().unwrap().param("pageToken").as_deref(),
        Some("q2")
    );

    // The same cursor under another filter is refused.
    req.filter = Some(eq_filter("total", Value::Int(2)));
    let e = source
        .browse(&meta, &req, CancelSlot::default())
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::InvalidInput(_)), "{e:?}");

    // A view has no table data to list: its pages are a query too.
    let fake = FakeBigquery::new();
    let s = session(&fake, false);
    fake.on(
        "GET",
        &path("/datasets/shop/tables/orders"),
        orders_table("VIEW"),
    );
    fake.on("POST", &path("/jobs"), inserted());
    fake.on(
        "GET",
        &path("/queries/*"),
        results(
            schema(&[("id", "INTEGER")]),
            rows(&[vec![json!("1")]]),
            1,
            None,
        ),
    );
    let meta = s
        .table_meta(&TableRef::new(Some("shop"), "orders"))
        .await
        .unwrap();
    assert_eq!(meta.kind, TableKind::View);
    s.table_source()
        .unwrap()
        .browse(
            &meta,
            &request(Some(vec!["id".into()])),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(inserts(&fake).len(), 1);
}

#[tokio::test]
async fn counts_and_cells() {
    let fake = FakeBigquery::new();
    let s = session(&fake, false);
    let meta = orders_meta(&fake, &s).await;
    let source = s.table_source().unwrap();
    // Unfiltered and not asked to count: the table's numRows, no job.
    let n = source
        .count(&meta, None, false, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((n.count, n.exact), (3, false));
    assert!(inserts(&fake).is_empty());
    fake.on("POST", &path("/jobs"), inserted());
    fake.on(
        "GET",
        &path("/queries/*"),
        results(
            schema(&[("c", "INTEGER")]),
            rows(&[vec![json!("2")]]),
            1,
            None,
        ),
    );
    let filter = eq_filter("id", Value::Int(1));
    let n = source
        .count(&meta, Some(&filter), false, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((n.count, n.exact), (2, true));
    let sql = job_sql(&inserts(&fake)[0]);
    assert!(sql.starts_with("SELECT COUNT(*)"), "{sql}");
    assert!(sql.contains("@p1"), "{sql}");

    // A whole cell, by the row's key.
    let fake = FakeBigquery::new();
    let s = session(&fake, false);
    let meta = orders_meta(&fake, &s).await;
    fake.on("POST", &path("/jobs"), inserted());
    fake.on(
        "GET",
        &path("/queries/*"),
        results(
            schema(&[("total", "NUMERIC")]),
            rows(&[vec![json!("99.5")]]),
            1,
            None,
        ),
    );
    let key = RowKey::Pk([("id".to_string(), Value::Int(7))].into_iter().collect());
    let v = s
        .table_source()
        .unwrap()
        .fetch_cell(&meta, &key, "total")
        .await
        .unwrap();
    assert_eq!(v, Value::Decimal("99.5".into()));
    let job = &inserts(&fake)[0];
    assert!(job_sql(job).contains("`id` = @p1"), "{}", job_sql(job));
    assert_eq!(
        job["configuration"]["query"]["queryParameters"][0]["parameterValue"]["value"],
        "7"
    );
    let e = s
        .table_source()
        .unwrap()
        .fetch_cell(&meta, &key, "nope")
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::InvalidInput(_)), "{e:?}");
}

fn tx(sql: &str, expect: Option<u64>) -> TxStatement {
    let st = TxStatement::new(sql, vec![Value::Int(1)]);
    match expect {
        Some(n) => st.expecting(n),
        None => st,
    }
}

#[tokio::test]
async fn a_commit_is_one_transaction_and_a_failing_change_rolls_it_back() {
    let fake = FakeBigquery::new();
    fake.on_seq(
        "POST",
        &path("/jobs"),
        vec![
            inserted_in("s-1"),
            inserted(),
            failed_insert("invalidQuery", "Table shop.nope was not found"),
            inserted(),
            inserted(),
        ],
    );
    fake.on_seq(
        "GET",
        &path("/queries/*"),
        vec![empty_done(), dml_done(1), empty_done()],
    );
    let s = session(&fake, false);
    let e = s
        .transaction(vec![
            tx("UPDATE shop.orders SET total = 1 WHERE id = @p1", Some(1)),
            tx("UPDATE shop.nope SET x = 1 WHERE id = @p1", Some(1)),
        ])
        .await
        .unwrap_err();
    assert!(
        matches!(&e, AppError::Db { statement_index: Some(1), message, .. } if message.contains("nope")),
        "{e:?}"
    );
    let sent: Vec<String> = inserts(&fake).iter().map(job_sql).collect();
    assert_eq!(
        sent,
        [
            "BEGIN TRANSACTION",
            "UPDATE shop.orders SET total = 1 WHERE id = @p1",
            "UPDATE shop.nope SET x = 1 WHERE id = @p1",
            "ROLLBACK TRANSACTION",
            "CALL BQ.ABORT_SESSION()",
        ]
    );
    let jobs = inserts(&fake);
    assert_eq!(jobs[0]["configuration"]["query"]["createSession"], true);
    for job in &jobs[1..] {
        assert_eq!(
            job["configuration"]["query"]["connectionProperties"][0]["value"],
            "s-1"
        );
    }

    // A change that matched another number of rows than staged rolls back too.
    let fake = FakeBigquery::new();
    fake.on_seq("POST", &path("/jobs"), vec![inserted_in("s-2"), inserted()]);
    fake.on_seq("GET", &path("/queries/*"), vec![empty_done(), dml_done(2)]);
    let s = session(&fake, false);
    let e = s
        .transaction(vec![tx("DELETE FROM shop.orders WHERE id = @p1", Some(1))])
        .await
        .unwrap_err();
    assert!(
        matches!(
            &e,
            AppError::Db {
                statement_index: Some(0),
                ..
            }
        ),
        "{e:?}"
    );
    let sent: Vec<String> = inserts(&fake).iter().map(job_sql).collect();
    assert_eq!(
        sent.last().map(String::as_str),
        Some("CALL BQ.ABORT_SESSION()")
    );
    assert!(!sent.iter().any(|s| s == "COMMIT TRANSACTION"));

    // All good: committed, each count exact.
    let fake = FakeBigquery::new();
    fake.on_seq("POST", &path("/jobs"), vec![inserted_in("s-3"), inserted()]);
    fake.on_seq(
        "GET",
        &path("/queries/*"),
        vec![empty_done(), dml_done(1), empty_done()],
    );
    let s = session(&fake, false);
    let out = s
        .transaction(vec![tx("DELETE FROM shop.orders WHERE id = @p1", Some(1))])
        .await
        .unwrap();
    assert_eq!(out[0].affected, 1);
    assert!(!out[0].affected_unknown);
    let sent: Vec<String> = inserts(&fake).iter().map(job_sql).collect();
    assert_eq!(sent.last().map(String::as_str), Some("COMMIT TRANSACTION"));
}

#[tokio::test(start_paused = true)]
async fn the_editor_runs_in_one_bigquery_session_with_a_row_cap() {
    let fake = FakeBigquery::new();
    fake.on_seq("POST", &path("/jobs"), vec![inserted(), inserted()]);
    // The insert's reply had no statistics yet: jobs.get names the session.
    fake.on(
        "GET",
        &path(&format!("/jobs/{JOB}")),
        HttpResponse::json(
            200,
            &json!({"jobReference": {"projectId": "demo-proj", "jobId": JOB},
                "status": {"state": "DONE"}, "statistics": {"sessionInfo": {"sessionId": "ed-1"}}}),
        ),
    );
    let mut page = results(
        schema(&[("n", "INTEGER")]),
        rows(&[vec![json!("1")], vec![json!("2")], vec![json!("3")]]),
        3,
        None,
    );
    let mut v: serde_json::Value = serde_json::from_slice(&page.body).unwrap();
    v["totalBytesBilled"] = "10485760".into();
    page = HttpResponse::json(200, &v);
    fake.on_seq(
        "GET",
        &path("/queries/*"),
        vec![still_running("demo-proj", JOB), page, dml_done(4)],
    );
    let s = session(&fake, false);
    let mut sink = CollectSink::default();
    let out = s
        .run_statement("SELECT n FROM t", 2, &mut sink, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(out.kind, StatementKind::Rows);
    assert_eq!((out.row_count, out.truncated), (2, true));
    assert_eq!(sink.sets[0].rows.len(), 2);
    assert_eq!(out.notices, ["BigQuery processed 1.0 KB, billed 10.0 MB"]);
    let out = s
        .run_statement(
            "UPDATE t SET n = 1 WHERE TRUE",
            10,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!((out.kind, out.affected), (StatementKind::Affected, Some(4)));
    let jobs = inserts(&fake);
    assert_eq!(jobs[0]["configuration"]["query"]["createSession"], true);
    assert_eq!(
        jobs[1]["configuration"]["query"]["connectionProperties"][0]["value"],
        "ed-1"
    );
    // The editor's statements are the renderer's to log.
    assert!(s.drain_trace().is_empty());
    assert!(s.editor_run_lock().is_some());
    assert_eq!(s.in_transaction(), None);
}

/// A finished job that failed (`jobs.getQueryResults`).
fn failed_done(reason: &str, message: &str) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({"jobComplete": true,
            "jobReference": {"projectId": "demo-proj", "jobId": JOB, "location": "US"},
            "errors": [{"reason": reason, "message": message}]}),
    )
}

async fn run_editor(s: &BigquerySession, sql: &str) -> Result<StatementOutcome, AppError> {
    s.run_statement(sql, 10, &mut CollectSink::default(), CancelSlot::default())
        .await
}

/// An export (no row cap) stops when the server sends the page token it was just asked for.
#[tokio::test]
async fn a_repeated_result_page_token_ends_the_read() {
    let fake = FakeBigquery::new();
    fake.on("POST", &path("/jobs"), inserted());
    let page = results(
        schema(&[("n", "INTEGER")]),
        rows(&[vec![json!("1")]]),
        3,
        Some("again"),
    );
    fake.on("GET", &path("/queries/*"), page);
    let s = session(&fake, false);
    let mut sink = CollectSink::default();
    let e = s
        .stream_query("SELECT n FROM t", &[], &mut sink, CancelSlot::default())
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::Internal(_)), "{e:?}");
    // The first page, then the one that named "again" and sent it back.
    let polls = fake
        .requests()
        .into_iter()
        .filter(|r| r.path().starts_with(&path("/queries/")))
        .count();
    assert_eq!(polls, 2);
}

#[tokio::test]
async fn an_expired_editor_session_is_replaced_once_for_a_read() {
    let fake = FakeBigquery::new();
    fake.on_seq(
        "POST",
        &path("/jobs"),
        vec![
            inserted_in("old"),
            failed_insert("invalidQuery", "Session old has expired"),
            inserted_in("new"),
        ],
    );
    fake.on("GET", &path("/queries/*"), empty_done());
    let s = session(&fake, false);
    for _ in 0..2 {
        run_editor(&s, "SELECT 1").await.unwrap();
    }
    let jobs = inserts(&fake);
    assert_eq!(jobs.len(), 3);
    assert_eq!(
        jobs[1]["configuration"]["query"]["connectionProperties"][0]["value"],
        "old"
    );
    assert_eq!(jobs[2]["configuration"]["query"]["createSession"], true);
}

#[tokio::test]
async fn a_write_refused_for_an_expired_session_is_not_sent_again() {
    let fake = FakeBigquery::new();
    fake.on_seq(
        "POST",
        &path("/jobs"),
        vec![
            inserted_in("old"),
            failed_insert("invalidQuery", "Session old has expired"),
            inserted_in("new"),
        ],
    );
    fake.on("GET", &path("/queries/*"), empty_done());
    let s = session(&fake, false);
    run_editor(&s, "SELECT 1").await.unwrap();
    let e = run_editor(&s, "INSERT INTO shop.log VALUES (1)")
        .await
        .unwrap_err();
    let AppError::Db { hint, .. } = &e else {
        panic!("{e:?}");
    };
    assert!(hint.as_deref().unwrap_or_default().contains("did not run"));
    assert_eq!(inserts(&fake).len(), 2);
    // The user's next run starts a new session.
    run_editor(&s, "INSERT INTO shop.log VALUES (1)")
        .await
        .unwrap();
    let jobs = inserts(&fake);
    assert_eq!(jobs.len(), 3);
    assert_eq!(jobs[2]["configuration"]["query"]["createSession"], true);
}

/// A script that failed after its job started (its INSERT has run) is never sent again, even
/// when the error's text holds "session" and "not found" (a table's name).
#[tokio::test]
async fn a_statement_that_failed_after_it_started_is_never_sent_again() {
    let script = "INSERT INTO shop.log VALUES (1); SELECT * FROM shop.user_sessions";
    let fake = FakeBigquery::new();
    fake.on_seq(
        "POST",
        &path("/jobs"),
        vec![inserted_in("ed"), inserted(), inserted()],
    );
    fake.on_seq(
        "GET",
        &path("/queries/*"),
        vec![
            empty_done(),
            failed_done(
                "notFound",
                "Not found: Table demo-proj:shop.user_sessions was not found in location US",
            ),
            empty_done(),
        ],
    );
    let s = session(&fake, false);
    run_editor(&s, "SELECT 1").await.unwrap();
    let e = run_editor(&s, script).await.unwrap_err();
    assert!(e.to_string().contains("user_sessions"), "{e:?}");
    assert_eq!(inserts(&fake).len(), 2);
    // Nor at insert, when the refusal does not name the editor's session.
    let fake = FakeBigquery::new();
    fake.on_seq(
        "POST",
        &path("/jobs"),
        vec![
            inserted_in("CgwKCmRlbW8tcHJvahAB"),
            failed_insert(
                "notFound",
                "Not found: Table shop.user_sessions was not found",
            ),
            failed_insert("invalidQuery", "session expired"),
            inserted(),
        ],
    );
    fake.on("GET", &path("/queries/*"), empty_done());
    let s = session(&fake, false);
    run_editor(&s, "SELECT 1").await.unwrap();
    run_editor(&s, "SELECT * FROM shop.user_sessions")
        .await
        .unwrap_err();
    run_editor(&s, "SELECT ERROR('session expired')")
        .await
        .unwrap_err();
    let jobs = inserts(&fake);
    assert_eq!(jobs.len(), 3);
    // The session was kept: both runs named it.
    for j in &jobs[1..] {
        assert_eq!(
            j["configuration"]["query"]["connectionProperties"][0]["value"],
            "CgwKCmRlbW8tcHJvahAB"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_cancel_stops_the_run_and_the_job_on_the_server() {
    let fake = FakeBigquery::new();
    fake.on("POST", &path("/jobs"), inserted_in("ed"));
    fake.on_replies(
        "GET",
        &path("/queries/*"),
        vec![FakeReply::after(
            still_running("demo-proj", JOB),
            Duration::from_secs(10),
        )],
    );
    fake.on(
        "POST",
        &path("/jobs/*"),
        HttpResponse::json(200, &json!({"job": {"status": {"state": "RUNNING"}}})),
    );
    let s = Arc::new(session(&fake, false));
    let slot = CancelSlot::default();
    let run = {
        let (s, slot) = (Arc::clone(&s), slot.clone());
        tokio::spawn(async move {
            let mut sink = CollectSink::default();
            s.run_statement("SELECT * FROM big", 100, &mut sink, slot)
                .await
        })
    };
    tokio::time::sleep(Duration::from_secs(25)).await;
    assert!(!run.is_finished(), "a long job keeps the run waiting");
    let handle = request_cancel(&slot).expect("the run armed its cancel");
    s.cancel(handle).await.unwrap();
    let ended = run.await.unwrap();
    assert!(matches!(ended, Err(AppError::Cancelled)), "{ended:?}");
    let cancels = fake.requests_to("/cancel");
    assert!(!cancels.is_empty());
    // The handle names the id Tabler gave the job; the run cancels it again by the id BigQuery
    // answered with.
    assert!(
        cancels
            .iter()
            .any(|c| c.path().ends_with(&format!("/jobs/{JOB}/cancel")))
    );
    assert!(
        cancels
            .iter()
            .all(|c| c.param("location").as_deref() == Some("US"))
    );
}

#[tokio::test]
async fn a_job_stopped_by_its_timeout_is_a_timeout() {
    let fake = FakeBigquery::new();
    fake.on("POST", &path("/jobs"), inserted());
    fake.on(
        "GET",
        &path("/queries/*"),
        HttpResponse::json(
            200,
            &json!({"jobComplete": true, "errors": [{"reason": "timeout",
                "message": "Job execution was cancelled: Job timed out after 1 sec"}]}),
        ),
    );
    let s = session(&fake, false);
    let e = s.query("SELECT 1", &[]).await.unwrap_err();
    assert!(matches!(e, AppError::Timeout), "{e:?}");
    // The app's own reads are traced, with the error.
    let traced = s.drain_trace();
    assert_eq!(traced.len(), 1);
    assert!(traced[0].error.is_some());
}

#[tokio::test]
async fn reads_collect_every_page_and_exports_stream_them() {
    let fake = FakeBigquery::new();
    fake.on("POST", &path("/jobs"), inserted());
    let cols = schema(&[("n", "INTEGER")]);
    fake.on_seq(
        "GET",
        &path("/queries/*"),
        vec![
            results(cols.clone(), rows(&[vec![json!("1")]]), 2, Some("t2")),
            results(cols.clone(), rows(&[vec![json!("2")]]), 2, None),
            results(cols.clone(), rows(&[vec![json!("1")]]), 2, Some("t2")),
            results(cols, rows(&[vec![json!("2")]]), 2, None),
        ],
    );
    let s = session(&fake, false);
    let rs = s
        .query("SELECT n FROM t WHERE n > @p1", &[Value::Int(0)])
        .await
        .unwrap();
    assert_eq!(rs.rows, vec![vec![Value::Int(1)], vec![Value::Int(2)]]);
    assert!(!rs.truncated);
    let mut sink = CollectSink::default();
    let n = s
        .stream_query("SELECT n FROM t", &[], &mut sink, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(n, 2);
    let traced = s.drain_trace();
    assert_eq!(traced.len(), 2);
    assert_eq!(traced[0].sql, "SELECT n FROM t WHERE n > 0");
    assert_eq!(traced[0].rows, Some(2));
}

#[tokio::test]
async fn read_only_sessions_refuse_every_write_before_sending_it() {
    let fake = FakeBigquery::new();
    let s = session(&fake, true);
    let ro = |r: Result<(), AppError>| {
        assert!(
            matches!(&r, Err(AppError::InvalidInput(m)) if m.contains("read-only")),
            "{r:?}"
        );
    };
    for sql in [
        "DELETE FROM shop.orders WHERE TRUE",
        "SELECT 1; DROP TABLE shop.orders",
        "EXECUTE IMMEDIATE 'DELETE FROM t WHERE TRUE'",
        "CALL shop.p()",
        "SELECT * FROM EXTERNAL_QUERY('c', 'DELETE FROM t')",
        "MERGE shop.t USING s ON FALSE WHEN NOT MATCHED THEN INSERT ROW",
        "BEGIN TRANSACTION",
        "frobnicate",
    ] {
        ro(
            s.run_statement(sql, 10, &mut CollectSink::default(), CancelSlot::default())
                .await
                .map(|_| ()),
        );
        ro(s.query(sql, &[]).await.map(|_| ()));
        ro(s.execute(sql, &[]).await.map(|_| ()));
        ro(s.execute_cancellable(sql, &[], CancelSlot::default())
            .await
            .map(|_| ()));
    }
    ro(s.stream_query(
        "DELETE FROM t WHERE TRUE",
        &[],
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .map(|_| ()));
    ro(
        s.transaction(vec![tx("DELETE FROM t WHERE id = @p1", Some(1))])
            .await
            .map(|_| ()),
    );
    let killed = s
        .kill_processes(&["demo-proj:US.tabler_1".into()], KillScope::Query)
        .await
        .unwrap();
    assert!(!killed[0].ok);
    assert!(
        killed[0]
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("read-only")
    );
    assert!(fake.requests().is_empty(), "nothing reached BigQuery");

    // Reads still run; a dry run runs nothing whatever the statement.
    assert_eq!(s.call_kind(DRY_RUN), CallKind::Read);
    assert_eq!(s.call_kind("anything"), CallKind::Write);
}

#[tokio::test]
async fn a_filter_that_would_run_another_statement_is_refused_on_any_session() {
    let fake = FakeBigquery::new();
    let s = session(&fake, false);
    let meta = orders_meta(&fake, &s).await;
    let before = fake.requests().len();
    let mut req = request(None);
    req.filter = Some(FilterGroup {
        conjunction: Conjunction::And,
        items: vec![FilterNode::Raw {
            sql: "TRUE); DELETE FROM shop.orders WHERE (TRUE".into(),
        }],
    });
    let e = s
        .table_source()
        .unwrap()
        .browse(&meta, &req, CancelSlot::default())
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::InvalidInput(_)), "{e:?}");
    let e = s
        .table_source()
        .unwrap()
        .count(&meta, req.filter.as_ref(), true, CancelSlot::default())
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::InvalidInput(_)), "{e:?}");
    assert_eq!(fake.requests().len(), before);
}

#[tokio::test]
async fn processes_kills_ddl_routines_and_dry_runs() {
    let fake = FakeBigquery::new();
    fake.on(
        "GET",
        &path("/jobs"),
        HttpResponse::json(
            200,
            &json!({"jobs": [{"jobReference": {"projectId": "demo-proj", "jobId": "tabler_1", "location": "EU"},
                "state": "RUNNING", "user_email": "a@b",
                "configuration": {"query": {"query": "SELECT 1"}},
                "statistics": {"creationTime": "1704164645000"}}]}),
        ),
    );
    fake.on(
        "POST",
        &path("/jobs/*"),
        HttpResponse::json(200, &json!({"job": {}})),
    );
    let s = session(&fake, false);
    let procs = s.list_processes().await.unwrap().unwrap();
    assert_eq!(procs[0].id, "demo-proj:EU.tabler_1");
    assert!(procs[0].own);
    let out = s
        .kill_processes(&[procs[0].id.clone(), "junk".into()], KillScope::Connection)
        .await
        .unwrap();
    assert!(out[0].ok, "{out:?}");
    assert!(!out[1].ok);
    let cancel = &fake.requests_to("/cancel")[0];
    assert!(cancel.path().ends_with("/jobs/tabler_1/cancel"));
    assert_eq!(cancel.param("location").as_deref(), Some("EU"));

    // Show DDL and a routine's definition: INFORMATION_SCHEMA by name, on demand.
    fake.on("POST", &path("/jobs"), inserted());
    fake.on(
        "GET",
        &path("/queries/*"),
        results(
            schema(&[("ddl", "STRING")]),
            rows(&[vec![json!(
                "CREATE TABLE `demo-proj.shop.orders` (id INT64)"
            )]]),
            1,
            None,
        ),
    );
    let ddl = s
        .table_ddl(&TableRef::new(Some("shop"), "orders"))
        .await
        .unwrap()
        .unwrap();
    assert!(ddl.starts_with("CREATE TABLE"), "{ddl}");
    let job = inserts(&fake).pop().unwrap();
    assert_eq!(
        job_sql(&job),
        "SELECT ddl FROM `demo-proj`.`shop`.INFORMATION_SCHEMA.TABLES WHERE table_name = @name"
    );
    let p = &job["configuration"]["query"]["queryParameters"][0];
    assert_eq!(p["name"], "name");
    assert_eq!(p["parameterValue"]["value"], "orders");
    let def = s
        .routine_definition(&RoutineRef {
            schema: Some("shop".into()),
            name: "f".into(),
            kind: RoutineKind::Function,
            identity_arguments: String::new(),
        })
        .await
        .unwrap()
        .unwrap();
    assert!(!def.is_empty());
    assert!(job_sql(&inserts(&fake).pop().unwrap()).contains("INFORMATION_SCHEMA.ROUTINES"));

    fake.on(
        "GET",
        &path("/datasets/shop/routines"),
        HttpResponse::json(
            200,
            &json!({"routines": [{"routineReference": {"projectId": "demo-proj", "datasetId": "shop", "routineId": "f"},
                "routineType": "SCALAR_FUNCTION", "language": "SQL"}]}),
        ),
    );
    let routines = s.list_routines("shop").await.unwrap().unwrap();
    assert_eq!(routines[0].name, "f");

    // A dry run: bytes as an int64 string, the columns, no job.
    fake.on(
        "POST",
        &path("/queries"),
        HttpResponse::json(
            200,
            &json!({"jobComplete": true, "totalBytesProcessed": "9007199254740993",
                "schema": schema(&[("id", "INTEGER")])}),
        ),
    );
    let jobs_before = inserts(&fake).len();
    let est = s
        .call(DRY_RUN, json!({"sql": "SELECT id FROM shop.orders"}))
        .await
        .unwrap();
    assert_eq!(est["bytesProcessed"], "9007199254740993");
    assert_eq!(est["columns"][0]["dataType"], "INT64");
    let sent = fake.requests_to("/queries").pop().unwrap();
    assert_eq!(sent.json()["dryRun"], true);
    assert_eq!(inserts(&fake).len(), jobs_before);
    // A full scan of one table: the statement is built here, by its whole quoted name.
    s.call(
        DRY_RUN,
        json!({"table": {"schema": "my ds", "name": "a`b"}, "project": "other-proj"}),
    )
    .await
    .unwrap();
    let sent = fake.requests_to("/queries").pop().unwrap();
    assert_eq!(
        sent.json()["query"],
        "SELECT * FROM `other-proj`.`my ds`.`a\\`b`"
    );
    s.call(
        DRY_RUN,
        json!({"table": {"schema": "shop", "name": "orders"}}),
    )
    .await
    .unwrap();
    let sent = fake.requests_to("/queries").pop().unwrap();
    assert_eq!(sent.json()["query"], "SELECT * FROM `shop`.`orders`");
    assert!(
        s.call(DRY_RUN, json!({"table": {"name": "orders"}}))
            .await
            .is_err()
    );
    assert!(s.call(DRY_RUN, json!({})).await.is_err());
    assert!(s.call("nope", json!({})).await.is_err());
}

#[tokio::test]
async fn a_closed_session_sends_nothing() {
    let fake = FakeBigquery::new();
    let s = session(&fake, false);
    s.close().await;
    assert!(matches!(s.ping().await, Err(AppError::NotConnected)));
    assert!(matches!(
        s.query("SELECT 1", &[]).await,
        Err(AppError::NotConnected)
    ));
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn errors_carry_their_reason() {
    let fake = FakeBigquery::new();
    fake.on(
        "GET",
        &path("/datasets/shop/tables/nope"),
        error_reply(404, "notFound", "Not found: Table demo-proj:shop.nope"),
    );
    fake.on(
        "POST",
        &path("/jobs"),
        failed_insert(
            "bytesBilledLimitExceeded",
            "Query exceeded limit for bytes billed",
        ),
    );
    let s = session(&fake, false);
    let e = s
        .table_meta(&TableRef::new(Some("shop"), "nope"))
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::NotFound(_)), "{e:?}");
    let e = s.query("SELECT * FROM big", &[]).await.unwrap_err();
    assert!(
        matches!(&e, AppError::Db { hint: Some(h), .. } if h.contains("Maximum bytes billed")),
        "{e:?}"
    );
}

// --- entry: sign-in and the form's calls over the wire ------------------------------------------

/// A BigQuery profile of `demo-proj` with `params`.
fn profile(params: serde_json::Value, read_only: bool) -> ConnectionProfile {
    let mut p = crate::drivers::common::token::tests::profile(Driver::Bigquery, None);
    p.host = None;
    p.database = Some("demo-proj".into());
    p.options.read_only = read_only;
    p.options.driver_params = params.as_object().cloned().map(DriverParams);
    p
}

fn bigquery_routes(token: &str) -> Vec<Route> {
    vec![
        Route::new("POST", "/token").reply(Reply::json(
            200,
            &json!({"access_token": token, "expires_in": 3600}),
        )),
        Route::new("GET", "/bigquery/v2/projects/demo-proj/datasets")
            .auth(&format!("Bearer {token}"))
            .reply(Reply::json(
                200,
                &json!({"datasets": [
                    {"datasetReference": {"projectId": "demo-proj", "datasetId": "shop"}, "location": "EU"}]}),
            )),
        Route::new("GET", "/bigquery/v2/projects")
            .auth(&format!("Bearer {token}"))
            .reply(Reply::json(
                200,
                &json!({"projects": [{"projectReference": {"projectId": "demo-proj"}, "friendlyName": "Demo"}]}),
            )),
    ]
}

fn assertion_scope(server: &FakeServer) -> String {
    let token = server
        .requests()
        .into_iter()
        .rev()
        .find(|r| r.path == "/token")
        .expect("a token request");
    let assertion = token
        .text()
        .split('&')
        .find_map(|p| p.strip_prefix("assertion="))
        .unwrap_or_default()
        .to_string();
    let (_, claims) =
        crate::drivers::common::jwt::verify_rs256(&test_key().public_key, &assertion).unwrap();
    claims["scope"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
async fn open_signs_in_with_a_key_file_and_proves_it() {
    crate::testkit::install_crypto();
    let server = FakeServer::start(bigquery_routes("sa-token")).await;
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key.json");
    std::fs::write(&key, sa_key_json(&server.url("/token"))).unwrap();
    let params = json!({
        "auth": "serviceAccount",
        "keyFilePath": key.to_string_lossy(),
        "apiEndpoint": server.url("/bigquery/v2"),
        "defaultDataset": "shop",
    });
    for read_only in [false, true] {
        let p = profile(params.clone(), read_only);
        let session = entry::open(ConnectTarget {
            profile: &p,
            password: None,
            host_override: None,
        })
        .await
        .unwrap();
        let Session::Sql(s) = session else {
            panic!("a SQL session");
        };
        let info = s.server_info().await.unwrap();
        assert_eq!(
            info.current_user.as_deref(),
            Some("tabler@demo-proj.iam.gserviceaccount.com")
        );
        assert_eq!(
            s.capabilities().namespaces.default_schema.as_deref(),
            Some("shop")
        );
        // The full scope on a read-only profile too: `jobs.insert` refuses `bigquery.readonly`,
        // so a narrowed token could run no query at all.
        assert_eq!(assertion_scope(&server), super::auth::SCOPE_BIGQUERY);
        assert_eq!(
            s.capabilities().safety.read_only,
            ReadOnlyMode::ClassifierOnly
        );
        // The probe carried the bearer.
        assert!(
            server
                .requests()
                .iter()
                .any(|r| r.path.ends_with("/datasets")
                    && r.header("authorization") == Some("Bearer sa-token"))
        );
    }
}

#[tokio::test]
async fn open_with_an_access_token_and_the_form_calls() {
    crate::testkit::install_crypto();
    let server = FakeServer::start(bigquery_routes("ya29.typed")).await;
    let params = json!({"auth": "accessToken", "apiEndpoint": server.url("/bigquery/v2")});
    let p = profile(params.clone(), false);
    entry::open(ConnectTarget {
        profile: &p,
        password: Some("ya29.typed"),
        host_override: None,
    })
    .await
    .unwrap();
    assert!(server.requests().iter().all(|r| r.path != "/token"));
    // No token: refused before anything is sent.
    let before = server.requests().len();
    let e = entry::open(ConnectTarget {
        profile: &p,
        password: None,
        host_override: None,
    })
    .await
    .err()
    .unwrap();
    assert!(matches!(e, AppError::Auth(_)), "{e:?}");
    assert_eq!(server.requests().len(), before);
    // A wrong token: the probe fails as Auth.
    let e = entry::open(ConnectTarget {
        profile: &p,
        password: Some("wrong"),
        host_override: None,
    })
    .await
    .err()
    .unwrap();
    assert!(matches!(e, AppError::Auth(_)), "{e:?}");

    // The form's pickers.
    let datasets = entry::util(
        UtilInput {
            profile: &p,
            secret: Some("ya29.typed"),
        },
        entry::LIST_DATASETS,
        json!({}),
    )
    .await
    .unwrap();
    assert_eq!(datasets, json!([{"name": "shop", "location": "EU"}]));
    let projects = entry::util(
        UtilInput {
            profile: &p,
            secret: Some("ya29.typed"),
        },
        entry::LIST_PROJECTS,
        json!({}),
    )
    .await
    .unwrap();
    assert_eq!(projects, json!([{"id": "demo-proj", "name": "Demo"}]));
    assert_eq!(entry::util_kind(entry::LIST_DATASETS), CallKind::Read);
    assert_eq!(entry::util_kind(entry::LIST_PROJECTS), CallKind::Read);
    assert_eq!(entry::util_kind("createDataset"), CallKind::Write);
    assert!(
        entry::util(
            UtilInput {
                profile: &p,
                secret: None,
            },
            "createDataset",
            json!({}),
        )
        .await
        .is_err()
    );

    // Command mode: the command's output is the token.
    let mut p = profile(params, false);
    p.options.password_mode = Some(PasswordMode::Command);
    p.options.password_command = Some("gcloud auth print-access-token".into());
    entry::open(ConnectTarget {
        profile: &p,
        password: Some("ya29.typed"),
        host_override: None,
    })
    .await
    .unwrap();
}

#[test]
fn validate_and_the_cost_guard() {
    let ok = profile(json!({"auth": "applicationDefault"}), false);
    entry::validate(&ok).unwrap();
    for bad in [
        json!({"auth": "serviceAccount"}),
        json!({"auth": "accessToken", "maximumBytesBilled": 0}),
        json!({"auth": "accessToken", "apiEndpoint": "http://evil.example/bigquery/v2"}),
        json!({"auth": "nope"}),
    ] {
        assert!(
            entry::validate(&profile(bad.clone(), false)).is_err(),
            "{bad}"
        );
    }
    let params = BigqueryParams::of(&profile(
        json!({"auth": "accessToken", "maximumBytesBilled": null, "location": "EU",
            "defaultDataset": "shop", "labels": {"team": "data"}}),
        false,
    ))
    .unwrap();
    let opts = entry::query_options(&params, "demo-proj", false);
    assert_eq!(opts.maximum_bytes_billed, None, "the user chose no limit");
    assert_eq!(opts.location.as_deref(), Some("EU"));
    assert_eq!(opts.default_dataset.unwrap().project_id, "demo-proj");
    // The MCP process never runs a job without a cost guard.
    let opts = entry::query_options(&params, "demo-proj", true);
    assert_eq!(
        opts.maximum_bytes_billed,
        Some(DEFAULT_MAXIMUM_BYTES_BILLED)
    );
}

/// The caps of a session (with a default dataset, read-only, and without a default dataset),
/// kept in `tests/fixtures/bigquery/caps.json`: the renderer's mock tests read the same file, so
/// the mock's caps cannot drift from these. `TABLER_BLESS=1` rewrites it.
#[tokio::test]
async fn caps_match_the_recorded_fixture() {
    let fake = FakeBigquery::new();
    let mut all = serde_json::Map::new();
    for (name, read_only, default) in [
        ("editable", false, true),
        ("readOnly", true, true),
        ("noDefaultDataset", false, false),
    ] {
        let mut cfg = config(read_only);
        if !default {
            cfg.options.default_dataset = None;
        }
        let client = Client::new(fake.transport(), BASE, "demo-proj");
        let s = BigquerySession::new(Arc::new(client), cfg);
        all.insert(
            name.into(),
            serde_json::to_value(s.capabilities()).expect("caps"),
        );
    }
    let got = serde_json::Value::Object(all);
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/bigquery/caps.json");
    if std::env::var("TABLER_BLESS").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("folder");
        let text = serde_json::to_string_pretty(&got).expect("json");
        std::fs::write(&path, text + "\n").expect("written");
        return;
    }
    let want: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    assert!(
        got == want,
        "{} differs (TABLER_BLESS=1 rewrites it); now:\n{}",
        path.display(),
        serde_json::to_string_pretty(&got).expect("json")
    );
}
