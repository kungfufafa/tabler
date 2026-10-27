//! The ER diagram (Generate Diagram…): loads a schema's tables, columns and foreign keys with the
//! one catalog query of `sqlgen::diagram` and assembles them into a `DiagramSchema`.

use std::collections::BTreeMap;

use base64::Engine;

use crate::drivers::DbSession;
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::diagram::{col, diagram_query, on_delete_action};
use crate::types::caps::Feature;
use crate::types::diagram::{DiagramColumn, DiagramRelation, DiagramSchema, DiagramTable};
use crate::types::schema::{TableKind, TableRef};
use crate::types::value::Value;

/// Every table and view of `schema` with its columns, and the foreign keys its tables declare.
pub async fn schema_relations(
    session: &dyn DbSession,
    schema: &str,
) -> Result<DiagramSchema, AppError> {
    let dialect = session.core_dialect(Feature::Diagram)?;
    let built = diagram_query(dialect, schema);
    let rows = session.query(&built.sql, &built.params).await?.rows;
    Ok(assemble(dialect, schema, &rows))
}

/// A cell as text: MySQL's text protocol may hand out bytes, SQLite integers.
fn text(row: &[Value], i: usize) -> Option<String> {
    match row.get(i)? {
        Value::Null | Value::Default | Value::Array(_) => None,
        Value::Text(s)
        | Value::BigInt(s)
        | Value::Decimal(s)
        | Value::Enum(s)
        | Value::Json(s)
        | Value::Uuid(s)
        | Value::Date(s)
        | Value::Time(s)
        | Value::Timestamp(s)
        | Value::TimestampTz(s)
        | Value::Interval(s) => Some(s.clone()),
        Value::Int(n) => Some(n.to_string()),
        Value::Float(f) => Some(f.to_string()),
        Value::Bool(b) => Some(if *b { "1" } else { "0" }.to_string()),
        Value::Other(o) => Some(o.text.clone()),
        Value::Bytes(b) => base64::engine::general_purpose::STANDARD
            .decode(&b.base64)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
    }
}

fn int(row: &[Value], i: usize) -> Option<i64> {
    let s = text(row, i)?;
    let s = s.trim();
    s.parse::<i64>()
        .ok()
        .or_else(|| s.parse::<f64>().ok().map(|f| f as i64))
}

fn kind_of(dialect: Dialect, raw: &str) -> TableKind {
    match (dialect, raw) {
        (Dialect::Postgres, "v") => TableKind::View,
        (Dialect::Postgres, "m") => TableKind::MaterializedView,
        (Dialect::Postgres, "f") => TableKind::ForeignTable,
        (Dialect::Postgres, "p") => TableKind::Partitioned,
        (Dialect::Mysql, "VIEW") => TableKind::View,
        (Dialect::Mysql, "SYSTEM VIEW") => TableKind::SystemView,
        (Dialect::Sqlite, "view") => TableKind::View,
        _ => TableKind::Table,
    }
}

/// Where a foreign key points, as (schema, table). SQLite (always) and MySQL (with
/// `lower_case_table_names` 1 or 2, the macOS default) match table names case-insensitively, and
/// report the referenced schema and table as the `REFERENCES` clause spells them: `Artists` for
/// the table `artists`. Those resolve to the diagram's table; an exact match always wins.
fn referenced_table(
    dialect: Dialect,
    schema: &str,
    tables: &BTreeMap<String, TableAcc>,
    ref_schema: Option<&str>,
    ref_table: &str,
) -> (String, String) {
    let ref_schema = ref_schema.unwrap_or(schema);
    let exact = ref_schema == schema && tables.contains_key(ref_table);
    if dialect == Dialect::Postgres || exact || !ref_schema.eq_ignore_ascii_case(schema) {
        return (ref_schema.to_string(), ref_table.to_string());
    }
    match tables.keys().find(|t| t.eq_ignore_ascii_case(ref_table)) {
        Some(t) => (schema.to_string(), t.clone()),
        None => (ref_schema.to_string(), ref_table.to_string()),
    }
}

/// A referenced column as `target` spells it: SQLite reports the `REFERENCES t (ID)` spelling,
/// and column names are case-insensitive on SQLite and MySQL.
fn column_name(dialect: Dialect, target: Option<&TableAcc>, name: &str) -> String {
    let columns = match target {
        Some(acc) if dialect != Dialect::Postgres => &acc.columns,
        _ => return name.to_string(),
    };
    if columns.iter().any(|c| c.2.name == name) {
        return name.to_string();
    }
    columns
        .iter()
        .find(|c| c.2.name.eq_ignore_ascii_case(name))
        .map_or_else(|| name.to_string(), |c| c.2.name.clone())
}

#[derive(Default)]
struct TableAcc {
    kind: Option<TableKind>,
    rows: Option<i64>,
    /// (ordinal, position in the primary key or 0, column).
    columns: Vec<(i64, i64, DiagramColumn)>,
}

struct KeyPart {
    ord: i64,
    column: String,
    ref_schema: Option<String>,
    ref_table: String,
    ref_column: Option<String>,
    on_delete: String,
}

/// Builds the diagram from the rows of `diagram_query` (see `sqlgen::diagram::col`).
pub fn assemble(dialect: Dialect, schema: &str, rows: &[Vec<Value>]) -> DiagramSchema {
    let mut tables: BTreeMap<String, TableAcc> = BTreeMap::new();
    let mut keys: BTreeMap<(String, String), Vec<KeyPart>> = BTreeMap::new();
    for row in rows {
        let (Some(rec), Some(table)) = (text(row, col::REC), text(row, col::TABLE)) else {
            continue;
        };
        match rec.as_str() {
            "c" => {
                let Some(name) = text(row, col::COLUMN) else {
                    continue;
                };
                let acc = tables.entry(table).or_default();
                if acc.kind.is_none() {
                    acc.kind = text(row, col::KIND).map(|k| kind_of(dialect, &k));
                    acc.rows = int(row, col::ROWS).filter(|n| *n >= 0);
                }
                let pk = int(row, col::PK).unwrap_or(0);
                acc.columns.push((
                    int(row, col::ORD).unwrap_or(i64::MAX),
                    pk,
                    DiagramColumn {
                        name,
                        data_type: text(row, col::TYPE).unwrap_or_default(),
                        is_pk: pk > 0,
                        nullable: int(row, col::NULLABLE) != Some(0),
                    },
                ));
            }
            "f" => {
                let (Some(column), Some(id), Some(ref_table)) = (
                    text(row, col::COLUMN),
                    text(row, col::FK_NAME),
                    text(row, col::REF_TABLE),
                ) else {
                    continue;
                };
                // SQLite keys are unnamed: their id is only unique inside the table.
                let name = match dialect {
                    Dialect::Sqlite => format!("{table}_fk{id}"),
                    _ => id,
                };
                keys.entry((table, name)).or_default().push(KeyPart {
                    ord: int(row, col::ORD).unwrap_or(0),
                    column,
                    ref_schema: text(row, col::REF_SCHEMA),
                    ref_table,
                    ref_column: text(row, col::REF_COLUMN),
                    on_delete: on_delete_action(
                        dialect,
                        &text(row, col::ON_DELETE).unwrap_or_default(),
                    ),
                });
            }
            _ => {}
        }
    }

    // Primary key columns in key order: what a SQLite key without target columns references.
    let primary_key = |acc: &TableAcc| -> Vec<String> {
        let mut pk: Vec<&(i64, i64, DiagramColumn)> =
            acc.columns.iter().filter(|c| c.1 > 0).collect();
        pk.sort_by_key(|c| (c.1, c.0));
        pk.into_iter().map(|c| c.2.name.clone()).collect()
    };

    let mut relations = Vec::with_capacity(keys.len());
    for ((table, name), mut parts) in keys {
        parts.sort_by_key(|p| p.ord);
        let Some(first) = parts.first() else {
            continue;
        };
        let (to_schema, to_table) = referenced_table(
            dialect,
            schema,
            &tables,
            first.ref_schema.as_deref(),
            &first.ref_table,
        );
        let target = (to_schema == schema)
            .then(|| tables.get(&to_table))
            .flatten();
        let on_delete = first.on_delete.clone();
        let from_columns: Vec<String> = parts.iter().map(|p| p.column.clone()).collect();
        let mut to_columns: Vec<String> = parts
            .iter()
            .filter_map(|p| p.ref_column.as_deref())
            .map(|c| column_name(dialect, target, c))
            .collect();
        if to_columns.len() != from_columns.len() {
            to_columns = target.map(primary_key).unwrap_or_default();
        }
        let to = TableRef::new(Some(&to_schema), &to_table);
        relations.push(DiagramRelation {
            name,
            from: TableRef::new(Some(schema), &table),
            from_columns,
            to,
            to_columns,
            on_delete,
        });
    }

    let tables = tables
        .into_iter()
        .map(|(name, mut acc)| {
            acc.columns.sort_by_key(|c| c.0);
            DiagramTable {
                table: TableRef::new(Some(schema), &name),
                kind: acc.kind.unwrap_or(TableKind::Table),
                columns: acc.columns.into_iter().map(|c| c.2).collect(),
                row_estimate: acc.rows,
            }
        })
        .collect();

    DiagramSchema {
        schema: schema.to_string(),
        tables,
        relations,
    }
}

/// Writes `base64` (standard alphabet) decoded to `path`: the diagram's PNG export.
pub async fn write_base64_file(path: &str, base64: &str) -> Result<(), AppError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64.trim())
        .map_err(|e| AppError::InvalidInput(format!("not valid base64: {e}")))?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|e| AppError::Io(format!("{path}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlgen::diagram::DIAGRAM_COLS;

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    /// A `c` row: table, kind, rows, column, type, ordinal, pk position, nullable.
    #[allow(clippy::too_many_arguments)]
    fn c(
        table: &str,
        kind: &str,
        rows: Option<&str>,
        name: &str,
        ty: &str,
        ord: i64,
        pk: i64,
        nullable: bool,
    ) -> Vec<Value> {
        let mut row = vec![Value::Null; DIAGRAM_COLS];
        row[col::REC] = t("c");
        row[col::TABLE] = t(table);
        row[col::KIND] = t(kind);
        row[col::ROWS] = rows.map_or(Value::Null, t);
        row[col::COLUMN] = t(name);
        row[col::TYPE] = t(ty);
        row[col::ORD] = Value::Int(ord);
        row[col::PK] = t(&pk.to_string());
        row[col::NULLABLE] = t(if nullable { "1" } else { "0" });
        row
    }

    /// An `f` row: table, column, key position, name, ref schema, ref table, ref column, action.
    #[allow(clippy::too_many_arguments)]
    fn f(
        table: &str,
        column: &str,
        ord: i64,
        name: &str,
        ref_schema: Option<&str>,
        ref_table: &str,
        ref_column: Option<&str>,
        on_delete: &str,
    ) -> Vec<Value> {
        let mut row = vec![Value::Null; DIAGRAM_COLS];
        row[col::REC] = t("f");
        row[col::TABLE] = t(table);
        row[col::COLUMN] = t(column);
        row[col::ORD] = Value::Int(ord);
        row[col::FK_NAME] = t(name);
        row[col::REF_SCHEMA] = ref_schema.map_or(Value::Null, t);
        row[col::REF_TABLE] = t(ref_table);
        row[col::REF_COLUMN] = ref_column.map_or(Value::Null, t);
        row[col::ON_DELETE] = t(on_delete);
        row
    }

    #[test]
    fn assembles_postgres_rows() {
        let rows = vec![
            c("orders", "r", Some("120"), "user_id", "bigint", 2, 0, false),
            c("orders", "r", Some("120"), "id", "bigint", 1, 1, false),
            c("orders", "r", Some("120"), "note", "text", 3, 0, true),
            c("users", "r", Some("-1"), "id", "bigint", 1, 1, false),
            c("active_users", "v", None, "id", "bigint", 1, 0, true),
            f(
                "orders",
                "user_id",
                1,
                "orders_user_id_fkey",
                Some("public"),
                "users",
                Some("id"),
                "c",
            ),
            f(
                "orders",
                "note",
                1,
                "orders_note_fkey",
                Some("audit"),
                "notes",
                Some("body"),
                "a",
            ),
        ];
        let d = assemble(Dialect::Postgres, "public", &rows);
        assert_eq!(d.schema, "public");
        let names: Vec<&str> = d.tables.iter().map(|t| t.table.name.as_str()).collect();
        assert_eq!(names, ["active_users", "orders", "users"]);
        let orders = &d.tables[1];
        assert_eq!(orders.kind, TableKind::Table);
        assert_eq!(orders.row_estimate, Some(120));
        assert_eq!(orders.table, TableRef::new(Some("public"), "orders"));
        let cols: Vec<&str> = orders.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(cols, ["id", "user_id", "note"]);
        assert!(orders.columns[0].is_pk && !orders.columns[1].is_pk);
        assert!(orders.columns[2].nullable && !orders.columns[0].nullable);
        assert_eq!(d.tables[0].kind, TableKind::View);
        // reltuples = -1 (never analyzed) is unknown.
        assert_eq!(d.tables[2].row_estimate, None);

        assert_eq!(d.relations.len(), 2);
        let audit = &d.relations[0];
        assert_eq!(audit.name, "orders_note_fkey");
        assert_eq!(audit.to, TableRef::new(Some("audit"), "notes"));
        assert_eq!(audit.on_delete, "NO ACTION");
        let users = &d.relations[1];
        assert_eq!(users.from, TableRef::new(Some("public"), "orders"));
        assert_eq!(users.from_columns, ["user_id"]);
        assert_eq!(users.to, TableRef::new(Some("public"), "users"));
        assert_eq!(users.to_columns, ["id"]);
        assert_eq!(users.on_delete, "CASCADE");
    }

    #[test]
    fn orders_composite_keys_by_position() {
        let rows = vec![
            c(
                "lines",
                "BASE TABLE",
                Some("5"),
                "order_id",
                "int",
                1,
                1,
                false,
            ),
            c(
                "lines",
                "BASE TABLE",
                Some("5"),
                "line_no",
                "int",
                2,
                1,
                false,
            ),
            c(
                "shipments",
                "BASE TABLE",
                None,
                "order_id",
                "int",
                1,
                0,
                false,
            ),
            c(
                "shipments",
                "BASE TABLE",
                None,
                "line_no",
                "int",
                2,
                0,
                false,
            ),
            c("v_lines", "VIEW", None, "order_id", "int", 1, 0, false),
            f(
                "shipments",
                "line_no",
                2,
                "fk_line",
                Some("shop"),
                "lines",
                Some("line_no"),
                "SET NULL",
            ),
            f(
                "shipments",
                "order_id",
                1,
                "fk_line",
                Some("shop"),
                "lines",
                Some("order_id"),
                "SET NULL",
            ),
        ];
        let d = assemble(Dialect::Mysql, "shop", &rows);
        assert_eq!(d.relations.len(), 1);
        let r = &d.relations[0];
        assert_eq!(r.from_columns, ["order_id", "line_no"]);
        assert_eq!(r.to_columns, ["order_id", "line_no"]);
        assert_eq!(r.on_delete, "SET NULL");
        assert_eq!(d.tables[2].kind, TableKind::View);
        assert_eq!(d.tables[0].row_estimate, Some(5));
    }

    #[test]
    fn sqlite_keys_default_to_the_primary_key() {
        let rows = vec![
            c("artists", "table", None, "name", "TEXT", 1, 0, true),
            c("artists", "table", None, "id", "INTEGER", 0, 1, true),
            c("albums", "table", None, "id", "INTEGER", 0, 1, true),
            c("albums", "table", None, "artist", "INTEGER", 1, 0, true),
            // `REFERENCES artists` without columns: `to` is NULL.
            f("albums", "artist", 0, "0", None, "artists", None, "CASCADE"),
            f(
                "tracks",
                "album",
                0,
                "0",
                None,
                "albums",
                Some("id"),
                "NO ACTION",
            ),
        ];
        let d = assemble(Dialect::Sqlite, "main", &rows);
        let by_name = |n: &str| d.relations.iter().find(|r| r.name == n).unwrap();
        let albums = by_name("albums_fk0");
        assert_eq!(albums.to, TableRef::new(Some("main"), "artists"));
        assert_eq!(albums.to_columns, ["id"]);
        assert_eq!(albums.on_delete, "CASCADE");
        // Unique per table: both keys have id 0.
        assert_eq!(by_name("tracks_fk0").to_columns, ["id"]);
        // Column order follows the ordinal (cid), not the row order.
        assert_eq!(d.tables[1].columns[0].name, "id");
    }

    #[test]
    fn resolves_references_spelled_in_another_case() {
        // SQLite: `REFERENCES Artists (ID)` and `REFERENCES ALBUMS` for `artists` / `albums`.
        let rows = vec![
            c("artists", "table", None, "id", "INTEGER", 0, 1, true),
            c("albums", "table", None, "id", "INTEGER", 0, 1, true),
            c("albums", "table", None, "artist", "INTEGER", 1, 0, true),
            c("tracks", "table", None, "album", "INTEGER", 1, 0, true),
            f(
                "albums",
                "artist",
                0,
                "0",
                None,
                "Artists",
                Some("ID"),
                "CASCADE",
            ),
            f("tracks", "album", 0, "0", None, "ALBUMS", None, "NO ACTION"),
            f("tracks", "x", 1, "1", None, "Missing", Some("Id"), ""),
        ];
        let d = assemble(Dialect::Sqlite, "main", &rows);
        let by_name = |n: &str| d.relations.iter().find(|r| r.name == n).unwrap();
        assert_eq!(
            by_name("albums_fk0").to,
            TableRef::new(Some("main"), "artists")
        );
        assert_eq!(by_name("albums_fk0").to_columns, ["id"]);
        assert_eq!(
            by_name("tracks_fk0").to,
            TableRef::new(Some("main"), "albums")
        );
        assert_eq!(by_name("tracks_fk0").to_columns, ["id"]);
        // A table that does not exist stays as spelled.
        assert_eq!(
            by_name("tracks_fk1").to,
            TableRef::new(Some("main"), "Missing")
        );
        assert_eq!(by_name("tracks_fk1").to_columns, ["Id"]);

        // MySQL with lower_case_table_names = 2: the schema and table as the clause spells them.
        let rows = vec![
            c("Artists", "BASE TABLE", None, "id", "int", 1, 1, false),
            c("Albums", "BASE TABLE", None, "artist", "int", 2, 0, true),
            f(
                "Albums",
                "artist",
                1,
                "fk_a",
                Some("SHOP"),
                "ARTISTS",
                Some("id"),
                "CASCADE",
            ),
            f(
                "Albums",
                "artist",
                1,
                "fk_b",
                Some("other"),
                "artists",
                Some("id"),
                "CASCADE",
            ),
        ];
        let d = assemble(Dialect::Mysql, "shop", &rows);
        assert_eq!(d.relations[0].to, TableRef::new(Some("shop"), "Artists"));
        // Another schema is left alone.
        assert_eq!(d.relations[1].to, TableRef::new(Some("other"), "artists"));

        // PostgreSQL names are case-sensitive: `Users` and `users` are two tables.
        let rows = vec![
            c("users", "r", None, "id", "bigint", 1, 1, false),
            c("orders", "r", None, "uid", "bigint", 2, 0, false),
            f(
                "orders",
                "uid",
                1,
                "fk",
                Some("public"),
                "Users",
                Some("ID"),
                "a",
            ),
        ];
        let d = assemble(Dialect::Postgres, "public", &rows);
        assert_eq!(d.relations[0].to, TableRef::new(Some("public"), "Users"));
        assert_eq!(d.relations[0].to_columns, ["ID"]);
    }

    #[test]
    fn skips_incomplete_rows() {
        let mut broken = c("t", "r", None, "a", "int", 1, 0, true);
        broken[col::COLUMN] = Value::Null;
        let rows = vec![broken, vec![t("x"), t("t")], vec![]];
        let d = assemble(Dialect::Postgres, "public", &rows);
        assert!(d.tables.is_empty() && d.relations.is_empty());
    }

    #[test]
    fn reads_bytes_and_numbers_as_text() {
        let bytes = Value::from_bytes_uncapped(b"orders");
        assert_eq!(text(&[bytes], 0).as_deref(), Some("orders"));
        assert_eq!(int(&[Value::Text("12.0".into())], 0), Some(12));
        assert_eq!(int(&[Value::Int(3)], 0), Some(3));
        assert_eq!(text(&[Value::Null], 0), None);
    }

    #[tokio::test]
    async fn writes_decoded_base64() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("diagram.png");
        let path = path.to_string_lossy().into_owned();
        write_base64_file(&path, "iVBORw0KGgo=").await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"\x89PNG\r\n\x1a\n");
        assert!(matches!(
            write_base64_file(&path, "not base64!").await,
            Err(AppError::InvalidInput(_))
        ));
        assert!(matches!(
            write_base64_file("/nonexistent/tabler/diagram.png", "AA==").await,
            Err(AppError::Io(_))
        ));
    }
}
