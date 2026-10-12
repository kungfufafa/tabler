//! MySQL / MariaDB catalog queries (`information_schema` + `SHOW CREATE`).

use std::collections::BTreeMap;

use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Params, Row};

use super::decode::{int_of, text_of};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::ddl::{check_columns, check_expression};
use crate::types::schema::{
    CheckMeta, ColumnMeta, CompletionMetadata, DatabaseInfo, ForeignKeyMeta, IndexMeta, SchemaInfo,
    TableKind, TableMeta, TableOptions, TableRef, TableSummary, TriggerMeta, ValueKind,
};

/// Schemas hidden from "user" listings.
pub const SYSTEM_SCHEMAS: [&str; 4] = ["information_schema", "performance_schema", "mysql", "sys"];

fn system_schema_list() -> String {
    SYSTEM_SCHEMAS
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(",")
}

fn p(s: &str) -> mysql_async::Value {
    mysql_async::Value::Bytes(s.as_bytes().to_vec())
}

/// Runs a catalog query (binary protocol when parameters are given) and returns its rows.
async fn rows(
    conn: &mut Conn,
    sql: &str,
    params: Vec<mysql_async::Value>,
) -> Result<Vec<Row>, AppError> {
    let rows: Vec<Row> = if params.is_empty() {
        conn.query(sql).await?
    } else {
        conn.exec(sql, Params::Positional(params)).await?
    };
    Ok(rows)
}

fn s(row: &mut Row, i: usize) -> Option<String> {
    row.take::<mysql_async::Value, _>(i).and_then(text_of)
}

fn s_or_empty(row: &mut Row, i: usize) -> String {
    s(row, i).unwrap_or_default()
}

fn n(row: &mut Row, i: usize) -> Option<i64> {
    row.take::<mysql_async::Value, _>(i).and_then(int_of)
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

/// Maps `information_schema.COLUMNS.DATA_TYPE` (base type name) to a `ValueKind`.
/// `column_type` (the full spelling) is only consulted when `data_type` is empty.
pub fn kind_from_data_type(data_type: &str, column_type: &str) -> ValueKind {
    let mut base = data_type.trim().to_ascii_lowercase();
    if base.is_empty() {
        base = column_type
            .trim()
            .split(['(', ' '])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
    }
    match base.as_str() {
        "tinyint" | "smallint" | "mediumint" | "int" | "integer" | "year" => ValueKind::Int,
        "bigint" => ValueKind::BigInt,
        "decimal" | "numeric" | "dec" | "fixed" => ValueKind::Decimal,
        "float" | "double" | "real" | "double precision" => ValueKind::Float,
        "bit" => ValueKind::Bit,
        "char" | "varchar" | "text" | "tinytext" | "mediumtext" | "longtext" => ValueKind::Text,
        "binary" | "varbinary" | "blob" | "tinyblob" | "mediumblob" | "longblob" => {
            ValueKind::Bytes
        }
        "date" => ValueKind::Date,
        "time" => ValueKind::Time,
        "datetime" | "timestamp" => ValueKind::Timestamp,
        "enum" => ValueKind::Enum,
        "set" => ValueKind::Set,
        "json" => ValueKind::Json,
        "uuid" => ValueKind::Uuid,
        "geometry" | "point" | "linestring" | "polygon" | "multipoint" | "multilinestring"
        | "multipolygon" | "geometrycollection" | "geomcollection" => ValueKind::Geometry,
        _ => ValueKind::Other,
    }
}

/// Parses the members of `enum('a','b''c')` / `set('x','y')` as shown in `COLUMN_TYPE`.
/// Returns `None` when the spelling is not an enum/set.
pub fn parse_enum_values(column_type: &str) -> Option<Vec<String>> {
    let lower = column_type.trim_start().to_ascii_lowercase();
    if !(lower.starts_with("enum(") || lower.starts_with("set(")) {
        return None;
    }
    let open = column_type.find('(')?;
    let close = column_type.rfind(')')?;
    if close <= open {
        return None;
    }
    let inner = &column_type[open + 1..close];
    let mut values = Vec::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\'' {
            continue; // separators / whitespace between members
        }
        let mut value = String::new();
        loop {
            match chars.next() {
                None => break,
                Some('\\') => {
                    // MySQL doubles backslashes inside COLUMN_TYPE.
                    if let Some(next) = chars.next() {
                        value.push(next);
                    }
                }
                Some('\'') => {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                        value.push('\'');
                    } else {
                        break;
                    }
                }
                Some(ch) => value.push(ch),
            }
        }
        values.push(value);
    }
    Some(values)
}

fn table_kind(table_type: &str) -> TableKind {
    match table_type.trim().to_ascii_uppercase().as_str() {
        "VIEW" => TableKind::View,
        "SYSTEM VIEW" => TableKind::SystemView,
        _ => TableKind::Table,
    }
}

pub async fn current_database(conn: &mut Conn) -> Result<Option<String>, AppError> {
    let mut rows = rows(conn, "SELECT DATABASE()", vec![]).await?;
    Ok(rows.first_mut().and_then(|r| s(r, 0)))
}

pub async fn list_databases(conn: &mut Conn) -> Result<Vec<DatabaseInfo>, AppError> {
    let current = current_database(conn).await?;
    let rows = rows(
        conn,
        "SELECT SCHEMA_NAME, DEFAULT_CHARACTER_SET_NAME FROM information_schema.SCHEMATA \
         ORDER BY SCHEMA_NAME",
        vec![],
    )
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for mut row in rows {
        let name = s_or_empty(&mut row, 0);
        out.push(DatabaseInfo {
            is_current: current.as_deref() == Some(name.as_str()),
            encoding: non_empty(s(&mut row, 1)),
            owner: None,
            name,
        });
    }
    Ok(out)
}

/// MySQL has no schemas inside a database: the current database is the single default schema.
/// Without a selected database every non-system database is offered, none default.
pub async fn list_schemas(conn: &mut Conn) -> Result<Vec<SchemaInfo>, AppError> {
    if let Some(db) = current_database(conn).await? {
        return Ok(vec![SchemaInfo {
            name: db,
            is_default: true,
        }]);
    }
    Ok(list_databases(conn)
        .await?
        .into_iter()
        .filter(|d| !SYSTEM_SCHEMAS.contains(&d.name.as_str()))
        .map(|d| SchemaInfo {
            name: d.name,
            is_default: false,
        })
        .collect())
}

pub async fn list_tables(conn: &mut Conn, schema: &str) -> Result<Vec<TableSummary>, AppError> {
    let rows = rows(
        conn,
        "SELECT TABLE_NAME, TABLE_TYPE, TABLE_ROWS, DATA_LENGTH, INDEX_LENGTH, TABLE_COMMENT \
         FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
        vec![p(schema)],
    )
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for mut row in rows {
        let name = s_or_empty(&mut row, 0);
        let kind = table_kind(&s_or_empty(&mut row, 1));
        let row_estimate = n(&mut row, 2);
        let data_len = n(&mut row, 3);
        let index_len = n(&mut row, 4);
        let size_bytes = match (data_len, index_len) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
        };
        out.push(TableSummary {
            name,
            schema: Some(schema.to_string()),
            kind,
            row_estimate: if kind == TableKind::Table {
                row_estimate
            } else {
                None
            },
            size_bytes: if kind == TableKind::Table {
                size_bytes
            } else {
                None
            },
            comment: non_empty(s(&mut row, 5)),
        });
    }
    Ok(out)
}

async fn schema_of(conn: &mut Conn, table: &TableRef) -> Result<String, AppError> {
    if let Some(schema) = table.schema.clone().filter(|s| !s.is_empty()) {
        return Ok(schema);
    }
    current_database(conn)
        .await?
        .ok_or_else(|| AppError::InvalidInput("no database selected".into()))
}

/// Second column of `SHOW CREATE TABLE|VIEW`; `None` on any error (permissions, missing object).
async fn show_create(conn: &mut Conn, what: &str, schema: &str, name: &str) -> Option<String> {
    let sql = format!(
        "SHOW CREATE {what} {}",
        Dialect::Mysql.qualify(Some(schema), name)
    );
    let mut row: Row = conn.query_first(sql).await.ok().flatten()?;
    s(&mut row, 1)
}

pub async fn view_definition(
    conn: &mut Conn,
    table: &TableRef,
) -> Result<Option<String>, AppError> {
    let schema = schema_of(conn, table).await?;
    let mut rows = rows(
        conn,
        "SELECT VIEW_DEFINITION FROM information_schema.VIEWS \
         WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
        vec![p(&schema), p(&table.name)],
    )
    .await?;
    if let Some(def) = rows
        .first_mut()
        .and_then(|r| s(r, 0))
        .filter(|d| !d.is_empty())
    {
        return Ok(Some(def));
    }
    Ok(show_create(conn, "VIEW", &schema, &table.name).await)
}

async fn columns(conn: &mut Conn, schema: &str, name: &str) -> Result<Vec<ColumnMeta>, AppError> {
    let rows = rows(
        conn,
        "SELECT ORDINAL_POSITION, COLUMN_NAME, COLUMN_TYPE, DATA_TYPE, IS_NULLABLE, \
         COLUMN_DEFAULT, COLUMN_KEY, EXTRA, COLUMN_COMMENT, CHARACTER_MAXIMUM_LENGTH, \
         NUMERIC_PRECISION, NUMERIC_SCALE, COLLATION_NAME, GENERATION_EXPRESSION \
         FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
         ORDER BY ORDINAL_POSITION",
        vec![p(schema), p(name)],
    )
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for mut row in rows {
        let ordinal = n(&mut row, 0).unwrap_or(0) as u32;
        let name = s_or_empty(&mut row, 1);
        let column_type = s_or_empty(&mut row, 2);
        let data_type = s_or_empty(&mut row, 3);
        let nullable = s_or_empty(&mut row, 4).eq_ignore_ascii_case("YES");
        let default_expr = s(&mut row, 5);
        let _column_key = s(&mut row, 6);
        let extra = s_or_empty(&mut row, 7);
        let comment = non_empty(s(&mut row, 8));
        let char_max_len = n(&mut row, 9);
        let numeric_precision = n(&mut row, 10).map(|v| v as i32);
        let numeric_scale = n(&mut row, 11).map(|v| v as i32);
        let collation = non_empty(s(&mut row, 12));
        let generation_expr = non_empty(s(&mut row, 13));
        let extra_lower = extra.to_ascii_lowercase();
        let is_generated = extra_lower.contains("generated") || generation_expr.is_some();
        let kind = kind_from_data_type(&data_type, &column_type);
        out.push(ColumnMeta {
            name,
            ordinal,
            kind,
            nullable,
            default_expr,
            is_pk: false,
            pk_ordinal: None,
            is_auto_increment: extra_lower.contains("auto_increment"),
            is_generated,
            is_unsigned: column_type.to_ascii_lowercase().contains("unsigned"),
            comment,
            enum_values: parse_enum_values(&column_type),
            char_max_len,
            numeric_precision,
            numeric_scale,
            collation,
            array_elem_type: None,
            extra: non_empty(Some(extra)),
            data_type: column_type,
        });
    }
    Ok(out)
}

struct IndexRow {
    name: String,
    non_unique: bool,
    column: Option<String>,
    expression: Option<String>,
    sub_part: Option<i64>,
    index_type: Option<String>,
    descending: bool,
    comment: Option<String>,
}

async fn index_rows(conn: &mut Conn, schema: &str, name: &str) -> Result<Vec<IndexRow>, AppError> {
    // `EXPRESSION` (functional indexes) exists on MySQL >= 8.0.13 only; retry without it.
    const WITH_EXPR: &str = "SELECT INDEX_NAME, NON_UNIQUE, COLUMN_NAME, SUB_PART, INDEX_TYPE, \
         COLLATION, EXPRESSION, INDEX_COMMENT FROM information_schema.STATISTICS \
         WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY INDEX_NAME, SEQ_IN_INDEX";
    const WITHOUT_EXPR: &str = "SELECT INDEX_NAME, NON_UNIQUE, COLUMN_NAME, SUB_PART, INDEX_TYPE, \
         COLLATION, NULL, INDEX_COMMENT FROM information_schema.STATISTICS \
         WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY INDEX_NAME, SEQ_IN_INDEX";
    let raw = match rows(conn, WITH_EXPR, vec![p(schema), p(name)]).await {
        Ok(rows) => rows,
        Err(AppError::Db {
            vendor_code: Some(1054),
            ..
        }) => rows(conn, WITHOUT_EXPR, vec![p(schema), p(name)]).await?,
        Err(e) => return Err(e),
    };
    let mut out = Vec::with_capacity(raw.len());
    for mut row in raw {
        out.push(IndexRow {
            name: s_or_empty(&mut row, 0),
            non_unique: n(&mut row, 1).unwrap_or(1) != 0,
            column: non_empty(s(&mut row, 2)),
            sub_part: n(&mut row, 3),
            index_type: non_empty(s(&mut row, 4)),
            descending: s(&mut row, 5).as_deref() == Some("D"),
            expression: non_empty(s(&mut row, 6)),
            comment: non_empty(s(&mut row, 7)),
        });
    }
    Ok(out)
}

fn group_indexes(rows: Vec<IndexRow>) -> Vec<IndexMeta> {
    let mut out: Vec<IndexMeta> = Vec::new();
    for r in rows {
        let mut col = match (&r.column, &r.expression) {
            (Some(c), _) => c.clone(),
            (None, Some(e)) => format!("({e})"),
            (None, None) => "<expr>".to_string(),
        };
        if let Some(len) = r.sub_part {
            col.push_str(&format!("({len})"));
        }
        if r.descending {
            col.push_str(" DESC");
        }
        match out.iter_mut().find(|i| i.name == r.name) {
            Some(existing) => existing.columns.push(col),
            None => out.push(IndexMeta {
                primary: r.name == "PRIMARY",
                name: r.name,
                columns: vec![col],
                unique: !r.non_unique,
                method: r.index_type,
                predicate: None,
                definition: None,
                comment: r.comment,
            }),
        }
    }
    // PRIMARY first, then alphabetical (the query already sorts by name).
    out.sort_by_key(|i| !i.primary);
    out
}

async fn foreign_keys(
    conn: &mut Conn,
    schema: &str,
    name: &str,
) -> Result<Vec<ForeignKeyMeta>, AppError> {
    let raw = rows(
        conn,
        "SELECT rc.CONSTRAINT_NAME, rc.UPDATE_RULE, rc.DELETE_RULE, kcu.REFERENCED_TABLE_SCHEMA, \
         rc.REFERENCED_TABLE_NAME, kcu.COLUMN_NAME, kcu.REFERENCED_COLUMN_NAME \
         FROM information_schema.REFERENTIAL_CONSTRAINTS rc \
         JOIN information_schema.KEY_COLUMN_USAGE kcu \
           ON kcu.CONSTRAINT_SCHEMA = rc.CONSTRAINT_SCHEMA \
          AND kcu.CONSTRAINT_NAME = rc.CONSTRAINT_NAME \
          AND kcu.TABLE_SCHEMA = rc.CONSTRAINT_SCHEMA \
          AND kcu.TABLE_NAME = rc.TABLE_NAME \
         WHERE rc.CONSTRAINT_SCHEMA = ? AND rc.TABLE_NAME = ? \
         ORDER BY rc.CONSTRAINT_NAME, kcu.ORDINAL_POSITION",
        vec![p(schema), p(name)],
    )
    .await?;
    let mut out: Vec<ForeignKeyMeta> = Vec::new();
    for mut row in raw {
        let fk_name = s_or_empty(&mut row, 0);
        let on_update = s_or_empty(&mut row, 1);
        let on_delete = s_or_empty(&mut row, 2);
        let ref_schema = s(&mut row, 3).unwrap_or_else(|| schema.to_string());
        let ref_table = s_or_empty(&mut row, 4);
        let column = s_or_empty(&mut row, 5);
        let ref_column = s_or_empty(&mut row, 6);
        match out.iter_mut().find(|f| f.name == fk_name) {
            Some(existing) => {
                existing.columns.push(column);
                existing.ref_columns.push(ref_column);
            }
            None => out.push(ForeignKeyMeta {
                name: fk_name,
                columns: vec![column],
                ref_table: TableRef::new(Some(&ref_schema), &ref_table),
                ref_columns: vec![ref_column],
                on_update,
                on_delete,
                definition: None,
            }),
        }
    }
    Ok(out)
}

/// CHECK constraints (MySQL 8.0.16+, MariaDB 10.2+); empty where the server has no
/// `CHECK_CONSTRAINTS` table.
async fn checks(
    conn: &mut Conn,
    schema: &str,
    name: &str,
    columns: &[String],
) -> Result<Vec<CheckMeta>, AppError> {
    let raw = rows(
        conn,
        "SELECT cc.CONSTRAINT_NAME, cc.CHECK_CLAUSE \
         FROM information_schema.CHECK_CONSTRAINTS cc \
         JOIN information_schema.TABLE_CONSTRAINTS tc \
           ON tc.CONSTRAINT_SCHEMA = cc.CONSTRAINT_SCHEMA \
          AND tc.CONSTRAINT_NAME = cc.CONSTRAINT_NAME \
          AND tc.CONSTRAINT_TYPE = 'CHECK' \
         WHERE tc.TABLE_SCHEMA = ? AND tc.TABLE_NAME = ? \
         ORDER BY cc.CONSTRAINT_NAME",
        vec![p(schema), p(name)],
    )
    .await;
    let raw = match raw {
        Ok(rows) => rows,
        // 1109 unknown table (no CHECK_CONSTRAINTS), 1054 unknown column, 1146 no such table
        Err(AppError::Db {
            vendor_code: Some(1109 | 1054 | 1146),
            ..
        }) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::with_capacity(raw.len());
    for mut row in raw {
        let clause = s_or_empty(&mut row, 1);
        let expression = check_expression(&clause);
        out.push(CheckMeta {
            name: s_or_empty(&mut row, 0),
            columns: check_columns(&expression, columns),
            expression,
        });
    }
    Ok(out)
}

/// Table options from `information_schema.TABLES` (`ENGINE`, `TABLE_COLLATION`,
/// `AUTO_INCREMENT`, `ROW_FORMAT`) and `SHOW CREATE TABLE`, whose `AUTO_INCREMENT=` is current
/// (MySQL 8 caches the `information_schema` value).
pub fn table_options(
    engine: Option<String>,
    collation: Option<String>,
    auto_increment: Option<i64>,
    row_format: Option<String>,
    ddl: Option<&str>,
) -> TableOptions {
    let from_ddl = ddl.and_then(|d| {
        let tail = &d[d.rfind(')')? + 1..];
        let at = tail.to_ascii_uppercase().find("AUTO_INCREMENT=")?;
        let digits: String = tail[at + "AUTO_INCREMENT=".len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse::<u64>().ok()
    });
    // a table created without rows or keys has no counter in SHOW CREATE TABLE
    let auto_increment = from_ddl.or(auto_increment.and_then(|n| u64::try_from(n).ok()));
    let charset = collation
        .as_deref()
        .map(|c| c.split('_').next().unwrap_or(c).to_string());
    TableOptions {
        engine,
        charset,
        collation,
        auto_increment,
        row_format,
    }
}

async fn triggers(conn: &mut Conn, schema: &str, name: &str) -> Result<Vec<TriggerMeta>, AppError> {
    let raw = rows(
        conn,
        "SELECT TRIGGER_NAME, ACTION_TIMING, EVENT_MANIPULATION, ACTION_STATEMENT \
         FROM information_schema.TRIGGERS \
         WHERE EVENT_OBJECT_SCHEMA = ? AND EVENT_OBJECT_TABLE = ? \
         ORDER BY ACTION_ORDER, TRIGGER_NAME",
        vec![p(schema), p(name)],
    )
    .await?;
    let mut out = Vec::with_capacity(raw.len());
    for mut row in raw {
        out.push(TriggerMeta {
            name: s_or_empty(&mut row, 0),
            timing: non_empty(s(&mut row, 1)),
            events: s(&mut row, 2).into_iter().collect(),
            enabled: true,
            definition: s_or_empty(&mut row, 3),
        });
    }
    Ok(out)
}

pub async fn table_meta(conn: &mut Conn, table: &TableRef) -> Result<TableMeta, AppError> {
    let schema = schema_of(conn, table).await?;
    let name = table.name.as_str();

    let mut head = rows(
        conn,
        "SELECT TABLE_TYPE, TABLE_ROWS, TABLE_COMMENT, ENGINE, TABLE_COLLATION, AUTO_INCREMENT, \
         ROW_FORMAT FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
        vec![p(&schema), p(name)],
    )
    .await?;
    let Some(head) = head.first_mut() else {
        return Err(AppError::NotFound(format!("table {schema}.{name}")));
    };
    let kind = table_kind(&s_or_empty(head, 0));
    let row_estimate = if kind == TableKind::Table {
        n(head, 1)
    } else {
        None
    };
    let comment = non_empty(s(head, 2));
    let engine = non_empty(s(head, 3));
    let table_collation = non_empty(s(head, 4));
    let next_auto_increment = n(head, 5);
    let row_format = non_empty(s(head, 6));

    let mut columns = columns(conn, &schema, name).await?;

    let (indexes, foreign_keys) = if kind == TableKind::Table {
        (
            group_indexes(index_rows(conn, &schema, name).await?),
            foreign_keys(conn, &schema, name).await?,
        )
    } else {
        (Vec::new(), Vec::new())
    };

    let primary_key: Vec<String> = indexes
        .iter()
        .find(|i| i.primary)
        .map(|i| i.columns.clone())
        .unwrap_or_default();
    for (pos, pk_col) in primary_key.iter().enumerate() {
        if let Some(col) = columns.iter_mut().find(|c| &c.name == pk_col) {
            col.is_pk = true;
            col.pk_ordinal = Some(pos as u32 + 1);
        }
    }

    let triggers = if kind == TableKind::Table {
        triggers(conn, &schema, name).await?
    } else {
        Vec::new()
    };

    let ddl = match kind {
        TableKind::View | TableKind::SystemView => show_create(conn, "VIEW", &schema, name).await,
        _ => show_create(conn, "TABLE", &schema, name).await,
    };

    let (checks, options) = if kind == TableKind::Table {
        let names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
        (
            checks(conn, &schema, name, &names).await?,
            Some(table_options(
                engine,
                table_collation,
                next_auto_increment,
                row_format,
                ddl.as_deref(),
            )),
        )
    } else {
        (Vec::new(), None)
    };

    Ok(TableMeta {
        table: TableRef::new(Some(&schema), name),
        kind,
        columns,
        primary_key,
        indexes,
        foreign_keys,
        triggers,
        comment,
        row_estimate,
        ddl,
        checks,
        options,
    })
}

pub async fn completion_metadata(conn: &mut Conn) -> Result<CompletionMetadata, AppError> {
    let default_schema = current_database(conn).await?;
    let sql = format!(
        "SELECT TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME FROM information_schema.COLUMNS \
         WHERE TABLE_SCHEMA NOT IN ({}) ORDER BY TABLE_SCHEMA, TABLE_NAME, ORDINAL_POSITION",
        system_schema_list()
    );
    let raw = rows(conn, &sql, vec![]).await?;
    let mut schemas: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for mut row in raw {
        let schema = s_or_empty(&mut row, 0);
        let table = s_or_empty(&mut row, 1);
        let column = s_or_empty(&mut row, 2);
        schemas
            .entry(schema)
            .or_default()
            .entry(table)
            .or_default()
            .push(column);
    }
    Ok(CompletionMetadata {
        default_schema,
        schemas,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_enum_and_set_members() {
        assert_eq!(
            parse_enum_values("enum('a','b''c')").unwrap(),
            vec!["a".to_string(), "b'c".to_string()]
        );
        assert_eq!(
            parse_enum_values("set('x','y z','')").unwrap(),
            vec!["x".to_string(), "y z".to_string(), String::new()]
        );
        assert_eq!(
            parse_enum_values("ENUM('admin','editor','viewer')").unwrap(),
            vec!["admin", "editor", "viewer"]
        );
        assert_eq!(
            parse_enum_values("enum('a\\\\b')").unwrap(),
            vec!["a\\b".to_string()]
        );
        assert!(parse_enum_values("varchar(255)").is_none());
        assert!(parse_enum_values("int unsigned").is_none());
    }

    #[test]
    fn maps_data_types_to_kinds() {
        assert_eq!(kind_from_data_type("tinyint", "tinyint(1)"), ValueKind::Int);
        assert_eq!(kind_from_data_type("int", "int unsigned"), ValueKind::Int);
        assert_eq!(
            kind_from_data_type("bigint", "bigint unsigned"),
            ValueKind::BigInt
        );
        assert_eq!(kind_from_data_type("year", "year"), ValueKind::Int);
        assert_eq!(
            kind_from_data_type("decimal", "decimal(10,2)"),
            ValueKind::Decimal
        );
        assert_eq!(kind_from_data_type("double", "double"), ValueKind::Float);
        assert_eq!(kind_from_data_type("bit", "bit(8)"), ValueKind::Bit);
        assert_eq!(
            kind_from_data_type("varchar", "varchar(255)"),
            ValueKind::Text
        );
        assert_eq!(kind_from_data_type("longtext", "longtext"), ValueKind::Text);
        assert_eq!(
            kind_from_data_type("varbinary", "varbinary(16)"),
            ValueKind::Bytes
        );
        assert_eq!(kind_from_data_type("blob", "blob"), ValueKind::Bytes);
        assert_eq!(kind_from_data_type("date", "date"), ValueKind::Date);
        assert_eq!(kind_from_data_type("time", "time(3)"), ValueKind::Time);
        assert_eq!(
            kind_from_data_type("datetime", "datetime(6)"),
            ValueKind::Timestamp
        );
        assert_eq!(
            kind_from_data_type("timestamp", "timestamp"),
            ValueKind::Timestamp
        );
        assert_eq!(kind_from_data_type("enum", "enum('a')"), ValueKind::Enum);
        assert_eq!(kind_from_data_type("set", "set('a')"), ValueKind::Set);
        assert_eq!(kind_from_data_type("json", "json"), ValueKind::Json);
        assert_eq!(kind_from_data_type("point", "point"), ValueKind::Geometry);
        assert_eq!(
            kind_from_data_type("geometry", "geometry"),
            ValueKind::Geometry
        );
        assert_eq!(kind_from_data_type("uuid", "uuid"), ValueKind::Uuid);
        assert_eq!(kind_from_data_type("inet6", "inet6"), ValueKind::Other);
        assert_eq!(kind_from_data_type("", "INT(11)"), ValueKind::Int);
    }

    #[test]
    fn groups_index_rows() {
        let rows = vec![
            IndexRow {
                name: "idx_name".into(),
                non_unique: true,
                column: Some("name".into()),
                expression: None,
                sub_part: Some(10),
                index_type: Some("BTREE".into()),
                descending: false,
                comment: None,
            },
            IndexRow {
                name: "idx_name".into(),
                non_unique: true,
                column: Some("created_at".into()),
                expression: None,
                sub_part: None,
                index_type: Some("BTREE".into()),
                descending: true,
                comment: Some("by name".into()),
            },
            IndexRow {
                name: "PRIMARY".into(),
                non_unique: false,
                column: Some("id".into()),
                expression: None,
                sub_part: None,
                index_type: Some("BTREE".into()),
                descending: false,
                comment: None,
            },
            IndexRow {
                name: "fx".into(),
                non_unique: true,
                column: None,
                expression: Some("lower(`email`)".into()),
                sub_part: None,
                index_type: Some("BTREE".into()),
                descending: false,
                comment: None,
            },
        ];
        let grouped = group_indexes(rows);
        assert_eq!(grouped[0].name, "PRIMARY");
        assert!(grouped[0].primary && grouped[0].unique);
        assert_eq!(grouped[0].columns, ["id"]);
        assert_eq!(grouped[1].name, "idx_name");
        assert_eq!(grouped[1].columns, ["name(10)", "created_at DESC"]);
        assert!(!grouped[1].unique);
        assert_eq!(grouped[1].method.as_deref(), Some("BTREE"));
        assert_eq!(grouped[1].comment, None);
        assert_eq!(grouped[2].columns, ["(lower(`email`))"]);
    }

    #[test]
    fn reads_table_options() {
        let ddl = "CREATE TABLE `t` (\n  `id` int NOT NULL AUTO_INCREMENT,\n  PRIMARY KEY (`id`)\n) ENGINE=InnoDB AUTO_INCREMENT=42 DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci";
        let o = table_options(
            Some("InnoDB".into()),
            Some("utf8mb4_0900_ai_ci".into()),
            Some(7),
            Some("Dynamic".into()),
            Some(ddl),
        );
        assert_eq!(o.engine.as_deref(), Some("InnoDB"));
        assert_eq!(o.charset.as_deref(), Some("utf8mb4"));
        assert_eq!(o.collation.as_deref(), Some("utf8mb4_0900_ai_ci"));
        // SHOW CREATE TABLE wins over the cached information_schema value
        assert_eq!(o.auto_increment, Some(42));
        assert_eq!(o.row_format.as_deref(), Some("Dynamic"));
        // the column's AUTO_INCREMENT keyword is not the table option
        let no_counter = "CREATE TABLE `t` (\n  `id` int NOT NULL AUTO_INCREMENT\n) ENGINE=InnoDB";
        let o = table_options(
            None,
            Some("latin1_swedish_ci".into()),
            Some(3),
            None,
            Some(no_counter),
        );
        assert_eq!(o.auto_increment, Some(3));
        assert_eq!(o.charset.as_deref(), Some("latin1"));
        let o = table_options(None, Some("binary".into()), None, None, None);
        assert_eq!(o.charset.as_deref(), Some("binary"));
        assert_eq!(o.auto_increment, None);
    }

    #[test]
    fn table_kinds() {
        assert_eq!(table_kind("BASE TABLE"), TableKind::Table);
        assert_eq!(table_kind("VIEW"), TableKind::View);
        assert_eq!(table_kind("SYSTEM VIEW"), TableKind::SystemView);
        assert_eq!(table_kind("SEQUENCE"), TableKind::Table);
    }
}
