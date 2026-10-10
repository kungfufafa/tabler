use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::schema::{ColumnMeta, TableRef};
use super::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FilterOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
    Like,
    NotLike,
    In,
    NotIn,
    IsNull,
    IsNotNull,
    Between,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Conjunction {
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum FilterNode {
    Condition {
        column: String,
        op: FilterOp,
        values: Vec<Value>,
    },
    Group(FilterGroup),
    /// Raw SQL appended verbatim inside parentheses (the user's own WHERE fragment).
    Raw {
        sql: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FilterGroup {
    pub conjunction: Conjunction,
    pub items: Vec<FilterNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NullsOrder {
    First,
    Last,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SortSpec {
    pub column: String,
    pub dir: SortDir,
    pub nulls: Option<NullsOrder>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BrowseRequest {
    pub table: TableRef,
    /// Subset of columns to fetch; `None` = all.
    pub columns: Option<Vec<String>>,
    pub filter: Option<FilterGroup>,
    #[serde(default)]
    pub sort: Vec<SortSpec>,
    pub limit: u32,
    pub offset: u64,
}

/// How rows of a table can be addressed for updates/deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RowIdentity {
    /// Primary key columns.
    Pk,
    /// SQLite implicit rowid (table without PK); `row_ids` carries it.
    Rowid,
    /// No usable key: edits must match every original column value.
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BrowsePage {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<Value>>,
    pub pk_columns: Vec<String>,
    pub identity: RowIdentity,
    /// Parallel to `rows` when `identity == Rowid`.
    pub row_ids: Option<Vec<i64>>,
    pub offset: u64,
    pub limit: u32,
    /// The SELECT that was executed (placeholders inlined for display).
    pub sql: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RowCount {
    pub count: i64,
    pub exact: bool,
}

/// Identifies one existing row for UPDATE/DELETE/fetch_cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum RowKey {
    Pk(BTreeMap<String, Value>),
    Rowid(i64),
    AllColumns(BTreeMap<String, Value>),
}
