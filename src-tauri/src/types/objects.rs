//! Sidebar database objects besides tables (M8): routines, sequences, the Item Overview grid, and
//! the object operations of the sidebar menus (drop / truncate / clone / comment). SQL text for
//! all of them is generated in `sqlgen::objects`; introspection and execution live in `objects`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::types::schema::{TableKind, TableRef};

/// What a routine is. PostgreSQL has all four (`pg_proc.prokind`); MySQL only functions and
/// procedures; SQLite has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RoutineKind {
    Function,
    Procedure,
    Aggregate,
    Window,
}

/// One function / procedure of a schema, as the sidebar's Functions section lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RoutineSummary {
    pub schema: Option<String>,
    pub name: String,
    pub kind: RoutineKind,
    /// Argument list as declared (`a integer, b text DEFAULT 'x'`); empty when there are none.
    pub arguments: String,
    /// What tells overloads apart in `DROP FUNCTION name(…)` (PG
    /// `pg_get_function_identity_arguments`); empty on MySQL, where names are unique.
    pub identity_arguments: String,
    /// Functions only (`integer`, `TABLE(id integer)`, `trigger`…).
    pub return_type: Option<String>,
    pub language: Option<String>,
    pub comment: Option<String>,
}

/// Addresses one routine (overload) for its definition or a DROP.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RoutineRef {
    pub schema: Option<String>,
    pub name: String,
    pub kind: RoutineKind,
    /// `RoutineSummary::identity_arguments` of the overload (PG); ignored on MySQL.
    pub identity_arguments: String,
}

/// A PostgreSQL sequence of a schema (sidebar "Sequences" section).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SequenceSummary {
    pub schema: Option<String>,
    pub name: String,
    /// `bigint`, `integer` or `smallint`.
    pub data_type: Option<String>,
    /// `table.column` of a serial / identity column the sequence belongs to.
    pub owned_by: Option<String>,
    pub comment: Option<String>,
}

/// One row of the Item Overview grid (spec §11.2). Sizes are bytes; what a server cannot tell
/// is `None` (views have no size, SQLite has no engine or owner).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OverviewItem {
    pub name: String,
    pub schema: Option<String>,
    pub kind: TableKind,
    pub estimated_rows: Option<i64>,
    pub total_size: Option<i64>,
    pub data_size: Option<i64>,
    pub index_size: Option<i64>,
    /// MySQL.
    pub engine: Option<String>,
    /// MySQL.
    pub charset: Option<String>,
    /// MySQL.
    pub collation: Option<String>,
    /// PostgreSQL.
    pub owner: Option<String>,
    pub comment: Option<String>,
}

/// A table-like object an operation applies to; the kind picks `DROP VIEW` over `DROP TABLE`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ObjectTarget {
    pub table: TableRef,
    pub kind: TableKind,
}

/// A sidebar operation on database objects. Each one becomes a `DdlPlan` (`preview_object_op`)
/// and runs through `apply_object_op`. Multi-object ops emit one statement per object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export, rename_all_fields = "camelCase")]
pub enum ObjectOp {
    /// `DROP TABLE / VIEW / MATERIALIZED VIEW / FOREIGN TABLE`. `cascade`: PostgreSQL only;
    /// `ignore_foreign_keys`: MySQL only (`SET FOREIGN_KEY_CHECKS = 0` around the drops, in the
    /// same session).
    DropTables {
        tables: Vec<ObjectTarget>,
        cascade: bool,
        ignore_foreign_keys: bool,
    },
    /// `TRUNCATE TABLE` (SQLite: `DELETE FROM`). `cascade` / `restart_identity`: PostgreSQL;
    /// `ignore_foreign_keys`: MySQL.
    TruncateTables {
        tables: Vec<TableRef>,
        cascade: bool,
        restart_identity: bool,
        ignore_foreign_keys: bool,
    },
    /// Copies a table's structure (and rows) into a new table of the same schema.
    CloneTable {
        source: TableRef,
        new_name: String,
        with_data: bool,
    },
    /// `DROP FUNCTION / PROCEDURE / AGGREGATE`; `cascade` is PostgreSQL only.
    DropRoutines {
        routines: Vec<RoutineRef>,
        cascade: bool,
    },
    /// `DROP SEQUENCE` (PostgreSQL).
    DropSequences {
        sequences: Vec<TableRef>,
        cascade: bool,
    },
    /// `COMMENT ON …` (PostgreSQL) / `ALTER TABLE … COMMENT` (MySQL). `None` or empty removes it.
    SetComment {
        target: ObjectTarget,
        comment: Option<String>,
    },
}
