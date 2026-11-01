//! The Cassandra session: one scylla session (a pool per node) behind a `Wire`, serving the app's
//! introspection, the grid (through `TableSource`, paged with the driver's paging state), edits
//! and the editor.
//!
//! - Read-only is Tabler's classifier alone (CQL has no read-only session): every statement the
//!   CQL classifier calls a write, or does not know, is refused before it is sent, on every path
//!   (reads, writes, edits, the editor, exports, the startup commands). The grid's raw WHERE is
//!   the user's text, so a read that would hold a write is refused on any session.
//! - Edits are lightweight transactions by full primary key (`dialect::cql`), sent one by one:
//!   CQL has no transaction, so a failed change leaves the ones before it applied (`TxMode::
//!   Sequential`), and each change's `[applied]` proves it touched its one row.
//! - CQL has no server-side cancel: a cancelled statement is dropped on this side and the node
//!   finishes it within its own timeout (no `CancelOnServer`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use ::scylla::response::PagingState;
use async_trait::async_trait;
use futures::future::BoxFuture;

use super::catalog::{self, CellRow, TableRows};
use super::cqlsh::{self, Command};
use super::cursor;
use super::params::CassandraParams;
use super::values::{decode, decode_row, decode_uncapped};
use super::wire::{Connector, Dial, Page, Reply, Request, RunOptions, Wire, keyspace_name};
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::native::TableSource;
use crate::drivers::{
    CancelAction, CancelHandle, CancelSlot, DbSession, RowChunker, RowSink, ServerInfo, SqlTrace,
    StatementOutcome, TraceCounts, TxStatement, arm_cancel, cancel_portable, classify_statement,
    trace_sql,
};
use crate::error::AppError;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::cql;
use crate::sqlgen::select::BuiltSql;
use crate::sqlgen::sql::{SelectQuery, TableTarget, inline_params, is_single_statement, writes};
use crate::types::browse::{
    BrowsePage, BrowseRequest, FilterGroup, RowCount, RowIdentity, RowKey, TraceCategory, TracedSql,
};
use crate::types::caps::{
    Affected, CountMode, DataCaps, Feature, LifecycleCaps, NamespaceCaps, NamespaceLabels, Paging,
    RawFilter, SafeConfirm, SafetyCaps, SchemaMode, SessionCaps, TxMode,
};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::ServerExpr;
use crate::types::profile::Driver;
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableKind, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub(super) const DIALECT: SqlDialect = SqlDialect::Cql;

/// How often the app pings an idle session (one row of `system.local`, answered by the node).
const KEEPALIVE_SECS: u32 = 30;

/// Rows per page when the app streams a whole query (an export).
const STREAM_PAGE: i32 = 1000;

/// What the session offers, each proven by the driver's tests (docs/drivers/cassandra.md,
/// Phase B status). Not `CancelOnServer`: CQL has none. Not `Mcp`, which the lead grants once
/// the classifier's red-team review has passed.
const FEATURES: [Feature; 11] = [
    Feature::Editor,
    Feature::StructureView,
    Feature::EditRows,
    Feature::InsertRows,
    Feature::DeleteRows,
    Feature::ServerExpressions,
    Feature::ExactCount,
    Feature::TableDdl,
    Feature::ViewDefinition,
    Feature::SwitchDatabase,
    Feature::StartupCommands,
];

/// The clocks CQL writes (`now()`, `toTimestamp(now())`, `toDate(now())`).
const SERVER_EXPRS: [ServerExpr; 3] = [
    ServerExpr::Now,
    ServerExpr::CurrentTimestamp,
    ServerExpr::CurrentDate,
];

/// What the editor's cqlsh commands change for the session's later runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorSettings {
    pub options: RunOptions,
    /// Rows per page; `PAGING OFF` keeps the size and reads on until the row cap.
    pub page_size: u32,
    pub paging: bool,
    pub expand: bool,
}

/// How `page_through` reads: at which options, how many rows per page, and up to how many rows
/// (None: all).
#[derive(Debug, Clone, Copy)]
pub(super) struct Paged {
    pub options: RunOptions,
    pub page_size: i32,
    pub max_rows: Option<u64>,
}

/// Who and where the session is, found at connect.
#[derive(Debug, Clone, Default)]
pub struct NodeInfo {
    pub release_version: Option<String>,
    /// ScyllaDB's own version (its `release_version` is the Cassandra one it speaks).
    pub scylla_version: Option<String>,
}

impl NodeInfo {
    pub fn version(&self) -> String {
        match (&self.scylla_version, &self.release_version) {
            (Some(v), _) => format!("ScyllaDB {v}"),
            (None, Some(v)) => format!("Cassandra {v}"),
            (None, None) => "Cassandra".into(),
        }
    }
}

pub struct CassandraSession {
    /// None once closed: dropping the scylla session closes its connections.
    wire: RwLock<Option<Arc<dyn Wire>>>,
    params: CassandraParams,
    read_only: bool,
    confirm: SafeConfirm,
    user: Option<String>,
    info: NodeInfo,
    keyspace: std::sync::Mutex<Option<String>>,
    pub(super) settings: std::sync::Mutex<EditorSettings>,
    editor_runs: tokio::sync::Mutex<()>,
    trace: SqlTrace,
    closed: AtomicBool,
}

pub(super) fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn read_only_error() -> AppError {
    AppError::InvalidInput(READ_ONLY_MESSAGE.into())
}

/// Refuses, on any session, a read the app sends that holds more than a read: the grid's raw
/// WHERE is the user's text (`x = 1; DROP TABLE t` or a write word in it).
fn refuse_added_statement(sql: &str) -> Result<(), AppError> {
    if writes(DIALECT, sql) || !is_single_statement(DIALECT, sql) {
        return Err(AppError::InvalidInput(
            "A filter can only read rows: this one would run another statement".into(),
        ));
    }
    Ok(())
}

/// A statement that changes the schema, after which the nodes must agree before anything reads
/// it again.
pub(super) fn changes_schema(sql: &str) -> bool {
    matches!(
        crate::sqlgen::split::leading_keywords(sql, 1)
            .first()
            .map(String::as_str),
        Some("CREATE" | "ALTER" | "DROP")
    )
}

/// The rows of a reply as a `ResultSet`.
fn result_set(reply: Reply) -> ResultSet {
    ResultSet {
        columns: reply.columns,
        rows: reply.rows.iter().map(|r| decode_row(r)).collect(),
        truncated: false,
    }
}

/// Whether a lightweight transaction applied, from its `[applied]` column; None for any other
/// statement.
pub(super) fn applied(reply: &Reply) -> Option<bool> {
    let first = reply.columns.first()?;
    if first.name != "[applied]" {
        return None;
    }
    reply.rows.first().and_then(|row| match row.first() {
        Some(Some(::scylla::value::CqlValue::Boolean(b))) => Some(*b),
        _ => None,
    })
}

/// The outcome of a write: a lightweight transaction counts the row it applied to; anything
/// else does not say.
pub(super) fn exec_outcome(reply: Reply) -> ExecOutcome {
    let lwt = applied(&reply);
    let rows = reply.is_rows.then(|| result_set(reply));
    match lwt {
        Some(done) => ExecOutcome {
            affected: u64::from(done),
            last_insert_id: None,
            rows,
            affected_unknown: false,
        },
        None => ExecOutcome {
            affected: 0,
            last_insert_id: None,
            rows,
            affected_unknown: true,
        },
    }
}

/// Stops a statement on this side: the run's future is dropped (CQL cannot cancel on the
/// server, which finishes it within its own timeout).
#[derive(Default)]
struct Abort {
    fired: AtomicBool,
    notify: tokio::sync::Notify,
}

struct AbortAction(Arc<Abort>);

impl CancelAction for AbortAction {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        self.0.fired.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
        Box::pin(async { Ok(()) })
    }

    fn label(&self) -> &'static str {
        "client-side stop"
    }
}

/// Runs `work` until it ends or `slot` is cancelled.
pub(super) async fn cancellable<T>(
    slot: &CancelSlot,
    work: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    let abort = Arc::new(Abort::default());
    let _armed = arm_cancel(
        slot,
        CancelHandle::Custom(Arc::new(AbortAction(abort.clone()))),
    )?;
    let notified = abort.notify.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    if abort.fired.load(Ordering::SeqCst) {
        return Err(AppError::Cancelled);
    }
    tokio::select! {
        biased;
        _ = notified => Err(AppError::Cancelled),
        result = work => result,
    }
}

impl CassandraSession {
    /// Opens the session of `dial` with `connector`, reads who the node is and runs the
    /// profile's startup commands.
    pub async fn open(
        connector: &dyn Connector,
        dial: &Dial,
        params: CassandraParams,
        read_only: bool,
        confirm: SafeConfirm,
        startup: Option<&str>,
    ) -> Result<CassandraSession, AppError> {
        // The startup commands run before anything else, so they are checked first.
        if let Some(text) = startup
            && read_only
            && cql::split_cql(text)
                .iter()
                .any(|s| cqlsh::parse(&s.sql).is_none() && writes(DIALECT, &s.sql))
        {
            return Err(AppError::InvalidInput(
                "Cassandra: this connection is read-only, and its startup commands write (or are \
                 not known to only read)"
                    .into(),
            ));
        }
        let wire = connector.connect(dial).await?;
        let mut session = CassandraSession {
            wire: RwLock::new(Some(wire)),
            read_only,
            confirm,
            user: dial.credentials.as_ref().map(|c| c.user.clone()),
            info: NodeInfo::default(),
            keyspace: std::sync::Mutex::new(dial.keyspace.clone()),
            settings: std::sync::Mutex::new(EditorSettings {
                options: dial.options,
                page_size: params.page_size(),
                paging: true,
                expand: false,
            }),
            params,
            editor_runs: tokio::sync::Mutex::new(()),
            trace: SqlTrace::default(),
            closed: AtomicBool::new(false),
        };
        let local = session.catalog(catalog::LOCAL, &[]).await?;
        let local = catalog::parse_local(&local).unwrap_or_default();
        // Only ScyllaDB has `system.versions`: Cassandra refuses the read.
        let scylla = match session.catalog(catalog::SCYLLA_VERSION, &[]).await {
            Ok(rows) => catalog::parse_scylla_version(&rows),
            Err(_) => None,
        };
        session.info = NodeInfo {
            release_version: local.release_version,
            scylla_version: scylla,
        };
        if let Some(text) = startup {
            for st in cql::split_cql(text) {
                session
                    .startup_command(&st.sql)
                    .await
                    .map_err(|e| match e {
                        AppError::Db { message, .. } => AppError::InvalidInput(format!(
                            "Cassandra: the startup commands failed: {message}"
                        )),
                        other => other,
                    })?;
            }
        }
        Ok(session)
    }

    pub(super) fn wire(&self) -> Result<Arc<dyn Wire>, AppError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(AppError::NotConnected);
        }
        self.wire
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or(AppError::NotConnected)
    }

    /// The options of the app's own requests (the profile's).
    fn app_options(&self) -> RunOptions {
        RunOptions {
            consistency: self.params.consistency.level(),
            serial: self.params.serial_consistency.level(),
            timeout: self.params.request_timeout(),
            tracing: false,
        }
    }

    pub(super) fn current_keyspace(&self) -> Option<String> {
        lock(&self.keyspace).clone()
    }

    pub(super) fn set_keyspace(&self, keyspace: String) {
        *lock(&self.keyspace) = Some(keyspace);
    }

    /// Refuses, on a read-only session, what the classifier calls a write (or does not know).
    pub(super) fn refuse_write(&self, sql: &str) -> Result<(), AppError> {
        if self.read_only && writes(DIALECT, sql) {
            Err(read_only_error())
        } else {
            Ok(())
        }
    }

    /// Sends one request, traced as `category`.
    pub(super) async fn send(
        &self,
        category: TraceCategory,
        cql: &str,
        params: &[Value],
        page: Page,
        options: RunOptions,
    ) -> Result<Reply, AppError> {
        let wire = self.wire()?;
        self.trace
            .run_async(
                category,
                trace_sql(DIALECT, cql, params),
                |r: &Reply| TraceCounts {
                    rows: r.is_rows.then_some(r.rows.len() as u64),
                    affected: applied(r).map(u64::from),
                },
                wire.run(Request {
                    cql,
                    params,
                    page,
                    options,
                }),
            )
            .await
    }

    /// The rows of an introspection query.
    async fn catalog(&self, cql: &str, params: &[Value]) -> Result<Vec<CellRow>, AppError> {
        let reply = self
            .send(
                TraceCategory::Structure,
                cql,
                params,
                Page::Unpaged,
                self.app_options(),
            )
            .await?;
        Ok(reply.rows)
    }

    /// A read the app builds (a grid page, a count, a cell), refused when it holds more.
    async fn read(&self, built: &BuiltSql, page: Page) -> Result<Reply, AppError> {
        self.refuse_write(&built.sql)?;
        refuse_added_statement(&built.sql)?;
        self.send(
            classify_statement(&built.sql, DIALECT),
            &built.sql,
            &built.params,
            page,
            self.app_options(),
        )
        .await
    }

    /// A write the app sends: checked, traced, and followed by schema agreement after DDL.
    async fn write(&self, sql: &str, params: &[Value]) -> Result<Reply, AppError> {
        self.refuse_write(sql)?;
        let reply = self
            .send(
                classify_statement(sql, DIALECT),
                sql,
                params,
                Page::Unpaged,
                self.app_options(),
            )
            .await?;
        if changes_schema(sql) {
            self.wire()?.await_schema_agreement().await?;
        }
        Ok(reply)
    }

    /// One statement of the profile's startup commands: a cqlsh command changes the editor's
    /// settings, `USE` the keyspace, anything else is sent.
    async fn startup_command(&self, sql: &str) -> Result<(), AppError> {
        if let Some(command) = cqlsh::parse(sql) {
            self.apply_cqlsh(command?);
            return Ok(());
        }
        if let Some(name) = use_target(sql) {
            return self.use_keyspace(&name).await.map(|_| ());
        }
        self.write(sql, &[]).await.map(|_| ())
    }

    /// Applies a cqlsh command; the text it answers with.
    pub(super) fn apply_cqlsh(&self, command: Command) -> String {
        let mut s = lock(&self.settings);
        match command {
            Command::Consistency(Some(level)) => {
                s.options.consistency = level;
                format!("Consistency level set to {level}.")
            }
            Command::Consistency(None) => {
                format!("Current consistency level is {}.", s.options.consistency)
            }
            Command::SerialConsistency(Some(level)) => {
                s.options.serial = level;
                format!("Serial consistency level set to {level}.")
            }
            Command::SerialConsistency(None) => {
                format!("Current serial consistency level is {}.", s.options.serial)
            }
            Command::Tracing(Some(on)) => {
                s.options.tracing = on;
                format!("Tracing is {}.", if on { "enabled" } else { "disabled" })
            }
            Command::Tracing(None) => format!(
                "Tracing is currently {}.",
                if s.options.tracing {
                    "enabled"
                } else {
                    "disabled"
                }
            ),
            Command::Paging(p) => {
                match p {
                    cqlsh::Paging::On => s.paging = true,
                    cqlsh::Paging::Off => s.paging = false,
                    cqlsh::Paging::Size(n) => {
                        s.paging = true;
                        s.page_size = n.min(super::params::MAX_PAGE_SIZE);
                    }
                    cqlsh::Paging::Show => {}
                }
                if s.paging {
                    format!("Query paging is enabled, page size {}.", s.page_size)
                } else {
                    format!(
                        "Query paging is disabled: pages of {} rows are read up to the row limit.",
                        super::params::MAX_PAGE_SIZE
                    )
                }
            }
            Command::Expand(Some(on)) => {
                s.expand = on;
                format!(
                    "Expanded output is {} (the grid shows rows as a table either way).",
                    if on { "enabled" } else { "disabled" }
                )
            }
            Command::Expand(None) => format!(
                "Expanded output is currently {}.",
                if s.expand { "enabled" } else { "disabled" }
            ),
        }
    }

    /// `USE ks`: every connection of the session switches.
    pub(super) async fn use_keyspace(&self, name: &str) -> Result<String, AppError> {
        let (keyspace, case_sensitive) = keyspace_name(name);
        let wire = self.wire()?;
        let shown = format!("USE {}", (cql::SPEC.quote_ident)(&keyspace));
        self.trace
            .run_async(
                TraceCategory::Structure,
                shown,
                |_: &()| TraceCounts::NONE,
                wire.use_keyspace(&keyspace, case_sensitive),
            )
            .await?;
        self.set_keyspace(keyspace.clone());
        Ok(keyspace)
    }

    /// The keyspace a table is in: its own, or the session's.
    fn keyspace_of(&self, table: &TableRef) -> Result<String, AppError> {
        table
            .schema
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| self.current_keyspace())
            .ok_or_else(|| AppError::InvalidInput("Cassandra: choose a keyspace first".into()))
    }

    /// The server's `DESCRIBE` of a table or view (Cassandra 4.0+, ScyllaDB 5.2+); None when the
    /// server has none.
    async fn describe(&self, keyspace: &str, name: &str, view: bool) -> Option<String> {
        let cql = if view {
            cql::describe_view(keyspace, name)
        } else {
            cql::describe_table(keyspace, name)
        };
        match self.catalog(&cql, &[]).await {
            Ok(rows) => catalog::parse_describe(&rows),
            Err(_) => None,
        }
    }

    /// Pages a query into `chunker` until `max_rows` (None: all), the sink stops, or the last
    /// page. Returns whether rows were left unread and the last reply.
    pub(super) async fn page_through(
        &self,
        cql: &str,
        params: &[Value],
        paging: Paged,
        chunker: &mut RowChunker<'_>,
        slot: &CancelSlot,
    ) -> Result<(bool, Reply), AppError> {
        let Paged {
            options,
            page_size,
            max_rows,
        } = paging;
        let mut state = PagingState::start();
        let mut first = true;
        loop {
            let page = Page::Single {
                state,
                size: page_size,
            };
            let reply = cancellable(
                slot,
                self.send(classify_statement(cql, DIALECT), cql, params, page, options),
            )
            .await?;
            if !reply.is_rows {
                return Ok((false, reply));
            }
            if first {
                first = false;
                if chunker.begin_set(reply.columns.clone()).is_break() {
                    return Ok((false, reply));
                }
            }
            for row in &reply.rows {
                if max_rows.is_some_and(|cap| chunker.total_rows >= cap) {
                    return Ok((true, reply));
                }
                if chunker.push(decode_row(row)).is_break() {
                    return Ok((false, reply));
                }
            }
            match reply.paging.clone().into_paging_control_flow() {
                std::ops::ControlFlow::Continue(next) => {
                    if max_rows.is_some_and(|cap| chunker.total_rows >= cap) {
                        return Ok((true, reply));
                    }
                    state = next;
                }
                std::ops::ControlFlow::Break(()) => return Ok((false, reply)),
            }
        }
    }
}

/// The keyspace of a `USE` statement (its name as written, quotes kept); None for any other.
pub(super) fn use_target(sql: &str) -> Option<String> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let (word, rest) = trimmed.split_once(char::is_whitespace)?;
    if !word.eq_ignore_ascii_case("USE") {
        return None;
    }
    let name = rest.trim();
    let quoted = name.len() >= 2 && name.starts_with('"') && name.ends_with('"');
    let bare = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    (quoted || bare).then(|| name.to_string())
}

#[async_trait]
impl DbSession for CassandraSession {
    fn driver(&self) -> Driver {
        Driver::Cassandra
    }

    fn dialect(&self) -> SqlDialect {
        DIALECT
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        Ok(ServerInfo {
            version: self.info.version(),
            current_database: self.current_keyspace(),
            current_user: self.user.clone(),
        })
    }

    async fn ping(&self) -> Result<(), AppError> {
        self.send(
            TraceCategory::Structure,
            catalog::PING,
            &[],
            Page::Unpaged,
            self.app_options(),
        )
        .await
        .map(|_| ())
    }

    async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        // Dropping the scylla session closes its connections at once.
        let wire = self.wire.write().unwrap_or_else(|p| p.into_inner()).take();
        drop(wire);
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        let rows = self.catalog(catalog::KEYSPACES, &[]).await?;
        Ok(catalog::databases(
            &catalog::parse_keyspaces(&rows),
            self.current_keyspace().as_deref(),
        ))
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        Ok(self
            .current_keyspace()
            .map(|name| SchemaInfo {
                name,
                is_default: true,
            })
            .into_iter()
            .collect())
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let ks = [Value::Text(schema.to_string())];
        let tables = self.catalog(catalog::TABLES, &ks).await?;
        let views = self.catalog(catalog::VIEWS, &ks).await?;
        Ok(catalog::parse_tables(schema, &tables, &views))
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let keyspace = self.keyspace_of(table)?;
        let table = TableRef::new(Some(&keyspace), &table.name);
        let names = [
            Value::Text(keyspace.clone()),
            Value::Text(table.name.clone()),
        ];
        let columns = self.catalog(catalog::COLUMNS, &names).await?;
        if columns.is_empty() {
            return Err(AppError::NotFound(format!(
                "table {}.{}",
                keyspace, table.name
            )));
        }
        let view = self.catalog(catalog::VIEW, &names).await?;
        let is_view = !view.is_empty();
        let options = if is_view {
            Vec::new()
        } else {
            self.catalog(catalog::TABLE_OPTIONS, &names).await?
        };
        let indexes = self.catalog(catalog::INDEXES, &names).await?;
        let triggers = if is_view {
            Vec::new()
        } else {
            self.catalog(catalog::TRIGGERS, &names).await?
        };
        // The node may have no estimate yet, or no such table (newer servers moved it).
        let sizes = self
            .catalog(catalog::SIZE_ESTIMATES, &names)
            .await
            .unwrap_or_default();
        let ddl = self.describe(&keyspace, &table.name, is_view).await;
        Ok(catalog::table_meta(
            &table,
            &TableRows {
                columns: &columns,
                options: &options,
                indexes: &indexes,
                triggers: &triggers,
                size_estimates: &sizes,
                view: is_view.then_some(view.as_slice()),
            },
            ddl,
        ))
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let meta = self.table_meta(table).await?;
        Ok((meta.kind == TableKind::MaterializedView)
            .then_some(meta.ddl)
            .flatten())
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let Some(keyspace) = self.current_keyspace() else {
            return Ok(CompletionMetadata::default());
        };
        let rows = self
            .catalog(catalog::KEYSPACE_COLUMNS, &[Value::Text(keyspace.clone())])
            .await?;
        Ok(catalog::completion(&keyspace, &rows))
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError> {
        self.query_cancellable(sql, params, CancelSlot::default())
            .await
    }

    async fn query_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ResultSet, AppError> {
        self.refuse_write(sql)?;
        let options = self.app_options();
        let reply = cancellable(
            &cancel,
            self.send(
                classify_statement(sql, DIALECT),
                sql,
                params,
                Page::Unpaged,
                options,
            ),
        )
        .await?;
        Ok(result_set(reply))
    }

    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        self.execute_cancellable(sql, params, CancelSlot::default())
            .await
    }

    async fn execute_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ExecOutcome, AppError> {
        cancellable(&cancel, self.write(sql, params))
            .await
            .map(exec_outcome)
    }

    /// The changes one by one (CQL has no transaction): a failed one stops the rest, and those
    /// before it stay applied. A lightweight transaction that did not apply fails its change.
    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        if self.read_only {
            return Err(read_only_error());
        }
        let mut out = Vec::with_capacity(statements.len());
        for (i, st) in statements.iter().enumerate() {
            let reply = self
                .write(&st.sql, &st.params)
                .await
                .map_err(|e| e.with_statement_index(i))?;
            let lwt = applied(&reply);
            let outcome = exec_outcome(reply);
            if st.expect_affected.is_some() {
                if lwt == Some(false) {
                    let insert = crate::sqlgen::split::leading_keywords(&st.sql, 1)
                        .first()
                        .is_some_and(|w| w == "INSERT");
                    return Err(AppError::db(if insert {
                        "a row with this primary key already exists, so it was not inserted"
                    } else {
                        "the row no longer exists (it was deleted or its key changed), so the \
                         change was not applied"
                    })
                    .with_statement_index(i));
                }
                crate::drivers::check_affected(st, &outcome, i)?;
            }
            out.push(outcome);
        }
        Ok(out)
    }

    async fn run_statement(
        &self,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        super::editor::run(self, sql, max_rows, sink, cancel).await
    }

    async fn stream_query(
        &self,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<u64, AppError> {
        self.refuse_write(sql)?;
        refuse_added_statement(sql)?;
        let mut chunker = RowChunker::new(sink);
        self.page_through(
            sql,
            params,
            Paged {
                options: self.app_options(),
                page_size: STREAM_PAGE,
                max_rows: None,
            },
            &mut chunker,
            &cancel,
        )
        .await?;
        let _ = chunker.flush();
        Ok(chunker.total_rows)
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        cancel_portable(handle).await.unwrap_or(Ok(()))
    }

    /// CQL has no transactions.
    fn in_transaction(&self) -> Option<bool> {
        Some(false)
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

    fn capabilities(&self) -> SessionCaps {
        let mut caps = SessionCaps::base(Driver::Cassandra, DIALECT, None).with(&FEATURES);
        caps.namespaces = NamespaceCaps {
            databases: true,
            schemas: SchemaMode::DatabaseIsSchema,
            default_schema: self.current_keyspace(),
            labels: NamespaceLabels {
                database: "Keyspace".into(),
                schema: "Keyspace".into(),
                ..NamespaceLabels::default()
            },
        };
        caps.data = DataCaps {
            paging: Paging::Cursor,
            // `count(*)` reads every partition; the size estimates stand in until asked.
            count: CountMode::Costly,
            // CQL orders by clustering columns within one partition only.
            sort: false,
            filter_ops: cql::FILTER_OPS.to_vec(),
            raw_filter: Some(RawFilter::SqlWhere),
            transactions: TxMode::Sequential,
            // The grid's edits are lightweight transactions, whose `[applied]` counts the row.
            affected: Affected::Exact,
            default_keyword: false,
            upsert: false,
            server_exprs: SERVER_EXPRS.to_vec(),
            max_params: cql::SPEC.max_params,
        };
        caps.safety = SafetyCaps {
            read_only: ReadOnlyMode::ClassifierOnly,
            // CQL reaches no file or other server (user-defined functions are sandboxed and off
            // unless the server enables them).
            hardened: false,
            confirm: self.confirm,
        };
        caps.session = LifecycleCaps {
            keepalive_secs: Some(KEEPALIVE_SECS),
            ping_billed: false,
        };
        caps
    }

    fn table_source(&self) -> Option<&dyn TableSource> {
        Some(self)
    }

    async fn table_ddl(&self, table: &TableRef) -> Option<Result<String, AppError>> {
        Some(self.table_meta(table).await.and_then(|meta| {
            meta.ddl
                .ok_or_else(|| AppError::NotFound(format!("the DDL of {}", table.name)))
        }))
    }
}

#[async_trait]
impl TableSource for CassandraSession {
    /// One page, from the cursor's paging state (the first without one).
    async fn browse(
        &self,
        meta: &TableMeta,
        req: &BrowseRequest,
        cancel: CancelSlot,
    ) -> Result<BrowsePage, AppError> {
        if req.cursor.is_none() && req.offset > 0 {
            return Err(AppError::InvalidInput(
                "Cassandra pages follow one another: there is no offset to start from".into(),
            ));
        }
        let started = std::time::Instant::now();
        let built = cql::browse_select(
            &SelectQuery {
                table: &meta.table,
                columns: &meta.columns,
                selected: req.columns.as_deref(),
                filter: req.filter.as_ref(),
                sort: &req.sort,
                limit: req.limit,
                offset: 0,
                pk: &meta.primary_key,
                identity: RowIdentity::Pk,
            },
            None,
        )?;
        let fingerprint = cursor::fingerprint(&built.sql, &built.params);
        let state = cursor::decode(req.cursor.as_deref(), fingerprint)?;
        let size = i32::try_from(req.limit.max(1)).unwrap_or(i32::MAX);
        let reply = cancellable(&cancel, self.read(&built, Page::Single { state, size })).await?;
        let columns = match &req.columns {
            Some(names) => names
                .iter()
                .filter_map(|n| meta.columns.iter().find(|c| &c.name == n).cloned())
                .collect(),
            None => meta.columns.clone(),
        };
        Ok(BrowsePage {
            columns,
            rows: reply.rows.iter().map(|r| decode_row(r)).collect(),
            pk_columns: meta.primary_key.clone(),
            identity: if meta.primary_key.is_empty() {
                RowIdentity::None
            } else {
                RowIdentity::Pk
            },
            row_ids: None,
            offset: req.offset,
            limit: req.limit,
            sql: inline_params(DIALECT, &built.sql, &built.params),
            duration_ms: started.elapsed().as_millis() as u64,
            next_cursor: cursor::encode(&reply.paging, fingerprint),
        })
    }

    /// The node's size estimate (partitions, not rows) unless asked to count; a count reads
    /// every partition.
    async fn count(
        &self,
        meta: &TableMeta,
        filter: Option<&FilterGroup>,
        exact: bool,
        cancel: CancelSlot,
    ) -> Result<RowCount, AppError> {
        if !exact
            && filter.is_none()
            && let Some(estimate) = meta.row_estimate
        {
            return Ok(RowCount {
                count: estimate,
                exact: false,
            });
        }
        let built = cql::count_select(&meta.table, &meta.columns, filter)?;
        let reply = cancellable(&cancel, self.read(&built, Page::Unpaged)).await?;
        let cell = reply
            .rows
            .first()
            .and_then(|r| r.first())
            .map_or(Value::Null, |c| decode(c.as_ref()));
        let count = match cell {
            Value::Int(n) => n,
            Value::BigInt(s) => s.parse().unwrap_or(i64::MAX),
            _ => 0,
        };
        Ok(RowCount { count, exact: true })
    }

    async fn fetch_cell(
        &self,
        meta: &TableMeta,
        key: &RowKey,
        column: &str,
    ) -> Result<Value, AppError> {
        let col = meta
            .columns
            .iter()
            .find(|c| c.name == column)
            .ok_or_else(|| AppError::InvalidInput(format!("unknown column \"{column}\"")))?;
        let built = cql::fetch_cell_select(
            &TableTarget {
                table: &meta.table,
                columns: &meta.columns,
            },
            col,
            key,
        )?;
        let reply = self.read(&built, Page::Unpaged).await?;
        let row = reply
            .rows
            .first()
            .ok_or_else(|| AppError::NotFound("the row no longer exists".into()))?;
        Ok(decode_uncapped(row.first().and_then(Option::as_ref)))
    }
}

#[cfg(test)]
pub(super) fn settings_of(session: &CassandraSession) -> EditorSettings {
    *lock(&session.settings)
}
