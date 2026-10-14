//! The MCP server (`tabler --mcp`) against PostgreSQL and MySQL: its tools, the read-only default
//! (refused before the server, and read-only on the server) and writes once allowed. Each test
//! skips itself when `TABLER_TEST_PG_URL` / `TABLER_TEST_MYSQL_URL` is unset. Objects are
//! `mcp_*` tables, dropped at the end.

// `AppError` carries the full server diagnostic (same allowance as the library).
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use serde_json::{Value as Json, json};
use uuid::Uuid;

use tabler_lib::appdb::AppDb;
use tabler_lib::drivers::{ConnectTarget, DbSession, connect};
use tabler_lib::mcp::prefs::PREFS_KEY;
use tabler_lib::mcp::server::Server;
use tabler_lib::mcp::tools::Ctx;
use tabler_lib::secrets::{Backend, SecretKind, Secrets};
use tabler_lib::state::AppState;
use tabler_lib::types::history::HistoryFilter;
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};

/// `scheme://user[:pass]@host[:port]/db` as a profile plus its password.
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
        Some((h, d)) => (h, d.split('?').next().map(str::to_string)),
        None => (hostdb, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (hostport.to_string(), None),
    };
    let profile = ConnectionProfile {
        id: Uuid::new_v4(),
        name: format!("mcp {driver:?}"),
        color: None,
        group: None,
        driver,
        host: Some(host),
        port,
        user,
        database: db.filter(|d| !d.is_empty()),
        ssl: SslMode::Prefer,
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
    (profile, password)
}

struct Env {
    server: Arc<Server>,
    appdb: AppDb,
    admin: Arc<dyn DbSession>,
    table: String,
    name: String,
}

async fn env(var: &str, driver: Driver) -> Option<Env> {
    let Ok(url) = std::env::var(var) else {
        println!("skipped: {var} unset");
        return None;
    };
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let (profile, password) = profile_from_url(&url, driver);
    let admin = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .unwrap_or_else(|e| panic!("connect failed: {e}"));
    let table = format!("mcp_items_{}", &Uuid::new_v4().simple().to_string()[..8]);
    let create = match driver {
        Driver::Postgres => format!(
            "CREATE TABLE {table} (id serial PRIMARY KEY, label text NOT NULL, qty int DEFAULT 0, \
             note jsonb); CREATE INDEX {table}_label ON {table}(label); \
             INSERT INTO {table} (label, qty, note) SELECT 'item ' || g, g, '{{\"g\": 1}}' \
             FROM generate_series(1, 30) g"
        ),
        _ => format!(
            "CREATE TABLE {table} (id int AUTO_INCREMENT PRIMARY KEY, label varchar(50) NOT NULL, \
             qty int DEFAULT 0, note json, KEY {table}_label (label))"
        ),
    };
    for sql in create.split("; ") {
        admin.execute(sql, &[]).await.unwrap();
    }
    if driver == Driver::Mysql {
        for g in 1..=30 {
            admin
                .execute(
                    &format!("INSERT INTO {table} (label, qty, note) VALUES ('item {g}', {g}, '{{\"g\": 1}}')"),
                    &[],
                )
                .await
                .unwrap();
        }
    }
    let appdb = AppDb::open_in_memory().unwrap();
    appdb
        .set_setting(PREFS_KEY, &json!({ "mcpEnabled": true }))
        .unwrap();
    let secrets = Secrets::new(Backend::File, appdb.clone());
    let saved = appdb.upsert_profile(&profile).unwrap();
    if let Some(p) = &password {
        secrets
            .set(saved.id, SecretKind::DbPassword, p)
            .await
            .unwrap();
    }
    let state = Arc::new(AppState::new(appdb.clone(), secrets));
    let server = Arc::new(Server::new(Arc::new(Ctx::new(state))));
    Some(Env {
        server,
        appdb,
        admin,
        table,
        name: saved.name,
    })
}

impl Env {
    async fn call(&self, tool: &str, mut args: Json) -> (Json, bool) {
        args["connection"] = json!(self.name);
        let reply = self
            .server
            .handle(json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": tool, "arguments": args },
            }))
            .await
            .unwrap();
        let text = reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        let is_error = reply["result"]["isError"].as_bool().unwrap();
        (
            serde_json::from_str(&text).unwrap_or(Json::String(text)),
            is_error,
        )
    }

    async fn count(&self) -> i64 {
        let rs = self
            .admin
            .query(&format!("SELECT count(*) AS n FROM {}", self.table), &[])
            .await
            .unwrap();
        match rs.rows[0][0].clone() {
            tabler_lib::types::value::Value::Int(n) => n,
            other => other.as_text().unwrap().parse().unwrap(),
        }
    }

    async fn drop_table(&self) {
        self.server.ctx().pool.close_all().await;
        self.admin
            .execute(&format!("DROP TABLE IF EXISTS {}", self.table), &[])
            .await
            .unwrap();
        self.admin.close().await;
    }
}

async fn exercise(env: &Env, default_schema: &str) {
    let t = env.table.as_str();
    let (body, err) = env.call("list_databases", json!({})).await;
    assert!(!err, "{body}");
    assert!(
        body["databases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["current"] == true),
        "{body}"
    );

    let (body, err) = env.call("list_tables", json!({})).await;
    assert!(!err, "{body}");
    assert_eq!(body["schema"], default_schema);
    assert!(
        body["tables"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["name"] == t),
        "{body}"
    );

    let (body, err) = env.call("describe_table", json!({ "table": t })).await;
    assert!(!err, "{body}");
    assert_eq!(body["primaryKey"], json!(["id"]));
    let cols: Vec<&str> = body["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(cols, ["id", "label", "qty", "note"]);
    assert!(
        body["indexes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["name"] == format!("{t}_label")),
        "{body}"
    );
    assert!(
        body["ddl"].as_str().unwrap().contains("CREATE TABLE"),
        "{body}"
    );

    let (body, err) = env
        .call(
            "run_query",
            json!({ "sql": format!("SELECT id, label, note FROM {t} ORDER BY id"), "limit": 5 }),
        )
        .await;
    assert!(!err, "{body}");
    let r = &body["results"][0];
    assert_eq!(r["rows"].as_array().unwrap().len(), 5);
    assert_eq!(r["rows"][0]["label"], "item 1");
    assert_eq!(
        r["rows"][0]["note"]["g"], 1,
        "JSON values stay JSON: {body}"
    );
    assert_eq!(r["truncated"], true);

    // Read-only by default: refused before reaching the server…
    let (body, err) = env
        .call("run_query", json!({ "sql": format!("DELETE FROM {t}") }))
        .await;
    assert!(
        err && body.as_str().unwrap().starts_with("Refused"),
        "{body}"
    );
    assert_eq!(env.count().await, 30);

    let (body, err) = env
        .call(
            "explain_query",
            json!({ "sql": format!("SELECT * FROM {t} WHERE label = 'item 3'") }),
        )
        .await;
    assert!(!err, "{body}");
    assert!(
        !body["plan"].as_str().unwrap_or_default().is_empty() || body["plan"].is_array(),
        "{body}"
    );

    // …and read-only on the server.
    let profile = env.appdb.list_profiles().unwrap().remove(0);
    let lease = env
        .server
        .ctx()
        .pool
        .get(&profile, None, false)
        .await
        .unwrap();
    let e = lease
        .session
        .execute(&format!("INSERT INTO {t} (label) VALUES ('sneaky')"), &[])
        .await
        .unwrap_err();
    assert!(e.to_string().to_lowercase().contains("read"), "{e}");
    drop(lease);
    assert_eq!(env.count().await, 30);

    // Allowed: writes go through on a Silent connection.
    env.appdb
        .set_setting(
            PREFS_KEY,
            &json!({ "mcpEnabled": true, "mcpAllowWrites": true }),
        )
        .unwrap();
    let (body, err) = env
        .call(
            "run_query",
            json!({ "sql": format!("DELETE FROM {t} WHERE qty > 20") }),
        )
        .await;
    assert!(!err, "{body}");
    assert_eq!(body["results"][0]["affectedRows"], 10);
    assert_eq!(env.count().await, 20);

    let history = env.appdb.list_history(&HistoryFilter::default()).unwrap();
    assert!(history.iter().all(|h| h.source == "mcp"));
    assert!(
        history
            .iter()
            .any(|h| h.sql.starts_with("DELETE") && !h.success)
    );
    assert!(
        history
            .iter()
            .any(|h| h.sql.starts_with("DELETE") && h.success && h.affected == Some(10))
    );
}

#[tokio::test]
async fn postgres_tools_and_safety() {
    let Some(env) = env("TABLER_TEST_PG_URL", Driver::Postgres).await else {
        return;
    };
    exercise(&env, "public").await;
    env.drop_table().await;
}

#[tokio::test]
async fn mysql_tools_and_safety() {
    let Some(env) = env("TABLER_TEST_MYSQL_URL", Driver::Mysql).await else {
        return;
    };
    let db = profile_from_url(
        &std::env::var("TABLER_TEST_MYSQL_URL").unwrap(),
        Driver::Mysql,
    )
    .0
    .database
    .unwrap();
    exercise(&env, &db).await;
    env.drop_table().await;
}

/// MySQL's `USE` would move the pooled session to another database while the answer and the
/// history still named the one it opened on: it is refused, and `database` is the way to ask.
/// The throwaway database is named `fx3_pmu_*` and dropped at the end.
#[tokio::test]
async fn mysql_use_is_refused_and_the_database_argument_is_the_way() {
    let Some(env) = env("TABLER_TEST_MYSQL_URL", Driver::Mysql).await else {
        return;
    };
    let home = profile_from_url(
        &std::env::var("TABLER_TEST_MYSQL_URL").unwrap(),
        Driver::Mysql,
    )
    .0
    .database
    .unwrap();
    let other = format!("fx3_pmu_{}", &Uuid::new_v4().simple().to_string()[..8]);
    env.admin
        .execute(&format!("CREATE DATABASE {other}"), &[])
        .await
        .unwrap();

    // Observe first, assert after the cleanup: a failure must not leave the database behind.
    let refused = env
        .call("run_query", json!({ "sql": format!("USE {other}") }))
        .await;
    let mixed = env
        .call(
            "run_query",
            json!({ "sql": format!("SELECT 1; USE {other}") }),
        )
        .await;
    let conditional = env
        .call(
            "run_query",
            json!({ "sql": format!("/*!50000 USE {other} */") }),
        )
        .await;
    // Comments in front of the USE that the server skips (a conditional comment with no body, a
    // hint), and a comment its version skips: each one really runs the USE when it gets through.
    let mut disguised = Vec::new();
    for sql in [
        format!("/*!50000 */ USE {other}"),
        format!("/*!*/USE {other}"),
        format!("/*! */ USE {other}"),
        format!("SELECT 1; /*!*/ USE {other}"),
        format!("/*!50000 */ /*!50000 USE {other} */"),
        format!("/*!12345*/  USE {other}"),
        format!("/*+ x */ USE {other}"),
        format!("/*M!50000 */ USE {other}"),
        format!("/*!99999 # */ ; USE {other}"),
        format!("/*!99999 ' */ ; /*!50000 USE {other} */"),
    ] {
        let answer = env.call("run_query", json!({ "sql": sql })).await;
        disguised.push((sql, answer));
    }
    // Even where changes are allowed.
    env.appdb
        .set_setting(
            PREFS_KEY,
            &json!({ "mcpEnabled": true, "mcpAllowWrites": true }),
        )
        .unwrap();
    let allowed = env
        .call("run_query", json!({ "sql": format!("USE {other}") }))
        .await;
    // The session did not move: it still answers from the database it opened on, and says so.
    let after = env
        .call("run_query", json!({ "sql": "SELECT DATABASE() AS db" }))
        .await;
    // The way to another database: the `database` argument, answered and logged as that one.
    let with_argument = env
        .call(
            "run_query",
            json!({ "sql": "SELECT DATABASE() AS db", "database": other }),
        )
        .await;
    // A table named like the keyword is no USE.
    let not_use = env
        .call("run_query", json!({ "sql": "SELECT 1 AS use_log" }))
        .await;
    let history = env.appdb.list_history(&HistoryFilter::default()).unwrap();

    env.server.ctx().pool.close_all().await;
    env.admin
        .execute(&format!("DROP DATABASE IF EXISTS {other}"), &[])
        .await
        .unwrap();
    env.drop_table().await;

    for (sql, (body, err)) in &disguised {
        assert!(
            *err && body
                .as_str()
                .unwrap()
                .starts_with("USE is not supported over MCP"),
            "{sql}: {body}"
        );
    }
    for (body, err) in [&refused, &mixed, &conditional, &allowed] {
        assert!(
            *err && body
                .as_str()
                .unwrap()
                .starts_with("USE is not supported over MCP"),
            "{body}"
        );
        assert!(
            body.as_str()
                .unwrap()
                .ends_with("Pass the database argument of run_query instead."),
            "{body}"
        );
    }
    assert!(!after.1, "{}", after.0);
    assert_eq!(after.0["database"], home);
    assert_eq!(after.0["results"][0]["rows"][0]["db"], home);
    assert!(!with_argument.1, "{}", with_argument.0);
    assert_eq!(with_argument.0["database"], other.as_str());
    assert_eq!(
        with_argument.0["results"][0]["rows"][0]["db"],
        other.as_str()
    );
    assert!(!not_use.1, "{}", not_use.0);
    // The refusals are in the history as failures; nothing ran.
    assert!(
        history
            .iter()
            .any(|h| h.sql.starts_with("USE ") && !h.success)
    );
    assert!(
        history
            .iter()
            .any(|h| h.sql.starts_with("SELECT DATABASE()")
                && h.database.as_deref() == Some(other.as_str()))
    );
}

/// The gate refuses a MySQL `USE` it can read, but the server may read the text another way. So
/// `run_query` asks the session where it ended up: a session that is not on the database it opened
/// on is closed, not pooled for the next call, and the answer says where it is now. Dropping the
/// current database moves a session without any `USE` for the gate to see.
#[tokio::test]
async fn mysql_a_session_that_ends_on_another_database_is_not_pooled() {
    let Some(env) = env("TABLER_TEST_MYSQL_URL", Driver::Mysql).await else {
        return;
    };
    let home = profile_from_url(
        &std::env::var("TABLER_TEST_MYSQL_URL").unwrap(),
        Driver::Mysql,
    )
    .0
    .database
    .unwrap();
    let scratch = format!("fx3_pmm_{}", &Uuid::new_v4().simple().to_string()[..8]);
    env.admin
        .execute(&format!("CREATE DATABASE {scratch}"), &[])
        .await
        .unwrap();
    env.appdb
        .set_setting(
            PREFS_KEY,
            &json!({ "mcpEnabled": true, "mcpAllowWrites": true }),
        )
        .unwrap();

    let read = env
        .call("run_query", json!({ "sql": "SELECT DATABASE() AS db" }))
        .await;
    let pooled_after_read = env.server.ctx().pool.open_count();
    let dropped = env
        .call(
            "run_query",
            json!({ "sql": format!("DROP DATABASE {scratch}"), "database": scratch }),
        )
        .await;
    let pooled_after_drop = env.server.ctx().pool.open_count();
    let again = env
        .call("run_query", json!({ "sql": "SELECT DATABASE() AS db" }))
        .await;

    env.server.ctx().pool.close_all().await;
    env.admin
        .execute(&format!("DROP DATABASE IF EXISTS {scratch}"), &[])
        .await
        .unwrap();
    env.drop_table().await;

    // A call that stays on its database leaves the session pooled and says nothing about it.
    assert!(!read.1, "{}", read.0);
    assert_eq!(read.0["database"], home);
    assert!(read.0.get("databaseChanged").is_none(), "{}", read.0);
    assert_eq!(pooled_after_read, 1);
    // One that ends without a database says so and closes its connection.
    assert!(!dropped.1, "{}", dropped.0);
    assert_eq!(dropped.0["database"], Json::Null, "{}", dropped.0);
    assert!(
        dropped.0["databaseChanged"]
            .as_str()
            .is_some_and(|m| m.contains(&scratch) && m.contains("no database")),
        "{}",
        dropped.0
    );
    assert_eq!(pooled_after_drop, 1, "the moved session was kept");
    assert!(!again.1, "{}", again.0);
    assert_eq!(again.0["database"], home);
}
