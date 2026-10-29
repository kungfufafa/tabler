//! SELECT / COUNT generation for table browsing: quoted identifiers, bind parameters, server-side
//! sort and filter, LIMIT/OFFSET paging.

use crate::error::AppError;
use crate::sqlgen::mutate::{TableSpec, row_key_where};
use crate::sqlgen::narrow::{Narrowing, big_integer_projection, holds_integers};
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

/// The PostgreSQL type a bind parameter is cast to for a column of type `data_type` (as
/// `format_type(atttypid, atttypmod)` spells it): the type without its modifier. An explicit cast
/// to the full type would silently fit the value to it (`'abcdefgh'::varchar(5)` is `'abcde'`,
/// `'1.239'::numeric(10,2)` is `1.24`, `'101'::bit(8)` is zero-padded), so a filter would match
/// the wrong rows and an edit or import would store a cut value instead of failing with "value
/// too long". Cast to the base type, the column applies its modifier when the value is stored
/// (with an error where the value does not fit).
/// - `character(n)` and `char` become `bpchar`, `bit(n)` and `bit` become `bit varying`: a bare
///   `character` / `bit` means length 1 and would cut the value just the same;
/// - `interval <fields>(p)` becomes `interval` (the fields round, too);
/// - array suffixes stay: `character varying(5)[]` becomes `character varying[]`;
/// - parentheses inside double-quoted names are kept.
///
/// A domain column passes its base type (`ColumnMeta::cast_type`): cast to the domain itself, a
/// domain over a sized type would still cut the value.
pub fn pg_cast_type(data_type: &str) -> String {
    let mut base = data_type.trim();
    let mut dims = 0;
    while let Some(rest) = base.strip_suffix("[]") {
        base = rest.trim_end();
        dims += 1;
    }
    let mut stripped = String::with_capacity(base.len());
    let mut depth = 0usize;
    let mut quoted = false;
    for ch in base.chars() {
        match ch {
            '"' if depth == 0 => {
                quoted = !quoted;
                stripped.push(ch);
            }
            '(' if !quoted => depth += 1,
            ')' if !quoted && depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            _ => stripped.push(ch),
        }
    }
    let name = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = name.to_ascii_lowercase();
    let base = match lower.as_str() {
        "character" | "char" => "bpchar".to_string(),
        "bit" => "bit varying".to_string(),
        l if l == "interval" || l.starts_with("interval ") => "interval".to_string(),
        _ => name,
    };
    format!("{base}{}", "[]".repeat(dims))
}

struct ParamSink<'a> {
    dialect: Dialect,
    params: &'a mut Vec<Value>,
    /// "Any column": the parameter numbers the first column's condition pushed, which the next
    /// columns reuse instead of binding the same values again (with the position of the next
    /// one). PostgreSQL `$n` and SQLite `?n` may repeat; MySQL's `?` cannot.
    replay: Option<(Vec<usize>, usize)>,
}

impl ParamSink<'_> {
    fn new(dialect: Dialect, params: &mut Vec<Value>) -> ParamSink<'_> {
        ParamSink {
            dialect,
            params,
            replay: None,
        }
    }

    /// Pushes a value and returns its placeholder (with a PG cast to the base type of `cast` when
    /// it is given, see `pg_cast_type`).
    fn push(&mut self, value: Value, cast: Option<&str>) -> String {
        let reused = match &mut self.replay {
            Some((numbers, next)) => {
                let n = numbers.get(*next).copied();
                *next += 1;
                n.filter(|&n| self.params.get(n - 1) == Some(&value))
            }
            None => None,
        };
        let n = match reused {
            Some(n) => n,
            None => {
                self.params.push(value);
                self.params.len()
            }
        };
        let ph = self.dialect.placeholder(n);
        match (self.dialect, cast) {
            (Dialect::Postgres, Some(ty)) if !ty.is_empty() => {
                format!("{ph}::{}", pg_cast_type(ty))
            }
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
        FilterOp::Ilike => "ILIKE",
        FilterOp::In => "IN",
        FilterOp::NotIn => "NOT IN",
        FilterOp::IsNull => "IS NULL",
        FilterOp::IsNotNull => "IS NOT NULL",
        FilterOp::Between => "BETWEEN",
        FilterOp::NotBetween => "NOT BETWEEN",
        FilterOp::Contains
        | FilterOp::NotContains
        | FilterOp::ContainsCase
        | FilterOp::NotContainsCase
        | FilterOp::HasPrefix
        | FilterOp::HasSuffix
        | FilterOp::HasPrefixCase
        | FilterOp::HasSuffixCase
        | FilterOp::JsonContains
        | FilterOp::JsonNotContains => "",
    }
}

/// Escape character of the generated LIKE patterns (`ESCAPE '!'`): not a backslash, whose meaning
/// inside string literals differs per server and SQL mode.
const LIKE_ESCAPE: char = '!';

/// Escapes LIKE's wildcards (`%`, `_`) and the escape character itself, so `text` matches
/// literally inside a pattern ending in `ESCAPE '!'`.
pub fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    for ch in text.chars() {
        if matches!(ch, '%' | '_') || ch == LIKE_ESCAPE {
            out.push(LIKE_ESCAPE);
        }
        out.push(ch);
    }
    out
}

/// Escapes SQLite GLOB's wildcards (`*`, `?`, `[`) by wrapping each in a bracket expression.
pub fn escape_glob(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    for ch in text.chars() {
        if matches!(ch, '*' | '?' | '[') {
            out.push('[');
            out.push(ch);
            out.push(']');
        } else {
            out.push(ch);
        }
    }
    out
}

/// What a filter condition is applied to: a column, or (for "Any column") its textual form.
struct Target {
    /// Name for error messages.
    name: String,
    /// The column itself: IS NULL / IS NOT NULL, JSON containment.
    raw: String,
    /// Comparisons, IN and BETWEEN.
    cmp: String,
    /// Textual form for pattern matching.
    text: String,
    /// MySQL's case-sensitive textual form (binary collation).
    text_bin: String,
    /// PostgreSQL parameter cast for comparisons (`$1::<type>`).
    cast: Option<String>,
    /// A `json` / `jsonb` column (JSON containment applies).
    json: bool,
}

impl Target {
    /// A condition on one column, compared in its own type.
    fn column(dialect: Dialect, col: &ColumnMeta) -> Target {
        let ident = dialect.quote_ident(&col.name);
        let text = match dialect {
            Dialect::Postgres => format!("{ident}::text"),
            _ => ident.clone(),
        };
        Target {
            name: col.name.clone(),
            text_bin: format!("CAST({ident} AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_bin"),
            raw: ident.clone(),
            cmp: ident,
            text,
            cast: Some(col.cast_type().to_string()),
            json: col.kind == ValueKind::Json,
        }
    }

    /// One column of an "Any column" condition: everything compares as text, NULL as ''.
    fn any(dialect: Dialect, col: &ColumnMeta) -> Target {
        let ident = dialect.quote_ident(&col.name);
        let cast = match dialect {
            Dialect::Postgres => format!("{ident}::text"),
            Dialect::Mysql => format!("CAST({ident} AS CHAR CHARACTER SET utf8mb4)"),
            Dialect::Sqlite => format!("CAST({ident} AS TEXT)"),
        };
        let text = format!("COALESCE({cast}, '')");
        Target {
            name: col.name.clone(),
            text_bin: format!("{text} COLLATE utf8mb4_bin"),
            raw: ident,
            cmp: text.clone(),
            text,
            cast: Some("text".into()),
            json: col.kind == ValueKind::Json,
        }
    }
}

/// Rejects operators the dialect has no form of.
fn check_dialect(dialect: Dialect, op: FilterOp) -> Result<(), AppError> {
    match op {
        FilterOp::Ilike if dialect != Dialect::Postgres => Err(AppError::InvalidInput(
            "ILIKE is a PostgreSQL operator".into(),
        )),
        FilterOp::JsonContains | FilterOp::JsonNotContains if dialect != Dialect::Postgres => {
            Err(AppError::InvalidInput(
                "JSON contains works on PostgreSQL json / jsonb columns only".into(),
            ))
        }
        _ => Ok(()),
    }
}

fn need(t: &Target, values: &[Value], n: usize) -> Result<(), AppError> {
    if values.len() < n {
        Err(AppError::InvalidInput(format!(
            "filter on \"{}\" needs {n} value(s)",
            t.name
        )))
    } else {
        Ok(())
    }
}

/// Contains / Not contains / Has prefix / Has suffix and their case variants: the value matches
/// literally. PostgreSQL: LIKE, the variant ILIKE. MySQL: LIKE (the column's collation, usually
/// case-insensitive), the variant a binary-collation LIKE. SQLite: case-sensitive `instr()` /
/// GLOB, the variant LIKE (case-insensitive for ASCII).
fn pattern_sql(
    sink: &mut ParamSink<'_>,
    t: &Target,
    op: FilterOp,
    value: &Value,
) -> Result<String, AppError> {
    #[derive(PartialEq)]
    enum Shape {
        Contains,
        Prefix,
        Suffix,
    }
    let (negated, variant, shape) = match op {
        FilterOp::Contains => (false, false, Shape::Contains),
        FilterOp::NotContains => (true, false, Shape::Contains),
        FilterOp::ContainsCase => (false, true, Shape::Contains),
        FilterOp::NotContainsCase => (true, true, Shape::Contains),
        FilterOp::HasPrefix => (false, false, Shape::Prefix),
        FilterOp::HasSuffix => (false, false, Shape::Suffix),
        FilterOp::HasPrefixCase => (false, true, Shape::Prefix),
        FilterOp::HasSuffixCase => (false, true, Shape::Suffix),
        _ => {
            return Err(AppError::Internal(format!(
                "{op:?} is not a pattern filter"
            )));
        }
    };
    let text = value
        .as_text()
        .ok_or_else(|| AppError::InvalidInput(format!("filter on \"{}\" needs a value", t.name)))?;
    let not = if negated { "NOT " } else { "" };
    let dialect = sink.dialect;
    if dialect == Dialect::Sqlite && !variant {
        return Ok(if shape == Shape::Contains {
            let ph = sink.push(Value::Text(text), None);
            let test = if negated { "= 0" } else { "> 0" };
            format!("instr({}, {ph}) {test}", t.text)
        } else {
            let esc = escape_glob(&text);
            let pattern = if shape == Shape::Prefix {
                format!("{esc}*")
            } else {
                format!("*{esc}")
            };
            let ph = sink.push(Value::Text(pattern), None);
            format!("{} {not}GLOB {ph}", t.text)
        });
    }
    let esc = escape_like(&text);
    let pattern = match shape {
        Shape::Contains => format!("%{esc}%"),
        Shape::Prefix => format!("{esc}%"),
        Shape::Suffix => format!("%{esc}"),
    };
    let ph = sink.push(Value::Text(pattern), None);
    let (lhs, keyword) = match (dialect, variant) {
        (Dialect::Postgres, true) => (t.text.as_str(), "ILIKE"),
        (Dialect::Mysql, true) => (t.text_bin.as_str(), "LIKE"),
        _ => (t.text.as_str(), "LIKE"),
    };
    Ok(format!("{lhs} {not}{keyword} {ph} ESCAPE '{LIKE_ESCAPE}'"))
}

/// One condition on `t`.
fn target_sql(
    sink: &mut ParamSink<'_>,
    t: &Target,
    op: FilterOp,
    values: &[Value],
) -> Result<String, AppError> {
    check_dialect(sink.dialect, op)?;
    let cast = t.cast.as_deref();
    Ok(match op {
        FilterOp::IsNull | FilterOp::IsNotNull => format!("{} {}", t.raw, op_sql(op)),
        FilterOp::Like | FilterOp::NotLike | FilterOp::Ilike => {
            need(t, values, 1)?;
            // Pattern matching always happens on the textual form.
            let ph = sink.push(values[0].clone(), None);
            format!("{} {} {ph}", t.text, op_sql(op))
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
            let phs: Vec<String> = values.iter().map(|v| sink.push(v.clone(), cast)).collect();
            format!("{} {} ({})", t.cmp, op_sql(op), phs.join(", "))
        }
        FilterOp::Between | FilterOp::NotBetween => {
            need(t, values, 2)?;
            let a = sink.push(values[0].clone(), cast);
            let b = sink.push(values[1].clone(), cast);
            format!("{} {} {a} AND {b}", t.cmp, op_sql(op))
        }
        FilterOp::Contains
        | FilterOp::NotContains
        | FilterOp::ContainsCase
        | FilterOp::NotContainsCase
        | FilterOp::HasPrefix
        | FilterOp::HasSuffix
        | FilterOp::HasPrefixCase
        | FilterOp::HasSuffixCase => {
            need(t, values, 1)?;
            pattern_sql(sink, t, op, &values[0])?
        }
        FilterOp::JsonContains | FilterOp::JsonNotContains => {
            need(t, values, 1)?;
            if !t.json {
                return Err(AppError::InvalidInput(format!(
                    "JSON contains needs a json / jsonb column; \"{}\" is not one",
                    t.name
                )));
            }
            let text = values[0].as_text().map(Value::Text).unwrap_or(Value::Null);
            let ph = sink.push(text, Some("jsonb"));
            let test = format!("{}::jsonb @> {ph}", t.raw);
            if op == FilterOp::JsonNotContains {
                format!("NOT ({test})")
            } else {
                test
            }
        }
        FilterOp::Eq | FilterOp::Ne | FilterOp::Gt | FilterOp::Lt | FilterOp::Ge | FilterOp::Le => {
            need(t, values, 1)?;
            let ph = sink.push(values[0].clone(), cast);
            format!("{} {} {ph}", t.cmp, op_sql(op))
        }
    })
}

fn condition_sql(
    sink: &mut ParamSink<'_>,
    columns: &[ColumnMeta],
    column: &str,
    op: FilterOp,
    values: &[Value],
) -> Result<String, AppError> {
    let col = find_column(columns, column)?;
    target_sql(sink, &Target::column(sink.dialect, col), op, values)
}

/// Whether "Any column" looks at `col`: binary and spatial values have no useful textual form,
/// and JSON containment only applies to JSON columns.
fn any_column_applies(col: &ColumnMeta, op: FilterOp) -> bool {
    if matches!(col.kind, ValueKind::Bytes | ValueKind::Geometry) {
        return false;
    }
    !matches!(op, FilterOp::JsonContains | FilterOp::JsonNotContains) || col.kind == ValueKind::Json
}

/// "Any column": the condition on every column's textual form, OR-ed (some column matches); a
/// negated operator is AND-ed instead (no column matches its positive form).
fn any_column_sql(
    sink: &mut ParamSink<'_>,
    columns: &[ColumnMeta],
    op: FilterOp,
    values: &[Value],
) -> Result<String, AppError> {
    let dialect = sink.dialect;
    check_dialect(dialect, op)?;
    // Values compare with the textual forms.
    let values: Vec<Value> = values
        .iter()
        .map(|v| v.as_text().map(Value::Text).unwrap_or(Value::Null))
        .collect();
    let mut parts = Vec::new();
    // Every column's condition binds the same values: bind them once where placeholders can
    // repeat, or a wide table times a long IN list runs past the server's parameter limit.
    let mut first: Option<Vec<usize>> = None;
    for col in columns.iter().filter(|c| any_column_applies(c, op)) {
        let start = sink.params.len();
        sink.replay = first.clone().map(|numbers| (numbers, 0));
        let part = target_sql(sink, &Target::any(dialect, col), op, &values);
        sink.replay = None;
        parts.push(part?);
        if first.is_none() && dialect != Dialect::Mysql {
            first = Some((start + 1..=sink.params.len()).collect());
        }
    }
    let negated = op.is_negated();
    Ok(match parts.len() {
        0 if negated => "1 = 1".into(),
        0 => "1 = 0".into(),
        1 => parts.remove(0),
        _ => format!("({})", parts.join(if negated { " AND " } else { " OR " })),
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
            FilterNode::AnyColumn { op, values } => {
                parts.push(any_column_sql(sink, columns, *op, values)?);
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
    let limit = crate::sqlgen::transfer::max_params(dialect);
    build_where_within(dialect, columns, filter, params, limit)
}

/// `build_where` for a server that binds at most `limit` values (`Narrowing::max_params`).
pub fn build_where_within(
    dialect: Dialect,
    columns: &[ColumnMeta],
    filter: Option<&FilterGroup>,
    params: &mut Vec<Value>,
    limit: usize,
) -> Result<Option<String>, AppError> {
    let Some(filter) = filter else {
        return Ok(None);
    };
    let mut sink = ParamSink::new(dialect, params);
    let sql = group_sql(&mut sink, columns, filter)?;
    if params.len() > limit {
        return Err(AppError::InvalidInput(format!(
            "the filter needs {} values, more than one query can bind ({limit}): use fewer \
             values{}",
            params.len(),
            if dialect == Dialect::Mysql {
                " or filter one column instead of Any column"
            } else {
                ""
            }
        )));
    }
    Ok(sql)
}

/// `table`, when given, qualifies each column: a select whose list reads a column under its own
/// name as another expression (`big_integer_projection`) would otherwise sort by that alias.
fn order_by(
    dialect: Dialect,
    columns: &[ColumnMeta],
    sort: &[SortSpec],
    pk: &[String],
    identity: RowIdentity,
    table: Option<&str>,
) -> Result<String, AppError> {
    let column = |name: &str| match table {
        Some(t) => format!("{t}.{}", dialect.quote_ident(name)),
        None => dialect.quote_ident(name),
    };
    let mut terms: Vec<String> = Vec::new();
    for s in sort {
        find_column(columns, &s.column)?;
        let ident = column(&s.column);
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
            terms.extend(pk.iter().map(|c| format!("{} ASC", column(c))));
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
    build_select_narrowed(spec, &Narrowing::of(spec.dialect))
}

/// `build_select` within what the session's server narrows: the filter's values bound by its
/// `max_params`, and with `big_integers_as_text` (SQLite only) the columns that may hold
/// integers read so that one beyond 2^53 comes as its text.
pub fn build_select_narrowed(
    spec: &SelectSpec<'_>,
    narrowing: &Narrowing,
) -> Result<BuiltSql, AppError> {
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
    let big_integers = narrowing.big_integers_as_text && d == Dialect::Sqlite;
    let mut select_list: Vec<String> = cols
        .iter()
        .map(|c| {
            if big_integers && holds_integers(c.kind) {
                big_integer_projection(&d.quote_ident(&c.name))
            } else {
                select_expr(d, c)
            }
        })
        .collect();
    if spec.identity == RowIdentity::Rowid && d == Dialect::Sqlite {
        select_list.push(format!("rowid AS {}", d.quote_ident(ROWID_ALIAS)));
    }
    let mut params = Vec::new();
    let where_sql = build_where_within(
        d,
        spec.columns,
        spec.filter,
        &mut params,
        narrowing.max_params,
    )?;
    let from = d.qualify(spec.table.schema.as_deref(), &spec.table.name);
    // SQLite resolves a bare ORDER BY name to the select's alias first: sort by the table's
    // column, not by the CASE that reads big integers as text (text sorts after every number).
    let order = order_by(
        d,
        spec.columns,
        spec.sort,
        spec.pk,
        spec.identity,
        big_integers.then_some(from.as_str()),
    )?;
    let mut sql = format!("SELECT {}\nFROM {from}", select_list.join(", "));
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
    let limit = crate::sqlgen::transfer::max_params(dialect);
    build_count_within(dialect, table, columns, filter, limit)
}

/// `build_count` for a server that binds at most `limit` values.
pub fn build_count_within(
    dialect: Dialect,
    table: &TableRef,
    columns: &[ColumnMeta],
    filter: Option<&FilterGroup>,
    limit: usize,
) -> Result<BuiltSql, AppError> {
    let mut params = Vec::new();
    let where_sql = build_where_within(dialect, columns, filter, &mut params, limit)?;
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
    let mut sink = ParamSink::new(d, params);
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
            base_type: None,
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

    fn where_of(dialect: Dialect, cols: &[ColumnMeta], node: FilterNode) -> (String, Vec<Value>) {
        let mut params = Vec::new();
        let group = FilterGroup {
            conjunction: Conjunction::And,
            items: vec![node],
        };
        let sql = build_where(dialect, cols, Some(&group), &mut params)
            .unwrap()
            .unwrap();
        (sql, params)
    }

    fn cond(column: &str, op: FilterOp, values: Vec<Value>) -> FilterNode {
        FilterNode::Condition {
            column: column.into(),
            op,
            values,
        }
    }

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn like_and_glob_escaping() {
        assert_eq!(escape_like("50%_off!"), "50!%!_off!!");
        assert_eq!(escape_like("plain"), "plain");
        assert_eq!(escape_glob("a*b?c[d]"), "a[*]b[?]c[[]d]");
    }

    #[test]
    fn postgres_contains_like_variant_ilike() {
        let cols = users();
        let pg = |op, v: &str| where_of(Dialect::Postgres, &cols, cond("name", op, vec![text(v)]));
        assert_eq!(
            pg(FilterOp::Contains, "50%"),
            (
                "\"name\"::text LIKE $1 ESCAPE '!'".into(),
                vec![text("%50!%%")]
            )
        );
        assert_eq!(
            pg(FilterOp::NotContains, "a_b").0,
            "\"name\"::text NOT LIKE $1 ESCAPE '!'"
        );
        assert_eq!(
            pg(FilterOp::ContainsCase, "Bob"),
            (
                "\"name\"::text ILIKE $1 ESCAPE '!'".into(),
                vec![text("%Bob%")]
            )
        );
        assert_eq!(
            pg(FilterOp::NotContainsCase, "Bob").0,
            "\"name\"::text NOT ILIKE $1 ESCAPE '!'"
        );
        assert_eq!(pg(FilterOp::HasPrefix, "Bo").1, vec![text("Bo%")]);
        assert_eq!(pg(FilterOp::HasSuffix, "ob").1, vec![text("%ob")]);
        assert_eq!(
            pg(FilterOp::HasPrefixCase, "bo"),
            (
                "\"name\"::text ILIKE $1 ESCAPE '!'".into(),
                vec![text("bo%")]
            )
        );
        assert_eq!(pg(FilterOp::HasSuffixCase, "OB").1, vec![text("%OB")]);
        // ILIKE / LIKE take the user's own pattern, unescaped.
        assert_eq!(
            pg(FilterOp::Ilike, "b_b%"),
            ("\"name\"::text ILIKE $1".into(), vec![text("b_b%")])
        );
        assert_eq!(pg(FilterOp::Like, "b%").0, "\"name\"::text LIKE $1");
    }

    #[test]
    fn mysql_contains_and_case_sensitive_variant() {
        let cols = users();
        let my = |op, v: &str| where_of(Dialect::Mysql, &cols, cond("name", op, vec![text(v)]));
        assert_eq!(
            my(FilterOp::Contains, "a%"),
            ("`name` LIKE ? ESCAPE '!'".into(), vec![text("%a!%%")])
        );
        assert_eq!(
            my(FilterOp::ContainsCase, "Bob"),
            (
                "CAST(`name` AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_bin LIKE ? ESCAPE '!'"
                    .into(),
                vec![text("%Bob%")]
            )
        );
        assert_eq!(
            my(FilterOp::NotContainsCase, "Bob").0,
            "CAST(`name` AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_bin NOT LIKE ? ESCAPE '!'"
        );
        assert_eq!(my(FilterOp::HasPrefix, "Bo").1, vec![text("Bo%")]);
        assert_eq!(
            my(FilterOp::HasSuffixCase, "ob").0,
            "CAST(`name` AS CHAR CHARACTER SET utf8mb4) COLLATE utf8mb4_bin LIKE ? ESCAPE '!'"
        );
        // PostgreSQL-only operators are refused.
        let mut params = Vec::new();
        for op in [
            FilterOp::Ilike,
            FilterOp::JsonContains,
            FilterOp::JsonNotContains,
        ] {
            let group = FilterGroup {
                conjunction: Conjunction::And,
                items: vec![cond("name", op, vec![text("x")])],
            };
            assert!(build_where(Dialect::Mysql, &cols, Some(&group), &mut params).is_err());
            assert!(build_where(Dialect::Sqlite, &cols, Some(&group), &mut params).is_err());
        }
    }

    #[test]
    fn sqlite_contains_is_case_sensitive_variant_uses_like() {
        let cols = users();
        let lite = |op, v: &str| where_of(Dialect::Sqlite, &cols, cond("name", op, vec![text(v)]));
        assert_eq!(
            lite(FilterOp::Contains, "50%"),
            ("instr(\"name\", ?1) > 0".into(), vec![text("50%")])
        );
        assert_eq!(
            lite(FilterOp::NotContains, "x").0,
            "instr(\"name\", ?1) = 0"
        );
        assert_eq!(
            lite(FilterOp::HasPrefix, "a*"),
            ("\"name\" GLOB ?1".into(), vec![text("a[*]*")])
        );
        assert_eq!(lite(FilterOp::HasSuffix, "z").1, vec![text("*z")]);
        assert_eq!(
            lite(FilterOp::ContainsCase, "a_"),
            ("\"name\" LIKE ?1 ESCAPE '!'".into(), vec![text("%a!_%")])
        );
        assert_eq!(
            lite(FilterOp::NotContainsCase, "a").0,
            "\"name\" NOT LIKE ?1 ESCAPE '!'"
        );
        assert_eq!(lite(FilterOp::HasPrefixCase, "a").1, vec![text("a%")]);
        assert_eq!(lite(FilterOp::HasSuffixCase, "a").1, vec![text("%a")]);
    }

    #[test]
    fn not_between_and_json_containment() {
        let mut cols = users();
        cols.push(col("meta", "jsonb", ValueKind::Json, false));
        let (sql, params) = where_of(
            Dialect::Postgres,
            &cols,
            cond(
                "age",
                FilterOp::NotBetween,
                vec![Value::Int(1), Value::Int(9)],
            ),
        );
        assert_eq!(sql, "\"age\" NOT BETWEEN $1::integer AND $2::integer");
        assert_eq!(params, vec![Value::Int(1), Value::Int(9)]);
        assert_eq!(
            where_of(
                Dialect::Sqlite,
                &cols,
                cond(
                    "age",
                    FilterOp::NotBetween,
                    vec![Value::Int(1), Value::Int(9)]
                )
            )
            .0,
            "\"age\" NOT BETWEEN ?1 AND ?2"
        );
        let (sql, params) = where_of(
            Dialect::Postgres,
            &cols,
            cond(
                "meta",
                FilterOp::JsonContains,
                vec![Value::Json("{\"a\":1}".into())],
            ),
        );
        assert_eq!(sql, "\"meta\"::jsonb @> $1::jsonb");
        assert_eq!(params, vec![text("{\"a\":1}")]);
        assert_eq!(
            where_of(
                Dialect::Postgres,
                &cols,
                cond("meta", FilterOp::JsonNotContains, vec![text("[1]")])
            )
            .0,
            "NOT (\"meta\"::jsonb @> $1::jsonb)"
        );
        // Not a JSON column.
        let group = FilterGroup {
            conjunction: Conjunction::And,
            items: vec![cond("name", FilterOp::JsonContains, vec![text("{}")])],
        };
        assert!(build_where(Dialect::Postgres, &cols, Some(&group), &mut Vec::new()).is_err());
    }

    #[test]
    fn any_column_ors_text_forms_and_ands_negations() {
        let mut cols = users();
        cols.push(col("avatar", "bytea", ValueKind::Bytes, false));
        let any = |d, op, values| where_of(d, &cols, FilterNode::AnyColumn { op, values });
        let (sql, params) = any(Dialect::Postgres, FilterOp::Contains, vec![text("bob")]);
        assert_eq!(
            sql,
            "(COALESCE(\"id\"::text, '') LIKE $1 ESCAPE '!' OR COALESCE(\"name\"::text, '') LIKE $1 ESCAPE '!' OR COALESCE(\"age\"::text, '') LIKE $1 ESCAPE '!' OR COALESCE(\"status\"::text, '') LIKE $1 ESCAPE '!')"
        );
        // The blob column is skipped; the value is bound once and its placeholder repeated.
        assert_eq!(params, vec![text("%bob%")]);
        let (sql, params) = any(Dialect::Mysql, FilterOp::Eq, vec![Value::Int(7)]);
        assert_eq!(
            sql,
            "(COALESCE(CAST(`id` AS CHAR CHARACTER SET utf8mb4), '') = ? OR COALESCE(CAST(`name` AS CHAR CHARACTER SET utf8mb4), '') = ? OR COALESCE(CAST(`age` AS CHAR CHARACTER SET utf8mb4), '') = ? OR COALESCE(CAST(`status` AS CHAR CHARACTER SET utf8mb4), '') = ?)"
        );
        // Values compare as text; MySQL's `?` cannot repeat: one per column.
        assert_eq!(params, vec![text("7"); 4]);
        let (sql, _) = any(Dialect::Sqlite, FilterOp::NotContains, vec![text("x")]);
        assert_eq!(
            sql,
            "(instr(COALESCE(CAST(\"id\" AS TEXT), ''), ?1) = 0 AND instr(COALESCE(CAST(\"name\" AS TEXT), ''), ?1) = 0 AND instr(COALESCE(CAST(\"age\" AS TEXT), ''), ?1) = 0 AND instr(COALESCE(CAST(\"status\" AS TEXT), ''), ?1) = 0)"
        );
        let (sql, _) = any(Dialect::Postgres, FilterOp::IsNull, vec![]);
        assert_eq!(
            sql,
            "(\"id\" IS NULL OR \"name\" IS NULL OR \"age\" IS NULL OR \"status\" IS NULL)"
        );
        let (sql, _) = any(Dialect::Postgres, FilterOp::Ne, vec![text("a")]);
        assert!(sql.contains(" AND ") && !sql.contains(" OR "), "{sql}");
        assert!(
            sql.starts_with("(COALESCE(\"id\"::text, '') <> $1::text AND"),
            "{sql}"
        );
        let (sql, _) = any(Dialect::Mysql, FilterOp::ContainsCase, vec![text("A")]);
        assert!(sql.starts_with(
            "(COALESCE(CAST(`id` AS CHAR CHARACTER SET utf8mb4), '') COLLATE utf8mb4_bin LIKE ? ESCAPE '!' OR"
        ), "{sql}");
        // JSON containment only looks at JSON columns; none here.
        assert_eq!(
            any(Dialect::Postgres, FilterOp::JsonContains, vec![text("{}")]).0,
            "1 = 0"
        );
        assert_eq!(
            any(
                Dialect::Postgres,
                FilterOp::JsonNotContains,
                vec![text("{}")]
            )
            .0,
            "1 = 1"
        );
        // The preview inlines every placeholder.
        let (sql, params) = any(Dialect::Mysql, FilterOp::HasPrefix, vec![text("it's")]);
        let shown = inline_params(Dialect::Mysql, &sql, &params);
        assert!(!shown.contains('?'), "{shown}");
        assert!(shown.contains("LIKE 'it\\'s%' ESCAPE '!'"), "{shown}");
    }

    #[test]
    fn any_column_combines_with_other_conditions() {
        let cols = users();
        let f = FilterGroup {
            conjunction: Conjunction::And,
            items: vec![
                FilterNode::AnyColumn {
                    op: FilterOp::Eq,
                    values: vec![text("x")],
                },
                cond("age", FilterOp::Gt, vec![Value::Int(3)]),
            ],
        };
        let built = build_count(
            Dialect::Sqlite,
            &TableRef::new(None, "users"),
            &cols,
            Some(&f),
        )
        .unwrap();
        assert!(built.sql.contains(") AND \"age\" > ?2"), "{}", built.sql);
        assert_eq!(built.params.len(), 2);
    }

    #[test]
    fn any_column_in_lists_bind_each_value_once() {
        let cols: Vec<ColumnMeta> = (0..100)
            .map(|i| col(&format!("c{i}"), "text", ValueKind::Text, false))
            .collect();
        let values: Vec<Value> = (0..1000).map(|i| text(&i.to_string())).collect();
        let group = |op| FilterGroup {
            conjunction: Conjunction::And,
            items: vec![FilterNode::AnyColumn {
                op,
                values: values.clone(),
            }],
        };
        for d in [Dialect::Postgres, Dialect::Sqlite] {
            let mut params = Vec::new();
            let sql = build_where(d, &cols, Some(&group(FilterOp::In)), &mut params)
                .unwrap()
                .unwrap();
            assert_eq!(params.len(), 1000, "{d:?}");
            assert_eq!(sql.matches(&d.placeholder(1000)).count(), 100, "{d:?}");
            // The preview inlines repeated placeholders too.
            let shown = inline_params(d, &sql, &params);
            assert_eq!(shown.matches("'999'").count(), 100, "{d:?}");
        }
        // MySQL binds per column: 100,000 values are refused with a clear error.
        let err = build_where(
            Dialect::Mysql,
            &cols,
            Some(&group(FilterOp::In)),
            &mut Vec::new(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("100000 values") && err.contains("Any column"),
            "{err}"
        );
        // A value pushed after an Any column condition gets the next number.
        let mut params = vec![Value::Int(1)];
        let f = FilterGroup {
            conjunction: Conjunction::And,
            items: vec![
                FilterNode::AnyColumn {
                    op: FilterOp::Between,
                    values: vec![text("a"), text("b")],
                },
                cond("c1", FilterOp::Eq, vec![text("z")]),
            ],
        };
        let sql = build_where(Dialect::Postgres, &cols[..2], Some(&f), &mut params)
            .unwrap()
            .unwrap();
        assert_eq!(
            sql,
            "(COALESCE(\"c0\"::text, '') BETWEEN $2::text AND $3::text OR COALESCE(\"c1\"::text, '') BETWEEN $2::text AND $3::text) AND \"c1\" = $4::text"
        );
        assert_eq!(params.len(), 4);
    }

    #[test]
    fn postgres_casts_drop_type_modifiers() {
        for (ty, cast) in [
            ("character varying(5)", "character varying"),
            ("varchar(5)", "varchar"),
            ("character(5)", "bpchar"),
            ("character", "bpchar"),
            ("char(3)", "bpchar"),
            ("\"char\"", "\"char\""),
            ("bpchar", "bpchar"),
            ("bit(8)", "bit varying"),
            ("bit varying(8)", "bit varying"),
            ("numeric(10,2)", "numeric"),
            ("numeric", "numeric"),
            ("timestamp(3) with time zone", "timestamp with time zone"),
            (
                "timestamp(0) without time zone",
                "timestamp without time zone",
            ),
            ("time(6) without time zone", "time without time zone"),
            ("interval day to second(3)", "interval"),
            ("interval(2)", "interval"),
            ("character varying(3)[]", "character varying[]"),
            ("character(2)[][]", "bpchar[][]"),
            ("public.geometry(Point,4326)", "public.geometry"),
            ("\"my(type)\"", "\"my(type)\""),
            ("integer", "integer"),
            ("double precision", "double precision"),
            ("text", "text"),
            ("", ""),
        ] {
            assert_eq!(pg_cast_type(ty), cast, "{ty}");
            assert_eq!(pg_cast_type(cast), cast, "idempotent: {cast}");
        }
        // A filter compares with the value as given, not cut to the column's size.
        let cols = vec![col("code", "character varying(5)", ValueKind::Text, false)];
        let (sql, _) = where_of(
            Dialect::Postgres,
            &cols,
            cond("code", FilterOp::Eq, vec![text("abcdeZZZZ")]),
        );
        assert_eq!(sql, "\"code\" = $1::character varying");
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
