//! SQL editor commands (M4). Stubs keep the command names registered; the editor agent fills them in.

use tauri::State;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

#[tauri::command]
pub async fn run_sql(
    _state: State<'_, AppState>,
    _connection_id: Uuid,
    _query_id: String,
    _sql: String,
    _options: serde_json::Value,
    _on_event: tauri::ipc::Channel<serde_json::Value>,
) -> Result<serde_json::Value, AppError> {
    Err(AppError::Unsupported(
        "run_sql is not implemented yet".into(),
    ))
}

#[tauri::command]
pub async fn cancel_query(
    _state: State<'_, AppState>,
    _connection_id: Uuid,
    _query_id: String,
) -> Result<(), AppError> {
    Err(AppError::Unsupported(
        "cancel_query is not implemented yet".into(),
    ))
}

#[tauri::command]
pub async fn explain(
    _state: State<'_, AppState>,
    _connection_id: Uuid,
    _sql: String,
    _mode: String,
) -> Result<serde_json::Value, AppError> {
    Err(AppError::Unsupported(
        "explain is not implemented yet".into(),
    ))
}
