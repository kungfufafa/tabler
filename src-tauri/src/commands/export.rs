//! Export commands (M6). The work happens in `crate::export::run_export`; these register the
//! export's cancellation token so `cancel_export` can stop it. A multi-table export of another
//! PostgreSQL database of the connection reads it through a session of its own, opened like the
//! connection's and closed afterwards (on MySQL a database is a schema of the same session).

use std::sync::Arc;
use std::time::Duration;

use tauri::State;
use tauri::ipc::Channel;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::commands::browse::cached_meta;
use crate::commands::import::session_on_database;
use crate::drivers::DbSession;
use crate::error::AppError;
use crate::export::results::run_export_results;
use crate::export::run_export;
use crate::export::tables::run_export_tables;
use crate::state::{AppState, Connection};
use crate::types::export::{
    ExportProgress, ExportRequest, ExportResult, ExportResultsRequest, ExportResultsResult,
    ExportSource, ExportTablesProgress, ExportTablesRequest, ExportTablesResult,
};
use crate::types::profile::Driver;
use crate::types::schema::TableSummary;

/// Bound on closing a session opened for another database.
const SESSION_CLOSE: Duration = Duration::from_secs(5);

/// Where tables of `database` are read: the connection's own session, or (PostgreSQL, another
/// database than the current one) a new session on it, to close afterwards (`Some` second).
async fn database_session(
    state: &AppState,
    conn: &Connection,
    database: Option<&str>,
) -> Result<(Arc<dyn DbSession>, bool), AppError> {
    let current = conn.info().await.current_database;
    match database {
        Some(db) if conn.profile.driver == Driver::Postgres && current.as_deref() != Some(db) => {
            Ok((session_on_database(state, conn, Some(db)).await?, true))
        }
        _ => Ok((conn.session().await, false)),
    }
}

async fn close_session(session: &dyn DbSession) {
    if tokio::time::timeout(SESSION_CLOSE, session.close())
        .await
        .is_err()
    {
        log::warn!("export: closing the session of another database timed out");
    }
}

/// The schemas whose tables a database lists in the multi-table export (system schemas left
/// out; MySQL lists the database itself).
fn listed_schema(driver: Driver, name: &str) -> bool {
    match driver {
        Driver::Postgres => !(name.starts_with("pg_") || name == "information_schema"),
        Driver::Mysql | Driver::Sqlite => true,
        other => !other.descriptor().is_system_schema(name),
    }
}

/// The tables and views of one database of the connection for the multi-table export: every
/// user schema's on PostgreSQL (another database through a temporary session), the database's on
/// MySQL (a database is a schema), every attached schema's on SQLite. `None` = the current one.
#[tauri::command]
pub async fn export_database_tables(
    state: State<'_, AppState>,
    connection_id: Uuid,
    database: Option<String>,
) -> Result<Vec<TableSummary>, AppError> {
    let conn = state.connection(connection_id).await?;
    let driver = conn.profile.driver;
    if driver == Driver::Mysql {
        let db = match database {
            Some(db) => db,
            None => conn
                .info()
                .await
                .current_database
                .ok_or_else(|| AppError::InvalidInput("choose a database".into()))?,
        };
        return conn.session().await.list_tables(&db).await;
    }
    let (session, temporary) = database_session(&state, &conn, database.as_deref()).await?;
    let listed = async {
        let mut out = Vec::new();
        for schema in session.list_schemas().await? {
            if listed_schema(driver, &schema.name) {
                out.extend(session.list_tables(&schema.name).await?);
            }
        }
        Ok::<_, AppError>(out)
    }
    .await;
    if temporary {
        close_session(session.as_ref()).await;
    }
    listed
}

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
    let (session, temporary) = database_session(&state, &conn, req.database.as_deref()).await?;
    log::info!(
        "export {} started: {} tables{}",
        req.export_id,
        req.tables.len(),
        if temporary { " (another database)" } else { "" }
    );
    let result = async {
        let mut metas = Vec::with_capacity(req.tables.len());
        for table in &req.tables {
            if token.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            metas.push(if temporary {
                Arc::new(session.table_meta(table).await?)
            } else {
                cached_meta(&conn, table).await?
            });
        }
        run_export_tables(
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
        .await
    }
    .await;
    if temporary {
        close_session(session.as_ref()).await;
    }
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

/// Exports every result of a multi-statement run, as the renderer holds them: one workbook with a
/// sheet per result, one HTML page or SQL script, or a file per result in a folder (CSV, JSON).
/// Progress per result on `on_progress`; `cancel_export` stops it and no file is left behind.
#[tauri::command]
pub async fn export_results(
    state: State<'_, AppState>,
    connection_id: Uuid,
    req: ExportResultsRequest,
    on_progress: Channel<ExportTablesProgress>,
) -> Result<ExportResultsResult, AppError> {
    if req.export_id.trim().is_empty() {
        return Err(AppError::InvalidInput("exportId is required".into()));
    }
    let token = CancellationToken::new();
    let _running = RunningExport::register(&state, &req.export_id, token.clone())?;
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await;
    log::info!(
        "export {} started: {} results",
        req.export_id,
        req.results.len()
    );
    let result = run_export_results(
        session.as_ref(),
        &req,
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
            "export {} finished: {} results, {} rows, {} bytes in {} ms",
            req.export_id,
            r.results.len(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_schemas_are_not_listed() {
        assert!(listed_schema(Driver::Postgres, "public"));
        assert!(listed_schema(Driver::Postgres, "audit"));
        assert!(!listed_schema(Driver::Postgres, "pg_catalog"));
        assert!(!listed_schema(Driver::Postgres, "pg_toast"));
        assert!(!listed_schema(Driver::Postgres, "information_schema"));
        assert!(listed_schema(Driver::Sqlite, "main"));
        // The other drivers: their descriptor's system schemas.
        assert!(listed_schema(Driver::Sqlserver, "dbo"));
        assert!(!listed_schema(Driver::Sqlserver, "sys"));
        assert!(!listed_schema(Driver::Cassandra, "system_schema"));
        assert!(listed_schema(Driver::Cassandra, "shop"));
    }

    /// The PostgreSQL descriptor's patterns say what the PostgreSQL arm does.
    #[test]
    fn the_postgres_descriptor_agrees_with_its_arm() {
        let pg = Driver::Postgres.descriptor();
        for name in [
            "public",
            "pg_catalog",
            "pg_toast",
            "pg_temp_1",
            "information_schema",
        ] {
            assert_eq!(
                listed_schema(Driver::Postgres, name),
                !pg.is_system_schema(name)
            );
        }
    }
}
