//! A connection to a store that speaks no SQL holds a native session: the commands that build SQL
//! are refused at `Connection::session` before anything reaches the store, even when the store's
//! caps grant their feature, and what every session does goes through `any_session`: the
//! sidebar's introspection, the grid (cursor pages, counts, cell fetches), staged edits with an
//! outcome per change, the console in place of the SQL editor, and the driver's own calls.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value as Json, json};
use tauri::ipc::Channel;
use tokio::sync::{RwLock, watch};
use uuid::Uuid;

use crate::appdb::AppDb;
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::commands::{
    backup, browse, conn_extras, connections, console, ddl, export, import, metrics, mutate,
    native, objects, query, schema, workspace_commit,
};
use crate::drivers::native::NativeSession;
use crate::drivers::{CancelSlot, DbSession, Session, registry};
use crate::error::{AppError, QUERY_TIMEOUT_CODE};
use crate::secrets::{Backend, Secrets};
use crate::sqlgen::Dialect;
use crate::state::{AppState, Connection};
use crate::testkit::{FakeSession, FakeStore};
use crate::types::browse::{BrowseRequest, RowKey};
use crate::types::caps::{Feature, SessionCaps, TxMode};
use crate::types::history::HistoryFilter;
use crate::types::mutate::ApplyRequest;
use crate::types::profile::{
    ConnStatus, ConnectionInfo, ConnectionProfile, Driver, ProfileInput, ProfileOptions, SslMode,
};
use crate::types::query::{QueryEvent, RunOptions, StatementResult};
use crate::types::schema::TableRef;
use crate::types::value::Value;

struct Fixture {
    state: Arc<AppState>,
    conn: Arc<Connection>,
    store: Arc<FakeStore>,
    dir: tempfile::TempDir,
}

fn redis_profile() -> ConnectionProfile {
    ConnectionProfile {
        id: Uuid::new_v4(),
        name: "cache".into(),
        color: None,
        group: None,
        driver: Driver::Redis,
        host: Some("127.0.0.1".into()),
        port: Some(1),
        user: None,
        database: Some("0".into()),
        ssl: SslMode::Disable,
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions::default(),
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    }
}

/// An open connection of a Redis profile whose session is a `FakeStore` with `caps`.
async fn fixture(caps: SessionCaps) -> Fixture {
    fixture_with(caps, false).await
}

/// `fixture` of a read-only profile (`read_only`) or a writable one.
async fn fixture_with(caps: SessionCaps, read_only: bool) -> Fixture {
    let mut profile = redis_profile();
    profile.options.read_only = read_only;
    let store = Arc::new(FakeStore::new(caps));
    let (state, conn) = open_with(profile, Session::Native(store.clone())).await;
    Fixture {
        state,
        conn,
        store,
        dir: tempfile::tempdir().unwrap(),
    }
}

/// A new app state with one open connection of `profile` holding `session`.
async fn open_with(
    profile: ConnectionProfile,
    session: Session,
) -> (Arc<AppState>, Arc<Connection>) {
    let appdb = AppDb::open_in_memory().unwrap();
    let profile = appdb.upsert_profile(&profile).unwrap();
    let state = Arc::new(AppState::new(
        appdb.clone(),
        Secrets::new(Backend::File, appdb),
    ));
    let id = Uuid::new_v4();
    let conn = Arc::new(Connection {
        id,
        profile: profile.clone(),
        info: RwLock::new(ConnectionInfo {
            connection_id: id,
            profile_id: profile.id,
            name: profile.name.clone(),
            color: None,
            driver: profile.driver,
            server_version: "fake 1.0".into(),
            current_database: Some("0".into()),
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

fn from<T: DeserializeOwned>(value: Json) -> T {
    serde_json::from_value(value).unwrap()
}

fn sink<T>() -> Channel<T> {
    Channel::new(|_| Ok(()))
}

fn users() -> TableRef {
    TableRef::new(None, "users")
}

const NOT_SQL: &str = "unsupported: Redis is not a SQL database: it has no SQL session";

/// What a store's caps may say: its own minimal ones, or every feature (a driver that grants
/// too much must still not get a SQL-only command through).
fn caps_variants() -> [SessionCaps; 2] {
    let minimal = SessionCaps::minimal(Driver::Redis);
    [minimal.clone(), minimal.with(&Feature::ALL)]
}

#[tokio::test]
async fn the_sql_view_of_a_native_connection_is_refused() {
    let f = fixture(SessionCaps::minimal(Driver::Redis)).await;
    let id = f.conn.id;
    assert_eq!(f.conn.session().await.err().unwrap().to_string(), NOT_SQL);
    assert_eq!(
        f.state.session(id).await.err().unwrap().to_string(),
        NOT_SQL
    );
    // A feature the store has: refused as SQL; one it lacks: refused for the feature first.
    let err = f.conn.session_for(Feature::StructureView).await.err();
    assert_eq!(err.unwrap().to_string(), NOT_SQL);
    let err = f.state.session_for(id, Feature::EditRows).await.err();
    let text = err.unwrap().to_string();
    assert!(text.ends_with("is not available"), "{text}");
    assert_eq!(f.store.calls(), Vec::<String>::new());

    // What every session does reaches the store.
    assert_eq!(f.conn.caps().await, SessionCaps::minimal(Driver::Redis));
    f.conn.any_session().await.ping().await.unwrap();
    let meta = browse::cached_meta(&f.conn, &users()).await.unwrap();
    assert_eq!(meta.primary_key, ["_id"]);
    // Cached: read once.
    browse::cached_meta(&f.conn, &users()).await.unwrap();
    assert_eq!(f.store.calls(), ["ping", "table_meta users"]);
}

/// Each SQL-only command (structure changes, object operations, workspace commits, exports,
/// imports, backups, the connection encoding, Metrics) on a native connection: refused, with
/// nothing sent to the store.
#[tokio::test]
async fn sql_only_commands_refuse_a_native_connection_before_sending_anything() {
    for caps in caps_variants() {
        let f = fixture(caps.clone()).await;
        let (state, conn) = (f.state.as_ref(), f.conn.as_ref());
        let (id, t) = (conn.id, users());
        let qid = |what: &str| format!("native-{what}-{}", caps.features.len());
        let csv = json!({ "format": "csv", "delimiter": ",", "header": true, "nullAs": "" });
        let calls: Vec<(&str, Result<(), AppError>)> =
            vec![
            (
                "ddl",
                ddl::plan_for(
                    conn,
                    Some(&t),
                    &[from(json!({ "op": "rawSql", "sql": "ALTER TABLE t ADD c int" }))],
                )
                .await
                .map(drop),
            ),
            (
                "object",
                objects::apply_object_op_on(
                    conn,
                    &from(json!({
                        "op": "dropTables", "tables": [{ "table": t, "kind": "table" }],
                        "cascade": false, "ignoreForeignKeys": false,
                    })),
                )
                .await
                .map(drop),
            ),
            (
                "preview workspace",
                workspace_commit::preview_workspace_on(conn, &[])
                    .await
                    .map(drop),
            ),
            (
                "apply workspace",
                workspace_commit::apply_workspace_on(conn, &[])
                    .await
                    .map(drop),
            ),
            (
                "export tables list",
                export::export_database_tables_on(state, id, None)
                    .await
                    .map(drop),
            ),
            (
                "export tables",
                export::export_tables_on(
                    state,
                    id,
                    from(json!({
                        "exportId": qid("tables"), "tables": [t], "format": csv,
                        "layout": "filePerTable", "target": f.dir.path().join("out"),
                    })),
                    sink(),
                )
                .await
                .map(drop),
            ),
            (
                "export data",
                export::export_data_on(
                    state,
                    id,
                    from(json!({
                        "exportId": qid("data"), "source": { "kind": "table", "table": t,
                        "filter": null, "columns": null }, "format": csv,
                        "path": f.dir.path().join("out.csv"), "maxRows": null,
                    })),
                    sink(),
                )
                .await
                .map(drop),
            ),
            (
                "import csv",
                import::import_csv_on(
                    state,
                    id,
                    from(json!({
                        "importId": qid("csv"), "path": "in.csv",
                        "format": { "delimiter": null, "quote": "\"", "encoding": null,
                        "header": true }, "target": { "kind": "existing", "table": t },
                        "mapping": [], "emptyAsNull": false, "mode": "stopOnError",
                    })),
                    sink(),
                )
                .await
                .map(drop),
            ),
            (
                "import json",
                import::import_json_on(
                    state,
                    id,
                    from(json!({
                        "importId": qid("json"), "path": "in.json", "encoding": null,
                        "keys": [], "target": { "kind": "existing", "table": t },
                        "mapping": [], "mode": "stopOnError",
                    })),
                    sink(),
                )
                .await
                .map(drop),
            ),
            (
                "preview sql dump",
                import::preview_sql_dump_on(state, id, from(json!({ "path": "in.sql" })))
                    .await
                    .map(drop),
            ),
            (
                "import sql dump",
                import::import_sql_dump_on(
                    state,
                    id,
                    from(json!({ "importId": qid("dump"), "path": "in.sql",
                        "stopOnError": true })),
                    sink(),
                )
                .await
                .map(drop),
            ),
            (
                "backup",
                backup::backup_inspect_target_on(state, conn.profile.id)
                    .await
                    .map(drop),
            ),
            (
                "get encoding",
                conn_extras::connection_encoding_on(state, id)
                    .await
                    .map(drop),
            ),
            (
                "set encoding",
                conn_extras::set_connection_encoding_on(state, id, Some("LATIN1".into()))
                    .await
                    .map(drop),
            ),
            ("table ddl", ddl::table_ddl_on(conn, &t).await.map(drop)),
            (
                "metrics",
                metrics::metrics_run_on(
                    state,
                    id,
                    qid("metrics"),
                    from(json!({
                        "parts": ["SELECT 1"], "values": [], "allowWrites": false, "maxRows": 10,
                    })),
                )
                .await
                .map(drop),
            ),
        ];
        let all = caps.has(Feature::Metrics);
        for (what, result) in calls {
            let err = result
                .err()
                .unwrap_or_else(|| panic!("{what}: not refused"));
            // Refused for the missing feature, or (with every feature) as a store without SQL.
            // Either way the store has no connection encoding to set, and only the core engines
            // have dump tools.
            match (what, all) {
                ("set encoding", true) => assert_eq!(err.code(), "INVALID_INPUT", "{what}: {err}"),
                ("backup", _) => assert_eq!(err.code(), "UNSUPPORTED", "{what}: {err}"),
                (_, true) => assert_eq!(err.to_string(), NOT_SQL, "{what}"),
                (_, false) => assert_eq!(err.code(), "UNSUPPORTED", "{what}: {err}"),
            }
        }
        assert_eq!(f.store.calls(), Vec::<String>::new(), "{caps:?}");
        assert!(!f.store.is_closed());
    }
}

/// A database switch that cannot open the new session leaves the store's session in place.
#[tokio::test]
async fn a_failed_switch_keeps_the_native_session() {
    for caps in caps_variants() {
        let f = fixture(caps).await;
        let err = connections::switch_database_on(&f.state, f.conn.id, "1".into())
            .await
            .unwrap_err();
        assert_eq!(err.code(), "UNSUPPORTED", "{err}");
        assert!(matches!(f.conn.any_session().await, Session::Native(_)));
        assert!(!f.store.is_closed());
        assert_eq!(f.store.calls(), Vec::<String>::new());
    }
}

/// Quitting closes native sessions too.
#[tokio::test]
async fn closing_all_sessions_closes_a_native_store() {
    let f = fixture(SessionCaps::minimal(Driver::Redis)).await;
    f.state.close_all_sessions().await;
    assert!(f.store.is_closed());
    assert!(f.state.connections.read().await.is_empty());
}

/// What a store's driver would grant: the console, edits, its definition and exact counts.
fn store_caps() -> SessionCaps {
    SessionCaps::minimal(Driver::Redis).with(&[
        Feature::Editor,
        Feature::EditRows,
        Feature::InsertRows,
        Feature::DeleteRows,
        Feature::ViewDefinition,
        Feature::ExactCount,
    ])
}

fn not_available(err: AppError) {
    let text = err.to_string();
    assert!(text.ends_with("is not available"), "{text}");
}

/// The events of a run by name (`started`, `rows`…).
fn event_names(events: &[QueryEvent]) -> Vec<String> {
    events
        .iter()
        .map(|e| {
            serde_json::to_value(e).unwrap()["event"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

/// A console run straight on the store, with its events.
async fn run_console(
    store: &FakeStore,
    text: &str,
    options: RunOptions,
) -> (Vec<StatementResult>, Vec<QueryEvent>) {
    let mut events = Vec::new();
    let results = {
        let mut emit = |e: QueryEvent| events.push(e);
        console::run_console_inner(store, text, &options, &CancelSlot::default(), &mut emit).await
    };
    (results, events)
}

/// Three staged changes of `users`: an update, a delete of a document that is not there, and an
/// insert.
fn changes() -> ApplyRequest {
    let int = |n: i64| json!(Value::Int(n));
    let text = |s: &str| json!(Value::Text(s.into()));
    from(json!({
        "table": users(),
        "changes": [
            { "op": "update", "rowId": "a", "key": { "kind": "pk", "value": { "_id": int(1) } },
              "changes": { "name": text("ada lovelace") } },
            { "op": "delete", "rowId": "b", "key": { "kind": "pk", "value": { "_id": int(9) } } },
            { "op": "insert", "rowId": "c", "values": { "_id": int(3), "name": text("cy") } },
        ],
    }))
}

#[tokio::test]
async fn the_sidebar_and_structure_read_a_native_store() {
    let f = fixture(store_caps()).await;
    let session = f.state.any_session(f.conn.id).await.unwrap();
    assert_eq!(session.list_databases().await.unwrap()[0].name, "0");
    let names: Vec<String> = session
        .list_tables("")
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["orders", "users"]);
    let meta = schema::table_on(&f.conn, users(), false).await.unwrap();
    assert_eq!(meta.primary_key, ["_id"]);
    // Cached for the grid: read once.
    schema::table_on(&f.conn, users(), false).await.unwrap();
    let definition = schema::view_definition_on(&f.conn, &users()).await.unwrap();
    assert_eq!(definition.as_deref(), Some("{ \"collection\": \"users\" }"));
    assert_eq!(
        f.store.calls(),
        [
            "list_databases",
            "list_tables ",
            "table_meta users",
            "definition users"
        ]
    );

    // A store that does not offer its definition is refused before it hears of it.
    let g = fixture(SessionCaps::minimal(Driver::Redis)).await;
    not_available(
        schema::view_definition_on(&g.conn, &users())
            .await
            .unwrap_err(),
    );
    assert_eq!(g.store.calls(), Vec::<String>::new());
}

/// The grid pages a store by cursor (two pages of one row, then no next cursor), counts it and
/// fetches a cell through its `TableSource`, under the query time out too.
#[tokio::test]
async fn the_grid_pages_counts_and_fetches_through_the_store() {
    let f = fixture(store_caps()).await;
    let page = |cursor: Option<&str>| -> BrowseRequest {
        from(json!({
            "table": users(), "columns": null, "filter": null, "limit": 1, "offset": 0,
            "cursor": cursor,
        }))
    };
    let limit = Some(Duration::from_secs(30));
    let first = browse::browse_rows(&f.conn, &page(None), limit)
        .await
        .unwrap();
    assert_eq!(first.rows, [[Value::Int(1), Value::Text("ada".into())]]);
    assert_eq!(first.next_cursor.as_deref(), Some("1"));
    let second = browse::browse_rows(&f.conn, &page(first.next_cursor.as_deref()), None)
        .await
        .unwrap();
    assert_eq!(second.rows, [[Value::Int(2), Value::Text("bob".into())]]);
    assert_eq!(second.next_cursor, None, "the last page");

    let count = browse::count_rows(&f.conn, &users(), None, true, 0, limit)
        .await
        .unwrap();
    assert_eq!((count.count, count.exact), (2, true));
    let key = RowKey::Pk(BTreeMap::from([("_id".to_string(), Value::Int(2))]));
    let cell = browse::fetch_cell_on(&f.conn, &users(), &key, "name", limit)
        .await
        .unwrap();
    assert_eq!(cell, Value::Text("bob".into()));
    assert_eq!(
        f.store.calls(),
        [
            "table_meta users",
            "browse users",
            "browse users",
            "count users",
            "fetch users name"
        ]
    );

    // An exact count is a feature of its own: refused before anything is read.
    let g = fixture(SessionCaps::minimal(Driver::Redis)).await;
    not_available(
        browse::count_rows(&g.conn, &users(), None, true, 0, None)
            .await
            .unwrap_err(),
    );
    assert_eq!(g.store.calls(), Vec::<String>::new());
}

/// The preview is the store's plan; applying it reports each change, so after one fails the
/// others are known to be applied (and the renderer unstages them).
#[tokio::test]
async fn staged_changes_preview_and_apply_with_an_outcome_per_change() {
    let f = fixture(store_caps()).await;
    let (state, id) = (f.state.as_ref(), f.conn.id);
    let preview = mutate::preview_changes_on(state, id, &changes())
        .await
        .unwrap();
    assert_eq!(
        preview,
        ["update users a", "delete users b", "insert users c"]
    );
    let result = mutate::apply_changes_on(state, id, changes())
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.sql, preview);
    let outcome: Vec<(&str, bool)> = result
        .results
        .iter()
        .map(|r| (r.row_id.as_str(), r.error.is_some()))
        .collect();
    assert_eq!(outcome, [("a", false), ("b", true), ("c", false)]);
    assert_eq!(result.error.as_ref().unwrap().code, "NOT_FOUND");
    let names: Vec<Value> = f
        .store
        .docs("users")
        .into_iter()
        .map(|d| d["name"].clone())
        .collect();
    assert_eq!(
        names,
        [
            Value::Text("ada lovelace".into()),
            Value::Text("bob".into()),
            Value::Text("cy".into())
        ]
    );
    assert_eq!(
        f.store.calls(),
        [
            "table_meta users",
            "MULTI",
            "update a",
            "delete b",
            "insert c",
            "EXEC"
        ]
    );

    // A read-only connection, and a store that offers no edits: refused, nothing sent.
    let ro = fixture_with(store_caps(), true).await;
    let none = fixture(SessionCaps::minimal(Driver::Redis)).await;
    for g in [&ro, &none] {
        let (state, id) = (g.state.as_ref(), g.conn.id);
        let preview = mutate::preview_changes_on(state, id, &changes()).await;
        let applied = mutate::apply_changes_on(state, id, changes()).await;
        for err in [preview.unwrap_err(), applied.unwrap_err()] {
            assert_eq!(err.code(), "UNSUPPORTED", "{err}");
        }
        assert_eq!(g.store.calls(), Vec::<String>::new());
    }
}

/// A SQL session that applies changes one by one (no transaction) reports which were applied
/// when one fails; an atomic one keeps reporting only the failure (nothing was applied).
#[tokio::test]
async fn a_sql_session_without_transactions_reports_what_it_applied() {
    let store = FakeStore::new(store_caps());
    let meta = store.table_meta(&users()).await.unwrap();
    let mut profile = redis_profile();
    profile.driver = Driver::Postgres;
    for (mode, after) in [(TxMode::Sequential, true), (TxMode::Atomic, false)] {
        let mut caps = SessionCaps::core(Driver::Postgres, Dialect::Postgres, None);
        caps.data.transactions = mode;
        // Run in order delete, update, insert: the update fails.
        let fake = Arc::new(FakeSession::new(caps).failing("UPDATE"));
        let (state, conn) = open_with(profile.clone(), Session::Sql(fake.clone())).await;
        conn.schema_cache
            .write()
            .await
            .insert(users(), Arc::new(meta.clone()));
        let result = mutate::apply_changes_on(&state, conn.id, changes())
            .await
            .unwrap();
        assert!(!result.success);
        let errors: Vec<(&str, Option<&str>)> = result
            .results
            .iter()
            .map(|r| (r.row_id.as_str(), r.error.as_ref().map(|e| e.code.as_str())))
            .collect();
        let rest = after.then_some("INVALID_INPUT");
        assert_eq!(
            errors,
            [("b", None), ("a", Some("DB_ERROR")), ("c", rest)],
            "{mode:?}"
        );
        if after {
            let message = &result.results[2].error.as_ref().unwrap().message;
            assert_eq!(message, "not applied: an earlier change failed");
        }
        assert_eq!(fake.calls().len(), 2, "the insert was never sent");
    }
}

/// The console streams its commands with the SQL editor's events, stops at the first error, and
/// through `run_sql` records the history and drops cached structure after a write.
#[tokio::test]
async fn the_console_runs_like_the_sql_editor() {
    let f = fixture(store_caps()).await;
    let text = "find users\n\ncount orders\nnope users\nfind orders";
    let (results, events) = run_console(&f.store, text, RunOptions::default()).await;
    assert_eq!(
        event_names(&events),
        [
            "started", "columns", "rows", "finished", "started", "columns", "rows", "finished",
            "started", "failed", "done"
        ]
    );
    assert_eq!(results.len(), 3);
    assert_eq!((results[0].row_count, results[0].span.line), (2, 1));
    assert_eq!(results[1].span.line, 3);
    let error = results[2].error.as_ref().expect("an unknown command fails");
    assert_eq!(
        (error.code.as_str(), error.statement_index),
        ("INVALID_INPUT", Some(2))
    );
    // Without stop_on_error the rest still runs.
    let options = RunOptions {
        stop_on_error: false,
        ..RunOptions::default()
    };
    let (results, _) = run_console(&f.store, text, options).await;
    assert_eq!(results.len(), 4);
    assert!(results[3].error.is_none());

    browse::cached_meta(&f.conn, &users()).await.unwrap();
    let (state, id) = (f.state.as_ref(), f.conn.id);
    let text = "find users\ndel users 2".to_string();
    let results = query::run_sql_on(state, id, "q".into(), text, None, sink())
        .await
        .unwrap();
    assert!(results.iter().all(|r| r.error.is_none()));
    assert_eq!(results[1].affected, Some(1));
    assert_eq!(f.store.docs("users").len(), 1);
    assert!(
        f.conn.schema_cache.read().await.is_empty(),
        "a write may change collections"
    );
    let history = state.appdb.list_history(&HistoryFilter::default()).unwrap();
    let rows: Vec<(&str, &str, Option<&str>)> = history
        .iter()
        .map(|h| (h.sql.as_str(), h.source.as_str(), h.database.as_deref()))
        .collect();
    assert_eq!(
        rows,
        [
            ("del users 2", "editor", Some("0")),
            ("find users", "editor", Some("0"))
        ]
    );
}

/// A console needs the editor and has no EXPLAIN: both refused before anything is sent.
#[tokio::test]
async fn the_console_refuses_what_it_cannot_run() {
    let explained = RunOptions {
        explain: Some(crate::types::query::ExplainMode::Plan),
        ..RunOptions::default()
    };
    let f = fixture(store_caps()).await;
    let (state, id) = (f.state.as_ref(), f.conn.id);
    let err = query::run_sql_on(
        state,
        id,
        "x".into(),
        "find users".into(),
        Some(explained),
        sink(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported: Redis: Explain is not available"
    );
    let g = fixture(SessionCaps::minimal(Driver::Redis)).await;
    let (state, id) = (g.state.as_ref(), g.conn.id);
    let err = query::run_sql_on(state, id, "x".into(), "find users".into(), None, sink())
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported: Redis: the query editor is not available"
    );
    assert_eq!(f.store.calls(), Vec::<String>::new());
    assert_eq!(g.store.calls(), Vec::<String>::new());
}

/// Past the query time out a command is cancelled through the session (the store's
/// `CancelHandle::Custom`) and reported as timed out; the rest of the run does not start.
#[tokio::test(start_paused = true)]
async fn a_console_command_past_the_time_out_fires_the_custom_cancel() {
    let f = fixture(store_caps()).await;
    let options = RunOptions {
        timeout_secs: 2,
        ..RunOptions::default()
    };
    let (results, events) = run_console(&f.store, "wait\nfind users", options).await;
    assert_eq!(event_names(&events), ["started", "failed", "done"]);
    assert_eq!(results.len(), 1);
    let error = results[0].error.as_ref().unwrap();
    assert_eq!(error.code, QUERY_TIMEOUT_CODE);
    assert_eq!(f.store.cancels_fired(), 1);
    assert_eq!(f.store.calls(), ["wait", "cancel"]);
}

/// Stop (`cancel_query`) reaches a running console command through the session.
#[tokio::test]
async fn stop_cancels_a_console_command() {
    let f = fixture(store_caps()).await;
    let (state, id) = (f.state.clone(), f.conn.id);
    let run = tokio::spawn(async move {
        query::run_sql_on(&state, id, "q".into(), "wait".into(), None, sink()).await
    });
    let armed = || {
        f.state
            .active_queries
            .lock()
            .unwrap()
            .get(&(id, "q".to_string()))
            .is_some_and(CancelSlot::is_armed)
    };
    for _ in 0..1000 {
        if armed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(armed(), "the command runs");
    query::cancel_query_on(&f.state, id, "q".into())
        .await
        .unwrap();
    let results = run.await.unwrap().unwrap();
    assert_eq!(results[0].error.as_ref().unwrap().code, "CANCELLED");
    assert_eq!(f.store.cancels_fired(), 1);
    // Nothing runs under the id any more: nothing to cancel.
    query::cancel_query_on(&f.state, id, "q".into())
        .await
        .unwrap();
    assert_eq!(f.store.cancels_fired(), 1);
}

/// On a read-only connection the console runs only commands the store says read, and the
/// driver's own calls only the reads; a command or call it does not know counts as a write.
#[tokio::test]
async fn a_read_only_connection_runs_only_reads() {
    let f = fixture_with(store_caps(), true).await;
    let (state, id) = (f.state.as_ref(), f.conn.id);
    for text in ["find users\ndel users 1", "frobnicate"] {
        let err = query::run_sql_on(state, id, "w".into(), text.into(), None, sink())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), READ_ONLY_MESSAGE, "{text}");
    }
    let text = "find users\ncount users".to_string();
    let read = query::run_sql_on(state, id, "r".into(), text, None, sink())
        .await
        .unwrap();
    assert!(read.iter().all(|r| r.error.is_none()));

    let stats = native::session_call_on(&f.conn, "stats", json!({})).await;
    assert_eq!(stats.unwrap(), json!({ "orders": 3, "users": 2 }));
    for method in ["flush", "anything else"] {
        let err = native::session_call_on(&f.conn, method, json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), READ_ONLY_MESSAGE, "{method}");
    }
    assert_eq!(f.store.calls(), ["find users", "count users", "call stats"]);
    assert_eq!(f.store.docs("users").len(), 2);

    // A writable connection makes the write call.
    let w = fixture(store_caps()).await;
    native::session_call_on(&w.conn, "flush", Json::Null)
        .await
        .unwrap();
    assert!(w.store.docs("users").is_empty());
}

/// A SQL session answers the driver's own calls too: the core drivers have none.
#[tokio::test]
async fn a_core_session_has_no_calls_of_its_own() {
    let mut profile = redis_profile();
    profile.driver = Driver::Postgres;
    let caps = SessionCaps::core(Driver::Postgres, Dialect::Postgres, None);
    let fake = Arc::new(FakeSession::new(caps));
    let session = Session::Sql(fake.clone() as Arc<dyn DbSession>);
    let (_state, conn) = open_with(profile, session).await;
    let err = native::session_call_on(&conn, "stats", Json::Null)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported: PostgreSQL: unknown call \u{201c}stats\u{201d}"
    );
    assert_eq!(fake.calls(), Vec::<String>::new());
}

/// The connection form's calls: the core drivers have none, and a driver this build lacks is
/// refused.
#[tokio::test]
async fn driver_util_reaches_only_a_built_lane() {
    let f = fixture(store_caps()).await;
    let input = |driver: Driver| {
        let mut profile = redis_profile();
        profile.driver = driver;
        ProfileInput {
            profile,
            password: None,
            ssh_password: None,
            ssh_passphrase: None,
            clear_password: false,
            key_passphrase: None,
            session_token: None,
        }
    };
    let err = native::driver_util_on(&f.state, &input(Driver::Postgres), "probe", Json::Null)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported: PostgreSQL: unknown call \u{201c}probe\u{201d}"
    );
    if !registry::built(Driver::Redis) {
        let err = native::driver_util_on(&f.state, &input(Driver::Redis), "probe", Json::Null)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "unsupported: Redis is not built into this Tabler"
        );
    }
}
