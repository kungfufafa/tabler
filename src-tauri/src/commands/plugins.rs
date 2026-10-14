//! Plugins ▸ Manage Plugins… (⌘L): list / install / remove plugins, read a command's script for the
//! renderer's sandbox, open the Plugins folder, and rebuild the native Plugins menu.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::error::AppError;
use crate::plugins;
use crate::types::plugins::{InstalledPlugin, PluginList, PluginMenuEntry};

fn dir(app: &AppHandle) -> Result<PathBuf, AppError> {
    let data = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(plugins::plugins_dir(&data))
}

#[tauri::command]
pub async fn plugins_list(app: AppHandle) -> Result<PluginList, AppError> {
    let dir = dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || plugins::list(&dir))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// Install from Folder…: copies the chosen folder into the Plugins folder.
#[tauri::command]
pub async fn plugins_install(app: AppHandle, path: String) -> Result<InstalledPlugin, AppError> {
    let dir = dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        plugins::install_from_folder(&dir, Path::new(&path))
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
}

#[tauri::command]
pub async fn plugins_remove(app: AppHandle, identifier: String) -> Result<(), AppError> {
    let dir = dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || plugins::remove(&dir, &identifier))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
}

/// The text of a command's script (the renderer runs it in a sandboxed worker).
#[tauri::command]
pub async fn plugins_read_script(
    app: AppHandle,
    identifier: String,
    script: String,
) -> Result<String, AppError> {
    let dir = dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || plugins::read_script(&dir, &identifier, &script))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
}

/// Open Plugins Folder (created when missing).
#[tauri::command]
pub async fn plugins_reveal_folder(app: AppHandle) -> Result<(), AppError> {
    plugins::menu::reveal_folder(&app)
}

/// The enabled plugins' commands for the native Plugins menu.
#[tauri::command]
pub fn plugins_set_menu(app: AppHandle, entries: Vec<PluginMenuEntry>) -> Result<(), AppError> {
    plugins::menu::set_entries(&app, &entries)
}
