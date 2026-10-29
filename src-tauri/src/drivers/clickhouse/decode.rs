//! Cells of a JSON result row to Tabler `Value`s, by the column's ClickHouse type.
//!
//! The server is asked to quote 64-bit and wider integers, decimals and non-finite floats
//! (`request::OUTPUT_SETTINGS`), but a user whose profile is `readonly=1` cannot change those
//! settings, so every cell is read both ways: a number as written or as a string. Nothing here
//! panics on what the server sends; a cell that does not fit its type keeps its text in
//! `Value::Other`.

use crate::types::query::ResultColumn;
use crate::types::value::Value;

use super::json::JsonVal;
use super::types::ChType;

/// The result column of a name and a ClickHouse type text; the type text (with its time zone,
/// `DateTime64(3, 'Asia/Jakarta')`) is what the grid shows as the column's type.
pub fn result_column(name: &str, type_text: &str) -> (ResultColumn, ChType) {
    let ty = ChType::parse(type_text);
    let column = ResultColumn {
        name: name.to_string(),
        data_type: type_text.to_string(),
        kind: ty.kind(),
        nullable: Some(ty.is_nullable()),
        table: None,
        origin: None,
    };
    (column, ty)
}

/// One cell.
pub fn decode(ty: &ChType, v: &JsonVal) -> Value {
    if matches!(v, JsonVal::Null) {
        return Value::Null;
    }
    match ty {
        ChType::Nullable(t) | ChType::LowCardinality(t) | ChType::SimpleAggregate(_, t) => {
            decode(t, v)
        }
        ChType::Int { bits, signed } => int(ty, *bits, *signed, v),
        ChType::Float { .. } => float(ty, v),
        ChType::Decimal { .. } => match v.as_scalar_text() {
            Some(t) if is_decimal(t) => Value::Decimal(t.to_string()),
            _ => other(ty, v),
        },
        ChType::Bool => match v {
            JsonVal::Bool(b) => Value::Bool(*b),
            JsonVal::Num(n) if n == "0" || n == "1" => Value::Bool(n == "1"),
            JsonVal::Str(s) if s == "true" || s == "false" => Value::Bool(s == "true"),
            _ => other(ty, v),
        },
        ChType::String | ChType::FixedString(_) => match v {
            JsonVal::Str(s) => Value::Text(s.clone()),
            other => Value::Text(other.to_text()),
        },
        ChType::Uuid => text_or_other(ty, v, Value::Uuid),
        ChType::Date | ChType::Date32 => text_or_other(ty, v, Value::Date),
        ChType::DateTime { .. } | ChType::DateTime64 { .. } => {
            text_or_other(ty, v, Value::Timestamp)
        }
        ChType::Time { .. } => text_or_other(ty, v, Value::Time),
        ChType::Enum { values, .. } => match v {
            JsonVal::Str(s) => Value::Enum(s.clone()),
            // Some settings send an enum's number: its label, when declared.
            JsonVal::Num(n) => match n
                .parse::<i64>()
                .ok()
                .and_then(|n| values.iter().find(|(_, k)| *k == n))
            {
                Some((label, _)) => Value::Enum(label.clone()),
                None => other(ty, v),
            },
            _ => other(ty, v),
        },
        ChType::Array(elem) => match v {
            JsonVal::Arr(items) => Value::Array(items.iter().map(|i| decode(elem, i)).collect()),
            _ => other(ty, v),
        },
        ChType::Tuple(_)
        | ChType::Map(..)
        | ChType::Nested(_)
        | ChType::Json
        | ChType::Dynamic
        | ChType::Variant(_) => Value::Json(v.to_text()),
        ChType::Nothing => Value::Null,
        ChType::Ipv4
        | ChType::Ipv6
        | ChType::Interval(_)
        | ChType::Geo(_)
        | ChType::AggregateFunction(_)
        | ChType::Other(_) => other(ty, v),
    }
}

/// `Value::Other` with the cell's text (a string as it is, anything else as JSON).
fn other(ty: &ChType, v: &JsonVal) -> Value {
    let text = match v {
        JsonVal::Str(s) | JsonVal::Num(s) => s.clone(),
        v => v.to_text(),
    };
    Value::other(&ty.label(), text)
}

fn text_or_other(ty: &ChType, v: &JsonVal, f: fn(String) -> Value) -> Value {
    match v {
        JsonVal::Str(s) => f(s.clone()),
        // A DateTime sent as epoch seconds (`date_time_output_format = 'unix_timestamp'` in the
        // user's profile) is still that column's value.
        JsonVal::Num(n) => f(n.clone()),
        _ => other(ty, v),
    }
}

fn int(ty: &ChType, bits: u16, signed: bool, v: &JsonVal) -> Value {
    let Some(text) = v.as_scalar_text() else {
        // `Bool` columns of old servers come as true/false in an integer context.
        return match v {
            JsonVal::Bool(b) => Value::Int(i64::from(*b)),
            _ => other(ty, v),
        };
    };
    if bits <= 64 {
        if let Ok(n) = text.parse::<i64>() {
            return Value::from_i64(n);
        }
        if !signed && let Ok(n) = text.parse::<u64>() {
            return Value::from_u64(n);
        }
    }
    if is_integer(text) {
        Value::BigInt(text.trim_start_matches('+').to_string())
    } else {
        other(ty, v)
    }
}

fn float(ty: &ChType, v: &JsonVal) -> Value {
    let Some(text) = v.as_scalar_text() else {
        return other(ty, v);
    };
    match text.parse::<f64>() {
        Ok(f) if f.is_finite() => Value::Float(f),
        // `nan`, `inf`, `-inf`, as the server spelled them.
        _ => other(ty, v),
    }
}

/// An optional sign and digits.
fn is_integer(t: &str) -> bool {
    let digits = t.strip_prefix(['-', '+']).unwrap_or(t);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// An optional sign, digits, and at most one `.` with digits on a side.
fn is_decimal(t: &str) -> bool {
    let body = t.strip_prefix('-').unwrap_or(t);
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    !(int.is_empty() && frac.is_empty())
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::clickhouse::json::parse;
    use crate::types::schema::ValueKind;

    fn cell(ty: &str, json: &str) -> Value {
        decode(&ChType::parse(ty), &parse(json).unwrap())
    }

    #[test]
    fn integers_past_two_to_the_53_are_big_ints() {
        assert_eq!(cell("UInt8", "255"), Value::Int(255));
        assert_eq!(cell("Int64", "\"-42\""), Value::Int(-42));
        assert_eq!(
            cell("Int64", "\"9007199254740993\""),
            Value::BigInt("9007199254740993".into())
        );
        // Unquoted too (a readonly=1 profile keeps the server's defaults).
        assert_eq!(
            cell("UInt64", "18446744073709551615"),
            Value::BigInt("18446744073709551615".into())
        );
        assert_eq!(
            cell("Int64", "9007199254740991"),
            Value::Int(9007199254740991)
        );
        let max256 =
            "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        assert_eq!(
            cell("UInt256", &format!("\"{max256}\"")),
            Value::BigInt(max256.into())
        );
        assert_eq!(
            cell("Int128", "-170141183460469231731687303715884105728"),
            Value::BigInt("-170141183460469231731687303715884105728".into())
        );
        assert_eq!(cell("Int32", "\"x\""), Value::other("Int32", "x"));
        assert_eq!(cell("UInt8", "true"), Value::Int(1));
    }

    #[test]
    fn floats_decimals_and_bools() {
        assert_eq!(cell("Float64", "1.5"), Value::Float(1.5));
        assert_eq!(cell("Float32", "\"nan\""), Value::other("Float32", "nan"));
        assert_eq!(cell("Float64", "\"-inf\""), Value::other("Float64", "-inf"));
        assert_eq!(cell("Float64", "inf"), Value::other("Float64", "inf"));
        assert_eq!(cell("Float64", "null"), Value::Null);
        assert_eq!(
            cell("Decimal(38, 10)", "\"12345678901234567890.0123456789\""),
            Value::Decimal("12345678901234567890.0123456789".into())
        );
        assert_eq!(
            cell("Decimal(9, 2)", "-1.50"),
            Value::Decimal("-1.50".into())
        );
        assert_eq!(
            cell("Decimal(9, 2)", "\"1e5\""),
            Value::other("Decimal", "1e5")
        );
        assert_eq!(cell("Bool", "true"), Value::Bool(true));
        assert_eq!(cell("Bool", "0"), Value::Bool(false));
    }

    #[test]
    fn text_times_and_enums() {
        assert_eq!(
            cell("LowCardinality(Nullable(String))", "\"a\""),
            Value::Text("a".into())
        );
        assert_eq!(
            cell("LowCardinality(Nullable(String))", "null"),
            Value::Null
        );
        assert_eq!(
            cell("FixedString(3)", "\"ab\\u0000\""),
            Value::Text("ab\0".into())
        );
        assert_eq!(
            cell(
                "DateTime64(3, 'Asia/Jakarta')",
                "\"2024-01-02 03:04:05.678\""
            ),
            Value::Timestamp("2024-01-02 03:04:05.678".into())
        );
        assert_eq!(
            cell("Date32", "\"1900-01-01\""),
            Value::Date("1900-01-01".into())
        );
        assert_eq!(
            cell("DateTime", "1700000000"),
            Value::Timestamp("1700000000".into())
        );
        assert_eq!(
            cell("UUID", "\"61f0c404-5cb3-11e7-907b-a6006ad3dba0\""),
            Value::Uuid("61f0c404-5cb3-11e7-907b-a6006ad3dba0".into())
        );
        assert_eq!(
            cell("Enum8('a' = 1, 'b' = 2)", "\"b\""),
            Value::Enum("b".into())
        );
        assert_eq!(
            cell("Enum8('a' = 1, 'b' = 2)", "2"),
            Value::Enum("b".into())
        );
        assert_eq!(cell("Enum8('a' = 1)", "7"), Value::other("Enum8", "7"));
        assert_eq!(
            cell("IPv4", "\"10.0.0.1\""),
            Value::other("IPv4", "10.0.0.1")
        );
        assert_eq!(cell("IPv6", "\"::1\""), Value::other("IPv6", "::1"));
        assert_eq!(cell("IntervalDay", "3"), Value::other("IntervalDay", "3"));
    }

    #[test]
    fn nested_values() {
        assert_eq!(
            cell(
                "Array(Nullable(Int64))",
                "[\"1\", null, \"9007199254740993\"]"
            ),
            Value::Array(vec![
                Value::Int(1),
                Value::Null,
                Value::BigInt("9007199254740993".into())
            ])
        );
        assert_eq!(
            cell("Map(String, UInt64)", "{\"b\":\"2\",\"a\":\"1\"}"),
            Value::Json("{\"b\":\"2\",\"a\":\"1\"}".into())
        );
        assert_eq!(
            cell("Tuple(a UInt8, b String)", "{\"a\":1,\"b\":\"x\"}"),
            Value::Json("{\"a\":1,\"b\":\"x\"}".into())
        );
        assert_eq!(
            cell("Nested(id UInt32, kind Enum8('a' = 1))", "[[1,\"a\"]]"),
            Value::Json("[[1,\"a\"]]".into())
        );
        assert_eq!(
            cell(
                "JSON",
                "{\"a\":{\"b\":340282366920938463463374607431768211455}}"
            ),
            Value::Json("{\"a\":{\"b\":340282366920938463463374607431768211455}}".into())
        );
        assert_eq!(cell("Point", "[1.5,2]"), Value::other("Point", "[1.5,2]"));
        assert_eq!(
            cell("AggregateFunction(uniq, UInt64)", "\"\\u0001\\u0002\""),
            Value::other("AggregateFunction", "\u{1}\u{2}")
        );
        assert_eq!(
            cell("Array(String)", "\"oops\""),
            Value::other("Array", "oops")
        );
        assert_eq!(cell("Nothing", "null"), Value::Null);
        assert_eq!(cell("Mystery(1)", "\"x\""), Value::other("Mystery", "x"));
    }

    #[test]
    fn result_columns_carry_the_full_type() {
        let (c, ty) = result_column("ts", "Nullable(DateTime64(3, 'UTC'))");
        assert_eq!(c.data_type, "Nullable(DateTime64(3, 'UTC'))");
        assert_eq!(c.kind, ValueKind::Timestamp);
        assert_eq!(c.nullable, Some(true));
        assert!(ty.is_nullable());
    }
}
