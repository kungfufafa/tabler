//! The driver end to end. The client against `FakeEs` behind the transport seam (list, browse
//! with `from`/`size`, a point in time and a scroll, count, fetch, a commit with one failing
//! change, console runs); over HTTP against `testkit::fake_http` replaying recorded replies
//! (`/`, `_cat/indices`, `_mapping`, point-in-time pages, a `_bulk` partial failure), through
//! the shared `HttpClient` (credentials, NDJSON bodies, SigV4, cancel by dropping a request,
//! time outs); and the session opened by `entry::open` on `FakeEs` served over HTTP: connect,
//! list, cursor pages, counts, fetch, commits, the console with SQL cursors, cancel and the
//! query time out, read-only refusals on every write path, tasks, definitions, TLS.

use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use super::catalog::{self, ColumnsFor};
use super::client::EsClient;
use super::console::{EsRequest, Method, parse_request};
use super::entry;
use super::info::Features;
use super::json::{self, Json};
use super::session::check_target;
use super::testing::{self, ES8_ROOT, FakeEs, OS2_ROOT, error};
use super::transport::{BaseUrl, HttpResponse, HttpTransport};
use super::values::hit_rows;
use crate::drivers::common::aws::{CredentialProvider, Credentials, Signer};
use crate::drivers::common::http::{Credential, HttpClient, HttpEndpoint, HttpOptions};
use crate::drivers::common::token::TokenSource;
use crate::drivers::native::{CallKind, NativeSession};
use crate::drivers::registry::UtilInput;
use crate::drivers::{CancelSlot, ConnectTarget, RowSink, Session, request_cancel};
use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::native::elastic::{self as build, BrowseSpec, EditTarget};
use crate::testkit::{FakeServer, Outcome, Reply, Route};
use crate::types::browse::{
    BrowseRequest, Conjunction, FilterGroup, FilterNode, FilterOp, RowKey, SortDir, SortSpec,
};
use crate::types::caps::{Feature, Paging, TxMode};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::{ApplyRequest, StagedChange};
use crate::types::profile::ConnectionProfile;
use crate::types::query::{QueryEvent, ResultColumn, RunOptions};
use crate::types::schema::{TableKind, TableMeta, TableRef};
use crate::types::value::Value;

const MAPPING: &str = r#"{"properties":{"n":{"type":"long"},"name":{"type":"text","fields":{"keyword":{"type":"keyword"}}},"user":{"properties":{"id":{"type":"keyword"}}}}}"#;

fn fake(root: &str, docs: usize) -> Arc<FakeEs> {
    let es = Arc::new(FakeEs::new(root));
    let docs: Vec<(String, String)> = (0..docs)
        .map(|i| {
            (
                format!("d{i}"),
                format!(
                    r#"{{"n":{i},"name":"doc {i}","user":{{"id":"u{}"}}}}"#,
                    i % 3
                ),
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = docs.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    es.index("items", MAPPING, &refs);
    es.alias("current", "items");
    es
}

fn client(es: &Arc<FakeEs>) -> EsClient {
    EsClient::new(
        es.clone(),
        BaseUrl::parse("http://es.test:9200", false).unwrap(),
        "tabler-1",
    )
}

async fn connect(es: &Arc<FakeEs>) -> (EsClient, Features) {
    let c = client(es);
    let info = c.info().await.unwrap();
    let f = info.features();
    (c, f)
}

fn spec(c_target: &str) -> BrowseSpec {
    BrowseSpec {
        target: c_target.to_string(),
        query: build::match_all(),
        sort: vec![Json::str("_doc")],
        source: None,
        seq_no: true,
    }
}

fn ids(hits: &[Json]) -> Vec<&str> {
    hits.iter().filter_map(|h| h.get("_id")?.as_str()).collect()
}

#[tokio::test]
async fn connect_list_and_describe() {
    let es = fake(ES8_ROOT, 3);
    let (c, f) = connect(&es).await;
    let tables = c.tables(Some("fake"), false, f).await.unwrap();
    let names: Vec<(&str, TableKind)> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(
        names,
        [("items", TableKind::Table), ("current", TableKind::View)]
    );
    assert_eq!(tables[0].row_estimate, Some(3));
    let m = c.mappings("items").await.unwrap();
    let cols = catalog::columns(
        &catalog::merge(&m),
        ColumnsFor {
            spans_indices: false,
            seq_no_guards: f.seq_no_guards,
        },
    );
    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        ["_id", "_seq_no", "_primary_term", "n", "name", "user.id"]
    );
    assert_eq!(c.result_window("items").await.unwrap(), 10_000);
    assert!(matches!(
        c.mappings("nope").await,
        Err(AppError::NotFound(_))
    ));
    // Every request carried the opaque id; credentials are the HTTP client's.
    for r in es.requests() {
        assert_eq!(r.header("x-opaque-id"), Some("tabler-1"));
        assert_eq!(r.header("authorization"), None);
    }
}

#[tokio::test]
async fn shallow_pages_use_from_and_size_and_hand_out_a_cursor() {
    let es = fake(ES8_ROOT, 12);
    let (c, f) = connect(&es).await;
    let page = c
        .browse(&spec("items"), f, 5, 5, 10_000, None, "tabler-1")
        .await
        .unwrap();
    assert_eq!(ids(&page.hits), ["d5", "d6", "d7", "d8", "d9"]);
    assert_eq!(
        es.lines().last().unwrap(),
        "POST /items/_search?preference=tabler-1"
    );
    // The cursor of a full page starts the next one, whatever the offset says.
    let next = c
        .browse(
            &spec("items"),
            f,
            0,
            5,
            10_000,
            page.next_cursor.as_deref(),
            "p",
        )
        .await
        .unwrap();
    assert_eq!(ids(&next.hits), ["d10", "d11"]);
    assert_eq!(next.next_cursor, None);
}

#[tokio::test]
async fn deep_pages_walk_a_point_in_time_and_continue_by_cursor() {
    let es = fake(ES8_ROOT, 23);
    let (c, f) = connect(&es).await;
    let s = spec("items");
    // A window of 10 makes offset 10 a deep page: open a PIT, skip 10 rows without _source.
    let p1 = c.browse(&s, f, 10, 5, 10, None, "p").await.unwrap();
    assert_eq!(ids(&p1.hits), ["d10", "d11", "d12", "d13", "d14"]);
    let cursor = p1.next_cursor.clone().expect("a cursor");
    assert_eq!(es.open_pits(), 1);
    let skip = es
        .requests()
        .into_iter()
        .find(|r| r.path() == "/_search")
        .unwrap();
    let body = json::parse(&skip.body_text()).unwrap();
    assert_eq!(body.get("_source"), Some(&Json::Bool(false)));
    assert_eq!(body.get("size").and_then(Json::as_u64), Some(10));
    // The next page continues from the cursor: one request.
    let before = es.requests().len();
    let p2 = c.browse(&s, f, 0, 5, 10, Some(&cursor), "p").await.unwrap();
    assert_eq!(es.requests().len(), before + 1);
    assert_eq!(ids(&p2.hits)[0], "d15");
    let p3 = c
        .browse(&s, f, 0, 5, 10, p2.next_cursor.as_deref(), "p")
        .await
        .unwrap();
    assert_eq!(p3.hits.len(), 3);
    assert_eq!(p3.next_cursor, None);
    assert_eq!(es.open_pits(), 0, "the last page closes the point in time");
    // Back to page 2: its point in time is closed, so the walk starts again there.
    let again = c.browse(&s, f, 0, 5, 10, Some(&cursor), "p").await.unwrap();
    assert_eq!(ids(&again.hits), ["d15", "d16", "d17", "d18", "d19"]);
    // A page past the end returns nothing and leaves nothing open.
    let past = c.browse(&s, f, 40, 5, 10, None, "p").await.unwrap();
    assert!(past.hits.is_empty());
}

#[tokio::test]
async fn opensearch_deep_pages_scroll_and_rewalk_going_back() {
    let es = fake(OS2_ROOT, 13);
    let (c, f) = connect(&es).await;
    assert!(!f.shard_doc);
    let s = spec("items");
    let p = c.browse(&s, f, 10, 5, 10, None, "p").await.unwrap();
    assert_eq!(ids(&p.hits), ["d10", "d11", "d12"]);
    assert_eq!(p.next_cursor, None);
    assert_eq!(es.open_scrolls(), 0, "the last batch clears the scroll");
    let es = fake(OS2_ROOT, 30);
    let (c, f) = connect(&es).await;
    let p = c.browse(&s, f, 10, 5, 10, None, "p").await.unwrap();
    assert_eq!(ids(&p.hits)[0], "d10");
    let at15 = p.next_cursor.clone().unwrap();
    let next = c.browse(&s, f, 0, 5, 10, Some(&at15), "p").await.unwrap();
    assert_eq!(ids(&next.hits)[0], "d15");
    assert_eq!(
        es.lines().last().unwrap(),
        "POST /_search/scroll",
        "a continued scroll is one request"
    );
    // Reading page 15 again: the scroll moved on, so it walks again instead of skipping rows.
    let back = c.browse(&s, f, 0, 5, 10, Some(&at15), "p").await.unwrap();
    assert_eq!(ids(&back.hits), ["d15", "d16", "d17", "d18", "d19"]);
    c.close_walks().await;
    assert_eq!(es.open_scrolls(), 0, "closing frees the open scrolls");
}

#[tokio::test]
async fn counts_and_single_documents() {
    let es = fake(ES8_ROOT, 9);
    let (c, f) = connect(&es).await;
    let fields = catalog::flatten_mapping(&json::parse(MAPPING).unwrap()).fields;
    let filter = FilterGroup {
        conjunction: Conjunction::And,
        items: vec![FilterNode::Condition {
            column: "_id".into(),
            op: FilterOp::In,
            values: vec![Value::Text("d1".into()), Value::Text("d2".into())],
        }],
    };
    let q = build::query(&fields, Some(&filter), f).unwrap();
    assert_eq!(c.count("items", &q).await.unwrap(), 2);
    assert_eq!(c.count("items", &build::match_all()).await.unwrap(), 9);
    let doc = c
        .document("items", None, "d4", None, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(doc.at(&["_source", "n"]), Some(&Json::Num("4".into())));
    assert_eq!(
        c.document("items", None, "zz", None, None).await.unwrap(),
        None
    );
}

fn key(id: &str, seq: i64) -> RowKey {
    RowKey::Pk(
        [
            ("_id".to_string(), Value::Text(id.into())),
            ("_seq_no".to_string(), Value::Int(seq)),
            ("_primary_term".to_string(), Value::Int(1)),
        ]
        .into_iter()
        .collect(),
    )
}

/// An update that applies, a delete read at an old `_seq_no` (someone changed the document),
/// an insert.
fn three_changes() -> Vec<StagedChange> {
    vec![
        StagedChange::Update {
            row_id: "1".into(),
            key: key("d0", 0),
            changes: [
                ("n".to_string(), Value::Text("100".into())),
                ("user.id".to_string(), Value::Text("ux".into())),
            ]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
            exprs: None,
        },
        StagedChange::Delete {
            row_id: "2".into(),
            key: key("d1", 0),
        },
        StagedChange::Insert {
            row_id: "3".into(),
            values: [("n".to_string(), Value::Int(7))].into_iter().collect(),
            exprs: None,
        },
    ]
}

#[tokio::test]
async fn a_commit_reports_each_change_and_keeps_the_applied_ones() {
    let es = fake(ES8_ROOT, 3);
    let (c, f) = connect(&es).await;
    let fields = catalog::flatten_mapping(&json::parse(MAPPING).unwrap()).fields;
    let t = EditTarget {
        target: "items",
        fields: &fields,
        type_name: None,
        features: f,
    };
    let plan = build::plan_edits(&t, &three_changes()).unwrap();
    let out = c.apply(&plan).await.unwrap();
    assert_eq!(out.len(), 3);
    let first = out[0].as_ref().unwrap();
    assert_eq!(first.affected, 1);
    assert!(matches!(first.returned.get("_seq_no"), Some(Value::Int(_))));
    assert!(matches!(
        &out[1],
        Err(AppError::Db {
            vendor_code: Some(409),
            ..
        })
    ));
    let inserted = out[2].as_ref().unwrap();
    let new_id = inserted
        .returned
        .get("_id")
        .and_then(Value::as_text)
        .unwrap();
    // The fake applied the first and third, not the second.
    let d0 = es.doc("items", "d0").unwrap();
    assert_eq!(d0.source.get("n"), Some(&Json::Num("100".into())));
    assert_eq!(d0.source.at(&["user", "id"]), Some(&Json::str("ux")));
    assert!(es.doc("items", "d1").is_some());
    assert!(es.doc("items", &new_id).is_some());
    // One request, NDJSON, waiting for the refresh.
    let bulk = es.requests().into_iter().last().unwrap();
    assert_eq!(bulk.url, "http://es.test:9200/_bulk?refresh=wait_for");
    assert_eq!(bulk.header("content-type"), Some("application/x-ndjson"));
    assert!(bulk.body_text().ends_with('\n'));
    assert_eq!(bulk.body_text().lines().count(), 5);
}

#[tokio::test]
async fn a_failed_bulk_request_fails_every_change() {
    let es = fake(ES8_ROOT, 1);
    let (c, f) = connect(&es).await;
    let t = EditTarget {
        target: "items",
        fields: &[],
        type_name: None,
        features: f,
    };
    let plan = build::plan_edits(
        &t,
        &[StagedChange::Delete {
            row_id: "1".into(),
            key: key("d0", 0),
        }],
    )
    .unwrap();
    es.fail_once(
        "POST /_bulk",
        error(429, "es_rejected_execution_exception", "rejected"),
    );
    match c.apply(&plan).await {
        Err(AppError::Db { hint, .. }) => assert!(hint.unwrap().contains("retry")),
        other => panic!("{other:?}"),
    }
    assert!(es.doc("items", "d0").is_some());
}

#[tokio::test]
async fn console_runs() {
    let es = fake(ES8_ROOT, 4);
    let (c, _) = connect(&es).await;
    let req =
        parse_request("POST /items/_search\n{\"query\":{\"match_all\":{}},\"size\":2}").unwrap();
    let grids = c.console(&req, 1).await.unwrap();
    assert_eq!(grids.len(), 1);
    assert_eq!(grids[0].rows.len(), 1);
    assert!(grids[0].truncated);
    let names: Vec<&str> = grids[0].columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["_index", "_id", "_score", "n", "name", "user.id"]);
    let bad = parse_request("GET /nope/_mapping").unwrap();
    assert!(matches!(
        c.console(&bad, 10).await,
        Err(AppError::NotFound(_))
    ));
    es.fail_once(
        "GET /_cat/health",
        HttpResponse::text(200, "1700000000 10:00:00 fake green 1 1\n"),
    );
    let text = c
        .console(&parse_request("GET _cat/health").unwrap(), 10)
        .await
        .unwrap();
    assert_eq!(
        text[0].rows[0][0],
        Value::Text("1700000000 10:00:00 fake green 1 1\n".into())
    );
}

// --- over HTTP, through the shared client -------------------------------------------------------

fn json_reply(body: &str) -> Reply {
    Reply::new(200)
        .header("content-type", "application/json")
        .body(body.to_string())
}

const BASIC: &str = "Basic ZWxhc3RpYzpjaGFuZ2VtZQ==";

fn fixture(name: &str) -> &'static str {
    match name {
        "root-es8" => include_str!("../../../tests/fixtures/elasticsearch/root-es8.json"),
        "root-os2" => include_str!("../../../tests/fixtures/elasticsearch/root-os2.json"),
        "cat-indices" => include_str!("../../../tests/fixtures/elasticsearch/cat-indices.json"),
        "cat-aliases" => include_str!("../../../tests/fixtures/elasticsearch/cat-aliases.json"),
        "mapping-logs" => include_str!("../../../tests/fixtures/elasticsearch/mapping-logs.json"),
        "pit-open" => include_str!("../../../tests/fixtures/elasticsearch/pit-open.json"),
        "pit-page-1" => include_str!("../../../tests/fixtures/elasticsearch/pit-page-1.json"),
        "pit-page-2" => include_str!("../../../tests/fixtures/elasticsearch/pit-page-2.json"),
        "bulk-partial" => include_str!("../../../tests/fixtures/elasticsearch/bulk-partial.json"),
        "security-401" => include_str!("../../../tests/fixtures/elasticsearch/security-401.json"),
        other => panic!("no fixture {other}"),
    }
}

async fn replay_server() -> FakeServer {
    FakeServer::start(vec![
        Route::new("GET", "/")
            .auth(BASIC)
            .reply(json_reply(fixture("root-es8"))),
        Route::new("GET", "/_cat/indices")
            .auth(BASIC)
            .reply(json_reply(fixture("cat-indices"))),
        Route::new("GET", "/_cat/aliases")
            .auth(BASIC)
            .reply(json_reply(fixture("cat-aliases"))),
        Route::new("GET", "/_data_stream")
            .auth(BASIC)
            .reply(json_reply(r#"{"data_streams":[]}"#)),
        Route::new("GET", "/logs-2024/_mapping")
            .auth(BASIC)
            .reply(json_reply(fixture("mapping-logs"))),
        Route::new("POST", "/logs-2024/_pit")
            .auth(BASIC)
            .reply(json_reply(fixture("pit-open"))),
        Route::new("POST", "/_search")
            .auth(BASIC)
            .reply(json_reply(fixture("pit-page-1")))
            .reply(json_reply(fixture("pit-page-2"))),
        Route::new("DELETE", "/_pit")
            .auth(BASIC)
            .reply(json_reply(r#"{"succeeded":true,"num_freed":1}"#)),
        Route::new("POST", "/_bulk")
            .auth(BASIC)
            .reply(json_reply(fixture("bulk-partial"))),
        Route::new("GET", "/slow").reply(Reply::ok().delay(Duration::from_secs(30))),
    ])
    .await
}

fn http_client(server: &FakeServer, credential: Credential, timeout: Option<Duration>) -> EsClient {
    crate::testkit::install_crypto();
    let base = BaseUrl::parse(&server.url(""), false).unwrap();
    let options = HttpOptions {
        request_timeout: timeout,
        ..HttpOptions::default()
    };
    let http = HttpClient::new(
        HttpEndpoint::new(&base.root(), None).unwrap(),
        options,
        credential,
    )
    .unwrap();
    EsClient::new(Arc::new(HttpTransport::new(http)), base, "tabler-7")
}

fn basic() -> Credential {
    Credential::Basic("elastic".into(), TokenSource::from_secret(Some("changeme")))
}

#[tokio::test]
async fn wire_replay_connect_list_browse_and_commit() {
    let server = replay_server().await;
    let c = http_client(&server, basic(), None);
    let info = c.info().await.unwrap();
    assert_eq!(info.display_version(), "Elasticsearch 8.15.2");
    assert_eq!(info.cluster_name, "docker-cluster");
    let f = info.features();
    let tables = c.tables(None, false, f).await.unwrap();
    let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["logs-2024", "products", "logs"]);

    let mapping = catalog::merge(&c.mappings("logs-2024").await.unwrap());
    let cols = catalog::columns(
        &mapping,
        ColumnsFor {
            spans_indices: false,
            seq_no_guards: true,
        },
    );
    let s = BrowseSpec {
        target: "logs-2024".into(),
        query: build::match_all(),
        sort: build::sort(&mapping.fields, &[], "_doc").unwrap(),
        source: None,
        seq_no: true,
    };
    // A window of 1 forces the point-in-time walk.
    let p1 = c.browse(&s, f, 0, 2, 1, None, "p").await.unwrap();
    assert_eq!(p1.hits.len(), 2);
    assert!(
        p1.request.starts_with(
            "POST /_search
"
        ) && p1.request.contains(r#""pit":"#),
        "{}",
        p1.request
    );
    let rows = hit_rows(&p1.hits, &cols);
    let col = |name: &str| cols.iter().position(|c| c.name == name).unwrap();
    assert_eq!(
        rows[0][col("bytes")],
        Value::BigInt("9007199254740993".into())
    );
    assert_eq!(rows[0][col("host.ip")], Value::other("ip", "10.0.0.1"));
    assert_eq!(
        rows[0][col("tags")],
        Value::Json(r#"["prod","web"]"#.into())
    );
    assert_eq!(
        rows[0][col("@timestamp")],
        Value::TimestampTz("2024-05-01T10:00:00.000Z".into())
    );
    assert_eq!(rows[1][col("host.name")], Value::Text("web-2".into()));
    assert!(super::values::is_missing(&rows[1][col("tags")]));
    assert_eq!(rows[0][col("_seq_no")], Value::Int(0));
    let p2 = c
        .browse(&s, f, 0, 2, 1, p1.next_cursor.as_deref(), "p")
        .await
        .unwrap();
    assert_eq!(p2.hits.len(), 1);
    assert_eq!(p2.next_cursor, None);
    let row = hit_rows(&p2.hits, &cols).remove(0);
    assert_eq!(row[col("bytes")], Value::Float(1500.0));
    assert_eq!(row[col("message")], Value::Null);

    // The edit of the first row applies; the delete of the second meets a conflict.
    let t = EditTarget {
        target: "logs-2024",
        fields: &mapping.fields,
        type_name: None,
        features: f,
    };
    let plan = build::plan_edits(
        &t,
        &[
            StagedChange::Update {
                row_id: "a1".into(),
                key: key("a1", 0),
                changes: [("message".to_string(), Value::Text("edited".into()))]
                    .into_iter()
                    .collect(),
                exprs: None,
            },
            StagedChange::Delete {
                row_id: "a2".into(),
                key: key("a2", 1),
            },
        ],
    )
    .unwrap();
    let out = c.apply(&plan).await.unwrap();
    assert_eq!(
        out[0].as_ref().unwrap().returned.get("_seq_no"),
        Some(&Value::Int(3))
    );
    assert!(matches!(
        &out[1],
        Err(AppError::Db {
            vendor_code: Some(409),
            ..
        })
    ));

    let reqs = server.requests();
    let lines: Vec<String> = reqs
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    assert_eq!(
        lines,
        [
            "GET /",
            "GET /_cat/indices",
            "GET /_cat/aliases",
            "GET /_data_stream",
            "GET /logs-2024/_mapping",
            "POST /logs-2024/_pit",
            "POST /_search",
            "POST /_search",
            "DELETE /_pit",
            "POST /_bulk",
        ]
    );
    for r in &reqs {
        assert_eq!(r.header("authorization"), Some(BASIC), "{}", r.path);
        assert_eq!(r.header("x-opaque-id"), Some("tabler-7"));
    }
    let second = json::parse(&reqs[7].text()).unwrap();
    assert_eq!(
        second.get("search_after").map(Json::to_text).as_deref(),
        Some("[1,4294967297]")
    );
    assert!(second.at(&["pit", "id"]).is_some());
    assert_eq!(reqs[9].query.as_deref(), Some("refresh=wait_for"));
    assert_eq!(reqs[9].header("content-type"), Some("application/x-ndjson"));
    assert_eq!(
        reqs[9].text(),
        "{\"update\":{\"_index\":\"logs-2024\",\"_id\":\"a1\",\"if_seq_no\":0,\"if_primary_term\":1}}\n{\"doc\":{\"message\":\"edited\"}}\n{\"delete\":{\"_index\":\"logs-2024\",\"_id\":\"a2\",\"if_seq_no\":1,\"if_primary_term\":1}}\n"
    );
}

#[tokio::test]
async fn wire_bad_credentials_are_auth_errors() {
    let server = replay_server().await;
    let c = http_client(&server, Credential::None, None);
    assert!(matches!(c.info().await, Err(AppError::Auth(_))));
    let wrong = http_client(
        &server,
        Credential::bearer(TokenSource::from_secret(Some("x"))),
        None,
    );
    assert!(matches!(wrong.info().await, Err(AppError::Auth(_))));
    let es_style = FakeServer::start(vec![
        Route::new("GET", "/").reply(
            Reply::new(401)
                .header("content-type", "application/json")
                .body(fixture("security-401")),
        ),
    ])
    .await;
    let c = http_client(&es_style, basic(), None);
    match c.info().await {
        Err(AppError::Auth(m)) => assert!(m.contains("missing authentication credentials")),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn wire_cancel_drops_the_request_and_timeouts_map() {
    let server = replay_server().await;
    let c = http_client(&server, basic(), None);
    let req = parse_request("GET /slow").unwrap();
    tokio::select! {
        _ = c.console(&req, 10) => panic!("the slow reply came back"),
        _ = tokio::time::sleep(Duration::from_millis(200)) => {}
    }
    server
        .wait_until(Duration::from_secs(5), |reqs| {
            reqs.iter()
                .any(|r| r.path == "/slow" && r.outcome == Outcome::Aborted)
        })
        .await;
    let quick = http_client(&server, basic(), Some(Duration::from_millis(200)));
    assert!(matches!(
        quick.console(&req, 10).await,
        Err(AppError::Timeout)
    ));
}

#[tokio::test]
async fn the_transport_sends_paths_as_written_or_not_at_all() {
    let server = FakeServer::start(vec![Route::new("*", "/*").reply(json_reply("{}"))]).await;
    let c = http_client(&server, Credential::None, None);
    // A segment the URL parser would fold into another path is refused; nothing is sent.
    let folded = EsRequest::new(Method::Get, &["a", "..", "_bulk"]);
    assert!(matches!(
        c.send(&folded).await,
        Err(AppError::InvalidInput(_))
    ));
    assert!(server.requests().is_empty());
    // A name with slashes stays one segment.
    let slashes = EsRequest::new(Method::Get, &["a/../_bulk", "_search"]);
    c.send(&slashes).await.unwrap();
    assert_eq!(server.requests()[0].path, "/a%2F..%2F_bulk/_search");
}

#[tokio::test]
async fn replies_are_read_up_to_a_limit() {
    let big = format!(r#"{{"a":"{}"}}"#, "x".repeat(4096));
    let failed = format!(
        r#"{{"error":{{"type":"x","reason":"{}"}},"status":500}}"#,
        "y".repeat(4096)
    );
    let server = FakeServer::start(vec![
        Route::new("GET", "/big").reply(json_reply(&big)),
        Route::new("GET", "/small").reply(json_reply(r#"{"ok":true}"#)),
        Route::new("GET", "/failed").reply(
            Reply::new(500)
                .header("content-type", "application/json")
                .body(failed),
        ),
    ])
    .await;
    crate::testkit::install_crypto();
    let base = BaseUrl::parse(&server.url(""), false).unwrap();
    let http = HttpClient::new(
        HttpEndpoint::new(&base.root(), None).unwrap(),
        HttpOptions::default(),
        Credential::None,
    )
    .unwrap();
    let transport = HttpTransport::new(http).with_limits(1024, 64);
    let c = EsClient::new(Arc::new(transport), base, "tabler-7");
    // A reply past the limit is an error, not memory without end.
    match c.send(&EsRequest::new(Method::Get, &["big"])).await {
        Err(AppError::Db { message, .. }) => assert!(message.contains("larger than"), "{message}"),
        other => panic!("{other:?}"),
    }
    let small = c.send(&EsRequest::new(Method::Get, &["small"])).await;
    assert_eq!(small.unwrap().body, br#"{"ok":true}"#);
    // An error reply is cut short; its status still says what failed.
    let failed = c
        .send(&EsRequest::new(Method::Get, &["failed"]))
        .await
        .unwrap();
    assert_eq!((failed.status, failed.body.len()), (500, 64));
    assert!(matches!(
        c.console(&EsRequest::new(Method::Get, &["failed"]), 10)
            .await,
        Err(AppError::Db {
            vendor_code: Some(500),
            ..
        })
    ));
}

#[test]
fn mcp_is_refused() {
    assert!(matches!(
        entry::refuse_unattended(true),
        Err(AppError::Unsupported(_))
    ));
    assert!(entry::refuse_unattended(false).is_ok());
}

#[tokio::test]
async fn wire_opensearch_and_sigv4() {
    let server = FakeServer::start(vec![
        Route::new("GET", "/").reply(json_reply(fixture("root-os2"))),
    ])
    .await;
    let creds = Credentials::static_keys("AKIDEXAMPLE", "secret", Some("tok")).unwrap();
    let signer = Signer::new(
        "es",
        "eu-west-1",
        Arc::new(CredentialProvider::fixed(creds)),
    );
    let c = http_client(&server, Credential::Sigv4(signer), None);
    let info = c.info().await.unwrap();
    assert_eq!(info.display_version(), "OpenSearch 2.17.1");
    let r = &server.requests()[0];
    let a = r.header("authorization").unwrap();
    assert!(
        a.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"),
        "{a}"
    );
    assert!(a.contains("/eu-west-1/es/aws4_request"));
    assert!(
        a.contains(
            "SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token;x-opaque-id"
        ),
        "{a}"
    );
    assert_eq!(r.header("x-amz-security-token"), Some("tok"));
    assert!(r.header("x-amz-date").is_some());
}

// --- the session, opened by `entry::open` on FakeEs over HTTP ----------------------------------

/// A profile of the fake server at `addr`: plain HTTP, the `elastic` user, `extra` merged in
/// (`options.driverParams`, `options.readOnly`…).
fn profile(server: &FakeServer, extra: serde_json::Value) -> ConnectionProfile {
    let mut base = serde_json::json!({
        "id": "00000000-0000-0000-0000-000000000001", "name": "es", "driver": "elasticsearch",
        "host": "127.0.0.1", "port": server.addr().port(), "user": "elastic",
        "ssl": "disable", "options": {}, "sortOrder": 0, "createdAt": "", "updatedAt": "",
    });
    merge(&mut base, &extra);
    serde_json::from_value(base).unwrap()
}

fn merge(into: &mut serde_json::Value, extra: &serde_json::Value) {
    if let (Some(b), Some(extra)) = (into.as_object_mut(), extra.as_object()) {
        for (k, v) in extra {
            match b.get_mut(k) {
                Some(existing) if existing.is_object() && v.is_object() => merge(existing, v),
                _ => {
                    b.insert(k.clone(), v.clone());
                }
            }
        }
    }
}

async fn open_with(
    profile: &ConnectionProfile,
    password: Option<&str>,
) -> Result<Arc<dyn NativeSession>, AppError> {
    crate::testkit::install_crypto();
    match entry::open(ConnectTarget {
        profile,
        password,
        host_override: None,
    })
    .await?
    {
        Session::Native(s) => Ok(s),
        Session::Sql(_) => panic!("Elasticsearch opens a native session"),
    }
}

/// FakeEs over HTTP (`extra` routes first) and a session on it.
async fn served(
    es: &Arc<FakeEs>,
    extra: Vec<Route>,
    profile_extra: serde_json::Value,
) -> (FakeServer, Arc<dyn NativeSession>) {
    let mut routes = extra;
    routes.push(testing::route(es.clone()));
    let server = FakeServer::start(routes).await;
    let p = profile(&server, profile_extra);
    let s = open_with(&p, Some("changeme")).await.unwrap();
    (server, s)
}

fn table(name: &str) -> TableRef {
    TableRef {
        schema: None,
        name: name.into(),
    }
}

fn browse_req(name: &str, limit: u32, cursor: Option<String>) -> BrowseRequest {
    BrowseRequest {
        table: table(name),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit,
        offset: 0,
        cursor,
    }
}

fn row_ids(page: &crate::types::browse::BrowsePage) -> Vec<String> {
    let i = page.columns.iter().position(|c| c.name == "_id").unwrap();
    page.rows
        .iter()
        .map(|r| r[i].as_text().unwrap_or_default())
        .collect()
}

#[derive(Default)]
struct Rows {
    sets: Vec<Vec<ResultColumn>>,
    rows: Vec<Vec<Value>>,
}

impl RowSink for Rows {
    fn begin_set(&mut self, columns: Vec<ResultColumn>) {
        self.sets.push(columns);
    }
    fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        self.rows.extend(rows);
        ControlFlow::Continue(())
    }
}

#[tokio::test]
async fn session_connects_lists_pages_by_cursor_counts_and_fetches() {
    let es = fake(ES8_ROOT, 12);
    let (server, s) = served(&es, vec![], serde_json::json!({})).await;
    let caps = s.capabilities();
    assert_eq!(caps.data.paging, Paging::Cursor);
    assert_eq!(caps.data.transactions, TxMode::Batch);
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    assert_eq!(caps.session.keepalive_secs, Some(60));
    for f in [
        Feature::StructureView,
        Feature::Editor,
        Feature::EditRows,
        Feature::InsertRows,
        Feature::DeleteRows,
        Feature::ExactCount,
        Feature::ProcessList,
        Feature::KillProcess,
        Feature::CancelOnServer,
        Feature::ViewDefinition,
    ] {
        assert!(caps.has(f), "{f:?}");
    }
    assert!(!caps.has(Feature::Mcp) && !caps.has(Feature::EditWithoutKey));
    let info = s.server_info().await.unwrap();
    assert_eq!(info.version, "Elasticsearch 8.15.2");
    assert_eq!(info.current_database.as_deref(), Some("fake"));
    assert_eq!(info.current_user.as_deref(), Some("elastic"));
    s.ping().await.unwrap();
    assert_eq!(s.list_databases().await.unwrap()[0].name, "fake");
    assert!(s.list_schemas().await.unwrap().is_empty());
    let tables = s.list_tables("").await.unwrap();
    assert_eq!(tables.len(), 2);
    assert!(tables.iter().all(|t| t.schema.is_none()));

    let meta = s.table_meta(&table("items")).await.unwrap();
    assert_eq!(meta.kind, TableKind::Table);
    assert_eq!(meta.primary_key, ["_id", "_seq_no", "_primary_term"]);
    assert_eq!(meta.row_estimate, Some(12));
    let alias = s.table_meta(&table("current")).await.unwrap();
    assert_eq!(alias.kind, TableKind::View);
    assert_eq!(alias.primary_key[0], "_index");

    // Cursor pages: 5, 5, 2; the renderer sends offset 0 and the cursor.
    let p1 = s
        .browse(&meta, &browse_req("items", 5, None), CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(row_ids(&p1), ["d0", "d1", "d2", "d3", "d4"]);
    let p2 = s
        .browse(
            &meta,
            &browse_req("items", 5, p1.next_cursor.clone()),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(row_ids(&p2)[0], "d5");
    // The page shows the search that read it, at the cursor's position.
    assert!(p2.sql.contains(r#""from":5"#), "{}", p2.sql);
    let p3 = s
        .browse(
            &meta,
            &browse_req("items", 5, p2.next_cursor.clone()),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(row_ids(&p3), ["d10", "d11"]);
    assert_eq!(p3.next_cursor, None);
    assert!(p3.sql.starts_with("POST /items/_search"));

    // Sort and filter go into the search.
    let mut sorted = browse_req("items", 3, None);
    sorted.sort = vec![SortSpec {
        column: "name".into(),
        dir: SortDir::Desc,
        nulls: None,
    }];
    sorted.filter = Some(FilterGroup {
        conjunction: Conjunction::And,
        items: vec![FilterNode::Condition {
            column: "user.id".into(),
            op: FilterOp::Eq,
            values: vec![Value::Text("u1".into())],
        }],
    });
    s.browse(&meta, &sorted, CancelSlot::default())
        .await
        .unwrap();
    let search = server
        .requests()
        .into_iter()
        .rev()
        .find(|r| r.path == "/items/_search")
        .unwrap();
    let body = json::parse(&search.text()).unwrap();
    assert_eq!(
        body.get("sort").map(Json::to_text).as_deref(),
        Some(r#"[{"name.keyword":{"order":"desc","unmapped_type":"keyword"}}]"#)
    );
    assert_eq!(
        body.get("query").map(Json::to_text).as_deref(),
        Some(r#"{"bool":{"filter":[{"term":{"user.id":"u1"}}]}}"#)
    );
    assert_eq!(body.get("seq_no_primary_term"), Some(&Json::Bool(true)));

    // Counts: the estimate unless asked; exact and filtered ones with `_count`.
    let estimate = s
        .count(&meta, None, false, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((estimate.count, estimate.exact), (12, false));
    let exact = s
        .count(&meta, None, true, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((exact.count, exact.exact), (12, true));
    let filtered = s
        .count(&meta, sorted.filter.as_ref(), false, CancelSlot::default())
        .await
        .unwrap();
    assert!(filtered.exact);

    // Fetch one cell, by the row's key.
    let k = RowKey::Pk(
        [("_id".to_string(), Value::Text("d3".into()))]
            .into_iter()
            .collect(),
    );
    assert_eq!(
        s.fetch_cell(&meta, &k, "name").await.unwrap(),
        Value::Text("doc 3".into())
    );
    assert_eq!(s.fetch_cell(&meta, &k, "n").await.unwrap(), Value::Int(3));
    assert_eq!(
        s.fetch_cell(&meta, &k, "_id").await.unwrap(),
        Value::Text("d3".into())
    );
    let gone = RowKey::Pk(
        [("_id".to_string(), Value::Text("zz".into()))]
            .into_iter()
            .collect(),
    );
    assert!(matches!(
        s.fetch_cell(&meta, &gone, "n").await,
        Err(AppError::NotFound(_))
    ));

    // Every request: Basic credentials and the session's opaque id; the app's own are traced.
    let reqs = server.requests();
    let opaque = reqs[0].header("x-opaque-id").unwrap().to_string();
    assert!(opaque.starts_with("tabler-"));
    for r in &reqs {
        assert_eq!(r.header("authorization"), Some(BASIC), "{}", r.path);
        assert_eq!(r.header("x-opaque-id"), Some(opaque.as_str()));
    }
    let trace = s.drain_trace();
    assert!(trace.iter().any(|t| t.sql.starts_with("GET /_cat/indices")));
    assert!(
        trace
            .iter()
            .any(|t| t.sql.starts_with("POST /items/_search") && t.rows == Some(5))
    );
    s.close().await;
}

#[tokio::test]
async fn session_deep_cursor_pages_walk_past_the_window() {
    let es = fake(ES8_ROOT, 23);
    es.window(10);
    let (_server, s) = served(&es, vec![], serde_json::json!({})).await;
    let meta = s.table_meta(&table("items")).await.unwrap();
    let mut cursor = None;
    let mut seen = Vec::new();
    for _ in 0..5 {
        let page = s
            .browse(
                &meta,
                &browse_req("items", 5, cursor.clone()),
                CancelSlot::default(),
            )
            .await
            .unwrap();
        seen.extend(row_ids(&page));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    let expected: Vec<String> = (0..23).map(|i| format!("d{i}")).collect();
    assert_eq!(seen, expected);
    assert_eq!(es.open_pits(), 0);
    assert!(
        es.lines().iter().any(|l| l.starts_with("POST /items/_pit")),
        "past the window the pages walk a point in time"
    );
}

#[tokio::test]
async fn session_commit_reports_each_change() {
    let es = fake(ES8_ROOT, 3);
    let (server, s) = served(&es, vec![], serde_json::json!({})).await;
    let meta = s.table_meta(&table("items")).await.unwrap();
    let req = ApplyRequest {
        table: table("items"),
        changes: three_changes(),
        returning: true,
    };
    let plan = s.plan_changes(&meta, &req).unwrap();
    assert_eq!(plan.preview.len(), 3);
    assert!(
        plan.preview[0]
            .1
            .starts_with("POST /_bulk?refresh=wait_for\n")
    );
    let preview: Vec<String> = plan.preview.iter().map(|(_, t)| t.clone()).collect();
    let out = s.apply_changes(&meta, &req, plan).await;
    assert!(!out.success);
    assert_eq!(out.sql, preview, "what is applied is what was shown");
    assert!(out.results[0].error.is_none());
    assert_eq!(out.results[0].affected, 1);
    assert!(
        out.results[0]
            .returned
            .as_ref()
            .is_some_and(|r| r.contains_key("_seq_no"))
    );
    assert_eq!(
        out.results[1].error.as_ref().unwrap().vendor_code,
        Some(409)
    );
    assert!(out.results[2].error.is_none());
    assert_eq!(
        es.doc("items", "d0").unwrap().source.get("n"),
        Some(&Json::Num("100".into()))
    );
    let bulks = server
        .requests()
        .iter()
        .filter(|r| r.path == "/_bulk")
        .count();
    assert_eq!(bulks, 1, "one _bulk for the whole commit");
    // Edits of metadata and server expressions are refused before anything is sent.
    let bad = ApplyRequest {
        table: table("items"),
        changes: vec![StagedChange::Update {
            row_id: "x".into(),
            key: key("d2", 2),
            changes: [("_id".to_string(), Value::Text("new".into()))]
                .into_iter()
                .collect(),
            exprs: None,
        }],
        returning: false,
    };
    assert!(s.plan_changes(&meta, &bad).is_err());
}

#[tokio::test]
async fn session_console_runs_requests_and_sql_with_cursors() {
    let es = fake(ES8_ROOT, 12);
    let (_server, s) = served(&es, vec![], serde_json::json!({})).await;
    let text =
        "GET /items/_search\n{\"size\": 3}\n\n# the n of every document\nSELECT n FROM items\n";
    let parts = s.split_console(text);
    assert_eq!(parts.len(), 2);
    assert!(
        parts
            .iter()
            .all(|p| s.classify_console(&p.sql) == StatementClass::Read)
    );
    let mut rows = Rows::default();
    let out = s
        .run_console(&parts[0].sql, 100, &mut rows, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!((out.result_sets, out.row_count), (1, 3));
    assert_eq!(rows.sets[0][1].name, "_id");

    // A SQL statement with a cap below its rows: one page, the cursor freed.
    let mut rows = Rows::default();
    let out = s
        .run_console(&parts[1].sql, 8, &mut rows, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(out.row_count, 8);
    assert!(out.truncated);
    assert_eq!(rows.rows[7], vec![Value::Int(7)]);
    assert_eq!(es.open_sql_cursors(), 0);
    assert_eq!(es.closed_sql_cursors().len(), 1);

    // A `_sql` request pages through its cursor while the cap allows: 5 + 5 + 2 rows.
    let mut rows = Rows::default();
    let out = s
        .run_console(
            "POST /_sql?format=json\n{\"query\": \"SELECT n FROM items\", \"fetch_size\": 5}",
            100,
            &mut rows,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!((out.row_count, out.truncated), (12, false));
    assert_eq!(rows.sets.len(), 1);
    assert_eq!(rows.rows[11], vec![Value::Int(11)]);
    assert_eq!(es.open_sql_cursors(), 0);

    // Errors come back as errors; text that is no request fails without being sent.
    let mut rows = Rows::default();
    assert!(matches!(
        s.run_console("GET /nope/_mapping", 10, &mut rows, CancelSlot::default())
            .await,
        Err(AppError::NotFound(_))
    ));
    assert!(
        s.run_console("hello", 10, &mut rows, CancelSlot::default())
            .await
            .is_err()
    );
    // The console is not traced: the editor logs what the user ran.
    assert!(
        !s.drain_trace()
            .iter()
            .any(|t| t.sql.contains("_sql") || t.sql.contains("/nope"))
    );
}

#[tokio::test]
async fn sql_paging_ends_at_an_empty_page() {
    // A server that hands out a cursor with every page, also an empty one: the console stops at
    // the empty page and frees the cursor instead of asking without end.
    let es = fake(ES8_ROOT, 1);
    let pages = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = pages.clone();
    let sql = Route::new("POST", "/_sql").handler(move |_| {
        let first = seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
        json_reply(if first {
            r#"{"columns":[{"name":"a","type":"long"}],"rows":[[1]],"cursor":"c1"}"#
        } else {
            r#"{"rows":[],"cursor":"c1"}"#
        })
    });
    let (_server, s) = served(&es, vec![sql], serde_json::json!({})).await;
    let mut rows = Rows::default();
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        s.run_console("SELECT a FROM t", 100, &mut rows, CancelSlot::default()),
    )
    .await
    .expect("the run ends")
    .unwrap();
    assert_eq!((out.row_count, out.truncated), (1, false));
    assert_eq!(pages.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(es.closed_sql_cursors(), ["c1"]);
}

/// A route whose search never answers, registering a task under the request's opaque id the
/// way Elasticsearch does.
fn slow_search(es: &Arc<FakeEs>) -> Route {
    let es = es.clone();
    Route::new("POST", "/slow/_search").handler(move |req| {
        let opaque = req.header("x-opaque-id").unwrap_or_default();
        es.task(&format!(
            r#"{{"node":"n1","id":7,"action":"indices:data/read/search","cancellable":true,"headers":{{"X-Opaque-Id":"{opaque}"}}}}"#
        ));
        Reply::ok().delay(Duration::from_secs(30))
    })
}

#[tokio::test]
async fn session_cancel_stops_the_request_and_its_tasks() {
    let es = fake(ES8_ROOT, 1);
    let (server, s) = served(&es, vec![slow_search(&es)], serde_json::json!({})).await;
    let slot = CancelSlot::default();
    let run = {
        let (s, slot) = (s.clone(), slot.clone());
        tokio::spawn(async move {
            let mut rows = Rows::default();
            s.run_console("POST /slow/_search\n{}", 10, &mut rows, slot)
                .await
        })
    };
    server
        .wait_until(Duration::from_secs(5), |reqs| {
            reqs.iter().any(|r| r.path == "/slow/_search")
        })
        .await;
    let handle = request_cancel(&slot).expect("the run is armed");
    s.cancel(handle).await.unwrap();
    assert!(matches!(run.await.unwrap(), Err(AppError::Cancelled)));
    server
        .wait_until(Duration::from_secs(5), |reqs| {
            reqs.iter()
                .any(|r| r.path == "/slow/_search" && r.outcome == Outcome::Aborted)
        })
        .await;
    assert_eq!(es.cancelled(), ["n1:7"], "the run's task is cancelled too");
}

#[tokio::test]
async fn session_console_query_time_out() {
    let es = fake(ES8_ROOT, 1);
    let (server, s) = served(&es, vec![slow_search(&es)], serde_json::json!({})).await;
    let options = RunOptions {
        timeout_secs: 1,
        ..RunOptions::default()
    };
    let mut events = Vec::new();
    let results = crate::commands::console::run_console_inner(
        s.as_ref(),
        "POST /slow/_search\n{}\nGET /items/_search",
        &options,
        &CancelSlot::default(),
        &mut |e| events.push(e),
    )
    .await;
    assert_eq!(results.len(), 1, "the run stops at the timed-out command");
    let error = results[0].error.as_ref().unwrap();
    assert_eq!(error.code, crate::error::QUERY_TIMEOUT_CODE);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, QueryEvent::Failed { .. }))
    );
    server
        .wait_until(Duration::from_secs(5), |reqs| {
            reqs.iter()
                .any(|r| r.path == "/slow/_search" && r.outcome == Outcome::Aborted)
        })
        .await;
    assert_eq!(es.cancelled(), ["n1:7"]);
}

#[tokio::test]
async fn session_read_only_refuses_every_write_path() {
    let es = fake(ES8_ROOT, 3);
    es.task(r#"{"node":"n1","id":5,"action":"indices:data/write/reindex","cancellable":true}"#);
    let (server, s) = served(
        &es,
        vec![],
        serde_json::json!({"options": {"readOnly": true}}),
    )
    .await;
    let meta = s.table_meta(&table("items")).await.unwrap();
    let req = ApplyRequest {
        table: table("items"),
        changes: three_changes(),
        returning: false,
    };
    assert!(s.plan_changes(&meta, &req).is_err());
    // A plan made elsewhere is refused too.
    let plan = crate::drivers::native::NativePlan {
        preview: Vec::new(),
        ops: Box::new(()),
    };
    let out = s.apply_changes(&meta, &req, plan).await;
    assert!(!out.success && out.results.iter().all(|r| r.error.is_some()));
    let mut rows = Rows::default();
    for write in [
        "PUT /items/_doc/x\n{\"n\": 1}",
        "POST /items/_delete_by_query\n{\"query\":{\"match_all\":{}}}",
        "GET /items/_flush",
        "POST /_sql\n{\"query\": \"DELETE FROM items\"}",
        "nonsense",
        // What a decoding reverse proxy would turn into a write.
        "GET /x%2F..%2F_flush",
        "GET /items%2F_flush",
        "POST /items/_explain/x%2F_delete_by_query\n{\"query\":{\"match_all\":{}}}",
        "GET /items/_doc/d1%3Frefresh=true",
        // A query in base64 that calls a model (`{"semantic":…}`), in a search and a msearch.
        "GET /items/_search
{\"query\":{\"wrapper\":{\"query\":\"eyJzZW1hbnRpYyI6eyJmaWVsZCI6ImJvZHkiLCJxdWVyeSI6ImhlbGxvIn19\"}}}",
        "POST /_msearch
{\"index\":\"items\"}
{\"query\":{\"wrapper\":{\"query\":\"e30=\"}}}",
    ] {
        assert!(
            s.run_console(write, 10, &mut rows, CancelSlot::default())
                .await
                .is_err(),
            "{write}"
        );
    }
    // The grid refuses the same query as a raw filter.
    let mut wrapped = browse_req("items", 5, None);
    wrapped.filter = Some(FilterGroup {
        conjunction: Conjunction::And,
        items: vec![FilterNode::Raw {
            sql: r#"{"wrapper":{"query":"eyJzZW1hbnRpYyI6eyJmaWVsZCI6ImJvZHkiLCJxdWVyeSI6ImhlbGxvIn19"}}"#
                .into(),
        }],
    });
    assert!(
        s.browse(&meta, &wrapped, CancelSlot::default())
            .await
            .is_err()
    );
    for call in ["deleteIndex", "truncate", "indexAction", "createIndex", "x"] {
        assert_eq!(s.call_kind(call), CallKind::Write);
        assert!(
            s.call(
                call,
                serde_json::json!({"index": "items", "name": "n", "action": "refresh"})
            )
            .await
            .is_err(),
            "{call}"
        );
    }
    let killed = s
        .kill_processes(&["n1:5".to_string()], crate::types::admin::KillScope::Query)
        .await
        .unwrap();
    assert!(!killed[0].ok);
    // Nothing that writes reached the server.
    for r in server.requests() {
        assert!(
            !(r.path.contains("_bulk")
                || r.path.contains("_doc")
                || r.path.contains("_delete_by_query")
                || r.path.contains("_flush")
                || r.path.contains("_cancel")
                || r.text().contains("wrapper")
                || r.method == "PUT"
                || r.method == "DELETE"),
            "{} {}",
            r.method,
            r.path
        );
    }
    assert!(es.cancelled().is_empty());
    // Reads still work.
    s.run_console("GET /items/_search", 10, &mut rows, CancelSlot::default())
        .await
        .unwrap();
    assert!(
        s.call(
            "document",
            serde_json::json!({"index": "items", "id": "d1"})
        )
        .await
        .unwrap()["found"]
            .as_bool()
            .unwrap()
    );
}

#[tokio::test]
async fn session_lists_and_cancels_tasks() {
    let es = fake(ES8_ROOT, 1);
    es.task(r#"{"node":"n1","id":5,"action":"indices:data/write/reindex","description":"reindex [a] to [b]","start_time_in_millis":1700000000000,"running_time_in_nanos":2000000,"cancellable":true,"headers":{"X-Opaque-Id":"someone"}}"#);
    let (server, s) = served(&es, vec![], serde_json::json!({})).await;
    let rows = s.list_processes().await.unwrap().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "n1:5");
    assert_eq!(rows[0].query.as_deref(), Some("reindex [a] to [b]"));
    assert!(!rows[0].own);
    let out = s
        .kill_processes(
            &[
                "n1:5".to_string(),
                "n1:6".to_string(),
                "../_bulk:1".to_string(),
            ],
            crate::types::admin::KillScope::Connection,
        )
        .await
        .unwrap();
    assert_eq!(
        out.iter().map(|o| o.ok).collect::<Vec<_>>(),
        [true, false, false]
    );
    assert_eq!(es.cancelled(), ["n1:5"]);
    // The malformed id never reached the server.
    assert!(!server.requests().iter().any(|r| r.path.contains("_bulk")));
}

#[tokio::test]
async fn session_definitions_and_calls() {
    let es = fake(ES8_ROOT, 2);
    let (_server, s) = served(&es, vec![], serde_json::json!({})).await;
    let index = s.definition(&table("items")).await.unwrap().unwrap();
    assert!(index.starts_with("PUT /items\n{"), "{index}");
    assert!(index.contains("\"index.number_of_shards\": \"1\""));
    assert!(!index.contains("index.uuid"));
    let alias = s.definition(&table("current")).await.unwrap().unwrap();
    assert!(alias.starts_with("POST /_aliases\n"), "{alias}");
    let doc = s
        .call(
            "document",
            serde_json::json!({"index": "items", "id": "d1"}),
        )
        .await
        .unwrap();
    assert!(
        doc["source"]
            .as_str()
            .unwrap()
            .contains("\"name\": \"doc 1\"")
    );
    assert!(s.call("bogus", serde_json::json!({})).await.is_err());
    let r = s
        .call("truncate", serde_json::json!({"index": "items"}))
        .await;
    // The fake has no `_delete_by_query`: the request went out and its error came back.
    assert!(r.is_err());

    // A data stream: its backing indices' mappings, no alias of that name, then `_data_stream`.
    let backing = r#"{".ds-logs-app-000001":{"mappings":{}}}"#;
    es.fail_once("GET /logs-app/_mapping", HttpResponse::json(200, backing));
    es.fail_once(
        "GET /_data_stream/logs-app",
        HttpResponse::json(
            200,
            r#"{"data_streams":[{"name":"logs-app","template":"logs"}]}"#,
        ),
    );
    let stream = s.definition(&table("logs-app")).await.unwrap().unwrap();
    assert!(
        stream.starts_with("# GET /_data_stream/logs-app\n{"),
        "{stream}"
    );
    // Only "no such stream" means no definition: any other failure comes back.
    es.fail_once("GET /logs-app/_mapping", HttpResponse::json(200, backing));
    es.fail_once(
        "GET /_data_stream/logs-app",
        error(400, "task_cancelled_exception", "cancelled"),
    );
    assert!(matches!(
        s.definition(&table("logs-app")).await,
        Err(AppError::Cancelled)
    ));
    es.fail_once("GET /logs-app/_mapping", HttpResponse::json(200, backing));
    assert_eq!(s.definition(&table("logs-app")).await.unwrap(), None);
}

#[tokio::test]
async fn object_operations_take_one_index() {
    // Deleting, truncating, closing or creating takes one concrete name, never a pattern, a
    // list or a remote index that could reach every index.
    let es = fake(ES8_ROOT, 1);
    let (server, s) = served(&es, vec![], serde_json::json!({})).await;
    let before = server.requests().len();
    for (call, args) in [
        ("deleteIndex", serde_json::json!({"index": "*"})),
        ("deleteIndex", serde_json::json!({"index": "items,current"})),
        ("truncate", serde_json::json!({"index": "logs-*"})),
        (
            "indexAction",
            serde_json::json!({"index": "*", "action": "close"}),
        ),
        ("createIndex", serde_json::json!({"name": "remote:x"})),
        ("deleteIndex", serde_json::json!({"index": "_all"})),
    ] {
        assert!(
            matches!(
                s.call(call, args.clone()).await,
                Err(AppError::InvalidInput(_))
            ),
            "{call} {args}"
        );
    }
    assert_eq!(server.requests().len(), before);
    // Reads still take patterns.
    s.call("indexSettings", serde_json::json!({"index": "item*"}))
        .await
        .unwrap();
    let r = s
        .call(
            "indexAction",
            serde_json::json!({"index": "items", "action": "refresh"}),
        )
        .await;
    // The fake has no `_refresh`: the request went out for the one index.
    assert!(r.is_err());
    assert!(
        server
            .requests()
            .iter()
            .any(|r| r.method == "POST" && r.path == "/items/_refresh")
    );
}

#[tokio::test]
async fn names_that_are_not_index_names_never_reach_the_server() {
    let es = fake(ES8_ROOT, 2);
    let (server, s) = served(&es, vec![], serde_json::json!({})).await;
    let before = server.requests().len();
    for name in [
        "", ".", "..", "../_bulk", "a/b", "_cluster", "-x", "a b", "a?b", "a#b",
    ] {
        assert!(check_target(name).is_err(), "{name}");
        assert!(s.table_meta(&table(name)).await.is_err(), "{name}");
        assert!(s.definition(&table(name)).await.is_err(), "{name}");
        assert!(
            s.call("indexSettings", serde_json::json!({"index": name}))
                .await
                .is_err(),
            "{name}"
        );
    }
    assert_eq!(server.requests().len(), before);
    for ok in ["logs", "logs-*", ".kibana", "remote:logs", "a,b"] {
        assert!(check_target(ok).is_ok(), "{ok}");
    }
    // A meta whose name was forged after it was read is refused before the search.
    let mut meta: TableMeta = s.table_meta(&table("items")).await.unwrap();
    meta.table.name = "../_bulk".into();
    assert!(
        s.browse(&meta, &browse_req("items", 5, None), CancelSlot::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn open_refuses_and_fails_over() {
    crate::testkit::install_crypto();
    let es = fake(ES8_ROOT, 1);
    let server = FakeServer::start(vec![testing::route(es.clone())]).await;
    // The first URL refuses connections; the second answers.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_port = dead.local_addr().unwrap().port();
    drop(dead);
    let p = profile(
        &server,
        serde_json::json!({"host": null, "options": {"driverParams": {"urls": [
            format!("http://127.0.0.1:{dead_port}"),
            server.url(""),
        ]}}}),
    );
    entry::validate(&p).unwrap();
    let s = open_with(&p, Some("pw")).await.unwrap();
    assert_eq!(
        s.server_info().await.unwrap().version,
        "Elasticsearch 8.15.2"
    );
    // An API key or bearer token without a secret is refused before anything is sent.
    for auth in ["apiKey", "bearer"] {
        let p = profile(
            &server,
            serde_json::json!({"options": {"driverParams": {"auth": auth}}}),
        );
        assert!(matches!(
            open_with(&p, None).await,
            Err(AppError::InvalidInput(_))
        ));
    }
    // Nothing to connect to.
    let p = profile(&server, serde_json::json!({"host": null}));
    assert!(entry::validate(&p).is_err());
}

#[tokio::test]
async fn open_fails_over_from_a_node_that_never_answers() {
    crate::testkit::install_crypto();
    let es = fake(ES8_ROOT, 1);
    let server = FakeServer::start(vec![testing::route(es.clone())]).await;
    // The first URL takes the connection (the listen backlog) and never replies.
    let hung = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let hung_port = hung.local_addr().unwrap().port();
    let p = profile(
        &server,
        serde_json::json!({"host": null, "options": {
            "connectTimeoutSecs": 4,
            "driverParams": {"urls": [format!("http://127.0.0.1:{hung_port}"), server.url("")]},
        }}),
    );
    let started = std::time::Instant::now();
    let s = open_with(&p, Some("pw")).await.unwrap();
    // Half the connect time out went to the hung node, the rest was left for the next.
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(
        s.server_info().await.unwrap().version,
        "Elasticsearch 8.15.2"
    );
    drop(hung);
}

#[tokio::test]
async fn an_endpoint_under_the_url_path_is_refused_before_anything_is_sent() {
    let es = fake(ES8_ROOT, 1);
    let server = FakeServer::start(vec![testing::route(es.clone())]).await;
    let port = server.addr().port();
    for extra in [
        serde_json::json!({"host": null, "options": {"readOnly": true, "driverParams": {
            "urls": [format!("http://127.0.0.1:{port}/_refresh")]}}}),
        serde_json::json!({"host": "127.0.0.1/_refresh", "options": {"readOnly": true}}),
        serde_json::json!({"host": "127.0.0.1/es/_flush"}),
        serde_json::json!({"options": {"driverParams": {"pathPrefix": "/_refresh"}}}),
    ] {
        let p = profile(&server, extra.clone());
        assert!(entry::validate(&p).is_err(), "{extra}");
        assert!(open_with(&p, Some("pw")).await.is_err(), "{extra}");
    }
    assert!(server.requests().is_empty());
    // A plain prefix still serves.
    let p = profile(
        &server,
        serde_json::json!({"host": null, "options": {"driverParams": {
            "urls": [format!("http://127.0.0.1:{port}/es/")]}}}),
    );
    entry::validate(&p).unwrap();
}

#[tokio::test]
async fn api_keys_and_bearer_tokens_go_in_the_authorization_header() {
    let es = fake(ES8_ROOT, 1);
    let server = FakeServer::start(vec![testing::route(es.clone())]).await;
    let cases = [
        (
            serde_json::json!({"auth": "apiKey"}),
            "ZW5jb2RlZA==",
            "ApiKey ZW5jb2RlZA==".to_string(),
        ),
        (
            serde_json::json!({"auth": "apiKey", "apiKeyId": "VuaCfGcBCdbkQm-e5aOx"}),
            "ui2lp2axTNmsyakw9tvNnw",
            "ApiKey VnVhQ2ZHY0JDZGJrUW0tZTVhT3g6dWkybHAyYXhUTm1zeWFrdzl0dk5udw==".to_string(),
        ),
        (
            serde_json::json!({"auth": "bearer"}),
            " t0k ",
            "Bearer t0k".to_string(),
        ),
        (
            serde_json::json!({"auth": "none"}),
            "ignored",
            String::new(),
        ),
    ];
    for (params, secret, expected) in cases {
        let p = profile(
            &server,
            serde_json::json!({"options": {"driverParams": params}}),
        );
        open_with(&p, Some(secret)).await.unwrap();
        let last = server.requests().pop().unwrap();
        assert_eq!(
            last.header("authorization").unwrap_or_default(),
            expected,
            "{params}"
        );
    }
    // A Command Line API key with an id: what the command printed for the connect, encoded.
    let p = profile(
        &server,
        serde_json::json!({"options": {
            "passwordMode": "command", "passwordCommand": "print-key",
            "driverParams": {"auth": "apiKey", "apiKeyId": "id1"}
        }}),
    );
    open_with(&p, Some("k1")).await.unwrap();
    assert_eq!(
        server.requests().pop().unwrap().header("authorization"),
        Some(format!("ApiKey {}", super::auth::encode_api_key("id1", "k1")).as_str())
    );
}

#[tokio::test]
async fn sigv4_sessions_sign_for_the_region_and_service() {
    let es = fake(OS2_ROOT, 1);
    let server = FakeServer::start(vec![testing::route(es.clone())]).await;
    let p = profile(
        &server,
        serde_json::json!({"user": "AKIDEXAMPLE", "options": {"driverParams": {
            "auth": "awsSigv4", "awsRegion": "us-east-1", "awsService": "es"
        }}}),
    );
    let extras = {
        let mut e = crate::secrets::ExtraSecrets::default();
        e.set(
            crate::types::driver::ExtraSecret::SessionToken,
            "session-tok".into(),
        );
        e
    };
    let s = extras.scope(open_with(&p, Some("secret"))).await.unwrap();
    assert_eq!(s.server_info().await.unwrap().version, "OpenSearch 2.17.1");
    assert_eq!(s.server_info().await.unwrap().current_user, None);
    let r = server.requests().pop().unwrap();
    let a = r.header("authorization").unwrap();
    assert!(a.contains("/us-east-1/es/aws4_request"), "{a}");
    assert_eq!(r.header("x-amz-security-token"), Some("session-tok"));
    assert!(r.header("x-amz-content-sha256").is_some());
    // Without a region (the host does not tell it) the profile is refused.
    let p = profile(
        &server,
        serde_json::json!({"options": {"driverParams": {"auth": "awsSigv4"}}}),
    );
    assert!(entry::validate(&p).is_err());
}

#[tokio::test]
async fn opensearch_serverless_is_refused_before_anything_is_sent() {
    // A collection has no `GET /`, scroll, `_tasks` or index actions: refused by the service
    // SigV4 would sign for, or by its endpoint host, whatever the auth.
    let es = fake(OS2_ROOT, 1);
    let server = FakeServer::start(vec![testing::route(es.clone())]).await;
    let by_service = profile(
        &server,
        serde_json::json!({"user": "AKID", "options": {"driverParams": {
            "auth": "awsSigv4", "awsRegion": "us-east-1", "awsService": "aoss"
        }}}),
    );
    let by_host = profile(
        &server,
        serde_json::json!({"host": null, "ssl": "require", "options": {"driverParams": {
            "auth": "basic", "urls": ["https://abc123.us-east-1.aoss.amazonaws.com"]
        }}}),
    );
    for p in [&by_service, &by_host] {
        assert!(matches!(
            entry::validate(p),
            Err(AppError::Unsupported(m)) if m.contains("Serverless")
        ));
        assert!(matches!(
            open_with(p, Some("secret")).await,
            Err(AppError::Unsupported(_))
        ));
    }
    assert!(server.requests().is_empty());
    // The form still learns what an aoss host is.
    let p = profile(&server, serde_json::json!({}));
    let aws = entry::util(
        UtilInput {
            profile: &p,
            secret: None,
        },
        "awsEndpoint",
        serde_json::json!({"host": "abc123.us-east-1.aoss.amazonaws.com"}),
    )
    .await
    .unwrap();
    assert_eq!(aws["service"], "aoss");
}

#[tokio::test]
async fn elasticsearch_serverless_has_no_running_queries() {
    // Serverless has no task management API: no Process List, and a cancel only drops the
    // request.
    let es = fake(
        r#"{"name":"serverless","cluster_name":"abc","cluster_uuid":"_na_","version":{"number":"8.11.0","build_flavor":"serverless","build_type":"docker"}}"#,
        1,
    );
    let (server, s) = served(&es, vec![slow_search(&es)], serde_json::json!({})).await;
    let caps = s.capabilities();
    assert!(!caps.has(Feature::ProcessList) && !caps.has(Feature::KillProcess));
    assert!(caps.has(Feature::EditRows));
    assert!(s.list_processes().await.is_none());
    assert!(
        s.kill_processes(&["n1:1".to_string()], crate::types::admin::KillScope::Query)
            .await
            .is_none()
    );
    let slot = CancelSlot::default();
    let run = {
        let (s, slot) = (s.clone(), slot.clone());
        tokio::spawn(async move {
            let mut rows = Rows::default();
            s.run_console("POST /slow/_search\n{}", 10, &mut rows, slot)
                .await
        })
    };
    server
        .wait_until(Duration::from_secs(5), |reqs| {
            reqs.iter().any(|r| r.path == "/slow/_search")
        })
        .await;
    s.cancel(request_cancel(&slot).expect("the run is armed"))
        .await
        .unwrap();
    assert!(matches!(run.await.unwrap(), Err(AppError::Cancelled)));
    assert!(
        !server
            .requests()
            .iter()
            .any(|r| r.path.starts_with("/_tasks"))
    );
}

#[tokio::test]
async fn tls_checks_the_ca_and_the_pin() {
    use crate::testkit::tls::{CA_PEM, TlsServer, ca_file, cert_der};
    crate::testkit::install_crypto();
    let server = TlsServer::start();
    let ca = ca_file();
    let base = serde_json::json!({
        "id": "00000000-0000-0000-0000-000000000002", "name": "tls", "driver": "elasticsearch",
        "host": "localhost", "port": server.addr().port(), "ssl": "verify-full",
        "sslCaPath": ca.path().to_string_lossy(),
        "options": {}, "sortOrder": 0, "createdAt": "", "updatedAt": "",
    });
    // The CA verifies the chain and the name: the request arrives (the reply is not JSON).
    let p: ConnectionProfile = serde_json::from_value(base.clone()).unwrap();
    assert!(open_with(&p, None).await.is_err());
    let seen = server.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].sni.as_deref(), Some("localhost"));
    assert_eq!(seen[0].line, "GET / HTTP/1.1");
    // Without the CA, verify-full refuses the certificate: nothing arrives.
    let mut no_ca = base.clone();
    no_ca["sslCaPath"] = serde_json::Value::Null;
    let p: ConnectionProfile = serde_json::from_value(no_ca).unwrap();
    assert!(matches!(
        open_with(&p, None).await,
        Err(AppError::Connect(_))
    ));
    assert_eq!(server.requests().len(), 1);
    // A pin of the CA certificate is enough, in any SSL mode; a wrong pin refuses.
    let pin = {
        let der = cert_der(CA_PEM);
        let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, der.as_ref());
        hex::encode(digest.as_ref())
    };
    for (fp, arrives) in [(pin.as_str(), true), (&"00".repeat(32)[..], false)] {
        let mut pinned = base.clone();
        pinned["sslCaPath"] = serde_json::Value::Null;
        pinned["ssl"] = "require".into();
        pinned["options"] = serde_json::json!({"driverParams": {"caFingerprint": fp}});
        let p: ConnectionProfile = serde_json::from_value(pinned).unwrap();
        let before = server.requests().len();
        let _ = open_with(&p, None).await;
        assert_eq!(server.requests().len() > before, arrives, "{fp}");
    }
}

#[tokio::test]
async fn util_calls_and_validation() {
    let p = serde_json::from_value::<ConnectionProfile>(serde_json::json!({
        "id": "00000000-0000-0000-0000-000000000003", "name": "u", "driver": "elasticsearch",
        "host": "localhost", "ssl": "disable", "options": {}, "sortOrder": 0,
        "createdAt": "", "updatedAt": "",
    }))
    .unwrap();
    let input = || UtilInput {
        profile: &p,
        secret: None,
    };
    use base64::Engine as _;
    let id = format!(
        "dep:{}",
        base64::engine::general_purpose::STANDARD.encode("eu-west-1.aws.found.io$abc$kb")
    );
    let url = entry::util(input(), "cloudIdUrl", serde_json::json!({"cloudId": id}))
        .await
        .unwrap();
    assert_eq!(url["url"], "https://abc.eu-west-1.aws.found.io");
    let aws = entry::util(
        input(),
        "awsEndpoint",
        serde_json::json!({"host": "https://search-x.eu-central-1.es.amazonaws.com"}),
    )
    .await
    .unwrap();
    assert_eq!(aws["region"], "eu-central-1");
    assert_eq!(aws["service"], "es");
    assert!(
        entry::util(input(), "nope", serde_json::json!({}))
            .await
            .is_err()
    );
    assert_eq!(entry::util_kind("cloudIdUrl"), CallKind::Read);
    assert_eq!(entry::util_kind("awsEndpoint"), CallKind::Read);
    assert_eq!(entry::util_kind("anything"), CallKind::Write);
    entry::validate(&p).unwrap();
    // Over SSH the cluster is the profile's host and port.
    let mut ssh = serde_json::to_value(&p).unwrap();
    ssh["ssh"] = serde_json::json!({"host": "bastion", "port": 22, "user": "u", "auth": "agent"});
    let with_ssh: ConnectionProfile = serde_json::from_value(ssh.clone()).unwrap();
    entry::validate(&with_ssh).unwrap();
    ssh["options"] = serde_json::json!({"driverParams": {"urls": ["http://other:9200"]}});
    let elsewhere: ConnectionProfile = serde_json::from_value(ssh).unwrap();
    assert!(entry::validate(&elsewhere).is_err());
}

#[tokio::test]
async fn plain_http_never_wins_over_tls() {
    // An `http://` URL with an SSL mode or a CA pin that asks for TLS is refused: the
    // credentials would go in clear text and nothing would be checked.
    let es = fake(ES8_ROOT, 1);
    let server = FakeServer::start(vec![testing::route(es.clone())]).await;
    let pin = "AA:".repeat(31) + "AA";
    let url = server.url("");
    for (ssl, params, ok) in [
        ("verify-full", serde_json::json!({"urls": [&url]}), false),
        ("verify-ca", serde_json::json!({"urls": [&url]}), false),
        ("require", serde_json::json!({"urls": [&url]}), false),
        (
            "prefer",
            serde_json::json!({"urls": [&url], "caFingerprint": &pin}),
            false,
        ),
        (
            "disable",
            serde_json::json!({"urls": [&url], "caFingerprint": &pin}),
            false,
        ),
        ("prefer", serde_json::json!({"urls": [&url]}), true),
        ("disable", serde_json::json!({"urls": [&url]}), true),
        // The host field: plain only when it says so.
        ("require", serde_json::json!({}), true),
        ("disable", serde_json::json!({}), true),
    ] {
        let p = profile(
            &server,
            serde_json::json!({"ssl": ssl, "options": {"driverParams": params}}),
        );
        assert_eq!(entry::validate(&p).is_ok(), ok, "{ssl} {params}");
        if !ok {
            assert!(matches!(
                open_with(&p, Some("pw")).await,
                Err(AppError::InvalidInput(m)) if m.contains("plain HTTP")
            ));
        }
    }
    let p = profile(
        &server,
        serde_json::json!({"ssl": "verify-full", "host": format!("http://127.0.0.1:{}", server.addr().port())}),
    );
    assert!(entry::validate(&p).is_err());
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn the_session_is_built_from_the_engine_flavor() {
    // OpenSearch has no `_shard_doc`: its deep pages scroll; Elasticsearch 6.x has no guards
    // before 6.7, so it cannot edit or delete.
    let es = fake(OS2_ROOT, 1);
    let (_server, s) = served(&es, vec![], serde_json::json!({})).await;
    assert!(s.capabilities().has(Feature::EditRows));
    let old = fake(
        r#"{"name":"n","cluster_name":"old","version":{"number":"6.5.4","lucene_version":"7.5.0"}}"#,
        1,
    );
    let (_server, s) = served(&old, vec![], serde_json::json!({})).await;
    let caps = s.capabilities();
    assert!(caps.has(Feature::InsertRows) && !caps.has(Feature::EditRows));
    assert!(!caps.data.filter_ops.contains(&FilterOp::ContainsCase));
}

#[test]
fn a_huge_key_over_many_children_stays_linear() {
    // A `dynamic: false` object needs no mapping for its keys, so a real cluster can return
    // this; the walks must not hash the long key once per child (minutes, past Stop).
    let long = "k".repeat(256 * 1024);
    let mut children = String::new();
    for i in 0..20_000 {
        children.push_str(&format!("\"c{i}\":1,"));
    }
    children.pop();
    let medium = "m".repeat(900);
    let body = format!(
        r#"{{"hits":{{"hits":[{{"_index":"i","_id":"1","_source":{{"{long}":{{{children}}},"{medium}":{{{children}}},"a":1}}}}]}}}}"#
    );
    let reply = json::parse(&body).unwrap();
    let req = parse_request("POST /i/_search").unwrap();
    let started = std::time::Instant::now();
    let grids = super::reply::grids(&req, &reply, 10);
    let g = &grids[0];
    // The long key stays in the JSON view, the medium one fills the cap, `a` is past it.
    // `_index`, `_id`, `_score`, then the leaves.
    assert_eq!(g.columns.len(), 3 + super::reply::MAX_COLUMNS);
    assert_eq!(g.columns[3].name, format!("{medium}.c0"));
    assert!(g.notices.iter().any(|n| n.contains("JSON view")));
    let source = reply
        .get("hits")
        .and_then(|h| h.get("hits"))
        .and_then(|h| h.as_array())
        .and_then(|h| h.first())
        .and_then(|h| h.get("_source"))
        .unwrap();
    let column = format!("{long}.c19999");
    let paths = super::values::Paths::new([column.as_str(), "a"]);
    let found = paths.locate(source);
    assert_eq!(found.value(&column, None), Value::Int(1));
    assert_eq!(found.value("a", None), Value::Int(1));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

/// The cases the renderer's mirror of the splitter and classifier runs too
/// (`src/features/drivers/elasticsearch/console.test.ts`): both must agree on every one.
#[test]
fn shared_console_cases() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/elasticsearch/console.cases.json"
    ))
    .expect("the case file is JSON");
    let classify = cases["classify"].as_array().expect("classify cases");
    assert!(classify.len() > 250);
    for c in classify {
        let text = c["sql"].as_str().expect("sql");
        let want = match c["class"].as_str() {
            Some("read") => StatementClass::Read,
            _ => StatementClass::Write,
        };
        assert_eq!(super::classify::classify_text(text), want, "{text:?}");
    }
    for c in cases["split"].as_array().expect("split cases") {
        let text = c["sql"].as_str().expect("sql");
        let want: Vec<&str> = c["statements"]
            .as_array()
            .expect("statements")
            .iter()
            .filter_map(|s| s.as_str())
            .collect();
        let got: Vec<String> = super::console::split(text)
            .into_iter()
            .map(|s| s.sql)
            .collect();
        assert_eq!(got, want, "{text:?}");
    }
}
