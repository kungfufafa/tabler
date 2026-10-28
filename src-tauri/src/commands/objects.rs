//! Sidebar object commands (M8): routines, sequences, system schemas, the Item Overview and the
//! sidebar's object operations. Logic lives in `crate::objects` and `sqlgen::objects`.

use tauri::State;
use uuid::Uuid;

use crate::commands::ddl::apply_core_plan;
use crate::error::AppError;
use crate::objects;
use crate::sqlgen::Dialect;
use crate::state::{AppState, Connection};
use crate::types::ddl::{DdlPlan, DdlResult};
use crate::types::objects::{
    DatabaseItems, ObjectOp, OverviewItem, RoutineRef, RoutineSummary, SequenceSummary,
};
use crate::types::schema::{SchemaInfo, TableRef};

#[tauri::command]
pub async fn list_routines(
    state: State<'_, AppState>,
    connection_id: Uuid,
    schema: String,
) -> Result<Vec<RoutineSummary>, AppError> {
    let session = state.session(connection_id).await?;
    objects::list_routines(session.as_ref(), &schema).await
}

#[tauri::command]
pub async fn list_sequences(
    state: State<'_, AppState>,
    connection_id: Uuid,
    schema: String,
) -> Result<Vec<SequenceSummary>, AppError> {
    let session = state.session(connection_id).await?;
    objects::list_sequences(session.as_ref(), &schema).await
}

#[tauri::command]
pub async fn list_system_schemas(
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<Vec<SchemaInfo>, AppError> {
    let session = state.session(connection_id).await?;
    objects::system_schemas(session.as_ref()).await
}

/// Every table, view and routine of the connection's database, across its schemas (the
/// sidebar's Search in Database…). `include_system`: PostgreSQL's system schemas too.
#[tauri::command]
pub async fn list_database_items(
    state: State<'_, AppState>,
    connection_id: Uuid,
    include_system: Option<bool>,
) -> Result<DatabaseItems, AppError> {
    let session = state.session(connection_id).await?;
    objects::database_items(session.as_ref(), include_system.unwrap_or(false)).await
}

#[tauri::command]
pub async fn get_routine_definition(
    state: State<'_, AppState>,
    connection_id: Uuid,
    routine: RoutineRef,
) -> Result<String, AppError> {
    let session = state.session(connection_id).await?;
    objects::routine_definition(session.as_ref(), &routine).await
}

#[tauri::command]
pub async fn get_sequence_definition(
    state: State<'_, AppState>,
    connection_id: Uuid,
    sequence: TableRef,
) -> Result<String, AppError> {
    let session = state.session(connection_id).await?;
    objects::sequence_definition(session.as_ref(), &sequence).await
}

#[tauri::command]
pub async fn get_item_overview(
    state: State<'_, AppState>,
    connection_id: Uuid,
    schema: String,
) -> Result<Vec<OverviewItem>, AppError> {
    let session = state.session(connection_id).await?;
    objects::item_overview(session.as_ref(), &schema).await
}

#[tauri::command]
pub async fn preview_object_op(
    state: State<'_, AppState>,
    connection_id: Uuid,
    op: ObjectOp,
) -> Result<DdlPlan, AppError> {
    let session = state.session(connection_id).await?;
    objects::plan_op(session.as_ref(), &op).await
}

/// Plans and runs an operation. PG and SQLite plans are transactional (`commands::ddl`'s
/// executor, with its lock timeouts); MySQL plans run on one session (`run_mysql_plan`). A
/// failure comes back in `DdlResult.error`.
#[tauri::command]
pub async fn apply_object_op(
    state: State<'_, AppState>,
    connection_id: Uuid,
    op: ObjectOp,
) -> Result<DdlResult, AppError> {
    let conn = state.connection(connection_id).await?;
    apply_object_op_on(&conn, &op).await
}

/// The body of [`apply_object_op`] on an open connection. The operation needs its own feature
/// only (`objects::op_feature`), as its preview does.
pub(crate) async fn apply_object_op_on(
    conn: &Connection,
    op: &ObjectOp,
) -> Result<DdlResult, AppError> {
    if conn.profile.options.read_only {
        return Err(AppError::Unsupported("this connection is read-only".into()));
    }
    let session = conn.session().await?;
    let plan = objects::plan_op(session.as_ref(), op).await?;
    log::info!("apply_object_op {} statement(s)", plan.statements.len());
    let result = match session.core_dialect(objects::op_feature(op))? {
        Dialect::Mysql => objects::run_mysql_plan(session.as_ref(), &plan).await,
        d @ (Dialect::Postgres | Dialect::Sqlite) => {
            apply_core_plan(session.as_ref(), d, &plan).await
        }
    };
    conn.invalidate(None).await;
    if let Some(err) = &result.error {
        log::info!(
            "apply_object_op failed at statement {:?}: {}",
            result.failed_index,
            err.message
        );
    }
    Ok(result)
}
