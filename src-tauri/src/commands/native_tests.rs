//! A connection to a store that speaks no SQL holds a native session: the commands that build SQL
//! are refused at `Connection::session` before anything reaches the store, even when the store's
//! caps grant their feature, and what every session does goes through `any_session`.

use std::collections::HashMap;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{Value as Json, json};
use tauri::ipc::Channel;
use tokio::sync::{RwLock, watch};
use uuid::Uuid;

use crate::appdb::AppDb;
use crate::commands::{
    backup, browse, conn_extras, connections, ddl, export, import, metrics, objects,
    workspace_commit,
};
use crate::drivers::Session;
use crate::error::AppError;
use crate::secrets::{Backend, Secrets};
use crate::state::{AppState, Connection};
use crate::testkit::FakeStore;
use crate::types::caps::{Feature, SessionCaps};
use crate::types::profile::{
    ConnStatus, ConnectionInfo, ConnectionProfile, Driver, ProfileOptions, SslMode,
};
use crate::types::schema::TableRef;

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
    let appdb = AppDb::open_in_memory().unwrap();
    let profile = appdb.upsert_profile(&redis_profile()).unwrap();
    let state = Arc::new(AppState::new(
        appdb.clone(),
        Secrets::new(Backend::File, appdb),
    ));
    let store = Arc::new(FakeStore::new(caps.clone()));
    let id = Uuid::new_v4();
    let conn = Arc::new(Connection {
        id,
        profile: profile.clone(),
        session: RwLock::new(Session::Native(store.clone())),
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
            read_only: false,
            capabilities: Some(caps),
        }),
        schema_cache: RwLock::new(HashMap::new()),
        status: watch::channel(ConnStatus::Connected).0,
        tunnel: None,
    });
    state.insert_connection(conn.clone(), None).await;
    Fixture {
        state,
        conn,
        store,
        dir: tempfile::tempdir().unwrap(),
    }
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
