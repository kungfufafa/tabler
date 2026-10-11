//! Export commands (M6). Stubs keep the command names registered.

use tauri::State;

use crate::error::AppError;
use crate::state::AppState;
use crate::types::export::{ExportProgress, ExportResult};

#[tauri::command]
pub async fn export_data(
    _state: State<'_, AppState>,
    _connection_id: uuid::Uuid,
    _req: serde_json::Value,
    _on_progress: tauri::ipc::Channel<ExportProgress>,
) -> Result<ExportResult, AppError> {
    Err(AppError::Unsupported(
        "export is not implemented yet".into(),
    ))
}

#[tauri::command]
pub async fn cancel_export(
    _state: State<'_, AppState>,
    _export_id: String,
) -> Result<(), AppError> {
    Ok(())
}
