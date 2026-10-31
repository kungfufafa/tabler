//! The SQLite catalog over a remote transport: each call is a list of statements with bound
//! arguments, sent in one round trip (a Hrana pipeline, a D1 batch), and a parser over the rows
//! that come back. The queries use the table-valued PRAGMA functions, so no identifier is ever
//! formatted into the text, and the parsers follow `drivers::sqlite::introspect` (rowid aliases,
//! generated columns, partial indexes, CHECK constraints, trigger heads).
//!
//! Only the `main` schema is read: `sqld` and D1 refuse `ATTACH`, and D1 refuses
//! `PRAGMA database_list`.

use std::collections::BTreeMap;

use crate::drivers::sqlite::introspect::parse_trigger_head;
use crate::error::AppError;
use crate::sqlgen::ddl::{index_predicate, sqlite_checks};
use crate::types::schema::{
    ColumnMeta, CompletionMetadata, ForeignKeyMeta, IndexMeta, SchemaInfo, TableKind, TableMeta,
    TableRef, TableSummary, TriggerMeta,
};
use crate::types::value::Value;

use super::value::kind_from_decltype;

/// Which engine a catalog call is for: each hides its own internal tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remote {
    /// `libsql_wasm_func_table`, `_litestream_*` and the shadow tables of vector indexes.
    Libsql,
    /// `_cf_KV`, which D1 lists but refuses to read. `d1_migrations` is the user's.
    D1,
}

/// One catalog statement: `?1`… bound to `params`.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogQuery {
    pub sql: &'static str,
    pub params: Vec<Value>,
}

impl CatalogQuery {
    fn new(sql: &'static str, params: Vec<Value>) -> CatalogQuery {
        CatalogQuery { sql, params }
    }
}

/// The rows of one catalog statement.
pub type Rows = Vec<Vec<Value>>;

pub const MAIN: &str = "main";

/// The rows of the first statement of a round trip (none when it brought nothing back).
pub fn first_set(results: &[Rows]) -> &[Vec<Value>] {
    results.first().map_or(&[][..], Vec::as_slice)
}

// The filters on `sqlite_schema` rows (alias `m`) of each engine. Macros, so the statements stay
// `&'static str`. The vector-index shadow tables are `<index name>_shadow`.
macro_rules! libsql_hidden {
    () => {
        "m.name NOT LIKE 'sqlite\\_%' ESCAPE '\\' AND m.name NOT LIKE 'libsql\\_%' ESCAPE '\\' \
         AND m.name NOT LIKE '\\_litestream\\_%' ESCAPE '\\' \
         AND NOT (m.name LIKE '%\\_shadow' ESCAPE '\\' AND substr(m.name, 1, length(m.name) - 7) \
         IN (SELECT name FROM sqlite_schema WHERE type = 'index'))"
    };
}
macro_rules! d1_hidden {
    () => {
        "m.name NOT LIKE 'sqlite\\_%' ESCAPE '\\' AND m.name NOT LIKE '\\_cf\\_%' ESCAPE '\\'"
    };
}

const LIBSQL_TABLES_SQL: &str = concat!(
    "SELECT m.name, m.type FROM sqlite_schema m WHERE m.type IN ('table','view') AND ",
    libsql_hidden!(),
    " ORDER BY m.name"
);
const D1_TABLES_SQL: &str = concat!(
    "SELECT m.name, m.type FROM sqlite_schema m WHERE m.type IN ('table','view') AND ",
    d1_hidden!(),
    " ORDER BY m.name"
);
const LIBSQL_COMPLETION_SQL: &str = concat!(
    "SELECT m.name, p.name FROM sqlite_schema m JOIN pragma_table_info(m.name) p \
     WHERE m.type IN ('table','view') AND ",
    libsql_hidden!(),
    " ORDER BY m.name, p.cid"
);
const D1_COMPLETION_SQL: &str = concat!(
    "SELECT m.name, p.name FROM sqlite_schema m JOIN pragma_table_info(m.name) p \
     WHERE m.type IN ('table','view') AND ",
    d1_hidden!(),
    " ORDER BY m.name, p.cid"
);

const OBJECT_SQL: &str =
    "SELECT type, sql FROM sqlite_schema WHERE type IN ('table','view') AND name = ?1";
const COLUMNS_SQL: &str =
    "SELECT cid, name, type, \"notnull\", dflt_value, pk, hidden FROM pragma_table_xinfo(?1)";
const INDEXES_SQL: &str = "SELECT il.name, il.\"unique\", il.origin, il.partial, ix.seqno, \
     ix.name, ix.\"desc\", ix.key FROM pragma_index_list(?1) il \
     JOIN pragma_index_xinfo(il.name) ix ORDER BY il.seq, ix.seqno";
const DEPENDENTS_SQL: &str = "SELECT type, name, sql FROM sqlite_schema \
     WHERE tbl_name = ?1 AND type IN ('index','trigger') ORDER BY name";
const FOREIGN_KEYS_SQL: &str = "SELECT id, seq, \"table\", \"from\", \"to\", on_update, on_delete \
     FROM pragma_foreign_key_list(?1) ORDER BY id, seq";
const VIEW_SQL: &str = "SELECT sql FROM sqlite_schema WHERE type = 'view' AND name = ?1";
const VERSION_SQL: &str = "SELECT sqlite_version()";

/// The schemas: `main` only (see the module comment).
pub fn schemas() -> Vec<SchemaInfo> {
    vec![SchemaInfo {
        name: MAIN.to_string(),
        is_default: true,
    }]
}

pub fn list_tables_query(remote: Remote) -> CatalogQuery {
    CatalogQuery::new(
        match remote {
            Remote::Libsql => LIBSQL_TABLES_SQL,
            Remote::D1 => D1_TABLES_SQL,
        },
        Vec::new(),
    )
}

pub fn parse_tables(rows: &[Vec<Value>]) -> Result<Vec<TableSummary>, AppError> {
    rows.iter()
        .map(|r| {
            Ok(TableSummary {
                name: text(r, 0)?,
                schema: Some(MAIN.to_string()),
                kind: table_kind(&text(r, 1)?),
                row_estimate: None,
                size_bytes: None,
                comment: None,
            })
        })
        .collect()
}

/// The statements `parse_table_meta` reads, in its order: the object, its columns, its indexes
/// with their columns, the SQL of its indexes and triggers, its foreign keys.
pub fn table_meta_queries(table: &TableRef) -> Vec<CatalogQuery> {
    let name = || vec![Value::Text(table.name.clone())];
    vec![
        CatalogQuery::new(OBJECT_SQL, name()),
        CatalogQuery::new(COLUMNS_SQL, name()),
        CatalogQuery::new(INDEXES_SQL, name()),
        CatalogQuery::new(DEPENDENTS_SQL, name()),
        CatalogQuery::new(FOREIGN_KEYS_SQL, name()),
    ]
}

pub fn parse_table_meta(table: &TableRef, results: &[Rows]) -> Result<TableMeta, AppError> {
    let [object, columns, indexes, dependents, foreign_keys] = results else {
        return Err(AppError::internal(format!(
            "table metadata: expected 5 result sets, got {}",
            results.len()
        )));
    };
    let Some(first) = object.first() else {
        return Err(AppError::NotFound(format!("table {}", table.name)));
    };
    let kind = table_kind(&text(first, 0)?);
    let ddl = opt_text(first, 1)?;
    let columns = parse_columns(columns, ddl.as_deref())?;
    let mut primary_key: Vec<(u32, String)> = columns
        .iter()
        .filter_map(|c| c.pk_ordinal.map(|o| (o, c.name.clone())))
        .collect();
    primary_key.sort();
    let primary_key: Vec<String> = primary_key.into_iter().map(|(_, n)| n).collect();

    let mut index_sql: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut triggers = Vec::new();
    for r in dependents {
        let name = text(r, 1)?;
        let sql = opt_text(r, 2)?;
        if text(r, 0)? == "trigger" {
            let definition = sql.unwrap_or_default();
            let (timing, events) = parse_trigger_head(&definition);
            triggers.push(TriggerMeta {
                name,
                timing,
                events,
                enabled: true,
                definition,
            });
        } else {
            index_sql.insert(name, sql);
        }
    }
    let (indexes, foreign_keys) = if kind == TableKind::Table {
        (
            parse_indexes(indexes, &index_sql)?,
            parse_foreign_keys(&table.name, foreign_keys)?,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let checks = match (kind, ddl.as_deref()) {
        (TableKind::Table, Some(sql)) => {
            let names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
            sqlite_checks(sql, &table.name, &names)
        }
        _ => Vec::new(),
    };
    Ok(TableMeta {
        table: TableRef::new(Some(MAIN), &table.name),
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

fn parse_columns(rows: &[Vec<Value>], ddl: Option<&str>) -> Result<Vec<ColumnMeta>, AppError> {
    let mut columns = Vec::with_capacity(rows.len());
    for r in rows {
        let cid = int(r, 0)?;
        let data_type = opt_text(r, 2)?.unwrap_or_default();
        let pk = int(r, 5)?;
        let hidden = int(r, 6)?;
        columns.push(ColumnMeta {
            name: text(r, 1)?,
            ordinal: cid
                .checked_add(1)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            kind: kind_from_decltype(Some(&data_type)),
            data_type,
            nullable: int(r, 3)? == 0 && pk == 0,
            default_expr: opt_text(r, 4)?,
            is_pk: pk > 0,
            pk_ordinal: (pk > 0).then(|| u32::try_from(pk).unwrap_or(u32::MAX)),
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
    // `INTEGER PRIMARY KEY` is a rowid alias and assigns ids itself.
    let autoinc = ddl.is_some_and(|s| s.to_ascii_uppercase().contains("AUTOINCREMENT"));
    if columns.iter().filter(|c| c.is_pk).count() == 1
        && let Some(col) = columns.iter_mut().find(|c| c.is_pk)
        && col.data_type.eq_ignore_ascii_case("INTEGER")
    {
        col.is_auto_increment = true;
        col.extra = Some(
            if autoinc {
                "AUTOINCREMENT"
            } else {
                "rowid alias"
            }
            .into(),
        );
    }
    Ok(columns)
}

fn parse_indexes(
    rows: &[Vec<Value>],
    index_sql: &BTreeMap<String, Option<String>>,
) -> Result<Vec<IndexMeta>, AppError> {
    let mut indexes: Vec<IndexMeta> = Vec::new();
    for r in rows {
        let name = text(r, 0)?;
        if indexes.last().is_none_or(|i| i.name != name) {
            let definition = index_sql.get(&name).cloned().flatten();
            let predicate = if int(r, 3)? != 0 {
                definition
                    .as_deref()
                    .and_then(index_predicate)
                    .or_else(|| Some("partial".into()))
            } else {
                None
            };
            indexes.push(IndexMeta {
                name: name.clone(),
                columns: Vec::new(),
                unique: int(r, 1)? != 0,
                primary: text(r, 2)? == "pk",
                method: None,
                predicate,
                definition,
                comment: None,
                include: Vec::new(),
            });
        }
        // `key` 0: the rowid and other auxiliary columns of the index entry.
        if int(r, 7)? == 0 {
            continue;
        }
        let mut column = opt_text(r, 5)?.unwrap_or_else(|| "<expr>".into());
        if int(r, 6)? != 0 {
            column.push_str(" DESC");
        }
        if let Some(index) = indexes.last_mut() {
            index.columns.push(column);
        }
    }
    Ok(indexes)
}

fn parse_foreign_keys(table: &str, rows: &[Vec<Value>]) -> Result<Vec<ForeignKeyMeta>, AppError> {
    let mut groups: BTreeMap<i64, ForeignKeyMeta> = BTreeMap::new();
    for r in rows {
        let id = int(r, 0)?;
        let ref_table = text(r, 2)?;
        let from = text(r, 3)?;
        // `to` is NULL when the key references the parent's primary key implicitly.
        let to = opt_text(r, 4)?.unwrap_or_default();
        let on_update = text(r, 5)?;
        let on_delete = text(r, 6)?;
        let entry = groups.entry(id).or_insert_with(|| ForeignKeyMeta {
            name: format!("fk_{table}_{id}"),
            columns: Vec::new(),
            ref_table: TableRef::new(Some(MAIN), &ref_table),
            ref_columns: Vec::new(),
            on_update,
            on_delete,
            definition: None,
        });
        entry.columns.push(from);
        entry.ref_columns.push(to);
    }
    Ok(groups.into_values().collect())
}

/// The statements `parse_table_ddl` reads: the object's own SQL, then its indexes' and
/// triggers'.
pub fn table_ddl_queries(table: &TableRef) -> Vec<CatalogQuery> {
    let name = || vec![Value::Text(table.name.clone())];
    vec![
        CatalogQuery::new(OBJECT_SQL, name()),
        CatalogQuery::new(DEPENDENTS_SQL, name()),
    ]
}

/// What Show DDL gives: the `CREATE` statement SQLite keeps for the table or view, then those of
/// its indexes (an index a constraint made has none) and triggers.
pub fn parse_table_ddl(table: &TableRef, results: &[Rows]) -> Result<String, AppError> {
    let [object, dependents] = results else {
        return Err(AppError::internal(format!(
            "table DDL: expected 2 result sets, got {}",
            results.len()
        )));
    };
    let Some(first) = object.first() else {
        return Err(AppError::NotFound(format!("table {}", table.name)));
    };
    let mut statements = vec![opt_text(first, 1)?.unwrap_or_default()];
    for r in dependents {
        if let Some(sql) = opt_text(r, 2)? {
            statements.push(sql);
        }
    }
    Ok(statements
        .iter()
        .map(|s| format!("{};", s.trim().trim_end_matches(';')))
        .collect::<Vec<_>>()
        .join("\n\n"))
}

pub fn view_definition_query(table: &TableRef) -> CatalogQuery {
    CatalogQuery::new(VIEW_SQL, vec![Value::Text(table.name.clone())])
}

pub fn parse_view_definition(rows: &[Vec<Value>]) -> Result<Option<String>, AppError> {
    match rows.first() {
        Some(r) => opt_text(r, 0),
        None => Ok(None),
    }
}

/// Every table's and view's columns in one statement (not one PRAGMA per table).
pub fn completion_query(remote: Remote) -> CatalogQuery {
    CatalogQuery::new(
        match remote {
            Remote::Libsql => LIBSQL_COMPLETION_SQL,
            Remote::D1 => D1_COMPLETION_SQL,
        },
        Vec::new(),
    )
}

pub fn parse_completion(rows: &[Vec<Value>]) -> Result<CompletionMetadata, AppError> {
    let mut tables: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in rows {
        tables.entry(text(r, 0)?).or_default().push(text(r, 1)?);
    }
    Ok(CompletionMetadata {
        default_schema: Some(MAIN.to_string()),
        schemas: BTreeMap::from([(MAIN.to_string(), tables)]),
    })
}

pub fn version_query() -> CatalogQuery {
    CatalogQuery::new(VERSION_SQL, Vec::new())
}

pub fn parse_version(rows: &[Vec<Value>]) -> Result<String, AppError> {
    match rows.first() {
        Some(r) => text(r, 0),
        None => Err(AppError::db("sqlite_version() returned no row")),
    }
}

fn table_kind(ty: &str) -> TableKind {
    if ty.eq_ignore_ascii_case("view") {
        TableKind::View
    } else {
        TableKind::Table
    }
}

// Total accessors over catalog rows: a server that answers something unexpected gets an error,
// never a panic.

fn cell(row: &[Value], i: usize) -> Result<&Value, AppError> {
    row.get(i).ok_or_else(|| {
        AppError::db(format!(
            "catalog row has {} column(s), expected at least {}",
            row.len(),
            i + 1
        ))
    })
}

fn unexpected(i: usize, v: &Value, want: &str) -> AppError {
    AppError::db(format!(
        "catalog column {} is {v:?}, expected {want}",
        i + 1
    ))
}

fn int(row: &[Value], i: usize) -> Result<i64, AppError> {
    let v = cell(row, i)?;
    match v {
        Value::Int(n) => Some(*n),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::BigInt(s) | Value::Text(s) | Value::Decimal(s) => s.trim().parse().ok(),
        Value::Float(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Some(*f as i64),
        _ => None,
    }
    .ok_or_else(|| unexpected(i, v, "an integer"))
}

fn opt_text(row: &[Value], i: usize) -> Result<Option<String>, AppError> {
    let v = cell(row, i)?;
    match v {
        Value::Null => Ok(None),
        Value::Bytes(_) | Value::Default => Err(unexpected(i, v, "text")),
        other => Ok(other.as_text()),
    }
}

fn text(row: &[Value], i: usize) -> Result<String, AppError> {
    opt_text(row, i)?.ok_or_else(|| unexpected(i, &Value::Null, "text"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::sqlite_http::testing::{catalog_rows, sqlite_rows};
    use crate::types::schema::ValueKind;

    fn schema() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, email TEXT NOT NULL UNIQUE, name TEXT, score REAL DEFAULT 1.5);
             CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE, title TEXT,
                                 slug TEXT GENERATED ALWAYS AS (lower(title)) VIRTUAL);
             CREATE INDEX posts_user ON posts(user_id DESC, lower(title));
             CREATE INDEX posts_live ON posts(title) WHERE user_id > 0;
             CREATE TABLE pt (post_id INTEGER, tag_id INTEGER, PRIMARY KEY (post_id, tag_id),
                              FOREIGN KEY (post_id, tag_id) REFERENCES posts) WITHOUT ROWID;
             CREATE TABLE items (id INTEGER PRIMARY KEY, price REAL CHECK (price > 0), CONSTRAINT cheap CHECK (price < 1000));
             CREATE VIEW v AS SELECT id FROM users;
             CREATE TRIGGER trg AFTER UPDATE ON users BEGIN UPDATE users SET name = name WHERE id = NEW.id; END;
             CREATE TABLE libsql_wasm_func_table (name TEXT);
             CREATE TABLE _litestream_seq (id INTEGER);
             CREATE TABLE _cf_KV (key TEXT PRIMARY KEY, value BLOB);
             CREATE TABLE d1_migrations (id INTEGER PRIMARY KEY, name TEXT);
             CREATE INDEX emb_idx ON items (price);
             CREATE TABLE emb_idx_shadow (index_key INTEGER, data BLOB);
             CREATE TABLE notes_shadow (id INTEGER);",
        )
        .unwrap();
        conn
    }

    fn names(tables: &[TableSummary]) -> Vec<&str> {
        tables.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn each_engine_hides_its_internal_tables() {
        let conn = schema();
        let libsql = parse_tables(&sqlite_rows(&conn, &list_tables_query(Remote::Libsql))).unwrap();
        assert_eq!(
            names(&libsql),
            [
                "_cf_KV",
                "d1_migrations",
                "items",
                "notes_shadow",
                "posts",
                "pt",
                "users",
                "v"
            ]
        );
        assert_eq!(libsql[7].kind, TableKind::View);
        assert_eq!(libsql[0].schema.as_deref(), Some("main"));
        let d1 = parse_tables(&sqlite_rows(&conn, &list_tables_query(Remote::D1))).unwrap();
        assert_eq!(
            names(&d1),
            [
                "_litestream_seq",
                "d1_migrations",
                "emb_idx_shadow",
                "items",
                "libsql_wasm_func_table",
                "notes_shadow",
                "posts",
                "pt",
                "users",
                "v"
            ]
        );
    }

    #[test]
    fn table_metadata_matches_the_local_driver() {
        let conn = schema();
        let meta = |name: &str| {
            let t = TableRef::new(None, name);
            parse_table_meta(&t, &catalog_rows(&conn, &table_meta_queries(&t))).unwrap()
        };
        let users = meta("users");
        assert_eq!(users.table, TableRef::new(Some("main"), "users"));
        assert_eq!(users.primary_key, ["id"]);
        assert!(users.columns[0].is_auto_increment);
        assert_eq!(users.columns[0].extra.as_deref(), Some("AUTOINCREMENT"));
        assert_eq!(users.columns[0].ordinal, 1);
        assert!(!users.columns[1].nullable);
        assert_eq!(users.columns[3].default_expr.as_deref(), Some("1.5"));
        assert_eq!(users.columns[3].kind, ValueKind::Float);
        assert_eq!(users.triggers.len(), 1);
        assert_eq!(users.triggers[0].timing.as_deref(), Some("AFTER"));
        assert_eq!(users.triggers[0].events, ["UPDATE"]);
        assert!(
            users
                .indexes
                .iter()
                .any(|i| i.unique && i.columns == ["email"] && i.definition.is_none())
        );
        assert!(
            users
                .ddl
                .as_deref()
                .unwrap()
                .starts_with("CREATE TABLE users")
        );

        let posts = meta("posts");
        assert_eq!(posts.columns[0].extra.as_deref(), Some("rowid alias"));
        assert!(posts.columns[3].is_generated);
        assert_eq!(posts.columns[3].extra.as_deref(), Some("VIRTUAL"));
        assert_eq!(posts.foreign_keys.len(), 1);
        assert_eq!(posts.foreign_keys[0].name, "fk_posts_0");
        assert_eq!(posts.foreign_keys[0].ref_table.name, "users");
        assert_eq!(posts.foreign_keys[0].on_delete, "CASCADE");
        let by_user = posts
            .indexes
            .iter()
            .find(|i| i.name == "posts_user")
            .unwrap();
        assert_eq!(by_user.columns, ["user_id DESC", "<expr>"]);
        assert!(!by_user.unique);
        assert!(
            by_user
                .definition
                .as_deref()
                .unwrap()
                .starts_with("CREATE INDEX")
        );
        let live = posts
            .indexes
            .iter()
            .find(|i| i.name == "posts_live")
            .unwrap();
        assert_eq!(live.predicate.as_deref(), Some("user_id > 0"));

        let pt = meta("pt");
        assert_eq!(pt.primary_key, ["post_id", "tag_id"]);
        assert!(!pt.columns[0].is_auto_increment);
        assert!(pt.indexes.iter().any(|i| i.primary));
        // A key that references the parent's primary key implicitly has no `to` column.
        assert_eq!(pt.foreign_keys[0].columns, ["post_id", "tag_id"]);
        assert_eq!(pt.foreign_keys[0].ref_columns, ["", ""]);

        let items = meta("items");
        let checks: Vec<&str> = items.checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(checks, ["ck_items_1", "cheap"]);

        let v = meta("v");
        assert_eq!(v.kind, TableKind::View);
        assert!(v.indexes.is_empty() && v.foreign_keys.is_empty());
        assert_eq!(v.columns.len(), 1);

        let missing = TableRef::new(None, "nope");
        let err = parse_table_meta(
            &missing,
            &catalog_rows(&conn, &table_meta_queries(&missing)),
        )
        .unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "{err:?}");
    }

    #[test]
    fn views_completion_and_version() {
        let conn = schema();
        let v = TableRef::new(Some("main"), "v");
        assert_eq!(
            parse_view_definition(&sqlite_rows(&conn, &view_definition_query(&v))).unwrap(),
            Some("CREATE VIEW v AS SELECT id FROM users".into())
        );
        let users = TableRef::new(None, "users");
        assert_eq!(
            parse_view_definition(&sqlite_rows(&conn, &view_definition_query(&users))).unwrap(),
            None
        );
        let meta = parse_completion(&sqlite_rows(&conn, &completion_query(Remote::D1))).unwrap();
        assert_eq!(meta.default_schema.as_deref(), Some("main"));
        assert_eq!(
            meta.schemas["main"]["users"],
            ["id", "email", "name", "score"]
        );
        assert_eq!(meta.schemas["main"]["v"], ["id"]);
        assert!(!meta.schemas["main"].contains_key("_cf_KV"));
        let libsql =
            parse_completion(&sqlite_rows(&conn, &completion_query(Remote::Libsql))).unwrap();
        assert!(!libsql.schemas["main"].contains_key("emb_idx_shadow"));
        let version = parse_version(&sqlite_rows(&conn, &version_query())).unwrap();
        assert!(version.starts_with("3."), "{version}");
        assert_eq!(schemas()[0].name, "main");
    }

    #[test]
    fn table_ddl_has_the_indexes_and_triggers() {
        let conn = schema();
        let ddl = |name: &str| {
            let t = TableRef::new(Some("main"), name);
            parse_table_ddl(&t, &catalog_rows(&conn, &table_ddl_queries(&t)))
        };
        let users = ddl("users").unwrap();
        // The UNIQUE constraint's index has no SQL of its own.
        assert!(
            users.starts_with("CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT"),
            "{users}"
        );
        assert!(
            users.ends_with("WHERE id = NEW.id; END;"),
            "the trigger comes last: {users}"
        );
        assert_eq!(users.matches(";\n\n").count(), 1, "{users}");
        let posts = ddl("posts").unwrap();
        assert!(
            posts.contains(";\n\nCREATE INDEX posts_live ON posts(title) WHERE user_id > 0;"),
            "{posts}"
        );
        assert_eq!(ddl("v").unwrap(), "CREATE VIEW v AS SELECT id FROM users;");
        assert!(matches!(ddl("nope"), Err(AppError::NotFound(_))));
        assert!(parse_table_ddl(&TableRef::new(None, "t"), &[]).is_err());
    }

    #[test]
    fn unexpected_rows_are_errors() {
        let t = TableRef::new(None, "t");
        assert!(parse_table_meta(&t, &[]).is_err());
        let bad_object = vec![vec![vec![Value::Int(1)]], vec![], vec![], vec![], vec![]];
        assert!(parse_table_meta(&t, &bad_object).is_err());
        let short_column = vec![
            vec![vec![Value::Text("table".into()), Value::Null]],
            vec![vec![Value::Int(0), Value::Text("a".into())]],
            vec![],
            vec![],
            vec![],
        ];
        assert!(parse_table_meta(&t, &short_column).is_err());
        let blob_name = vec![vec![Value::from_bytes(b"x"), Value::Text("table".into())]];
        assert!(parse_tables(&blob_name).is_err());
        assert!(parse_version(&[]).is_err());
        // Numbers that arrive as text or whole floats still count.
        let loose = vec![
            vec![vec![Value::Text("table".into()), Value::Null]],
            vec![vec![
                Value::Text("0".into()),
                Value::Text("a".into()),
                Value::Text("INTEGER".into()),
                Value::Float(0.0),
                Value::Null,
                Value::BigInt("1".into()),
                Value::Int(0),
            ]],
            vec![],
            vec![],
            vec![],
        ];
        let meta = parse_table_meta(&t, &loose).unwrap();
        assert_eq!(meta.primary_key, ["a"]);
        assert_eq!(meta.columns[0].ordinal, 1);
        // A column number no SQLite sends: no ordinal, and no overflow.
        let mut huge = loose.clone();
        huge[1][0][0] = Value::BigInt(i64::MAX.to_string());
        assert_eq!(parse_table_meta(&t, &huge).unwrap().columns[0].ordinal, 0);
    }
}
