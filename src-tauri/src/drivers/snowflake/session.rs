//! The Snowflake session: the editor's own Snowflake session, whose `USE`, `ALTER SESSION`,
//! variables, temporary tables and transactions last from one run to the next, and a second one
//! for what the app runs on its own (introspection, the grid, counts, commits), so that a `USE`
//! or an open transaction in the editor never moves or holds the grid. An account whose password
//! sign-in asks MFA signs in once (every sign-in would ask again), and both share it.
//!
//! Snowflake ends idle sessions (`390112`); the session then signs in again and runs the startup
//! commands and the editor's `USE` and `ALTER SESSION` statements again (`Live::renew`). Only a
//! read Snowflake refused before accepting it (no query id) is sent once more: a statement it had
//! accepted keeps running after the expiry, so it is reported as possibly done rather than run
//! twice. An open transaction is gone with the old session, which the editor reports instead of
//! running on without it.
//!
//! Read-only is Tabler's classifier alone (Snowflake has no read-only session): every statement
//! the classifier calls a write or does not know is refused here before anything is sent, on
//! every path. Introspection uses `SHOW`, which runs without a warehouse; `ping` is `SELECT 1`,
//! which no warehouse resumes for, and the app never pings on its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

use super::catalog::{self, Rows, SessionState};
use super::conn::{Outcome, Part, SnowflakeCancel, SnowflakeConn};
use super::editor;
use super::errors::StatementError;
use super::signin::SignIn;
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::conn_options::effective_read_only;
use crate::drivers::{
    CancelHandle, CancelSlot, ConnectTarget, DbSession, EditorRunGuard, RowChunker, RowSink,
    ServerInfo, SqlTrace, StatementOutcome, TraceCounts, TxState, TxStatement, arm_cancel,
    cancel_portable, cancel_requested, check_affected, classify_statement, is_dml, trace_sql,
};
use crate::error::AppError;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::snowflake::{self as sf, DdlKind, SchemaObjects, TableObjects};
use crate::sqlgen::sql::{is_single_statement, max_params, split_statements, writes};
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::browse::{FilterOp, TraceCategory, TracedSql};
use crate::types::caps::{
    Affected, CountMode, DataCaps, Feature, LifecycleCaps, NamespaceCaps, NamespaceLabels, Paging,
    RawFilter, SafetyCaps, SchemaMode, SessionCaps, TxMode,
};
use crate::types::driver::ReadOnlyMode;
use crate::types::objects::{RoutineKind, RoutineRef, RoutineSummary};
use crate::types::profile::{ConnectionProfile, Driver};
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableKind, TableMeta, TableRef, TableSummary,
};
use crate::types::security::SessionDetails;
use crate::types::value::Value;

pub(super) const DIALECT: SqlDialect = SqlDialect::Snowflake;

/// How long `close` waits for the editor's statement to stop.
const CLOSE_WITHIN: Duration = Duration::from_secs(2);

/// SHOW pages followed at most (10,000 names each).
const MAX_SHOW_PAGES: usize = 20;

/// How many `USE` / `ALTER SESSION` statements of the editor are kept to run again.
const MAX_REPLAY: usize = 64;

/// What the session offers, each proven by the lane's tests against the fake Snowflake
/// (docs/drivers/snowflake.md, Phase B status). Not `Returning` (Snowflake has none), not
/// `EditWithoutKey` (rows are addressed by primary key only, and a key that is only
/// informational is guarded by the count before the commit), not `ServerExpressions` (no
/// `DEFAULT` keyword in VALUES or SET).
const FEATURES: [Feature; 17] = [
    Feature::Editor,
    Feature::Explain,
    Feature::CancelOnServer,
    Feature::EditRows,
    Feature::InsertRows,
    Feature::DeleteRows,
    Feature::ExactCount,
    Feature::StructureView,
    Feature::TableDdl,
    Feature::ViewDefinition,
    Feature::Routines,
    Feature::SwitchDatabase,
    Feature::ProcessList,
    Feature::KillProcess,
    Feature::SessionDetails,
    Feature::StartupCommands,
    Feature::Mcp,
];

/// Every filter but JSON containment, which Snowflake spells with functions of its own.
const FILTER_OPS: [FilterOp; 23] = [
    FilterOp::Eq,
    FilterOp::Ne,
    FilterOp::Gt,
    FilterOp::Lt,
    FilterOp::Ge,
    FilterOp::Le,
    FilterOp::Like,
    FilterOp::NotLike,
    FilterOp::In,
    FilterOp::NotIn,
    FilterOp::IsNull,
    FilterOp::IsNotNull,
    FilterOp::Between,
    FilterOp::NotBetween,
    FilterOp::Contains,
    FilterOp::NotContains,
    FilterOp::ContainsCase,
    FilterOp::NotContainsCase,
    FilterOp::HasPrefix,
    FilterOp::HasSuffix,
    FilterOp::HasPrefixCase,
    FilterOp::HasSuffixCase,
    FilterOp::Ilike,
];

/// Why the app's write waits on a shared (MFA) session.
fn transaction_open_error() -> AppError {
    AppError::InvalidInput(
        "Snowflake: the editor has a transaction open on this session: commit or roll it back first"
            .into(),
    )
}

pub(super) fn read_only_error() -> AppError {
    AppError::InvalidInput(READ_ONLY_MESSAGE.into())
}

/// One signed-in Snowflake session that can be replaced by a new sign-in, with what a new one
/// runs first: the startup commands, then (the editor's) `USE` and `ALTER SESSION` statements.
pub(super) struct Live {
    conn: RwLock<Arc<SnowflakeConn>>,
    startup: Vec<String>,
    replay: std::sync::Mutex<Vec<String>>,
}

pub(super) fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Live {
    /// Signs in and runs the startup commands.
    async fn open(signin: &SignIn, startup: Vec<String>) -> Result<Live, AppError> {
        let conn = signin.connect().await?;
        for sql in &startup {
            conn.query(sql, &[], Some(0)).await?;
        }
        Ok(Live {
            conn: RwLock::new(Arc::new(conn)),
            startup,
            replay: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub(super) async fn current(&self) -> Arc<SnowflakeConn> {
        Arc::clone(&*self.conn.read().await)
    }

    /// A new session in place of `stale` (another task may have replaced it already), set up as
    /// the old one was.
    pub(super) async fn renew(
        &self,
        signin: &SignIn,
        stale: &Arc<SnowflakeConn>,
    ) -> Result<Arc<SnowflakeConn>, AppError> {
        self.replace(signin, stale, || {}).await
    }

    /// `renew`, calling `on_replace` (the old session held, before the new sign-in) only when
    /// this call is the one that replaces `stale`.
    async fn replace(
        &self,
        signin: &SignIn,
        stale: &Arc<SnowflakeConn>,
        on_replace: impl FnOnce(),
    ) -> Result<Arc<SnowflakeConn>, AppError> {
        let mut slot = self.conn.write().await;
        if !Arc::ptr_eq(&slot, stale) {
            return Ok(Arc::clone(&slot));
        }
        on_replace();
        let conn = signin.connect().await?;
        let replay = lock(&self.replay).clone();
        for sql in self.startup.iter().chain(&replay) {
            conn.query(sql, &[], Some(0)).await?;
        }
        *slot = Arc::new(conn);
        Ok(Arc::clone(&slot))
    }

    /// Keeps a statement of the editor's that set the session up, to run again after a new
    /// sign-in.
    pub(super) fn remember(&self, sql: &str) {
        let mut replay = lock(&self.replay);
        replay.retain(|s| s != sql);
        replay.push(sql.to_string());
        if replay.len() > MAX_REPLAY {
            replay.remove(0);
        }
    }
}

pub struct SnowflakeSession {
    pub(super) signin: SignIn,
    pub(super) editor: Live,
    /// The app's own session; None: it shares the editor's (an MFA sign-in).
    worker: Option<Live>,
    state: SessionState,
    pub(super) read_only: bool,
    pub(super) tx: TxState,
    /// On a shared (MFA) session: the app signed in again while the editor's transaction was open
    /// (or not known), so it went with the old session. The editor's next statement says so.
    tx_lost: AtomicBool,
    pub(super) trace: SqlTrace,
    editor_runs: Mutex<()>,
    /// One commit at a time on the app's session, and no other write of the app's inside it.
    commits: Mutex<()>,
    /// What the editor's running statement is stopped with, for `close`.
    pub(super) editor_stop: std::sync::Mutex<Option<SnowflakeCancel>>,
    /// The database the editor left the session in, as last read (None: not known since a
    /// statement that may have moved it). Checked on a shared (MFA) session only.
    editor_database: std::sync::Mutex<Option<Option<String>>>,
    /// On a shared (MFA) session: held for writing while an editor statement that may move the
    /// session runs and is read back, for reading while the app sends a read.
    moving: RwLock<()>,
}

/// Whether a text is more than one statement (`;` or the flow operator `->>`): the grid's raw
/// WHERE and an exported query are the user's text, and Safe Mode sees neither.
fn refuse_added_statement(sql: &str) -> Result<(), AppError> {
    if !is_single_statement(DIALECT, sql) {
        return Err(AppError::InvalidInput(
            "A filter or an exported query can only read rows: this one would run another statement"
                .into(),
        ));
    }
    Ok(())
}

impl SnowflakeSession {
    /// Signs in (twice, unless the sign-in asks MFA) and reads where the session stands.
    pub async fn open(target: &ConnectTarget<'_>) -> Result<SnowflakeSession, AppError> {
        let signin = SignIn::for_target(target)?;
        SnowflakeSession::start(target.profile, signin).await
    }

    pub async fn start(
        profile: &ConnectionProfile,
        signin: SignIn,
    ) -> Result<SnowflakeSession, AppError> {
        let read_only = effective_read_only(profile);
        let startup: Vec<String> = profile
            .options
            .startup_sql
            .as_deref()
            .map(|text| {
                split_statements(DIALECT, text)
                    .into_iter()
                    .map(|s| s.sql)
                    .collect()
            })
            .unwrap_or_default();
        // The startup commands run at every sign-in, before any check of the session's.
        if startup.iter().any(|s| editor::sets_autocommit(s)) {
            return Err(editor::autocommit_error());
        }
        if read_only && startup.iter().any(|s| writes(DIALECT, s)) {
            return Err(AppError::InvalidInput(
                "Snowflake: this connection is read-only, and its startup commands write (or are \
                 not known to only read)"
                    .into(),
            ));
        }
        let editor = Live::open(&signin, startup.clone()).await?;
        let trace = SqlTrace::default();
        let conn = editor.current().await;
        let rows = trace
            .run_async(
                TraceCategory::Structure,
                sf::SERVER_INFO,
                |rows: &Rows| TraceCounts::rows(rows.rows.len()),
                async {
                    conn.query(sf::SERVER_INFO, &[], None)
                        .await
                        .map(Outcome::into_rows)
                        .map_err(AppError::from)
                },
            )
            .await?;
        let state = catalog::session_state(&rows).ok_or_else(|| {
            AppError::Internal("Snowflake: the session's state came back empty".into())
        })?;
        let worker = if signin.asks_mfa() {
            None
        } else {
            Some(Live::open(&signin, startup).await?)
        };
        Ok(SnowflakeSession {
            signin,
            editor,
            worker,
            read_only,
            tx: TxState::default(),
            tx_lost: AtomicBool::new(false),
            trace,
            editor_runs: Mutex::new(()),
            commits: Mutex::new(()),
            editor_stop: std::sync::Mutex::new(None),
            editor_database: std::sync::Mutex::new(Some(state.database.clone())),
            moving: RwLock::new(()),
            state,
        })
    }

    fn worker(&self) -> &Live {
        self.worker.as_ref().unwrap_or(&self.editor)
    }

    /// Signs the app's session in again in place of `stale`. On a shared (MFA) session that is
    /// the editor's too: a transaction it held is gone, and its next statement must not run on,
    /// autocommitted, in the new session as if it were still inside it.
    async fn renew_worker(
        &self,
        stale: &Arc<SnowflakeConn>,
    ) -> Result<Arc<SnowflakeConn>, AppError> {
        self.worker()
            .replace(&self.signin, stale, || {
                if self.worker.is_none() && self.tx.get() != Some(false) {
                    self.tx_lost.store(true, Ordering::SeqCst);
                }
            })
            .await
    }

    /// After an editor statement that ran on `ran_on`: when the app signed the shared session in
    /// again meanwhile, `on_replace` read the transaction from before the statement, so one the
    /// statement opened (or left not known) went with the old session unseen. Called once `tx`
    /// holds the statement's outcome: a later sign-in again sees it there. `succeeded`: the
    /// statement returned without an error.
    pub(super) async fn editor_ran_on(&self, ran_on: &Arc<SnowflakeConn>, succeeded: bool) {
        if Arc::ptr_eq(ran_on, &self.editor.current().await) {
            return;
        }
        // Read back from the old session.
        self.forget_database();
        if succeeded && self.tx.get() == Some(false) {
            // The statement itself ended the transaction on the old session (a COMMIT, DDL or
            // ROLLBACK the sign-in raced): nothing went with it, and saying "run it again" would
            // apply a commit twice.
            self.tx_lost.store(false, Ordering::SeqCst);
        } else if self.tx.get() != Some(false) {
            self.tx_lost.store(true, Ordering::SeqCst);
            self.tx.set(Some(false));
        }
    }

    /// Whether the app's new sign-in took the editor's transaction with it (`tx_lost`); cleared.
    pub(super) fn take_lost_transaction(&self) -> bool {
        self.tx_lost.swap(false, Ordering::SeqCst)
    }

    /// Refuses, on a read-only session, what the classifier does not call a read.
    pub(super) fn refuse_write(&self, sql: &str) -> Result<(), AppError> {
        if self.read_only && writes(DIALECT, sql) {
            Err(read_only_error())
        } else {
            Ok(())
        }
    }

    /// The database the app's statements name (the session's, which only a reconnect changes).
    fn database(&self) -> Result<&str, AppError> {
        self.state.database.as_deref().ok_or_else(|| {
            AppError::InvalidInput(
                "Snowflake: the session has no database: choose one in the connection".into(),
            )
        })
    }

    fn schema_of<'a>(&'a self, table: &'a TableRef) -> Result<&'a str, AppError> {
        table
            .schema
            .as_deref()
            .or(self.state.schema.as_deref())
            .ok_or_else(|| {
                AppError::InvalidInput(format!("Snowflake: {} has no schema", table.name))
            })
    }

    /// Runs one statement on the app's session, signing in again once when Snowflake expired it
    /// (and sending it again only when that cannot run it twice).
    /// `uses_database`: the statement names its table by schema only (the grid's), so on a shared
    /// session the new session is checked before it is sent again.
    async fn on_worker(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<&CancelSlot>,
        uses_database: bool,
    ) -> Result<Outcome, AppError> {
        let live = self.worker();
        let conn = live.current().await;
        match run_one(&conn, sql, params, cancel).await {
            Err(e) if e.expired => {
                let fresh = self.renew_worker(&conn).await?;
                // The new session ran the editor's USE statements again.
                self.forget_database();
                if !may_resend(&e, sql) {
                    return Err(may_have_run(&e));
                }
                if uses_database {
                    self.check_database(false).await?;
                }
                Ok(run_one(&fresh, sql, params, cancel).await?)
            }
            other => Ok(other?),
        }
    }

    /// On a shared (MFA) session, the editor's lock while the app runs a write, refused while the
    /// editor has a transaction open (the write would join it, and DDL would commit it) or has
    /// moved the session to another database.
    async fn shared_write_guard(&self) -> Result<Option<EditorRunGuard<'_>>, AppError> {
        if self.worker.is_some() {
            return Ok(None);
        }
        let guard = self.editor_runs.lock().await;
        if self.tx.get() == Some(true) {
            return Err(transaction_open_error());
        }
        // A transaction not known (its read-back failed) is read again here.
        self.check_database(true).await?;
        if self.tx.get() != Some(false) {
            return Err(transaction_open_error());
        }
        Ok(Some(guard))
    }

    /// Where the editor's statement may have moved the session: read again before the app's
    /// next statement on a shared session.
    pub(super) fn forget_database(&self) {
        *lock(&self.editor_database) = None;
    }

    /// Reads which database the editor's session `conn` is in, and keeps it; true when it holds
    /// a transaction (one AUTOCOMMIT = FALSE opened implicitly included).
    pub(super) async fn probe_editor(&self, conn: &SnowflakeConn) -> Result<bool, AppError> {
        let out = conn.query(sf::EDITOR_STATE, &[], Some(1)).await?;
        let Some([database, transaction, ..]) = out.rows.first().map(Vec::as_slice) else {
            return Err(AppError::Internal(
                "Snowflake: the session's database and transaction came back empty".into(),
            ));
        };
        let database = match database {
            Value::Text(name) => Some(name.clone()),
            _ => None,
        };
        *lock(&self.editor_database) = Some(database);
        Ok(!matches!(transaction, Value::Null))
    }

    /// On a shared (MFA) session, refuses the app's statement while the editor has left the
    /// session in another database than the session's: the grid's SQL names schema and table
    /// only, so it would read or change that database's table. `writing`: the caller holds the
    /// editor's lock, and a transaction found open refuses too.
    async fn check_database(&self, writing: bool) -> Result<(), AppError> {
        if self.worker.is_some() {
            return Ok(());
        }
        if lock(&self.editor_database).is_none() || self.tx.get().is_none() {
            // Kept whoever asks: a read that finds a transaction open must not let the app's next
            // commit join it. A BEGIN the editor saw stays open until its COMMIT or ROLLBACK.
            if self.probe_editor(&*self.editor.current().await).await? {
                self.tx.set(Some(true));
                if writing {
                    return Err(transaction_open_error());
                }
            } else if self.tx.get().is_none() {
                self.tx.set(Some(false));
            }
        }
        let current = lock(&self.editor_database).clone().flatten();
        if current == self.state.database {
            return Ok(());
        }
        let back = match &self.state.database {
            Some(db) => format!("run USE DATABASE {} in the editor", sf::qualified(&[db])),
            None => "reconnect".to_string(),
        };
        Err(AppError::InvalidInput(format!(
            "Snowflake: the editor moved this session to database {} (the app shares it, as the \
             sign-in asks MFA), and the app's statements would land there: {back} first",
            current.as_deref().unwrap_or("(none)")
        )))
    }

    /// On a shared (MFA) session, the lock an editor statement that may move the session holds
    /// until it has been read back. It waits for the app's reads in flight, which may take long
    /// (a count on a warehouse), so Cancel and the time out end the wait (as `lock_runs` does).
    pub(super) async fn moving(
        &self,
        may_move: bool,
        cancel: &CancelSlot,
    ) -> Result<Option<RwLockWriteGuard<'_, ()>>, AppError> {
        if self.worker.is_some() || !may_move {
            return Ok(None);
        }
        let acquire = self.moving.write();
        tokio::pin!(acquire);
        loop {
            let notified = cancel.0.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if cancel_requested(cancel) {
                return Err(AppError::Cancelled);
            }
            tokio::select! {
                biased;
                guard = &mut acquire => return Ok(Some(guard)),
                _ = notified => {}
            }
        }
    }

    /// `check_database` for a read, which does not wait for the editor: while the editor's
    /// statement that may move the session runs, the read is refused. The guard is held until
    /// the read has been answered, so the session cannot move under it.
    async fn check_database_for_read(&self) -> Result<Option<RwLockReadGuard<'_, ()>>, AppError> {
        if self.worker.is_some() {
            return Ok(None);
        }
        let Ok(reading) = self.moving.try_read() else {
            return Err(AppError::InvalidInput(
                "Snowflake: the editor is running a statement that may change this session's \
                 database (the app shares it, as the sign-in asks MFA): try again when it ends"
                    .into(),
            ));
        };
        self.check_database(false).await?;
        Ok(Some(reading))
    }

    /// A read the app runs, traced. The grid's raw WHERE is the user's text, so a read that
    /// holds another statement is refused on every session.
    async fn read(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<&CancelSlot>,
    ) -> Result<ResultSet, AppError> {
        self.refuse_write(sql)?;
        refuse_added_statement(sql)?;
        let _reading = self.check_database_for_read().await?;
        self.trace
            .run_async(
                classify_statement(sql, DIALECT),
                trace_sql(DIALECT, sql, params),
                |rs: &ResultSet| TraceCounts::rows(rs.rows.len()),
                async {
                    let out = self.on_worker(sql, params, cancel, true).await?;
                    Ok(ResultSet {
                        columns: out.columns,
                        rows: out.rows,
                        truncated: out.truncated,
                    })
                },
            )
            .await
    }

    /// A write the app runs, traced, counted by the `number of rows …` columns DML answers with.
    async fn write(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<&CancelSlot>,
    ) -> Result<ExecOutcome, AppError> {
        self.refuse_write(sql)?;
        // Not inside a commit of staged changes running on the same session.
        let _commit = self.commits.lock().await;
        let _editor = self.shared_write_guard().await?;
        self.trace
            .run_async(
                classify_statement(sql, DIALECT),
                trace_sql(DIALECT, sql, params),
                TraceCounts::of_exec,
                async {
                    let out = self.on_worker(sql, params, cancel, true).await?;
                    Ok(exec_outcome(sql, out))
                },
            )
            .await
    }

    /// A catalog statement's rows, traced as structure.
    async fn catalog(&self, sql: &str) -> Result<Rows, AppError> {
        self.trace
            .run_async(
                TraceCategory::Structure,
                sql,
                |rows: &Rows| TraceCounts::rows(rows.rows.len()),
                async { Ok(self.on_worker(sql, &[], None, false).await?.into_rows()) },
            )
            .await
    }

    /// Every page of a SHOW: a full page of 10,000 names goes on after its last name.
    async fn show_all(&self, sql: impl Fn(Option<&str>) -> String) -> Result<Rows, AppError> {
        let mut all = self.catalog(&sql(None)).await?;
        let mut page = all.rows.len();
        for _ in 1..MAX_SHOW_PAGES {
            if page < sf::SHOW_PAGE as usize {
                break;
            }
            let Some(last) = all.last_name() else { break };
            let next = self.catalog(&sql(Some(&last))).await?;
            page = next.rows.len();
            all.rows.extend(next.rows);
        }
        Ok(all)
    }

    fn record(
        &self,
        st: &TxStatement,
        started: std::time::Instant,
        outcome: Result<u64, &AppError>,
    ) {
        let traced = trace_sql(DIALECT, &st.sql, &st.params);
        let (affected, error) = match outcome {
            Ok(n) => (Some(n), None),
            Err(e) => (None, Some(traced.scrub(e.to_string()))),
        };
        self.trace.record(TracedSql {
            sql: traced.text,
            category: classify_statement(&st.sql, DIALECT),
            duration_ms: started.elapsed().as_millis() as u64,
            rows: None,
            affected,
            error,
        });
    }

    /// Runs the statements between BEGIN and COMMIT on `conn`, each checked against its
    /// `expect_affected` before the commit; the first failure rolls everything back.
    async fn commit_on(
        &self,
        conn: &SnowflakeConn,
        statements: &[TxStatement],
    ) -> Result<Vec<ExecOutcome>, AppError> {
        let mut done = Vec::with_capacity(statements.len());
        for (i, st) in statements.iter().enumerate() {
            let started = std::time::Instant::now();
            let result = conn
                .query(&st.sql, &st.params, None)
                .await
                .map_err(|e| e.error.with_statement_index(i))
                .map(|out| exec_outcome(&st.sql, out))
                .and_then(|outcome| check_affected(st, &outcome, i).map(|()| outcome));
            match result {
                Ok(outcome) => {
                    self.record(st, started, Ok(outcome.affected));
                    done.push(outcome);
                }
                Err(e) => {
                    self.record(st, started, Err(&e));
                    return Err(e);
                }
            }
        }
        match conn.query(sf::COMMIT, &[], Some(0)).await {
            // Accepted before the session expired: the commit may have gone through, and
            // committing the staged changes again would apply them twice.
            Err(e) if e.expired && e.query_id.is_some() => Err(may_have_run(&e)),
            Err(e) => Err(e.error),
            Ok(_) => Ok(done),
        }
    }

    async fn table_kind(&self, db: &str, schema: &str, name: &str) -> Result<TableLook, AppError> {
        let tables = self.catalog(&sf::show_table_like(db, schema, name)).await?;
        if let Some(row) = tables
            .iter()
            .find(|r| r.text("name").as_deref() == Some(name))
        {
            return Ok(TableLook {
                kind: if row.flag("is_external") {
                    TableKind::ForeignTable
                } else {
                    TableKind::Table
                },
                rows: row.int("rows"),
                comment: row.some("comment"),
                hybrid: row.flag("is_hybrid"),
            });
        }
        let views = self.catalog(&sf::show_views_like(db, schema, name)).await?;
        match views
            .iter()
            .find(|r| r.text("name").as_deref() == Some(name))
        {
            Some(row) => Ok(TableLook {
                kind: if row.flag("is_materialized") {
                    TableKind::MaterializedView
                } else {
                    TableKind::View
                },
                rows: None,
                comment: row.some("comment"),
                hybrid: false,
            }),
            None => Err(AppError::InvalidInput(format!(
                "Snowflake: {schema}.{name} does not exist or is not visible to this role"
            ))),
        }
    }

    async fn ddl(&self, table: &TableRef) -> Result<String, AppError> {
        let db = self.database()?;
        let schema = self.schema_of(table)?;
        let look = self.table_kind(db, schema, &table.name).await?;
        let kind = match look.kind {
            TableKind::View | TableKind::MaterializedView => DdlKind::View,
            _ => DdlKind::Table,
        };
        let rows = self
            .catalog(&sf::get_ddl(
                kind,
                &sf::qualified(&[db, schema, &table.name]),
            ))
            .await?;
        catalog::first_text(&rows)
            .ok_or_else(|| AppError::Internal("Snowflake: GET_DDL returned nothing".into()))
    }

    async fn routines(&self, schema: &str) -> Result<Vec<RoutineSummary>, AppError> {
        let db = self.database()?;
        let functions = self
            .show_all(|after| sf::show_in_schema(SchemaObjects::UserFunctions, db, schema, after))
            .await?;
        let procedures = self
            .show_all(|after| sf::show_in_schema(SchemaObjects::Procedures, db, schema, after))
            .await?;
        let mut out = catalog::routines(&functions, false);
        out.extend(catalog::routines(&procedures, true));
        Ok(out)
    }

    async fn routine_ddl(&self, routine: &RoutineRef) -> Result<String, AppError> {
        let db = self.database()?;
        let schema = routine
            .schema
            .as_deref()
            .or(self.state.schema.as_deref())
            .ok_or_else(|| AppError::InvalidInput("Snowflake: the routine has no schema".into()))?;
        let kind = match routine.kind {
            RoutineKind::Procedure => DdlKind::Procedure,
            _ => DdlKind::Function,
        };
        // GET_DDL names an overload by its argument types.
        let name = format!(
            "{}({})",
            sf::qualified(&[db, schema, &routine.name]),
            routine.identity_arguments
        );
        let rows = self.catalog(&sf::get_ddl(kind, &name)).await?;
        catalog::first_text(&rows)
            .ok_or_else(|| AppError::Internal("Snowflake: GET_DDL returned nothing".into()))
    }

    async fn kill_one(&self, id: &str, scope: KillScope) -> Result<(), AppError> {
        if scope == KillScope::Connection {
            return Err(AppError::Unsupported(
                "Snowflake cancels one query, not a whole session: use Kill Query".into(),
            ));
        }
        let sql = sf::cancel_query(id.trim());
        // SYSTEM$CANCEL_QUERY is a write for the classifier, as every SYSTEM$ call is.
        self.refuse_write(&sql)?;
        self.trace
            .run_async(
                classify_statement(&sql, DIALECT),
                sql.as_str(),
                |_: &()| TraceCounts::NONE,
                async { self.on_worker(&sql, &[], None, false).await.map(|_| ()) },
            )
            .await
    }
}

/// What `table_meta` learns from the SHOW of a table or view.
struct TableLook {
    kind: TableKind,
    rows: Option<i64>,
    comment: Option<String>,
    hybrid: bool,
}

/// Whether a statement Snowflake answered with an expired session may be sent again on the new
/// one: only a read it had not accepted yet (no query id). An accepted statement goes on after
/// the expiry, and a write sent again could run twice.
pub(super) fn may_resend(e: &StatementError, sql: &str) -> bool {
    e.query_id.is_none() && !writes(DIALECT, sql)
}

/// The error of a statement not sent again after the session expired under it.
pub(super) fn may_have_run(e: &StatementError) -> AppError {
    let id = e
        .query_id
        .as_deref()
        .map(|id| format!(" (Query ID: {id})"))
        .unwrap_or_default();
    AppError::db(format!(
        "Snowflake ended the session while the statement ran{id}. Tabler signed in again but did \
         not send it again: it may have completed, so check before running it again."
    ))
}

/// Runs one statement with its cancel handle armed in `cancel`.
pub(super) async fn run_one(
    conn: &SnowflakeConn,
    sql: &str,
    params: &[Value],
    cancel: Option<&CancelSlot>,
) -> Result<Outcome, StatementError> {
    let (handle, stop) = conn.prepare(sql, params)?;
    let _armed = match cancel {
        Some(slot) => Some(arm_cancel(slot, cancel_handle(stop))?),
        None => None,
    };
    SnowflakeConn::fetch(handle, sql, None).await
}

pub(super) fn cancel_handle(stop: SnowflakeCancel) -> CancelHandle {
    CancelHandle::Custom(Arc::new(stop))
}

/// The outcome of a statement run for its effect: DML is counted by its `number of rows …`
/// columns (a count Snowflake did not send is unknown, which a guard refuses); anything else
/// keeps its rows.
fn exec_outcome(sql: &str, out: Outcome) -> ExecOutcome {
    match out.affected() {
        Some(n) => ExecOutcome {
            affected: n,
            last_insert_id: None,
            rows: None,
            affected_unknown: false,
        },
        None => ExecOutcome {
            affected: 0,
            last_insert_id: None,
            affected_unknown: is_dml(sql),
            rows: Some(ResultSet {
                columns: out.columns,
                rows: out.rows,
                truncated: out.truncated,
            }),
        },
    }
}

#[async_trait]
impl DbSession for SnowflakeSession {
    fn driver(&self) -> Driver {
        Driver::Snowflake
    }

    fn dialect(&self) -> SqlDialect {
        DIALECT
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        let rows = self.catalog(sf::SERVER_INFO).await?;
        let state = catalog::session_state(&rows).unwrap_or_else(|| self.state.clone());
        Ok(ServerInfo {
            version: format!("Snowflake {}", state.version),
            current_database: state.database,
            current_user: state.user,
        })
    }

    /// `SELECT 1` on the app's session: answered without a warehouse. An expired session is
    /// signed in again rather than reported lost.
    async fn ping(&self) -> Result<(), AppError> {
        self.on_worker(sf::PING, &[], None, false).await.map(|_| ())
    }

    /// Stops what the editor runs and lets the sessions go (the connector has no logout:
    /// Snowflake ends them once idle), within about 2 s.
    async fn close(&self) {
        let stop = lock(&self.editor_stop).take();
        if let Some(stop) = stop
            && tokio::time::timeout(CLOSE_WITHIN, stop.cancel())
                .await
                .is_err()
        {
            log::warn!("Snowflake: the editor's statement did not stop within 2 s");
        }
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        let rows = self.catalog(sf::SHOW_DATABASES).await?;
        let mut out = catalog::databases(&rows);
        if let Some(db) = &self.state.database {
            for d in &mut out {
                d.is_current = d.name == *db;
            }
        }
        Ok(out)
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        let db = self.database()?;
        let rows = self.show_all(|after| sf::show_schemas(db, after)).await?;
        let mut out = catalog::schemas(&rows);
        if let Some(schema) = &self.state.schema {
            for s in &mut out {
                s.is_default = s.name == *schema;
            }
        }
        Ok(out)
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let db = self.database()?;
        let tables = self
            .show_all(|after| sf::show_in_schema(SchemaObjects::Tables, db, schema, after))
            .await?;
        let views = self
            .show_all(|after| sf::show_in_schema(SchemaObjects::Views, db, schema, after))
            .await?;
        let mut out = catalog::tables(&tables);
        out.extend(catalog::views(&views));
        for t in &mut out {
            t.schema.get_or_insert_with(|| schema.to_string());
        }
        Ok(out)
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let db = self.database()?;
        let schema = self.schema_of(table)?;
        let name = table.name.as_str();
        let look = self.table_kind(db, schema, name).await?;
        let show = |what| sf::show_in_table(what, db, schema, name);
        let columns = self.catalog(&show(TableObjects::Columns)).await?;
        let is_table = matches!(look.kind, TableKind::Table | TableKind::ForeignTable);
        let (primary_key, indexes, foreign_keys) = if is_table {
            let primary = self.catalog(&show(TableObjects::PrimaryKeys)).await?;
            let unique = self.catalog(&show(TableObjects::UniqueKeys)).await?;
            let imported = self.catalog(&show(TableObjects::ImportedKeys)).await?;
            let mut indexes = catalog::key_indexes(&primary, &unique);
            if look.hybrid {
                indexes.extend(catalog::hybrid_indexes(
                    &self.catalog(&show(TableObjects::Indexes)).await?,
                ));
            }
            (
                catalog::primary_key(&primary).1,
                indexes,
                catalog::foreign_keys(&imported),
            )
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        Ok(TableMeta {
            table: TableRef {
                schema: Some(schema.to_string()),
                name: name.to_string(),
            },
            kind: look.kind,
            columns: catalog::columns(&columns, &primary_key),
            primary_key,
            indexes,
            foreign_keys,
            triggers: Vec::new(),
            comment: look.comment,
            row_estimate: look.rows,
            ddl: None,
            checks: Vec::new(),
            options: None,
        })
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let db = self.database()?;
        let schema = self.schema_of(table)?;
        let rows = self
            .catalog(&sf::show_views_like(db, schema, &table.name))
            .await?;
        Ok(catalog::view_text(&rows, &table.name))
    }

    /// The columns of the session's schema (`SHOW COLUMNS IN SCHEMA`, no warehouse).
    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let mut schemas = std::collections::BTreeMap::new();
        if let (Some(db), Some(schema)) = (&self.state.database, &self.state.schema) {
            let rows = self
                .catalog(&sf::show_in_schema(
                    SchemaObjects::Columns,
                    db,
                    schema,
                    None,
                ))
                .await?;
            schemas.insert(schema.clone(), catalog::schema_columns(&rows));
        }
        Ok(CompletionMetadata {
            default_schema: self.state.schema.clone(),
            schemas,
        })
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError> {
        self.read(sql, params, None).await
    }

    async fn query_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ResultSet, AppError> {
        self.read(sql, params, Some(&cancel)).await
    }

    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        self.write(sql, params, None).await
    }

    async fn execute_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ExecOutcome, AppError> {
        self.write(sql, params, Some(&cancel)).await
    }

    /// BEGIN, the statements each checked against `expect_affected` (Snowflake's keys are only
    /// informational, so a key may match several rows), COMMIT; the first failure rolls back.
    /// On a shared (MFA) session, a transaction the user opened in the editor would take the
    /// commit in: it is refused then.
    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        // Nothing to commit: nothing is sent (not even BEGIN on a read-only session).
        if statements.is_empty() {
            return Ok(Vec::new());
        }
        for st in &statements {
            self.refuse_write(&st.sql)?;
        }
        let _commit = self.commits.lock().await;
        let _editor = self.shared_write_guard().await?;
        let live = self.worker();
        let mut conn = live.current().await;
        if let Err(e) = conn.query(sf::BEGIN, &[], Some(0)).await {
            if !e.expired {
                return Err(e.error);
            }
            conn = self.renew_worker(&conn).await?;
            // The new session ran the editor's USE statements again.
            self.forget_database();
            self.check_database(true).await?;
            conn.query(sf::BEGIN, &[], Some(0)).await?;
        }
        match self.commit_on(&conn, &statements).await {
            Ok(done) => Ok(done),
            Err(e) => {
                if let Err(rollback) = conn.query(sf::ROLLBACK, &[], Some(0)).await {
                    // Left open, the transaction would take in the app's next statements: the
                    // session is replaced.
                    log::warn!("Snowflake: ROLLBACK failed: {}", rollback.error);
                    if let Err(renew) = self.renew_worker(&conn).await {
                        log::warn!("Snowflake: signing in again failed: {renew}");
                    }
                    self.forget_database();
                }
                Err(e)
            }
        }
    }

    async fn run_statement(
        &self,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        editor::run(self, sql, max_rows, sink, cancel).await
    }

    /// Every row of a read into `sink` (exports), on the app's session.
    async fn stream_query(
        &self,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<u64, AppError> {
        self.refuse_write(sql)?;
        refuse_added_statement(sql)?;
        let mut reading = self.check_database_for_read().await?;
        let conn = self.worker().current().await;
        let (handle, stop) = conn.prepare(sql, params)?;
        let _armed = arm_cancel(&cancel, cancel_handle(stop))?;
        self.trace
            .run_async(
                classify_statement(sql, DIALECT),
                trace_sql(DIALECT, sql, params),
                |rows: &u64| TraceCounts {
                    rows: Some(*rows),
                    affected: None,
                },
                async {
                    let mut chunker = RowChunker::new(sink);
                    SnowflakeConn::run(handle, sql, |part| {
                        // Answered: the rows are the statement's, wherever the session moves.
                        reading.take();
                        match part {
                            Part::Columns(columns) => chunker.begin_set(columns),
                            Part::Rows(rows) => {
                                for row in rows {
                                    if chunker.push(row).is_break() {
                                        return std::ops::ControlFlow::Break(());
                                    }
                                }
                                std::ops::ControlFlow::Continue(())
                            }
                        }
                    })
                    .await?;
                    // A sink that stops early has what it asked for.
                    let _ = chunker.flush();
                    Ok(chunker.total_rows)
                },
            )
            .await
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        cancel_portable(handle).await.unwrap_or(Ok(()))
    }

    fn in_transaction(&self) -> Option<bool> {
        self.tx.get()
    }

    fn drain_trace(&self) -> Vec<TracedSql> {
        self.trace.drain()
    }

    fn sql_trace(&self) -> Option<&SqlTrace> {
        Some(&self.trace)
    }

    fn editor_run_lock(&self) -> Option<&Mutex<()>> {
        Some(&self.editor_runs)
    }

    fn capabilities(&self) -> SessionCaps {
        capabilities(self.state.schema.clone())
    }

    async fn list_processes(&self) -> Option<Result<Vec<ProcessRow>, AppError>> {
        Some(
            self.catalog(sf::RUNNING_QUERIES)
                .await
                .map(|rows| catalog::processes(&rows)),
        )
    }

    async fn kill_processes(&self, ids: &[String], scope: KillScope) -> Option<Vec<KillOutcome>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(match self.kill_one(id, scope).await {
                Ok(()) => KillOutcome {
                    id: id.clone(),
                    ok: true,
                    message: None,
                },
                Err(e) => KillOutcome {
                    id: id.clone(),
                    ok: false,
                    message: Some(e.to_string()),
                },
            });
        }
        Some(out)
    }

    async fn table_ddl(&self, table: &TableRef) -> Option<Result<String, AppError>> {
        Some(self.ddl(table).await)
    }

    async fn list_routines(&self, schema: &str) -> Option<Result<Vec<RoutineSummary>, AppError>> {
        Some(self.routines(schema).await)
    }

    async fn routine_definition(&self, routine: &RoutineRef) -> Option<Result<String, AppError>> {
        Some(self.routine_ddl(routine).await)
    }

    /// HTTPS to the account (plain http only to a test server on this machine); text is UTF-8.
    async fn session_details(&self) -> Option<Result<SessionDetails, AppError>> {
        Some(Ok(SessionDetails {
            tls: self.signin.tls(),
            tls_version: None,
            tls_cipher: None,
            encoding: Some("UTF-8".into()),
        }))
    }
}

/// A Snowflake session's caps (`default_schema`: the session's schema).
pub fn capabilities(default_schema: Option<String>) -> SessionCaps {
    let mut caps = SessionCaps::base(Driver::Snowflake, DIALECT, None).with(&FEATURES);
    caps.namespaces = NamespaceCaps {
        databases: true,
        schemas: SchemaMode::Schemas,
        default_schema,
        labels: NamespaceLabels::default(),
    };
    caps.data = DataCaps {
        paging: Paging::Offset,
        // A count needs a warehouse, which it may resume (and bill): only on request, the
        // metadata's row count otherwise.
        count: CountMode::Costly,
        sort: true,
        filter_ops: FILTER_OPS.to_vec(),
        raw_filter: Some(RawFilter::SqlWhere),
        transactions: TxMode::Atomic,
        affected: Affected::Exact,
        default_keyword: false,
        upsert: false,
        server_exprs: Vec::new(),
        max_params: max_params(DIALECT) as u32,
    };
    caps.safety = SafetyCaps {
        read_only: ReadOnlyMode::ClassifierOnly,
        // No session setting keeps SQL from external functions, stages or other accounts.
        hardened: false,
        ..caps.safety
    };
    caps.session = LifecycleCaps {
        // A ping costs nothing, but Snowflake ends idle sessions on its own schedule and the
        // driver signs in again when it has: no keep-alive.
        keepalive_secs: None,
        ping_billed: false,
    };
    caps
}
