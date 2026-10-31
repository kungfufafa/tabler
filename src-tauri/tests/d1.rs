//! Cloudflare D1 against a real account: the sqlite-remote lane's suite
//! (docs/drivers/sqlite-remote.md). Built only with `drv-d1` (`required-features` in Cargo.toml)
//! and run with `node dev/scripts/lane.mjs sqlite-remote it`, which loads `.env`. Each test skips
//! itself while its variable is unset, and is `#[ignore]`d so that a plain `cargo test
//! --all-features` never reaches a server (D1 bills the rows read).
//!
//! `TABLER_TEST_D1_URL` is `d1://:<api-token>@<account-id>/<database name or UUID>`: the token
//! (D1 Edit) as the password, the account as the host. The test works in a table of its own
//! (`tabler_it_<pid>`), which it drops again.

#![cfg(feature = "drv-d1")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use tabler_lib::commands::browse::{browse_page, fetch_cell_inner};
use tabler_lib::commands::query::{query_limited, run_sql_inner};
use tabler_lib::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, TxStatement, connect, request_cancel,
};
use tabler_lib::error::AppError;
use tabler_lib::types::browse::{BrowseRequest, RowKey};
use tabler_lib::types::caps::Feature;
use tabler_lib::types::driver::ReadOnlyMode;
use tabler_lib::types::profile::DriverParams;
use tabler_lib::types::query::{RunOptions, StatementKind};
use tabler_lib::types::schema::TableRef;
use tabler_lib::types::value::Value;

/// A statement that keeps the database busy for a while (D1 stops it after 30 s).
const SLOW: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c LIMIT 100000000) \
     SELECT count(*) FROM c";

/// A session on the database of `var`; None (skipped) while it is unset.
async fn open(var: &str, read_only: bool) -> Option<Arc<dyn DbSession>> {
    let url = common::url(var)?;
    common::install_crypto();
    let (mut profile, token) = common::profile_from_url(&url);
    // The account is the URL's host.
    let account = profile.host.take().unwrap_or_default();
    let params = serde_json::json!({ "accountId": account });
    profile.options.driver_params = params.as_object().cloned().map(DriverParams);
    profile.options.read_only = read_only;
    let session = connect(ConnectTarget {
        profile: &profile,
        password: token.as_deref(),
        host_override: None,
    })
    .await
    .unwrap_or_else(|e| panic!("{var}: connect failed: {e}"));
    Some(session)
}

async fn run(session: &dyn DbSession, sql: &str) -> Vec<tabler_lib::types::query::StatementResult> {
    let options = RunOptions {
        max_rows: 2,
        stop_on_error: false,
        ..RunOptions::default()
    };
    let mut emit = |_| {};
    run_sql_inner(session, sql, &options, &CancelSlot::default(), &mut emit).await
}

#[tokio::test]
#[ignore = "set TABLER_TEST_D1_URL"]
async fn real_server() {
    let Some(s) = open("TABLER_TEST_D1_URL", false).await else {
        return;
    };
    let table = format!("tabler_it_{}", std::process::id());
    let t = TableRef::new(Some("main"), &table);
    s.execute(&format!("DROP TABLE IF EXISTS {table}"), &[])
        .await
        .unwrap();
    s.execute(
        &format!(
            "CREATE TABLE {table} (id INTEGER PRIMARY KEY, v TEXT UNIQUE, big INTEGER, price REAL)"
        ),
        &[],
    )
    .await
    .unwrap();
    let insert = format!("INSERT INTO {table} (v, big, price) VALUES (?1, ?2, ?3)");
    s.transaction(vec![
        TxStatement::new(
            &insert,
            vec![
                Value::Text("a".into()),
                Value::BigInt("9007199254740993".into()),
                Value::Float(2.5),
            ],
        ),
        TxStatement::new(
            &insert,
            vec![Value::Text("b".into()), Value::Int(2), Value::Null],
        ),
        TxStatement::new(
            &insert,
            vec![Value::Text("c".into()), Value::Int(3), Value::Null],
        ),
    ])
    .await
    .unwrap();

    // Connect, list and structure; the account's databases.
    let info = s.server_info().await.unwrap();
    assert!(
        info.version.starts_with("Cloudflare D1"),
        "{}",
        info.version
    );
    assert!(
        s.list_databases()
            .await
            .unwrap()
            .iter()
            .any(|d| d.is_current)
    );
    let tables = s.list_tables("main").await.unwrap();
    assert!(tables.iter().any(|x| x.name == table));
    assert!(!tables.iter().any(|x| x.name.starts_with("_cf_")));
    let meta = s.table_meta(&t).await.unwrap();
    assert_eq!(meta.primary_key, ["id"]);
    let caps = s.capabilities();
    assert!(caps.has(Feature::EditRows) && !caps.has(Feature::EditWithoutKey));
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);

    // Browse with paging (an integer beyond 2^53 read exactly), count, fetch.
    let page = |offset| BrowseRequest {
        table: t.clone(),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit: 2,
        offset,
        cursor: None,
    };
    let first = browse_page(s.as_ref(), &meta, &page(0), None)
        .await
        .unwrap();
    assert_eq!(first.rows[0][2], Value::BigInt("9007199254740993".into()));
    let second = browse_page(s.as_ref(), &meta, &page(2), None)
        .await
        .unwrap();
    assert_eq!(second.rows.len(), 1);
    let counted = s
        .query(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .unwrap();
    assert_eq!(counted.rows, [[Value::Int(3)]]);
    let key = RowKey::Pk([("id".to_string(), Value::Int(1))].into());
    let v = fetch_cell_inner(s.as_ref(), &meta, "v", &key)
        .await
        .unwrap();
    assert_eq!(v, Value::Text("a".into()));

    // A commit with one failing change applies nothing (the batch is one transaction).
    let failed = s
        .transaction(vec![
            TxStatement::new(format!("UPDATE {table} SET v = 'z' WHERE id = 1"), vec![])
                .expecting(1),
            TxStatement::new(
                &insert,
                vec![Value::Text("b".into()), Value::Null, Value::Null],
            ),
        ])
        .await;
    assert!(failed.is_err());
    let z = s
        .query(&format!("SELECT count(*) FROM {table} WHERE v = 'z'"), &[])
        .await
        .unwrap();
    assert_eq!(z.rows, [[Value::Int(0)]]);

    // The editor: one request per statement, no SQL transactions.
    let results = run(
        s.as_ref(),
        &format!("SELECT * FROM {table}; INSERT INTO {table} (v) VALUES ('d'); BEGIN"),
    )
    .await;
    assert!(results[0].truncated);
    assert_eq!(results[1].kind, StatementKind::Affected);
    assert!(results[2].error.is_some(), "D1 refuses BEGIN");

    // A cancel and a time out (client side: D1 finishes the statement).
    let slot = CancelSlot::default();
    let mut sink = CollectSink::default();
    let statement = s.run_statement(SLOW, 10, &mut sink, slot.clone());
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Some(handle) = request_cancel(&slot) {
            s.cancel(handle).await.unwrap();
        }
    };
    let (result, ()) = tokio::join!(statement, cancel);
    assert!(matches!(result, Err(AppError::Cancelled)), "{result:?}");
    let limited = query_limited(s.as_ref(), SLOW, &[], Some(Duration::from_millis(300))).await;
    assert!(
        matches!(limited, Err(AppError::QueryTimeout { .. })),
        "{limited:?}"
    );

    // D1 runs every statement of a text: the session sends one. And D1 refuses a variable it is
    // given no value for, the editor's case, so a variable SQLite reads as one token (`$a(--)`)
    // cannot hide a DELETE from the classifier.
    let rows = |s: Arc<dyn DbSession>| {
        let sql = format!("SELECT count(*) FROM {table}");
        async move { s.query(&sql, &[]).await.unwrap().rows }
    };
    let before = rows(s.clone()).await;
    assert!(
        s.query(&format!("SELECT 1; DELETE FROM {table}"), &[])
            .await
            .is_err()
    );
    let mut sink = CollectSink::default();
    let hidden = format!("WITH x AS (SELECT $a(--)) DELETE FROM {table}");
    let refused = s
        .run_statement(&hidden, 10, &mut sink, CancelSlot::default())
        .await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(rows(s.clone()).await, before);

    // A read-only session refuses writes before sending them.
    if let Some(ro) = open("TABLER_TEST_D1_URL", true).await {
        assert!(
            ro.execute(&format!("DELETE FROM {table}"), &[])
                .await
                .is_err()
        );
        assert_eq!(
            ro.query(&format!("SELECT count(*) FROM {table}"), &[])
                .await
                .unwrap()
                .rows
                .len(),
            1
        );
    }

    s.execute(&format!("DROP TABLE {table}"), &[])
        .await
        .unwrap();
    s.close().await;
}
