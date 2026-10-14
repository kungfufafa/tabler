//! SQLite catalog queries (sqlite_master + PRAGMAs).

use std::collections::BTreeMap;

use rusqlite::{Connection, Row};

use super::decode::kind_from_decltype;
use crate::drivers::{SqlTrace, TraceCounts, trace_text};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::ddl::{index_predicate, sqlite_checks, trigger_tail};
use crate::types::browse::TraceCategory;
use crate::types::schema::{
    ColumnMeta, CompletionMetadata, ForeignKeyMeta, IndexMeta, SchemaInfo, TableKind, TableMeta,
    TableRef, TableSummary, TriggerMeta, ValueKind,
};
use crate::types::value::Value;

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

/// Runs a catalog statement (`?1`… bound to `args`), maps every row and records it in `trace` as
/// a structure query: the console log shows the text the app really ran.
fn rows<T>(
    conn: &Connection,
    trace: &SqlTrace,
    sql: &str,
    args: &[&str],
    map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>, AppError> {
    let shown: Vec<Value> = args.iter().map(|a| Value::Text((*a).to_string())).collect();
    trace.run(
        TraceCategory::Structure,
        trace_text(Dialect::Sqlite, sql, &shown),
        |found: &Vec<T>| TraceCounts::rows(found.len()),
        || {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), map)?;
            Ok(rows.collect::<Result<Vec<T>, _>>()?)
        },
    )
}

/// `rows` for a statement that yields at most one row.
fn row_opt<T>(
    conn: &Connection,
    trace: &SqlTrace,
    sql: &str,
    args: &[&str],
    map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Option<T>, AppError> {
    Ok(rows(conn, trace, sql, args, map)?.into_iter().next())
}

pub fn list_schemas(conn: &mut Connection, trace: &SqlTrace) -> Result<Vec<SchemaInfo>, AppError> {
    let names = rows(conn, trace, "PRAGMA database_list", &[], |r| {
        r.get::<_, String>(1)
    })?;
    Ok(names
        .into_iter()
        .map(|name| SchemaInfo {
            is_default: name == "main",
            name,
        })
        .collect())
}

pub fn list_tables(
    conn: &mut Connection,
    trace: &SqlTrace,
    schema: &str,
) -> Result<Vec<TableSummary>, AppError> {
    let sql = format!(
        "SELECT name, type FROM {}.sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name",
        q(schema)
    );
    let found = rows(conn, trace, &sql, &[], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for (name, ty) in found {
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
    trace: &SqlTrace,
    table: &TableRef,
) -> Result<Option<String>, AppError> {
    let sql = format!(
        "SELECT sql FROM {}.sqlite_master WHERE type = 'view' AND name = ?1",
        q(&schema_of(table))
    );
    Ok(row_opt(conn, trace, &sql, &[&table.name], |r| {
        r.get::<_, Option<String>>(0)
    })?
    .flatten())
}

pub fn table_meta(
    conn: &mut Connection,
    trace: &SqlTrace,
    table: &TableRef,
) -> Result<TableMeta, AppError> {
    let schema = schema_of(table);
    let (kind, ddl): (TableKind, Option<String>) = row_opt(
        conn,
        trace,
        &format!(
            "SELECT type, sql FROM {}.sqlite_master WHERE type IN ('table','view') AND name = ?1",
            q(&schema)
        ),
        &[&table.name],
        |r| {
            let ty: String = r.get(0)?;
            Ok((
                if ty == "view" {
                    TableKind::View
                } else {
                    TableKind::Table
                },
                r.get::<_, Option<String>>(1)?,
            ))
        },
    )?
    .ok_or_else(|| AppError::NotFound(format!("table {}", table.name)))?;

    let autoinc = ddl
        .as_deref()
        .map(|s| s.to_ascii_uppercase().contains("AUTOINCREMENT"))
        .unwrap_or(false);

    // Columns
    let mut columns = Vec::new();
    {
        let found = rows(
            conn,
            trace,
            &format!("PRAGMA {}.table_xinfo({})", q(&schema), q(&table.name)),
            &[],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            },
        )?;
        for (cid, name, decl, notnull, dflt, pk, hidden) in found {
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
                base_type: None,
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
        let list: Vec<(String, bool, String, bool)> = rows(
            conn,
            trace,
            &format!("PRAGMA {}.index_list({})", q(&schema), q(&table.name)),
            &[],
            |r| {
                Ok((
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)? != 0,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)? != 0,
                ))
            },
        )?;
        for (name, unique, origin, partial) in list {
            let mut cols = Vec::new();
            {
                let found = rows(
                    conn,
                    trace,
                    &format!("PRAGMA {}.index_xinfo({})", q(&schema), q(&name)),
                    &[],
                    |r| {
                        Ok((
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, i64>(5)?,
                            r.get::<_, i64>(3)?,
                        ))
                    },
                )?;
                for (col, key, desc) in found {
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
            let definition: Option<String> = row_opt(
                conn,
                trace,
                &format!(
                    "SELECT sql FROM {}.sqlite_master WHERE type = 'index' AND name = ?1",
                    q(&schema)
                ),
                &[&name],
                |r| r.get::<_, Option<String>>(0),
            )?
            .flatten();
            let predicate = if partial {
                definition
                    .as_deref()
                    .and_then(index_predicate)
                    .or_else(|| Some("partial".into()))
            } else {
                None
            };
            indexes.push(IndexMeta {
                name,
                columns: cols,
                unique,
                primary: origin == "pk",
                method: None,
                predicate,
                definition,
                comment: None,
                include: Vec::new(),
            });
        }
    }

    // Foreign keys (grouped by id)
    let mut foreign_keys: Vec<ForeignKeyMeta> = Vec::new();
    if kind == TableKind::Table {
        let found = rows(
            conn,
            trace,
            &format!("PRAGMA {}.foreign_key_list({})", q(&schema), q(&table.name)),
            &[],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                ))
            },
        )?;
        let mut groups: BTreeMap<i64, ForeignKeyMeta> = BTreeMap::new();
        for (id, _seq, ref_table, from, to, on_update, on_delete) in found {
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
        let found = rows(
            conn,
            trace,
            &format!(
                "SELECT name, sql FROM {}.sqlite_master WHERE type = 'trigger' AND tbl_name = ?1 ORDER BY name",
                q(&schema)
            ),
            &[&table.name],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        )?;
        for (name, sql) in found {
            let def = sql.unwrap_or_default();
            let (timing, events) = parse_trigger_head(&def);
            triggers.push(TriggerMeta {
                name,
                timing,
                events,
                enabled: true,
                definition: def,
            });
        }
    }

    let checks = match (kind, ddl.as_deref()) {
        (TableKind::Table, Some(sql)) => {
            let names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
            sqlite_checks(sql, &table.name, &names)
        }
        _ => Vec::new(),
    };

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
        checks,
        options: None,
    })
}

/// Timing and event of a SQLite trigger from its `CREATE TRIGGER` text: the words between the
/// trigger name and `ON` (`AFTER UPDATE OF a, b ON …`). SQLite's default timing is BEFORE.
pub fn parse_trigger_head(def: &str) -> (Option<String>, Vec<String>) {
    let Some(tail) = trigger_tail(def) else {
        return (None, Vec::new());
    };
    let words: Vec<String> = tail
        .split_whitespace()
        .take_while(|w| !w.eq_ignore_ascii_case("ON"))
        .map(str::to_ascii_uppercase)
        .collect();
    let timing = if words.first().is_some_and(|w| w == "INSTEAD") {
        "INSTEAD OF"
    } else if words.first().is_some_and(|w| w == "AFTER") {
        "AFTER"
    } else {
        "BEFORE"
    };
    let events = words
        .iter()
        .find(|w| matches!(w.as_str(), "INSERT" | "UPDATE" | "DELETE"))
        .map(|w| vec![w.clone()])
        .unwrap_or_default();
    (Some(timing.to_string()), events)
}

/// Every table's column names, for autocompletion. The per-table `PRAGMA table_info` statements
/// are recorded as one trace entry (a database with hundreds of tables would otherwise flood the
/// console log): the statements, one per line, with their total time.
pub fn completion_metadata(
    conn: &mut Connection,
    trace: &SqlTrace,
) -> Result<CompletionMetadata, AppError> {
    let mut meta = CompletionMetadata {
        default_schema: Some("main".into()),
        schemas: BTreeMap::new(),
    };
    let quiet = SqlTrace::default();
    let started = std::time::Instant::now();
    let mut statements = Vec::new();
    let mut columns = 0;
    let mut failure: Option<AppError> = None;
    for schema in list_schemas(conn, trace)? {
        let mut tables = BTreeMap::new();
        for t in list_tables(conn, trace, &schema.name)? {
            let sql = format!("PRAGMA {}.table_info({})", q(&schema.name), q(&t.name));
            let cols = match rows(conn, &quiet, &sql, &[], |r| r.get::<_, String>(1)) {
                Ok(cols) => cols,
                Err(e) => {
                    failure = Some(e);
                    statements.push(sql);
                    break;
                }
            };
            columns += cols.len();
            statements.push(sql);
            tables.insert(t.name, cols);
        }
        meta.schemas.insert(schema.name, tables);
        if failure.is_some() {
            break;
        }
    }
    if !statements.is_empty() {
        trace.record(crate::types::browse::TracedSql {
            sql: crate::drivers::trace_text(Dialect::Sqlite, &statements.join(";\n"), &[]),
            category: TraceCategory::Structure,
            duration_ms: started.elapsed().as_millis() as u64,
            rows: failure.is_none().then_some(columns as u64),
            affected: None,
            error: failure.as_ref().map(ToString::to_string),
        });
    }
    match failure {
        Some(e) => Err(e),
        None => Ok(meta),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rebuild planned from introspected metadata keeps the table's CHECK constraints, adds and
    /// drops them, and runs on a real SQLite database.
    #[test]
    fn rebuilds_keep_and_change_checks() {
        use crate::sqlgen::ddl::plan_ddl;
        use crate::types::ddl::{CheckDef, DdlOp};

        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE stock (id INTEGER PRIMARY KEY, qty INT CHECK (qty >= 0), price REAL, CONSTRAINT cheap CHECK (price < 100));
             INSERT INTO stock (qty, price) VALUES (1, 2.5);",
        )
        .unwrap();
        let t = TableRef::new(None, "stock");
        let meta = table_meta(&mut conn, &SqlTrace::default(), &t).unwrap();
        let plan = plan_ddl(
            Dialect::Sqlite,
            Some(&meta),
            &[
                DdlOp::RenameColumn {
                    table: t.clone(),
                    name: "price".into(),
                    new_name: "cost".into(),
                },
                DdlOp::DropCheck {
                    table: t.clone(),
                    name: "ck_stock_1".into(),
                },
                DdlOp::AddCheck {
                    table: t.clone(),
                    check: CheckDef {
                        name: Some("cost_ok".into()),
                        expression: "cost > 0".into(),
                    },
                },
            ],
        )
        .unwrap();
        for sql in plan.statements.iter().filter(|s| !s.starts_with("SELECT")) {
            conn.execute_batch(sql)
                .unwrap_or_else(|e| panic!("{sql}: {e}"));
        }
        let after = table_meta(&mut conn, &SqlTrace::default(), &t).unwrap();
        let checks: Vec<(&str, &str)> = after
            .checks
            .iter()
            .map(|c| (c.name.as_str(), c.expression.as_str()))
            .collect();
        assert_eq!(
            checks,
            [("cheap", "\"cost\" < 100"), ("cost_ok", "\"cost\" > 0")]
        );
        // the new check holds
        assert!(conn.execute("UPDATE stock SET cost = -1", []).is_err());
        assert!(conn.execute("UPDATE stock SET qty = -1", []).is_ok());
    }

    #[test]
    fn reads_checks_partial_indexes_and_trigger_heads() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE items (
                id INTEGER PRIMARY KEY,
                price REAL NOT NULL CHECK (price > 0),
                qty INTEGER CONSTRAINT qty_ok CHECK(qty >= 0) DEFAULT 0,
                note TEXT DEFAULT 'CHECK (x)',
                CONSTRAINT cheap CHECK (price < 1000 OR qty = 0),
                CHECK (length(note) < 200)
             );
             CREATE INDEX items_live ON items (price) WHERE qty > 0;
             CREATE TRIGGER items_audit BEFORE DELETE ON items BEGIN SELECT RAISE(ABORT, 'no INSERT or UPDATE'); END;
             CREATE TRIGGER items_touch UPDATE OF qty ON items BEGIN SELECT 1; END;",
        )
        .unwrap();
        let m = table_meta(
            &mut conn,
            &SqlTrace::default(),
            &TableRef::new(None, "items"),
        )
        .unwrap();
        let checks: Vec<(&str, &str, Vec<&str>)> = m
            .checks
            .iter()
            .map(|c| {
                (
                    c.name.as_str(),
                    c.expression.as_str(),
                    c.columns.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            checks,
            [
                ("ck_items_1", "price > 0", vec!["price"]),
                ("qty_ok", "qty >= 0", vec!["qty"]),
                ("cheap", "price < 1000 OR qty = 0", vec!["price", "qty"]),
                ("ck_items_2", "length(note) < 200", vec!["note"]),
            ]
        );
        let live = m.indexes.iter().find(|i| i.name == "items_live").unwrap();
        assert_eq!(live.predicate.as_deref(), Some("qty > 0"));
        let audit = m.triggers.iter().find(|t| t.name == "items_audit").unwrap();
        assert_eq!(audit.timing.as_deref(), Some("BEFORE"));
        assert_eq!(audit.events, ["DELETE"]);
        let touch = m.triggers.iter().find(|t| t.name == "items_touch").unwrap();
        assert_eq!(touch.timing.as_deref(), Some("BEFORE"));
        assert_eq!(touch.events, ["UPDATE"]);
    }

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
        let tables = list_tables(&mut conn, &SqlTrace::default(), "main").unwrap();
        assert_eq!(
            tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["posts", "pt", "users", "v"]
        );
        assert_eq!(tables[3].kind, TableKind::View);

        let users = table_meta(
            &mut conn,
            &SqlTrace::default(),
            &TableRef::new(None, "users"),
        )
        .unwrap();
        assert_eq!(users.primary_key, ["id"]);
        assert!(users.columns[0].is_auto_increment);
        assert_eq!(users.columns[0].extra.as_deref(), Some("AUTOINCREMENT"));
        assert!(!users.columns[1].nullable);
        assert_eq!(users.columns[3].default_expr.as_deref(), Some("1.5"));
        assert_eq!(users.columns[3].kind, ValueKind::Float);
        assert_eq!(users.triggers.len(), 1);
        assert_eq!(users.triggers[0].timing.as_deref(), Some("AFTER"));
        assert_eq!(users.triggers[0].events, ["UPDATE"]);
        assert!(users.checks.is_empty());
        assert!(
            users
                .indexes
                .iter()
                .any(|i| i.unique && i.columns == ["email"])
        );
        assert!(users.ddl.unwrap().starts_with("CREATE TABLE users"));

        let posts = table_meta(
            &mut conn,
            &SqlTrace::default(),
            &TableRef::new(Some("main"), "posts"),
        )
        .unwrap();
        assert_eq!(posts.foreign_keys.len(), 1);
        assert_eq!(posts.foreign_keys[0].ref_table.name, "users");
        assert_eq!(posts.foreign_keys[0].on_delete, "CASCADE");
        assert!(
            posts
                .indexes
                .iter()
                .any(|i| i.name == "posts_user" && i.columns == ["user_id"])
        );

        let pt = table_meta(&mut conn, &SqlTrace::default(), &TableRef::new(None, "pt")).unwrap();
        assert_eq!(pt.primary_key, ["post_id", "tag_id"]);
        assert!(!pt.columns[0].is_auto_increment);

        let meta = completion_metadata(&mut conn, &SqlTrace::default()).unwrap();
        assert!(meta.schemas["main"].contains_key("users"));
        assert_eq!(
            meta.schemas["main"]["users"],
            ["id", "email", "name", "score"]
        );
    }
}
