//! Query history types (M4). Owned by the SQL editor agent.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryEntry {
    pub id: i64,
    pub profile_id: Option<String>,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct HistoryFilter {
    pub profile_id: Option<String>,
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub before_id: Option<i64>,
}
