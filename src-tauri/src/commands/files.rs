//! Small file helpers for the SQL editor (open/save .sql). Paths come from the native dialog.

use crate::error::AppError;

#[tauri::command]
pub async fn read_text_file(path: String) -> Result<String, AppError> {
    tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| AppError::Io(format!("{path}: {e}")))
}

#[tauri::command]
pub async fn write_text_file(path: String, content: String) -> Result<(), AppError> {
    tokio::fs::write(&path, content)
        .await
        .map_err(|e| AppError::Io(format!("{path}: {e}")))
}
