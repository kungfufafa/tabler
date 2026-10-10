use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::schema::ValueKind;
use super::value::Value;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResultColumn {
    pub name: String,
    /// Type name as reported by the server for this result column.
    pub data_type: String,
    pub kind: ValueKind,
    pub nullable: Option<bool>,
    /// Source table when the server reports it (used to detect editable results).
    pub table: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResultSet {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Value>>,
    pub truncated: bool,
}

/// Outcome of a statement run through `DbSession::execute`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExecOutcome {
    pub affected: u64,
    pub last_insert_id: Option<String>,
    /// Rows produced by `RETURNING` (or a SELECT run through execute).
    pub rows: Option<ResultSet>,
}
