//! The BigQuery session: the REST client of one project, serving the app's introspection (the
//! free metadata calls), the grid (through `TableSource`), the editor and the app's own reads.
//!
//! - Every statement is a query job with an id Tabler picks, so a cancel names it at once
//!   (`JobCancel`: the local wait ends and `jobs.cancel` stops it on the server). Each job carries
//!   the profile's cost guard (`maximumBytesBilled`, never unlimited in the MCP process).
//! - A plain page of a native table is read for free with `tabledata.list`; a sorted or filtered
//!   page, or a page of a view, runs one query whose later pages are read from the same job
//!   (`cursor`).
//! - The editor's statements run in one BigQuery session (temp tables, variables and
//!   transactions last across runs); a read that BigQuery refused at `jobs.insert` because its
//!   session is gone runs once more in a new one.
//! - Read-only is Tabler's classifier alone (every profile has the full scope: `jobs.insert`
//!   refuses `bigquery.readonly`): every statement it calls a write, or does not know, is refused before it is sent, on
//!   every path. The grid's raw WHERE is the user's text, so a filter that would hold another
//!   statement is refused on any session.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

use super::catalog::{self, Table};
use super::client::{Client, DataPage, JobCancel, QueryRun, RunError};
use super::cursor::{Cursor, fingerprint, stale};
use super::decode::{FieldSchema, decode_row, decode_rows, result_column};
use super::jobs::{JobReference, QueryOptions, QueryResponse, new_job_id};
use super::params::{query_parameter, query_parameters};
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::native::{CallKind, TableSource};
use crate::drivers::{
    ArmedCancel, CancelHandle, CancelSlot, CollectSink, DbSession, RowChunker, RowSink, ServerInfo,
    SqlTrace, StatementOutcome, TraceCounts, TxStatement, arm_cancel, cancel_portable,
    check_affected, classify_statement, trace_sql,
};
use crate::error::AppError;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::bigquery as dialect;
use crate::sqlgen::sql::{
    SelectQuery, TableTarget, fetch_cell, inline_params, is_single_statement, select, writes,
};
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::browse::{
    BrowsePage, BrowseRequest, FilterGroup, FilterOp, RowCount, RowIdentity, RowKey, TracedSql,
};
use crate::types::caps::{
    Affected, CountMode, DataCaps, Feature, LifecycleCaps, NamespaceCaps, NamespaceLabels, Paging,
    RawFilter, SafeConfirm, SafetyCaps, SchemaMode, SessionCaps, TxMode,
};
use crate::types::driver::ReadOnlyMode;
use crate::types::objects::{RoutineRef, RoutineSummary};
use crate::types::profile::Driver;
use crate::types::query::{ExecOutcome, ResultSet, StatementKind};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub(super) const DIALECT: SqlDialect = SqlDialect::Bigquery;

/// What the session offers, each proven by the driver's tests (docs/drivers/bigquery.md, Phase B
/// status). No row edits in v1; no Explain (BigQuery's plan comes after a run, and a dry run is
/// the `dryRun` call); no database switch (jobs run and bill in the profile's project); not
/// `Mcp`, which the lead grants once the classifier's red-team review has passed.
const FEATURES: [Feature; 9] = [
    Feature::Editor,
    Feature::StructureView,
    Feature::ViewDefinition,
    Feature::TableDdl,
    Feature::Routines,
    Feature::CancelOnServer,
    Feature::ProcessList,
    Feature::KillProcess,
    Feature::ExactCount,
];

/// The grid's operators: everything GoogleSQL can write but the JSON containment of
/// PostgreSQL (case-insensitive matches go through `LOWER(…) LIKE LOWER(…)`).
pub const FILTER_OPS: [FilterOp; 23] = [
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

/// The most rows the query of a sorted or filtered page keeps: its later pages come from the
/// same job's results.
pub const QUERY_BROWSE_LIMIT: u32 = 1_000_000;
/// Rows per result page read after the first (`jobs.getQueryResults`).
const RESULT_PAGE: u32 = 10_000;
/// Result pages one read follows at most (a billion rows at `RESULT_PAGE`): a server that keeps
/// sending page tokens cannot keep an export busy forever.
const MAX_RESULT_PAGES: usize = 100_000;
/// The most rows `query` reads: the app's own reads are small.
const QUERY_MAX_ROWS: u64 = 100_000;
/// Completion reads only the free metadata calls, and only this much of them.
const COMPLETION_DATASETS: usize = 50;
const COMPLETION_TABLES: usize = 200;
const COMPLETION_CONCURRENCY: usize = 8;
/// Jobs of each state the process list shows.
const PROCESS_LIST_MAX: u32 = 200;

/// The driver call that estimates a statement with a dry run (no job, nothing billed).
pub const DRY_RUN: &str = "dryRun";

/// Who and where a session is, and what every job carries.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// What every query carries: location, default dataset, cost guard, cache, job timeout and
    /// labels.
    pub options: QueryOptions,
    pub extra_projects: Vec<String>,
    pub read_only: bool,
    /// The service account's address, when the login names one.
    pub user: Option<String>,
}

pub struct BigquerySession {
    client: Arc<Client>,
    config: SessionConfig,
    /// The BigQuery session the editor's statements run in, once one was started.
    editor_session: Mutex<Option<String>>,
    editor_runs: tokio::sync::Mutex<()>,
    /// `tables.get` of the tables the session has looked at: a page of rows is read in its
    /// table's schema.
    tables: Mutex<HashMap<(String, String), Arc<Table>>>,
    trace: SqlTrace,
    closed: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
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

/// A job that has finished, its cancel still armed while its pages are read.
struct Started<'s> {
    run: QueryRun,
    stop: CancellationToken,
    _armed: ArmedCancel<'s>,
}

impl Started<'_> {
    fn fields(&self) -> &[FieldSchema] {
        self.run
            .first
            .schema
            .as_ref()
            .map(|s| s.fields.as_slice())
            .unwrap_or_default()
    }
}

/// Whether BigQuery's refusal of a job at `jobs.insert` says the session `id` it named is gone
/// (expired, aborted). It must name that session: a table, or a message, that merely holds the
/// word "session" is the statement's own error.
fn session_gone(e: &RunError, id: &str) -> bool {
    let RunError::Refused(AppError::Db { message, .. } | AppError::NotFound(message)) = e else {
        return false;
    };
    let m = message.to_ascii_lowercase();
    !id.is_empty()
        && message.contains(id)
        && m.contains("session")
        && [
            "not found",
            "expired",
            "terminated",
            "aborted",
            "is not valid",
        ]
        .iter()
        .any(|w| m.contains(w))
}

/// A write the editor did not send again because its session had ended: the hint says so.
fn session_ended(mut e: AppError) -> AppError {
    const HINT: &str = "The BigQuery session had ended, so the statement did not run. Run it again \
                        to start a new session (its temp tables and variables are gone).";
    match &mut e {
        AppError::Db { hint, .. } => *hint = Some(HINT.into()),
        AppError::NotFound(m) => *m = format!("{m}. {HINT}"),
        _ => {}
    }
    e
}

/// `1.5 MB`: bytes as the editor's notice shows them.
fn bytes_text(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// What a finished job cost, for the editor.
fn cost_notice(r: &QueryResponse) -> Option<String> {
    let processed = r.total_bytes_processed?;
    let mut text = format!("BigQuery processed {}", bytes_text(processed));
    if let Some(billed) = r.total_bytes_billed {
        text.push_str(&format!(", billed {}", bytes_text(billed)));
    }
    if r.cache_hit == Some(true) {
        text.push_str(" (cached result)");
    }
    Some(text)
}

impl BigquerySession {
    pub fn new(client: Arc<Client>, config: SessionConfig) -> BigquerySession {
        BigquerySession {
            client,
            config,
            editor_session: Mutex::new(None),
            editor_runs: tokio::sync::Mutex::new(()),
            tables: Mutex::new(HashMap::new()),
            trace: SqlTrace::default(),
            closed: AtomicBool::new(false),
        }
    }

    fn project(&self) -> &str {
        self.client.project()
    }

    fn default_dataset(&self) -> Option<String> {
        self.config
            .options
            .default_dataset
            .as_ref()
            .map(|d| d.dataset_id.clone())
            .filter(|d| !d.is_empty())
    }

    fn check_open(&self) -> Result<(), AppError> {
        if self.closed.load(Ordering::SeqCst) {
            Err(AppError::NotConnected)
        } else {
            Ok(())
        }
    }

    /// Refuses, on a read-only session, what the classifier calls a write (or does not know).
    fn refuse_write(&self, sql: &str) -> Result<(), AppError> {
        if self.config.read_only && writes(DIALECT, sql) {
            Err(read_only_error())
        } else {
            Ok(())
        }
    }

    /// The dataset a table is in: its own, or the profile's default one.
    fn dataset_of(&self, table: &TableRef) -> Result<String, AppError> {
        table
            .schema
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| self.default_dataset())
            .ok_or_else(|| AppError::InvalidInput("BigQuery: choose a dataset first".into()))
    }

    /// `tables.get` of a table, from the session's cache unless `fresh`.
    async fn table(&self, dataset: &str, name: &str, fresh: bool) -> Result<Arc<Table>, AppError> {
        let key = (dataset.to_string(), name.to_string());
        if !fresh && let Some(t) = lock(&self.tables).get(&key) {
            return Ok(Arc::clone(t));
        }
        let t = Arc::new(self.client.get_table(self.project(), dataset, name).await?);
        lock(&self.tables).insert(key, Arc::clone(&t));
        Ok(t)
    }

    /// Runs `sql` as a job (with `job_params`; `shown` are the values the trace inlines) and
    /// waits for it; the first page holds at most `first_page` rows. The job's cancel stays armed
    /// in `cancel` until the returned run is dropped. Traced unless `traced` is false (the
    /// editor's statements, which the renderer logs itself).
    #[allow(clippy::too_many_arguments)]
    async fn start<'s>(
        &self,
        sql: &str,
        job_params: Vec<serde_json::Value>,
        shown: &[Value],
        opts: &QueryOptions,
        first_page: Option<u32>,
        cancel: &'s CancelSlot,
        traced: bool,
    ) -> Result<Started<'s>, AppError> {
        let (job_id, stop, armed) = self.arm_job(opts, cancel)?;
        let run = self
            .client
            .run_query(&job_id, sql, &job_params, opts, first_page, &stop);
        let run = if traced {
            self.trace
                .run_async(
                    classify_statement(sql, DIALECT),
                    trace_sql(DIALECT, sql, shown),
                    |r: &QueryRun| TraceCounts {
                        rows: r
                            .first
                            .schema
                            .as_ref()
                            .map(|_| r.first.total_rows.unwrap_or(r.first.rows.len() as u64)),
                        affected: r.first.affected(),
                    },
                    run,
                )
                .await?
        } else {
            run.await?
        };
        Ok(Started {
            run,
            stop,
            _armed: armed,
        })
    }

    /// A new job id with its cancel armed in `cancel`.
    fn arm_job<'s>(
        &self,
        opts: &QueryOptions,
        cancel: &'s CancelSlot,
    ) -> Result<(String, CancellationToken, ArmedCancel<'s>), AppError> {
        self.check_open()?;
        let job_id = new_job_id();
        let stop = CancellationToken::new();
        let job = JobReference {
            project_id: self.project().to_string(),
            job_id: job_id.clone(),
            location: opts.location.clone(),
        };
        let armed = arm_cancel(
            cancel,
            CancelHandle::Custom(Arc::new(JobCancel {
                client: Arc::clone(&self.client),
                job,
                stop: stop.clone(),
            })),
        )?;
        Ok((job_id, stop, armed))
    }

    /// One run of the editor's `sql` (untraced: the renderer logs it), saying whether BigQuery
    /// refused the job before it existed.
    async fn start_editor<'s>(
        &self,
        sql: &str,
        opts: &QueryOptions,
        first_page: u32,
        cancel: &'s CancelSlot,
    ) -> Result<Started<'s>, RunError> {
        let (job_id, stop, armed) = self.arm_job(opts, cancel).map_err(RunError::Refused)?;
        let run = self
            .client
            .try_run_query(&job_id, sql, &[], opts, Some(first_page), &stop)
            .await?;
        Ok(Started {
            run,
            stop,
            _armed: armed,
        })
    }

    /// `start` with positional values (`@p1…`) and the session's options.
    async fn start_app<'s>(
        &self,
        sql: &str,
        params: &[Value],
        first_page: Option<u32>,
        cancel: &'s CancelSlot,
    ) -> Result<Started<'s>, AppError> {
        self.start(
            sql,
            query_parameters(params)?,
            params,
            &self.config.options,
            first_page,
            cancel,
            true,
        )
        .await
    }

    /// Hands the job's rows to `chunker`, page by page, up to `cap` rows (None: all). Returns
    /// whether rows were left unread.
    async fn stream_rows(
        &self,
        started: &Started<'_>,
        chunker: &mut RowChunker<'_>,
        cap: Option<u64>,
    ) -> Result<bool, AppError> {
        let fields = started.fields();
        if fields.is_empty() {
            return Ok(false);
        }
        if chunker
            .begin_set(fields.iter().map(result_column).collect())
            .is_break()
        {
            return Ok(false);
        }
        let full = |c: &RowChunker<'_>| cap.is_some_and(|cap| c.total_rows >= cap);
        let mut next = None::<QueryResponse>;
        let mut seen = None::<String>;
        for _ in 0..MAX_RESULT_PAGES {
            let page = next.as_ref().unwrap_or(&started.run.first);
            for row in &page.rows {
                if full(chunker) {
                    return Ok(true);
                }
                if chunker.push(decode_row(fields, row)).is_break() {
                    return Ok(false);
                }
            }
            let Some(token) = page.page_token.clone().filter(|t| !t.is_empty()) else {
                return Ok(false);
            };
            if full(chunker) {
                return Ok(true);
            }
            if seen.as_ref() == Some(&token) {
                return Err(AppError::Internal(
                    "BigQuery sent the same page of results again".into(),
                ));
            }
            seen = Some(token.clone());
            let read = self
                .client
                .next_page(&started.run.job, &token, Some(RESULT_PAGE));
            next = Some(tokio::select! {
                biased;
                _ = started.stop.cancelled() => return Err(AppError::Cancelled),
                r = read => r?,
            });
        }
        // Rows were left unread past the ceiling.
        Ok(true)
    }

    /// Every row of a finished job, up to `cap`.
    async fn collect(&self, started: &Started<'_>, cap: u64) -> Result<ResultSet, AppError> {
        let mut sink = CollectSink::default();
        let truncated = {
            let mut chunker = RowChunker::new(&mut sink);
            let truncated = self.stream_rows(started, &mut chunker, Some(cap)).await?;
            let _ = chunker.flush();
            truncated
        };
        let mut rs = sink.sets.pop().unwrap_or_default();
        rs.truncated = truncated;
        Ok(rs)
    }

    /// The single text cell a catalog query returns (`ddl`), with `@name` bound to `name`.
    async fn ddl_of(&self, sql: &str, name: &str) -> Result<String, AppError> {
        let slot = CancelSlot::default();
        let params = vec![query_parameter("name", &Value::Text(name.to_string()))?];
        let started = self
            .start(sql, params, &[], &self.config.options, Some(1), &slot, true)
            .await?;
        let rows = decode_rows(started.fields(), &started.run.first.rows);
        match rows.first().and_then(|r| r.first()) {
            Some(Value::Text(ddl)) => Ok(ddl.clone()),
            _ => Err(AppError::NotFound(format!("the DDL of {name}"))),
        }
    }

    /// One statement of a commit, in the commit's BigQuery session.
    async fn exec_in(
        &self,
        sql: &str,
        params: &[Value],
        opts: &QueryOptions,
    ) -> Result<ExecOutcome, AppError> {
        let slot = CancelSlot::default();
        let started = self
            .start(
                sql,
                query_parameters(params)?,
                params,
                opts,
                Some(RESULT_PAGE),
                &slot,
                true,
            )
            .await?;
        self.outcome(&started).await
    }

    /// What a finished write reports: the DML count BigQuery gives (none for DDL), and its rows
    /// when it returned some.
    async fn outcome(&self, started: &Started<'_>) -> Result<ExecOutcome, AppError> {
        let affected = started.run.first.affected();
        let rows = if started.fields().is_empty() {
            None
        } else {
            Some(self.collect(started, QUERY_MAX_ROWS).await?)
        };
        Ok(ExecOutcome {
            affected: affected.unwrap_or(0),
            last_insert_id: None,
            rows,
            affected_unknown: affected.is_none(),
        })
    }

    /// Undoes a commit that failed: the transaction, then its session (best effort: a session
    /// left behind expires on its own).
    async fn abandon(&self, opts: &QueryOptions) {
        for sql in [dialect::ROLLBACK_TRANSACTION, dialect::ABORT_SESSION] {
            let _ = self.exec_in(sql, &[], opts).await;
        }
    }

    /// The editor's run of `sql` in its BigQuery session: a new one on the first run, and once
    /// more for a read that BigQuery refused because the session it named is gone.
    async fn editor_start<'s>(
        &self,
        sql: &str,
        first_page: u32,
        cancel: &'s CancelSlot,
    ) -> Result<Started<'s>, AppError> {
        let known = lock(&self.editor_session).clone();
        let mut opts = self.config.options.clone();
        match &known {
            Some(id) => opts.session_id = Some(id.clone()),
            None => opts.create_session = true,
        }
        let first = self.start_editor(sql, &opts, first_page, cancel).await;
        let started = match (first, &known) {
            // Only a refusal at `jobs.insert` that names the session: the job never started, so
            // none of the statement ran. A failure after that (a script's later statement) is the
            // statement's own error and is never sent again.
            (Err(e), Some(id)) if session_gone(&e, id) => {
                *lock(&self.editor_session) = None;
                if writes(DIALECT, sql) {
                    // A write is not sent again unasked: the user runs it once more.
                    return Err(session_ended(e.into_error()));
                }
                opts.session_id = None;
                opts.create_session = true;
                self.start_editor(sql, &opts, first_page, cancel)
                    .await
                    .map_err(RunError::into_error)?
            }
            (first, _) => first.map_err(RunError::into_error)?,
        };
        if let Some(id) = &started.run.session_id {
            *lock(&self.editor_session) = Some(id.clone());
        }
        Ok(started)
    }

    /// The browse select of a sorted or filtered page (or a page of a view): one query whose
    /// pages come from its job.
    fn browse_query(
        &self,
        meta: &TableMeta,
        dataset: &str,
        req: &BrowseRequest,
    ) -> Result<crate::sqlgen::select::BuiltSql, AppError> {
        let table = TableRef::new(Some(dataset), &meta.table.name);
        let built = select(
            DIALECT,
            &SelectQuery {
                table: &table,
                columns: &meta.columns,
                selected: req.columns.as_deref(),
                filter: req.filter.as_ref(),
                sort: &req.sort,
                limit: QUERY_BROWSE_LIMIT,
                offset: 0,
                // No ORDER BY of its own: the job's result keeps one order for every page.
                pk: &[],
                identity: RowIdentity::None,
            },
        )?;
        refuse_added_statement(&built.sql)?;
        self.refuse_write(&built.sql)?;
        Ok(built)
    }
}

/// The columns a page shows, in the order asked for.
fn page_columns(meta: &TableMeta, req: &BrowseRequest) -> Vec<crate::types::schema::ColumnMeta> {
    match &req.columns {
        Some(names) => names
            .iter()
            .filter_map(|n| meta.columns.iter().find(|c| &c.name == n).cloned())
            .collect(),
        None => meta.columns.clone(),
    }
}

/// A `tabledata.list` row (every field of the table) cut to the columns asked for.
fn project_row(fields: &[FieldSchema], row: &serde_json::Value, wanted: &[usize]) -> Vec<Value> {
    let all = decode_row(fields, row);
    wanted
        .iter()
        .map(|&i| {
            all.get(i)
                .cloned()
                .unwrap_or_else(|| Value::other("missing", ""))
        })
        .collect()
}

#[async_trait]
impl DbSession for BigquerySession {
    fn driver(&self) -> Driver {
        Driver::Bigquery
    }

    fn dialect(&self) -> SqlDialect {
        DIALECT
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        Ok(ServerInfo {
            version: "BigQuery".into(),
            current_database: Some(self.project().to_string()),
            current_user: self.config.user.clone(),
        })
    }

    /// One dataset of the project: a free metadata call, no job.
    async fn ping(&self) -> Result<(), AppError> {
        self.check_open()?;
        self.client.ping().await
    }

    /// Nothing to close on the server: BigQuery sessions expire on their own, and a run still
    /// going is the caller's to cancel.
    async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        self.check_open()?;
        // The account may not be allowed to list projects: its own and the extra ones remain.
        let listed = self.client.list_projects().await.unwrap_or_default();
        Ok(catalog::databases(
            self.project(),
            &self.config.extra_projects,
            &listed,
        ))
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        self.check_open()?;
        let datasets = self.client.list_datasets(self.project()).await?;
        Ok(catalog::schemas(
            &datasets,
            self.default_dataset().as_deref(),
        ))
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        self.check_open()?;
        let tables = self.client.list_tables(self.project(), schema).await?;
        Ok(catalog::table_summaries(&tables, schema))
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        self.check_open()?;
        let dataset = self.dataset_of(table)?;
        let t = self.table(&dataset, &table.name, true).await?;
        Ok(catalog::table_meta(&t))
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        self.check_open()?;
        let dataset = self.dataset_of(table)?;
        let t = self.table(&dataset, &table.name, true).await?;
        Ok(catalog::view_definition(&t))
    }

    /// Datasets, their tables and the default dataset's columns, from the free metadata calls
    /// only (an INFORMATION_SCHEMA read would bill each time).
    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        self.check_open()?;
        let default = self.default_dataset();
        let mut schemas: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
        let datasets = self.client.list_datasets(self.project()).await?;
        for d in catalog::schemas(&datasets, default.as_deref())
            .into_iter()
            .take(COMPLETION_DATASETS)
        {
            let tables = self.client.list_tables(self.project(), &d.name).await?;
            let names: BTreeMap<String, Vec<String>> = catalog::table_summaries(&tables, &d.name)
                .into_iter()
                .map(|t| (t.name, Vec::new()))
                .collect();
            schemas.insert(d.name, names);
        }
        if let Some(ds) = default.as_deref()
            && let Some(tables) = schemas.get_mut(ds)
        {
            let names: Vec<String> = tables.keys().take(COMPLETION_TABLES).cloned().collect();
            let columns: Vec<(String, Vec<String>)> = futures::stream::iter(names)
                .map(|name| async move {
                    let cols = match self.table(ds, &name, true).await {
                        Ok(t) => catalog::table_meta(&t)
                            .columns
                            .into_iter()
                            .map(|c| c.name)
                            .collect(),
                        Err(_) => Vec::new(),
                    };
                    (name, cols)
                })
                .buffer_unordered(COMPLETION_CONCURRENCY)
                .collect()
                .await;
            for (name, cols) in columns {
                tables.insert(name, cols);
            }
        }
        Ok(CompletionMetadata {
            default_schema: default,
            schemas,
        })
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
        let started = self
            .start_app(sql, params, Some(RESULT_PAGE), &cancel)
            .await?;
        self.collect(&started, QUERY_MAX_ROWS).await
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
        self.refuse_write(sql)?;
        let started = self
            .start_app(sql, params, Some(RESULT_PAGE), &cancel)
            .await?;
        self.outcome(&started).await
    }

    /// One multi-statement transaction in a BigQuery session of its own, each statement a job
    /// whose DML count is checked before the next one runs; any failure rolls it all back.
    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        if self.config.read_only {
            return Err(read_only_error());
        }
        if statements.is_empty() {
            return Ok(Vec::new());
        }
        let mut opts = self.config.options.clone();
        opts.create_session = true;
        let slot = CancelSlot::default();
        let begin = self
            .start(
                dialect::BEGIN_TRANSACTION,
                Vec::new(),
                &[],
                &opts,
                None,
                &slot,
                true,
            )
            .await?;
        let session = begin.run.session_id.clone().ok_or_else(|| {
            AppError::db("BigQuery started no session for the transaction, so nothing was changed")
        })?;
        drop(begin);
        opts.create_session = false;
        opts.session_id = Some(session);
        let mut out = Vec::with_capacity(statements.len());
        for (i, st) in statements.iter().enumerate() {
            let checked = match self.exec_in(&st.sql, &st.params, &opts).await {
                Ok(outcome) => check_affected(st, &outcome, i).map(|()| outcome),
                Err(e) => Err(e.with_statement_index(i)),
            };
            match checked {
                Ok(outcome) => out.push(outcome),
                Err(e) => {
                    self.abandon(&opts).await;
                    return Err(e);
                }
            }
        }
        if let Err(e) = self.exec_in(dialect::COMMIT_TRANSACTION, &[], &opts).await {
            self.abandon(&opts).await;
            return Err(e);
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
        self.refuse_write(sql)?;
        let cap = max_rows as u64;
        let first_page = u32::try_from(max_rows)
            .unwrap_or(u32::MAX)
            .clamp(1, RESULT_PAGE);
        let started = self.editor_start(sql, first_page, &cancel).await?;
        let mut chunker = RowChunker::new(sink);
        let truncated = self.stream_rows(&started, &mut chunker, Some(cap)).await?;
        let _ = chunker.flush();
        let first = &started.run.first;
        let affected = first.affected();
        let kind = if !started.fields().is_empty() {
            StatementKind::Rows
        } else if affected.is_some() {
            StatementKind::Affected
        } else {
            StatementKind::Empty
        };
        Ok(StatementOutcome {
            kind,
            result_sets: chunker.sets,
            row_count: chunker.total_rows,
            truncated,
            affected,
            last_insert_id: None,
            notices: cost_notice(first).into_iter().collect(),
        })
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
        let started = self
            .start_app(sql, params, Some(RESULT_PAGE), &cancel)
            .await?;
        let mut chunker = RowChunker::new(sink);
        self.stream_rows(&started, &mut chunker, None).await?;
        let _ = chunker.flush();
        Ok(chunker.total_rows)
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        cancel_portable(handle).await.unwrap_or(Ok(()))
    }

    /// Unknown: a transaction may be open in the editor's BigQuery session, and `BEGIN` also
    /// opens a block of statements.
    fn in_transaction(&self) -> Option<bool> {
        None
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
        let mut caps = SessionCaps::base(Driver::Bigquery, DIALECT, None).with(&FEATURES);
        caps.namespaces = NamespaceCaps {
            databases: true,
            schemas: SchemaMode::Schemas,
            default_schema: self.default_dataset(),
            labels: NamespaceLabels {
                database: "Project".into(),
                schema: "Dataset".into(),
                ..NamespaceLabels::default()
            },
        };
        caps.data = DataCaps {
            // Page tokens: of `tabledata.list`, or of one query job's results.
            paging: Paging::Cursor,
            // An unfiltered count is the table's `numRows`; any other runs a billed query.
            count: CountMode::Costly,
            sort: true,
            filter_ops: FILTER_OPS.to_vec(),
            raw_filter: Some(RawFilter::SqlWhere),
            // `transaction` is one BigQuery transaction; no row edits in v1.
            transactions: TxMode::Atomic,
            affected: Affected::Exact,
            default_keyword: false,
            upsert: false,
            server_exprs: Vec::new(),
            max_params: dialect::SPEC.max_params,
        };
        caps.safety = SafetyCaps {
            read_only: ReadOnlyMode::ClassifierOnly,
            // SQL reaches other services (EXTERNAL_QUERY, remote functions, ML): the classifier
            // calls those writes, but the engine is not hardened.
            hardened: false,
            confirm: SafeConfirm::Passcode,
        };
        caps.session = LifecycleCaps {
            keepalive_secs: None,
            ping_billed: false,
        };
        caps
    }

    fn table_source(&self) -> Option<&dyn TableSource> {
        Some(self)
    }

    /// `dryRun` `{sql}`, or `{table: {schema, name}, project?}` for a full scan of one table (the
    /// statement built here, so the renderer writes no SQL): what it would process (and bill),
    /// and its columns. A dry run runs nothing, whatever the statement.
    async fn call(
        &self,
        method: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        if method != DRY_RUN {
            return Err(crate::drivers::native::unknown_call(
                Driver::Bigquery,
                method,
            ));
        }
        self.check_open()?;
        fn text(v: Option<&serde_json::Value>) -> Option<&str> {
            v.and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty())
        }
        let sql = match args.get("table") {
            Some(table) => {
                let (Some(dataset), Some(name)) =
                    (text(table.get("schema")), text(table.get("name")))
                else {
                    return Err(AppError::InvalidInput(
                        "BigQuery: dryRun needs the table's dataset and name".into(),
                    ));
                };
                dialect::full_scan_query(text(args.get("project")), dataset, name)
            }
            None => text(args.get("sql"))
                .ok_or_else(|| AppError::InvalidInput("BigQuery: dryRun needs the sql".into()))?
                .to_owned(),
        };
        let dry = self.client.dry_run(&sql, &[], &self.config.options).await?;
        let columns: Vec<serde_json::Value> = dry
            .schema
            .map(|s| s.fields)
            .unwrap_or_default()
            .iter()
            .map(|f| serde_json::json!({ "name": f.name, "dataType": f.data_type() }))
            .collect();
        Ok(serde_json::json!({
            // An int64 string, as the API writes it: no number beyond 2^53 loses digits.
            "bytesProcessed": dry.bytes_processed.map(|b| b.to_string()),
            "columns": columns,
        }))
    }

    fn call_kind(&self, method: &str) -> CallKind {
        if method == DRY_RUN {
            CallKind::Read
        } else {
            CallKind::Write
        }
    }

    async fn list_processes(&self) -> Option<Result<Vec<ProcessRow>, AppError>> {
        if let Err(e) = self.check_open() {
            return Some(Err(e));
        }
        let now = chrono::Utc::now().timestamp_millis();
        Some(
            self.client
                .list_running_jobs(PROCESS_LIST_MAX)
                .await
                .map(|jobs| catalog::process_rows(&jobs, now)),
        )
    }

    /// `jobs.cancel` of each job; refused on a read-only session (it stops someone's work).
    async fn kill_processes(&self, ids: &[String], _scope: KillScope) -> Option<Vec<KillOutcome>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let result = if self.config.read_only {
                Err(read_only_error())
            } else {
                match catalog::parse_process_id(id) {
                    Some((project_id, location, job_id)) => {
                        self.client
                            .cancel_job(&JobReference {
                                project_id,
                                job_id,
                                location,
                            })
                            .await
                    }
                    None => Err(AppError::InvalidInput(format!(
                        "\u{201c}{id}\u{201d} is not a BigQuery job"
                    ))),
                }
            };
            out.push(KillOutcome {
                id: id.clone(),
                ok: result.is_ok(),
                message: result.err().map(|e| e.to_string()),
            });
        }
        Some(out)
    }

    /// The CREATE statement BigQuery keeps (an INFORMATION_SCHEMA read: billed, on demand).
    async fn table_ddl(&self, table: &TableRef) -> Option<Result<String, AppError>> {
        let dataset = match self.dataset_of(table) {
            Ok(d) => d,
            Err(e) => return Some(Err(e)),
        };
        let sql = dialect::table_ddl_query(Some(self.project()), &dataset);
        Some(self.ddl_of(&sql, &table.name).await)
    }

    async fn list_routines(&self, schema: &str) -> Option<Result<Vec<RoutineSummary>, AppError>> {
        if let Err(e) = self.check_open() {
            return Some(Err(e));
        }
        Some(
            self.client
                .list_routines(self.project(), schema)
                .await
                .map(|r| catalog::routine_summaries(&r, schema)),
        )
    }

    async fn routine_definition(&self, routine: &RoutineRef) -> Option<Result<String, AppError>> {
        let dataset = match self.dataset_of(&TableRef::new(routine.schema.as_deref(), "")) {
            Ok(d) => d,
            Err(e) => return Some(Err(e)),
        };
        let sql = dialect::routine_ddl_query(Some(self.project()), &dataset);
        Some(self.ddl_of(&sql, &routine.name).await)
    }
}

#[async_trait]
impl TableSource for BigquerySession {
    /// One page: a native table's from `tabledata.list` (free) while it is neither sorted nor
    /// filtered, otherwise from one query job's results.
    async fn browse(
        &self,
        meta: &TableMeta,
        req: &BrowseRequest,
        cancel: CancelSlot,
    ) -> Result<BrowsePage, AppError> {
        self.check_open()?;
        if req.cursor.is_none() && req.offset > 0 {
            return Err(AppError::InvalidInput(
                "BigQuery pages follow one another: there is no offset to start from".into(),
            ));
        }
        let started_at = std::time::Instant::now();
        let dataset = self.dataset_of(&meta.table)?;
        let name = &meta.table.name;
        let table = self.table(&dataset, name, false).await?;
        let columns = page_columns(meta, req);
        let limit = req.limit.max(1);
        let native = table.ty.as_deref().is_none_or(|t| t == "TABLE");
        let (rows, sql, next_cursor) = if native && req.filter.is_none() && req.sort.is_empty() {
            let fp = fingerprint(&["data", self.project(), &dataset, name]);
            let token = match req.cursor.as_deref() {
                Some(text) => match Cursor::decode(text, &fp)? {
                    Cursor::Data { token, .. } => Some(token),
                    Cursor::Job { .. } => return Err(stale()),
                },
                None => None,
            };
            let stop = CancellationToken::new();
            let _armed = arm_cancel(
                &cancel,
                CancelHandle::Custom(Arc::new(super::client::StopOnly(stop.clone()))),
            )?;
            let page = DataPage {
                page_token: token,
                max_results: Some(limit),
                ..DataPage::default()
            };
            let read = self
                .client
                .list_table_data(self.project(), &dataset, name, &page);
            let page = tokio::select! {
                biased;
                _ = stop.cancelled() => return Err(AppError::Cancelled),
                r = read => r?,
            };
            let fields = table
                .schema
                .as_ref()
                .map(|s| s.fields.as_slice())
                .unwrap_or_default();
            let wanted: Vec<usize> = columns
                .iter()
                .filter_map(|c| fields.iter().position(|f| f.name == c.name))
                .collect();
            let rows = page
                .rows
                .iter()
                .map(|r| project_row(fields, r, &wanted))
                .collect();
            let next = page
                .page_token
                .filter(|t| !t.is_empty())
                .map(|t| Cursor::data(&fp, &t).encode());
            (
                rows,
                dialect::tabledata_display(Some(self.project()), &dataset, name),
                next,
            )
        } else {
            let built = self.browse_query(meta, &dataset, req)?;
            let fp = fingerprint(&[
                "job",
                &built.sql,
                &serde_json::to_string(&built.params).unwrap_or_default(),
            ]);
            let shown = inline_params(DIALECT, &built.sql, &built.params);
            let (page, job) = match req.cursor.as_deref() {
                Some(text) => {
                    let Cursor::Job {
                        project,
                        location,
                        job,
                        token,
                        ..
                    } = Cursor::decode(text, &fp)?
                    else {
                        return Err(stale());
                    };
                    let job = JobReference {
                        project_id: project,
                        job_id: job,
                        location,
                    };
                    let stop = CancellationToken::new();
                    let _armed = arm_cancel(
                        &cancel,
                        CancelHandle::Custom(Arc::new(super::client::StopOnly(stop.clone()))),
                    )?;
                    let read = self.client.next_page(&job, &token, Some(limit));
                    let page = tokio::select! {
                        biased;
                        _ = stop.cancelled() => return Err(AppError::Cancelled),
                        r = read => r?,
                    };
                    (page, job)
                }
                None => {
                    let started = self
                        .start_app(&built.sql, &built.params, Some(limit), &cancel)
                        .await?;
                    (started.run.first.clone(), started.run.job.clone())
                }
            };
            let fields = page
                .schema
                .as_ref()
                .map(|s| s.fields.as_slice())
                .unwrap_or_default();
            let rows = decode_rows(fields, &page.rows);
            let next = page
                .page_token
                .filter(|t| !t.is_empty())
                .map(|t| Cursor::job(&fp, &job, &t).encode());
            (rows, shown, next)
        };
        Ok(BrowsePage {
            columns,
            rows,
            pk_columns: meta.primary_key.clone(),
            identity: if meta.primary_key.is_empty() {
                RowIdentity::None
            } else {
                RowIdentity::Pk
            },
            row_ids: None,
            offset: req.offset,
            limit: req.limit,
            sql,
            duration_ms: started_at.elapsed().as_millis() as u64,
            next_cursor,
        })
    }

    /// The table's `numRows` unless asked to count or filtered (a view has none); a count is a
    /// query (an unfiltered `COUNT(*)` of a table reads its metadata and bills nothing).
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
            // Rows still in the streaming buffer are not in it yet.
            return Ok(RowCount {
                count: estimate,
                exact: false,
            });
        }
        let dataset = self.dataset_of(&meta.table)?;
        let table = TableRef::new(Some(&dataset), &meta.table.name);
        let built = crate::sqlgen::sql::count(DIALECT, &table, &meta.columns, filter)?;
        refuse_added_statement(&built.sql)?;
        self.refuse_write(&built.sql)?;
        let started = self
            .start_app(&built.sql, &built.params, Some(1), &cancel)
            .await?;
        let rows = decode_rows(started.fields(), &started.run.first.rows);
        let count = match rows.first().and_then(|r| r.first()) {
            Some(Value::Int(n)) => *n,
            Some(Value::BigInt(s)) => s.parse().unwrap_or(i64::MAX),
            _ => 0,
        };
        Ok(RowCount { count, exact: true })
    }

    /// The whole cell, read again by the row's key (a query, billed; the grid's pages already
    /// hold whole values).
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
        let dataset = self.dataset_of(&meta.table)?;
        let table = TableRef::new(Some(&dataset), &meta.table.name);
        let (sql, params) = fetch_cell(
            DIALECT,
            &TableTarget {
                table: &table,
                columns: &meta.columns,
            },
            col,
            key,
        )?;
        refuse_added_statement(&sql)?;
        self.refuse_write(&sql)?;
        let slot = CancelSlot::default();
        let started = self.start_app(&sql, &params, Some(1), &slot).await?;
        let rows = decode_rows(started.fields(), &started.run.first.rows);
        rows.into_iter()
            .next()
            .and_then(|r| r.into_iter().next())
            .ok_or_else(|| AppError::NotFound("the row no longer exists".into()))
    }
}
