//! SQL Server catalog queries (`sys.*` of the current database) and the parsers of their rows.
//!
//! The queries take their names as bind parameters (`@P1`): a schema name for `SCHEMA_ID`, or a
//! table's quoted, qualified name for `OBJECT_ID` (`dialect::tsql::object_name`), so a dot or a
//! bracket in a name stays part of it. The parsers read rows as the decoders produce them, which
//! lets them be tested without a server. Every query works from SQL Server 2012 on and needs no
//! server-level permission (row counts come from `sys.partitions`, not the DMVs).

use std::collections::BTreeMap;

use crate::error::AppError;
use crate::types::admin::ProcessRow;
use crate::types::schema::{
    CheckMeta, ColumnMeta, CompletionMetadata, DatabaseInfo, ForeignKeyMeta, IndexMeta, SchemaInfo,
    TableKind, TableMeta, TableRef, TableSummary, TriggerMeta, ValueKind,
};
use crate::types::value::Value;

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

/// An integer cell (int, bigint, decimal without decimals, bit).
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

/// A bit cell (or an int one); NULL is false.
pub fn flag(row: &[Value], i: usize) -> bool {
    int(row, i).is_some_and(|n| n != 0)
}

// --- Server --------------------------------------------------------------------------------------

/// Who and where the session is. `SERVERPROPERTY` values are `sql_variant`s, cast to plain types.
pub const SERVER_INFO: &str = "SELECT CAST(SERVERPROPERTY('ProductVersion') AS NVARCHAR(128)), \
     @@VERSION, CAST(SERVERPROPERTY('EngineEdition') AS INT), \
     CAST(SERVERPROPERTY('Edition') AS NVARCHAR(128)), DB_NAME(), SUSER_SNAME(), SCHEMA_NAME(), \
     @@SPID";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MssqlServerInfo {
    /// `16.0.4135.4`
    pub product_version: String,
    /// `@@VERSION`, several lines.
    pub version_text: String,
    /// 1–4 boxed editions, 5 Azure SQL Database, 6 and 11 Synapse, 8 Managed Instance, 9 SQL Edge.
    pub engine_edition: Option<i64>,
    pub edition: Option<String>,
    pub database: Option<String>,
    pub login: Option<String>,
    pub default_schema: Option<String>,
    pub spid: Option<i64>,
}

impl MssqlServerInfo {
    pub fn major(&self) -> Option<u32> {
        self.product_version.split('.').next()?.parse().ok()
    }

    /// Azure SQL Database: a connection belongs to one database (no `USE`).
    pub fn is_azure_database(&self) -> bool {
        self.engine_edition == Some(5)
    }

    /// What the status pill shows: `SQL Server 2022 (16.0.4135.4)`, `Azure SQL Database (…)`.
    pub fn label(&self) -> String {
        let product = match self.engine_edition {
            Some(5) => "Azure SQL Database".to_string(),
            Some(6) => "Azure Synapse Analytics".to_string(),
            Some(8) => "Azure SQL Managed Instance".to_string(),
            Some(9) => "Azure SQL Edge".to_string(),
            Some(11) => "Azure Synapse serverless SQL pool".to_string(),
            _ => match self.major().and_then(release_year) {
                Some(year) => format!("SQL Server {year}"),
                None => "SQL Server".to_string(),
            },
        };
        if self.product_version.is_empty() {
            product
        } else {
            format!("{product} ({})", self.product_version)
        }
    }
}

/// The release a major version number belongs to.
fn release_year(major: u32) -> Option<&'static str> {
    Some(match major {
        10 => "2008",
        11 => "2012",
        12 => "2014",
        13 => "2016",
        14 => "2017",
        15 => "2019",
        16 => "2022",
        17 => "2025",
        _ => return None,
    })
}

pub fn parse_server_info(rows: &[Vec<Value>]) -> Result<MssqlServerInfo, AppError> {
    let row = rows
        .first()
        .ok_or_else(|| AppError::Internal("SQL Server sent no server information".into()))?;
    Ok(MssqlServerInfo {
        product_version: text(row, 0).unwrap_or_default(),
        version_text: text(row, 1).unwrap_or_default(),
        engine_edition: int(row, 2),
        edition: text(row, 3),
        database: text(row, 4),
        login: text(row, 5),
        default_schema: text(row, 6),
        spid: int(row, 7),
    })
}

/// The editor's transaction depth: `dialect::tsql::TRANCOUNT`.
pub fn parse_trancount(rows: &[Vec<Value>]) -> Option<bool> {
    rows.first().and_then(|r| int(r, 0)).map(|n| n > 0)
}

// --- Databases and schemas -----------------------------------------------------------------------

pub const DATABASES: &str = "SELECT name, SUSER_SNAME(owner_sid), collation_name, \
     CASE WHEN name = DB_NAME() THEN 1 ELSE 0 END FROM sys.databases \
     WHERE HAS_DBACCESS(name) = 1 ORDER BY name";

pub fn parse_databases(rows: &[Vec<Value>]) -> Vec<DatabaseInfo> {
    rows.iter()
        .filter_map(|r| {
            Some(DatabaseInfo {
                name: text(r, 0)?,
                owner: text(r, 1),
                encoding: text(r, 2),
                is_current: flag(r, 3),
            })
        })
        .collect()
}

/// The schemas of the current database, without the catalog ones, guest and the fixed database
/// roles' schemas (ids 16384–16399).
pub const SCHEMAS: &str = "SELECT name, CASE WHEN name = SCHEMA_NAME() THEN 1 ELSE 0 END \
     FROM sys.schemas WHERE schema_id NOT BETWEEN 16384 AND 16399 \
     AND name NOT IN (N'sys', N'INFORMATION_SCHEMA', N'guest') ORDER BY name";

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

// --- Tables --------------------------------------------------------------------------------------

/// The rows of the object `o` (its heap or clustered index), from `sys.partitions` alone: joined
/// to its allocation units (in-row, LOB, row-overflow), a partition would count its rows two or
/// three times. TABLES and TABLE share it, so the sidebar and the structure view agree.
macro_rules! rows_of_o {
    () => {
        "(SELECT SUM(pa.rows) FROM sys.partitions pa WHERE pa.object_id = o.object_id AND \
         pa.index_id IN (0, 1))"
    };
}

/// The pages of the object `o` and its indexes. Each allocation unit belongs to one partition:
/// in-row and row-overflow data to its hobt, LOB data to the partition itself.
macro_rules! pages_of_o {
    () => {
        "(SELECT SUM(au.total_pages) FROM sys.partitions pa JOIN sys.allocation_units au ON \
         (au.type IN (1, 3) AND au.container_id = pa.hobt_id) OR \
         (au.type = 2 AND au.container_id = pa.partition_id) WHERE pa.object_id = o.object_id)"
    };
}

/// Tables and views of the schema `@P1`, with their row counts and sizes from `sys.partitions`
/// and `sys.allocation_units`, which need no VIEW DATABASE STATE.
pub const TABLES: &str = concat!(
    "SELECT o.name, o.type, CAST(ep.value AS NVARCHAR(MAX)), ",
    rows_of_o!(),
    ", ",
    pages_of_o!(),
    " * 8192 FROM sys.objects o \
     LEFT JOIN sys.extended_properties ep ON ep.class = 1 AND ep.major_id = o.object_id AND \
     ep.minor_id = 0 AND ep.name = N'MS_Description' \
     WHERE o.schema_id = SCHEMA_ID(@P1) AND o.type IN ('U', 'V') AND o.is_ms_shipped = 0 \
     ORDER BY o.name"
);

fn kind_of_object(code: &str) -> TableKind {
    match code.trim() {
        "V" => TableKind::View,
        _ => TableKind::Table,
    }
}

pub fn parse_tables(rows: &[Vec<Value>], schema: &str) -> Vec<TableSummary> {
    rows.iter()
        .filter_map(|r| {
            Some(TableSummary {
                name: text(r, 0)?,
                schema: Some(schema.to_string()),
                kind: kind_of_object(&text(r, 1).unwrap_or_default()),
                row_estimate: int(r, 3),
                size_bytes: int(r, 4),
                comment: non_empty(r, 2),
            })
        })
        .collect()
}

/// One table `@P1`: kind, comment and row count.
pub const TABLE: &str = concat!(
    "SELECT o.type, CAST(ep.value AS NVARCHAR(MAX)), ",
    rows_of_o!(),
    " FROM sys.objects o \
     LEFT JOIN sys.extended_properties ep ON ep.class = 1 AND ep.major_id = o.object_id AND \
     ep.minor_id = 0 AND ep.name = N'MS_Description' \
     WHERE o.object_id = OBJECT_ID(@P1) AND o.type IN ('U', 'V')"
);

#[derive(Debug, Clone, PartialEq)]
pub struct TableInfo {
    pub kind: TableKind,
    pub comment: Option<String>,
    pub row_estimate: Option<i64>,
}

pub fn parse_table(rows: &[Vec<Value>], table: &TableRef) -> Result<TableInfo, AppError> {
    let row = rows.first().ok_or_else(|| {
        AppError::NotFound(match &table.schema {
            Some(s) => format!("table {s}.{}", table.name),
            None => format!("table {}", table.name),
        })
    })?;
    Ok(TableInfo {
        kind: kind_of_object(&text(row, 0).unwrap_or_default()),
        comment: non_empty(row, 1),
        row_estimate: int(row, 2),
    })
}

// --- Columns -------------------------------------------------------------------------------------

/// The columns of `@P1`. `base_type` is the system type of an alias type; COLUMNPROPERTY keeps the
/// temporal-table flag readable on servers before 2016, where the column does not exist.
pub const COLUMNS: &str = "SELECT c.column_id, c.name, t.name, \
     CASE WHEN t.is_user_defined = 1 AND t.is_assembly_type = 0 THEN TYPE_NAME(t.system_type_id) \
     END, c.max_length, c.precision, c.scale, c.is_nullable, c.is_identity, c.is_computed, \
     COLUMNPROPERTY(c.object_id, c.name, 'GeneratedAlwaysType'), c.collation_name, d.definition, \
     cc.definition, cc.is_persisted, CAST(ic.seed_value AS NVARCHAR(40)), \
     CAST(ic.increment_value AS NVARCHAR(40)), CAST(ep.value AS NVARCHAR(MAX)), \
     t.is_assembly_type \
     FROM sys.columns c JOIN sys.types t ON t.user_type_id = c.user_type_id \
     LEFT JOIN sys.default_constraints d ON d.object_id = c.default_object_id \
     LEFT JOIN sys.computed_columns cc ON cc.object_id = c.object_id AND cc.column_id = \
     c.column_id \
     LEFT JOIN sys.identity_columns ic ON ic.object_id = c.object_id AND ic.column_id = \
     c.column_id \
     LEFT JOIN sys.extended_properties ep ON ep.class = 1 AND ep.major_id = c.object_id AND \
     ep.minor_id = c.column_id AND ep.name = N'MS_Description' \
     WHERE c.object_id = OBJECT_ID(@P1) ORDER BY c.column_id";

/// The kind of values of a system type (or of a CLR type's, `assembly`).
pub fn kind_of_type(name: &str, assembly: bool) -> ValueKind {
    match name.to_ascii_lowercase().as_str() {
        "bit" => ValueKind::Bool,
        "tinyint" | "smallint" | "int" => ValueKind::Int,
        "bigint" => ValueKind::BigInt,
        "decimal" | "numeric" | "money" | "smallmoney" => ValueKind::Decimal,
        "float" | "real" => ValueKind::Float,
        "char" | "varchar" | "nchar" | "nvarchar" | "text" | "ntext" | "sysname" | "xml" => {
            ValueKind::Text
        }
        "binary" | "varbinary" | "image" | "timestamp" | "rowversion" => ValueKind::Bytes,
        "date" => ValueKind::Date,
        "time" => ValueKind::Time,
        "datetime" | "datetime2" | "smalldatetime" => ValueKind::Timestamp,
        "datetimeoffset" => ValueKind::TimestampTz,
        "uniqueidentifier" => ValueKind::Uuid,
        "json" => ValueKind::Json,
        "geometry" | "geography" => ValueKind::Geometry,
        "hierarchyid" | "sql_variant" | "vector" => ValueKind::Other,
        // Other CLR types arrive as their serialization.
        _ if assembly => ValueKind::Bytes,
        _ => ValueKind::Other,
    }
}

/// A type as SQL Server spells it in DDL: `nvarchar(50)`, `varbinary(max)`, `decimal(10,2)`,
/// `datetime2(3)`. `max_length` is in bytes (-1 for max), halved for the n-types.
pub fn full_type(
    name: &str,
    max_length: Option<i64>,
    precision: Option<i64>,
    scale: Option<i64>,
) -> String {
    let lower = name.to_ascii_lowercase();
    let len = |chars: Option<i64>| match chars {
        Some(-1) => "max".to_string(),
        Some(n) => n.to_string(),
        None => "max".to_string(),
    };
    match lower.as_str() {
        "char" | "varchar" | "binary" | "varbinary" => format!("{lower}({})", len(max_length)),
        "nchar" | "nvarchar" => format!(
            "{lower}({})",
            len(max_length.map(|n| if n > 0 { n / 2 } else { n }))
        ),
        "decimal" | "numeric" => format!(
            "{lower}({},{})",
            precision.unwrap_or(18),
            scale.unwrap_or(0)
        ),
        "datetime2" | "time" | "datetimeoffset" => format!("{lower}({})", scale.unwrap_or(7)),
        _ => name.to_string(),
    }
}

/// Characters a text column holds (None for max and the other types).
fn char_max_len(name: &str, max_length: Option<i64>) -> Option<i64> {
    let n = max_length.filter(|&n| n > 0)?;
    match name.to_ascii_lowercase().as_str() {
        "char" | "varchar" | "binary" | "varbinary" => Some(n),
        "nchar" | "nvarchar" => Some(n / 2),
        _ => None,
    }
}

/// A default or computed definition without the parentheses SQL Server wraps it in: `((0))` is
/// `0`, `(getdate())` is `getdate()`; `(a)+(b)` stays as it is.
pub fn strip_parens(definition: &str) -> String {
    let mut s = definition.trim();
    while s.starts_with('(') && s.ends_with(')') && wraps_whole(s) {
        s = s[1..s.len() - 1].trim();
    }
    s.to_string()
}

/// Whether the `(` at the start of `s` closes at its last character, quotes skipped.
fn wraps_whole(s: &str) -> bool {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let last = s.chars().count() - 1;
    for (i, c) in s.chars().enumerate() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' => quote = Some('\''),
                '[' => quote = Some(']'),
                '"' => quote = Some('"'),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 && i != last {
                        return false;
                    }
                }
                _ => {}
            },
        }
    }
    depth == 0
}

/// The columns, without key flags (`table_meta` sets them from the primary key).
pub fn parse_columns(rows: &[Vec<Value>]) -> Vec<ColumnMeta> {
    rows.iter().filter_map(|r| column(r)).collect()
}

fn column(r: &[Value]) -> Option<ColumnMeta> {
    let name = text(r, 1)?;
    let type_name = text(r, 2).unwrap_or_else(|| "sql_variant".into());
    let base = text(r, 3);
    let system = base.clone().unwrap_or_else(|| type_name.clone());
    let assembly = flag(r, 18);
    let (max_length, precision, scale) = (int(r, 4), int(r, 5), int(r, 6));
    let identity = flag(r, 8);
    let computed = flag(r, 9);
    let generated_always = int(r, 10).unwrap_or(0);
    let lower = system.to_ascii_lowercase();
    let data_type = if base.is_some() {
        type_name.clone()
    } else {
        full_type(&type_name, max_length, precision, scale)
    };
    let mut extra = Vec::new();
    if identity {
        extra.push(format!(
            "IDENTITY({},{})",
            text(r, 15).unwrap_or_else(|| "1".into()),
            text(r, 16).unwrap_or_else(|| "1".into())
        ));
    }
    if computed {
        let def = text(r, 13).map(|d| strip_parens(&d)).unwrap_or_default();
        extra.push(if flag(r, 14) {
            format!("AS ({def}) PERSISTED")
        } else {
            format!("AS ({def})")
        });
    }
    match generated_always {
        1 => extra.push("GENERATED ALWAYS AS ROW START".into()),
        2 => extra.push("GENERATED ALWAYS AS ROW END".into()),
        _ => {}
    }
    let numeric = matches!(lower.as_str(), "decimal" | "numeric");
    Some(ColumnMeta {
        name,
        ordinal: int(r, 0).and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
        data_type,
        kind: kind_of_type(&system, assembly),
        nullable: flag(r, 7),
        default_expr: text(r, 12).map(|d| strip_parens(&d)),
        is_pk: false,
        pk_ordinal: None,
        is_auto_increment: identity,
        // Values the server writes: computed, rowversion and the period columns.
        is_generated: computed
            || generated_always != 0
            || matches!(lower.as_str(), "timestamp" | "rowversion"),
        is_unsigned: lower == "tinyint",
        comment: non_empty(r, 17),
        enum_values: None,
        char_max_len: if base.is_some() {
            None
        } else {
            char_max_len(&type_name, max_length)
        },
        numeric_precision: numeric
            .then(|| precision.and_then(|n| i32::try_from(n).ok()))
            .flatten(),
        numeric_scale: numeric
            .then(|| scale.and_then(|n| i32::try_from(n).ok()))
            .flatten(),
        collation: text(r, 11),
        array_elem_type: None,
        extra: (!extra.is_empty()).then(|| extra.join(" ")),
        base_type: base,
    })
}

// --- Indexes, keys, checks and triggers -----------------------------------------------------------

/// Index columns of `@P1`: key columns in key order, then included ones.
pub const INDEXES: &str = "SELECT i.index_id, i.name, i.type_desc, i.is_primary_key, \
     i.is_unique, i.filter_definition, ic.is_included_column, COL_NAME(ic.object_id, \
     ic.column_id), CAST(ep.value AS NVARCHAR(MAX)) FROM sys.indexes i \
     JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
     LEFT JOIN sys.extended_properties ep ON ep.class = 7 AND ep.major_id = i.object_id AND \
     ep.minor_id = i.index_id AND ep.name = N'MS_Description' \
     WHERE i.object_id = OBJECT_ID(@P1) AND i.type > 0 AND i.is_hypothetical = 0 \
     ORDER BY i.index_id, ic.is_included_column, ic.key_ordinal, ic.index_column_id";

/// The indexes, and the primary key's columns in key order.
pub fn parse_indexes(rows: &[Vec<Value>]) -> (Vec<IndexMeta>, Vec<String>) {
    let mut out: Vec<(i64, IndexMeta)> = Vec::new();
    for r in rows {
        let (Some(id), Some(column)) = (int(r, 0), text(r, 7)) else {
            continue;
        };
        if out.last().is_none_or(|(last, _)| *last != id) {
            let method = text(r, 2).map(|m| m.replace('_', " ").to_lowercase());
            out.push((
                id,
                IndexMeta {
                    name: text(r, 1).unwrap_or_default(),
                    columns: Vec::new(),
                    unique: flag(r, 4),
                    primary: flag(r, 3),
                    method,
                    predicate: text(r, 5).map(|p| strip_parens(&p)),
                    definition: None,
                    comment: non_empty(r, 8),
                    include: Vec::new(),
                },
            ));
        }
        let Some((_, index)) = out.last_mut() else {
            continue;
        };
        // A columnstore index lists its columns as included ones.
        let columnstore = index
            .method
            .as_deref()
            .is_some_and(|m| m.contains("columnstore"));
        if flag(r, 6) && !columnstore {
            index.include.push(column);
        } else {
            index.columns.push(column);
        }
    }
    let indexes: Vec<IndexMeta> = out.into_iter().map(|(_, i)| i).collect();
    let pk = indexes
        .iter()
        .find(|i| i.primary)
        .map(|i| i.columns.clone())
        .unwrap_or_default();
    (indexes, pk)
}

/// Foreign keys of `@P1`, one row per column pair.
pub const FOREIGN_KEYS: &str = "SELECT fk.object_id, fk.name, \
     OBJECT_SCHEMA_NAME(fk.referenced_object_id), OBJECT_NAME(fk.referenced_object_id), \
     COL_NAME(fkc.parent_object_id, fkc.parent_column_id), \
     COL_NAME(fkc.referenced_object_id, fkc.referenced_column_id), \
     fk.update_referential_action_desc, fk.delete_referential_action_desc \
     FROM sys.foreign_keys fk JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = \
     fk.object_id WHERE fk.parent_object_id = OBJECT_ID(@P1) \
     ORDER BY fk.name, fkc.constraint_column_id";

/// `NO_ACTION` as DDL spells it: `NO ACTION`.
fn action(desc: Option<String>) -> String {
    desc.map(|d| d.replace('_', " "))
        .unwrap_or_else(|| "NO ACTION".into())
}

pub fn parse_foreign_keys(rows: &[Vec<Value>]) -> Vec<ForeignKeyMeta> {
    let mut out: Vec<(i64, ForeignKeyMeta)> = Vec::new();
    for r in rows {
        let (Some(id), Some(from), Some(to)) = (int(r, 0), text(r, 4), text(r, 5)) else {
            continue;
        };
        if out.last().is_none_or(|(last, _)| *last != id) {
            out.push((
                id,
                ForeignKeyMeta {
                    name: text(r, 1).unwrap_or_default(),
                    columns: Vec::new(),
                    ref_table: TableRef::new(
                        text(r, 2).as_deref(),
                        &text(r, 3).unwrap_or_default(),
                    ),
                    ref_columns: Vec::new(),
                    on_update: action(text(r, 6)),
                    on_delete: action(text(r, 7)),
                    definition: None,
                },
            ));
        }
        if let Some((_, fk)) = out.last_mut() {
            fk.columns.push(from);
            fk.ref_columns.push(to);
        }
    }
    out.into_iter().map(|(_, fk)| fk).collect()
}

pub const CHECKS: &str = "SELECT name, definition, COL_NAME(parent_object_id, \
     parent_column_id) FROM sys.check_constraints WHERE parent_object_id = OBJECT_ID(@P1) \
     ORDER BY name";

/// The `[names]` of a definition SQL Server normalized (`([a]>(0) AND [b]<>N'x')`).
pub fn bracketed_names(definition: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut chars = definition.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        match c {
            '\'' => in_string = !in_string,
            '[' if !in_string => {
                let mut name = String::new();
                while let Some(n) = chars.next() {
                    if n == ']' {
                        if chars.peek() == Some(&']') {
                            chars.next();
                            name.push(']');
                        } else {
                            break;
                        }
                    } else {
                        name.push(n);
                    }
                }
                if !out.contains(&name) {
                    out.push(name);
                }
            }
            _ => {}
        }
    }
    out
}

pub fn parse_checks(rows: &[Vec<Value>]) -> Vec<CheckMeta> {
    rows.iter()
        .filter_map(|r| {
            let definition = text(r, 1).unwrap_or_default();
            let mut columns = bracketed_names(&definition);
            if let Some(c) = text(r, 2)
                && !columns.contains(&c)
            {
                columns.insert(0, c);
            }
            Some(CheckMeta {
                name: text(r, 0)?,
                expression: strip_parens(&definition),
                columns,
            })
        })
        .collect()
}

/// Triggers of `@P1`, one row per event. The definition is NULL for one created WITH ENCRYPTION.
pub const TRIGGERS: &str = "SELECT t.name, t.is_instead_of_trigger, t.is_disabled, \
     te.type_desc, OBJECT_DEFINITION(t.object_id) FROM sys.triggers t \
     JOIN sys.trigger_events te ON te.object_id = t.object_id \
     WHERE t.parent_id = OBJECT_ID(@P1) ORDER BY t.name, te.type";

pub fn parse_triggers(rows: &[Vec<Value>]) -> Vec<TriggerMeta> {
    let mut out: Vec<TriggerMeta> = Vec::new();
    for r in rows {
        let Some(name) = text(r, 0) else {
            continue;
        };
        if out.last().is_none_or(|t| t.name != name) {
            out.push(TriggerMeta {
                name,
                timing: Some(if flag(r, 1) { "INSTEAD OF" } else { "AFTER" }.into()),
                events: Vec::new(),
                enabled: !flag(r, 2),
                definition: text(r, 4).unwrap_or_else(|| "-- WITH ENCRYPTION".into()),
            });
        }
        if let (Some(t), Some(event)) = (out.last_mut(), text(r, 3))
            && !t.events.contains(&event)
        {
            t.events.push(event);
        }
    }
    out
}

/// What `table_meta` assembles: the parsed rows of TABLE, COLUMNS, INDEXES, FOREIGN_KEYS,
/// CHECKS and TRIGGERS.
#[derive(Debug, Clone)]
pub struct TableParts {
    pub info: TableInfo,
    pub columns: Vec<ColumnMeta>,
    pub indexes: Vec<IndexMeta>,
    pub primary_key: Vec<String>,
    pub foreign_keys: Vec<ForeignKeyMeta>,
    pub checks: Vec<CheckMeta>,
    pub triggers: Vec<TriggerMeta>,
}

/// A table's structure from the parsed parts: the primary key's columns marked in key order.
pub fn table_meta(table: TableRef, parts: TableParts) -> TableMeta {
    let TableParts {
        info,
        mut columns,
        indexes,
        primary_key,
        foreign_keys,
        checks,
        triggers,
    } = parts;
    for c in &mut columns {
        if let Some(i) = primary_key.iter().position(|k| *k == c.name) {
            c.is_pk = true;
            c.pk_ordinal = u32::try_from(i + 1).ok();
        }
    }
    TableMeta {
        table,
        kind: info.kind,
        columns,
        primary_key,
        indexes,
        foreign_keys,
        triggers,
        comment: info.comment,
        row_estimate: info.row_estimate,
        ddl: None,
        checks,
        options: None,
    }
}

// --- Definitions, completion and processes --------------------------------------------------------

/// The text of a view, procedure, function or trigger `@P1` (NULL WITH ENCRYPTION).
pub const DEFINITION: &str = "SELECT OBJECT_DEFINITION(OBJECT_ID(@P1))";

pub fn parse_definition(rows: &[Vec<Value>]) -> Option<String> {
    rows.first().and_then(|r| text(r, 0))
}

pub const COMPLETION: &str = "SELECT s.name, o.name, c.name FROM sys.columns c \
     JOIN sys.objects o ON o.object_id = c.object_id \
     JOIN sys.schemas s ON s.schema_id = o.schema_id \
     WHERE o.type IN ('U', 'V') AND o.is_ms_shipped = 0 ORDER BY s.name, o.name, c.column_id";

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

/// User sessions. Without VIEW SERVER STATE the DMVs show only the session's own.
pub const PROCESSES: &str = "SELECT s.session_id, s.login_name, \
     DB_NAME(COALESCE(r.database_id, s.database_id)), s.program_name, s.host_name, \
     COALESCE(r.status, s.status), r.command, \
     CONVERT(NVARCHAR(30), COALESCE(r.start_time, s.last_request_start_time), 126), \
     DATEDIFF(SECOND, COALESCE(r.start_time, s.last_request_start_time), GETDATE()), \
     r.wait_type, t.text FROM sys.dm_exec_sessions s \
     LEFT JOIN sys.dm_exec_requests r ON r.session_id = s.session_id \
     OUTER APPLY sys.dm_exec_sql_text(r.sql_handle) t \
     WHERE s.is_user_process = 1 ORDER BY s.session_id";

/// The process list; `own_spid` is the listing session, `application` Tabler's program name.
pub fn parse_processes(
    rows: &[Vec<Value>],
    own_spid: Option<i64>,
    application: &str,
) -> Vec<ProcessRow> {
    rows.iter()
        .filter_map(|r| {
            let id = int(r, 0)?;
            let program = non_empty(r, 3);
            Some(ProcessRow {
                id: id.to_string(),
                user: non_empty(r, 1),
                database: text(r, 2),
                own: program.as_deref() == Some(application),
                application: program,
                client: non_empty(r, 4),
                state: text(r, 5),
                command: non_empty(r, 6),
                started_at: text(r, 7),
                duration_ms: int(r, 8).map(|s| s.saturating_mul(1000)),
                wait_event: non_empty(r, 9),
                query: non_empty(r, 10),
                current: own_spid == Some(id),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn n(i: i64) -> Value {
        Value::Int(i)
    }

    const NULL: Value = Value::Null;

    #[test]
    fn server_info_and_labels() {
        let rows = vec![vec![
            t("16.0.4135.4"),
            t("Microsoft SQL Server 2022 (RTM-CU15)"),
            n(3),
            t("Developer Edition (64-bit)"),
            t("master"),
            t("sa"),
            t("dbo"),
            n(53),
        ]];
        let info = parse_server_info(&rows).unwrap();
        assert_eq!(info.label(), "SQL Server 2022 (16.0.4135.4)");
        assert_eq!(info.spid, Some(53));
        assert!(!info.is_azure_database());
        let azure = MssqlServerInfo {
            product_version: "12.0.2000.8".into(),
            engine_edition: Some(5),
            ..MssqlServerInfo::default()
        };
        assert_eq!(azure.label(), "Azure SQL Database (12.0.2000.8)");
        assert!(azure.is_azure_database());
        assert!(parse_server_info(&[]).is_err());
        assert_eq!(parse_trancount(&[vec![n(2)]]), Some(true));
        assert_eq!(parse_trancount(&[vec![n(0)]]), Some(false));
    }

    #[test]
    fn databases_schemas_and_tables() {
        let dbs = parse_databases(&[
            vec![
                t("master"),
                t("sa"),
                t("SQL_Latin1_General_CP1_CI_AS"),
                n(1),
            ],
            vec![NULL, NULL, NULL, n(0)],
            vec![t("shop"), NULL, NULL, n(0)],
        ]);
        assert_eq!(dbs.len(), 2);
        assert!(dbs[0].is_current && !dbs[1].is_current);
        assert_eq!(
            dbs[0].encoding.as_deref(),
            Some("SQL_Latin1_General_CP1_CI_AS")
        );
        let schemas = parse_schemas(&[vec![t("dbo"), n(1)], vec![t("sales"), n(0)]]);
        assert!(schemas[0].is_default && schemas[1].name == "sales");
        let tables = parse_tables(
            &[
                vec![
                    t("Orders"),
                    t("U "),
                    t("All orders"),
                    Value::BigInt("12".into()),
                    n(16384),
                ],
                vec![t("v_Orders"), t("V "), NULL, NULL, NULL],
            ],
            "dbo",
        );
        assert_eq!(tables[0].kind, TableKind::Table);
        assert_eq!(tables[0].row_estimate, Some(12));
        assert_eq!(tables[0].comment.as_deref(), Some("All orders"));
        assert_eq!(tables[1].kind, TableKind::View);
        assert_eq!(tables[1].schema.as_deref(), Some("dbo"));
        let table = TableRef::new(Some("dbo"), "nope");
        assert!(matches!(
            parse_table(&[], &table),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn row_counts_never_meet_allocation_units() {
        // A table with LOB or row-overflow data has two or three allocation units per
        // partition: rows summed across that join would count that many times.
        let rows = rows_of_o!();
        assert!(rows.contains("FROM sys.partitions pa WHERE"));
        assert!(!rows.contains("allocation_units"));
        assert!(TABLES.contains(rows) && TABLE.contains(rows));
        assert_eq!(TABLES.matches("pa.rows").count(), 1);
        assert_eq!(TABLE.matches("pa.rows").count(), 1);
        let pages = pages_of_o!();
        assert!(TABLES.contains(pages) && !pages.contains("rows"));
        assert!(pages.contains("(au.type IN (1, 3) AND au.container_id = pa.hobt_id)"));
        assert!(pages.contains("(au.type = 2 AND au.container_id = pa.partition_id)"));
    }

    #[test]
    fn types_are_spelled_like_ddl() {
        assert_eq!(full_type("nvarchar", Some(100), None, None), "nvarchar(50)");
        assert_eq!(full_type("nvarchar", Some(-1), None, None), "nvarchar(max)");
        assert_eq!(
            full_type("varbinary", Some(-1), None, None),
            "varbinary(max)"
        );
        assert_eq!(full_type("char", Some(10), None, None), "char(10)");
        assert_eq!(
            full_type("decimal", Some(9), Some(10), Some(2)),
            "decimal(10,2)"
        );
        assert_eq!(
            full_type("datetime2", Some(8), Some(27), Some(7)),
            "datetime2(7)"
        );
        assert_eq!(full_type("int", Some(4), Some(10), Some(0)), "int");
        assert_eq!(kind_of_type("Point", true), ValueKind::Bytes);
        assert_eq!(kind_of_type("hierarchyid", true), ValueKind::Other);
        assert_eq!(kind_of_type("GEOGRAPHY", true), ValueKind::Geometry);
        assert_eq!(kind_of_type("sysname", false), ValueKind::Text);
    }

    #[test]
    fn definitions_lose_their_wrapping_parentheses() {
        assert_eq!(strip_parens("((0))"), "0");
        assert_eq!(strip_parens("(getdate())"), "getdate()");
        assert_eq!(strip_parens("(N'a)(b')"), "N'a)(b'");
        assert_eq!(strip_parens("([a]+(1))"), "[a]+(1)");
        assert_eq!(strip_parens("(a)+(b)"), "(a)+(b)");
        assert_eq!(strip_parens("([x)])"), "[x)]");
        assert_eq!(strip_parens(""), "");
        assert_eq!(strip_parens("("), "(");
        assert_eq!(
            bracketed_names("([a]>(0) AND [b]]c]<>N'[d]' AND [a]<(9))"),
            ["a", "b]c"]
        );
    }

    fn column_row(
        id: i64,
        name: &str,
        ty: &str,
        base: Option<&str>,
        len: i64,
        nullable: bool,
    ) -> Vec<Value> {
        vec![
            n(id),
            t(name),
            t(ty),
            base.map_or(NULL, t),
            n(len),
            n(0),
            n(0),
            Value::Bool(nullable),
            Value::Bool(false),
            Value::Bool(false),
            n(0),
            NULL,
            NULL,
            NULL,
            NULL,
            NULL,
            NULL,
            NULL,
            Value::Bool(false),
        ]
    }

    #[test]
    fn columns_and_table_meta() {
        let mut id = column_row(1, "Id", "int", None, 4, false);
        id[8] = Value::Bool(true);
        id[15] = t("1");
        id[16] = t("1");
        let mut total = column_row(2, "Total", "money", None, 8, true);
        total[12] = t("((0))");
        let phone = column_row(3, "Phone", "Phone", Some("nvarchar"), 40, true);
        let mut twice = column_row(4, "Twice", "decimal", None, 9, true);
        twice[5] = n(19);
        twice[6] = n(4);
        twice[9] = Value::Bool(true);
        twice[13] = t("([Total]*(2))");
        twice[14] = Value::Bool(true);
        let mut from = column_row(5, "ValidFrom", "datetime2", None, 8, false);
        from[6] = n(7);
        from[10] = n(1);
        let stamp = column_row(6, "Ver", "timestamp", None, 8, false);
        let cols = parse_columns(&[id, total, phone, twice, from, stamp]);
        assert_eq!(cols[0].extra.as_deref(), Some("IDENTITY(1,1)"));
        assert!(cols[0].is_auto_increment && !cols[0].nullable);
        assert_eq!(cols[1].kind, ValueKind::Decimal);
        assert_eq!(cols[1].default_expr.as_deref(), Some("0"));
        assert_eq!(cols[2].data_type, "Phone");
        assert_eq!(cols[2].base_type.as_deref(), Some("nvarchar"));
        assert_eq!(cols[2].kind, ValueKind::Text);
        assert_eq!(cols[3].data_type, "decimal(19,4)");
        assert_eq!(cols[3].extra.as_deref(), Some("AS ([Total]*(2)) PERSISTED"));
        assert!(cols[3].is_generated);
        assert_eq!(cols[3].numeric_scale, Some(4));
        assert_eq!(cols[4].data_type, "datetime2(7)");
        assert_eq!(
            cols[4].extra.as_deref(),
            Some("GENERATED ALWAYS AS ROW START")
        );
        assert!(cols[4].is_generated && cols[5].is_generated);
        assert_eq!(cols[5].kind, ValueKind::Bytes);

        let (indexes, pk) = parse_indexes(&[
            vec![
                n(1),
                t("PK_Orders"),
                t("CLUSTERED"),
                Value::Bool(true),
                Value::Bool(true),
                NULL,
                Value::Bool(false),
                t("Id"),
                NULL,
            ],
            vec![
                n(2),
                t("IX_Total"),
                t("NONCLUSTERED"),
                Value::Bool(false),
                Value::Bool(false),
                t("([Total]>(0))"),
                Value::Bool(false),
                t("Total"),
                t("by total"),
            ],
            vec![
                n(2),
                t("IX_Total"),
                t("NONCLUSTERED"),
                Value::Bool(false),
                Value::Bool(false),
                t("([Total]>(0))"),
                Value::Bool(true),
                t("Phone"),
                t("by total"),
            ],
            vec![
                n(3),
                t("CS"),
                t("NONCLUSTERED_COLUMNSTORE"),
                Value::Bool(false),
                Value::Bool(false),
                NULL,
                Value::Bool(true),
                t("Total"),
                NULL,
            ],
        ]);
        assert_eq!(pk, ["Id"]);
        assert_eq!(indexes[1].columns, ["Total"]);
        assert_eq!(indexes[1].include, ["Phone"]);
        assert_eq!(indexes[1].predicate.as_deref(), Some("[Total]>(0)"));
        assert_eq!(indexes[1].method.as_deref(), Some("nonclustered"));
        assert_eq!(indexes[2].columns, ["Total"]);
        assert!(indexes[2].include.is_empty());

        let fks = parse_foreign_keys(&[
            vec![
                n(9),
                t("FK_a"),
                t("dbo"),
                t("Customers"),
                t("CustomerId"),
                t("Id"),
                t("NO_ACTION"),
                t("SET_NULL"),
            ],
            vec![
                n(9),
                t("FK_a"),
                t("dbo"),
                t("Customers"),
                t("Region"),
                t("Region"),
                t("NO_ACTION"),
                t("SET_NULL"),
            ],
        ]);
        assert_eq!(fks.len(), 1);
        assert_eq!(fks[0].columns, ["CustomerId", "Region"]);
        assert_eq!(fks[0].ref_table, TableRef::new(Some("dbo"), "Customers"));
        assert_eq!(
            (fks[0].on_update.as_str(), fks[0].on_delete.as_str()),
            ("NO ACTION", "SET NULL")
        );

        let checks = parse_checks(&[vec![t("CK_Total"), t("([Total]>=(0))"), t("Total")]]);
        assert_eq!(checks[0].expression, "[Total]>=(0)");
        assert_eq!(checks[0].columns, ["Total"]);

        let triggers = parse_triggers(&[
            vec![
                t("trg"),
                Value::Bool(false),
                Value::Bool(true),
                t("INSERT"),
                t("CREATE TRIGGER trg …"),
            ],
            vec![
                t("trg"),
                Value::Bool(false),
                Value::Bool(true),
                t("UPDATE"),
                t("CREATE TRIGGER trg …"),
            ],
            vec![
                t("hidden"),
                Value::Bool(true),
                Value::Bool(false),
                t("DELETE"),
                NULL,
            ],
        ]);
        assert_eq!(triggers[0].events, ["INSERT", "UPDATE"]);
        assert!(!triggers[0].enabled);
        assert_eq!(triggers[1].timing.as_deref(), Some("INSTEAD OF"));
        assert_eq!(triggers[1].definition, "-- WITH ENCRYPTION");

        let info = TableInfo {
            kind: TableKind::Table,
            comment: None,
            row_estimate: Some(3),
        };
        let meta = table_meta(
            TableRef::new(Some("dbo"), "Orders"),
            TableParts {
                info,
                columns: cols,
                indexes,
                primary_key: pk,
                foreign_keys: fks,
                checks,
                triggers,
            },
        );
        assert!(meta.columns[0].is_pk && meta.columns[0].pk_ordinal == Some(1));
        assert!(!meta.columns[1].is_pk);
        assert_eq!(meta.row_estimate, Some(3));
    }

    #[test]
    fn completion_and_processes() {
        let c = parse_completion(
            &[
                vec![t("dbo"), t("Orders"), t("Id")],
                vec![t("dbo"), t("Orders"), t("Total")],
                vec![t("sales"), t("Leads"), t("Id")],
            ],
            Some("dbo".into()),
        );
        assert_eq!(c.schemas["dbo"]["Orders"], ["Id", "Total"]);
        assert_eq!(c.default_schema.as_deref(), Some("dbo"));
        let p = parse_processes(
            &[vec![
                n(53),
                t("sa"),
                t("master"),
                t("Tabler"),
                t("WS1"),
                t("running"),
                t("SELECT"),
                t("2024-01-02T10:11:12.123"),
                n(3),
                NULL,
                t("SELECT 1"),
            ]],
            Some(53),
            "Tabler",
        );
        assert!(p[0].current && p[0].own);
        assert_eq!(p[0].duration_ms, Some(3000));
        assert_eq!(p[0].query.as_deref(), Some("SELECT 1"));
        assert_eq!(int(&[Value::Decimal("12.00".into())], 0), Some(12));
        assert_eq!(int(&[Value::Float(f64::NAN)], 0), None);
    }

    /// The catalog queries take names as parameters only, and every one of them is a read to
    /// Safe Mode (they run for the app, not through the classifier, but must stay reads).
    #[test]
    fn catalog_queries_are_reads_with_bound_names() {
        use crate::sqlgen::classify::StatementClass;
        use crate::sqlgen::dialect::tsql::SPEC;
        for sql in [
            SERVER_INFO,
            DATABASES,
            SCHEMAS,
            TABLES,
            TABLE,
            COLUMNS,
            INDEXES,
            FOREIGN_KEYS,
            CHECKS,
            TRIGGERS,
            DEFINITION,
            COMPLETION,
            PROCESSES,
        ] {
            assert_eq!((SPEC.classify)(sql), StatementClass::Read, "{sql}");
            assert!(!sql.contains("@P2"), "{sql}");
        }
    }
}
