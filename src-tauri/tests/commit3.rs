//! The workspace commit (data edits with server expressions, structure, sidebar operations and
//! routine definitions in one `run_workspace`) against real PostgreSQL and MySQL. Gated on
//! `TABLER_TEST_PG_URL` / `TABLER_TEST_MYSQL_URL` (each test prints a skip notice and passes when
//! its variable is unset). Every object is named `commit3_*` and dropped at the end.

// `AppError` carries the full server diagnostic (same allowance as the library).
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::FutureExt as _;

use tabler_lib::commands::mutate::PlannedRows;
use tabler_lib::commands::workspace_commit::{
    PlannedGroup, is_atomic, plan_definition, run_workspace,
};
use tabler_lib::drivers::{ConnectTarget, DbSession, TxStatement, connect};
use tabler_lib::objects::routine_definition;
use tabler_lib::sqlgen::Dialect;
use tabler_lib::sqlgen::mutate::{TableSpec, build_change, ordered};
use tabler_lib::sqlgen::select::inline_params;
use tabler_lib::types::browse::RowKey;
use tabler_lib::types::ddl::DdlPlan;
use tabler_lib::types::mutate::{ApplyRequest, Clock, GroupStatus, ServerExpr, StagedChange};
use tabler_lib::types::objects::{RoutineKind, RoutineRef};
use tabler_lib::types::profile::{ConnectionProfile, Driver};
use tabler_lib::types::schema::TableRef;
use tabler_lib::types::value::Value;

fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A session for `postgres://…` / `mysql://user[:pass]@host[:port]/db`.
async fn session(url: &str, driver: &str) -> Arc<dyn DbSession> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let (auth, hostdb) = rest
        .rsplit_once('@')
        .map_or((None, rest), |(a, h)| (Some(a), h));
    let (user, password) = match auth.map(|a| a.split_once(':').unwrap_or((a, ""))) {
        Some((u, p)) => (
            Some(pct_decode(u)),
            Some(pct_decode(p)).filter(|p| !p.is_empty()),
        ),
        None => (None, None),
    };
    let (hostport, db) = hostdb.split_once('/').unwrap_or((hostdb, ""));
    let (host, port) = hostport
        .rsplit_once(':')
        .map_or((hostport, None), |(h, p)| (h, p.parse::<u16>().ok()));
    let profile: ConnectionProfile = serde_json::from_value(serde_json::json!({
        "id": uuid::Uuid::new_v4(),
        "name": "commit3",
        "driver": driver,
        "host": host,
        "port": port,
        "user": user,
        "database": db.split('?').next().filter(|d| !d.is_empty()),
        "ssl": "disable",
    }))
    .expect("profile");
    connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .expect("connect")
}

/// Runs `body`, then `cleanup` (also when `body` panics), then re-raises a panic.
async fn with_cleanup(body: impl Future<Output = ()>, cleanup: impl Future<Output = ()>) {
    let outcome = AssertUnwindSafe(body).catch_unwind().await;
    cleanup.await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

fn suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..10].to_string()
}

fn int(n: i64) -> Value {
    Value::Int(n)
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn pk(id: i64) -> RowKey {
    RowKey::Pk(BTreeMap::from([("id".to_string(), int(id))]))
}

/// The row edits of `table` as a planned group (what `plan_rows` builds from the metadata).
async fn rows(s: &dyn DbSession, table: &TableRef, changes: Vec<StagedChange>) -> PlannedGroup {
    let meta = s.table_meta(table).await.expect("table meta");
    let dialect = s.dialect();
    let spec = TableSpec {
        dialect,
        table: &meta.table,
        columns: &meta.columns,
    };
    let req = ApplyRequest {
        table: table.clone(),
        changes,
        returning: true,
    };
    let statements = ordered(&req.changes)
        .into_iter()
        .map(|change| {
            let index = req
                .changes
                .iter()
                .position(|c| std::ptr::eq(c, change))
                .unwrap();
            let (built, expect) = build_change(&spec, change, true).expect("statement");
            let mut st = TxStatement::new(built.sql, built.params);
            st.expect_affected = expect;
            (index, st)
        })
        .collect();
    PlannedGroup::Rows {
        planned: PlannedRows {
            statements,
            columns: meta.columns.clone(),
            driver: s.driver(),
            dialect,
        },
        req,
    }
}

fn plan(transactional: bool, statements: &[&str], object: bool) -> PlannedGroup {
    PlannedGroup::Plan {
        plan: DdlPlan {
            statements: statements.iter().map(|s| s.to_string()).collect(),
            warnings: Vec::new(),
            transactional,
        },
        object,
        created: None,
        replaces: None,
    }
}

fn group_sql(group: &PlannedGroup) -> Vec<String> {
    match group {
        PlannedGroup::Plan { plan, .. } => plan.statements.clone(),
        PlannedGroup::Rows { .. } => Vec::new(),
    }
}

async fn count(s: &dyn DbSession, sql: &str) -> i64 {
    let rs = s
        .query(sql, &[])
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    match rs.rows.first().and_then(|r| r.first()) {
        Some(Value::Int(n)) => *n,
        Some(Value::Text(t)) => t.parse().unwrap_or(-1),
        other => panic!("{sql}: unexpected {other:?}"),
    }
}

async fn name_of(s: &dyn DbSession, table: &str, id: i64) -> String {
    let rs = s
        .query(&format!("SELECT name FROM {table} WHERE id = {id}"), &[])
        .await
        .unwrap();
    match rs.rows.first().and_then(|r| r.first()) {
        Some(Value::Text(t)) => t.clone(),
        other => panic!("unexpected {other:?}"),
    }
}

fn update(id: i64, name: &str, expr: ServerExpr) -> StagedChange {
    StagedChange::Update {
        row_id: format!("r{id}"),
        key: pk(id),
        changes: BTreeMap::from([("name".to_string(), text(name))]),
        exprs: Some(BTreeMap::from([("seen".to_string(), expr)])),
    }
}

// ---------------------------------------------------------------------------------------------
// PostgreSQL
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn postgres_commits_every_tab_in_one_transaction_and_reverts_them_together() {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return;
    };
    let s = session(&url, "postgres").await;
    let sfx = suffix();
    let a = format!("commit3_a_{sfx}");
    let b = format!("commit3_b_{sfx}");
    let ta = TableRef::new(Some("public"), &a);
    with_cleanup(
        async {
            for sql in [
                format!(
                    "CREATE TABLE public.{a} (id serial PRIMARY KEY, name text, seen timestamptz)"
                ),
                format!("INSERT INTO public.{a} (name) VALUES ('one'), ('two')"),
                format!("CREATE TABLE public.{b} (x int)"),
                format!("INSERT INTO public.{b} VALUES (1), (2)"),
            ] {
                s.execute(&sql, &[]).await.expect(&sql);
            }

            // Data edits (with the server's NOW() and CURRENT_TIMESTAMP), structure, then an
            // operation: one transaction.
            let ok = vec![
                plan(true, &[&format!("TRUNCATE TABLE public.{b}")], true),
                plan(
                    true,
                    &[&format!("ALTER TABLE public.{b} ADD COLUMN y int")],
                    false,
                ),
                rows(
                    s.as_ref(),
                    &ta,
                    vec![
                        update(1, "uno", ServerExpr::Now),
                        StagedChange::Insert {
                            row_id: "n1".into(),
                            values: BTreeMap::from([("name".to_string(), text("three"))]),
                            exprs: Some(BTreeMap::from([(
                                "seen".to_string(),
                                ServerExpr::CurrentTimestamp,
                            )])),
                        },
                    ],
                )
                .await,
            ];
            let result = run_workspace(s.as_ref(), &ok).await;
            assert!(result.success, "{:?}", result.error);
            assert!(result.atomic);
            assert!(
                result
                    .groups
                    .iter()
                    .all(|g| g.status == GroupStatus::Applied)
            );
            assert_eq!(name_of(s.as_ref(), &format!("public.{a}"), 1).await, "uno");
            // NOW() is the server's clock, not a value the renderer made up.
            assert_eq!(
                count(
                    s.as_ref(),
                    &format!(
                        "SELECT count(*) FROM public.{a} WHERE seen IS NOT NULL \
                         AND abs(extract(epoch FROM now() - seen)) < 60"
                    )
                )
                .await,
                2
            );
            assert_eq!(
                count(s.as_ref(), &format!("SELECT count(*) FROM public.{b}")).await,
                0
            );
            assert_eq!(
                count(
                    s.as_ref(),
                    &format!(
                        "SELECT count(*) FROM information_schema.columns \
                         WHERE table_name = '{b}' AND column_name = 'y'"
                    )
                )
                .await,
                1
            );
            // The stored row comes back for the grid (RETURNING).
            let returned = result.groups[2].rows[0]
                .returned
                .as_ref()
                .expect("returned");
            assert!(returned.contains_key("seen"));

            // A failure in the LAST group reverts the data edits and the DDL before it.
            let bad = vec![
                rows(s.as_ref(), &ta, vec![update(2, "dos", ServerExpr::Now)]).await,
                plan(
                    true,
                    &[&format!("ALTER TABLE public.{b} ADD COLUMN z int")],
                    false,
                ),
                plan(
                    true,
                    &[&format!("DROP TABLE public.commit3_missing_{sfx}")],
                    true,
                ),
            ];
            let result = run_workspace(s.as_ref(), &bad).await;
            assert!(!result.success);
            assert!(result.atomic);
            assert_eq!(result.failed_group, Some(2));
            assert_eq!(result.groups[0].status, GroupStatus::RolledBack);
            assert_eq!(result.groups[1].status, GroupStatus::RolledBack);
            assert_eq!(result.groups[2].status, GroupStatus::Failed);
            assert!(result.groups.iter().all(|g| g.executed.is_empty()));
            assert_eq!(name_of(s.as_ref(), &format!("public.{a}"), 2).await, "two");
            assert_eq!(
                count(
                    s.as_ref(),
                    &format!(
                        "SELECT count(*) FROM information_schema.columns \
                         WHERE table_name = '{b}' AND column_name = 'z'"
                    )
                )
                .await,
                0
            );

            // A row guard (the row is gone) in the second tab reverts the first tab too.
            let guarded = vec![
                rows(s.as_ref(), &ta, vec![update(1, "again", ServerExpr::Now)]).await,
                rows(s.as_ref(), &ta, vec![update(99, "ghost", ServerExpr::Now)]).await,
            ];
            let result = run_workspace(s.as_ref(), &guarded).await;
            assert!(!result.success);
            assert_eq!(result.failed_group, Some(1));
            assert_eq!(result.groups[1].rows[0].row_id, "r99");
            assert!(result.groups[1].rows[0].error.is_some());
            assert_eq!(name_of(s.as_ref(), &format!("public.{a}"), 1).await, "uno");
        },
        async {
            for t in [&a, &b] {
                s.execute(&format!("DROP TABLE IF EXISTS public.{t} CASCADE"), &[])
                    .await
                    .ok();
            }
        },
    )
    .await;
}

#[tokio::test]
async fn postgres_definitions_replace_in_place_or_drop_and_create() {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return;
    };
    let s = session(&url, "postgres").await;
    let sfx = suffix();
    let f = format!("commit3_f_{sfx}");
    let g = format!("commit3_g_{sfx}");
    let v = format!("commit3_v_{sfx}");
    with_cleanup(
        async {
            let run = |sql: String, drop: Option<RoutineRef>| {
                let s = s.clone();
                async move {
                    let group =
                        plan_definition(Dialect::Postgres, drop.as_ref(), &sql).expect("plan");
                    let statements = group_sql(&group);
                    let result = run_workspace(s.as_ref(), &[group]).await;
                    (result, statements)
                }
            };
            let routine = |name: &str, args: &str| RoutineRef {
                schema: Some("public".into()),
                name: name.into(),
                kind: RoutineKind::Function,
                identity_arguments: args.into(),
            };

            // A new function, then a view that depends on it.
            let (r, _) = run(
                format!("CREATE FUNCTION public.{f}(a integer) RETURNS integer AS 'select 1' LANGUAGE sql"),
                None,
            )
            .await;
            assert!(r.success, "{:?}", r.error);
            // The tab keeps saving the routine its text names.
            assert_eq!(
                r.groups[0].routine,
                Some(routine(&f, "a integer"))
            );
            s.execute(
                &format!("CREATE VIEW public.{v} AS SELECT public.{f}(1) AS n"),
                &[],
            )
            .await
            .unwrap();

            // CREATE OR REPLACE with the same signature: no DROP (the view depends on it).
            let (r, statements) = run(
                format!("CREATE OR REPLACE FUNCTION public.{f}(a integer DEFAULT 5) RETURNS integer AS 'select 2' LANGUAGE sql;"),
                Some(routine(&f, "a integer")),
            )
            .await;
            assert_eq!(statements.len(), 1, "{statements:?}");
            assert!(r.success, "{:?}", r.error);
            assert_eq!(
                count(s.as_ref(), &format!("SELECT n FROM public.{v}")).await,
                2
            );

            // A changed signature drops the old routine first, then creates the new one.
            s.execute(
                &format!("CREATE FUNCTION public.{g}(a integer) RETURNS integer AS 'select 1' LANGUAGE sql"),
                &[],
            )
            .await
            .unwrap();
            let (r, statements) = run(
                format!("CREATE OR REPLACE FUNCTION public.{g}(a integer, b integer) RETURNS integer AS 'select a + b' LANGUAGE sql"),
                Some(routine(&g, "a integer")),
            )
            .await;
            assert_eq!(statements.len(), 2, "{statements:?}");
            assert!(r.success, "{:?}", r.error);
            assert_eq!(
                count(s.as_ref(), &format!("SELECT public.{g}(2, 3)")).await,
                5
            );
            // The one-argument version is gone.
            assert!(s
                .query(&format!("SELECT public.{g}(1)"), &[])
                .await
                .is_err());

            // A definition with an error changes nothing (one transaction with its DROP).
            let (r, _) = run(
                format!("CREATE OR REPLACE FUNCTION public.{g}(a integer) RETURNS integer AS 'select nosuchcol' LANGUAGE sql"),
                Some(routine(&g, "a integer, b integer")),
            )
            .await;
            assert!(!r.success);
            assert_eq!(
                count(s.as_ref(), &format!("SELECT public.{g}(2, 3)")).await,
                5
            );
        },
        async {
            s.execute(&format!("DROP VIEW IF EXISTS public.{v}"), &[])
                .await
                .ok();
            for name in [&f, &g] {
                s.execute(&format!("DROP FUNCTION IF EXISTS public.{name}(integer)"), &[])
                    .await
                    .ok();
                s.execute(
                    &format!("DROP FUNCTION IF EXISTS public.{name}(integer, integer)"),
                    &[],
                )
                .await
                .ok();
            }
        },
    )
    .await;
}

// ---------------------------------------------------------------------------------------------
// MySQL
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn mysql_runs_the_data_edits_first_then_each_ddl_and_reports_what_stayed() {
    let Ok(url) = std::env::var("TABLER_TEST_MYSQL_URL") else {
        println!("skipped: TABLER_TEST_MYSQL_URL unset");
        return;
    };
    let s = session(&url, "mysql").await;
    assert_eq!(s.driver(), Driver::Mysql);
    let sfx = suffix();
    let a = format!("commit3_a_{sfx}");
    let b = format!("commit3_b_{sfx}");
    let p = format!("commit3_p_{sfx}");
    let ta = TableRef::new(None, &a);
    with_cleanup(
        async {
            for sql in [
                format!("CREATE TABLE {a} (id int AUTO_INCREMENT PRIMARY KEY, name varchar(20), seen datetime)"),
                format!("INSERT INTO {a} (name) VALUES ('one'), ('two')"),
                format!("CREATE TABLE {b} (x int)"),
            ] {
                s.execute(&sql, &[]).await.expect(&sql);
            }
            // The edit: `seen` = NOW() on the server, unquoted.
            let edit = rows(s.as_ref(), &ta, vec![update(1, "uno", ServerExpr::Now)]).await;
            if let PlannedGroup::Rows { planned, .. } = &edit {
                let sql = inline_params(Dialect::Mysql, &planned.statements[0].1.sql, &planned.statements[0].1.params);
                assert!(sql.contains("`seen` = NOW()"), "{sql}");
            }
            let groups = vec![
                plan(false, &[&format!("ALTER TABLE {b} ADD COLUMN y int")], false),
                plan(
                    false,
                    &[&format!("ALTER TABLE {b} ADD COLUMN z int"), "ALTER TABLE commit3_nope ADD COLUMN q int"],
                    false,
                ),
                plan(false, &[&format!("DROP TABLE {b}")], true),
                edit,
            ];
            let result = run_workspace(s.as_ref(), &groups).await;
            // Not one transaction: DDL commits on its own.
            assert!(!result.success);
            assert!(!result.atomic);
            // Data edits ran first (in a transaction of their own), then each DDL in order.
            assert_eq!(result.groups[3].status, GroupStatus::Applied);
            assert_eq!(result.groups[0].status, GroupStatus::Applied);
            assert_eq!(result.groups[1].status, GroupStatus::Failed);
            assert_eq!(result.groups[1].failed_index, Some(1));
            assert_eq!(
                result.groups[1].executed,
                [format!("ALTER TABLE {b} ADD COLUMN z int")]
            );
            assert_eq!(result.groups[2].status, GroupStatus::NotRun);
            assert_eq!(result.failed_group, Some(1));
            assert_eq!(name_of(s.as_ref(), &a, 1).await, "uno");
            assert_eq!(
                count(
                    s.as_ref(),
                    &format!("SELECT count(*) FROM {a} WHERE seen IS NOT NULL AND ABS(TIMESTAMPDIFF(SECOND, seen, NOW())) < 60")
                )
                .await,
                1
            );
            // The first ALTERs stayed; the dropped table was never reached.
            assert_eq!(
                count(
                    s.as_ref(),
                    &format!("SELECT count(*) FROM information_schema.columns WHERE table_schema = DATABASE() AND table_name = '{b}' AND column_name IN ('y', 'z')")
                )
                .await,
                2
            );

            // A failing row guard reverts the data edits and runs no DDL.
            let ghost = rows(s.as_ref(), &ta, vec![update(99, "ghost", ServerExpr::Now)]).await;
            let result = run_workspace(
                s.as_ref(),
                &[
                    plan(false, &[&format!("ALTER TABLE {b} ADD COLUMN w int")], false),
                    ghost,
                ],
            )
            .await;
            assert!(!result.success);
            assert_eq!(result.groups[1].status, GroupStatus::Failed);
            assert_eq!(result.groups[0].status, GroupStatus::NotRun);
            // Nothing stayed (the data transaction rolled back, the DDL never ran): "All changes
            // were reverted", although this mixed commit is not one transaction.
            assert!(result.atomic);

            // A procedure definition: drop (if it exists) and create, each on its own.
            let create = |body: &str, drop: Option<RoutineRef>| {
                plan_definition(
                    Dialect::Mysql,
                    drop.as_ref(),
                    &format!("CREATE PROCEDURE {p}()\nBEGIN\n  SELECT {body} AS n;\nEND;"),
                )
                .expect("plan")
            };
            let first = run_workspace(s.as_ref(), &[create("1", None)]).await;
            assert!(first.success, "{:?}", first.error);
            let created = first.groups[0].routine.clone().expect("routine");
            assert_eq!(created.name, p);
            let again = create("2", Some(created));
            if let PlannedGroup::Plan { plan, replaces, .. } = &again {
                assert_eq!(plan.statements.len(), 2);
                assert!(plan.statements[0].starts_with("DROP PROCEDURE IF EXISTS"));
                // MySQL remembers what it drops, to put it back when the CREATE fails.
                assert_eq!(replaces.as_ref().map(|r| r.name.as_str()), Some(p.as_str()));
            }
            let second = run_workspace(s.as_ref(), &[again]).await;
            assert!(second.success, "{:?}", second.error);
            let called = s.query(&format!("CALL {p}()"), &[]).await.unwrap();
            assert!(matches!(called.rows[0][0], Value::Int(2) | Value::BigInt(_)));
        },
        async {
            for t in [&a, &b] {
                s.execute(&format!("DROP TABLE IF EXISTS {t}"), &[]).await.ok();
            }
            s.execute(&format!("DROP PROCEDURE IF EXISTS {p}"), &[])
                .await
                .ok();
        },
    )
    .await;
}

/// A staged delete of row `id` (it must exist: the guard expects one affected row).
fn delete(id: i64) -> StagedChange {
    StagedChange::Delete {
        row_id: format!("r{id}"),
        key: pk(id),
    }
}

#[tokio::test]
async fn mysql_a_rows_only_commit_is_one_transaction_and_a_failure_reverts_all_of_it() {
    let Ok(url) = std::env::var("TABLER_TEST_MYSQL_URL") else {
        println!("skipped: TABLER_TEST_MYSQL_URL unset");
        return;
    };
    let s = session(&url, "mysql").await;
    let sfx = suffix();
    let a = format!("fx3_a_{sfx}");
    let b = format!("fx3_b_{sfx}");
    let (ta, tb) = (TableRef::new(None, &a), TableRef::new(None, &b));
    with_cleanup(
        async {
            for t in [&a, &b] {
                for sql in [
                    format!("CREATE TABLE {t} (id int AUTO_INCREMENT PRIMARY KEY, name varchar(20), seen datetime)"),
                    format!("INSERT INTO {t} (name) VALUES ('one'), ('two')"),
                ] {
                    s.execute(&sql, &[]).await.expect(&sql);
                }
            }
            // Two tabs: the first edit is fine, the second tab deletes a row that is not there.
            let failing = vec![
                rows(s.as_ref(), &ta, vec![update(1, "uno", ServerExpr::Now)]).await,
                rows(
                    s.as_ref(),
                    &tb,
                    vec![update(1, "eins", ServerExpr::Now), delete(99)],
                )
                .await,
            ];
            // MySQL runs row edits in one transaction, so the whole commit is atomic.
            assert!(is_atomic(Dialect::Mysql, &failing));
            let result = run_workspace(s.as_ref(), &failing).await;
            assert!(!result.success);
            assert!(
                result.atomic,
                "a rows-only failure on MySQL reverts everything: \"All changes were reverted\""
            );
            assert_eq!(result.failed_group, Some(1));
            assert_eq!(result.groups[0].status, GroupStatus::RolledBack);
            assert_eq!(result.groups[1].status, GroupStatus::Failed);
            assert!(result.groups[1].failed_index.is_some());
            assert!(result.groups.iter().all(|g| g.executed.is_empty()));
            // The original rows of BOTH tables are untouched (also the edit that was fine, in the
            // tab that failed).
            for t in [&a, &b] {
                assert_eq!(name_of(s.as_ref(), t, 1).await, "one");
                assert_eq!(name_of(s.as_ref(), t, 2).await, "two");
            }

            // The same two tabs without the bad edit commit together, in one transaction.
            let fine = vec![
                rows(s.as_ref(), &ta, vec![update(1, "uno", ServerExpr::Now)]).await,
                rows(s.as_ref(), &tb, vec![update(2, "zwei", ServerExpr::Now)]).await,
            ];
            let result = run_workspace(s.as_ref(), &fine).await;
            assert!(result.success, "{:?}", result.error);
            assert!(result.atomic);
            assert_eq!(name_of(s.as_ref(), &a, 1).await, "uno");
            assert_eq!(name_of(s.as_ref(), &b, 2).await, "zwei");

            // With structure pending the commit is not one transaction (MySQL commits DDL on its
            // own) ...
            let mixed = vec![
                rows(s.as_ref(), &ta, vec![update(2, "dos", ServerExpr::Now)]).await,
                plan(false, &[&format!("ALTER TABLE {b} ADD COLUMN y int")], false),
            ];
            assert!(!is_atomic(Dialect::Mysql, &mixed));
            let result = run_workspace(s.as_ref(), &mixed).await;
            assert!(result.success, "{:?}", result.error);
            assert!(!result.atomic);
            // ... and a DDL that fails after the data was committed says what stayed.
            let late = vec![
                rows(s.as_ref(), &ta, vec![update(1, "ein", ServerExpr::Now)]).await,
                plan(false, &["ALTER TABLE fx3_nope ADD COLUMN q int"], false),
            ];
            let result = run_workspace(s.as_ref(), &late).await;
            assert!(!result.success);
            assert!(!result.atomic, "the data edits stayed committed");
            assert_eq!(result.groups[0].status, GroupStatus::Applied);
            assert_eq!(name_of(s.as_ref(), &a, 1).await, "ein");
        },
        async {
            for t in [&a, &b] {
                s.execute(&format!("DROP TABLE IF EXISTS {t}"), &[]).await.ok();
            }
        },
    )
    .await;
}

// ---------------------------------------------------------------------------------------------
// Definitions: what opening a routine shows must save, and a failing save keeps the old routine
// ---------------------------------------------------------------------------------------------

/// The URL of the server of `url` for another account and database.
fn url_for(url: &str, user: &str, db: &str) -> String {
    let (scheme, rest) = url.split_once("://").expect("url");
    let hostport = rest
        .rsplit_once('@')
        .map_or(rest, |(_, h)| h)
        .split('/')
        .next()
        .unwrap();
    format!("{scheme}://{user}@{hostport}/{db}")
}

async fn current_db(s: &dyn DbSession) -> String {
    s.server_info()
        .await
        .unwrap()
        .current_database
        .expect("a current database")
}

fn routine_ref(schema: &str, name: &str, kind: RoutineKind, args: &str) -> RoutineRef {
    RoutineRef {
        schema: Some(schema.into()),
        name: name.into(),
        kind,
        identity_arguments: args.into(),
    }
}

/// The first cell of `CALL name()` / `SELECT …` as a number.
async fn first_number(s: &dyn DbSession, sql: &str) -> i64 {
    count(s, sql).await
}

async fn routines_named(s: &dyn DbSession, db: &str, name: &str) -> i64 {
    count(
        s,
        &format!(
            "SELECT count(*) FROM information_schema.routines \
             WHERE routine_schema = '{db}' AND routine_name = '{name}'"
        ),
    )
    .await
}

#[tokio::test]
async fn postgres_an_opened_function_saves() {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return;
    };
    let s = session(&url, "postgres").await;
    let f = format!("commit3_o_{}", suffix());
    with_cleanup(
        async {
            s.execute(
                &format!(
                    "CREATE FUNCTION public.{f}(a integer) RETURNS integer \
                     LANGUAGE sql AS 'select a + 1'"
                ),
                &[],
            )
            .await
            .unwrap();
            let r = routine_ref("public", &f, RoutineKind::Function, "a integer");
            // What opening it shows, edited and saved with ⌘S.
            let opened = routine_definition(s.as_ref(), &r).await.unwrap();
            assert!(opened.starts_with("CREATE OR REPLACE FUNCTION"), "{opened}");
            let edited = opened.replace("a + 1", "a + 10");
            assert_ne!(edited, opened);
            let group = plan_definition(Dialect::Postgres, Some(&r), &edited).expect("plans");
            // The same signature: replaced in place, no DROP.
            assert_eq!(group_sql(&group).len(), 1);
            let result = run_workspace(s.as_ref(), &[group]).await;
            assert!(result.success, "{:?}", result.error);
            assert_eq!(
                first_number(s.as_ref(), &format!("SELECT public.{f}(1)")).await,
                11
            );
        },
        async {
            s.execute(&format!("DROP FUNCTION IF EXISTS public.{f}(integer)"), &[])
                .await
                .ok();
        },
    )
    .await;
}

#[tokio::test]
async fn mysql_an_opened_routine_saves_and_a_failing_save_puts_it_back() {
    let Ok(url) = std::env::var("TABLER_TEST_MYSQL_URL") else {
        println!("skipped: TABLER_TEST_MYSQL_URL unset");
        return;
    };
    let s = session(&url, "mysql").await;
    let db = current_db(s.as_ref()).await;
    let p = format!("commit3_o_{}", suffix());
    let r = routine_ref(&db, &p, RoutineKind::Procedure, "");
    let n = |sql: &str| {
        let s = s.clone();
        let sql = sql.to_string();
        async move { first_number(s.as_ref(), &sql).await }
    };
    with_cleanup(
        async {
            s.execute(
                &format!("CREATE PROCEDURE {p}() BEGIN SELECT 1 AS n; END"),
                &[],
            )
            .await
            .unwrap();
            // What opening it shows: DROP … IF EXISTS, then the CREATE.
            let opened = routine_definition(s.as_ref(), &r).await.unwrap();
            assert!(opened.starts_with("DROP PROCEDURE IF EXISTS"), "{opened}");

            // ⌘S on the text exactly as opened (it started with DROP and never saved).
            let edited = opened.replace("SELECT 1 AS n", "SELECT 2 AS n");
            assert_ne!(edited, opened);
            let group = plan_definition(Dialect::Mysql, Some(&r), &edited).expect("plans");
            let sql = group_sql(&group);
            assert_eq!(sql.len(), 2, "{sql:?}");
            assert_eq!(sql[0], format!("DROP PROCEDURE IF EXISTS `{db}`.`{p}`"));
            let result = run_workspace(s.as_ref(), &[group]).await;
            assert!(result.success, "{:?}", result.error);
            assert!(result.groups[0].restore.is_none());
            assert_eq!(n(&format!("CALL {p}()")).await, 2);

            // A save whose CREATE fails: the DROP is committed, then the syntax error. The old
            // routine (the one that returns 2) is created again.
            let bad = opened.replace("SELECT 1 AS n", "SELEC 3 AS n");
            let group = plan_definition(Dialect::Mysql, Some(&r), &bad).expect("plans");
            let result = run_workspace(s.as_ref(), &[group]).await;
            assert!(!result.success);
            let g = &result.groups[0];
            assert_eq!(g.status, GroupStatus::Failed);
            assert_eq!(g.failed_index, Some(1));
            // The reason is the new definition's, not the restore's.
            assert_eq!(result.error.as_ref().unwrap().vendor_code, Some(1064));
            let restore = g.restore.as_ref().expect("what became of the old version");
            assert!(restore.restored, "{restore:?}");
            assert!(!restore.without_definer);
            assert!(restore.error.is_none());
            assert!(restore.previous.contains("SELECT 2 AS n"), "{restore:?}");
            // Named with its schema: creating it again from this text lands where it lived.
            assert!(
                restore.previous.contains(&format!("`{db}`.`{p}`")),
                "{restore:?}"
            );
            // The restored routine did not change: nothing of the failed save persists.
            assert!(g.executed.is_empty(), "{:?}", g.executed);
            assert_eq!(routines_named(s.as_ref(), &db, &p).await, 1);
            assert_eq!(n(&format!("CALL {p}()")).await, 2);

            // The tab's routine is gone already (dropped elsewhere): nothing to put back.
            s.execute(&format!("DROP PROCEDURE {p}"), &[])
                .await
                .unwrap();
            let group = plan_definition(Dialect::Mysql, Some(&r), &bad).expect("plans");
            let result = run_workspace(s.as_ref(), &[group]).await;
            assert!(!result.success);
            assert!(result.groups[0].restore.is_none());
            assert_eq!(routines_named(s.as_ref(), &db, &p).await, 0);
        },
        async {
            s.execute(&format!("DROP PROCEDURE IF EXISTS {p}"), &[])
                .await
                .ok();
        },
    )
    .await;
}

#[tokio::test]
async fn mysql_a_routine_is_replaced_in_its_own_database() {
    let Ok(url) = std::env::var("TABLER_TEST_MYSQL_URL") else {
        println!("skipped: TABLER_TEST_MYSQL_URL unset");
        return;
    };
    let s = session(&url, "mysql").await;
    // The connection has another database selected than the routine's.
    let current = current_db(s.as_ref()).await;
    let sfx = suffix();
    let db = format!("commit3_x_{sfx}");
    let p = format!("commit3_xp_{sfx}");
    let r = routine_ref(&db, &p, RoutineKind::Procedure, "");
    with_cleanup(
        async {
            s.execute(&format!("CREATE DATABASE {db}"), &[])
                .await
                .unwrap();
            s.execute(&format!("CREATE PROCEDURE {db}.{p}() SELECT 1 AS n"), &[])
                .await
                .unwrap();
            let opened = routine_definition(s.as_ref(), &r).await.unwrap();
            let edited = opened.replace("SELECT 1 AS n", "SELECT 2 AS n");
            assert_ne!(edited, opened);
            let group = plan_definition(Dialect::Mysql, Some(&r), &edited).expect("plans");
            let sql = group_sql(&group);
            // The DROP and the CREATE name the same routine (the text of SHOW CREATE has no schema).
            assert_eq!(sql[0], format!("DROP PROCEDURE IF EXISTS `{db}`.`{p}`"));
            assert!(sql[1].contains(&format!("`{db}`.`{p}`")), "{sql:?}");
            let result = run_workspace(s.as_ref(), &[group]).await;
            assert!(result.success, "{:?}", result.error);
            // Replaced where it lived; nothing appeared in the database the session has selected.
            assert_eq!(routines_named(s.as_ref(), &db, &p).await, 1);
            assert_eq!(routines_named(s.as_ref(), &current, &p).await, 0);
            assert_eq!(
                first_number(s.as_ref(), &format!("CALL {db}.{p}()")).await,
                2
            );
            // A failing save puts the old one back into that database too.
            let bad = opened.replace("SELECT 1 AS n", "SELEC 3 AS n");
            let group = plan_definition(Dialect::Mysql, Some(&r), &bad).expect("plans");
            let result = run_workspace(s.as_ref(), &[group]).await;
            assert!(!result.success);
            assert!(
                result.groups[0]
                    .restore
                    .as_ref()
                    .is_some_and(|r| r.restored)
            );
            assert_eq!(routines_named(s.as_ref(), &db, &p).await, 1);
            assert_eq!(routines_named(s.as_ref(), &current, &p).await, 0);
        },
        async {
            s.execute(&format!("DROP DATABASE IF EXISTS {db}"), &[])
                .await
                .ok();
        },
    )
    .await;
}

#[tokio::test]
async fn mysql_a_routine_the_account_cannot_create_again_is_kept_or_handed_back() {
    let Ok(url) = std::env::var("TABLER_TEST_MYSQL_URL") else {
        println!("skipped: TABLER_TEST_MYSQL_URL unset");
        return;
    };
    let root = session(&url, "mysql").await;
    let sfx = suffix();
    let db = format!("commit3_r_{sfx}");
    // owner: the definer of the routine; a: may read it but not assume its definer; b: may
    // neither read nor assume; c: may drop but not create routines.
    let owner = format!("cd_o_{sfx}");
    let (a, b, c) = (
        format!("cd_a_{sfx}"),
        format!("cd_b_{sfx}"),
        format!("cd_c_{sfx}"),
    );
    let r = routine_ref(&db, "p", RoutineKind::Procedure, "");
    // The text of the new version: a definer the accounts above cannot assume.
    let new_text = format!("CREATE DEFINER=`{owner}`@`%` PROCEDURE p() SELECT 2 AS n");
    with_cleanup(
        async {
            for sql in [
                format!("CREATE DATABASE {db}"),
                format!("CREATE USER '{owner}'@'%'"),
                format!("CREATE USER '{a}'@'%'"),
                format!("CREATE USER '{b}'@'%'"),
                format!("CREATE USER '{c}'@'%'"),
                format!("GRANT CREATE ROUTINE, ALTER ROUTINE, EXECUTE ON {db}.* TO '{a}'@'%'"),
                format!("GRANT SHOW_ROUTINE ON *.* TO '{a}'@'%'"),
                format!("GRANT CREATE ROUTINE, ALTER ROUTINE, EXECUTE ON {db}.* TO '{b}'@'%'"),
                format!("GRANT ALTER ROUTINE, EXECUTE ON {db}.* TO '{c}'@'%'"),
                format!("GRANT SHOW_ROUTINE ON *.* TO '{c}'@'%'"),
            ] {
                root.execute(&sql, &[]).await.expect(&sql);
            }
            let make = || {
                let root = root.clone();
                let drop = format!("DROP PROCEDURE IF EXISTS {db}.p");
                let sql = format!("CREATE DEFINER='{owner}'@'%' PROCEDURE {db}.p() SELECT 1 AS n");
                async move {
                    root.execute(&drop, &[]).await.ok();
                    root.execute(&sql, &[]).await.expect("create the routine");
                }
            };
            let definer = || {
                let root = root.clone();
                let sql = format!(
                    "SELECT count(*) FROM information_schema.routines \
                     WHERE routine_schema = '{db}' AND routine_name = 'p' AND definer LIKE 'cd_o_%'"
                );
                async move { first_number(root.as_ref(), &sql).await }
            };

            // a: can read the old version, cannot create the new one (its definer): the old one
            // is created again, owned by a now.
            make().await;
            let sa = session(&url_for(&url, &a, &db), "mysql").await;
            let group = plan_definition(Dialect::Mysql, Some(&r), &new_text).expect("plans");
            let result = run_workspace(sa.as_ref(), &[group]).await;
            assert!(!result.success);
            // ER_SPECIFIC_ACCESS_DENIED_ERROR: the new text's own failure.
            assert_eq!(result.error.as_ref().unwrap().vendor_code, Some(1227));
            let g = &result.groups[0];
            let restore = g.restore.as_ref().expect("restore outcome");
            assert!(restore.restored, "{restore:?}");
            assert!(restore.without_definer, "{restore:?}");
            assert!(g.executed.is_empty());
            assert_eq!(routines_named(root.as_ref(), &db, "p").await, 1);
            assert_eq!(
                first_number(root.as_ref(), &format!("CALL {db}.p()")).await,
                1
            );
            assert_eq!(definer().await, 0, "the definer was replaced");

            // b: cannot even read the old version: nothing is dropped.
            make().await;
            let sb = session(&url_for(&url, &b, &db), "mysql").await;
            let group = plan_definition(Dialect::Mysql, Some(&r), &new_text).expect("plans");
            let result = run_workspace(sb.as_ref(), &[group]).await;
            assert!(!result.success);
            let g = &result.groups[0];
            assert_eq!(g.status, GroupStatus::Failed);
            assert!(g.executed.is_empty() && g.restore.is_none());
            let message = result.error.as_ref().unwrap().message.clone();
            assert!(message.contains("cannot be read"), "{message}");
            assert!(message.contains("Nothing was changed"), "{message}");
            assert_eq!(routines_named(root.as_ref(), &db, "p").await, 1);
            assert_eq!(definer().await, 1, "the routine is untouched");

            // c: can read and drop, cannot create anything: the old version cannot be put back
            // either, and its text comes back in the result.
            make().await;
            let sc = session(&url_for(&url, &c, &db), "mysql").await;
            let group = plan_definition(Dialect::Mysql, Some(&r), &new_text).expect("plans");
            let result = run_workspace(sc.as_ref(), &[group]).await;
            assert!(!result.success);
            let g = &result.groups[0];
            let restore = g.restore.as_ref().expect("restore outcome");
            assert!(!restore.restored, "{restore:?}");
            assert!(restore.error.is_some());
            assert!(restore.previous.contains("SELECT 1 AS n"), "{restore:?}");
            assert!(restore.previous.starts_with("CREATE DEFINER="));
            assert!(
                restore.previous.contains(&format!("`{db}`.`p`")),
                "{restore:?}"
            );
            // The DROP persists and the group says so.
            assert_eq!(g.executed.len(), 1);
            assert!(g.executed[0].starts_with("DROP PROCEDURE IF EXISTS"));
            assert_eq!(routines_named(root.as_ref(), &db, "p").await, 0);
        },
        async {
            root.execute(&format!("DROP DATABASE IF EXISTS {db}"), &[])
                .await
                .ok();
            for user in [&owner, &a, &b, &c] {
                root.execute(&format!("DROP USER IF EXISTS '{user}'@'%'"), &[])
                    .await
                    .ok();
            }
        },
    )
    .await;
}

// ---------------------------------------------------------------------------------------------
// Staged NOW(n) / CURRENT_TIMESTAMP(n): the precision reaches the server
// ---------------------------------------------------------------------------------------------

fn set_expr(id: i64, column: &str, expr: ServerExpr) -> StagedChange {
    StagedChange::Update {
        row_id: format!("r{id}"),
        key: pk(id),
        changes: BTreeMap::new(),
        exprs: Some(BTreeMap::from([(column.to_string(), expr)])),
    }
}

fn fsp(clock: Clock, digits: u8) -> ServerExpr {
    ServerExpr::with_fsp(clock, digits).expect("a precision of 0 to 6")
}

#[tokio::test]
async fn mysql_a_typed_precision_stores_fractional_seconds() {
    let Ok(url) = std::env::var("TABLER_TEST_MYSQL_URL") else {
        println!("skipped: TABLER_TEST_MYSQL_URL unset");
        return;
    };
    let s = session(&url, "mysql").await;
    let t = format!("commit3_f_{}", suffix());
    let ta = TableRef::new(None, &t);
    with_cleanup(
        async {
            for sql in [
                format!(
                    "CREATE TABLE {t} (id int PRIMARY KEY, seen datetime(6), at time(6), plain datetime)"
                ),
                format!("INSERT INTO {t} (id) VALUES (1)"),
            ] {
                s.execute(&sql, &[]).await.expect(&sql);
            }
            // Stores `expr` in `column` and says whether the value has fractional seconds. A
            // clock that lands exactly on a second (1 in a million) is asked again.
            let fraction = |column: &'static str, expr: ServerExpr| {
                let s = s.clone();
                let ta = ta.clone();
                let t = t.clone();
                async move {
                    for _ in 0..3 {
                        let group = rows(s.as_ref(), &ta, vec![set_expr(1, column, expr)]).await;
                        let result = run_workspace(s.as_ref(), &[group]).await;
                        assert!(result.success, "{expr:?}: {:?}", result.error);
                        let micro = count(
                            s.as_ref(),
                            &format!("SELECT MICROSECOND({column}) FROM {t} WHERE id = 1"),
                        )
                        .await;
                        if micro != 0 {
                            return true;
                        }
                    }
                    false
                }
            };
            // The bare keywords have 0 digits whatever the column's precision: this is why the
            // precision must travel (a DATETIME(6) cell typed CURRENT_TIMESTAMP(6) lost it).
            assert!(!fraction("seen", ServerExpr::CurrentTimestamp).await);
            assert!(!fraction("seen", ServerExpr::Now).await);
            assert!(!fraction("seen", ServerExpr::LocalTimestamp).await);
            assert!(fraction("seen", fsp(Clock::CurrentTimestamp, 6)).await);
            assert!(fraction("seen", fsp(Clock::Now, 6)).await);
            assert!(fraction("seen", fsp(Clock::LocalTimestamp, 6)).await);
            assert!(fraction("at", fsp(Clock::CurrentTime, 6)).await);
            assert!(!fraction("at", ServerExpr::CurrentTime).await);
            // A precision the column does not have rounds, it does not fail.
            let group = rows(
                s.as_ref(),
                &ta,
                vec![set_expr(1, "plain", fsp(Clock::Now, 3))],
            )
            .await;
            let result = run_workspace(s.as_ref(), &[group]).await;
            assert!(result.success, "{:?}", result.error);
            // The statement that ran carries it.
            let group = rows(s.as_ref(), &ta, vec![set_expr(1, "seen", fsp(Clock::Now, 6))]).await;
            if let PlannedGroup::Rows { planned, .. } = &group {
                let sql = inline_params(
                    Dialect::Mysql,
                    &planned.statements[0].1.sql,
                    &planned.statements[0].1.params,
                );
                assert!(sql.contains("`seen` = NOW(6)"), "{sql}");
            }
        },
        async {
            s.execute(&format!("DROP TABLE IF EXISTS {t}"), &[]).await.ok();
        },
    )
    .await;
}

#[tokio::test]
async fn postgres_a_typed_precision_rounds_the_clock() {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return;
    };
    let s = session(&url, "postgres").await;
    let t = format!("commit3_f_{}", suffix());
    let ta = TableRef::new(Some("public"), &t);
    with_cleanup(
        async {
            for sql in [
                format!("CREATE TABLE public.{t} (id int PRIMARY KEY, seen timestamptz, at time)"),
                format!("INSERT INTO public.{t} (id) VALUES (1)"),
            ] {
                s.execute(&sql, &[]).await.expect(&sql);
            }
            let whole_seconds = |column: &'static str, expr: ServerExpr| {
                let s = s.clone();
                let ta = ta.clone();
                let t = t.clone();
                async move {
                    let group = rows(s.as_ref(), &ta, vec![set_expr(1, column, expr)]).await;
                    let result = run_workspace(s.as_ref(), &[group]).await;
                    assert!(result.success, "{expr:?}: {:?}", result.error);
                    count(
                        s.as_ref(),
                        &format!(
                            "SELECT (extract(microseconds FROM {column})::bigint % 1000000 = 0)::int \
                             FROM public.{t} WHERE id = 1"
                        ),
                    )
                    .await
                        == 1
                }
            };
            // `NOW(0)` is PostgreSQL's `CURRENT_TIMESTAMP(0)`: no fractional seconds.
            assert!(whole_seconds("seen", fsp(Clock::Now, 0)).await);
            assert!(whole_seconds("seen", fsp(Clock::CurrentTimestamp, 0)).await);
            assert!(whole_seconds("at", fsp(Clock::CurrentTime, 0)).await);
            // Without the precision the clock keeps its microseconds (a clock that lands exactly
            // on a second, 1 in a million, is asked again).
            let mut kept = false;
            for _ in 0..3 {
                if !whole_seconds("seen", ServerExpr::Now).await {
                    kept = true;
                    break;
                }
            }
            assert!(kept);
        },
        async {
            s.execute(&format!("DROP TABLE IF EXISTS public.{t}"), &[])
                .await
                .ok();
        },
    )
    .await;
}
