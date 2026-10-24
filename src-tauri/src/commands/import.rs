//! Import commands (File ▸ Import ▸ From CSV… / From JSON… / From SQL Dump…). The work happens in
//! `crate::import`; these resolve the connection, register the import's cancellation so
//! `cancel_import` can stop it, and drop cached metadata the import may have changed. A read-only
//! connection refuses both. A SQL dump runs on a connection of its own, opened like the
//! connection's (profile, current database, SSH tunnel) and closed afterwards, so its `SET`,
//! `USE` and transactions never change the SQL editor's session.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use tauri::State;
use tauri::ipc::Channel;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::commands::browse::cached_meta;
use crate::commands::connections::{connect_password, connect_timeout};
use crate::drivers::{self, CancelSlot, ConnectTarget, DbSession, request_cancel};
use crate::error::AppError;
use crate::import::{
    preview_csv, preview_dump, preview_json, run_csv_import, run_json_import, run_sql_import,
};
use crate::state::{AppState, Connection};
use crate::types::driver::Location;
use crate::types::import::{
    CsvImportProgress, CsvImportReport, CsvImportRequest, CsvPreview, CsvPreviewRequest,
    ImportTarget, JsonImportRequest, JsonPreview, JsonPreviewRequest, SqlDumpPreview,
    SqlDumpPreviewRequest, SqlImportProgress, SqlImportReport, SqlImportRequest,
};

struct Running {
    connection_id: Uuid,
    token: CancellationToken,
    /// The SQL dump's statement slot (CSV imports stop between chunks).
    slot: Option<CancelSlot>,
    /// The SQL dump's own session, once it is open (its statements are cancelled there).
    session: Option<Arc<dyn DbSession>>,
}

static IMPORTS: LazyLock<Mutex<HashMap<String, Running>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn imports() -> std::sync::MutexGuard<'static, HashMap<String, Running>> {
    IMPORTS.lock().unwrap_or_else(|p| p.into_inner())
}

/// Keeps an import registered while it runs.
struct Registration(String);

impl Registration {
    fn new(id: &str, running: Running) -> Result<Registration, AppError> {
        if id.trim().is_empty() {
            return Err(AppError::InvalidInput("importId is required".into()));
        }
        let mut all = imports();
        if all.contains_key(id) {
            return Err(AppError::InvalidInput(format!(
                "import \"{id}\" is already running"
            )));
        }
        all.insert(id.to_string(), running);
        Ok(Registration(id.to_string()))
    }
}

impl Registration {
    fn set_session(&self, session: Arc<dyn DbSession>) {
        if let Some(r) = imports().get_mut(&self.0) {
            r.session = Some(session);
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        imports().remove(&self.0);
    }
}

/// The CSV wizard's preview: resolved delimiter / encoding, the first rows, detected types.
#[tauri::command]
pub async fn preview_csv_import(req: CsvPreviewRequest) -> Result<CsvPreview, AppError> {
    tokio::task::spawn_blocking(move || preview_csv(&req.path, &req.format))
        .await
        .map_err(|e| AppError::internal(format!("preview task failed: {e}")))?
}

/// Imports a CSV file into a table (or a table it creates), reporting progress on
/// `on_progress`. Failures while importing come back in the report, with what was committed.
#[tauri::command]
pub async fn import_csv(
    state: State<'_, AppState>,
    connection_id: Uuid,
    req: CsvImportRequest,
    on_progress: Channel<CsvImportProgress>,
) -> Result<CsvImportReport, AppError> {
    let conn = state.connection(connection_id).await?;
    refuse_read_only(&conn)?;
    let token = CancellationToken::new();
    let _registered = Registration::new(
        &req.import_id,
        Running {
            connection_id,
            token: token.clone(),
            slot: None,
            session: None,
        },
    )?;
    let session = conn.session().await;
    let (table, meta) = match &req.target {
        ImportTarget::Existing { table } => {
            // The structure as it is now: the import must not bind against stale columns.
            conn.invalidate(Some(table)).await;
            (table.clone(), Some(cached_meta(&conn, table).await?))
        }
        ImportTarget::Create { table, .. } => (table.clone(), None),
    };
    log::info!("import {} (CSV) into {} started", req.import_id, table.name);
    let report = run_csv_import(
        session.as_ref(),
        &req,
        meta.as_deref(),
        move |p| {
            if let Err(e) = on_progress.send(p) {
                log::debug!("import progress not delivered: {e}");
            }
        },
        token,
    )
    .await;
    conn.invalidate(Some(&table)).await;
    match &report {
        Ok(r) => log::info!(
            "import {} finished: {} of {} rows in {} ms{}",
            req.import_id,
            r.rows_imported,
            r.rows_read,
            r.duration_ms,
            r.failure
                .as_ref()
                .map(|f| format!(", failed: {}", f.message))
                .unwrap_or_default()
        ),
        Err(e) => log::info!("import {} failed: {e}", req.import_id),
    }
    report
}

/// The JSON import's preview: the keys of the first objects as columns, their detected types and
/// the first rows.
#[tauri::command]
pub async fn preview_json_import(req: JsonPreviewRequest) -> Result<JsonPreview, AppError> {
    tokio::task::spawn_blocking(move || preview_json(&req.path, req.encoding))
        .await
        .map_err(|e| AppError::internal(format!("preview task failed: {e}")))?
}

/// Imports a JSON array of objects (or NDJSON) into a table or a new one, as `import_csv` does.
#[tauri::command]
pub async fn import_json(
    state: State<'_, AppState>,
    connection_id: Uuid,
    req: JsonImportRequest,
    on_progress: Channel<CsvImportProgress>,
) -> Result<CsvImportReport, AppError> {
    let conn = state.connection(connection_id).await?;
    refuse_read_only(&conn)?;
    let token = CancellationToken::new();
    let _registered = Registration::new(
        &req.import_id,
        Running {
            connection_id,
            token: token.clone(),
            slot: None,
            session: None,
        },
    )?;
    let session = conn.session().await;
    let (table, meta) = match &req.target {
        ImportTarget::Existing { table } => {
            conn.invalidate(Some(table)).await;
            (table.clone(), Some(cached_meta(&conn, table).await?))
        }
        ImportTarget::Create { table, .. } => (table.clone(), None),
    };
    log::info!(
        "import {} (JSON) into {} started",
        req.import_id,
        table.name
    );
    let report = run_json_import(
        session.as_ref(),
        &req,
        meta.as_deref(),
        move |p| {
            if let Err(e) = on_progress.send(p) {
                log::debug!("import progress not delivered: {e}");
            }
        },
        token,
    )
    .await;
    conn.invalidate(Some(&table)).await;
    match &report {
        Ok(r) => log::info!(
            "import {} finished: {} of {} objects in {} ms",
            req.import_id,
            r.rows_imported,
            r.rows_read,
            r.duration_ms
        ),
        Err(e) => log::info!("import {} failed: {e}", req.import_id),
    }
    report
}

/// The start of a SQL dump (plain or gzip), split as it will be run.
#[tauri::command]
pub async fn preview_sql_dump(
    state: State<'_, AppState>,
    connection_id: Uuid,
    req: SqlDumpPreviewRequest,
) -> Result<SqlDumpPreview, AppError> {
    let dialect = state.session(connection_id).await?.dialect();
    tokio::task::spawn_blocking(move || preview_dump(&req.path, dialect))
        .await
        .map_err(|e| AppError::internal(format!("preview task failed: {e}")))?
}

/// Imports write: a read-only connection refuses them (the renderer's guard already does).
fn refuse_read_only(conn: &Connection) -> Result<(), AppError> {
    if conn.profile.options.read_only {
        return Err(AppError::Unsupported("this connection is read-only".into()));
    }
    Ok(())
}

/// Bound on closing the dump's session.
const DUMP_SESSION_CLOSE: Duration = Duration::from_secs(5);

/// A new session for a SQL dump, opened like `conn`'s: its profile (bootstrap commands
/// included), the database it is on now, its SSH tunnel and its password.
async fn dump_session(state: &AppState, conn: &Connection) -> Result<Arc<dyn DbSession>, AppError> {
    session_on_database(state, conn, None)
        .await
        .map_err(|e| match e {
            AppError::Connect(m) => {
                AppError::Connect(format!("cannot open a connection for the import: {m}"))
            }
            other => other,
        })
}

/// A new session opened like `conn`'s (profile with its bootstrap commands, SSH tunnel,
/// password) on `database`, or on the database `conn` is on now. The caller closes it. Used by
/// SQL dumps and by exports that read another PostgreSQL database of the connection.
pub(crate) async fn session_on_database(
    state: &AppState,
    conn: &Connection,
    database: Option<&str>,
) -> Result<Arc<dyn DbSession>, AppError> {
    let mut profile = conn.profile.clone();
    if profile.driver.descriptor().location != Location::File {
        let db = match database {
            Some(db) => Some(db.to_string()),
            None => conn.info().await.current_database,
        };
        if let Some(db) = db {
            profile.database = Some(db);
        }
    }
    let host_override = match &conn.tunnel {
        Some(t) if !t.is_up() => {
            return Err(AppError::Tunnel(
                t.state()
                    .message
                    .unwrap_or_else(|| "the SSH session ended".into()),
            ));
        }
        Some(t) => Some(t.connect_addr()),
        None => None,
    };
    let password = connect_password(state, &profile, None).await?;
    tokio::time::timeout(
        connect_timeout(&profile),
        drivers::connect(ConnectTarget {
            profile: &profile,
            password: password.as_deref(),
            host_override,
        }),
    )
    .await
    .map_err(|_| AppError::Timeout)?
}

/// Runs a SQL dump's statements one by one on a session of its own (`dump_session`).
#[tauri::command]
pub async fn import_sql_dump(
    state: State<'_, AppState>,
    connection_id: Uuid,
    req: SqlImportRequest,
    on_progress: Channel<SqlImportProgress>,
) -> Result<SqlImportReport, AppError> {
    let conn = state.connection(connection_id).await?;
    refuse_read_only(&conn)?;
    let token = CancellationToken::new();
    let slot = CancelSlot::default();
    let registered = Registration::new(
        &req.import_id,
        Running {
            connection_id,
            token: token.clone(),
            slot: Some(slot.clone()),
            session: None,
        },
    )?;
    let session = dump_session(&state, &conn).await?;
    registered.set_session(session.clone());
    log::info!("import {} (SQL dump) started", req.import_id);
    let report = run_sql_import(
        session.as_ref(),
        &req,
        slot,
        move |p| {
            if let Err(e) = on_progress.send(p) {
                log::debug!("import progress not delivered: {e}");
            }
        },
        token,
    )
    .await;
    if tokio::time::timeout(DUMP_SESSION_CLOSE, session.close())
        .await
        .is_err()
    {
        log::warn!("import {}: closing its session timed out", req.import_id);
    }
    // The dump may have changed any table.
    conn.invalidate(None).await;
    match &report {
        Ok(r) => log::info!(
            "import {} finished: {} statements, {} failed, {} skipped in {} ms",
            req.import_id,
            r.executed,
            r.failed,
            r.skipped,
            r.duration_ms
        ),
        Err(e) => log::info!("import {} failed: {e}", req.import_id),
    }
    report
}

/// Stops a running import: a CSV import between chunks (committed chunks stay), a SQL dump at
/// its running statement. Unknown ids are ignored.
#[tauri::command]
pub async fn cancel_import(state: State<'_, AppState>, import_id: String) -> Result<(), AppError> {
    let found = imports().get(&import_id).map(|r| {
        (
            r.connection_id,
            r.token.clone(),
            r.slot.clone(),
            r.session.clone(),
        )
    });
    let Some((connection_id, token, slot, own)) = found else {
        return Ok(());
    };
    token.cancel();
    if let Some(slot) = slot
        && let Some(handle) = request_cancel(&slot)
    {
        let session = match own {
            Some(session) => Ok(session),
            None => state.session(connection_id).await,
        };
        match session {
            Ok(session) => session.cancel(handle).await?,
            // Disconnecting: closing the session stops the statement itself.
            Err(AppError::NotConnected) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
