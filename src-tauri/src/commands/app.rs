use crate::types::app::AppInfo;

#[tauri::command]
pub fn app_info(app: tauri::AppHandle) -> AppInfo {
    log::debug!("renderer ready: app_info requested");
    AppInfo {
        version: app.package_info().version.to_string(),
        platform: std::env::consts::OS.to_string(),
        debug: cfg!(debug_assertions),
    }
}
