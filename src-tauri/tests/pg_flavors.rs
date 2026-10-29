//! CockroachDB, Greenplum and Amazon Redshift against a real server: the pg-flavors lane's suite
//! (docs/drivers/pg-flavors.md). Built only with `drv-pg-flavors` (`required-features` in
//! Cargo.toml) and run with `node dev/scripts/lane.mjs pg-flavors it`, which loads `.env`. Each
//! test skips itself while its variable is unset, and is `#[ignore]`d so that a plain
//! `cargo test --all-features` never reaches a server (a cloud one bills the queries).
//!
//! The variables are PostgreSQL URLs (`postgres://user:password@host:port/database`, with an
//! optional `?sslmode=require`); the profile names the engine, as the connection form does. Each
//! test works in a table of its own in `public` (`tabler_it_<pid>`), which it drops again.

#![cfg(feature = "drv-pg-flavors")]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use tabler_lib::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, TxStatement, connect, request_cancel,
};
use tabler_lib::sqlgen::SqlDialect;
use tabler_lib::sqlgen::dialect::redshift::SPEC;
use tabler_lib::sqlgen::portable::mutate::build_change;
use tabler_lib::sqlgen::sql::TableTarget;
use tabler_lib::types::browse::RowKey;
use tabler_lib::types::caps::Feature;
use tabler_lib::types::driver::{Flavor, PgFlavor, ReadOnlyMode};
use tabler_lib::types::mutate::StagedChange;
use tabler_lib::types::profile::SslMode;
use tabler_lib::types::schema::{TableKind, TableRef};
use tabler_lib::types::value::Value;

/// A session on the server of `var` as a profile of `flavor`; None (skipped) while it is unset.
async fn open(var: &str, flavor: PgFlavor) -> Option<Arc<dyn DbSession>> {
    let url = common::url(var)?;
    common::install_crypto();
    let rest = url.split_once("://").map_or(url.as_str(), |(_, r)| r);
    let (mut profile, password) = common::profile_from_url(&format!("postgres://{rest}"));
    profile.options.pg_flavor = Some(flavor);
    let sslmode = url
        .split_once('?')
        .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("sslmode=")));
    profile.ssl = match sslmode {
        Some("disable") => SslMode::Disable,
        Some("require") => SslMode::Require,
        Some("verify-ca") => SslMode::VerifyCa,
        Some("verify-full") => SslMode::VerifyFull,
        _ => SslMode::Prefer,
    };
    let session = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap_or_else(|e| panic!("{var}: connect failed: {e}"));
    Some(session)
}

/// Runs `sql` in the editor; the rows of its first result.
async fn run(session: &dyn DbSession, sql: &str) -> Vec<Vec<Value>> {
    let mut sink = CollectSink::default();
    session
        .run_statement(sql, 1_000, &mut sink, CancelSlot::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    sink.sets
        .into_iter()
        .next()
        .map(|s| s.rows)
        .unwrap_or_default()
}

fn scratch() -> TableRef {
    TableRef::new(Some("public"), &format!("tabler_it_{}", std::process::id()))
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(n) => *n,
        Value::BigInt(s) | Value::Decimal(s) | Value::Text(s) => s.parse().unwrap(),
        other => panic!("not a number: {other:?}"),
    }
}

/// What every engine does: name itself, list, browse with paging, count, fetch, a commit with
/// one failing change (nothing of it stays), its sessions and its DDL. `create` makes the
/// scratch table `(id, name)` keyed by `id`.
async fn exercise(session: &dyn DbSession, flavor: Flavor, create: &str) -> String {
    let t = scratch();
    let q = format!("public.{}", t.name);
    assert_eq!(session.flavor(), Some(flavor));
    assert!(!session.server_info().await.unwrap().version.is_empty());
    assert!(!session.list_databases().await.unwrap().is_empty());
    let schemas = session.list_schemas().await.unwrap();
    assert!(schemas.iter().any(|s| s.name == "public"), "{schemas:?}");

    run(session, &format!("DROP TABLE IF EXISTS {q}")).await;
    run(session, create).await;
    run(
        session,
        &format!("INSERT INTO {q} (id, name) VALUES (1, 'one'), (2, 'two'), (3, 'three')"),
    )
    .await;

    let tables = session.list_tables("public").await.unwrap();
    let listed = tables.iter().find(|x| x.name == t.name).expect("listed");
    assert_eq!(listed.kind, TableKind::Table);
    let meta = session.table_meta(&t).await.unwrap();
    assert_eq!(meta.primary_key, ["id"]);
    assert!(meta.columns.iter().any(|c| c.name == "name"));

    let page = session
        .query(
            &format!("SELECT id, name FROM {q} ORDER BY id LIMIT 2 OFFSET 1"),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 2);
    assert_eq!(int(&page.rows[0][0]), 2);
    let count = session
        .query(&format!("SELECT count(*) FROM {q}"), &[])
        .await
        .unwrap();
    assert_eq!(int(&count.rows[0][0]), 3);
    let fetched = session
        .query(
            &format!("SELECT name FROM {q} WHERE id = $1::int"),
            &[Value::Int(2)],
        )
        .await
        .unwrap();
    assert_eq!(fetched.rows[0][0], Value::Text("two".into()));

    let failed = session
        .transaction(vec![
            TxStatement {
                sql: format!("UPDATE {q} SET name = 'uno' WHERE id = 1"),
                params: Vec::new(),
                expect_affected: Some(1),
            },
            TxStatement {
                sql: format!("UPDATE {q} SET name = 'none' WHERE id = 99"),
                params: Vec::new(),
                expect_affected: Some(1),
            },
        ])
        .await;
    assert!(failed.is_err());
    let kept = run(session, &format!("SELECT name FROM {q} WHERE id = 1")).await;
    assert_eq!(kept[0][0], Value::Text("one".into()));

    let processes = session
        .list_processes()
        .await
        .expect("the engine lists its sessions")
        .unwrap();
    assert!(processes.iter().any(|p| p.current), "{processes:?}");

    let ddl = match session.table_ddl(&t).await {
        Some(ddl) => ddl.unwrap(),
        None => String::new(),
    };
    run(session, &format!("DROP TABLE {q}")).await;
    ddl
}

/// A statement the user stops ends at once, and one past the statement timeout fails.
async fn cancels_and_times_out(session: Arc<dyn DbSession>) {
    let slot = CancelSlot::default();
    let task = {
        let session = session.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut sink = CollectSink::default();
            session
                .run_statement("SELECT pg_sleep(30)", 10, &mut sink, slot)
                .await
        })
    };
    for _ in 0..500 {
        if slot.is_armed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    session
        .cancel(request_cancel(&slot).expect("a live handle"))
        .await
        .unwrap();
    assert!(task.await.unwrap().is_err());
    assert!(started.elapsed() < Duration::from_secs(5));

    run(session.as_ref(), "SET statement_timeout = '1s'").await;
    let mut sink = CollectSink::default();
    let timed_out = session
        .run_statement("SELECT pg_sleep(3)", 10, &mut sink, CancelSlot::default())
        .await;
    assert!(timed_out.is_err());
    run(session.as_ref(), "RESET statement_timeout").await;
}

#[tokio::test]
#[ignore = "set TABLER_TEST_CRDB_URL"]
async fn cockroachdb_server() {
    let Some(session) = open("TABLER_TEST_CRDB_URL", PgFlavor::Cockroachdb).await else {
        return;
    };
    assert_eq!(session.dialect(), SqlDialect::Postgres);
    let caps = session.capabilities();
    assert!(caps.has(Feature::KillProcess) && !caps.has(Feature::StructureEdit));
    let t = scratch();
    let ddl = exercise(
        session.as_ref(),
        Flavor::Cockroachdb,
        &format!(
            "CREATE TABLE public.{} (id INT8 PRIMARY KEY, name VARCHAR(20))",
            t.name
        ),
    )
    .await;
    assert!(ddl.contains("CREATE TABLE"), "{ddl}");

    // A table without a key gets the hidden rowid, which stays its (read-only) key.
    let q = format!("public.{}_nokey", t.name);
    run(session.as_ref(), &format!("DROP TABLE IF EXISTS {q}")).await;
    run(
        session.as_ref(),
        &format!("CREATE TABLE {q} (a INT8, b DECIMAL(10,2))"),
    )
    .await;
    let meta = session
        .table_meta(&TableRef::new(Some("public"), &format!("{}_nokey", t.name)))
        .await
        .unwrap();
    assert_eq!(meta.primary_key, ["rowid"]);
    let rowid = meta.columns.iter().find(|c| c.name == "rowid").unwrap();
    assert!(rowid.is_generated && rowid.is_auto_increment);
    let b = meta.columns.iter().find(|c| c.name == "b").unwrap();
    assert_eq!(b.numeric_scale, Some(2), "{}", b.data_type);
    run(session.as_ref(), &format!("DROP TABLE {q}")).await;

    cancels_and_times_out(session.clone()).await;
    session.close().await;
}

#[tokio::test]
#[ignore = "set TABLER_TEST_GREENPLUM_URL"]
async fn greenplum_server() {
    let Some(session) = open("TABLER_TEST_GREENPLUM_URL", PgFlavor::Greenplum).await else {
        return;
    };
    assert_eq!(session.dialect(), SqlDialect::Postgres);
    let t = scratch();
    let ddl = exercise(
        session.as_ref(),
        Flavor::Greenplum,
        &format!(
            "CREATE TABLE public.{} (id int PRIMARY KEY, name text) DISTRIBUTED BY (id)",
            t.name
        ),
    )
    .await;
    assert!(ddl.contains("DISTRIBUTED BY (id)"), "{ddl}");
    assert!(!session.capabilities().has(Feature::EditWithoutKey));
    cancels_and_times_out(session.clone()).await;
    session.close().await;
}

#[tokio::test]
#[ignore = "set TABLER_TEST_REDSHIFT_URL"]
async fn redshift_server() {
    let Some(session) = open("TABLER_TEST_REDSHIFT_URL", PgFlavor::Redshift).await else {
        return;
    };
    assert_eq!(session.dialect(), SqlDialect::Redshift);
    let caps = session.capabilities();
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    assert!(!caps.has(Feature::Returning));
    let t = scratch();
    let ddl = exercise(
        session.as_ref(),
        Flavor::Redshift,
        &format!(
            "CREATE TABLE public.{} (id int PRIMARY KEY, name varchar(20), doc super) \
             DISTKEY (id)",
            t.name
        ),
    )
    .await;
    assert!(ddl.contains("DISTKEY"), "{ddl}");

    // An edit in Redshift SQL, checked to affect one row, and the SUPER column read as JSON.
    let q = format!("public.{}", t.name);
    run(
        session.as_ref(),
        &format!("CREATE TABLE {q} (id int PRIMARY KEY, name varchar(20), doc super) DISTKEY (id)"),
    )
    .await;
    run(
        session.as_ref(),
        &format!("INSERT INTO {q} VALUES (1, 'one', JSON_PARSE('{{\"a\": 1}}'))"),
    )
    .await;
    let meta = session.table_meta(&t).await.unwrap();
    let doc = meta.columns.iter().find(|c| c.name == "doc").unwrap();
    assert!(tabler_lib::sqlgen::portable::select::is_lossy(&SPEC, doc));
    let target = TableTarget {
        table: &meta.table,
        columns: &meta.columns,
    };
    let change = StagedChange::Update {
        row_id: "r".into(),
        key: RowKey::Pk([("id".to_string(), Value::Int(1))].into()),
        changes: [("name".to_string(), Value::Text("uno".into()))].into(),
        exprs: None,
    };
    let (built, expect) = build_change(&SPEC, &target, &change, false).unwrap();
    session
        .transaction(vec![TxStatement {
            sql: built.sql,
            params: built.params,
            expect_affected: expect,
        }])
        .await
        .unwrap();
    let name = run(
        session.as_ref(),
        &format!("SELECT name FROM {q} WHERE id = 1"),
    )
    .await;
    assert_eq!(name[0][0], Value::Text("uno".into()));
    run(session.as_ref(), &format!("DROP TABLE {q}")).await;
    session.close().await;
}
