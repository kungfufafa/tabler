//! DuckDB catalog queries (the `duckdb_*()` table functions of the current database) and the
//! parsers of their rows.
//!
//! Every query is limited to `current_database()` and leaves the internal objects out
//! (`information_schema`, `pg_catalog`, the `system` and `temp` databases). Names travel as bind
//! parameters (`$1` the schema, `$2` the table), so a dot or a quote in a name stays part of it.
//! Lists arrive as JSON text (`decode` writes nested values as JSON), which the parsers read
//! with serde. The parsers read rows as the decoders produce them, so they are tested without
//! an engine as well as against one.
//!
//! DuckDB's own pg_catalog is a partial emulation; nothing here reads it.

use std::collections::BTreeMap;

use super::types::{array_element, kind_of_type};
use crate::error::AppError;
use crate::sqlgen::dialect::duckdb as duckdb_sql;
use crate::types::objects::{RoutineKind, RoutineSummary, SequenceSummary};
use crate::types::schema::{
    CheckMeta, ColumnMeta, CompletionMetadata, DatabaseInfo, ForeignKeyMeta, IndexMeta, SchemaInfo,
    TableKind, TableMeta, TableRef, TableSummary, UserTypeInfo, ValueKind,
};
use crate::types::value::Value;

// --- Reading cells -------------------------------------------------------------------------

fn cell(row: &[Value], i: usize) -> Option<&Value> {
    row.get(i).filter(|v| !v.is_null())
}

/// Text of a cell; None for NULL or a missing column.
pub fn text(row: &[Value], i: usize) -> Option<String> {
    cell(row, i).and_then(Value::as_text)
}

fn non_empty(row: &[Value], i: usize) -> Option<String> {
    text(row, i).filter(|s| !s.trim().is_empty())
}

/// An integer cell.
pub fn int(row: &[Value], i: usize) -> Option<i64> {
    match cell(row, i)? {
        Value::Int(n) => Some(*n),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::BigInt(s) | Value::Decimal(s) | Value::Text(s) => s.trim().parse().ok(),
        Value::Float(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Some(*f as i64),
        _ => None,
    }
}

/// A boolean cell; NULL is false.
pub fn flag(row: &[Value], i: usize) -> bool {
    match cell(row, i) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Text(s)) => matches!(s.to_ascii_lowercase().as_str(), "true" | "yes" | "1"),
        _ => int(row, i).is_some_and(|n| n != 0),
    }
}

/// A list cell (`VARCHAR[]`), from the JSON text the decoder writes; empty for NULL.
pub fn list(row: &[Value], i: usize) -> Vec<String> {
    let Some(raw) = text(row, i) else {
        return Vec::new();
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::Array(items)) => items
            .into_iter()
            .filter_map(|v| match v {
                serde_json::Value::String(s) => Some(s),
                serde_json::Value::Null => None,
                other => Some(other.to_string()),
            })
            .collect(),
        _ => Vec::new(),
    }
}

// --- Server and databases ------------------------------------------------------------------

/// The engine's version and where the session is.
pub const SERVER_INFO: &str = "SELECT library_version, source_id, current_database(), \
     current_schema() FROM pragma_version()";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DuckdbServerInfo {
    /// `v1.5.6`
    pub version: String,
    pub source_id: Option<String>,
    pub database: Option<String>,
    pub schema: Option<String>,
}

impl DuckdbServerInfo {
    /// `DuckDB v1.5.6 (abc123)`, as the status pill shows it.
    pub fn label(&self) -> String {
        match &self.source_id {
            Some(id) => format!("DuckDB {} ({id})", self.version),
            None => format!("DuckDB {}", self.version),
        }
    }
}

pub fn parse_server_info(rows: &[Vec<Value>]) -> Result<DuckdbServerInfo, AppError> {
    let r = rows
        .first()
        .ok_or_else(|| AppError::internal("pragma_version() returned no row"))?;
    Ok(DuckdbServerInfo {
        version: text(r, 0).unwrap_or_default(),
        source_id: non_empty(r, 1),
        database: non_empty(r, 2),
        schema: non_empty(r, 3),
    })
}

/// The attached databases (catalogs) a session can switch to.
pub const DATABASES: &str = "SELECT database_name, path, type, readonly, comment, \
     database_name = current_database() FROM duckdb_databases() WHERE NOT internal \
     ORDER BY database_name";

/// An attached database as the catalog lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckdbDatabase {
    pub name: String,
    /// The file; None in memory.
    pub path: Option<String>,
    /// `duckdb`, `sqlite`, `postgres`…
    pub kind: Option<String>,
    pub read_only: bool,
    pub comment: Option<String>,
    pub is_current: bool,
}

impl DuckdbDatabase {
    pub fn info(&self) -> DatabaseInfo {
        DatabaseInfo {
            name: self.name.clone(),
            owner: None,
            encoding: None,
            is_current: self.is_current,
        }
    }
}

pub fn parse_databases(rows: &[Vec<Value>]) -> Vec<DuckdbDatabase> {
    rows.iter()
        .filter_map(|r| {
            Some(DuckdbDatabase {
                name: text(r, 0)?,
                path: non_empty(r, 1),
                kind: non_empty(r, 2),
                read_only: flag(r, 3),
                comment: non_empty(r, 4),
                is_current: flag(r, 5),
            })
        })
        .collect()
}

/// Schemas of the current database. `main` counts as internal in DuckDB's catalog, so the
/// emulated catalogs are left out by name instead.
pub const SCHEMAS: &str = "SELECT schema_name, schema_name = current_schema() \
     FROM duckdb_schemas() WHERE database_name = current_database() \
     AND schema_name NOT IN ('information_schema', 'pg_catalog') ORDER BY schema_name";

pub fn parse_schemas(rows: &[Vec<Value>]) -> Vec<SchemaInfo> {
    rows.iter()
        .filter_map(|r| {
            Some(SchemaInfo {
                name: text(r, 0)?,
                is_default: flag(r, 1),
            })
        })
        .collect()
}

// --- Tables and views ----------------------------------------------------------------------

/// Tables and views of schema `$1`: name, kind, row estimate, comment.
pub const TABLES: &str = "SELECT table_name, 'table', estimated_size, comment \
     FROM duckdb_tables() WHERE database_name = current_database() AND schema_name = $1 \
     AND NOT internal \
     UNION ALL SELECT view_name, 'view', NULL, comment FROM duckdb_views() \
     WHERE database_name = current_database() AND schema_name = $1 AND NOT internal \
     ORDER BY 1";

fn kind_of(s: Option<String>) -> TableKind {
    match s.as_deref() {
        Some("view") => TableKind::View,
        _ => TableKind::Table,
    }
}

pub fn parse_tables(rows: &[Vec<Value>], schema: &str) -> Vec<TableSummary> {
    rows.iter()
        .filter_map(|r| {
            Some(TableSummary {
                name: text(r, 0)?,
                schema: Some(schema.to_string()),
                kind: kind_of(text(r, 1)),
                row_estimate: int(r, 2),
                size_bytes: None,
                comment: non_empty(r, 3),
            })
        })
        .collect()
}

/// One table or view `$2` of schema `$1`: kind, row estimate, comment, its `CREATE` text.
pub const TABLE: &str = "SELECT 'table', estimated_size, comment, sql FROM duckdb_tables() \
     WHERE database_name = current_database() AND schema_name = $1 AND table_name = $2 \
     UNION ALL SELECT 'view', NULL, comment, sql FROM duckdb_views() \
     WHERE database_name = current_database() AND schema_name = $1 AND view_name = $2 \
     AND NOT internal";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    pub kind: TableKind,
    pub row_estimate: Option<i64>,
    pub comment: Option<String>,
    /// The `CREATE TABLE` / `CREATE VIEW` text DuckDB keeps.
    pub sql: Option<String>,
}

pub fn parse_table(rows: &[Vec<Value>], table: &TableRef) -> Result<TableInfo, AppError> {
    let r = rows.first().ok_or_else(|| {
        AppError::NotFound(format!(
            "table {}.{}",
            table.schema.as_deref().unwrap_or("main"),
            table.name
        ))
    })?;
    Ok(TableInfo {
        kind: kind_of(text(r, 0)),
        row_estimate: int(r, 1),
        comment: non_empty(r, 2),
        sql: non_empty(r, 3),
    })
}

/// Columns of `$2` in schema `$1`, in order.
pub const COLUMNS: &str = "SELECT column_name, column_index, data_type, is_nullable, \
     column_default, comment, character_maximum_length, numeric_precision, numeric_scale \
     FROM duckdb_columns() WHERE database_name = current_database() AND schema_name = $1 \
     AND table_name = $2 ORDER BY column_index";

/// The ENUM types of the database with their labels, for columns typed by a type's name.
pub const ENUM_TYPES: &str = "SELECT schema_name, type_name, labels FROM duckdb_types() \
     WHERE database_name = current_database() AND NOT internal AND logical_type = 'ENUM'";

/// User-defined types by name (`mood`) and by schema-qualified name, with their labels.
pub type EnumLabels = BTreeMap<String, Vec<String>>;

pub fn parse_enum_types(rows: &[Vec<Value>]) -> EnumLabels {
    let mut out = EnumLabels::new();
    for r in rows {
        let (Some(schema), Some(name)) = (text(r, 0), text(r, 1)) else {
            continue;
        };
        let labels = list(r, 2);
        out.insert(format!("{schema}.{name}"), labels.clone());
        out.entry(name).or_insert(labels);
    }
    out
}

/// The user types a column may use (the structure editor's picker).
pub fn user_types(enums: &EnumLabels) -> Vec<UserTypeInfo> {
    enums
        .keys()
        .filter(|k| !k.contains('.'))
        .map(|name| UserTypeInfo {
            name: name.clone(),
            kind: "enum".into(),
        })
        .collect()
}

/// The labels of an inline `ENUM('a', 'b')` type name.
pub fn enum_labels(data_type: &str) -> Option<Vec<String>> {
    let t = data_type.trim();
    let inner = t
        .strip_prefix("ENUM(")
        .or_else(|| t.strip_prefix("enum("))?
        .strip_suffix(')')?;
    let mut labels = Vec::new();
    let mut chars = inner.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        match chars.next() {
            None => break,
            Some('\'') => {
                let mut s = String::new();
                loop {
                    match chars.next() {
                        Some('\'') if chars.peek() == Some(&'\'') => {
                            chars.next();
                            s.push('\'');
                        }
                        Some('\'') => break,
                        Some(c) => s.push(c),
                        None => return None,
                    }
                }
                labels.push(s);
            }
            Some(_) => return None,
        }
    }
    Some(labels)
}

/// Whether a column default makes the column count up by itself (`nextval('seq')`).
fn auto_increments(default: Option<&str>) -> bool {
    default.is_some_and(|d| d.trim_start().to_ascii_lowercase().starts_with("nextval("))
}

pub fn parse_columns(rows: &[Vec<Value>], enums: &EnumLabels) -> Vec<ColumnMeta> {
    rows.iter()
        .filter_map(|r| {
            let name = text(r, 0)?;
            let data_type = text(r, 2).unwrap_or_default();
            let default_expr = non_empty(r, 4);
            let mut kind = kind_of_type(&data_type);
            let enum_values = enum_labels(&data_type).or_else(|| enums.get(&data_type).cloned());
            if enum_values.is_some() {
                kind = ValueKind::Enum;
            }
            Some(ColumnMeta {
                ordinal: int(r, 1).and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
                kind,
                nullable: flag(r, 3),
                is_pk: false,
                pk_ordinal: None,
                is_auto_increment: auto_increments(default_expr.as_deref()),
                is_generated: false,
                is_unsigned: data_type.to_ascii_uppercase().starts_with('U')
                    && matches!(kind, ValueKind::Int | ValueKind::BigInt),
                comment: non_empty(r, 5),
                enum_values,
                char_max_len: int(r, 6),
                numeric_precision: int(r, 7).and_then(|n| i32::try_from(n).ok()),
                numeric_scale: int(r, 8).and_then(|n| i32::try_from(n).ok()),
                collation: None,
                array_elem_type: array_element(&data_type),
                extra: None,
                base_type: None,
                default_expr,
                data_type,
                name,
            })
        })
        .collect()
}

/// Constraints of `$2` in schema `$1`: kind, name, columns, referenced table and columns,
/// CHECK expression, the constraint's text.
pub const CONSTRAINTS: &str = "SELECT constraint_type, constraint_name, \
     constraint_column_names, referenced_table, referenced_column_names, expression, \
     constraint_text FROM duckdb_constraints() WHERE database_name = current_database() \
     AND schema_name = $1 AND table_name = $2 ORDER BY constraint_index";

/// What the constraints of a table give its structure.
#[derive(Debug, Clone, Default)]
pub struct Constraints {
    pub primary_key: Vec<String>,
    /// The primary key and UNIQUE constraints, as the unique indexes behind them.
    pub indexes: Vec<IndexMeta>,
    pub foreign_keys: Vec<ForeignKeyMeta>,
    pub checks: Vec<CheckMeta>,
}

pub fn parse_constraints(rows: &[Vec<Value>], schema: &str, table: &str) -> Constraints {
    let mut out = Constraints::default();
    for r in rows {
        let kind = text(r, 0).unwrap_or_default();
        let columns = list(r, 2);
        let definition = non_empty(r, 6);
        let name = non_empty(r, 1).unwrap_or_else(|| default_name(table, &columns, &kind));
        match kind.as_str() {
            "PRIMARY KEY" => {
                out.primary_key = columns.clone();
                out.indexes.push(IndexMeta {
                    name,
                    columns,
                    unique: true,
                    primary: true,
                    method: None,
                    predicate: None,
                    definition,
                    comment: None,
                    include: Vec::new(),
                });
            }
            "UNIQUE" => out.indexes.push(IndexMeta {
                name,
                columns,
                unique: true,
                primary: false,
                method: None,
                predicate: None,
                definition,
                comment: None,
                include: Vec::new(),
            }),
            "FOREIGN KEY" => {
                let ref_name = text(r, 3).unwrap_or_default();
                let ref_schema = definition
                    .as_deref()
                    .and_then(referenced_schema)
                    .unwrap_or_else(|| schema.to_string());
                out.foreign_keys.push(ForeignKeyMeta {
                    name,
                    columns,
                    ref_table: TableRef::new(Some(&ref_schema), &ref_name),
                    ref_columns: list(r, 4),
                    // DuckDB enforces only NO ACTION (RESTRICT).
                    on_update: "NO ACTION".into(),
                    on_delete: "NO ACTION".into(),
                    definition,
                });
            }
            "CHECK" => {
                let expression = non_empty(r, 5)
                    .or_else(|| definition.as_deref().map(check_body))
                    .unwrap_or_default();
                out.checks.push(CheckMeta {
                    name,
                    expression,
                    columns,
                });
            }
            // NOT NULL is the column's; anything else is not structure the grid shows.
            _ => {}
        }
    }
    out
}

fn default_name(table: &str, columns: &[String], kind: &str) -> String {
    let suffix = match kind {
        "PRIMARY KEY" => "pkey",
        "UNIQUE" => "key",
        "FOREIGN KEY" => "fkey",
        "CHECK" => "check",
        _ => "constraint",
    };
    let mut parts = vec![table.to_string()];
    parts.extend(columns.iter().cloned());
    parts.push(suffix.into());
    parts.join("_")
}

/// The schema of `REFERENCES s.t(…)` in a constraint's text, when it names one.
fn referenced_schema(definition: &str) -> Option<String> {
    let upper = definition.to_ascii_uppercase();
    let at = upper.find("REFERENCES")? + "REFERENCES".len();
    let rest = definition[at..].trim_start();
    let name_end = rest.find('(').unwrap_or(rest.len());
    let parts = split_qualified(rest[..name_end].trim());
    (parts.len() >= 2).then(|| parts[parts.len() - 2].clone())
}

/// `a."b c".d` as its parts, unquoted.
fn split_qualified(name: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut chars = name.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                cur.push('"');
            }
            '"' => quoted = !quoted,
            '.' if !quoted => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    parts
}

/// The expression of `CHECK(expr)`.
fn check_body(definition: &str) -> String {
    let t = definition.trim();
    let upper = t.to_ascii_uppercase();
    match upper.find("CHECK") {
        Some(i) => {
            let rest = t[i + 5..].trim();
            rest.strip_prefix('(')
                .and_then(|r| r.strip_suffix(')'))
                .unwrap_or(rest)
                .trim()
                .to_string()
        }
        None => t.to_string(),
    }
}

/// The indexes of `$2` in schema `$1` (CREATE INDEX ones; keys and UNIQUE constraints come from
/// the constraints). `expressions` is NULL in DuckDB 1.5, so the columns come from `sql`.
pub const INDEXES: &str = "SELECT index_name, is_unique, sql, comment FROM duckdb_indexes() \
     WHERE database_name = current_database() AND schema_name = $1 AND table_name = $2 \
     ORDER BY index_name";

pub fn parse_indexes(rows: &[Vec<Value>]) -> Vec<IndexMeta> {
    rows.iter()
        .filter_map(|r| {
            let sql = non_empty(r, 2);
            Some(IndexMeta {
                name: text(r, 0)?,
                columns: sql.as_deref().map(index_columns).unwrap_or_default(),
                unique: flag(r, 1),
                primary: false,
                method: None,
                predicate: None,
                definition: sql,
                comment: non_empty(r, 3),
                include: Vec::new(),
            })
        })
        .collect()
}

/// The key parts of `CREATE [UNIQUE] INDEX i ON t (a, "b c", lower(d))`: plain names unquoted,
/// expressions as written.
pub fn index_columns(sql: &str) -> Vec<String> {
    let upper = sql.to_ascii_uppercase();
    let Some(on) = upper.find(" ON ") else {
        return Vec::new();
    };
    let Some(open) = sql[on..].find('(').map(|i| on + i) else {
        return Vec::new();
    };
    // The list up to its matching parenthesis, past strings and quoted names.
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut parts = Vec::new();
    let mut cur = String::new();
    for c in sql[open..].chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
                continue;
            }
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                cur.push(c);
                continue;
            }
            None => {}
        }
        match c {
            '(' => {
                depth += 1;
                if depth > 1 {
                    cur.push(c);
                }
            }
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    parts.push(std::mem::take(&mut cur));
                    break;
                }
                cur.push(c);
            }
            ',' if depth == 1 => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts
        .into_iter()
        .map(|p| {
            let p = p.trim();
            let names = split_qualified(p);
            let plain = names.len() == 1
                && (p.starts_with('"') && p.ends_with('"')
                    || p.chars().all(|c| c.is_alphanumeric() || c == '_'));
            if plain {
                names.into_iter().next().unwrap_or_default()
            } else {
                // DuckDB writes an expression key in parentheses: `(lower(status))`.
                strip_outer_parens(p).to_string()
            }
        })
        .filter(|p| !p.is_empty())
        .collect()
}

/// `(e)` as `e` when the parentheses enclose all of it (not `(a) + (b)`).
fn strip_outer_parens(e: &str) -> &str {
    let Some(inner) = e.strip_prefix('(').and_then(|r| r.strip_suffix(')')) else {
        return e;
    };
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in inner.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        return e;
                    }
                }
                _ => {}
            },
        }
    }
    if depth == 0 { inner.trim() } else { e }
}

/// A table's structure from the parsed parts: the primary key's columns marked in key order,
/// the constraint indexes before the CREATE INDEX ones.
pub fn table_meta(
    table: TableRef,
    info: TableInfo,
    mut columns: Vec<ColumnMeta>,
    constraints: Constraints,
    indexes: Vec<IndexMeta>,
) -> TableMeta {
    let Constraints {
        primary_key,
        indexes: mut all,
        foreign_keys,
        checks,
    } = constraints;
    for c in &mut columns {
        if let Some(i) = primary_key.iter().position(|k| *k == c.name) {
            c.is_pk = true;
            c.pk_ordinal = u32::try_from(i + 1).ok();
        }
    }
    all.extend(indexes);
    TableMeta {
        table,
        kind: info.kind,
        columns,
        primary_key,
        indexes: all,
        foreign_keys,
        triggers: Vec::new(),
        comment: info.comment,
        row_estimate: info.row_estimate,
        ddl: info.sql,
        checks,
        options: None,
    }
}

// --- Definitions, sequences, macros, completion --------------------------------------------

/// The `CREATE VIEW` text of view `$2` in schema `$1`.
pub const VIEW_DEFINITION: &str = "SELECT sql FROM duckdb_views() \
     WHERE database_name = current_database() AND schema_name = $1 AND view_name = $2";

/// The `CREATE TABLE` text of `$2` in schema `$1`, then its indexes' `CREATE INDEX`.
pub const TABLE_DDL: &str = "SELECT sql FROM duckdb_tables() \
     WHERE database_name = current_database() AND schema_name = $1 AND table_name = $2 \
     UNION ALL SELECT sql FROM (SELECT sql FROM duckdb_indexes() \
     WHERE database_name = current_database() AND schema_name = $1 AND table_name = $2 \
     AND sql IS NOT NULL ORDER BY index_name)";

/// The first row's text (a definition); None when there is none.
pub fn parse_definition(rows: &[Vec<Value>]) -> Option<String> {
    rows.first().and_then(|r| non_empty(r, 0))
}

/// The statements of TABLE_DDL as one script.
pub fn parse_table_ddl(rows: &[Vec<Value>]) -> Option<String> {
    let parts: Vec<String> = rows
        .iter()
        .filter_map(|r| non_empty(r, 0))
        .map(|s| {
            let s = s.trim_end().to_string();
            if s.ends_with(';') { s } else { format!("{s};") }
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Sequences of schema `$1`.
pub const SEQUENCES: &str = "SELECT sequence_name, start_value, increment_by, last_value, \
     comment, sql FROM duckdb_sequences() WHERE database_name = current_database() \
     AND schema_name = $1 AND NOT temporary ORDER BY sequence_name";

/// The columns whose default draws from a sequence (`nextval('s')`), for `owned_by`.
pub const SEQUENCE_USERS: &str = "SELECT table_name, column_name, column_default \
     FROM duckdb_columns() WHERE database_name = current_database() AND schema_name = $1 \
     AND column_default ILIKE 'nextval(%'";

pub fn parse_sequences(
    rows: &[Vec<Value>],
    users: &[Vec<Value>],
    schema: &str,
) -> Vec<SequenceSummary> {
    rows.iter()
        .filter_map(|r| {
            let name = text(r, 0)?;
            let owned_by = users.iter().find_map(|u| {
                let default = text(u, 2)?;
                if nextval_target(&default)? != name {
                    return None;
                }
                Some(format!("{}.{}", text(u, 0)?, text(u, 1)?))
            });
            Some(SequenceSummary {
                schema: Some(schema.to_string()),
                // DuckDB sequences are BIGINT.
                data_type: Some("BIGINT".into()),
                owned_by,
                comment: non_empty(r, 4),
                name,
            })
        })
        .collect()
}

/// The sequence `nextval('s')` / `nextval('main.s')` draws from (its last name part).
fn nextval_target(default: &str) -> Option<String> {
    let t = default.trim();
    let open = t.find('(')?;
    let inner = t[open + 1..].trim_end().strip_suffix(')')?.trim();
    let inner = inner
        .strip_prefix('\'')?
        .strip_suffix('\'')?
        .replace("''", "'");
    split_qualified(&inner).pop()
}

/// The `CREATE SEQUENCE` text of sequence `$2` in schema `$1`.
pub const SEQUENCE_DEFINITION: &str = "SELECT sql FROM duckdb_sequences() \
     WHERE database_name = current_database() AND schema_name = $1 AND sequence_name = $2";

/// Macros of schema `$1` (DuckDB's user-defined functions), one row per overload.
pub const MACROS: &str = "SELECT function_name, function_type, parameters, comment, \
     macro_definition FROM duckdb_functions() WHERE database_name = current_database() \
     AND schema_name = $1 AND NOT internal AND function_type IN ('macro', 'table_macro') \
     ORDER BY function_name";

pub fn parse_macros(rows: &[Vec<Value>], schema: &str) -> Vec<RoutineSummary> {
    rows.iter()
        .filter_map(|r| {
            let table = text(r, 1).as_deref() == Some("table_macro");
            Some(RoutineSummary {
                schema: Some(schema.to_string()),
                name: text(r, 0)?,
                kind: RoutineKind::Function,
                arguments: list(r, 2).join(", "),
                // DROP MACRO drops every overload.
                identity_arguments: String::new(),
                return_type: Some(if table { "TABLE" } else { "MACRO" }.into()),
                language: Some("SQL".into()),
                comment: non_empty(r, 3),
            })
        })
        .collect()
}

/// The definition of macro `$2` in schema `$1`, one row per overload: its type, parameters and
/// body.
pub const MACRO_DEFINITION: &str = "SELECT function_type, parameters, macro_definition \
     FROM duckdb_functions() WHERE database_name = current_database() AND schema_name = $1 \
     AND function_name = $2 AND function_type IN ('macro', 'table_macro') AND NOT internal";

/// The `CREATE MACRO` text of macro `name` in `schema` from its MACRO_DEFINITION rows, every
/// overload (a table macro's with `TABLE`), written by `sqlgen::dialect::duckdb`; None without a
/// row.
pub fn parse_macro_definition(rows: &[Vec<Value>], schema: &str, name: &str) -> Option<String> {
    let parts: Vec<(Vec<String>, bool, String)> = rows
        .iter()
        .filter_map(|r| {
            let table = text(r, 0).as_deref() == Some("table_macro");
            Some((list(r, 1), table, text(r, 2)?))
        })
        .collect();
    let overloads: Vec<duckdb_sql::MacroOverload<'_>> = parts
        .iter()
        .map(|(parameters, table, body)| duckdb_sql::MacroOverload {
            parameters,
            table: *table,
            body,
        })
        .collect();
    (!overloads.is_empty()).then(|| duckdb_sql::create_macro(schema, name, &overloads))
}

/// Every column of every table and view of the current database, for completion.
pub const COMPLETION: &str = "SELECT schema_name, table_name, column_name \
     FROM duckdb_columns() WHERE database_name = current_database() AND NOT internal \
     ORDER BY schema_name, table_name, column_index";

pub fn parse_completion(rows: &[Vec<Value>], default_schema: Option<String>) -> CompletionMetadata {
    let mut schemas: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for r in rows {
        let (Some(s), Some(t), Some(c)) = (text(r, 0), text(r, 1), text(r, 2)) else {
            continue;
        };
        schemas.entry(s).or_default().entry(t).or_default().push(c);
    }
    CompletionMetadata {
        default_schema,
        schemas,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn enum_labels_of_inline_types() {
        assert_eq!(
            enum_labels("ENUM('sad', 'it''s ok')"),
            Some(vec!["sad".to_string(), "it's ok".to_string()])
        );
        assert_eq!(enum_labels("ENUM()"), Some(Vec::new()));
        assert_eq!(enum_labels("VARCHAR"), None);
        assert_eq!(enum_labels("ENUM('a"), None);
    }

    #[test]
    fn index_columns_from_sql() {
        assert_eq!(
            index_columns("CREATE UNIQUE INDEX i ON t(a, \"b c\", lower(d));"),
            vec!["a", "b c", "lower(d)"]
        );
        assert_eq!(
            index_columns("CREATE INDEX \"x ON y\" ON main.t (\"a,b\", (c || ','))"),
            vec!["a,b", "c || ','"]
        );
        assert!(index_columns("CREATE INDEX i").is_empty());
    }

    #[test]
    fn constraints_make_keys_and_checks() {
        let rows = vec![
            vec![
                t("PRIMARY KEY"),
                t("orders_id_pkey"),
                Value::Json("[\"id\"]".into()),
                Value::Null,
                Value::Null,
                Value::Null,
                t("PRIMARY KEY(id)"),
            ],
            vec![
                t("FOREIGN KEY"),
                Value::Null,
                Value::Json("[\"customer_id\"]".into()),
                t("customers"),
                Value::Json("[\"id\"]".into()),
                Value::Null,
                t("FOREIGN KEY (customer_id) REFERENCES crm.\"customers\"(id)"),
            ],
            vec![
                t("CHECK"),
                t("orders_total_check"),
                Value::Json("[\"total\"]".into()),
                Value::Null,
                Value::Null,
                Value::Null,
                t("CHECK((total >= 0))"),
            ],
            vec![
                t("NOT NULL"),
                t("orders_id_not_null"),
                Value::Json("[\"id\"]".into()),
                Value::Null,
                Value::Null,
                Value::Null,
                t("NOT NULL"),
            ],
        ];
        let c = parse_constraints(&rows, "main", "orders");
        assert_eq!(c.primary_key, vec!["id"]);
        assert_eq!(c.indexes.len(), 1);
        assert!(c.indexes[0].primary);
        assert_eq!(c.foreign_keys.len(), 1);
        let fk = &c.foreign_keys[0];
        assert_eq!(fk.name, "orders_customer_id_fkey");
        assert_eq!(fk.ref_table, TableRef::new(Some("crm"), "customers"));
        assert_eq!(fk.ref_columns, vec!["id"]);
        assert_eq!(c.checks[0].expression, "(total >= 0)");
    }

    #[test]
    fn sequences_find_their_columns() {
        let rows = vec![vec![
            t("order_seq"),
            Value::Int(1),
            Value::Int(1),
            Value::Null,
            Value::Null,
        ]];
        let users = vec![vec![t("orders"), t("id"), t("nextval('main.order_seq')")]];
        let s = parse_sequences(&rows, &users, "main");
        assert_eq!(s[0].owned_by.as_deref(), Some("orders.id"));
        assert_eq!(nextval_target("nextval('it''s')").as_deref(), Some("it's"));
        assert!(auto_increments(Some("nextval('order_seq')")));
        assert!(!auto_increments(Some("42")));
    }

    #[test]
    fn macros_and_their_definitions() {
        let rows = vec![vec![
            t("add"),
            t("macro"),
            Value::Json("[\"a\", \"b\"]".into()),
            Value::Null,
            t("(a + b)"),
        ]];
        let m = parse_macros(&rows, "main");
        assert_eq!(m[0].arguments, "a, b");
        assert_eq!(m[0].return_type.as_deref(), Some("MACRO"));
        let def = vec![vec![
            t("table_macro"),
            Value::Json("[\"n\"]".into()),
            t("SELECT * FROM range(n)"),
        ]];
        assert_eq!(
            parse_macro_definition(&def, "main", "r").as_deref(),
            Some("CREATE MACRO \"main\".\"r\"(\"n\") AS TABLE SELECT * FROM range(n);")
        );
    }
}
