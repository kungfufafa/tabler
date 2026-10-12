//! SELECT / COUNT generation for table browsing: quoted identifiers, bind parameters, server-side
//! sort and filter, LIMIT/OFFSET paging.

use crate::error::AppError;
use crate::sqlgen::mutate::{TableSpec, row_key_where};
use crate::sqlgen::quote::Dialect;
use crate::types::browse::{
    Conjunction, FilterGroup, FilterNode, FilterOp, NullsOrder, RowIdentity, RowKey, SortDir,
    SortSpec,
};
use crate::types::schema::{ColumnMeta, TableRef, ValueKind};
use crate::types::value::Value;

/// SQL text plus its positional parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltSql {
    pub sql: String,
    pub params: Vec<Value>,
}

/// Name of the synthetic column carrying SQLite's rowid for tables without a primary key.
pub const ROWID_ALIAS: &str = "_tabler_rowid";

pub struct SelectSpec<'a> {
    pub dialect: Dialect,
    pub table: &'a TableRef,
    /// All columns of the table (for type casts and validation).
    pub columns: &'a [ColumnMeta],
    /// Subset to select; `None` = all.
    pub selected: Option<&'a [String]>,
    pub filter: Option<&'a FilterGroup>,
    pub sort: &'a [SortSpec],
    pub limit: u32,
    pub offset: u64,
    pub pk: &'a [String],
    pub identity: RowIdentity,
}

struct ParamSink<'a> {
    dialect: Dialect,
    params: &'a mut Vec<Value>,
}

impl ParamSink<'_> {
    /// Pushes a value and returns its placeholder (with a PG cast when `cast` is given).
    fn push(&mut self, value: Value, cast: Option<&str>) -> String {
        self.params.push(value);
        let ph = self.dialect.placeholder(self.params.len());
        match (self.dialect, cast) {
            (Dialect::Postgres, Some(ty)) if !ty.is_empty() => format!("{ph}::{ty}"),
            _ => ph,
        }
    }
}

fn find_column<'a>(columns: &'a [ColumnMeta], name: &str) -> Result<&'a ColumnMeta, AppError> {
    columns
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| AppError::InvalidInput(format!("unknown column \"{name}\"")))
}

/// Column expression used in the select list. PG casts to text so one decoder covers every type.
fn select_expr(dialect: Dialect, col: &ColumnMeta) -> String {
    let ident = dialect.quote_ident(&col.name);
    match dialect {
        Dialect::Postgres => format!("{ident}::text AS {ident}"),
        _ => ident,
    }
}

fn op_sql(op: FilterOp) -> &'static str {
    match op {
        FilterOp::Eq => "=",
        FilterOp::Ne => "<>",
        FilterOp::Gt => ">",
        FilterOp::Lt => "<",
        FilterOp::Ge => ">=",
        FilterOp::Le => "<=",
        FilterOp::Like => "LIKE",
        FilterOp::NotLike => "NOT LIKE",
        FilterOp::In => "IN",
        FilterOp::NotIn => "NOT IN",
        FilterOp::IsNull => "IS NULL",
        FilterOp::IsNotNull => "IS NOT NULL",
        FilterOp::Between => "BETWEEN",
    }
}

fn condition_sql(
    sink: &mut ParamSink<'_>,
    columns: &[ColumnMeta],
    column: &str,
    op: FilterOp,
    values: &[Value],
) -> Result<String, AppError> {
    let col = find_column(columns, column)?;
    let dialect = sink.dialect;
    let ident = dialect.quote_ident(&col.name);
    let cast = col.data_type.as_str();
    let need = |n: usize| -> Result<(), AppError> {
        if values.len() < n {
            Err(AppError::InvalidInput(format!(
                "filter on \"{column}\" needs {n} value(s)"
            )))
        } else {
            Ok(())
        }
    };
    Ok(match op {
        FilterOp::IsNull | FilterOp::IsNotNull => format!("{ident} {}", op_sql(op)),
        FilterOp::Like | FilterOp::NotLike => {
            need(1)?;
            // Pattern matching always happens on the textual form.
            let lhs = if dialect == Dialect::Postgres {
                format!("{ident}::text")
            } else {
                ident
            };
            let ph = sink.push(values[0].clone(), None);
            format!("{lhs} {} {ph}", op_sql(op))
        }
        FilterOp::In | FilterOp::NotIn => {
            if values.is_empty() {
                // Empty IN list: never / always matches.
                return Ok(if op == FilterOp::In {
                    "1 = 0".into()
                } else {
                    "1 = 1".into()
                });
            }
            let phs: Vec<String> = values
                .iter()
                .map(|v| sink.push(v.clone(), Some(cast)))
                .collect();
            format!("{ident} {} ({})", op_sql(op), phs.join(", "))
        }
        FilterOp::Between => {
            need(2)?;
            let a = sink.push(values[0].clone(), Some(cast));
            let b = sink.push(values[1].clone(), Some(cast));
            format!("{ident} BETWEEN {a} AND {b}")
        }
        _ => {
            need(1)?;
            let ph = sink.push(values[0].clone(), Some(cast));
            format!("{ident} {} {ph}", op_sql(op))
        }
    })
}

fn group_sql(
    sink: &mut ParamSink<'_>,
    columns: &[ColumnMeta],
    group: &FilterGroup,
) -> Result<Option<String>, AppError> {
    let mut parts = Vec::new();
    for item in &group.items {
        match item {
            FilterNode::Condition { column, op, values } => {
                parts.push(condition_sql(sink, columns, column, *op, values)?);
            }
            FilterNode::Group(g) => {
                if let Some(sql) = group_sql(sink, columns, g)? {
                    parts.push(format!("({sql})"));
                }
            }
            FilterNode::Raw { sql } => {
                let trimmed = sql.trim();
                if !trimmed.is_empty() {
                    parts.push(format!("({trimmed})"));
                }
            }
        }
    }
    if parts.is_empty() {
        return Ok(None);
    }
    let joiner = match group.conjunction {
        Conjunction::And => " AND ",
        Conjunction::Or => " OR ",
    };
    Ok(Some(parts.join(joiner)))
}

/// Builds the WHERE clause body (without the keyword) and appends its params.
pub fn build_where(
    dialect: Dialect,
    columns: &[ColumnMeta],
    filter: Option<&FilterGroup>,
    params: &mut Vec<Value>,
) -> Result<Option<String>, AppError> {
    let Some(filter) = filter else {
        return Ok(None);
    };
    let mut sink = ParamSink { dialect, params };
    group_sql(&mut sink, columns, filter)
}

fn order_by(
    dialect: Dialect,
    columns: &[ColumnMeta],
    sort: &[SortSpec],
    pk: &[String],
    identity: RowIdentity,
) -> Result<String, AppError> {
    let mut terms: Vec<String> = Vec::new();
    for s in sort {
        find_column(columns, &s.column)?;
        let ident = dialect.quote_ident(&s.column);
        let dir = match s.dir {
            SortDir::Asc => "ASC",
            SortDir::Desc => "DESC",
        };
        match (dialect, s.nulls) {
            (Dialect::Mysql, Some(nulls)) => {
                // MySQL has no NULLS FIRST/LAST: sort on the null flag first.
                let flag_dir = match nulls {
                    NullsOrder::First => "DESC",
                    NullsOrder::Last => "ASC",
                };
                terms.push(format!("({ident} IS NULL) {flag_dir}"));
                terms.push(format!("{ident} {dir}"));
            }
            (_, Some(NullsOrder::First)) => terms.push(format!("{ident} {dir} NULLS FIRST")),
            (_, Some(NullsOrder::Last)) => terms.push(format!("{ident} {dir} NULLS LAST")),
            (_, None) => terms.push(format!("{ident} {dir}")),
        }
    }
    if terms.is_empty() {
        // Stable paging needs a deterministic order: fall back to the primary key / rowid.
        if !pk.is_empty() {
            terms.extend(pk.iter().map(|c| format!("{} ASC", dialect.quote_ident(c))));
        } else if identity == RowIdentity::Rowid && dialect == Dialect::Sqlite {
            terms.push("rowid ASC".into());
        }
    }
    Ok(if terms.is_empty() {
        String::new()
    } else {
        format!(" ORDER BY {}", terms.join(", "))
    })
}

/// Builds the paged SELECT for a table browse.
pub fn build_select(spec: &SelectSpec<'_>) -> Result<BuiltSql, AppError> {
    let d = spec.dialect;
    let cols: Vec<&ColumnMeta> = match spec.selected {
        Some(names) => names
            .iter()
            .map(|n| find_column(spec.columns, n))
            .collect::<Result<_, _>>()?,
        None => spec.columns.iter().collect(),
    };
    if cols.is_empty() {
        return Err(AppError::InvalidInput("no columns to select".into()));
    }
    let mut select_list: Vec<String> = cols.iter().map(|c| select_expr(d, c)).collect();
    if spec.identity == RowIdentity::Rowid && d == Dialect::Sqlite {
        select_list.push(format!("rowid AS {}", d.quote_ident(ROWID_ALIAS)));
    }
    let mut params = Vec::new();
    let where_sql = build_where(d, spec.columns, spec.filter, &mut params)?;
    let order = order_by(d, spec.columns, spec.sort, spec.pk, spec.identity)?;
    let mut sql = format!(
        "SELECT {}\nFROM {}",
        select_list.join(", "),
        d.qualify(spec.table.schema.as_deref(), &spec.table.name)
    );
    if let Some(w) = where_sql {
        sql.push_str("\nWHERE ");
        sql.push_str(&w);
    }
    sql.push_str(&order);
    sql.push_str(&format!("\nLIMIT {} OFFSET {}", spec.limit, spec.offset));
    Ok(BuiltSql { sql, params })
}

/// `SELECT count(*) FROM table WHERE …`.
pub fn build_count(
    dialect: Dialect,
    table: &TableRef,
    columns: &[ColumnMeta],
    filter: Option<&FilterGroup>,
) -> Result<BuiltSql, AppError> {
    let mut params = Vec::new();
    let where_sql = build_where(dialect, columns, filter, &mut params)?;
    let mut sql = format!(
        "SELECT count(*) FROM {}",
        dialect.qualify(table.schema.as_deref(), &table.name)
    );
    if let Some(w) = where_sql {
        sql.push_str(" WHERE ");
        sql.push_str(&w);
    }
    Ok(BuiltSql { sql, params })
}

/// `SELECT <column> FROM table WHERE <row key> LIMIT 1` for one full cell (the inspector's "load
/// full value"), with exactly one `LIMIT 1`.
///
/// Result shape (the caller decodes it):
/// - PostgreSQL / MySQL: one column. Bytes columns come back as hex text (`NULL` stays `NULL`); PG
///   casts everything else to text like the browse query does.
/// - SQLite: two columns, `<column>` itself and its hex when — and only when — the cell's storage
///   class is blob. SQLite is dynamically typed, so a blob can sit in an untyped or TEXT column; the
///   declared type cannot decide whether to hex-encode it.
///
/// A fetch is read-only, so an all-columns key (no primary key, e.g. a view) becomes a plain
/// predicate list. `row_key_where`'s `ctid` / `rowid` subqueries exist to pin exactly one row for
/// UPDATE/DELETE — and views have neither column.
pub fn build_fetch_cell(
    spec: &TableSpec<'_>,
    column: &ColumnMeta,
    key: &RowKey,
) -> Result<(String, Vec<Value>), AppError> {
    let d = spec.dialect;
    let ident = d.quote_ident(&column.name);
    let bytes = column.kind == ValueKind::Bytes;
    let expr = match d {
        Dialect::Postgres if bytes => format!("encode({ident}, 'hex')"),
        Dialect::Postgres => format!("{ident}::text"),
        Dialect::Mysql if bytes => format!("HEX({ident})"),
        Dialect::Mysql => ident,
        Dialect::Sqlite => {
            format!("{ident}, CASE WHEN typeof({ident}) = 'blob' THEN hex({ident}) END")
        }
    };
    let mut params = Vec::new();
    let (condition, suffix) = match key {
        RowKey::AllColumns(map) => (
            all_columns_predicate(spec, map, &mut params)?,
            String::new(),
        ),
        _ => {
            let w = row_key_where(spec, key, &mut params)?;
            (w.condition, w.suffix)
        }
    };
    let limit = if suffix.is_empty() { " LIMIT 1" } else { "" };
    let sql = format!(
        "SELECT {expr} FROM {} WHERE {condition}{suffix}{limit}",
        d.qualify(spec.table.schema.as_deref(), &spec.table.name),
    );
    Ok((sql, params))
}

/// `col1 <null-safe => ? AND …` for every key column — the same comparisons `row_key_where` uses
/// for an all-columns key, without its single-row subquery.
fn all_columns_predicate(
    spec: &TableSpec<'_>,
    map: &std::collections::BTreeMap<String, Value>,
    params: &mut Vec<Value>,
) -> Result<String, AppError> {
    if map.is_empty() {
        return Err(AppError::InvalidInput("row key is empty".into()));
    }
    let d = spec.dialect;
    let mut sink = ParamSink { dialect: d, params };
    let mut parts = Vec::with_capacity(map.len());
    for (name, v) in map {
        find_column(spec.columns, name)?;
        let ident = d.quote_ident(name);
        parts.push(match d {
            // Compare textual forms: works for every type, including json and arrays.
            Dialect::Postgres => {
                let ph = sink.push(
                    v.as_text().map(Value::Text).unwrap_or(Value::Null),
                    Some("text"),
                );
                format!("{ident}::text IS NOT DISTINCT FROM {ph}")
            }
            Dialect::Mysql => format!("{ident} <=> {}", sink.push(v.clone(), None)),
            Dialect::Sqlite => format!("{ident} IS {}", sink.push(v.clone(), None)),
        });
    }
    Ok(parts.join(" AND "))
}

/// Renders a `Value` as a SQL literal for display (Preview SQL). Never used for execution.
pub fn literal(dialect: Dialect, v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Default => "DEFAULT".into(),
        Value::Bool(b) => match dialect {
            Dialect::Mysql | Dialect::Sqlite => {
                if *b {
                    "1".into()
                } else {
                    "0".into()
                }
            }
            Dialect::Postgres => {
                if *b {
                    "TRUE".into()
                } else {
                    "FALSE".into()
                }
            }
        },
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::BigInt(s) | Value::Decimal(s) => s.clone(),
        Value::Bytes(b) => {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&b.base64)
                .unwrap_or_default();
            let hex: String = bytes.iter().map(|x| format!("{x:02x}")).collect();
            match dialect {
                Dialect::Postgres => format!("'\\x{hex}'::bytea"),
                Dialect::Mysql => format!("X'{hex}'"),
                Dialect::Sqlite => format!("X'{hex}'"),
            }
        }
        other => dialect.quote_literal(&other.as_text().unwrap_or_default()),
    }
}

/// Replaces placeholders with literals for display. Only safe for SQL generated by this module.
pub fn inline_params(dialect: Dialect, sql: &str, params: &[Value]) -> String {
    match dialect {
        Dialect::Mysql => {
            let mut out = String::with_capacity(sql.len());
            let mut it = params.iter();
            let mut in_str = false;
            for ch in sql.chars() {
                if ch == '\'' {
                    in_str = !in_str;
                }
                if ch == '?' && !in_str {
                    match it.next() {
                        Some(v) => out.push_str(&literal(dialect, v)),
                        None => out.push('?'),
                    }
                } else {
                    out.push(ch);
                }
            }
            out
        }
        Dialect::Postgres | Dialect::Sqlite => {
            let prefix = if dialect == Dialect::Postgres {
                '$'
            } else {
                '?'
            };
            let mut out = String::with_capacity(sql.len());
            let chars: Vec<char> = sql.chars().collect();
            let mut i = 0;
            let mut in_str = false;
            while i < chars.len() {
                let ch = chars[i];
                if ch == '\'' {
                    in_str = !in_str;
                }
                if ch == prefix && !in_str {
                    let start = i + 1;
                    let mut end = start;
                    while end < chars.len() && chars[end].is_ascii_digit() {
                        end += 1;
                    }
                    if end > start {
                        let n: usize = chars[start..end]
                            .iter()
                            .collect::<String>()
                            .parse()
                            .unwrap_or(0);
                        if let Some(v) = n.checked_sub(1).and_then(|k| params.get(k)) {
                            out.push_str(&literal(dialect, v));
                            i = end;
                            continue;
                        }
                    }
                }
                out.push(ch);
                i += 1;
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::schema::ValueKind;

    fn col(name: &str, data_type: &str, kind: ValueKind, pk: bool) -> ColumnMeta {
        ColumnMeta {
            name: name.into(),
            ordinal: 1,
            data_type: data_type.into(),
            kind,
            nullable: !pk,
            default_expr: None,
            is_pk: pk,
            pk_ordinal: if pk { Some(1) } else { None },
            is_auto_increment: false,
            is_generated: false,
            is_unsigned: false,
            comment: None,
            enum_values: None,
            char_max_len: None,
            numeric_precision: None,
            numeric_scale: None,
            collation: None,
            array_elem_type: None,
            extra: None,
        }
    }

    fn users() -> Vec<ColumnMeta> {
        vec![
            col("id", "bigint", ValueKind::BigInt, true),
            col("name", "text", ValueKind::Text, false),
            col("age", "integer", ValueKind::Int, false),
            col("status", "user_status", ValueKind::Enum, false),
        ]
    }

    fn filter() -> FilterGroup {
        FilterGroup {
            conjunction: Conjunction::Or,
            items: vec![
                FilterNode::Group(FilterGroup {
                    conjunction: Conjunction::And,
                    items: vec![
                        FilterNode::Condition {
                            column: "status".into(),
                            op: FilterOp::Eq,
                            values: vec![Value::Text("active".into())],
                        },
                        FilterNode::Condition {
                            column: "age".into(),
                            op: FilterOp::Ge,
                            values: vec![Value::Int(18)],
                        },
                    ],
                }),
                FilterNode::Condition {
                    column: "name".into(),
                    op: FilterOp::Like,
                    values: vec![Value::Text("%bob%".into())],
                },
            ],
        }
    }

    #[test]
    fn postgres_select_casts_everything_to_text() {
        let cols = users();
        let table = TableRef::new(Some("public"), "users");
        let f = filter();
        let sort = vec![SortSpec {
            column: "name".into(),
            dir: SortDir::Desc,
            nulls: Some(NullsOrder::Last),
        }];
        let built = build_select(&SelectSpec {
            dialect: Dialect::Postgres,
            table: &table,
            columns: &cols,
            selected: None,
            filter: Some(&f),
            sort: &sort,
            limit: 200,
            offset: 400,
            pk: &["id".into()],
            identity: RowIdentity::Pk,
        })
        .unwrap();
        assert_eq!(
            built.sql,
            "SELECT \"id\"::text AS \"id\", \"name\"::text AS \"name\", \"age\"::text AS \"age\", \"status\"::text AS \"status\"\nFROM \"public\".\"users\"\nWHERE (\"status\" = $1::user_status AND \"age\" >= $2::integer) OR \"name\"::text LIKE $3 ORDER BY \"name\" DESC NULLS LAST\nLIMIT 200 OFFSET 400"
        );
        assert_eq!(
            built.params,
            vec![
                Value::Text("active".into()),
                Value::Int(18),
                Value::Text("%bob%".into())
            ]
        );
        let preview = inline_params(Dialect::Postgres, &built.sql, &built.params);
        assert!(preview.contains("\"status\" = 'active'::user_status AND \"age\" >= 18::integer"));
        assert!(preview.contains("LIKE '%bob%'"));
    }

    #[test]
    fn mysql_emulates_nulls_order_and_defaults_to_pk() {
        let cols = users();
        let table = TableRef::new(Some("app"), "users");
        let sort = vec![SortSpec {
            column: "age".into(),
            dir: SortDir::Asc,
            nulls: Some(NullsOrder::Last),
        }];
        let built = build_select(&SelectSpec {
            dialect: Dialect::Mysql,
            table: &table,
            columns: &cols,
            selected: Some(&["id".into(), "name".into()]),
            filter: None,
            sort: &sort,
            limit: 100,
            offset: 0,
            pk: &["id".into()],
            identity: RowIdentity::Pk,
        })
        .unwrap();
        assert_eq!(
            built.sql,
            "SELECT `id`, `name`\nFROM `app`.`users` ORDER BY (`age` IS NULL) ASC, `age` ASC\nLIMIT 100 OFFSET 0"
        );
        let built = build_select(&SelectSpec {
            dialect: Dialect::Mysql,
            table: &table,
            columns: &cols,
            selected: None,
            filter: None,
            sort: &[],
            limit: 100,
            offset: 0,
            pk: &["id".into()],
            identity: RowIdentity::Pk,
        })
        .unwrap();
        assert!(built.sql.ends_with("ORDER BY `id` ASC\nLIMIT 100 OFFSET 0"));
    }

    #[test]
    fn sqlite_rowid_identity_adds_rowid_column() {
        let cols = vec![col("a", "TEXT", ValueKind::Text, false)];
        let table = TableRef::new(Some("main"), "audit_log");
        let built = build_select(&SelectSpec {
            dialect: Dialect::Sqlite,
            table: &table,
            columns: &cols,
            selected: None,
            filter: None,
            sort: &[],
            limit: 50,
            offset: 0,
            pk: &[],
            identity: RowIdentity::Rowid,
        })
        .unwrap();
        assert_eq!(
            built.sql,
            "SELECT \"a\", rowid AS \"_tabler_rowid\"\nFROM \"main\".\"audit_log\" ORDER BY rowid ASC\nLIMIT 50 OFFSET 0"
        );
    }

    #[test]
    fn in_between_null_and_raw_filters() {
        let cols = users();
        let table = TableRef::new(None, "users");
        let f = FilterGroup {
            conjunction: Conjunction::And,
            items: vec![
                FilterNode::Condition {
                    column: "id".into(),
                    op: FilterOp::In,
                    values: vec![Value::Int(1), Value::Int(2)],
                },
                FilterNode::Condition {
                    column: "age".into(),
                    op: FilterOp::Between,
                    values: vec![Value::Int(1), Value::Int(9)],
                },
                FilterNode::Condition {
                    column: "name".into(),
                    op: FilterOp::IsNotNull,
                    values: vec![],
                },
                FilterNode::Condition {
                    column: "status".into(),
                    op: FilterOp::NotIn,
                    values: vec![],
                },
                FilterNode::Raw {
                    sql: "age <> 7".into(),
                },
            ],
        };
        let built = build_count(Dialect::Sqlite, &table, &cols, Some(&f)).unwrap();
        assert_eq!(
            built.sql,
            "SELECT count(*) FROM \"users\" WHERE \"id\" IN (?1, ?2) AND \"age\" BETWEEN ?3 AND ?4 AND \"name\" IS NOT NULL AND 1 = 1 AND (age <> 7)"
        );
        assert_eq!(built.params.len(), 4);
        assert_eq!(
            inline_params(Dialect::Sqlite, &built.sql, &built.params),
            "SELECT count(*) FROM \"users\" WHERE \"id\" IN (1, 2) AND \"age\" BETWEEN 1 AND 9 AND \"name\" IS NOT NULL AND 1 = 1 AND (age <> 7)"
        );
    }

    #[test]
    fn rejects_unknown_columns() {
        let cols = users();
        let table = TableRef::new(None, "users");
        let f = FilterGroup {
            conjunction: Conjunction::And,
            items: vec![FilterNode::Condition {
                column: "nope\"; DROP".into(),
                op: FilterOp::Eq,
                values: vec![Value::Int(1)],
            }],
        };
        assert!(build_count(Dialect::Postgres, &table, &cols, Some(&f)).is_err());
        let sort = vec![SortSpec {
            column: "nope".into(),
            dir: SortDir::Asc,
            nulls: None,
        }];
        assert!(
            build_select(&SelectSpec {
                dialect: Dialect::Postgres,
                table: &table,
                columns: &cols,
                selected: None,
                filter: None,
                sort: &sort,
                limit: 1,
                offset: 0,
                pk: &[],
                identity: RowIdentity::None
            })
            .is_err()
        );
    }

    #[test]
    fn literals_per_dialect() {
        assert_eq!(literal(Dialect::Postgres, &Value::Bool(true)), "TRUE");
        assert_eq!(literal(Dialect::Mysql, &Value::Bool(true)), "1");
        assert_eq!(
            literal(Dialect::Mysql, &Value::Text("it's".into())),
            "'it\\'s'"
        );
        assert_eq!(literal(Dialect::Sqlite, &Value::Null), "NULL");
        assert_eq!(
            literal(Dialect::Postgres, &Value::from_bytes(&[0, 255])),
            "'\\x00ff'::bytea"
        );
        assert_eq!(
            inline_params(
                Dialect::Mysql,
                "a = ? AND b = '?' AND c = ?",
                &[Value::Int(1), Value::Int(2)]
            ),
            "a = 1 AND b = '?' AND c = 2"
        );
    }

    fn blob_table() -> Vec<ColumnMeta> {
        vec![
            col("id", "bigint", ValueKind::BigInt, true),
            col("name", "text", ValueKind::Text, false),
            col("data", "bytea", ValueKind::Bytes, false),
        ]
    }

    fn keys() -> [RowKey; 3] {
        use std::collections::BTreeMap;
        [
            RowKey::Pk(BTreeMap::from([("id".to_string(), Value::Int(7))])),
            RowKey::Rowid(42),
            RowKey::AllColumns(BTreeMap::from([
                ("id".to_string(), Value::Int(7)),
                ("name".to_string(), Value::Null),
            ])),
        ]
    }

    fn fetch_sql(dialect: Dialect, column: &str, key: &RowKey) -> (String, Vec<Value>) {
        let cols = blob_table();
        let table = TableRef::new(Some("app"), "files");
        let spec = TableSpec {
            dialect,
            table: &table,
            columns: &cols,
        };
        let col = cols.iter().find(|c| c.name == column).unwrap();
        build_fetch_cell(&spec, col, key).unwrap()
    }

    #[test]
    fn fetch_cell_postgres() {
        let [pk, rowid, all] = keys();
        let (sql, params) = fetch_sql(Dialect::Postgres, "data", &pk);
        assert_eq!(
            sql,
            "SELECT encode(\"data\", 'hex') FROM \"app\".\"files\" WHERE \"id\" = $1::bigint LIMIT 1"
        );
        assert_eq!(params, vec![Value::Int(7)]);
        let (sql, _) = fetch_sql(Dialect::Postgres, "name", &rowid);
        assert_eq!(
            sql,
            "SELECT \"name\"::text FROM \"app\".\"files\" WHERE rowid = $1 LIMIT 1"
        );
        // No ctid subquery: a view (always keyed by all columns) has no ctid column.
        let (sql, params) = fetch_sql(Dialect::Postgres, "name", &all);
        assert_eq!(
            sql,
            "SELECT \"name\"::text FROM \"app\".\"files\" WHERE \"id\"::text IS NOT DISTINCT FROM $1::text AND \"name\"::text IS NOT DISTINCT FROM $2::text LIMIT 1"
        );
        assert_eq!(params, vec![Value::Text("7".into()), Value::Null]);
        let (sql, _) = fetch_sql(Dialect::Postgres, "data", &all);
        assert_eq!(
            sql,
            "SELECT encode(\"data\", 'hex') FROM \"app\".\"files\" WHERE \"id\"::text IS NOT DISTINCT FROM $1::text AND \"name\"::text IS NOT DISTINCT FROM $2::text LIMIT 1"
        );
    }

    #[test]
    fn fetch_cell_mysql_has_a_single_limit() {
        let [pk, rowid, all] = keys();
        let (sql, _) = fetch_sql(Dialect::Mysql, "data", &pk);
        assert_eq!(
            sql,
            "SELECT HEX(`data`) FROM `app`.`files` WHERE `id` = ? LIMIT 1"
        );
        let (sql, _) = fetch_sql(Dialect::Mysql, "name", &rowid);
        assert_eq!(
            sql,
            "SELECT `name` FROM `app`.`files` WHERE rowid = ? LIMIT 1"
        );
        let (sql, params) = fetch_sql(Dialect::Mysql, "data", &all);
        assert_eq!(
            sql,
            "SELECT HEX(`data`) FROM `app`.`files` WHERE `id` <=> ? AND `name` <=> ? LIMIT 1"
        );
        assert_eq!(sql.matches("LIMIT 1").count(), 1);
        assert_eq!(params, vec![Value::Int(7), Value::Null]);
    }

    #[test]
    fn fetch_cell_sqlite() {
        let [pk, rowid, all] = keys();
        // The storage class decides, not the declared type: any column may hold a blob.
        let (sql, _) = fetch_sql(Dialect::Sqlite, "data", &pk);
        assert_eq!(
            sql,
            "SELECT \"data\", CASE WHEN typeof(\"data\") = 'blob' THEN hex(\"data\") END FROM \"app\".\"files\" WHERE \"id\" = ?1 LIMIT 1"
        );
        let (sql, params) = fetch_sql(Dialect::Sqlite, "name", &rowid);
        assert_eq!(
            sql,
            "SELECT \"name\", CASE WHEN typeof(\"name\") = 'blob' THEN hex(\"name\") END FROM \"app\".\"files\" WHERE rowid = ?1 LIMIT 1"
        );
        assert_eq!(params, vec![Value::Int(42)]);
        // No rowid subquery: a view (always keyed by all columns) has no rowid.
        let (sql, params) = fetch_sql(Dialect::Sqlite, "data", &all);
        assert_eq!(
            sql,
            "SELECT \"data\", CASE WHEN typeof(\"data\") = 'blob' THEN hex(\"data\") END FROM \"app\".\"files\" WHERE \"id\" IS ?1 AND \"name\" IS ?2 LIMIT 1"
        );
        assert_eq!(params, vec![Value::Int(7), Value::Null]);
    }

    #[test]
    fn fetch_cell_view_keys_never_use_ctid_or_rowid() {
        let [_, _, all] = keys();
        for d in [Dialect::Postgres, Dialect::Mysql, Dialect::Sqlite] {
            for column in ["id", "name", "data"] {
                let (sql, _) = fetch_sql(d, column, &all);
                assert!(!sql.contains("ctid") && !sql.contains("rowid"), "{sql}");
                assert_eq!(sql.matches("LIMIT 1").count(), 1, "{sql}");
                assert!(!sql.contains("(SELECT"), "{sql}");
            }
        }
    }

    #[test]
    fn fetch_cell_rejects_empty_or_unknown_all_columns_keys() {
        use std::collections::BTreeMap;
        let cols = blob_table();
        let table = TableRef::new(None, "v");
        for dialect in [Dialect::Postgres, Dialect::Mysql, Dialect::Sqlite] {
            let spec = TableSpec {
                dialect,
                table: &table,
                columns: &cols,
            };
            let empty = RowKey::AllColumns(BTreeMap::new());
            assert!(build_fetch_cell(&spec, &cols[2], &empty).is_err());
            let unknown = RowKey::AllColumns(BTreeMap::from([("nope".to_string(), Value::Int(1))]));
            assert!(build_fetch_cell(&spec, &cols[2], &unknown).is_err());
        }
    }

    #[test]
    fn fetch_cell_rejects_unknown_key_columns() {
        use std::collections::BTreeMap;
        let cols = blob_table();
        let table = TableRef::new(None, "files");
        let spec = TableSpec {
            dialect: Dialect::Postgres,
            table: &table,
            columns: &cols,
        };
        let key = RowKey::Pk(BTreeMap::from([("nope".to_string(), Value::Int(1))]));
        assert!(build_fetch_cell(&spec, &cols[1], &key).is_err());
    }
}
