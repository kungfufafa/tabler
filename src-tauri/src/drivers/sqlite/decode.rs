//! SQLite value conversion and generic query/execute helpers.

use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, Statement};

use crate::error::AppError;
use crate::types::query::{ExecOutcome, ResultColumn, ResultSet};
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

fn columns_of(stmt: &Statement<'_>) -> Vec<ResultColumn> {
    stmt.columns()
        .iter()
        .map(|c| {
            let decl = c.decl_type();
            ResultColumn {
                name: c.name().to_string(),
                data_type: decl.unwrap_or("").to_string(),
                kind: kind_from_decltype(decl),
                nullable: None,
                table: None,
            }
        })
        .collect()
}

/// Runs a row-returning statement. `max_rows` caps the result (sets `truncated`).
pub fn run_query(
    conn: &Connection,
    sql: &str,
    params: &[Value],
    max_rows: Option<usize>,
) -> Result<ResultSet, AppError> {
    let mut stmt = conn.prepare(sql)?;
    let columns = columns_of(&stmt);
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
            let v = row.get_ref(i)?;
            // Runtime type wins over the declared type for SQLite's dynamic typing.
            let k = match v {
                ValueRef::Text(_) => *kind,
                ValueRef::Integer(_) => *kind,
                _ => ValueKind::Other,
            };
            cells.push(value_from_ref(v, k));
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
        let columns = columns_of(&stmt);
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
        })
    } else {
        let affected = stmt.execute(rusqlite::params_from_iter(bound.iter()))? as u64;
        Ok(ExecOutcome {
            affected,
            last_insert_id: Some(conn.last_insert_rowid().to_string()),
            rows: None,
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
}
