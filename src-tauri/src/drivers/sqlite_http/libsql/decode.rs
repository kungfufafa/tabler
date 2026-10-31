//! Hrana values to Tabler's `Value` and back. A cell's storage class wins over its column's
//! declared type, as for local SQLite: an integer in a TEXT column stays a number.

use crate::drivers::sqlite_http::value::{
    bytes_value, decode_base64, encode_base64, inserts, integer_value, kind_from_decltype,
    param_bytes, result_column, text_value,
};
use crate::error::AppError;
use crate::types::query::{ExecOutcome, ResultColumn, ResultSet};
use crate::types::schema::ValueKind;
use crate::types::value::Value;

use super::hrana::{Col, HranaValue, StmtResult};

/// One cell. `full` keeps a whole blob (`fetch_cell`) instead of the capped preview.
pub fn decode_value(v: &HranaValue, kind: ValueKind, full: bool) -> Value {
    match v {
        HranaValue::Null => Value::Null,
        HranaValue::Integer { value } => match value.trim().parse::<i64>() {
            Ok(n) => integer_value(n, kind),
            // Not an i64: keep the digits exactly, or show what came.
            Err(_) if is_integer_text(value) => Value::BigInt(value.trim().to_string()),
            Err(_) => Value::other("integer", value.clone()),
        },
        HranaValue::Float { value: Some(f) } => Value::from_f64("real", *f),
        HranaValue::Float { value: None } => Value::other("real", "non-finite"),
        HranaValue::Text { value } => text_value(value.clone(), kind),
        HranaValue::Blob { base64 } => match decode_base64(base64) {
            Some(bytes) => bytes_value(&bytes, full),
            None => Value::other("blob", "invalid base64 from the server"),
        },
    }
}

fn is_integer_text(s: &str) -> bool {
    let s = s.trim();
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// A parameter. The base64 of `Value::Bytes` is checked (and a truncated preview refused), and a
/// non-finite float, which JSON cannot carry, is an error.
pub fn encode_value(v: &Value) -> Result<HranaValue, AppError> {
    let integer = |n: i64| HranaValue::Integer {
        value: n.to_string(),
    };
    Ok(match v {
        Value::Null | Value::Default => HranaValue::Null,
        Value::Bool(b) => integer(i64::from(*b)),
        Value::Int(n) => integer(*n),
        // Beyond i64 the local driver binds text too.
        Value::BigInt(s) => match s.trim().parse::<i64>() {
            Ok(n) => integer(n),
            Err(_) => HranaValue::Text { value: s.clone() },
        },
        Value::Float(f) if f.is_finite() => HranaValue::Float { value: Some(*f) },
        Value::Float(f) => {
            return Err(AppError::InvalidInput(format!(
                "{f} cannot be sent to a LibSQL server"
            )));
        }
        Value::Bytes(b) => HranaValue::Blob {
            base64: encode_base64(&param_bytes(b)?),
        },
        other => HranaValue::Text {
            value: other.as_text().unwrap_or_default(),
        },
    })
}

pub fn encode_values(values: &[Value]) -> Result<Vec<HranaValue>, AppError> {
    values.iter().map(encode_value).collect()
}

pub fn columns(cols: &[Col]) -> Vec<ResultColumn> {
    cols.iter()
        .map(|c| result_column(c.name.as_deref(), c.decltype.as_deref()))
        .collect()
}

/// Kinds per column: those `kinds` gives (a table's columns, for a browse) or else the declared
/// types of the result.
pub fn kinds_for(cols: &[Col], kinds: Option<&[ValueKind]>) -> Vec<ValueKind> {
    cols.iter()
        .enumerate()
        .map(|(i, c)| {
            kinds
                .and_then(|k| k.get(i).copied())
                .unwrap_or_else(|| kind_from_decltype(c.decltype.as_deref()))
        })
        .collect()
}

pub fn decode_row(row: &[HranaValue], kinds: &[ValueKind], full: bool) -> Vec<Value> {
    row.iter()
        .enumerate()
        .map(|(i, v)| decode_value(v, kinds.get(i).copied().unwrap_or(ValueKind::Other), full))
        .collect()
}

/// A statement's rows as a result set, at most `max_rows` of them.
pub fn result_set(
    result: &StmtResult,
    kinds: Option<&[ValueKind]>,
    max_rows: Option<usize>,
) -> ResultSet {
    let column_kinds = kinds_for(&result.cols, kinds);
    let cap = max_rows.unwrap_or(usize::MAX);
    ResultSet {
        columns: columns(&result.cols),
        rows: result
            .rows
            .iter()
            .take(cap)
            .map(|r| decode_row(r, &column_kinds, false))
            .collect(),
        truncated: result.rows.len() > cap,
    }
}

/// The rows of a catalog statement, by storage class alone.
pub fn plain_rows(result: &StmtResult) -> Vec<Vec<Value>> {
    let kinds = vec![ValueKind::Other; result.cols.len()];
    result
        .rows
        .iter()
        .map(|r| decode_row(r, &kinds, false))
        .collect()
}

/// What a write reports. Rows (`RETURNING`) come along when the statement has columns. The
/// rowid counts only for an INSERT that inserted (the stream's last rowid is otherwise stale).
pub fn exec_outcome(sql: &str, result: &StmtResult) -> ExecOutcome {
    ExecOutcome {
        affected: result.affected_row_count,
        last_insert_id: (result.affected_row_count > 0 && inserts(sql))
            .then(|| result.last_insert_rowid.clone())
            .flatten(),
        rows: (!result.cols.is_empty()).then(|| result_set(result, None, None)),
        affected_unknown: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::value::BYTES_PREVIEW_CAP;

    fn int(s: &str) -> HranaValue {
        HranaValue::Integer { value: s.into() }
    }

    #[test]
    fn integers_beyond_2_53_are_big_ints() {
        assert_eq!(
            decode_value(&int("9007199254740993"), ValueKind::Int, false),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            decode_value(&int("9007199254740991"), ValueKind::Int, false),
            Value::Int(9_007_199_254_740_991)
        );
        assert_eq!(
            decode_value(&int("-9223372036854775808"), ValueKind::Other, false),
            Value::BigInt("-9223372036854775808".into())
        );
        assert_eq!(
            decode_value(&int("18446744073709551616"), ValueKind::Int, false),
            Value::BigInt("18446744073709551616".into())
        );
        assert_eq!(
            decode_value(&int("1e3"), ValueKind::Int, false),
            Value::other("integer", "1e3")
        );
        assert_eq!(
            decode_value(&int("1"), ValueKind::Bool, false),
            Value::Bool(true)
        );
    }

    #[test]
    fn timestamps_and_decimals_keep_their_text() {
        let text = |v: &str| HranaValue::Text { value: v.into() };
        assert_eq!(
            decode_value(
                &text("2026-10-30 12:00:00.123"),
                ValueKind::Timestamp,
                false
            ),
            Value::Timestamp("2026-10-30 12:00:00.123".into())
        );
        assert_eq!(
            decode_value(&text("12.50"), ValueKind::Decimal, false),
            Value::Decimal("12.50".into())
        );
        // An integer text beyond i64, as a server may send it, stays exact.
        assert_eq!(
            decode_value(&int("99999999999999999999"), ValueKind::Int, false),
            Value::BigInt("99999999999999999999".into())
        );
    }

    #[test]
    fn storage_class_wins_over_the_declared_type() {
        let text = HranaValue::Text {
            value: "2026-10-27".into(),
        };
        assert_eq!(
            decode_value(&text, ValueKind::Date, false),
            Value::Date("2026-10-27".into())
        );
        // An integer in a DATE column stays a number; text in an INTEGER column stays text.
        assert_eq!(
            decode_value(&int("7"), ValueKind::Date, false),
            Value::Int(7)
        );
        assert_eq!(
            decode_value(
                &HranaValue::Text { value: "x".into() },
                ValueKind::Int,
                false
            ),
            Value::Text("x".into())
        );
        assert_eq!(
            decode_value(
                &HranaValue::Float { value: Some(2.5) },
                ValueKind::Int,
                false
            ),
            Value::Float(2.5)
        );
        assert_eq!(
            decode_value(&HranaValue::Float { value: None }, ValueKind::Float, false),
            Value::other("real", "non-finite")
        );
        assert_eq!(
            decode_value(
                &HranaValue::Text {
                    value: "{\"a\":1}".into()
                },
                ValueKind::Json,
                false
            ),
            Value::Json("{\"a\":1}".into())
        );
    }

    #[test]
    fn blobs_are_capped_unless_fetched_whole() {
        let big = vec![7u8; BYTES_PREVIEW_CAP + 1];
        let blob = HranaValue::Blob {
            base64: encode_base64(&big),
        };
        let Value::Bytes(preview) = decode_value(&blob, ValueKind::Bytes, false) else {
            panic!();
        };
        assert!(preview.truncated);
        let Value::Bytes(whole) = decode_value(&blob, ValueKind::Bytes, true) else {
            panic!();
        };
        assert!(!whole.truncated);
        assert_eq!(whole.len, big.len() as u64);
        assert_eq!(
            decode_value(
                &HranaValue::Blob {
                    base64: "%%".into()
                },
                ValueKind::Bytes,
                false
            ),
            Value::other("blob", "invalid base64 from the server")
        );
    }

    #[test]
    fn parameters_encode_like_the_local_driver_binds_them() {
        assert_eq!(encode_value(&Value::Bool(true)).unwrap(), int("1"));
        assert_eq!(
            encode_value(&Value::BigInt("9007199254740993".into())).unwrap(),
            int("9007199254740993")
        );
        assert_eq!(
            encode_value(&Value::BigInt("99999999999999999999".into())).unwrap(),
            HranaValue::Text {
                value: "99999999999999999999".into()
            }
        );
        assert_eq!(encode_value(&Value::Default).unwrap(), HranaValue::Null);
        assert_eq!(
            encode_value(&Value::from_bytes(&[1, 2])).unwrap(),
            HranaValue::Blob {
                base64: "AQI".into()
            }
        );
        assert_eq!(
            encode_value(&Value::Date("2026-01-01".into())).unwrap(),
            HranaValue::Text {
                value: "2026-01-01".into()
            }
        );
        assert!(encode_value(&Value::Float(f64::NAN)).is_err());
        let Value::Bytes(mut cut) = Value::from_bytes(&[1]) else {
            panic!();
        };
        cut.truncated = true;
        assert!(encode_value(&Value::Bytes(cut)).is_err());
    }

    #[test]
    fn results_cap_rows_and_report_writes() {
        let result = StmtResult {
            cols: vec![Col {
                name: Some("id".into()),
                decltype: Some("INTEGER".into()),
            }],
            rows: vec![vec![int("1")], vec![int("2")], vec![int("3")]],
            affected_row_count: 0,
            ..StmtResult::default()
        };
        let set = result_set(&result, None, Some(2));
        assert!(set.truncated);
        assert_eq!(set.rows, [[Value::Int(1)], [Value::Int(2)]]);
        assert_eq!(set.columns[0].kind, ValueKind::Int);
        assert_eq!(set.columns[0].origin, None);
        let bools = result_set(&result, Some(&[ValueKind::Bool]), None);
        assert_eq!(bools.rows[0], [Value::Bool(true)]);
        assert!(!bools.truncated);

        let write = StmtResult {
            affected_row_count: 2,
            last_insert_rowid: Some("9".into()),
            ..StmtResult::default()
        };
        let out = exec_outcome("INSERT INTO t DEFAULT VALUES", &write);
        assert_eq!(
            exec_outcome("UPDATE t SET a = 1", &write).last_insert_id,
            None
        );
        assert_eq!(out.affected, 2);
        assert_eq!(out.last_insert_id.as_deref(), Some("9"));
        assert!(out.rows.is_none());
        let returning = exec_outcome("DELETE FROM t RETURNING id", &result);
        assert_eq!(returning.rows.unwrap().rows.len(), 3);
        assert_eq!(returning.last_insert_id, None);
        assert_eq!(plain_rows(&result)[2], [Value::Int(3)]);
    }
}
