//! Query history commands: every statement run from the SQL editor is recorded by `run_sql`.

use tauri::State;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use crate::types::history::{HistoryEntry, HistoryFilter};

/// History rows, newest first (default 200 per page; page with `beforeId`).
#[tauri::command]
pub async fn list_history(
    state: State<'_, AppState>,
    filter: Option<HistoryFilter>,
) -> Result<Vec<HistoryEntry>, AppError> {
    state.appdb.list_history(&filter.unwrap_or_default())
}

#[tauri::command]
pub async fn delete_history(state: State<'_, AppState>, ids: Vec<i64>) -> Result<(), AppError> {
    state.appdb.delete_history(&ids)?;
    Ok(())
}

/// Clears one profile's history, or everything when `profileId` is omitted.
#[tauri::command]
pub async fn clear_history(
    state: State<'_, AppState>,
    profile_id: Option<Uuid>,
) -> Result<(), AppError> {
    state.appdb.clear_history(profile_id)?;
    Ok(())
}
