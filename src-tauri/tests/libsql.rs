//! LibSQL (Turso, `sqld`) against a real server: the sqlite-remote lane's suite
//! (docs/drivers/sqlite-remote.md). Built only with `drv-libsql` (`required-features` in
//! Cargo.toml) and run with `node dev/scripts/lane.mjs sqlite-remote it`, which loads `.env`. Each
//! test skips itself while its variable is unset, and is `#[ignore]`d so that a plain `cargo test
//! --all-features` never reaches a server (Turso bills the rows read).
//!
//! `TABLER_TEST_LIBSQL_URL` is the URL as the connection form takes it: `libsql://<db>-<org>.turso.io
//! ?authToken=<token>` for Turso, `http://127.0.0.1:8080` for a local `sqld`. The token goes into
//! the password, as the form does. The test works in a table of its own (`tabler_it_<pid>`),
//! which it drops again.

#![cfg(feature = "drv-libsql")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use tabler_lib::commands::browse::{browse_page, fetch_cell_inner};
use tabler_lib::commands::query::{explain_inner, query_limited, run_sql_inner};
use tabler_lib::drivers::sqlite_http::libsql::url::parse_url;
use tabler_lib::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, TxStatement, connect, request_cancel,
};
use tabler_lib::error::AppError;
use tabler_lib::types::browse::{BrowseRequest, RowKey};
use tabler_lib::types::caps::Feature;
use tabler_lib::types::driver::ReadOnlyMode;
use tabler_lib::types::profile::ConnectionProfile;
use tabler_lib::types::query::{ExplainMode, RunOptions, StatementKind};
use tabler_lib::types::schema::TableRef;
use tabler_lib::types::value::Value;

/// A statement that keeps the server busy for a few seconds.
const SLOW: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c LIMIT 100000000) \
     SELECT count(*) FROM c";

/// The profile of the URL in `var` (its server in the `url` param), and its token; None
/// (skipped) while the variable is unset.
fn profile_of(var: &str, read_only: bool) -> Option<(ConnectionProfile, Option<String>)> {
    let url = common::url(var)?;
    let parsed = parse_url(&url).unwrap_or_else(|e| panic!("{var}: {e}"));
    let profile = serde_json::from_value(serde_json::json!({
        "id": uuid::Uuid::new_v4(),
        "name": "libsql-test",
        "driver": "libsql",
        // A new profile's mode: HTTPS checks the certificate (a URL's scheme says which).
        "ssl": "prefer",
        "options": {
            "connectTimeoutSecs": 10,
            "readOnly": read_only,
            "driverParams": { "url": parsed.base },
        },
    }))
    .unwrap_or_else(|e| panic!("{var}: {e}"));
    Some((profile, parsed.token))
}

async fn open(var: &str, read_only: bool) -> Option<Arc<dyn DbSession>> {
    let (profile, token) = profile_of(var, read_only)?;
    common::install_crypto();
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
        ..RunOptions::default()
    };
    let mut emit = |_| {};
    run_sql_inner(session, sql, &options, &CancelSlot::default(), &mut emit).await
}

#[tokio::test]
#[ignore = "set TABLER_TEST_LIBSQL_URL"]
async fn real_server() {
    let Some(s) = open("TABLER_TEST_LIBSQL_URL", false).await else {
        return;
    };
    let table = format!("tabler_it_{}", std::process::id());
    let t = TableRef::new(Some("main"), &table);
    s.execute(&format!("DROP TABLE IF EXISTS {table}"), &[])
        .await
        .unwrap();
    s.execute(
        &format!(
            "CREATE TABLE {table} (id INTEGER PRIMARY KEY, v TEXT UNIQUE, big INTEGER, b BLOB)"
        ),
        &[],
    )
    .await
    .unwrap();
    let insert = format!("INSERT INTO {table} (v, big, b) VALUES (?1, ?2, ?3)");
    s.transaction(vec![
        TxStatement::new(
            &insert,
            vec![
                Value::Text("a".into()),
                Value::BigInt("9007199254740993".into()),
                Value::from_bytes(&[1, 2, 3]),
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

    // Connect, list and structure.
    let info = s.server_info().await.unwrap();
    assert!(
        info.version.starts_with("LibSQL (SQLite"),
        "{}",
        info.version
    );
    let tables = s.list_tables("main").await.unwrap();
    assert!(tables.iter().any(|x| x.name == table));
    let meta = s.table_meta(&t).await.unwrap();
    assert_eq!(meta.primary_key, ["id"]);
    let caps = s.capabilities();
    assert!(caps.has(Feature::EditRows));
    // A writable session never asks whether the server takes `query_only`.
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);

    // Browse with paging, count, fetch.
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
    assert_eq!(first.rows.len(), 2);
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
    let blob = fetch_cell_inner(s.as_ref(), &meta, "b", &key)
        .await
        .unwrap();
    assert_eq!(blob, Value::from_bytes(&[1, 2, 3]));

    // A commit with one failing change applies nothing.
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

    // The editor: a transaction across statements, a capped read.
    let results = run(
        s.as_ref(),
        &format!("BEGIN; INSERT INTO {table} (v) VALUES ('d'); SELECT * FROM {table}; ROLLBACK"),
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(results[1].kind, StatementKind::Affected);
    assert!(results[2].truncated);
    assert_eq!(s.in_transaction(), Some(false));

    // A cancel and a time out (the server finishes the statement: no cancel of its own).
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

    // A read-only session: it connects (`sqld` and Turso refuse `PRAGMA query_only = 1`, so the
    // session found that out and left it off) and refuses writes itself.
    if let Some(ro) = open("TABLER_TEST_LIBSQL_URL", true).await {
        assert_eq!(
            ro.capabilities().safety.read_only,
            ReadOnlyMode::ClassifierOnly,
            "the server took PRAGMA query_only = 1"
        );
        let refused = ro
            .execute(&format!("DELETE FROM {table}"), &[])
            .await
            .unwrap_err();
        assert!(refused.to_string().contains("read-only"), "{refused}");
        let mut sink = CollectSink::default();
        assert!(
            ro.run_statement(
                &format!("DELETE FROM {table}"),
                10,
                &mut sink,
                CancelSlot::default()
            )
            .await
            .is_err()
        );
        // `sqld` reads any EXPLAIN QUERY PLAN as a read, but SQLite applies the PRAGMA.
        let explained = explain_inner(
            ro.as_ref(),
            "PRAGMA soft_heap_limit = 7340033",
            ExplainMode::Plan,
            &CancelSlot::default(),
        )
        .await;
        assert!(explained.is_err(), "{explained:?}");
        let plan = explain_inner(
            ro.as_ref(),
            &format!("SELECT * FROM {table}"),
            ExplainMode::Plan,
            &CancelSlot::default(),
        )
        .await
        .unwrap();
        assert!(!plan.rows.is_empty());
        // A startup command that writes fails the connect.
        if let Some((mut profile, token)) = profile_of("TABLER_TEST_LIBSQL_URL", true) {
            profile.options.startup_sql = Some(format!(
                "PRAGMA query_only = 0; INSERT INTO {table} (v) VALUES ('boot')"
            ));
            let opened = connect(ConnectTarget {
                profile: &profile,
                password: token.as_deref(),
                host_override: None,
            })
            .await;
            assert!(opened.is_err(), "a read-only startup command wrote");
        }
        let boot = s
            .query(
                &format!("SELECT count(*) FROM {table} WHERE v = 'boot'"),
                &[],
            )
            .await
            .unwrap();
        assert_eq!(boot.rows, [[Value::Int(0)]]);
    }

    s.execute(&format!("DROP TABLE {table}"), &[])
        .await
        .unwrap();
    s.close().await;
}
