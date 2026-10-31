//! The session of an Elasticsearch or OpenSearch cluster: a native store whose sidebar lists
//! indices, aliases and data streams, whose grid pages hits by cursor, whose edits are one guarded
//! `_bulk` per commit, whose console runs Kibana-style requests and SQL, and whose Running Queries
//! are the cluster's tasks.
//!
//! Read-only is Tabler's to enforce (`ReadOnlyMode::ClassifierOnly`): on a read-only profile the
//! session refuses every write path itself (edits, console commands the classifier does not call
//! reads, write calls, task cancels), next to the commands' own checks.
//!
//! Cancel: a read armed in the cancel slot is dropped (its connection closes, which stops a
//! running search on the server); a console run also cancels the tasks its requests started
//! (found by its `x-opaque-id`), which closing the connection does not stop.

use std::collections::{BTreeMap, HashMap};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::drivers::native::{CallKind, NativePlan, NativeSession, TableSource, unknown_call};
use crate::drivers::{
    CancelAction, CancelHandle, CancelSlot, RowSink, ServerInfo, SqlTrace, StatementOutcome,
    arm_cancel, cancel_portable,
};
use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::native::elastic::{self as build, BrowseSpec, EditPlan, EditTarget};
use crate::sqlgen::split::SplitStatement;
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::browse::{
    BrowsePage, BrowseRequest, FilterGroup, FilterOp, RowCount, RowIdentity, RowKey,
};
use crate::types::caps::{
    Affected, CountMode, DataCaps, Feature, LifecycleCaps, NamespaceCaps, NamespaceLabels, Paging,
    RawFilter, SchemaMode, SessionCaps, TxMode,
};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::{ApplyRequest, ApplyResult, RowResult};
use crate::types::profile::Driver;
use crate::types::query::StatementKind;
use crate::types::schema::{
    ColumnMeta, DatabaseInfo, SchemaInfo, TableKind, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

use super::catalog::{self, ColumnsFor, Field};
use super::classify;
use super::client::EsClient;
use super::console::{self, Command, EsRequest, Method};
use super::info::{ClusterInfo, Features};
use super::json::{self, Json};
use super::params::ElasticParams;
use super::processes;
use super::reply::{self, Grid};
use super::values;

const DRIVER: Driver = Driver::Elasticsearch;

/// The app pings an idle connection this often (`GET /`, free).
pub const KEEPALIVE_SECS: u32 = 60;
/// A ping that takes longer fails.
const PING_TIMEOUT: Duration = Duration::from_secs(15);
/// `close` frees open scrolls for at most this long.
const CLOSE_TIMEOUT: Duration = Duration::from_millis(1500);
/// Cancelling a run's tasks takes at most this long.
const TASK_CANCEL_TIMEOUT: Duration = Duration::from_secs(5);
/// Rows per SQL page of the console.
const SQL_FETCH_SIZE: usize = 1000;

/// What the session knows of a browse target beyond its columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TargetInfo {
    /// The smallest `index.max_result_window` of its indices.
    window: u64,
}

pub struct EsSession {
    client: EsClient,
    info: ClusterInfo,
    features: Features,
    params: ElasticParams,
    read_only: bool,
    /// The Basic user, shown as the session's user.
    user: Option<String>,
    trace: SqlTrace,
    /// The 6.x mapping type of each target (`_doc`, `doc`…), read with its structure.
    types: Mutex<HashMap<String, Option<String>>>,
    targets: Mutex<HashMap<String, TargetInfo>>,
    runs: AtomicU64,
}

impl EsSession {
    pub fn new(
        client: EsClient,
        info: ClusterInfo,
        params: ElasticParams,
        read_only: bool,
        user: Option<String>,
        trace: SqlTrace,
    ) -> EsSession {
        let features = info.features();
        EsSession {
            client,
            info,
            features,
            params,
            read_only,
            user,
            trace,
            types: Mutex::new(HashMap::new()),
            targets: Mutex::new(HashMap::new()),
            runs: AtomicU64::new(0),
        }
    }

    /// Refuses a write on a read-only connection.
    fn writable(&self, what: &str) -> Result<(), AppError> {
        if self.read_only {
            Err(AppError::InvalidInput(format!(
                "this connection is read-only: {what} is refused"
            )))
        } else {
            Ok(())
        }
    }

    fn type_name(&self, target: &str) -> Option<String> {
        self.types
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(target)
            .cloned()
            .flatten()
    }

    /// The result window of `target` (read once per session).
    async fn target_info(&self, target: &str) -> Result<TargetInfo, AppError> {
        if let Some(t) = self
            .targets
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(target)
        {
            return Ok(*t);
        }
        // Serverless and some managed clusters hide `_settings`: the default window then.
        let window = match self.client.result_window(target).await {
            Ok(w) => w,
            Err(AppError::Db { .. }) => catalog::DEFAULT_RESULT_WINDOW,
            Err(e) => return Err(e),
        };
        let info = TargetInfo { window };
        self.targets
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(target.to_string(), info);
        Ok(info)
    }

    /// Runs `work` armed in `cancel`: a cancel drops it (closing its connection), and for a
    /// console run (`opaque`) also cancels the tasks its requests started.
    async fn cancellable<T>(
        &self,
        cancel: &CancelSlot,
        opaque: Option<String>,
        work: impl std::future::Future<Output = Result<T, AppError>>,
    ) -> Result<T, AppError> {
        let token = CancellationToken::new();
        let action = Arc::new(RunCancel {
            token: token.clone(),
            tasks: opaque
                .filter(|_| self.features.tasks)
                .map(|o| (self.client.clone(), o)),
        });
        let _armed = arm_cancel(cancel, CancelHandle::Custom(action))?;
        tokio::select! {
            biased;
            _ = token.cancelled() => Err(AppError::Cancelled),
            r = work => r,
        }
    }

    fn mapped_fields(meta: &TableMeta) -> Vec<Field> {
        meta.columns
            .iter()
            .filter(|c| !catalog::META_COLUMNS.contains(&c.name.as_str()))
            .map(Field::from_column)
            .collect()
    }

    /// Runs one console statement and streams its grids into `sink`.
    async fn console_run(
        &self,
        client: &EsClient,
        command: Command,
        max_rows: usize,
        sink: &mut dyn RowSink,
    ) -> Result<StatementOutcome, AppError> {
        let max_rows = max_rows.max(1);
        let (req, sql_paged) = match command {
            Command::Request(req) => {
                let paged = runs_sql_query(&req);
                (req, paged)
            }
            Command::Sql(sql) => {
                let api = self.features.sql.ok_or_else(|| {
                    AppError::Unsupported(format!(
                        "{} has no SQL endpoint (the OSS distribution, or a plugin owns _sql)",
                        self.info.display_version()
                    ))
                })?;
                (
                    build::sql_query(api, &sql, max_rows.min(SQL_FETCH_SIZE) as u64),
                    true,
                )
            }
        };
        let grids = client.console(&req, max_rows).await?;
        let mut out = Outcome::default();
        for grid in grids {
            let cursor = grid.cursor.clone();
            let kinds: Vec<_> = grid.columns.iter().map(|c| c.kind).collect();
            out.emit(sink, grid);
            match cursor {
                Some(c) if sql_paged => {
                    self.sql_pages(client, &req, c, &kinds, max_rows, sink, &mut out)
                        .await?;
                }
                Some(c) => out.notices.push(format!(
                    "more results: continue with the cursor or scroll id {c}"
                )),
                None => {}
            }
        }
        Ok(StatementOutcome {
            kind: StatementKind::Rows,
            result_sets: out.sets,
            row_count: out.rows as u64,
            truncated: out.truncated,
            affected: None,
            last_insert_id: None,
            notices: out.notices,
        })
    }

    /// Reads the further pages of a SQL result (opened by `req`) while the cap allows, then frees
    /// its cursor.
    #[allow(clippy::too_many_arguments)]
    async fn sql_pages(
        &self,
        client: &EsClient,
        req: &EsRequest,
        mut cursor: String,
        kinds: &[crate::types::schema::ValueKind],
        max_rows: usize,
        sink: &mut dyn RowSink,
        out: &mut Outcome,
    ) -> Result<(), AppError> {
        loop {
            if out.rows >= max_rows || out.stopped {
                out.truncated = true;
                let _ = client.send(&build::sql_close(req, &cursor)).await;
                return Ok(());
            }
            let reply = client.fetch_json(&build::sql_next(req, &cursor)).await?;
            let (rows, more) = reply::sql_page_rows(&reply, kinds, max_rows - out.rows);
            let empty = rows.is_empty();
            out.push_rows(sink, rows);
            match reply::sql_cursor(&reply) {
                // A page without rows ends the result, whatever cursor it hands out: a server
                // that kept handing one out would otherwise be asked for pages without end.
                Some(next) if empty => {
                    let _ = client.send(&build::sql_close(req, &next)).await;
                    return Ok(());
                }
                Some(next) => cursor = next,
                None => {
                    out.truncated |= more;
                    return Ok(());
                }
            }
        }
    }

    /// The structure of an index, alias, data stream or pattern.
    async fn describe(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let target = check_target(&table.name)?;
        let mappings = self.client.mappings(target).await?;
        if mappings.is_empty() {
            return Err(AppError::NotFound(format!("no such index {target}")));
        }
        let single = mappings.len() == 1 && mappings[0].0 == target;
        let kind = if single {
            TableKind::Table
        } else if mappings.iter().all(|(i, _)| i.starts_with(".ds-")) {
            TableKind::Partitioned
        } else {
            TableKind::View
        };
        let merged = catalog::merge(&mappings);
        self.types
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(target.to_string(), merged.type_name.clone());
        let columns = catalog::columns(
            &merged,
            ColumnsFor {
                spans_indices: !single,
                seq_no_guards: self.features.seq_no_guards,
            },
        );
        let primary_key = columns
            .iter()
            .filter(|c| c.is_pk)
            .map(|c| c.name.clone())
            .collect();
        let row_estimate = self.client.doc_count(target).await.ok().flatten();
        let mut comment = Vec::new();
        if !single {
            let names: Vec<&str> = mappings.iter().map(|(i, _)| i.as_str()).collect();
            comment.push(format!("over {}", names.join(", ")));
        }
        if !merged.source_enabled {
            comment.push("_source is disabled: documents cannot be shown or edited".to_string());
        }
        Ok(TableMeta {
            table: TableRef {
                schema: table.schema.clone(),
                name: target.to_string(),
            },
            kind,
            columns,
            primary_key,
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
            triggers: Vec::new(),
            comment: (!comment.is_empty()).then(|| comment.join("; ")),
            row_estimate,
            ddl: None,
            checks: Vec::new(),
            options: None,
        })
    }

    /// The request text that defines `target`: an index's `PUT`, an alias's `_aliases`, a data
    /// stream as `_data_stream` describes it.
    async fn definition_text(&self, target: &str) -> Result<Option<String>, AppError> {
        let mappings = self.client.mappings(target).await?;
        if let [(index, mapping)] = mappings.as_slice()
            && index == target
        {
            let settings = self.client.settings(target).await?;
            let flat = settings
                .into_iter()
                .next()
                .map(|(_, s)| s.flat)
                .unwrap_or_default();
            return Ok(Some(build::index_definition(target, &flat, &mapping.raw)));
        }
        match self.client.json(&build::alias(target)).await {
            Ok(reply) if reply.as_object().is_some_and(|o| !o.is_empty()) => {
                return Ok(Some(build::alias_definition(target, &reply)));
            }
            Ok(_) | Err(AppError::NotFound(_) | AppError::Db { .. }) => {}
            Err(e) => return Err(e),
        }
        if self.features.data_streams {
            match self.client.json(&build::data_stream(target)).await {
                Ok(reply)
                    if reply
                        .get("data_streams")
                        .and_then(Json::as_array)
                        .is_some_and(|s| !s.is_empty()) =>
                {
                    return Ok(Some(build::data_stream_definition(target, &reply)));
                }
                // Not a data stream either, or one this user may not describe.
                Ok(_) | Err(AppError::NotFound(_) | AppError::Db { .. }) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    fn caps(&self) -> SessionCaps {
        session_caps(self.features)
    }
}

/// What an Elasticsearch session can do on a cluster with `features`.
pub fn session_caps(features: Features) -> SessionCaps {
    let mut granted = vec![
        Feature::Editor,
        Feature::CancelOnServer,
        Feature::InsertRows,
        Feature::Returning,
        Feature::ExactCount,
        Feature::ViewDefinition,
    ];
    // Updates and deletes are guarded by `if_seq_no` (6.7+), or not offered.
    if features.seq_no_guards {
        granted.extend([Feature::EditRows, Feature::DeleteRows]);
    }
    // Running Queries are the task management API, which Serverless does not have.
    if features.tasks {
        granted.extend([Feature::ProcessList, Feature::KillProcess]);
    }
    let mut caps = SessionCaps::minimal(DRIVER).with(&granted);
    caps.namespaces = NamespaceCaps {
        databases: false,
        schemas: SchemaMode::Single,
        default_schema: None,
        labels: NamespaceLabels {
            database: "Cluster".into(),
            schema: "Schema".into(),
            table: "Index".into(),
            tables: "Indices".into(),
        },
    };
    let case_insensitive = [
        FilterOp::ContainsCase,
        FilterOp::NotContainsCase,
        FilterOp::HasPrefixCase,
        FilterOp::HasSuffixCase,
        FilterOp::Ilike,
    ];
    let filter_ops = [
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
    ]
    .into_iter()
    .filter(|op| features.case_insensitive || !case_insensitive.contains(op))
    .collect();
    caps.data = DataCaps {
        paging: Paging::Cursor,
        count: CountMode::Costly,
        sort: true,
        filter_ops,
        // A Lucene query string, or Query DSL when it starts with `{`.
        raw_filter: Some(RawFilter::QueryString),
        transactions: TxMode::Batch,
        affected: Affected::Exact,
        default_keyword: false,
        upsert: false,
        server_exprs: Vec::new(),
        max_params: 0,
    };
    caps.safety.read_only = ReadOnlyMode::ClassifierOnly;
    caps.safety.hardened = false;
    caps.session = LifecycleCaps {
        keepalive_secs: Some(KEEPALIVE_SECS),
        ping_billed: false,
    };
    caps
}

/// A browse target as the sidebar names one: an index, alias, data stream or pattern. Never a
/// path: no `/`, `\`, `?`, `#`, whitespace, `.` or `..`, and no leading `_`, `-` or `+` (an
/// endpoint, which index names cannot be).
pub fn check_target(name: &str) -> Result<&str, AppError> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.starts_with(['_', '-', '+'])
        || name.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || matches!(c, '/' | '\\' | '?' | '#' | '"' | '<' | '>' | '|')
        });
    if bad {
        Err(AppError::InvalidInput(format!(
            "\u{201c}{name}\u{201d} is not an index, alias or data stream name"
        )))
    } else {
        Ok(name)
    }
}

/// The one index, alias or data stream an operation of the object menu acts on: a target that is
/// no pattern (`*`), list (`,`) or remote name (`:`), so deleting, truncating or closing one item
/// never reaches every index a pattern matches.
pub fn check_index(name: &str) -> Result<&str, AppError> {
    let name = check_target(name)?;
    if name.contains(['*', ',', ':']) {
        Err(AppError::InvalidInput(format!(
            "\u{201c}{name}\u{201d} names several indices: the operation takes one"
        )))
    } else {
        Ok(name)
    }
}

/// Whether a console request runs a SQL query (`_sql` with a `query`, not `translate` or
/// `_explain`), whose cursor the console follows.
fn runs_sql_query(req: &EsRequest) -> bool {
    let segs: Vec<&str> = req.segments.iter().map(String::as_str).collect();
    classify::is_sql(&segs)
        && !matches!(req.last(), Some("translate" | "_explain"))
        && req.method == Method::Post
        && req
            .body
            .as_ref()
            .and_then(|b| b.values().first())
            .and_then(|b| b.get("query"))
            .is_some()
}

/// What a console statement delivered so far.
#[derive(Default)]
struct Outcome {
    sets: u32,
    rows: usize,
    truncated: bool,
    notices: Vec<String>,
    /// The sink asked for no more rows.
    stopped: bool,
}

impl Outcome {
    fn emit(&mut self, sink: &mut dyn RowSink, grid: Grid) {
        self.sets += 1;
        self.truncated |= grid.truncated;
        self.notices.extend(grid.notices);
        sink.begin_set(grid.columns);
        self.push_rows(sink, grid.rows);
    }

    fn push_rows(&mut self, sink: &mut dyn RowSink, rows: Vec<Vec<Value>>) {
        if rows.is_empty() || self.stopped {
            return;
        }
        self.rows += rows.len();
        if sink.rows(rows) == ControlFlow::Break(()) {
            self.stopped = true;
        }
    }
}

/// Stops one read or console run: drops its request and, for a run, cancels its tasks.
struct RunCancel {
    token: CancellationToken,
    tasks: Option<(EsClient, String)>,
}

impl CancelAction for RunCancel {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        self.token.cancel();
        let tasks = self.tasks.clone();
        Box::pin(async move {
            if let Some((client, opaque)) = tasks {
                // Best effort: a task that already ended has nothing to cancel.
                let _ = tokio::time::timeout(TASK_CANCEL_TIMEOUT, client.cancel_tasks_of(&opaque))
                    .await;
            }
            Ok(())
        })
    }

    fn label(&self) -> &'static str {
        "Elasticsearch request and task cancel"
    }
}

fn key_text(key: &BTreeMap<String, Value>, name: &str) -> Option<String> {
    key.get(name)
        .and_then(Value::as_text)
        .filter(|s| !s.is_empty())
}

/// A JSON value of a call's reply as text the renderer shows (numbers kept as written).
fn text_json(v: &Json) -> serde_json::Value {
    serde_json::Value::String(v.to_pretty())
}

fn arg<'a>(args: &'a serde_json::Value, name: &str) -> Result<&'a str, AppError> {
    args.get(name)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::InvalidInput(format!("{}: {name} is required", DRIVER.label())))
}

#[async_trait]
impl TableSource for EsSession {
    async fn browse(
        &self,
        meta: &TableMeta,
        req: &BrowseRequest,
        cancel: CancelSlot,
    ) -> Result<BrowsePage, AppError> {
        let started = Instant::now();
        let target = check_target(&meta.table.name)?;
        let fields = Self::mapped_fields(meta);
        let columns: Vec<ColumnMeta> = match &req.columns {
            Some(names) => meta
                .columns
                .iter()
                .filter(|c| c.is_pk || names.contains(&c.name))
                .cloned()
                .collect(),
            None => meta.columns.clone(),
        };
        let source = req.columns.as_ref().map(|_| {
            columns
                .iter()
                .filter(|c| !catalog::META_COLUMNS.contains(&c.name.as_str()))
                .map(|c| c.name.clone())
                .collect()
        });
        let spec = BrowseSpec {
            target: target.to_string(),
            query: build::query(&fields, req.filter.as_ref(), self.features)?,
            sort: build::sort(&fields, &req.sort, "_doc")?,
            source,
            seq_no: self.features.seq_no_guards,
        };
        let limit = u64::from(req.limit.max(1));
        let preference = self.client.opaque_id().to_string();
        let page = self
            .cancellable(&cancel, None, async {
                let window = self.target_info(target).await?.window;
                self.client
                    .browse(
                        &spec,
                        self.features,
                        req.offset,
                        limit,
                        window,
                        req.cursor.as_deref(),
                        &preference,
                    )
                    .await
            })
            .await?;
        let rows = values::hit_rows(&page.hits, &columns);
        Ok(BrowsePage {
            columns,
            rows,
            pk_columns: meta.primary_key.clone(),
            identity: RowIdentity::Pk,
            row_ids: None,
            offset: req.offset,
            limit: req.limit,
            sql: page.request,
            duration_ms: started.elapsed().as_millis() as u64,
            next_cursor: page.next_cursor,
        })
    }

    /// An unfiltered count that need not be exact is the indices' document count; anything
    /// else counts with `_count`.
    async fn count(
        &self,
        meta: &TableMeta,
        filter: Option<&FilterGroup>,
        exact: bool,
        cancel: CancelSlot,
    ) -> Result<RowCount, AppError> {
        let target = check_target(&meta.table.name)?;
        if !exact
            && filter.is_none()
            && let Some(n) = meta.row_estimate
        {
            return Ok(RowCount {
                count: n,
                exact: false,
            });
        }
        let query = build::query(&Self::mapped_fields(meta), filter, self.features)?;
        let count = self
            .cancellable(&cancel, None, self.client.count(target, &query))
            .await?;
        Ok(RowCount { count, exact: true })
    }

    async fn fetch_cell(
        &self,
        meta: &TableMeta,
        key: &RowKey,
        column: &str,
    ) -> Result<Value, AppError> {
        let target = check_target(&meta.table.name)?;
        let RowKey::Pk(key) = key else {
            return Err(AppError::Unsupported(
                "Elasticsearch reads a document by its _id".into(),
            ));
        };
        let id = key_text(key, catalog::ID)
            .ok_or_else(|| AppError::InvalidInput("the row has no _id".into()))?;
        let index = key_text(key, catalog::INDEX).unwrap_or_else(|| target.to_string());
        check_target(&index)?;
        let routing = key_text(key, catalog::ROUTING);
        let meta_column = catalog::META_COLUMNS.contains(&column);
        let type_name = self.type_name(target).filter(|_| self.features.typed_paths);
        let doc = self
            .client
            .document(
                &index,
                type_name.as_deref(),
                &id,
                routing.as_deref(),
                (!meta_column).then_some(column),
            )
            .await?
            .ok_or_else(|| AppError::NotFound("the document no longer exists".into()))?;
        if meta_column {
            return Ok(doc.get(column).map_or(Value::Null, |v| match v {
                Json::Str(s) => Value::Text(s.clone()),
                other => values::to_value(other, None),
            }));
        }
        let es_type = values::es_type_of(&Self::mapped_fields(meta), column).map(str::to_string);
        Ok(match doc.get(catalog::SOURCE) {
            Some(source) => values::field_value(source, column, es_type.as_deref()),
            None => values::missing(),
        })
    }
}

#[async_trait]
impl NativeSession for EsSession {
    fn driver(&self) -> Driver {
        DRIVER
    }

    fn capabilities(&self) -> SessionCaps {
        self.caps()
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        Ok(ServerInfo {
            version: self.info.display_version(),
            current_database: Some(self.info.cluster_name.clone()),
            current_user: self.user.clone(),
        })
    }

    async fn ping(&self) -> Result<(), AppError> {
        match tokio::time::timeout(PING_TIMEOUT, self.client.info()).await {
            Ok(r) => r.map(|_| ()),
            Err(_) => Err(AppError::Timeout),
        }
    }

    async fn close(&self) {
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, self.client.close_walks()).await;
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        cancel_portable(handle).await.unwrap_or(Ok(()))
    }

    fn sql_trace(&self) -> Option<&SqlTrace> {
        Some(&self.trace)
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        Ok(vec![DatabaseInfo {
            name: self.info.cluster_name.clone(),
            owner: None,
            encoding: None,
            is_current: true,
        }])
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        Ok(Vec::new())
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let schema = Some(schema.trim()).filter(|s| !s.is_empty());
        self.client
            .tables(schema, self.params.show_system_indices, self.features)
            .await
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        self.describe(table).await
    }

    async fn definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let target = check_target(&table.name)?;
        self.definition_text(target).await
    }

    fn plan_changes(&self, meta: &TableMeta, req: &ApplyRequest) -> Result<NativePlan, AppError> {
        self.writable("editing rows")?;
        let target = check_target(&meta.table.name)?;
        let fields = Self::mapped_fields(meta);
        let type_name = self.type_name(target);
        let plan = build::plan_edits(
            &EditTarget {
                target,
                fields: &fields,
                type_name: type_name.as_deref(),
                features: self.features,
            },
            &req.changes,
        )?;
        // A row names its index (`_index` of an alias's rows): never anything but an index.
        for op in &plan.ops {
            if let Some(index) = op
                .action
                .as_object()
                .and_then(|a| a.first())
                .and_then(|(_, body)| body.get(catalog::INDEX))
                .and_then(Json::as_str)
            {
                check_target(index)?;
            }
        }
        Ok(NativePlan {
            preview: plan.preview.clone(),
            ops: Box::new(plan),
        })
    }

    async fn apply_changes(
        &self,
        _meta: &TableMeta,
        req: &ApplyRequest,
        plan: NativePlan,
    ) -> ApplyResult {
        let started = Instant::now();
        let sql: Vec<String> = plan.preview.iter().map(|(_, t)| t.clone()).collect();
        let failed_all = |e: AppError| {
            let payload = e.to_payload();
            ApplyResult {
                success: false,
                results: req
                    .changes
                    .iter()
                    .map(|c| RowResult {
                        row_id: c.row_id().to_string(),
                        op: c.op_name().to_string(),
                        affected: 0,
                        returned: None,
                        error: Some(payload.clone()),
                    })
                    .collect(),
                sql: sql.clone(),
                error: Some(payload.clone()),
                duration_ms: started.elapsed().as_millis() as u64,
            }
        };
        if let Err(e) = self.writable("editing rows") {
            return failed_all(e);
        }
        let Ok(plan) = plan.ops.downcast::<EditPlan>() else {
            return failed_all(AppError::internal("not an Elasticsearch plan"));
        };
        let outcomes = match self.client.apply(&plan).await {
            Ok(o) => o,
            Err(e) => return failed_all(e),
        };
        let mut results: Vec<RowResult> = req
            .changes
            .iter()
            .map(|c| RowResult {
                row_id: c.row_id().to_string(),
                op: c.op_name().to_string(),
                affected: 0,
                returned: None,
                error: Some(
                    AppError::db("the change was not sent; nothing was applied").to_payload(),
                ),
            })
            .collect();
        for (op, outcome) in plan.ops.iter().zip(outcomes) {
            let Some(r) = results.get_mut(op.change) else {
                continue;
            };
            match outcome {
                Ok(applied) => {
                    r.affected = applied.affected;
                    r.returned = req.returning.then_some(applied.returned);
                    r.error = None;
                }
                Err(e) => r.error = Some(e.to_payload()),
            }
        }
        let error = results.iter().find_map(|r| r.error.clone());
        ApplyResult {
            success: error.is_none(),
            results,
            sql,
            error,
            duration_ms: started.elapsed().as_millis() as u64,
        }
    }

    fn split_console(&self, text: &str) -> Vec<SplitStatement> {
        console::split(text)
    }

    fn classify_console(&self, command: &str) -> StatementClass {
        classify::classify_text(command)
    }

    async fn run_console(
        &self,
        command: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        if self.read_only && classify::classify_text(command) == StatementClass::Write {
            return Err(AppError::InvalidInput(
                "this connection is read-only: the command may write, and it was not run".into(),
            ));
        }
        let parsed = console::parse_command(command)?;
        let run = self.runs.fetch_add(1, Ordering::Relaxed) + 1;
        let client = self.client.for_run(run);
        let opaque = client.opaque_id().to_string();
        self.cancellable(
            &cancel,
            Some(opaque),
            self.console_run(&client, parsed, max_rows, sink),
        )
        .await
    }

    async fn call(
        &self,
        method: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        if self.call_kind(method) == CallKind::Write {
            self.writable(method)?;
        }
        let write = |req: EsRequest| async move {
            let reply = self.client.json(&req).await?;
            Ok::<_, AppError>(serde_json::json!({
                "request": req.to_console_text(),
                "reply": text_json(&reply),
            }))
        };
        match method {
            "clusterHealth" => Ok(text_json(
                &self.client.json(&build::cluster_health()).await?,
            )),
            "indexSettings" => {
                let index = check_target(arg(&args, "index")?)?;
                Ok(text_json(&self.client.json(&build::settings(index)).await?))
            }
            "document" => {
                let index = check_target(arg(&args, "index")?)?;
                let id = arg(&args, "id")?;
                let routing = args.get("routing").and_then(serde_json::Value::as_str);
                let type_name = self.type_name(index).filter(|_| self.features.typed_paths);
                let doc = self
                    .client
                    .document(index, type_name.as_deref(), id, routing, None)
                    .await?;
                Ok(match doc {
                    Some(d) => serde_json::json!({
                        "found": true,
                        "index": d.get("_index").and_then(Json::as_str),
                        "id": d.get("_id").and_then(Json::as_str),
                        "source": d.get(catalog::SOURCE).map(Json::to_pretty),
                    }),
                    None => serde_json::json!({ "found": false }),
                })
            }
            "indexAction" => {
                let index = check_index(arg(&args, "index")?)?;
                write(build::index_action(index, arg(&args, "action")?)?).await
            }
            "deleteIndex" => write(build::delete_index(check_index(arg(&args, "index")?)?)).await,
            "truncate" => write(build::truncate(check_index(arg(&args, "index")?)?)).await,
            "createIndex" => {
                let name = check_index(arg(&args, "name")?)?;
                let body = match args.get("body").and_then(serde_json::Value::as_str) {
                    Some(text) if !text.trim().is_empty() => {
                        Some(json::parse(text).map_err(|e| {
                            AppError::InvalidInput(format!("index body: {}", e.describe(text)))
                        })?)
                    }
                    _ => None,
                };
                write(build::create_index(name, body)).await
            }
            other => Err(unknown_call(DRIVER, other)),
        }
    }

    fn call_kind(&self, method: &str) -> CallKind {
        match method {
            "clusterHealth" | "indexSettings" | "document" => CallKind::Read,
            _ => CallKind::Write,
        }
    }

    async fn list_processes(&self) -> Option<Result<Vec<ProcessRow>, AppError>> {
        if !self.features.tasks {
            return None;
        }
        Some(
            self.client
                .tasks()
                .await
                .map(|tasks| processes::rows(&tasks, self.client.opaque_id())),
        )
    }

    /// Cancels each task; Elasticsearch has no connections, so both scopes cancel the task.
    async fn kill_processes(&self, ids: &[String], _scope: KillScope) -> Option<Vec<KillOutcome>> {
        if !self.features.tasks {
            return None;
        }
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let failed = |e: AppError| KillOutcome {
                id: id.clone(),
                ok: false,
                message: Some(e.to_string()),
            };
            if let Err(e) = self.writable("cancelling tasks") {
                out.push(failed(e));
                continue;
            }
            let req = match build::cancel_task(id) {
                Ok(req) => req,
                Err(e) => {
                    out.push(failed(e));
                    continue;
                }
            };
            out.push(match self.client.send(&req).await {
                Ok(reply) => processes::kill_outcome(id, reply.status, &reply.body),
                Err(e) => failed(e),
            });
        }
        Some(out)
    }
}
