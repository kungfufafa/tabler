//! UPDATE / INSERT / DELETE generation for staged grid edits. Rows are addressed by primary key,
//! SQLite rowid, or — as a last resort — every original column value (limited to one row).

use std::collections::BTreeMap;

use crate::error::AppError;
use crate::sqlgen::quote::Dialect;
use crate::sqlgen::select::{BuiltSql, pg_cast_type};
use crate::types::browse::RowKey;
use crate::types::mutate::{Clock, ServerExpr, StagedChange};
use crate::types::schema::{ColumnMeta, TableRef, ValueKind};
use crate::types::value::Value;

/// A staged cell: a value (bound as a parameter) or a whitelisted server expression.
#[derive(Debug, Clone, Copy)]
pub enum Cell<'a> {
    Value(&'a Value),
    Expr(ServerExpr),
}

impl Cell<'_> {
    /// `DEFAULT`, as a value or as an expression.
    fn is_default(&self) -> bool {
        matches!(
            self,
            Cell::Value(Value::Default) | Cell::Expr(ServerExpr::Default)
        )
    }
}

/// The cells of an UPDATE / INSERT by column: `values`, overridden by `exprs`.
pub fn cells<'a>(
    values: &'a BTreeMap<String, Value>,
    exprs: Option<&'a BTreeMap<String, ServerExpr>>,
) -> BTreeMap<&'a str, Cell<'a>> {
    let mut out: BTreeMap<&str, Cell> = values
        .iter()
        .map(|(k, v)| (k.as_str(), Cell::Value(v)))
        .collect();
    for (k, e) in exprs.into_iter().flatten() {
        out.insert(k.as_str(), Cell::Expr(*e));
    }
    out
}

/// SQL for a server expression a staged cell holds. SQLite has no `NOW()`: its `CURRENT_*`
/// keyword matching the column's kind (UTC, as SQLite's own `DEFAULT CURRENT_TIMESTAMP`).
/// `DEFAULT` is handled by the callers (an INSERT omits the column).
///
/// A clock with a fractional-seconds precision (`NOW(6)`, `CURRENT_TIMESTAMP(3)`) keeps it where
/// the server has one: MySQL / MariaDB take the argument on every clock (a bare
/// `CURRENT_TIMESTAMP` has 0 digits there whatever the column's precision), PostgreSQL on all
/// but `now()`, which is its own `CURRENT_TIMESTAMP(n)`. SQLite has no precision, so it gets the
/// plain keyword. A precision above 6 is refused.
pub fn server_expr_sql(d: Dialect, expr: ServerExpr, kind: ValueKind) -> Result<String, AppError> {
    if let ServerExpr::Fsp(_, n) = expr
        && n > ServerExpr::MAX_FSP
    {
        return Err(AppError::InvalidInput(format!(
            "a time precision is 0 to {} digits, not {n}",
            ServerExpr::MAX_FSP
        )));
    }
    let Some((clock, fsp)) = expr.clock() else {
        return Ok(match expr {
            ServerExpr::CurrentDate => "CURRENT_DATE",
            _ => "DEFAULT",
        }
        .into());
    };
    Ok(match (d, clock, fsp) {
        (Dialect::Sqlite, Clock::Now, _) => match kind {
            ValueKind::Date => "CURRENT_DATE".into(),
            ValueKind::Time => "CURRENT_TIME".into(),
            _ => "CURRENT_TIMESTAMP".into(),
        },
        (Dialect::Sqlite, Clock::LocalTimestamp, _) => "datetime('now', 'localtime')".into(),
        // The other SQLite clocks are plain keywords: it has no precision to carry.
        (Dialect::Sqlite, c, _) => c.keyword().into(),
        // PostgreSQL's now() takes no argument; CURRENT_TIMESTAMP(n) is the same instant.
        (Dialect::Postgres, Clock::Now, Some(n)) => format!("CURRENT_TIMESTAMP({n})"),
        (_, Clock::Now, None) => "NOW()".into(),
        (_, c, None) => c.keyword().into(),
        (_, c, Some(n)) => format!("{}({n})", c.keyword()),
    })
}

pub struct TableSpec<'a> {
    pub dialect: Dialect,
    pub table: &'a TableRef,
    pub columns: &'a [ColumnMeta],
}

impl TableSpec<'_> {
    fn column(&self, name: &str) -> Result<&ColumnMeta, AppError> {
        self.columns
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| AppError::InvalidInput(format!("unknown column \"{name}\"")))
    }

    fn target(&self) -> String {
        self.dialect
            .qualify(self.table.schema.as_deref(), &self.table.name)
    }

    /// Pushes a value and returns its placeholder, on PostgreSQL cast to the base type of `cast`
    /// (`pg_cast_type`): a value too long for the column fails instead of being cut by the cast.
    fn placeholder(&self, params: &mut Vec<Value>, value: Value, cast: Option<&str>) -> String {
        params.push(value);
        let ph = self.dialect.placeholder(params.len());
        match (self.dialect, cast) {
            (Dialect::Postgres, Some(ty)) if !ty.is_empty() => {
                format!("{ph}::{}", pg_cast_type(ty))
            }
            _ => ph,
        }
    }
}

/// Parts of a row-addressing WHERE: the condition and a trailing clause (MySQL `LIMIT 1`).
pub struct Where {
    pub condition: String,
    pub suffix: String,
}

/// Builds the WHERE for a `RowKey`, appending bind params.
pub fn row_key_where(
    spec: &TableSpec<'_>,
    key: &RowKey,
    params: &mut Vec<Value>,
) -> Result<Where, AppError> {
    let d = spec.dialect;
    match key {
        RowKey::Rowid(id) => {
            let ph = spec.placeholder(params, Value::from_i64(*id), None);
            Ok(Where {
                condition: format!("rowid = {ph}"),
                suffix: String::new(),
            })
        }
        RowKey::Pk(map) => {
            if map.is_empty() {
                return Err(AppError::InvalidInput("row key is empty".into()));
            }
            let mut parts = Vec::new();
            for (name, v) in map {
                let col = spec.column(name)?;
                let ident = d.quote_ident(name);
                if v.is_null() {
                    parts.push(format!("{ident} IS NULL"));
                } else {
                    let ph = spec.placeholder(params, v.clone(), Some(col.cast_type()));
                    parts.push(format!("{ident} = {ph}"));
                }
            }
            Ok(Where {
                condition: parts.join(" AND "),
                suffix: String::new(),
            })
        }
        RowKey::AllColumns(map) => {
            if map.is_empty() {
                return Err(AppError::InvalidInput("row key is empty".into()));
            }
            let mut parts = Vec::new();
            for (name, v) in map {
                spec.column(name)?;
                let ident = d.quote_ident(name);
                parts.push(match d {
                    // Compare textual forms: works for every type, including json and arrays.
                    Dialect::Postgres => {
                        let ph = spec.placeholder(
                            params,
                            v.as_text().map(Value::Text).unwrap_or(Value::Null),
                            Some("text"),
                        );
                        format!("{ident}::text IS NOT DISTINCT FROM {ph}")
                    }
                    Dialect::Mysql => {
                        let ph = spec.placeholder(params, v.clone(), None);
                        format!("{ident} <=> {ph}")
                    }
                    Dialect::Sqlite => {
                        let ph = spec.placeholder(params, v.clone(), None);
                        format!("{ident} IS {ph}")
                    }
                });
            }
            let all = parts.join(" AND ");
            Ok(match d {
                Dialect::Postgres => Where {
                    condition: format!(
                        "ctid = (SELECT ctid FROM {} WHERE {all} LIMIT 1)",
                        spec.target()
                    ),
                    suffix: String::new(),
                },
                Dialect::Mysql => Where {
                    condition: all,
                    suffix: " LIMIT 1".into(),
                },
                Dialect::Sqlite => Where {
                    condition: format!(
                        "rowid = (SELECT rowid FROM {} WHERE {all} LIMIT 1)",
                        spec.target()
                    ),
                    suffix: String::new(),
                },
            })
        }
    }
}

fn returning_clause(spec: &TableSpec<'_>) -> String {
    match spec.dialect {
        Dialect::Postgres => {
            let cols: Vec<String> = spec
                .columns
                .iter()
                .map(|c| {
                    let i = spec.dialect.quote_ident(&c.name);
                    format!("{i}::text AS {i}")
                })
                .collect();
            format!(" RETURNING {}", cols.join(", "))
        }
        Dialect::Sqlite => " RETURNING *".into(),
        Dialect::Mysql => String::new(),
    }
}

pub fn build_update(
    spec: &TableSpec<'_>,
    key: &RowKey,
    changes: &BTreeMap<String, Value>,
    returning: bool,
) -> Result<BuiltSql, AppError> {
    build_update_cells(spec, key, &cells(changes, None), returning)
}

/// `build_update` of cells that may hold server expressions.
pub fn build_update_cells(
    spec: &TableSpec<'_>,
    key: &RowKey,
    changes: &BTreeMap<&str, Cell<'_>>,
    returning: bool,
) -> Result<BuiltSql, AppError> {
    if changes.is_empty() {
        return Err(AppError::InvalidInput(
            "update has no changed columns".into(),
        ));
    }
    let d = spec.dialect;
    let mut params = Vec::new();
    let mut sets = Vec::new();
    for (name, cell) in changes {
        let col = spec.column(name)?;
        let ident = d.quote_ident(name);
        if cell.is_default() {
            if d == Dialect::Sqlite {
                return Err(AppError::Unsupported(
                    "SQLite cannot SET a column to DEFAULT; enter the value explicitly".into(),
                ));
            }
            sets.push(format!("{ident} = DEFAULT"));
            continue;
        }
        match cell {
            Cell::Expr(e) => sets.push(format!("{ident} = {}", server_expr_sql(d, *e, col.kind)?)),
            Cell::Value(v) => {
                let ph = spec.placeholder(&mut params, (*v).clone(), Some(col.cast_type()));
                sets.push(format!("{ident} = {ph}"));
            }
        }
    }
    let w = row_key_where(spec, key, &mut params)?;
    let mut sql = format!(
        "UPDATE {} SET {} WHERE {}{}",
        spec.target(),
        sets.join(", "),
        w.condition,
        w.suffix
    );
    if returning {
        sql.push_str(&returning_clause(spec));
    }
    Ok(BuiltSql { sql, params })
}

pub fn build_insert(
    spec: &TableSpec<'_>,
    values: &BTreeMap<String, Value>,
    returning: bool,
) -> Result<BuiltSql, AppError> {
    build_insert_cells(spec, &cells(values, None), returning)
}

/// `build_insert` of cells that may hold server expressions.
pub fn build_insert_cells(
    spec: &TableSpec<'_>,
    values: &BTreeMap<&str, Cell<'_>>,
    returning: bool,
) -> Result<BuiltSql, AppError> {
    let d = spec.dialect;
    let mut params = Vec::new();
    let mut names = Vec::new();
    let mut exprs = Vec::new();
    for (name, cell) in values {
        let col = spec.column(name)?;
        if cell.is_default() {
            continue; // omitted columns take their default on every engine
        }
        names.push(d.quote_ident(name));
        exprs.push(match cell {
            Cell::Expr(e) => server_expr_sql(d, *e, col.kind)?,
            Cell::Value(v) => spec.placeholder(&mut params, (*v).clone(), Some(col.cast_type())),
        });
    }
    let mut sql = if names.is_empty() {
        match d {
            Dialect::Mysql => format!("INSERT INTO {} () VALUES ()", spec.target()),
            _ => format!("INSERT INTO {} DEFAULT VALUES", spec.target()),
        }
    } else {
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            spec.target(),
            names.join(", "),
            exprs.join(", ")
        )
    };
    if returning {
        sql.push_str(&returning_clause(spec));
    }
    Ok(BuiltSql { sql, params })
}

pub fn build_delete(spec: &TableSpec<'_>, key: &RowKey) -> Result<BuiltSql, AppError> {
    let mut params = Vec::new();
    let w = row_key_where(spec, key, &mut params)?;
    Ok(BuiltSql {
        sql: format!(
            "DELETE FROM {} WHERE {}{}",
            spec.target(),
            w.condition,
            w.suffix
        ),
        params,
    })
}

/// Builds the statement for one staged change and how many rows it must affect.
pub fn build_change(
    spec: &TableSpec<'_>,
    change: &StagedChange,
    returning: bool,
) -> Result<(BuiltSql, Option<u64>), AppError> {
    Ok(match change {
        StagedChange::Update {
            key,
            changes,
            exprs,
            ..
        } => (
            build_update_cells(spec, key, &cells(changes, exprs.as_ref()), returning)?,
            Some(1),
        ),
        StagedChange::Insert { values, exprs, .. } => (
            build_insert_cells(spec, &cells(values, exprs.as_ref()), returning)?,
            Some(1),
        ),
        StagedChange::Delete { key, .. } => (build_delete(spec, key)?, Some(1)),
    })
}

/// Execution order: deletes first (frees unique keys), then updates, then inserts.
pub fn ordered(changes: &[StagedChange]) -> Vec<&StagedChange> {
    let mut out: Vec<&StagedChange> = Vec::with_capacity(changes.len());
    out.extend(
        changes
            .iter()
            .filter(|c| matches!(c, StagedChange::Delete { .. })),
    );
    out.extend(
        changes
            .iter()
            .filter(|c| matches!(c, StagedChange::Update { .. })),
    );
    out.extend(
        changes
            .iter()
            .filter(|c| matches!(c, StagedChange::Insert { .. })),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlgen::select::inline_params;
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
            is_auto_increment: pk,
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

    fn cols() -> Vec<ColumnMeta> {
        vec![
            col("id", "bigint", ValueKind::BigInt, true),
            col("name", "text", ValueKind::Text, false),
            col("age", "integer", ValueKind::Int, false),
        ]
    }

    fn pk(id: i64) -> RowKey {
        RowKey::Pk(BTreeMap::from([("id".to_string(), Value::Int(id))]))
    }

    #[test]
    fn update_by_pk_with_returning() {
        let c = cols();
        let t = TableRef::new(Some("public"), "users");
        let spec = TableSpec {
            dialect: Dialect::Postgres,
            table: &t,
            columns: &c,
        };
        let changes = BTreeMap::from([
            ("name".to_string(), Value::Text("Bob".into())),
            ("age".to_string(), Value::Default),
        ]);
        let b = build_update(&spec, &pk(7), &changes, true).unwrap();
        assert_eq!(
            b.sql,
            "UPDATE \"public\".\"users\" SET \"age\" = DEFAULT, \"name\" = $1::text WHERE \"id\" = $2::bigint RETURNING \"id\"::text AS \"id\", \"name\"::text AS \"name\", \"age\"::text AS \"age\""
        );
        assert_eq!(
            inline_params(Dialect::Postgres, &b.sql, &b.params)
                .split(" WHERE ")
                .nth(1)
                .unwrap(),
            "\"id\" = 7::bigint RETURNING \"id\"::text AS \"id\", \"name\"::text AS \"name\", \"age\"::text AS \"age\""
        );

        let spec = TableSpec {
            dialect: Dialect::Mysql,
            table: &t,
            columns: &c,
        };
        let b = build_update(
            &spec,
            &pk(7),
            &BTreeMap::from([("name".to_string(), Value::Null)]),
            true,
        )
        .unwrap();
        assert_eq!(
            b.sql,
            "UPDATE `public`.`users` SET `name` = ? WHERE `id` = ?"
        );
        assert_eq!(b.params, vec![Value::Null, Value::Int(7)]);

        let spec = TableSpec {
            dialect: Dialect::Sqlite,
            table: &t,
            columns: &c,
        };
        assert!(
            build_update(
                &spec,
                &pk(1),
                &BTreeMap::from([("age".to_string(), Value::Default)]),
                false
            )
            .is_err()
        );
        let b = build_update(
            &spec,
            &RowKey::Rowid(42),
            &BTreeMap::from([("age".to_string(), Value::Int(3))]),
            true,
        )
        .unwrap();
        assert_eq!(
            b.sql,
            "UPDATE \"public\".\"users\" SET \"age\" = ?1 WHERE rowid = ?2 RETURNING *"
        );
    }

    #[test]
    fn all_columns_fallback_per_dialect() {
        let c = cols();
        let t = TableRef::new(None, "t");
        let key = RowKey::AllColumns(BTreeMap::from([
            ("age".to_string(), Value::Null),
            ("name".to_string(), Value::Text("x".into())),
        ]));
        let spec = TableSpec {
            dialect: Dialect::Postgres,
            table: &t,
            columns: &c,
        };
        let b = build_delete(&spec, &key).unwrap();
        assert_eq!(
            b.sql,
            "DELETE FROM \"t\" WHERE ctid = (SELECT ctid FROM \"t\" WHERE \"age\"::text IS NOT DISTINCT FROM $1::text AND \"name\"::text IS NOT DISTINCT FROM $2::text LIMIT 1)"
        );
        assert_eq!(b.params, vec![Value::Null, Value::Text("x".into())]);
        let spec = TableSpec {
            dialect: Dialect::Mysql,
            table: &t,
            columns: &c,
        };
        assert_eq!(
            build_delete(&spec, &key).unwrap().sql,
            "DELETE FROM `t` WHERE `age` <=> ? AND `name` <=> ? LIMIT 1"
        );
        let spec = TableSpec {
            dialect: Dialect::Sqlite,
            table: &t,
            columns: &c,
        };
        assert_eq!(
            build_delete(&spec, &key).unwrap().sql,
            "DELETE FROM \"t\" WHERE rowid = (SELECT rowid FROM \"t\" WHERE \"age\" IS ?1 AND \"name\" IS ?2 LIMIT 1)"
        );
    }

    #[test]
    fn insert_omits_defaults() {
        let c = cols();
        let t = TableRef::new(Some("main"), "users");
        let spec = TableSpec {
            dialect: Dialect::Sqlite,
            table: &t,
            columns: &c,
        };
        let values = BTreeMap::from([
            ("id".to_string(), Value::Default),
            ("name".to_string(), Value::Text("n".into())),
        ]);
        assert_eq!(
            build_insert(&spec, &values, true).unwrap().sql,
            "INSERT INTO \"main\".\"users\" (\"name\") VALUES (?1) RETURNING *"
        );
        let all_default = BTreeMap::from([("id".to_string(), Value::Default)]);
        assert_eq!(
            build_insert(&spec, &all_default, false).unwrap().sql,
            "INSERT INTO \"main\".\"users\" DEFAULT VALUES"
        );
        let spec = TableSpec {
            dialect: Dialect::Mysql,
            table: &t,
            columns: &c,
        };
        assert_eq!(
            build_insert(&spec, &all_default, false).unwrap().sql,
            "INSERT INTO `main`.`users` () VALUES ()"
        );
    }

    #[test]
    fn postgres_values_are_cast_to_base_types() {
        let c = vec![
            col("code", "character(4)", ValueKind::Text, true),
            col("name", "character varying(5)", ValueKind::Text, false),
            col("amount", "numeric(10,2)", ValueKind::Decimal, false),
        ];
        let t = TableRef::new(None, "t");
        let spec = TableSpec {
            dialect: Dialect::Postgres,
            table: &t,
            columns: &c,
        };
        let key = RowKey::Pk(BTreeMap::from([(
            "code".to_string(),
            Value::Text("ab".into()),
        )]));
        let b = build_update(
            &spec,
            &key,
            &BTreeMap::from([("name".to_string(), Value::Text("abcdefgh".into()))]),
            false,
        )
        .unwrap();
        assert_eq!(
            b.sql,
            "UPDATE \"t\" SET \"name\" = $1::character varying WHERE \"code\" = $2::bpchar"
        );
        let values = BTreeMap::from([("amount".to_string(), Value::Text("1.239".into()))]);
        assert_eq!(
            build_insert(&spec, &values, false).unwrap().sql,
            "INSERT INTO \"t\" (\"amount\") VALUES ($1::numeric)"
        );
    }

    #[test]
    fn orders_deletes_updates_inserts() {
        let changes = vec![
            StagedChange::Insert {
                row_id: "i".into(),
                values: BTreeMap::new(),
                exprs: None,
            },
            StagedChange::Update {
                row_id: "u".into(),
                key: pk(1),
                changes: BTreeMap::new(),
                exprs: None,
            },
            StagedChange::Delete {
                row_id: "d".into(),
                key: pk(2),
            },
        ];
        let ids: Vec<&str> = ordered(&changes).iter().map(|c| c.row_id()).collect();
        assert_eq!(ids, ["d", "u", "i"]);
    }

    fn temporal_cols() -> Vec<ColumnMeta> {
        vec![
            col("id", "bigint", ValueKind::BigInt, true),
            col("born", "date", ValueKind::Date, false),
            col("seen", "timestamp", ValueKind::Timestamp, false),
            col("note", "text", ValueKind::Text, false),
        ]
    }

    #[test]
    fn server_expressions_render_unquoted_per_dialect() {
        let c = temporal_cols();
        let t = TableRef::new(None, "t");
        let change = |exprs: &[(&str, ServerExpr)]| StagedChange::Update {
            row_id: "r".into(),
            key: pk(1),
            changes: BTreeMap::from([
                ("note".to_string(), Value::Text("x".into())),
                // an expression wins over a value staged for the same column
                ("seen".to_string(), Value::Text("ignored".into())),
            ]),
            exprs: Some(
                exprs
                    .iter()
                    .map(|(k, e)| (k.to_string(), *e))
                    .collect::<BTreeMap<_, _>>(),
            ),
        };
        let pg = TableSpec {
            dialect: Dialect::Postgres,
            table: &t,
            columns: &c,
        };
        let (b, expect) = build_change(
            &pg,
            &change(&[("seen", ServerExpr::Now), ("born", ServerExpr::CurrentDate)]),
            false,
        )
        .unwrap();
        assert_eq!(expect, Some(1));
        assert_eq!(
            b.sql,
            "UPDATE \"t\" SET \"born\" = CURRENT_DATE, \"note\" = $1::text, \"seen\" = NOW() WHERE \"id\" = $2::bigint"
        );
        assert_eq!(b.params, vec![Value::Text("x".into()), Value::Int(1)]);

        let my = TableSpec {
            dialect: Dialect::Mysql,
            table: &t,
            columns: &c,
        };
        let (b, _) =
            build_change(&my, &change(&[("seen", ServerExpr::LocalTimestamp)]), false).unwrap();
        assert_eq!(
            b.sql,
            "UPDATE `t` SET `note` = ?, `seen` = LOCALTIMESTAMP WHERE `id` = ?"
        );

        // SQLite has no NOW(): the CURRENT_* keyword of the column's kind.
        let lite = TableSpec {
            dialect: Dialect::Sqlite,
            table: &t,
            columns: &c,
        };
        let (b, _) = build_change(
            &lite,
            &change(&[("seen", ServerExpr::Now), ("born", ServerExpr::Now)]),
            false,
        )
        .unwrap();
        assert_eq!(
            b.sql,
            "UPDATE \"t\" SET \"born\" = CURRENT_DATE, \"note\" = ?1, \"seen\" = CURRENT_TIMESTAMP WHERE \"id\" = ?2"
        );
        // DEFAULT as an expression: refused in a SQLite UPDATE like the DEFAULT value.
        assert!(build_change(&lite, &change(&[("seen", ServerExpr::Default)]), false).is_err());
        let (b, _) = build_change(&pg, &change(&[("seen", ServerExpr::Default)]), false).unwrap();
        assert!(b.sql.contains("\"seen\" = DEFAULT"), "{}", b.sql);
    }

    #[test]
    fn insert_takes_expressions_and_omits_default_ones() {
        let c = temporal_cols();
        let t = TableRef::new(None, "t");
        let change = StagedChange::Insert {
            row_id: "n".into(),
            values: BTreeMap::from([
                ("id".to_string(), Value::Default),
                ("note".to_string(), Value::Text("hi".into())),
            ]),
            exprs: Some(BTreeMap::from([
                ("seen".to_string(), ServerExpr::CurrentTimestamp),
                ("born".to_string(), ServerExpr::Default),
            ])),
        };
        let spec = TableSpec {
            dialect: Dialect::Postgres,
            table: &t,
            columns: &c,
        };
        let (b, _) = build_change(&spec, &change, false).unwrap();
        assert_eq!(
            b.sql,
            "INSERT INTO \"t\" (\"note\", \"seen\") VALUES ($1::text, CURRENT_TIMESTAMP)"
        );
        assert_eq!(b.params, vec![Value::Text("hi".into())]);
        assert_eq!(
            server_expr_sql(
                Dialect::Sqlite,
                ServerExpr::LocalTimestamp,
                ValueKind::Timestamp
            )
            .unwrap(),
            "datetime('now', 'localtime')"
        );
        assert_eq!(
            server_expr_sql(Dialect::Sqlite, ServerExpr::Now, ValueKind::Time).unwrap(),
            "CURRENT_TIME"
        );
    }

    fn sql_of(d: Dialect, e: ServerExpr) -> String {
        server_expr_sql(d, e, ValueKind::Timestamp).unwrap()
    }

    /// The finding: `CURRENT_TIMESTAMP(6)` typed in a DATETIME(6) cell must reach MySQL with its
    /// precision (a bare keyword stores 0 fractional digits).
    #[test]
    fn a_clock_keeps_its_fractional_seconds_precision() {
        use crate::types::mutate::Clock::*;
        let fsp = |c, n| ServerExpr::with_fsp(c, n).unwrap();
        // MySQL / MariaDB: the argument on every clock.
        for (c, expected) in [
            (Now, "NOW(6)"),
            (CurrentTimestamp, "CURRENT_TIMESTAMP(6)"),
            (CurrentTime, "CURRENT_TIME(6)"),
            (LocalTimestamp, "LOCALTIMESTAMP(6)"),
        ] {
            assert_eq!(sql_of(Dialect::Mysql, fsp(c, 6)), expected);
        }
        // PostgreSQL: now() has no argument, CURRENT_TIMESTAMP(n) is the same instant.
        assert_eq!(
            sql_of(Dialect::Postgres, fsp(Now, 3)),
            "CURRENT_TIMESTAMP(3)"
        );
        assert_eq!(
            sql_of(Dialect::Postgres, fsp(CurrentTimestamp, 0)),
            "CURRENT_TIMESTAMP(0)"
        );
        assert_eq!(
            sql_of(Dialect::Postgres, fsp(CurrentTime, 2)),
            "CURRENT_TIME(2)"
        );
        assert_eq!(
            sql_of(Dialect::Postgres, fsp(LocalTimestamp, 6)),
            "LOCALTIMESTAMP(6)"
        );
        // SQLite has no precision: the plain keyword (NOW picks by the column's kind).
        assert_eq!(
            sql_of(Dialect::Sqlite, fsp(CurrentTimestamp, 6)),
            "CURRENT_TIMESTAMP"
        );
        assert_eq!(sql_of(Dialect::Sqlite, fsp(CurrentTime, 3)), "CURRENT_TIME");
        assert_eq!(sql_of(Dialect::Sqlite, fsp(Now, 6)), "CURRENT_TIMESTAMP");
        assert_eq!(
            server_expr_sql(Dialect::Sqlite, fsp(Now, 6), ValueKind::Date).unwrap(),
            "CURRENT_DATE"
        );
        assert_eq!(
            sql_of(Dialect::Sqlite, fsp(LocalTimestamp, 6)),
            "datetime('now', 'localtime')"
        );
        // Without a precision nothing changes.
        assert_eq!(sql_of(Dialect::Mysql, ServerExpr::Now), "NOW()");
        assert_eq!(
            sql_of(Dialect::Mysql, ServerExpr::CurrentTimestamp),
            "CURRENT_TIMESTAMP"
        );
        assert_eq!(sql_of(Dialect::Postgres, ServerExpr::Now), "NOW()");
        // Above 6 digits is refused, whatever the dialect.
        assert!(ServerExpr::with_fsp(Now, 7).is_none());
        for d in [Dialect::Mysql, Dialect::Postgres, Dialect::Sqlite] {
            assert!(server_expr_sql(d, ServerExpr::Fsp(Now, 7), ValueKind::Timestamp).is_err());
        }
    }

    #[test]
    fn a_staged_precision_is_rendered_into_the_statement() {
        let c = temporal_cols();
        let t = TableRef::new(None, "t");
        let change = StagedChange::Insert {
            row_id: "n".into(),
            values: BTreeMap::new(),
            exprs: Some(BTreeMap::from([(
                "seen".to_string(),
                ServerExpr::with_fsp(Clock::CurrentTimestamp, 6).unwrap(),
            )])),
        };
        let my = TableSpec {
            dialect: Dialect::Mysql,
            table: &t,
            columns: &c,
        };
        let (b, _) = build_change(&my, &change, false).unwrap();
        assert_eq!(
            b.sql,
            "INSERT INTO `t` (`seen`) VALUES (CURRENT_TIMESTAMP(6))"
        );
        let update = StagedChange::Update {
            row_id: "u".into(),
            key: pk(1),
            changes: BTreeMap::new(),
            exprs: Some(BTreeMap::from([(
                "seen".to_string(),
                ServerExpr::with_fsp(Clock::Now, 3).unwrap(),
            )])),
        };
        let (b, _) = build_change(&my, &update, false).unwrap();
        assert_eq!(b.sql, "UPDATE `t` SET `seen` = NOW(3) WHERE `id` = ?");
        // A precision the server would refuse never reaches it.
        let bad = StagedChange::Update {
            row_id: "u".into(),
            key: pk(1),
            changes: BTreeMap::new(),
            exprs: Some(BTreeMap::from([(
                "seen".to_string(),
                ServerExpr::Fsp(Clock::Now, 9),
            )])),
        };
        assert!(build_change(&my, &bad, false).is_err());
    }

    #[test]
    fn expressions_travel_as_one_string_and_old_payloads_still_parse() {
        use crate::types::mutate::Clock::*;
        // The unit variants keep their wire names.
        for (wire, e) in [
            ("now", ServerExpr::Now),
            ("currentTimestamp", ServerExpr::CurrentTimestamp),
            ("currentDate", ServerExpr::CurrentDate),
            ("currentTime", ServerExpr::CurrentTime),
            ("localTimestamp", ServerExpr::LocalTimestamp),
            ("default", ServerExpr::Default),
            ("now(6)", ServerExpr::Fsp(Now, 6)),
            ("currentTimestamp(0)", ServerExpr::Fsp(CurrentTimestamp, 0)),
            ("currentTime(3)", ServerExpr::Fsp(CurrentTime, 3)),
            ("localTimestamp(1)", ServerExpr::Fsp(LocalTimestamp, 1)),
        ] {
            let json = format!("\"{wire}\"");
            assert_eq!(
                serde_json::from_str::<ServerExpr>(&json).unwrap(),
                e,
                "{wire}"
            );
            assert_eq!(serde_json::to_string(&e).unwrap(), json, "{wire}");
        }
        for bad in [
            "\"now(7)\"",
            "\"now(10)\"",
            "\"now()\"",
            "\"now(-1)\"",
            "\"currentDate(3)\"",
            "\"default(1)\"",
            "\"NOW\"",
            "\"now(6\"",
            "\"now(6)x\"",
            "\"sleep(1)\"",
            "\"\"",
        ] {
            assert!(serde_json::from_str::<ServerExpr>(bad).is_err(), "{bad}");
        }
        let change: StagedChange = serde_json::from_str(
            r#"{"op":"update","rowId":"r","key":{"kind":"rowid","value":3},"changes":{},"exprs":{"seen":"now(6)"}}"#,
        )
        .unwrap();
        match change {
            StagedChange::Update { exprs, .. } => assert_eq!(
                exprs,
                Some(BTreeMap::from([(
                    "seen".to_string(),
                    ServerExpr::Fsp(Now, 6)
                )]))
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn expressions_deserialize_from_the_renderer_payload() {
        let change: StagedChange = serde_json::from_str(
            r#"{"op":"update","rowId":"r","key":{"kind":"rowid","value":3},"changes":{},"exprs":{"seen":"now"}}"#,
        )
        .unwrap();
        match change {
            StagedChange::Update { exprs, .. } => assert_eq!(
                exprs,
                Some(BTreeMap::from([("seen".to_string(), ServerExpr::Now)]))
            ),
            other => panic!("{other:?}"),
        }
        // Without `exprs` (every older caller).
        let change: StagedChange =
            serde_json::from_str(r#"{"op":"insert","rowId":"n","values":{}}"#).unwrap();
        assert!(matches!(change, StagedChange::Insert { exprs: None, .. }));
    }
}
