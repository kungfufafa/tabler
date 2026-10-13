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
    /// PG: for a column whose type is a domain (or an array of one), the type the domain is
    /// over, resolved through nested domains and without its modifier (`character varying`,
    /// `numeric[]`); `None` otherwise and on the other drivers. See `ColumnMeta::cast_type`.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub base_type: Option<String>,
}

impl ColumnMeta {
    /// The type a PostgreSQL bind parameter for this column is cast to (after `pg_cast_type`):
    /// a domain column's base type, so the domain's modifier is applied when the value is
    /// stored (an error when it does not fit) instead of cutting the value in the explicit cast.
    pub fn cast_type(&self) -> &str {
        self.base_type.as_deref().unwrap_or(&self.data_type)
    }
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
    /// MySQL `INDEX_COMMENT`, PG `COMMENT ON INDEX`; `None` on SQLite.
    #[serde(default)]
    pub comment: Option<String>,
    /// PG `INCLUDE (…)` columns (non-key columns stored in the index); `columns` holds the key
    /// columns only. Empty elsewhere. (Optional in TS so renderer literals may leave it out;
    /// the backend always sends it.)
    #[serde(default)]
    #[ts(as = "Option<Vec<String>>", optional)]
    pub include: Vec<String>,
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

/// A CHECK constraint (table- or column-level).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CheckMeta {
    /// Constraint name. SQLite does not report names of unnamed checks; they are addressed as
    /// `ck_<table>_<n>` (n counts the table's unnamed checks from 1).
    pub name: String,
    /// The boolean expression, without the `CHECK (…)` around it.
    pub expression: String,
    /// Columns the expression uses (PG: from the catalog; MySQL / SQLite: column names found in
    /// the expression; a SQLite column-level check names its column).
    pub columns: Vec<String>,
}

/// MySQL table options. Every field is optional: as introspected, `None` means unknown; as the
/// payload of `DdlOp::AlterTableOptions`, `None` means "leave unchanged".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TableOptions {
    pub engine: Option<String>,
    pub charset: Option<String>,
    pub collation: Option<String>,
    /// Next `AUTO_INCREMENT` value.
    pub auto_increment: Option<u64>,
    pub row_format: Option<String>,
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
    /// CHECK constraints.
    #[serde(default)]
    pub checks: Vec<CheckMeta>,
    /// MySQL engine / charset / collation / AUTO_INCREMENT / row format; `None` elsewhere.
    #[serde(default)]
    pub options: Option<TableOptions>,
}

/// A user-defined type a column can use (PG enums, domains, ranges, composite types), spelled as
/// a column type (`mood`, `"other schema".money_amount`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserTypeInfo {
    pub name: String,
    /// `enum`, `domain`, `range` or `composite`.
    pub kind: String,
}

/// A MySQL character set with its collations (default first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CharsetInfo {
    pub name: String,
    pub default_collation: Option<String>,
    pub collations: Vec<String>,
}

/// What the structure editor's pickers offer for a connection: user-defined column types (PG),
/// storage engines and character sets / collations (MySQL). Empty lists where not applicable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StructureChoices {
    pub user_types: Vec<UserTypeInfo>,
    pub engines: Vec<String>,
    pub charsets: Vec<CharsetInfo>,
}

/// Compact schema → table → columns map for SQL editor autocompletion.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CompletionMetadata {
    pub default_schema: Option<String>,
    pub schemas: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cast_type_prefers_the_domain_base_type() {
        let mut col: ColumnMeta = serde_json::from_value(serde_json::json!({
            "name": "code", "ordinal": 1, "dataType": "short_code", "kind": "text",
            "nullable": true, "isPk": false, "isAutoIncrement": false, "isGenerated": false,
            "isUnsigned": false
        }))
        .unwrap();
        assert_eq!(col.base_type, None);
        assert_eq!(col.cast_type(), "short_code");
        col.base_type = Some("character varying".into());
        assert_eq!(col.cast_type(), "character varying");
    }
}
