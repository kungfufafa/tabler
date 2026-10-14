//! SQLite driver. Each session owns two actor threads with their own `rusqlite::Connection`:
//! `editor` for the SQL editor (keeps transaction state) and `worker` for browsing/introspection.

mod actor;
pub mod cipher;
mod decode;
mod exec;
mod introspect;

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::OpenFlags;

use crate::drivers::{
    CANCEL_SETTLE_LIMIT, CancelHandle, CancelSlot, DbSession, RowSink, ServerInfo, SqlTrace,
    StatementOutcome, TraceCounts, TxState, TxStatement, arm_cancel, check_affected,
    classify_statement, settle_cancels_blocking, trace_sql,
};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::types::browse::TracedSql;
use crate::types::profile::{ConnectionProfile, Driver, PasswordMode};
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub use actor::Actor;
pub use cipher::Cipher;

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
        return match header_kind(path) {
            // Opening tells whether the key (the profile's password) fits; a profile that never
            // has a password can only be refused.
            HeaderKind::Encrypted if profile.options.password_mode() != PasswordMode::None => {
                Ok(())
            }
            kind => match kind.error(path) {
                Some(e) => Err(e),
                None => Ok(()),
            },
        };
    }
    Err(AppError::NotFound(format!(
        "SQLite database file not found: {path}. Use “New…” to create one."
    )))
}

/// The first 16 bytes of every SQLite database file.
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";
/// Shown for an encrypted database (SQLCipher, b100 / #347 in the spec) opened without its key.
pub const ENCRYPTED_DATABASE: &str = "This database is encrypted (SQLCipher): enter its \
     encryption key in the connection's settings";

/// What the first bytes of a database file say it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderKind {
    /// A plain SQLite database.
    Sqlite,
    /// Empty: a new database (or one that cannot be read here: opening reports why).
    Empty,
    /// Whole pages of random-looking bytes: SQLCipher encrypts the whole file, header included.
    Encrypted,
    /// Something else.
    Foreign,
}

impl HeaderKind {
    /// Why a file of this kind cannot be opened without a key; `None` when it can.
    pub fn error(self, path: &str) -> Option<AppError> {
        match self {
            HeaderKind::Sqlite | HeaderKind::Empty => None,
            HeaderKind::Encrypted => Some(AppError::Connect(ENCRYPTED_DATABASE.into())),
            HeaderKind::Foreign => Some(AppError::Connect(format!(
                "{path} is not a SQLite database"
            ))),
        }
    }
}

/// Reads the header of the file at `path` (SQLite itself only notices a foreign or encrypted
/// file at the first query, with "file is not a database").
pub fn header_kind(path: &str) -> HeaderKind {
    let Ok(mut file) = std::fs::File::open(path) else {
        return HeaderKind::Empty;
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return HeaderKind::Empty;
    }
    let mut header = [0u8; 16];
    let read = file.read_exact(&mut header);
    if read.is_ok() && &header == SQLITE_HEADER {
        return HeaderKind::Sqlite;
    }
    // Page sizes are powers of two from 512 to 65536.
    if read.is_ok() && len >= 1024 && len % 512 == 0 {
        return HeaderKind::Encrypted;
    }
    HeaderKind::Foreign
}

/// Why the file at `path` cannot be opened without a key (see [`header_kind`]); `None` for a
/// SQLite file, an empty file or one that cannot be read here.
pub fn header_error(path: &str) -> Option<AppError> {
    header_kind(path).error(path)
}

/// The SQL statements of a resource file (`.sqliterc`): its text without the sqlite3 shell's
/// dot-commands (`.headers on`, `.mode column`…), which only the shell understands.
pub fn resource_file_sql(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('.'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reads the profile's resource file, if it has one.
fn resource_file(profile: &ConnectionProfile) -> Result<Option<String>, AppError> {
    let Some(path) = profile
        .options
        .sqlite_resource_file
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    else {
        return Ok(None);
    };
    let path = crate::tunnel::expand_tilde(path);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        AppError::Connect(format!(
            "cannot read the resource file {}: {e}",
            path.display()
        ))
    })?;
    let sql = resource_file_sql(&text);
    Ok((!sql.trim().is_empty()).then_some(sql))
}

/// Creates an empty SQLite database at `path` (with its header written, so other tools
/// recognise it). An existing file is left untouched; anything else at `path` is an error.
pub async fn create_database_file(path: &str) -> Result<(), AppError> {
    create_database_file_with(path, None).await
}

/// [`create_database_file`], encrypted with `cipher` when one is given (SQLCipher).
pub async fn create_database_file_with(path: &str, cipher: Option<Cipher>) -> Result<(), AppError> {
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
        if let Some(cipher) = &cipher {
            cipher::create_encrypted(&path, cipher)?;
            log::info!("created encrypted SQLite database {path}");
            return Ok(());
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
    /// The statements this session ran for the app (see `SqlTrace`); the actor jobs hold clones.
    trace: SqlTrace,
}

impl LiteSession {
    pub async fn open(profile: &ConnectionProfile) -> Result<LiteSession, AppError> {
        LiteSession::open_with_key(profile, None).await
    }

    /// Opens the profile's database; `key` is its encryption key (the profile's password), set
    /// on both connections before anything else runs there. Then come the session's pragmas,
    /// the resource file's statements and the bootstrap commands.
    pub async fn open_with_key(
        profile: &ConnectionProfile,
        key: Option<&str>,
    ) -> Result<LiteSession, AppError> {
        let path = profile
            .sqlite_path
            .clone()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| AppError::InvalidInput("SQLite profile has no file path".into()))?;
        let cipher = Cipher::from_key(key, profile.options.sqlite_cipher_compatibility);
        match (header_kind(&path), &cipher) {
            (HeaderKind::Sqlite, Some(_)) => {
                return Err(AppError::Connect(cipher::NOT_ENCRYPTED.into()));
            }
            (HeaderKind::Encrypted, Some(_)) => {}
            (kind, _) => {
                if let Some(e) = kind.error(&path) {
                    return Err(e);
                }
            }
        }
        let resource = resource_file(profile)?;
        let read_only = profile.options.read_only;
        let foreign_keys = profile.options.sqlite_foreign_keys;
        let flags = if read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
        };
        let editor = Actor::spawn(&path, flags, foreign_keys, cipher.as_ref())?;
        let worker = Actor::spawn(&path, flags, foreign_keys, cipher.as_ref())?;
        // The resource file, then the bootstrap commands, run on both connections, after the
        // session's own pragmas.
        let startup = profile
            .options
            .startup_sql
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        for sql in [resource, startup].into_iter().flatten() {
            for actor in [&editor, &worker] {
                let sql = sql.to_string();
                let ran = actor
                    .run(move |conn| {
                        conn.execute_batch(&sql)?;
                        Ok(())
                    })
                    .await;
                if let Err(e) = ran {
                    editor.close();
                    worker.close();
                    return Err(e);
                }
            }
        }
        Ok(LiteSession {
            path,
            editor,
            worker,
            tx_state: Arc::new(TxState::default()),
            editor_runs: tokio::sync::Mutex::new(()),
            trace: SqlTrace::default(),
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
        let trace = self.trace.clone();
        self.worker
            .run(move |conn| introspect::list_schemas(conn, &trace))
            .await
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let schema = schema.to_string();
        let trace = self.trace.clone();
        self.worker
            .run(move |conn| introspect::list_tables(conn, &trace, &schema))
            .await
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let table = table.clone();
        let trace = self.trace.clone();
        self.worker
            .run(move |conn| introspect::table_meta(conn, &trace, &table))
            .await
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let table = table.clone();
        let trace = self.trace.clone();
        self.worker
            .run(move |conn| introspect::view_definition(conn, &trace, &table))
            .await
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let trace = self.trace.clone();
        self.worker
            .run(move |conn| introspect::completion_metadata(conn, &trace))
            .await
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError> {
        let sql = sql.to_string();
        let params = params.to_vec();
        let trace = self.trace.clone();
        self.worker
            .run(move |conn| traced_query(conn, &trace, &sql, &params))
            .await
    }

    async fn query_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ResultSet, AppError> {
        let sql = sql.to_string();
        let params = params.to_vec();
        let trace = self.trace.clone();
        let interrupt = self.worker.interrupt_handle();
        self.worker
            .run(move |conn| {
                // Armed only while the statement runs on the actor thread, not while the job
                // waits behind others (see `execute_cancellable`).
                let armed = arm_cancel(&cancel, CancelHandle::Sqlite(interrupt))?;
                let result = traced_query(conn, &trace, &sql, &params);
                drop(armed);
                // A late `interrupt()` must not hit the next job's statement.
                if !settle_cancels_blocking(&cancel, CANCEL_SETTLE_LIMIT) {
                    log::warn!("sqlite: a cancel request did not finish in time");
                }
                result
            })
            .await
    }

    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        let sql = sql.to_string();
        let params = params.to_vec();
        let trace = self.trace.clone();
        self.worker
            .run(move |conn| traced_execute(conn, &trace, &sql, &params))
            .await
    }

    async fn execute_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ExecOutcome, AppError> {
        let sql = sql.to_string();
        let params = params.to_vec();
        let trace = self.trace.clone();
        let interrupt = self.worker.interrupt_handle();
        self.worker
            .run(move |conn| {
                // The interrupt handle goes into the slot only while this statement runs on the
                // actor thread, not while the job waits behind others (see `exec::run_on_actor`).
                let armed = arm_cancel(&cancel, CancelHandle::Sqlite(interrupt))?;
                let result = traced_execute(conn, &trace, &sql, &params);
                drop(armed);
                // A late `interrupt()` must not hit the next job's statement.
                if !settle_cancels_blocking(&cancel, CANCEL_SETTLE_LIMIT) {
                    log::warn!("sqlite: a cancel request did not finish in time");
                }
                result
            })
            .await
    }

    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        let trace = self.trace.clone();
        self.worker
            .run(move |conn| {
                conn.execute_batch("BEGIN")?;
                let mut out = Vec::with_capacity(statements.len());
                for (i, st) in statements.iter().enumerate() {
                    match traced_execute(conn, &trace, &st.sql, &st.params).and_then(|o| {
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
        let started = std::time::Instant::now();
        let result = exec::run_on_actor(&self.worker, sql, params, None, sink, cancel, None).await;
        self.trace.finish(
            classify_statement(sql, Dialect::Sqlite),
            trace_sql(Dialect::Sqlite, sql, params),
            started,
            &result,
            |o| TraceCounts::rows(o.row_count as usize),
        );
        Ok(result?.row_count)
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

    fn drain_trace(&self) -> Vec<TracedSql> {
        self.trace.drain()
    }

    fn sql_trace(&self) -> Option<&SqlTrace> {
        Some(&self.trace)
    }

    fn editor_run_lock(&self) -> Option<&tokio::sync::Mutex<()>> {
        Some(&self.editor_runs)
    }
}

/// `decode::run_query` on the actor thread, recorded in the session's trace.
fn traced_query(
    conn: &rusqlite::Connection,
    trace: &SqlTrace,
    sql: &str,
    params: &[Value],
) -> Result<ResultSet, AppError> {
    trace.run(
        classify_statement(sql, Dialect::Sqlite),
        trace_sql(Dialect::Sqlite, sql, params),
        |rs: &ResultSet| TraceCounts::rows(rs.rows.len()),
        || decode::run_query(conn, sql, params, None),
    )
}

/// `decode::run_execute` on the actor thread, recorded in the session's trace.
fn traced_execute(
    conn: &rusqlite::Connection,
    trace: &SqlTrace,
    sql: &str,
    params: &[Value],
) -> Result<ExecOutcome, AppError> {
    trace.run(
        classify_statement(sql, Dialect::Sqlite),
        trace_sql(Dialect::Sqlite, sql, params),
        TraceCounts::of_exec,
        || decode::run_execute(conn, sql, params),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::profile::{PasswordMode, ProfileOptions, SslMode};

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
                sqlite_foreign_keys: true,
                ..ProfileOptions::default()
            },
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    #[tokio::test]
    async fn bootstrap_commands_run_on_both_connections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("boot.sqlite");
        let path = path.to_string_lossy().into_owned();
        create_database_file(&path).await.unwrap();
        let mut p = profile(Some(&path));
        p.options.sqlite_foreign_keys = true;
        p.options.startup_sql =
            Some("PRAGMA foreign_keys = OFF;\nCREATE TEMP TABLE boot (x);".into());
        let session = LiteSession::open(&p).await.unwrap();
        for actor in [&session.editor, &session.worker] {
            let (fk, temp): (i64, i64) = actor
                .run(|c| {
                    Ok(c.query_row(
                        "SELECT (SELECT foreign_keys FROM pragma_foreign_keys), \
                         (SELECT count(*) FROM temp.sqlite_master WHERE name = 'boot')",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?)
                })
                .await
                .unwrap();
            assert_eq!((fk, temp), (0, 1));
        }
        session.close().await;

        // A failing bootstrap command fails the connect.
        p.options.startup_sql = Some("SELECT * FROM no_such_table".into());
        let err = LiteSession::open(&p).await.err().unwrap();
        assert!(err.to_string().contains("no_such_table"), "{err}");
    }

    async fn seeded_session() -> (LiteSession, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.sqlite");
        let path = path.to_string_lossy().into_owned();
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE fx3_users (id INTEGER PRIMARY KEY, name TEXT);
             CREATE INDEX fx3_users_name ON fx3_users (name);
             INSERT INTO fx3_users (name) VALUES ('ann'), ('bob'), ('cy');",
        )
        .unwrap();
        drop(db);
        (LiteSession::open(&profile(Some(&path))).await.unwrap(), dir)
    }

    #[tokio::test]
    async fn traces_the_statements_it_runs_for_the_app() {
        use crate::types::browse::TraceCategory::{Data, Structure};
        let (session, _dir) = seeded_session().await;
        // Opening the session (pragmas, bootstrap) is not the app's own work.
        assert!(session.drain_trace().is_empty());

        session.list_tables("main").await.unwrap();
        let t = TableRef::new(Some("main"), "fx3_users");
        session.table_meta(&t).await.unwrap();
        let counted = session
            .query(
                "SELECT COUNT(*) FROM \"main\".\"fx3_users\" WHERE \"name\" <> ?1",
                &[Value::Text("ann".into())],
            )
            .await
            .unwrap();
        assert_eq!(counted.rows, vec![vec![Value::Int(2)]]);
        session
            .execute(
                "UPDATE fx3_users SET name = ?1 WHERE id = ?2",
                &[Value::Text("zed".into()), Value::Int(1)],
            )
            .await
            .unwrap();
        assert!(
            session
                .query("SELECT * FROM fx3_missing", &[])
                .await
                .is_err()
        );

        let trace = session.drain_trace();
        let find = |needle: &str| {
            trace
                .iter()
                .find(|e| e.sql.contains(needle))
                .unwrap_or_else(|| panic!("no `{needle}` in {trace:#?}"))
        };
        // The catalog reads are meta queries, in the SQL that really ran.
        let tables = find("sqlite_master WHERE type IN ('table','view')");
        assert_eq!(tables.category, Structure);
        assert_eq!(tables.rows, Some(1));
        assert!(!tables.sql.starts_with("--"));
        let columns = find("table_xinfo");
        assert_eq!(columns.category, Structure);
        assert_eq!(columns.rows, Some(2));
        assert_eq!(find("PRAGMA \"main\".index_list").category, Structure);
        // The count is a data query, its parameter inlined for display.
        let count = find("COUNT(*)");
        assert_eq!(count.category, Data);
        assert!(count.sql.ends_with("<> 'ann'"), "{}", count.sql);
        assert_eq!((count.rows, count.error.as_deref()), (Some(1), None));
        let update = find("UPDATE fx3_users");
        assert_eq!((update.category, update.affected), (Data, Some(1)));
        assert!(
            update.sql.contains("name = 'zed' WHERE id = 1"),
            "{}",
            update.sql
        );
        // A failing statement is recorded with the server's message.
        let failed = find("fx3_missing");
        assert!(failed.error.as_deref().unwrap().contains("no such table"));
        // Everything has a timing, and the trace was drained.
        assert!(trace.iter().all(|e| e.duration_ms < 5_000));
        assert!(session.drain_trace().is_empty());
    }

    /// The statements the app runs for its own purposes (the EXPLAIN a button asks for, a SQL
    /// dump import) go through `run_statement_traced` and are recorded; the SQL editor's own runs
    /// (`run_statement`) are logged by the renderer from the run's events and are not.
    #[tokio::test]
    async fn run_statement_traced_records_what_the_editor_path_does_not() {
        use crate::types::browse::TraceCategory::{Data, Structure};
        let (session, _dir) = seeded_session().await;
        session.drain_trace();
        let mut sink = crate::drivers::CollectSink::default();

        session
            .run_statement(
                "SELECT * FROM fx3_users",
                100,
                &mut sink,
                CancelSlot::default(),
            )
            .await
            .unwrap();
        assert!(
            session.drain_trace().is_empty(),
            "the editor path is untraced"
        );

        let mut sink = crate::drivers::CollectSink::default();
        session
            .run_statement_traced(
                "EXPLAIN QUERY PLAN SELECT * FROM fx3_users",
                100,
                &mut sink,
                CancelSlot::default(),
            )
            .await
            .unwrap();
        session
            .run_statement_traced(
                "INSERT INTO fx3_users (id, name) VALUES (10, 'x')",
                100,
                &mut sink,
                CancelSlot::default(),
            )
            .await
            .unwrap();
        session
            .run_statement_traced(
                "CREATE TABLE fx3_more (a int)",
                100,
                &mut sink,
                CancelSlot::default(),
            )
            .await
            .unwrap();
        assert!(
            session
                .run_statement_traced(
                    "SELECT * FROM fx3_nope",
                    100,
                    &mut sink,
                    CancelSlot::default()
                )
                .await
                .is_err()
        );
        let trace = session.drain_trace();
        assert_eq!(trace.len(), 4, "{trace:#?}");
        assert_eq!(trace[0].sql, "EXPLAIN QUERY PLAN SELECT * FROM fx3_users");
        assert_eq!((trace[0].category, trace[0].error.as_deref()), (Data, None));
        assert!(trace[0].rows.is_some());
        assert_eq!(trace[1].category, Data);
        assert_eq!(trace[1].affected, Some(1));
        assert_eq!(trace[2].category, Structure);
        assert!(trace[3].error.as_deref().unwrap().contains("no such table"));
        assert!(session.drain_trace().is_empty());
    }

    #[tokio::test]
    async fn completion_metadata_is_one_trace_entry() {
        let (session, _dir) = seeded_session().await;
        session.completion_metadata().await.unwrap();
        let trace = session.drain_trace();
        let pragmas: Vec<_> = trace
            .iter()
            .filter(|e| e.sql.contains("table_info"))
            .collect();
        assert_eq!(pragmas.len(), 1, "{trace:#?}");
        assert!(pragmas[0].sql.contains("fx3_users"));
        assert!(trace.iter().any(|e| e.sql.contains("database_list")));
    }

    #[tokio::test]
    async fn a_cancellable_query_is_stopped_by_its_slot() {
        use crate::drivers::request_cancel;
        let (session, _dir) = seeded_session().await;
        let session = Arc::new(session);
        let slot = CancelSlot::default();
        let running = {
            let (session, slot) = (session.clone(), slot.clone());
            tokio::spawn(async move {
                session
                    .query_cancellable(
                        "with recursive c(x) as (select 1 union all select x + 1 from c \
                         where x < 2000000000) select count(*) from c",
                        &[],
                        slot,
                    )
                    .await
            })
        };
        let started = std::time::Instant::now();
        while !slot.is_armed() {
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let handle = request_cancel(&slot).expect("the statement is running");
        session.cancel(handle).await.unwrap();
        let err = running.await.unwrap().unwrap_err();
        assert!(matches!(err, AppError::Cancelled), "{err:?}");
        // The worker still answers.
        let rs = session.query("select 5", &[]).await.unwrap();
        assert_eq!(rs.rows, vec![vec![Value::Int(5)]]);
        // A slot cancelled before the statement starts stops it before it runs.
        let early = CancelSlot::default();
        let _ = request_cancel(&early);
        assert!(matches!(
            session.query_cancellable("select 1", &[], early).await,
            Err(AppError::Cancelled)
        ));
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
    async fn encrypted_and_foreign_files_get_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        // SQLCipher: whole pages of random bytes, no plain header.
        let encrypted = dir.path().join("secret.db");
        let bytes: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        std::fs::write(&encrypted, &bytes).unwrap();
        let encrypted = encrypted.to_string_lossy().into_owned();
        // Its key may be the profile's password: only opening can tell. A profile without a
        // password is refused right away.
        check_database_file(&profile(Some(&encrypted))).unwrap();
        let mut keyless = profile(Some(&encrypted));
        keyless.options.password_mode = Some(PasswordMode::None);
        let err = check_database_file(&keyless).unwrap_err();
        assert_eq!(err.to_string(), ENCRYPTED_DATABASE);
        assert!(err.to_string().contains("encryption key"), "{err}");
        let err = LiteSession::open(&profile(Some(&encrypted)))
            .await
            .err()
            .unwrap();
        assert_eq!(err.to_string(), ENCRYPTED_DATABASE);

        // Something else entirely.
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "hello").unwrap();
        let err = check_database_file(&profile(Some(&text.to_string_lossy()))).unwrap_err();
        assert!(
            err.to_string().contains("is not a SQLite database"),
            "{err}"
        );

        // Real and empty databases open.
        let real = dir
            .path()
            .join("real.sqlite")
            .to_string_lossy()
            .into_owned();
        create_database_file(&real).await.unwrap();
        check_database_file(&profile(Some(&real))).unwrap();
        let empty = dir.path().join("empty.sqlite");
        std::fs::write(&empty, "").unwrap();
        check_database_file(&profile(Some(&empty.to_string_lossy()))).unwrap();
    }

    fn key(k: &str, compatibility: Option<u8>) -> Option<Cipher> {
        Cipher::from_key(Some(k), compatibility)
    }

    async fn count(session: &LiteSession) -> Value {
        let rows = session.query("SELECT count(*) FROM t", &[]).await.unwrap();
        rows.rows[0][0].clone()
    }

    /// SQLCipher (b100): "New…" with a key creates an encrypted file; it opens with its key
    /// only; a wrong key or a key for a plain database is a clear error.
    #[tokio::test]
    async fn sqlcipher_databases_open_with_their_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.sqlite");
        let path = path.to_string_lossy().into_owned();
        create_database_file_with(&path, key("s3cret", None))
            .await
            .unwrap();
        let header = std::fs::read(&path).unwrap();
        assert!(
            !header.starts_with(SQLITE_HEADER),
            "the file is not encrypted"
        );
        assert_eq!(header_kind(&path), HeaderKind::Encrypted);

        let p = profile(Some(&path));
        let session = LiteSession::open_with_key(&p, Some("s3cret"))
            .await
            .unwrap();
        session
            .execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)", &[])
            .await
            .unwrap();
        session
            .execute("INSERT INTO t (v) VALUES ('a'), ('b')", &[])
            .await
            .unwrap();
        // Both connections are keyed (the editor runs the SQL editor's statements).
        let mut sink = crate::drivers::CollectSink::default();
        session
            .run_statement(
                "SELECT count(*) FROM t",
                10,
                &mut sink,
                CancelSlot::default(),
            )
            .await
            .unwrap();
        session.close().await;
        assert!(!std::fs::read(&path).unwrap().starts_with(SQLITE_HEADER));

        // Reopened with the key: the data is there.
        let session = LiteSession::open_with_key(&p, Some("s3cret"))
            .await
            .unwrap();
        assert_eq!(count(&session).await, Value::Int(2));
        session.close().await;
        // Without it, or with another one.
        let err = LiteSession::open(&p).await.err().unwrap();
        assert_eq!(err.to_string(), ENCRYPTED_DATABASE);
        let err = LiteSession::open_with_key(&p, Some("wrong"))
            .await
            .err()
            .unwrap();
        assert_eq!(err.to_string(), cipher::WRONG_KEY);
        // An empty key is no key.
        let err = LiteSession::open_with_key(&p, Some(""))
            .await
            .err()
            .unwrap();
        assert_eq!(err.to_string(), ENCRYPTED_DATABASE);

        // A key for a plain database.
        let plain = dir.path().join("plain.sqlite");
        let plain = plain.to_string_lossy().into_owned();
        create_database_file(&plain).await.unwrap();
        let err = LiteSession::open_with_key(&profile(Some(&plain)), Some("s3cret"))
            .await
            .err()
            .unwrap();
        assert_eq!(err.to_string(), cipher::NOT_ENCRYPTED);
        // The app's own database stays a plain SQLite file that opens without a key.
        let app = crate::appdb::AppDb::open(&dir.path().join("tabler.sqlite")).unwrap();
        drop(app);
        assert_eq!(
            header_kind(&dir.path().join("tabler.sqlite").to_string_lossy()),
            HeaderKind::Sqlite
        );
    }

    /// "SQLCipher compatibility": a database in SQLCipher 3's format opens only with
    /// `cipher_compatibility = 3`.
    #[tokio::test]
    async fn sqlcipher_compatibility_3() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v3.db");
        let path = path.to_string_lossy().into_owned();
        create_database_file_with(&path, key("k3", Some(3)))
            .await
            .unwrap();
        let mut p = profile(Some(&path));
        let err = LiteSession::open_with_key(&p, Some("k3"))
            .await
            .err()
            .unwrap();
        assert_eq!(err.to_string(), cipher::WRONG_KEY);
        p.options.sqlite_cipher_compatibility = Some(3);
        let session = LiteSession::open_with_key(&p, Some("k3")).await.unwrap();
        session
            .execute("CREATE TABLE t (id INTEGER)", &[])
            .await
            .unwrap();
        assert_eq!(count(&session).await, Value::Int(0));
        session.close().await;
    }

    /// Backup ▸ Copy Database File… of an encrypted database: the copy is encrypted with the
    /// same key and format.
    #[tokio::test]
    async fn encrypted_copies_stay_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        for compatibility in [None, Some(3)] {
            let src = dir.path().join(format!("src{compatibility:?}.db"));
            let dest = dir.path().join(format!("dest{compatibility:?}.db"));
            let src_str = src.to_string_lossy().into_owned();
            let cipher = key("pw", compatibility).unwrap();
            create_database_file_with(&src_str, Some(cipher.clone()))
                .await
                .unwrap();
            let mut p = profile(Some(&src_str));
            p.options.sqlite_cipher_compatibility = compatibility;
            let session = LiteSession::open_with_key(&p, Some("pw")).await.unwrap();
            for sql in [
                "CREATE TABLE t (id INTEGER)",
                "INSERT INTO t VALUES (1), (2), (3)",
            ] {
                session.execute(sql, &[]).await.unwrap();
            }
            session.close().await;

            std::fs::write(&dest, "old").unwrap();
            let size = cipher::copy_encrypted_to(&src, &dest, &cipher).unwrap();
            assert_eq!(size, std::fs::metadata(&dest).unwrap().len());
            let dest_str = dest.to_string_lossy().into_owned();
            assert_eq!(header_kind(&dest_str), HeaderKind::Encrypted);
            let mut copy = profile(Some(&dest_str));
            copy.options.sqlite_cipher_compatibility = compatibility;
            let session = LiteSession::open_with_key(&copy, Some("pw")).await.unwrap();
            assert_eq!(count(&session).await, Value::Int(3));
            session.close().await;
        }
    }

    /// The `.sqliterc` resource file: its SQL runs after open (dot-commands skipped), before the
    /// bootstrap commands, on both connections; a missing file fails the connect.
    #[tokio::test]
    async fn resource_files_run_after_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rc.sqlite");
        let path = path.to_string_lossy().into_owned();
        create_database_file(&path).await.unwrap();
        let rc = dir.path().join(".sqliterc");
        std::fs::write(
            &rc,
            ".headers on\n.mode column\nPRAGMA foreign_keys = OFF;\nCREATE TEMP TABLE rc (x);\n",
        )
        .unwrap();
        let mut p = profile(Some(&path));
        p.options.sqlite_resource_file = Some(rc.to_string_lossy().into_owned());
        p.options.startup_sql = Some("INSERT INTO temp.rc VALUES (1);".into());
        let session = LiteSession::open(&p).await.unwrap();
        for actor in [&session.editor, &session.worker] {
            let (fk, rows): (i64, i64) = actor
                .run(|c| {
                    Ok(c.query_row(
                        "SELECT (SELECT foreign_keys FROM pragma_foreign_keys), \
                         (SELECT count(*) FROM temp.rc)",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?)
                })
                .await
                .unwrap();
            assert_eq!((fk, rows), (0, 1));
        }
        session.close().await;
        assert_eq!(
            resource_file_sql(".mode box\nSELECT 1;\n  .quit"),
            "SELECT 1;"
        );
        p.options.sqlite_resource_file = Some(dir.path().join("none").to_string_lossy().into());
        let err = LiteSession::open(&p).await.err().unwrap();
        assert!(err.to_string().contains("resource file"), "{err}");
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
