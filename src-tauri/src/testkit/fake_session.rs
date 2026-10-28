//! A SQL session that reaches no server: it reports the caps a test gives it, answers like an
//! empty database, and records every call that would reach a server in its `SqlTrace`, so a test
//! can prove that a refused command sent nothing.

use async_trait::async_trait;

use crate::drivers::{
    CancelHandle, CancelSlot, DbSession, RowSink, ServerInfo, SqlTrace, StatementOutcome,
    TxStatement,
};
use crate::error::AppError;
use crate::sqlgen::SqlDialect;
use crate::types::browse::{TraceCategory, TracedSql};
use crate::types::caps::SessionCaps;
use crate::types::profile::Driver;
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub struct FakeSession {
    caps: SessionCaps,
    dialect: SqlDialect,
    trace: SqlTrace,
    /// Statements holding this text fail (`failing`).
    fail_on: Option<String>,
}

impl FakeSession {
    /// A session of `caps.driver` speaking `caps.dialect` (a SQL dialect) with these caps.
    pub fn new(caps: SessionCaps) -> FakeSession {
        let dialect = caps.dialect.expect("a fake SQL session has a dialect");
        FakeSession {
            caps,
            dialect,
            trace: SqlTrace::default(),
            fail_on: None,
        }
    }

    /// The same session, on which a statement holding `text` fails (after it is recorded).
    pub fn failing(mut self, text: &str) -> FakeSession {
        self.fail_on = Some(text.to_string());
        self
    }

    fn check(&self, sql: &str) -> Result<(), AppError> {
        match &self.fail_on {
            Some(text) if sql.contains(text.as_str()) => {
                Err(AppError::db(format!("fake failure of {sql}")))
            }
            _ => Ok(()),
        }
    }

    /// What reached the "server", oldest first (the trace is not drained).
    pub fn calls(&self) -> Vec<String> {
        let calls = self.trace.drain();
        for call in &calls {
            self.trace.record(call.clone());
        }
        calls.into_iter().map(|c| c.sql).collect()
    }

    fn touch(&self, what: impl Into<String>) {
        self.trace.record(TracedSql {
            sql: what.into(),
            category: TraceCategory::Data,
            duration_ms: 0,
            rows: None,
            affected: None,
            error: None,
        });
    }
}

#[async_trait]
impl DbSession for FakeSession {
    fn driver(&self) -> Driver {
        self.caps.driver
    }

    fn dialect(&self) -> SqlDialect {
        self.dialect
    }

    fn capabilities(&self) -> SessionCaps {
        self.caps.clone()
    }

    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        self.touch("server_info");
        Ok(ServerInfo::default())
    }

    async fn ping(&self) -> Result<(), AppError> {
        self.touch("ping");
        Ok(())
    }

    async fn close(&self) {}

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        self.touch("list_databases");
        Ok(Vec::new())
    }

    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        self.touch("list_schemas");
        Ok(Vec::new())
    }

    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        self.touch(format!("list_tables {schema}"));
        Ok(Vec::new())
    }

    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        self.touch(format!("table_meta {}", table.name));
        Err(AppError::NotFound(table.name.clone()))
    }

    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        self.touch(format!("view_definition {}", table.name));
        Ok(None)
    }

    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        self.touch("completion_metadata");
        Ok(CompletionMetadata::default())
    }

    async fn query(&self, sql: &str, _params: &[Value]) -> Result<ResultSet, AppError> {
        self.touch(sql);
        self.check(sql)?;
        Ok(ResultSet::default())
    }

    async fn execute(&self, sql: &str, _params: &[Value]) -> Result<ExecOutcome, AppError> {
        self.touch(sql);
        self.check(sql)?;
        Ok(ExecOutcome::default())
    }

    /// Records every statement; one that fails rolls back the whole batch.
    async fn transaction(
        &self,
        statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        for (i, st) in statements.iter().enumerate() {
            self.touch(st.sql.clone());
            self.check(&st.sql).map_err(|e| e.with_statement_index(i))?;
        }
        Ok(vec![ExecOutcome::default(); statements.len()])
    }

    async fn run_statement(
        &self,
        sql: &str,
        _max_rows: usize,
        _sink: &mut dyn RowSink,
        _cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        self.touch(sql);
        Ok(StatementOutcome::default())
    }

    async fn stream_query(
        &self,
        sql: &str,
        _params: &[Value],
        _sink: &mut dyn RowSink,
        _cancel: CancelSlot,
    ) -> Result<u64, AppError> {
        self.touch(sql);
        Ok(0)
    }

    async fn cancel(&self, _handle: CancelHandle) -> Result<(), AppError> {
        self.touch("cancel");
        Ok(())
    }

    fn in_transaction(&self) -> Option<bool> {
        Some(false)
    }

    fn sql_trace(&self) -> Option<&SqlTrace> {
        Some(&self.trace)
    }
}
