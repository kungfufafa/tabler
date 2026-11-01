//! The client against the scripted fake (`testing::FakeBigquery`) and, for the wire, against
//! `testkit::fake_http` through the shared HTTP client: runs and their polls, pages, cancels,
//! token exchanges and refreshes, errors, listings, free table reads, dry runs and the process
//! list.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::auth::{SCOPE_BIGQUERY, issued_tokens, parse_credential_file, token_endpoint};
use super::catalog::{self, schemas, table_summaries};
use super::client::{Client, DataPage, JobCancel};
use super::decode::{FieldSchema, decode_rows};
use super::jobs::{DatasetReference, JobReference, QueryOptions, new_job_id};
use super::params::query_parameters;
use super::testing::{
    FakeBigquery, FakeReply, error_reply, job_inserted, results, rows, sa_key_json, schema,
    still_running,
};
use super::transport::{HttpResponse, HttpTransport, Transport};
use crate::drivers::common::http::{Credential, HttpOptions};
use crate::drivers::{CancelAction, CancelHandle, cancel_portable};
use crate::error::AppError;
use crate::testkit::{FakeServer, Reply, Route};
use crate::types::value::Value;

const BASE: &str = "https://bigquery.example/bigquery/v2";

fn client(fake: &FakeBigquery) -> Client {
    Client::new(fake.transport(), BASE, "demo-proj")
}

fn opts() -> QueryOptions {
    QueryOptions {
        location: Some("asia-southeast2".into()),
        default_dataset: Some(DatasetReference {
            project_id: "demo-proj".into(),
            dataset_id: "shop".into(),
        }),
        maximum_bytes_billed: Some(1 << 30),
        use_query_cache: true,
        ..QueryOptions::default()
    }
}

#[tokio::test(start_paused = true)]
async fn a_query_job_is_polled_until_it_finishes_then_paged() {
    let fake = FakeBigquery::new();
    let job_id = new_job_id();
    fake.on(
        "POST",
        "/bigquery/v2/projects/demo-proj/jobs",
        job_inserted("demo-proj", &job_id, "asia-southeast2"),
    );
    let cols = schema(&[
        ("id", "INTEGER"),
        ("name", "STRING"),
        ("at", "TIMESTAMP"),
        ("tags", "STRING*"),
    ]);
    fake.on_seq(
        "GET",
        &format!("/bigquery/v2/projects/demo-proj/queries/{job_id}"),
        vec![
            still_running("demo-proj", &job_id),
            still_running("demo-proj", &job_id),
            results(
                cols.clone(),
                rows(&[
                    vec![
                        json!("9007199254740993"),
                        json!("a"),
                        json!("1704164645000000"),
                        json!([{"v": "x"}]),
                    ],
                    vec![json!("2"), json!(null), json!(null), json!([])],
                ]),
                3,
                Some("page-2"),
            ),
            results(
                cols.clone(),
                rows(&[vec![json!("3"), json!("c"), json!("0"), json!([])]]),
                3,
                None,
            ),
        ],
    );
    let c = client(&fake);
    let params = query_parameters(&[Value::Int(1)]).unwrap();
    let run = c
        .run_query(
            &job_id,
            "SELECT * FROM t WHERE id > @p1",
            &params,
            &opts(),
            Some(2),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(run.job.location.as_deref(), Some("asia-southeast2"));
    let fields = run.first.schema.clone().unwrap().fields;
    let first = decode_rows(&fields, &run.first.rows);
    assert_eq!(first[0][0], Value::BigInt("9007199254740993".into()));
    assert_eq!(
        first[0][2],
        Value::TimestampTz("2024-01-02 03:04:05+00".into())
    );
    assert_eq!(first[0][3], Value::Json("[\"x\"]".into()));
    assert_eq!(first[1][1], Value::Null);
    assert_eq!(run.first.total_rows, Some(3));
    let next = c
        .next_page(&run.job, run.first.page_token.as_deref().unwrap(), Some(2))
        .await
        .unwrap();
    assert_eq!(decode_rows(&fields, &next.rows)[0][0], Value::Int(3));
    assert_eq!(next.page_token, None);

    let sent = fake.requests();
    assert_eq!(sent.len(), 5, "insert, three polls, one page");
    // The transport adds the bearer: the client's requests carry none.
    assert!(sent.iter().all(|r| r.header("authorization").is_none()));
    let body = sent[0].json();
    assert_eq!(body["jobReference"]["jobId"], job_id.as_str());
    assert_eq!(
        body["configuration"]["query"]["maximumBytesBilled"],
        "1073741824"
    );
    assert_eq!(body["configuration"]["query"]["useLegacySql"], false);
    assert_eq!(
        body["configuration"]["query"]["queryParameters"][0]["name"],
        "p1"
    );
    let poll = &sent[1];
    assert_eq!(poll.param("location").as_deref(), Some("asia-southeast2"));
    assert_eq!(poll.param("timeoutMs").as_deref(), Some("10000"));
    assert_eq!(
        poll.param("formatOptions.useInt64Timestamp").as_deref(),
        Some("true")
    );
    assert_eq!(poll.param("maxResults").as_deref(), Some("2"));
    assert_eq!(sent[4].param("pageToken").as_deref(), Some("page-2"));
}

#[tokio::test(start_paused = true)]
async fn a_cancel_stops_the_wait_and_cancels_the_job_on_the_server() {
    let fake = FakeBigquery::new();
    fake.on(
        "POST",
        "/bigquery/v2/projects/demo-proj/jobs",
        job_inserted("demo-proj", "tabler_j", "US"),
    );
    fake.on_replies(
        "GET",
        "/bigquery/v2/projects/demo-proj/queries/tabler_j",
        vec![FakeReply::after(
            still_running("demo-proj", "tabler_j"),
            Duration::from_secs(10),
        )],
    );
    fake.on(
        "POST",
        "/bigquery/v2/projects/demo-proj/jobs/tabler_j/cancel",
        HttpResponse::json(
            200,
            &json!({"kind": "bigquery#jobCancelResponse", "job": {"status": {"state": "RUNNING"}}}),
        ),
    );
    let c = Arc::new(client(&fake));
    let stop = CancellationToken::new();
    let action = Arc::new(JobCancel {
        client: Arc::clone(&c),
        job: JobReference {
            project_id: "demo-proj".into(),
            job_id: "tabler_j".into(),
            location: Some("US".into()),
        },
        stop: stop.clone(),
    });
    assert_eq!(action.label(), "job cancel");
    let runner = {
        let c = Arc::clone(&c);
        let stop = stop.clone();
        tokio::spawn(async move {
            c.run_query(
                "tabler_j",
                "SELECT 1",
                &[],
                &QueryOptions::default(),
                None,
                &stop,
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_secs(25)).await;
    assert!(!runner.is_finished(), "a long job keeps the run waiting");
    cancel_portable(CancelHandle::Custom(action))
        .await
        .expect("a custom handle")
        .unwrap();
    let ended = runner.await.unwrap();
    assert!(matches!(ended, Err(AppError::Cancelled)), "{ended:?}");
    // The handle's request, and the run's own once it sees the cancel.
    let cancels = fake.requests_to("/cancel");
    assert!(!cancels.is_empty());
    assert!(
        cancels
            .iter()
            .all(|c| c.param("location").as_deref() == Some("US"))
    );
}

#[tokio::test]
async fn cancelling_a_job_that_is_gone_is_no_error() {
    let fake = FakeBigquery::new();
    let c = client(&fake);
    // No route: the fake answers 404 like BigQuery for a job it does not know.
    c.cancel_job(&JobReference {
        project_id: "demo-proj".into(),
        job_id: "never".into(),
        location: None,
    })
    .await
    .unwrap();
    fake.on(
        "POST",
        "/bigquery/v2/projects/demo-proj/jobs/j/cancel",
        error_reply(403, "accessDenied", "no"),
    );
    let e = c
        .cancel_job(&JobReference {
            project_id: String::new(),
            job_id: "j".into(),
            location: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::Db { .. }), "{e:?}");
}

#[tokio::test]
async fn query_errors_carry_their_reason_and_position() {
    let fake = FakeBigquery::new();
    fake.on(
        "POST",
        "/bigquery/v2/projects/demo-proj/jobs",
        error_reply(
            400,
            "invalidQuery",
            "Syntax error: Unexpected keyword FORM at [2:3]",
        ),
    );
    let c = client(&fake);
    let sql = "SELECT 1\n  FORM t";
    let e = c
        .run_query(
            "j",
            sql,
            &[],
            &QueryOptions::default(),
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    match e {
        AppError::Db {
            position, detail, ..
        } => {
            let p = position.unwrap();
            assert_eq!((p.line, p.column), (2, 3));
            assert_eq!(detail.as_deref(), Some("reason: invalidQuery"));
        }
        other => panic!("{other:?}"),
    }

    // A job that failed at once, and one that fails while it runs.
    let fake = FakeBigquery::new();
    fake.on(
        "POST",
        "/bigquery/v2/projects/demo-proj/jobs",
        HttpResponse::json(200, &json!({"jobReference": {"projectId": "demo-proj", "jobId": "j"},
            "status": {"state": "DONE", "errorResult": {"reason": "bytesBilledLimitExceeded",
            "message": "Query exceeded limit for bytes billed: 1073741824. 2147483648 or higher required."}}})),
    );
    let e = client(&fake)
        .run_query(
            "j",
            "SELECT * FROM big",
            &[],
            &opts(),
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&e, AppError::Db { hint: Some(h), .. } if h.contains("Maximum bytes billed")),
        "{e:?}"
    );

    let fake = FakeBigquery::new();
    fake.on(
        "POST",
        "/bigquery/v2/projects/demo-proj/jobs",
        job_inserted("demo-proj", "j", "US"),
    );
    fake.on(
        "GET",
        "/bigquery/v2/projects/demo-proj/queries/j",
        HttpResponse::json(200, &json!({"jobComplete": true, "errors": [{"reason": "stopped", "message": "cancelled"}]})),
    );
    let e = client(&fake)
        .run_query(
            "j",
            "SELECT 1",
            &[],
            &opts(),
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::Cancelled), "{e:?}");
}

#[tokio::test]
async fn listings_follow_page_tokens_and_stop_on_a_repeated_one() {
    let fake = FakeBigquery::new();
    let page = |ids: &[&str], next: Option<&str>| {
        let tables: Vec<serde_json::Value> = ids
            .iter()
            .map(|t| json!({"tableReference": {"projectId": "demo-proj", "datasetId": "shop", "tableId": t}, "type": "TABLE"}))
            .collect();
        let mut v = json!({"tables": tables});
        if let Some(n) = next {
            v["nextPageToken"] = n.into();
        }
        HttpResponse::json(200, &v)
    };
    fake.on_seq(
        "GET",
        "/bigquery/v2/projects/demo-proj/datasets/shop/tables",
        vec![
            page(&["a", "b"], Some("t2")),
            page(&["c"], Some("t3")),
            page(&["d"], Some("t3")),
        ],
    );
    let c = client(&fake);
    let t = c.list_tables("demo-proj", "shop").await.unwrap();
    let names: Vec<String> = table_summaries(&t, "shop")
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["a", "b", "c", "d"]);
    let sent = fake.requests();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0].param("pageToken"), None);
    assert_eq!(sent[0].param("maxResults").as_deref(), Some("1000"));
    assert_eq!(sent[1].param("pageToken").as_deref(), Some("t2"));
    assert_eq!(sent[2].param("pageToken").as_deref(), Some("t3"));
}

#[tokio::test]
async fn names_are_escaped_in_paths() {
    let fake = FakeBigquery::new();
    fake.on(
        "GET",
        "/bigquery/v2/projects/example.com%3Aproj/datasets/d/tables/a%2Fb%20c",
        HttpResponse::json(200, &json!({"tableReference": {"projectId": "example.com:proj", "datasetId": "d", "tableId": "a/b c"}})),
    );
    let t = client(&fake)
        .get_table("example.com:proj", "d", "a/b c")
        .await
        .unwrap();
    assert_eq!(catalog::table_meta(&t).table.name, "a/b c");
}

#[tokio::test]
async fn dot_names_are_refused_before_a_request() {
    let fake = FakeBigquery::new();
    let c = client(&fake);
    for name in [".", ".."] {
        let e = c
            .list_table_data("demo-proj", "d", name, &DataPage::default())
            .await
            .unwrap_err();
        assert!(matches!(e, AppError::InvalidInput(_)), "{e:?}");
        assert!(c.get_table("demo-proj", name, "t").await.is_err());
    }
    // Dots inside a name are fine.
    fake.on(
        "GET",
        "/bigquery/v2/projects/demo-proj/datasets/d/tables/a..b",
        HttpResponse::json(200, &json!({"tableReference": {"projectId": "demo-proj", "datasetId": "d", "tableId": "a..b"}})),
    );
    c.get_table("demo-proj", "d", "a..b").await.unwrap();
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn table_data_is_read_for_free_page_by_page() {
    let fake = FakeBigquery::new();
    fake.on(
        "GET",
        "/bigquery/v2/projects/demo-proj/datasets/shop/tables/orders/data",
        HttpResponse::json(
            200,
            &json!({"kind": "bigquery#tableDataList", "totalRows": "1000",
            "pageToken": "BEQ", "rows": rows(&[vec![json!("1"), json!("1.25")]])}),
        ),
    );
    let c = client(&fake);
    let page = c
        .list_table_data(
            "demo-proj",
            "shop",
            "orders",
            &DataPage {
                page_token: None,
                start_index: Some(100),
                max_results: Some(50),
                selected_fields: vec!["id".into(), "total".into()],
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (page.total_rows, page.page_token.as_deref()),
        (Some(1000), Some("BEQ"))
    );
    let fields: Vec<FieldSchema> =
        serde_json::from_value(schema(&[("id", "INT64"), ("total", "NUMERIC")])["fields"].clone())
            .unwrap();
    assert_eq!(
        decode_rows(&fields, &page.rows)[0],
        [Value::Int(1), Value::Decimal("1.25".into())]
    );
    let r = &fake.requests()[0];
    assert_eq!(r.param("startIndex").as_deref(), Some("100"));
    assert_eq!(r.param("maxResults").as_deref(), Some("50"));
    assert_eq!(r.param("selectedFields").as_deref(), Some("id,total"));
    assert_eq!(
        r.param("formatOptions.useInt64Timestamp").as_deref(),
        Some("true")
    );
    // With a page token the start index is left out (the token says where).
    c.list_table_data(
        "demo-proj",
        "shop",
        "orders",
        &DataPage {
            page_token: Some("BEQ".into()),
            start_index: Some(100),
            ..DataPage::default()
        },
    )
    .await
    .unwrap();
    let r = &fake.requests()[1];
    assert_eq!(
        (r.param("pageToken").as_deref(), r.param("startIndex")),
        (Some("BEQ"), None)
    );
}

#[tokio::test]
async fn a_dry_run_estimates_the_bytes() {
    let fake = FakeBigquery::new();
    fake.on(
        "POST",
        "/bigquery/v2/projects/demo-proj/queries",
        HttpResponse::json(
            200,
            &json!({"kind": "bigquery#queryResponse", "jobComplete": true,
            "totalBytesProcessed": "52428800", "schema": schema(&[("n", "INT64")])}),
        ),
    );
    let d = client(&fake)
        .dry_run("SELECT n FROM t", &[], &opts())
        .await
        .unwrap();
    assert_eq!(d.bytes_processed, Some(52_428_800));
    assert_eq!(d.schema.unwrap().fields[0].name, "n");
    let body = fake.requests()[0].json();
    assert_eq!(body["dryRun"], true);
    assert_eq!(body["location"], "asia-southeast2");
    assert_eq!(body["defaultDataset"]["datasetId"], "shop");
}

#[tokio::test]
async fn running_jobs_make_the_process_list() {
    let fake = FakeBigquery::new();
    fake.on_seq(
        "GET",
        "/bigquery/v2/projects/demo-proj/jobs",
        vec![
            HttpResponse::json(200, &json!({"jobs": [{"jobReference": {"projectId": "demo-proj", "jobId": "tabler_1", "location": "US"},
                "state": "RUNNING", "configuration": {"query": {"query": "SELECT 1"}}}]})),
            HttpResponse::json(200, &json!({})),
        ],
    );
    let c = client(&fake);
    let jobs = c.list_running_jobs(50).await.unwrap();
    let rows = catalog::process_rows(&jobs, 0);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "demo-proj:US.tabler_1");
    let sent = fake.requests();
    assert_eq!(sent[0].param("stateFilter").as_deref(), Some("running"));
    assert_eq!(sent[1].param("stateFilter").as_deref(), Some("pending"));
    assert_eq!(sent[0].param("projection").as_deref(), Some("full"));
}

#[tokio::test]
async fn a_broken_connection_is_a_connect_error() {
    let fake = FakeBigquery::new();
    fake.on_replies(
        "GET",
        "/bigquery/v2/projects/demo-proj/datasets",
        vec![FakeReply::broken("reset")],
    );
    let e = client(&fake).list_datasets("demo-proj").await.unwrap_err();
    assert!(matches!(e, AppError::Connect(_)), "{e:?}");
}

#[tokio::test]
async fn user_credentials_bill_their_quota_project() {
    let fake = FakeBigquery::new();
    fake.on(
        "GET",
        "/bigquery/v2/projects",
        HttpResponse::json(200, &json!({"projects": []})),
    );
    let c = client(&fake).with_quota_project(Some("quota-proj"));
    c.list_projects().await.unwrap();
    assert_eq!(
        fake.requests()[0].header("x-goog-user-project"),
        Some("quota-proj")
    );
}

#[tokio::test]
async fn the_bearer_never_goes_in_clear_text_to_another_machine() {
    let fake = FakeBigquery::new();
    fake.on(
        "GET",
        "/bigquery/v2/projects",
        HttpResponse::json(200, &json!({"projects": []})),
    );
    for base in [
        "http://bq.example/bigquery/v2",
        "http://localhost.example.com/bigquery/v2",
    ] {
        let c = Client::new(fake.transport(), base, "demo-proj");
        let e = c.list_projects().await.unwrap_err();
        assert!(matches!(e, AppError::InvalidInput(_)), "{e:?}");
    }
    assert!(fake.requests().is_empty());
    let local = Client::new(
        fake.transport(),
        "http://localhost:9050/bigquery/v2",
        "demo-proj",
    );
    local.list_projects().await.unwrap();
}

#[tokio::test]
async fn a_google_sign_in_goes_to_google_hosts_only() {
    let fake = FakeBigquery::new();
    fake.on(
        "GET",
        "/bigquery/v2/projects",
        HttpResponse::json(200, &json!({"projects": []})),
    );
    for base in [
        "https://evil.example/bigquery/v2",
        "https://googleapis.com.evil.example/bigquery/v2",
        "https://bigquery.googleapis.com@evil.example/bigquery/v2",
    ] {
        let c = Client::new(fake.transport(), base, "demo-proj").with_google_sign_in(true);
        let e = c.list_projects().await.unwrap_err();
        assert!(matches!(e, AppError::InvalidInput(_)), "{e:?}");
    }
    assert!(fake.requests().is_empty());
    for base in [
        "https://bigquery.googleapis.com/bigquery/v2",
        "http://localhost:9050/bigquery/v2",
    ] {
        let c = Client::new(fake.transport(), base, "demo-proj").with_google_sign_in(true);
        c.list_projects().await.unwrap();
    }
    // The profile's own access token may go to another https endpoint.
    Client::new(
        fake.transport(),
        "https://evil.example/bigquery/v2",
        "demo-proj",
    )
    .list_projects()
    .await
    .unwrap();
}

/// A profile for the wire tests (no SSH, no secret).
fn wire_profile() -> crate::types::profile::ConnectionProfile {
    crate::drivers::common::token::tests::profile(crate::types::profile::Driver::Bigquery, None)
}

/// The API's transport to `base` with `credential`, as a session builds it.
fn api(base: &str, credential: Credential, options: HttpOptions) -> Arc<dyn Transport> {
    crate::testkit::install_crypto();
    let profile = wire_profile();
    let target = crate::drivers::ConnectTarget {
        profile: &profile,
        password: None,
        host_override: None,
    };
    Arc::new(HttpTransport::for_target(&target, base, options, credential).unwrap())
}

/// The tokens of `file_json` from its token endpoint, over the wire.
fn wire_tokens(file_json: &str) -> crate::drivers::common::token::TokenSource {
    crate::testkit::install_crypto();
    let file = parse_credential_file(file_json).unwrap();
    let endpoint = token_endpoint(&file).unwrap();
    let transport =
        Arc::new(HttpTransport::token_endpoint(&endpoint, HttpOptions::default()).unwrap());
    issued_tokens(transport, &file, SCOPE_BIGQUERY).unwrap()
}

/// Over a real socket: the service account's token exchange (a form post to the token
/// endpoint, which gets no bearer) and a query that carries the token.
#[tokio::test]
async fn the_wire_token_exchange_and_a_query() {
    let server = FakeServer::start(vec![
        Route::new("POST", "/token").reply(Reply::json(
            200,
            &json!({"access_token": "wire-token", "expires_in": 3599, "token_type": "Bearer"}),
        )),
        Route::new("POST", "/bigquery/v2/projects/demo-proj/queries")
            .auth("Bearer wire-token")
            .reply(Reply::json(
                200,
                &json!({"jobComplete": true, "jobReference": {"projectId": "demo-proj", "jobId": "j"},
                    "schema": schema(&[("x", "INT64")]), "rows": rows(&[vec![json!("1")]]), "totalRows": "1"}),
            )),
    ])
    .await;
    let base = server.url("/bigquery/v2");
    let tokens = wire_tokens(&sa_key_json(&server.url("/token")));
    let transport = api(&base, Credential::bearer(tokens), HttpOptions::default());
    let c = Client::new(transport, &base, "demo-proj");
    let body = super::jobs::query_body(
        "SELECT 1 AS x",
        &[],
        &QueryOptions::default(),
        Some(10),
        Some(1000),
    );
    let r = c.query(&body, "SELECT 1 AS x").await.unwrap();
    assert_eq!(r.rows.len(), 1);
    let sent = server.requests();
    assert_eq!(
        sent[0].header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(sent[0].header("authorization"), None);
    let form = sent[0].text();
    assert!(
        form.starts_with(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion="
        ),
        "{form}"
    );
    assert_eq!(sent[1].header("authorization"), Some("Bearer wire-token"));
    assert_eq!(sent[1].json()["formatOptions"]["useInt64Timestamp"], true);
}

/// A token the API refuses is fetched again once (`TokenSource::issued` under the shared
/// client), and user credentials bill their quota project.
#[tokio::test]
async fn a_refused_token_is_fetched_again_once() {
    let server = FakeServer::start(vec![
        Route::new("POST", "/token")
            .reply(Reply::json(200, &json!({"access_token": "old", "expires_in": 3600})))
            .reply(Reply::json(200, &json!({"access_token": "new", "expires_in": 3600}))),
        Route::new("GET", "/bigquery/v2/projects/demo-proj/datasets")
            .auth("Bearer new")
            .reply(Reply::json(
                200,
                &json!({"datasets": [{"datasetReference": {"projectId": "demo-proj", "datasetId": "shop"}}]}),
            )),
    ])
    .await;
    let base = server.url("/bigquery/v2");
    let tokens = wire_tokens(&super::testing::user_json(&server.url("/token")));
    let c = Client::new(
        api(&base, Credential::bearer(tokens), HttpOptions::default()),
        &base,
        "demo-proj",
    )
    .with_quota_project(Some("quota-proj"));
    let ds = c.list_datasets("demo-proj").await.unwrap();
    assert_eq!(schemas(&ds, None)[0].name, "shop");
    let sent = server.requests();
    let calls: Vec<_> = sent
        .iter()
        .filter(|r| r.path.ends_with("/datasets"))
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].header("authorization"), Some("Bearer old"));
    assert_eq!(calls[1].header("authorization"), Some("Bearer new"));
    assert!(
        calls
            .iter()
            .all(|r| r.header("x-goog-user-project") == Some("quota-proj"))
    );
    assert_eq!(sent.iter().filter(|r| r.path == "/token").count(), 2);

    // A fixed token cannot be refreshed: the 401 is the answer, after one request.
    let fixed = crate::drivers::common::token::TokenSource::from_secret(Some("expired"));
    let c = Client::new(
        api(&base, Credential::bearer(fixed), HttpOptions::default()),
        &base,
        "demo-proj",
    );
    let before = server.requests().len();
    let e = c.list_datasets("demo-proj").await.unwrap_err();
    assert!(matches!(e, AppError::Auth(_)), "{e:?}");
    assert_eq!(server.requests().len(), before + 1);
}

/// A GET that failed on the server's side (`backendError`) is sent again; a POST, which may have
/// started a job, is not.
#[tokio::test]
async fn server_errors_repeat_gets_but_not_posts() {
    let backend = || {
        Reply::json(
            500,
            &json!({"error": {"code": 500, "message": "Backend error", "errors": [{"reason": "backendError"}]}}),
        )
    };
    let server = FakeServer::start(vec![
        Route::new("GET", "/bigquery/v2/projects")
            .reply(backend())
            .reply(Reply::json(200, &json!({"projects": []}))),
        Route::new("POST", "/bigquery/v2/projects/demo-proj/jobs")
            .reply(backend())
            .reply(job_inserted_reply()),
    ])
    .await;
    let base = server.url("/bigquery/v2");
    let options = HttpOptions {
        retry: super::transport::retry_policy(),
        ..HttpOptions::default()
    };
    let c = Client::new(api(&base, Credential::None, options), &base, "demo-proj");
    c.list_projects().await.unwrap();
    let e = c
        .insert_job(&json!({"configuration": {}}), "SELECT 1")
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::Db { .. }), "{e:?}");
    let sent = server.requests();
    assert_eq!(sent.iter().filter(|r| r.method == "GET").count(), 2);
    assert_eq!(sent.iter().filter(|r| r.method == "POST").count(), 1);
}

fn job_inserted_reply() -> Reply {
    Reply::json(
        200,
        &json!({"jobReference": {"projectId": "demo-proj", "jobId": "j"}, "status": {"state": "RUNNING"}}),
    )
}

/// A request that hangs ends in `Timeout` at the transport's limit.
#[tokio::test]
async fn a_request_that_hangs_times_out() {
    let server =
        FakeServer::start(vec![Route::new("GET", "/bigquery/v2/projects").reply(
            Reply::json(200, &json!({"projects": []})).delay(Duration::from_secs(10)),
        )])
        .await;
    let base = server.url("/bigquery/v2");
    let options = HttpOptions {
        request_timeout: Some(Duration::from_millis(200)),
        ..HttpOptions::default()
    };
    let c = Client::new(api(&base, Credential::None, options), &base, "demo-proj");
    let e = c.list_projects().await.unwrap_err();
    assert!(matches!(e, AppError::Timeout), "{e:?}");
}
