//! Metrics Board commands: the boards in the app database, their `.tablerboard` files and the
//! object queries (see `crate::metrics`).

use tauri::State;
use uuid::Uuid;

use crate::drivers::CancelSlot;
use crate::error::AppError;
use crate::metrics;
use crate::state::AppState;
use crate::types::caps::Feature;
use crate::types::metrics::{
    MetricsBoard, MetricsBoardInput, MetricsImport, MetricsQuery, MetricsResult,
};

/// The boards of a connection profile plus those of every connection (`None`: all boards).
#[tauri::command]
pub async fn metrics_list_boards(
    state: State<'_, AppState>,
    profile_id: Option<Uuid>,
) -> Result<Vec<MetricsBoard>, AppError> {
    metrics::list_boards(&state.appdb, profile_id)
}

#[tauri::command]
pub async fn metrics_save_board(
    state: State<'_, AppState>,
    input: MetricsBoardInput,
) -> Result<MetricsBoard, AppError> {
    metrics::save_board(&state.appdb, &input)
}

#[tauri::command]
pub async fn metrics_delete_board(state: State<'_, AppState>, id: Uuid) -> Result<(), AppError> {
    metrics::delete_board(&state.appdb, id)?;
    Ok(())
}

/// Export Board…: writes the boards to `path` (a `.tablerboard` file), encrypted with
/// `password` when one is given. Returns how many boards were written.
#[tauri::command]
pub async fn metrics_export_boards(
    state: State<'_, AppState>,
    ids: Vec<Uuid>,
    path: String,
    password: Option<String>,
) -> Result<u32, AppError> {
    if path.trim().is_empty() {
        return Err(AppError::InvalidInput("no file chosen".into()));
    }
    let records = metrics::export_records(&state.appdb, &ids)?;
    if records.is_empty() {
        return Err(AppError::NotFound("board".into()));
    }
    let exported_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let bytes = tokio::task::spawn_blocking(move || {
        metrics::write_file(&records, password.as_deref(), &exported_at)
            .map(|bytes| (bytes, records.len()))
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))??;
    tokio::fs::write(&path, &bytes.0).await?;
    Ok(bytes.1 as u32)
}

/// Import Board…: adds the boards of a `.tablerboard` file for `profile_id` (`None`: every
/// connection). An encrypted file without `password` comes back as `needsPassword`.
#[tauri::command]
pub async fn metrics_import_boards(
    state: State<'_, AppState>,
    path: String,
    password: Option<String>,
    profile_id: Option<Uuid>,
) -> Result<MetricsImport, AppError> {
    let bytes = tokio::fs::read(&path).await?;
    let outcome =
        tokio::task::spawn_blocking(move || metrics::read_file(&bytes, password.as_deref()))
            .await
            .map_err(|e| AppError::Internal(e.to_string()))??;
    match outcome {
        metrics::ReadOutcome::NeedsPassword => Ok(MetricsImport {
            needs_password: true,
            boards: Vec::new(),
        }),
        metrics::ReadOutcome::Opened(records) => Ok(MetricsImport {
            needs_password: false,
            boards: metrics::import_records(&state.appdb, &records, profile_id)?,
        }),
    }
}

/// Registers a running object query under (connection, query id) so `cancel_query` can stop it;
/// removed on drop.
struct Running<'a> {
    state: &'a AppState,
    key: (Uuid, String),
}

impl<'a> Running<'a> {
    fn register(
        state: &'a AppState,
        connection_id: Uuid,
        query_id: String,
        slot: CancelSlot,
    ) -> Result<Running<'a>, AppError> {
        let key = (connection_id, query_id);
        let mut active = state
            .active_queries
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if active.contains_key(&key) {
            return Err(AppError::InvalidInput(format!(
                "query \"{}\" is already running",
                key.1
            )));
        }
        active.insert(key.clone(), slot);
        Ok(Running { state, key })
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.state
            .active_queries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.key);
    }
}

/// Runs one board object's query (read-only unless `allowWrites`; refused on a read-only
/// connection when it writes). Cancellable with `cancel_query` under `queryId`, a write as well
/// as a read. A statement that is still running after the object's `timeoutSecs` (default 60 s,
/// 5 to 600) is cancelled the same way and fails with "Query timed out after N s": nothing of it
/// outlives the command, so closing the board's tab (which cancels its runs) or the window leaves
/// no timer or statement behind.
#[tauri::command]
pub async fn metrics_run(
    state: State<'_, AppState>,
    connection_id: Uuid,
    query_id: String,
    query: MetricsQuery,
) -> Result<MetricsResult, AppError> {
    metrics_run_on(&state, connection_id, query_id, query).await
}

/// The body of [`metrics_run`], callable without a Tauri runtime.
pub(crate) async fn metrics_run_on(
    state: &AppState,
    connection_id: Uuid,
    query_id: String,
    query: MetricsQuery,
) -> Result<MetricsResult, AppError> {
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await?;
    let dialect = session.core_dialect(Feature::Metrics)?;
    let prepared = metrics::prepare(dialect, &query)?;
    crate::commands::safety::refuse_read_only_run(&conn, &prepared.sql, dialect.into(), None)?;
    let slot = CancelSlot::default();
    let _running = Running::register(state, connection_id, query_id, slot.clone())?;
    let result = metrics::run_with_timeout(
        session.as_ref(),
        &prepared,
        metrics::row_cap(query.max_rows),
        slot,
        metrics::query_timeout(query.timeout_secs),
    )
    .await?;
    if prepared.writes && crate::commands::query::changes_schema(&prepared.sql) {
        conn.invalidate(None).await;
    }
    Ok(result)
}
