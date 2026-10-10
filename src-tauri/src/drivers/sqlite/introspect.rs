//! SQLite catalog queries (sqlite_master + PRAGMAs).

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};

use super::decode::kind_from_decltype;
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::types::schema::{
    ColumnMeta, CompletionMetadata, ForeignKeyMeta, IndexMeta, SchemaInfo, TableKind, TableMeta,
    TableRef, TableSummary, TriggerMeta, ValueKind,
};

fn schema_of(table: &TableRef) -> String {
    table
        .schema
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main".into())
}

fn q(s: &str) -> String {
    Dialect::Sqlite.quote_ident(s)
}

pub fn list_schemas(conn: &mut Connection) -> Result<Vec<SchemaInfo>, AppError> {
    let mut stmt = conn.prepare("PRAGMA database_list")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    let mut out = Vec::new();
    for name in rows {
        let name = name?;
        out.push(SchemaInfo {
            is_default: name == "main",
            name,
        });
    }
    Ok(out)
}

pub fn list_tables(conn: &mut Connection, schema: &str) -> Result<Vec<TableSummary>, AppError> {
    let sql = format!(
        "SELECT name, type FROM {}.sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name",
        q(schema)
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (name, ty) = row?;
        out.push(TableSummary {
            name,
            schema: Some(schema.to_string()),
            kind: if ty == "view" {
                TableKind::View
            } else {
                TableKind::Table
            },
            row_estimate: None,
            size_bytes: None,
            comment: None,
        });
    }
    Ok(out)
}

pub fn view_definition(
    conn: &mut Connection,
    table: &TableRef,
) -> Result<Option<String>, AppError> {
    let sql = format!(
        "SELECT sql FROM {}.sqlite_master WHERE type = 'view' AND name = ?1",
        q(&schema_of(table))
    );
    Ok(conn
        .query_row(&sql, params![table.name], |r| r.get::<_, Option<String>>(0))
        .optional()?
        .flatten())
}

pub fn table_meta(conn: &mut Connection, table: &TableRef) -> Result<TableMeta, AppError> {
    let schema = schema_of(table);
    let (kind, ddl): (TableKind, Option<String>) = conn
        .query_row(
            &format!(
                "SELECT type, sql FROM {}.sqlite_master WHERE type IN ('table','view') AND name = ?1",
                q(&schema)
            ),
            params![table.name],
            |r| {
                let ty: String = r.get(0)?;
                Ok((
                    if ty == "view" { TableKind::View } else { TableKind::Table },
                    r.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("table {}", table.name)))?;

    let autoinc = ddl
        .as_deref()
        .map(|s| s.to_ascii_uppercase().contains("AUTOINCREMENT"))
        .unwrap_or(false);

    // Columns
    let mut columns = Vec::new();
    {
        let mut stmt = conn.prepare(&format!(
            "PRAGMA {}.table_xinfo({})",
            q(&schema),
            q(&table.name)
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?;
        for row in rows {
            let (cid, name, decl, notnull, dflt, pk, hidden) = row?;
            let decl_str = decl.clone().unwrap_or_default();
            let kind = kind_from_decltype(decl.as_deref());
            let kind = if decl_str.is_empty() {
                ValueKind::Other
            } else {
                kind
            };
            columns.push(ColumnMeta {
                name,
                ordinal: cid as u32 + 1,
                data_type: decl_str,
                kind,
                nullable: notnull == 0 && pk == 0,
                default_expr: dflt,
                is_pk: pk > 0,
                pk_ordinal: if pk > 0 { Some(pk as u32) } else { None },
                is_auto_increment: false,
                is_generated: hidden == 2 || hidden == 3,
                is_unsigned: false,
                comment: None,
                enum_values: None,
                char_max_len: None,
                numeric_precision: None,
                numeric_scale: None,
                collation: None,
                array_elem_type: None,
                extra: match hidden {
                    2 => Some("VIRTUAL".into()),
                    3 => Some("STORED".into()),
                    _ => None,
                },
            });
        }
    }
    let mut primary_key: Vec<(u32, String)> = columns
        .iter()
        .filter_map(|c| c.pk_ordinal.map(|o| (o, c.name.clone())))
        .collect();
    primary_key.sort();
    let primary_key: Vec<String> = primary_key.into_iter().map(|(_, n)| n).collect();
    // `INTEGER PRIMARY KEY` is a rowid alias and auto-assigns ids.
    if primary_key.len() == 1
        && let Some(col) = columns.iter_mut().find(|c| c.is_pk)
        && col.data_type.eq_ignore_ascii_case("INTEGER")
    {
        col.is_auto_increment = true;
        col.extra = Some(if autoinc {
            "AUTOINCREMENT".into()
        } else {
            "rowid alias".into()
        });
    }

    // Indexes
    let mut indexes = Vec::new();
    if kind == TableKind::Table {
        let list: Vec<(String, bool, String, bool)> = {
            let mut stmt = conn.prepare(&format!(
                "PRAGMA {}.index_list({})",
                q(&schema),
                q(&table.name)
            ))?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)? != 0,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)? != 0,
                ))
            })?;
            rows.collect::<Result<_, _>>()?
        };
        for (name, unique, origin, partial) in list {
            let mut cols = Vec::new();
            {
                let mut stmt =
                    conn.prepare(&format!("PRAGMA {}.index_xinfo({})", q(&schema), q(&name)))?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, i64>(3)?,
                    ))
                })?;
                for row in rows {
                    let (col, key, desc) = row?;
                    if key == 0 {
                        continue;
                    }
                    let mut c = col.unwrap_or_else(|| "<expr>".into());
                    if desc != 0 {
                        c.push_str(" DESC");
                    }
                    cols.push(c);
                }
            }
            let definition: Option<String> = conn
                .query_row(
                    &format!(
                        "SELECT sql FROM {}.sqlite_master WHERE type = 'index' AND name = ?1",
                        q(&schema)
                    ),
                    params![name],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
            indexes.push(IndexMeta {
                name,
                columns: cols,
                unique,
                primary: origin == "pk",
                method: None,
                predicate: if partial {
                    Some("partial".into())
                } else {
                    None
                },
                definition,
            });
        }
    }

    // Foreign keys (grouped by id)
    let mut foreign_keys: Vec<ForeignKeyMeta> = Vec::new();
    if kind == TableKind::Table {
        let mut stmt = conn.prepare(&format!(
            "PRAGMA {}.foreign_key_list({})",
            q(&schema),
            q(&table.name)
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut groups: BTreeMap<i64, ForeignKeyMeta> = BTreeMap::new();
        for row in rows {
            let (id, _seq, ref_table, from, to, on_update, on_delete) = row?;
            let entry = groups.entry(id).or_insert_with(|| ForeignKeyMeta {
                name: format!("fk_{}_{}", table.name, id),
                columns: Vec::new(),
                ref_table: TableRef::new(Some(&schema), &ref_table),
                ref_columns: Vec::new(),
                on_update,
                on_delete,
                definition: None,
            });
            entry.columns.push(from);
            entry.ref_columns.push(to.unwrap_or_default());
        }
        foreign_keys.extend(groups.into_values());
    }

    // Triggers
    let mut triggers = Vec::new();
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT name, sql FROM {}.sqlite_master WHERE type = 'trigger' AND tbl_name = ?1 ORDER BY name",
            q(&schema)
        ))?;
        let rows = stmt.query_map(params![table.name], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (name, sql) = row?;
            let def = sql.unwrap_or_default();
            let upper = def.to_ascii_uppercase();
            let timing = ["BEFORE", "AFTER", "INSTEAD OF"]
                .iter()
                .find(|t| upper.contains(&format!(" {t} ")))
                .map(|t| t.to_string());
            let events = ["INSERT", "UPDATE", "DELETE"]
                .iter()
                .filter(|e| upper.contains(&format!(" {e} ")) || upper.contains(&format!(" {e}\n")))
                .map(|e| e.to_string())
                .collect();
            triggers.push(TriggerMeta {
                name,
                timing,
                events,
                enabled: true,
                definition: def,
            });
        }
    }

    Ok(TableMeta {
        table: TableRef::new(Some(&schema), &table.name),
        kind,
        columns,
        primary_key,
        indexes,
        foreign_keys,
        triggers,
        comment: None,
        row_estimate: None,
        ddl,
    })
}

pub fn completion_metadata(conn: &mut Connection) -> Result<CompletionMetadata, AppError> {
    let mut meta = CompletionMetadata {
        default_schema: Some("main".into()),
        schemas: BTreeMap::new(),
    };
    for schema in list_schemas(conn)? {
        let mut tables = BTreeMap::new();
        for t in list_tables(conn, &schema.name)? {
            let mut stmt = conn.prepare(&format!(
                "PRAGMA {}.table_info({})",
                q(&schema.name),
                q(&t.name)
            ))?;
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<Result<_, _>>()?;
            tables.insert(t.name, cols);
        }
        meta.schemas.insert(schema.name, tables);
    }
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn introspects_a_small_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, email TEXT NOT NULL UNIQUE, name TEXT, score REAL DEFAULT 1.5);
             CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE, title TEXT);
             CREATE INDEX posts_user ON posts(user_id);
             CREATE TABLE pt (post_id INTEGER, tag_id INTEGER, PRIMARY KEY (post_id, tag_id)) WITHOUT ROWID;
             CREATE VIEW v AS SELECT id FROM users;
             CREATE TRIGGER trg AFTER UPDATE ON users BEGIN UPDATE users SET name = name WHERE id = NEW.id; END;",
        )
        .unwrap();
        let tables = list_tables(&mut conn, "main").unwrap();
        assert_eq!(
            tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["posts", "pt", "users", "v"]
        );
        assert_eq!(tables[3].kind, TableKind::View);

        let users = table_meta(&mut conn, &TableRef::new(None, "users")).unwrap();
        assert_eq!(users.primary_key, ["id"]);
        assert!(users.columns[0].is_auto_increment);
        assert_eq!(users.columns[0].extra.as_deref(), Some("AUTOINCREMENT"));
        assert!(!users.columns[1].nullable);
        assert_eq!(users.columns[3].default_expr.as_deref(), Some("1.5"));
        assert_eq!(users.columns[3].kind, ValueKind::Float);
        assert_eq!(users.triggers.len(), 1);
        assert_eq!(users.triggers[0].timing.as_deref(), Some("AFTER"));
        assert!(
            users
                .indexes
                .iter()
                .any(|i| i.unique && i.columns == ["email"])
        );
        assert!(users.ddl.unwrap().starts_with("CREATE TABLE users"));

        let posts = table_meta(&mut conn, &TableRef::new(Some("main"), "posts")).unwrap();
        assert_eq!(posts.foreign_keys.len(), 1);
        assert_eq!(posts.foreign_keys[0].ref_table.name, "users");
        assert_eq!(posts.foreign_keys[0].on_delete, "CASCADE");
        assert!(
            posts
                .indexes
                .iter()
                .any(|i| i.name == "posts_user" && i.columns == ["user_id"])
        );

        let pt = table_meta(&mut conn, &TableRef::new(None, "pt")).unwrap();
        assert_eq!(pt.primary_key, ["post_id", "tag_id"]);
        assert!(!pt.columns[0].is_auto_increment);

        let meta = completion_metadata(&mut conn).unwrap();
        assert_eq!(
            meta.schemas["main"]["users"],
            ["id", "email", "name", "score"]
        );
    }
}
