use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A table/view reference. `schema` is the PG schema, the MySQL database, or the SQLite attached
/// database name (`main`); `None` means "the connection's default".
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
}

impl TableRef {
    pub fn new(schema: Option<&str>, name: &str) -> Self {
        Self {
            schema: schema.map(str::to_string),
            name: name.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
    ForeignTable,
    Partitioned,
    SystemView,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DatabaseInfo {
    pub name: String,
    pub owner: Option<String>,
    pub encoding: Option<String>,
    pub is_current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SchemaInfo {
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TableSummary {
    pub name: String,
    pub schema: Option<String>,
    pub kind: TableKind,
    pub row_estimate: Option<i64>,
    pub size_bytes: Option<i64>,
    pub comment: Option<String>,
}

/// Coarse classification of a column type; drives cell rendering/editing in the renderer and the
/// text → `Value` decoders in the drivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ValueKind {
    Int,
    BigInt,
    Decimal,
    Float,
    Bool,
    Text,
    Bytes,
    Date,
    Time,
    Timestamp,
    TimestampTz,
    Interval,
    Uuid,
    Json,
    Array,
    Enum,
    Set,
    Bit,
    Geometry,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ColumnMeta {
    pub name: String,
    pub ordinal: u32,
    /// Full type as the database spells it, e.g. `character varying(255)`, `bigint unsigned`.
    pub data_type: String,
    pub kind: ValueKind,
    pub nullable: bool,
    pub default_expr: Option<String>,
    pub is_pk: bool,
    pub pk_ordinal: Option<u32>,
    pub is_auto_increment: bool,
    pub is_generated: bool,
    pub is_unsigned: bool,
    pub comment: Option<String>,
    pub enum_values: Option<Vec<String>>,
    pub char_max_len: Option<i64>,
    pub numeric_precision: Option<i32>,
    pub numeric_scale: Option<i32>,
    pub collation: Option<String>,
    /// For arrays: the element type name.
    pub array_elem_type: Option<String>,
    /// Driver-specific extra info (MySQL `EXTRA`, PG identity kind…).
    pub extra: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IndexMeta {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
    pub method: Option<String>,
    pub predicate: Option<String>,
    pub definition: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ForeignKeyMeta {
    pub name: String,
    pub columns: Vec<String>,
    pub ref_table: TableRef,
    pub ref_columns: Vec<String>,
    pub on_update: String,
    pub on_delete: String,
    pub definition: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TriggerMeta {
    pub name: String,
    pub timing: Option<String>,
    pub events: Vec<String>,
    pub enabled: bool,
    pub definition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TableMeta {
    pub table: TableRef,
    pub kind: TableKind,
    pub columns: Vec<ColumnMeta>,
    /// Primary key column names in key order (empty when the table has no PK).
    pub primary_key: Vec<String>,
    pub indexes: Vec<IndexMeta>,
    pub foreign_keys: Vec<ForeignKeyMeta>,
    pub triggers: Vec<TriggerMeta>,
    pub comment: Option<String>,
    pub row_estimate: Option<i64>,
    /// `CREATE TABLE` text when the engine can provide it cheaply (MySQL, SQLite).
    pub ddl: Option<String>,
}

/// Compact schema → table → columns map for SQL editor autocompletion.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CompletionMetadata {
    pub default_schema: Option<String>,
    pub schemas: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}
