//! The SQL trace (what the console log shows of the statements the app ran) must never hold a
//! password: Connection ▸ User Management runs `CREATE USER … IDENTIFIED BY '<password>'` /
//! `ALTER ROLE … PASSWORD '<password>'` on the session, and the trace records the statements the
//! session runs. Against real servers, gated on `TABLER_TEST_PG_URL` / `TABLER_TEST_MYSQL_URL`
//! (each test prints a skip notice and passes when unset). Every account is named
//! `fx3_trace_<random>` and dropped at the end.

use std::sync::Arc;

use tabler_lib::admin;
use tabler_lib::drivers::{ConnectTarget, DbSession, connect};
use tabler_lib::types::admin::{RoleAttributes, UserChange, UserState};
use tabler_lib::types::browse::{TraceCategory, TracedSql};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};

/// `scheme://user[:pass]@host[:port]/db` → profile + password.
fn profile_from_url(url: &str, driver: Driver) -> (ConnectionProfile, Option<String>) {
    let rest = url.split_once("://").map(|(_, r)| r).expect("a URL");
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
        name: "trace-secrets-test".into(),
        color: None,
        group: None,
        driver,
        host: Some(host),
        port,
        user,
        database: db.filter(|d| !d.is_empty()),
        ssl: if driver == Driver::Mysql {
            SslMode::Disable
        } else {
            SslMode::Prefer
        },
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

async fn open(var: &str, driver: Driver) -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var(var) else {
        println!("skipped: {var} unset");
        return None;
    };
    let (profile, password) = profile_from_url(&url, driver);
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    Some(
        connect(ConnectTarget {
            profile: &profile,
            password: password.as_deref(),
            host_override: None,
        })
        .await
        .expect("connect"),
    )
}

fn suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// A password with everything that needs escaping, and one that is easy to search for.
const PASSWORD: &str = r"fx3-s3cr3t it's \ tail";
const PASSWORD_PARTS: [&str; 3] = ["fx3-s3cr3t", "it's", "it''s"];

fn assert_no_password(trace: &[TracedSql]) {
    for entry in trace {
        let shown = format!("{} {}", entry.sql, entry.error.as_deref().unwrap_or(""));
        for part in PASSWORD_PARTS {
            assert!(!shown.contains(part), "`{part}` in the trace: {entry:#?}");
        }
    }
}

/// Applies `change` and checks what the session's trace recorded of it: the account statements,
/// masked as the result reports them, once each, and no trace of the password anywhere.
async fn apply_and_check(session: &dyn DbSession, change: &UserChange) {
    session.drain_trace();
    let result = admin::apply_user(session, change).await.unwrap();
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(!result.executed.is_empty());
    let trace = session.drain_trace();
    assert_no_password(&trace);
    for shown in &result.executed {
        let entries: Vec<_> = trace
            .iter()
            .filter(|e| e.sql.trim() == shown.trim())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "`{shown}` should be traced once, as the result shows it; got {trace:#?}"
        );
        assert_eq!(entries[0].category, TraceCategory::Structure);
        assert!(entries[0].error.is_none());
    }
    assert!(
        trace.iter().any(|e| e.sql.contains("'********'")),
        "the password literal is masked: {trace:#?}"
    );
}

#[tokio::test]
async fn postgres_account_statements_leave_no_password_in_the_trace() {
    let Some(session) = open("TABLER_TEST_PG_URL", Driver::Postgres).await else {
        return;
    };
    let s = session.as_ref();
    let role = format!("fx3_trace_{}", suffix());
    let state = UserState {
        name: role.clone(),
        attributes: Some(RoleAttributes {
            login: true,
            ..RoleAttributes::default()
        }),
        ..UserState::default()
    };

    // Create with a password, then change it.
    apply_and_check(
        s,
        &UserChange {
            original: None,
            edited: Some(state.clone()),
            password: Some(PASSWORD.into()),
        },
    )
    .await;
    apply_and_check(
        s,
        &UserChange {
            original: Some(state.clone()),
            edited: Some(state.clone()),
            password: Some(PASSWORD.into()),
        },
    )
    .await;

    // A statement that fails: its error is recorded without the password too.
    s.drain_trace();
    let again = admin::apply_user(
        s,
        &UserChange {
            original: None,
            edited: Some(state.clone()),
            password: Some(PASSWORD.into()),
        },
    )
    .await
    .unwrap();
    assert!(again.error.is_some(), "the role exists already");
    let trace = s.drain_trace();
    assert_no_password(&trace);
    let failed = trace
        .iter()
        .find(|e| e.error.is_some())
        .unwrap_or_else(|| panic!("the failure is traced: {trace:#?}"));
    assert!(failed.sql.contains("PASSWORD '********'"), "{}", failed.sql);
    assert!(failed.error.as_deref().unwrap().contains("already exists"));

    // The password really was set: the masking is of the trace only (superusers can look).
    let hashed = s
        .query(
            "SELECT (rolpassword IS NOT NULL)::text FROM pg_authid WHERE rolname = $1",
            &[tabler_lib::types::value::Value::Text(role.clone())],
        )
        .await;
    if let Ok(hashed) = hashed {
        assert_eq!(hashed.rows[0][0].as_text().as_deref(), Some("true"));
    }

    // Drop it.
    let dropped = admin::apply_user(
        s,
        &UserChange {
            original: Some(state),
            edited: None,
            password: None,
        },
    )
    .await
    .unwrap();
    assert!(dropped.error.is_none(), "{:?}", dropped.error);
    session.close().await;
}

#[tokio::test]
async fn mysql_account_statements_leave_no_password_in_the_trace() {
    let Some(session) = open("TABLER_TEST_MYSQL_URL", Driver::Mysql).await else {
        return;
    };
    let s = session.as_ref();
    let user = format!("fx3_trace_{}", suffix());
    let state = UserState {
        name: user.clone(),
        host: Some("%".into()),
        ..UserState::default()
    };

    apply_and_check(
        s,
        &UserChange {
            original: None,
            edited: Some(state.clone()),
            password: Some(PASSWORD.into()),
        },
    )
    .await;
    apply_and_check(
        s,
        &UserChange {
            original: Some(state.clone()),
            edited: Some(state.clone()),
            password: Some(PASSWORD.into()),
        },
    )
    .await;

    // A failing statement (the account exists): the error is recorded without the password.
    s.drain_trace();
    let again = admin::apply_user(
        s,
        &UserChange {
            original: None,
            edited: Some(state.clone()),
            password: Some(PASSWORD.into()),
        },
    )
    .await
    .unwrap();
    assert!(again.error.is_some(), "the account exists already");
    let trace = s.drain_trace();
    assert_no_password(&trace);
    let failed = trace
        .iter()
        .find(|e| e.error.is_some())
        .unwrap_or_else(|| panic!("the failure is traced: {trace:#?}"));
    assert!(
        failed.sql.contains("IDENTIFIED BY '********'"),
        "{}",
        failed.sql
    );

    let dropped = admin::apply_user(
        s,
        &UserChange {
            original: Some(state),
            edited: None,
            password: None,
        },
    )
    .await
    .unwrap();
    assert!(dropped.error.is_none(), "{:?}", dropped.error);
    session.close().await;
}
