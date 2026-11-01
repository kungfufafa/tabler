//! BigQuery against a real project: the bigquery lane's suite (docs/drivers/bigquery.md). Built
//! only with `drv-bigquery` (`required-features` in Cargo.toml) and run with
//! `node dev/scripts/lane.mjs bigquery it`, which loads `.env`. Each test skips itself while its
//! variable is unset, and is `#[ignore]`d so that a plain `cargo test --all-features` never
//! reaches a server (BigQuery bills the queries; this suite reads a few MB).
//!
//! `TABLER_TEST_BIGQUERY_URL` is `bigquery://<project>?keyFile=<path>&dataset=<dataset>` (a
//! service-account key), or `bigquery://:<access token>@<project>?dataset=<dataset>`, with an
//! optional `location=` and `endpoint=` (the emulator's `http://localhost:9050/bigquery/v2`). The
//! dataset must exist and take DML (not the sandbox); the suite works in a table of its own
//! (`tabler_it_<pid>`), which it drops again.

#![cfg(feature = "drv-bigquery")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use tabler_lib::commands::query::query_limited;
use tabler_lib::drivers::bigquery::auth::{
    SCOPE_BIGQUERY_READONLY, issued_tokens, read_credential_file, token_endpoint,
};
use tabler_lib::drivers::bigquery::client::Client;
use tabler_lib::drivers::bigquery::jobs::{QueryOptions, query_body};
use tabler_lib::drivers::bigquery::transport::HttpTransport;
use tabler_lib::drivers::common::http::{Credential, HttpOptions};
use tabler_lib::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, TxStatement, connect, request_cancel,
};
use tabler_lib::error::AppError;
use tabler_lib::types::browse::{BrowseRequest, RowKey, SortDir, SortSpec};
use tabler_lib::types::driver::ReadOnlyMode;
use tabler_lib::types::profile::{ConnectionProfile, DriverParams};
use tabler_lib::types::schema::TableRef;
use tabler_lib::types::value::Value;

/// A query that keeps BigQuery busy for a while and scans no table (nothing billed by bytes).
const SLOW: &str = "SELECT COUNT(*) FROM UNNEST(GENERATE_ARRAY(1, 1000000)) AS a, \
     UNNEST(GENERATE_ARRAY(1, 100000)) AS b WHERE MOD(a * b, 7) = 3";

struct Target {
    profile: ConnectionProfile,
    token: Option<String>,
    dataset: String,
    key_file: Option<String>,
}

/// The project, sign-in and dataset of `var`; None (skipped) while it is unset.
fn target(var: &str, read_only: bool) -> Option<Target> {
    let url = common::url(var)?;
    common::install_crypto();
    let (mut profile, token) = common::profile_from_url(&url);
    // The project is the URL's host.
    profile.database = profile.host.take();
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or_default();
    let get = |k: &str| {
        query
            .split('&')
            .find_map(|p| p.strip_prefix(&format!("{k}=")))
            .map(str::to_string)
    };
    let key_file = get("keyFile");
    let mut params = serde_json::json!({
        "auth": if key_file.is_some() { "serviceAccount" } else { "accessToken" },
        "keyFilePath": key_file,
        "location": get("location"),
        "apiEndpoint": get("endpoint"),
    });
    let dataset = get("dataset").unwrap_or_else(|| "tabler_it".into());
    params["defaultDataset"] = dataset.clone().into();
    profile.options.driver_params = params.as_object().cloned().map(DriverParams);
    profile.options.read_only = read_only;
    Some(Target {
        profile,
        token,
        dataset,
        key_file,
    })
}

async fn open(t: &Target) -> Arc<dyn DbSession> {
    connect(ConnectTarget {
        profile: &t.profile,
        password: t.token.as_deref(),
        host_override: None,
    })
    .await
    .unwrap_or_else(|e| panic!("connect failed: {e}"))
}

#[tokio::test]
#[ignore = "set TABLER_TEST_BIGQUERY_URL"]
async fn real_server() {
    let Some(t) = target("TABLER_TEST_BIGQUERY_URL", false) else {
        return;
    };
    let s = open(&t).await;
    let ds = t.dataset.clone();
    let table = format!("tabler_it_{}", std::process::id());
    let path = format!("`{ds}`.`{table}`");
    s.execute(
        &format!(
            "CREATE OR REPLACE TABLE {path} (id INT64, big INT64, price NUMERIC, at TIMESTAMP, \
             tags ARRAY<STRING>, PRIMARY KEY (id) NOT ENFORCED)"
        ),
        &[],
    )
    .await
    .unwrap();
    let inserted = s
        .execute(
            &format!(
                "INSERT INTO {path} (id, big, price, at, tags) VALUES \
                 (1, 9007199254740993, 12.50, TIMESTAMP '2024-01-02 03:04:05.123456+00', ['a']), \
                 (2, 2, 0.000000001, NULL, []), (3, 3, NULL, NULL, ['b', 'c'])"
            ),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(inserted.affected, 3);

    // The sidebar and the structure.
    assert!(s.list_schemas().await.unwrap().iter().any(|d| d.name == ds));
    assert!(
        s.list_tables(&ds)
            .await
            .unwrap()
            .iter()
            .any(|x| x.name == table)
    );
    let tref = TableRef::new(Some(&ds), &table);
    let meta = s.table_meta(&tref).await.unwrap();
    assert_eq!(meta.columns.len(), 5);
    assert_eq!(meta.primary_key, ["id"]);

    // Free pages with their cursor, then a sorted page from one job.
    let source = s.table_source().unwrap();
    let mut req = BrowseRequest {
        table: tref.clone(),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit: 2,
        offset: 0,
        cursor: None,
    };
    let first = source
        .browse(&meta, &req, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(first.rows.len(), 2);
    req.cursor = first.next_cursor.clone();
    let second = source
        .browse(&meta, &req, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(second.rows.len(), 1);
    req.cursor = None;
    req.sort = vec![SortSpec {
        column: "id".into(),
        dir: SortDir::Desc,
        nulls: None,
    }];
    let sorted = source
        .browse(&meta, &req, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(sorted.rows[0][0], Value::Int(3));
    let one = sorted
        .rows
        .iter()
        .chain(second.rows.iter())
        .chain(first.rows.iter());
    let row1 = one.clone().find(|r| r[0] == Value::Int(1)).expect("row 1");
    // 2^53 + 1 stays exact, NUMERIC is text, a TIMESTAMP is text, an array is JSON.
    assert_eq!(row1[1], Value::BigInt("9007199254740993".into()));
    assert_eq!(row1[2], Value::Decimal("12.5".into()));
    assert_eq!(
        row1[3],
        Value::TimestampTz("2024-01-02 03:04:05.123456+00".into())
    );
    assert_eq!(row1[4], Value::Json("[\"a\"]".into()));

    // Counts and a cell.
    let n = source
        .count(&meta, None, true, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(n.count, 3);
    let key = RowKey::Pk([("id".to_string(), Value::Int(1))].into_iter().collect());
    assert_eq!(
        source.fetch_cell(&meta, &key, "big").await.unwrap(),
        Value::BigInt("9007199254740993".into())
    );

    // A commit with one failing change leaves everything as it was.
    let e = s
        .transaction(vec![
            TxStatement::new(
                format!("UPDATE {path} SET big = 0 WHERE id = @p1"),
                vec![Value::Int(1)],
            )
            .expecting(1),
            TxStatement::new(
                format!("UPDATE `{ds}`.`tabler_it_missing` SET x = 0 WHERE id = @p1"),
                vec![Value::Int(1)],
            ),
        ])
        .await
        .unwrap_err();
    assert!(
        matches!(
            &e,
            AppError::Db {
                statement_index: Some(1),
                ..
            }
        ),
        "{e:?}"
    );
    let rs = s
        .query(&format!("SELECT big FROM {path} WHERE id = 1"), &[])
        .await
        .unwrap();
    assert_eq!(rs.rows[0][0], Value::BigInt("9007199254740993".into()));

    // The editor: a row cap, and its BigQuery session keeps a temp table between runs.
    let mut sink = CollectSink::default();
    let out = s
        .run_statement(
            &format!("SELECT id FROM {path} ORDER BY id"),
            2,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert!(out.truncated);
    assert_eq!(out.row_count, 2);
    s.run_statement(
        "CREATE TEMP TABLE tabler_tmp AS SELECT 42 AS x",
        10,
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .unwrap();
    let mut sink = CollectSink::default();
    s.run_statement(
        "SELECT x FROM tabler_tmp",
        10,
        &mut sink,
        CancelSlot::default(),
    )
    .await
    .unwrap();
    assert_eq!(sink.sets[0].rows[0][0], Value::Int(42));

    // Cancel, on the server.
    let slot = CancelSlot::default();
    let run = {
        let (s, slot) = (Arc::clone(&s), slot.clone());
        tokio::spawn(async move {
            s.run_statement(SLOW, 10, &mut CollectSink::default(), slot)
                .await
        })
    };
    tokio::time::sleep(Duration::from_secs(3)).await;
    if let Some(handle) = request_cancel(&slot) {
        s.cancel(handle).await.unwrap();
    }
    let ended = run.await.unwrap();
    assert!(matches!(ended, Err(AppError::Cancelled)), "{ended:?}");

    // The app's time out.
    let e = query_limited(s.as_ref(), SLOW, &[], Some(Duration::from_secs(2)))
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::QueryTimeout { .. }), "{e:?}");

    s.execute(&format!("DROP TABLE {path}"), &[]).await.unwrap();
    s.close().await;

    // Read-only: refused before anything is sent.
    let ro = target("TABLER_TEST_BIGQUERY_URL", true).unwrap();
    let r = open(&ro).await;
    assert_eq!(
        r.capabilities().safety.read_only,
        ReadOnlyMode::ClassifierOnly
    );
    let e = r
        .execute(&format!("DELETE FROM {path} WHERE TRUE"), &[])
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::InvalidInput(_)), "{e:?}");
}

/// Critique D6: whether a token minted with the `bigquery.readonly` scope makes BigQuery refuse
/// DML. Both statements go through `jobs.query` (which takes that scope; `jobs.insert` does not,
/// so the driver signs every profile in with the full scope today): a SELECT first proves the scope
/// can run queries on that call at all, then the DELETE must be refused for its scope. Sent past
/// Tabler's classifier on purpose (the client, not a session), against a table that does not
/// exist, so nothing can change either way. Until this passes on a real project, and the driver
/// sends read-only sessions' statements through `jobs.query`, the caps say ClassifierOnly.
#[tokio::test]
#[ignore = "set TABLER_TEST_BIGQUERY_URL with keyFile="]
async fn the_readonly_scope_refuses_dml() {
    let Some(t) = target("TABLER_TEST_BIGQUERY_URL", true) else {
        return;
    };
    let Some(key) = t.key_file.as_deref() else {
        println!("skipped: the URL names no keyFile");
        return;
    };
    let file = read_credential_file(std::path::Path::new(key))
        .await
        .unwrap();
    let token_transport =
        HttpTransport::token_endpoint(&token_endpoint(&file).unwrap(), HttpOptions::default())
            .unwrap();
    let tokens = issued_tokens(Arc::new(token_transport), &file, SCOPE_BIGQUERY_READONLY).unwrap();
    let base = "https://bigquery.googleapis.com/bigquery/v2";
    let transport = HttpTransport::for_target(
        &ConnectTarget {
            profile: &t.profile,
            password: None,
            host_override: None,
        },
        base,
        HttpOptions::default(),
        Credential::bearer(tokens),
    )
    .unwrap();
    let project = t.profile.database.clone().unwrap_or_default();
    let client = Client::new(Arc::new(transport), base, &project);
    let opts = QueryOptions::default();
    let select = "SELECT 1 AS one";
    let ok = client
        .query(&query_body(select, &[], &opts, None, Some(30_000)), select)
        .await;
    assert!(
        ok.is_ok(),
        "the read-only scope cannot run a SELECT through jobs.query: {ok:?}"
    );
    let sql = format!(
        "DELETE FROM `{}`.`tabler_it_never_created` WHERE TRUE",
        t.dataset
    );
    let e = client
        .query(&query_body(&sql, &[], &opts, None, Some(30_000)), &sql)
        .await
        .unwrap_err();
    // A scope refusal is a 403 with "insufficient" scopes, not the missing table.
    let said = e.to_string().to_ascii_lowercase();
    assert!(
        said.contains("insufficient") || said.contains("scope"),
        "the read-only scope did not refuse the DELETE: {e}"
    );
}
