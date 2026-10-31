//! D1's JSON cells to Tabler's `Value`, and parameters to JSON.
//!
//! D1 serializes rows through JavaScript: integers beyond 2^53 arrive rounded, a whole REAL
//! arrives as an integer, and blobs arrive as arrays of byte values. The column's kind (from the
//! table's metadata on a browse) puts a whole REAL back as a float. An integer column that the
//! select builder reads as text when it is beyond 2^53 (a change request, see the lane brief)
//! comes back here as an exact `BigInt`.

use crate::drivers::sqlite_http::catalog::Rows;
use crate::drivers::sqlite_http::value::{bytes_value, inserts, result_column, text_value};
use crate::error::AppError;
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::ValueKind;
use crate::types::value::Value;

use super::api::{ENGINE, MAX_PARAMS, MAX_STATEMENT_BYTES, RawResult, RawRows};

/// How parameters travel. The API documents strings only; whether it takes JSON numbers and
/// `null` as well is unverified, so the session can fall back to text, which INTEGER and REAL
/// affinity convert back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParamEncoding {
    /// Numbers as JSON numbers, NULL as `null`.
    #[default]
    Native,
    /// Numbers as decimal text, NULL as `null`.
    Strings,
}

/// One parameter. Blobs cannot be bound over the REST API: the statement must carry them as
/// `X'…'` literals (built by sqlgen), so a `Bytes` parameter is refused here.
pub fn encode_param(v: &Value, encoding: ParamEncoding) -> Result<serde_json::Value, AppError> {
    use serde_json::Value as J;
    let number = |n: J, text: String| match encoding {
        ParamEncoding::Native => n,
        ParamEncoding::Strings => J::String(text),
    };
    Ok(match v {
        Value::Null | Value::Default => J::Null,
        Value::Bool(b) => number(J::from(i64::from(*b)), i64::from(*b).to_string()),
        Value::Int(n) => number(J::from(*n), n.to_string()),
        // Beyond 2^53 a JSON number would be rounded on the way; text is exact.
        Value::BigInt(s) => J::String(s.clone()),
        Value::Float(f) => match serde_json::Number::from_f64(*f) {
            Some(n) => number(J::Number(n), f.to_string()),
            None => {
                return Err(AppError::InvalidInput(format!(
                    "{f} cannot be sent to {ENGINE}"
                )));
            }
        },
        Value::Bytes(_) => {
            return Err(AppError::Unsupported(format!(
                "{ENGINE} cannot bind binary parameters"
            )));
        }
        other => J::String(other.as_text().unwrap_or_default()),
    })
}

/// A statement's parameters, within D1's limits (100 parameters, 100 KB of SQL).
pub fn encode_params(
    sql: &str,
    params: &[Value],
    encoding: ParamEncoding,
) -> Result<Vec<serde_json::Value>, AppError> {
    check_statement(sql, params.len())?;
    params.iter().map(|p| encode_param(p, encoding)).collect()
}

pub fn check_statement(sql: &str, params: usize) -> Result<(), AppError> {
    if params > MAX_PARAMS {
        return Err(AppError::InvalidInput(format!(
            "{ENGINE} takes at most {MAX_PARAMS} parameters per statement; this one has {params}"
        )));
    }
    if sql.len() > MAX_STATEMENT_BYTES {
        return Err(AppError::InvalidInput(format!(
            "{ENGINE} takes statements of at most 100 KB; this one has {} KB",
            sql.len().div_ceil(1000)
        )));
    }
    Ok(())
}

/// One cell; `full` keeps a whole blob (`fetch_cell`).
pub fn decode_cell(v: &serde_json::Value, kind: ValueKind, full: bool) -> Value {
    use serde_json::Value as J;
    match v {
        J::Null => Value::Null,
        J::Bool(b) => Value::Bool(*b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                match kind {
                    ValueKind::Bool => Value::Bool(i != 0),
                    // A whole REAL, which JavaScript prints without a fraction.
                    ValueKind::Float => Value::Float(i as f64),
                    _ => Value::from_i64(i),
                }
            } else if let Some(u) = n.as_u64() {
                Value::from_u64(u)
            } else {
                Value::from_f64("real", n.as_f64().unwrap_or(f64::NAN))
            }
        }
        J::String(s) => match kind {
            ValueKind::Int | ValueKind::BigInt if is_canonical_integer(s) => match s.parse::<i64>()
            {
                Ok(i) => Value::from_i64(i),
                Err(_) => Value::BigInt(s.clone()),
            },
            _ => text_value(s.clone(), kind),
        },
        J::Array(items) => match byte_array(items) {
            Some(bytes) => bytes_value(&bytes, full),
            None => Value::Json(v.to_string()),
        },
        // A Node `Buffer` as JSON.
        J::Object(map) => match map
            .get("data")
            .and_then(J::as_array)
            .and_then(|d| byte_array(d))
        {
            Some(bytes) if map.len() <= 2 => bytes_value(&bytes, full),
            _ => Value::Json(v.to_string()),
        },
    }
}

/// Digits as SQLite prints an integer: no sign but `-`, no leading zeros, no spaces.
fn is_canonical_integer(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
        && s != "-0"
}

fn byte_array(items: &[serde_json::Value]) -> Option<Vec<u8>> {
    items
        .iter()
        .map(|i| i.as_u64().and_then(|n| u8::try_from(n).ok()))
        .collect()
}

/// A statement's rows as a result set. D1 reports column names only, so the kinds come from
/// `kinds` (a browse knows the table's) or stay unknown.
pub fn result_set(
    rows: &RawRows,
    kinds: Option<&[ValueKind]>,
    max_rows: Option<usize>,
) -> ResultSet {
    let kind_of = |i: usize| {
        kinds
            .and_then(|k| k.get(i).copied())
            .unwrap_or(ValueKind::Other)
    };
    let cap = max_rows.unwrap_or(usize::MAX);
    ResultSet {
        columns: rows
            .columns
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let mut c = result_column(Some(name), None);
                c.kind = kind_of(i);
                c
            })
            .collect(),
        rows: rows
            .rows
            .iter()
            .take(cap)
            .map(|r| {
                r.iter()
                    .enumerate()
                    .map(|(i, v)| decode_cell(v, kind_of(i), false))
                    .collect()
            })
            .collect(),
        truncated: rows.rows.len() > cap,
    }
}

/// The rows of a catalog statement, by their JSON type alone.
pub fn plain_rows(result: &RawResult) -> Rows {
    result
        .rows()
        .iter()
        .map(|r| {
            r.iter()
                .map(|v| decode_cell(v, ValueKind::Other, false))
                .collect()
        })
        .collect()
}

/// What a write reports; rows (`RETURNING`) when the statement has columns.
pub fn exec_outcome(sql: &str, result: &RawResult) -> ExecOutcome {
    let affected = result.changes();
    ExecOutcome {
        affected,
        last_insert_id: (affected > 0 && inserts(sql))
            .then(|| result.meta.as_ref().and_then(|m| m.last_row_id))
            .flatten()
            .map(|id| id.to_string()),
        rows: result
            .results
            .as_ref()
            .filter(|r| !r.columns.is_empty())
            .map(|r| result_set(r, None, None)),
        affected_unknown: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_follow_the_column_kind() {
        assert_eq!(
            decode_cell(&json!(3), ValueKind::Other, false),
            Value::Int(3)
        );
        assert_eq!(
            decode_cell(&json!(3), ValueKind::Float, false),
            Value::Float(3.0)
        );
        assert_eq!(
            decode_cell(&json!(1), ValueKind::Bool, false),
            Value::Bool(true)
        );
        assert_eq!(
            decode_cell(&json!(2.5), ValueKind::Int, false),
            Value::Float(2.5)
        );
        // What D1 sends for 2^53 + 1: already rounded, still beyond the safe range.
        assert_eq!(
            decode_cell(&json!(9007199254740992_i64), ValueKind::Int, false),
            Value::BigInt("9007199254740992".into())
        );
        assert_eq!(
            decode_cell(&json!(u64::MAX), ValueKind::Int, false),
            Value::BigInt(u64::MAX.to_string())
        );
        assert_eq!(
            decode_cell(&json!(1e300), ValueKind::Other, false),
            Value::Float(1e300)
        );
    }

    #[test]
    fn timestamps_decimals_and_nested_values() {
        assert_eq!(
            decode_cell(&json!("2026-10-30 12:00:00"), ValueKind::Timestamp, false),
            Value::Timestamp("2026-10-30 12:00:00".into())
        );
        assert_eq!(
            decode_cell(&json!("12.50"), ValueKind::Decimal, false),
            Value::Decimal("12.50".into())
        );
        assert_eq!(
            decode_cell(&json!("{\"a\":[1]}"), ValueKind::Json, false),
            Value::Json("{\"a\":[1]}".into())
        );
    }

    #[test]
    fn integers_read_as_text_are_exact() {
        assert_eq!(
            decode_cell(&json!("9007199254740993"), ValueKind::Int, false),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            decode_cell(&json!("-9223372036854775808"), ValueKind::Int, false),
            Value::BigInt("-9223372036854775808".into())
        );
        for text in ["007", "1.0", " 1", "-0", "", "1e3", "abc"] {
            assert_eq!(
                decode_cell(&json!(text), ValueKind::Int, false),
                Value::Text(text.into()),
                "{text:?}"
            );
        }
        assert_eq!(
            decode_cell(&json!("2026-10-27"), ValueKind::Date, false),
            Value::Date("2026-10-27".into())
        );
    }

    #[test]
    fn blobs_arrive_as_byte_arrays() {
        assert_eq!(
            decode_cell(&json!([0, 1, 255]), ValueKind::Bytes, false),
            Value::from_bytes(&[0, 1, 255])
        );
        assert_eq!(
            decode_cell(&json!([]), ValueKind::Other, false),
            Value::from_bytes(&[])
        );
        assert_eq!(
            decode_cell(
                &json!({"type": "Buffer", "data": [7]}),
                ValueKind::Other,
                false
            ),
            Value::from_bytes(&[7])
        );
        assert_eq!(
            decode_cell(&json!([1, 256]), ValueKind::Other, false),
            Value::Json("[1,256]".into())
        );
        assert_eq!(
            decode_cell(&json!({"a": 1}), ValueKind::Other, false),
            Value::Json("{\"a\":1}".into())
        );
        let big = vec![json!(9); crate::types::value::BYTES_PREVIEW_CAP + 1];
        let Value::Bytes(b) = decode_cell(&json!(big), ValueKind::Bytes, true) else {
            panic!();
        };
        assert!(!b.truncated);
    }

    #[test]
    fn parameters() {
        let native: Vec<_> = [
            Value::Null,
            Value::Bool(true),
            Value::Int(5),
            Value::BigInt("9007199254740993".into()),
            Value::Float(1.5),
            Value::Text("x".into()),
            Value::Date("2026-01-01".into()),
        ]
        .iter()
        .map(|v| encode_param(v, ParamEncoding::Native).unwrap())
        .collect();
        assert_eq!(
            native,
            [
                json!(null),
                json!(1),
                json!(5),
                json!("9007199254740993"),
                json!(1.5),
                json!("x"),
                json!("2026-01-01")
            ]
        );
        assert_eq!(
            encode_param(&Value::Int(5), ParamEncoding::Strings).unwrap(),
            json!("5")
        );
        assert_eq!(
            encode_param(&Value::Float(1.5), ParamEncoding::Strings).unwrap(),
            json!("1.5")
        );
        assert_eq!(
            encode_param(&Value::Null, ParamEncoding::Strings).unwrap(),
            json!(null)
        );
        assert!(matches!(
            encode_param(&Value::from_bytes(&[1]), ParamEncoding::Native),
            Err(AppError::Unsupported(_))
        ));
        assert!(encode_param(&Value::Float(f64::INFINITY), ParamEncoding::Native).is_err());
        let many = vec![Value::Int(1); 101];
        assert!(encode_params("SELECT 1", &many, ParamEncoding::Native).is_err());
        assert!(encode_params("SELECT 1", &many[..100], ParamEncoding::Native).is_ok());
        let long = format!("SELECT '{}'", "x".repeat(MAX_STATEMENT_BYTES));
        assert!(encode_params(&long, &[], ParamEncoding::Native).is_err());
    }

    #[test]
    fn results_and_outcomes() {
        let raw: RawResult = serde_json::from_value(json!({
            "results": {"columns": ["id", "price"], "rows": [[1, 2], [2, 2.5], [3, null]]},
            "meta": {"changes": 0, "last_row_id": 9}
        }))
        .unwrap();
        let set = result_set(
            raw.results.as_ref().unwrap(),
            Some(&[ValueKind::Int, ValueKind::Float]),
            Some(2),
        );
        assert!(set.truncated);
        assert_eq!(
            set.rows,
            [
                [Value::Int(1), Value::Float(2.0)],
                [Value::Int(2), Value::Float(2.5)]
            ]
        );
        assert_eq!(set.columns[1].kind, ValueKind::Float);
        assert_eq!(set.columns[0].table, None);
        assert_eq!(plain_rows(&raw)[0], [Value::Int(1), Value::Int(2)]);
        let out = exec_outcome("DELETE FROM t RETURNING id, price", &raw);
        assert_eq!(out.rows.unwrap().rows.len(), 3);
        assert_eq!(out.last_insert_id, None);
        let insert: RawResult = serde_json::from_value(json!({
            "results": {"columns": [], "rows": []}, "meta": {"changes": 1, "last_row_id": 9}
        }))
        .unwrap();
        let out = exec_outcome("INSERT INTO t DEFAULT VALUES", &insert);
        assert_eq!(
            (out.affected, out.last_insert_id.as_deref()),
            (1, Some("9"))
        );
        assert!(out.rows.is_none());
        assert_eq!(
            exec_outcome("UPDATE t SET a = 1", &insert).last_insert_id,
            None
        );
    }
}
