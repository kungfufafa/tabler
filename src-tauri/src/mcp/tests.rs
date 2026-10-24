//! The MCP server end to end against SQLite files: JSON-RPC framing and dispatch, every tool, the
//! safety gate and the history it writes.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value as Json, json};
use uuid::Uuid;

use super::prefs::PREFS_KEY;
use super::server::{self, LATEST_PROTOCOL, Limits, Server};
use super::tools::{Ctx, DISABLED_MESSAGE, MAX_SQL_BYTES};
use crate::appdb::AppDb;
use crate::secrets::{Backend, SecretKind, Secrets};
use crate::state::AppState;
use crate::types::history::HistoryFilter;
use crate::types::profile::{
    ConnectionProfile, ConnectionTag, Driver, ProfileOptions, SafeModeLevel, SslMode,
};

struct Fixture {
    server: Arc<Server>,
    appdb: AppDb,
    path: String,
    _dir: tempfile::TempDir,
}

fn profile(name: &str, path: &str, options: ProfileOptions) -> ConnectionProfile {
    ConnectionProfile {
        id: Uuid::new_v4(),
        name: name.into(),
        color: None,
        group: None,
        driver: Driver::Sqlite,
        host: None,
        port: None,
        user: None,
        database: None,
        ssl: SslMode::Disable,
        ssl_ca_path: None,
        sqlite_path: Some(path.into()),
        ssh: None,
        options,
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    }
}

const SCHEMA: &str = "
    CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT UNIQUE,
        score REAL DEFAULT 0, data BLOB, meta TEXT);
    CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id)
        ON DELETE CASCADE, total NUMERIC CHECK (total >= 0));
    CREATE INDEX orders_user ON orders(user_id);
    CREATE VIEW big_spenders AS SELECT user_id, sum(total) AS spent FROM orders GROUP BY user_id;
    INSERT INTO users (name, email, score, data, meta) VALUES
        ('Ada', 'ada@example.com', 9.5, x'0001', '{\"a\":1}'),
        ('Linus', NULL, 7, NULL, NULL),
        ('Grace', 'grace@example.com', 8, NULL, NULL);
    INSERT INTO orders (user_id, total) VALUES (1, 10.5), (1, 20), (3, 7);
";

async fn fixture(prefs: Json) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shop.sqlite");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(SCHEMA)
        .unwrap();
    let path = path.to_string_lossy().into_owned();
    let appdb = AppDb::open_in_memory().unwrap();
    appdb.set_setting(PREFS_KEY, &prefs).unwrap();
    let secrets = Secrets::new(Backend::File, appdb.clone());
    let opts = |tag: Option<ConnectionTag>, safe: Option<SafeModeLevel>, ro: bool| ProfileOptions {
        tag,
        safe_mode: safe,
        read_only: ro,
        ..ProfileOptions::default()
    };
    for p in [
        profile(
            "Shop",
            &path,
            opts(Some(ConnectionTag::Development), None, false),
        ),
        profile(
            "Shop prod",
            &path,
            opts(Some(ConnectionTag::Production), None, false),
        ),
        profile(
            "Shop RO",
            &path,
            opts(None, Some(SafeModeLevel::Silent), true),
        ),
        profile(
            "Shop safe",
            &path,
            opts(None, Some(SafeModeLevel::Safe), false),
        ),
        profile("twin", &path, opts(None, None, false)),
        profile("Twin", &path, opts(None, None, false)),
    ] {
        let saved = appdb.upsert_profile(&p).unwrap();
        // A stored password must never come back through MCP (`twin` is never opened: a SQLite
        // profile's password is its encryption key).
        if saved.name == "twin" {
            secrets
                .set(saved.id, SecretKind::DbPassword, "hunter2-secret")
                .await
                .unwrap();
        }
        secrets
            .set(saved.id, SecretKind::SshPassword, "hunter3-secret")
            .await
            .unwrap();
    }
    let state = Arc::new(AppState::new(appdb.clone(), secrets));
    let server = Arc::new(Server::new(Arc::new(Ctx::new(state))));
    Fixture {
        server,
        appdb,
        path,
        _dir: dir,
    }
}

fn enabled() -> Json {
    json!({ "mcpEnabled": true })
}

impl Fixture {
    async fn rpc(&self, msg: Json) -> Option<Json> {
        self.server.handle(msg).await
    }

    async fn call(&self, name: &str, args: Json) -> (Json, bool) {
        let reply = self
            .rpc(json!({
                "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                "params": { "name": name, "arguments": args },
            }))
            .await
            .expect("a response");
        assert_eq!(reply["id"], json!(7));
        let result = &reply["result"];
        assert!(reply.get("error").is_none(), "{reply}");
        let text = result["content"][0]["text"].as_str().unwrap().to_string();
        assert_eq!(result["content"][0]["type"], "text");
        let is_error = result["isError"].as_bool().unwrap();
        let body = serde_json::from_str(&text).unwrap_or(Json::String(text));
        (body, is_error)
    }

    fn set_prefs(&self, prefs: Json) {
        self.appdb.set_setting(PREFS_KEY, &prefs).unwrap();
    }

    fn history(&self) -> Vec<crate::types::history::HistoryEntry> {
        self.appdb.list_history(&HistoryFilter::default()).unwrap()
    }

    fn count_users(&self) -> i64 {
        rusqlite::Connection::open(&self.path)
            .unwrap()
            .query_row("SELECT count(*) FROM users", [], |r| r.get(0))
            .unwrap()
    }
}

// ----- JSON-RPC framing and dispatch ------------------------------------------------------------

#[tokio::test]
async fn initialize_negotiates_the_protocol_version() {
    let f = fixture(enabled()).await;
    let r = f
        .rpc(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" },
            },
        }))
        .await
        .unwrap();
    assert_eq!(r["jsonrpc"], "2.0");
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(r["result"]["serverInfo"]["name"], "tabler");
    assert_eq!(r["result"]["capabilities"]["tools"]["listChanged"], false);
    assert!(
        r["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("list_connections")
    );

    let r = f
        .rpc(json!({ "jsonrpc": "2.0", "id": "a", "method": "initialize",
                     "params": { "protocolVersion": "1999-01-01" } }))
        .await
        .unwrap();
    assert_eq!(r["result"]["protocolVersion"], LATEST_PROTOCOL);
    assert_eq!(r["id"], "a");
    assert_eq!(server::negotiate(None), LATEST_PROTOCOL);

    // Notifications get no answer.
    assert!(!f.server.is_initialized());
    assert!(
        f.rpc(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await
            .is_none()
    );
    assert!(f.server.is_initialized());
    assert!(
        f.rpc(json!({ "jsonrpc": "2.0", "method": "notifications/whatever" }))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn framing_errors_and_unknown_methods() {
    let f = fixture(enabled()).await;
    let parse = f.server.handle_line("{not json").await.unwrap();
    let parse: Json = serde_json::from_str(&parse).unwrap();
    assert_eq!(parse["error"]["code"], server::PARSE_ERROR);
    assert_eq!(parse["id"], Json::Null);
    assert!(
        !f.server
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
            .await
            .unwrap()
            .contains('\n')
    );

    let r = f
        .rpc(json!({ "jsonrpc": "2.0", "id": 2, "method": "resources/list" }))
        .await
        .unwrap();
    assert_eq!(r["error"]["code"], server::METHOD_NOT_FOUND);
    let r = f
        .rpc(json!({ "jsonrpc": "1.0", "id": 3, "method": "ping" }))
        .await
        .unwrap();
    assert_eq!(r["error"]["code"], server::INVALID_REQUEST);
    let r = f.rpc(json!(42)).await.unwrap();
    assert_eq!(r["error"]["code"], server::INVALID_REQUEST);
    let r = f
        .rpc(json!({ "jsonrpc": "2.0", "id": {"x": 1}, "method": "ping" }))
        .await
        .unwrap();
    assert_eq!(r["error"]["code"], server::INVALID_REQUEST);
    let r = f
        .rpc(json!({ "jsonrpc": "2.0", "id": 4, "method": "ping" }))
        .await
        .unwrap();
    assert_eq!(r["result"], json!({}));
    // A response from the client (we send no requests) is ignored.
    assert!(
        f.rpc(json!({ "jsonrpc": "2.0", "id": 5, "result": {} }))
            .await
            .is_none()
    );

    // Batches.
    let r = f
        .rpc(json!([
            { "jsonrpc": "2.0", "id": 1, "method": "ping" },
            { "jsonrpc": "2.0", "method": "notifications/initialized" },
            { "jsonrpc": "2.0", "id": 2, "method": "nope" },
        ]))
        .await
        .unwrap();
    assert_eq!(r.as_array().unwrap().len(), 2);
    assert_eq!(r[1]["error"]["code"], server::METHOD_NOT_FOUND);
    assert_eq!(
        f.rpc(json!([])).await.unwrap()["error"]["code"],
        server::INVALID_REQUEST
    );
}

#[tokio::test]
async fn tools_list_and_call_errors() {
    let f = fixture(enabled()).await;
    let r = f
        .rpc(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .await
        .unwrap();
    let tools = r["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "list_connections",
            "list_databases",
            "list_tables",
            "describe_table",
            "run_query",
            "explain_query"
        ]
    );
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object");
        assert!(t["description"].as_str().unwrap().len() > 20);
    }
    let run = tools.iter().find(|t| t["name"] == "run_query").unwrap();
    assert_eq!(run["inputSchema"]["required"], json!(["connection", "sql"]));
    assert_eq!(run["annotations"]["readOnlyHint"], false);

    let r = f
        .rpc(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                     "params": { "name": "drop_everything", "arguments": {} } }))
        .await
        .unwrap();
    assert_eq!(r["error"]["code"], server::INVALID_PARAMS);
    let r = f
        .rpc(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {} }))
        .await
        .unwrap();
    assert_eq!(r["error"]["code"], server::INVALID_PARAMS);

    // Bad arguments are tool errors the model can correct.
    let (body, err) = f.call("list_tables", json!({})).await;
    assert!(err);
    assert_eq!(body, json!("`connection` is required"));
    let (body, err) = f
        .call("run_query", json!({ "connection": "Shop", "sql": 5 }))
        .await;
    assert!(err);
    assert_eq!(body, json!("`sql` must be a string"));
    let (body, err) = f
        .call("run_query", json!({ "connection": "Shop", "sql": "  ;  " }))
        .await;
    assert!(err, "{body}");
    let (body, err) = f.call("list_tables", json!({ "connection": "Nope" })).await;
    assert!(err);
    assert!(
        body.as_str()
            .unwrap()
            .contains("No saved connection is named")
    );
}

#[tokio::test]
async fn a_disabled_server_answers_every_call_with_an_error() {
    let f = fixture(json!({ "mcpEnabled": false })).await;
    let (body, err) = f.call("list_connections", json!({})).await;
    assert!(err);
    assert_eq!(body, json!(DISABLED_MESSAGE));
    // Turning it on in the app applies to the next call.
    f.set_prefs(enabled());
    let (_, err) = f.call("list_connections", json!({})).await;
    assert!(!err);
}

// ----- tools -------------------------------------------------------------------------------------

#[tokio::test]
async fn list_connections_hides_production_and_never_shows_passwords() {
    let f = fixture(enabled()).await;
    let (body, err) = f.call("list_connections", json!({})).await;
    assert!(!err);
    let names: Vec<&str> = body["connections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Shop", "Shop RO", "Shop safe", "twin", "Twin"]);
    assert_eq!(body["hiddenProductionConnections"], 1);
    let shop = &body["connections"][0];
    assert_eq!(shop["driver"], "sqlite");
    assert_eq!(shop["tag"], "development");
    assert_eq!(shop["safeMode"], "Silent Mode");
    assert_eq!(shop["canChangeData"], false);
    assert_eq!(shop["database"], json!(f.path));
    assert_eq!(body["connections"][2]["safeMode"], "Safe Mode 1");
    assert!(body["note"].as_str().unwrap().contains("only read"));
    assert!(!body.to_string().contains("hunter"));

    f.set_prefs(json!({ "mcpEnabled": true, "mcpExposeProduction": true, "mcpAllowWrites": true }));
    let (body, _) = f.call("list_connections", json!({})).await;
    assert_eq!(body["connections"].as_array().unwrap().len(), 6);
    assert!(body.get("hiddenProductionConnections").is_none());
    let can: Vec<(String, bool)> = body["connections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap().to_string(),
                c["canChangeData"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        can,
        [
            ("Shop".to_string(), true),
            ("Shop prod".to_string(), true),
            ("Shop RO".to_string(), false),
            ("Shop safe".to_string(), false),
            ("twin".to_string(), true),
            ("Twin".to_string(), true),
        ]
    );
}

/// Regression: one saved connection this build cannot read (a newer Tabler saved it) made every
/// tool fail.
#[tokio::test]
async fn a_connection_saved_by_a_newer_build_leaves_the_others_usable() {
    let f = fixture(enabled()).await;
    let (future, _) = crate::appdb::newer::insert_profile(&f.appdb, "Analytics");
    let (body, err) = f.call("list_connections", json!({})).await;
    assert!(!err, "{body}");
    let names: Vec<&str> = body["connections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Shop", "Shop RO", "Shop safe", "twin", "Twin"]);
    let (body, err) = f.call("list_tables", json!({ "connection": "Shop" })).await;
    assert!(!err, "{body}");
    // It is out of reach, by name and by id.
    for wanted in ["Analytics".to_string(), future.to_string()] {
        let (body, err) = f.call("list_tables", json!({ "connection": wanted })).await;
        assert!(err);
        assert!(
            body.as_str().unwrap().contains("No saved connection"),
            "{body}"
        );
    }
}

#[tokio::test]
async fn connections_are_found_by_name_or_id() {
    let f = fixture(enabled()).await;
    let (_, err) = f.call("list_tables", json!({ "connection": "shop" })).await;
    assert!(!err, "case-insensitive name");
    // `twin` and `Twin`: the exact spelling wins.
    let (body, err) = f.call("list_tables", json!({ "connection": "Twin" })).await;
    assert!(!err, "{body}");
    let (body, err) = f.call("list_tables", json!({ "connection": "TWIN" })).await;
    assert!(err);
    assert!(body.as_str().unwrap().contains("pass the id"), "{body}");
    let prod = f
        .appdb
        .list_profiles()
        .unwrap()
        .into_iter()
        .find(|p| p.name == "Shop prod")
        .unwrap();
    let (body, err) = f
        .call("list_tables", json!({ "connection": prod.id.to_string() }))
        .await;
    assert!(err);
    assert!(
        body.as_str().unwrap().contains("tagged Production"),
        "{body}"
    );
    f.set_prefs(json!({ "mcpEnabled": true, "mcpExposeProduction": true }));
    let (_, err) = f
        .call("list_tables", json!({ "connection": prod.id.to_string() }))
        .await;
    assert!(!err);
}

#[tokio::test]
async fn lists_databases_tables_and_describes_a_table() {
    let f = fixture(enabled()).await;
    let (body, err) = f
        .call("list_databases", json!({ "connection": "Shop" }))
        .await;
    assert!(!err, "{body}");
    assert!(!body["databases"].as_array().unwrap().is_empty());

    let (body, err) = f.call("list_tables", json!({ "connection": "Shop" })).await;
    assert!(!err, "{body}");
    assert_eq!(body["schema"], "main");
    let tables: Vec<(&str, &str)> = body["tables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap(), t["kind"].as_str().unwrap()))
        .collect();
    assert!(tables.contains(&("users", "table")), "{tables:?}");
    assert!(tables.contains(&("big_spenders", "view")), "{tables:?}");

    let (body, err) = f
        .call(
            "describe_table",
            json!({ "connection": "Shop", "table": "orders" }),
        )
        .await;
    assert!(!err, "{body}");
    assert_eq!(body["table"], "orders");
    assert_eq!(body["primaryKey"], json!(["id"]));
    let cols: Vec<&str> = body["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(cols, ["id", "user_id", "total"]);
    assert_eq!(body["columns"][1]["nullable"], false);
    assert_eq!(body["foreignKeys"][0]["references"]["table"], "users");
    assert_eq!(body["foreignKeys"][0]["onDelete"], "CASCADE");
    assert!(
        body["indexes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["name"] == "orders_user")
    );
    assert!(
        body["ddl"].as_str().unwrap().contains("CREATE TABLE"),
        "{body}"
    );

    let (body, err) = f
        .call(
            "describe_table",
            json!({ "connection": "Shop", "table": "main.big_spenders" }),
        )
        .await;
    assert!(!err, "{body}");
    assert_eq!(body["kind"], "view");
    assert!(body["ddl"].as_str().unwrap().contains("VIEW"), "{body}");

    let (body, err) = f
        .call(
            "describe_table",
            json!({ "connection": "Shop", "table": "nope" }),
        )
        .await;
    assert!(err, "{body}");
}

#[tokio::test]
async fn run_query_returns_rows_as_json_objects() {
    let f = fixture(enabled()).await;
    let (body, err) = f
        .call(
            "run_query",
            json!({ "connection": "Shop", "sql": "SELECT id, name, email, score, data, meta, 1 AS id FROM users ORDER BY id" }),
        )
        .await;
    assert!(!err, "{body}");
    let r = &body["results"][0];
    assert_eq!(r["rows"].as_array().unwrap().len(), 3);
    assert_eq!(
        r["rows"][0],
        json!({
            "id": 1, "name": "Ada", "email": "ada@example.com", "score": 9.5,
            "data": "base64:AAE=", "meta": "{\"a\":1}", "id (2)": 1,
        })
    );
    assert_eq!(r["rows"][1]["email"], Json::Null);
    assert_eq!(r["columns"][0]["name"], "id");
    assert!(r.get("truncated").is_none());
    assert!(body.get("note").is_none());

    // The row limit.
    let (body, err) = f
        .call(
            "run_query",
            json!({ "connection": "Shop", "sql": "SELECT * FROM users", "limit": 2 }),
        )
        .await;
    assert!(!err);
    assert_eq!(body["results"][0]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(body["results"][0]["truncated"], true);
    assert!(body["note"].as_str().unwrap().contains("Not every row"));

    // Several statements; a failing one stops the run and marks the call as an error.
    let (body, err) = f
        .call(
            "run_query",
            json!({ "connection": "Shop", "sql": "SELECT 1 AS a; SELECT * FROM missing; SELECT 2" }),
        )
        .await;
    assert!(err);
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["rows"], json!([{ "a": 1 }]));
    assert!(
        results[1]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("missing")
    );
    assert!(body["notRun"].as_str().unwrap().starts_with("1 later"));

    // Each statement is in the history, from MCP.
    let history = f.history();
    assert!(history.iter().all(|h| h.source == "mcp"));
    assert!(
        history
            .iter()
            .any(|h| h.sql == "SELECT * FROM missing" && !h.success)
    );
    assert!(
        history
            .iter()
            .any(|h| h.sql.starts_with("SELECT id, name") && h.row_count == Some(3))
    );
    assert!(history.iter().all(|h| h.connection_name == "Shop"));
}

#[tokio::test]
async fn writes_need_the_pref_silent_mode_and_a_writable_connection() {
    let f = fixture(enabled()).await;
    let insert = "INSERT INTO users (name) VALUES ('Mallory')";
    for conn in ["Shop", "Shop RO", "Shop safe"] {
        let (body, err) = f
            .call("run_query", json!({ "connection": conn, "sql": insert }))
            .await;
        assert!(err, "{conn}: {body}");
        assert!(body.as_str().unwrap().starts_with("Refused"), "{body}");
    }
    // Hidden in a read: still refused.
    let (body, err) = f
        .call(
            "run_query",
            json!({ "connection": "Shop", "sql": "SELECT 1; DELETE FROM users" }),
        )
        .await;
    assert!(err, "{body}");
    assert_eq!(f.count_users(), 3);
    assert!(
        f.history()
            .iter()
            .any(|h| !h.success && h.error.as_deref().is_some_and(|e| e.starts_with("Refused")))
    );

    f.set_prefs(json!({ "mcpEnabled": true, "mcpAllowWrites": true }));
    for conn in ["Shop RO", "Shop safe"] {
        let (body, err) = f
            .call("run_query", json!({ "connection": conn, "sql": insert }))
            .await;
        assert!(err, "{conn}: {body}");
    }
    let (body, err) = f
        .call("run_query", json!({ "connection": "Shop", "sql": insert }))
        .await;
    assert!(!err, "{body}");
    assert_eq!(body["results"][0]["affectedRows"], 1);
    assert_eq!(f.count_users(), 4);

    // Turned off again: the next write is refused at once (and the session is read-only again).
    f.set_prefs(enabled());
    let (_, err) = f
        .call("run_query", json!({ "connection": "Shop", "sql": insert }))
        .await;
    assert!(err);
    assert_eq!(f.count_users(), 4);
}

#[tokio::test]
async fn read_only_sessions_stop_what_the_classifier_lets_through() {
    // The gate lets reads through; the server-side read-only session is the second line.
    let f = fixture(enabled()).await;
    let (body, err) = f
        .call(
            "run_query",
            json!({ "connection": "Shop", "sql": "SELECT name FROM users LIMIT 1" }),
        )
        .await;
    assert!(!err, "{body}");
    let ctx_pool_lease = f
        .server
        .ctx()
        .pool
        .get(
            &f.appdb
                .list_profiles()
                .unwrap()
                .into_iter()
                .find(|p| p.name == "Shop")
                .unwrap(),
            None,
            false,
        )
        .await
        .unwrap();
    assert!(ctx_pool_lease.profile.options.read_only);
    let err = ctx_pool_lease
        .session
        .execute("INSERT INTO users (name) VALUES ('x')", &[])
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("readonly")
            || err.to_string().to_lowercase().contains("read-only")
            || err.to_string().to_lowercase().contains("read only"),
        "{err}"
    );
}

#[tokio::test]
async fn explain_query_shows_the_plan_without_running_it() {
    let f = fixture(enabled()).await;
    let (body, err) = f
        .call(
            "explain_query",
            json!({ "connection": "Shop", "sql": "SELECT * FROM orders WHERE user_id = 1" }),
        )
        .await;
    assert!(!err, "{body}");
    assert!(
        body["plan"].as_str().unwrap().contains("orders_user"),
        "{body}"
    );
    // EXPLAIN of a write is only planned, so it is allowed while read-only.
    let (body, err) = f
        .call(
            "explain_query",
            json!({ "connection": "Shop", "sql": "DELETE FROM users" }),
        )
        .await;
    assert!(!err, "{body}");
    assert_eq!(f.count_users(), 3);
    assert!(
        f.history()
            .iter()
            .any(|h| h.source == "mcp" && h.sql == "DELETE FROM users")
    );
}

#[tokio::test]
async fn a_cancelled_call_gets_no_response() {
    let f = fixture(enabled()).await;
    let server = f.server.clone();
    let running = tokio::spawn(async move {
        server
            .handle(json!({
                "jsonrpc": "2.0", "id": 99, "method": "tools/call",
                "params": { "name": "run_query", "arguments": {
                    "connection": "Shop",
                    "sql": "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c",
                } },
            }))
            .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        f.rpc(
            json!({ "jsonrpc": "2.0", "method": "notifications/cancelled",
                     "params": { "requestId": 99, "reason": "user" } })
        )
        .await
        .is_none()
    );
    let reply = tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("the statement was interrupted")
        .unwrap();
    assert!(reply.is_none(), "{reply:?}");
}

#[tokio::test]
async fn serves_lines_over_a_byte_stream() {
    let f = fixture(enabled()).await;
    let input = [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "run_query", "arguments": { "connection": "Shop", "sql": "SELECT 'a\nb' AS t" } } }),
    ]
    .iter()
    .map(|m| m.to_string())
    .collect::<Vec<_>>()
    .join("\n")
        + "\n\nnot json\r\n";
    let (client, server_side) = tokio::io::duplex(1 << 20);
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (server_read, server_write) = tokio::io::split(server_side);
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    client_write.write_all(input.as_bytes()).await.unwrap();
    client_write.shutdown().await.unwrap();
    drop(client_write);
    server::serve(
        f.server.clone(),
        tokio::io::BufReader::new(server_read),
        server_write,
    )
    .await;
    let mut out = String::new();
    client_read.read_to_string(&mut out).await.unwrap();
    let replies: Vec<Json> = out
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(replies.len(), 3, "{out}");
    let by_id = |id: Json| replies.iter().find(|r| r["id"] == id).unwrap().clone();
    assert_eq!(by_id(json!(1))["result"]["protocolVersion"], "2025-06-18");
    let text = by_id(json!(2))["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    let body: Json = serde_json::from_str(&text).unwrap();
    assert_eq!(body["results"][0]["rows"][0]["t"], "a\nb");
    assert_eq!(by_id(Json::Null)["error"]["code"], server::PARSE_ERROR);
    // The client went away: its connections are closed.
    assert_eq!(f.server.ctx().pool.open_count(), 0);
}

// ----- limits: line size, requests in flight, sql size ---------------------------------------------

/// A client of `serve_with` over an in-memory stream.
struct Wire {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    served: tokio::task::JoinHandle<()>,
}

impl Wire {
    fn open(f: &Fixture, limits: Limits) -> Wire {
        let (client, server_side) = tokio::io::duplex(1 << 16);
        let (client_read, client_write) = tokio::io::split(client);
        let (server_read, server_write) = tokio::io::split(server_side);
        let server = f.server.clone();
        let served = tokio::spawn(async move {
            server::serve_with(
                server,
                tokio::io::BufReader::new(server_read),
                server_write,
                limits,
            )
            .await
        });
        use tokio::io::AsyncBufReadExt;
        Wire {
            write: client_write,
            lines: tokio::io::BufReader::new(client_read).lines(),
            served,
        }
    }

    async fn send_raw(&mut self, bytes: &[u8]) {
        use tokio::io::AsyncWriteExt;
        self.write.write_all(bytes).await.unwrap();
    }

    async fn send(&mut self, msg: Json) {
        self.send_raw(format!("{msg}\n").as_bytes()).await;
    }

    /// The next reply the server wrote.
    async fn next(&mut self) -> Json {
        let line = tokio::time::timeout(Duration::from_secs(20), self.lines.next_line())
            .await
            .expect("the server answered")
            .unwrap()
            .expect("a reply, not the end of the stream");
        serde_json::from_str(&line).unwrap()
    }

    /// Closes the client's end and waits for the server to finish; the replies still unread.
    async fn finish(mut self) -> Vec<Json> {
        use tokio::io::AsyncWriteExt;
        self.write.shutdown().await.unwrap();
        drop(self.write);
        tokio::time::timeout(Duration::from_secs(20), self.served)
            .await
            .expect("the server stopped")
            .unwrap();
        let mut rest = Vec::new();
        while let Some(line) = self.lines.next_line().await.unwrap() {
            rest.push(serde_json::from_str(&line).unwrap());
        }
        rest
    }
}

fn ping(id: i64) -> Json {
    json!({ "jsonrpc": "2.0", "id": id, "method": "ping" })
}

/// A ping request whose JSON text is exactly `len` bytes.
fn ping_of_len(id: i64, len: usize) -> String {
    let bare = json!({ "jsonrpc": "2.0", "id": id, "method": "ping", "pad": "" }).to_string();
    assert!(len >= bare.len());
    json!({ "jsonrpc": "2.0", "id": id, "method": "ping", "pad": "x".repeat(len - bare.len()) })
        .to_string()
}

#[tokio::test]
async fn read_line_bounds_a_line_and_drops_the_rest_of_a_long_one() {
    use super::server::{Line, read_line};
    let mut data = Vec::new();
    data.extend_from_slice(b"short\n");
    data.extend_from_slice(&[b'a'; 10_000]); // far over the limit of 16
    data.extend_from_slice(b"\n");
    data.extend_from_slice(b"0123456789abcdef\n"); // exactly the limit
    data.extend_from_slice(b"0123456789abcdefX\n"); // one over
    data.extend_from_slice(b"\r\n"); // an empty line with CRLF
    data.extend_from_slice(b"last without newline");
    // A tiny buffer: the long line is read away in many small chunks.
    let mut reader = tokio::io::BufReader::with_capacity(7, &data[..]);
    let mut got = Vec::new();
    loop {
        match read_line(&mut reader, 16).await.unwrap() {
            Line::Eof => break,
            Line::Text(t) => got.push(Some(String::from_utf8(t).unwrap())),
            Line::TooLong => got.push(None),
        }
    }
    assert_eq!(
        got,
        vec![
            Some("short".to_string()),
            None,
            Some("0123456789abcdef".to_string()),
            None,
            Some("\r".to_string()),
            // 20 bytes: over 16, and it has no newline.
            None,
        ]
    );
}

#[tokio::test]
async fn an_oversized_line_gets_the_too_large_error_and_the_next_line_is_answered() {
    let f = fixture(enabled()).await;
    let limits = Limits {
        max_line_bytes: 300,
        ..Limits::default()
    };
    let mut w = Wire::open(&f, limits);
    // At the limit: answered. One byte over: refused (no id can be read from it).
    w.send_raw(format!("{}\n", ping_of_len(1, 300)).as_bytes())
        .await;
    assert_eq!(w.next().await["id"], 1);
    w.send_raw(format!("{}\n", ping_of_len(2, 301)).as_bytes())
        .await;
    let refused = w.next().await;
    assert_eq!(refused["id"], Json::Null);
    assert_eq!(refused["error"]["code"], server::INVALID_REQUEST);
    assert_eq!(refused["error"]["message"], "Request too large");
    // A line of 200 KB, in many reads, then a normal request: still served.
    w.send_raw(&vec![b'z'; 200_000]).await;
    w.send_raw(b"\n").await;
    w.send(ping(3)).await;
    let refused = w.next().await;
    assert_eq!(refused["error"]["message"], "Request too large");
    let answered = w.next().await;
    assert_eq!(answered["id"], 3);
    assert!(answered["result"].is_object(), "{answered}");
    assert!(w.finish().await.is_empty());
}

#[tokio::test]
async fn the_default_line_limit_is_8_mib() {
    assert_eq!(server::MAX_LINE_BYTES, 8 * 1024 * 1024);
    let f = fixture(enabled()).await;
    let mut w = Wire::open(&f, Limits::default());
    // A request whose `sql` is 9 MiB: refused at the line, never parsed.
    let big = json!({
        "jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": { "name": "run_query", "arguments": {
            "connection": "Shop", "sql": "a".repeat(9 * 1024 * 1024) } },
    });
    w.send(big).await;
    w.send(ping(6)).await;
    let refused = w.next().await;
    assert_eq!(refused["error"]["code"], server::INVALID_REQUEST);
    assert_eq!(refused["error"]["message"], "Request too large");
    assert_eq!(w.next().await["id"], 6);
    // Nothing of it reached the history.
    assert!(f.history().is_empty());
}

#[tokio::test]
async fn a_busy_server_refuses_new_requests_but_still_reads_a_cancellation() {
    let f = fixture(enabled()).await;
    let limits = Limits {
        max_in_flight: 1,
        ..Limits::default()
    };
    let mut w = Wire::open(&f, limits);
    // One request takes the only slot and runs until it is cancelled.
    w.send(json!({
        "jsonrpc": "2.0", "id": 99, "method": "tools/call",
        "params": { "name": "run_query", "arguments": {
            "connection": "Shop",
            "sql": "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c",
        } },
    }))
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // A new tools/call, and a ping, get the busy error with their own ids.
    w.send(json!({
        "jsonrpc": "2.0", "id": 100, "method": "tools/call",
        "params": { "name": "run_query", "arguments": { "connection": "Shop", "sql": "SELECT 1" } },
    }))
    .await;
    let busy = w.next().await;
    assert_eq!(busy["id"], 100);
    assert_eq!(busy["error"]["code"], server::SERVER_BUSY);
    assert_eq!(busy["error"]["code"], -32000);
    assert_eq!(busy["error"]["message"], "Too many requests in flight");
    w.send(ping(101)).await;
    assert_eq!(w.next().await["error"]["code"], -32000);
    // A batch: its requests are refused, its notification is not dropped.
    w.send(json!([
        ping(102),
        { "jsonrpc": "2.0", "method": "notifications/initialized" },
    ]))
    .await;
    let refused = w.next().await;
    assert_eq!(refused[0]["id"], 102);
    assert_eq!(refused[0]["error"]["code"], -32000);
    assert_eq!(refused.as_array().unwrap().len(), 1);
    // The cancellation goes through although no slot is free: the running query is interrupted,
    // and it gets no response.
    w.send(
        json!({ "jsonrpc": "2.0", "method": "notifications/cancelled",
                   "params": { "requestId": 99, "reason": "user" } }),
    )
    .await;
    let mut next_id = 200;
    let answered = loop {
        next_id += 1;
        w.send(ping(next_id)).await;
        let reply = w.next().await;
        assert_ne!(reply["id"], 99, "a cancelled call is not answered: {reply}");
        if reply.get("result").is_some() {
            break reply;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(answered["id"], next_id);
    // The batch's notification ran too.
    assert!(f.server.is_initialized());
    assert!(w.finish().await.is_empty());
}

#[test]
fn a_request_without_a_slot_is_told_so_and_a_notification_is_kept() {
    use super::server::refuse_busy;
    let (replies, rest) = refuse_busy(&ping(1).to_string());
    assert_eq!(replies.len(), 1);
    assert!(replies[0].contains("-32000") && replies[0].contains("Too many requests in flight"));
    assert!(rest.is_none());
    let note = json!({ "jsonrpc": "2.0", "method": "notifications/cancelled",
                       "params": { "requestId": 1 } })
    .to_string();
    let (replies, rest) = refuse_busy(&note);
    assert!(replies.is_empty());
    assert_eq!(rest.as_deref(), Some(note.as_str()));
    // Not JSON: left to the parser, which answers it.
    let (replies, rest) = refuse_busy("{nope");
    assert!(replies.is_empty());
    assert_eq!(rest.as_deref(), Some("{nope"));
    // A batch is split: requests refused (each with its id), the notifications kept.
    let batch =
        json!([ping(7), { "jsonrpc": "2.0", "method": "notifications/initialized" }, ping(8)])
            .to_string();
    let (replies, rest) = refuse_busy(&batch);
    // One answer for the batch, holding both refusals.
    assert_eq!(replies.len(), 1);
    let answer: Json = serde_json::from_str(&replies[0]).unwrap();
    assert_eq!(answer[0]["id"], 7);
    assert_eq!(answer[1]["id"], 8);
    assert_eq!(answer[1]["error"]["code"], -32000);
    let kept: Json = serde_json::from_str(&rest.unwrap()).unwrap();
    assert_eq!(kept.as_array().unwrap().len(), 1);
    assert_eq!(kept[0]["method"], "notifications/initialized");
    // A batch of only requests leaves nothing to handle.
    let (replies, rest) = refuse_busy(&json!([ping(1), ping(2)]).to_string());
    assert_eq!(replies.len(), 1);
    assert!(rest.is_none());
}

#[tokio::test]
async fn an_oversized_sql_argument_is_refused() {
    let f = fixture(enabled()).await;
    assert_eq!(MAX_SQL_BYTES, 1024 * 1024);
    let before = f.history().len();
    let big = format!("SELECT '{}'", "a".repeat(MAX_SQL_BYTES));
    for tool in ["run_query", "explain_query"] {
        let (body, is_error) = f
            .call(tool, json!({ "connection": "Shop", "sql": big }))
            .await;
        assert!(is_error, "{tool}");
        let text = body.as_str().unwrap();
        assert!(
            text.contains("too long") && text.contains("1024 KiB"),
            "{tool}: {text}"
        );
    }
    // Neither ran nor was logged (the text would be stored in the history).
    assert_eq!(f.history().len(), before);
    // Exactly at the limit is accepted.
    let at_limit = format!(
        "SELECT '{}' AS t",
        "a".repeat(MAX_SQL_BYTES - "SELECT '' AS t".len())
    );
    assert_eq!(at_limit.len(), MAX_SQL_BYTES);
    let (_, is_error) = f
        .call(
            "run_query",
            json!({ "connection": "Shop", "sql": at_limit }),
        )
        .await;
    assert!(!is_error);
}

#[tokio::test]
async fn use_is_only_special_for_mysql() {
    // SQLite has no USE: the statement reaches the engine (and its syntax error comes back), the
    // gate does not refuse it.
    let f = fixture(enabled()).await;
    let (body, _) = f
        .call(
            "run_query",
            json!({ "connection": "Shop", "sql": "USE other" }),
        )
        .await;
    let text = body.to_string();
    assert!(!text.contains("not supported over MCP"), "{text}");
}

#[test]
fn the_mcp_flag_is_the_first_argument() {
    assert!(super::requested([
        "/Applications/Tabler.app/Contents/MacOS/tabler",
        "--mcp"
    ]));
    assert!(!super::requested(["tabler"]));
    assert!(!super::requested(["tabler", "postgres://x", "--mcp"]));
    assert!(!super::requested(["tabler", "--mcpx"]));
}
