//! CQL (the cassandra lane): Apache Cassandra 3.0+ and ScyllaDB. Built with `drv-cassandra` only.
//!
//! Double-quoted names (`""` doubled), single-quoted strings (`''` doubled; a backslash is a plain
//! character), `$$…$$` bodies without tags, `--`, `//` and flat `/* */` comments, `?` binds. The
//! server runs one statement per request, and a batch (`BEGIN [UNLOGGED | COUNTER] BATCH …
//! APPLY BATCH`) is one statement whose inner `;` end nothing, so the editor splits with
//! `split_cql`.
//!
//! CQL filters and sorts very little: there is no casting in a WHERE, no NULL ordering, no
//! case-insensitive match, no OR and no offset (later pages need the driver's paging state).
//! Writes are upserts with no row count, so the lane proves them (lightweight transactions)
//! before EditRows is on. The DDL text of `table_meta` (when the server has no DESCRIBE) is
//! rebuilt here from `system_schema` rows.

use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, PLAIN, TokenKind, scan};
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, NullSafeEq, NullsSort, Paging, PlaceholderStyle, RedactRule, SqlHooks,
    SqlKnobs, double_quoted, explain_unsupported, hex_0x, ph_question, single_quoted,
};
use crate::sqlgen::mutate::{Cell, cells};
use crate::sqlgen::portable::select::{Params, find_column, ident, table_name};
use crate::sqlgen::portable::{classify, split};
use crate::sqlgen::select::BuiltSql;
use crate::sqlgen::split::SplitStatement;
use crate::sqlgen::sql::{SelectQuery, TableTarget};
use crate::types::browse::{Conjunction, FilterGroup, FilterNode, FilterOp, RowKey};
use crate::types::mutate::{ServerExpr, StagedChange};
use crate::types::schema::{ColumnMeta, TableRef, ValueKind};
use crate::types::value::Value;

pub static LEX: LexProfile = LexProfile {
    ident_quotes: &[('"', '"')],
    string_quotes: &['\''],
    // CQL strings take no backslash escapes, on every server.
    readings: &[PLAIN],
    e_strings: false,
    dollar_quotes: true,
    // CQL has only `$$` bodies, and `$a$` is no token at all.
    dollar_tags: false,
    triple_quotes: false,
    raw_prefixes: false,
    byte_prefixes: false,
    hash_comments: false,
    slash_slash_comments: true,
    // Cassandra's and ScyllaDB's lexers end a block comment at its first `*/`.
    nested_block_comments: false,
    dash_comment_needs_space: false,
    batch_separator: None,
    terminator_optional: false,
    // A batch ends with `APPLY BATCH`, not `END`: `split_cql` keeps it whole.
    begin_end_bodies: false,
    attribute_calls: false,
};

pub static RULES: ClassifyRules = ClassifyRules {
    queries: &["SELECT"],
    // Server-side DESCRIBE (Cassandra 4.0+, ScyllaDB 5.2+) and LIST ROLES / USERS / PERMISSIONS
    // only read; USE switches the keyspace. CONSISTENCY, SERIAL CONSISTENCY, TRACING, PAGING and
    // EXPAND are cqlsh's commands: the session handles them itself (`cassandra::cqlsh`, which
    // refuses trailing text), and the server refuses them as syntax errors.
    // An inspection reads whatever follows it (`USE ks DELETE FROM t` is a Read). That is safe
    // here, for these words and the cqlsh ones alike: Cassandra and ScyllaDB parse exactly one
    // statement per request (`cqlStatement ';'* EOF`), so text after it is a syntax error and
    // nothing runs.
    inspections: &[
        "DESCRIBE",
        "DESC",
        "USE",
        "LIST",
        "CONSISTENCY",
        "SERIAL",
        "TRACING",
        "PAGING",
        "EXPAND",
    ],
    // All reserved in CQL, so no read can use one as a bare name.
    writes: &[
        "INTO", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "TRUNCATE", "BATCH",
        "APPLY", "BEGIN", "GRANT", "REVOKE",
    ],
    // CQL functions cannot write (UDFs run sandboxed), and there are no table functions.
    write_calls: &[],
    table_functions: None,
};

fn classify_cql(sql: &str) -> StatementClass {
    classify::with(&LEX, &RULES, sql)
}

/// No column can be filtered as text: a WHERE takes no casts.
fn no_text(_: &str, _: &ColumnMeta) -> Option<String> {
    None
}

/// The clocks CQL can write: a new time-based UUID for a timeuuid column, and the timestamp and
/// date of one (`toTimestamp(now())`, `toDate(now())`, which Cassandra 2.2+ and ScyllaDB take).
/// CQL timestamps hold milliseconds, so a precision is not offered.
fn cql_server_expr(expr: ServerExpr, kind: ValueKind) -> Option<String> {
    let text = match (expr, kind) {
        (ServerExpr::Now, ValueKind::Uuid) => "now()",
        (
            ServerExpr::Now | ServerExpr::CurrentTimestamp,
            ValueKind::Timestamp | ValueKind::TimestampTz,
        ) => "toTimestamp(now())",
        (ServerExpr::CurrentDate, ValueKind::Date) => "toDate(now())",
        _ => return None,
    };
    Some(text.into())
}

/// Splits editor text into the statements the server runs: the portable split, with a batch
/// (`BEGIN [UNLOGGED | COUNTER] BATCH` up to the statement that ends with `APPLY BATCH`, or the
/// end of the text) kept as one statement.
pub fn split_cql(text: &str) -> Vec<SplitStatement> {
    let parts = split::split(&LEX, text);
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::with_capacity(parts.len());
    let mut parts = parts.into_iter();
    while let Some(first) = parts.next() {
        let words = code_words(&first.sql);
        if !opens_batch(&words) || closes_batch(&words) {
            out.push(first);
            continue;
        }
        let mut end = first.end;
        for next in parts.by_ref() {
            end = next.end;
            if closes_batch(&code_words(&next.sql)) {
                break;
            }
        }
        out.push(SplitStatement {
            sql: chars[first.start..end].iter().collect(),
            start: first.start,
            end,
            line: first.line,
        });
    }
    out
}

/// The tokens of code of a statement, upper case: words as they are, any other token as its
/// kind (strings, quoted names and punctuation never count as keywords).
fn code_words(sql: &str) -> Vec<String> {
    let s = scan(&LEX, PLAIN, sql);
    s.tokens
        .iter()
        .filter_map(|t| match t.kind {
            TokenKind::Space | TokenKind::Comment => None,
            TokenKind::Word => Some(s.upper(t)),
            TokenKind::Ident | TokenKind::Str | TokenKind::Punct => Some(String::new()),
        })
        .collect()
}

/// `BEGIN [UNLOGGED | COUNTER] BATCH …`
fn opens_batch(words: &[String]) -> bool {
    let w = |i: usize| words.get(i).map(String::as_str);
    w(0) == Some("BEGIN")
        && (w(1) == Some("BATCH")
            || (matches!(w(1), Some("UNLOGGED" | "COUNTER")) && w(2) == Some("BATCH")))
}

/// `… APPLY BATCH`
fn closes_batch(words: &[String]) -> bool {
    matches!(words, [.., a, b] if a == "APPLY" && b == "BATCH")
}

// The grid's statements. CQL filters with AND only, on a column and a value (no OR, no IS NULL,
// no casts, no negation), and a filter on anything but the partition key needs ALLOW FILTERING,
// which comes after LIMIT. There is no OFFSET: the session pages with the driver's paging state.
// Rows are written by their full primary key, as lightweight transactions (`IF EXISTS`, `IF NOT
// EXISTS`) whose `[applied]` proves that the one row was there (or was not), since a plain CQL
// write is an upsert that reports nothing.

/// The filter operators the grid's WHERE takes.
pub const FILTER_OPS: [FilterOp; 8] = [
    FilterOp::Eq,
    FilterOp::Gt,
    FilterOp::Lt,
    FilterOp::Ge,
    FilterOp::Le,
    FilterOp::In,
    FilterOp::Like,
    FilterOp::Between,
];

fn refused(why: &str) -> AppError {
    AppError::InvalidInput(format!("CQL {why}"))
}

/// The conditions of a filter (AND-ed), appending their values.
fn conditions(
    columns: &[ColumnMeta],
    group: &FilterGroup,
    params: &mut Params<'_>,
    out: &mut Vec<String>,
) -> Result<(), AppError> {
    let counted = group
        .items
        .iter()
        .filter(|i| !matches!(i, FilterNode::Raw { sql } if sql.trim().is_empty()))
        .count();
    if group.conjunction == Conjunction::Or && counted > 1 {
        return Err(refused("has no OR: combine the filters with AND"));
    }
    for item in &group.items {
        match item {
            FilterNode::Condition { column, op, values } => {
                let col = find_column(columns, column)?;
                let id = ident(&SPEC, &col.name);
                let one = |values: &[Value]| -> Result<Value, AppError> {
                    match values.first() {
                        None => Err(AppError::InvalidInput(format!(
                            "filter on \"{column}\" needs a value"
                        ))),
                        Some(Value::Null) => Err(refused("cannot match NULL")),
                        Some(v) => Ok(v.clone()),
                    }
                };
                match op {
                    FilterOp::Eq
                    | FilterOp::Gt
                    | FilterOp::Lt
                    | FilterOp::Ge
                    | FilterOp::Le
                    | FilterOp::Like => {
                        let sign = match op {
                            FilterOp::Eq => "=",
                            FilterOp::Gt => ">",
                            FilterOp::Lt => "<",
                            FilterOp::Ge => ">=",
                            FilterOp::Le => "<=",
                            _ => "LIKE",
                        };
                        let ph = params.push(one(values)?, Some(col));
                        out.push(format!("{id} {sign} {ph}"));
                    }
                    FilterOp::In => {
                        if values.is_empty() || values.iter().any(Value::is_null) {
                            return Err(refused("IN needs values and cannot match NULL"));
                        }
                        let phs: Vec<String> = values
                            .iter()
                            .map(|v| params.push(v.clone(), Some(col)))
                            .collect();
                        out.push(format!("{id} IN ({})", phs.join(", ")));
                    }
                    FilterOp::Between => {
                        let low = one(values)?;
                        let high = one(values.get(1..).unwrap_or_default())?;
                        let a = params.push(low, Some(col));
                        let b = params.push(high, Some(col));
                        out.push(format!("{id} >= {a}"));
                        out.push(format!("{id} <= {b}"));
                    }
                    other => {
                        return Err(AppError::InvalidInput(format!(
                            "CQL cannot filter with {other:?}"
                        )));
                    }
                }
            }
            FilterNode::Group(g) => conditions(columns, g, params, out)?,
            // The user's own relations, as they are: CQL takes no parentheses around them.
            FilterNode::Raw { sql } => {
                let trimmed = sql.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
            FilterNode::AnyColumn { .. } => {
                return Err(refused("cannot filter on any column: pick one"));
            }
        }
    }
    Ok(())
}

/// The WHERE body of a filter and its values; None without conditions.
pub fn where_clause(
    columns: &[ColumnMeta],
    filter: Option<&FilterGroup>,
    values: &mut Vec<Value>,
) -> Result<Option<String>, AppError> {
    let Some(filter) = filter else {
        return Ok(None);
    };
    let mut parts = Vec::new();
    conditions(columns, filter, &mut Params::new(&SPEC, values), &mut parts)?;
    if values.len() > SPEC.max_params as usize {
        return Err(AppError::InvalidInput(format!(
            "the filter needs {} values, more than one query can bind ({})",
            values.len(),
            SPEC.max_params
        )));
    }
    Ok((!parts.is_empty()).then(|| parts.join(" AND ")))
}

/// `SELECT cols FROM "ks"."t" [WHERE …] [LIMIT n] [ALLOW FILTERING]`. The session pages it
/// without a limit; `select` (the facade's) caps it at the page.
pub fn browse_select(q: &SelectQuery<'_>, limit: Option<u32>) -> Result<BuiltSql, AppError> {
    if !q.sort.is_empty() {
        return Err(refused(
            "sorts only by clustering columns within one partition: the grid keeps the table's \
             order",
        ));
    }
    if q.offset > 0 {
        return Err(refused("cannot skip rows: pages follow one another"));
    }
    let names: Vec<&str> = match q.selected {
        Some(names) => names.iter().map(String::as_str).collect(),
        None => q.columns.iter().map(|c| c.name.as_str()).collect(),
    };
    if names.is_empty() {
        return Err(AppError::InvalidInput("no columns to select".into()));
    }
    let mut list = Vec::with_capacity(names.len());
    for n in names {
        find_column(q.columns, n)?;
        list.push(ident(&SPEC, n));
    }
    let mut params = Vec::new();
    let condition = where_clause(q.columns, q.filter, &mut params)?;
    let mut sql = format!(
        "SELECT {} FROM {}",
        list.join(", "),
        table_name(&SPEC, q.table)
    );
    if let Some(w) = &condition {
        sql.push_str(" WHERE ");
        sql.push_str(w);
    }
    if let Some(n) = limit {
        sql.push_str(&format!(" LIMIT {n}"));
    }
    if condition.is_some() {
        sql.push_str(" ALLOW FILTERING");
    }
    Ok(BuiltSql { sql, params })
}

fn select_hook(_: &DialectSpec, q: &SelectQuery<'_>) -> Result<BuiltSql, AppError> {
    browse_select(q, Some(q.limit))
}

fn count_hook(
    _: &DialectSpec,
    table: &TableRef,
    columns: &[ColumnMeta],
    filter: Option<&FilterGroup>,
) -> Result<BuiltSql, AppError> {
    count_select(table, columns, filter)
}

/// `SELECT count(*) FROM "ks"."t" [WHERE …] [ALLOW FILTERING]`: a scan of every partition.
pub fn count_select(
    table: &TableRef,
    columns: &[ColumnMeta],
    filter: Option<&FilterGroup>,
) -> Result<BuiltSql, AppError> {
    let mut params = Vec::new();
    let condition = where_clause(columns, filter, &mut params)?;
    let mut sql = format!("SELECT count(*) FROM {}", table_name(&SPEC, table));
    if let Some(w) = condition {
        sql.push_str(&format!(" WHERE {w} ALLOW FILTERING"));
    }
    Ok(BuiltSql { sql, params })
}

/// `"pk" = ? AND "ck" = ?` of a row's full primary key: every key column and no other, so a
/// write or a read never reaches more than that row (a partial clustering key would delete a
/// range of rows).
fn key_condition(
    columns: &[ColumnMeta],
    key: &RowKey,
    params: &mut Params<'_>,
) -> Result<String, AppError> {
    let RowKey::Pk(map) = key else {
        return Err(refused("addresses a row by its primary key only"));
    };
    let key_columns: Vec<&ColumnMeta> = columns.iter().filter(|c| c.is_pk).collect();
    if key_columns.is_empty() || map.len() != key_columns.len() {
        return Err(refused(
            "addresses a row by its whole primary key (partition and clustering columns)",
        ));
    }
    let mut parts = Vec::with_capacity(key_columns.len());
    for col in key_columns {
        let value = map.get(&col.name).ok_or_else(|| {
            refused("addresses a row by its whole primary key (partition and clustering columns)")
        })?;
        if value.is_null() {
            return Err(refused("key columns are never NULL"));
        }
        let ph = params.push(value.clone(), Some(col));
        parts.push(format!("{} = {ph}", ident(&SPEC, &col.name)));
    }
    Ok(parts.join(" AND "))
}

fn fetch_cell_hook(
    _: &DialectSpec,
    t: &TableTarget<'_>,
    column: &ColumnMeta,
    key: &RowKey,
) -> Result<(String, Vec<Value>), AppError> {
    let built = fetch_cell_select(t, column, key)?;
    Ok((built.sql, built.params))
}

/// `SELECT "c" FROM "ks"."t" WHERE <full key>`.
pub fn fetch_cell_select(
    t: &TableTarget<'_>,
    column: &ColumnMeta,
    key: &RowKey,
) -> Result<BuiltSql, AppError> {
    let mut params = Vec::new();
    let condition = key_condition(t.columns, key, &mut Params::new(&SPEC, &mut params))?;
    Ok(BuiltSql {
        sql: format!(
            "SELECT {} FROM {} WHERE {condition}",
            ident(&SPEC, &column.name),
            table_name(&SPEC, t.table)
        ),
        params,
    })
}

/// A value or server expression of a written cell.
fn cell_sql(
    cell: &Cell<'_>,
    col: &ColumnMeta,
    params: &mut Params<'_>,
) -> Result<String, AppError> {
    match cell {
        Cell::Value(Value::Default) | Cell::Expr(ServerExpr::Default) => {
            Err(refused("has no DEFAULT: enter the value explicitly"))
        }
        Cell::Expr(e) => cql_server_expr(*e, col.kind).ok_or_else(|| {
            AppError::Unsupported(format!("CQL has no {} for \"{}\"", e.wire(), col.name))
        }),
        Cell::Value(Value::Null) => Ok("NULL".into()),
        Cell::Value(v) => Ok(params.push((*v).clone(), Some(col))),
    }
}

/// UPDATE … IF EXISTS, INSERT … IF NOT EXISTS, DELETE … IF EXISTS of one staged change: each
/// must apply to exactly one row, which its `[applied]` says.
fn change_hook(
    _: &DialectSpec,
    t: &TableTarget<'_>,
    change: &StagedChange,
    _returning: bool,
) -> Result<(BuiltSql, Option<u64>), AppError> {
    // Counter tables take no lightweight transactions, and a counter is never set, only moved.
    if t.columns.iter().any(|c| c.data_type == "counter") {
        return Err(refused(
            "counter tables cannot be edited in the grid: run UPDATE … SET c = c + n in the \
             editor",
        ));
    }
    let table = table_name(&SPEC, t.table);
    let mut values = Vec::new();
    let mut params = Params::new(&SPEC, &mut values);
    let sql = match change {
        StagedChange::Update {
            key,
            changes,
            exprs,
            ..
        } => {
            let cells = cells(changes, exprs.as_ref());
            if cells.is_empty() {
                return Err(AppError::InvalidInput(
                    "update has no changed columns".into(),
                ));
            }
            let mut sets = Vec::with_capacity(cells.len());
            for (name, cell) in &cells {
                let col = find_column(t.columns, name)?;
                if col.is_pk {
                    return Err(refused(
                        "cannot change a primary key column: delete the row and insert it again",
                    ));
                }
                sets.push(format!(
                    "{} = {}",
                    ident(&SPEC, name),
                    cell_sql(cell, col, &mut params)?
                ));
            }
            let condition = key_condition(t.columns, key, &mut params)?;
            format!(
                "UPDATE {table} SET {} WHERE {condition} IF EXISTS",
                sets.join(", ")
            )
        }
        StagedChange::Insert {
            values: row, exprs, ..
        } => {
            let cells = cells(row, exprs.as_ref());
            let mut names = Vec::new();
            let mut exprs_sql = Vec::new();
            for (name, cell) in &cells {
                let col = find_column(t.columns, name)?;
                // Left out, a cell stays empty: CQL columns have no defaults.
                if matches!(
                    cell,
                    Cell::Value(Value::Default) | Cell::Expr(ServerExpr::Default)
                ) {
                    continue;
                }
                names.push(ident(&SPEC, name));
                exprs_sql.push(cell_sql(cell, col, &mut params)?);
            }
            if let Some(missing) = t
                .columns
                .iter()
                .find(|c| c.is_pk && !cells.contains_key(c.name.as_str()))
            {
                return Err(AppError::InvalidInput(format!(
                    "a new row needs its key column \"{}\"",
                    missing.name
                )));
            }
            format!(
                "INSERT INTO {table} ({}) VALUES ({}) IF NOT EXISTS",
                names.join(", "),
                exprs_sql.join(", ")
            )
        }
        StagedChange::Delete { key, .. } => {
            let condition = key_condition(t.columns, key, &mut params)?;
            format!("DELETE FROM {table} WHERE {condition} IF EXISTS")
        }
    };
    Ok((
        BuiltSql {
            sql,
            params: values,
        },
        Some(1),
    ))
}

pub static SPEC: DialectSpec = DialectSpec {
    dialect: SqlDialect::Cql,
    label: "CQL",
    quote_ident: double_quoted,
    quote_literal: single_quoted,
    quote_literal_exec: single_quoted,
    placeholder: ph_question,
    placeholder_style: PlaceholderStyle::Question,
    placeholders_reusable: false,
    max_params: 1000,
    max_name_parts: 2,
    lex: &LEX,
    rules: &RULES,
    classify: classify_cql,
    knobs: SqlKnobs {
        bool_literal: ["false", "true"],
        bytes_literal: hex_0x,
        text_expr: no_text,
        paging: Paging::LimitOnly,
        nulls_order: NullsSort::Unsupported,
        ci_like: CiLike::Unsupported,
        // SASI's LIKE knows `%` at either end only, and nothing escapes it.
        like_escape: None,
        like_wildcards: &['%'],
        null_safe_eq: NullSafeEq::OrForm,
        insert_all_defaults: None,
        set_default: false,
        server_expr: cql_server_expr,
        explain: explain_unsupported,
        catalog_words: &[
            "SYSTEM_SCHEMA",
            "SYSTEM.",
            "SYSTEM_VIEWS",
            "SYSTEM_VIRTUAL_SCHEMA",
            "SYSTEM_AUTH",
            "SYSTEM_DISTRIBUTED",
            "SYSTEM_TRACES",
        ],
        redact: &[
            // `CREATE ROLE r WITH PASSWORD = '…'`, `CREATE USER u WITH PASSWORD '…'`, and
            // Cassandra 4.1's `HASHED PASSWORD = '…'`.
            RedactRule {
                keyword: "PASSWORD",
            },
        ],
        // cqlsh's `LOGIN user 'password'`, at the start of a statement only: `WHERE login =
        // 'bob'` and `WITH LOGIN = true` are no command.
        redact_commands: &["LOGIN"],
        cancel_notice: "what it changed before it stopped stays changed (Cassandra has no \
                        transactions)",
        ..SqlKnobs::ANSI
    },
    hooks: SqlHooks {
        build_select: Some(select_hook),
        build_count: Some(count_hook),
        build_fetch_cell: Some(fetch_cell_hook),
        build_change: Some(change_hook),
        split: Some(split_cql),
    },
    ddl: None,
};

// Schema text rebuilt from `system_schema` rows (the DDL of `table_meta` when the server has no
// DESCRIBE). Names are always quoted; type texts are the server's own (`frozen<list<text>>`).

/// `"ks"."name"`
pub fn qualified(keyspace: &str, name: &str) -> String {
    format!("{}.{}", double_quoted(keyspace), double_quoted(name))
}

/// `DESCRIBE TABLE "ks"."t"` (server-side on Cassandra 4.0+ and ScyllaDB 5.2+).
pub fn describe_table(keyspace: &str, table: &str) -> String {
    format!("DESCRIBE TABLE {}", qualified(keyspace, table))
}

/// `DESCRIBE MATERIALIZED VIEW "ks"."v"`
pub fn describe_view(keyspace: &str, view: &str) -> String {
    format!("DESCRIBE MATERIALIZED VIEW {}", qualified(keyspace, view))
}

/// `{'k': 'v', …}` of text pairs, in the order given.
pub fn map_literal<K: AsRef<str>, V: AsRef<str>>(pairs: &[(K, V)]) -> String {
    let items: Vec<String> = pairs
        .iter()
        .map(|(k, v)| {
            format!(
                "{}: {}",
                single_quoted(k.as_ref()),
                single_quoted(v.as_ref())
            )
        })
        .collect();
    format!("{{{}}}", items.join(", "))
}

/// The part a column plays in its table's primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRole {
    Partition,
    Clustering { desc: bool },
    Static,
    Regular,
}

/// A column of a rebuilt `CREATE TABLE`, in table order (partition key, clustering columns, then
/// the rest).
#[derive(Debug, Clone, Copy)]
pub struct DdlColumn<'a> {
    pub name: &'a str,
    /// The server's type text.
    pub cql_type: &'a str,
    pub role: KeyRole,
}

/// A table option of a rebuilt `CREATE TABLE`: its name and its value as CQL (a quoted string,
/// a number, a map literal).
#[derive(Debug, Clone)]
pub struct DdlOption {
    pub name: String,
    pub value: String,
}

/// `PRIMARY KEY ((pk…), ck…)` of the columns; None without a partition key.
fn primary_key(columns: &[DdlColumn<'_>]) -> Option<String> {
    let names = |f: fn(&KeyRole) -> bool| -> Vec<String> {
        columns
            .iter()
            .filter(|c| f(&c.role))
            .map(|c| double_quoted(c.name))
            .collect()
    };
    let partition = names(|r| *r == KeyRole::Partition);
    if partition.is_empty() {
        return None;
    }
    let mut key = vec![format!("({})", partition.join(", "))];
    key.extend(names(|r| matches!(r, KeyRole::Clustering { .. })));
    Some(format!("PRIMARY KEY ({})", key.join(", ")))
}

/// `CLUSTERING ORDER BY (ck ASC|DESC, …)`; None without clustering columns.
fn clustering_order(columns: &[DdlColumn<'_>]) -> Option<String> {
    let order: Vec<String> = columns
        .iter()
        .filter_map(|c| match c.role {
            KeyRole::Clustering { desc } => Some(format!(
                "{} {}",
                double_quoted(c.name),
                if desc { "DESC" } else { "ASC" }
            )),
            _ => None,
        })
        .collect();
    (!order.is_empty()).then(|| format!("CLUSTERING ORDER BY ({})", order.join(", ")))
}

/// `CREATE TABLE "ks"."t" (…, PRIMARY KEY ((pk…), ck…)) WITH CLUSTERING ORDER BY (…) AND …;`
pub fn create_table(
    keyspace: &str,
    table: &str,
    columns: &[DdlColumn<'_>],
    options: &[DdlOption],
) -> String {
    let mut lines: Vec<String> = columns
        .iter()
        .map(|c| {
            let fixed = if c.role == KeyRole::Static {
                " static"
            } else {
                ""
            };
            format!("    {} {}{fixed}", double_quoted(c.name), c.cql_type)
        })
        .collect();
    lines.extend(primary_key(columns).map(|k| format!("    {k}")));
    let mut with: Vec<String> = clustering_order(columns).into_iter().collect();
    with.extend(options.iter().map(|o| format!("{} = {}", o.name, o.value)));
    let mut out = format!(
        "CREATE TABLE {} (\n{}\n)",
        qualified(keyspace, table),
        lines.join(",\n")
    );
    if !with.is_empty() {
        out.push_str(" WITH ");
        out.push_str(&with.join("\n    AND "));
    }
    out.push(';');
    out
}

/// What a secondary index covers (`options.target`): a column, or a part of a collection one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexTarget {
    /// `"c"`, or `keys("m")`, `values("m")`, `entries("m")`, `full("l")`.
    Column {
        part: Option<&'static str>,
        column: String,
    },
    /// A ScyllaDB local index: `(("pk"…), "c"…)`.
    Local {
        partition: Vec<String>,
        columns: Vec<String>,
    },
}

impl IndexTarget {
    /// The columns the index names, partition columns first.
    pub fn columns(&self) -> Vec<String> {
        match self {
            IndexTarget::Column { column, .. } => vec![column.clone()],
            IndexTarget::Local { partition, columns } => {
                partition.iter().chain(columns).cloned().collect()
            }
        }
    }

    fn cql(&self) -> String {
        match self {
            IndexTarget::Column { part: None, column } => double_quoted(column),
            IndexTarget::Column {
                part: Some(p),
                column,
            } => format!("{p}({})", double_quoted(column)),
            IndexTarget::Local { partition, columns } => {
                let quoted =
                    |v: &[String]| -> Vec<String> { v.iter().map(|c| double_quoted(c)).collect() };
                let mut parts = vec![format!("({})", quoted(partition).join(", "))];
                parts.extend(quoted(columns));
                parts.join(", ")
            }
        }
    }
}

/// `CREATE INDEX "i" ON "ks"."t" (target)`, or with a class a `CREATE CUSTOM INDEX … USING
/// 'class'` and its other options.
pub fn create_index(
    keyspace: &str,
    table: &str,
    index: &str,
    target: &IndexTarget,
    class: Option<&str>,
    options: &[(String, String)],
) -> String {
    let on = format!(
        "{} ON {} ({})",
        double_quoted(index),
        qualified(keyspace, table),
        target.cql()
    );
    match class {
        None => format!("CREATE INDEX {on};"),
        Some(class) if options.is_empty() => {
            format!("CREATE CUSTOM INDEX {on} USING {};", single_quoted(class))
        }
        Some(class) => format!(
            "CREATE CUSTOM INDEX {on} USING {} WITH OPTIONS = {};",
            single_quoted(class),
            map_literal(options)
        ),
    }
}

/// `CREATE TRIGGER "tr" ON "ks"."t" USING 'class';`
pub fn create_trigger(keyspace: &str, table: &str, trigger: &str, class: &str) -> String {
    format!(
        "CREATE TRIGGER {} ON {} USING {};",
        double_quoted(trigger),
        qualified(keyspace, table),
        single_quoted(class)
    )
}

/// `CREATE MATERIALIZED VIEW "ks"."v" AS SELECT … FROM "ks"."base" WHERE … PRIMARY KEY (…)
/// WITH CLUSTERING ORDER BY (…);`. `where_clause` is the server's text of the view's filter.
pub fn create_view(
    keyspace: &str,
    view: &str,
    base: &str,
    columns: &[DdlColumn<'_>],
    include_all: bool,
    where_clause: &str,
) -> String {
    let selected = if include_all {
        "*".to_string()
    } else {
        columns
            .iter()
            .map(|c| double_quoted(c.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out = format!(
        "CREATE MATERIALIZED VIEW {} AS\n    SELECT {selected}\n    FROM {}",
        qualified(keyspace, view),
        qualified(keyspace, base)
    );
    let filter = where_clause.trim();
    if !filter.is_empty() {
        out.push_str("\n    WHERE ");
        out.push_str(filter);
    }
    if let Some(key) = primary_key(columns) {
        out.push_str("\n    ");
        out.push_str(&key);
    }
    if let Some(order) = clustering_order(columns) {
        out.push_str(" WITH ");
        out.push_str(&order);
    }
    out.push(';');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_bindings_lex_cql() {
        crate::sqlgen::dialect::lex::export_lex("cql", &LEX, &RULES);
    }

    fn col(name: &str, ty: &str, pk: Option<u32>) -> ColumnMeta {
        ColumnMeta {
            name: name.into(),
            ordinal: 1,
            data_type: ty.into(),
            kind: crate::drivers::cassandra::types::native_kind(ty),
            nullable: pk.is_none(),
            default_expr: None,
            is_pk: pk.is_some(),
            pk_ordinal: pk,
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
            base_type: None,
        }
    }

    /// `"My""Ks"."t"`: `PRIMARY KEY (("Id"), seq)`, a text and a timestamp.
    fn table() -> (TableRef, Vec<ColumnMeta>) {
        (
            TableRef::new(Some("My\"Ks"), "t"),
            vec![
                col("Id", "int", Some(1)),
                col("seq", "int", Some(2)),
                col("val", "text", None),
                col("at", "timestamp", None),
            ],
        )
    }

    fn eq(column: &str, v: Value) -> FilterNode {
        FilterNode::Condition {
            column: column.into(),
            op: FilterOp::Eq,
            values: vec![v],
        }
    }

    fn key(id: i64, seq: Option<i64>) -> RowKey {
        let mut map = std::collections::BTreeMap::from([("Id".to_string(), Value::Int(id))]);
        if let Some(seq) = seq {
            map.insert("seq".into(), Value::Int(seq));
        }
        RowKey::Pk(map)
    }

    #[test]
    fn the_grid_selects_with_limit_before_allow_filtering() {
        let (t, cols) = table();
        let filter = FilterGroup {
            conjunction: Conjunction::And,
            items: vec![
                eq("val", Value::Text("x'; DROP TABLE t; --".into())),
                FilterNode::Condition {
                    column: "seq".into(),
                    op: FilterOp::Between,
                    values: vec![Value::Int(1), Value::Int(5)],
                },
                FilterNode::Condition {
                    column: "Id".into(),
                    op: FilterOp::In,
                    values: vec![Value::Int(1), Value::Int(2)],
                },
            ],
        };
        let q = SelectQuery {
            table: &t,
            columns: &cols,
            selected: None,
            filter: Some(&filter),
            sort: &[],
            limit: 300,
            offset: 0,
            pk: &[],
            identity: crate::types::browse::RowIdentity::Pk,
        };
        let built = crate::sqlgen::portable::select::build_select(&SPEC, &q).unwrap();
        assert_eq!(
            built.sql,
            "SELECT \"Id\", \"seq\", \"val\", \"at\" FROM \"My\"\"Ks\".\"t\" WHERE \"val\" = ? \
             AND \"seq\" >= ? AND \"seq\" <= ? AND \"Id\" IN (?, ?) LIMIT 300 ALLOW FILTERING"
        );
        // Values are bound, never written into the text.
        assert_eq!(built.params.len(), 5);
        assert_eq!(built.params[0], Value::Text("x'; DROP TABLE t; --".into()));
        let paged = browse_select(&q, None).unwrap();
        assert!(
            paged.sql.ends_with("IN (?, ?) ALLOW FILTERING"),
            "{}",
            paged.sql
        );
        let none = SelectQuery { filter: None, ..q };
        assert_eq!(
            browse_select(&none, Some(10)).unwrap().sql,
            "SELECT \"Id\", \"seq\", \"val\", \"at\" FROM \"My\"\"Ks\".\"t\" LIMIT 10"
        );
        let count = count_select(&t, &cols, Some(&filter)).unwrap();
        assert!(
            count
                .sql
                .starts_with("SELECT count(*) FROM \"My\"\"Ks\".\"t\" WHERE")
        );
        assert!(count.sql.ends_with(" ALLOW FILTERING"));
        assert_eq!(
            count_select(&t, &cols, None).unwrap().sql,
            "SELECT count(*) FROM \"My\"\"Ks\".\"t\""
        );
        // What CQL cannot filter, sort or skip is refused.
        let sorted = [crate::types::browse::SortSpec {
            column: "seq".into(),
            dir: crate::types::browse::SortDir::Desc,
            nulls: None,
        }];
        assert!(browse_select(&SelectQuery { sort: &sorted, ..q }, None).is_err());
        assert!(browse_select(&SelectQuery { offset: 300, ..q }, None).is_err());
        for bad in [
            FilterGroup {
                conjunction: Conjunction::Or,
                items: vec![eq("val", Value::Text("a".into())), eq("seq", Value::Int(1))],
            },
            FilterGroup {
                conjunction: Conjunction::And,
                items: vec![eq("val", Value::Null)],
            },
            FilterGroup {
                conjunction: Conjunction::And,
                items: vec![FilterNode::Condition {
                    column: "val".into(),
                    op: FilterOp::NotLike,
                    values: vec![Value::Text("a".into())],
                }],
            },
            FilterGroup {
                conjunction: Conjunction::And,
                items: vec![FilterNode::AnyColumn {
                    op: FilterOp::Eq,
                    values: vec![Value::Text("a".into())],
                }],
            },
            FilterGroup {
                conjunction: Conjunction::And,
                items: vec![eq("nope", Value::Int(1))],
            },
        ] {
            assert!(count_select(&t, &cols, Some(&bad)).is_err(), "{bad:?}");
        }
        // One raw relation in an OR group is still just that relation.
        let raw = FilterGroup {
            conjunction: Conjunction::Or,
            items: vec![FilterNode::Raw {
                sql: " token(\"Id\") > 0 ".into(),
            }],
        };
        assert!(
            count_select(&t, &cols, Some(&raw))
                .unwrap()
                .sql
                .ends_with("WHERE token(\"Id\") > 0 ALLOW FILTERING")
        );
    }

    #[test]
    fn edits_are_lightweight_transactions_by_full_key() {
        let (t, cols) = table();
        let target = TableTarget {
            table: &t,
            columns: &cols,
        };
        let change = |c: &StagedChange| (SPEC.hooks.build_change.unwrap())(&SPEC, &target, c, true);
        let update = StagedChange::Update {
            row_id: "r".into(),
            key: key(1, Some(2)),
            changes: std::collections::BTreeMap::from([
                ("val".to_string(), Value::Null),
                ("at".to_string(), Value::Text("x".into())),
            ]),
            exprs: Some(std::collections::BTreeMap::from([(
                "at".to_string(),
                ServerExpr::Now,
            )])),
        };
        let (built, expect) = change(&update).unwrap();
        assert_eq!(
            built.sql,
            "UPDATE \"My\"\"Ks\".\"t\" SET \"at\" = toTimestamp(now()), \"val\" = NULL WHERE \
             \"Id\" = ? AND \"seq\" = ? IF EXISTS"
        );
        assert_eq!(built.params, [Value::Int(1), Value::Int(2)]);
        assert_eq!(expect, Some(1));
        let insert = StagedChange::Insert {
            row_id: "n".into(),
            values: std::collections::BTreeMap::from([
                ("Id".to_string(), Value::Int(1)),
                ("seq".to_string(), Value::Int(2)),
                ("val".to_string(), Value::Text("v".into())),
                ("at".to_string(), Value::Default),
            ]),
            exprs: None,
        };
        let (built, _) = change(&insert).unwrap();
        assert_eq!(
            built.sql,
            "INSERT INTO \"My\"\"Ks\".\"t\" (\"Id\", \"seq\", \"val\") VALUES (?, ?, ?) IF NOT \
             EXISTS"
        );
        let delete = StagedChange::Delete {
            row_id: "r".into(),
            key: key(1, Some(2)),
        };
        assert_eq!(
            change(&delete).unwrap().0.sql,
            "DELETE FROM \"My\"\"Ks\".\"t\" WHERE \"Id\" = ? AND \"seq\" = ? IF EXISTS"
        );
        let fetch = fetch_cell_select(&target, &cols[2], &key(1, Some(2))).unwrap();
        assert_eq!(
            fetch.sql,
            "SELECT \"val\" FROM \"My\"\"Ks\".\"t\" WHERE \"Id\" = ? AND \"seq\" = ?"
        );

        // A partial key would reach a range of rows; a key column cannot be set; no DEFAULT.
        let partial = StagedChange::Delete {
            row_id: "r".into(),
            key: key(1, None),
        };
        assert!(change(&partial).is_err());
        assert!(fetch_cell_select(&target, &cols[2], &key(1, None)).is_err());
        let all_columns = StagedChange::Delete {
            row_id: "r".into(),
            key: RowKey::AllColumns(std::collections::BTreeMap::from([(
                "val".to_string(),
                Value::Text("v".into()),
            )])),
        };
        assert!(change(&all_columns).is_err());
        let set_key = StagedChange::Update {
            row_id: "r".into(),
            key: key(1, Some(2)),
            changes: std::collections::BTreeMap::from([("seq".to_string(), Value::Int(3))]),
            exprs: None,
        };
        assert!(change(&set_key).is_err());
        let set_default = StagedChange::Update {
            row_id: "r".into(),
            key: key(1, Some(2)),
            changes: std::collections::BTreeMap::from([("val".to_string(), Value::Default)]),
            exprs: None,
        };
        assert!(change(&set_default).is_err());
        let keyless_insert = StagedChange::Insert {
            row_id: "n".into(),
            values: std::collections::BTreeMap::from([("val".to_string(), Value::Int(1))]),
            exprs: None,
        };
        assert!(change(&keyless_insert).is_err());
        let null_key = StagedChange::Delete {
            row_id: "r".into(),
            key: RowKey::Pk(std::collections::BTreeMap::from([
                ("Id".to_string(), Value::Null),
                ("seq".to_string(), Value::Int(1)),
            ])),
        };
        assert!(change(&null_key).is_err());
        // Counter tables take no lightweight transactions.
        let counters = vec![col("Id", "int", Some(1)), col("n", "counter", None)];
        let counter_target = TableTarget {
            table: &t,
            columns: &counters,
        };
        let bump = StagedChange::Update {
            row_id: "r".into(),
            key: RowKey::Pk(std::collections::BTreeMap::from([(
                "Id".to_string(),
                Value::Int(1),
            )])),
            changes: std::collections::BTreeMap::from([("n".to_string(), Value::Int(1))]),
            exprs: None,
        };
        assert!((SPEC.hooks.build_change.unwrap())(&SPEC, &counter_target, &bump, false).is_err());
    }

    #[test]
    fn quotes_and_classifies() {
        assert_eq!((SPEC.quote_ident)("MixedCase\""), "\"MixedCase\"\"\"");
        assert_eq!((SPEC.quote_literal)("it's C:\\"), "'it''s C:\\'");
        let read = |sql: &str| assert_eq!(classify_cql(sql), StatementClass::Read, "{sql:?}");
        let write = |sql: &str| assert_eq!(classify_cql(sql), StatementClass::Write, "{sql:?}");
        read("SELECT * FROM ks.t WHERE k = 1 ALLOW FILTERING");
        read("SELECT JSON k, ttl(v), writetime(v) FROM t");
        read("DESCRIBE KEYSPACES");
        read("LIST ROLES");
        read("CONSISTENCY LOCAL_QUORUM");
        read("SERIAL CONSISTENCY LOCAL_SERIAL");
        read("SELECT $$; DELETE FROM t; $$ FROM t");
        read("SELECT 'a\\' FROM t");
        // An inspection reads whatever follows it; the server refuses such text whole (one
        // statement per request), and the cqlsh commands refuse it too (`cassandra::cqlsh`).
        read("USE ks DELETE FROM t");
        read("SERIAL CONSISTENCY SERIAL\nDELETE FROM t WHERE k = 1");
        read("PAGING 10 INSERT INTO t (k) VALUES (1)");
        read("EXPAND ON\r\nUPDATE t SET v = 1 WHERE k = 1");
        // A duration glued to a keyword is one word.
        read("SELECT * FROM t WHERE d = P1DT2HDELETE");
        write("BEGIN BATCH INSERT INTO t (k) VALUES (1); APPLY BATCH");
        write("BEGIN UNLOGGED BATCH UPDATE t SET v = 1 WHERE k = 1 APPLY BATCH");
        write("INSERT INTO t (k) VALUES (1) IF NOT EXISTS");
        write("DELETE FROM t WHERE k = 1 IF EXISTS");
        write("SELECT 1 FROM t; UPDATE t USING TTL 10 SET v = 1 WHERE k = 1");
        // A backslash does not escape the quote: the DELETE is code.
        write("SELECT 'a\\'; DELETE FROM t; --'");
        write("PRUNE MATERIALIZED VIEW v WHERE k = 1");
        write("COPY t TO 'x.csv'");
        write("SOURCE 'x.cql'");
    }

    #[test]
    fn redacts_passwords_and_cqlsh_login() {
        let redact = |sql: &str| crate::sqlgen::portable::redact::redact_spec(&SPEC, sql);
        let r = redact("LOGIN alice 'se''cret'");
        assert_eq!(r.sql, "LOGIN alice '********'");
        assert_eq!(r.secrets, ["se'cret", "'se''cret'"]);
        assert_eq!(
            redact("SELECT * FROM t WHERE k = 1; LOGIN 'alice' 'pw'").sql,
            "SELECT * FROM t WHERE k = 1; LOGIN 'alice' '********'"
        );
        // cqlsh prompts for the password when the line has none.
        assert_eq!(redact("LOGIN alice").sql, "LOGIN alice");
        // LOGIN is the command only at the start of a statement.
        for sql in [
            "SELECT * FROM users WHERE login = 'bob' ALLOW FILTERING",
            "UPDATE users SET login = 'bob' WHERE k = 1",
        ] {
            assert_eq!(redact(sql).sql, sql, "{sql}");
        }
        assert_eq!(
            redact("CREATE ROLE r WITH PASSWORD = 'x' AND LOGIN = true").sql,
            "CREATE ROLE r WITH PASSWORD = '********' AND LOGIN = true"
        );
        assert_eq!(
            redact("create role r with login = true and password = 'x\\'").sql,
            "create role r with login = true and password = '********'"
        );
        assert_eq!(
            redact("CREATE USER u WITH PASSWORD 'p' NOSUPERUSER").sql,
            "CREATE USER u WITH PASSWORD '********' NOSUPERUSER"
        );
        let r = redact("ALTER ROLE r WITH HASHED PASSWORD = '$2a$10$JSJEMFm6GeaW9XxT5JIheu'");
        assert_eq!(r.sql, "ALTER ROLE r WITH HASHED PASSWORD = '********'");
        assert!(
            r.secrets
                .contains(&"$2a$10$JSJEMFm6GeaW9XxT5JIheu".to_string())
        );
        let r = redact("CREATE ROLE r WITH PASSWORD = $$pw$$ AND LOGIN = true");
        assert_eq!(
            r.sql,
            "CREATE ROLE r WITH PASSWORD = '********' AND LOGIN = true"
        );
        assert_eq!(r.secrets, ["pw", "$$pw$$"]);
        let r = redact("ALTER ROLE r WITH PASSWORD = $$p'w$$");
        assert_eq!(r.secrets, ["p'w", "$$p'w$$"]);
        // A password word in a string, a name or a comment masks nothing.
        for sql in [
            "SELECT 'password' FROM t",
            "SELECT \"login\" FROM t WHERE k = 'a'",
            "SELECT k FROM t -- LOGIN alice 'x'\n",
        ] {
            assert_eq!(redact(sql).sql, sql, "{sql:?}");
        }
    }

    #[test]
    fn batches_split_as_one_statement() {
        let texts =
            |text: &str| -> Vec<String> { split_cql(text).into_iter().map(|s| s.sql).collect() };
        let script = "SELECT 1 FROM t;\nBEGIN BATCH\n  INSERT INTO t (k) VALUES (1);\n  \
                      UPDATE t SET v = ';' WHERE k = 2;\nAPPLY BATCH;\nSELECT 2 FROM t;";
        assert_eq!(
            texts(script),
            [
                "SELECT 1 FROM t",
                "BEGIN BATCH\n  INSERT INTO t (k) VALUES (1);\n  UPDATE t SET v = ';' WHERE \
                 k = 2;\nAPPLY BATCH",
                "SELECT 2 FROM t",
            ]
        );
        let parts = split_cql(script);
        assert_eq!((parts[1].line, parts[2].line), (2, 6));
        let chars: Vec<char> = script.chars().collect();
        for p in &parts {
            assert_eq!(chars[p.start..p.end].iter().collect::<String>(), p.sql);
        }
        // Counter and unlogged batches, a batch closed in its own statement, a batch never
        // closed, and APPLY BATCH glued to the last statement.
        assert_eq!(
            texts("begin counter batch update c set n = n + 1 where k = 1; apply batch; SELECT 1"),
            [
                "begin counter batch update c set n = n + 1 where k = 1; apply batch",
                "SELECT 1"
            ]
        );
        assert_eq!(
            texts("BEGIN BATCH DELETE FROM t WHERE k = 1 APPLY BATCH; SELECT 1"),
            [
                "BEGIN BATCH DELETE FROM t WHERE k = 1 APPLY BATCH",
                "SELECT 1"
            ]
        );
        assert_eq!(
            texts("BEGIN UNLOGGED BATCH INSERT INTO t (k) VALUES (1); SELECT 1"),
            ["BEGIN UNLOGGED BATCH INSERT INTO t (k) VALUES (1); SELECT 1"]
        );
        assert_eq!(
            texts(
                "BEGIN BATCH INSERT INTO t (k) VALUES (1); DELETE FROM t WHERE k = 2 APPLY BATCH; SELECT 1"
            ),
            [
                "BEGIN BATCH INSERT INTO t (k) VALUES (1); DELETE FROM t WHERE k = 2 APPLY BATCH",
                "SELECT 1"
            ]
        );
        // Words in strings, names, bodies and comments open or close nothing.
        assert_eq!(
            texts(
                "SELECT 'BEGIN BATCH' FROM t; SELECT \"apply\", $$APPLY BATCH$$ FROM t -- BEGIN BATCH\n; SELECT 3"
            ),
            [
                "SELECT 'BEGIN BATCH' FROM t",
                "SELECT \"apply\", $$APPLY BATCH$$ FROM t -- BEGIN BATCH",
                "SELECT 3"
            ]
        );
        assert_eq!(
            texts("BEGIN BATCH INSERT INTO t (k) VALUES ('APPLY BATCH'); APPLY BATCH"),
            ["BEGIN BATCH INSERT INTO t (k) VALUES ('APPLY BATCH'); APPLY BATCH"]
        );
        // The facade sends CQL here.
        assert_eq!(
            crate::sqlgen::sql::split_statements(SqlDialect::Cql, "BEGIN BATCH x; APPLY BATCH")
                .len(),
            1
        );
    }

    #[test]
    fn server_expressions_are_the_cql_clocks() {
        let at = |e, k| cql_server_expr(e, k);
        assert_eq!(
            at(ServerExpr::Now, ValueKind::Uuid).as_deref(),
            Some("now()")
        );
        assert_eq!(
            at(ServerExpr::Now, ValueKind::TimestampTz).as_deref(),
            Some("toTimestamp(now())")
        );
        assert_eq!(
            at(ServerExpr::CurrentDate, ValueKind::Date).as_deref(),
            Some("toDate(now())")
        );
        assert_eq!(at(ServerExpr::CurrentTime, ValueKind::Time), None);
        assert_eq!(at(ServerExpr::LocalTimestamp, ValueKind::Timestamp), None);
        assert_eq!(at(ServerExpr::Now, ValueKind::Text), None);
        assert!((SPEC.knobs.switch_db)("use other_ks"));
    }

    #[test]
    fn rebuilds_create_table() {
        let cols = [
            DdlColumn {
                name: "user_id",
                cql_type: "uuid",
                role: KeyRole::Partition,
            },
            DdlColumn {
                name: "Bucket",
                cql_type: "int",
                role: KeyRole::Partition,
            },
            DdlColumn {
                name: "ts",
                cql_type: "timestamp",
                role: KeyRole::Clustering { desc: true },
            },
            DdlColumn {
                name: "seq",
                cql_type: "int",
                role: KeyRole::Clustering { desc: false },
            },
            DdlColumn {
                name: "owner",
                cql_type: "text",
                role: KeyRole::Static,
            },
            DdlColumn {
                name: "tags",
                cql_type: "frozen<list<text>>",
                role: KeyRole::Regular,
            },
        ];
        let options = [
            DdlOption {
                name: "comment".into(),
                value: single_quoted("it's"),
            },
            DdlOption {
                name: "compaction".into(),
                value: map_literal(&[(
                    "class",
                    "org.apache.cassandra.db.compaction.SizeTieredCompactionStrategy",
                )]),
            },
        ];
        assert_eq!(
            create_table("shop", "events", &cols, &options),
            "CREATE TABLE \"shop\".\"events\" (\n    \"user_id\" uuid,\n    \"Bucket\" int,\n    \
             \"ts\" timestamp,\n    \"seq\" int,\n    \"owner\" text static,\n    \"tags\" \
             frozen<list<text>>,\n    PRIMARY KEY ((\"user_id\", \"Bucket\"), \"ts\", \"seq\")\n) \
             WITH CLUSTERING ORDER BY (\"ts\" DESC, \"seq\" ASC)\n    AND comment = 'it''s'\n    \
             AND compaction = {'class': \
             'org.apache.cassandra.db.compaction.SizeTieredCompactionStrategy'};"
        );
        assert_eq!(
            create_table("ks", "kv", &cols[..1], &[]),
            "CREATE TABLE \"ks\".\"kv\" (\n    \"user_id\" uuid,\n    PRIMARY KEY ((\"user_id\"))\n);"
        );
        assert_eq!(
            create_view("ks", "by_ts", "events", &cols[..4], true, "ts IS NOT NULL"),
            "CREATE MATERIALIZED VIEW \"ks\".\"by_ts\" AS\n    SELECT *\n    FROM \
             \"ks\".\"events\"\n    WHERE ts IS NOT NULL\n    PRIMARY KEY ((\"user_id\", \
             \"Bucket\"), \"ts\", \"seq\") WITH CLUSTERING ORDER BY (\"ts\" DESC, \"seq\" ASC);"
        );
        assert_eq!(
            describe_table("Ks", "t\""),
            "DESCRIBE TABLE \"Ks\".\"t\"\"\""
        );
    }

    #[test]
    fn rebuilds_indexes_and_triggers() {
        let col = |part, c: &str| IndexTarget::Column {
            part,
            column: c.into(),
        };
        assert_eq!(
            create_index("ks", "t", "t_email", &col(None, "Email"), None, &[]),
            "CREATE INDEX \"t_email\" ON \"ks\".\"t\" (\"Email\");"
        );
        assert_eq!(
            create_index(
                "ks",
                "t",
                "t_m",
                &col(Some("keys"), "m"),
                Some("org.apache.cassandra.index.sasi.SASIIndex"),
                &[("mode".into(), "CONTAINS".into())]
            ),
            "CREATE CUSTOM INDEX \"t_m\" ON \"ks\".\"t\" (keys(\"m\")) USING \
             'org.apache.cassandra.index.sasi.SASIIndex' WITH OPTIONS = {'mode': 'CONTAINS'};"
        );
        let local = IndexTarget::Local {
            partition: vec!["p".into()],
            columns: vec!["c".into()],
        };
        assert_eq!(local.columns(), ["p", "c"]);
        assert_eq!(
            create_index("ks", "t", "l", &local, None, &[]),
            "CREATE INDEX \"l\" ON \"ks\".\"t\" ((\"p\"), \"c\");"
        );
        assert_eq!(
            create_trigger("ks", "t", "audit", "org.x.Audit'"),
            "CREATE TRIGGER \"audit\" ON \"ks\".\"t\" USING 'org.x.Audit''';"
        );
    }
}
