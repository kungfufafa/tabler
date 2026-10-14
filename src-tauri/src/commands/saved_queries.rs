//! Saved queries (the sidebar's Queries pane) and their folders: `.sql` files in the queries
//! folder (Settings ▸ Locations), see `query_files`. The first listing moves the queries the app
//! DB still holds into that folder.

use std::path::{Path, PathBuf};

use tauri::State;
use uuid::Uuid;

use crate::error::AppError;
use crate::locations::Locations;
use crate::query_files;
use crate::state::AppState;
use crate::types::saved_query::{
    QueryFolder, QueryFolderInput, SavedQuery, SavedQueryInput, SavedQueryLibrary,
};

/// The queries folder, after the one-time migration from the app DB. The migration is best
/// effort: when it cannot run (the folder cannot be created, the app DB is busy) every command
/// still works on the files that are there, and the next command tries again (it never writes
/// a query twice, see `query_files::migrate_library`).
fn root(state: &AppState, locations: &Locations) -> Result<PathBuf, AppError> {
    let root = locations.queries_dir();
    if let Err(e) = query_files::migrate_from_appdb(&state.appdb, &root) {
        log::warn!("queries: moving the saved queries out of the app database failed: {e}");
    }
    Ok(root)
}

/// Every folder and saved query (the renderer filters them per connection).
#[tauri::command]
pub async fn list_saved_queries(
    state: State<'_, AppState>,
    locations: State<'_, Locations>,
) -> Result<SavedQueryLibrary, AppError> {
    query_files::list(&root(&state, &locations)?)
}

/// Creates or replaces a saved query; returns it as stored.
#[tauri::command]
pub async fn save_saved_query(
    state: State<'_, AppState>,
    locations: State<'_, Locations>,
    input: SavedQueryInput,
) -> Result<SavedQuery, AppError> {
    query_files::save_query(&root(&state, &locations)?, &input)
}

#[tauri::command]
pub async fn delete_saved_query(
    state: State<'_, AppState>,
    locations: State<'_, Locations>,
    id: Uuid,
) -> Result<(), AppError> {
    query_files::delete_query(&root(&state, &locations)?, id)?;
    Ok(())
}

/// Creates, renames or moves a folder; returns it as stored (its id follows its path).
#[tauri::command]
pub async fn save_query_folder(
    state: State<'_, AppState>,
    locations: State<'_, Locations>,
    input: QueryFolderInput,
) -> Result<QueryFolder, AppError> {
    query_files::save_folder(&root(&state, &locations)?, &input)
}

/// Deletes a folder's queries together with the folders inside it.
#[tauri::command]
pub async fn delete_query_folder(
    state: State<'_, AppState>,
    locations: State<'_, Locations>,
    id: Uuid,
) -> Result<(), AppError> {
    query_files::delete_folder(&root(&state, &locations)?, id)?;
    Ok(())
}

/// "Import…": `.sql` / `.sql.gz` files become new queries in `folderId` (null: the top level).
#[tauri::command]
pub async fn import_saved_queries(
    state: State<'_, AppState>,
    locations: State<'_, Locations>,
    paths: Vec<String>,
    folder_id: Option<Uuid>,
) -> Result<Vec<SavedQuery>, AppError> {
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    query_files::import_files(&root(&state, &locations)?, &paths, folder_id)
}

/// "Add Folder to Favorites…": copies a folder of `.sql` files into the queries folder.
#[tauri::command]
pub async fn add_query_folder_from_disk(
    state: State<'_, AppState>,
    locations: State<'_, Locations>,
    path: String,
    parent_id: Option<Uuid>,
) -> Result<QueryFolder, AppError> {
    query_files::add_folder(&root(&state, &locations)?, Path::new(&path), parent_id)
}

/// Show in Finder: a query's file, a folder, or (`id` null) the queries folder itself.
#[tauri::command]
pub async fn reveal_saved_query(
    state: State<'_, AppState>,
    locations: State<'_, Locations>,
    id: Option<Uuid>,
    folder: bool,
) -> Result<(), AppError> {
    let root = root(&state, &locations)?;
    let path = match id {
        Some(id) => query_files::path_of(&root, id, folder)?,
        None => {
            std::fs::create_dir_all(&root)?;
            root
        }
    };
    super::locations::reveal(&path)
}
