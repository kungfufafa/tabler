//! The DynamoDB session: a `NativeSession` and `TableSource` over the signed client. The region is
//! the sidebar's one database and schema, `ListTables` its tables; grid pages are Scan or Query
//! pages with the `LastEvaluatedKey` as the cursor; edits are conditional writes that prove each
//! change hit the item the grid showed (one request, or transactions of at most 100 items); the
//! console runs PartiQL and API commands.
//!
//! Read-only is Tabler's classifier alone (DynamoDB has no read-only session): a read-only
//! profile's session refuses edits, console commands the classifier calls writes and calls that
//! are not reads, before anything is sent. Every request in flight stops on a cancel (its
//! `CancelHandle::Custom` drops it, or the pause between retries) or when the session closes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::future::BoxFuture;
use serde_json::{Value as J, json};
use tokio_util::sync::CancellationToken;

use super::attr::Item;
use super::auth;
use super::browse::{self, GridOptions, IndexRead, Outcome};
use super::catalog;
use super::client::{CallError, Client};
use super::params::{DynamoAuth, DynamoParams, check_region, is_aws_host, normalize_endpoint};
use super::transport::HttpTransport;
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::common::http::{self, Credential, HttpClient, HttpOptions};
use crate::drivers::native::{CallKind, NativePlan, NativeSession, TableSource, unknown_call};
use crate::drivers::{
    ArmedCancel, CancelAction, CancelHandle, CancelSlot, ConnectTarget, RowSink, ServerInfo,
    SqlTrace, StatementOutcome, arm_cancel, cancel_portable,
};
use crate::error::{AppError, ErrorPayload};
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::native::dynamo::{self as b, ConsoleCommand, EditPlan, LoadedItems};
use crate::sqlgen::split::SplitStatement;
use crate::types::browse::{BrowsePage, BrowseRequest, FilterGroup, FilterOp, RowCount, RowKey};
use crate::types::caps::{
    Affected, CountMode, DataCaps, Feature, NamespaceCaps, NamespaceLabels, Paging, SchemaMode,
    SessionCaps, TxMode,
};
use crate::types::mutate::{ApplyRequest, ApplyResult, RowResult, StagedChange};
use crate::types::profile::{ConnectionProfile, Driver, SslMode};
use crate::types::query::StatementKind;
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

/// A request that has not answered in this long is given up (`Timeout`): a reply is at most a
/// few MB, and the console's and the grid's time outs cancel sooner.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The tables `completion_metadata` names at most.
const MAX_COMPLETION_TABLES: usize = 1000;

/// The bytes of loaded items (`LoadedItems`) the session keeps over all its tables.
const LOADED_BYTES: usize = 2 * LoadedItems::BYTE_CAP;

/// The filter operators the grid offers: what `sqlgen::native::dynamo` turns into
/// `KeyConditionExpression` and `FilterExpression` (case-sensitive, LIKE only as a prefix).
pub const FILTER_OPS: [FilterOp; 17] = [
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
    FilterOp::HasPrefix,
];

/// The session's driver calls (`session_call`), all reads.
const CALLS: [&str; 2] = ["tableInfo", "browseIndex"];

fn read_only_error() -> AppError {
    AppError::InvalidInput(READ_ONLY_MESSAGE.into())
}

/// Stops the requests a cancel token was handed to (`Client::call` races them against it).
struct TokenCancel(CancellationToken);

impl CancelAction for TokenCancel {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        self.0.cancel();
        Box::pin(async { Ok(()) })
    }

    fn label(&self) -> &'static str {
        "DynamoDB request cancel"
    }
}

pub struct DynamoSession {
    client: Client,
    params: DynamoParams,
    region: String,
    /// The profile names an endpoint of its own (an emulator, a VPC or FIPS endpoint).
    custom_endpoint: bool,
    read_only: bool,
    caps: SessionCaps,
    trace: SqlTrace,
    console: tokio::sync::Mutex<()>,
    /// The items the grid showed, per table: their types and values type and guard the edits.
    loaded: std::sync::Mutex<HashMap<String, LoadedItems>>,
    /// The attributes seen per table, for the console's completions.
    attributes: std::sync::Mutex<HashMap<String, Vec<String>>>,
    /// Cancelled when the session closes: every request still in flight stops.
    closed: CancellationToken,
    /// The server has no `DescribeEndpoints` (DynamoDB Local): a ping lists one table instead.
    ping_lists: AtomicBool,
}

/// Where a session connects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub url: String,
    /// Not the region's own endpoint (the server info names it).
    pub custom: bool,
    /// The form's TLS mode applies: only to an endpoint the form names (or DynamoDB Local's)
    /// off AWS's domains. AWS's own hosts and an AWS profile's `endpoint_url` (which the AWS
    /// SDKs always verify) are verified in full, whatever the form's mode.
    pub form_tls: bool,
}

/// The endpoint of `params` in `region`; `aws_config`: the AWS profile's `endpoint_url`, used
/// when the form names none.
pub fn endpoint(
    params: &DynamoParams,
    region: &str,
    aws_config: Option<&str>,
) -> Result<Endpoint, AppError> {
    let own = params
        .endpoint
        .as_deref()
        .is_some_and(|e| !e.trim().is_empty());
    let from_config = aws_config.filter(|_| !own);
    let url = match from_config {
        Some(e) => normalize_endpoint(e)?,
        None => params.endpoint_url(region)?,
    };
    let form = own || (from_config.is_none() && params.auth == DynamoAuth::Local);
    Ok(Endpoint {
        custom: own || from_config.is_some() || params.auth == DynamoAuth::Local,
        form_tls: form && !is_aws_host(&url),
        url,
    })
}

/// The HTTP client's options: the profile's TLS (mode, CA file, client certificate) where the
/// form's mode applies (`form_tls`: its plain HTTPS modes are for emulators and private hosts),
/// else full verification with the profile's CA file and client certificate. No retries: the
/// client repeats what may be repeated, under its own throttling loop.
pub fn http_options(profile: &ConnectionProfile, form_tls: bool) -> Result<HttpOptions, AppError> {
    let mut options = HttpOptions::for_profile(profile)?;
    if !form_tls {
        options.tls.mode = SslMode::VerifyFull;
    }
    options.retry = http::RetryPolicy::NONE;
    options.request_timeout = Some(REQUEST_TIMEOUT);
    Ok(options)
}

/// What a session of a profile can do: read with cursors, count (an exact count reads the whole
/// table), sort by the sort key of a Query, edit with conditional writes, run the console.
pub fn capabilities(region: &str) -> SessionCaps {
    let mut caps = SessionCaps::minimal(Driver::Dynamodb).with(&[
        Feature::Editor,
        Feature::EditRows,
        Feature::InsertRows,
        Feature::DeleteRows,
        Feature::ExactCount,
        Feature::ViewDefinition,
    ]);
    caps.namespaces = NamespaceCaps {
        databases: false,
        schemas: SchemaMode::Single,
        default_schema: Some(region.to_string()),
        labels: NamespaceLabels {
            database: "Region".into(),
            schema: "Region".into(),
            table: "Table".into(),
            tables: "Tables".into(),
        },
    };
    caps.data = DataCaps {
        paging: Paging::Cursor,
        count: CountMode::Costly,
        sort: true,
        filter_ops: FILTER_OPS.to_vec(),
        raw_filter: None,
        // A commit's transactions are all or nothing up to 100 items; more go in several.
        transactions: TxMode::Batch,
        // Every write's condition proves it changed the one item it names.
        affected: Affected::Exact,
        default_keyword: false,
        upsert: false,
        server_exprs: Vec::new(),
        max_params: 0,
    };
    caps
}

impl DynamoSession {
    /// A session over `client` (a region's or an emulator's endpoint).
    pub fn new(
        client: Client,
        params: DynamoParams,
        region: &str,
        custom_endpoint: bool,
        read_only: bool,
    ) -> DynamoSession {
        let trace = SqlTrace::default();
        DynamoSession {
            client: client.with_trace(trace.clone()),
            params,
            region: region.to_string(),
            custom_endpoint,
            read_only,
            caps: capabilities(region),
            trace,
            console: tokio::sync::Mutex::new(()),
            loaded: std::sync::Mutex::new(HashMap::new()),
            attributes: std::sync::Mutex::new(HashMap::new()),
            closed: CancellationToken::new(),
            ping_lists: AtomicBool::new(false),
        }
    }

    /// Opens the session of `target`'s profile and proves the endpoint and the credentials with
    /// a ping.
    pub async fn open(target: &ConnectTarget<'_>) -> Result<DynamoSession, AppError> {
        let profile = target.profile;
        let params = DynamoParams::from_driver_params(profile.options.driver_params.as_ref())?;
        let region = check_region(profile.database.as_deref().unwrap_or_default())?.to_string();
        let source = auth::source(profile, &params, target.password).await?;
        let endpoint = endpoint(&params, &region, source.endpoint.as_deref())?;
        let options = http_options(profile, endpoint.form_tls)?;
        let http = HttpClient::for_target(target, &endpoint.url, options, Credential::None)?;
        let client = Client::new(
            Arc::new(HttpTransport::new(http)),
            &endpoint.url,
            &region,
            source.provider,
        )?
        .with_command(source.command);
        let read_only = crate::conn_options::effective_read_only(profile);
        let session = DynamoSession::new(client, params, &region, endpoint.custom, read_only);
        session.ping().await?;
        Ok(session)
    }

    /// A token for one operation, cancelled by its slot's cancel or when the session closes.
    fn arm<'a>(
        &self,
        slot: &'a CancelSlot,
    ) -> Result<(CancellationToken, ArmedCancel<'a>), AppError> {
        let token = self.closed.child_token();
        let armed = arm_cancel(
            slot,
            CancelHandle::Custom(Arc::new(TokenCancel(token.clone()))),
        )?;
        Ok((token, armed))
    }

    fn grid(&self) -> GridOptions {
        GridOptions {
            consistent: self.params.consistent_reads,
            consumed_capacity: self.params.consumed_capacity,
            page_requests: self.params.page_requests,
            max_bytes: browse::Budget::DEFAULT.bytes,
        }
    }

    /// Remembers the items a page of `table` showed, for the edits (`keys`: the table's; none
    /// for an index's items, which may lack attributes).
    fn remember_items(&self, table: &str, keys: &b::KeySchema, items: &[Item]) {
        if items.is_empty() {
            return;
        }
        let mut loaded = self.loaded.lock().unwrap_or_else(|p| p.into_inner());
        let entry = loaded.entry(table.to_string()).or_default();
        for item in items {
            entry.record(keys, item);
        }
        b::trim_loaded(&mut loaded, table, LOADED_BYTES);
    }

    /// Remembers attribute names of `table` for the console's completions.
    fn remember_names<'a>(&self, table: &str, names: impl IntoIterator<Item = &'a String>) {
        let mut attributes = self.attributes.lock().unwrap_or_else(|p| p.into_inner());
        let known = attributes.entry(table.to_string()).or_default();
        // Hashed: a page of a sparse table can name a million attributes.
        let mut seen: HashSet<String> = known.iter().cloned().collect();
        for name in names {
            if known.len() >= catalog::MAX_COLUMNS {
                break;
            }
            if !seen.contains(name) {
                seen.insert(name.clone());
                known.push(name.clone());
            }
        }
    }

    /// Forgets the loaded items of the changes a commit applied: their values no longer guard
    /// them, and the next page loads them again.
    fn forget<'a>(&self, meta: &TableMeta, changes: impl Iterator<Item = &'a StagedChange>) {
        let Ok(keys) = catalog::key_schema_of(meta) else {
            return;
        };
        let mut loaded = self.loaded.lock().unwrap_or_else(|p| p.into_inner());
        let Some(items) = loaded.get_mut(&meta.table.name) else {
            return;
        };
        for change in changes {
            let key = match change {
                StagedChange::Update { key, .. } | StagedChange::Delete { key, .. } => key,
                StagedChange::Insert { .. } => continue,
            };
            if let Ok(key) = b::item_key(key, &keys) {
                items.forget(&key);
            }
        }
    }

    /// `tableInfo`: what the table info panel shows (the description, TTL and point-in-time
    /// recovery), each part `null` where the server has none (an emulator).
    async fn table_info(&self, table: &str, cancel: &CancellationToken) -> Result<J, AppError> {
        let described = self
            .client
            .call("DescribeTable", &b::describe_table(table), Some(cancel))
            .await?;
        let optional = |r: Result<J, CallError>| match r {
            Ok(reply) => Ok(reply),
            Err(CallError::Service(e)) if !e.is_throttling() && e.status < 500 => Ok(J::Null),
            Err(e) => Err(e.into_app_error()),
        };
        let ttl = optional(
            self.client
                .call_detailed(
                    "DescribeTimeToLive",
                    &b::describe_time_to_live(table),
                    Some(cancel),
                )
                .await,
        )?;
        let backups = optional(
            self.client
                .call_detailed(
                    "DescribeContinuousBackups",
                    &b::describe_continuous_backups(table),
                    Some(cancel),
                )
                .await,
        )?;
        Ok(json!({
            "table": described.get("Table").cloned().unwrap_or(J::Null),
            "timeToLive": ttl.get("TimeToLiveDescription").cloned().unwrap_or(J::Null),
            "continuousBackups": backups
                .get("ContinuousBackupsDescription")
                .cloned()
                .unwrap_or(J::Null),
        }))
    }

    /// `browseIndex`: a page of a secondary index of `table`, read like the table's grid (the
    /// filter's key conditions on the index's keys). Its items are not remembered for edits: an
    /// index may not project every attribute.
    async fn browse_index(
        &self,
        args: &J,
        cancel: &CancellationToken,
    ) -> Result<BrowsePage, AppError> {
        let table = args.get("table").and_then(J::as_str).unwrap_or_default();
        let index = args.get("index").and_then(J::as_str).unwrap_or_default();
        let req: BrowseRequest =
            serde_json::from_value(args.get("request").cloned().unwrap_or(J::Null))
                .map_err(|e| AppError::InvalidInput(format!("browseIndex: {e}")))?;
        let desc = browse::describe(&self.client, table, Some(cancel)).await?;
        let (found, global) = desc.index(index).ok_or_else(|| {
            AppError::NotFound(format!("{table} has no index \u{201c}{index}\u{201d}"))
        })?;
        let meta = catalog::table_meta(&desc, &[], &self.region, None);
        let read = IndexRead {
            name: index,
            keys: &found.keys,
            global,
        };
        let (page, items) = browse::browse(
            &self.client,
            &meta,
            &desc.keys,
            Some(read),
            &req,
            &self.grid(),
            Some(cancel),
        )
        .await?;
        self.remember_names(table, items.iter().flat_map(|i| i.keys()));
        Ok(page)
    }
}

/// The error of a change that was not applied.
fn payload(outcome: &Outcome) -> Option<ErrorPayload> {
    match outcome {
        Outcome::Applied => None,
        Outcome::Failed(e) => Some(e.to_payload()),
        Outcome::NotApplied(why) => Some(AppError::db(why.clone()).to_payload()),
    }
}

#[async_trait]
impl TableSource for DynamoSession {
    async fn browse(
        &self,
        meta: &TableMeta,
        req: &BrowseRequest,
        cancel: CancelSlot,
    ) -> Result<BrowsePage, AppError> {
        let keys = catalog::key_schema_of(meta)?;
        let (token, _armed) = self.arm(&cancel)?;
        let (page, items) = browse::browse(
            &self.client,
            meta,
            &keys,
            None,
            req,
            &self.grid(),
            Some(&token),
        )
        .await?;
        self.remember_items(&meta.table.name, &keys, &items);
        self.remember_names(&meta.table.name, items.iter().flat_map(|i| i.keys()));
        Ok(page)
    }

    async fn count(
        &self,
        meta: &TableMeta,
        filter: Option<&FilterGroup>,
        exact: bool,
        cancel: CancelSlot,
    ) -> Result<RowCount, AppError> {
        let keys = catalog::key_schema_of(meta)?;
        let (token, _armed) = self.arm(&cancel)?;
        browse::count(
            &self.client,
            meta,
            &keys,
            filter,
            exact,
            self.params.consistent_reads,
            Some(&token),
        )
        .await
    }

    async fn fetch_cell(
        &self,
        meta: &TableMeta,
        key: &RowKey,
        column: &str,
    ) -> Result<Value, AppError> {
        let keys = catalog::key_schema_of(meta)?;
        let token = self.closed.child_token();
        browse::fetch_cell(
            &self.client,
            &meta.table.name,
            &keys,
            key,
            column,
            self.params.consistent_reads,
            Some(&token),
        )
        .await
    }
}

#[async_trait]
impl NativeSession for DynamoSession {
    fn driver(&self) -> Driver {
        Driver::Dynamodb
    }

    fn capabilities(&self) -> SessionCaps {
        self.caps.clone()
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        let creds = self.client.credentials().get(self.client.now()).await?;
        let version = if self.custom_endpoint {
            format!("DynamoDB API 2012-08-10 at {}", self.client.endpoint())
        } else {
            "DynamoDB API 2012-08-10".to_string()
        };
        Ok(ServerInfo {
            version,
            current_database: Some(self.region.clone()),
            current_user: Some(creds.access_key_id),
        })
    }

    /// `DescribeEndpoints` costs nothing; a server without it is asked for one table name.
    async fn ping(&self) -> Result<(), AppError> {
        let token = self.closed.child_token();
        if !self.ping_lists.load(Ordering::Relaxed) {
            match self
                .client
                .call_detailed("DescribeEndpoints", &b::describe_endpoints(), Some(&token))
                .await
            {
                Ok(_) => return Ok(()),
                Err(CallError::Service(e))
                    if e.status < 500 && !e.is_throttling() && !e.is_auth() =>
                {
                    self.ping_lists.store(true, Ordering::Relaxed);
                }
                Err(e) => return Err(e.into_app_error()),
            }
        }
        self.client
            .call("ListTables", &b::list_tables(None, 1), Some(&token))
            .await
            .map(|_| ())
    }

    async fn close(&self) {
        self.closed.cancel();
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        cancel_portable(handle).await.unwrap_or(Ok(()))
    }

    fn sql_trace(&self) -> Option<&SqlTrace> {
        Some(&self.trace)
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        Ok(catalog::databases(&self.region))
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        Ok(catalog::schemas(&self.region))
    }

    async fn list_tables(&self, _schema: &str) -> Result<Vec<TableSummary>, AppError> {
        let token = self.closed.child_token();
        let names = browse::list_tables(&self.client, Some(&token)).await?;
        Ok(names
            .iter()
            .map(|n| catalog::summary(n, &self.region))
            .collect())
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let token = self.closed.child_token();
        let (meta, _) = browse::table_meta(
            &self.client,
            &table.name,
            self.params.sample_size,
            Some(&token),
        )
        .await?;
        self.remember_names(&meta.table.name, meta.columns.iter().map(|c| &c.name));
        Ok(meta)
    }

    /// The table as the `CreateTable` request that makes one like it.
    async fn definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let token = self.closed.child_token();
        let desc = browse::describe(&self.client, &table.name, Some(&token)).await?;
        Ok(Some(catalog::definition(&desc)))
    }

    /// Every table, with the attributes this session has seen of it (no table is read for it).
    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let token = self.closed.child_token();
        let names = browse::list_tables(&self.client, Some(&token)).await?;
        let attributes = self.attributes.lock().unwrap_or_else(|p| p.into_inner());
        let tables: BTreeMap<String, Vec<String>> = names
            .into_iter()
            .take(MAX_COMPLETION_TABLES)
            .map(|n| {
                let known = attributes.get(&n).cloned().unwrap_or_default();
                (n, known)
            })
            .collect();
        Ok(CompletionMetadata {
            default_schema: Some(self.region.clone()),
            schemas: BTreeMap::from([(self.region.clone(), tables)]),
        })
    }

    fn plan_changes(&self, meta: &TableMeta, req: &ApplyRequest) -> Result<NativePlan, AppError> {
        if self.read_only {
            return Err(read_only_error());
        }
        let keys = catalog::key_schema_of(meta)?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let loaded = self.loaded.lock().unwrap_or_else(|p| p.into_inner());
        let none = LoadedItems::default();
        let items = loaded.get(&meta.table.name).unwrap_or(&none);
        let plan = b::plan_changes(
            &meta.table.name,
            &keys,
            &meta.columns,
            items,
            &req.changes,
            &token,
        )?;
        Ok(NativePlan {
            preview: plan.preview(),
            ops: Box::new(plan),
        })
    }

    /// One change is one request (or a key's delete and put in one transaction); more are sent
    /// as transactions of at most 100 items, each all or nothing.
    async fn apply_changes(
        &self,
        meta: &TableMeta,
        req: &ApplyRequest,
        plan: NativePlan,
    ) -> ApplyResult {
        let started = Instant::now();
        let sql: Vec<String> = plan.preview.iter().map(|(_, t)| t.clone()).collect();
        let outcomes: Vec<(usize, Outcome)> = if self.read_only {
            (0..req.changes.len())
                .map(|i| (i, Outcome::Failed(read_only_error())))
                .collect()
        } else {
            match plan.ops.downcast::<EditPlan>() {
                Err(_) => (0..req.changes.len())
                    .map(|i| {
                        let e = AppError::internal("the plan is not a DynamoDB plan");
                        (i, Outcome::Failed(e))
                    })
                    .collect(),
                Ok(edit) => {
                    let token = self.closed.child_token();
                    let out = if edit.ops.len() <= 1 {
                        browse::apply_each(&self.client, &edit, Some(&token)).await
                    } else {
                        match browse::apply_atomic(&self.client, &edit, Some(&token)).await {
                            Ok(out) => out,
                            Err(e) => (0..req.changes.len())
                                .map(|i| (i, Outcome::Failed(e.clone())))
                                .collect(),
                        }
                    };
                    let applied = out
                        .iter()
                        .filter(|(_, o)| matches!(o, Outcome::Applied))
                        .filter_map(|(i, _)| req.changes.get(*i));
                    self.forget(meta, applied);
                    out
                }
            }
        };
        let results: Vec<RowResult> = req
            .changes
            .iter()
            .enumerate()
            .map(|(i, change)| {
                let outcome = outcomes
                    .iter()
                    .find(|(c, _)| *c == i)
                    .map(|(_, o)| o.clone())
                    .unwrap_or_else(|| Outcome::NotApplied("not applied".into()));
                RowResult {
                    row_id: change.row_id().to_string(),
                    op: change.op_name().to_string(),
                    affected: u64::from(matches!(outcome, Outcome::Applied)),
                    returned: None,
                    error: payload(&outcome),
                }
            })
            .collect();
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
        b::split_console(text)
    }

    fn classify_console(&self, command: &str) -> StatementClass {
        b::classify_console(command)
    }

    async fn run_console(
        &self,
        command: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        let cmd = b::parse_command(command)?;
        let class = match &cmd {
            ConsoleCommand::Partiql(statement) => b::partiql_class(statement),
            ConsoleCommand::Api { action, body } => b::api_class(action, body),
        };
        if self.read_only && class == StatementClass::Write {
            return Err(read_only_error());
        }
        let (token, _armed) = self.arm(&cancel)?;
        let out = browse::run_command(&self.client, &cmd, max_rows, Some(&token)).await?;
        let row_count = out.rows.len() as u64;
        let kind = match (out.write && out.columns.is_empty(), out.affected) {
            (false, _) => StatementKind::Rows,
            (true, Some(_)) => StatementKind::Affected,
            // A DELETE of a key that may not exist: it succeeded, how much it deleted is unknown
            // ("0 rows affected" could be as wrong as "1").
            (true, None) => StatementKind::Empty,
        };
        if kind == StatementKind::Rows {
            sink.begin_set(out.columns);
            // One chunk: nothing follows it for a sink that stops early to skip.
            let _ = sink.rows(out.rows);
        }
        Ok(StatementOutcome {
            kind,
            result_sets: u32::from(kind == StatementKind::Rows),
            row_count,
            truncated: out.truncated,
            affected: out.affected,
            last_insert_id: None,
            notices: out.notices,
        })
    }

    fn console_lock(&self) -> Option<&tokio::sync::Mutex<()>> {
        Some(&self.console)
    }

    async fn call(&self, method: &str, args: J) -> Result<J, AppError> {
        let token = self.closed.child_token();
        match method {
            "tableInfo" => {
                let table = args.get("table").and_then(J::as_str).unwrap_or_default();
                self.table_info(table, &token).await
            }
            "browseIndex" => Ok(serde_json::to_value(
                self.browse_index(&args, &token).await?,
            )?),
            _ => Err(unknown_call(Driver::Dynamodb, method)),
        }
    }

    fn call_kind(&self, method: &str) -> CallKind {
        if CALLS.contains(&method) {
            CallKind::Read
        } else {
            CallKind::Write
        }
    }
}
