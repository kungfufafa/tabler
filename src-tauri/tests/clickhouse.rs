//! ClickHouse against a real server: the clickhouse lane's suite (docs/drivers/clickhouse.md).
//! Built only with `drv-clickhouse` (`required-features` in Cargo.toml) and run with
//! `node dev/scripts/lane.mjs clickhouse it`, which loads `.env`. Each test skips itself while its
//! variable is unset, and is `#[ignore]`d so that a plain `cargo test --all-features` never
//! reaches a server (a cloud one bills the queries).
//!
//! `TABLER_TEST_CLICKHOUSE_URL` is `clickhouse://user:password@host:8123/database` for plain HTTP
//! or `clickhouses://…:8443/…` for HTTPS with the certificate checked. The user needs to create
//! and drop a table in the database; the test works in one of its own (`tabler_it_<pid>`).

#![cfg(feature = "drv-clickhouse")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use tabler_lib::commands::browse::{browse_page, fetch_cell_inner};
use tabler_lib::commands::query::{query_limited, run_sql_inner};
use tabler_lib::drivers::clickhouse::client::Client;
use tabler_lib::drivers::clickhouse::entry::endpoint_of;
use tabler_lib::drivers::clickhouse::request::{ConnSettings, Credentials, Query};
use tabler_lib::drivers::clickhouse::transport::HttpTransport;
use tabler_lib::drivers::common::http::{Credential, HttpClient, HttpEndpoint, HttpOptions};
use tabler_lib::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, TxStatement, connect, request_cancel,
};
use tabler_lib::error::AppError;
use tabler_lib::types::admin::KillScope;
use tabler_lib::types::browse::{BrowseRequest, RowKey};
use tabler_lib::types::caps::Feature;
use tabler_lib::types::driver::ReadOnlyMode;
use tabler_lib::types::profile::{ConnectionProfile, SslMode};
use tabler_lib::types::query::{RunOptions, StatementKind, StatementResult};
use tabler_lib::types::schema::TableRef;
use tabler_lib::types::value::Value;

const VAR: &str = "TABLER_TEST_CLICKHOUSE_URL";

/// A read that keeps the server busy for many seconds.
const SLOW: &str = "SELECT count() FROM numbers(1000000000000)";

/// The profile of the URL in `VAR`, and its password; None (skipped) while it is unset.
fn profile_of(read_only: bool) -> Option<(ConnectionProfile, Option<String>)> {
    let url = common::url(VAR)?;
    let (https, url) = match url.strip_prefix("clickhouses://") {
        Some(rest) => (true, format!("clickhouse://{rest}")),
        None => (false, url),
    };
    let (mut profile, password) = common::profile_from_url(&url);
    profile.ssl = if https {
        SslMode::VerifyFull
    } else {
        SslMode::Disable
    };
    profile.options.read_only = read_only;
    Some((profile, password))
}

async fn open(read_only: bool) -> Option<(Arc<dyn DbSession>, ConnectionProfile, Option<String>)> {
    let (profile, password) = profile_of(read_only)?;
    common::install_crypto();
    let session = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap_or_else(|e| panic!("{VAR}: connect failed: {e}"));
    Some((session, profile, password))
}

async fn run(session: &dyn DbSession, sql: &str) -> Vec<StatementResult> {
    let options = RunOptions {
        max_rows: 2,
        ..RunOptions::default()
    };
    let mut emit = |_| {};
    run_sql_inner(session, sql, &options, &CancelSlot::default(), &mut emit).await
}

#[tokio::test]
#[ignore = "set TABLER_TEST_CLICKHOUSE_URL"]
async fn real_server() {
    let Some((s, profile, password)) = open(false).await else {
        return;
    };
    let db = profile.database.clone().unwrap_or_else(|| "default".into());
    let name = format!("tabler_it_{}", std::process::id());
    let table = format!("`{db}`.`{name}`");
    let t = TableRef::new(Some(&db), &name);
    s.execute(&format!("DROP TABLE IF EXISTS {table}"), &[])
        .await
        .unwrap();
    s.execute(
        &format!(
            "CREATE TABLE {table} (id UInt64, name String, amount Decimal(10, 2), \
             at DateTime('UTC'), tags Map(String, UInt8)) ENGINE = MergeTree ORDER BY id"
        ),
        &[],
    )
    .await
    .unwrap();
    s.execute(
        &format!(
            "INSERT INTO {table} VALUES (9007199254740993, 'a', 1.50, '2026-10-30 12:00:00', \
             {{'k': 1}}), (2, 'b', 2, now(), {{}}), (3, 'c', 3, now(), {{}})"
        ),
        &[],
    )
    .await
    .unwrap();

    // Connect, list and structure.
    let info = s.server_info().await.unwrap();
    assert!(info.version.starts_with("ClickHouse "), "{}", info.version);
    let schemas = s.list_schemas().await.unwrap();
    assert!(schemas.iter().any(|x| x.name == db));
    let tables = s.list_tables(&db).await.unwrap();
    assert!(tables.iter().any(|x| x.name == name));
    let meta = s.table_meta(&t).await.unwrap();
    assert_eq!(meta.primary_key, ["id"]);
    let caps = s.capabilities();
    assert!(caps.has(Feature::EditRows) && caps.has(Feature::CancelOnServer));
    assert_eq!(caps.safety.read_only, ReadOnlyMode::PerRequest);
    assert!(
        s.table_ddl(&t)
            .await
            .unwrap()
            .unwrap()
            .contains("MergeTree")
    );

    // Browse with paging (values exact), count, fetch.
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
    let second = browse_page(s.as_ref(), &meta, &page(2), None)
        .await
        .unwrap();
    assert_eq!(second.rows.len(), 1);
    let a = first
        .rows
        .iter()
        .chain(&second.rows)
        .find(|r| r[1] == Value::Text("a".into()))
        .expect("row a on one of the pages");
    assert_eq!(a[0], Value::BigInt("9007199254740993".into()));
    assert_eq!(a[2], Value::Decimal("1.50".into()));
    assert_eq!(a[3], Value::Timestamp("2026-10-30 12:00:00".into()));
    assert!(
        matches!(&a[4], Value::Json(j) if j.contains("\"k\"")),
        "{:?}",
        a[4]
    );
    let counted = s
        .query(&format!("SELECT count() FROM {table}"), &[])
        .await
        .unwrap();
    assert_eq!(counted.rows, [[Value::Int(3)]]);
    let key = RowKey::Pk([("id".to_string(), Value::Int(2))].into());
    let cell = fetch_cell_inner(s.as_ref(), &meta, "name", &key)
        .await
        .unwrap();
    assert_eq!(cell, Value::Text("b".into()));

    // A commit with one failing change: a key that matches two rows is refused, the change
    // before it stays applied (no transactions), and the error names the failed statement.
    s.execute(
        &format!("INSERT INTO {table} (id, name) VALUES (3, 'dup')"),
        &[],
    )
    .await
    .unwrap();
    let update =
        format!("ALTER TABLE {table} UPDATE name = {{p1:String}} WHERE id = {{p2:UInt64}}");
    let failed = s
        .transaction(vec![
            TxStatement::new(&update, vec![Value::Text("z".into()), Value::Int(2)]).expecting(1),
            TxStatement::new(&update, vec![Value::Text("y".into()), Value::Int(3)]).expecting(1),
        ])
        .await
        .unwrap_err();
    assert!(
        matches!(
            failed,
            AppError::Db {
                statement_index: Some(1),
                ..
            }
        ),
        "{failed:?}"
    );
    let z = s
        .query(
            &format!("SELECT count() FROM {table} WHERE name = 'z'"),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(z.rows, [[Value::Int(1)]]);
    let y = s
        .query(
            &format!("SELECT count() FROM {table} WHERE name = 'y'"),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(y.rows, [[Value::Int(0)]]);

    // The editor: a SET lasts in its session, a read is capped.
    let results = run(
        s.as_ref(),
        "SET max_threads = 1; SELECT getSetting('max_threads') AS t; SELECT number FROM numbers(10)",
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(results[1].kind, StatementKind::Rows);
    assert_eq!(results[2].kind, StatementKind::Rows);
    assert!(results[2].truncated);

    // A cancel and a time out, both stopped on the server by KILL QUERY.
    let slot = CancelSlot::default();
    let mut sink = CollectSink::default();
    let statement = s.run_statement(SLOW, 10, &mut sink, slot.clone());
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Some(handle) = request_cancel(&slot) {
            s.cancel(handle).await.unwrap();
        }
    };
    let (result, ()) = tokio::join!(statement, cancel);
    assert!(matches!(result, Err(AppError::Cancelled)), "{result:?}");
    let limited = query_limited(s.as_ref(), SLOW, &[], Some(Duration::from_millis(500))).await;
    assert!(
        matches!(limited, Err(AppError::QueryTimeout { .. })),
        "{limited:?}"
    );
    let processes = s.list_processes().await.unwrap().unwrap();
    assert!(processes.iter().any(|p| p.current), "{processes:?}");
    let killed = s
        .kill_processes(&["tabler-no-such-query".into()], KillScope::Query)
        .await
        .unwrap();
    assert!(killed[0].ok, "{killed:?}");

    // Read-only: the session refuses writes before sending, and the server refuses them under
    // `readonly=2` even when sent.
    if let Some((ro, _, _)) = open(true).await {
        assert!(ro.capabilities().safety.hardened);
        let refused = ro
            .execute(&format!("INSERT INTO {table} (id) VALUES (7)"), &[])
            .await
            .unwrap_err();
        assert!(refused.to_string().contains("read-only"), "{refused}");
        let bare = Client::new(
            Arc::new(HttpTransport::new(
                HttpClient::new(
                    HttpEndpoint::new(&endpoint_of(&profile).base_url(), None).unwrap(),
                    HttpOptions::for_profile(&profile).unwrap(),
                    Credential::None,
                )
                .unwrap(),
            )),
            endpoint_of(&profile),
            Credentials::Password {
                user: profile.user.clone().unwrap_or_else(|| "default".into()),
                password: password.clone().unwrap_or_default(),
            },
            ConnSettings {
                database: Some(db.clone()),
                read_only: true,
                output_settings: true,
                ..ConnSettings::default()
            },
        );
        for sql in [
            format!("INSERT INTO {table} (id) VALUES (7)"),
            format!("CREATE TABLE `{db}`.`{name}_ro` (a UInt8) ENGINE = Memory"),
            format!("ALTER TABLE {table} UPDATE name = 'ro' WHERE 1"),
            // A statement's own setting cannot lift it.
            format!("INSERT INTO {table} (id) SETTINGS readonly = 0 VALUES (7)"),
        ] {
            let e = bare
                .rows(&Query {
                    sql: &sql,
                    ..Query::default()
                })
                .await
                .unwrap_err();
            assert!(
                matches!(
                    e,
                    AppError::Db {
                        vendor_code: Some(164),
                        ..
                    }
                ),
                "{sql}: {e:?}"
            );
        }
        ro.close().await;
    }

    s.execute(&format!("DROP TABLE {table}"), &[])
        .await
        .unwrap();
    s.close().await;
}
