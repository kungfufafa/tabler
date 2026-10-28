//! What the drivers of key-value, document and search stores have in common with each other
//! rather than with SQL sessions: a [`NativeSession`] browses, edits and runs console commands in
//! the store's own terms, and shared code reaches it only through `drivers::Session`.

use async_trait::async_trait;

use crate::drivers::{CancelHandle, CancelSlot, RowSink, ServerInfo, SqlTrace, StatementOutcome};
use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::split::SplitStatement;
use crate::types::browse::{BrowsePage, BrowseRequest, FilterGroup, RowCount, RowKey, TracedSql};
use crate::types::caps::SessionCaps;
use crate::types::mutate::{ApplyRequest, ApplyResult};
use crate::types::profile::Driver;
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

/// Whether a driver call only reads (allowed on a read-only connection) or may change something.
/// A driver answers `Write` for any call it does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    Read,
    Write,
}

/// The error for a driver call (`call`, `util`) the driver does not know.
pub fn unknown_call(driver: Driver, method: &str) -> AppError {
    AppError::Unsupported(format!(
        "{}: unknown call \u{201c}{method}\u{201d}",
        driver.label()
    ))
}

/// Reads a table's rows without SQL: every native store, and the SQL engines whose rows are
/// better paged by their own API (BigQuery `tabledata.list`, Cassandra paging state).
#[async_trait]
pub trait TableSource: Send + Sync {
    async fn browse(
        &self,
        meta: &TableMeta,
        req: &BrowseRequest,
        cancel: CancelSlot,
    ) -> Result<BrowsePage, AppError>;

    async fn count(
        &self,
        meta: &TableMeta,
        filter: Option<&FilterGroup>,
        exact: bool,
        cancel: CancelSlot,
    ) -> Result<RowCount, AppError>;

    async fn fetch_cell(
        &self,
        meta: &TableMeta,
        key: &RowKey,
        column: &str,
    ) -> Result<Value, AppError>;
}

/// Staged changes planned once: the preview the user confirms and the operations that apply
/// them come from the same plan, so what is applied is what was shown. `preview` pairs the index
/// of a change with its text; `ops` is the driver's own, read back by its `apply_changes`.
pub struct NativePlan {
    pub preview: Vec<(usize, String)>,
    pub ops: Box<dyn std::any::Any + Send + Sync>,
}

/// A live connection to a store that speaks no SQL (Redis, MongoDB, Elasticsearch, DynamoDB).
/// SQL-only commands refuse it (`Session::sql`); the rest reach it through `Session`.
#[async_trait]
pub trait NativeSession: TableSource {
    fn driver(&self) -> Driver;
    /// What the session can do; the backend refuses what they lack before anything is sent.
    fn capabilities(&self) -> SessionCaps;

    async fn server_info(&self) -> Result<ServerInfo, AppError>;
    /// Must be free: no billed job and no rate-limited call (the app pings on its own).
    async fn ping(&self) -> Result<(), AppError>;
    /// Finishes within about 2 s (callers give up at 5 s).
    async fn close(&self);
    /// Aborts what a handle was taken from (`request_cancel`). Never fails because nothing was
    /// running.
    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError>;

    /// Whether the console has an open transaction (`None` when the store has none or it is
    /// unknown).
    fn in_transaction(&self) -> Option<bool> {
        None
    }

    /// Takes what the session sent for the app since the last call, oldest first (see
    /// `SqlTrace`): a session that records into `sql_trace` needs nothing more.
    fn drain_trace(&self) -> Vec<TracedSql> {
        self.sql_trace().map(SqlTrace::drain).unwrap_or_default()
    }

    /// The trace this session records into (`None`: it keeps none).
    fn sql_trace(&self) -> Option<&SqlTrace> {
        None
    }

    // --- introspection: the store's namespaces and collections in the sidebar's terms ---
    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError>;
    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError>;
    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError>;
    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError>;
    /// What defines a collection beyond its columns (a mapping, a validator, a key schema).
    async fn definition(&self, _table: &TableRef) -> Result<Option<String>, AppError> {
        Ok(None)
    }
    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        Ok(CompletionMetadata::default())
    }

    // --- edits ---
    /// Plans staged changes; the preview text comes from `sqlgen/native/<store>.rs`.
    fn plan_changes(&self, meta: &TableMeta, req: &ApplyRequest) -> Result<NativePlan, AppError>;
    /// Applies a plan from `plan_changes`, reporting each change's outcome in its `RowResult`.
    async fn apply_changes(
        &self,
        meta: &TableMeta,
        req: &ApplyRequest,
        plan: NativePlan,
    ) -> ApplyResult;

    // --- console ---
    fn split_console(&self, text: &str) -> Vec<SplitStatement>;
    /// Whether a console command only reads; anything unknown is a `Write`.
    fn classify_console(&self, command: &str) -> StatementClass;
    async fn run_console(
        &self,
        command: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError>;
    /// Lock a whole console run holds, so the commands of concurrent runs never interleave.
    fn console_lock(&self) -> Option<&tokio::sync::Mutex<()>> {
        None
    }

    // --- the driver's own calls (`session_call`) ---
    async fn call(
        &self,
        method: &str,
        _args: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        Err(unknown_call(self.driver(), method))
    }
    /// Whether `call(method)` only reads; anything unknown changes something.
    fn call_kind(&self, _method: &str) -> CallKind {
        CallKind::Write
    }
}
