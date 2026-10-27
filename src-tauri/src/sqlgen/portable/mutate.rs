//! UPDATE / INSERT / DELETE of staged grid edits for the extension dialects. Rows are addressed
//! by primary key only (or by rowid where the spec has one): a statement that might touch more
//! than the row the user edited is never built.

use std::collections::BTreeMap;

use crate::error::AppError;
use crate::sqlgen::dialect::spec::{DialectSpec, Returning};
use crate::sqlgen::mutate::{Cell, cells};
use crate::sqlgen::portable::select::{
    Params, check_whole, find_column, ident, row_condition, select_expr, table_name,
};
use crate::sqlgen::select::BuiltSql;
use crate::sqlgen::sql::TableTarget;
use crate::types::browse::RowKey;
use crate::types::mutate::{ServerExpr, StagedChange};
use crate::types::schema::ColumnMeta;
use crate::types::value::Value;

fn is_default(cell: &Cell<'_>) -> bool {
    matches!(
        cell,
        Cell::Value(Value::Default) | Cell::Expr(ServerExpr::Default)
    )
}

/// The SQL of a server expression of a cell, or why the dialect has none.
fn server_expr(spec: &DialectSpec, expr: ServerExpr, col: &ColumnMeta) -> Result<String, AppError> {
    if let ServerExpr::Fsp(_, n) = expr
        && n > ServerExpr::MAX_FSP
    {
        return Err(AppError::InvalidInput(format!(
            "a time precision is 0 to {} digits, not {n}",
            ServerExpr::MAX_FSP
        )));
    }
    (spec.knobs.server_expr)(expr, col.kind).ok_or_else(|| {
        AppError::Unsupported(format!(
            "{} has no {} for \"{}\"",
            spec.label,
            expr.wire(),
            col.name
        ))
    })
}

/// The written row's columns, as `RETURNING` lists them or `OUTPUT` with `INSERTED.`.
fn returned_columns(spec: &DialectSpec, t: &TableTarget<'_>, prefix: &str) -> String {
    t.columns
        .iter()
        .map(|c| select_expr(spec, &format!("{prefix}{}", ident(spec, &c.name)), c))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `(output, returning)`: the clause before VALUES / WHERE and the one at the end.
fn returning_clauses(spec: &DialectSpec, t: &TableTarget<'_>, returning: bool) -> (String, String) {
    if !returning {
        return (String::new(), String::new());
    }
    match spec.knobs.returning {
        Returning::None => (String::new(), String::new()),
        Returning::Returning => (
            String::new(),
            format!(" RETURNING {}", returned_columns(spec, t, "")),
        ),
        Returning::OutputInserted => (
            format!(" OUTPUT {}", returned_columns(spec, t, "INSERTED.")),
            String::new(),
        ),
    }
}

pub fn build_update(
    spec: &DialectSpec,
    t: &TableTarget<'_>,
    key: &RowKey,
    changes: &BTreeMap<&str, Cell<'_>>,
    returning: bool,
) -> Result<BuiltSql, AppError> {
    if changes.is_empty() {
        return Err(AppError::InvalidInput(
            "update has no changed columns".into(),
        ));
    }
    let mut values = Vec::new();
    let mut params = Params::new(spec, &mut values);
    let mut sets = Vec::new();
    for (name, cell) in changes {
        let col = find_column(t.columns, name)?;
        check_whole(spec, col, "be edited")?;
        let id = ident(spec, name);
        if is_default(cell) {
            if !spec.knobs.set_default {
                return Err(AppError::Unsupported(format!(
                    "{} cannot SET a column to DEFAULT; enter the value explicitly",
                    spec.label
                )));
            }
            sets.push(format!("{id} = DEFAULT"));
            continue;
        }
        sets.push(match cell {
            Cell::Expr(e) => format!("{id} = {}", server_expr(spec, *e, col)?),
            Cell::Value(v) => format!("{id} = {}", params.push((*v).clone(), Some(col))),
        });
    }
    let condition = row_condition(spec, t.columns, key, &mut params, false)?;
    let (output, tail) = returning_clauses(spec, t, returning);
    let sql = format!(
        "UPDATE {} SET {}{output} WHERE {condition}{tail}",
        table_name(spec, t.table),
        sets.join(", "),
    );
    Ok(BuiltSql {
        sql,
        params: values,
    })
}

pub fn build_insert(
    spec: &DialectSpec,
    t: &TableTarget<'_>,
    cells: &BTreeMap<&str, Cell<'_>>,
    returning: bool,
) -> Result<BuiltSql, AppError> {
    let mut values = Vec::new();
    let mut params = Params::new(spec, &mut values);
    let mut names = Vec::new();
    let mut exprs = Vec::new();
    for (name, cell) in cells {
        let col = find_column(t.columns, name)?;
        // A duplicated row would copy the shortened text.
        check_whole(spec, col, "be edited")?;
        if is_default(cell) {
            continue; // omitted columns take their default
        }
        names.push(ident(spec, name));
        exprs.push(match cell {
            Cell::Expr(e) => server_expr(spec, *e, col)?,
            Cell::Value(v) => params.push((*v).clone(), Some(col)),
        });
    }
    let table = table_name(spec, t.table);
    let (output, tail) = returning_clauses(spec, t, returning);
    let sql = if names.is_empty() {
        let defaults = spec.knobs.insert_all_defaults.ok_or_else(|| {
            AppError::Unsupported(format!(
                "{} cannot insert a row of defaults only; enter at least one value",
                spec.label
            ))
        })?;
        format!("INSERT INTO {table}{output} {defaults}{tail}")
    } else {
        format!(
            "INSERT INTO {table} ({}){output} VALUES ({}){tail}",
            names.join(", "),
            exprs.join(", ")
        )
    };
    Ok(BuiltSql {
        sql,
        params: values,
    })
}

pub fn build_delete(
    spec: &DialectSpec,
    t: &TableTarget<'_>,
    key: &RowKey,
) -> Result<BuiltSql, AppError> {
    let mut values = Vec::new();
    let condition = row_condition(
        spec,
        t.columns,
        key,
        &mut Params::new(spec, &mut values),
        false,
    )?;
    Ok(BuiltSql {
        sql: format!(
            "DELETE FROM {} WHERE {condition}",
            table_name(spec, t.table)
        ),
        params: values,
    })
}

/// The statement of one staged change and how many rows it must affect.
pub fn build_change(
    spec: &DialectSpec,
    t: &TableTarget<'_>,
    change: &StagedChange,
    returning: bool,
) -> Result<(BuiltSql, Option<u64>), AppError> {
    if let Some(hook) = spec.hooks.build_change {
        return hook(spec, t, change, returning);
    }
    Ok(match change {
        StagedChange::Update {
            key,
            changes,
            exprs,
            ..
        } => (
            build_update(spec, t, key, &cells(changes, exprs.as_ref()), returning)?,
            Some(1),
        ),
        StagedChange::Insert { values, exprs, .. } => (
            build_insert(spec, t, &cells(values, exprs.as_ref()), returning)?,
            Some(1),
        ),
        StagedChange::Delete { key, .. } => (build_delete(spec, t, key)?, Some(1)),
    })
}
