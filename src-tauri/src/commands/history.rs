//! Query history commands (M4). Stubs; the editor agent fills them in.

use tauri::State;

use crate::error::AppError;
use crate::state::AppState;
use crate::types::history::{HistoryEntry, HistoryFilter};

#[tauri::command]
pub async fn list_history(
    _state: State<'_, AppState>,
    _filter: HistoryFilter,
) -> Result<Vec<HistoryEntry>, AppError> {
    Ok(vec![])
}

#[tauri::command]
pub async fn delete_history(_state: State<'_, AppState>, _ids: Vec<i64>) -> Result<(), AppError> {
    Ok(())
}

#[tauri::command]
pub async fn clear_history(
    _state: State<'_, AppState>,
    _profile_id: Option<String>,
) -> Result<(), AppError> {
    Ok(())
}
