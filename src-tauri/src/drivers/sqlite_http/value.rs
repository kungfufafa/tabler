//! Value mapping both remote engines share: SQLite's declared-type kinds and the rule that a
//! cell's storage class wins over its column's declared type (as `drivers::sqlite::decode`).

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE_NO_PAD};

use crate::types::query::ResultColumn;
use crate::types::schema::ValueKind;
use crate::types::value::Value;

// Declared types read as the local driver reads them (SQLite's affinity plus the DATE / DATETIME /
// BOOLEAN / JSON conventions of ORMs).
pub(crate) use crate::drivers::sqlite::decode::kind_from_decltype;

/// An integer cell: a BOOLEAN column shows true / false, anything else the number.
pub fn integer_value(n: i64, kind: ValueKind) -> Value {
    match kind {
        ValueKind::Bool => Value::Bool(n != 0),
        _ => Value::from_i64(n),
    }
}

/// A text cell, typed by its column's kind (dates, JSON and decimals keep their text).
pub fn text_value(s: String, kind: ValueKind) -> Value {
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

/// A blob cell: a preview capped like every other driver's, or the whole value (`fetch_cell`).
pub fn bytes_value(bytes: &[u8], full: bool) -> Value {
    if full {
        Value::from_bytes_uncapped(bytes)
    } else {
        Value::from_bytes(bytes)
    }
}

/// Base64 from a server, padded or not (Hrana clients strip the padding), standard or URL-safe
/// alphabet. `None` when it is not base64 at all.
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let trimmed = text.trim().trim_end_matches('=');
    STANDARD_NO_PAD
        .decode(trimmed)
        .or_else(|_| URL_SAFE_NO_PAD.decode(trimmed))
        .ok()
}

/// Base64 the way Hrana clients send blobs: standard alphabet, no padding.
pub fn encode_base64(bytes: &[u8]) -> String {
    STANDARD_NO_PAD.encode(bytes)
}

/// The bytes of a `Value::Bytes` parameter. A preview cut at the cap cannot be sent back.
pub fn param_bytes(b: &crate::types::value::BytesValue) -> Result<Vec<u8>, crate::error::AppError> {
    if b.truncated {
        return Err(crate::error::AppError::InvalidInput(
            "a truncated binary preview cannot be written back; load the full value first".into(),
        ));
    }
    STANDARD
        .decode(&b.base64)
        .map_err(|e| crate::error::AppError::InvalidInput(format!("invalid base64: {e}")))
}

/// `INSERT` or `REPLACE`, the statements whose rowid the local driver reports (a connection's
/// last rowid is stale after anything else).
pub fn inserts(sql: &str) -> bool {
    matches!(
        crate::sqlgen::split::leading_keywords(sql, 1)
            .first()
            .map(String::as_str),
        Some("INSERT" | "REPLACE")
    )
}

/// A result column known only by its name and declared type: neither engine reports the base
/// table of a result column, so editor results stay read-only.
pub fn result_column(name: Option<&str>, decltype: Option<&str>) -> ResultColumn {
    ResultColumn {
        name: name.unwrap_or("").to_string(),
        data_type: decltype.unwrap_or("").to_string(),
        kind: kind_from_decltype(decltype),
        nullable: None,
        table: None,
        origin: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_types_follow_sqlite_affinity() {
        let k = |d: &str| kind_from_decltype(Some(d));
        assert_eq!(kind_from_decltype(None), ValueKind::Other);
        assert_eq!(k(""), ValueKind::Other);
        assert_eq!(k("ANY"), ValueKind::Other);
        assert_eq!(k("INTEGER"), ValueKind::Int);
        assert_eq!(k("bigint"), ValueKind::Int);
        assert_eq!(k("VARCHAR(20)"), ValueKind::Text);
        assert_eq!(k("blob"), ValueKind::Bytes);
        assert_eq!(k("F32_BLOB(3)"), ValueKind::Bytes);
        assert_eq!(k("DOUBLE PRECISION"), ValueKind::Float);
        assert_eq!(k("NUMERIC(10,2)"), ValueKind::Decimal);
        assert_eq!(k("DATETIME"), ValueKind::Timestamp);
        assert_eq!(k("boolean"), ValueKind::Bool);
        assert_eq!(k("JSON"), ValueKind::Json);
        assert_eq!(k("uuid"), ValueKind::Uuid);
        assert_eq!(k("money"), ValueKind::Decimal);
    }

    #[test]
    fn cells_follow_their_kind() {
        assert_eq!(integer_value(1, ValueKind::Bool), Value::Bool(true));
        assert_eq!(
            integer_value(9_007_199_254_740_993, ValueKind::Int),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            text_value("2026-10-27".into(), ValueKind::Date),
            Value::Date("2026-10-27".into())
        );
        assert_eq!(
            text_value("abc".into(), ValueKind::Int),
            Value::Text("abc".into())
        );
    }

    #[test]
    fn base64_with_or_without_padding() {
        assert_eq!(decode_base64("AQID"), Some(vec![1, 2, 3]));
        assert_eq!(decode_base64("AQI="), Some(vec![1, 2]));
        assert_eq!(decode_base64("AQI"), Some(vec![1, 2]));
        assert_eq!(decode_base64("_-8"), Some(vec![255, 239]));
        assert_eq!(decode_base64(""), Some(vec![]));
        assert_eq!(decode_base64("!!"), None);
        assert_eq!(encode_base64(&[1, 2]), "AQI");
    }

    #[test]
    fn truncated_previews_are_not_parameters() {
        let Value::Bytes(mut b) = Value::from_bytes(&[1, 2, 3]) else {
            panic!("bytes");
        };
        assert_eq!(param_bytes(&b).unwrap(), vec![1, 2, 3]);
        b.truncated = true;
        assert!(param_bytes(&b).is_err());
    }
}
