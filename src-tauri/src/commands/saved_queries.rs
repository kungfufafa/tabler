//! Saved queries (the sidebar's Queries pane) and their folders, stored in the app DB.

use tauri::State;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use crate::types::saved_query::{
    QueryFolder, QueryFolderInput, SavedQuery, SavedQueryInput, SavedQueryLibrary,
};

/// Every folder and saved query (the renderer filters them per connection).
#[tauri::command]
pub async fn list_saved_queries(state: State<'_, AppState>) -> Result<SavedQueryLibrary, AppError> {
    state.appdb.list_saved_queries()
}

/// Creates or replaces a saved query; returns it as stored.
#[tauri::command]
pub async fn save_saved_query(
    state: State<'_, AppState>,
    input: SavedQueryInput,
) -> Result<SavedQuery, AppError> {
    state.appdb.save_saved_query(&input)
}

#[tauri::command]
pub async fn delete_saved_query(state: State<'_, AppState>, id: Uuid) -> Result<(), AppError> {
    state.appdb.delete_saved_query(id)?;
    Ok(())
}

/// Creates, renames or moves a folder; returns it as stored.
#[tauri::command]
pub async fn save_query_folder(
    state: State<'_, AppState>,
    input: QueryFolderInput,
) -> Result<QueryFolder, AppError> {
    state.appdb.save_query_folder(&input)
}

/// Deletes a folder together with the folders and queries inside it.
#[tauri::command]
pub async fn delete_query_folder(state: State<'_, AppState>, id: Uuid) -> Result<(), AppError> {
    state.appdb.delete_query_folder(id)?;
    Ok(())
}
