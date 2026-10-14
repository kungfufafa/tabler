//! Generate Diagram…: the schema's tables and foreign keys, and the PNG export's file write.

use tauri::State;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use crate::types::diagram::DiagramSchema;

/// Every table and view of `schema` with its columns plus the foreign keys between them, in one
/// catalog round trip.
#[tauri::command]
pub async fn schema_relations(
    state: State<'_, AppState>,
    connection_id: Uuid,
    schema: String,
) -> Result<DiagramSchema, AppError> {
    let session = state.session(connection_id).await?;
    crate::diagram::schema_relations(session.as_ref(), &schema).await
}

/// Writes base64-encoded bytes to `path` (from the native save panel): Export ▸ PNG….
#[tauri::command]
pub async fn write_binary_file(path: String, base64: String) -> Result<(), AppError> {
    crate::diagram::write_base64_file(&path, &base64).await
}
