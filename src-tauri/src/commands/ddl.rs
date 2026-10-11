//! DDL commands (M5). Stubs keep the command names registered; the DDL agent fills them in.

use tauri::State;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use crate::types::ddl::{DdlOp, DdlPlan, DdlResult};

#[tauri::command]
pub async fn preview_ddl(
    _state: State<'_, AppState>,
    _connection_id: Uuid,
    _ops: Vec<DdlOp>,
) -> Result<DdlPlan, AppError> {
    Err(AppError::Unsupported(
        "DDL preview is not implemented yet".into(),
    ))
}

#[tauri::command]
pub async fn apply_ddl(
    _state: State<'_, AppState>,
    _connection_id: Uuid,
    _ops: Vec<DdlOp>,
) -> Result<DdlResult, AppError> {
    Err(AppError::Unsupported(
        "DDL apply is not implemented yet".into(),
    ))
}

#[tauri::command]
pub async fn get_table_ddl(
    _state: State<'_, AppState>,
    _connection_id: Uuid,
    _table: crate::types::schema::TableRef,
) -> Result<Option<String>, AppError> {
    Err(AppError::Unsupported(
        "table DDL is not implemented yet".into(),
    ))
}
