//! Every command and MCP tool that needs a `Feature` refuses a session whose caps lack it, and
//! does so before anything reaches the server (the table in `docs/drivers/README.md`).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{Value as Json, json};
use tauri::ipc::Channel;
use tokio::sync::{RwLock, watch};
use uuid::Uuid;

use crate::appdb::AppDb;
use crate::commands::{
    admin as admin_cmd, assistant, backup, browse, conn_extras, connections, ddl, export, import,
    lock, metrics, mutate, objects as objects_cmd, query, schema, workspace_commit,
};
use crate::drivers::postgres::flavor;
use crate::drivers::{CancelSlot, DbSession, Session};
use crate::error::AppError;
use crate::mcp::gate::session_writable;
use crate::mcp::prefs::{McpPrefs, PREFS_KEY};
use crate::mcp::tools::{self, CallCancel, Ctx};
use crate::secrets::{Backend, Secrets};
use crate::sqlgen::SqlDialect;
use crate::sqlgen::dialect::testing::{Override, override_spec};
use crate::sqlgen::portable::testing::ANSI_TEST;
use crate::state::{AppState, Connection};
use crate::testkit::FakeSession;
use crate::types::admin::KillScope;
use crate::types::browse::{FilterGroup, RowKey};
use crate::types::caps::{Feature, SessionCaps};
use crate::types::driver::{Flavor, Location, PgFlavor};
use crate::types::objects::ObjectOp;
use crate::types::profile::{
    ConnStatus, ConnectionInfo, ConnectionProfile, Driver, ProfileOptions, SslMode,
};
use crate::types::query::{ExplainMode, RunOptions};
use crate::types::schema::{TableMeta, TableRef};
use crate::types::value::Value;

/// One call of a command (or of the logic a command runs first) that needs a feature.
#[derive(Debug, Clone, Copy)]
enum Call {
    RunSql,
    RunSqlExplained,
    Explain,
    ExplainAnalyze,
    Update,
    Insert,
    Delete,
    UpdateByAllColumns,
    DeleteByRowid,
    InsertExpression,
    ExactCount,
    FilteredCount,
    PreviewWorkspace,
    ApplyWorkspace,
    Browse,
    GetTable,
    FetchCell,
    SystemSchemas,
    DatabaseItems,
    ItemOverview,
    RawDdl,
    StructureChoices,
    SetComment,
    ApplySetComment,
    CreateTable,
    CloneTable,
    ApplyCloneTable,
    DropTable,
    DropTables,
    ApplyDropTables,
    TruncateTable,
    TruncateTables,
    ApplyTruncateTables,
    RenameTable,
    DropTrigger,
    TableDdl,
    ViewDefinition,
    ListRoutines,
    RoutineDefinition,
    DropRoutines,
    ApplyDropRoutines,
    ListSequences,
    SequenceDefinition,
    DropSequences,
    ApplyDropSequences,
    Diagram,
    SwitchDatabase,
    DatabaseOptions,
    PreviewCreateDatabase,
    ApplyCreateDatabase,
    RenameDatabase,
    DropDatabase,
    CreateSchema,
    DropSchema,
    ListProcesses,
    Kill,
    ListUsers,
    LoadUser,
    PreviewUser,
    ApplyUser,
    ExportDatabaseTables,
    ExportTables,
    ExportTablesSql,
    ExportData,
    ExportDataSql,
    ExportResults,
    ExportResultsSql,
    ImportCsv,
    ImportJson,
    PreviewSqlDump,
    ImportSqlDump,
    BackupInspect,
    BackupRun,
    RestoreRun,
    GetEncoding,
    SetEncoding,
    SessionDetails,
    Metrics,
    AssistantContext,
    AssistantChat,
    McpListDatabases,
    McpListTables,
    McpDescribe,
    McpRunQuery,
    McpExplain,
}

/// The calls, each with the feature it is refused without and the features it needs besides.
const CALLS: &[(Feature, &[Feature], Call)] = &[
    (Feature::Editor, &[], Call::RunSql),
    (Feature::Explain, &[Feature::Editor], Call::RunSqlExplained),
    (Feature::Explain, &[], Call::Explain),
    (Feature::ExplainAnalyze, &[], Call::ExplainAnalyze),
    (Feature::EditRows, &[], Call::Update),
    (Feature::InsertRows, &[], Call::Insert),
    (Feature::DeleteRows, &[], Call::Delete),
    (
        Feature::EditWithoutKey,
        &[Feature::EditRows],
        Call::UpdateByAllColumns,
    ),
    (
        Feature::EditWithoutKey,
        &[Feature::DeleteRows],
        Call::DeleteByRowid,
    ),
    (
        Feature::ServerExpressions,
        &[Feature::InsertRows],
        Call::InsertExpression,
    ),
    (Feature::ExactCount, &[], Call::ExactCount),
    (Feature::ExactCount, &[], Call::FilteredCount),
    (Feature::WorkspaceCommit, &[], Call::PreviewWorkspace),
    (Feature::WorkspaceCommit, &[], Call::ApplyWorkspace),
    (Feature::StructureView, &[], Call::Browse),
    (Feature::StructureView, &[], Call::GetTable),
    (Feature::StructureView, &[], Call::FetchCell),
    (Feature::StructureView, &[], Call::SystemSchemas),
    (Feature::StructureView, &[], Call::DatabaseItems),
    (Feature::StructureView, &[], Call::ItemOverview),
    (Feature::StructureEdit, &[], Call::RawDdl),
    (Feature::StructureEdit, &[], Call::StructureChoices),
    (Feature::StructureEdit, &[], Call::SetComment),
    (Feature::StructureEdit, &[], Call::ApplySetComment),
    (Feature::CreateTable, &[], Call::CreateTable),
    (Feature::CreateTable, &[], Call::CloneTable),
    (Feature::CreateTable, &[], Call::ApplyCloneTable),
    (Feature::DropTable, &[], Call::DropTable),
    (Feature::DropTable, &[], Call::DropTables),
    (Feature::DropTable, &[], Call::ApplyDropTables),
    (Feature::TruncateTable, &[], Call::TruncateTable),
    (Feature::TruncateTable, &[], Call::TruncateTables),
    (Feature::TruncateTable, &[], Call::ApplyTruncateTables),
    (Feature::RenameTable, &[], Call::RenameTable),
    (Feature::Triggers, &[], Call::DropTrigger),
    (Feature::TableDdl, &[], Call::TableDdl),
    (Feature::ViewDefinition, &[], Call::ViewDefinition),
    (Feature::Routines, &[], Call::ListRoutines),
    (Feature::Routines, &[], Call::RoutineDefinition),
    (Feature::Routines, &[], Call::DropRoutines),
    (Feature::Routines, &[], Call::ApplyDropRoutines),
    (Feature::Sequences, &[], Call::ListSequences),
    (Feature::Sequences, &[], Call::SequenceDefinition),
    (Feature::Sequences, &[], Call::DropSequences),
    (Feature::Sequences, &[], Call::ApplyDropSequences),
    (Feature::Diagram, &[], Call::Diagram),
    (Feature::SwitchDatabase, &[], Call::SwitchDatabase),
    (Feature::CreateDatabase, &[], Call::DatabaseOptions),
    (Feature::CreateDatabase, &[], Call::PreviewCreateDatabase),
    (Feature::CreateDatabase, &[], Call::ApplyCreateDatabase),
    (Feature::RenameDatabase, &[], Call::RenameDatabase),
    (Feature::DropDatabase, &[], Call::DropDatabase),
    (Feature::CreateSchema, &[], Call::CreateSchema),
    (Feature::DropSchema, &[], Call::DropSchema),
    (Feature::ProcessList, &[], Call::ListProcesses),
    (Feature::KillProcess, &[], Call::Kill),
    (Feature::Users, &[], Call::ListUsers),
    (Feature::Users, &[], Call::LoadUser),
    (Feature::Users, &[], Call::PreviewUser),
    (Feature::Users, &[], Call::ApplyUser),
    (Feature::ExportTables, &[], Call::ExportDatabaseTables),
    (Feature::ExportTables, &[], Call::ExportTables),
    (
        Feature::ExportSql,
        &[Feature::ExportTables],
        Call::ExportTablesSql,
    ),
    (Feature::ExportTables, &[], Call::ExportData),
    (
        Feature::ExportSql,
        &[Feature::ExportTables],
        Call::ExportDataSql,
    ),
    (Feature::ExportTables, &[], Call::ExportResults),
    (
        Feature::ExportSql,
        &[Feature::ExportTables],
        Call::ExportResultsSql,
    ),
    (Feature::ImportCsv, &[], Call::ImportCsv),
    (Feature::ImportJson, &[], Call::ImportJson),
    (Feature::ImportSqlDump, &[], Call::PreviewSqlDump),
    (Feature::ImportSqlDump, &[], Call::ImportSqlDump),
    (Feature::Backup, &[], Call::BackupInspect),
    (Feature::Backup, &[], Call::BackupRun),
    (Feature::Restore, &[], Call::RestoreRun),
    (Feature::Encoding, &[], Call::GetEncoding),
    (Feature::Encoding, &[], Call::SetEncoding),
    (Feature::SessionDetails, &[], Call::SessionDetails),
    (Feature::Metrics, &[], Call::Metrics),
    (Feature::Assistant, &[], Call::AssistantContext),
    (Feature::Assistant, &[], Call::AssistantChat),
    (Feature::Mcp, &[], Call::McpListDatabases),
    (Feature::Mcp, &[], Call::McpListTables),
    (Feature::Mcp, &[], Call::McpDescribe),
    (Feature::Mcp, &[], Call::McpRunQuery),
    (Feature::Mcp, &[], Call::McpExplain),
    (Feature::StructureView, &[Feature::Mcp], Call::McpDescribe),
    (Feature::Editor, &[Feature::Mcp], Call::McpRunQuery),
    (Feature::Explain, &[Feature::Mcp], Call::McpExplain),
];

/// An open connection of `profile` on a fake session, and the MCP server's view of it.
struct Fixture {
    state: Arc<AppState>,
    ctx: Ctx,
    conn: Arc<Connection>,
    profile: ConnectionProfile,
    writable: bool,
    dir: tempfile::TempDir,
}

/// A profile of the session's engine: a file for the file engines, a host for the others, and
/// the flavor its caps name.
fn profile_of(caps: &SessionCaps, dir: &std::path::Path) -> ConnectionProfile {
    let file = caps.driver.descriptor().location == Location::File;
    ConnectionProfile {
        id: Uuid::new_v4(),
        name: format!("{:?} {:?}", caps.driver, caps.flavor),
        color: None,
        group: None,
        driver: caps.driver,
        host: (!file).then(|| "db.example".to_string()),
        port: None,
        user: None,
        database: None,
        ssl: SslMode::Disable,
        ssl_ca_path: None,
        sqlite_path: file.then(|| dir.join("db.file").to_string_lossy().into_owned()),
        ssh: None,
        options: ProfileOptions {
            pg_flavor: match caps.flavor {
                Some(Flavor::Cockroachdb) => Some(PgFlavor::Cockroachdb),
                Some(Flavor::Greenplum) => Some(PgFlavor::Greenplum),
                Some(Flavor::Redshift) => Some(PgFlavor::Redshift),
                Some(Flavor::Mariadb) | None => None,
            },
            ..ProfileOptions::default()
        },
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    }
}

async fn fixture(caps: &SessionCaps) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let appdb = AppDb::open_in_memory().unwrap();
    appdb
        .set_setting(
            PREFS_KEY,
            &json!({ "mcpEnabled": true, "mcpAllowWrites": true }),
        )
        .unwrap();
    let profile = appdb.upsert_profile(&profile_of(caps, dir.path())).unwrap();
    let writable = session_writable(&profile, &McpPrefs::load(&appdb).unwrap());
    let state = Arc::new(AppState::new(
        appdb.clone(),
        Secrets::new(Backend::File, appdb),
    ));
    let id = Uuid::new_v4();
    let session: Arc<dyn DbSession> = Arc::new(FakeSession::new(caps.clone()));
    let conn = Arc::new(Connection {
        id,
        profile: profile.clone(),
        session: RwLock::new(session.into()),
        info: RwLock::new(ConnectionInfo {
            connection_id: id,
            profile_id: profile.id,
            name: profile.name.clone(),
            color: None,
            driver: profile.driver,
            server_version: String::new(),
            current_database: Some("main".into()),
            current_user: None,
            tunnel: None,
            read_only: false,
            capabilities: Some(caps.clone()),
        }),
        schema_cache: RwLock::new(HashMap::new()),
        status: watch::channel(ConnStatus::Connected).0,
        tunnel: None,
    });
    state.insert_connection(conn.clone(), None).await;
    Fixture {
        ctx: Ctx::new(state.clone()),
        state,
        conn,
        profile,
        writable,
        dir,
    }
}

impl Fixture {
    /// Gives the connection, and the MCP pool's connection of the profile, a new fake session.
    async fn use_session(&self, caps: SessionCaps) -> Arc<FakeSession> {
        let fake = Arc::new(FakeSession::new(caps));
        *self.conn.session.write().await = Session::Sql(fake.clone());
        self.ctx
            .pool
            .insert_for_test(&self.profile, None, self.writable, fake.clone())
            .await;
        fake
    }

    /// Calls MCP tool `name` on the profile. A failed call's text comes back as an
    /// `Unsupported` error (MCP has no error codes: the tests read the text).
    async fn tool(&self, name: &str, extra: Json) -> Result<(), AppError> {
        let mut args = json!({ "connection": self.profile.id.to_string() });
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let out = tools::call(&self.ctx, name, &args, &CallCancel::default())
            .await
            .unwrap();
        if out.is_error {
            Err(AppError::Unsupported(out.text))
        } else {
            Ok(())
        }
    }
}

fn from<T: DeserializeOwned>(value: Json) -> T {
    serde_json::from_value(value).unwrap()
}

fn sink<T>() -> Channel<T> {
    Channel::new(|_| Ok(()))
}

fn table() -> TableRef {
    TableRef::new(Some("s"), "t")
}

fn routine() -> Json {
    json!({ "schema": "s", "name": "f", "kind": "function", "identityArguments": "" })
}

fn csv() -> Json {
    json!({ "format": "csv", "delimiter": ",", "header": true, "nullAs": "" })
}

fn sql_insert() -> Json {
    json!({ "format": "sqlInsert", "tableName": "t", "batchSize": 100 })
}

fn rows(changes: Json) -> crate::types::mutate::ApplyRequest {
    from(json!({ "table": table(), "changes": [changes], "returning": true }))
}

fn ddl_op(op: Json) -> Vec<crate::types::ddl::DdlOp> {
    vec![from(op)]
}

/// The operation an object-menu call previews or applies.
fn object_call(call: Call) -> ObjectOp {
    let t = table();
    from(match call {
        Call::SetComment | Call::ApplySetComment => json!({
            "op": "setComment", "target": { "table": t, "kind": "table" }, "comment": "c",
        }),
        Call::CloneTable | Call::ApplyCloneTable => {
            json!({ "op": "cloneTable", "source": t, "newName": "t2", "withData": false })
        }
        Call::DropTables | Call::ApplyDropTables => json!({
            "op": "dropTables", "tables": [{ "table": t, "kind": "table" }],
            "cascade": false, "ignoreForeignKeys": false,
        }),
        Call::TruncateTables | Call::ApplyTruncateTables => json!({
            "op": "truncateTables", "tables": [t], "cascade": false,
            "restartIdentity": false, "ignoreForeignKeys": false,
        }),
        Call::DropRoutines | Call::ApplyDropRoutines => {
            json!({ "op": "dropRoutines", "routines": [routine()], "cascade": false })
        }
        Call::DropSequences | Call::ApplyDropSequences => {
            json!({ "op": "dropSequences", "sequences": [t], "cascade": false })
        }
        other => panic!("{other:?} is not an object operation"),
    })
}

fn filter() -> FilterGroup {
    from(json!({ "conjunction": "and", "items": [{ "kind": "raw", "sql": "v > 1" }] }))
}

/// Metadata of `table()` with the planner's row estimate `estimate`.
fn meta(estimate: Option<i64>) -> TableMeta {
    from(json!({
        "table": table(), "kind": "table", "columns": [], "primaryKey": [], "indexes": [],
        "foreignKeys": [], "triggers": [], "comment": null, "rowEstimate": estimate, "ddl": null,
    }))
}

fn admin_op(op: Json) -> crate::types::admin::AdminDdl {
    from(op)
}

fn backup_job(kind: &str, profile_id: Uuid) -> crate::types::backup::BackupJobRequest {
    from(json!({
        "jobId": format!("caps-{kind}-{profile_id}"), "kind": kind, "profileId": profile_id,
        "database": "d", "binaryPath": "", "flags": [], "gzip": false, "path": "out.dump",
    }))
}

/// Runs `call` on the fixture's connection; Ok when it got past every check (which a refused
/// call never does).
async fn run(call: Call, f: &Fixture) -> Result<(), AppError> {
    let state = f.state.as_ref();
    let conn = f.conn.as_ref();
    let id = conn.id;
    let session = conn.session().await?;
    let s = session.as_ref();
    let t = table();
    let one = json!(Value::Int(1));
    let pk = RowKey::Pk(BTreeMap::from([("id".to_string(), Value::Int(1))]));
    let user = from(json!({ "name": "u", "host": null }));
    let change = from(json!({ "original": null, "edited": null, "password": null }));
    let explained = RunOptions {
        explain: Some(ExplainMode::Plan),
        ..RunOptions::default()
    };
    let qid = || format!("caps-{call:?}");
    match call {
        Call::RunSql => query::run_sql_on(state, id, qid(), "SELECT 1".into(), None, sink())
            .await
            .map(drop),
        Call::RunSqlExplained => {
            query::run_sql_on(state, id, qid(), "SELECT 1".into(), Some(explained), sink())
                .await
                .map(drop)
        }
        Call::Explain => {
            query::explain_inner(s, "SELECT 1", ExplainMode::Plan, &CancelSlot::default())
                .await
                .map(drop)
        }
        Call::ExplainAnalyze => {
            query::explain_inner(s, "SELECT 1", ExplainMode::Analyze, &CancelSlot::default())
                .await
                .map(drop)
        }
        Call::Update => mutate::plan_rows(
            conn,
            &rows(json!({ "op": "update", "rowId": "1", "key": pk, "changes": { "v": one } })),
        )
        .await
        .map(drop),
        Call::Insert => mutate::plan_rows(
            conn,
            &rows(json!({ "op": "insert", "rowId": "1", "values": { "v": one } })),
        )
        .await
        .map(drop),
        Call::Delete => mutate::plan_rows(
            conn,
            &rows(json!({ "op": "delete", "rowId": "1", "key": pk })),
        )
        .await
        .map(drop),
        Call::UpdateByAllColumns => mutate::plan_rows(
            conn,
            &rows(json!({
                "op": "update", "rowId": "1",
                "key": { "kind": "allColumns", "value": { "v": one } }, "changes": { "v": one },
            })),
        )
        .await
        .map(drop),
        Call::DeleteByRowid => mutate::plan_rows(
            conn,
            &rows(json!({ "op": "delete", "rowId": "1", "key": { "kind": "rowid", "value": 7 } })),
        )
        .await
        .map(drop),
        Call::InsertExpression => mutate::plan_rows(
            conn,
            &rows(json!({
                "op": "insert", "rowId": "1", "values": {}, "exprs": { "at": "now" },
            })),
        )
        .await
        .map(drop),
        Call::ExactCount => browse::count_rows(conn, &t, None, true, 0, None)
            .await
            .map(drop),
        Call::FilteredCount => browse::count_rows(conn, &t, Some(&filter()), false, 0, None)
            .await
            .map(drop),
        Call::PreviewWorkspace => workspace_commit::preview_workspace_on(conn, &[])
            .await
            .map(drop),
        Call::ApplyWorkspace => workspace_commit::apply_workspace_on(conn, &[])
            .await
            .map(drop),
        Call::Browse => browse::browse_rows(
            conn,
            &from(json!({ "table": t, "columns": null, "filter": null, "limit": 10, "offset": 0 })),
            None,
        )
        .await
        .map(drop),
        Call::GetTable => schema::table_on(conn, t, false).await.map(drop),
        Call::FetchCell => browse::fetch_cell_on(conn, &t, &pk, "v", None)
            .await
            .map(drop),
        Call::SystemSchemas => crate::objects::system_schemas(s).await.map(drop),
        Call::DatabaseItems => crate::objects::database_items(s, false).await.map(drop),
        Call::ItemOverview => crate::objects::item_overview(s, "s").await.map(drop),
        Call::RawDdl => ddl::plan_for(
            conn,
            Some(&t),
            &ddl_op(json!({ "op": "rawSql", "sql": "ALTER TABLE t ADD c int" })),
        )
        .await
        .map(drop),
        Call::StructureChoices => schema::structure_choices(s).await.map(drop),
        // The object menu: `preview_object_op` is `plan_op`; `apply_object_op` plans and runs.
        Call::SetComment
        | Call::CloneTable
        | Call::DropTables
        | Call::TruncateTables
        | Call::DropRoutines
        | Call::DropSequences => crate::objects::plan_op(s, &object_call(call))
            .await
            .map(drop),
        Call::ApplySetComment
        | Call::ApplyCloneTable
        | Call::ApplyDropTables
        | Call::ApplyTruncateTables
        | Call::ApplyDropRoutines
        | Call::ApplyDropSequences => objects_cmd::apply_object_op_on(conn, &object_call(call))
            .await
            .map(drop),
        Call::CreateTable => ddl::plan_for(
            conn,
            None,
            &ddl_op(json!({
                "op": "createTable", "table": t, "columns": [], "primaryKey": [],
                "ifNotExists": false,
            })),
        )
        .await
        .map(drop),
        Call::DropTable => ddl::plan_for(
            conn,
            Some(&t),
            &ddl_op(json!({ "op": "dropTable", "table": t, "ifExists": false, "cascade": false })),
        )
        .await
        .map(drop),
        Call::TruncateTable => ddl::plan_for(
            conn,
            Some(&t),
            &ddl_op(json!({
                "op": "truncateTable", "table": t, "restartIdentity": false, "cascade": false,
            })),
        )
        .await
        .map(drop),
        Call::RenameTable => ddl::plan_for(
            conn,
            Some(&t),
            &ddl_op(json!({ "op": "renameTable", "table": t, "newName": "u" })),
        )
        .await
        .map(drop),
        Call::DropTrigger => ddl::plan_for(
            conn,
            Some(&t),
            &ddl_op(json!({ "op": "dropTrigger", "table": t, "name": "tr" })),
        )
        .await
        .map(drop),
        Call::TableDdl => ddl::table_ddl_on(conn, &t).await.map(drop),
        Call::ViewDefinition => schema::view_definition_on(conn, &t).await.map(drop),
        Call::ListRoutines => crate::objects::list_routines(s, "s").await.map(drop),
        Call::RoutineDefinition => crate::objects::routine_definition(s, &from(routine()))
            .await
            .map(drop),
        Call::ListSequences => crate::objects::list_sequences(s, "s").await.map(drop),
        Call::SequenceDefinition => crate::objects::sequence_definition(s, &t).await.map(drop),
        Call::Diagram => crate::diagram::schema_relations(s, "s").await.map(drop),
        Call::SwitchDatabase => connections::switch_database_on(state, id, "other".into())
            .await
            .map(drop),
        Call::DatabaseOptions => crate::admin::database_options(s).await.map(drop),
        Call::PreviewCreateDatabase => admin_cmd::preview_ddl_on(
            s,
            &admin_op(json!({
                "op": "createDatabase", "name": "n", "encoding": null, "collation": null,
                "template": null,
            })),
        )
        .map(drop),
        Call::ApplyCreateDatabase => crate::admin::apply_ddl(
            s,
            &admin_op(json!({
                "op": "createDatabase", "name": "n", "encoding": null, "collation": null,
                "template": null,
            })),
        )
        .await
        .map(drop),
        Call::RenameDatabase => admin_cmd::preview_ddl_on(
            s,
            &admin_op(json!({ "op": "renameDatabase", "name": "n", "newName": "m" })),
        )
        .map(drop),
        Call::DropDatabase => {
            admin_cmd::preview_ddl_on(s, &admin_op(json!({ "op": "dropDatabase", "name": "n" })))
                .map(drop)
        }
        Call::CreateSchema => admin_cmd::preview_ddl_on(
            s,
            &admin_op(json!({ "op": "createSchema", "name": "n", "owner": null })),
        )
        .map(drop),
        Call::DropSchema => admin_cmd::preview_ddl_on(
            s,
            &admin_op(json!({ "op": "dropSchema", "name": "n", "cascade": false })),
        )
        .map(drop),
        Call::ListProcesses => crate::admin::list_processes(s).await.map(drop),
        Call::Kill => crate::admin::kill(s, &["1".to_string()], KillScope::Query)
            .await
            .map(drop),
        Call::ListUsers => crate::admin::list_users(s, false).await.map(drop),
        Call::LoadUser => crate::admin::load_user(s, &user).await.map(drop),
        Call::PreviewUser => crate::admin::preview_user_on(s, &change).await.map(drop),
        Call::ApplyUser => crate::admin::apply_user(s, &change).await.map(drop),
        Call::ExportDatabaseTables => export::export_database_tables_on(state, id, None)
            .await
            .map(drop),
        Call::ExportTables | Call::ExportTablesSql => {
            let format = match call {
                Call::ExportTablesSql => sql_insert(),
                _ => csv(),
            };
            let req = from(json!({
                "exportId": qid(), "tables": [t], "format": format, "layout": "filePerTable",
                "target": f.dir.path().join("out"),
            }));
            export::export_tables_on(state, id, req, sink())
                .await
                .map(drop)
        }
        Call::ExportData | Call::ExportDataSql => {
            let format = match call {
                Call::ExportDataSql => sql_insert(),
                _ => csv(),
            };
            let req = from(json!({
                "exportId": qid(), "source": { "kind": "table", "table": t, "filter": null,
                "columns": null }, "format": format,
                "path": f.dir.path().join("out.csv"), "maxRows": null,
            }));
            export::export_data_on(state, id, req, sink())
                .await
                .map(drop)
        }
        // `export_results` runs this on the connection's session.
        Call::ExportResults | Call::ExportResultsSql => {
            let format = match call {
                Call::ExportResultsSql => sql_insert(),
                _ => csv(),
            };
            let req = from(json!({
                "exportId": qid(), "results": [], "format": format, "layout": "filePerTable",
                "target": f.dir.path().join("results"),
            }));
            crate::export::results::run_export_results(
                s,
                &req,
                |_| {},
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .map(drop)
        }
        Call::ImportCsv => {
            let req = from(json!({
                "importId": qid(), "path": "in.csv",
                "format": { "delimiter": null, "quote": "\"", "encoding": null, "header": true },
                "target": { "kind": "existing", "table": t }, "mapping": [],
                "emptyAsNull": false, "mode": "stopOnError",
            }));
            import::import_csv_on(state, id, req, sink())
                .await
                .map(drop)
        }
        Call::ImportJson => {
            let req = from(json!({
                "importId": qid(), "path": "in.json", "encoding": null, "keys": [],
                "target": { "kind": "existing", "table": t }, "mapping": [],
                "mode": "stopOnError",
            }));
            import::import_json_on(state, id, req, sink())
                .await
                .map(drop)
        }
        Call::PreviewSqlDump => {
            import::preview_sql_dump_on(state, id, from(json!({ "path": "in.sql" })))
                .await
                .map(drop)
        }
        Call::ImportSqlDump => {
            let req = from(json!({ "importId": qid(), "path": "in.sql", "stopOnError": true }));
            import::import_sql_dump_on(state, id, req, sink())
                .await
                .map(drop)
        }
        Call::BackupInspect => backup::backup_inspect_target_on(state, f.profile.id)
            .await
            .map(drop),
        Call::BackupRun => backup::backup_run_on(state, backup_job("backup", f.profile.id), sink())
            .await
            .map(drop),
        Call::RestoreRun => {
            backup::backup_run_on(state, backup_job("restore", f.profile.id), sink())
                .await
                .map(drop)
        }
        Call::GetEncoding => conn_extras::connection_encoding_on(state, id)
            .await
            .map(drop),
        Call::SetEncoding => {
            conn_extras::set_connection_encoding_on(state, id, Some("LATIN1".into()))
                .await
                .map(drop)
        }
        Call::SessionDetails => lock::session_details_of(s).await.map(drop),
        Call::Metrics => {
            let query = from(json!({
                "parts": ["SELECT 1"], "values": [], "allowWrites": false, "maxRows": 10,
            }));
            metrics::metrics_run_on(state, id, qid(), query)
                .await
                .map(drop)
        }
        Call::AssistantContext => {
            assistant::assistant_context_on(state, &from(json!({ "connectionId": id })))
                .await
                .map(drop)
        }
        Call::AssistantChat => {
            let req = from(json!({
                "requestId": qid(), "provider": { "provider": "ollama", "model": "m" },
                "systemPrompt": "", "messages": [], "context": { "connectionId": id },
            }));
            assistant::assistant_chat_on(state, req, sink())
                .await
                .map(drop)
        }
        Call::McpListDatabases => f.tool("list_databases", json!({})).await,
        Call::McpListTables => f.tool("list_tables", json!({})).await,
        Call::McpDescribe => f.tool("describe_table", json!({ "table": "s.t" })).await,
        Call::McpRunQuery => f.tool("run_query", json!({ "sql": "SELECT 1" })).await,
        Call::McpExplain => f.tool("explain_query", json!({ "sql": "SELECT 1" })).await,
    }
}

/// The extension dialects of `sessions`, as built ones whatever this build's features (MCP
/// reads the statements with them before it connects), while the guards live.
fn built_dialects() -> [Override; 2] {
    [
        override_spec(SqlDialect::Duckdb, &ANSI_TEST),
        override_spec(SqlDialect::Redshift, &ANSI_TEST),
    ]
}

/// The sessions the calls are refused on: a core one, the minimal caps of a driver on an
/// extension dialect and of one on a core dialect, a Redshift session (the PostgreSQL driver on
/// Redshift SQL), and a CockroachDB one (every PostgreSQL feature but what the flavors are not
/// proven for).
fn sessions() -> Vec<SessionCaps> {
    vec![
        SessionCaps::core(Driver::Postgres, crate::sqlgen::Dialect::Postgres, None),
        SessionCaps::minimal(Driver::Duckdb),
        SessionCaps::minimal(Driver::Libsql),
        SessionCaps::base(
            Driver::Postgres,
            SqlDialect::Redshift,
            Some(Flavor::Redshift),
        ),
        flavor::adjust_caps(SessionCaps::base(
            Driver::Postgres,
            SqlDialect::Postgres,
            Some(Flavor::Cockroachdb),
        )),
    ]
}

/// Each call is refused, with the missing feature named and nothing sent, on a session that
/// lacks its feature (and has every other one it needs).
#[tokio::test]
async fn every_gated_call_refuses_a_session_without_its_feature_before_sending_anything() {
    // Every feature a command checks is in the table.
    for f in Feature::ALL {
        let checked = CALLS.iter().any(|(target, _, _)| *target == f);
        let renderer_only = matches!(
            f,
            Feature::CancelOnServer
                | Feature::Format
                | Feature::Returning
                | Feature::CopyAsInsert
                | Feature::Timezone
                | Feature::StartupCommands
                | Feature::LaravelMigration
                | Feature::DummyData
        );
        assert!(checked != renderer_only, "{f:?}");
    }
    let _built = built_dialects();
    for base in sessions() {
        let f = fixture(&base).await;
        for (target, besides, call) in CALLS {
            let caps = base.clone().with(besides).without(&[*target]);
            let fake = f.use_session(caps).await;
            let what = format!("{call:?} on {:?} {:?}", base.driver, base.dialect);
            let err = match run(*call, &f).await {
                Err(e) => e,
                Ok(()) => panic!("{what}: not refused"),
            };
            assert_eq!(err.code(), "UNSUPPORTED", "{what}: {err}");
            assert_eq!(fake.calls(), Vec::<String>::new(), "{what}: sent");
            // Only the core engines have dump tools (and a flavor this build lacks is not
            // dumped as PostgreSQL): other profiles are refused before the caps are read.
            let backup = matches!(
                call,
                Call::BackupInspect | Call::BackupRun | Call::RestoreRun
            );
            if backup && (!base.driver.is_core() || base.flavor.is_some()) {
                continue;
            }
            let message = err.to_string();
            assert!(
                message.contains(&format!("{} is not available", target.label())),
                "{what}: {message}"
            );
            assert!(
                message.contains(base.engine()),
                "{what}: {message} names the engine"
            );
        }
    }
}

/// The minimal caps of every driver refuse every call but the editor's and the structure's.
#[tokio::test]
async fn minimal_sessions_refuse_every_other_call() {
    let _built = built_dialects();
    let base = SessionCaps::minimal(Driver::Duckdb);
    let f = fixture(&base).await;
    for (target, besides, call) in CALLS {
        if [Feature::Editor, Feature::StructureView].contains(target) && besides.is_empty() {
            continue;
        }
        let fake = f.use_session(base.clone()).await;
        let err = match run(*call, &f).await {
            Err(e) => e,
            Ok(()) => panic!("{call:?}: not refused"),
        };
        assert_eq!(err.code(), "UNSUPPORTED", "{call:?}: {err}");
        assert_eq!(fake.calls(), Vec::<String>::new(), "{call:?}: sent");
    }
}

/// A PostgreSQL flavor has no structure editor but keeps the object menu's drops and truncates:
/// each applies with its own feature (as its preview does), and is refused without it.
#[tokio::test]
async fn object_operations_apply_on_a_flavor_without_the_structure_editor() {
    let cockroach = flavor::adjust_caps(SessionCaps::base(
        Driver::Postgres,
        SqlDialect::Postgres,
        Some(Flavor::Cockroachdb),
    ));
    assert!(!cockroach.has(Feature::StructureEdit));
    let f = fixture(&cockroach).await;
    for (call, feature, sent) in [
        (Call::ApplyDropTables, Feature::DropTable, "DROP TABLE"),
        (
            Call::ApplyTruncateTables,
            Feature::TruncateTable,
            "TRUNCATE",
        ),
        (
            Call::ApplyDropSequences,
            Feature::Sequences,
            "DROP SEQUENCE",
        ),
    ] {
        let fake = f.use_session(cockroach.clone()).await;
        let result = objects_cmd::apply_object_op_on(&f.conn, &object_call(call))
            .await
            .unwrap();
        assert!(result.error.is_none(), "{call:?}: {:?}", result.error);
        assert!(
            fake.calls().iter().any(|sql| sql.contains(sent)),
            "{call:?}: {:?}",
            fake.calls()
        );
        let fake = f.use_session(cockroach.clone().without(&[feature])).await;
        let err = objects_cmd::apply_object_op_on(&f.conn, &object_call(call))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "UNSUPPORTED", "{call:?}: {err}");
        assert_eq!(fake.calls(), Vec::<String>::new(), "{call:?}: sent");
    }
}

/// Without `ExactCount` a count gives the table's estimate or nothing: a filtered count (never
/// estimated) is refused before the table is read, an unfiltered one once its structure shows
/// no estimate to give.
#[tokio::test]
async fn a_count_without_exact_count_gives_the_estimate_or_is_refused() {
    let caps = SessionCaps::core(Driver::Postgres, crate::sqlgen::Dialect::Postgres, None)
        .without(&[Feature::ExactCount]);
    let f = fixture(&caps).await;
    let fake = f.use_session(caps).await;
    let t = table();
    let err = browse::count_rows(&f.conn, &t, Some(&filter()), false, 0, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "UNSUPPORTED", "{err}");
    assert_eq!(fake.calls(), Vec::<String>::new());
    // The grid read the structure already.
    f.conn
        .schema_cache
        .write()
        .await
        .insert(t.clone(), Arc::new(meta(Some(900_000))));
    let estimate = browse::count_rows(&f.conn, &t, None, false, 500_000, None)
        .await
        .unwrap();
    assert_eq!((estimate.count, estimate.exact), (900_000, false));
    // Asked to count, or below the estimate's threshold.
    for (exact, above) in [(true, 500_000), (false, 1_000_000)] {
        let err = browse::count_rows(&f.conn, &t, None, exact, above, None)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "UNSUPPORTED", "{err}");
    }
    assert_eq!(fake.calls(), Vec::<String>::new());
}

/// MCP reads the statements as the profile's kind speaks before it connects; a session that
/// speaks another SQL (a server that is not what its profile says) runs none of them.
#[tokio::test]
async fn mcp_runs_nothing_on_a_session_that_speaks_another_sql() {
    let postgres = SessionCaps::core(Driver::Postgres, crate::sqlgen::Dialect::Postgres, None);
    let f = fixture(&postgres).await;
    let mut redshift =
        SessionCaps::base(Driver::Postgres, SqlDialect::Redshift, None).with(&[Feature::Mcp]);
    redshift.flavor = Some(Flavor::Redshift);
    let fake = f.use_session(redshift).await;
    let err = f
        .tool("run_query", json!({ "sql": "SELECT 1" }))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("speaks Redshift"), "{err}");
    assert!(fake.calls().is_empty());
}

/// `ConnectionInfo` from before capabilities existed (an older payload) still reads, and one
/// without them is written as before.
#[test]
fn connection_info_without_capabilities_decodes() {
    let id = Uuid::new_v4();
    let old = json!({
        "connectionId": id, "profileId": id, "name": "n", "color": null, "driver": "postgres",
        "serverVersion": "17", "currentDatabase": "d", "currentUser": "u", "tunnel": null,
        "readOnly": false,
    });
    let info: ConnectionInfo = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(info.capabilities, None);
    assert_eq!(serde_json::to_value(&info).unwrap(), old);
    let caps = SessionCaps::core(Driver::Postgres, crate::sqlgen::Dialect::Postgres, None);
    let with = ConnectionInfo {
        capabilities: Some(caps.clone()),
        ..info
    };
    let back: ConnectionInfo =
        serde_json::from_value(serde_json::to_value(&with).unwrap()).unwrap();
    assert_eq!(back.capabilities, Some(caps));
}
