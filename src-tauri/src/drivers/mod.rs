//! Database drivers behind one trait. Each driver owns its connections; commands only see
//! `Arc<dyn DbSession>`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::types::profile::{ConnectionProfile, Driver};
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};
use crate::types::value::Value;

pub mod mysql;
pub mod postgres;
pub mod sqlite;

#[derive(Debug, Clone, Default)]
pub struct ServerInfo {
    pub version: String,
    pub current_database: Option<String>,
    pub current_user: Option<String>,
}

/// A live connection to one database server (pool + dedicated editor session).
#[async_trait]
pub trait DbSession: Send + Sync {
    fn driver(&self) -> Driver;
    fn dialect(&self) -> Dialect;

    async fn server_info(&self) -> Result<ServerInfo, AppError>;
    async fn ping(&self) -> Result<(), AppError>;
    async fn close(&self);

    // --- introspection (worker pool) ---
    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError>;
    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError>;
    async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError>;
    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError>;
    async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError>;
    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError>;

    // --- generic execution (worker pool) ---
    /// Runs a statement that returns rows. Postgres: columns should be cast to text by the caller
    /// (sqlgen does this); the driver returns `Value::Text`/`Null` plus the described type name so
    /// the caller can decode. MySQL/SQLite return typed values.
    async fn query(&self, sql: &str, params: &[Value]) -> Result<ResultSet, AppError>;
    /// Runs a statement for its side effects (optionally with RETURNING rows).
    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecOutcome, AppError>;
    /// Runs several statements inside one transaction; rolls back on the first error or when a
    /// statement's `expect_affected` is not met.
    async fn transaction(&self, statements: Vec<TxStatement>)
    -> Result<Vec<ExecOutcome>, AppError>;
}

/// One statement of a transaction. `expect_affected` makes the driver roll back when the
/// statement touches a different number of rows (guards UPDATE/DELETE by key).
#[derive(Debug, Clone)]
pub struct TxStatement {
    pub sql: String,
    pub params: Vec<Value>,
    pub expect_affected: Option<u64>,
}

impl TxStatement {
    pub fn new(sql: impl Into<String>, params: Vec<Value>) -> TxStatement {
        TxStatement {
            sql: sql.into(),
            params,
            expect_affected: None,
        }
    }

    pub fn expecting(mut self, n: u64) -> TxStatement {
        self.expect_affected = Some(n);
        self
    }
}

/// Shared check used by every driver inside `transaction`.
pub fn check_affected(
    st: &TxStatement,
    outcome: &ExecOutcome,
    index: usize,
) -> Result<(), AppError> {
    match st.expect_affected {
        Some(n) if outcome.affected != n => Err(AppError::db(format!(
            "expected to affect {n} row(s) but matched {}; the row may have been changed or deleted by someone else",
            outcome.affected
        ))
        .with_statement_index(index)),
        _ => Ok(()),
    }
}

/// Everything needed to open a session. `host_override` points at a local SSH tunnel listener.
pub struct ConnectTarget<'a> {
    pub profile: &'a ConnectionProfile,
    pub password: Option<&'a str>,
    pub host_override: Option<(String, u16)>,
}

impl ConnectTarget<'_> {
    pub fn host(&self) -> String {
        match &self.host_override {
            Some((h, _)) => h.clone(),
            None => self
                .profile
                .host
                .clone()
                .unwrap_or_else(|| "127.0.0.1".to_string()),
        }
    }

    pub fn port(&self) -> u16 {
        match &self.host_override {
            Some((_, p)) => *p,
            None => self
                .profile
                .port
                .or_else(|| self.profile.driver.default_port())
                .unwrap_or(0),
        }
    }

    /// Host name used for TLS verification / SNI even when connecting through a tunnel.
    pub fn tls_host(&self) -> String {
        self.profile
            .host
            .clone()
            .unwrap_or_else(|| "localhost".to_string())
    }
}

pub async fn connect(target: ConnectTarget<'_>) -> Result<Arc<dyn DbSession>, AppError> {
    match target.profile.driver {
        Driver::Sqlite => Ok(Arc::new(sqlite::LiteSession::open(target.profile).await?)),
        Driver::Postgres => postgres::connect(target).await,
        Driver::Mysql => mysql::connect(target).await,
    }
}
