//! Result column origins (editable SQL editor results) and the query time out against the local
//! PostgreSQL / MySQL servers. Each test runs only when its URL is set:
//! `set -a; . ./.env; set +a; cargo test --manifest-path src-tauri/Cargo.toml --test results_origins`.
//! Creates `results_*` tables only, and drops them again.

#![allow(clippy::result_large_err)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use tabler_lib::commands::query::{QUERY_TIMEOUT_CODE, run_sql_inner};
use tabler_lib::drivers::{CancelSlot, ConnectTarget, DbSession, connect};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::query::{QueryEvent, ResultColumn, RunOptions, StatementResult};

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

/// `scheme://user[:pass]@host[:port]/db` as a profile of `driver` plus the password.
fn profile_from_url(url: &str, driver: Driver) -> (ConnectionProfile, Option<String>) {
    let rest = url.split_once("://").map(|(_, r)| r).expect("a URL");
    let (creds, hostdb) = match rest.rsplit_once('@') {
        Some((c, h)) => (Some(c), h),
        None => (None, rest),
    };
    let (user, pass) = match creds {
        Some(c) => match c.split_once(':') {
            Some((u, p)) => (Some(pct_decode(u)), Some(pct_decode(p))),
            None => (Some(pct_decode(c)), None),
        },
        None => (None, None),
    };
    let (hostport, db) = match hostdb.split_once('/') {
        Some((h, d)) => (
            h,
            Some(d.split('?').next().unwrap_or("").to_string()).filter(|d| !d.is_empty()),
        ),
        None => (hostdb, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (hostport.to_string(), None),
    };
    let profile = ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "results-test".into(),
        color: None,
        group: None,
        driver,
        host: Some(host),
        port,
        user,
        database: db,
        ssl: if driver == Driver::Postgres {
            SslMode::Prefer
        } else {
            SslMode::Disable
        },
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions {
            connect_timeout_secs: Some(10),
            ..ProfileOptions::default()
        },
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    };
    (profile, pass)
}

async fn open(env: &str, driver: Driver) -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var(env) else {
        println!("skipped: {env} unset");
        return None;
    };
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let (profile, password) = profile_from_url(&url, driver);
    let session = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap_or_else(|e| panic!("connect failed: {e}"));
    Some(session)
}

fn suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

async fn run(
    session: &dyn DbSession,
    sql: &str,
    options: RunOptions,
) -> (Vec<StatementResult>, Vec<QueryEvent>) {
    let slot = CancelSlot::default();
    let mut events = Vec::new();
    let results = {
        let mut emit = |e: QueryEvent| events.push(e);
        run_sql_inner(session, sql, &options, &slot, &mut emit).await
    };
    (results, events)
}

async fn ok(session: &dyn DbSession, sql: &str) {
    let (results, _) = run(session, sql, RunOptions::default()).await;
    assert!(
        results.iter().all(|r| r.error.is_none()),
        "{sql}: {results:?}"
    );
}

/// (table, column) each result column of statement `index` reads.
fn origins(events: &[QueryEvent], index: usize) -> Vec<Option<(String, String)>> {
    let columns: &Vec<ResultColumn> = events
        .iter()
        .find_map(|e| match e {
            QueryEvent::Columns {
                index: i, columns, ..
            } if *i == index => Some(columns),
            _ => None,
        })
        .expect("a result set");
    columns
        .iter()
        .map(|c| {
            c.origin
                .as_ref()
                .map(|o| (o.table.name.clone(), o.column.clone()))
        })
        .collect()
}

#[tokio::test]
async fn results_postgres_origins_and_time_out() {
    let Some(session) = open("TABLER_TEST_PG_URL", Driver::Postgres).await else {
        return;
    };
    let s = suffix();
    let (users, posts) = (format!("results_users_{s}"), format!("results_posts_{s}"));
    ok(
        session.as_ref(),
        &format!(
            "create table {users} (id int primary key, email text, gone int); \
             alter table {users} drop column gone; alter table {users} add column name text; \
             create table {posts} (id int primary key, user_id int, title text); \
             insert into {users} values (1, 'a@x', 'Ana'); insert into {posts} values (7, 1, 'Hi');"
        ),
    )
    .await;

    let sql = format!(
        "select p.title as t, u.id, u.name, u.email || '!' as shout, p.ctid from {posts} p \
         join {users} u on u.id = p.user_id; select u.id from {users} u"
    );
    let (results, events) = run(session.as_ref(), &sql, RunOptions::default()).await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    let pair = |t: &str, c: &str| Some((t.to_string(), c.to_string()));
    // Aliases resolve to the base columns; a column added after a dropped one keeps its
    // attribute number; expressions and system columns have no origin.
    assert_eq!(
        origins(&events, 0),
        [
            pair(&posts, "title"),
            pair(&users, "id"),
            pair(&users, "name"),
            None,
            None
        ]
    );
    let first = &results[0].columns[1];
    assert_eq!(
        first.origin.as_ref().unwrap().table.schema.as_deref(),
        Some("public")
    );
    assert_eq!(first.table.as_deref(), Some(users.as_str()));
    // The second statement hits the run's cache.
    assert_eq!(origins(&events, 1), [pair(&users, "id")]);

    // Inside a transaction block the lookup leaves the transaction intact.
    let (results, events) = run(
        session.as_ref(),
        &format!("begin; select id, email from {users}; select 1; rollback;"),
        RunOptions::default(),
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(
        origins(&events, 1),
        [pair(&users, "id"), pair(&users, "email")]
    );

    // The query time out cancels the statement and stops the run; the editor stays usable.
    let started = Instant::now();
    let opts = RunOptions {
        timeout_secs: 1,
        ..RunOptions::default()
    };
    let (results, _) = run(session.as_ref(), "select pg_sleep(30); select 1", opts).await;
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(results.len(), 1, "{results:?}");
    let err = results[0].error.as_ref().expect("timed out");
    assert_eq!(err.code, QUERY_TIMEOUT_CODE);
    assert_eq!(err.message, "Query timed out after 1 s");
    ok(session.as_ref(), "select 1").await;

    ok(
        session.as_ref(),
        &format!("drop table {posts}; drop table {users};"),
    )
    .await;
    session.close().await;
}

#[tokio::test]
async fn results_mysql_origins() {
    let Some(session) = open("TABLER_TEST_MYSQL_URL", Driver::Mysql).await else {
        return;
    };
    let s = suffix();
    let users = format!("results_users_{s}");
    ok(
        session.as_ref(),
        &format!(
            "create table {users} (id int primary key, email varchar(50)); \
             insert into {users} values (1, 'a@x');"
        ),
    )
    .await;
    let sql = format!(
        "select u.id as uid, u.email, concat(u.email, '!') as shout from {users} u; \
         select d.id from (select id from {users} group by id) d"
    );
    let (results, events) = run(session.as_ref(), &sql, RunOptions::default()).await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    let pair = |c: &str| Some((users.clone(), c.to_string()));
    assert_eq!(origins(&events, 0), [pair("id"), pair("email"), None]);
    // MySQL reports the alias as the column's table.
    assert_eq!(results[0].columns[0].table.as_deref(), Some("u"));
    assert!(
        results[0].columns[0]
            .origin
            .as_ref()
            .unwrap()
            .table
            .schema
            .is_some()
    );
    // A materialized derived table names no base table the rows could be written back to.
    let derived = origins(&events, 1);
    assert!(
        derived[0].is_none() || derived[0] == pair("id"),
        "unexpected origin {derived:?}"
    );
    ok(session.as_ref(), &format!("drop table {users}")).await;
    session.close().await;
}
