//! The D1 session: every call one request to Cloudflare's REST API, with no connection and no
//! transaction across calls. A staged commit is one batch, which D1 runs as one transaction.
//! Nothing on the server can refuse a write for a session, so a read-only session refuses here
//! every statement the classifier does not know is a read, before anything is sent.

use std::time::Instant;

use async_trait::async_trait;

use crate::drivers::sqlite_http::catalog::{self, CatalogQuery, MAIN, Remote, Rows};
use crate::drivers::sqlite_http::session::{
    Closed, cancellable, cancellable_by, refuse_write, trace_transaction, traced, traced_catalog,
};
use crate::drivers::sqlite_http::value::inserts;
use crate::drivers::{
    CancelHandle, CancelSlot, DbSession, RowChunker, RowSink, ServerInfo, SqlTrace,
    StatementOutcome, TraceCounts, TxStatement, cancel_portable, is_dml,
};
use crate::error::AppError;
use crate::sqlgen::SqlDialect;
use crate::types::browse::TracedSql;
use crate::types::caps::{CountMode, Feature, SessionCaps};
use crate::types::profile::Driver;
use crate::types::query::{ExecOutcome, ResultSet, StatementKind};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::security::SessionDetails;
use crate::types::value::Value;

use super::api::RawResult;
use super::client::D1Client;
use super::decode::result_set;

/// What a D1 session offers beyond `SessionCaps::base` (the structure view and the editor), each
/// proven by the lane's tests against the fake API. No `EditWithoutKey`: the batch commits before
/// a count could be checked by the client, so rows are addressed by primary key only. No
/// `Returning`: the rows a `RETURNING` sends back are not narrowed (D1's JavaScript numbers).
pub const FEATURES: [Feature; 13] = [
    Feature::Explain,
    Feature::Format,
    Feature::EditRows,
    Feature::InsertRows,
    Feature::DeleteRows,
    Feature::ServerExpressions,
    Feature::ExactCount,
    Feature::CopyAsInsert,
    Feature::TableDdl,
    Feature::ViewDefinition,
    Feature::SwitchDatabase,
    Feature::SessionDetails,
    Feature::Mcp,
];

/// The session's caps: SQLite's grid and namespaces (100 parameters, from the descriptor),
/// `FEATURES`, the account's databases to switch between, read-only enforced by the classifier
/// alone, and counts only on request (D1 bills the rows a count reads, and the API's requests are
/// rate limited).
pub fn capabilities() -> SessionCaps {
    let mut caps = SessionCaps::base(Driver::D1, SqlDialect::Sqlite, None).with(&FEATURES);
    caps.namespaces.databases = true;
    caps.data.count = CountMode::Costly;
    caps.safety.read_only = Driver::D1.descriptor().read_only;
    caps
}

pub struct D1Session {
    client: D1Client,
    /// The database's name (the profile may name it by UUID).
    name: String,
    /// `sqlite_version()` at connect.
    version: String,
    read_only: bool,
    editor_runs: tokio::sync::Mutex<()>,
    trace: SqlTrace,
    closed: Closed,
}

impl D1Session {
    /// Reads the database's name and SQLite version, so a refused token or a missing database
    /// fails the connect.
    pub async fn connect(client: D1Client, read_only: bool) -> Result<D1Session, AppError> {
        let info = client.database().await?;
        let mut session = D1Session {
            client,
            name: info.name,
            version: String::new(),
            read_only,
            editor_runs: tokio::sync::Mutex::new(()),
            trace: SqlTrace::default(),
            closed: Closed::default(),
        };
        let rows = session.catalog(&[catalog::version_query()]).await?;
        session.version = catalog::parse_version(catalog::first_set(&rows))?;
        Ok(session)
    }

    async fn catalog(&self, queries: &[CatalogQuery]) -> Result<Vec<Rows>, AppError> {
        self.closed.check()?;
        traced_catalog(&self.trace, queries, self.client.catalog(queries)).await
    }

    /// A query of the app's; a cancelled one is logged too.
    async fn worker_query(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<&CancelSlot>,
    ) -> Result<ResultSet, AppError> {
        self.closed.check()?;
        refuse_write(self.read_only, sql)?;
        traced(
            &self.trace,
            sql,
            params,
            |rs: &ResultSet| TraceCounts::rows(rs.rows.len()),
            cancellable_by(cancel, self.client.query(sql, params, None, None)),
        )
        .await
    }

    async fn worker_execute(
        &self,
        sql: &str,
        params: &[Value],
        cancel: Option<&CancelSlot>,
    ) -> Result<ExecOutcome, AppError> {
        self.closed.check()?;
        refuse_write(self.read_only, sql)?;
        traced(
            &self.trace,
            sql,
            params,
            TraceCounts::of_exec,
            cancellable_by(cancel, self.client.execute(sql, params)),
        )
        .await
    }

    /// One statement's rows into `sink`, at most `max_rows` of them: D1 sends the whole result
    /// at once.
    async fn statement_rows(
        &self,
        sql: &str,
        params: &[Value],
        max_rows: Option<usize>,
        sink: &mut dyn RowSink,
    ) -> Result<(StatementOutcome, RawResult), AppError> {
        let raw = self.client.statement(sql, params).await?;
        let mut chunker = RowChunker::new(sink);
        let mut outcome = StatementOutcome::default();
        if let Some(rows) = raw.results.as_ref().filter(|r| !r.columns.is_empty()) {
            let set = result_set(rows, None, max_rows);
            outcome.kind = StatementKind::Rows;
            outcome.result_sets = 1;
            outcome.truncated = set.truncated;
            let _ = chunker.begin_set(set.columns);
            for row in set.rows {
                if chunker.push(row).is_break() {
                    outcome.truncated = true;
                    break;
                }
            }
        }
        let _ = chunker.flush();
        outcome.row_count = chunker.total_rows;
        Ok((outcome, raw))
    }
}

#[async_trait]
impl DbSession for D1Session {
    fn driver(&self) -> Driver {
        Driver::D1
    }

    fn dialect(&self) -> SqlDialect {
        SqlDialect::Sqlite
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        self.closed.check()?;
        Ok(ServerInfo {
            version: format!("Cloudflare D1 (SQLite {})", self.version),
            current_database: Some(self.name.clone()),
            current_user: None,
        })
    }

    /// Nothing to check: there is no connection, and every call to the API counts against the
    /// account's rate limit (a refused token shows on the next real call).
    async fn ping(&self) -> Result<(), AppError> {
        self.closed.check()
    }

    async fn close(&self) {
        self.closed.close();
    }

    /// The account's databases, for the switcher.
    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        self.closed.check()?;
        let current = &self.client.target().database_id;
        Ok(self
            .client
            .list_databases(None)
            .await?
            .into_iter()
            .map(|d| DatabaseInfo {
                is_current: &d.uuid == current,
                name: d.name,
                owner: None,
                encoding: None,
            })
            .collect())
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        self.closed.check()?;
        Ok(catalog::schemas())
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        if schema != MAIN {
            return Ok(Vec::new());
        }
        let rows = self
            .catalog(&[catalog::list_tables_query(Remote::D1)])
            .await?;
        catalog::parse_tables(catalog::first_set(&rows))
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        let queries = catalog::table_meta_queries(table);
        let rows = self.catalog(&queries).await?;
        catalog::parse_table_meta(table, &rows)
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        let rows = self
            .catalog(&[catalog::view_definition_query(table)])
            .await?;
        catalog::parse_view_definition(catalog::first_set(&rows))
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        let rows = self
            .catalog(&[catalog::completion_query(Remote::D1)])
            .await?;
        catalog::parse_completion(catalog::first_set(&rows))
    }

    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError> {
        self.worker_query(sql, params, None).await
    }

    async fn query_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ResultSet, AppError> {
        self.worker_query(sql, params, Some(&cancel)).await
    }

    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError> {
        self.worker_execute(sql, params, None).await
    }

    async fn execute_cancellable(
        &self,
        sql: &str,
        params: &[Value],
        cancel: CancelSlot,
    ) -> Result<ExecOutcome, AppError> {
        self.worker_execute(sql, params, Some(&cancel)).await
    }

    /// One batch with a `changes()` guard after each statement that expects a count: D1 rolls
    /// the whole batch back when a statement or a guard fails.
    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        self.closed.check()?;
        for (i, st) in statements.iter().enumerate() {
            refuse_write(self.read_only, &st.sql).map_err(|e| e.with_statement_index(i))?;
        }
        if statements.is_empty() {
            return Ok(Vec::new());
        }
        let started = Instant::now();
        let result = self.client.transaction(&statements).await;
        trace_transaction(&self.trace, &statements, started, &result);
        result
    }

    async fn run_statement(
        &self,
        sql: &str,
        max_rows: usize,
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        self.closed.check()?;
        refuse_write(self.read_only, sql)?;
        let (mut outcome, raw) =
            cancellable(&cancel, self.statement_rows(sql, &[], Some(max_rows), sink)).await?;
        // `meta.changes` is the statement's `total_changes()` delta, so after DDL it is 0.
        let changes = raw.changes();
        if changes > 0 || is_dml(sql) {
            if outcome.kind != StatementKind::Rows {
                outcome.kind = StatementKind::Affected;
            }
            outcome.affected = Some(changes);
        }
        if changes > 0 && inserts(sql) {
            outcome.last_insert_id = raw
                .meta
                .as_ref()
                .and_then(|m| m.last_row_id)
                .map(|id| id.to_string());
        }
        Ok(outcome)
    }

    async fn stream_query(
        &self,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<u64, AppError> {
        self.closed.check()?;
        refuse_write(self.read_only, sql)?;
        traced(
            &self.trace,
            sql,
            params,
            |n: &u64| TraceCounts::rows(*n as usize),
            cancellable(&cancel, async {
                let (outcome, _) = self.statement_rows(sql, params, None, sink).await?;
                Ok::<_, AppError>(outcome.row_count)
            }),
        )
        .await
    }

    async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        cancel_portable(handle).await.unwrap_or(Ok(()))
    }

    /// Every request is its own transaction.
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
        capabilities()
    }

    /// The `CREATE` statements SQLite keeps for the table and its indexes and triggers.
    async fn table_ddl(&self, table: &TableRef) -> Option<Result<String, AppError>> {
        let queries = catalog::table_ddl_queries(table);
        Some(
            self.catalog(&queries)
                .await
                .and_then(|rows| catalog::parse_table_ddl(table, &rows)),
        )
    }

    /// Whether the API is reached over HTTPS (Cloudflare's always is); the text is UTF-8.
    async fn session_details(&self) -> Option<Result<SessionDetails, AppError>> {
        Some(self.closed.check().map(|()| SessionDetails {
            tls: self.client.target().api_base.starts_with("https://"),
            tls_version: None,
            tls_cipher: None,
            encoding: Some("UTF-8".into()),
        }))
    }
}
