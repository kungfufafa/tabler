//! MySQL result decoding: `Column` metadata → `ValueKind`, `mysql_async::Value` → `Value`.
//!
//! The text protocol (`query_iter`) delivers every cell as `Value::Bytes`; the binary protocol
//! (`exec_iter`, prepared statements) delivers typed `Int/UInt/Float/Double/Date/Time/Bytes`.
//! Both paths end in the same tagged `Value`.

use std::fmt::Write as _;

use mysql_async::Column;
use mysql_async::consts::{ColumnFlags, ColumnType};

use crate::types::query::ResultColumn;
use crate::types::schema::ValueKind;
use crate::types::value::Value;

/// Character set id of the `binary` pseudo-charset.
const BINARY_CHARSET: u16 = 63;

fn is_binary(col: &Column) -> bool {
    col.flags().contains(ColumnFlags::BINARY_FLAG) && col.character_set() == BINARY_CHARSET
}

/// Classifies a result column from its wire type and flags.
pub fn kind_from_column(col: &Column) -> ValueKind {
    use ColumnType::*;
    let flags = col.flags();
    if flags.contains(ColumnFlags::ENUM_FLAG) {
        return ValueKind::Enum;
    }
    if flags.contains(ColumnFlags::SET_FLAG) {
        return ValueKind::Set;
    }
    match col.column_type() {
        MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_INT24 | MYSQL_TYPE_LONG
        | MYSQL_TYPE_YEAR => ValueKind::Int,
        MYSQL_TYPE_LONGLONG => ValueKind::BigInt,
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => ValueKind::Decimal,
        MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE => ValueKind::Float,
        MYSQL_TYPE_BIT => ValueKind::Bit,
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => ValueKind::Date,
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => ValueKind::Time,
        MYSQL_TYPE_DATETIME
        | MYSQL_TYPE_DATETIME2
        | MYSQL_TYPE_TIMESTAMP
        | MYSQL_TYPE_TIMESTAMP2 => ValueKind::Timestamp,
        MYSQL_TYPE_JSON => ValueKind::Json,
        MYSQL_TYPE_GEOMETRY => ValueKind::Geometry,
        MYSQL_TYPE_ENUM => ValueKind::Enum,
        MYSQL_TYPE_SET => ValueKind::Set,
        MYSQL_TYPE_VARCHAR
        | MYSQL_TYPE_VAR_STRING
        | MYSQL_TYPE_STRING
        | MYSQL_TYPE_TINY_BLOB
        | MYSQL_TYPE_MEDIUM_BLOB
        | MYSQL_TYPE_LONG_BLOB
        | MYSQL_TYPE_BLOB => {
            if is_binary(col) {
                ValueKind::Bytes
            } else {
                ValueKind::Text
            }
        }
        MYSQL_TYPE_NULL => ValueKind::Other,
        _ => ValueKind::Other,
    }
}

/// Human-readable type name for a result column (`bigint unsigned`, `varchar`, `datetime`…).
pub fn type_name(col: &Column) -> String {
    use ColumnType::*;
    let flags = col.flags();
    let binary = is_binary(col);
    if flags.contains(ColumnFlags::ENUM_FLAG) {
        return "enum".into();
    }
    if flags.contains(ColumnFlags::SET_FLAG) {
        return "set".into();
    }
    let (base, numeric) = match col.column_type() {
        MYSQL_TYPE_TINY => ("tinyint", true),
        MYSQL_TYPE_SHORT => ("smallint", true),
        MYSQL_TYPE_INT24 => ("mediumint", true),
        MYSQL_TYPE_LONG => ("int", true),
        MYSQL_TYPE_LONGLONG => ("bigint", true),
        MYSQL_TYPE_YEAR => ("year", false),
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => ("decimal", true),
        MYSQL_TYPE_FLOAT => ("float", true),
        MYSQL_TYPE_DOUBLE => ("double", true),
        MYSQL_TYPE_BIT => ("bit", false),
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => ("date", false),
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => ("time", false),
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 => ("datetime", false),
        MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => ("timestamp", false),
        MYSQL_TYPE_JSON => ("json", false),
        MYSQL_TYPE_GEOMETRY => ("geometry", false),
        MYSQL_TYPE_ENUM => ("enum", false),
        MYSQL_TYPE_SET => ("set", false),
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING => {
            (if binary { "varbinary" } else { "varchar" }, false)
        }
        MYSQL_TYPE_STRING => (if binary { "binary" } else { "char" }, false),
        MYSQL_TYPE_TINY_BLOB => (if binary { "tinyblob" } else { "tinytext" }, false),
        MYSQL_TYPE_MEDIUM_BLOB => (if binary { "mediumblob" } else { "mediumtext" }, false),
        MYSQL_TYPE_LONG_BLOB => (if binary { "longblob" } else { "longtext" }, false),
        MYSQL_TYPE_BLOB => (if binary { "blob" } else { "text" }, false),
        MYSQL_TYPE_NULL => ("null", false),
        MYSQL_TYPE_VECTOR => ("vector", false),
        _ => ("unknown", false),
    };
    if numeric && flags.contains(ColumnFlags::UNSIGNED_FLAG) {
        format!("{base} unsigned")
    } else {
        base.to_string()
    }
}

/// Builds the `ResultColumn` description for a wire column.
pub fn result_column(col: &Column) -> ResultColumn {
    let table = col.table_str();
    ResultColumn {
        name: col.name_str().into_owned(),
        data_type: type_name(col),
        kind: kind_from_column(col),
        nullable: Some(!col.flags().contains(ColumnFlags::NOT_NULL_FLAG)),
        table: if table.is_empty() {
            None
        } else {
            Some(table.into_owned())
        },
    }
}

/// `YYYY-MM-DD HH:MM:SS[.ffffff]` (fraction only when non-zero).
pub fn format_datetime(y: u16, mo: u8, d: u8, h: u8, mi: u8, s: u8, us: u32) -> String {
    let mut out = format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}");
    if us > 0 {
        let _ = write!(out, ".{us:06}");
    }
    out
}

/// `[-]HH:MM:SS[.ffffff]` with days folded into hours (MySQL TIME spans -838..838 hours).
pub fn format_time(neg: bool, days: u32, h: u8, mi: u8, s: u8, us: u32) -> String {
    let hours = days as u64 * 24 + h as u64;
    let mut out = format!("{}{hours:02}:{mi:02}:{s:02}", if neg { "-" } else { "" });
    if us > 0 {
        let _ = write!(out, ".{us:06}");
    }
    out
}

/// `b'101'` from the raw big-endian bytes of a BIT value.
pub fn bit_literal(bytes: &[u8]) -> String {
    let mut bits = String::with_capacity(bytes.len() * 8);
    for b in bytes {
        let _ = write!(bits, "{b:08b}");
    }
    let trimmed = bits.trim_start_matches('0');
    let digits = if trimmed.is_empty() { "0" } else { trimmed };
    format!("b'{digits}'")
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02X}");
    }
    out
}

/// Decodes a text-protocol cell (raw bytes) according to the column kind.
fn value_from_text(kind: ValueKind, bytes: Vec<u8>) -> Value {
    match kind {
        ValueKind::Bytes => return Value::from_bytes(&bytes),
        ValueKind::Geometry => return Value::other("geometry", hex(&bytes)),
        ValueKind::Bit => return Value::other("bit", bit_literal(&bytes)),
        _ => {}
    }
    let s = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => return Value::from_bytes(e.as_bytes()),
    };
    match kind {
        ValueKind::Int | ValueKind::BigInt => match s.parse::<i64>() {
            Ok(i) => Value::from_i64(i),
            Err(_) => match s.parse::<u64>() {
                Ok(u) => Value::from_u64(u),
                Err(_) => Value::Text(s),
            },
        },
        ValueKind::Float => match s.parse::<f64>() {
            Ok(f) => Value::from_f64("double", f),
            Err(_) => Value::Text(s),
        },
        ValueKind::Bool => match s.as_str() {
            "0" => Value::Bool(false),
            "1" => Value::Bool(true),
            _ => Value::Text(s),
        },
        ValueKind::Decimal => Value::Decimal(s),
        ValueKind::Date => Value::Date(s),
        ValueKind::Time => Value::Time(s),
        ValueKind::Timestamp => Value::Timestamp(s),
        ValueKind::TimestampTz => Value::TimestampTz(s),
        ValueKind::Interval => Value::Interval(s),
        ValueKind::Uuid => Value::Uuid(s),
        ValueKind::Json => Value::Json(s),
        ValueKind::Enum => Value::Enum(s),
        // SET values travel as their comma-joined text; there is no dedicated variant.
        ValueKind::Set
        | ValueKind::Text
        | ValueKind::Array
        | ValueKind::Other
        | ValueKind::Bytes
        | ValueKind::Geometry
        | ValueKind::Bit => Value::Text(s),
    }
}

/// Converts one cell from either wire protocol into the app's `Value`.
pub fn value_from_mysql(kind: ValueKind, col: &Column, v: mysql_async::Value) -> Value {
    use mysql_async::Value as M;
    match v {
        M::NULL => Value::Null,
        M::Int(i) => match kind {
            ValueKind::Bit => Value::other("bit", bit_literal(&(i as u64).to_be_bytes())),
            ValueKind::Bool => Value::Bool(i != 0),
            _ => Value::from_i64(i),
        },
        M::UInt(u) => match kind {
            ValueKind::Bit => Value::other("bit", bit_literal(&u.to_be_bytes())),
            ValueKind::Bool => Value::Bool(u != 0),
            _ => Value::from_u64(u),
        },
        M::Float(f) => Value::from_f64(&type_name(col), f as f64),
        M::Double(d) => Value::from_f64(&type_name(col), d),
        M::Date(y, mo, d, h, mi, s, us) => {
            if kind == ValueKind::Date {
                Value::Date(format!("{y:04}-{mo:02}-{d:02}"))
            } else {
                Value::Timestamp(format_datetime(y, mo, d, h, mi, s, us))
            }
        }
        M::Time(neg, days, h, mi, s, us) => Value::Time(format_time(neg, days, h, mi, s, us)),
        M::Bytes(bytes) => value_from_text(kind, bytes),
    }
}

/// Lossy textual form of a raw value, for catalog queries whose cells are known to be scalar.
pub fn text_of(v: mysql_async::Value) -> Option<String> {
    use mysql_async::Value as M;
    match v {
        M::NULL => None,
        M::Bytes(b) => Some(String::from_utf8_lossy(&b).into_owned()),
        M::Int(i) => Some(i.to_string()),
        M::UInt(u) => Some(u.to_string()),
        M::Float(f) => Some(f.to_string()),
        M::Double(d) => Some(d.to_string()),
        M::Date(y, mo, d, h, mi, s, us) => Some(format_datetime(y, mo, d, h, mi, s, us)),
        M::Time(neg, days, h, mi, s, us) => Some(format_time(neg, days, h, mi, s, us)),
    }
}

/// Integer form of a raw value (text protocol digits, binary protocol ints).
pub fn int_of(v: mysql_async::Value) -> Option<i64> {
    use mysql_async::Value as M;
    match v {
        M::Int(i) => Some(i),
        M::UInt(u) => i64::try_from(u).ok(),
        M::Float(f) => Some(f as i64),
        M::Double(d) => Some(d as i64),
        M::Bytes(b) => std::str::from_utf8(&b).ok()?.trim().parse().ok(),
        M::NULL | M::Date(..) | M::Time(..) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(ty: ColumnType, flags: ColumnFlags, charset: u16) -> Column {
        Column::new(ty)
            .with_flags(flags)
            .with_character_set(charset)
    }

    #[test]
    fn classifies_wire_columns() {
        use ColumnType::*;
        let f = ColumnFlags::empty();
        assert_eq!(
            kind_from_column(&col(MYSQL_TYPE_LONG, f, 63)),
            ValueKind::Int
        );
        assert_eq!(
            kind_from_column(&col(MYSQL_TYPE_LONGLONG, ColumnFlags::UNSIGNED_FLAG, 63)),
            ValueKind::BigInt
        );
        assert_eq!(
            kind_from_column(&col(MYSQL_TYPE_NEWDECIMAL, f, 63)),
            ValueKind::Decimal
        );
        assert_eq!(
            kind_from_column(&col(MYSQL_TYPE_VAR_STRING, f, 255)),
            ValueKind::Text
        );
        assert_eq!(
            kind_from_column(&col(MYSQL_TYPE_BLOB, ColumnFlags::BINARY_FLAG, 63)),
            ValueKind::Bytes
        );
        // utf8 text column that happens to carry BINARY_FLAG (e.g. `_bin` collation) stays text.
        assert_eq!(
            kind_from_column(&col(MYSQL_TYPE_VAR_STRING, ColumnFlags::BINARY_FLAG, 255)),
            ValueKind::Text
        );
        assert_eq!(
            kind_from_column(&col(MYSQL_TYPE_STRING, ColumnFlags::ENUM_FLAG, 255)),
            ValueKind::Enum
        );
        assert_eq!(
            kind_from_column(&col(MYSQL_TYPE_STRING, ColumnFlags::SET_FLAG, 255)),
            ValueKind::Set
        );
        assert_eq!(
            kind_from_column(&col(MYSQL_TYPE_JSON, f, 63)),
            ValueKind::Json
        );
        assert_eq!(
            kind_from_column(&col(MYSQL_TYPE_GEOMETRY, f, 63)),
            ValueKind::Geometry
        );
        assert_eq!(
            kind_from_column(&col(MYSQL_TYPE_BIT, f, 63)),
            ValueKind::Bit
        );
        assert_eq!(
            kind_from_column(&col(MYSQL_TYPE_NULL, f, 63)),
            ValueKind::Other
        );
        assert_eq!(
            type_name(&col(MYSQL_TYPE_LONGLONG, ColumnFlags::UNSIGNED_FLAG, 63)),
            "bigint unsigned"
        );
        assert_eq!(
            type_name(&col(MYSQL_TYPE_BLOB, ColumnFlags::BINARY_FLAG, 63)),
            "blob"
        );
        assert_eq!(type_name(&col(MYSQL_TYPE_BLOB, f, 255)), "text");
    }

    #[test]
    fn decodes_text_protocol_bytes_by_kind() {
        use ColumnType::*;
        use mysql_async::Value as M;
        let f = ColumnFlags::empty();
        let b = |s: &str| M::Bytes(s.as_bytes().to_vec());

        let c = col(MYSQL_TYPE_LONG, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Int, &c, b("42")),
            Value::Int(42)
        );
        let c = col(MYSQL_TYPE_LONGLONG, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::BigInt, &c, b("9007199254740993")),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            value_from_mysql(ValueKind::BigInt, &c, b("18446744073709551615")),
            Value::BigInt("18446744073709551615".into())
        );
        let c = col(MYSQL_TYPE_NEWDECIMAL, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Decimal, &c, b("1.50")),
            Value::Decimal("1.50".into())
        );
        let c = col(MYSQL_TYPE_DOUBLE, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Float, &c, b("2.5")),
            Value::Float(2.5)
        );
        let c = col(MYSQL_TYPE_DATETIME, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Timestamp, &c, b("2026-01-02 03:04:05.000000")),
            Value::Timestamp("2026-01-02 03:04:05.000000".into())
        );
        let c = col(MYSQL_TYPE_DATE, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Date, &c, b("2026-01-02")),
            Value::Date("2026-01-02".into())
        );
        let c = col(MYSQL_TYPE_TIME, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Time, &c, b("-01:02:03")),
            Value::Time("-01:02:03".into())
        );
        let c = col(MYSQL_TYPE_JSON, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Json, &c, b("{\"a\": 1}")),
            Value::Json("{\"a\": 1}".into())
        );
        let c = col(MYSQL_TYPE_STRING, ColumnFlags::ENUM_FLAG, 255);
        assert_eq!(
            value_from_mysql(ValueKind::Enum, &c, b("admin")),
            Value::Enum("admin".into())
        );
        let c = col(MYSQL_TYPE_STRING, ColumnFlags::SET_FLAG, 255);
        assert_eq!(
            value_from_mysql(ValueKind::Set, &c, b("a,b")),
            Value::Text("a,b".into())
        );
        let c = col(MYSQL_TYPE_BLOB, ColumnFlags::BINARY_FLAG, 63);
        match value_from_mysql(ValueKind::Bytes, &c, M::Bytes(vec![0, 255, 1])) {
            Value::Bytes(bv) => {
                assert_eq!(bv.len, 3);
                assert_eq!(bv.base64, "AP8B");
                assert!(!bv.truncated);
            }
            other => panic!("expected bytes, got {other:?}"),
        }
        let c = col(MYSQL_TYPE_BIT, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Bit, &c, M::Bytes(vec![0b101])),
            Value::other("bit", "b'101'")
        );
        assert_eq!(
            value_from_mysql(ValueKind::Bit, &c, M::Bytes(vec![0])),
            Value::other("bit", "b'0'")
        );
        let c = col(MYSQL_TYPE_GEOMETRY, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Geometry, &c, M::Bytes(vec![0x00, 0xAB])),
            Value::other("geometry", "00AB")
        );
        // Unparseable numbers fall back to text instead of panicking.
        let c = col(MYSQL_TYPE_LONG, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Int, &c, b("abc")),
            Value::Text("abc".into())
        );
        assert_eq!(value_from_mysql(ValueKind::Text, &c, M::NULL), Value::Null);
    }

    #[test]
    fn decodes_binary_protocol_typed_values() {
        use ColumnType::*;
        use mysql_async::Value as M;
        let f = ColumnFlags::empty();
        let c = col(MYSQL_TYPE_LONGLONG, ColumnFlags::UNSIGNED_FLAG, 63);
        assert_eq!(
            value_from_mysql(ValueKind::BigInt, &c, M::UInt(u64::MAX)),
            Value::BigInt(u64::MAX.to_string())
        );
        assert_eq!(
            value_from_mysql(ValueKind::Int, &c, M::Int(-7)),
            Value::Int(-7)
        );
        let c = col(MYSQL_TYPE_FLOAT, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Float, &c, M::Float(1.5)),
            Value::Float(1.5)
        );
        let c = col(MYSQL_TYPE_DATETIME, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Timestamp, &c, M::Date(2026, 1, 2, 3, 4, 5, 0)),
            Value::Timestamp("2026-01-02 03:04:05".into())
        );
        assert_eq!(
            value_from_mysql(ValueKind::Timestamp, &c, M::Date(2026, 1, 2, 3, 4, 5, 120)),
            Value::Timestamp("2026-01-02 03:04:05.000120".into())
        );
        let c = col(MYSQL_TYPE_DATE, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Date, &c, M::Date(2026, 1, 2, 0, 0, 0, 0)),
            Value::Date("2026-01-02".into())
        );
        let c = col(MYSQL_TYPE_TIME, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Time, &c, M::Time(true, 0, 1, 2, 3, 0)),
            Value::Time("-01:02:03".into())
        );
        assert_eq!(
            value_from_mysql(ValueKind::Time, &c, M::Time(false, 2, 3, 4, 5, 6)),
            Value::Time("51:04:05.000006".into())
        );
        let c = col(MYSQL_TYPE_BIT, f, 63);
        assert_eq!(
            value_from_mysql(ValueKind::Bit, &c, M::UInt(6)),
            Value::other("bit", "b'110'")
        );
    }

    #[test]
    fn scalar_helpers() {
        use mysql_async::Value as M;
        assert_eq!(text_of(M::Bytes(b"abc".to_vec())).as_deref(), Some("abc"));
        assert_eq!(text_of(M::Int(5)).as_deref(), Some("5"));
        assert_eq!(text_of(M::NULL), None);
        assert_eq!(int_of(M::Bytes(b"12".to_vec())), Some(12));
        assert_eq!(int_of(M::UInt(7)), Some(7));
        assert_eq!(int_of(M::UInt(u64::MAX)), None);
        assert_eq!(int_of(M::NULL), None);
    }
}
