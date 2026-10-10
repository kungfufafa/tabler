use std::sync::Arc;

use tauri::State;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};

#[tauri::command]
pub async fn list_databases(
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<Vec<DatabaseInfo>, AppError> {
    state.session(connection_id).await?.list_databases().await
}

#[tauri::command]
pub async fn list_schemas(
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<Vec<SchemaInfo>, AppError> {
    state.session(connection_id).await?.list_schemas().await
}

#[tauri::command]
pub async fn list_tables(
    state: State<'_, AppState>,
    connection_id: Uuid,
    schema: String,
) -> Result<Vec<TableSummary>, AppError> {
    state
        .session(connection_id)
        .await?
        .list_tables(&schema)
        .await
}

/// Table metadata, cached per connection until `refresh` or invalidation.
#[tauri::command]
pub async fn get_table(
    state: State<'_, AppState>,
    connection_id: Uuid,
    table: TableRef,
    refresh: Option<bool>,
) -> Result<TableMeta, AppError> {
    let conn = state.connection(connection_id).await?;
    if !refresh.unwrap_or(false)
        && let Some(meta) = conn.schema_cache.read().await.get(&table)
    {
        return Ok((**meta).clone());
    }
    let meta = conn.session().await.table_meta(&table).await?;
    conn.schema_cache
        .write()
        .await
        .insert(table, Arc::new(meta.clone()));
    Ok(meta)
}

#[tauri::command]
pub async fn get_view_definition(
    state: State<'_, AppState>,
    connection_id: Uuid,
    table: TableRef,
) -> Result<Option<String>, AppError> {
    state
        .session(connection_id)
        .await?
        .view_definition(&table)
        .await
}

#[tauri::command]
pub async fn get_completion_metadata(
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<CompletionMetadata, AppError> {
    state
        .session(connection_id)
        .await?
        .completion_metadata()
        .await
}

#[tauri::command]
pub async fn invalidate_schema_cache(
    state: State<'_, AppState>,
    connection_id: Uuid,
    table: Option<TableRef>,
) -> Result<(), AppError> {
    state
        .connection(connection_id)
        .await?
        .invalidate(table.as_ref())
        .await;
    Ok(())
}
