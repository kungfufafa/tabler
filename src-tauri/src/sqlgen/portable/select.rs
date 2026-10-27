//! Browse SELECT, COUNT, the fetch of one cell and the WHERE of grid filters for the extension
//! dialects, as their spec shapes them: placeholder style, paging, NULL ordering,
//! case-insensitive matching, text projections.

use crate::error::AppError;
use crate::sqlgen::dialect::qualify_with;
use crate::sqlgen::dialect::spec::{
    CiLike, DialectSpec, LimitOne, NullSafeEq, NullsSort, Paging, RowLocator,
};
use crate::sqlgen::select::{BuiltSql, ROWID_ALIAS};
use crate::sqlgen::sql::{SelectQuery, TableTarget};
use crate::types::browse::{
    Conjunction, FilterGroup, FilterNode, FilterOp, NullsOrder, RowIdentity, RowKey, SortDir,
    SortSpec,
};
use crate::types::schema::{ColumnMeta, TableRef, ValueKind};
use crate::types::value::Value;

/// Collects bind parameters and writes their placeholders.
pub(crate) struct Params<'a> {
    spec: &'a DialectSpec,
    values: &'a mut Vec<Value>,
    /// "Any column": the parameter numbers the first column's condition pushed, which the next
    /// columns reuse where placeholders may repeat (with the position of the next one).
    replay: Option<(Vec<usize>, usize)>,
}

impl<'a> Params<'a> {
    pub(crate) fn new(spec: &'a DialectSpec, values: &'a mut Vec<Value>) -> Params<'a> {
        Params {
            spec,
            values,
            replay: None,
        }
    }

    /// Pushes `value` (a value of `column`, when it is one) and returns its placeholder.
    pub(crate) fn push(&mut self, value: Value, column: Option<&ColumnMeta>) -> String {
        let reused = match &mut self.replay {
            Some((numbers, next)) => {
                let n = numbers.get(*next).copied();
                *next += 1;
                n.filter(|&n| self.values.get(n - 1) == Some(&value))
            }
            None => None,
        };
        let n = match reused {
            Some(n) => n,
            None => {
                self.values.push(value);
                self.values.len()
            }
        };
        let ph = (self.spec.placeholder)(n, column);
        match (self.spec.knobs.param_cast, column) {
            (Some(cast), Some(c)) => cast(ph, c),
            _ => ph,
        }
    }

    /// One value used twice in an expression: one placeholder where placeholders repeat,
    /// otherwise two.
    fn push_twice(&mut self, value: Value, column: Option<&ColumnMeta>) -> (String, String) {
        let first = self.push(value.clone(), column);
        if self.spec.placeholders_reusable {
            (first.clone(), first)
        } else {
            (first, self.push(value, column))
        }
    }
}

pub(crate) fn find_column<'c>(
    columns: &'c [ColumnMeta],
    name: &str,
) -> Result<&'c ColumnMeta, AppError> {
    columns
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| AppError::InvalidInput(format!("unknown column \"{name}\"")))
}

pub(crate) fn ident(spec: &DialectSpec, name: &str) -> String {
    (spec.quote_ident)(name)
}

pub(crate) fn table_name(spec: &DialectSpec, table: &TableRef) -> String {
    qualify_with(spec, table.schema.as_deref(), &table.name)
}

/// The text projection of the column expression `e`, when the dialect sends `col` as text.
pub(crate) fn projection(spec: &DialectSpec, e: &str, col: &ColumnMeta) -> Option<String> {
    spec.knobs.text_projection.and_then(|p| p(e, col))
}

/// Whether `col` reaches the grid through a projection that loses data.
pub fn is_lossy(spec: &DialectSpec, col: &ColumnMeta) -> bool {
    projection(spec, &ident(spec, &col.name), col).is_some() && (spec.knobs.lossy_text)(col)
}

/// Refuses a column the grid shows cut: editing it, keying a row on it or loading it whole would
/// work on the cut value.
pub(crate) fn check_whole(
    spec: &DialectSpec,
    col: &ColumnMeta,
    what: &str,
) -> Result<(), AppError> {
    if is_lossy(spec, col) {
        Err(AppError::Unsupported(format!(
            "\"{}\" is shown shortened in {}, so it cannot {what}",
            col.name, spec.label
        )))
    } else {
        Ok(())
    }
}

/// The select-list expression of `e` for `col`: its projection `AS` the column's name, or `e`.
pub(crate) fn select_expr(spec: &DialectSpec, e: &str, col: &ColumnMeta) -> String {
    match projection(spec, e, col) {
        Some(p) => format!("{p} AS {}", ident(spec, &col.name)),
        None => e.to_string(),
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
        FilterOp::NotBetween => "NOT BETWEEN",
        FilterOp::Ilike
        | FilterOp::Contains
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

/// What a condition applies to: a column, or (for "Any column") its textual form.
struct Target<'c> {
    /// For error messages.
    name: String,
    /// The column itself: IS NULL / IS NOT NULL, JSON containment.
    raw: String,
    /// Comparisons, IN and BETWEEN.
    cmp: String,
    /// Textual form for pattern matching; None when the column has none.
    text: Option<String>,
    /// The column whose type a compared value takes (typed placeholders, casts).
    column: Option<&'c ColumnMeta>,
    json: bool,
}

impl<'c> Target<'c> {
    fn column(spec: &DialectSpec, col: &'c ColumnMeta) -> Target<'c> {
        let id = ident(spec, &col.name);
        Target {
            name: col.name.clone(),
            text: (spec.knobs.text_expr)(&id, col),
            raw: id.clone(),
            cmp: id,
            column: Some(col),
            json: col.kind == ValueKind::Json,
        }
    }

    /// One column of an "Any column" condition: everything compares as text, NULL as ''. None
    /// when the column has no textual form.
    fn any(spec: &DialectSpec, col: &'c ColumnMeta) -> Option<Target<'c>> {
        let id = ident(spec, &col.name);
        let text = format!("COALESCE({}, '')", (spec.knobs.text_expr)(&id, col)?);
        Some(Target {
            name: col.name.clone(),
            raw: id,
            cmp: text.clone(),
            text: Some(text),
            column: None,
            json: col.kind == ValueKind::Json,
        })
    }

    fn text(&self, spec: &DialectSpec) -> Result<&str, AppError> {
        self.text.as_deref().ok_or_else(|| {
            AppError::Unsupported(format!(
                "{}: \"{}\" cannot be matched as text",
                spec.label, self.name
            ))
        })
    }
}

/// Refuses operators the dialect has no form of.
fn check_op(spec: &DialectSpec, op: FilterOp) -> Result<(), AppError> {
    let case_insensitive = matches!(
        op,
        FilterOp::Ilike
            | FilterOp::ContainsCase
            | FilterOp::NotContainsCase
            | FilterOp::HasPrefixCase
            | FilterOp::HasSuffixCase
    );
    if case_insensitive && spec.knobs.ci_like == CiLike::Unsupported {
        return Err(AppError::InvalidInput(format!(
            "{} has no case-insensitive match",
            spec.label
        )));
    }
    if matches!(op, FilterOp::JsonContains | FilterOp::JsonNotContains)
        && spec.knobs.json_contains.is_none()
    {
        return Err(AppError::InvalidInput(format!(
            "{} has no JSON contains",
            spec.label
        )));
    }
    Ok(())
}

fn need(t: &Target<'_>, values: &[Value], n: usize) -> Result<(), AppError> {
    if values.len() < n {
        Err(AppError::InvalidInput(format!(
            "filter on \"{}\" needs {n} value(s)",
            t.name
        )))
    } else {
        Ok(())
    }
}

/// `text` matching the pattern placeholder `ph`, case-insensitively when `ci`.
fn like(spec: &DialectSpec, text: &str, not: &str, ph: &str, ci: bool, escape: &str) -> String {
    match (ci, spec.knobs.ci_like) {
        (true, CiLike::Ilike) => format!("{text} {not}ILIKE {ph}{escape}"),
        (true, CiLike::LowerLike | CiLike::Unsupported) => {
            format!("LOWER({text}) {not}LIKE LOWER({ph}){escape}")
        }
        (false, _) => format!("{text} {not}LIKE {ph}{escape}"),
    }
}

/// Escapes LIKE's wildcards and the escape character itself in `text`.
pub fn escape_like(text: &str, escape: char, wildcards: &[char]) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    for ch in text.chars() {
        if ch == escape || wildcards.contains(&ch) {
            out.push(escape);
        }
        out.push(ch);
    }
    out
}

/// Contains / Has prefix / Has suffix and their negated and case-insensitive variants: the value
/// matches literally (wildcards escaped).
fn pattern_sql(
    spec: &DialectSpec,
    params: &mut Params<'_>,
    t: &Target<'_>,
    op: FilterOp,
    value: &Value,
) -> Result<String, AppError> {
    #[derive(PartialEq)]
    enum Shape {
        Contains,
        Prefix,
        Suffix,
    }
    let (negated, ci, shape) = match op {
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
    let value = value
        .as_text()
        .ok_or_else(|| AppError::InvalidInput(format!("filter on \"{}\" needs a value", t.name)))?;
    let text = t.text(spec)?;
    let (escape, clause) = match spec.knobs.like_escape {
        Some(c) => (c, format!(" ESCAPE '{c}'")),
        None => ('\\', String::new()),
    };
    let esc = escape_like(&value, escape, spec.knobs.like_wildcards);
    let pattern = match shape {
        Shape::Contains => format!("%{esc}%"),
        Shape::Prefix => format!("{esc}%"),
        Shape::Suffix => format!("%{esc}"),
    };
    let ph = params.push(Value::Text(pattern), None);
    let not = if negated { "NOT " } else { "" };
    Ok(like(spec, text, not, &ph, ci, &clause))
}

/// One condition on `t`.
fn target_sql(
    spec: &DialectSpec,
    params: &mut Params<'_>,
    t: &Target<'_>,
    op: FilterOp,
    values: &[Value],
) -> Result<String, AppError> {
    check_op(spec, op)?;
    Ok(match op {
        FilterOp::IsNull | FilterOp::IsNotNull => format!("{} {}", t.raw, op_sql(op)),
        FilterOp::Like | FilterOp::NotLike | FilterOp::Ilike => {
            need(t, values, 1)?;
            // Pattern matching always happens on the textual form.
            let text = t.text(spec)?;
            let ph = params.push(values[0].clone(), None);
            match op {
                FilterOp::Ilike => like(spec, text, "", &ph, true, ""),
                _ => format!("{text} {} {ph}", op_sql(op)),
            }
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
                .map(|v| params.push(v.clone(), t.column))
                .collect();
            format!("{} {} ({})", t.cmp, op_sql(op), phs.join(", "))
        }
        FilterOp::Between | FilterOp::NotBetween => {
            need(t, values, 2)?;
            let a = params.push(values[0].clone(), t.column);
            let b = params.push(values[1].clone(), t.column);
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
            pattern_sql(spec, params, t, op, &values[0])?
        }
        FilterOp::JsonContains | FilterOp::JsonNotContains => {
            need(t, values, 1)?;
            if !t.json {
                return Err(AppError::InvalidInput(format!(
                    "JSON contains needs a JSON column; \"{}\" is not one",
                    t.name
                )));
            }
            let contains = spec.knobs.json_contains.ok_or_else(|| {
                AppError::InvalidInput(format!("{} has no JSON contains", spec.label))
            })?;
            let text = values[0].as_text().map(Value::Text).unwrap_or(Value::Null);
            let ph = params.push(text, None);
            let test = contains(&t.raw, &ph);
            if op == FilterOp::JsonNotContains {
                format!("NOT ({test})")
            } else {
                test
            }
        }
        FilterOp::Eq | FilterOp::Ne | FilterOp::Gt | FilterOp::Lt | FilterOp::Ge | FilterOp::Le => {
            need(t, values, 1)?;
            let ph = params.push(values[0].clone(), t.column);
            format!("{} {} {ph}", t.cmp, op_sql(op))
        }
    })
}

/// Whether "Any column" looks at `col` (as the core builders decide).
fn any_column_applies(col: &ColumnMeta, op: FilterOp) -> bool {
    if matches!(col.kind, ValueKind::Bytes | ValueKind::Geometry) {
        return false;
    }
    !matches!(op, FilterOp::JsonContains | FilterOp::JsonNotContains) || col.kind == ValueKind::Json
}

/// "Any column": the condition on every column's textual form, OR-ed; a negated operator is
/// AND-ed instead (no column matches its positive form).
fn any_column_sql(
    spec: &DialectSpec,
    params: &mut Params<'_>,
    columns: &[ColumnMeta],
    op: FilterOp,
    values: &[Value],
) -> Result<String, AppError> {
    check_op(spec, op)?;
    let values: Vec<Value> = values
        .iter()
        .map(|v| v.as_text().map(Value::Text).unwrap_or(Value::Null))
        .collect();
    let mut parts = Vec::new();
    let mut first: Option<Vec<usize>> = None;
    for col in columns.iter().filter(|c| any_column_applies(c, op)) {
        let Some(target) = Target::any(spec, col) else {
            continue;
        };
        let start = params.values.len();
        params.replay = first.clone().map(|numbers| (numbers, 0));
        let part = target_sql(spec, params, &target, op, &values);
        params.replay = None;
        parts.push(part?);
        if first.is_none() && spec.placeholders_reusable {
            first = Some((start + 1..=params.values.len()).collect());
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
    spec: &DialectSpec,
    params: &mut Params<'_>,
    columns: &[ColumnMeta],
    group: &FilterGroup,
) -> Result<Option<String>, AppError> {
    let mut parts = Vec::new();
    for item in &group.items {
        match item {
            FilterNode::Condition { column, op, values } => {
                let col = find_column(columns, column)?;
                parts.push(target_sql(
                    spec,
                    params,
                    &Target::column(spec, col),
                    *op,
                    values,
                )?);
            }
            FilterNode::AnyColumn { op, values } => {
                parts.push(any_column_sql(spec, params, columns, *op, values)?);
            }
            FilterNode::Group(g) => {
                if let Some(sql) = group_sql(spec, params, columns, g)? {
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

/// The WHERE body (without the keyword) of `filter`, appending its parameters to `params`.
pub fn build_where(
    spec: &DialectSpec,
    columns: &[ColumnMeta],
    filter: Option<&FilterGroup>,
    params: &mut Vec<Value>,
) -> Result<Option<String>, AppError> {
    let Some(filter) = filter else {
        return Ok(None);
    };
    let sql = group_sql(spec, &mut Params::new(spec, params), columns, filter)?;
    let limit = spec.max_params as usize;
    if params.len() > limit {
        return Err(AppError::InvalidInput(format!(
            "the filter needs {} values, more than one query can bind ({limit}): use fewer \
             values{}",
            params.len(),
            if spec.placeholders_reusable {
                ""
            } else {
                " or filter one column instead of Any column"
            }
        )));
    }
    Ok(sql)
}

fn order_terms(
    spec: &DialectSpec,
    columns: &[ColumnMeta],
    sort: &[SortSpec],
    pk: &[String],
    rowid: bool,
) -> Result<Vec<String>, AppError> {
    let mut terms: Vec<String> = Vec::new();
    for s in sort {
        find_column(columns, &s.column)?;
        let id = ident(spec, &s.column);
        let dir = match s.dir {
            SortDir::Asc => "ASC",
            SortDir::Desc => "DESC",
        };
        let Some(nulls) = s.nulls else {
            terms.push(format!("{id} {dir}"));
            continue;
        };
        match spec.knobs.nulls_order {
            NullsSort::Native => terms.push(match nulls {
                NullsOrder::First => format!("{id} {dir} NULLS FIRST"),
                NullsOrder::Last => format!("{id} {dir} NULLS LAST"),
            }),
            NullsSort::NullFlag => {
                let flag_dir = match nulls {
                    NullsOrder::First => "DESC",
                    NullsOrder::Last => "ASC",
                };
                terms.push(format!(
                    "CASE WHEN {id} IS NULL THEN 1 ELSE 0 END {flag_dir}"
                ));
                terms.push(format!("{id} {dir}"));
            }
            NullsSort::Unsupported => {
                return Err(AppError::InvalidInput(format!(
                    "{} cannot sort NULLs first or last",
                    spec.label
                )));
            }
        }
    }
    if terms.is_empty() {
        // Stable paging needs a deterministic order: the primary key, or the rowid.
        if !pk.is_empty() {
            terms.extend(pk.iter().map(|c| format!("{} ASC", ident(spec, c))));
        } else if rowid {
            terms.push("rowid ASC".into());
        }
    }
    Ok(terms)
}

/// The paged SELECT of a table browse.
pub fn build_select(spec: &DialectSpec, q: &SelectQuery<'_>) -> Result<BuiltSql, AppError> {
    if let Some(hook) = spec.hooks.build_select {
        return hook(spec, q);
    }
    let cols: Vec<&ColumnMeta> = match q.selected {
        Some(names) => names
            .iter()
            .map(|n| find_column(q.columns, n))
            .collect::<Result<_, _>>()?,
        None => q.columns.iter().collect(),
    };
    if cols.is_empty() {
        return Err(AppError::InvalidInput("no columns to select".into()));
    }
    let rowid = q.identity == RowIdentity::Rowid && spec.knobs.row_locator == RowLocator::Rowid;
    let mut list: Vec<String> = cols
        .iter()
        .map(|c| select_expr(spec, &ident(spec, &c.name), c))
        .collect();
    if rowid {
        list.push(format!("rowid AS {}", ident(spec, ROWID_ALIAS)));
    }
    let mut params = Vec::new();
    let where_sql = build_where(spec, q.columns, q.filter, &mut params)?;
    let mut terms = order_terms(spec, q.columns, q.sort, q.pk, rowid)?;
    let mut sql = format!(
        "SELECT {}\nFROM {}",
        list.join(", "),
        table_name(spec, q.table)
    );
    if let Some(w) = where_sql {
        sql.push_str("\nWHERE ");
        sql.push_str(&w);
    }
    let page = match spec.knobs.paging {
        Paging::LimitOffset => format!("\nLIMIT {} OFFSET {}", q.limit, q.offset),
        Paging::OffsetFetch => {
            // OFFSET … FETCH needs an ORDER BY, even one that orders nothing.
            if terms.is_empty() {
                terms.push("(SELECT NULL)".into());
            }
            format!(
                "\nOFFSET {} ROWS FETCH NEXT {} ROWS ONLY",
                q.offset, q.limit
            )
        }
        Paging::LimitOnly => {
            if q.offset > 0 {
                return Err(AppError::Unsupported(format!(
                    "{} cannot skip rows: only the first page can be read this way",
                    spec.label
                )));
            }
            format!("\nLIMIT {}", q.limit)
        }
    };
    if !terms.is_empty() {
        sql.push_str(" ORDER BY ");
        sql.push_str(&terms.join(", "));
    }
    sql.push_str(&page);
    Ok(BuiltSql { sql, params })
}

/// `SELECT count(*) FROM table WHERE …`.
pub fn build_count(
    spec: &DialectSpec,
    table: &TableRef,
    columns: &[ColumnMeta],
    filter: Option<&FilterGroup>,
) -> Result<BuiltSql, AppError> {
    if let Some(hook) = spec.hooks.build_count {
        return hook(spec, table, columns, filter);
    }
    let mut params = Vec::new();
    let where_sql = build_where(spec, columns, filter, &mut params)?;
    let mut sql = format!(
        "SELECT {} FROM {}",
        spec.knobs.count_expr,
        table_name(spec, table)
    );
    if let Some(w) = where_sql {
        sql.push_str(" WHERE ");
        sql.push_str(&w);
    }
    Ok(BuiltSql { sql, params })
}

/// The condition that addresses one row by `key`. An edit (`read_only` false) takes a primary
/// key, or a rowid where the dialect has one; a read may also match every column of a row
/// (NULL-safe), since it changes nothing whatever it matches.
pub(crate) fn row_condition(
    spec: &DialectSpec,
    columns: &[ColumnMeta],
    key: &RowKey,
    params: &mut Params<'_>,
    read_only: bool,
) -> Result<String, AppError> {
    match key {
        RowKey::Pk(map) => {
            if map.is_empty() {
                return Err(AppError::InvalidInput("row key is empty".into()));
            }
            let mut parts = Vec::with_capacity(map.len());
            for (name, v) in map {
                let col = find_column(columns, name)?;
                check_whole(spec, col, "identify a row")?;
                let id = ident(spec, name);
                if v.is_null() {
                    parts.push(format!("{id} IS NULL"));
                } else {
                    parts.push(format!("{id} = {}", params.push(v.clone(), Some(col))));
                }
            }
            Ok(parts.join(" AND "))
        }
        RowKey::Rowid(id) => {
            if spec.knobs.row_locator != RowLocator::Rowid {
                return Err(AppError::Unsupported(format!(
                    "{} has no row id: rows are addressed by their primary key",
                    spec.label
                )));
            }
            Ok(format!(
                "rowid = {}",
                params.push(Value::from_i64(*id), None)
            ))
        }
        RowKey::AllColumns(map) => {
            if !read_only {
                return Err(AppError::Unsupported(format!(
                    "{}: a row of a table without a primary key cannot be edited",
                    spec.label
                )));
            }
            if map.is_empty() {
                return Err(AppError::InvalidInput("row key is empty".into()));
            }
            let mut parts = Vec::with_capacity(map.len());
            for (name, v) in map {
                let col = find_column(columns, name)?;
                check_whole(spec, col, "identify a row")?;
                // A column the grid gets as text compares in that form.
                let id = ident(spec, name);
                let (lhs, column) = match projection(spec, &id, col) {
                    Some(p) => (p, None),
                    None => (id, Some(col)),
                };
                if v.is_null() {
                    parts.push(format!("{lhs} IS NULL"));
                    continue;
                }
                parts.push(match spec.knobs.null_safe_eq {
                    NullSafeEq::IsNotDistinctFrom => {
                        format!(
                            "{lhs} IS NOT DISTINCT FROM {}",
                            params.push(v.clone(), column)
                        )
                    }
                    NullSafeEq::OrForm => {
                        let (a, b) = params.push_twice(v.clone(), column);
                        format!("({lhs} = {a} OR ({lhs} IS NULL AND {b} IS NULL))")
                    }
                });
            }
            Ok(parts.join(" AND "))
        }
    }
}

/// `SELECT <column> FROM table WHERE <row> LIMIT 1` (or `TOP (1)`) for one whole cell. The cell
/// comes back as the browse query sends it: through the column's projection, if it has one.
pub fn build_fetch_cell(
    spec: &DialectSpec,
    t: &TableTarget<'_>,
    column: &ColumnMeta,
    key: &RowKey,
) -> Result<(String, Vec<Value>), AppError> {
    if let Some(hook) = spec.hooks.build_fetch_cell {
        return hook(spec, t, column, key);
    }
    check_whole(spec, column, "be loaded whole")?;
    let id = ident(spec, &column.name);
    let expr = projection(spec, &id, column).unwrap_or(id);
    let mut params = Vec::new();
    let condition = row_condition(
        spec,
        t.columns,
        key,
        &mut Params::new(spec, &mut params),
        true,
    )?;
    let table = table_name(spec, t.table);
    let sql = match spec.knobs.limit_one {
        LimitOne::Limit => format!("SELECT {expr} FROM {table} WHERE {condition} LIMIT 1"),
        LimitOne::Top => format!("SELECT TOP (1) {expr} FROM {table} WHERE {condition}"),
    };
    Ok((sql, params))
}
