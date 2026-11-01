//! The DuckDB session: two connections of one engine instance, each on its own thread
//! (`conn::Worker`). The **editor** connection runs the SQL editor's statements, so `BEGIN`,
//! `USE` and `SET` last from one run to the next; the **worker** connection runs what the app
//! runs for itself (the catalog, the grid, counts, commits, exports), so it never waits behind
//! the editor's transaction.
//!
//! Read-only is the engine's (`engine`): a read-only profile's instance is opened
//! `access_mode=READ_ONLY` and hardened. The session also refuses, before it runs, what the
//! classifier calls a write, so a write fails the same way on every path and the engine's
//! refusal is the second wall, not the only one. Every session refuses a statement that visibly
//! switches DuckDB's logger (`switches_logger`): file storage it cannot write would abort the
//! process at the next file it closes. That check is lexical (a view or macro of the file still
//! switches it), so every job also looks at the logger after it runs (`Instance::guarded`): a
//! switched one stops the instance, and its connections are leaked rather than closed.
//!
//! Cancel interrupts the connection the statement runs on (`conn::Interrupt`), which stops it
//! inside the engine: there is no server to ask.

use std::sync::Arc;
use std::time::Instant;

use ::duckdb::Connection;
use async_trait::async_trait;

use super::catalog::{self, DuckdbServerInfo};
use super::conn::{self, Interrupt, Worker};
use super::decode::Bytes;
use super::engine::Instance;
use super::errors;
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::{
    CANCEL_SETTLE_LIMIT, CancelAction, CancelHandle, CancelSlot, CollectSink, DbSession, RowSink,
    ServerInfo, SqlTrace, StatementOutcome, TraceCounts, TxState, TxStatement, arm_cancel,
    cancel_portable, check_affected, classify_statement, settle_cancels_blocking, trace_sql,
};
use crate::error::AppError;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::duckdb::{LEX, switches_logger, use_database};
use crate::sqlgen::dialect::lex::{PLAIN, TokenKind, scan};
use crate::sqlgen::sql::{max_params, writes};
use crate::types::browse::{FilterOp, TraceCategory, TracedSql};
use crate::types::caps::{
    Affected, CountMode, DataCaps, Feature, LifecycleCaps, NamespaceCaps, NamespaceLabels, Paging,
    RawFilter, SafeConfirm, SafetyCaps, SchemaMode, SessionCaps, TxMode,
};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::ServerExpr;
use crate::types::objects::{RoutineRef, RoutineSummary};
use crate::types::profile::Driver;
use crate::types::query::{ExecOutcome, ResultSet, StatementKind};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::security::SessionDetails;
use crate::types::value::Value;

const DIALECT: SqlDialect = SqlDialect::Duckdb;

/// How often the app pings an idle session (the descriptor's keepalive): a ping is a `SELECT 1`
/// in this process.
const KEEPALIVE_SECS: u32 = 30;

/// Why a statement that switches the logger is refused (`switches_logger`).
pub const LOGGER_REFUSED: &str = "DuckDB's logger cannot be switched from Tabler: file storage \
     it cannot write stops the database and closes the app. Use the startup SQL of another \
     DuckDB to log.";

/// What every session offers beyond the structure view and the editor, each proven by the
/// lane's tests (docs/drivers/duckdb.md, Phase B status). Nothing that writes a core dialect's
/// SQL, no structure editing (no DDL planner yet).
pub const FEATURES: [Feature; 10] = [
    Feature::Explain,
    Feature::ExplainAnalyze,
    Feature::CancelOnServer,
    Feature::ExactCount,
    Feature::TableDdl,
    Feature::ViewDefinition,
    Feature::Routines,
    Feature::StartupCommands,
    Feature::SessionDetails,
    Feature::Mcp,
];

/// Grid edits, where the database file takes writes: atomic transactions on the worker
/// connection, by primary key under the affected-rows guard. Not `EditWithoutKey`: the portable
/// change builder addresses rows by key only (and DuckDB has no stable row id).
pub const EDIT_FEATURES: [Feature; 5] = [
    Feature::EditRows,
    Feature::InsertRows,
    Feature::DeleteRows,
    Feature::Returning,
    Feature::ServerExpressions,
];

/// Every filter: ILIKE and `json_contains` are DuckDB's own.
const FILTER_OPS: [FilterOp; 25] = [
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
    FilterOp::JsonContains,
    FilterOp::JsonNotContains,
];

/// The SQL-standard clocks the dialect writes, and DEFAULT in a new row (an UPDATE cannot set
/// it: `default_keyword` is off). No `CURRENT_DATE`: it is ICU's, which the bundled build lacks.
const SERVER_EXPRS: [ServerExpr; 3] = [
    ServerExpr::Now,
    ServerExpr::CurrentTimestamp,
    ServerExpr::Default,
];

/// How `entry::open` starts a session on its instance.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    /// `conn_options::effective_read_only` of the profile.
    pub read_only: bool,
    /// The database (catalog) both connections `USE`: the profile's, or a switch's.
    pub database: Option<String>,
    pub confirm: SafeConfirm,
}

/// What the session knows of its instance, kept after `close` lets the instance go.
#[derive(Debug, Clone, Copy)]
struct InstanceFacts {
    /// `Instance::is_read_only`: what `ReadOnlyMode::Server` may claim.
    server_read_only: bool,
    hardened: bool,
    /// `Instance::is_file_read_only`: a read-only profile, or an encrypted file.
    file_read_only: bool,
    /// A file, whose second sessions (another workspace, a database switch) share the instance.
    /// An in-memory database would be a new, empty one.
    shared: bool,
}

pub struct DuckdbSession {
    editor: Arc<Worker>,
    worker: Arc<Worker>,
    facts: InstanceFacts,
    /// Writes the classifier sees are refused before they run (and the engine refuses the rest).
    read_only: bool,
    /// Safe Mode's prompt: the passcode when nobody types the key (a Command Line).
    confirm: SafeConfirm,
    /// The editor's transaction, by the statements it ran (duckdb-rs cannot tell).
    tx_state: Arc<TxState>,
    editor_runs: tokio::sync::Mutex<()>,
    trace: SqlTrace,
}

fn read_only_error() -> AppError {
    AppError::InvalidInput(READ_ONLY_MESSAGE.into())
}

/// The error of statement `index` of a commit, naming it whatever kind it is.
fn at_statement(e: AppError, index: usize) -> AppError {
    match e {
        e @ AppError::Db { .. } => e.with_statement_index(index),
        other => other,
    }
}

/// Whether a read loads one row whole: the fetch of one cell (`SELECT c FROM t WHERE … LIMIT
/// 1`), whose blob the inspector wants whole, not the grid's preview. One row bounds it: the
/// last two tokens of code are `LIMIT 1` (a comment after them is no code, so `… -- LIMIT 1`
/// keeps the preview).
pub(super) fn reads_one_row(sql: &str) -> bool {
    let s = scan(&LEX, PLAIN, sql);
    if s.ambiguous {
        return false;
    }
    let mut code = s
        .tokens
        .iter()
        .rev()
        .filter(|t| !matches!(t.kind, TokenKind::Space | TokenKind::Comment));
    match (code.next(), code.next()) {
        (Some(one), Some(limit)) => {
            one.kind == TokenKind::Word
                && s.text(one) == "1"
                && limit.kind == TokenKind::Word
                && s.upper(limit) == "LIMIT"
        }
        _ => false,
    }
}

/// The rows of a finished statement as a result set (empty for one without rows).
fn result_set(sink: CollectSink, outcome: &StatementOutcome) -> ResultSet {
    let mut set = sink.sets.into_iter().next().unwrap_or(ResultSet {
        columns: Vec::new(),
        rows: Vec::new(),
        truncated: false,
    });
    set.truncated = outcome.truncated;
    set
}

/// A statement the app runs, to the end, on `conn`.
fn read_on(
    conn: &Connection,
    sql: &str,
    params: &[Value],
    cancel: &CancelSlot,
) -> Result<ResultSet, AppError> {
    let bytes = if reads_one_row(sql) {
        Bytes::Whole
    } else {
        Bytes::Preview
    };
    let mut sink = CollectSink::default();
    let outcome = conn::execute_streaming(conn, sql, params, None, &mut sink, cancel, bytes)?;
    Ok(result_set(sink, &outcome))
}

/// A statement the app runs for its effect: the count DuckDB reports, or the rows of its
/// RETURNING (each one a changed row).
fn write_on(
    conn: &Connection,
    sql: &str,
    params: &[Value],
    cancel: &CancelSlot,
) -> Result<ExecOutcome, AppError> {
    let mut sink = CollectSink::default();
    let outcome =
        conn::execute_streaming(conn, sql, params, None, &mut sink, cancel, Bytes::Preview)?;
    Ok(match outcome.kind {
        StatementKind::Rows => {
            let rows = result_set(sink, &outcome);
            ExecOutcome {
                affected: rows.rows.len() as u64,
                last_insert_id: None,
                rows: Some(rows),
                affected_unknown: false,
            }
        }
        _ => ExecOutcome {
            affected: outcome.affected.unwrap_or(0),
            last_insert_id: None,
            rows: None,
            affected_unknown: false,
        },
    })
}

/// The schema a table names, or the connection's current one.
fn schema_on(conn: &Connection, table: &TableRef) -> Result<String, AppError> {
    if let Some(s) = table.schema.as_deref().filter(|s| !s.is_empty()) {
        return Ok(s.to_string());
    }
    let (_, rows) = conn::query_all(conn, "SELECT system.main.current_schema()", &[])?;
    Ok(rows
        .first()
        .and_then(|r| catalog::text(r, 0))
        .unwrap_or_else(|| "main".into()))
}

/// A catalog query on `conn`, traced as structure.
fn catalog_on(
    conn: &Connection,
    trace: &SqlTrace,
    sql: &str,
    params: &[Value],
) -> Result<Vec<Vec<Value>>, AppError> {
    trace.run(
        TraceCategory::Structure,
        trace_sql(DIALECT, sql, params),
        |rows: &Vec<Vec<Value>>| TraceCounts::rows(rows.len()),
        || conn::query_all(conn, sql, params).map(|(_, rows)| rows),
    )
}

/// The structure of `table` on `conn`: its row in the catalog, columns, constraints, indexes.
fn table_meta_on(
    conn: &Connection,
    trace: &SqlTrace,
    table: &TableRef,
) -> Result<TableMeta, AppError> {
    let schema = schema_on(conn, table)?;
    let params = [Value::Text(schema.clone()), Value::Text(table.name.clone())];
    let table = TableRef::new(Some(&schema), &table.name);
    let info = catalog::parse_table(&catalog_on(conn, trace, catalog::TABLE, &params)?, &table)?;
    let enums = catalog::parse_enum_types(&catalog_on(conn, trace, catalog::ENUM_TYPES, &[])?);
    let columns =
        catalog::parse_columns(&catalog_on(conn, trace, catalog::COLUMNS, &params)?, &enums);
    let constraints = catalog::parse_constraints(
        &catalog_on(conn, trace, catalog::CONSTRAINTS, &params)?,
        &schema,
        &table.name,
    );
    let indexes = catalog::parse_indexes(&catalog_on(conn, trace, catalog::INDEXES, &params)?);
    Ok(catalog::table_meta(
        table,
        info,
        columns,
        constraints,
        indexes,
    ))
}

impl DuckdbSession {
    /// The session on `instance`: its two connections, each switched to `database` when one is
    /// named (a database switch, or the profile's), then the engine's version, which proves the
    /// connections work.
    pub async fn start(
        instance: Arc<Instance>,
        options: SessionOptions,
    ) -> Result<DuckdbSession, AppError> {
        let SessionOptions {
            read_only,
            database,
            confirm,
        } = options;
        let facts = InstanceFacts {
            server_read_only: instance.is_read_only(),
            hardened: instance.is_hardened(),
            file_read_only: instance.is_file_read_only(),
            shared: instance.file().is_some(),
        };
        let (editor, worker) = tokio::task::spawn_blocking(move || {
            let editor = Worker::spawn(instance.clone())?;
            let worker = Worker::spawn(instance)?;
            Ok::<_, AppError>((Arc::new(editor), Arc::new(worker)))
        })
        .await
        .map_err(|e| AppError::Internal(format!("open duckdb connections: {e}")))??;
        let session = DuckdbSession {
            editor,
            worker,
            facts,
            read_only,
            confirm,
            tx_state: Arc::new(TxState::new(Some(false))),
            editor_runs: tokio::sync::Mutex::new(()),
            trace: SqlTrace::default(),
        };
        let started = async {
            if let Some(db) = database.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                let sql = use_database(db);
                for w in [&session.editor, &session.worker] {
                    let sql = sql.clone();
                    w.run(move |conn| conn.execute_batch(&sql).map_err(errors::map))
                        .await?;
                }
            }
            session.info().await
        };
        if let Err(e) = started.await {
            session.close().await;
            return Err(e);
        }
        Ok(session)
    }

    async fn info(&self) -> Result<DuckdbServerInfo, AppError> {
        let trace = self.trace.clone();
        self.worker
            .run(move |conn| {
                catalog::parse_server_info(&catalog_on(conn, &trace, catalog::SERVER_INFO, &[])?)
            })
            .await
    }

    /// Refuses what no session runs (the logger), and on a read-only session what the
    /// classifier calls a write (or does not know).
    fn admit(&self, sql: &str) -> Result<(), AppError> {
        if switches_logger(sql) {
            return Err(AppError::InvalidInput(LOGGER_REFUSED.into()));
        }
        if self.read_only && writes(DIALECT, sql) {
            return Err(read_only_error());
        }
        Ok(())
    }

    /// Runs `f` on the worker connection; with a slot, the statement can be interrupted while
    /// it runs (armed on the connection thread only, and settled before the connection is used
    /// again, so a late interrupt cannot stop the next job).
    async fn on_worker<R, F>(&self, cancel: Option<CancelSlot>, f: F) -> Result<R, AppError>
    where
        R: Send + 'static,
        F: FnOnce(&Connection, &CancelSlot) -> Result<R, AppError> + Send + 'static,
    {
        let interrupt = self.worker.interrupt_handle();
        self.worker
            .run(move |conn| {
                let Some(slot) = cancel else {
                    return f(conn, &CancelSlot::default());
                };
                let action: Arc<dyn CancelAction> = Arc::new(Interrupt(interrupt));
                let armed = arm_cancel(&slot, CancelHandle::Custom(action))?;
                let result = f(conn, &slot);
                drop(armed);
                if !settle_cancels_blocking(&slot, CANCEL_SETTLE_LIMIT) {
                    log::warn!("duckdb: a cancel request did not finish in time");
                }
                result
            })
            .await
    }

    async fn read(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<CancelSlot>,
    ) -> Result<ResultSet, AppError> {
        self.admit(sql)?;
        let (sql, params, trace) = (sql.to_string(), params.to_vec(), self.trace.clone());
        self.on_worker(cancel, move |conn, slot| {
            trace.run(
                classify_statement(&sql, DIALECT),
                trace_sql(DIALECT, &sql, &params),
                |rs: &ResultSet| TraceCounts::rows(rs.rows.len()),
                || read_on(conn, &sql, &params, slot),
            )
        })
        .await
    }

    async fn write(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<CancelSlot>,
    ) -> Result<ExecOutcome, AppError> {
        self.admit(sql)?;
        let (sql, params, trace) = (sql.to_string(), params.to_vec(), self.trace.clone());
        self.on_worker(cancel, move |conn, slot| {
            trace.run(
                classify_statement(&sql, DIALECT),
                trace_sql(DIALECT, &sql, &params),
                TraceCounts::of_exec,
                || write_on(conn, &sql, &params, slot),
            )
        })
        .await
    }

    /// A catalog read of `sql` with `params` on the worker connection.
    async fn catalog(
        &self,
        sql: &'static str,
        params: Vec<Value>,
    ) -> Result<Vec<Vec<Value>>, AppError> {
        let trace = self.trace.clone();
        self.worker
            .run(move |conn| catalog_on(conn, &trace, sql, &params))
            .await
    }

    /// A catalog read for an object of `table`'s schema (the current one when it names none).
    async fn catalog_of(
        &self,
        sql: &'static str,
        table: &TableRef,
    ) -> Result<Vec<Vec<Value>>, AppError> {
        let (trace, table) = (self.trace.clone(), table.clone());
        self.worker
            .run(move |conn| {
                let schema = schema_on(conn, &table)?;
                catalog_on(
                    conn,
                    &trace,
                    sql,
                    &[Value::Text(schema), Value::Text(table.name)],
                )
            })
            .await
    }
}

#[async_trait]
impl DbSession for DuckdbSession {
    fn driver(&self) -> Driver {
        Driver::Duckdb
    }

    fn dialect(&self) -> SqlDialect {
        DIALECT
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        let info = self.info().await?;
        Ok(ServerInfo {
            version: info.label(),
            current_database: info.database,
            current_user: None,
        })
    }

    async fn ping(&self) -> Result<(), AppError> {
        self.worker
            .run(|conn| conn.execute_batch("SELECT 1").map_err(errors::map))
            .await
    }

    /// Interrupts both connections, ends their threads (each within about 2 s, together) and
    /// lets the instance go: the file closes with the last session that holds it.
    async fn close(&self) {
        self.editor.interrupt();
        self.worker.interrupt();
        let (editor, worker) = (self.editor.clone(), self.worker.clone());
        let closing = tokio::task::spawn_blocking(move || {
            editor.close();
            worker.close();
        });
        if closing.await.is_err() {
            log::warn!("duckdb: closing the connections failed");
        }
    }

    /// The attached databases (catalogs).
    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        let rows = self.catalog(catalog::DATABASES, Vec::new()).await?;
        Ok(catalog::parse_databases(&rows)
            .iter()
            .map(|d| d.info())
            .collect())
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        Ok(catalog::parse_schemas(
            &self.catalog(catalog::SCHEMAS, Vec::new()).await?,
        ))
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let rows = self
            .catalog(catalog::TABLES, vec![Value::Text(schema.to_string())])
            .await?;
        Ok(catalog::parse_tables(&rows, schema))
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let (trace, table) = (self.trace.clone(), table.clone());
        self.worker
            .run(move |conn| table_meta_on(conn, &trace, &table))
            .await
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        Ok(catalog::parse_definition(
            &self.catalog_of(catalog::VIEW_DEFINITION, table).await?,
        ))
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let schema = self.info().await?.schema;
        let rows = self.catalog(catalog::COMPLETION, Vec::new()).await?;
        Ok(catalog::parse_completion(&rows, schema))
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
        self.read(sql, params, Some(cancel)).await
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
        self.write(sql, params, Some(cancel)).await
    }

    /// One transaction on the worker connection: every statement under its guard, rolled back
    /// at the first failure (DuckDB's DDL is transactional too).
    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        for (i, st) in statements.iter().enumerate() {
            self.admit(&st.sql).map_err(|e| at_statement(e, i))?;
        }
        let trace = self.trace.clone();
        self.on_worker(None, move |conn, slot| {
            conn.execute_batch("BEGIN TRANSACTION")
                .map_err(errors::map)?;
            let mut done = Vec::with_capacity(statements.len());
            for (i, st) in statements.iter().enumerate() {
                let result = trace
                    .run(
                        classify_statement(&st.sql, DIALECT),
                        trace_sql(DIALECT, &st.sql, &st.params),
                        TraceCounts::of_exec,
                        || write_on(conn, &st.sql, &st.params, slot),
                    )
                    .and_then(|o| check_affected(st, &o, i).map(|()| o));
                match result {
                    Ok(o) => done.push(o),
                    Err(e) => {
                        let _ = conn.execute_batch("ROLLBACK");
                        return Err(at_statement(e, i));
                    }
                }
            }
            if let Err(e) = conn.execute_batch("COMMIT") {
                // A failed commit (a write-write conflict) leaves the transaction aborted.
                let _ = conn.execute_batch("ROLLBACK");
                return Err(errors::map(e));
            }
            Ok(done)
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
        self.admit(sql)?;
        conn::run(
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
        self.admit(sql)?;
        let started = Instant::now();
        let result = conn::run(&self.worker, sql, params, None, sink, cancel, None).await;
        self.trace.finish(
            classify_statement(sql, DIALECT),
            trace_sql(DIALECT, sql, params),
            started,
            &result,
            |o| TraceCounts::rows(o.row_count as usize),
        );
        Ok(result?.row_count)
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        cancel_portable(handle).await.unwrap_or(Ok(()))
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

    fn capabilities(&self) -> SessionCaps {
        let mut features = FEATURES.to_vec();
        if !self.facts.file_read_only {
            features.extend(EDIT_FEATURES);
        }
        if self.facts.shared {
            // The new session of a switch opens the same file, so it shares this instance.
            features.push(Feature::SwitchDatabase);
        }
        let mut caps = SessionCaps::base(Driver::Duckdb, DIALECT, None).with(&features);
        caps.namespaces = NamespaceCaps {
            databases: self.facts.shared,
            schemas: SchemaMode::Schemas,
            default_schema: Some("main".into()),
            labels: NamespaceLabels::default(),
        };
        caps.data = DataCaps {
            paging: Paging::Offset,
            // `estimated_size` stands for a big table until asked to count.
            count: CountMode::Estimate,
            sort: true,
            filter_ops: FILTER_OPS.to_vec(),
            raw_filter: Some(RawFilter::SqlWhere),
            transactions: TxMode::Atomic,
            affected: Affected::Exact,
            default_keyword: false,
            upsert: false,
            server_exprs: SERVER_EXPRS.to_vec(),
            max_params: max_params(DIALECT) as u32,
        };
        caps.safety = SafetyCaps {
            read_only: if self.facts.server_read_only {
                ReadOnlyMode::Server
            } else {
                ReadOnlyMode::ClassifierOnly
            },
            hardened: self.facts.hardened,
            confirm: self.confirm,
        };
        caps.session = LifecycleCaps {
            keepalive_secs: Some(KEEPALIVE_SECS),
            ping_billed: false,
        };
        caps
    }

    /// The `CREATE TABLE` text DuckDB keeps, with the table's `CREATE INDEX`es; a view's
    /// `CREATE VIEW`.
    async fn table_ddl(&self, table: &TableRef) -> Option<Result<String, AppError>> {
        let ddl = async {
            let rows = self.catalog_of(catalog::TABLE_DDL, table).await?;
            if let Some(ddl) = catalog::parse_table_ddl(&rows) {
                return Ok(ddl);
            }
            let rows = self.catalog_of(catalog::VIEW_DEFINITION, table).await?;
            catalog::parse_definition(&rows)
                .ok_or_else(|| AppError::NotFound(format!("the definition of {}", table.name)))
        };
        Some(ddl.await)
    }

    /// The schema's macros (DuckDB's user-defined functions).
    async fn list_routines(&self, schema: &str) -> Option<Result<Vec<RoutineSummary>, AppError>> {
        let rows = self
            .catalog(catalog::MACROS, vec![Value::Text(schema.to_string())])
            .await;
        Some(rows.map(|rows| catalog::parse_macros(&rows, schema)))
    }

    /// A macro's `CREATE MACRO`, every overload.
    async fn routine_definition(&self, routine: &RoutineRef) -> Option<Result<String, AppError>> {
        let table = TableRef::new(routine.schema.as_deref(), &routine.name);
        let found = async {
            let (trace, table) = (self.trace.clone(), table.clone());
            let (schema, rows) = self
                .worker
                .run(move |conn| {
                    let schema = schema_on(conn, &table)?;
                    let params = [Value::Text(schema.clone()), Value::Text(table.name)];
                    Ok((
                        schema,
                        catalog_on(conn, &trace, catalog::MACRO_DEFINITION, &params)?,
                    ))
                })
                .await?;
            catalog::parse_macro_definition(&rows, &schema, &routine.name)
                .ok_or_else(|| AppError::NotFound(format!("macro {}", routine.name)))
        };
        Some(found.await)
    }

    /// An engine in this process: nothing to encrypt; DuckDB's text is UTF-8.
    async fn session_details(&self) -> Option<Result<SessionDetails, AppError>> {
        Some(Ok(SessionDetails {
            tls: false,
            tls_version: None,
            tls_cipher: None,
            encoding: Some("UTF-8".into()),
        }))
    }
}
