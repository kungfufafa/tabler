//! The DuckDB driver end to end on the real engine (it runs in the test process): `entry::open`
//! on temp files and in-memory databases, the form's calls, the caps, and the session under the
//! app's command bodies: the sidebar, the grid, commits, the editor, cancel, the time out,
//! Explain, the hooks and read-only refusals on every write path.
//!
//! Every test closes its sessions before its `TempDir` goes, so the file is unlocked.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::{RwLock, watch};

use super::engine;
use super::entry::{self, CHECK_FILE, CREATE_FILE};
use super::session::{DuckdbSession, EDIT_FEATURES, FEATURES, LOGGER_REFUSED};
use super::testing::{build_sample, sample, sql_path};
use crate::appdb::AppDb;
use crate::commands::browse::{DEFAULT_ESTIMATE_ABOVE, browse_rows, count_rows, fetch_cell_on};
use crate::commands::ddl::table_ddl_on;
use crate::commands::lock::session_details_of;
use crate::commands::mutate::apply_changes_on;
use crate::commands::query::{explain_inner, query_limited};
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::native::CallKind;
use crate::drivers::registry::UtilInput;
use crate::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, Session, TxStatement, request_cancel,
};
use crate::error::AppError;
use crate::secrets::{Backend, Secrets};
use crate::state::{AppState, Connection as AppConnection};
use crate::types::browse::{BrowseRequest, RowIdentity, RowKey};
use crate::types::caps::{CountMode, Feature, SafeConfirm, TxMode};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::{ApplyRequest, ServerExpr, StagedChange};
use crate::types::objects::{RoutineKind, RoutineRef};
use crate::types::profile::{ConnStatus, ConnectionInfo, ConnectionProfile};
use crate::types::query::{ExplainMode, StatementKind};
use crate::types::schema::{TableKind, TableRef};
use crate::types::value::Value;

fn profile(fields: serde_json::Value) -> ConnectionProfile {
    let mut json = json!({
        "id": uuid::Uuid::new_v4(), "name": "duck", "driver": "duckdb",
        "ssl": "disable", "options": {}, "sortOrder": 0, "createdAt": "", "updatedAt": "",
    });
    if let (Some(base), Some(extra)) = (json.as_object_mut(), fields.as_object()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    serde_json::from_value(json).expect("a profile")
}

fn file_profile(path: &Path, options: serde_json::Value) -> ConnectionProfile {
    profile(json!({ "sqlitePath": path.to_string_lossy(), "options": options }))
}

async fn open(p: &ConnectionProfile, password: Option<&str>, unattended: bool) -> DuckdbSession {
    try_open(p, password, unattended)
        .await
        .unwrap_or_else(|e| panic!("open: {e:?}"))
}

async fn try_open(
    p: &ConnectionProfile,
    password: Option<&str>,
    unattended: bool,
) -> Result<DuckdbSession, AppError> {
    let target = ConnectTarget {
        profile: p,
        password,
        host_override: None,
    };
    entry::open_session(&target, unattended).await
}

/// A new app state with one open connection of `profile` holding `session`, for the command
/// bodies (`browse_rows`, `apply_changes_on`…).
async fn app_connection(
    profile: ConnectionProfile,
    session: Session,
) -> (Arc<AppState>, Arc<AppConnection>) {
    let appdb = AppDb::open_in_memory().expect("an app database");
    let profile = appdb.upsert_profile(&profile).expect("a saved profile");
    let state = Arc::new(AppState::new(
        appdb.clone(),
        Secrets::new(Backend::File, appdb),
    ));
    let id = uuid::Uuid::new_v4();
    let conn = Arc::new(AppConnection {
        id,
        profile: profile.clone(),
        info: RwLock::new(ConnectionInfo {
            connection_id: id,
            profile_id: profile.id,
            name: profile.name.clone(),
            color: None,
            driver: profile.driver,
            server_version: String::new(),
            current_database: None,
            current_user: None,
            tunnel: None,
            read_only: profile.options.read_only,
            capabilities: Some(session.capabilities()),
        }),
        session: RwLock::new(session),
        schema_cache: RwLock::new(HashMap::new()),
        status: watch::channel(ConnStatus::Connected).0,
        tunnel: None,
    });
    state.insert_connection(conn.clone(), None).await;
    (state, conn)
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn message(e: &AppError) -> String {
    match e {
        AppError::Db { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

async fn run(s: &DuckdbSession, sql: &str) -> Result<crate::drivers::StatementOutcome, AppError> {
    let mut sink = CollectSink::default();
    s.run_statement(sql, 1000, &mut sink, CancelSlot::default())
        .await
}

async fn rows(s: &DuckdbSession, sql: &str) -> Vec<Vec<Value>> {
    s.query(sql, &[])
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e:?}"))
        .rows
}

fn util_input<'a>(p: &'a ConnectionProfile, secret: Option<&'a str>) -> UtilInput<'a> {
    UtilInput { profile: p, secret }
}

// --- Opening, the form's calls and the caps ------------------------------------------------

#[tokio::test]
async fn open_connects_to_a_file_and_refuses_a_missing_one() {
    let (dir, path) = sample();
    let p = file_profile(&path, json!({}));
    let s = open(&p, None, false).await;
    let info = s.server_info().await.unwrap();
    assert!(info.version.contains("v1.5"), "{}", info.version);
    assert_eq!(info.current_database.as_deref(), Some("sample"));
    s.ping().await.unwrap();
    s.close().await;
    // Closed: the file is let go.
    assert!(engine::open_instance(&path).is_none());

    // DuckDB would create a missing file: refused, and nothing is created.
    let missing = dir.path().join("missing.duckdb");
    let e = try_open(&file_profile(&missing, json!({})), None, false)
        .await
        .err()
        .expect("refused");
    assert!(!missing.exists(), "{e:?}");
    // Not a DuckDB file.
    let junk = dir.path().join("junk.duckdb");
    std::fs::write(&junk, b"not a database at all, really not").unwrap();
    assert!(
        try_open(&file_profile(&junk, json!({})), None, false)
            .await
            .is_err()
    );

    // The profile's database is where both connections start.
    let other = dir.path().join("other.duckdb");
    let startup = format!("ATTACH {} AS other", sql_path(&other));
    let mut p = file_profile(&path, json!({ "startupSql": startup }));
    p.database = Some("other".into());
    let s = open(&p, None, false).await;
    assert_eq!(
        s.server_info().await.unwrap().current_database.as_deref(),
        Some("other")
    );
    run(&s, "CREATE TABLE in_other AS SELECT 1 AS a")
        .await
        .unwrap();
    assert_eq!(
        rows(&s, "SELECT count(*) FROM other.main.in_other").await,
        vec![vec![Value::Int(1)]]
    );
    s.close().await;
    drop(dir);
}

#[test]
fn validate_checks_the_params_the_location_and_the_startup_sql() {
    entry::validate(&profile(json!({ "sqlitePath": "C:/x.duckdb" }))).unwrap();
    entry::validate(&profile(json!({
        "sqlitePath": "C:/x.duckdb",
        "options": { "driverParams": { "autoloadExtensions": true } }
    })))
    .unwrap();
    let bad = |fields: serde_json::Value| {
        let e = entry::validate(&profile(fields)).unwrap_err();
        assert_eq!(e.code(), "INVALID_INPUT", "{e:?}");
        e.to_string()
    };
    bad(json!({ "options": { "driverParams": { "autoloadExtensions": "yes" } } }));
    let e = bad(json!({ "sqlitePath": ":memory:", "options": { "readOnly": true } }));
    assert!(e.contains("in-memory"), "{e}");
    let e = bad(json!({
        "sqlitePath": "C:/x.duckdb",
        "options": { "startupSql": "SET threads = 2; CALL enable_logging(storage := 'file')" }
    }));
    assert!(e.contains(LOGGER_REFUSED), "{e}");
}

#[tokio::test]
async fn util_checks_and_creates_files() {
    let (dir, path) = sample();
    let p = file_profile(&path, json!({}));
    let ok = entry::util(util_input(&p, None), CHECK_FILE, json!({}))
        .await
        .unwrap();
    assert_eq!(ok["path"], json!(path.to_string_lossy()));
    // A file this process has open is DuckDB's already.
    let s = open(&p, None, false).await;
    entry::util(util_input(&p, None), CHECK_FILE, json!({}))
        .await
        .unwrap();
    s.close().await;
    let junk = dir.path().join("junk.bin");
    std::fs::write(&junk, b"0123456789abcdef0123").unwrap();
    let args = json!({ "path": junk.to_string_lossy() });
    assert!(
        entry::util(util_input(&p, None), CHECK_FILE, args)
            .await
            .is_err()
    );

    // New…: an empty database that opens.
    let new = dir.path().join("new.duckdb");
    let args = json!({ "path": new.to_string_lossy() });
    entry::util(util_input(&p, None), CREATE_FILE, args.clone())
        .await
        .unwrap();
    assert!(new.exists());
    let s = open(&file_profile(&new, json!({})), None, false).await;
    assert!(s.list_tables("main").await.unwrap().is_empty());
    s.close().await;
    // Not over an existing file, and never encrypted (DuckDB here cannot write that).
    assert!(
        entry::util(util_input(&p, None), CREATE_FILE, args)
            .await
            .is_err()
    );
    let enc = json!({ "path": dir.path().join("enc.duckdb").to_string_lossy() });
    let e = entry::util(util_input(&p, Some("k3y")), CREATE_FILE, enc)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("encrypted"), "{e}");
    let e = entry::util(util_input(&p, None), "dropFile", json!({}))
        .await
        .unwrap_err();
    assert_eq!(e.code(), "UNSUPPORTED", "{e:?}");

    assert_eq!(entry::util_kind(CHECK_FILE), CallKind::Read);
    assert_eq!(entry::util_kind(CREATE_FILE), CallKind::Write);
    assert_eq!(entry::util_kind("anything"), CallKind::Write);
    drop(dir);
}

#[tokio::test]
async fn caps_grant_what_the_session_does() {
    let (dir, path) = sample();

    // A read-write file: edits, a database switch (the same instance), the classifier.
    let s = open(&file_profile(&path, json!({})), None, false).await;
    let caps = s.capabilities();
    for f in FEATURES.iter().chain(EDIT_FEATURES.iter()) {
        assert!(caps.has(*f), "{f:?}");
    }
    assert!(caps.has(Feature::SwitchDatabase) && caps.has(Feature::StructureView));
    for f in [
        Feature::StructureEdit,
        Feature::ExportTables,
        Feature::ImportCsv,
        Feature::Backup,
        Feature::ProcessList,
        Feature::Users,
        Feature::Sequences,
        Feature::Encoding,
    ] {
        assert!(!caps.has(f), "{f:?}");
    }
    assert!(caps.namespaces.databases);
    assert_eq!(caps.namespaces.default_schema.as_deref(), Some("main"));
    assert_eq!(caps.data.count, CountMode::Estimate);
    assert_eq!(caps.data.transactions, TxMode::Atomic);
    assert!(!caps.data.default_keyword && !caps.data.upsert);
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    assert!(!caps.safety.hardened);
    assert_eq!(caps.safety.confirm, SafeConfirm::Password);
    assert_eq!(caps.session.keepalive_secs, Some(30));
    assert!(!caps.session.ping_billed);
    s.close().await;

    // Read-only: the engine refuses (Server), hardened, no edits.
    let s = open(
        &file_profile(&path, json!({ "readOnly": true })),
        None,
        false,
    )
    .await;
    let caps = s.capabilities();
    assert_eq!(caps.safety.read_only, ReadOnlyMode::Server);
    assert!(caps.safety.hardened);
    assert!(EDIT_FEATURES.iter().all(|f| !caps.has(*f)));
    assert!(caps.has(Feature::Mcp) && caps.has(Feature::SwitchDatabase));
    s.close().await;

    // Unattended (MCP) on a read-write profile: hardened, still only the classifier.
    let s = open(&file_profile(&path, json!({})), None, true).await;
    let caps = s.capabilities();
    assert!(caps.safety.hardened);
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    s.close().await;

    // In memory: a second session would be another database, so no switch.
    let s = open(&profile(json!({ "sqlitePath": ":memory:" })), None, false).await;
    let caps = s.capabilities();
    assert!(!caps.has(Feature::SwitchDatabase) && !caps.namespaces.databases);
    assert!(caps.has(Feature::EditRows));
    s.close().await;

    // A Command Line key: Safe Mode asks for the passcode.
    let enc = encrypted_file(dir.path());
    let p = file_profile(
        &enc,
        json!({ "passwordMode": "command", "passwordCommand": "printf k3y" }),
    );
    let s = open(&p, Some("k3y"), false).await;
    let caps = s.capabilities();
    assert_eq!(caps.safety.confirm, SafeConfirm::Passcode);
    // An encrypted file is read-only here: no edits, and no Server claim (other databases
    // still take writes).
    assert!(EDIT_FEATURES.iter().all(|f| !caps.has(*f)));
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    assert_eq!(
        rows(&s, "SELECT x FROM t").await,
        vec![vec![Value::Int(42)]]
    );
    s.close().await;
    drop(dir);
}

/// An encrypted database file (key `k3y`) holding `t(x)` = 42.
fn encrypted_file(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("enc.duckdb");
    let c = ::duckdb::Connection::open_in_memory().unwrap();
    // Writing one needs httpfs's crypto; the test fixture may use the unsafe fallback.
    c.execute_batch(&format!(
        "SET force_mbedtls_unsafe = true; ATTACH {} AS e (ENCRYPTION_KEY 'k3y'); \
         CREATE TABLE e.t AS SELECT 42 AS x; DETACH e;",
        sql_path(&path)
    ))
    .unwrap();
    path
}

#[tokio::test]
async fn second_sessions_share_the_instance_or_are_refused() {
    let (dir, path) = sample();
    let p = file_profile(&path, json!({}));
    let a = open(&p, None, false).await;
    // A second workspace, or a database switch, of the same settings: the same instance.
    let mut switched = p.clone();
    switched.database = Some("sample".into());
    let b = open(&switched, None, false).await;
    run(&a, "CREATE TABLE seen AS SELECT 7 AS n").await.unwrap();
    assert_eq!(
        rows(&b, "SELECT n FROM seen").await,
        vec![vec![Value::Int(7)]]
    );
    // Other settings while it is open: refused with the way out.
    let ro = file_profile(&path, json!({ "readOnly": true }));
    let e = try_open(&ro, None, false).await.err().expect("refused");
    assert!(message(&e).contains("other settings"), "{e:?}");
    a.close().await;
    assert!(engine::open_instance(&path).is_some(), "b still holds it");
    b.close().await;
    // All closed: other settings open it now.
    let c = open(&ro, None, false).await;
    c.close().await;
    drop(dir);
}

// --- The sidebar, structure and hooks ------------------------------------------------------

#[tokio::test]
async fn the_sidebar_lists_databases_schemas_tables_and_structure() {
    let (dir, path) = sample();
    let s = open(&file_profile(&path, json!({})), None, false).await;
    let dbs = s.list_databases().await.unwrap();
    assert!(dbs.iter().any(|d| d.name == "sample"), "{dbs:?}");
    let schemas: Vec<String> = s
        .list_schemas()
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(schemas.contains(&"main".into()) && schemas.contains(&"crm".into()));
    let tables = s.list_tables("main").await.unwrap();
    let kind = |n: &str| tables.iter().find(|t| t.name == n).map(|t| t.kind);
    assert_eq!(kind("orders"), Some(TableKind::Table));
    assert_eq!(kind("recent_orders"), Some(TableKind::View));
    let meta = s
        .table_meta(&TableRef::new(Some("main"), "orders"))
        .await
        .unwrap();
    assert_eq!(meta.columns[0].name, "id");
    assert!(
        meta.foreign_keys
            .iter()
            .any(|f| f.ref_table.name == "customers")
    );
    assert!(meta.indexes.iter().any(|i| i.name == "orders_placed"));
    // No schema named: the current one.
    let meta = s
        .table_meta(&TableRef::new(None, "customers"))
        .await
        .unwrap();
    assert_eq!(meta.columns.len(), 2);
    let crm = s
        .table_meta(&TableRef::new(Some("crm"), "customers"))
        .await
        .unwrap();
    assert_eq!(crm.columns.len(), 6);

    let view = s
        .view_definition(&TableRef::new(Some("main"), "recent_orders"))
        .await
        .unwrap()
        .unwrap();
    assert!(view.contains("SELECT"), "{view}");
    let completion = s.completion_metadata().await.unwrap();
    let json = serde_json::to_string(&completion).unwrap();
    assert!(json.contains("placed_at"), "{json}");

    // The hooks: Show DDL, macros, session details.
    let ddl = s
        .table_ddl(&TableRef::new(Some("main"), "orders"))
        .await
        .unwrap()
        .unwrap();
    assert!(ddl.starts_with("CREATE TABLE"), "{ddl}");
    let view_ddl = s
        .table_ddl(&TableRef::new(Some("main"), "recent_orders"))
        .await
        .unwrap()
        .unwrap();
    assert!(view_ddl.contains("CREATE VIEW"), "{view_ddl}");
    let macros = s.list_routines("main").await.unwrap().unwrap();
    let names: Vec<&str> = macros.iter().map(|m| m.name.as_str()).collect();
    assert!(
        names.contains(&"add_tax") && names.contains(&"top_orders"),
        "{names:?}"
    );
    let def = s
        .routine_definition(&RoutineRef {
            schema: Some("main".into()),
            name: "add_tax".into(),
            kind: RoutineKind::Function,
            identity_arguments: String::new(),
        })
        .await
        .unwrap()
        .unwrap();
    assert!(def.contains("CREATE MACRO"), "{def}");
    let details = session_details_of(&s).await.unwrap();
    assert!(!details.tls);
    assert_eq!(details.encoding.as_deref(), Some("UTF-8"));
    // The catalog reads are traced as structure.
    assert!(!s.drain_trace().is_empty());
    s.close().await;
    drop(dir);
}

// --- The grid and commits ------------------------------------------------------------------

#[tokio::test]
async fn the_grid_reads_counts_fetches_and_commits_through_the_commands() {
    let (dir, path) = sample();
    let p = file_profile(&path, json!({}));
    let s: Arc<dyn DbSession> = Arc::new(open(&p, None, false).await);
    s.execute(
        "CREATE TABLE vals (id INTEGER PRIMARY KEY, big BIGINT, d DECIMAL(10, 2), \
         ts TIMESTAMP, s STRUCT(a INTEGER, b VARCHAR), l INTEGER[], blob BLOB)",
        &[],
    )
    .await
    .unwrap();
    s.execute(
        "INSERT INTO vals VALUES (1, 9007199254740993, 1.50, TIMESTAMP '2026-10-30 12:00:00', \
         {'a': 1, 'b': 'x'}, [1, 2], 'ab'::BLOB), (2, 2, 3.00, NULL, NULL, NULL, NULL), \
         (3, 3, 0.10, NULL, NULL, [], NULL)",
        &[],
    )
    .await
    .unwrap();
    let (state, conn) = app_connection(p, Session::Sql(s.clone())).await;
    let vals = TableRef::new(Some("main"), "vals");
    let req = |offset: u64| BrowseRequest {
        table: vals.clone(),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit: 2,
        offset,
        cursor: None,
    };
    let first = browse_rows(&conn, &req(0), None).await.unwrap();
    assert_eq!(first.identity, RowIdentity::Pk);
    assert_eq!(first.rows.len(), 2);
    let row = &first.rows[0];
    // 2^53 + 1 stays exact, a decimal is its text, a timestamp its text, nested values JSON.
    assert_eq!(row[1], Value::BigInt("9007199254740993".into()));
    assert_eq!(row[2], Value::Decimal("1.50".into()));
    assert_eq!(row[3], Value::Timestamp("2026-10-30 12:00:00".into()));
    assert!(
        matches!(&row[4], Value::Json(j) if j.contains("\"a\"")),
        "{:?}",
        row[4]
    );
    assert!(
        matches!(&row[5], Value::Json(j) if j.contains('2')),
        "{:?}",
        row[5]
    );
    let last = browse_rows(&conn, &req(2), None).await.unwrap();
    assert_eq!(last.rows.len(), 1);
    assert_eq!(last.rows[0][0], Value::Int(3));
    assert!(last.sql.contains("OFFSET"), "{}", last.sql);

    let counted = count_rows(&conn, &vals, None, true, DEFAULT_ESTIMATE_ABOVE, None)
        .await
        .unwrap();
    assert_eq!((counted.count, counted.exact), (3, true));
    let key = |id: i64| RowKey::Pk(BTreeMap::from([("id".into(), Value::Int(id))]));
    let cell = fetch_cell_on(&conn, &vals, &key(1), "blob", None)
        .await
        .unwrap();
    assert!(
        matches!(&cell, Value::Bytes(b) if b.len == 2 && !b.truncated),
        "{cell:?}"
    );

    // Two updates in one commit: the second key matches no row, so the commit fails, names it,
    // and the first is rolled back with it (Atomic).
    let customers = TableRef::new(Some("main"), "customers");
    let update = |row: &str, id: i64, name: &str| StagedChange::Update {
        row_id: row.into(),
        key: key(id),
        changes: BTreeMap::from([("name".into(), text(name))]),
        exprs: None,
    };
    let result = apply_changes_on(
        &state,
        conn.id,
        ApplyRequest {
            table: customers.clone(),
            changes: vec![update("r1", 1, "Ada L."), update("r2", 99, "Nobody")],
            returning: false,
        },
    )
    .await
    .unwrap();
    assert!(!result.success);
    assert!(result.error.is_some(), "{result:?}");
    assert_eq!(
        s.query("SELECT name FROM customers WHERE id = 1", &[])
            .await
            .unwrap()
            .rows,
        vec![vec![text("Ada")]]
    );

    // A table without a key: rows are addressed by key only, so the edit is refused.
    let events = TableRef::new(Some("main"), "events");
    let result = apply_changes_on(
        &state,
        conn.id,
        ApplyRequest {
            table: events,
            changes: vec![StagedChange::Delete {
                row_id: "e1".into(),
                key: RowKey::AllColumns(BTreeMap::from([
                    (
                        "happened_at".into(),
                        Value::Timestamp("2024-01-01 00:00:00".into()),
                    ),
                    ("kind".into(), text("login")),
                ])),
            }],
            returning: false,
        },
    )
    .await;
    assert!(!matches!(&result, Ok(r) if r.success), "{result:?}");
    assert_eq!(
        s.query("SELECT count(*) FROM events", &[])
            .await
            .unwrap()
            .rows,
        vec![vec![Value::Int(2)]]
    );

    // An update, an insert with a clock and a delete, read back through RETURNING.
    let result = apply_changes_on(
        &state,
        conn.id,
        ApplyRequest {
            table: TableRef::new(Some("crm"), "customers"),
            changes: vec![
                StagedChange::Update {
                    row_id: "c1".into(),
                    key: key(1),
                    changes: BTreeMap::from([("name".into(), text("Ada Lovelace"))]),
                    exprs: None,
                },
                StagedChange::Insert {
                    row_id: "c3".into(),
                    values: BTreeMap::from([
                        ("id".into(), Value::Int(3)),
                        ("name".into(), text("Grace")),
                    ]),
                    exprs: Some(BTreeMap::from([("created_at".into(), ServerExpr::Now)])),
                },
                StagedChange::Delete {
                    row_id: "c2".into(),
                    key: key(2),
                },
                // The other server expressions the caps grant: the column's DEFAULT (left
                // out of the INSERT) and CURRENT_TIMESTAMP.
                StagedChange::Insert {
                    row_id: "c4".into(),
                    values: BTreeMap::from([
                        ("id".into(), Value::Int(4)),
                        ("name".into(), text("Hopper")),
                    ]),
                    exprs: Some(BTreeMap::from([("created_at".into(), ServerExpr::Default)])),
                },
                StagedChange::Insert {
                    row_id: "c5".into(),
                    values: BTreeMap::from([
                        ("id".into(), Value::Int(5)),
                        ("name".into(), text("Liskov")),
                    ]),
                    exprs: Some(BTreeMap::from([(
                        "created_at".into(),
                        ServerExpr::CurrentTimestamp,
                    )])),
                },
            ],
            returning: true,
        },
    )
    .await
    .unwrap();
    assert!(result.success, "{:?}", result.error);
    assert!(
        result.results.iter().all(|r| r.affected == 1),
        "{:?}",
        result.results
    );
    for i in [1, 3, 4] {
        let inserted = result.results[i].returned.as_ref().expect("returned");
        assert!(
            matches!(inserted.get("created_at"), Some(Value::Timestamp(_))),
            "{inserted:?}"
        );
    }
    assert_eq!(
        s.query("SELECT id, name FROM crm.customers ORDER BY id", &[])
            .await
            .unwrap()
            .rows,
        vec![
            vec![Value::Int(1), text("Ada Lovelace")],
            vec![Value::Int(3), text("Grace")],
            vec![Value::Int(4), text("Hopper")],
            vec![Value::Int(5), text("Liskov")],
        ]
    );

    // Show DDL through its command.
    let ddl = table_ddl_on(&conn, &customers).await.unwrap();
    assert!(ddl.contains("PRIMARY KEY"), "{ddl}");
    s.close().await;
    drop(dir);
}

#[tokio::test]
async fn a_transaction_checks_every_guard_and_rolls_back() {
    let (dir, path) = sample();
    let s = open(&file_profile(&path, json!({})), None, false).await;
    let st = |sql: &str, expect: Option<u64>| TxStatement {
        sql: sql.into(),
        params: Vec::new(),
        expect_affected: expect,
    };
    let done = s
        .transaction(vec![
            st("UPDATE customers SET name = 'A' WHERE id = 1", Some(1)),
            st("DELETE FROM customers WHERE id = 3", Some(1)),
        ])
        .await
        .unwrap();
    assert_eq!(done.iter().map(|o| o.affected).collect::<Vec<_>>(), [1, 1]);
    let e = s
        .transaction(vec![
            st("UPDATE customers SET name = 'B' WHERE id = 1", Some(1)),
            st("UPDATE customers SET name = 'C' WHERE id = 42", Some(1)),
        ])
        .await
        .unwrap_err();
    assert!(!matches!(e, AppError::Cancelled), "{e:?}");
    // A server error names its statement.
    let e = s
        .transaction(vec![
            st("UPDATE customers SET name = 'B' WHERE id = 1", Some(1)),
            st("INSERT INTO customers VALUES (1, 'dup')", None),
        ])
        .await
        .unwrap_err();
    assert!(
        matches!(
            e,
            AppError::Db {
                statement_index: Some(1),
                ..
            }
        ),
        "{e:?}"
    );
    assert_eq!(
        rows(&s, "SELECT name FROM customers WHERE id = 1").await,
        vec![vec![text("A")]]
    );
    // The worker connection is usable after a rollback.
    assert_eq!(s.in_transaction(), Some(false));
    s.close().await;
    drop(dir);
}

// --- The editor, cancel, the time out, Explain ---------------------------------------------

#[tokio::test]
async fn the_editor_runs_in_its_connection_and_stops_at_the_row_cap() {
    let s = open(&profile(json!({ "sqlitePath": ":memory:" })), None, false).await;
    let mut sink = CollectSink::default();
    let out = s
        .run_statement(
            "SELECT * FROM range(5)",
            2,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(out.kind, StatementKind::Rows);
    assert_eq!((out.row_count, out.truncated), (2, true));
    assert_eq!(
        sink.sets[0].rows,
        [vec![Value::Int(0)], vec![Value::Int(1)]]
    );

    // A transaction lasts from one run to the next, on the editor's connection only.
    run(&s, "CREATE TABLE t (a INTEGER)").await.unwrap();
    run(&s, "BEGIN TRANSACTION").await.unwrap();
    assert_eq!(s.in_transaction(), Some(true));
    let ins = run(&s, "INSERT INTO t VALUES (1), (2)").await.unwrap();
    assert_eq!((ins.kind, ins.affected), (StatementKind::Affected, Some(2)));
    // The worker connection does not see the open transaction's rows.
    assert_eq!(
        rows(&s, "SELECT count(*) FROM t").await,
        vec![vec![Value::Int(0)]]
    );
    run(&s, "COMMIT").await.unwrap();
    assert_eq!(s.in_transaction(), Some(false));
    assert_eq!(
        rows(&s, "SELECT count(*) FROM t").await,
        vec![vec![Value::Int(2)]]
    );
    assert!(s.editor_run_lock().is_some());

    // Streaming for exports and the like, on the worker.
    let mut sink = CollectSink::default();
    let n = s
        .stream_query(
            "SELECT * FROM range(1000)",
            &[],
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(n, 1000);

    // An error reaches the editor with DuckDB's message.
    let e = run(&s, "SELEC 1").await.unwrap_err();
    assert!(message(&e).starts_with("Parser Error"), "{e:?}");
    // Every session refuses the logger, before the engine sees it.
    let e = run(
        &s,
        "CALL enable_logging(storage := 'file', storage_path := 'x')",
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("logger"), "{e:?}");
    s.close().await;
    // Closed: nothing runs.
    assert!(matches!(
        run(&s, "SELECT 1").await,
        Err(AppError::NotConnected)
    ));
}

const LONG_QUERY: &str = "SELECT count(*) FROM range(100000000000) r(i) WHERE i % 7 = 3";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_interrupts_the_running_statement() {
    let s = Arc::new(open(&profile(json!({ "sqlitePath": ":memory:" })), None, false).await);
    let slot = CancelSlot::default();
    let running = {
        let (s, slot) = (s.clone(), slot.clone());
        tokio::spawn(async move {
            let mut sink = CollectSink::default();
            s.run_statement(LONG_QUERY, 10, &mut sink, slot).await
        })
    };
    let mut handle = None;
    for _ in 0..400 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        if slot.is_armed() {
            handle = request_cancel(&slot);
            break;
        }
    }
    s.cancel(handle.expect("a running statement"))
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(20), running)
        .await
        .expect("the run ends")
        .unwrap();
    assert!(matches!(result, Err(AppError::Cancelled)), "{result:?}");
    // The editor's connection works afterwards.
    let mut sink = CollectSink::default();
    s.run_statement("SELECT 1", 10, &mut sink, CancelSlot::default())
        .await
        .unwrap();
    s.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_query_past_the_time_out_is_interrupted() {
    let s = open(&profile(json!({ "sqlitePath": ":memory:" })), None, false).await;
    let started = std::time::Instant::now();
    let e = query_limited(&s, LONG_QUERY, &[], Some(Duration::from_millis(200)))
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::QueryTimeout { .. }), "{e:?}");
    assert!(started.elapsed() < Duration::from_secs(20));
    // The worker connection is free again.
    assert_eq!(rows(&s, "SELECT 1").await, vec![vec![Value::Int(1)]]);
    s.close().await;
}

#[tokio::test]
async fn explain_shows_the_plan_and_analyze_runs_it() {
    let (dir, path) = sample();
    let s = open(&file_profile(&path, json!({})), None, false).await;
    let plan = explain_inner(
        &s,
        "SELECT * FROM orders",
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert!(!plan.rows.is_empty());
    let analyzed = explain_inner(
        &s,
        "SELECT * FROM orders",
        ExplainMode::Analyze,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert!(!analyzed.rows.is_empty());
    // Analyze of a write runs it in a transaction that is rolled back.
    explain_inner(
        &s,
        "DELETE FROM order_items",
        ExplainMode::Analyze,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        rows(&s, "SELECT count(*) FROM order_items").await,
        vec![vec![Value::Int(3)]]
    );
    s.close().await;
    drop(dir);
}

// --- Read-only and hardened sessions -------------------------------------------------------

#[tokio::test]
async fn a_read_only_session_refuses_every_write_path() {
    let (dir, path) = sample();
    let p = file_profile(&path, json!({ "readOnly": true }));
    let s: Arc<dyn DbSession> = Arc::new(open(&p, None, false).await);
    let refused = |e: AppError| {
        assert!(
            e.to_string().contains(READ_ONLY_MESSAGE) || matches!(e, AppError::Unsupported(_)),
            "{e:?}"
        );
    };
    let write = "DELETE FROM customers";
    refused(s.execute(write, &[]).await.unwrap_err());
    refused(s.query(write, &[]).await.unwrap_err());
    refused(
        s.execute_cancellable(write, &[], CancelSlot::default())
            .await
            .unwrap_err(),
    );
    refused(
        s.query_cancellable(write, &[], CancelSlot::default())
            .await
            .unwrap_err(),
    );
    let mut sink = CollectSink::default();
    refused(
        s.run_statement(write, 10, &mut sink, CancelSlot::default())
            .await
            .unwrap_err(),
    );
    refused(
        s.stream_query(write, &[], &mut sink, CancelSlot::default())
            .await
            .unwrap_err(),
    );
    refused(
        s.transaction(vec![TxStatement {
            sql: write.into(),
            params: Vec::new(),
            expect_affected: None,
        }])
        .await
        .unwrap_err(),
    );
    let (state, conn) = app_connection(p, Session::Sql(s.clone())).await;
    let e = apply_changes_on(
        &state,
        conn.id,
        ApplyRequest {
            table: TableRef::new(Some("main"), "customers"),
            changes: vec![StagedChange::Delete {
                row_id: "r".into(),
                key: RowKey::Pk(BTreeMap::from([("id".into(), Value::Int(1))])),
            }],
            returning: false,
        },
    )
    .await;
    match e {
        Err(e) => refused(e),
        Ok(r) => assert!(!r.success, "{r:?}"),
    }
    // Reads still work, and nothing changed.
    assert_eq!(
        s.query("SELECT count(*) FROM customers", &[])
            .await
            .unwrap()
            .rows,
        vec![vec![Value::Int(3)]]
    );

    // What the classifier calls a read but names a file is the engine's to refuse.
    let csv = dir.path().join("x.csv");
    std::fs::write(&csv, "a\n1\n").unwrap();
    let e = s
        .query(&format!("SELECT * FROM {}", sql_path(&csv)), &[])
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::Db { .. }), "{e:?}");
    s.close().await;
    drop(dir);
}

#[tokio::test]
async fn an_unattended_session_reads_no_files_and_keeps_its_settings() {
    let (dir, path) = sample();
    let s = open(&file_profile(&path, json!({})), None, true).await;
    let csv = dir.path().join("x.csv");
    std::fs::write(&csv, "a\n1\n").unwrap();
    for sql in [
        format!("SELECT * FROM {}", sql_path(&csv)),
        format!("SELECT * FROM read_csv({})", sql_path(&csv)),
        "SET enable_external_access = true".to_string(),
        "SET lock_configuration = false".to_string(),
    ] {
        assert!(run(&s, &sql).await.is_err(), "{sql}");
    }
    // It may still write its own database.
    run(&s, "CREATE TABLE mcp_made AS SELECT 1 AS a")
        .await
        .unwrap();
    s.close().await;
    drop(dir);
}

#[tokio::test]
async fn a_closed_session_lets_the_file_go() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.duckdb");
    build_sample(&path);
    let s = open(&file_profile(&path, json!({})), None, false).await;
    run(&s, "CREATE TABLE z AS SELECT 1 AS a").await.unwrap();
    s.close().await;
    assert!(engine::open_instance(&path).is_none());
    std::fs::remove_file(&path).expect("the file is unlocked");
    drop(dir);
}

/// A file can carry a call of `enable_logging` that no check of the SQL text sees (here a table
/// macro; a view is the same): with file storage that cannot be written, the logger fails
/// every statement of the instance, and closing a connection would abort the process. The
/// session closes anyway: the broken connections are leaked, and the test process lives on.
#[tokio::test]
async fn a_logger_switched_from_the_file_cannot_abort_the_process_on_close() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hostile.duckdb");
    build_sample(&path);
    // A file where the log folder would go, so the folder cannot be created.
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    {
        let inst = engine::Instance::open(engine::EngineConfig::file(&path)).unwrap();
        let conn = inst.connect().unwrap();
        conn.execute_batch(
            "CREATE MACRO crm.hostile(p) AS TABLE \
             SELECT * FROM enable_logging(storage := 'file', storage_path := p)",
        )
        .unwrap();
        inst.release(conn);
    }
    let s = open(&file_profile(&path, json!({})), None, false).await;
    // The text check refuses what it can see, dynamic SQL included.
    for sql in [
        "SELECT * FROM query('SELECT * FROM enable_' || 'logging()')",
        "SELECT * FROM crm.\"enable_logging\"()",
    ] {
        let e = s.query(sql, &[]).await.unwrap_err();
        assert!(e.to_string().contains(LOGGER_REFUSED), "{sql}: {e:?}");
    }
    assert_eq!(rows(&s, "SELECT 1").await, vec![vec![Value::Int(1)]]);
    // The stored macro gets past it.
    let logs = blocker.join("logs");
    let _ = s
        .query(
            &format!("SELECT * FROM crm.hostile({})", sql_path(&logs)),
            &[],
        )
        .await;
    assert!(s.query("SELECT 1", &[]).await.is_err());
    assert!(run(&s, "SELECT 1").await.is_err());
    let inst = engine::open_instance(&path).expect("the open instance");
    s.close().await;
    assert!(inst.is_broken());
    // Its root connection is leaked too (dropping the last handle closes nothing), and the
    // registry no longer lists the instance.
    drop(inst);
    assert!(engine::open_instance(&path).is_none());
    assert_refused_until_restart(&path).await;
    drop(dir);
}

/// The leaked connections still hold `path`: no new session opens it, by any spelling of its
/// path, read-only or unattended, and neither does the form's file check.
async fn assert_refused_until_restart(path: &Path) {
    let spelled = path
        .parent()
        .expect("a folder")
        .join(".")
        .join(path.file_name().expect("a file name"));
    for (p, unattended) in [
        (file_profile(path, json!({})), false),
        (file_profile(&spelled, json!({})), false),
        (file_profile(path, json!({ "readOnly": true })), false),
        (file_profile(path, json!({})), true),
    ] {
        let e = try_open(&p, None, unattended)
            .await
            .err()
            .expect("the file is refused");
        assert!(e.to_string().contains(engine::LOGGER_SWITCHED), "{e:?}");
    }
    let e = engine::check_database_file(&path.to_string_lossy()).unwrap_err();
    assert!(e.to_string().contains(engine::LOGGER_SWITCHED), "{e:?}");
    assert!(engine::open_instance(path).is_none());
}

/// A logger that flushes only once its buffer is full, and fails then: closing a connection
/// with an open transaction would log the rollback inside DuckDB's destructor, and the flush
/// that entry triggers would abort the process. Swept over buffer sizes and editor statements,
/// so the entry that fills the buffer lands on every step of the close.
#[tokio::test]
async fn a_buffered_logger_cannot_abort_the_process_when_a_transaction_closes() {
    for size in 1..=5 {
        for extra in 0..=3 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("hostile.duckdb");
            build_sample(&path);
            let blocker = dir.path().join("blocker");
            std::fs::write(&blocker, "x").unwrap();
            {
                let inst = engine::Instance::open(engine::EngineConfig::file(&path)).unwrap();
                let conn = inst.connect().unwrap();
                conn.execute_batch(&format!(
                    "CREATE MACRO crm.hostile(p) AS TABLE SELECT * FROM enable_logging(\
                     level := 'debug', storage := 'file', storage_path := p, \
                     storage_buffer_size := {size}, \
                     storage_config := {{'only_flush_on_full_buffer': true}})"
                ))
                .unwrap();
                inst.release(conn);
            }
            let s = open(&file_profile(&path, json!({})), None, false).await;
            // A table read, so the transaction reaches the file (`SELECT 1` alone does not).
            run(&s, "BEGIN").await.unwrap();
            run(&s, "SELECT * FROM customers").await.unwrap();
            let logs = blocker.join("logs");
            let _ = s
                .query(
                    &format!("SELECT * FROM crm.hostile({})", sql_path(&logs)),
                    &[],
                )
                .await;
            for _ in 0..extra {
                let _ = run(&s, "SELECT 1").await;
            }
            // Reaching the next line is the test: an abort ends the test process.
            s.close().await;
            drop(dir);
        }
    }
}

/// A macro of the file switches the logger to file storage that cannot be written, buffered
/// until N entries wait: DuckDB logs a file's close from its handle's destructor, so the next
/// CSV read or table write would fill the buffer there and abort the process (any N, sooner or
/// later; with every log type, the statements' own entries do too). The job that switched it
/// stops the instance: every later statement is refused, on both connections, in a new session
/// of the file and in an unattended (MCP) session, and nothing closes.
#[tokio::test]
async fn a_logger_switched_from_the_file_stops_the_instance_before_it_can_abort() {
    let cases: [(&str, &[u32]); 2] = [("['FileSystem'], ", &[1, 2, 8, 50, 1000]), ("", &[1, 2, 8])];
    for (types, sizes) in cases {
        for &size in sizes {
            for unattended in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("hostile.duckdb");
                build_sample(&path);
                let blocker = dir.path().join("blocker");
                std::fs::write(&blocker, "x").unwrap();
                let csv = dir.path().join("x.csv");
                std::fs::write(&csv, "a,b\n1,2\n3,4\n").unwrap();
                {
                    let inst = engine::Instance::open(engine::EngineConfig::file(&path)).unwrap();
                    let conn = inst.connect().unwrap();
                    conn.execute_batch(&format!(
                        "CREATE MACRO crm.hostile(p) AS TABLE SELECT * FROM enable_logging(\
                         {types}level := 'trace', storage := 'file', storage_path := p, \
                         storage_buffer_size := {size}, \
                         storage_config := {{'only_flush_on_full_buffer': true}})"
                    ))
                    .unwrap();
                    inst.release(conn);
                }
                let s = open(&file_profile(&path, json!({})), None, unattended).await;
                let case = format!("types {types:?}, size {size}, unattended {unattended}");
                let logs = blocker.join("logs");
                let _ = s
                    .query(
                        &format!("SELECT * FROM crm.hostile({})", sql_path(&logs)),
                        &[],
                    )
                    .await;
                let read = format!("SELECT count(*) FROM read_csv({})", sql_path(&csv));
                for _ in 0..3 {
                    for sql in [
                        read.as_str(),
                        "CREATE TABLE w0 AS SELECT range i FROM range(1000)",
                    ] {
                        let e = s.query(sql, &[]).await.unwrap_err();
                        assert!(
                            e.to_string().contains(engine::LOGGER_SWITCHED),
                            "{case}: {sql}: {e:?}"
                        );
                        let e = run(&s, sql).await.unwrap_err();
                        assert!(
                            e.to_string().contains(engine::LOGGER_SWITCHED),
                            "{case}: {sql}: {e:?}"
                        );
                    }
                }
                let e = try_open(&file_profile(&path, json!({})), None, unattended)
                    .await
                    .err()
                    .unwrap_or_else(|| panic!("{case}: a second session opened"));
                assert!(
                    e.to_string().contains(engine::LOGGER_SWITCHED),
                    "{case}: {e:?}"
                );
                let inst = engine::open_instance(&path).expect("the open instance");
                assert!(inst.is_broken(), "{case}");
                drop(inst);
                // Reaching the next line is the test: an abort ends the test process.
                s.close().await;
                assert_refused_until_restart(&path).await;
                drop(dir);
            }
        }
    }
}

/// The startup SQL can call such a macro too: the open fails there, before any session runs,
/// and the instance's root connection is leaked rather than closed.
#[tokio::test]
async fn a_logger_switched_by_the_startup_sql_fails_the_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hostile.duckdb");
    build_sample(&path);
    let logs = dir.path().join("blocker").join("logs");
    std::fs::write(dir.path().join("blocker"), "x").unwrap();
    {
        let inst = engine::Instance::open(engine::EngineConfig::file(&path)).unwrap();
        let conn = inst.connect().unwrap();
        conn.execute_batch(
            "CREATE MACRO crm.hostile(p) AS TABLE SELECT * FROM enable_logging(['FileSystem'], \
             level := 'trace', storage := 'file', storage_path := p, storage_buffer_size := 1, \
             storage_config := {'only_flush_on_full_buffer': true})",
        )
        .unwrap();
        inst.release(conn);
    }
    let startup = format!(
        "SELECT * FROM crm.hostile({}); CREATE TABLE w0 AS SELECT range i FROM range(1000)",
        sql_path(&logs)
    );
    let p = file_profile(&path, json!({ "startupSql": startup }));
    let e = try_open(&p, None, false)
        .await
        .err()
        .expect("the open fails");
    assert!(e.to_string().contains(engine::LOGGER_SWITCHED), "{e:?}");
    assert!(engine::open_instance(&path).is_none());
    assert_refused_until_restart(&path).await;
    drop(dir);
}

/// A view can switch the logger in a materialized CTE and then fail the editor's open
/// transaction with `error()`: the editor connection cannot answer the check then, so the root
/// answers for it, and the worker's next write or CSV read (which would flush the log and abort
/// the process) is refused, as is everything after. The file is refused before the session
/// closes.
#[tokio::test]
async fn a_logger_switched_in_a_failed_transaction_stops_the_instance() {
    for unattended in [false, true] {
        for next in ["write", "csv"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("hostile.duckdb");
            build_sample(&path);
            let blocker = dir.path().join("blocker");
            std::fs::write(&blocker, "x").unwrap();
            let csv = dir.path().join("x.csv");
            std::fs::write(&csv, "a,b\n1,2\n").unwrap();
            {
                let inst = engine::Instance::open(engine::EngineConfig::file(&path)).unwrap();
                let conn = inst.connect().unwrap();
                conn.execute_batch(&format!(
                    "CREATE VIEW crm.v AS WITH e AS MATERIALIZED (SELECT count(*) AS n FROM \
                     enable_logging(['FileSystem'], level := 'trace', storage := 'file', \
                     storage_path := {}, storage_buffer_size := 1, \
                     storage_config := {{'only_flush_on_full_buffer': true}})) \
                     SELECT error('boom') AS x FROM e",
                    sql_path(&blocker.join("logs"))
                ))
                .unwrap();
                inst.release(conn);
            }
            let s = open(&file_profile(&path, json!({})), None, unattended).await;
            let case = format!("unattended {unattended}, then {next}");
            run(&s, "BEGIN TRANSACTION").await.unwrap();
            let e = run(&s, "SELECT * FROM crm.v").await.unwrap_err();
            assert!(message(&e).contains("boom"), "{case}: {e:?}");
            let inst = engine::open_instance(&path).expect("the open instance");
            assert!(inst.is_broken(), "{case}");
            drop(inst);
            // Reaching the next lines is the test: an abort ends the test process.
            let e = if next == "write" {
                s.execute("CREATE TABLE crm.w0 AS SELECT 1", &[])
                    .await
                    .map(|_| ())
                    .unwrap_err()
            } else {
                let read = format!("SELECT count(*) FROM read_csv({})", sql_path(&csv));
                s.query(&read, &[]).await.map(|_| ()).unwrap_err()
            };
            assert!(
                e.to_string().contains(engine::LOGGER_SWITCHED),
                "{case}: {e:?}"
            );
            let e = run(&s, "SELECT 1").await.unwrap_err();
            assert!(
                e.to_string().contains(engine::LOGGER_SWITCHED),
                "{case}: {e:?}"
            );
            // Refused while the session is still open, not only after its last close.
            let e = engine::refuse_broken_file(&path).unwrap_err();
            assert!(e.to_string().contains(engine::LOGGER_SWITCHED), "{case}");
            s.close().await;
            assert_refused_until_restart(&path).await;
            drop(dir);
        }
    }
}

/// A file's BIGNUM of 60,000 digits reads at once as a note of its size (converting it would
/// hold the thread past cancel and the timeout); one of 9,000 digits reads as DuckDB prints it.
#[tokio::test]
async fn a_huge_bignum_reads_within_a_time_budget() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.duckdb");
    build_sample(&path);
    let s = open(&file_profile(&path, json!({})), None, false).await;
    run(
        &s,
        "CREATE TABLE crm.big AS SELECT 1 AS id, ('9' || repeat('7', 60000))::BIGNUM AS b \
         UNION ALL SELECT 2, ('-8' || repeat('3', 9000))::BIGNUM",
    )
    .await
    .unwrap();
    let started = std::time::Instant::now();
    let got = rows(&s, "SELECT b, b::VARCHAR FROM crm.big ORDER BY id").await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    let [huge, fits] = got.as_slice() else {
        panic!("two rows")
    };
    assert!(
        matches!(huge.first(), Some(Value::Other(_))),
        "{:?}",
        huge.first()
            .map(|v| format!("{v:?}").chars().take(80).collect::<String>())
    );
    let Some(Value::Text(printed)) = fits.get(1) else {
        panic!("DuckDB's text")
    };
    assert_eq!(fits.first(), Some(&Value::BigInt(printed.clone())));
    s.close().await;
}

/// Writes macros named like the builtins the driver's own checks call into `path`'s main
/// schema (where an unqualified name resolves first), and a view that switches the logger to
/// file storage at `logs`.
fn store_shadowing_macros(path: &Path, current_setting: &str, logs: &Path) {
    let inst = engine::Instance::open(engine::EngineConfig::file(path)).unwrap();
    let conn = inst.connect().unwrap();
    conn.execute_batch(&format!(
        "CREATE MACRO current_setting(x) AS {current_setting}; \
         CREATE MACRO len(x) AS 0; \
         CREATE MACRO duckdb_databases() AS TABLE SELECT NULL AS path, true AS readonly; \
         CREATE VIEW crm.v AS SELECT * FROM enable_logging(['FileSystem'], level := 'trace', \
         storage := 'file', storage_path := {}, storage_buffer_size := 1, \
         storage_config := {{'only_flush_on_full_buffer': true}})",
        sql_path(logs)
    ))
    .unwrap();
    // The shadowing is real: an unqualified call answers the macro.
    let shadowed: String = conn
        .query_row("SELECT current_setting('logging_storage')", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(shadowed, "memory");
    inst.release(conn);
}

/// A file that stores macros named `current_setting`, `len` and `duckdb_databases` cannot make
/// the logger check answer `memory`: the view that switches the logger breaks the instance,
/// and the CSV read that would have aborted the process on the first log flush is refused.
#[tokio::test]
async fn builtins_shadowed_by_the_file_do_not_fool_the_logger_check() {
    for unattended in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hostile.duckdb");
        build_sample(&path);
        std::fs::write(dir.path().join("blocker"), "x").unwrap();
        let csv = dir.path().join("x.csv");
        std::fs::write(&csv, "a,b\n1,2\n").unwrap();
        store_shadowing_macros(&path, "'memory'", &dir.path().join("blocker").join("logs"));
        let s = open(&file_profile(&path, json!({})), None, unattended).await;
        let _ = s.query("SELECT * FROM crm.v", &[]).await;
        let inst = engine::open_instance(&path).expect("the open instance");
        assert!(inst.is_broken(), "unattended {unattended}");
        drop(inst);
        let read = format!("SELECT count(*) FROM read_csv({})", sql_path(&csv));
        let e = s.query(&read, &[]).await.unwrap_err();
        assert!(e.to_string().contains(engine::LOGGER_SWITCHED), "{e:?}");
        // Reaching the next line is the test: an abort ends the test process.
        s.close().await;
        assert_refused_until_restart(&path).await;
        drop(dir);
    }
}

/// The same macros cannot keep the local file system on for a read-only profile: the check
/// that turns it off reads the builtins, so the view's file logging is refused.
#[tokio::test]
async fn builtins_shadowed_by_the_file_do_not_keep_local_files_on() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hostile.duckdb");
    build_sample(&path);
    let logs = dir.path().join("logs");
    store_shadowing_macros(
        &path,
        "(CASE WHEN x = 'logging_storage' THEN 'memory' ELSE 'x' END)",
        &logs,
    );
    let s = open(
        &file_profile(&path, json!({ "readOnly": true })),
        None,
        false,
    )
    .await;
    // (`disabled_filesystems` reads '' either way: `local_files_off` and the refusal show it.)
    let inst = engine::open_instance(&path).expect("the open instance");
    assert!(inst.local_files_off() && inst.is_read_only());
    assert_eq!(
        s.capabilities().safety.read_only,
        ReadOnlyMode::Server,
        "the caps"
    );
    let e = s.query("SELECT * FROM crm.v", &[]).await.unwrap_err();
    assert!(e.to_string().contains("LocalFileSystem"), "{e:?}");
    assert!(!inst.is_broken());
    assert!(!logs.exists());
    drop(inst);
    s.close().await;
    drop(dir);
}

/// A failed transaction answers nothing but `ROLLBACK`, the logger check included: that is no
/// reason to stop the instance.
#[tokio::test]
async fn a_failed_transaction_does_not_stop_the_instance() {
    let (dir, path) = sample();
    let s = open(&file_profile(&path, json!({})), None, false).await;
    run(&s, "CREATE TEMP TABLE k (a INTEGER PRIMARY KEY)")
        .await
        .unwrap();
    run(&s, "BEGIN").await.unwrap();
    run(&s, "INSERT INTO k VALUES (1)").await.unwrap();
    // A constraint violation fails the transaction (a binder error would not).
    assert!(run(&s, "INSERT INTO k VALUES (1)").await.is_err());
    let e = run(&s, "SELECT 1").await.unwrap_err();
    assert!(e.to_string().contains("transaction is aborted"), "{e:?}");
    // The worker's guard asks the root for the instance's logger meanwhile: memory.
    assert_eq!(rows(&s, "SELECT 2").await, vec![vec![Value::Int(2)]]);
    run(&s, "ROLLBACK").await.unwrap();
    run(&s, "SELECT 1").await.unwrap();
    assert_eq!(rows(&s, "SELECT 1").await, vec![vec![Value::Int(1)]]);
    let inst = engine::open_instance(&path).expect("the open instance");
    assert!(!inst.is_broken());
    drop(inst);
    s.close().await;
    assert!(engine::open_instance(&path).is_none());
    drop(dir);
}

#[test]
fn only_a_trailing_limit_1_in_code_loads_blobs_whole() {
    use super::session::reads_one_row;
    for sql in [
        "SELECT \"b\" FROM \"t\" WHERE \"id\" = $1 LIMIT 1",
        "SELECT b FROM t limit 1 ",
        "SELECT b FROM t LIMIT 1 -- note",
    ] {
        assert!(reads_one_row(sql), "{sql}");
    }
    for sql in [
        "SELECT b FROM t -- LIMIT 1",
        "SELECT b FROM t /* LIMIT 1 */",
        "SELECT b FROM t LIMIT 10",
        "SELECT 'LIMIT 1'",
        "LIMIT",
    ] {
        assert!(!reads_one_row(sql), "{sql}");
    }
}

/// The caps of a session (a read-write file, a Command Line key, a read-only profile and an
/// in-memory database), kept in `tests/fixtures/duckdb/caps.json`: the renderer's mock reads
/// the same file, so its caps cannot drift from these. `TABLER_BLESS=1` rewrites it.
#[tokio::test]
async fn caps_match_the_recorded_fixture() {
    let (dir, path) = sample();
    let mut all = serde_json::Map::new();
    for (name, p, password) in [
        ("readWrite", file_profile(&path, json!({})), None),
        (
            "command",
            file_profile(
                &path,
                json!({ "passwordMode": "command", "passwordCommand": "printf ''" }),
            ),
            Some(""),
        ),
        (
            "readOnly",
            file_profile(&path, json!({ "readOnly": true })),
            None,
        ),
        ("memory", profile(json!({ "sqlitePath": ":memory:" })), None),
    ] {
        let s = open(&p, password, false).await;
        all.insert(
            name.into(),
            serde_json::to_value(s.capabilities()).expect("caps"),
        );
        s.close().await;
    }
    drop(dir);
    let got = serde_json::Value::Object(all);
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/duckdb/caps.json");
    if std::env::var("TABLER_BLESS").as_deref() == Ok("1") {
        let text = serde_json::to_string_pretty(&got).expect("json") + "\n";
        std::fs::write(&fixture, text).expect("write the fixture");
        return;
    }
    let want: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&fixture).expect("the caps fixture (TABLER_BLESS=1 writes it)"),
    )
    .expect("fixture json");
    assert_eq!(
        got, want,
        "caps changed: rerun with TABLER_BLESS=1 and review the diff"
    );
}
