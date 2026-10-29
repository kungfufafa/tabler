//! The client end to end: against `FakeEs` behind the transport seam (connect, list, browse with
//! `from`/`size`, a point in time and a scroll, count, fetch, a commit with one failing change,
//! console runs), and over HTTP against `testkit::fake_http` replaying recorded replies (`/`,
//! `_cat/indices`, `_mapping`, point-in-time pages, a `_bulk` partial failure), which also
//! checks the wire: credentials, content types, NDJSON bodies, cancel by dropping a request,
//! timeouts.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use super::auth::Auth;
use super::catalog::{self, ColumnsFor};
use super::client::EsClient;
use super::console::parse_request;
use super::info::Features;
use super::json::{self, Json};
use super::testing::{ES8_ROOT, FakeEs, OS2_ROOT, ReqwestTransport, error};
use super::transport::{BaseUrl, HttpResponse};
use super::values::hit_row;
use crate::error::AppError;
use crate::sqlgen::native::elastic::{self as build, BrowseSpec, EditTarget};
use crate::testkit::{FakeServer, Outcome, Reply, Route};
use crate::types::browse::{Conjunction, FilterGroup, FilterNode, FilterOp, RowKey};
use crate::types::mutate::StagedChange;
use crate::types::schema::TableKind;
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
        BaseUrl::parse("http://es.test:9200").unwrap(),
        Auth::ApiKey("a2V5".into()),
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

#[tokio::test]
async fn connect_list_and_describe() {
    let es = fake(ES8_ROOT, 3);
    let (c, f) = connect(&es).await;
    let tables = c.tables("fake", false, f).await.unwrap();
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
    // Every request carried the key and the opaque id.
    for r in es.requests() {
        assert_eq!(r.header("authorization"), Some("ApiKey a2V5"));
        assert_eq!(r.header("x-opaque-id"), Some("tabler-1"));
    }
}

#[tokio::test]
async fn shallow_pages_use_from_and_size() {
    let es = fake(ES8_ROOT, 12);
    let (c, f) = connect(&es).await;
    let page = c
        .browse(&spec("items"), f, 5, 5, 10_000, None, "tabler-1")
        .await
        .unwrap();
    let ids: Vec<&str> = page
        .hits
        .iter()
        .filter_map(|h| h.get("_id")?.as_str())
        .collect();
    assert_eq!(ids, ["d5", "d6", "d7", "d8", "d9"]);
    assert_eq!(page.next_cursor, None);
    assert_eq!(
        es.lines().last().unwrap(),
        "POST /items/_search?preference=tabler-1"
    );
}

#[tokio::test]
async fn deep_pages_walk_a_point_in_time_and_continue_by_cursor() {
    let es = fake(ES8_ROOT, 23);
    let (c, f) = connect(&es).await;
    let s = spec("items");
    // A window of 10 makes offset 10 a deep page: open a PIT, skip 10 rows without _source.
    let p1 = c.browse(&s, f, 10, 5, 10, None, "p").await.unwrap();
    let ids: Vec<&str> = p1
        .hits
        .iter()
        .filter_map(|h| h.get("_id")?.as_str())
        .collect();
    assert_eq!(ids, ["d10", "d11", "d12", "d13", "d14"]);
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
    let p2 = c
        .browse(&s, f, 15, 5, 10, Some(&cursor), "p")
        .await
        .unwrap();
    assert_eq!(es.requests().len(), before + 1);
    assert_eq!(p2.hits[0].get("_id").and_then(Json::as_str), Some("d15"));
    let p3 = c
        .browse(&s, f, 20, 5, 10, p2.next_cursor.as_deref(), "p")
        .await
        .unwrap();
    assert_eq!(p3.hits.len(), 3);
    assert_eq!(p3.next_cursor, None);
    assert_eq!(es.open_pits(), 0, "the last page closes the point in time");
    // A page past the end returns nothing and leaves nothing open.
    let past = c.browse(&s, f, 40, 5, 10, None, "p").await.unwrap();
    assert!(past.hits.is_empty());
    assert_eq!(es.open_pits(), 0);
}

#[tokio::test]
async fn opensearch_deep_pages_scroll() {
    let es = fake(OS2_ROOT, 13);
    let (c, f) = connect(&es).await;
    assert!(!f.shard_doc);
    let s = spec("items");
    let p = c.browse(&s, f, 10, 5, 10, None, "p").await.unwrap();
    let ids: Vec<&str> = p
        .hits
        .iter()
        .filter_map(|h| h.get("_id")?.as_str())
        .collect();
    assert_eq!(ids, ["d10", "d11", "d12"]);
    assert_eq!(p.next_cursor, None);
    assert_eq!(es.open_scrolls(), 0, "the last batch clears the scroll");
    let es = fake(OS2_ROOT, 30);
    let (c, f) = connect(&es).await;
    let p = c.browse(&s, f, 10, 5, 10, None, "p").await.unwrap();
    assert_eq!(p.hits[0].get("_id").and_then(Json::as_str), Some("d10"));
    let next = c
        .browse(&s, f, 15, 5, 10, p.next_cursor.as_deref(), "p")
        .await
        .unwrap();
    assert_eq!(next.hits[0].get("_id").and_then(Json::as_str), Some("d15"));
    assert_eq!(
        es.lines().last().unwrap(),
        "POST /_search/scroll",
        "a continued scroll is one request"
    );
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
    let changes = vec![
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
        // Read with seq_no 0, but d1 is at 1: someone changed it.
        StagedChange::Delete {
            row_id: "2".into(),
            key: key("d1", 0),
        },
        StagedChange::Insert {
            row_id: "3".into(),
            values: [("n".to_string(), Value::Int(7))].into_iter().collect(),
            exprs: None,
        },
    ];
    let plan = build::plan_edits(&t, &changes).unwrap();
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

fn http_client(server: &FakeServer, auth: Auth, timeout: Option<Duration>) -> EsClient {
    EsClient::new(
        Arc::new(ReqwestTransport::new(timeout)),
        BaseUrl::parse(&server.url("")).unwrap(),
        auth,
        "tabler-7",
    )
}

fn basic() -> Auth {
    Auth::Basic {
        user: "elastic".into(),
        password: "changeme".into(),
    }
}

#[tokio::test]
async fn wire_replay_connect_list_browse_and_commit() {
    let server = replay_server().await;
    let c = http_client(&server, basic(), None);
    let info = c.info().await.unwrap();
    assert_eq!(info.display_version(), "Elasticsearch 8.15.2");
    assert_eq!(info.cluster_name, "docker-cluster");
    let f = info.features();
    let tables = c.tables(&info.cluster_name, false, f).await.unwrap();
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
    let rows: Vec<Vec<Value>> = p1.hits.iter().map(|h| hit_row(h, &cols)).collect();
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
        .browse(&s, f, 2, 2, 1, p1.next_cursor.as_deref(), "p")
        .await
        .unwrap();
    assert_eq!(p2.hits.len(), 1);
    assert_eq!(p2.next_cursor, None);
    let row = hit_row(&p2.hits[0], &cols);
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
    let c = http_client(&server, Auth::None, None);
    assert!(matches!(c.info().await, Err(AppError::Auth(_))));
    let wrong = http_client(&server, Auth::Bearer("x".into()), None);
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
async fn wire_opensearch_and_sigv4() {
    let server = FakeServer::start(vec![
        Route::new("GET", "/").reply(json_reply(fixture("root-os2"))),
    ])
    .await;
    let c = http_client(
        &server,
        Auth::SigV4 {
            credentials: super::sigv4::AwsCredentials {
                access_key_id: "AKIDEXAMPLE".into(),
                secret_access_key: "secret".into(),
                session_token: Some("tok".into()),
            },
            region: "eu-west-1".into(),
            service: super::auth::AwsService::Es,
        },
        None,
    );
    let info = c.info().await.unwrap();
    assert_eq!(info.display_version(), "OpenSearch 2.17.1");
    let r = &server.requests()[0];
    let a = r.header("authorization").unwrap();
    assert!(
        a.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"),
        "{a}"
    );
    assert!(a.contains("/eu-west-1/es/aws4_request"));
    assert!(a.contains(
        "SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token;x-opaque-id"
    ));
    assert_eq!(r.header("x-amz-security-token"), Some("tok"));
    assert!(r.header("x-amz-date").is_some());
}
