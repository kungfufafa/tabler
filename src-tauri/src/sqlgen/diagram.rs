//! The ER diagram's catalog query: one statement per dialect returning the columns of every table
//! and view of a schema plus its foreign keys, so the diagram loads in one round trip.
//!
//! Every row has the same 14 text cells (`DIAGRAM_COLS`): a record type (`c` column, `f` foreign
//! key column) followed by the fields that record uses; the others are NULL.

use super::quote::Dialect;
use super::select::BuiltSql;
use crate::types::value::Value;

/// Cell positions of a diagram row.
pub mod col {
    /// `c` (a column of a table) or `f` (one column of a foreign key).
    pub const REC: usize = 0;
    pub const TABLE: usize = 1;
    /// `c`: PG `relkind`, MySQL `TABLE_TYPE`, SQLite `sqlite_master.type`.
    pub const KIND: usize = 2;
    /// `c`: the row estimate, NULL when unknown.
    pub const ROWS: usize = 3;
    /// `c`: the column; `f`: the referencing column.
    pub const COLUMN: usize = 4;
    /// `c`: the column type.
    pub const TYPE: usize = 5;
    /// `c`: the column's ordinal; `f`: the position inside the key.
    pub const ORD: usize = 6;
    /// `c`: the position inside the primary key, `0` when not part of it (MySQL: `1` / `0`).
    pub const PK: usize = 7;
    /// `c`: `1` when nullable.
    pub const NULLABLE: usize = 8;
    /// `f`: the constraint name (SQLite: the key's id).
    pub const FK_NAME: usize = 9;
    /// `f`: the referenced schema (NULL on SQLite: the same one).
    pub const REF_SCHEMA: usize = 10;
    pub const REF_TABLE: usize = 11;
    /// `f`: the referenced column (SQLite: NULL when the key references the primary key).
    pub const REF_COLUMN: usize = 12;
    /// `f`: the ON DELETE action (PG: `confdeltype`).
    pub const ON_DELETE: usize = 13;
}

/// Number of cells in a diagram row.
pub const DIAGRAM_COLS: usize = 14;

// PG: a partitioned table is one box. Its partitions (`relispartition`) are left out, and so are
// the foreign keys PostgreSQL clones from a declared one (`conparentid <> 0`): one per partition
// of a referenced partitioned table, and the copy on every partition of a referencing one.
const PG_DIAGRAM_SQL: &str = "\
SELECT 'c'::text, c.relname::text, c.relkind::text, \
       CASE WHEN c.reltuples < 0 THEN NULL ELSE c.reltuples::bigint::text END, \
       a.attname::text, pg_catalog.format_type(a.atttypid, a.atttypmod)::text, a.attnum::text, \
       COALESCE(pg_catalog.array_position(pk.conkey, a.attnum), 0)::text, \
       CASE WHEN a.attnotnull THEN '0' ELSE '1' END, \
       NULL::text, NULL::text, NULL::text, NULL::text, NULL::text \
FROM pg_catalog.pg_class c \
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
LEFT JOIN pg_catalog.pg_constraint pk ON pk.conrelid = c.oid AND pk.contype = 'p' \
WHERE n.nspname = $1 AND c.relkind IN ('r','p','v','m','f') AND NOT c.relispartition \
UNION ALL \
SELECT 'f', c.relname::text, NULL, NULL, a.attname::text, NULL, k.ord::text, NULL, NULL, \
       con.conname::text, rn.nspname::text, rc.relname::text, ra.attname::text, \
       con.confdeltype::text \
FROM pg_catalog.pg_constraint con \
JOIN pg_catalog.pg_class c ON c.oid = con.conrelid \
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
JOIN pg_catalog.pg_class rc ON rc.oid = con.confrelid \
JOIN pg_catalog.pg_namespace rn ON rn.oid = rc.relnamespace \
CROSS JOIN LATERAL unnest(con.conkey, con.confkey) WITH ORDINALITY AS k(attnum, refnum, ord) \
JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum = k.attnum \
JOIN pg_catalog.pg_attribute ra ON ra.attrelid = rc.oid AND ra.attnum = k.refnum \
WHERE con.contype = 'f' AND n.nspname = $1 AND NOT c.relispartition AND con.conparentid = 0";

// information_schema columns carry different collations (utf8mb3_bin / _tolower_ci…): each cell is
// converted to utf8mb4 so the UNION never mixes collations.
const MYSQL_DIAGRAM_SQL: &str = "\
SELECT 'c', CONVERT(c.TABLE_NAME USING utf8mb4), CONVERT(t.TABLE_TYPE USING utf8mb4), \
       CAST(t.TABLE_ROWS AS CHAR), CONVERT(c.COLUMN_NAME USING utf8mb4), \
       CONVERT(c.COLUMN_TYPE USING utf8mb4), CAST(c.ORDINAL_POSITION AS CHAR), \
       CASE WHEN c.COLUMN_KEY = 'PRI' THEN '1' ELSE '0' END, \
       CASE WHEN c.IS_NULLABLE = 'YES' THEN '1' ELSE '0' END, \
       NULL, NULL, NULL, NULL, NULL \
FROM information_schema.COLUMNS c \
JOIN information_schema.TABLES t ON t.TABLE_SCHEMA = c.TABLE_SCHEMA AND t.TABLE_NAME = c.TABLE_NAME \
WHERE c.TABLE_SCHEMA = ? \
UNION ALL \
SELECT 'f', CONVERT(k.TABLE_NAME USING utf8mb4), NULL, NULL, CONVERT(k.COLUMN_NAME USING utf8mb4), \
       NULL, CAST(k.ORDINAL_POSITION AS CHAR), NULL, NULL, \
       CONVERT(k.CONSTRAINT_NAME USING utf8mb4), CONVERT(k.REFERENCED_TABLE_SCHEMA USING utf8mb4), \
       CONVERT(k.REFERENCED_TABLE_NAME USING utf8mb4), \
       CONVERT(k.REFERENCED_COLUMN_NAME USING utf8mb4), CONVERT(r.DELETE_RULE USING utf8mb4) \
FROM information_schema.KEY_COLUMN_USAGE k \
JOIN information_schema.REFERENTIAL_CONSTRAINTS r ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA \
     AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME AND r.TABLE_NAME = k.TABLE_NAME \
WHERE k.TABLE_SCHEMA = ? AND k.REFERENCED_TABLE_NAME IS NOT NULL";

/// SQLite: the PRAGMA table-valued functions take the schema as their last argument.
/// Primary key columns count as NOT NULL, as in the structure tab.
fn sqlite_diagram_sql(schema: &str) -> String {
    let master = format!("{}.sqlite_master", Dialect::Sqlite.quote_ident(schema));
    format!(
        r#"SELECT 'c', m.name, m.type, NULL, p.name, p.type, p.cid, p.pk,
       CASE WHEN p."notnull" OR p.pk > 0 THEN '0' ELSE '1' END, NULL, NULL, NULL, NULL, NULL
FROM {master} AS m JOIN pragma_table_info(m.name, ?1) AS p
WHERE m.type IN ('table', 'view') AND m.name NOT LIKE 'sqlite\_%' ESCAPE '\'
UNION ALL
SELECT 'f', m.name, NULL, NULL, f."from", NULL, f.seq, NULL, NULL, f.id, NULL, f."table",
       f."to", f.on_delete
FROM {master} AS m JOIN pragma_foreign_key_list(m.name, ?1) AS f
WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite\_%' ESCAPE '\'"#
    )
}

/// The catalog query of `schema` (PG schema, MySQL database, SQLite attached database).
pub fn diagram_query(dialect: Dialect, schema: &str) -> BuiltSql {
    let param = || Value::Text(schema.to_string());
    match dialect {
        Dialect::Postgres => BuiltSql {
            sql: PG_DIAGRAM_SQL.to_string(),
            params: vec![param()],
        },
        Dialect::Mysql => BuiltSql {
            sql: MYSQL_DIAGRAM_SQL.to_string(),
            params: vec![param(), param()],
        },
        Dialect::Sqlite => BuiltSql {
            sql: sqlite_diagram_sql(schema),
            params: vec![param()],
        },
    }
}

/// A PG `confdeltype` as its SQL spelling; MySQL and SQLite already spell the action out.
pub fn on_delete_action(dialect: Dialect, raw: &str) -> String {
    match (dialect, raw) {
        (Dialect::Postgres, "c") => "CASCADE".into(),
        (Dialect::Postgres, "n") => "SET NULL".into(),
        (Dialect::Postgres, "d") => "SET DEFAULT".into(),
        (Dialect::Postgres, "r") => "RESTRICT".into(),
        (Dialect::Postgres, _) => "NO ACTION".into(),
        (_, "") => "NO ACTION".into(),
        (_, other) => other.trim().to_ascii_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Number of top-level cells of one `SELECT … FROM` branch.
    fn select_cells(branch: &str) -> usize {
        let list = branch
            .split_once("SELECT")
            .and_then(|(_, rest)| rest.split_once("FROM"))
            .map(|(cells, _)| cells)
            .expect("a SELECT … FROM branch");
        let mut depth = 0i32;
        let mut quoted = false;
        let mut cells = 1;
        for ch in list.chars() {
            match ch {
                '\'' => quoted = !quoted,
                '(' if !quoted => depth += 1,
                ')' if !quoted => depth -= 1,
                ',' if !quoted && depth == 0 => cells += 1,
                _ => {}
            }
        }
        cells
    }

    #[test]
    fn every_branch_returns_the_same_cells() {
        for dialect in [Dialect::Postgres, Dialect::Mysql, Dialect::Sqlite] {
            let built = diagram_query(dialect, "app");
            let branches: Vec<&str> = built.sql.split("UNION ALL").collect();
            assert_eq!(branches.len(), 2, "{dialect:?}");
            for branch in branches {
                assert_eq!(select_cells(branch), DIAGRAM_COLS, "{dialect:?}: {branch}");
            }
        }
    }

    #[test]
    fn binds_the_schema_once_per_placeholder() {
        let pg = diagram_query(Dialect::Postgres, "public");
        assert_eq!(pg.params, vec![Value::Text("public".into())]);
        assert_eq!(pg.sql.matches("$1").count(), 2);
        assert!(!pg.sql.contains("$2"));

        let my = diagram_query(Dialect::Mysql, "shop");
        assert_eq!(my.sql.matches('?').count(), my.params.len());
        assert_eq!(my.params.len(), 2);

        let lite = diagram_query(Dialect::Sqlite, "main");
        assert_eq!(lite.params, vec![Value::Text("main".into())]);
        assert!(lite.sql.contains(r#"FROM "main".sqlite_master AS m"#));
        assert!(lite.sql.contains("pragma_table_info(m.name, ?1)"));
        assert!(lite.sql.contains("pragma_foreign_key_list(m.name, ?1)"));
    }

    #[test]
    fn pg_leaves_out_partitions_and_cloned_keys() {
        let sql = diagram_query(Dialect::Postgres, "public").sql;
        let (columns, keys) = sql.split_once("UNION ALL").unwrap();
        assert!(columns.contains("AND NOT c.relispartition"), "{columns}");
        assert!(keys.contains("AND NOT c.relispartition"), "{keys}");
        assert!(keys.contains("AND con.conparentid = 0"), "{keys}");
    }

    #[test]
    fn quotes_the_sqlite_schema() {
        let sql = diagram_query(Dialect::Sqlite, r#"we"ird"#).sql;
        assert!(sql.contains(r#"FROM "we""ird".sqlite_master"#), "{sql}");
    }

    #[test]
    fn spells_out_delete_actions() {
        let pg = |c| on_delete_action(Dialect::Postgres, c);
        assert_eq!(pg("c"), "CASCADE");
        assert_eq!(pg("n"), "SET NULL");
        assert_eq!(pg("d"), "SET DEFAULT");
        assert_eq!(pg("r"), "RESTRICT");
        assert_eq!(pg("a"), "NO ACTION");
        assert_eq!(on_delete_action(Dialect::Mysql, "SET NULL"), "SET NULL");
        assert_eq!(on_delete_action(Dialect::Sqlite, "cascade"), "CASCADE");
        assert_eq!(on_delete_action(Dialect::Sqlite, ""), "NO ACTION");
    }
}
