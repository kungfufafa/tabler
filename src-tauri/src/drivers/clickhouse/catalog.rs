//! ClickHouse's catalog: the `system.*` queries the driver runs and the parsers of their rows.
//!
//! Names travel as bound parameters (`{db:String}` with `param_db`), never in the text. The rows
//! are the `Value`s the reader decodes, so the parsers are tested without a server. ClickHouse
//! has two levels, databases and their tables: a database shows where other engines show a
//! schema (`TableRef.schema`). There are no foreign keys or triggers; the primary, sorting,
//! partition and sampling keys and the data-skipping indexes stand where indexes do. The
//! primary key is not unique: edits count the rows it matches first
//! (`dialect::clickhouse::guard_count`).

use crate::error::AppError;
use crate::sqlgen::dialect::clickhouse::KEY_ROLES;
use crate::types::schema::{
    ColumnMeta, CompletionMetadata, DatabaseInfo, IndexMeta, SchemaInfo, TableKind, TableMeta,
    TableOptions, TableRef, TableSummary,
};
use crate::types::value::Value;

use super::types::ChType;

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

/// An integer cell (also a quoted 64-bit one).
pub fn int(row: &[Value], i: usize) -> Option<i64> {
    match cell(row, i)? {
        Value::Int(n) => Some(*n),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::BigInt(s) | Value::Decimal(s) | Value::Text(s) => s.trim().parse().ok(),
        Value::Float(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Some(*f as i64),
        _ => None,
    }
}

/// A UInt8 flag (`is_in_primary_key`); NULL is false.
pub fn flag(row: &[Value], i: usize) -> bool {
    match cell(row, i) {
        Some(Value::Bool(b)) => *b,
        Some(_) => int(row, i).is_some_and(|n| n != 0),
        None => false,
    }
}

fn i32_of(row: &[Value], i: usize) -> Option<i32> {
    int(row, i).and_then(|n| i32::try_from(n).ok())
}

fn first_row<'r>(rows: &'r [Vec<Value>], what: &str) -> Result<&'r [Value], AppError> {
    rows.first()
        .map(Vec::as_slice)
        .ok_or_else(|| AppError::NotFound(what.to_string()))
}

// --- Server --------------------------------------------------------------------------------------

/// Who and where the session is, and whether its user may change settings (`readonly` 1).
pub const SERVER_INFO: &str = "SELECT version(), currentDatabase(), currentUser(), timezone(), \
     toString(getSetting('readonly'))";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClickhouseServerInfo {
    /// `24.8.4.13`
    pub version: String,
    pub database: Option<String>,
    pub user: Option<String>,
    pub timezone: Option<String>,
    /// The `readonly` setting the request ran with: 1 forbids every setting change.
    pub readonly: u8,
}

impl ClickhouseServerInfo {
    /// `(major, minor)` of the version (`24.8.4.13` → `(24, 8)`).
    pub fn release(&self) -> Option<(u32, u32)> {
        let mut parts = self.version.trim().split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        Some((major, minor))
    }

    /// Whether the server is at least `major.minor`.
    pub fn at_least(&self, major: u32, minor: u32) -> bool {
        self.release().is_some_and(|v| v >= (major, minor))
    }

    /// Lightweight `UPDATE … SET` (25.7).
    pub fn has_lightweight_update(&self) -> bool {
        self.at_least(25, 7)
    }

    /// The `role` URL parameter (24.4).
    pub fn has_role_parameter(&self) -> bool {
        self.at_least(24, 4)
    }

    /// `X-ClickHouse-Format` overrides a FORMAT clause (26.8).
    pub fn format_header_wins(&self) -> bool {
        self.at_least(26, 8)
    }

    /// What the status pill shows.
    pub fn label(&self) -> String {
        if self.version.is_empty() {
            "ClickHouse".into()
        } else {
            format!("ClickHouse {}", self.version)
        }
    }
}

pub fn parse_server_info(rows: &[Vec<Value>]) -> Result<ClickhouseServerInfo, AppError> {
    let row = first_row(rows, "ClickHouse server information")?;
    Ok(ClickhouseServerInfo {
        version: text(row, 0).unwrap_or_default(),
        database: non_empty(row, 1),
        user: non_empty(row, 2),
        timezone: non_empty(row, 3),
        readonly: int(row, 4).and_then(|n| u8::try_from(n).ok()).unwrap_or(0),
    })
}

/// The oldest server the driver supports (lightweight DELETE).
pub const MIN_VERSION: (u32, u32) = (23, 3);

// --- Databases and tables --------------------------------------------------------------------

/// Databases ClickHouse keeps for itself (hidden unless asked for).
pub const SYSTEM_DATABASES: [&str; 3] = ["system", "INFORMATION_SCHEMA", "information_schema"];

pub const DATABASES: &str = "SELECT name, engine, comment, name = currentDatabase() AS is_current \
     FROM system.databases ORDER BY name";

pub fn parse_databases(rows: &[Vec<Value>]) -> Vec<DatabaseInfo> {
    rows.iter()
        .filter_map(|r| {
            Some(DatabaseInfo {
                name: text(r, 0)?,
                owner: None,
                encoding: None,
                is_current: flag(r, 3),
            })
        })
        .collect()
}

/// The databases as the schemas of the server's one database entry.
pub fn parse_schemas(rows: &[Vec<Value>]) -> Vec<SchemaInfo> {
    rows.iter()
        .filter_map(|r| {
            Some(SchemaInfo {
                name: text(r, 0)?,
                is_default: flag(r, 3),
            })
        })
        .collect()
}

/// `{db:String}`.
pub const TABLES: &str = "SELECT name, engine, total_rows, total_bytes, comment, is_temporary \
     FROM system.tables WHERE database = {db:String} ORDER BY name";

/// What a table's engine makes of it in the sidebar.
pub fn table_kind(engine: &str, database: &str) -> TableKind {
    match engine {
        "View" | "LiveView" | "WindowView" => TableKind::View,
        "MaterializedView" => TableKind::MaterializedView,
        e if e.starts_with("System") => TableKind::SystemView,
        _ if SYSTEM_DATABASES.contains(&database) => TableKind::SystemView,
        // Read through another server, a file, a stream or a dictionary's source.
        "MySQL"
        | "PostgreSQL"
        | "MongoDB"
        | "Redis"
        | "S3"
        | "URL"
        | "File"
        | "HDFS"
        | "JDBC"
        | "ODBC"
        | "SQLite"
        | "Kafka"
        | "RabbitMQ"
        | "NATS"
        | "S3Queue"
        | "AzureQueue"
        | "AzureBlobStorage"
        | "DeltaLake"
        | "Hudi"
        | "Hive"
        | "ExternalDistributed"
        | "ArrowFlight"
        | "BigQuery"
        | "Elasticsearch"
        | "YTsaurus"
        | "Paimon"
        | "FileLog"
        | "Executable"
        | "ExecutablePool"
        | "Dictionary"
        | "MeiliSearch"
        | "COSN"
        | "OSS"
        | "GCS" => TableKind::ForeignTable,
        e if e.starts_with("Iceberg") || e.starts_with("DeltaLake") || e.starts_with("Hudi") => {
            TableKind::ForeignTable
        }
        _ => TableKind::Table,
    }
}

pub fn parse_tables(database: &str, rows: &[Vec<Value>]) -> Vec<TableSummary> {
    rows.iter()
        .filter_map(|r| {
            let engine = text(r, 1).unwrap_or_default();
            Some(TableSummary {
                name: text(r, 0)?,
                schema: Some(database.to_string()),
                kind: table_kind(&engine, database),
                row_estimate: int(r, 2),
                size_bytes: int(r, 3),
                comment: non_empty(r, 4),
            })
        })
        .collect()
}

// --- One table -------------------------------------------------------------------------------

/// `{db:String}`, `{t:String}`.
pub const COLUMNS: &str = "SELECT name, type, position, default_kind, default_expression, \
     comment, is_in_primary_key, is_in_sorting_key, is_in_partition_key, is_in_sampling_key, \
     compression_codec, numeric_precision, numeric_scale \
     FROM system.columns WHERE database = {db:String} AND table = {t:String} ORDER BY position";

/// `{db:String}`, `{t:String}`.
pub const TABLE_INFO: &str = "SELECT engine, engine_full, partition_key, sorting_key, \
     primary_key, sampling_key, create_table_query, as_select, comment, total_rows \
     FROM system.tables WHERE database = {db:String} AND name = {t:String}";

/// `{db:String}`, `{t:String}`.
pub const SKIPPING_INDEXES: &str = "SELECT name, type, expr, granularity \
     FROM system.data_skipping_indices WHERE database = {db:String} AND table = {t:String} \
     ORDER BY name";

/// `{db:String}`, `{t:String}`. `system.projections` exists from 24.4; older servers have
/// none to show.
pub const PROJECTIONS: &str = "SELECT name, query FROM system.projections \
     WHERE database = {db:String} AND table = {t:String} ORDER BY name";

pub fn parse_columns(rows: &[Vec<Value>]) -> Vec<ColumnMeta> {
    rows.iter()
        .filter_map(|r| {
            let name = text(r, 0)?;
            let type_text = text(r, 1).unwrap_or_default();
            let ty = ChType::parse(&type_text);
            let default_kind = text(r, 3).unwrap_or_default().to_ascii_uppercase();
            let mut roles: Vec<&str> = Vec::new();
            for (i, role) in (6..10).zip(KEY_ROLES) {
                if flag(r, i) {
                    roles.push(role);
                }
            }
            let mut extra: Vec<String> = roles.iter().map(|s| (*s).to_string()).collect();
            if matches!(
                default_kind.as_str(),
                "MATERIALIZED" | "ALIAS" | "EPHEMERAL"
            ) {
                extra.push(default_kind.clone());
            }
            if let Some(codec) = non_empty(r, 10) {
                extra.push(codec);
            }
            let precision = i32_of(r, 11);
            let scale = i32_of(r, 12);
            let (precision, scale) = match ty.decimal() {
                Some((p, s)) => (Some(i32::from(p)), Some(i32::from(s))),
                None => (precision, scale),
            };
            Some(ColumnMeta {
                name,
                ordinal: int(r, 2).and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
                kind: ty.kind(),
                nullable: ty.is_nullable(),
                default_expr: non_empty(r, 4),
                is_pk: flag(r, 6),
                // Set from the key's order by `parse_table`.
                pk_ordinal: None,
                is_auto_increment: false,
                is_generated: matches!(default_kind.as_str(), "MATERIALIZED" | "ALIAS"),
                is_unsigned: ty.is_unsigned(),
                comment: non_empty(r, 5),
                enum_values: ty.enum_labels(),
                char_max_len: match ty.base() {
                    ChType::FixedString(n) => Some(i64::from(*n)),
                    _ => None,
                },
                numeric_precision: precision,
                numeric_scale: scale,
                collation: None,
                array_elem_type: match &ty {
                    ChType::Array(_) => array_element(&type_text),
                    _ => None,
                },
                extra: (!extra.is_empty()).then(|| extra.join(", ")),
                base_type: None,
                data_type: type_text,
            })
        })
        .collect()
}

/// `T` of `Array(T)`.
fn array_element(type_text: &str) -> Option<String> {
    let inner = type_text.trim().strip_prefix("Array(")?.strip_suffix(')')?;
    Some(inner.trim().to_string())
}

/// The parts of a key expression list (`a, toDate(b), c` → three), split at top-level commas.
pub fn key_parts(expr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut current = String::new();
    for c in expr.chars() {
        if let Some(q) = quote {
            current.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '`' | '"' => {
                quote = Some(c);
                current.push(c);
            }
            '(' | '[' => {
                depth += 1;
                current.push(c);
            }
            ')' | ']' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    out.push(current);
    out.into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// The column a key part names, when it is a plain (possibly quoted) column name.
fn plain_column(part: &str) -> Option<String> {
    let part = part.trim();
    for q in ['`', '"'] {
        if let Some(inner) = part.strip_prefix(q).and_then(|p| p.strip_suffix(q)) {
            let mut out = String::new();
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    out.push(chars.next()?);
                } else {
                    out.push(c);
                }
            }
            return Some(out);
        }
    }
    let plain = part
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.');
    (plain && !part.is_empty()).then(|| part.to_string())
}

/// A table's structure from its `TABLE_INFO` row and its columns, skipping indexes and
/// projections. The row identity is the primary key's plain columns in key order; where the key
/// holds an expression, the columns it uses, in table order.
pub fn parse_table(
    table: &TableRef,
    info: &[Vec<Value>],
    columns: &[Vec<Value>],
    skipping: &[Vec<Value>],
    projections: &[Vec<Value>],
) -> Result<TableMeta, AppError> {
    let info = first_row(info, &format!("table {}", table.name))?;
    let engine = text(info, 0).unwrap_or_default();
    let database = table.schema.clone().unwrap_or_default();
    let mut cols = parse_columns(columns);
    let primary_expr = non_empty(info, 4);
    let parts = primary_expr.as_deref().map(key_parts).unwrap_or_default();
    let plain: Option<Vec<String>> = parts.iter().map(|p| plain_column(p)).collect();
    let mut primary_key: Vec<String> = match plain {
        Some(names) if names.iter().all(|n| cols.iter().any(|c| &c.name == n)) => names,
        _ => cols
            .iter()
            .filter(|c| c.is_pk)
            .map(|c| c.name.clone())
            .collect(),
    };
    primary_key.dedup();
    for c in &mut cols {
        c.pk_ordinal = primary_key
            .iter()
            .position(|n| n == &c.name)
            .and_then(|i| u32::try_from(i + 1).ok());
        c.is_pk = c.pk_ordinal.is_some();
    }
    let mut indexes = Vec::new();
    let mut key_index = |name: &str, expr: Option<String>, clause: &str, primary: bool| {
        if let Some(expr) = expr {
            let names: Vec<String> = key_parts(&expr)
                .iter()
                .filter_map(|p| plain_column(p))
                .collect();
            indexes.push(IndexMeta {
                name: name.to_string(),
                columns: names,
                unique: false,
                primary,
                method: Some(clause.to_ascii_lowercase()),
                predicate: None,
                definition: Some(format!("{clause} ({expr})")),
                comment: None,
                include: Vec::new(),
            });
        }
    };
    key_index("PRIMARY", primary_expr.clone(), "PRIMARY KEY", true);
    let sorting = non_empty(info, 3).filter(|s| Some(s) != primary_expr.as_ref());
    key_index("ORDER BY", sorting, "ORDER BY", false);
    key_index("PARTITION BY", non_empty(info, 2), "PARTITION BY", false);
    key_index("SAMPLE BY", non_empty(info, 5), "SAMPLE BY", false);
    for r in skipping {
        let Some(name) = text(r, 0) else { continue };
        let ty = text(r, 1).unwrap_or_default();
        let expr = text(r, 2).unwrap_or_default();
        let granularity = int(r, 3).unwrap_or(1);
        indexes.push(IndexMeta {
            name: name.clone(),
            columns: key_parts(&expr)
                .iter()
                .filter_map(|p| plain_column(p))
                .collect(),
            unique: false,
            primary: false,
            method: Some(ty.clone()),
            predicate: None,
            definition: Some(format!(
                "INDEX {} {expr} TYPE {ty} GRANULARITY {granularity}",
                quote_name(&name)
            )),
            comment: None,
            include: Vec::new(),
        });
    }
    for r in projections {
        let Some(name) = text(r, 0) else { continue };
        indexes.push(IndexMeta {
            name: name.clone(),
            columns: Vec::new(),
            unique: false,
            primary: false,
            method: Some("projection".into()),
            predicate: None,
            definition: text(r, 1).map(|q| format!("PROJECTION {} ({q})", quote_name(&name))),
            comment: None,
            include: Vec::new(),
        });
    }
    Ok(TableMeta {
        table: table.clone(),
        kind: table_kind(&engine, &database),
        columns: cols,
        primary_key,
        indexes,
        foreign_keys: Vec::new(),
        triggers: Vec::new(),
        comment: non_empty(info, 8),
        row_estimate: int(info, 9),
        ddl: non_empty(info, 6),
        checks: Vec::new(),
        options: Some(TableOptions {
            engine: non_empty(info, 1).or(Some(engine).filter(|e| !e.is_empty())),
            sort_key: non_empty(info, 3),
            ..TableOptions::default()
        }),
    })
}

/// A name for display in an index definition (the catalog's own text), backquoted.
fn quote_name(name: &str) -> String {
    crate::sqlgen::dialect::spec::backticked_backslash(name)
}

/// The SELECT of a view (`as_select`), or its whole `CREATE` text.
pub fn view_definition(info: &[Vec<Value>]) -> Option<String> {
    let row = info.first()?;
    non_empty(row, 7).or_else(|| non_empty(row, 6))
}

// --- Editor --------------------------------------------------------------------------------------

pub const COMPLETION: &str = "SELECT database, table, name FROM system.columns \
     WHERE database NOT IN ('system', 'INFORMATION_SCHEMA', 'information_schema') \
     ORDER BY database, table, position";

pub fn parse_completion(default: Option<&str>, rows: &[Vec<Value>]) -> CompletionMetadata {
    let mut out = CompletionMetadata {
        default_schema: default.map(str::to_string),
        ..CompletionMetadata::default()
    };
    for r in rows {
        let (Some(db), Some(t), Some(c)) = (text(r, 0), text(r, 1), text(r, 2)) else {
            continue;
        };
        out.schemas
            .entry(db)
            .or_default()
            .entry(t)
            .or_default()
            .push(c);
    }
    out
}

/// `{db:String}`.
pub const DICTIONARIES: &str = "SELECT name, status, type, comment FROM system.dictionaries \
     WHERE database = {db:String} ORDER BY name";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dictionary {
    pub name: String,
    pub status: Option<String>,
    pub layout: Option<String>,
    pub comment: Option<String>,
}

pub fn parse_dictionaries(rows: &[Vec<Value>]) -> Vec<Dictionary> {
    rows.iter()
        .filter_map(|r| {
            Some(Dictionary {
                name: text(r, 0)?,
                status: non_empty(r, 1),
                layout: non_empty(r, 2),
                comment: non_empty(r, 3),
            })
        })
        .collect()
}

pub const USER_FUNCTIONS: &str = "SELECT name, create_query FROM system.functions \
     WHERE origin = 'SQLUserDefined' ORDER BY name";

/// `(name, CREATE FUNCTION text)`.
pub fn parse_user_functions(rows: &[Vec<Value>]) -> Vec<(String, String)> {
    rows.iter()
        .filter_map(|r| Some((text(r, 0)?, text(r, 1).unwrap_or_default())))
        .collect()
}

// --- Administration ------------------------------------------------------------------------------

pub const PROCESSES: &str = "SELECT query_id, user, address, elapsed, read_rows, memory_usage, \
     current_database, query FROM system.processes ORDER BY elapsed DESC";

#[derive(Debug, Clone, PartialEq)]
pub struct Process {
    /// What `KILL QUERY` takes.
    pub query_id: String,
    pub user: Option<String>,
    pub address: Option<String>,
    pub elapsed_secs: Option<f64>,
    pub read_rows: Option<i64>,
    pub memory_bytes: Option<i64>,
    pub database: Option<String>,
    pub query: Option<String>,
}

pub fn parse_processes(rows: &[Vec<Value>]) -> Vec<Process> {
    rows.iter()
        .filter_map(|r| {
            Some(Process {
                query_id: text(r, 0)?,
                user: non_empty(r, 1),
                address: non_empty(r, 2),
                elapsed_secs: match cell(r, 3) {
                    Some(Value::Float(f)) => Some(*f),
                    _ => int(r, 3).map(|n| n as f64),
                },
                read_rows: int(r, 4),
                memory_bytes: int(r, 5),
                database: non_empty(r, 6),
                query: text(r, 7),
            })
        })
        .collect()
}

/// `{db:String}`, `{t:String}`: mutations still running (or stuck) on a table.
pub const MUTATIONS: &str = "SELECT mutation_id, command, create_time, latest_fail_reason, \
     parts_to_do FROM system.mutations \
     WHERE database = {db:String} AND table = {t:String} AND is_done = 0 ORDER BY create_time";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mutation {
    pub id: String,
    pub command: String,
    pub created: Option<String>,
    pub fail_reason: Option<String>,
    pub parts_to_do: Option<i64>,
}

pub fn parse_mutations(rows: &[Vec<Value>]) -> Vec<Mutation> {
    rows.iter()
        .filter_map(|r| {
            Some(Mutation {
                id: text(r, 0)?,
                command: text(r, 1).unwrap_or_default(),
                created: text(r, 2),
                fail_reason: non_empty(r, 3),
                parts_to_do: int(r, 4),
            })
        })
        .collect()
}

/// The count of a `guard_count` query.
pub fn parse_count(rows: &[Vec<Value>]) -> Result<u64, AppError> {
    let row = first_row(rows, "the row count")?;
    int(row, 0)
        .and_then(|n| u64::try_from(n).ok())
        .or_else(|| match cell(row, 0) {
            Some(Value::BigInt(s)) => s.parse().ok(),
            _ => None,
        })
        .ok_or_else(|| AppError::db("ClickHouse sent a count Tabler cannot read"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::schema::ValueKind;

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn server_info_and_versions() {
        let info = parse_server_info(&[vec![
            t("25.8.2.29"),
            t("default"),
            t("alice"),
            t("Asia/Jakarta"),
            t("2"),
        ]])
        .unwrap();
        assert_eq!(info.release(), Some((25, 8)));
        assert!(info.has_lightweight_update() && info.has_role_parameter());
        assert!(!info.format_header_wins());
        assert_eq!(info.readonly, 2);
        assert_eq!(info.label(), "ClickHouse 25.8.2.29");
        let old = ClickhouseServerInfo {
            version: "23.3.1".into(),
            ..ClickhouseServerInfo::default()
        };
        assert!(old.at_least(MIN_VERSION.0, MIN_VERSION.1) && !old.has_lightweight_update());
        assert_eq!(ClickhouseServerInfo::default().release(), None);
        assert!(parse_server_info(&[]).is_err());
    }

    #[test]
    fn databases_and_tables() {
        let rows = vec![
            vec![t("default"), t("Atomic"), t(""), Value::Int(1)],
            vec![t("system"), t("Atomic"), Value::Null, Value::Int(0)],
            vec![Value::Null],
        ];
        let dbs = parse_databases(&rows);
        assert_eq!(dbs.len(), 2);
        assert!(dbs[0].is_current && !dbs[1].is_current);
        assert!(parse_schemas(&rows)[0].is_default);
        let tables = parse_tables(
            "default",
            &[
                vec![
                    t("events"),
                    t("ReplicatedMergeTree"),
                    Value::BigInt("9007199254740993".into()),
                    Value::Int(10),
                    t("hits"),
                    Value::Int(0),
                ],
                vec![
                    t("v"),
                    t("View"),
                    Value::Null,
                    Value::Null,
                    t(""),
                    Value::Int(0),
                ],
                vec![
                    t("mv"),
                    t("MaterializedView"),
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Int(0),
                ],
                vec![
                    t("pg"),
                    t("PostgreSQL"),
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Int(0),
                ],
                vec![
                    t("ice"),
                    t("IcebergS3"),
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Int(0),
                ],
                vec![
                    t("d"),
                    t("Dictionary"),
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Int(0),
                ],
            ],
        );
        let kinds: Vec<TableKind> = tables.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            [
                TableKind::Table,
                TableKind::View,
                TableKind::MaterializedView,
                TableKind::ForeignTable,
                TableKind::ForeignTable,
                TableKind::ForeignTable
            ]
        );
        assert_eq!(tables[0].row_estimate, Some(9007199254740993));
        assert_eq!(tables[0].comment.as_deref(), Some("hits"));
        assert_eq!(tables[1].comment, None);
        assert_eq!(table_kind("SystemNumbers", "system"), TableKind::SystemView);
        assert_eq!(table_kind("MergeTree", "system"), TableKind::SystemView);
    }

    fn column_row(name: &str, ty: &str, pos: i64, kind: &str, keys: [i64; 4]) -> Vec<Value> {
        vec![
            t(name),
            t(ty),
            Value::Int(pos),
            t(kind),
            if kind.is_empty() { t("") } else { t("now()") },
            t(""),
            Value::Int(keys[0]),
            Value::Int(keys[1]),
            Value::Int(keys[2]),
            Value::Int(keys[3]),
            t(""),
            Value::Null,
            Value::Null,
        ]
    }

    #[test]
    fn a_table_with_keys_indexes_and_projections() {
        let table = TableRef::new(Some("default"), "events");
        let info = vec![vec![
            t("MergeTree"),
            t(
                "MergeTree PARTITION BY toYYYYMM(ts) ORDER BY (tenant, `user id`, ts) SETTINGS index_granularity = 8192",
            ),
            t("toYYYYMM(ts)"),
            t("tenant, `user id`, ts"),
            t("tenant, `user id`"),
            t(""),
            t("CREATE TABLE default.events (…) ENGINE = MergeTree …"),
            t(""),
            t("click events"),
            Value::Int(42),
        ]];
        let columns = vec![
            column_row("ts", "DateTime64(3, 'UTC')", 1, "", [0, 1, 1, 0]),
            column_row("user id", "UInt64", 2, "", [1, 1, 0, 0]),
            column_row("tenant", "LowCardinality(String)", 3, "", [1, 1, 0, 0]),
            column_row(
                "amount",
                "Nullable(Decimal(18, 4))",
                4,
                "DEFAULT",
                [0, 0, 0, 0],
            ),
            column_row("total", "UInt64", 5, "MATERIALIZED", [0, 0, 0, 0]),
            column_row("tags", "Array(LowCardinality(String))", 6, "", [0, 0, 0, 0]),
            column_row("kind", "Enum8('a' = 1, 'b' = 2)", 7, "", [0, 0, 0, 0]),
        ];
        let skipping = vec![vec![
            t("idx_amount"),
            t("minmax"),
            t("amount"),
            Value::Int(4),
        ]];
        let projections = vec![vec![t("by_user"), t("SELECT * ORDER BY `user id`")]];
        let meta = parse_table(&table, &info, &columns, &skipping, &projections).unwrap();
        // Key order, not table order.
        assert_eq!(meta.primary_key, ["tenant", "user id"]);
        let by = |n: &str| meta.columns.iter().find(|c| c.name == n).unwrap();
        assert_eq!(by("tenant").pk_ordinal, Some(1));
        assert_eq!(by("user id").pk_ordinal, Some(2));
        assert!(!by("ts").is_pk);
        assert_eq!(
            by("ts").extra.as_deref(),
            Some("sorting key, partition key")
        );
        assert_eq!(by("ts").kind, ValueKind::Timestamp);
        assert!(by("amount").nullable);
        assert_eq!(by("amount").numeric_precision, Some(18));
        assert_eq!(by("amount").numeric_scale, Some(4));
        assert_eq!(by("amount").default_expr.as_deref(), Some("now()"));
        assert!(by("total").is_generated);
        assert_eq!(
            by("tags").array_elem_type.as_deref(),
            Some("LowCardinality(String)")
        );
        assert_eq!(by("kind").enum_values.as_ref().unwrap(), &["a", "b"]);
        assert_eq!(by("tenant").kind, ValueKind::Text);
        let names: Vec<&str> = meta.indexes.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "PRIMARY",
                "ORDER BY",
                "PARTITION BY",
                "idx_amount",
                "by_user"
            ]
        );
        assert_eq!(meta.indexes[0].columns, ["tenant", "user id"]);
        assert!(meta.indexes[0].primary);
        assert_eq!(meta.indexes[2].columns, Vec::<String>::new());
        assert_eq!(
            meta.indexes[3].definition.as_deref(),
            Some("INDEX `idx_amount` amount TYPE minmax GRANULARITY 4")
        );
        assert_eq!(meta.comment.as_deref(), Some("click events"));
        assert_eq!(meta.row_estimate, Some(42));
        assert!(meta.ddl.as_deref().unwrap().starts_with("CREATE TABLE"));
        let options = meta.options.unwrap();
        assert!(
            options
                .engine
                .unwrap()
                .starts_with("MergeTree PARTITION BY")
        );
        // The sorting key as the server spells it.
        assert_eq!(options.sort_key.as_deref(), Some("tenant, `user id`, ts"));
        assert!(parse_table(&table, &[], &columns, &[], &[]).is_err());
    }

    #[test]
    fn keys_of_expressions_fall_back_to_their_columns() {
        let table = TableRef::new(Some("db"), "t");
        let info = vec![vec![
            t("MergeTree"),
            t(""),
            t(""),
            t("cityHash64(id), ts"),
            t("cityHash64(id), ts"),
            t("cityHash64(id)"),
            t(""),
            t(""),
            t(""),
            Value::Null,
        ]];
        let columns = vec![
            column_row("id", "UInt64", 1, "", [1, 1, 0, 1]),
            column_row("ts", "DateTime", 2, "", [1, 1, 0, 0]),
        ];
        let meta = parse_table(&table, &info, &columns, &[], &[]).unwrap();
        assert_eq!(meta.primary_key, ["id", "ts"]);
        assert_eq!(meta.indexes.len(), 2, "{:?}", meta.indexes);
        assert_eq!(meta.indexes[1].name, "SAMPLE BY");
        // A table without a key cannot be edited.
        let info = vec![vec![
            t("Log"),
            t("Log"),
            t(""),
            t(""),
            t(""),
            t(""),
            t(""),
            t(""),
            t(""),
            Value::Null,
        ]];
        let columns = vec![column_row("a", "String", 1, "", [0, 0, 0, 0])];
        let meta = parse_table(&table, &info, &columns, &[], &[]).unwrap();
        assert!(meta.primary_key.is_empty() && meta.indexes.is_empty());
        assert_eq!(meta.options.and_then(|o| o.sort_key), None);
        assert_eq!(
            key_parts("a, (b, c), f('x,y'), `d,e`"),
            ["a", "(b, c)", "f('x,y')", "`d,e`"]
        );
        assert_eq!(plain_column("`a\\`b`").as_deref(), Some("a`b"));
        assert_eq!(plain_column("f(x)"), None);
    }

    #[test]
    fn views_completion_and_administration() {
        let info = vec![vec![
            t("View"),
            t("View"),
            t(""),
            t(""),
            t(""),
            t(""),
            t("CREATE VIEW db.v AS SELECT 1"),
            t("SELECT 1"),
            t(""),
            Value::Null,
        ]];
        assert_eq!(view_definition(&info).as_deref(), Some("SELECT 1"));
        let completion = parse_completion(
            Some("default"),
            &[
                vec![t("default"), t("a"), t("x")],
                vec![t("default"), t("a"), t("y")],
                vec![t("other"), t("b"), t("z")],
                vec![Value::Null, t("c"), t("w")],
            ],
        );
        assert_eq!(completion.schemas["default"]["a"], ["x", "y"]);
        assert_eq!(completion.schemas.len(), 2);
        let procs = parse_processes(&[vec![
            t("q-1"),
            t("alice"),
            t("::ffff:10.0.0.1"),
            Value::Float(1.5),
            Value::Int(10),
            Value::BigInt("9007199254740993".into()),
            t("default"),
            t("SELECT sleep(3)"),
        ]]);
        assert_eq!(procs[0].query_id, "q-1");
        assert_eq!(procs[0].elapsed_secs, Some(1.5));
        assert_eq!(procs[0].memory_bytes, Some(9007199254740993));
        let muts = parse_mutations(&[vec![
            t("mutation_3.txt"),
            t("UPDATE a = 1 WHERE id = 7"),
            t("2024-01-01 00:00:00"),
            t(""),
            Value::Int(2),
        ]]);
        assert_eq!(muts[0].parts_to_do, Some(2));
        assert_eq!(muts[0].fail_reason, None);
        assert_eq!(parse_count(&[vec![Value::Int(1)]]).unwrap(), 1);
        assert_eq!(
            parse_count(&[vec![Value::BigInt("18446744073709551615".into())]]).unwrap(),
            u64::MAX
        );
        assert!(parse_count(&[vec![t("x")]]).is_err());
        assert!(parse_count(&[]).is_err());
        let dicts = parse_dictionaries(&[vec![t("geo"), t("LOADED"), t("Hashed"), Value::Null]]);
        assert_eq!(dicts[0].status.as_deref(), Some("LOADED"));
        assert_eq!(
            parse_user_functions(&[vec![t("f"), t("CREATE FUNCTION f AS x -> x")]])[0].0,
            "f"
        );
    }
}
