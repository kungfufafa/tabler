//! What a server narrows in its dialect's SQL, beyond the dialect itself. D1 runs SQLite behind a
//! JSON API: one statement binds at most `caps.data.max_params` (100) values and no blobs, and
//! integers come back as JavaScript numbers, exact only up to 2^53. The `sql` facade's `*_on`
//! builders take these limits from the session's caps (`Narrowing::for_caps`); a session that
//! narrows nothing gets the same SQL as the dialect's own builders.

use crate::error::AppError;
use crate::sqlgen::dialect::{DialectRef, SqlDialect};
use crate::sqlgen::select::BuiltSql;
use crate::sqlgen::{Dialect, sql};
use crate::types::caps::SessionCaps;
use crate::types::schema::ValueKind;
use crate::types::value::Value;

/// The largest integer a JavaScript number holds exactly (2^53 - 1).
pub const JS_SAFE_INTEGER: i64 = (1 << 53) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Narrowing {
    /// Bind parameters one statement may carry.
    pub max_params: usize,
    /// The select reads an integer beyond 2^53 as its text (`ColumnForm::IntegerText`).
    pub big_integers_as_text: bool,
    /// Blobs are written into statements as `X'…'` literals instead of being bound.
    pub blob_literals: bool,
}

impl Narrowing {
    /// The dialect as it is.
    pub fn of(d: impl Into<SqlDialect>) -> Narrowing {
        Narrowing {
            max_params: sql::max_params(d),
            big_integers_as_text: false,
            blob_literals: false,
        }
    }

    /// What a session with `caps` narrows: `caps.data.max_params` or the descriptor's
    /// `wire.max_params` where lower than the dialect's (0 says nothing), and on SQLite what the
    /// driver's API cannot carry (`wire`: D1's integers and blobs).
    pub fn for_caps(caps: &SessionCaps) -> Narrowing {
        let Some(dialect) = caps.dialect else {
            // A store that speaks no SQL builds none.
            return Narrowing {
                max_params: 0,
                big_integers_as_text: false,
                blob_literals: false,
            };
        };
        let own = Narrowing::of(dialect);
        let stated = caps.data.max_params as usize;
        let wire = caps.driver.descriptor().wire;
        let sqlite = matches!(dialect.resolve(), DialectRef::Core(Dialect::Sqlite));
        let max_params = [
            Some(stated).filter(|n| *n > 0),
            wire.max_params.map(|n| n as usize),
        ]
        .into_iter()
        .flatten()
        .fold(own.max_params, usize::min);
        Narrowing {
            max_params,
            big_integers_as_text: sqlite && wire.js_numbers,
            blob_literals: sqlite && wire.blob_literals,
        }
    }

    /// Applies the narrowing to a statement built for it: blobs written in, then the number of
    /// values checked (`what` names the statement in the refusal).
    pub(crate) fn finish(&self, built: BuiltSql, what: &str) -> Result<BuiltSql, AppError> {
        let built = if self.blob_literals {
            inline_blob_params(built)?
        } else {
            built
        };
        if built.params.len() > self.max_params {
            return Err(AppError::InvalidInput(format!(
                "{what} needs {} values, more than one statement can bind ({})",
                built.params.len(),
                self.max_params
            )));
        }
        Ok(built)
    }
}

/// Whether a SQLite column of `kind` may hold values stored as integers: INTEGER affinity
/// (`kind_from_decltype`'s Int), the NUMERIC affinity of Decimal and Bool (a BOOLEAN keeps any
/// integer it is given) and untyped columns. TEXT turns numbers into text, REAL into floats; the
/// date, time, JSON and UUID kinds hold text in practice, and a BLOB column's integers are rare
/// enough to stay numbers.
pub fn holds_integers(kind: ValueKind) -> bool {
    matches!(
        kind,
        ValueKind::Int
            | ValueKind::BigInt
            | ValueKind::Decimal
            | ValueKind::Bool
            | ValueKind::Other
    )
}

/// `ident` as a select reads it where integers beyond 2^53 must come as text: only those values,
/// every other one in its own type (a REAL, text or blob in the column stays what it is).
pub fn big_integer_projection(ident: &str) -> String {
    format!(
        "CASE WHEN typeof({ident}) = 'integer' AND {ident} NOT BETWEEN -{JS_SAFE_INTEGER} AND \
         {JS_SAFE_INTEGER} THEN CAST({ident} AS TEXT) ELSE {ident} END AS {ident}"
    )
}

/// Whether `text` is an integer as SQLite writes one (`CAST(i AS TEXT)`): an optional minus and
/// digits without a leading zero.
pub fn is_integer_text(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
}

/// A text cell of a `ColumnForm::IntegerText` column as the integer the select wrote as text: one
/// beyond 2^53. Other text (`'12'` in an untyped column) is text the column holds: None.
pub fn big_integer_from_text(text: &str) -> Option<Value> {
    if !is_integer_text(text) {
        return None;
    }
    let n: i64 = text.parse().ok()?;
    (!(-JS_SAFE_INTEGER..=JS_SAFE_INTEGER).contains(&n)).then(|| Value::from_i64(n))
}

/// `built` (SQLite, `?N` placeholders) with each blob parameter written in as an `X'…'` literal
/// and the other parameters renumbered in their order. Quoted text and names (`'…'`, `"…"`,
/// `` `…` ``, `[…]`) and comments are left alone, so a raw filter cannot move a placeholder.
pub fn inline_blob_params(built: BuiltSql) -> Result<BuiltSql, AppError> {
    if !built.params.iter().any(|p| matches!(p, Value::Bytes(_))) {
        return Ok(built);
    }
    // The new number of each kept parameter (1-based), None for a blob.
    let mut kept = Vec::new();
    let mut numbers = Vec::with_capacity(built.params.len());
    for p in &built.params {
        if matches!(p, Value::Bytes(_)) {
            numbers.push(None);
        } else {
            kept.push(p.clone());
            numbers.push(Some(kept.len()));
        }
    }
    let mut seen = vec![false; built.params.len()];
    let chars: Vec<char> = built.sql.chars().collect();
    let mut out = String::with_capacity(built.sql.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let close = match c {
            '\'' | '"' | '`' => Some(c),
            '[' => Some(']'),
            _ => None,
        };
        if let Some(close) = close {
            // A doubled quote inside reads as the end and a new start: the same text comes out.
            let end = chars[i + 1..]
                .iter()
                .position(|&x| x == close)
                .map_or(chars.len(), |p| i + 1 + p + 1);
            out.extend(&chars[i..end]);
            i = end;
            continue;
        }
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            let end = chars[i..]
                .iter()
                .position(|&x| x == '\n')
                .map_or(chars.len(), |p| i + p);
            out.extend(&chars[i..end]);
            i = end;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            let end = (i + 2..chars.len().saturating_sub(1))
                .find(|&k| chars[k] == '*' && chars[k + 1] == '/')
                .map_or(chars.len(), |k| k + 2);
            out.extend(&chars[i..end]);
            i = end;
            continue;
        }
        if c == '?' {
            let digits = chars[i + 1..]
                .iter()
                .take_while(|x| x.is_ascii_digit())
                .count();
            let n: Option<usize> = chars[i + 1..i + 1 + digits]
                .iter()
                .collect::<String>()
                .parse()
                .ok();
            if let Some(index) = n.and_then(|n| n.checked_sub(1))
                && index < built.params.len()
            {
                seen[index] = true;
                match numbers[index] {
                    Some(number) => out.push_str(&format!("?{number}")),
                    None => out.push_str(&crate::sqlgen::select::literal(
                        Dialect::Sqlite,
                        &built.params[index],
                    )),
                }
                i += 1 + digits;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    // Every value the builder bound has a placeholder; one not found would bind another value.
    if seen.contains(&false) {
        return Err(AppError::Internal(
            "a blob value could not be written into the statement".into(),
        ));
    }
    Ok(BuiltSql {
        sql: out,
        params: kept,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::profile::Driver;

    fn blob(bytes: &[u8]) -> Value {
        Value::from_bytes(bytes)
    }

    fn built(sql: &str, params: Vec<Value>) -> BuiltSql {
        BuiltSql {
            sql: sql.into(),
            params,
        }
    }

    #[test]
    fn only_d1_narrows_the_sqlite_dialect() {
        for d in [Dialect::Postgres, Dialect::Mysql, Dialect::Sqlite] {
            let driver = match d {
                Dialect::Postgres => Driver::Postgres,
                Dialect::Mysql => Driver::Mysql,
                Dialect::Sqlite => Driver::Sqlite,
            };
            let caps = SessionCaps::core(driver, d, None);
            assert_eq!(Narrowing::for_caps(&caps), Narrowing::of(d), "{d:?}");
        }
        // LibSQL binds as many values as SQLite.
        let libsql = SessionCaps::for_driver(Driver::Libsql);
        assert_eq!(Narrowing::for_caps(&libsql), Narrowing::of(Dialect::Sqlite));
        let d1 = Narrowing::for_caps(&SessionCaps::for_driver(Driver::D1));
        assert_eq!(
            d1,
            Narrowing {
                max_params: 100,
                big_integers_as_text: true,
                blob_literals: true,
            }
        );
        // Caps that state nothing keep the dialect's limit.
        let redshift = SessionCaps::base(Driver::Postgres, SqlDialect::Redshift, None);
        assert_eq!(redshift.data.max_params, 0);
        assert_eq!(
            Narrowing::for_caps(&redshift),
            Narrowing::of(SqlDialect::Redshift)
        );
    }

    #[test]
    fn blobs_are_written_in_and_the_rest_renumbered() {
        let b = built(
            "UPDATE \"t\" SET \"data\" = ?1, \"name\" = ?2 WHERE \"id\" = ?3 AND \"key\" = ?4",
            vec![
                blob(&[0xca, 0xfe]),
                Value::Text("x".into()),
                Value::Int(7),
                blob(&[]),
            ],
        );
        let out = inline_blob_params(b).unwrap();
        assert_eq!(
            out.sql,
            "UPDATE \"t\" SET \"data\" = X'cafe', \"name\" = ?1 WHERE \"id\" = ?2 AND \"key\" = X''"
        );
        assert_eq!(out.params, [Value::Text("x".into()), Value::Int(7)]);
        // A placeholder used twice (the any-column filter) keeps one number.
        let any = built(
            "\"a\" = ?1 OR \"b\" = ?1 OR \"c\" = ?2",
            vec![blob(&[1]), Value::Int(2)],
        );
        let out = inline_blob_params(any).unwrap();
        assert_eq!(out.sql, "\"a\" = X'01' OR \"b\" = X'01' OR \"c\" = ?1");
        // Without a blob nothing changes.
        let plain = built("SELECT ?1, ?2", vec![Value::Int(1), Value::Null]);
        assert_eq!(inline_blob_params(plain.clone()).unwrap(), plain);
    }

    #[test]
    fn quoted_text_names_and_comments_keep_their_question_marks() {
        let b = built(
            "SELECT \"a?1\", [b?1], `c?2` FROM t WHERE (x = '?1' -- ?2 it's\n) AND y = ?1 \
             /* ?2 ' */ AND z = ?2",
            vec![blob(&[0xff]), Value::Int(5)],
        );
        let out = inline_blob_params(b).unwrap();
        assert_eq!(
            out.sql,
            "SELECT \"a?1\", [b?1], `c?2` FROM t WHERE (x = '?1' -- ?2 it's\n) AND y = X'ff' \
             /* ?2 ' */ AND z = ?1"
        );
        assert_eq!(out.params, [Value::Int(5)]);
        // A value whose placeholder is not where the scan expects it is never bound elsewhere.
        let unbalanced = built(
            "x = 'oops AND y = ?1 AND z = ?2",
            vec![blob(&[1]), Value::Int(2)],
        );
        assert!(matches!(
            inline_blob_params(unbalanced),
            Err(AppError::Internal(_))
        ));
    }

    #[test]
    fn a_narrowed_statement_is_refused_past_the_servers_limit() {
        let d1 = Narrowing::for_caps(&SessionCaps::for_driver(Driver::D1));
        let at = |n: usize| {
            let sql = (1..=n)
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(", ");
            built(
                &format!("SELECT {sql}"),
                (0..n).map(|i| Value::Int(i as i64)).collect(),
            )
        };
        assert!(d1.finish(at(100), "the change").is_ok());
        assert_eq!(
            d1.finish(at(101), "the change").unwrap_err().to_string(),
            "the change needs 101 values, more than one statement can bind (100)"
        );
        // Blobs no longer count once they are written in.
        let mut params: Vec<Value> = (0..100).map(|i| Value::Int(i as i64)).collect();
        params.push(blob(&[1, 2]));
        let sql = (1..=101)
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let out = d1.finish(built(&sql, params), "the change").unwrap();
        assert_eq!(out.params.len(), 100);
        assert!(out.sql.ends_with("?100, X'0102'"));
    }

    use std::collections::BTreeMap;

    use rusqlite::types::ValueRef;

    use crate::drivers::sqlite::decode::kind_from_decltype;
    use crate::sqlgen::sql::{self, ColumnForm, SelectQuery, TableTarget};
    use crate::types::browse::{
        Conjunction, FilterGroup, FilterNode, FilterOp, RowIdentity, RowKey, SortDir, SortSpec,
    };
    use crate::types::mutate::StagedChange;
    use crate::types::schema::{ColumnMeta, TableRef};

    /// The columns of `t` below, with the kinds the SQLite drivers give their declared types.
    fn table_columns() -> Vec<ColumnMeta> {
        [
            ("id", "INTEGER"),
            ("n", "INTEGER"),
            ("r", "REAL"),
            ("s", "TEXT"),
            ("b", "BLOB"),
            ("u", ""),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (name, ty))| {
            let mut c: ColumnMeta = serde_json::from_value(serde_json::json!({
                "name": name, "ordinal": i + 1, "dataType": ty,
                "kind": kind_from_decltype((!ty.is_empty()).then_some(ty)),
                "nullable": i > 0, "isPk": i == 0, "isAutoIncrement": false,
                "isGenerated": false, "isUnsigned": false,
            }))
            .unwrap();
            c.pk_ordinal = (i == 0).then_some(1);
            c
        })
        .collect()
    }

    fn bind(params: &[Value]) -> Vec<rusqlite::types::Value> {
        params
            .iter()
            .map(|v| match v {
                Value::Int(i) => rusqlite::types::Value::Integer(*i),
                Value::Null => rusqlite::types::Value::Null,
                other => rusqlite::types::Value::Text(other.as_text().unwrap_or_default()),
            })
            .collect()
    }

    /// The rows `built` returns, each cell by its SQLite storage class.
    fn run(db: &rusqlite::Connection, built: &BuiltSql) -> Vec<Vec<String>> {
        let mut stmt = db.prepare(&built.sql).unwrap();
        let width = stmt.column_count();
        let rows = stmt
            .query_map(rusqlite::params_from_iter(bind(&built.params)), |row| {
                Ok((0..width)
                    .map(|i| match row.get_ref(i).unwrap() {
                        ValueRef::Null => "null".to_string(),
                        ValueRef::Integer(n) => format!("int {n}"),
                        ValueRef::Real(f) => format!("real {f}"),
                        ValueRef::Text(t) => format!("text {}", String::from_utf8_lossy(t)),
                        ValueRef::Blob(b) => format!("blob {b:?}"),
                    })
                    .collect())
            })
            .unwrap();
        rows.map(Result::unwrap).collect()
    }

    fn database() -> rusqlite::Connection {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER, r REAL, s TEXT, b BLOB, u);
             INSERT INTO t VALUES (1, 9007199254740993, 1.5, 'x', X'00', -9223372036854775808),
                                  (2, 9007199254740991, 2.0, 'y', NULL, 'abc'),
                                  (3, 'oops', NULL, NULL, NULL, 2.5);",
        )
        .unwrap();
        db
    }

    fn d1() -> Narrowing {
        Narrowing::for_caps(&SessionCaps::for_driver(Driver::D1))
    }

    #[test]
    fn d1_selects_integers_beyond_2_53_as_their_text() {
        let db = database();
        let cols = table_columns();
        let t = TableRef::new(None, "t");
        let pk = ["id".to_string()];
        let q = SelectQuery {
            table: &t,
            columns: &cols,
            selected: None,
            filter: None,
            sort: &[],
            limit: 10,
            offset: 0,
            pk: &pk,
            identity: RowIdentity::Pk,
        };
        let built = sql::select_on(&d1(), Dialect::Sqlite, &q).unwrap();
        let first = [
            "int 1",
            "text 9007199254740993",
            "real 1.5",
            "text x",
            "blob [0]",
            "text -9223372036854775808",
        ];
        let second = [
            "int 2",
            "int 9007199254740991",
            "real 2",
            "text y",
            "null",
            "text abc",
        ];
        let third = ["int 3", "text oops", "null", "null", "null", "real 2.5"];
        assert_eq!(run(&db, &built), [first, second, third]);
        use ColumnForm::{IntegerText, Native};
        let forms: Vec<ColumnForm> = cols
            .iter()
            .map(|c| sql::column_form_on(&d1(), Dialect::Sqlite, c))
            .collect();
        assert_eq!(
            forms,
            [
                IntegerText,
                IntegerText,
                Native,
                Native,
                Native,
                IntegerText
            ]
        );
        // SQLite and LibSQL read every column as it is, with the plain builder's SQL.
        for caps in [
            SessionCaps::core(Driver::Sqlite, Dialect::Sqlite, None),
            SessionCaps::for_driver(Driver::Libsql),
        ] {
            let own = Narrowing::for_caps(&caps);
            assert_eq!(
                sql::select_on(&own, Dialect::Sqlite, &q).unwrap(),
                sql::select(Dialect::Sqlite, &q).unwrap()
            );
            assert!(
                cols.iter()
                    .all(|c| sql::column_form_on(&own, Dialect::Sqlite, c) == Native)
            );
        }
        assert!(
            !sql::select(Dialect::Sqlite, &q)
                .unwrap()
                .sql
                .contains("CASE")
        );
    }

    #[test]
    fn d1_sorts_big_integers_as_numbers() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER);
             INSERT INTO t VALUES (5, 1234567890123456789), (100, -9007199254740995),
                                  (-9007199254740995, 7), (9007199254740993, NULL),
                                  (1234567890123456789, 9007199254740993);",
        )
        .unwrap();
        let cols = &table_columns()[..2];
        let pk = ["id".to_string()];
        let id_order = |sort: &[SortSpec], table: &TableRef| -> Vec<String> {
            let q = SelectQuery {
                table,
                columns: cols,
                selected: None,
                filter: None,
                sort,
                limit: 10,
                offset: 0,
                pk: &pk,
                identity: RowIdentity::Pk,
            };
            let built = sql::select_on(&d1(), Dialect::Sqlite, &q).unwrap();
            run(&db, &built).into_iter().map(|r| r[0].clone()).collect()
        };
        let by_n = |dir| {
            [SortSpec {
                column: "n".into(),
                dir,
                nulls: None,
            }]
        };
        // Without a sort the rows page in key order, with or without the schema named.
        let key_order = [
            "text -9007199254740995",
            "int 5",
            "int 100",
            "text 9007199254740993",
            "text 1234567890123456789",
        ];
        for table in [TableRef::new(Some("main"), "t"), TableRef::new(None, "t")] {
            assert_eq!(id_order(&[], &table), key_order);
        }
        let t = TableRef::new(Some("main"), "t");
        // n: NULL, -9007199254740995, 7, 9007199254740993, 1234567890123456789.
        assert_eq!(
            id_order(&by_n(SortDir::Asc), &t),
            [
                "text 9007199254740993",
                "int 100",
                "text -9007199254740995",
                "text 1234567890123456789",
                "int 5",
            ]
        );
        assert_eq!(
            id_order(&by_n(SortDir::Desc), &t),
            [
                "int 5",
                "text 1234567890123456789",
                "text -9007199254740995",
                "int 100",
                "text 9007199254740993",
            ]
        );
        let built = sql::select_on(
            &d1(),
            Dialect::Sqlite,
            &SelectQuery {
                table: &t,
                columns: cols,
                selected: None,
                filter: None,
                sort: &by_n(SortDir::Desc),
                limit: 10,
                offset: 0,
                pk: &pk,
                identity: RowIdentity::Pk,
            },
        )
        .unwrap();
        assert!(
            built.sql.contains(" ORDER BY \"main\".\"t\".\"n\" DESC\n"),
            "{}",
            built.sql
        );
        // A BOOLEAN keeps any integer it is given.
        assert!(holds_integers(kind_from_decltype(Some("BOOLEAN"))));
    }

    #[test]
    fn d1_filters_bind_at_most_its_limit() {
        let cols = table_columns();
        let t = TableRef::new(None, "t");
        let filter = |n: i64| FilterGroup {
            conjunction: Conjunction::And,
            items: vec![FilterNode::Condition {
                column: "n".into(),
                op: FilterOp::In,
                values: (0..n).map(Value::Int).collect(),
            }],
        };
        let q = |f| SelectQuery {
            table: &t,
            columns: &cols,
            selected: None,
            filter: Some(f),
            sort: &[],
            limit: 10,
            offset: 0,
            pk: &[],
            identity: RowIdentity::None,
        };
        let hundred = filter(100);
        let built = sql::select_on(&d1(), Dialect::Sqlite, &q(&hundred)).unwrap();
        assert_eq!(built.params.len(), 100);
        let more = filter(101);
        let refused = sql::select_on(&d1(), Dialect::Sqlite, &q(&more))
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("101 values") && refused.contains("(100)"),
            "{refused}"
        );
        assert!(sql::count_on(&d1(), Dialect::Sqlite, &t, &cols, Some(&more)).is_err());
        assert!(sql::count_on(&d1(), Dialect::Sqlite, &t, &cols, Some(&hundred)).is_ok());
        // SQLite's own limit is far above.
        assert!(sql::select(Dialect::Sqlite, &q(&more)).is_ok());
    }

    #[test]
    fn d1_edits_write_blobs_in_and_bind_the_rest() {
        let db = database();
        let cols = table_columns();
        let t = TableRef::new(None, "t");
        let target = TableTarget {
            table: &t,
            columns: &cols,
        };
        let key = RowKey::Pk(BTreeMap::from([("id".to_string(), Value::Int(2))]));
        let update = StagedChange::Update {
            row_id: "r".into(),
            key,
            changes: BTreeMap::from([
                ("b".to_string(), blob(&[0xca, 0xfe])),
                ("s".to_string(), Value::Text("it's".into())),
            ]),
            exprs: None,
        };
        let (built, expect) =
            sql::change_on(&d1(), Dialect::Sqlite, &target, &update, false).unwrap();
        assert_eq!(expect, Some(1));
        assert!(built.sql.contains("\"b\" = X'cafe'"), "{}", built.sql);
        assert!(!built.params.iter().any(|p| matches!(p, Value::Bytes(_))));
        let changed = db
            .execute(&built.sql, rusqlite::params_from_iter(bind(&built.params)))
            .unwrap();
        assert_eq!(changed, 1);
        let (b, s): (Vec<u8>, String) = db
            .query_row("SELECT b, s FROM t WHERE id = 2", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((b, s.as_str()), (vec![0xca, 0xfe], "it's"));
        // The same change on SQLite binds the blob.
        let (plain, _) = sql::change(Dialect::Sqlite, &target, &update, false).unwrap();
        assert!(plain.params.iter().any(|p| matches!(p, Value::Bytes(_))));
        // A fetch by a blob key writes it in too.
        let by_blob = RowKey::AllColumns(BTreeMap::from([("b".to_string(), blob(&[0]))]));
        let (fetch, params) =
            sql::fetch_cell_on(&d1(), Dialect::Sqlite, &target, &cols[3], &by_blob).unwrap();
        assert!(fetch.contains("X'00'") && params.is_empty(), "{fetch}");
        // An insert of more values than D1 binds is refused before it is sent.
        let wide: Vec<ColumnMeta> = (0..101)
            .map(|i| {
                let mut c = cols[1].clone();
                c.name = format!("c{i}");
                c
            })
            .collect();
        let insert = StagedChange::Insert {
            row_id: "n".into(),
            values: wide
                .iter()
                .map(|c| (c.name.clone(), Value::Int(1)))
                .collect(),
            exprs: None,
        };
        let wide_target = TableTarget {
            table: &t,
            columns: &wide,
        };
        let refused = sql::change_on(&d1(), Dialect::Sqlite, &wide_target, &insert, false)
            .unwrap_err()
            .to_string();
        assert!(refused.contains("101 values"), "{refused}");
        assert!(sql::change(Dialect::Sqlite, &wide_target, &insert, false).is_ok());
    }

    #[test]
    fn integer_text_is_what_sqlite_writes() {
        for (text, yes) in [
            ("0", true),
            ("-9007199254740993", true),
            ("9223372036854775807", true),
            ("007", false),
            ("-", false),
            ("", false),
            ("1.0", false),
            ("1e3", false),
            (" 1", false),
            ("abc", false),
        ] {
            assert_eq!(is_integer_text(text), yes, "{text:?}");
        }
        // Only what the select casts (beyond 2^53) is read back as a number.
        for (text, value) in [
            (
                "9007199254740992",
                Some(Value::BigInt("9007199254740992".into())),
            ),
            (
                "-9007199254740992",
                Some(Value::BigInt("-9007199254740992".into())),
            ),
            ("9007199254740991", None),
            ("12", None),
            ("99999999999999999999", None),
            ("x", None),
        ] {
            assert_eq!(big_integer_from_text(text), value, "{text:?}");
        }
    }
}
