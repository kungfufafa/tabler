use tauri::State;

use crate::error::AppError;
use crate::state::AppState;

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<serde_json::Value, AppError> {
    Ok(serde_json::Value::Object(state.appdb.get_settings()?))
}

#[tauri::command]
pub async fn set_setting(
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> Result<(), AppError> {
    state.appdb.set_setting(&key, &value)
}
