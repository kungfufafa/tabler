//! DataRow cells to Tabler's `Value`, in Vertica's text format (what the client asks for:
//! `binary_data_protocol=0`) and its binary format, following vertica-python's deserializers.
//!
//! Both are total: a cell that does not parse as its type is kept as `Value::Other` with its
//! text (or `Value::Bytes` when it is not UTF-8), never a panic and never lost. Temporal values
//! keep the server's text; the binary forms are written the way the text format writes them
//! (`2024-01-31 12:00:00.5`, `+00` for a TIMESTAMPTZ, which the binary form carries in UTC).

use chrono::{Datelike as _, NaiveDate};

use super::types;
use super::wire::{DataRow, Field, TypeRef};
use crate::types::query::{ColumnOrigin, ResultColumn};
use crate::types::schema::ValueKind;
use crate::types::value::Value;

/// The Julian day number of 0001-01-01, the epoch of a binary DATE's day count.
const JDN_0001_01_01: i64 = 1_721_426;
/// 2000-01-01, the epoch of the binary TIMESTAMP, as chrono's day of the common era.
const CE_DAY_2000_01_01: i64 = 730_120;
const MICROS_PER_DAY: i64 = 86_400_000_000;

/// The result column of a field: its name, type name and kind, nullability and table. No origin:
/// the field's name is the column's label (an alias, maybe), so the base column is found by the
/// field's table OID and attribute number (`origin`, after a catalog lookup).
pub fn result_column(field: &Field) -> ResultColumn {
    ResultColumn {
        name: field.name.clone(),
        data_type: types::type_name(&field.ty),
        kind: types::kind(&field.ty),
        nullable: Some(field.nullable),
        table: field.table.clone(),
        origin: None,
    }
}

/// Where a field reads from, given the name of the base column its table OID and attribute
/// number resolve to (`catalog::COLUMN_BY_ATTRIBUTE`); None for a computed column.
pub fn origin(field: &Field, base_column: &str) -> Option<ColumnOrigin> {
    if field.table_oid == 0 || field.attribute == 0 {
        return None;
    }
    let (schema, table) = (field.schema.as_deref()?, field.table.as_deref()?);
    ColumnOrigin::from_names(schema, table, base_column)
}

/// A row's values, cell by cell in its fields' formats. A row with fewer cells than fields
/// gets NULLs, and extra cells are ignored (neither is valid, neither may panic).
pub fn decode_row(fields: &[Field], row: &DataRow) -> Vec<Value> {
    fields
        .iter()
        .enumerate()
        .map(|(i, f)| match row.cell(i) {
            None => Value::Null,
            Some(cell) if f.format == 1 => binary_value(&f.ty, cell),
            Some(cell) => text_value(&f.ty, cell),
        })
        .collect()
}

fn utf8(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes).ok()
}

/// The type a value that does not parse is kept under.
fn other(ty: &TypeRef, bytes: &[u8]) -> Value {
    match utf8(bytes) {
        Some(text) => Value::other(&types::type_name(ty), text),
        None => Value::from_bytes(bytes),
    }
}

/// A cell in the text format.
pub fn text_value(ty: &TypeRef, bytes: &[u8]) -> Value {
    let kind = types::kind(ty);
    if kind == ValueKind::Bytes {
        return Value::from_bytes(&unescape_binary(bytes));
    }
    let Some(text) = utf8(bytes) else {
        // A VARCHAR holds bytes, and they need not be UTF-8: keep them whole.
        return Value::from_bytes(bytes);
    };
    match kind {
        ValueKind::Bool => match text {
            "t" | "true" | "1" => Value::Bool(true),
            "f" | "false" | "0" => Value::Bool(false),
            _ => other(ty, bytes),
        },
        ValueKind::Int => match text.trim().parse::<i64>() {
            Ok(n) => Value::from_i64(n),
            Err(_) => other(ty, bytes),
        },
        ValueKind::Float => match text.trim().parse::<f64>() {
            Ok(f) => Value::from_f64(&types::type_name(ty), f),
            Err(_) => other(ty, bytes),
        },
        ValueKind::Decimal => Value::Decimal(text.to_string()),
        ValueKind::Text => Value::Text(text.to_string()),
        ValueKind::Date => Value::Date(text.to_string()),
        ValueKind::Time => Value::Time(text.to_string()),
        ValueKind::Timestamp => Value::Timestamp(text.to_string()),
        ValueKind::TimestampTz => Value::TimestampTz(text.to_string()),
        ValueKind::Interval => Value::Interval(text.to_string()),
        ValueKind::Uuid => Value::Uuid(text.to_string()),
        ValueKind::Json => Value::Json(text.to_string()),
        _ => other(ty, bytes),
    }
}

/// The text format of a binary value: `\\` is a backslash, `\ooo` (three octal digits) a byte,
/// anything else itself. A backslash not followed by either is kept as it is.
pub fn unescape_binary(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            if bytes.get(i + 1) == Some(&b'\\') {
                out.push(b'\\');
                i += 2;
                continue;
            }
            let octal = bytes.get(i + 1..i + 4).and_then(|d| {
                d.iter().try_fold(0u16, |acc, &c| {
                    (b'0'..=b'7')
                        .contains(&c)
                        .then(|| acc * 8 + u16::from(c - b'0'))
                })
            });
            if let Some(byte) = octal.and_then(|n| u8::try_from(n).ok()) {
                out.push(byte);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// A binary value as a bind parameter's text: backslashes written `\134`, as vertica-python's Bind
/// does (the server reads the parameter like a VARBINARY literal).
pub fn escape_binary(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        if b == b'\\' {
            out.extend_from_slice(b"\\134");
        } else {
            out.push(b);
        }
    }
    out
}

fn be_i64(bytes: &[u8]) -> Option<i64> {
    Some(i64::from_be_bytes(bytes.try_into().ok()?))
}

/// A cell in the binary format.
pub fn binary_value(ty: &TypeRef, bytes: &[u8]) -> Value {
    let decoded = match ty.oid {
        types::BOOL => match bytes {
            [b] => Some(Value::Bool(*b == 1)),
            _ => None,
        },
        types::INT8 => be_i64(bytes).map(Value::from_i64),
        types::FLOAT8 => bytes
            .try_into()
            .ok()
            .map(|b: [u8; 8]| Value::from_f64(&types::type_name(ty), f64::from_be_bytes(b))),
        types::NUMERIC => {
            let scale = types::scale(ty.oid, ty.modifier).unwrap_or(0);
            (!bytes.is_empty()).then(|| Value::Decimal(numeric_text(bytes, scale)))
        }
        types::DATE => be_i64(bytes).and_then(date_text).map(Value::Date),
        types::TIME => be_i64(bytes)
            .filter(|us| (0..=MICROS_PER_DAY).contains(us))
            .map(|us| Value::Time(clock_text(us))),
        types::TIMETZ => be_i64(bytes).and_then(timetz_text).map(Value::Time),
        types::TIMESTAMP => be_i64(bytes).and_then(timestamp_text).map(Value::Timestamp),
        types::TIMESTAMPTZ => be_i64(bytes)
            .and_then(timestamp_text)
            .map(|t| Value::TimestampTz(format!("{t}+00"))),
        types::INTERVAL => be_i64(bytes).map(|us| Value::Interval(day_time_text(us))),
        types::INTERVALYM => be_i64(bytes).map(|m| Value::Interval(year_month_text(m))),
        types::UUID => <[u8; 16]>::try_from(bytes)
            .ok()
            .map(|b| Value::Uuid(uuid_text(&b))),
        // Text, binary and complex values are sent the same way in both formats (complex ones as
        // JSON text).
        _ => return text_or_bytes(ty, bytes),
    };
    decoded.unwrap_or_else(|| Value::other(&types::type_name(ty), hex::encode(bytes)))
}

fn text_or_bytes(ty: &TypeRef, bytes: &[u8]) -> Value {
    match types::kind(ty) {
        ValueKind::Bytes => Value::from_bytes(bytes),
        ValueKind::Text | ValueKind::Json | ValueKind::Other => text_value(ty, bytes),
        _ => other(ty, bytes),
    }
}

/// A big-endian two's-complement integer (any length) as decimal digits, with `scale` digits
/// after the point: the unscaled value of a binary NUMERIC.
pub fn numeric_text(bytes: &[u8], scale: i32) -> String {
    let negative = bytes.first().is_some_and(|b| b & 0x80 != 0);
    // The magnitude: the bytes themselves, or their two's complement.
    let mut mag: Vec<u8> = bytes.to_vec();
    if negative {
        for b in &mut mag {
            *b = !*b;
        }
        for b in mag.iter_mut().rev() {
            let (v, carry) = b.overflowing_add(1);
            *b = v;
            if !carry {
                break;
            }
        }
    }
    // Base 256 to base 10^9, by repeated division of the big-endian bytes.
    let mut chunks: Vec<u32> = Vec::new();
    let mut digits = mag;
    while digits.iter().any(|&d| d != 0) {
        let mut rem: u64 = 0;
        for d in &mut digits {
            let cur = (rem << 8) | u64::from(*d);
            *d = (cur / 1_000_000_000) as u8;
            rem = cur % 1_000_000_000;
        }
        chunks.push(rem as u32);
    }
    let mut text = match chunks.split_last() {
        None => "0".to_string(),
        Some((top, rest)) => {
            let mut s = top.to_string();
            for c in rest.iter().rev() {
                s.push_str(&format!("{c:09}"));
            }
            s
        }
    };
    let scale = usize::try_from(scale).unwrap_or(0);
    if scale > 0 {
        if text.len() <= scale {
            text = format!("{}{text}", "0".repeat(scale + 1 - text.len()));
        }
        text.insert(text.len() - scale, '.');
    }
    if negative && text.bytes().any(|b| b.is_ascii_digit() && b != b'0') {
        text.insert(0, '-');
    }
    text
}

fn fraction(us: i64) -> String {
    if us == 0 {
        return String::new();
    }
    let digits = format!(".{us:06}");
    digits.trim_end_matches('0').to_string()
}

/// `HH:MM:SS[.ffffff]` of a count of microseconds within a day (24:00:00 included).
fn clock_text(us: i64) -> String {
    let secs = us / 1_000_000;
    format!(
        "{:02}:{:02}:{:02}{}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60,
        fraction(us % 1_000_000)
    )
}

/// A date the way the text format writes it: ISO, with ` BC` for years before 1.
fn date_from(d: NaiveDate) -> String {
    let y = d.year();
    if y <= 0 {
        format!("{:04}-{:02}-{:02} BC", 1 - y, d.month(), d.day())
    } else {
        format!("{y:04}-{:02}-{:02}", d.month(), d.day())
    }
}

/// A day of the common era (0001-01-01 is 1) as text.
fn ce_day_text(day: i64) -> Option<String> {
    NaiveDate::from_num_days_from_ce_opt(i32::try_from(day).ok()?).map(date_from)
}

fn date_text(jdn: i64) -> Option<String> {
    ce_day_text(jdn.checked_sub(JDN_0001_01_01 - 1)?)
}

/// Microseconds since 2000-01-01 00:00:00 as `YYYY-MM-DD HH:MM:SS[.ffffff]`.
fn timestamp_text(us: i64) -> Option<String> {
    let day = us.div_euclid(MICROS_PER_DAY);
    let date = ce_day_text(day.checked_add(CE_DAY_2000_01_01)?)?;
    Some(format!(
        "{date} {}",
        clock_text(us.rem_euclid(MICROS_PER_DAY))
    ))
}

/// A TIMETZ: the upper 40 bits are microseconds since midnight UTC, the lower 24 bits the zone
/// as `86400 - offset seconds` (vertica-python's reading); the text is the local time and offset.
fn timetz_text(v: i64) -> Option<String> {
    let utc_us = v >> 24;
    let offset = 86_400 - (v & 0xff_ffff);
    if !(0..=MICROS_PER_DAY).contains(&utc_us) || offset.abs() > 86_400 {
        return None;
    }
    let local = (utc_us + offset * 1_000_000).rem_euclid(MICROS_PER_DAY);
    let sign = if offset < 0 { '-' } else { '+' };
    let off = offset.abs();
    let (h, m, s) = (off / 3600, off / 60 % 60, off % 60);
    let zone = match (m, s) {
        (0, 0) => format!("{sign}{h:02}"),
        (_, 0) => format!("{sign}{h:02}:{m:02}"),
        _ => format!("{sign}{h:02}:{m:02}:{s:02}"),
    };
    Some(format!("{}{zone}", clock_text(local)))
}

/// A day-time interval of microseconds as `[-]D HH:MM:SS[.ffffff]`.
fn day_time_text(us: i64) -> String {
    let sign = if us < 0 { "-" } else { "" };
    let us = us.unsigned_abs();
    let day = 86_400_000_000u64;
    let (days, rest) = (us / day, us % day);
    let secs = rest / 1_000_000;
    let frac = fraction((rest % 1_000_000) as i64);
    format!(
        "{sign}{days} {:02}:{:02}:{:02}{frac}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// A year-month interval of months as `[-]Y-M`.
fn year_month_text(months: i64) -> String {
    let sign = if months < 0 { "-" } else { "" };
    let m = months.unsigned_abs();
    format!("{sign}{}-{}", m / 12, m % 12)
}

fn uuid_text(b: &[u8; 16]) -> String {
    let h = hex::encode(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::value::JS_SAFE_INT;

    fn t(oid: u32) -> TypeRef {
        TypeRef {
            oid,
            user_type: None,
            modifier: -1,
        }
    }

    fn txt(oid: u32, s: &str) -> Value {
        text_value(&t(oid), s.as_bytes())
    }

    #[test]
    fn text_format() {
        assert_eq!(txt(types::BOOL, "t"), Value::Bool(true));
        assert_eq!(txt(types::BOOL, "f"), Value::Bool(false));
        assert_eq!(txt(types::INT8, "42"), Value::Int(42));
        // 2^53 + 1 does not fit a JS number.
        assert_eq!(
            txt(types::INT8, "9007199254740993"),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            txt(types::INT8, &JS_SAFE_INT.to_string()),
            Value::Int(JS_SAFE_INT)
        );
        assert_eq!(
            txt(types::INT8, "-9223372036854775808"),
            Value::BigInt("-9223372036854775808".into())
        );
        assert_eq!(txt(types::FLOAT8, "1.5"), Value::Float(1.5));
        assert!(matches!(txt(types::FLOAT8, "Infinity"), Value::Other(_)));
        assert!(matches!(txt(types::FLOAT8, "NaN"), Value::Other(_)));
        assert_eq!(
            txt(types::NUMERIC, "123456789012345678901234567890.123"),
            Value::Decimal("123456789012345678901234567890.123".into())
        );
        assert_eq!(txt(types::VARCHAR, "héllo"), Value::Text("héllo".into()));
        assert_eq!(
            txt(types::DATE, "2024-02-29"),
            Value::Date("2024-02-29".into())
        );
        assert_eq!(
            txt(types::DATE, "0044-03-15 BC"),
            Value::Date("0044-03-15 BC".into())
        );
        assert_eq!(
            txt(types::TIMETZ, "10:00:00+07"),
            Value::Time("10:00:00+07".into())
        );
        assert_eq!(
            txt(types::TIMESTAMPTZ, "2024-01-01 10:00:00.5+07"),
            Value::TimestampTz("2024-01-01 10:00:00.5+07".into())
        );
        assert_eq!(
            txt(types::INTERVAL, "1 02:03"),
            Value::Interval("1 02:03".into())
        );
        assert_eq!(
            txt(types::UUID, "6ab09bec-e68e-48d9-a5f8-97e6fb4c9b47"),
            Value::Uuid("6ab09bec-e68e-48d9-a5f8-97e6fb4c9b47".into())
        );
        assert_eq!(txt(1506, "[1,2,null]"), Value::Json("[1,2,null]".into()));
        assert_eq!(
            txt(types::ROW, "{\"a\":1,\"b\":\"x\"}"),
            Value::Json("{\"a\":1,\"b\":\"x\"}".into())
        );
        // What does not parse stays as text, and bytes that are not UTF-8 stay bytes.
        assert_eq!(txt(types::INT8, "abc"), Value::other("int", "abc"));
        assert_eq!(txt(types::BOOL, "maybe"), Value::other("boolean", "maybe"));
        assert_eq!(
            text_value(&t(types::VARCHAR), &[0xff, 0x00]),
            Value::from_bytes(&[0xff, 0x00])
        );
        assert_eq!(txt(999, "x"), Value::other("oid 999", "x"));
    }

    #[test]
    fn binary_text_unescapes_octal() {
        assert_eq!(
            unescape_binary(br"a\\b\000\377\101z"),
            b"a\\b\x00\xffAz".to_vec()
        );
        // A backslash not followed by an escape, an octal value above 255, and one cut short.
        assert_eq!(unescape_binary(br"\x"), br"\x".to_vec());
        assert_eq!(unescape_binary(br"\777"), br"\777".to_vec());
        assert_eq!(unescape_binary(br"\12"), br"\12".to_vec());
        assert_eq!(unescape_binary(b"\\"), b"\\".to_vec());
        assert_eq!(
            text_value(&t(types::VARBINARY), br"\001\\"),
            Value::from_bytes(&[1, b'\\'])
        );
        assert_eq!(escape_binary(b"a\\b"), b"a\\134b".to_vec());
        assert_eq!(unescape_binary(&escape_binary(b"\\\\x")), b"\\\\x".to_vec());
    }

    fn bin(oid: u32, bytes: &[u8]) -> Value {
        binary_value(&t(oid), bytes)
    }

    #[test]
    fn binary_format() {
        assert_eq!(bin(types::BOOL, &[1]), Value::Bool(true));
        assert_eq!(bin(types::BOOL, &[0]), Value::Bool(false));
        assert_eq!(bin(types::INT8, &(-7i64).to_be_bytes()), Value::Int(-7));
        assert_eq!(
            bin(types::INT8, &(i64::MAX).to_be_bytes()),
            Value::BigInt(i64::MAX.to_string())
        );
        assert_eq!(
            bin(types::FLOAT8, &2.25f64.to_be_bytes()),
            Value::Float(2.25)
        );
        assert!(matches!(
            bin(types::FLOAT8, &f64::INFINITY.to_be_bytes()),
            Value::Other(_)
        ));
        // 2000-01-01 is Julian day 2451545.
        assert_eq!(
            bin(types::DATE, &2_451_545i64.to_be_bytes()),
            Value::Date("2000-01-01".into())
        );
        assert_eq!(
            bin(types::DATE, &JDN_0001_01_01.to_be_bytes()),
            Value::Date("0001-01-01".into())
        );
        assert_eq!(
            bin(types::DATE, &(JDN_0001_01_01 - 1).to_be_bytes()),
            Value::Date("0001-12-31 BC".into())
        );
        assert_eq!(
            bin(types::TIME, &(45_296_500_000i64).to_be_bytes()),
            Value::Time("12:34:56.5".into())
        );
        assert_eq!(
            bin(types::TIMESTAMP, &0i64.to_be_bytes()),
            Value::Timestamp("2000-01-01 00:00:00".into())
        );
        assert_eq!(
            bin(types::TIMESTAMP, &(-1i64).to_be_bytes()),
            Value::Timestamp("1999-12-31 23:59:59.999999".into())
        );
        assert_eq!(
            bin(types::TIMESTAMPTZ, &(86_400_000_123i64).to_be_bytes()),
            Value::TimestampTz("2000-01-02 00:00:00.000123+00".into())
        );
        // 03:00:00 UTC in +07:00: the lower bits hold 86400 - 25200.
        let v = (3i64 * 3600 * 1_000_000) << 24 | (86_400 - 25_200);
        assert_eq!(
            bin(types::TIMETZ, &v.to_be_bytes()),
            Value::Time("10:00:00+07".into())
        );
        let v: i64 = 86_400 + 19_800;
        assert_eq!(
            bin(types::TIMETZ, &v.to_be_bytes()),
            Value::Time("18:30:00-05:30".into())
        );
        assert_eq!(
            bin(
                types::INTERVAL,
                &(-(86_400_000_000i64 + 3_723_000_001)).to_be_bytes()
            ),
            Value::Interval("-1 01:02:03.000001".into())
        );
        assert_eq!(
            bin(types::INTERVALYM, &14i64.to_be_bytes()),
            Value::Interval("1-2".into())
        );
        let uuid = [
            0x6a, 0xb0, 0x9b, 0xec, 0xe6, 0x8e, 0x48, 0xd9, 0xa5, 0xf8, 0x97, 0xe6, 0xfb, 0x4c,
            0x9b, 0x47,
        ];
        assert_eq!(
            bin(types::UUID, &uuid),
            Value::Uuid("6ab09bec-e68e-48d9-a5f8-97e6fb4c9b47".into())
        );
        assert_eq!(bin(types::VARCHAR, b"abc"), Value::Text("abc".into()));
        assert_eq!(bin(types::VARBINARY, b"\\1"), Value::from_bytes(b"\\1"));
        assert_eq!(bin(1506, b"[1]"), Value::Json("[1]".into()));
        // Wrong lengths and out-of-range values are kept, as hex.
        for (oid, bytes) in [
            (types::BOOL, &[1u8, 2][..]),
            (types::INT8, &[1, 2, 3][..]),
            (types::FLOAT8, &[][..]),
            (types::NUMERIC, &[][..]),
            (types::DATE, &i64::MAX.to_be_bytes()[..]),
            (types::TIMESTAMP, &i64::MIN.to_be_bytes()[..]),
            (types::TIME, &(-1i64).to_be_bytes()[..]),
            (types::TIMETZ, &(-1i64).to_be_bytes()[..]),
            (types::UUID, &[0; 15][..]),
        ] {
            assert!(
                matches!(bin(oid, bytes), Value::Other(_)),
                "{oid} {bytes:?}"
            );
        }
    }

    #[test]
    fn binary_numerics() {
        let num = |bytes: &[u8], scale: i32| numeric_text(bytes, scale);
        assert_eq!(num(&[0x00, 0x7b], 0), "123");
        assert_eq!(num(&[0x30, 0x39], 2), "123.45");
        assert_eq!(num(&[0xff, 0xff, 0xcf, 0xc7], 2), "-123.45");
        assert_eq!(num(&[0x05], 3), "0.005");
        assert_eq!(num(&[0xfb], 3), "-0.005");
        assert_eq!(num(&[0x00], 2), "0.00");
        assert_eq!(num(&[0x80], 0), "-128");
        // 10^30 - 1 over 16 bytes, which no machine integer holds.
        let big: [u8; 16] = [
            0x00, 0x00, 0x00, 0x0c, 0x9f, 0x2c, 0x9c, 0xd0, 0x46, 0x74, 0xed, 0xea, 0x3f, 0xff,
            0xff, 0xff,
        ];
        assert_eq!(num(&big, 0), "999999999999999999999999999999");
        assert_eq!(num(&big, 10), "99999999999999999999.9999999999");
        // numeric(10,2): the modifier gives the scale.
        let ty = TypeRef {
            oid: types::NUMERIC,
            user_type: None,
            modifier: (10 << 16 | 2) + 4,
        };
        assert_eq!(
            binary_value(&ty, &[0x30, 0x39]),
            Value::Decimal("123.45".into())
        );
    }

    #[test]
    fn columns_and_rows() {
        use crate::drivers::vertica::wire::Field;
        let field = |name: &str, oid: u32, format: u16, table: Option<(&str, &str)>| Field {
            name: name.into(),
            table_oid: u64::from(table.is_some()) * 45_035_996_273_704_982,
            schema: table.map(|t| t.0.into()),
            table: table.map(|t| t.1.into()),
            attribute: u16::from(table.is_some()),
            ty: t(oid),
            size: -1,
            nullable: true,
            identity: false,
            format,
            children: Vec::new(),
        };
        let fields = [
            field("id", types::INT8, 0, Some(("public", "t"))),
            field("n", types::INT8, 1, None),
            field("s", types::VARCHAR, 0, None),
        ];
        let col = result_column(&fields[0]);
        assert_eq!(col.data_type, "int");
        assert_eq!(col.table.as_deref(), Some("t"));
        assert!(col.origin.is_none());
        let found = origin(&fields[0], "id").unwrap();
        assert_eq!(
            found.table,
            crate::types::schema::TableRef::new(Some("public"), "t")
        );
        assert_eq!(found.column, "id");
        assert!(origin(&fields[1], "n").is_none());
        let body = crate::drivers::vertica::testing::msg::data_row(&[
            Some(&b"7"[..]),
            Some(&5i64.to_be_bytes()[..]),
        ]);
        let row = DataRow::parse(body[5..].to_vec()).unwrap();
        assert_eq!(
            decode_row(&fields, &row),
            vec![Value::Int(7), Value::Int(5), Value::Null]
        );
    }
}
