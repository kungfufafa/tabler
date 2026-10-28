//! SQLite value conversion and generic query/execute helpers.

use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, Statement};

use crate::error::AppError;
use crate::types::query::{ColumnOrigin, ExecOutcome, ResultColumn, ResultSet};
use crate::types::schema::ValueKind;
use crate::types::value::Value;

/// Maps a declared column type (`decltype`) to a `ValueKind`, following SQLite affinity rules
/// plus a few conventions (DATE/DATETIME/BOOLEAN/JSON) used by ORMs.
pub fn kind_from_decltype(decl: Option<&str>) -> ValueKind {
    let Some(decl) = decl else {
        return ValueKind::Other;
    };
    let upper = decl.to_ascii_uppercase();
    let base = upper.split('(').next().unwrap_or("").trim();
    if base == "JSON" || base == "JSONB" {
        ValueKind::Json
    } else if base == "BOOLEAN" || base == "BOOL" {
        ValueKind::Bool
    } else if base == "DATETIME" || base == "TIMESTAMP" {
        ValueKind::Timestamp
    } else if base == "DATE" {
        ValueKind::Date
    } else if base == "TIME" {
        ValueKind::Time
    } else if base == "UUID" {
        ValueKind::Uuid
    } else if base.contains("INT") {
        ValueKind::Int
    } else if base.contains("CHAR") || base.contains("CLOB") || base.contains("TEXT") {
        ValueKind::Text
    } else if base.contains("BLOB") || base.is_empty() {
        if base.is_empty() {
            ValueKind::Other
        } else {
            ValueKind::Bytes
        }
    } else if base.contains("REAL") || base.contains("FLOA") || base.contains("DOUB") {
        ValueKind::Float
    } else if base.contains("NUMERIC") || base.contains("DECIMAL") {
        ValueKind::Decimal
    } else if base == "ANY" {
        ValueKind::Other
    } else {
        ValueKind::Decimal // NUMERIC affinity catch-all
    }
}

pub fn value_from_ref(v: ValueRef<'_>, kind: ValueKind) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => match kind {
            ValueKind::Bool => Value::Bool(i != 0),
            _ => Value::from_i64(i),
        },
        ValueRef::Real(f) => Value::from_f64("real", f),
        ValueRef::Text(bytes) => {
            let s = String::from_utf8_lossy(bytes).into_owned();
            match kind {
                ValueKind::Json => Value::Json(s),
                ValueKind::Date => Value::Date(s),
                ValueKind::Time => Value::Time(s),
                ValueKind::Timestamp => Value::Timestamp(s),
                ValueKind::TimestampTz => Value::TimestampTz(s),
                ValueKind::Uuid => Value::Uuid(s),
                ValueKind::Decimal => Value::Decimal(s),
                _ => Value::Text(s),
            }
        }
        ValueRef::Blob(b) => Value::from_bytes(b),
    }
}

/// Decodes one cell; the runtime storage class wins over the declared type (dynamic typing).
pub(crate) fn cell_value(v: ValueRef<'_>, kind: ValueKind) -> Value {
    let k = match v {
        ValueRef::Text(_) | ValueRef::Integer(_) => kind,
        _ => ValueKind::Other,
    };
    value_from_ref(v, k)
}

/// Binds a `Value` as a SQLite parameter.
pub fn to_sql(v: &Value) -> Result<ToSqlOutput<'static>, AppError> {
    use base64::Engine as _;
    use rusqlite::types::Value as Sv;
    Ok(ToSqlOutput::Owned(match v {
        Value::Null | Value::Default => Sv::Null,
        Value::Bool(b) => Sv::Integer(*b as i64),
        Value::Int(i) => Sv::Integer(*i),
        Value::Float(f) => Sv::Real(*f),
        Value::BigInt(s) => match s.parse::<i64>() {
            Ok(i) => Sv::Integer(i),
            Err(_) => Sv::Text(s.clone()),
        },
        Value::Bytes(b) => Sv::Blob(
            base64::engine::general_purpose::STANDARD
                .decode(&b.base64)
                .map_err(|e| AppError::InvalidInput(format!("invalid base64: {e}")))?,
        ),
        other => Sv::Text(other.as_text().unwrap_or_default()),
    }))
}

/// Result columns of a prepared statement `sql`, with the base-table column each one reads
/// (`sqlite3_column_database_name` / `table_name` / `origin_name`: SQLite resolves those through
/// aliases, joins, subqueries and views; expressions have none), kept only where they name the
/// row a result row really came from (`keep_trusted_origins`).
pub(crate) fn columns_of(conn: &Connection, stmt: &Statement<'_>, sql: &str) -> Vec<ResultColumn> {
    let origins = stmt.columns_with_metadata();
    let mut columns: Vec<ResultColumn> = stmt
        .columns()
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let decl = c.decl_type();
            let origin = origins.get(i).and_then(|m| {
                ColumnOrigin::from_names(
                    m.database_name().unwrap_or(""),
                    m.table_name()?,
                    m.origin_name()?,
                )
            });
            ResultColumn {
                name: c.name().to_string(),
                data_type: decl.unwrap_or("").to_string(),
                kind: kind_from_decltype(decl),
                nullable: None,
                table: origin.as_ref().map(|o| o.table.name.clone()),
                origin,
            }
        })
        .collect();
    keep_trusted_origins(conn, sql, &mut columns);
    columns
}

/// Every view of the connection's schemas (main, temp, attached), lower-cased.
const VIEW_NAMES_SQL: &str = "SELECT name FROM pragma_table_list WHERE type = 'view'";

/// Drops the origins SQLite reports for rows that do not come from that table row:
/// - a compound statement (`UNION` / `EXCEPT` / `INTERSECT`, also in a subquery) reports the
///   columns of its leftmost `SELECT` for every row, a supplier row as a customer;
/// - a view is resolved to the tables its body reads, so a view over a `UNION` or a self-join
///   (or a view joined with its own base table) would name the wrong row: a statement that reads
///   a view keeps no origins (also when the views cannot be listed);
/// - an origin whose table the statement text never names came through such a view.
///
/// Without an origin the result is read-only in the editor. A table named twice (a self-join) is
/// left to the renderer, which can say why it refuses.
fn keep_trusted_origins(conn: &Connection, sql: &str, columns: &mut [ResultColumn]) {
    if columns.iter().all(|c| c.origin.is_none()) {
        return;
    }
    let words = code_words(sql);
    let compound = words
        .iter()
        .any(|(w, quoted)| !quoted && matches!(w.as_str(), "union" | "except" | "intersect"));
    let reads_view = !compound
        && match view_names(conn) {
            Ok(views) => words.iter().any(|(w, _)| views.contains(w)),
            Err(e) => {
                log::debug!("sqlite: listing views for result origins failed: {e}");
                true
            }
        };
    for col in columns.iter_mut() {
        let named = col.origin.as_ref().is_some_and(|o| {
            let table = o.table.name.to_lowercase();
            words.iter().any(|(w, _)| *w == table)
        });
        if compound || reads_view || !named {
            col.origin = None;
            col.table = None;
        }
    }
}

fn view_names(conn: &Connection) -> rusqlite::Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare(VIEW_NAMES_SQL)?;
    let names = stmt.query_map([], |row| row.get::<_, String>(0))?;
    names.map(|n| n.map(|n| n.to_lowercase())).collect()
}

/// The names in the code of a SQLite statement, lower-cased, with whether each was quoted:
/// bare words and `"…"`, `` `…` `` and `[…]` identifiers. String literals and comments are
/// skipped.
fn code_words(sql: &str) -> Vec<(String, bool)> {
    let chars: Vec<char> = sql.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    // Index just past the `close` that ends the quoted run opened at `i` (doubled = escaped).
    let close_of = |i: usize, close: char, doubled: bool| -> (usize, String) {
        let mut j = i + 1;
        let mut text = String::new();
        while j < n {
            if chars[j] == close {
                if doubled && j + 1 < n && chars[j + 1] == close {
                    text.push(close);
                    j += 2;
                    continue;
                }
                return (j + 1, text);
            }
            text.push(chars[j]);
            j += 1;
        }
        (n, text)
    };
    while i < n {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '\'' {
            i = close_of(i, '\'', true).0;
        } else if c == '"' || c == '`' {
            let (end, text) = close_of(i, c, true);
            out.push((text.to_lowercase(), true));
            i = end;
        } else if c == '[' {
            let (end, text) = close_of(i, ']', false);
            out.push((text.to_lowercase(), true));
            i = end;
        } else if c == '-' && next == Some('-') {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            i += 2;
            while i < n && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i = (i + 2).min(n);
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                i += 1;
            }
            out.push((
                chars[start..i].iter().collect::<String>().to_lowercase(),
                false,
            ));
        } else {
            i += 1;
        }
    }
    out
}

/// Runs a row-returning statement. `max_rows` caps the result (sets `truncated`).
pub fn run_query(
    conn: &Connection,
    sql: &str,
    params: &[Value],
    max_rows: Option<usize>,
) -> Result<ResultSet, AppError> {
    let mut stmt = conn.prepare(sql)?;
    let columns = columns_of(conn, &stmt, sql);
    let kinds: Vec<ValueKind> = columns.iter().map(|c| c.kind).collect();
    let bound: Vec<ToSqlOutput<'static>> = params.iter().map(to_sql).collect::<Result<_, _>>()?;
    let mut rows = stmt.query(rusqlite::params_from_iter(bound.iter()))?;
    let mut out = Vec::new();
    let mut truncated = false;
    while let Some(row) = rows.next()? {
        if let Some(cap) = max_rows
            && out.len() >= cap
        {
            truncated = true;
            break;
        }
        let mut cells = Vec::with_capacity(kinds.len());
        for (i, kind) in kinds.iter().enumerate() {
            cells.push(cell_value(row.get_ref(i)?, *kind));
        }
        out.push(cells);
    }
    Ok(ResultSet {
        columns,
        rows: out,
        truncated,
    })
}

/// Runs a statement for its effects; statements with result columns (RETURNING) also return rows.
pub fn run_execute(
    conn: &Connection,
    sql: &str,
    params: &[Value],
) -> Result<ExecOutcome, AppError> {
    let mut stmt = conn.prepare(sql)?;
    let bound: Vec<ToSqlOutput<'static>> = params.iter().map(to_sql).collect::<Result<_, _>>()?;
    if stmt.column_count() > 0 {
        let columns = columns_of(conn, &stmt, sql);
        let kinds: Vec<ValueKind> = columns.iter().map(|c| c.kind).collect();
        let mut rows = stmt.query(rusqlite::params_from_iter(bound.iter()))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let mut cells = Vec::with_capacity(kinds.len());
            for (i, kind) in kinds.iter().enumerate() {
                cells.push(value_from_ref(row.get_ref(i)?, *kind));
            }
            out.push(cells);
        }
        drop(rows);
        Ok(ExecOutcome {
            affected: conn.changes(),
            last_insert_id: Some(conn.last_insert_rowid().to_string()),
            rows: Some(ResultSet {
                columns,
                rows: out,
                truncated: false,
            }),
            affected_unknown: false,
        })
    } else {
        let affected = stmt.execute(rusqlite::params_from_iter(bound.iter()))? as u64;
        Ok(ExecOutcome {
            affected,
            last_insert_id: Some(conn.last_insert_rowid().to_string()),
            rows: None,
            affected_unknown: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decltype_mapping() {
        assert_eq!(kind_from_decltype(Some("INTEGER")), ValueKind::Int);
        assert_eq!(kind_from_decltype(Some("varchar(20)")), ValueKind::Text);
        assert_eq!(kind_from_decltype(Some("BOOLEAN")), ValueKind::Bool);
        assert_eq!(kind_from_decltype(Some("DATETIME")), ValueKind::Timestamp);
        assert_eq!(kind_from_decltype(Some("JSON")), ValueKind::Json);
        assert_eq!(kind_from_decltype(Some("BLOB")), ValueKind::Bytes);
        assert_eq!(kind_from_decltype(Some("REAL")), ValueKind::Float);
        assert_eq!(
            kind_from_decltype(Some("NUMERIC(20,6)")),
            ValueKind::Decimal
        );
        assert_eq!(kind_from_decltype(None), ValueKind::Other);
    }

    #[test]
    fn query_and_execute_round_trip() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, flag BOOLEAN, data BLOB, big INTEGER, j JSON);",
        )
        .unwrap();
        let out = run_execute(
            &conn,
            "INSERT INTO t (name, flag, data, big, j) VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
            &[
                Value::Text("it's".into()),
                Value::Bool(true),
                Value::from_bytes(&[1, 2, 3]),
                Value::BigInt("9007199254740993".into()),
                Value::Json("{\"a\":1}".into()),
            ],
        )
        .unwrap();
        assert_eq!(out.affected, 1);
        assert_eq!(out.rows.unwrap().rows[0][0], Value::Int(1));

        let rs = run_query(&conn, "SELECT * FROM t", &[], None).unwrap();
        assert_eq!(
            rs.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["id", "name", "flag", "data", "big", "j"]
        );
        let row = &rs.rows[0];
        assert_eq!(row[1], Value::Text("it's".into()));
        assert_eq!(row[2], Value::Bool(true));
        assert_eq!(row[4], Value::BigInt("9007199254740993".into()));
        assert_eq!(row[5], Value::Json("{\"a\":1}".into()));
        match &row[3] {
            Value::Bytes(b) => assert_eq!(b.len, 3),
            _ => panic!("expected bytes"),
        }

        let rs = run_query(&conn, "SELECT id FROM t", &[], Some(0)).unwrap();
        assert!(rs.truncated);
        assert!(rs.rows.is_empty());
    }

    #[test]
    fn results_column_origins() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INT, title TEXT);
             CREATE VIEW titles AS SELECT id AS post_id, title FROM posts;",
        )
        .unwrap();
        let sql = "SELECT p.title AS t, u.id, u.name || '!' AS shout, 1 AS one \
                   FROM posts p JOIN users u ON u.id = p.user_id";
        let stmt = conn.prepare(sql).unwrap();
        let cols = columns_of(&conn, &stmt, sql);
        let origin = |i: usize| {
            cols[i].origin.as_ref().map(|o| {
                (
                    o.table.schema.clone(),
                    o.table.name.clone(),
                    o.column.clone(),
                )
            })
        };
        assert_eq!(
            origin(0),
            Some((Some("main".into()), "posts".into(), "title".into()))
        );
        assert_eq!(cols[0].name, "t");
        assert_eq!(cols[0].table.as_deref(), Some("posts"));
        assert_eq!(
            origin(1),
            Some((Some("main".into()), "users".into(), "id".into()))
        );
        // SQLite cannot tell the references to a table apart.
        assert_eq!(cols[1].origin.as_ref().unwrap().instance, None);
        assert_eq!(origin(2), None);
        assert_eq!(origin(3), None);
        assert_eq!(cols[3].table, None);

        // Through a subquery or a CTE the base column is still the row's.
        for sql in [
            "SELECT id FROM (SELECT * FROM posts)",
            "WITH x AS (SELECT * FROM \"POSTS\") SELECT id FROM x",
        ] {
            let stmt = conn.prepare(sql).unwrap();
            let o = columns_of(&conn, &stmt, sql)[0].origin.clone().expect(sql);
            assert_eq!((o.table.name.as_str(), o.column.as_str()), ("posts", "id"));
        }
    }

    /// The origins that would write an edit to another row than the one shown are dropped.
    #[test]
    fn results_untrusted_origins_are_dropped() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE suppliers (id INTEGER PRIMARY KEY, name TEXT);
             CREATE VIEW titles AS SELECT id AS post_id, name FROM customers;
             CREATE VIEW everyone AS SELECT id, name FROM customers \
                 UNION ALL SELECT id, name FROM suppliers;
             CREATE TEMP VIEW recent AS SELECT * FROM suppliers;",
        )
        .unwrap();
        let origins = |sql: &str| -> Vec<Option<(String, String)>> {
            let stmt = conn.prepare(sql).unwrap();
            columns_of(&conn, &stmt, sql)
                .into_iter()
                .map(|c| {
                    assert_eq!(c.table.is_some(), c.origin.is_some(), "{sql}");
                    c.origin.map(|o| (o.table.name, o.column))
                })
                .collect()
        };
        let none = |n: usize| vec![None; n];
        // A compound statement reports its leftmost SELECT's columns for every row.
        for sql in [
            "SELECT id, name FROM customers UNION ALL SELECT id, name FROM suppliers",
            "SELECT id, name FROM customers EXCEPT SELECT id, name FROM suppliers",
            "select id, name from customers intersect select id, name from suppliers",
            "SELECT * FROM (SELECT id, name FROM customers UNION SELECT id, name FROM suppliers)",
        ] {
            assert_eq!(origins(sql), none(2), "{sql}");
        }
        // A view resolves to its base tables: through a UNION, a plain view, a temp view, and
        // a view joined with its own base table.
        assert_eq!(origins("SELECT id, name FROM everyone"), none(2));
        assert_eq!(origins("SELECT post_id, name FROM titles"), none(2));
        assert_eq!(origins("SELECT id FROM recent"), none(1));
        assert_eq!(
            origins("SELECT c.id, t.name FROM customers c JOIN \"Titles\" t ON t.post_id = c.id"),
            none(2)
        );
        // A word in a string or a comment is no reference; quoted names are.
        assert_eq!(
            origins("SELECT id, name FROM customers WHERE name <> 'union' -- union\n"),
            [
                Some(("customers".into(), "id".into())),
                Some(("customers".into(), "name".into()))
            ]
        );
        assert_eq!(
            origins("SELECT id FROM [customers] /* titles */"),
            [Some(("customers".into(), "id".into()))]
        );
        assert_eq!(
            origins("SELECT id FROM `customers` WHERE 'everyone' = 'everyone'"),
            [Some(("customers".into(), "id".into()))]
        );
    }

    #[test]
    fn code_words_skip_strings_and_comments() {
        assert_eq!(
            code_words("SELECT \"A\"\"b\", [c d], `e` FROM t -- x\n/* y */ WHERE s = 'it''s'"),
            [
                ("select".into(), false),
                ("a\"b".into(), true),
                ("c d".into(), true),
                ("e".into(), true),
                ("from".into(), false),
                ("t".into(), false),
                ("where".into(), false),
                ("s".into(), false),
            ]
        );
        assert!(code_words("'unterminated").is_empty());
    }
}
