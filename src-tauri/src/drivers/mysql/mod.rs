//! MySQL / MariaDB driver (mysql_async). A session owns a small worker pool for browsing and
//! introspection plus one dedicated editor connection whose session state (`BEGIN`, `SET …`)
//! persists between statements.

mod connect;
mod decode;
mod editor;
mod exec;
mod introspect;
mod params;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, Pool, Row, TxOpts};
use tokio::sync::Mutex;

use crate::drivers::{
    CANCEL_SETTLE_LIMIT, CancelHandle, CancelSlot, ConnectTarget, DbSession, LATE_CANCEL_NOTICE,
    RowSink, ServerInfo, SqlTrace, StatementOutcome, TraceCounts, TxState, TxStatement, arm_cancel,
    cancel_requested, cancel_safe_on_stop, check_affected, classify_statement, settle_cancels,
    trace_sql, tx_effect,
};
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::sqlgen::Dialect;
use crate::types::browse::TracedSql;
use crate::types::profile::Driver;
use crate::types::query::{ExecOutcome, ResultSet, StatementKind};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub use connect::{cleartext_password_allowed, resolve_socket};
pub use decode::{kind_from_column, value_from_mysql};
pub use introspect::{kind_from_data_type, parse_enum_values};
pub use params::bind;

/// How long `close` waits for the server to acknowledge disconnects.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a pool replaced by a renewed one may take to wind down (its connections still in use
/// finish first).
const RETIRE_TIMEOUT: Duration = Duration::from_secs(120);
/// `ping` keeps the editor connection alive once the server says it has been idle this long
/// (seconds): `COM_PING` resets the `ROW_COUNT()` the user may read after a statement, so a
/// recently used editor is left alone.
const EDITOR_KEEPALIVE_IDLE_SECS: i64 = 60;

/// The worker pool with the options it connects with (also used for side connections when the
/// pool is exhausted). Replaced as a whole when the password of a "Command Line (IAM
/// authentication)" profile is renewed (`MySession::renew_after`); `generation` counts that.
#[derive(Clone)]
struct Workers {
    pool: Pool,
    opts: Opts,
    generation: u64,
}

impl Workers {
    fn killer(&self, conn_id: u32) -> editor::Killer<'_> {
        editor::Killer {
            pool: &self.pool,
            opts: &self.opts,
            conn_id,
        }
    }
}

pub struct MySession {
    workers: std::sync::Mutex<Workers>,
    /// Held while the workers are renewed, so one renewal serves everyone waiting.
    renewing: Mutex<()>,
    /// Where new connections get their password when the profile's comes from a command.
    token: Option<Arc<CommandPassword>>,
    /// Dedicated SQL-editor connection; `None` once closed.
    editor: Mutex<Option<Conn>>,
    /// Server-side id of the editor connection, for `KILL QUERY <id>` from a pooled connection.
    editor_conn_id: u32,
    is_mariadb: bool,
    /// Editor transaction state, tracked from the statements it runs (heuristic).
    tx_state: TxState,
    /// Set once the editor connection broke (server restart, `wait_timeout`, `KILL`, network):
    /// the editor then reports `NotConnected` and `ping` fails, so the connection shows as lost.
    editor_lost: AtomicBool,
    /// Set by `close`: statements still waiting for the editor give up.
    closing: AtomicBool,
    /// Held by one editor run at a time (`lock_editor_runs`).
    editor_runs: Mutex<()>,
    /// Idle seconds after which `ping` pings the editor ([`EDITOR_KEEPALIVE_IDLE_SECS`]).
    editor_keepalive_idle_secs: i64,
    /// The statements this session ran for the app, for the console log (see `SqlTrace`).
    trace: SqlTrace,
}

impl MySession {
    pub async fn open(target: &ConnectTarget<'_>) -> Result<MySession, AppError> {
        let opened = connect::open(target).await?;
        Ok(MySession {
            workers: std::sync::Mutex::new(Workers {
                pool: opened.pool,
                opts: opened.pool_opts,
                generation: 0,
            }),
            renewing: Mutex::new(()),
            token: CommandPassword::for_profile(target.profile, target.password),
            editor_conn_id: opened.editor.id(),
            editor: Mutex::new(Some(opened.editor)),
            is_mariadb: opened.is_mariadb,
            tx_state: TxState::default(),
            editor_lost: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            editor_runs: Mutex::new(()),
            editor_keepalive_idle_secs: EDITOR_KEEPALIVE_IDLE_SECS,
            trace: SqlTrace::default(),
        })
    }

    /// The dedicated SQL-editor connection.
    pub fn editor(&self) -> &Mutex<Option<Conn>> {
        &self.editor
    }

    pub fn editor_conn_id(&self) -> u32 {
        self.editor_conn_id
    }

    pub fn is_mariadb(&self) -> bool {
        self.is_mariadb
    }

    fn workers(&self) -> Workers {
        self.workers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// A worker connection. When the pool cannot log in a new one because the password command's
    /// token expired, the token is renewed (`renew_after`) and the connection retried once.
    async fn conn(&self) -> Result<Conn, AppError> {
        let workers = self.workers();
        let error = match workers.pool.get_conn().await {
            Ok(conn) => return Ok(conn),
            Err(e) => AppError::from(e),
        };
        match self.renew_after(&workers, &error).await? {
            Some(renewed) => Ok(renewed.pool.get_conn().await?),
            None => Err(error),
        }
    }

    /// After `failed` could not log in a new connection (`error`): when the profile's password
    /// comes from a command whose last output is older than the TTL (an expired IAM token),
    /// runs the command again and replaces the pool and the side-connection options with ones
    /// that use its new output (mysql_async cannot change a pool's password). The old pool winds
    /// down in the background; connections already open (the editor's too) stay as they are.
    /// Returns the workers to retry with, or `None` when renewing cannot help (any other error, a
    /// password that is not from a command, or one that is still fresh and was refused anyway).
    async fn renew_after(
        &self,
        failed: &Workers,
        error: &AppError,
    ) -> Result<Option<Workers>, AppError> {
        let Some(token) = &self.token else {
            return Ok(None);
        };
        if !matches!(error, AppError::Auth(_)) || self.closing.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let _one = self.renewing.lock().await;
        let current = self.workers();
        if current.generation != failed.generation {
            // Renewed while this caller waited.
            return Ok(Some(current));
        }
        if !token.is_stale().await {
            return Ok(None);
        }
        let password = token.password().await?;
        let opts: Opts = OptsBuilder::from_opts(current.opts.clone())
            .pass(Some(password))
            .into();
        let renewed = Workers {
            pool: Pool::new(opts.clone()),
            opts,
            generation: current.generation + 1,
        };
        let retired = std::mem::replace(
            &mut *self.workers.lock().unwrap_or_else(|p| p.into_inner()),
            renewed.clone(),
        );
        log::info!("mysql: renewed the password command's token for new connections");
        tokio::spawn(async move {
            let _ = tokio::time::timeout(RETIRE_TIMEOUT, retired.pool.disconnect()).await;
        });
        Ok(Some(renewed))
    }

    fn editor_unusable(&self) -> bool {
        self.closing.load(Ordering::SeqCst) || self.editor_lost.load(Ordering::SeqCst)
    }

    /// Pings the editor connection so `wait_timeout` does not end it, when it is idle: no
    /// editor run holds it (never waits behind one) and the server reports it has slept for
    /// [`EDITOR_KEEPALIVE_IDLE_SECS`] (or cannot say). A failed ping marks the editor lost.
    async fn keep_editor_alive(&self, pooled: &mut Conn) -> Result<(), AppError> {
        let Ok(_run) = self.editor_runs.try_lock() else {
            return Ok(());
        };
        let Ok(mut guard) = self.editor.try_lock() else {
            return Ok(());
        };
        if self.editor_unusable() {
            return Ok(());
        }
        let Some(conn) = guard.as_mut() else {
            return Ok(());
        };
        // No row: the server already ended the editor connection; the ping below finds out.
        let idle: Result<Option<i64>, _> = pooled
            .exec_first(
                "SELECT TIME FROM information_schema.PROCESSLIST WHERE ID = ?",
                (self.editor_conn_id,),
            )
            .await;
        if let Ok(Some(secs)) = idle
            && secs < self.editor_keepalive_idle_secs
        {
            return Ok(());
        }
        if let Err(e) = conn.ping().await {
            log::warn!("mysql: the editor connection was lost: {e}");
            guard.take();
            self.editor_lost.store(true, Ordering::SeqCst);
            return Err(AppError::NotConnected);
        }
        Ok(())
    }
}

pub async fn connect(target: ConnectTarget<'_>) -> Result<Arc<dyn DbSession>, AppError> {
    Ok(Arc::new(MySession::open(&target).await?))
}

async fn settle(cancel: &CancelSlot) {
    if !settle_cancels(cancel, CANCEL_SETTLE_LIMIT).await {
        log::warn!("mysql: a KILL request did not finish in time");
    }
}

#[async_trait]
impl DbSession for MySession {
    fn driver(&self) -> Driver {
        Driver::Mysql
    }

    fn dialect(&self) -> Dialect {
        Dialect::Mysql
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        let mut conn = self.conn().await?;
        let row: Option<Row> = conn
            .query_first("SELECT VERSION(), DATABASE(), CURRENT_USER()")
            .await?;
        let mut row = row.ok_or_else(|| AppError::internal("SELECT VERSION() returned no row"))?;
        let mut text = |i: usize| {
            row.take::<mysql_async::Value, _>(i)
                .and_then(decode::text_of)
        };
        let raw = text(0).unwrap_or_default();
        let current_database = text(1);
        let current_user = text(2);
        let is_mariadb = self.is_mariadb || connect::is_mariadb_version(&raw);
        Ok(ServerInfo {
            version: connect::display_version(&raw, is_mariadb),
            current_database,
            current_user,
        })
    }

    /// Checks the pool and the editor, and keeps an idle editor connection alive (see
    /// `keep_editor_alive`: only after a minute of idling, since `COM_PING` resets the
    /// `ROW_COUNT()` the user may read next). A broken editor connection is also noticed by the
    /// statement that hits it (`run_statement` sets `editor_lost`) and reported here.
    async fn ping(&self) -> Result<(), AppError> {
        if self.editor_lost.load(Ordering::SeqCst) {
            return Err(AppError::NotConnected);
        }
        let mut conn = self.conn().await?;
        conn.ping().await?;
        self.keep_editor_alive(&mut conn).await
    }

    /// Closes the editor and the pool. A statement still running on the editor is ended first
    /// (`KILL CONNECTION`), so closing (disconnect, database switch) does not wait for it.
    async fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        let editor = match self.editor.try_lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => {
                let workers = self.workers();
                if let Err(e) =
                    editor::kill_connection(&workers.pool, &workers.opts, self.editor_conn_id).await
                {
                    log::warn!("mysql: ending the editor statement on close failed: {e}");
                }
                match tokio::time::timeout(CLOSE_TIMEOUT, self.editor.lock()).await {
                    Ok(mut guard) => guard.take(),
                    Err(_) => {
                        log::warn!("mysql: the editor statement did not stop before closing");
                        None
                    }
                }
            }
        };
        if let Some(conn) = editor {
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, conn.disconnect()).await;
        }
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, self.workers().pool.disconnect()).await;
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        let mut conn = self.conn().await?;
        introspect::list_databases(&mut introspect::Traced::new(&mut conn, &self.trace)).await
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        let mut conn = self.conn().await?;
        introspect::list_schemas(&mut introspect::Traced::new(&mut conn, &self.trace)).await
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let mut conn = self.conn().await?;
        introspect::list_tables(&mut introspect::Traced::new(&mut conn, &self.trace), schema).await
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let mut conn = self.conn().await?;
        introspect::table_meta(&mut introspect::Traced::new(&mut conn, &self.trace), table).await
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let mut conn = self.conn().await?;
        introspect::view_definition(&mut introspect::Traced::new(&mut conn, &self.trace), table)
            .await
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let mut conn = self.conn().await?;
        introspect::completion_metadata(&mut introspect::Traced::new(&mut conn, &self.trace)).await
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError> {
        let mut conn = self.conn().await?;
        self.trace
            .run_async(
                classify_statement(sql, Dialect::Mysql),
                trace_sql(Dialect::Mysql, sql, params),
                |rs: &ResultSet| TraceCounts::rows(rs.rows.len()),
                exec::run_query(&mut conn, sql, params),
            )
            .await
    }

    async fn query_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ResultSet, AppError> {
        let mut conn = self.conn().await?;
        let conn_id = conn.id();
        let result = {
            let _armed = arm_cancel(&cancel, CancelHandle::Mysql { conn_id })?;
            self.trace
                .run_async(
                    classify_statement(sql, Dialect::Mysql),
                    trace_sql(Dialect::Mysql, sql, params),
                    |rs: &ResultSet| TraceCounts::rows(rs.rows.len()),
                    exec::run_query(&mut conn, sql, params),
                )
                .await
        };
        if cancel_requested(&cancel) && !conn.is_disconnected() {
            // The connection goes back to the pool: no late KILL may follow it there.
            settle(&cancel).await;
            editor::absorb_stray_kill(&mut conn).await;
            // A killed `SELECT SLEEP(n)` ends without an error: a read the user (or the query
            // time out) cancelled is reported as cancelled, as `run_statement` does.
            if result.is_ok() && cancel_safe_on_stop(sql, Dialect::Mysql) {
                return Err(AppError::Cancelled);
            }
        }
        result
    }

    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        let mut conn = self.conn().await?;
        self.trace
            .run_async(
                classify_statement(sql, Dialect::Mysql),
                trace_sql(Dialect::Mysql, sql, params),
                TraceCounts::of_exec,
                exec::run_execute(&mut conn, sql, params),
            )
            .await
    }

    async fn execute_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ExecOutcome, AppError> {
        let mut conn = self.conn().await?;
        let conn_id = conn.id();
        let result = {
            let _armed = arm_cancel(&cancel, CancelHandle::Mysql { conn_id })?;
            self.trace
                .run_async(
                    classify_statement(sql, Dialect::Mysql),
                    trace_sql(Dialect::Mysql, sql, params),
                    TraceCounts::of_exec,
                    exec::run_execute(&mut conn, sql, params),
                )
                .await
        };
        if cancel_requested(&cancel) && !conn.is_disconnected() {
            // The connection goes back to the pool: no late KILL may follow it there.
            settle(&cancel).await;
            editor::absorb_stray_kill(&mut conn).await;
        }
        result
    }

    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        let mut conn = self.conn().await?;
        let mut tx = conn.start_transaction(TxOpts::default()).await?;
        let mut out = Vec::with_capacity(statements.len());
        for (index, st) in statements.iter().enumerate() {
            match self
                .trace
                .run_async(
                    classify_statement(&st.sql, Dialect::Mysql),
                    trace_sql(Dialect::Mysql, &st.sql, &st.params),
                    TraceCounts::of_exec,
                    exec::run_execute(&mut tx, &st.sql, &st.params),
                )
                .await
                .and_then(|o| {
                    check_affected(st, &o, index)?;
                    Ok(o)
                }) {
                Ok(outcome) => out.push(outcome),
                Err(e) => {
                    // Dropping the transaction would also roll back; do it explicitly so the
                    // pooled connection is clean when it is returned.
                    let _ = tx.rollback().await;
                    return Err(e.with_statement_index(index));
                }
            }
        }
        tx.commit().await?;
        Ok(out)
    }

    async fn run_statement(
        &self,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        let mut guard = self.editor.lock().await;
        if self.editor_unusable() {
            return Err(AppError::NotConnected);
        }
        let conn = guard.as_mut().ok_or(AppError::NotConnected)?;
        let workers = self.workers();
        let killer = workers.killer(self.editor_conn_id);
        let result = {
            let _armed = arm_cancel(
                &cancel,
                CancelHandle::Mysql {
                    conn_id: self.editor_conn_id,
                },
            )?;
            editor::run(conn, sql, &[], Some(max_rows), sink, &killer, true).await
        };
        if conn.is_disconnected() {
            // mysql_async closes the stream on any I/O or protocol error: the editor is gone.
            log::warn!("mysql: the editor connection was lost: {result:?}");
            guard.take();
            self.editor_lost.store(true, Ordering::SeqCst);
            return Err(AppError::NotConnected);
        }
        let result = if cancel_requested(&cancel) {
            // Every KILL sent for this run must have landed before the editor runs anything
            // else, or it would abort the next statement (another run's).
            settle(&cancel).await;
            editor::absorb_stray_kill(conn).await;
            match result {
                // A killed `SELECT SLEEP(n)` ends without an error; report what the user asked
                // for. Only a provably read-only statement may be reported that way: anything
                // else (`CALL`, a `DELIMITER` batch, DML) may have committed changes that a
                // "cancelled" result would hide, so it keeps its real outcome.
                Ok(outcome)
                    if outcome.kind == StatementKind::Rows
                        && cancel_safe_on_stop(sql, Dialect::Mysql) =>
                {
                    Err(AppError::Cancelled)
                }
                Ok(mut outcome) => {
                    outcome.notices.push(LATE_CANCEL_NOTICE.to_string());
                    Ok(outcome)
                }
                other => other,
            }
        } else {
            result
        };
        if let Some(open) = tx_effect(sql, Dialect::Mysql, result.as_ref().err()) {
            self.tx_state.set(Some(open));
        }
        result
    }

    async fn stream_query(
        &self,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<u64, AppError> {
        let mut conn = self.conn().await?;
        let conn_id = conn.id();
        let workers = self.workers();
        let killer = workers.killer(conn_id);
        let result = {
            let _armed = arm_cancel(&cancel, CancelHandle::Mysql { conn_id })?;
            self.trace
                .run_async(
                    classify_statement(sql, Dialect::Mysql),
                    trace_sql(Dialect::Mysql, sql, params),
                    |o: &StatementOutcome| TraceCounts::rows(o.row_count as usize),
                    editor::run(&mut conn, sql, params, None, sink, &killer, false),
                )
                .await
        };
        if cancel_requested(&cancel) && !conn.is_disconnected() {
            // The connection goes back to the pool: no late KILL may follow it there.
            settle(&cancel).await;
            editor::absorb_stray_kill(&mut conn).await;
        }
        Ok(result?.row_count)
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        // The ticket (if any) is released once the KILL has been sent.
        let (handle, _ticket) = handle.into_parts();
        match handle {
            CancelHandle::Mysql { conn_id } => {
                let workers = self.workers();
                let Err(error) = workers.killer(conn_id).kill().await else {
                    return Ok(());
                };
                // A side connection may need a renewed password-command token.
                match self.renew_after(&workers, &error).await? {
                    Some(renewed) => renewed.killer(conn_id).kill().await,
                    None => Err(error),
                }
            }
            _ => Ok(()),
        }
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

    fn is_mariadb(&self) -> bool {
        self.is_mariadb
    }

    fn editor_run_lock(&self) -> Option<&Mutex<()>> {
        Some(&self.editor_runs)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::drivers::CollectSink;
    use crate::types::profile::{ConnectionProfile, ProfileOptions, SslMode};

    /// The profile and password of `TABLER_TEST_MYSQL_URL` (`mysql://user[:pass]@host:port/db`);
    /// None (the test passes) when it is unset.
    fn url_profile() -> Option<(ConnectionProfile, Option<String>)> {
        let Ok(url) = std::env::var("TABLER_TEST_MYSQL_URL") else {
            println!("skipped: TABLER_TEST_MYSQL_URL unset");
            return None;
        };
        let rest = url.strip_prefix("mysql://").expect("a mysql:// URL");
        let (creds, hostdb) = rest.rsplit_once('@').unwrap_or(("", rest));
        let (user, password) = creds
            .split_once(':')
            .map_or((creds, None), |(u, p)| (u, Some(p.to_string())));
        let (hostport, db) = hostdb.split_once('/').unwrap_or((hostdb, ""));
        let (host, port) = hostport.rsplit_once(':').unwrap_or((hostport, "3306"));
        let profile = ConnectionProfile {
            id: uuid::Uuid::new_v4(),
            name: "mysql-keepalive".into(),
            color: None,
            group: None,
            driver: Driver::Mysql,
            host: Some(host.into()),
            port: port.parse().ok(),
            user: Some(user.into()).filter(|u: &String| !u.is_empty()),
            database: Some(db.into()).filter(|d: &String| !d.is_empty()),
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: None,
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        };
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok();
        Some((profile, password))
    }

    /// A session on `TABLER_TEST_MYSQL_URL`; None (the test passes) when it is unset.
    async fn open() -> Option<MySession> {
        let (profile, password) = url_profile()?;
        let target = ConnectTarget {
            profile: &profile,
            password: password.as_deref(),
            host_override: None,
        };
        Some(MySession::open(&target).await.expect("connect"))
    }

    /// Regression: the password of a "Command Line (IAM authentication)" profile was frozen into
    /// the pool, so pool connections opened after the token expired failed to log in. A refused
    /// token older than the TTL is renewed (the command runs again) and the pool rebuilt.
    #[tokio::test]
    async fn an_expired_command_token_is_renewed_for_new_pool_connections() {
        let Some(admin) = open().await else {
            return;
        };
        let (mut profile, _) = url_profile().unwrap();
        let user = "fx_conn_iam";
        let create = format!("CREATE USER IF NOT EXISTS '{user}'@'%' IDENTIFIED BY 'tok-a'");
        if let Err(e) = admin.execute(&create, &[]).await {
            println!("skipped: cannot create a test user ({e})");
            admin.close().await;
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("token");
        std::fs::write(&file, "tok-a\n").unwrap();
        profile.user = Some(user.into());
        profile.database = None;
        profile.options.password_mode = Some(crate::types::profile::PasswordMode::Command);
        let command = format!("cat '{}'", file.display());
        profile.options.password_command = Some(command.clone());
        let target = ConnectTarget {
            profile: &profile,
            password: Some("tok-a"),
            host_override: None,
        };
        let outcome = async {
            let mut session = MySession::open(&target).await?;
            // A TTL short enough for the test.
            session.token = Some(Arc::new(
                CommandPassword::new(&command, Duration::from_millis(300)).seeded(Some("tok-a")),
            ));
            admin
                .execute(
                    &format!("ALTER USER '{user}'@'%' IDENTIFIED BY 'tok-b'"),
                    &[],
                )
                .await?;
            // A fresh token that is refused is not renewed (the password is wrong, not old).
            let err = session.list_databases().await.unwrap_err();
            assert!(matches!(err, AppError::Auth(_)), "{err:?}");
            assert_eq!(session.workers().generation, 0);
            // Once it is old, the command runs again and the pool is rebuilt with its output.
            std::fs::write(&file, "tok-b\n").unwrap();
            tokio::time::sleep(Duration::from_millis(400)).await;
            session.list_databases().await?;
            assert_eq!(session.workers().generation, 1);
            assert_eq!(session.workers().opts.pass(), Some("tok-b"));
            // The editor connection, open since the connect, still works.
            assert_eq!(on_editor(&session, "SELECT 1").await, Some(Value::Int(1)));
            session.close().await;
            Ok::<_, AppError>(())
        }
        .await;
        let _ = admin
            .execute(&format!("DROP USER IF EXISTS '{user}'@'%'"), &[])
            .await;
        admin.close().await;
        outcome.unwrap();
    }

    /// Runs `sql` on the editor; the first value of its first row.
    async fn on_editor(session: &MySession, sql: &str) -> Option<Value> {
        let mut sink = CollectSink::default();
        session
            .run_statement(sql, 10, &mut sink, CancelSlot::default())
            .await
            .unwrap();
        sink.sets.first()?.rows.first()?.first().cloned()
    }

    #[tokio::test]
    async fn ping_keeps_the_row_count_of_a_recent_statement() {
        let Some(session) = open().await else {
            return;
        };
        on_editor(&session, "CREATE TEMPORARY TABLE fix_security_rc (a int)").await;
        on_editor(&session, "INSERT INTO fix_security_rc VALUES (1), (2), (3)").await;
        session.ping().await.unwrap();
        let count = on_editor(&session, "SELECT ROW_COUNT()").await;
        assert_eq!(count, Some(Value::Int(3)));
        session.close().await;
    }

    #[tokio::test]
    async fn ping_keeps_an_idle_editor_past_wait_timeout() {
        let Some(mut session) = open().await else {
            return;
        };
        session.editor_keepalive_idle_secs = 1;
        on_editor(&session, "SET SESSION wait_timeout = 3").await;
        // 6 s idle in all: the server would have ended the editor after 3 s.
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_millis(1_500)).await;
            session.ping().await.unwrap();
        }
        assert_eq!(on_editor(&session, "SELECT 1").await, Some(Value::Int(1)));
        session.close().await;
    }

    #[tokio::test]
    async fn ping_does_not_wait_for_a_running_editor_statement() {
        let Some(session) = open().await else {
            return;
        };
        let session = Arc::new(session);
        let running = tokio::spawn({
            let session = session.clone();
            async move { on_editor(&session, "SELECT SLEEP(2)").await }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let started = Instant::now();
        session.ping().await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        running.await.unwrap();
        session.close().await;
    }
}
