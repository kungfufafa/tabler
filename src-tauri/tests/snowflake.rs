//! Snowflake against a real account: the snowflake lane's suite (docs/drivers/snowflake.md).
//! Built only with `drv-snowflake` (`required-features` in Cargo.toml) and run with
//! `node dev/scripts/lane.mjs snowflake it`, which loads `.env`. Each test skips itself while
//! `TABLER_TEST_SNOWFLAKE_URL` is unset, and is `#[ignore]`d so that a plain `cargo test
//! --all-features` never reaches a server (the queries bill the warehouse).
//!
//! The URL: `snowflake://USER[:SECRET]@ACCOUNT/DATABASE/SCHEMA?warehouse=W&role=R&auth=…`, with
//! `auth=keyPair&privateKeyPath=<file>[&passphrase=<key passphrase>]` (the default when a
//! `privateKeyPath` is given), `auth=pat` (SECRET is the token) or `auth=password` (SECRET is the
//! password; an account that asks MFA cannot run unattended tests). Percent-escape what the URL
//! needs escaped.
//!
//! The tests make their own tables (`TABLER_IT_<uuid>` in the URL's schema) and drop them. Beyond
//! what the fake Snowflake of the unit tests proves, they check what only an account can: the
//! result format this client gets, SHOW and its columns, `GET_DDL`, a commit guarded by its
//! counts, the editor's session state, the server's own cancel and the time out.

#![cfg(feature = "drv-snowflake")]
#![allow(clippy::result_large_err)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use tabler_lib::commands::browse::{browse_page, fetch_cell_inner};
use tabler_lib::commands::query::query_limited;
use tabler_lib::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, StatementOutcome, TxStatement, connect,
    request_cancel,
};
use tabler_lib::error::AppError;
use tabler_lib::secrets::ExtraSecrets;
use tabler_lib::types::browse::{BrowseRequest, RowKey};
use tabler_lib::types::caps::Feature;
use tabler_lib::types::driver::ExtraSecret;
use tabler_lib::types::profile::ConnectionProfile;
use tabler_lib::types::query::StatementKind;
use tabler_lib::types::schema::TableRef;
use tabler_lib::types::value::Value;

const VAR: &str = "TABLER_TEST_SNOWFLAKE_URL";

/// `%XX` escapes decoded.
fn pct(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct Account {
    profile: ConnectionProfile,
    secret: Option<String>,
    passphrase: Option<String>,
    schema: String,
}

/// The profile of the URL in `TABLER_TEST_SNOWFLAKE_URL`; None (skipped) when it is unset.
fn account(read_only: bool) -> Option<Account> {
    let url = common::url(VAR)?;
    let rest = url
        .strip_prefix("snowflake://")
        .unwrap_or_else(|| panic!("{VAR}: not a snowflake:// URL"));
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (auth, path) = rest
        .rsplit_once('@')
        .unwrap_or_else(|| panic!("{VAR}: no USER@ACCOUNT"));
    let (user, secret) = match auth.split_once(':') {
        Some((u, s)) => (pct(u), Some(pct(s))),
        None => (pct(auth), None),
    };
    let mut parts = path.split('/');
    let account = parts.next().unwrap_or_default().to_string();
    let database = parts.next().map(pct).filter(|d| !d.is_empty());
    let schema = parts
        .next()
        .map(pct)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "PUBLIC".into());
    let mut params = serde_json::json!({ "account": account, "schema": schema });
    let mut passphrase = None;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = pct(v);
        match k {
            "passphrase" => passphrase = Some(v),
            "auth" | "warehouse" | "role" | "privateKeyPath" | "baseUrl" => {
                params[k] = serde_json::Value::String(v)
            }
            other => panic!("{VAR}: unknown parameter {other}"),
        }
    }
    if params.get("auth").is_none() {
        let auth = if params.get("privateKeyPath").is_some() {
            "keyPair"
        } else {
            "pat"
        };
        params["auth"] = auth.into();
    }
    let profile = serde_json::from_value(serde_json::json!({
        "id": uuid::Uuid::new_v4(),
        "name": "snowflake-test",
        "driver": "snowflake",
        "user": user,
        "database": database,
        "options": { "connectTimeoutSecs": 60, "driverParams": params, "readOnly": read_only },
    }))
    .unwrap_or_else(|e| panic!("{VAR}: {e}"));
    Some(Account {
        profile,
        secret,
        passphrase,
        schema,
    })
}

async fn open_account(a: &Account) -> Arc<dyn DbSession> {
    common::install_crypto();
    let mut extras = ExtraSecrets::default();
    if let Some(p) = &a.passphrase {
        extras.set(ExtraSecret::KeyPassphrase, p.clone());
    }
    let target = ConnectTarget {
        profile: &a.profile,
        password: a.secret.as_deref(),
        host_override: None,
    };
    extras
        .scope(connect(target))
        .await
        .unwrap_or_else(|e| panic!("{VAR}: connect failed: {e}"))
}

async fn open(read_only: bool) -> Option<(Arc<dyn DbSession>, Account)> {
    let a = account(read_only)?;
    Some((open_account(&a).await, a))
}

fn scratch() -> String {
    format!("TABLER_IT_{}", uuid::Uuid::new_v4().simple()).to_uppercase()
}

async fn run(s: &dyn DbSession, sql: &str) -> Result<(StatementOutcome, CollectSink), AppError> {
    let mut sink = CollectSink::default();
    let out = s
        .run_statement(sql, 10_000, &mut sink, CancelSlot::default())
        .await?;
    Ok((out, sink))
}

/// Drops the table when the test ends, pass or fail.
struct Cleanup(Arc<dyn DbSession>, String);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let (s, sql) = (Arc::clone(&self.0), self.1.clone());
        let done = std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .ok()
                .map(|rt| rt.block_on(async move { run(s.as_ref(), &sql).await.map(|_| ()) }))
        })
        .join();
        if !matches!(done, Ok(Some(Ok(())))) {
            println!("cleanup failed: {}", self.1);
        }
    }
}

#[tokio::test]
#[ignore = "set TABLER_TEST_SNOWFLAKE_URL"]
async fn connects_lists_browses_counts_and_commits() {
    let Some((s, a)) = open(false).await else {
        return;
    };
    let info = s.server_info().await.unwrap();
    assert!(info.version.starts_with("Snowflake "), "{info:?}");
    s.ping().await.unwrap();
    let caps = s.capabilities();
    assert!(caps.has(Feature::EditRows) && caps.has(Feature::CancelOnServer));

    let table = scratch();
    let q = format!("\"{}\".\"{table}\"", a.schema);
    run(
        s.as_ref(),
        &format!(
            "CREATE TABLE {q} (ID NUMBER(38,0) PRIMARY KEY, NAME VARCHAR(100), PRICE NUMBER(10,2), \
             BIG NUMBER(38,0), DOC VARIANT, AT TIMESTAMP_NTZ(3))"
        ),
    )
    .await
    .unwrap();
    let _cleanup = Cleanup(Arc::clone(&s), format!("DROP TABLE IF EXISTS {q}"));
    let (ins, _) = run(
        s.as_ref(),
        &format!(
            "INSERT INTO {q} SELECT 1, 'pen', 1.50, 9007199254740993, PARSE_JSON('{{\"a\":[1,2]}}'), \
             '2024-05-01 13:45:00.123' UNION ALL SELECT 2, 'ink', 2.00, 2, NULL, NULL \
             UNION ALL SELECT 3, '日本語', 3.25, 3, NULL, NULL"
        ),
    )
    .await
    .unwrap();
    assert_eq!(ins.kind, StatementKind::Affected);
    assert_eq!(ins.affected, Some(3));

    let tables = s.list_tables(&a.schema).await.unwrap();
    assert!(tables.iter().any(|t| t.name == table));
    let tref = TableRef {
        schema: Some(a.schema.clone()),
        name: table.clone(),
    };
    let meta = s.table_meta(&tref).await.unwrap();
    assert_eq!(meta.primary_key, vec!["ID"]);
    assert_eq!(meta.columns.len(), 6);
    assert!(
        s.table_ddl(&tref)
            .await
            .unwrap()
            .unwrap()
            .to_uppercase()
            .contains("CREATE OR REPLACE TABLE")
    );

    let req = BrowseRequest {
        table: tref.clone(),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit: 2,
        offset: 0,
        cursor: None,
    };
    let page = browse_page(s.as_ref(), &meta, &req, None).await.unwrap();
    assert_eq!(page.rows.len(), 2);
    let second = browse_page(
        s.as_ref(),
        &meta,
        &BrowseRequest {
            offset: 2,
            ..req.clone()
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(second.rows.len(), 1);
    let all = [page.rows, second.rows].concat();
    let pen = all.iter().find(|r| r[0] == Value::Int(1)).expect("row 1");
    assert_eq!(pen[2], Value::Decimal("1.50".into()));
    assert_eq!(pen[3], Value::BigInt("9007199254740993".into()));
    assert!(matches!(&pen[4], Value::Json(_)), "{:?}", pen[4]);
    assert_eq!(pen[5], Value::Timestamp("2024-05-01 13:45:00.123".into()));
    assert!(all.iter().any(|r| r[1] == Value::Text("日本語".into())));

    let (count, sink) = run(s.as_ref(), &format!("SELECT COUNT(*) FROM {q}"))
        .await
        .unwrap();
    assert_eq!(count.kind, StatementKind::Rows);
    assert_eq!(sink.sets[0].rows[0][0], Value::Int(3));

    let key = RowKey::Pk([("ID".to_string(), Value::Int(1))].into_iter().collect());
    let doc = fetch_cell_inner(s.as_ref(), &meta, "DOC", &key)
        .await
        .unwrap();
    assert!(matches!(doc, Value::Json(_)), "{doc:?}");

    // A commit with one failing change: the key is informational, so a duplicate ID matches
    // two rows, and the whole commit rolls back.
    run(
        s.as_ref(),
        &format!("INSERT INTO {q} (ID, NAME) VALUES (2, 'dup')"),
    )
    .await
    .unwrap();
    let update = format!("UPDATE {q} SET NAME = ? WHERE ID = ?");
    let err = s
        .transaction(vec![
            TxStatement::new(&update, vec![Value::Text("x".into()), Value::Int(1)]).expecting(1),
            TxStatement::new(&update, vec![Value::Text("y".into()), Value::Int(2)]).expecting(1),
        ])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("matched 2"), "{err}");
    let (_, sink) = run(s.as_ref(), &format!("SELECT NAME FROM {q} WHERE ID = 1"))
        .await
        .unwrap();
    assert_eq!(sink.sets[0].rows[0][0], Value::Text("pen".into()));
    s.transaction(vec![
        TxStatement::new(&update, vec![Value::Text("x".into()), Value::Int(1)]).expecting(1),
    ])
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "set TABLER_TEST_SNOWFLAKE_URL"]
async fn the_editor_keeps_its_state_and_cancels_on_the_server() {
    let Some((s, _a)) = open(false).await else {
        return;
    };
    run(s.as_ref(), "SET tabler_it = 42").await.unwrap();
    let (_, sink) = run(s.as_ref(), "SELECT $tabler_it").await.unwrap();
    assert_eq!(sink.sets[0].rows[0][0], Value::Int(42));
    run(s.as_ref(), "BEGIN").await.unwrap();
    assert_eq!(s.in_transaction(), Some(true));
    run(s.as_ref(), "ROLLBACK").await.unwrap();
    assert_eq!(s.in_transaction(), Some(false));

    // Cancel: SYSTEM$WAIT holds the statement on the server until the abort arrives.
    let slot = CancelSlot::default();
    let running = {
        let (s, slot) = (Arc::clone(&s), slot.clone());
        tokio::spawn(async move {
            let mut sink = CollectSink::default();
            s.run_statement("SELECT SYSTEM$WAIT(60)", 10, &mut sink, slot)
                .await
        })
    };
    tokio::time::sleep(Duration::from_secs(3)).await;
    let handle = request_cancel(&slot).expect("the statement armed the slot");
    s.cancel(handle).await.unwrap();
    let err = running.await.unwrap().unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");

    // The time out cancels on the server too.
    let err = query_limited(
        s.as_ref(),
        "SELECT SYSTEM$WAIT(60)",
        &[],
        Some(Duration::from_secs(3)),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::QueryTimeout { .. }), "{err:?}");
}

#[tokio::test]
#[ignore = "set TABLER_TEST_SNOWFLAKE_URL"]
async fn read_only_refuses_writes_and_reads_on() {
    let Some((s, a)) = open(true).await else {
        return;
    };
    for sql in [
        format!("CREATE TABLE \"{}\".\"{}\" (A INT)", a.schema, scratch()),
        "SELECT SYSTEM$CANCEL_ALL_QUERIES(0)".into(),
        "CALL SYSTEM$WAIT(1)".into(),
    ] {
        let err = run(s.as_ref(), &sql).await.unwrap_err();
        assert!(err.to_string().contains("read-only"), "{sql}: {err}");
        assert!(s.execute(&sql, &[]).await.is_err());
    }
    let (out, _) = run(s.as_ref(), "SHOW SCHEMAS").await.unwrap();
    assert_eq!(out.kind, StatementKind::Rows);
}
