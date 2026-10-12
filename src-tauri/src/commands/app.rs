use tauri::{AppHandle, State};

use crate::shell::QuitState;
use crate::types::app::AppInfo;

#[tauri::command]
pub fn app_info(app: AppHandle) -> AppInfo {
    log::debug!("renderer ready: app_info requested");
    AppInfo {
        version: app.package_info().version.to_string(),
        platform: std::env::consts::OS.to_string(),
        debug: cfg!(debug_assertions),
    }
}

/// Quits for real: the renderer calls this once nothing unsaved is left (or the user chose to
/// discard it), so the exit guard lets the request through.
#[tauri::command]
pub fn quit_app(app: AppHandle, quit: State<'_, QuitState>) {
    log::info!("quit confirmed by the renderer");
    quit.confirm();
    app.exit(0);
}

/// The renderer reports whether any tab holds unsaved work, so a Dock / system quit can be
/// answered right away; it also acknowledges a pending `app:quit-requested`.
#[tauri::command]
pub fn set_unsaved(unsaved: bool, quit: State<'_, QuitState>) {
    quit.set_unsaved(unsaved);
}
