//! Server administration: the ⌘K database list (create / rename / drop databases), PG schemas,
//! Connection ▸ Process List… and Connection ▸ User Management…. Logic lives in `crate::admin`.

use tauri::State;
use uuid::Uuid;

use crate::admin;
use crate::error::AppError;
use crate::state::{AppState, Connection};
use crate::types::admin::{
    AdminDdl, AdminResult, DatabaseOptions, KillOutcome, KillScope, ProcessRow, UserChange,
    UserDetail, UserRef, UserSummary,
};

fn writable(conn: &Connection) -> Result<(), AppError> {
    if conn.profile.options.read_only {
        return Err(AppError::Unsupported("this connection is read-only".into()));
    }
    Ok(())
}

/// Encodings, collations and templates for the Create Database dialog.
#[tauri::command]
pub async fn admin_database_options(
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<DatabaseOptions, AppError> {
    let session = state.session(connection_id).await?;
    admin::database_options(session.as_ref()).await
}

#[tauri::command]
pub async fn admin_preview_ddl(
    state: State<'_, AppState>,
    connection_id: Uuid,
    op: AdminDdl,
) -> Result<Vec<String>, AppError> {
    let session = state.session(connection_id).await?;
    admin::preview_ddl(session.core_dialect(admin::ddl_feature(&op))?, &op)
}

/// Creates / renames / drops a database or a schema. The database the connection uses cannot
/// be renamed or dropped.
#[tauri::command]
pub async fn admin_apply_ddl(
    state: State<'_, AppState>,
    connection_id: Uuid,
    op: AdminDdl,
) -> Result<AdminResult, AppError> {
    let conn = state.connection(connection_id).await?;
    writable(&conn)?;
    if let AdminDdl::RenameDatabase { name, .. } | AdminDdl::DropDatabase { name } = &op
        && conn.info().await.current_database.as_deref() == Some(name.as_str())
    {
        return Err(AppError::InvalidInput(format!(
            "{name} is the database this connection is using: switch to another database first"
        )));
    }
    let session = conn.session().await;
    let result = admin::apply_ddl(session.as_ref(), &op).await?;
    if matches!(op, AdminDdl::DropSchema { .. }) {
        conn.invalidate(None).await;
    }
    log::info!(
        "admin ddl: {} statement(s) in {} ms{}",
        result.executed.len(),
        result.duration_ms,
        if result.error.is_some() {
            ", failed"
        } else {
            ""
        }
    );
    Ok(result)
}

/// Sessions of the server (Running Queries).
#[tauri::command]
pub async fn admin_list_processes(
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<Vec<ProcessRow>, AppError> {
    let session = state.session(connection_id).await?;
    admin::list_processes(session.as_ref()).await
}

/// Kill Query / Kill Connection, one outcome per id.
#[tauri::command]
pub async fn admin_kill_processes(
    state: State<'_, AppState>,
    connection_id: Uuid,
    ids: Vec<i64>,
    scope: KillScope,
) -> Result<Vec<KillOutcome>, AppError> {
    let conn = state.connection(connection_id).await?;
    writable(&conn)?;
    let session = conn.session().await;
    let outcomes = admin::kill(session.as_ref(), &ids, scope).await?;
    log::info!(
        "admin kill {scope:?}: {} of {} done",
        outcomes.iter().filter(|o| o.ok).count(),
        outcomes.len()
    );
    Ok(outcomes)
}

#[tauri::command]
pub async fn admin_list_users(
    state: State<'_, AppState>,
    connection_id: Uuid,
    include_nologin: bool,
) -> Result<Vec<UserSummary>, AppError> {
    let session = state.session(connection_id).await?;
    admin::list_users(session.as_ref(), include_nologin).await
}

#[tauri::command]
pub async fn admin_get_user(
    state: State<'_, AppState>,
    connection_id: Uuid,
    user: UserRef,
) -> Result<UserDetail, AppError> {
    let session = state.session(connection_id).await?;
    admin::load_user(session.as_ref(), &user).await
}

/// The statements a user change runs on this connection (MySQL literals per its `sql_mode`),
/// passwords masked.
#[tauri::command]
pub async fn admin_preview_user(
    state: State<'_, AppState>,
    connection_id: Uuid,
    change: UserChange,
) -> Result<Vec<String>, AppError> {
    let session = state.session(connection_id).await?;
    admin::preview_user_on(session.as_ref(), &change).await
}

/// Applies a user change. Not `run_sql`: nothing of it reaches the query history, and the
/// result only carries the masked statements.
#[tauri::command]
pub async fn admin_apply_user(
    state: State<'_, AppState>,
    connection_id: Uuid,
    change: UserChange,
) -> Result<AdminResult, AppError> {
    let conn = state.connection(connection_id).await?;
    writable(&conn)?;
    let session = conn.session().await;
    let result = admin::apply_user(session.as_ref(), &change).await?;
    log::info!(
        "admin users: {} statement(s) in {} ms{}",
        result.executed.len(),
        result.duration_ms,
        if result.error.is_some() {
            ", failed"
        } else {
            ""
        }
    );
    Ok(result)
}
