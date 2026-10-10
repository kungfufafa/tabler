//! `Value` → `mysql_async::Value` bind parameters (binary protocol).

use crate::error::AppError;
use crate::types::value::Value;

/// Converts app values into positional MySQL parameters.
pub fn bind(values: &[Value]) -> Result<Vec<mysql_async::Value>, AppError> {
    values.iter().map(to_mysql).collect()
}

fn to_mysql(v: &Value) -> Result<mysql_async::Value, AppError> {
    use base64::Engine as _;
    use mysql_async::Value as M;
    Ok(match v {
        // `DEFAULT` cannot be bound; sqlgen emits the keyword instead of a placeholder.
        Value::Null | Value::Default => M::NULL,
        Value::Bool(b) => M::Int(*b as i64),
        Value::Int(i) => M::Int(*i),
        Value::BigInt(s) => match s.parse::<i64>() {
            Ok(i) => M::Int(i),
            Err(_) => match s.parse::<u64>() {
                Ok(u) => M::UInt(u),
                Err(_) => M::Bytes(s.clone().into_bytes()),
            },
        },
        Value::Float(f) => M::Double(*f),
        Value::Bytes(b) => M::Bytes(
            base64::engine::general_purpose::STANDARD
                .decode(&b.base64)
                .map_err(|e| AppError::InvalidInput(format!("invalid base64: {e}")))?,
        ),
        // Decimal/text/temporal/json/enum/other: MySQL coerces from the textual form.
        other => M::Bytes(other.as_text().unwrap_or_default().into_bytes()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mysql_async::Value as M;

    #[test]
    fn binds_each_variant() {
        let out = bind(&[
            Value::Null,
            Value::Default,
            Value::Bool(true),
            Value::Int(-3),
            Value::BigInt("9007199254740993".into()),
            Value::BigInt("18446744073709551615".into()),
            Value::BigInt("99999999999999999999".into()),
            Value::Float(1.5),
            Value::Bytes(crate::types::value::BytesValue {
                base64: "AP8B".into(),
                len: 3,
                truncated: false,
            }),
            Value::Decimal("1.50".into()),
            Value::Timestamp("2026-01-02 03:04:05".into()),
            Value::Json("{\"a\":1}".into()),
            Value::Enum("admin".into()),
        ])
        .unwrap();
        assert_eq!(out[0], M::NULL);
        assert_eq!(out[1], M::NULL);
        assert_eq!(out[2], M::Int(1));
        assert_eq!(out[3], M::Int(-3));
        assert_eq!(out[4], M::Int(9007199254740993));
        assert_eq!(out[5], M::UInt(u64::MAX));
        assert_eq!(out[6], M::Bytes(b"99999999999999999999".to_vec()));
        assert_eq!(out[7], M::Double(1.5));
        assert_eq!(out[8], M::Bytes(vec![0, 255, 1]));
        assert_eq!(out[9], M::Bytes(b"1.50".to_vec()));
        assert_eq!(out[10], M::Bytes(b"2026-01-02 03:04:05".to_vec()));
        assert_eq!(out[11], M::Bytes(b"{\"a\":1}".to_vec()));
        assert_eq!(out[12], M::Bytes(b"admin".to_vec()));
    }

    #[test]
    fn rejects_bad_base64() {
        let err = bind(&[Value::Bytes(crate::types::value::BytesValue {
            base64: "not base64!".into(),
            len: 0,
            truncated: false,
        })])
        .unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)));
    }
}
