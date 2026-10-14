//! Metrics Board (TablePlus's dashboards): boards kept in the app database, the object queries
//! they run, and their `.tablerboard` files.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::types::query::ResultColumn;
use crate::types::value::Value;

/// A board: a named canvas of objects (charts, scoreboards, tables, inputs…). The objects are the
/// renderer's document; the backend stores them as they come.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MetricsBoard {
    pub id: Uuid,
    /// The connection profile it belongs to; `None` = shown for every connection.
    pub profile_id: Option<Uuid>,
    pub name: String,
    /// The board's objects (a JSON array).
    #[ts(type = "unknown")]
    pub objects: serde_json::Value,
    /// RFC 3339 timestamps.
    pub created_at: String,
    pub updated_at: String,
}

/// Creates (`id` = `None`, or an id that does not exist yet) or replaces a board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MetricsBoardInput {
    pub id: Option<Uuid>,
    pub profile_id: Option<Uuid>,
    pub name: String,
    #[ts(type = "unknown")]
    pub objects: serde_json::Value,
}

/// One board object's query. The SQL arrives cut at its `{{variables}}` (found by the renderer,
/// outside strings and comments): `parts[0] {{v0}} parts[1] {{v1}} … parts[n]`, with the value
/// of each occurrence in `values`. The backend puts the dialect's bind parameters there (inline
/// literals on PostgreSQL, see `metrics::bind_template`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MetricsQuery {
    pub parts: Vec<String>,
    pub values: Vec<Value>,
    /// The object's "Allow data changes": without it a statement that writes is refused.
    pub allow_writes: bool,
    /// Rows kept (at most `metrics::MAX_ROWS`).
    pub max_rows: u32,
    /// The object's "Query timeout" in seconds: a statement still running after it, a write too,
    /// is cancelled (`metrics::query_timeout`: absent = 60 s, kept between 5 and 600).
    #[serde(default)]
    #[ts(optional)]
    pub timeout_secs: Option<u32>,
}

/// What a board object's query returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MetricsResult {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Value>>,
    /// More rows than `max_rows` came back; the rest were not read.
    pub truncated: bool,
    /// Rows changed (a statement that writes).
    pub affected: Option<u64>,
    pub duration_ms: u64,
    /// The statement as it was sent (placeholders or literals in place of the variables).
    pub sql: String,
}

/// Reading a `.tablerboard` file: it is encrypted and needs its password, or these boards were
/// added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MetricsImport {
    pub needs_password: bool,
    pub boards: Vec<MetricsBoard>,
}
