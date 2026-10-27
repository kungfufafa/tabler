//! CQL values, as the driver decodes a row untyped (`Option<CqlValue>` per column), to Tabler's
//! `Value`, and result column metadata to `ResultColumn`.
//!
//! Total: nothing here panics on what a server sends. Integers beyond 2^53 (bigint, counter,
//! varint) are `BigInt` text, decimals their exact text (negative ones too), timestamps UTC text
//! with milliseconds (a key match needs them), dates over the whole CQL range, durations in
//! Cassandra's `1y2mo3d4h…` form. Collections, tuples, UDTs and vectors are JSON text in the
//! shape of Cassandra's `toJson()` (map keys quoted as its `MapType` does; scalars in the texts
//! above); inet addresses and empty values are `Other`.

use std::fmt::Write as _;

use ::scylla::frame::response::result::ColumnSpec;
use ::scylla::value::{CqlDuration, CqlValue};
use num_bigint::{BigInt, Sign};

use crate::drivers::cassandra::types::{column_kind, column_type_text};
use crate::types::query::ResultColumn;
use crate::types::value::Value;

/// A column of a row: `None` is NULL.
pub fn decode(v: Option<&CqlValue>) -> Value {
    v.map_or(Value::Null, decode_value)
}

/// A row, column by column.
pub fn decode_row(columns: &[Option<CqlValue>]) -> Vec<Value> {
    columns.iter().map(|c| decode(c.as_ref())).collect()
}

/// A value whole: a blob is not cut at the preview cap (`fetch_cell`, whose caller bounds the
/// size).
pub fn decode_uncapped(v: Option<&CqlValue>) -> Value {
    match v {
        Some(CqlValue::Blob(b)) => Value::from_bytes_uncapped(b),
        v => decode(v),
    }
}

pub fn decode_value(v: &CqlValue) -> Value {
    match v {
        CqlValue::Ascii(s) | CqlValue::Text(s) => Value::Text(s.clone()),
        CqlValue::Boolean(b) => Value::Bool(*b),
        CqlValue::Blob(b) => Value::from_bytes(b),
        CqlValue::Counter(c) => Value::from_i64(c.0),
        CqlValue::Decimal(d) => {
            let (bytes, scale) = d.as_signed_be_bytes_slice_and_exponent();
            Value::Decimal(decimal_text(bytes, scale))
        }
        CqlValue::Date(d) => Value::Date(date_text(d.0)),
        CqlValue::Double(f) => Value::from_f64("double", *f),
        CqlValue::Duration(d) => match duration_text(*d) {
            Some(text) => Value::Interval(text),
            None => Value::other("duration", raw_duration(*d)),
        },
        CqlValue::Empty => Value::other("empty", ""),
        CqlValue::Float(f) => Value::from_f64("float", f32_as_f64(*f)),
        CqlValue::Int(i) => Value::Int(i64::from(*i)),
        CqlValue::BigInt(i) => Value::from_i64(*i),
        CqlValue::Timestamp(ts) => Value::TimestampTz(timestamp_text(ts.0)),
        CqlValue::Inet(ip) => Value::other("inet", ip.to_string()),
        CqlValue::SmallInt(i) => Value::Int(i64::from(*i)),
        CqlValue::TinyInt(i) => Value::Int(i64::from(*i)),
        CqlValue::Time(t) => match time_text(t.0) {
            Some(text) => Value::Time(text),
            None => Value::other("time", t.0.to_string()),
        },
        CqlValue::Timeuuid(u) => Value::Uuid(u.to_string()),
        CqlValue::Uuid(u) => Value::Uuid(u.to_string()),
        CqlValue::Varint(v) => {
            let n = BigInt::from_signed_bytes_be(v.as_signed_bytes_be_slice());
            match i64::try_from(&n) {
                Ok(i) => Value::from_i64(i),
                Err(_) => Value::BigInt(n.to_string()),
            }
        }
        CqlValue::List(_)
        | CqlValue::Set(_)
        | CqlValue::Map(_)
        | CqlValue::Tuple(_)
        | CqlValue::UserDefinedType { .. }
        | CqlValue::Vector(_) => Value::Json(json(v)),
        // A type a newer driver adds: its CQL-like text.
        other => Value::other("unknown", other.to_string()),
    }
}

/// The result column of a column spec. The origin stays unknown: CQL names the table of every
/// column, computed ones (`ttl(v)`, `count`) included.
pub fn result_column(spec: &ColumnSpec<'_>) -> ResultColumn {
    ResultColumn {
        name: spec.name().to_string(),
        data_type: column_type_text(spec.typ()),
        kind: column_kind(spec.typ()),
        nullable: None,
        table: Some(spec.table_spec().table_name().to_string()),
        origin: None,
    }
}

/// An f32 as the f64 its shortest text reads as (`0.1`, not `0.10000000149011612`).
fn f32_as_f64(f: f32) -> f64 {
    f.to_string().parse().unwrap_or(f64::from(f))
}

/// Plain decimal text up to this many padding zeros; beyond, `<digits>E<exponent>`, which
/// stays exact (a server may send any 32-bit scale).
const MAX_PLAIN_ZEROS: u64 = 100;

/// The exact text of `unscaled × 10^-scale` (`unscaled` in two's complement, big-endian), its
/// scale kept: `10.00`, `-0.05`, `1E+300`.
pub fn decimal_text(unscaled: &[u8], scale: i32) -> String {
    let n = BigInt::from_signed_bytes_be(unscaled);
    let sign = if n.sign() == Sign::Minus { "-" } else { "" };
    let digits = n.magnitude().to_string();
    let scale = i64::from(scale);
    if scale <= 0 {
        let zeros = scale.unsigned_abs();
        if digits == "0" {
            return "0".into();
        }
        if zeros <= MAX_PLAIN_ZEROS {
            return format!("{sign}{digits}{}", "0".repeat(zeros as usize));
        }
        return format!("{sign}{digits}E+{zeros}");
    }
    let scale = scale.unsigned_abs();
    let len = digits.len() as u64;
    if len > scale {
        let (int, frac) = digits.split_at((len - scale) as usize);
        format!("{sign}{int}.{frac}")
    } else if scale - len <= MAX_PLAIN_ZEROS {
        format!("{sign}0.{}{digits}", "0".repeat((scale - len) as usize))
    } else {
        format!("{sign}{digits}E-{scale}")
    }
}

/// The civil date of a day count from 1970-01-01 (Howard Hinnant's algorithm), for any `i64`
/// a CQL value can hold.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `YYYY`, or ISO 8601's expanded `+10000` / `-0001` outside 0000–9999.
fn year_text(y: i64) -> String {
    if (0..=9999).contains(&y) {
        format!("{y:04}")
    } else if y < 0 {
        format!("-{:04}", y.unsigned_abs())
    } else {
        format!("+{y}")
    }
}

/// A CQL date (days with the epoch at 2^31) as `YYYY-MM-DD`.
pub fn date_text(days: u32) -> String {
    let (y, m, d) = civil(i64::from(days) - (1 << 31));
    format!("{}-{m:02}-{d:02}", year_text(y))
}

/// A CQL timestamp (milliseconds since the epoch) as `YYYY-MM-DD HH:MM:SS.mmm+0000`, the form
/// cqlsh shows and every Cassandra and ScyllaDB parses.
pub fn timestamp_text(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let in_day = ms.rem_euclid(86_400_000);
    let (y, mo, d) = civil(days);
    let (h, mi, s, milli) = (
        in_day / 3_600_000,
        in_day / 60_000 % 60,
        in_day / 1000 % 60,
        in_day % 1000,
    );
    format!(
        "{}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{milli:03}+0000",
        year_text(y)
    )
}

/// A CQL time (nanoseconds since midnight) as `HH:MM:SS[.fff[fff[fff]]]`; None outside a day.
pub fn time_text(ns: i64) -> Option<String> {
    if !(0..86_400_000_000_000).contains(&ns) {
        return None;
    }
    let secs = ns / 1_000_000_000;
    let frac = ns % 1_000_000_000;
    let mut out = format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
    if frac != 0 {
        let digits = format!("{frac:09}");
        // Milli-, micro- or nanoseconds, whichever holds every digit.
        let keep = match digits.trim_end_matches('0').len() {
            0..=3 => 3,
            4..=6 => 6,
            _ => 9,
        };
        out.push('.');
        out.push_str(&digits[..keep]);
    }
    Some(out)
}

/// A duration as Cassandra writes it (`1y2mo3d4h5m6s7ms8us9ns`, `-1d`, `0s`); None when its
/// parts disagree in sign, which no server stores.
pub fn duration_text(d: CqlDuration) -> Option<String> {
    let negative = d.months < 0 || d.days < 0 || d.nanoseconds < 0;
    let positive = d.months > 0 || d.days > 0 || d.nanoseconds > 0;
    if negative && positive {
        return None;
    }
    if !negative && !positive {
        return Some("0s".into());
    }
    let mut out = String::from(if negative { "-" } else { "" });
    let mut part = |n: u64, unit: &str| {
        if n != 0 {
            let _ = write!(out, "{n}{unit}");
        }
    };
    let months = u64::from(d.months.unsigned_abs());
    part(months / 12, "y");
    part(months % 12, "mo");
    part(u64::from(d.days.unsigned_abs()), "d");
    let ns = d.nanoseconds.unsigned_abs();
    part(ns / 3_600_000_000_000, "h");
    part(ns / 60_000_000_000 % 60, "m");
    part(ns / 1_000_000_000 % 60, "s");
    part(ns / 1_000_000 % 1000, "ms");
    part(ns / 1000 % 1000, "us");
    part(ns % 1000, "ns");
    Some(out)
}

fn raw_duration(d: CqlDuration) -> String {
    format!("{}mo{}d{}ns", d.months, d.days, d.nanoseconds)
}

/// The JSON text of a value, in the shape of Cassandra's `toJson()`: collections and tuples are
/// arrays, maps and UDTs objects (a key whose JSON is a string is that string, any other is keyed
/// by its JSON text: `{"1": …, "[1, null]": …}`),
/// numbers exact, temporal values, uuids, addresses and blobs (`0x…`) strings, non-finite floats
/// strings, an empty value null.
pub fn json(v: &CqlValue) -> String {
    let mut out = String::new();
    write_json(v, &mut out);
    out
}

fn write_json(v: &CqlValue, out: &mut String) {
    match v {
        CqlValue::Ascii(s) | CqlValue::Text(s) => push_str(out, s),
        CqlValue::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
        CqlValue::Blob(b) => {
            let mut hex = String::with_capacity(2 + b.len() * 2);
            hex.push_str("0x");
            for byte in b {
                let _ = write!(hex, "{byte:02x}");
            }
            push_str(out, &hex);
        }
        CqlValue::Counter(c) => push_display(out, c.0),
        CqlValue::BigInt(i) => push_display(out, i),
        CqlValue::Int(i) => push_display(out, i),
        CqlValue::SmallInt(i) => push_display(out, i),
        CqlValue::TinyInt(i) => push_display(out, i),
        CqlValue::Varint(n) => push_display(
            out,
            BigInt::from_signed_bytes_be(n.as_signed_bytes_be_slice()),
        ),
        CqlValue::Decimal(d) => {
            let (bytes, scale) = d.as_signed_be_bytes_slice_and_exponent();
            out.push_str(&decimal_text(bytes, scale));
        }
        CqlValue::Float(f) => push_float(out, f32_as_f64(*f)),
        CqlValue::Double(f) => push_float(out, *f),
        CqlValue::Date(d) => push_str(out, &date_text(d.0)),
        CqlValue::Time(t) => push_str(out, &time_text(t.0).unwrap_or_else(|| t.0.to_string())),
        CqlValue::Timestamp(ts) => push_str(out, &timestamp_text(ts.0)),
        CqlValue::Duration(d) => {
            push_str(out, &duration_text(*d).unwrap_or_else(|| raw_duration(*d)))
        }
        CqlValue::Inet(ip) => push_str(out, &ip.to_string()),
        CqlValue::Uuid(u) => push_str(out, &u.to_string()),
        CqlValue::Timeuuid(u) => push_str(out, &u.to_string()),
        CqlValue::Empty => out.push_str("null"),
        CqlValue::List(items) | CqlValue::Set(items) | CqlValue::Vector(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_json(item, out);
            }
            out.push(']');
        }
        CqlValue::Tuple(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_opt(item.as_ref(), out);
            }
            out.push(']');
        }
        CqlValue::Map(pairs) => {
            out.push('{');
            for (i, (k, val)) in pairs.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                // As Cassandra's MapType: a key whose JSON is a string (text, uuid, timestamp,
                // inet, blob…) stays as it is, any other (a number, a tuple) becomes its text.
                let key = json(k);
                if key.starts_with('"') {
                    out.push_str(&key);
                } else {
                    push_str(out, &key);
                }
                out.push_str(": ");
                write_json(val, out);
            }
            out.push('}');
        }
        CqlValue::UserDefinedType { fields, .. } => {
            out.push('{');
            for (i, (name, val)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                push_str(out, name);
                out.push_str(": ");
                write_opt(val.as_ref(), out);
            }
            out.push('}');
        }
        other => push_str(out, &other.to_string()),
    }
}

fn write_opt(v: Option<&CqlValue>, out: &mut String) {
    match v {
        Some(v) => write_json(v, out),
        None => out.push_str("null"),
    }
}

fn push_display(out: &mut String, n: impl std::fmt::Display) {
    let _ = write!(out, "{n}");
}

fn push_float(out: &mut String, f: f64) {
    if f.is_finite() {
        // Rust's float text is plain decimal: valid JSON for any finite value.
        let _ = write!(out, "{f}");
    } else if f.is_nan() {
        push_str(out, "NaN");
    } else if f > 0.0 {
        push_str(out, "Infinity");
    } else {
        push_str(out, "-Infinity");
    }
}

/// A JSON string literal of `s`.
fn push_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::str::FromStr;

    use ::scylla::frame::response::result::{ColumnType, NativeType, TableSpec};
    use ::scylla::value::{
        Counter, CqlDate, CqlDecimal, CqlTime, CqlTimestamp, CqlTimeuuid, CqlVarint,
    };
    use uuid::Uuid;

    use super::*;
    use crate::types::schema::ValueKind;
    use crate::types::value::JS_SAFE_INT;

    fn dec(bytes: &[u8], scale: i32) -> CqlValue {
        CqlValue::Decimal(CqlDecimal::from_signed_be_bytes_slice_and_exponent(
            bytes, scale,
        ))
    }

    fn varint(n: i128) -> CqlValue {
        let big = BigInt::from(n);
        CqlValue::Varint(CqlVarint::from_signed_bytes_be(big.to_signed_bytes_be()))
    }

    #[test]
    fn scalars() {
        assert_eq!(decode(None), Value::Null);
        assert_eq!(
            decode_value(&CqlValue::Text("é'x".into())),
            Value::Text("é'x".into())
        );
        assert_eq!(
            decode_value(&CqlValue::Ascii("a".into())),
            Value::Text("a".into())
        );
        assert_eq!(decode_value(&CqlValue::Boolean(true)), Value::Bool(true));
        assert_eq!(decode_value(&CqlValue::TinyInt(-128)), Value::Int(-128));
        assert_eq!(decode_value(&CqlValue::SmallInt(32767)), Value::Int(32767));
        assert_eq!(
            decode_value(&CqlValue::Int(i32::MIN)),
            Value::Int(i64::from(i32::MIN))
        );
        assert_eq!(
            decode_value(&CqlValue::BigInt(JS_SAFE_INT)),
            Value::Int(JS_SAFE_INT)
        );
        assert_eq!(
            decode_value(&CqlValue::BigInt(JS_SAFE_INT + 1)),
            Value::BigInt("9007199254740992".into())
        );
        assert_eq!(
            decode_value(&CqlValue::Counter(Counter(i64::MIN))),
            Value::BigInt("-9223372036854775808".into())
        );
        assert_eq!(decode_value(&CqlValue::Float(0.1)), Value::Float(0.1));
        assert_eq!(decode_value(&CqlValue::Double(-2.5)), Value::Float(-2.5));
        assert_eq!(
            decode_value(&CqlValue::Double(f64::NAN)),
            Value::other("double", "NaN")
        );
        assert_eq!(
            decode_value(&CqlValue::Float(f32::INFINITY)),
            Value::other("float", "inf")
        );
        assert_eq!(
            decode_value(&CqlValue::Blob(vec![0xde, 0xad])),
            Value::from_bytes(&[0xde, 0xad])
        );
        let big = vec![7u8; crate::types::value::BYTES_PREVIEW_CAP + 1];
        assert!(matches!(
            decode_value(&CqlValue::Blob(big.clone())),
            Value::Bytes(b) if b.truncated
        ));
        assert!(matches!(
            decode_uncapped(Some(&CqlValue::Blob(big))),
            Value::Bytes(b) if !b.truncated
        ));
        assert_eq!(decode_value(&CqlValue::Empty), Value::other("empty", ""));
        assert_eq!(
            decode_value(&CqlValue::Inet(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))),
            Value::other("inet", "10.0.0.1")
        );
        assert_eq!(
            decode_value(&CqlValue::Inet(IpAddr::V6(Ipv6Addr::LOCALHOST))),
            Value::other("inet", "::1")
        );
        let id = "6ab09bec-e68e-48d9-a5f8-97e6fb4c9b47";
        assert_eq!(
            decode_value(&CqlValue::Uuid(Uuid::from_str(id).unwrap())),
            Value::Uuid(id.into())
        );
        let tid = "d2177dd0-eaa2-11de-a572-001b779c76e3";
        assert_eq!(
            decode_value(&CqlValue::Timeuuid(CqlTimeuuid::from_str(tid).unwrap())),
            Value::Uuid(tid.into())
        );
    }

    #[test]
    fn varints_and_decimals_are_exact() {
        assert_eq!(decode_value(&varint(0)), Value::Int(0));
        assert_eq!(decode_value(&varint(-1)), Value::Int(-1));
        assert_eq!(
            decode_value(&varint(i128::from(JS_SAFE_INT) + 1)),
            Value::BigInt("9007199254740992".into())
        );
        assert_eq!(
            decode_value(&varint(
                -170_141_183_460_469_231_731_687_303_715_884_105_728
            )),
            Value::BigInt("-170141183460469231731687303715884105728".into())
        );
        // 123.456, -0.05, 10.00, 0, 1E+3, -1234 with scale -2.
        assert_eq!(
            decode_value(&dec(&[0x01, 0xE2, 0x40], 3)),
            Value::Decimal("123.456".into())
        );
        assert_eq!(
            decode_value(&dec(&[0xFB], 2)),
            Value::Decimal("-0.05".into())
        );
        assert_eq!(
            decode_value(&dec(&[0x03, 0xE8], 2)),
            Value::Decimal("10.00".into())
        );
        assert_eq!(decode_value(&dec(&[], 5)), Value::Decimal("0.00000".into()));
        assert_eq!(decode_value(&dec(&[0x00], -3)), Value::Decimal("0".into()));
        assert_eq!(
            decode_value(&dec(&[0x01], -3)),
            Value::Decimal("1000".into())
        );
        assert_eq!(
            decode_value(&dec(&[0xFB, 0x2E], -2)),
            Value::Decimal("-123400".into())
        );
        // Extreme scales stay short and exact.
        assert_eq!(decimal_text(&[0x01], i32::MAX), "1E-2147483647");
        assert_eq!(decimal_text(&[0xFF], i32::MIN), "-1E+2147483648");
        assert_eq!(decimal_text(&[0x7F], 104), "127E-104");
        assert_eq!(decimal_text(&[0x7F], 103).len(), "0.".len() + 103);
        assert_eq!(
            decimal_text(&[0x7F], 5),
            "0.00127",
            "leading zeros of a small fraction"
        );
    }

    #[test]
    fn temporal_values() {
        let date = |days_from_epoch: i64| {
            decode_value(&CqlValue::Date(CqlDate(
                u32::try_from(days_from_epoch + (1 << 31)).unwrap(),
            )))
        };
        assert_eq!(date(0), Value::Date("1970-01-01".into()));
        assert_eq!(date(-1), Value::Date("1969-12-31".into()));
        assert_eq!(date(19_737), Value::Date("2024-01-15".into()));
        assert_eq!(date(-719_528), Value::Date("0000-01-01".into()));
        assert_eq!(date(-719_529), Value::Date("-0001-12-31".into()));
        assert_eq!(date(2_932_897), Value::Date("+10000-01-01".into()));
        // The two ends of the CQL range.
        assert_eq!(
            decode_value(&CqlValue::Date(CqlDate(0))),
            Value::Date("-5877641-06-23".into())
        );
        assert_eq!(
            decode_value(&CqlValue::Date(CqlDate(u32::MAX))),
            Value::Date("+5881580-07-11".into())
        );

        let ts = |ms: i64| decode_value(&CqlValue::Timestamp(CqlTimestamp(ms)));
        assert_eq!(
            ts(0),
            Value::TimestampTz("1970-01-01 00:00:00.000+0000".into())
        );
        assert_eq!(
            ts(1_705_314_600_123),
            Value::TimestampTz("2024-01-15 10:30:00.123+0000".into())
        );
        // Before 1970, the milliseconds count up from the second before.
        assert_eq!(
            ts(-1),
            Value::TimestampTz("1969-12-31 23:59:59.999+0000".into())
        );
        assert_eq!(
            ts(-62_135_596_800_001),
            Value::TimestampTz("0000-12-31 23:59:59.999+0000".into())
        );
        // No panic at either end of i64.
        assert!(matches!(ts(i64::MIN), Value::TimestampTz(t) if t.starts_with("-2922")));
        assert!(matches!(ts(i64::MAX), Value::TimestampTz(t) if t.starts_with("+2922")));

        let time = |ns: i64| decode_value(&CqlValue::Time(CqlTime(ns)));
        assert_eq!(time(0), Value::Time("00:00:00".into()));
        assert_eq!(time(45_296_123_000_000), Value::Time("12:34:56.123".into()));
        assert_eq!(
            time(45_296_000_100_000),
            Value::Time("12:34:56.000100".into())
        );
        assert_eq!(
            time(86_399_999_999_999),
            Value::Time("23:59:59.999999999".into())
        );
        assert_eq!(time(-1), Value::other("time", "-1"));
        assert_eq!(
            time(86_400_000_000_000),
            Value::other("time", "86400000000000")
        );
    }

    #[test]
    fn durations() {
        let d = |months, days, nanoseconds| {
            decode_value(&CqlValue::Duration(CqlDuration {
                months,
                days,
                nanoseconds,
            }))
        };
        assert_eq!(
            d(14, 3, 3_723_004_005_006),
            Value::Interval("1y2mo3d1h2m3s4ms5us6ns".into())
        );
        assert_eq!(d(0, 0, 0), Value::Interval("0s".into()));
        assert_eq!(d(0, -1, 0), Value::Interval("-1d".into()));
        assert_eq!(d(-1, -2, -3), Value::Interval("-1mo2d3ns".into()));
        assert_eq!(d(1, -2, 0), Value::other("duration", "1mo-2d0ns"));
        assert!(matches!(
            d(i32::MIN, i32::MIN, i64::MIN),
            Value::Interval(t) if t.starts_with("-178956970y8mo2147483648d")
        ));
    }

    #[test]
    fn nested_values_are_json() {
        let text = |s: &str| CqlValue::Text(s.into());
        assert_eq!(
            decode_value(&CqlValue::List(vec![text("a\"b"), text("c\n")])),
            Value::Json("[\"a\\\"b\", \"c\\n\"]".into())
        );
        assert_eq!(
            decode_value(&CqlValue::Set(vec![CqlValue::Int(1), CqlValue::Int(2)])),
            Value::Json("[1, 2]".into())
        );
        assert_eq!(
            decode_value(&CqlValue::Map(vec![
                (text("k"), CqlValue::BigInt(i64::MAX)),
                (text("d"), dec(&[0xFB], 2)),
            ])),
            Value::Json("{\"k\": 9223372036854775807, \"d\": -0.05}".into())
        );
        // A key that is not text is keyed by its JSON text.
        assert_eq!(
            decode_value(&CqlValue::Map(vec![
                (CqlValue::Int(1), text("a")),
                (
                    CqlValue::Tuple(vec![Some(CqlValue::Int(1)), None]),
                    CqlValue::Boolean(false)
                ),
            ])),
            Value::Json("{\"1\": \"a\", \"[1, null]\": false}".into())
        );
        // A key whose JSON is already a string is not quoted again (MapType.toJSONString).
        let id = "6ab09bec-e68e-48d9-a5f8-97e6fb4c9b47";
        let tid = "8e14e760-7fa8-11eb-bc66-000000000001";
        let key = |k: CqlValue| match decode_value(&CqlValue::Map(vec![(k, CqlValue::Int(1))])) {
            Value::Json(j) => j,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            key(CqlValue::Uuid(Uuid::from_str(id).unwrap())),
            format!("{{\"{id}\": 1}}")
        );
        assert_eq!(
            key(CqlValue::Timeuuid(CqlTimeuuid::from_str(tid).unwrap())),
            format!("{{\"{tid}\": 1}}")
        );
        assert_eq!(
            key(CqlValue::Timestamp(CqlTimestamp(0))),
            "{\"1970-01-01 00:00:00.000+0000\": 1}"
        );
        assert_eq!(key(CqlValue::Date(CqlDate(1 << 31))), "{\"1970-01-01\": 1}");
        assert_eq!(
            key(CqlValue::Time(CqlTime(45_296_123_000_000))),
            "{\"12:34:56.123\": 1}"
        );
        assert_eq!(
            key(CqlValue::Duration(CqlDuration {
                months: 0,
                days: 1,
                nanoseconds: 0
            })),
            "{\"1d\": 1}"
        );
        assert_eq!(
            key(CqlValue::Inet(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))),
            "{\"10.0.0.1\": 1}"
        );
        assert_eq!(
            key(CqlValue::Inet(IpAddr::V6(Ipv6Addr::LOCALHOST))),
            "{\"::1\": 1}"
        );
        assert_eq!(key(CqlValue::Blob(vec![0xca, 0xfe])), "{\"0xcafe\": 1}");
        assert_eq!(key(text("a\"b")), "{\"a\\\"b\": 1}");
        // Not strings: quoted once, their own quotes escaped.
        assert_eq!(key(CqlValue::Boolean(true)), "{\"true\": 1}");
        assert_eq!(key(dec(&[0xFB], 2)), "{\"-0.05\": 1}");
        assert_eq!(key(CqlValue::Float(f32::NAN)), "{\"NaN\": 1}");
        assert_eq!(key(CqlValue::List(vec![text("x")])), "{\"[\\\"x\\\"]\": 1}");
        // Every key above is a JSON string that holds the key's own text.
        let parsed: serde_json::Value =
            serde_json::from_str(&key(CqlValue::Uuid(Uuid::from_str(id).unwrap()))).unwrap();
        assert_eq!(parsed[id], serde_json::json!(1));
        let udt = CqlValue::UserDefinedType {
            keyspace: "ks".into(),
            name: "address".into(),
            fields: vec![
                ("city".into(), Some(text("Jakarta"))),
                ("zip".into(), None),
                (
                    "geo".into(),
                    Some(CqlValue::UserDefinedType {
                        keyspace: "ks".into(),
                        name: "point".into(),
                        fields: vec![("lat".into(), Some(CqlValue::Double(-6.2)))],
                    }),
                ),
            ],
        };
        assert_eq!(
            decode_value(&udt),
            Value::Json("{\"city\": \"Jakarta\", \"zip\": null, \"geo\": {\"lat\": -6.2}}".into())
        );
        assert_eq!(
            decode_value(&CqlValue::Tuple(vec![
                Some(CqlValue::Timestamp(CqlTimestamp(0))),
                Some(CqlValue::Blob(vec![0xca, 0xfe])),
                Some(CqlValue::Float(f32::NAN)),
                Some(CqlValue::Empty),
                Some(varint(-(1 << 70))),
            ])),
            Value::Json(
                "[\"1970-01-01 00:00:00.000+0000\", \"0xcafe\", \"NaN\", null, \
                 -1180591620717411303424]"
                    .into()
            )
        );
        assert_eq!(
            decode_value(&CqlValue::Vector(vec![
                CqlValue::Float(0.5),
                CqlValue::Float(-1.0)
            ])),
            Value::Json("[0.5, -1]".into())
        );
        // Whatever the JSON holds, it parses.
        let parsed: serde_json::Value = match decode_value(&udt) {
            Value::Json(j) => serde_json::from_str(&j).unwrap(),
            other => panic!("{other:?}"),
        };
        assert_eq!(parsed["geo"]["lat"], serde_json::json!(-6.2));
        assert_eq!(
            json(&text("\u{1}")),
            "\"\\u0001\"",
            "control characters are escaped"
        );
    }

    #[test]
    fn rows_and_result_columns() {
        assert_eq!(
            decode_row(&[Some(CqlValue::Int(1)), None]),
            [Value::Int(1), Value::Null]
        );
        let spec = ColumnSpec::borrowed(
            "ts",
            ColumnType::Native(NativeType::Timestamp),
            TableSpec::borrowed("shop", "orders"),
        );
        let col = result_column(&spec);
        assert_eq!(
            (col.name.as_str(), col.data_type.as_str(), col.kind),
            ("ts", "timestamp", ValueKind::TimestampTz)
        );
        assert_eq!(col.table.as_deref(), Some("orders"));
    }
}
