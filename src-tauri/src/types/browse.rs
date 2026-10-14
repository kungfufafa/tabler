use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::schema::{ColumnMeta, TableRef};
use super::value::Value;

/// A filter-bar operator (TablePlus's list). The `…Case` variants are the driver's other case
/// behaviour: case-insensitive on PostgreSQL and SQLite ("– Case insensitive"), case-sensitive on
/// MySQL ("– Case sensitive"). Contains / prefix / suffix match their value literally (`%` and `_`
/// are escaped); `Like` / `NotLike` / `Ilike` take the user's own pattern.
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
    NotBetween,
    Contains,
    NotContains,
    ContainsCase,
    NotContainsCase,
    HasPrefix,
    HasSuffix,
    HasPrefixCase,
    HasSuffixCase,
    /// PostgreSQL only.
    Ilike,
    /// PostgreSQL `json` / `jsonb` columns only: `col::jsonb @> value`.
    JsonContains,
    JsonNotContains,
}

impl FilterOp {
    /// Operators that match rows NOT satisfying their positive form; "Any column" joins them with
    /// AND (no column matches) instead of OR (some column matches).
    pub fn is_negated(self) -> bool {
        matches!(
            self,
            FilterOp::Ne
                | FilterOp::NotLike
                | FilterOp::NotIn
                | FilterOp::NotBetween
                | FilterOp::NotContains
                | FilterOp::NotContainsCase
                | FilterOp::JsonNotContains
        )
    }
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
    /// The filter bar's "Any column": `op` applied to every column's textual form, expanded by the
    /// backend from the table's columns (OR; AND for negated operators).
    AnyColumn {
        op: FilterOp,
        values: Vec<Value>,
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

/// What a statement the drivers ran for the app (`SqlTrace`) was for: reading or writing rows, or
/// reading / changing the schema (the console log's "Data queries" / "Meta queries").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TraceCategory {
    Data,
    Structure,
}

/// One statement a session really sent to the server, as the console log shows it. Bound
/// parameters are inlined for display (`WHERE id = 5`); the text is capped, never reworded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TracedSql {
    pub sql: String,
    pub category: TraceCategory,
    pub duration_ms: u64,
    /// Rows the statement returned.
    pub rows: Option<u64>,
    /// Rows a write changed.
    pub affected: Option<u64>,
    /// The server's (or driver's) message when the statement failed.
    pub error: Option<String>,
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
