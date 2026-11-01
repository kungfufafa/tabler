//! The seam between the session and the scylla driver: a `Wire` runs one request (unpaged, or
//! one page from a paging state) and a `Connector` opens a wire from a `Dial`. The session's
//! logic (read-only, paging, edits, the editor's cqlsh commands) is tested over the fakes of
//! `testing.rs`; `client.rs` is the real one.

use std::sync::Arc;
use std::time::Duration;

use ::scylla::frame::types::{Consistency, SerialConsistency};
use ::scylla::response::{PagingState, PagingStateResponse};
use async_trait::async_trait;
use uuid::Uuid;

use super::catalog::CellRow;
use super::params::CassandraCompression;
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::types::profile::SslMode;
use crate::types::query::ResultColumn;
use crate::types::value::Value;

/// What one request asks for of the server's paging.
#[derive(Debug, Clone)]
pub enum Page {
    /// Every row in one response (introspection, edits, counts).
    Unpaged,
    /// One page of `size` rows from `state`.
    Single { state: PagingState, size: i32 },
}

/// How a request runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOptions {
    pub consistency: Consistency,
    pub serial: SerialConsistency,
    pub timeout: Duration,
    pub tracing: bool,
}

/// One statement to send. Values are bound through a prepared statement, which gives their
/// CQL types; a statement without values is sent as it is.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub cql: &'a str,
    pub params: &'a [Value],
    pub page: Page,
    pub options: RunOptions,
}

/// What the server answered.
#[derive(Debug, Clone)]
pub struct Reply {
    /// Whether the result has rows (a SELECT, a lightweight transaction's `[applied]`).
    pub is_rows: bool,
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<CellRow>,
    pub paging: PagingStateResponse,
    pub tracing_id: Option<Uuid>,
    pub warnings: Vec<String>,
}

impl Reply {
    /// A result without rows (a write, DDL).
    pub fn done() -> Reply {
        Reply {
            is_rows: false,
            columns: Vec::new(),
            rows: Vec::new(),
            paging: PagingStateResponse::NoMorePages,
            tracing_id: None,
            warnings: Vec::new(),
        }
    }
}

#[async_trait]
pub trait Wire: Send + Sync {
    async fn run(&self, req: Request<'_>) -> Result<Reply, AppError>;
    /// `USE`: the session's connections switch keyspace (a quoted name keeps its case).
    async fn use_keyspace(&self, keyspace: &str, case_sensitive: bool) -> Result<(), AppError>;
    /// Waits until every node has the same schema (after DDL, before the sidebar reads it).
    async fn await_schema_agreement(&self) -> Result<(), AppError>;
    /// The trace of a traced request, as lines.
    async fn tracing(&self, id: Uuid) -> Result<Vec<String>, AppError>;
}

/// The password of a connection: the profile's, or a Command's output (run again when it
/// expires, for every new connection the driver opens).
#[derive(Clone)]
pub enum Password {
    Fixed(String),
    Command(Arc<CommandPassword>),
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Password::Fixed(_) => f.write_str("Password::Fixed(..)"),
            Password::Command(_) => f.write_str("Password::Command(..)"),
        }
    }
}

/// PasswordAuthenticator's credentials.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub user: String,
    pub password: Password,
}

/// TLS of the connections, from the profile: the mode, the CA and client certificate files, and
/// the name the server's certificate is checked for (the profile's host, also through a tunnel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tls {
    pub mode: SslMode,
    pub ca_path: Option<String>,
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
    pub server_name: String,
}

/// Everything a session is opened with.
#[derive(Debug, Clone)]
pub struct Dial {
    /// The contact point: the profile's host, or the SSH tunnel's local end.
    pub host: String,
    pub port: u16,
    /// Only the contact point is used (forced through a tunnel).
    pub single_node: bool,
    pub tls: Option<Tls>,
    pub credentials: Option<Credentials>,
    /// The keyspace the session starts in, as the profile names it.
    pub keyspace: Option<String>,
    pub connect_timeout: Duration,
    pub options: RunOptions,
    pub local_datacenter: Option<String>,
    pub compression: CassandraCompression,
    pub keepalive: Duration,
}

#[async_trait]
pub trait Connector: Send + Sync {
    async fn connect(&self, dial: &Dial) -> Result<Arc<dyn Wire>, AppError>;
}

/// A keyspace name as `USE` takes it: a quoted one keeps its case (`"MyKs"`, `""` doubled), a
/// bare one is folded to lower case, as the server does.
pub fn keyspace_name(name: &str) -> (String, bool) {
    let trimmed = name.trim();
    match trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        Some(inner) => (inner.replace("\"\"", "\""), true),
        None => (trimmed.to_lowercase(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyspace_names_fold_unless_quoted() {
        assert_eq!(keyspace_name(" Shop "), ("shop".to_string(), false));
        assert_eq!(keyspace_name("\"MyKs\""), ("MyKs".to_string(), true));
        assert_eq!(keyspace_name("\"a\"\"b\""), ("a\"b".to_string(), true));
    }
}
