//! PostgreSQL `RAISE NOTICE` / `WARNING` reach the SQL editor as `QueryEvent::Notice`, while the
//! statement runs. Runs only when `TABLER_TEST_PG_URL` is set, e.g.
//! `TABLER_TEST_PG_URL=postgres://apriansyahrs@127.0.0.1:5432/tabler_dev cargo test --test editor2_notice`.

// `AppError` carries the full server diagnostic (same allowance as the library).
#![allow(clippy::result_large_err)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use tabler_lib::commands::query::run_sql_inner;
use tabler_lib::drivers::{CancelSlot, CollectSink, ConnectTarget, DbSession, connect};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::query::{QueryEvent, RunOptions, StatementResult};

/// Parses `postgres://user[:pass]@host[:port]/db` into a profile plus the password.
fn profile_from_url(url: &str) -> (ConnectionProfile, Option<String>) {
    let rest = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))
        .expect("TABLER_TEST_PG_URL must start with postgres://");
    let (auth, hostdb) = match rest.rsplit_once('@') {
        Some((a, h)) => (Some(a), h),
        None => (None, rest),
    };
    let (user, password) = match auth {
        Some(a) => match a.split_once(':') {
            Some((u, p)) => (Some(u.to_string()), Some(p.to_string())),
            None => (Some(a.to_string()), None),
        },
        None => (None, None),
    };
    let (hostport, db) = match hostdb.split_once('/') {
        Some((h, d)) => (h, Some(d.split('?').next().unwrap_or("").to_string())),
        None => (hostdb, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (hostport.to_string(), None),
    };
    let profile = ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "editor2-notice".into(),
        color: None,
        group: None,
        driver: Driver::Postgres,
        host: Some(host),
        port,
        user,
        database: db.filter(|d| !d.is_empty()),
        ssl: SslMode::Prefer,
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions::default(),
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    };
    (profile, password)
}

async fn open() -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return None;
    };
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let (profile, password) = profile_from_url(&url);
    let session = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .expect("connect");
    Some(session)
}

/// The run's events, each with when it arrived.
async fn run(
    session: &dyn DbSession,
    sql: &str,
) -> (Vec<StatementResult>, Vec<(Instant, QueryEvent)>) {
    let slot = CancelSlot::default();
    let mut events = Vec::new();
    let results = {
        let mut emit = |e: QueryEvent| events.push((Instant::now(), e));
        run_sql_inner(session, sql, &RunOptions::default(), &slot, &mut emit).await
    };
    (results, events)
}

/// `(index, severity, message, hint)` of every notice event.
fn notices(
    events: &[(Instant, QueryEvent)],
) -> Vec<(Option<usize>, String, String, Option<String>)> {
    events
        .iter()
        .filter_map(|(_, e)| match e {
            QueryEvent::Notice {
                index,
                severity,
                message,
                hint,
                ..
            } => Some((*index, severity.clone(), message.clone(), hint.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn editor2_notice() {
    let Some(session) = open().await else {
        return;
    };
    let (results, events) = run(
        session.as_ref(),
        "select 1;\n\
         DO $$ BEGIN RAISE NOTICE 'editor2 hi %', 7; \
         RAISE WARNING 'editor2 careful' USING HINT = 'editor2 hint'; END $$;",
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(
        notices(&events),
        [
            (Some(1), "NOTICE".into(), "editor2 hi 7".into(), None),
            (
                Some(1),
                "WARNING".into(),
                "editor2 careful".into(),
                Some("editor2 hint".into())
            ),
        ]
    );
    // Sent as events only: the statement summary does not repeat them.
    assert!(results[1].notices.is_empty(), "{:?}", results[1].notices);
    let context = events.iter().find_map(|(_, e)| match e {
        QueryEvent::Notice { context, .. } => context.clone(),
        _ => None,
    });
    assert!(context.is_some_and(|c| c.contains("PL/pgSQL")));

    // Live: a notice raised before a slow part of the statement arrives before it finishes.
    let (_, events) = run(
        session.as_ref(),
        "DO $$ BEGIN RAISE NOTICE 'editor2 early'; PERFORM pg_sleep(1); END $$",
    )
    .await;
    let at = |want: fn(&QueryEvent) -> bool| {
        events
            .iter()
            .find(|(_, e)| want(e))
            .map(|(t, _)| *t)
            .expect("event")
    };
    let noticed = at(|e| matches!(e, QueryEvent::Notice { .. }));
    let finished = at(|e| matches!(e, QueryEvent::Finished { .. }));
    assert!(
        finished.duration_since(noticed) >= Duration::from_millis(700),
        "the notice came {:?} before the end",
        finished.duration_since(noticed)
    );

    // Notices raised before a failure still reach the failed statement.
    let (results, events) = run(
        session.as_ref(),
        "DO $$ BEGIN RAISE NOTICE 'editor2 before'; RAISE EXCEPTION 'editor2 boom'; END $$",
    )
    .await;
    assert!(results[0].error.is_some());
    assert_eq!(notices(&events)[0].2, "editor2 before");

    // Sinks that do not take notices still get them as text (exports, internal statements).
    let outcome = session
        .run_statement(
            "do $$ begin raise notice 'editor2 text'; end $$",
            100,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(outcome.notices, ["NOTICE: editor2 text"]);
    session.close().await;
}
