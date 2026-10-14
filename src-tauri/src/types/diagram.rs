//! The ER diagram (Generate Diagram…): every table of a schema with its columns, and the foreign
//! keys between them, loaded in one catalog round trip.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::schema::{TableKind, TableRef};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiagramColumn {
    pub name: String,
    /// The type as the database spells it (`character varying(255)`, `int unsigned`…).
    pub data_type: String,
    pub is_pk: bool,
    pub nullable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiagramTable {
    #[serde(rename = "ref")]
    pub table: TableRef,
    pub kind: TableKind,
    /// In ordinal order.
    pub columns: Vec<DiagramColumn>,
    /// The server's statistics (PG `reltuples`, MySQL `TABLE_ROWS`); None on SQLite.
    pub row_estimate: Option<i64>,
}

/// A foreign key: `from` (the child, many side) references `to` (the parent, one side).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiagramRelation {
    /// The constraint name (SQLite: `<table>_fk<id>`, its keys are unnamed).
    pub name: String,
    pub from: TableRef,
    pub from_columns: Vec<String>,
    pub to: TableRef,
    pub to_columns: Vec<String>,
    /// `CASCADE`, `SET NULL`, `SET DEFAULT`, `RESTRICT` or `NO ACTION`.
    pub on_delete: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiagramSchema {
    pub schema: String,
    /// Sorted by name.
    pub tables: Vec<DiagramTable>,
    /// Foreign keys declared by the schema's tables, sorted by (table, name). `to` may lie in
    /// another schema (PostgreSQL / MySQL).
    pub relations: Vec<DiagramRelation>,
}
