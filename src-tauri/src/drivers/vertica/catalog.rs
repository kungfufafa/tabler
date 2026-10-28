//! Vertica's catalog: the `v_catalog` / `v_monitor` queries the driver runs and the parsers of
//! their rows.
//!
//! The queries take names as bind parameters (`?`), never as text, and the parsers read rows as
//! `conn` decodes them (also from the scripted server), so they are tested without a server.
//! Vertica keeps one database per cluster; below it are schemas, whose tables are regular,
//! temporary, flex or external tables and views. Projections (the physical, sorted and
//! segmented copies of a table) stand where other engines show indexes. Primary key, unique and
//! check constraints may be disabled (not enforced); foreign keys are never enforced.

use std::collections::BTreeMap;

use crate::error::AppError;
use crate::types::objects::{RoutineKind, RoutineSummary, SequenceSummary};
use crate::types::schema::{
    CheckMeta, ColumnMeta, CompletionMetadata, DatabaseInfo, ForeignKeyMeta, IndexMeta, SchemaInfo,
    TableKind, TableSummary,
};
use crate::types::value::Value;

use super::types;

// --- Reading cells -------------------------------------------------------------------------------

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

/// An integer cell (int, numeric without decimals, or digits as text: `character_maximum_length`
/// is a VARCHAR).
pub fn int(row: &[Value], i: usize) -> Option<i64> {
    match cell(row, i)? {
        Value::Int(n) => Some(*n),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::BigInt(s) | Value::Decimal(s) | Value::Text(s) => {
            let s = s.trim();
            s.parse()
                .ok()
                .or_else(|| s.split_once('.').and_then(|(i, _)| i.parse().ok()))
        }
        Value::Float(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Some(*f as i64),
        _ => None,
    }
}

/// A boolean cell (`t` / `f` as text too); NULL is false.
pub fn flag(row: &[Value], i: usize) -> bool {
    match cell(row, i) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Text(s)) => matches!(s.trim(), "t" | "true" | "1"),
        Some(_) => int(row, i).is_some_and(|n| n != 0),
        None => false,
    }
}

fn i32_of(row: &[Value], i: usize) -> Option<i32> {
    int(row, i).and_then(|n| i32::try_from(n).ok())
}

// --- Server --------------------------------------------------------------------------------------

/// Who and where the session is. `CURRENT_SESSION()` names the session the way
/// `v_monitor.sessions.session_id` does.
pub const SERVER_INFO: &str = "SELECT VERSION(), CURRENT_DATABASE(), CURRENT_USER(), \
     CURRENT_SCHEMA(), CURRENT_SESSION()";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerticaServerInfo {
    /// `Vertica Analytic Database v24.1.0-0`
    pub version_text: String,
    pub database: Option<String>,
    pub user: Option<String>,
    pub schema: Option<String>,
    pub session_id: Option<String>,
}

impl VerticaServerInfo {
    /// The release (`24.1.0-0`), after the product name's ` v`.
    pub fn release(&self) -> Option<&str> {
        let t = &self.version_text;
        let at = t
            .match_indices(" v")
            .map(|(i, _)| i + 2)
            .find(|&i| t[i..].starts_with(|c: char| c.is_ascii_digit()))?;
        t[at..].split_whitespace().next()
    }

    /// The major version (24 of `24.1.0-0`).
    pub fn major(&self) -> Option<u32> {
        self.release()?.split('.').next()?.parse().ok()
    }

    /// What the status pill shows: the product's name and release (`Vertica Analytic Database
    /// 24.1.0-0`; the product is called OpenText Analytics Database in later releases).
    pub fn label(&self) -> String {
        match self.release() {
            Some(release) => {
                let product = self.version_text.split(" v").next().unwrap_or("Vertica");
                format!("{} {release}", product.trim())
            }
            None if self.version_text.is_empty() => "Vertica".into(),
            None => self.version_text.clone(),
        }
    }
}

pub fn parse_server_info(rows: &[Vec<Value>]) -> Result<VerticaServerInfo, AppError> {
    let row = rows
        .first()
        .ok_or_else(|| AppError::Internal("Vertica sent no server information".into()))?;
    Ok(VerticaServerInfo {
        version_text: text(row, 0).unwrap_or_default(),
        database: text(row, 1),
        user: text(row, 2),
        schema: text(row, 3),
        session_id: text(row, 4),
    })
}

// --- Databases and schemas -----------------------------------------------------------------------

/// The cluster's one database.
pub const DATABASES: &str = "SELECT database_name, owner_name FROM v_catalog.databases";

pub fn parse_databases(rows: &[Vec<Value>], current: Option<&str>) -> Vec<DatabaseInfo> {
    let mut out: Vec<DatabaseInfo> = rows
        .iter()
        .filter_map(|r| {
            let name = non_empty(r, 0)?;
            Some(DatabaseInfo {
                is_current: current.is_some_and(|c| c.eq_ignore_ascii_case(&name)),
                owner: text(r, 1),
                encoding: Some("UTF-8".into()),
                name,
            })
        })
        .collect();
    // A user without access to v_catalog.databases still has the database it is in.
    if out.is_empty()
        && let Some(c) = current
    {
        out.push(DatabaseInfo {
            name: c.to_string(),
            owner: None,
            encoding: Some("UTF-8".into()),
            is_current: true,
        });
    }
    out
}

/// Every schema, the system ones (`v_catalog`, `v_monitor`…) flagged; the sidebar leaves those
/// out through the descriptor's `system_schemas` unless asked to show them.
pub const SCHEMAS: &str =
    "SELECT schema_name, is_system_schema FROM v_catalog.schemata ORDER BY schema_name";

pub fn parse_schemas(rows: &[Vec<Value>], default_schema: Option<&str>) -> Vec<SchemaInfo> {
    rows.iter()
        .filter_map(|r| {
            let name = non_empty(r, 0)?;
            Some(SchemaInfo {
                is_default: default_schema == Some(name.as_str()),
                name,
            })
        })
        .collect()
}

// --- Tables --------------------------------------------------------------------------------------

/// The tables and views of a schema (`?` twice: the schema), with their kind and comment.
pub const TABLES: &str = "SELECT t.table_name, CASE WHEN t.table_definition IS NOT NULL THEN \
     'external' WHEN t.is_temp_table THEN 'temp' WHEN t.is_flextable THEN 'flex' ELSE 'table' END, \
     c.comment FROM v_catalog.tables t LEFT JOIN v_catalog.comments c ON c.object_id = t.table_id \
     AND c.object_type = 'TABLE' WHERE t.table_schema = ? UNION ALL SELECT v.table_name, 'view', \
     c.comment FROM v_catalog.views v LEFT JOIN v_catalog.comments c ON c.object_id = v.table_id \
     AND c.object_type = 'VIEW' WHERE v.table_schema = ? ORDER BY 1";

/// A table's kind as `TABLES` spells it.
fn table_kind(kind: &str) -> TableKind {
    match kind {
        "view" => TableKind::View,
        "external" => TableKind::ForeignTable,
        _ => TableKind::Table,
    }
}

pub fn parse_tables(rows: &[Vec<Value>], schema: &str) -> Vec<TableSummary> {
    rows.iter()
        .filter_map(|r| {
            Some(TableSummary {
                name: non_empty(r, 0)?,
                schema: Some(schema.to_string()),
                kind: table_kind(text(r, 1).unwrap_or_default().as_str()),
                row_estimate: None,
                size_bytes: None,
                comment: non_empty(r, 2),
            })
        })
        .collect()
}

/// Rows and bytes per table of a schema (`?`: the schema), from the projections' storage. A
/// segmented projection spreads its rows over the nodes (their sum), an unsegmented one holds
/// all of them on each node (their maximum); a super projection has every row, so a table's
/// rows are its largest super projection's. Bytes count every projection.
pub const TABLE_SIZES: &str = "SELECT anchor_table_name, MAX(CASE WHEN is_super THEN n END), \
     SUM(bytes) FROM (SELECT ps.anchor_table_name, ps.projection_id, p.is_super_projection AS \
     is_super, CASE WHEN p.is_segmented THEN SUM(ps.row_count) ELSE MAX(ps.row_count) END AS n, \
     SUM(ps.used_bytes) AS bytes FROM v_monitor.projection_storage ps JOIN v_catalog.projections p \
     ON p.projection_id = ps.projection_id WHERE ps.anchor_table_schema = ? GROUP BY \
     ps.anchor_table_name, ps.projection_id, p.is_super_projection, p.is_segmented) s GROUP BY \
     anchor_table_name";

/// Puts the sizes of `TABLE_SIZES` on the tables.
pub fn apply_sizes(tables: &mut [TableSummary], rows: &[Vec<Value>]) {
    let sizes: BTreeMap<String, (Option<i64>, Option<i64>)> = rows
        .iter()
        .filter_map(|r| Some((text(r, 0)?, (int(r, 1), int(r, 2)))))
        .collect();
    for t in tables.iter_mut() {
        if let Some((rows, bytes)) = sizes.get(&t.name) {
            t.row_estimate = *rows;
            t.size_bytes = *bytes;
        }
    }
}

// --- Columns -------------------------------------------------------------------------------------

/// A table's or view's columns (`?` four times: schema and name, for tables then views).
pub const COLUMNS: &str = "SELECT column_name, data_type, data_type_id, \
     character_maximum_length, numeric_precision, numeric_scale, is_nullable, column_default, \
     is_identity, ordinal_position, column_set_using FROM v_catalog.columns WHERE table_schema = ? \
     AND table_name = ? UNION ALL SELECT column_name, data_type, data_type_id, \
     character_maximum_length, numeric_precision, numeric_scale, true, NULL, false, \
     ordinal_position, NULL FROM v_catalog.view_columns WHERE table_schema = ? AND table_name = ? \
     ORDER BY 10";

/// The comments on a schema's columns (`?`: the schema): `table.column` names.
pub const COLUMN_COMMENTS: &str = "SELECT object_name, comment FROM v_catalog.comments WHERE \
     object_type = 'COLUMN' AND object_schema = ?";

pub fn parse_columns(rows: &[Vec<Value>]) -> Vec<ColumnMeta> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let name = non_empty(r, 0)?;
            let data_type = text(r, 1).unwrap_or_default();
            let identity = flag(r, 8);
            let default_expr = non_empty(r, 7);
            // An AUTO_INCREMENT / IDENTITY column's default is its sequence's NEXTVAL.
            let auto = identity
                || default_expr
                    .as_deref()
                    .is_some_and(|d| d.to_ascii_lowercase().contains("nextval("));
            let set_using = non_empty(r, 10);
            Some(ColumnMeta {
                ordinal: int(r, 9)
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(i as u32 + 1),
                kind: types::kind_from_name(&data_type),
                nullable: flag(r, 6),
                default_expr,
                is_pk: false,
                pk_ordinal: None,
                is_auto_increment: auto,
                is_generated: set_using.is_some(),
                is_unsigned: false,
                comment: None,
                enum_values: None,
                char_max_len: int(r, 3),
                numeric_precision: i32_of(r, 4),
                numeric_scale: i32_of(r, 5),
                collation: None,
                array_elem_type: array_element(&data_type),
                extra: set_using.map(|e| format!("SET USING {e}")),
                base_type: None,
                data_type,
                name,
            })
        })
        .collect()
}

/// The element type of `Array[int]` / `Set[varchar(10)]`.
fn array_element(data_type: &str) -> Option<String> {
    let lower = data_type.to_ascii_lowercase();
    if !(lower.starts_with("array[") || lower.starts_with("set[")) {
        return None;
    }
    let open = data_type.find('[')?;
    let close = data_type.rfind(']')?;
    (close > open + 1).then(|| data_type[open + 1..close].trim().to_string())
}

/// Puts the comments of `COLUMN_COMMENTS` on the columns of `table`.
pub fn apply_column_comments(columns: &mut [ColumnMeta], table: &str, rows: &[Vec<Value>]) {
    let prefix = format!("{table}.");
    for r in rows {
        let (Some(object), Some(comment)) = (text(r, 0), non_empty(r, 1)) else {
            continue;
        };
        let Some(column) = object.strip_prefix(&prefix) else {
            continue;
        };
        if let Some(c) = columns.iter_mut().find(|c| c.name == column) {
            c.comment = Some(comment);
        }
    }
}

/// The column a RowDescription field reads (`?`: the table OIDs of the fields, one each): the
/// attribute number is the part of `column_id` after the table's id (`45035996273719486-1`).
pub fn column_by_attribute(tables: usize) -> String {
    let marks = vec!["?"; tables.max(1)].join(", ");
    format!(
        "SELECT table_id, column_id, column_name FROM v_catalog.columns WHERE table_id IN ({marks})"
    )
}

/// (table OID, attribute number) → column name, from `column_by_attribute`'s rows.
pub fn parse_attributes(rows: &[Vec<Value>]) -> BTreeMap<(u64, u16), String> {
    rows.iter()
        .filter_map(|r| {
            let table = u64::try_from(int(r, 0)?).ok()?;
            let attribute = text(r, 1)?.rsplit('-').next()?.trim().parse().ok()?;
            Some(((table, attribute), non_empty(r, 2)?))
        })
        .collect()
}

// --- Keys and constraints ------------------------------------------------------------------------

/// The primary key's columns (`?`: schema, table). A disabled key is not enforced: rows may
/// repeat it, which the edits' affected-row guard catches. The reference gives the keys'
/// `ordinal_position` as VARCHAR, so it sorts as a number (a tenth column after the ninth).
pub const PRIMARY_KEY: &str = "SELECT constraint_name, column_name, ordinal_position, \
     is_enabled FROM v_catalog.primary_keys WHERE table_schema = ? AND table_name = ? ORDER BY \
     ordinal_position::INT";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrimaryKey {
    pub name: String,
    pub columns: Vec<String>,
    pub enabled: bool,
}

pub fn parse_primary_key(rows: &[Vec<Value>]) -> Option<PrimaryKey> {
    let first = rows.first()?;
    let columns: Vec<String> = rows.iter().filter_map(|r| non_empty(r, 1)).collect();
    (!columns.is_empty()).then(|| PrimaryKey {
        name: text(first, 0).unwrap_or_default(),
        columns,
        enabled: rows.iter().all(|r| flag(r, 3)),
    })
}

/// Marks the key's columns.
pub fn apply_primary_key(columns: &mut [ColumnMeta], key: &PrimaryKey) {
    for (i, name) in key.columns.iter().enumerate() {
        if let Some(c) = columns.iter_mut().find(|c| &c.name == name) {
            c.is_pk = true;
            c.pk_ordinal = Some(i as u32 + 1);
        }
    }
}

/// Foreign keys (`?`: schema, table); Vertica never enforces them.
pub const FOREIGN_KEYS: &str = "SELECT constraint_name, column_name, reference_table_schema, \
     reference_table_name, reference_column_name, ordinal_position FROM v_catalog.foreign_keys \
     WHERE table_schema = ? AND table_name = ? ORDER BY constraint_name, ordinal_position::INT";

pub fn parse_foreign_keys(rows: &[Vec<Value>]) -> Vec<ForeignKeyMeta> {
    let mut out: Vec<ForeignKeyMeta> = Vec::new();
    for r in rows {
        let (Some(name), Some(column), Some(ref_table), Some(ref_column)) = (
            text(r, 0),
            non_empty(r, 1),
            non_empty(r, 3),
            non_empty(r, 4),
        ) else {
            continue;
        };
        let ref_schema = non_empty(r, 2);
        match out.iter_mut().find(|f| f.name == name) {
            Some(fk) => {
                fk.columns.push(column);
                fk.ref_columns.push(ref_column);
            }
            None => out.push(ForeignKeyMeta {
                name,
                columns: vec![column],
                ref_table: crate::types::schema::TableRef::new(ref_schema.as_deref(), &ref_table),
                ref_columns: vec![ref_column],
                // Informational only: Vertica has no referential actions.
                on_update: "NO ACTION".into(),
                on_delete: "NO ACTION".into(),
                definition: None,
            }),
        }
    }
    out
}

/// Unique and check constraints (`?`: schema, table), one row per column (none for a check on
/// an expression the catalog keeps no columns of).
pub const CONSTRAINTS: &str = "SELECT tc.constraint_name, tc.constraint_type, cc.column_name, \
     tc.predicate, tc.is_enabled FROM v_catalog.table_constraints tc JOIN v_catalog.tables t ON \
     t.table_id = tc.table_id LEFT JOIN v_catalog.constraint_columns cc ON cc.constraint_id = \
     tc.constraint_id WHERE t.table_schema = ? AND t.table_name = ? AND tc.constraint_type IN \
     ('u', 'c') ORDER BY tc.constraint_name";

/// What `CONSTRAINTS` holds: unique constraints as indexes (a disabled one says so in its
/// definition) and the checks.
pub fn parse_constraints(rows: &[Vec<Value>]) -> (Vec<IndexMeta>, Vec<CheckMeta>) {
    let mut uniques: Vec<IndexMeta> = Vec::new();
    let mut checks: Vec<CheckMeta> = Vec::new();
    for r in rows {
        let Some(name) = non_empty(r, 0) else {
            continue;
        };
        let column = non_empty(r, 2);
        let enabled = flag(r, 4);
        match text(r, 1).unwrap_or_default().trim() {
            "u" => {
                let i = match uniques.iter().position(|u| u.name == name) {
                    Some(i) => i,
                    None => {
                        uniques.push(IndexMeta {
                            name,
                            columns: Vec::new(),
                            unique: true,
                            primary: false,
                            method: Some("unique constraint".into()),
                            predicate: None,
                            definition: (!enabled).then(|| "DISABLED (not enforced)".into()),
                            comment: None,
                            include: Vec::new(),
                        });
                        uniques.len() - 1
                    }
                };
                uniques[i].columns.extend(column);
            }
            "c" => match checks.iter_mut().find(|c| c.name == name) {
                Some(c) => c.columns.extend(column),
                None => checks.push(CheckMeta {
                    name,
                    expression: text(r, 3).unwrap_or_default(),
                    columns: column.into_iter().collect(),
                }),
            },
            _ => {}
        }
    }
    (uniques, checks)
}

// --- Projections ---------------------------------------------------------------------------------

/// A table's projections and their columns in sort order (`?`: schema, table).
pub const PROJECTIONS: &str = "SELECT p.projection_name, p.is_super_projection, p.is_segmented, \
     p.segment_expression, pc.table_column_name, pc.sort_position, pc.column_position FROM \
     v_catalog.projections p JOIN v_catalog.projection_columns pc ON pc.projection_id = \
     p.projection_id WHERE p.projection_schema = ? AND p.anchor_table_name = ? ORDER BY \
     p.projection_name, pc.column_position";

/// Projections as the Structure tab's indexes: the columns are the sort order (the columns
/// without a sort position follow in projection order), the method says what kind it is.
pub fn parse_projections(rows: &[Vec<Value>]) -> Vec<IndexMeta> {
    struct P {
        name: String,
        sorted: Vec<(i64, String)>,
        rest: Vec<String>,
        method: String,
    }
    let mut out: Vec<P> = Vec::new();
    for r in rows {
        let Some(name) = non_empty(r, 0) else {
            continue;
        };
        let i = match out.iter().position(|p| p.name == name) {
            Some(i) => i,
            None => {
                let kind = if flag(r, 1) {
                    "super projection"
                } else {
                    "projection"
                };
                let method = match (flag(r, 2), non_empty(r, 3)) {
                    (true, Some(e)) => format!("{kind}, segmented by {e}"),
                    (true, None) => format!("{kind}, segmented"),
                    (false, _) => format!("{kind}, unsegmented"),
                };
                out.push(P {
                    name,
                    sorted: Vec::new(),
                    rest: Vec::new(),
                    method,
                });
                out.len() - 1
            }
        };
        let Some(column) = non_empty(r, 4) else {
            continue;
        };
        match int(r, 5) {
            Some(pos) if pos >= 0 => out[i].sorted.push((pos, column)),
            _ => out[i].rest.push(column),
        }
    }
    out.into_iter()
        .map(|mut p| {
            p.sorted.sort_by_key(|(pos, _)| *pos);
            let mut columns: Vec<String> = p.sorted.into_iter().map(|(_, c)| c).collect();
            columns.extend(p.rest);
            IndexMeta {
                name: p.name,
                columns,
                unique: false,
                primary: false,
                method: Some(p.method),
                predicate: None,
                definition: None,
                comment: None,
                include: Vec::new(),
            }
        })
        .collect()
}

// --- Views, sequences, routines ------------------------------------------------------------------

/// A view's text (`?`: schema, view).
pub const VIEW_DEFINITION: &str =
    "SELECT view_definition FROM v_catalog.views WHERE table_schema = ? AND table_name = ?";

pub fn parse_view_definition(rows: &[Vec<Value>]) -> Option<String> {
    rows.first().and_then(|r| text(r, 0))
}

/// The DDL `EXPORT_OBJECTS` returns (the statement is `dialect::vertica::export_objects`).
pub fn parse_ddl(rows: &[Vec<Value>]) -> Option<String> {
    rows.first()
        .and_then(|r| text(r, 0))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// A schema's sequences (`?`: the schema); an IDENTITY / AUTO_INCREMENT column's names its
/// table.
pub const SEQUENCES: &str = "SELECT sequence_name, identity_table_name FROM v_catalog.sequences \
     WHERE sequence_schema = ? ORDER BY sequence_name";

pub fn parse_sequences(rows: &[Vec<Value>], schema: &str) -> Vec<SequenceSummary> {
    rows.iter()
        .filter_map(|r| {
            Some(SequenceSummary {
                schema: Some(schema.to_string()),
                name: non_empty(r, 0)?,
                data_type: Some("int".into()),
                owned_by: non_empty(r, 1),
                comment: None,
            })
        })
        .collect()
}

/// A schema's functions (UDx and SQL functions) and stored procedures (`?` twice: the schema).
pub const ROUTINES: &str = "SELECT function_name, procedure_type, function_argument_type, \
     function_return_type, NULL, comment FROM v_catalog.user_functions WHERE schema_name = ? \
     UNION ALL SELECT procedure_name, 'Stored Procedure', procedure_arguments, NULL, language, \
     NULL FROM v_catalog.user_procedures WHERE schema_name = ? ORDER BY 1";

pub fn parse_routines(rows: &[Vec<Value>], schema: &str) -> Vec<RoutineSummary> {
    rows.iter()
        .filter_map(|r| {
            let ty = text(r, 1).unwrap_or_default().to_ascii_lowercase();
            let kind = if ty.contains("procedure") {
                RoutineKind::Procedure
            } else if ty.contains("aggregate") {
                RoutineKind::Aggregate
            } else if ty.contains("analytic") {
                RoutineKind::Window
            } else {
                RoutineKind::Function
            };
            let arguments = text(r, 2).unwrap_or_default();
            Some(RoutineSummary {
                schema: Some(schema.to_string()),
                name: non_empty(r, 0)?,
                kind,
                identity_arguments: arguments.clone(),
                arguments,
                return_type: non_empty(r, 3),
                language: non_empty(r, 4).or_else(|| ty.contains("sql").then(|| "SQL".into())),
                comment: non_empty(r, 5),
            })
        })
        .collect()
}

// --- Completion ----------------------------------------------------------------------------------

/// Every column of the user's tables and views, for the editor's completion.
pub const COMPLETION: &str = "SELECT table_schema, table_name, column_name, ordinal_position \
     FROM v_catalog.columns WHERE NOT is_system_table UNION ALL SELECT table_schema, table_name, \
     column_name, ordinal_position FROM v_catalog.view_columns ORDER BY 1, 2, 4";

pub fn parse_completion(rows: &[Vec<Value>], default_schema: Option<&str>) -> CompletionMetadata {
    let mut out = CompletionMetadata {
        default_schema: default_schema.map(str::to_string),
        schemas: BTreeMap::new(),
    };
    for r in rows {
        let (Some(schema), Some(table), Some(column)) = (text(r, 0), text(r, 1), text(r, 2)) else {
            continue;
        };
        out.schemas
            .entry(schema)
            .or_default()
            .entry(table)
            .or_default()
            .push(column);
    }
    out
}

// --- Sessions ------------------------------------------------------------------------------------

/// The cluster's sessions (a user without SYSMONITOR sees only its own).
pub const SESSIONS: &str = "SELECT session_id, user_name, client_hostname, client_label, \
     login_timestamp, statement_start, current_statement, statement_id, transaction_id, \
     node_name, client_type FROM v_monitor.sessions ORDER BY login_timestamp";

/// A row of `SESSIONS`. Vertica's session ids are text (`v_vmart_node0001-3104:0x1b2`), which
/// `admin::ProcessRow::id` cannot hold (a change request).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerticaSession {
    pub session_id: String,
    pub user: Option<String>,
    pub client_hostname: Option<String>,
    pub client_label: Option<String>,
    pub login: Option<String>,
    pub statement_start: Option<String>,
    pub statement: Option<String>,
    /// What `INTERRUPT_STATEMENT` names, with the session id.
    pub statement_id: Option<i64>,
    pub transaction_id: Option<i64>,
    pub node: Option<String>,
    pub client_type: Option<String>,
    /// The session that listed them.
    pub current: bool,
}

impl VerticaSession {
    /// One of Tabler's own sessions.
    pub fn own(&self) -> bool {
        self.client_type.as_deref() == Some("Tabler")
    }
}

pub fn parse_sessions(rows: &[Vec<Value>], current: Option<&str>) -> Vec<VerticaSession> {
    rows.iter()
        .filter_map(|r| {
            let session_id = non_empty(r, 0)?;
            Some(VerticaSession {
                current: current == Some(session_id.as_str()),
                user: text(r, 1),
                client_hostname: non_empty(r, 2),
                client_label: non_empty(r, 3),
                login: text(r, 4),
                statement_start: text(r, 5),
                statement: non_empty(r, 6),
                statement_id: int(r, 7),
                transaction_id: int(r, 8),
                node: text(r, 9),
                client_type: non_empty(r, 10),
                session_id,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::schema::{TableRef, ValueKind};

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    const N: Value = Value::Null;

    #[test]
    fn server_info_and_labels() {
        let info = parse_server_info(&[vec![
            t("Vertica Analytic Database v24.1.0-0"),
            t("VMart"),
            t("dbadmin"),
            t("public"),
            t("v_vmart_node0001-3104:0x1b2"),
        ]])
        .unwrap();
        assert_eq!(info.release(), Some("24.1.0-0"));
        assert_eq!(info.major(), Some(24));
        assert_eq!(info.label(), "Vertica Analytic Database 24.1.0-0");
        let renamed = VerticaServerInfo {
            version_text: "OpenText Analytics Database v25.3.0-1".into(),
            ..VerticaServerInfo::default()
        };
        assert_eq!(renamed.label(), "OpenText Analytics Database 25.3.0-1");
        assert_eq!(VerticaServerInfo::default().label(), "Vertica");
        let odd = VerticaServerInfo {
            version_text: "something else".into(),
            ..VerticaServerInfo::default()
        };
        assert_eq!(
            (odd.label().as_str(), odd.major()),
            ("something else", None)
        );
        assert!(parse_server_info(&[]).is_err());
    }

    #[test]
    fn databases_and_schemas() {
        let dbs = parse_databases(&[vec![t("VMart"), t("dbadmin")]], Some("vmart"));
        assert_eq!(dbs.len(), 1);
        assert!(dbs[0].is_current && dbs[0].owner.as_deref() == Some("dbadmin"));
        let fallback = parse_databases(&[], Some("VMart"));
        assert_eq!(fallback[0].name, "VMart");
        assert!(parse_databases(&[], None).is_empty());
        let schemas = parse_schemas(
            &[
                vec![t("public"), Value::Bool(false)],
                vec![t("store"), Value::Bool(false)],
                vec![N, Value::Bool(true)],
            ],
            Some("public"),
        );
        assert_eq!(schemas.len(), 2);
        assert!(schemas[0].is_default && !schemas[1].is_default);
    }

    #[test]
    fn tables_with_kinds_comments_and_sizes() {
        let mut tables = parse_tables(
            &[
                vec![t("events"), t("table"), t("the events")],
                vec![t("raw"), t("flex"), N],
                vec![t("ext"), t("external"), N],
                vec![t("scratch"), t("temp"), N],
                vec![t("v_daily"), t("view"), N],
            ],
            "public",
        );
        let kinds: Vec<TableKind> = tables.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            [
                TableKind::Table,
                TableKind::Table,
                TableKind::ForeignTable,
                TableKind::Table,
                TableKind::View
            ]
        );
        assert_eq!(tables[0].comment.as_deref(), Some("the events"));
        apply_sizes(
            &mut tables,
            &[
                vec![t("events"), Value::Int(1000), Value::Int(4096)],
                vec![t("raw"), N, Value::Int(10)],
            ],
        );
        assert_eq!(
            (tables[0].row_estimate, tables[0].size_bytes),
            (Some(1000), Some(4096))
        );
        assert_eq!(
            (tables[1].row_estimate, tables[1].size_bytes),
            (None, Some(10))
        );
        assert_eq!(tables[4].row_estimate, None);
    }

    fn column_row(
        name: &str,
        ty: &str,
        nullable: bool,
        default: Value,
        identity: bool,
    ) -> Vec<Value> {
        vec![
            t(name),
            t(ty),
            Value::Int(6),
            t("80"),
            Value::Int(10),
            Value::Int(2),
            Value::Bool(nullable),
            default,
            Value::Bool(identity),
            N,
            N,
        ]
    }

    #[test]
    fn columns_keys_and_comments() {
        let mut columns = parse_columns(&[
            column_row("id", "int", false, N, true),
            column_row("name", "varchar(80)", true, t("'x'"), false),
            column_row("n", "int", false, t("nextval('public.s')"), false),
            column_row("tags", "Array[varchar(10)]", true, N, false),
            {
                let mut r = column_row("total", "numeric(10,2)", true, N, false);
                r[9] = Value::Int(9);
                r[10] = t("price * qty");
                r
            },
            vec![N; 11],
        ]);
        assert_eq!(columns.len(), 5);
        assert_eq!(columns[0].kind, ValueKind::Int);
        assert!(columns[0].is_auto_increment && !columns[0].nullable);
        assert_eq!(columns[1].char_max_len, Some(80));
        assert_eq!(columns[1].default_expr.as_deref(), Some("'x'"));
        assert!(columns[2].is_auto_increment);
        assert_eq!(columns[3].kind, ValueKind::Json);
        assert_eq!(columns[3].array_elem_type.as_deref(), Some("varchar(10)"));
        assert_eq!(columns[4].ordinal, 9);
        assert!(columns[4].is_generated);
        assert_eq!(columns[4].extra.as_deref(), Some("SET USING price * qty"));
        assert_eq!(
            (columns[4].numeric_precision, columns[4].numeric_scale),
            (Some(10), Some(2))
        );

        let key = parse_primary_key(&[
            vec![t("C_PRIMARY"), t("id"), Value::Int(1), Value::Bool(true)],
            vec![t("C_PRIMARY"), t("name"), Value::Int(2), t("f")],
        ])
        .unwrap();
        assert_eq!(key.columns, ["id", "name"]);
        assert!(!key.enabled);
        apply_primary_key(&mut columns, &key);
        assert_eq!(columns[1].pk_ordinal, Some(2));
        assert!(parse_primary_key(&[]).is_none());

        apply_column_comments(
            &mut columns,
            "t",
            &[
                vec![t("t.name"), t("the name")],
                vec![t("u.name"), t("not this table")],
                vec![t("t.gone"), t("no such column")],
                vec![t("t.id"), N],
            ],
        );
        assert_eq!(columns[1].comment.as_deref(), Some("the name"));
        assert_eq!(columns[0].comment, None);
    }

    #[test]
    fn attributes_constraints_and_foreign_keys() {
        assert_eq!(
            column_by_attribute(2),
            "SELECT table_id, column_id, column_name FROM v_catalog.columns WHERE table_id IN (?, ?)"
        );
        let attrs = parse_attributes(&[
            vec![
                Value::Int(45_035_996_273_719_486),
                t("45035996273719486-2"),
                t("name"),
            ],
            vec![t("45035996273719486"), t("45035996273719486-1"), t("id")],
            vec![N, t("x-1"), t("lost")],
            vec![Value::Int(1), t("garbage"), t("lost")],
        ]);
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[&(45_035_996_273_719_486, 2)], "name");

        let fks = parse_foreign_keys(&[
            vec![
                t("fk_a"),
                t("a1"),
                t("store"),
                t("parent"),
                t("p1"),
                Value::Int(1),
            ],
            vec![
                t("fk_a"),
                t("a2"),
                t("store"),
                t("parent"),
                t("p2"),
                Value::Int(2),
            ],
            vec![t("fk_b"), t("b"), N, t("other"), t("id"), Value::Int(1)],
            vec![t("fk_c"), N, N, N, N, N],
        ]);
        assert_eq!(fks.len(), 2);
        assert_eq!(fks[0].columns, ["a1", "a2"]);
        assert_eq!(fks[0].ref_table, TableRef::new(Some("store"), "parent"));
        assert_eq!(fks[1].ref_table, TableRef::new(None, "other"));

        let (uniques, checks) = parse_constraints(&[
            vec![t("u_email"), t("u"), t("email"), N, Value::Bool(true)],
            vec![t("u_pair"), t("u"), t("a"), N, Value::Bool(false)],
            vec![t("u_pair"), t("u"), t("b"), N, Value::Bool(false)],
            vec![
                t("ck_pos"),
                t("c"),
                t("qty"),
                t("(qty > 0)"),
                Value::Bool(true),
            ],
            vec![t("ck_expr"), t("c"), N, t("(1 = 1)"), Value::Bool(true)],
            vec![t("x"), t("p"), t("id"), N, Value::Bool(true)],
        ]);
        assert_eq!(uniques.len(), 2);
        assert_eq!(uniques[1].columns, ["a", "b"]);
        assert_eq!(
            uniques[1].definition.as_deref(),
            Some("DISABLED (not enforced)")
        );
        assert!(uniques[0].definition.is_none() && uniques[0].unique);
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].expression, "(qty > 0)");
        assert_eq!(checks[0].columns, ["qty"]);
        assert!(checks[1].columns.is_empty());
    }

    #[test]
    fn projections_as_indexes() {
        let p = parse_projections(&[
            vec![
                t("t_super"),
                Value::Bool(true),
                Value::Bool(true),
                t("hash(t.id)"),
                t("b"),
                Value::Int(1),
                Value::Int(0),
            ],
            vec![
                t("t_super"),
                Value::Bool(true),
                Value::Bool(true),
                t("hash(t.id)"),
                t("a"),
                Value::Int(0),
                Value::Int(1),
            ],
            vec![
                t("t_super"),
                Value::Bool(true),
                Value::Bool(true),
                t("hash(t.id)"),
                t("c"),
                N,
                Value::Int(2),
            ],
            vec![
                t("t_small"),
                Value::Bool(false),
                Value::Bool(false),
                N,
                t("a"),
                Value::Int(0),
                Value::Int(0),
            ],
            vec![
                t("t_seg"),
                Value::Bool(false),
                Value::Bool(true),
                N,
                N,
                N,
                N,
            ],
        ]);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].columns, ["a", "b", "c"]);
        assert_eq!(
            p[0].method.as_deref(),
            Some("super projection, segmented by hash(t.id)")
        );
        assert_eq!(p[1].method.as_deref(), Some("projection, unsegmented"));
        assert_eq!(p[2].method.as_deref(), Some("projection, segmented"));
        assert!(p[2].columns.is_empty());
    }

    #[test]
    fn views_sequences_routines_completion_sessions() {
        assert_eq!(
            parse_view_definition(&[vec![t("SELECT 1")]]).as_deref(),
            Some("SELECT 1")
        );
        assert_eq!(parse_view_definition(&[]), None);
        assert_eq!(
            parse_ddl(&[vec![t("\nCREATE TABLE t (a int);\n")]]).as_deref(),
            Some("CREATE TABLE t (a int);")
        );
        assert_eq!(parse_ddl(&[vec![t("  ")]]), None);

        let seqs = parse_sequences(&[vec![t("s1"), N], vec![t("t_id_seq"), t("t")]], "public");
        assert_eq!(seqs[1].owned_by.as_deref(), Some("t"));

        let routines = parse_routines(
            &[
                vec![
                    t("add2"),
                    t("User Defined SQL Function"),
                    t("x Integer"),
                    t("Integer"),
                    N,
                    t("adds"),
                ],
                vec![
                    t("agg"),
                    t("User Defined Aggregate"),
                    t(""),
                    t("Float"),
                    N,
                    N,
                ],
                vec![
                    t("win"),
                    t("User Defined Analytic"),
                    t(""),
                    t("Float"),
                    N,
                    N,
                ],
                vec![
                    t("load_all"),
                    t("Stored Procedure"),
                    t("n int"),
                    N,
                    t("PL/vSQL"),
                    N,
                ],
                vec![N, t("x"), N, N, N, N],
            ],
            "public",
        );
        let kinds: Vec<RoutineKind> = routines.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            [
                RoutineKind::Function,
                RoutineKind::Aggregate,
                RoutineKind::Window,
                RoutineKind::Procedure
            ]
        );
        assert_eq!(routines[0].language.as_deref(), Some("SQL"));
        assert_eq!(routines[3].language.as_deref(), Some("PL/vSQL"));
        assert_eq!(routines[0].comment.as_deref(), Some("adds"));

        let c = parse_completion(
            &[
                vec![t("public"), t("t"), t("a"), Value::Int(1)],
                vec![t("public"), t("t"), t("b"), Value::Int(2)],
                vec![t("store"), t("u"), t("x"), Value::Int(1)],
                vec![N, t("u"), t("x"), Value::Int(1)],
            ],
            Some("public"),
        );
        assert_eq!(c.schemas["public"]["t"], ["a", "b"]);
        assert_eq!(c.schemas.len(), 2);

        let sessions = parse_sessions(
            &[
                vec![
                    t("node1-1:0x1"),
                    t("dbadmin"),
                    t("10.0.0.1:5000"),
                    t("Tabler"),
                    t("2024-01-01 10:00:00"),
                    N,
                    t(""),
                    Value::Int(3),
                    Value::Int(45_035_996_273_705_000),
                    t("v_vmart_node0001"),
                    t("Tabler"),
                ],
                vec![
                    t("node1-2:0x2"),
                    t("u"),
                    N,
                    N,
                    N,
                    N,
                    t("SELECT 1"),
                    N,
                    N,
                    N,
                    t("vsql"),
                ],
                vec![N; 11],
            ],
            Some("node1-1:0x1"),
        );
        assert_eq!(sessions.len(), 2);
        assert!(sessions[0].current && sessions[0].own() && sessions[0].statement.is_none());
        assert_eq!(sessions[0].statement_id, Some(3));
        assert!(!sessions[1].current && !sessions[1].own());
        assert_eq!(sessions[1].statement.as_deref(), Some("SELECT 1"));
    }

    #[test]
    fn cells_are_read_leniently() {
        let r = vec![
            t(" 12 "),
            t("3.0"),
            Value::Float(4.0),
            t("t"),
            Value::Int(0),
            t("x"),
        ];
        assert_eq!(int(&r, 0), Some(12));
        assert_eq!(int(&r, 1), Some(3));
        assert_eq!(int(&r, 2), Some(4));
        assert!(flag(&r, 3) && !flag(&r, 4) && !flag(&r, 5) && !flag(&r, 9));
        assert_eq!(int(&r, 5), None);
    }
}
