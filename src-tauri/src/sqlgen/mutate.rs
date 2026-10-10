//! UPDATE / INSERT / DELETE generation for staged grid edits. Rows are addressed by primary key,
//! SQLite rowid, or — as a last resort — every original column value (limited to one row).

use std::collections::BTreeMap;

use crate::error::AppError;
use crate::sqlgen::quote::Dialect;
use crate::sqlgen::select::BuiltSql;
use crate::types::browse::RowKey;
use crate::types::mutate::StagedChange;
use crate::types::schema::{ColumnMeta, TableRef};
use crate::types::value::Value;

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

    fn placeholder(&self, params: &mut Vec<Value>, value: Value, cast: Option<&str>) -> String {
        params.push(value);
        let ph = self.dialect.placeholder(params.len());
        match (self.dialect, cast) {
            (Dialect::Postgres, Some(ty)) if !ty.is_empty() => format!("{ph}::{ty}"),
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
                    let ph = spec.placeholder(params, v.clone(), Some(&col.data_type));
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
    if changes.is_empty() {
        return Err(AppError::InvalidInput(
            "update has no changed columns".into(),
        ));
    }
    let d = spec.dialect;
    let mut params = Vec::new();
    let mut sets = Vec::new();
    for (name, v) in changes {
        let col = spec.column(name)?;
        let ident = d.quote_ident(name);
        if matches!(v, Value::Default) {
            if d == Dialect::Sqlite {
                return Err(AppError::Unsupported(
                    "SQLite cannot SET a column to DEFAULT; enter the value explicitly".into(),
                ));
            }
            sets.push(format!("{ident} = DEFAULT"));
        } else {
            let ph = spec.placeholder(&mut params, v.clone(), Some(&col.data_type));
            sets.push(format!("{ident} = {ph}"));
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
    let d = spec.dialect;
    let mut params = Vec::new();
    let mut names = Vec::new();
    let mut exprs = Vec::new();
    for (name, v) in values {
        let col = spec.column(name)?;
        if matches!(v, Value::Default) {
            continue; // omitted columns take their default on every engine
        }
        names.push(d.quote_ident(name));
        exprs.push(spec.placeholder(&mut params, v.clone(), Some(&col.data_type)));
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
        StagedChange::Update { key, changes, .. } => {
            (build_update(spec, key, changes, returning)?, Some(1))
        }
        StagedChange::Insert { values, .. } => (build_insert(spec, values, returning)?, Some(1)),
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
    fn orders_deletes_updates_inserts() {
        let changes = vec![
            StagedChange::Insert {
                row_id: "i".into(),
                values: BTreeMap::new(),
            },
            StagedChange::Update {
                row_id: "u".into(),
                key: pk(1),
                changes: BTreeMap::new(),
            },
            StagedChange::Delete {
                row_id: "d".into(),
                key: pk(2),
            },
        ];
        let ids: Vec<&str> = ordered(&changes).iter().map(|c| c.row_id()).collect();
        assert_eq!(ids, ["d", "u", "i"]);
    }
}
