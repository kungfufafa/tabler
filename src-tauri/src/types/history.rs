//! Query history types: one row per statement run from the SQL editor.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryEntry {
    pub id: i64,
    pub profile_id: Option<Uuid>,
    pub connection_name: String,
    pub database: Option<String>,
    pub sql: String,
    /// RFC 3339 timestamp.
    pub started_at: String,
    pub duration_ms: Option<u64>,
    pub row_count: Option<u64>,
    pub affected: Option<u64>,
    pub success: bool,
    pub error: Option<String>,
    /// Where the statement came from: `editor` or `explain`.
    pub source: String,
}

/// A history row before it is stored (no id yet).
#[derive(Debug, Clone, PartialEq)]
pub struct NewHistoryEntry {
    pub profile_id: Option<Uuid>,
    pub connection_name: String,
    pub database: Option<String>,
    pub sql: String,
    pub started_at: String,
    pub duration_ms: Option<u64>,
    pub row_count: Option<u64>,
    pub affected: Option<u64>,
    pub success: bool,
    pub error: Option<String>,
    pub source: String,
}

/// Newest-first listing filter. `before_id` pages backwards (pass the last id you received).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct HistoryFilter {
    pub profile_id: Option<Uuid>,
    /// Case-insensitive substring match on the SQL text.
    pub search: Option<String>,
    /// Defaults to 200, capped at 1000.
    pub limit: Option<u32>,
    pub before_id: Option<i64>,
}
