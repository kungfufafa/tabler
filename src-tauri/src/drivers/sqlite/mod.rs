//! SQLite driver. Each session owns two actor threads with their own `rusqlite::Connection`:
//! `editor` for the SQL editor (keeps transaction state) and `worker` for browsing/introspection.

mod actor;
mod decode;
mod exec;
mod introspect;

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::OpenFlags;

use crate::drivers::{
    CancelHandle, CancelSlot, DbSession, RowSink, ServerInfo, StatementOutcome, TxState,
    TxStatement, check_affected,
};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::types::profile::{ConnectionProfile, Driver};
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub use actor::Actor;

/// Refuses a SQLite profile whose database file does not exist. `LiteSession::open` creates
/// missing files, so a mistyped or moved path would otherwise "connect" to a new, empty database;
/// files are created explicitly with [`create_database_file`] ("New…" in the connection dialog).
/// `:memory:` is refused too: each of a session's two connections would get its own database.
pub fn check_database_file(profile: &ConnectionProfile) -> Result<(), AppError> {
    let path = profile.sqlite_path.as_deref().unwrap_or("");
    if path.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "SQLite file path is required".into(),
        ));
    }
    if path != ":memory:" && Path::new(path).is_file() {
        return Ok(());
    }
    Err(AppError::NotFound(format!(
        "SQLite database file not found: {path}. Use “New…” to create one."
    )))
}

/// Creates an empty SQLite database at `path` (with its header written, so other tools
/// recognise it). An existing file is left untouched; anything else at `path` is an error.
pub async fn create_database_file(path: &str) -> Result<(), AppError> {
    if path.trim().is_empty() || path == ":memory:" {
        return Err(AppError::InvalidInput(
            "Choose where to create the SQLite file".into(),
        ));
    }
    let path = path.to_string();
    tokio::task::spawn_blocking(move || {
        let target = Path::new(&path);
        if target.is_file() {
            return Ok(());
        }
        if target.exists() {
            return Err(AppError::InvalidInput(format!("{path} is not a file")));
        }
        let cannot = |e: rusqlite::Error| AppError::Io(format!("cannot create {path}: {e}"));
        let conn = rusqlite::Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(cannot)?;
        // Reading opens the file; writing the (unchanged) user version writes the header page.
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(cannot)?;
        conn.execute_batch(&format!("PRAGMA user_version = {version};"))
            .map_err(cannot)?;
        conn.close().map_err(|(_, e)| cannot(e))?;
        log::info!("created SQLite database {path}");
        Ok(())
    })
    .await
    .map_err(|e| AppError::Internal(format!("create sqlite file: {e}")))?
}

pub struct LiteSession {
    pub(crate) path: String,
    pub(crate) editor: Actor,
    pub(crate) worker: Actor,
    /// Editor transaction state (`!is_autocommit()`), refreshed after every editor statement.
    tx_state: Arc<TxState>,
    /// Held by one editor run at a time (`lock_editor_runs`).
    editor_runs: tokio::sync::Mutex<()>,
}

impl LiteSession {
    pub async fn open(profile: &ConnectionProfile) -> Result<LiteSession, AppError> {
        let path = profile
            .sqlite_path
            .clone()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| AppError::InvalidInput("SQLite profile has no file path".into()))?;
        let read_only = profile.options.read_only;
        let foreign_keys = profile.options.sqlite_foreign_keys;
        let flags = if read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
        };
        let editor = Actor::spawn(&path, flags, foreign_keys)?;
        let worker = Actor::spawn(&path, flags, foreign_keys)?;
        Ok(LiteSession {
            path,
            editor,
            worker,
            tx_state: Arc::new(TxState::default()),
            editor_runs: tokio::sync::Mutex::new(()),
        })
    }
}

#[async_trait]
impl DbSession for LiteSession {
    fn driver(&self) -> Driver {
        Driver::Sqlite
    }

    fn dialect(&self) -> Dialect {
        Dialect::Sqlite
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        let path = self.path.clone();
        self.worker
            .run(move |conn| {
                let version: String =
                    conn.query_row("SELECT sqlite_version()", [], |r| r.get(0))?;
                Ok(ServerInfo {
                    version: format!("SQLite {version}"),
                    current_database: Some(path),
                    current_user: None,
                })
            })
            .await
    }

    async fn ping(&self) -> Result<(), AppError> {
        self.worker
            .run(|conn| {
                conn.query_row("SELECT 1", [], |_| Ok(()))?;
                Ok(())
            })
            .await
    }

    /// Interrupts whatever runs on either connection (a long write would otherwise keep the file
    /// locked after a disconnect) and shuts both actors down.
    async fn close(&self) {
        self.editor.close();
        self.worker.close();
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        let path = self.path.clone();
        Ok(vec![DatabaseInfo {
            name: path,
            owner: None,
            encoding: None,
            is_current: true,
        }])
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        self.worker.run(introspect::list_schemas).await
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let schema = schema.to_string();
        self.worker
            .run(move |conn| introspect::list_tables(conn, &schema))
            .await
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let table = table.clone();
        self.worker
            .run(move |conn| introspect::table_meta(conn, &table))
            .await
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let table = table.clone();
        self.worker
            .run(move |conn| introspect::view_definition(conn, &table))
            .await
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        self.worker.run(introspect::completion_metadata).await
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError> {
        let sql = sql.to_string();
        let params = params.to_vec();
        self.worker
            .run(move |conn| decode::run_query(conn, &sql, &params, None))
            .await
    }

    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        let sql = sql.to_string();
        let params = params.to_vec();
        self.worker
            .run(move |conn| decode::run_execute(conn, &sql, &params))
            .await
    }

    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        self.worker
            .run(move |conn| {
                conn.execute_batch("BEGIN")?;
                let mut out = Vec::with_capacity(statements.len());
                for (i, st) in statements.iter().enumerate() {
                    match decode::run_execute(conn, &st.sql, &st.params).and_then(|o| {
                        check_affected(st, &o, i)?;
                        Ok(o)
                    }) {
                        Ok(o) => out.push(o),
                        Err(e) => {
                            let _ = conn.execute_batch("ROLLBACK");
                            return Err(e);
                        }
                    }
                }
                conn.execute_batch("COMMIT")?;
                Ok(out)
            })
            .await
    }

    async fn run_statement(
        &self,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        exec::run_on_actor(
            &self.editor,
            sql,
            &[],
            Some(max_rows),
            sink,
            cancel,
            Some(self.tx_state.clone()),
        )
        .await
    }

    async fn stream_query(
        &self,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<u64, AppError> {
        let outcome =
            exec::run_on_actor(&self.worker, sql, params, None, sink, cancel, None).await?;
        Ok(outcome.row_count)
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        // The ticket (if any) is released once the interrupt has been raised.
        let (handle, _ticket) = handle.into_parts();
        if let CancelHandle::Sqlite(interrupt) = handle {
            interrupt.interrupt();
        }
        Ok(())
    }

    fn in_transaction(&self) -> Option<bool> {
        self.tx_state.get()
    }

    fn editor_run_lock(&self) -> Option<&tokio::sync::Mutex<()>> {
        Some(&self.editor_runs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::profile::{ProfileOptions, SslMode};

    fn profile(path: Option<&str>) -> ConnectionProfile {
        ConnectionProfile {
            id: uuid::Uuid::new_v4(),
            name: "lite".into(),
            color: None,
            group: None,
            driver: Driver::Sqlite,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: path.map(str::to_string),
            ssh: None,
            options: ProfileOptions {
                timezone: None,
                connect_timeout_secs: None,
                read_only: false,
                startup_sql: None,
                default_row_limit: None,
                sqlite_foreign_keys: true,
            },
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    #[test]
    fn refuses_missing_files_instead_of_creating_them() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("typo.sqlite");
        let missing = missing.to_string_lossy();
        let err = check_database_file(&profile(Some(&missing))).unwrap_err();
        assert_eq!(err.code(), "NOT_FOUND");
        assert!(err.to_string().contains("typo.sqlite"), "{err}");
        assert!(err.to_string().contains("New…"), "{err}");
        assert!(!dir.path().join("typo.sqlite").exists());

        // A directory, `:memory:` and an empty path are refused too.
        let dir_path = dir.path().to_string_lossy();
        assert_eq!(
            check_database_file(&profile(Some(&dir_path)))
                .unwrap_err()
                .code(),
            "NOT_FOUND"
        );
        assert_eq!(
            check_database_file(&profile(Some(":memory:")))
                .unwrap_err()
                .code(),
            "NOT_FOUND"
        );
        assert_eq!(
            check_database_file(&profile(None)).unwrap_err().code(),
            "INVALID_INPUT"
        );
    }

    #[tokio::test]
    async fn creates_a_valid_database_and_never_touches_an_existing_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.sqlite");
        let path_str = path.to_string_lossy().into_owned();
        create_database_file(&path_str).await.unwrap();
        check_database_file(&profile(Some(&path_str))).unwrap();
        let header = std::fs::read(&path).unwrap();
        assert!(
            header.starts_with(b"SQLite format 3\0"),
            "no header written"
        );

        // The new file opens as a database (through the regular session) and takes writes.
        let session = LiteSession::open(&profile(Some(&path_str))).await.unwrap();
        session
            .execute("CREATE TABLE t (id INTEGER PRIMARY KEY)", &[])
            .await
            .unwrap();
        session
            .execute("INSERT INTO t (id) VALUES (1)", &[])
            .await
            .unwrap();
        session.close().await;

        // "New…" on an existing file keeps its data.
        create_database_file(&path_str).await.unwrap();
        let session = LiteSession::open(&profile(Some(&path_str))).await.unwrap();
        let rows = session.query("SELECT count(*) FROM t", &[]).await.unwrap();
        assert_eq!(rows.rows.len(), 1);
        assert_eq!(rows.rows[0][0], Value::Int(1));
        session.close().await;

        // A non-database file is left alone as well.
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "hello").unwrap();
        create_database_file(&text.to_string_lossy()).await.unwrap();
        assert_eq!(std::fs::read_to_string(&text).unwrap(), "hello");

        // Directories, missing parents and empty paths are errors.
        assert!(
            create_database_file(&dir.path().to_string_lossy())
                .await
                .is_err()
        );
        let orphan = dir.path().join("no/such/dir/x.sqlite");
        let err = create_database_file(&orphan.to_string_lossy())
            .await
            .unwrap_err();
        assert_eq!(err.code(), "IO");
        assert_eq!(
            create_database_file("  ").await.unwrap_err().code(),
            "INVALID_INPUT"
        );
    }
}
