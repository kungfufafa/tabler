//! Export commands (M6). The work happens in `crate::export::run_export`; these register the
//! export's cancellation token so `cancel_export` can stop it.

use tauri::State;
use tauri::ipc::Channel;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::commands::browse::cached_meta;
use crate::error::AppError;
use crate::export::run_export;
use crate::export::tables::run_export_tables;
use crate::state::AppState;
use crate::types::export::{
    ExportProgress, ExportRequest, ExportResult, ExportSource, ExportTablesProgress,
    ExportTablesRequest, ExportTablesResult,
};

/// Keeps an export's token in `AppState::exports` while it runs.
struct RunningExport<'a> {
    state: &'a AppState,
    id: String,
}

impl<'a> RunningExport<'a> {
    fn register(
        state: &'a AppState,
        id: &str,
        token: CancellationToken,
    ) -> Result<RunningExport<'a>, AppError> {
        let mut exports = state.exports.lock().unwrap_or_else(|p| p.into_inner());
        if exports.contains_key(id) {
            return Err(AppError::InvalidInput(format!(
                "export \"{id}\" is already running"
            )));
        }
        exports.insert(id.to_string(), token);
        Ok(RunningExport {
            state,
            id: id.to_string(),
        })
    }
}

impl Drop for RunningExport<'_> {
    fn drop(&mut self) {
        self.state
            .exports
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.id);
    }
}

/// Streams a table, one SQL statement or rows the renderer holds to `req.path`, reporting
/// progress on `on_progress`.
#[tauri::command]
pub async fn export_data(
    state: State<'_, AppState>,
    connection_id: Uuid,
    req: ExportRequest,
    on_progress: Channel<ExportProgress>,
) -> Result<ExportResult, AppError> {
    if req.export_id.trim().is_empty() {
        return Err(AppError::InvalidInput("exportId is required".into()));
    }
    let token = CancellationToken::new();
    let _running = RunningExport::register(&state, &req.export_id, token.clone())?;
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await;
    let meta = match &req.source {
        ExportSource::Table { table, .. } => Some(cached_meta(&conn, table).await?),
        ExportSource::Sql { .. } | ExportSource::Rows { .. } => None,
    };
    log::info!("export {} started", req.export_id);
    let result = run_export(
        session.as_ref(),
        &req,
        meta.as_deref(),
        move |p| {
            if let Err(e) = on_progress.send(p) {
                log::debug!("export progress not delivered: {e}");
            }
        },
        token,
    )
    .await;
    match &result {
        Ok(r) => log::info!(
            "export {} finished: {} rows, {} bytes in {} ms",
            req.export_id,
            r.rows,
            r.bytes,
            r.duration_ms
        ),
        Err(e) => log::info!("export {} failed: {e}", req.export_id),
    }
    result
}

/// Exports several tables in full: one file per table into a folder, or every table into one
/// SQL script / Excel workbook. Progress per table on `on_progress`; `cancel_export` stops it
/// and no file is left behind.
#[tauri::command]
pub async fn export_tables(
    state: State<'_, AppState>,
    connection_id: Uuid,
    req: ExportTablesRequest,
    on_progress: Channel<ExportTablesProgress>,
) -> Result<ExportTablesResult, AppError> {
    if req.export_id.trim().is_empty() {
        return Err(AppError::InvalidInput("exportId is required".into()));
    }
    let token = CancellationToken::new();
    let _running = RunningExport::register(&state, &req.export_id, token.clone())?;
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await;
    let mut metas = Vec::with_capacity(req.tables.len());
    for table in &req.tables {
        if token.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        metas.push(cached_meta(&conn, table).await?);
    }
    log::info!(
        "export {} started: {} tables",
        req.export_id,
        req.tables.len()
    );
    let result = run_export_tables(
        session.as_ref(),
        &req,
        &metas,
        move |p| {
            if let Err(e) = on_progress.send(p) {
                log::debug!("export progress not delivered: {e}");
            }
        },
        token,
    )
    .await;
    match &result {
        Ok(r) => log::info!(
            "export {} finished: {} tables, {} rows, {} bytes in {} ms",
            req.export_id,
            r.tables.len(),
            r.rows,
            r.bytes,
            r.duration_ms
        ),
        Err(e) => log::info!("export {} failed: {e}", req.export_id),
    }
    result
}

/// Stops a running export (its file is removed). Unknown ids are ignored.
#[tauri::command]
pub async fn cancel_export(state: State<'_, AppState>, export_id: String) -> Result<(), AppError> {
    let token = state
        .exports
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&export_id)
        .cloned();
    if let Some(token) = token {
        token.cancel();
    }
    Ok(())
}
