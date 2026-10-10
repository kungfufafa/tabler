use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::browse::RowKey;
use super::schema::TableRef;
use super::value::Value;
use crate::error::ErrorPayload;

/// One staged edit from the grid. `row_id` is the renderer's own key, echoed back in results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "camelCase")]
#[ts(export)]
pub enum StagedChange {
    Update {
        row_id: String,
        key: RowKey,
        changes: BTreeMap<String, Value>,
    },
    Insert {
        row_id: String,
        values: BTreeMap<String, Value>,
    },
    Delete {
        row_id: String,
        key: RowKey,
    },
}

impl StagedChange {
    pub fn row_id(&self) -> &str {
        match self {
            StagedChange::Update { row_id, .. }
            | StagedChange::Insert { row_id, .. }
            | StagedChange::Delete { row_id, .. } => row_id,
        }
    }

    pub fn op_name(&self) -> &'static str {
        match self {
            StagedChange::Update { .. } => "update",
            StagedChange::Insert { .. } => "insert",
            StagedChange::Delete { .. } => "delete",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApplyRequest {
    pub table: TableRef,
    pub changes: Vec<StagedChange>,
    /// Ask for the stored row back (PostgreSQL/SQLite `RETURNING`).
    #[serde(default)]
    pub returning: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RowResult {
    pub row_id: String,
    pub op: String,
    pub affected: u64,
    pub returned: Option<BTreeMap<String, Value>>,
    pub error: Option<ErrorPayload>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApplyResult {
    pub success: bool,
    pub results: Vec<RowResult>,
    /// Statements that were (or would have been) executed, with literals inlined.
    pub sql: Vec<String>,
    pub error: Option<ErrorPayload>,
    pub duration_ms: u64,
}
