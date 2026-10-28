//! DuckDB values (Arrow arrays of a result chunk) as Tabler's `Value`s, and `Value`s back as bind
//! parameters.
//!
//! Every decoder is total. The rows are read straight from the Arrow arrays, never through
//! duckdb-rs's `Row::get_ref`, which panics on Arrow shapes it does not know (a string view after
//! `SET produce_arrow_string_view = true`, a list view, a `Float16`): a shape this module does not
//! expect is written by Arrow's own formatter, and a value nothing can write becomes `Other`.
//! Release builds abort on a panic, so no index here is trusted without a bounds check.
//!
//! Temporal values are written the way DuckDB prints them: `2024-01-31 12:00:00.5`, BC dates as
//! `0044-03-15 (BC)`, `infinity`, TIMESTAMPTZ in UTC with `+00` (the bundled DuckDB has no ICU, so
//! UTC is the only zone it prints), intervals as `1 year 2 months 3 days 04:05:06`. Nested values
//! (LIST, ARRAY, STRUCT, MAP, UNION) become JSON text.

use std::fmt::Write as _;

use ::duckdb::arrow::array::{
    Array, ArrayRef, BinaryArray, BinaryViewArray, BooleanArray, Date32Array, Decimal32Array,
    Decimal64Array, Decimal128Array, DictionaryArray, FixedSizeBinaryArray, FixedSizeListArray,
    Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array,
    IntervalMonthDayNanoArray, LargeBinaryArray, LargeListArray, LargeListViewArray,
    LargeStringArray, ListArray, ListViewArray, MapArray, StringArray, StringViewArray,
    StructArray, Time64MicrosecondArray, Time64NanosecondArray, TimestampMicrosecondArray,
    TimestampMillisecondArray, TimestampNanosecondArray, TimestampSecondArray, UInt8Array,
    UInt16Array, UInt32Array, UInt64Array, UnionArray,
};
use ::duckdb::arrow::datatypes::{
    DataType, Field, Int8Type, Int16Type, Int32Type, Int64Type, TimeUnit, UInt8Type, UInt16Type,
    UInt32Type, UInt64Type,
};
use ::duckdb::arrow::util::display::{ArrayFormatter, FormatOptions};
use base64::Engine as _;

use super::types::{DuckType, TsUnit};
use crate::error::AppError;
use crate::types::query::ResultColumn;
use crate::types::value::{JS_SAFE_INT, Value};

/// How much of a binary value a cell keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bytes {
    /// The grid's preview (`Value::from_bytes`).
    Preview,
    /// All of it (`fetch_cell`'s "load full value").
    Whole,
}

/// A result column as the grid shows it. DuckDB names no base table for result columns.
pub fn result_column(name: &str, ty: &DuckType) -> ResultColumn {
    ResultColumn {
        name: name.to_string(),
        data_type: ty.name(),
        kind: ty.kind(),
        nullable: None,
        table: None,
        origin: None,
    }
}

/// The type of each column of a result: the logical type, completed with what only the Arrow
/// shape tells (an ARRAY's size).
pub fn column_type(logical: DuckType, field: &Field) -> DuckType {
    complete(logical, field.data_type())
}

fn complete(t: DuckType, dt: &DataType) -> DuckType {
    match (t, dt) {
        (DuckType::Array(inner, _), DataType::FixedSizeList(child, n)) => DuckType::Array(
            Box::new(complete(*inner, child.data_type())),
            usize::try_from(*n).ok(),
        ),
        (DuckType::List(inner), DataType::List(child) | DataType::LargeList(child)) => {
            DuckType::List(Box::new(complete(*inner, child.data_type())))
        }
        (DuckType::Struct(members), DataType::Struct(fields)) if members.len() == fields.len() => {
            DuckType::Struct(
                members
                    .into_iter()
                    .zip(fields.iter())
                    .map(|((name, t), f)| (name, complete(t, f.data_type())))
                    .collect(),
            )
        }
        (t, _) => t,
    }
}

/// The value at `row` of a column of type `ty`. NULL is `Null`, whatever the type.
pub fn cell(arr: &dyn Array, row: usize, ty: &DuckType, bytes: Bytes) -> Value {
    if row >= arr.len() {
        return Value::other("missing", "");
    }
    if arr.is_null(row) || matches!(arr.data_type(), DataType::Null) {
        return Value::Null;
    }
    match ty {
        DuckType::Null => Value::Null,
        DuckType::Boolean => match arr.as_any().downcast_ref::<BooleanArray>() {
            Some(a) => Value::Bool(a.value(row)),
            None => fallback(arr, row, ty),
        },
        DuckType::TinyInt
        | DuckType::SmallInt
        | DuckType::Integer
        | DuckType::BigInt
        | DuckType::UTinyInt
        | DuckType::USmallInt
        | DuckType::UInteger
        | DuckType::UBigInt => match integer(arr, row) {
            Some(n) => int_value(n),
            None => fallback(arr, row, ty),
        },
        DuckType::HugeInt => match wide(arr, row) {
            Some(n) => int_value(n),
            None => fallback(arr, row, ty),
        },
        // Arrow carries a UHUGEINT in the same 128 bits as a signed one.
        DuckType::UHugeInt => match wide(arr, row) {
            Some(n) => uint_value(n as u128),
            None => fallback(arr, row, ty),
        },
        DuckType::Float | DuckType::Double => match float(arr, row) {
            Some(f) => Value::from_f64(&ty.name().to_lowercase(), f),
            None => fallback(arr, row, ty),
        },
        DuckType::Decimal { .. } => match decimal(arr, row) {
            Some(text) => Value::Decimal(text),
            None => fallback(arr, row, ty),
        },
        DuckType::Varchar => match text(arr, row) {
            Some(s) => Value::Text(s.to_string()),
            None => fallback(arr, row, ty),
        },
        DuckType::Json => match text(arr, row) {
            Some(s) => Value::Json(s.to_string()),
            None => fallback(arr, row, ty),
        },
        DuckType::Uuid => match uuid(arr, row) {
            Some(s) => Value::Uuid(s),
            None => fallback(arr, row, ty),
        },
        DuckType::Enum(_) => match enum_label(arr, row) {
            Some(s) => Value::Enum(s),
            None => fallback(arr, row, ty),
        },
        DuckType::Blob => match binary(arr, row) {
            Some(b) => blob_value(b, bytes),
            None => fallback(arr, row, ty),
        },
        DuckType::Geometry => match binary(arr, row) {
            Some(b) => geometry_value(b),
            None => fallback(arr, row, ty),
        },
        DuckType::Bit => match binary(arr, row) {
            Some(b) => match bit_text(b) {
                Some(bits) => Value::other("bit", bits),
                None => Value::other("bit", hex(b)),
            },
            None => match text(arr, row) {
                Some(s) => Value::other("bit", s),
                None => fallback(arr, row, ty),
            },
        },
        DuckType::Bignum => match text(arr, row) {
            Some(s) => Value::BigInt(s.to_string()),
            None => match binary(arr, row).and_then(bignum_text) {
                Some(s) => Value::BigInt(s),
                None => fallback(arr, row, ty),
            },
        },
        DuckType::Date => match date(arr, row) {
            Some(s) => Value::Date(s),
            None => fallback(arr, row, ty),
        },
        DuckType::Time | DuckType::TimeNs => match time(arr, row) {
            Some(s) => Value::Time(s),
            None => fallback(arr, row, ty),
        },
        // By default Arrow carries a TIMETZ as its local time, without the offset (the grid
        // reads such columns as text); with `arrow_lossless_conversion` as DuckDB's 64 bits.
        DuckType::TimeTz => match time(arr, row) {
            Some(s) => Value::Time(s),
            None => match time_tz_bits(arr, row) {
                Some(bits) => Value::Time(time_tz_text(bits)),
                None => fallback(arr, row, ty),
            },
        },
        DuckType::Timestamp(_) => match timestamp(arr, row) {
            Some(s) => Value::Timestamp(s),
            None => fallback(arr, row, ty),
        },
        DuckType::TimestampTz => match timestamp(arr, row) {
            Some(s) if is_infinite(&s) => Value::TimestampTz(s),
            Some(s) => Value::TimestampTz(format!("{s}+00")),
            None => fallback(arr, row, ty),
        },
        DuckType::Interval => match interval(arr, row) {
            Some(s) => Value::Interval(s),
            None => fallback(arr, row, ty),
        },
        DuckType::List(_)
        | DuckType::Array(..)
        | DuckType::Struct(_)
        | DuckType::Map(..)
        | DuckType::Union(_) => {
            let mut out = String::new();
            json(&mut out, arr, row, ty, 0);
            // A union keeps its NULLs in its members: Arrow reports none for the union itself.
            if out == "null" {
                Value::Null
            } else {
                Value::Json(out)
            }
        }
        DuckType::Variant | DuckType::Other(_) => fallback(arr, row, ty),
    }
}

/// An integer within the JS safe range is an `Int`, a larger one `BigInt` text.
pub fn int_value(n: i128) -> Value {
    if (-(JS_SAFE_INT as i128)..=JS_SAFE_INT as i128).contains(&n) {
        Value::Int(n as i64)
    } else {
        Value::BigInt(n.to_string())
    }
}

fn uint_value(n: u128) -> Value {
    if n <= JS_SAFE_INT as u128 {
        Value::Int(n as i64)
    } else {
        Value::BigInt(n.to_string())
    }
}

fn blob_value(b: &[u8], bytes: Bytes) -> Value {
    match bytes {
        Bytes::Preview => Value::from_bytes(b),
        Bytes::Whole => Value::from_bytes_uncapped(b),
    }
}

/// WKB as WKT while the app converts geometries (Settings ▸ Data Decoder), hex otherwise or when
/// it does not parse.
fn geometry_value(b: &[u8]) -> Value {
    let wkt = crate::geometry::convert_to_wkt()
        .then(|| crate::geometry::parse_wkb(b).ok())
        .flatten();
    match wkt {
        Some(g) => Value::other("geometry", g.to_ewkt()),
        None => Value::other("geometry", hex(b)),
    }
}

/// What Arrow's formatter writes for a value, or its type and nothing.
fn fallback(arr: &dyn Array, row: usize, ty: &DuckType) -> Value {
    Value::other(
        &ty.name().to_lowercase(),
        formatted(arr, row).unwrap_or_default(),
    )
}

fn formatted(arr: &dyn Array, row: usize) -> Option<String> {
    if row >= arr.len() {
        return None;
    }
    let options = FormatOptions::new();
    let formatter = ArrayFormatter::try_new(arr, &options).ok()?;
    formatter.value(row).try_to_string().ok()
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        let _ = write!(s, "{byte:02x}");
    }
    s
}

// --- Primitive readers: None when the array has another shape -------------------------------

fn integer(arr: &dyn Array, row: usize) -> Option<i128> {
    let any = arr.as_any();
    macro_rules! read {
        ($($t:ty),*) => {
            $(if let Some(a) = any.downcast_ref::<$t>() {
                return Some(a.value(row) as i128);
            })*
        };
    }
    read!(
        Int8Array,
        Int16Array,
        Int32Array,
        Int64Array,
        UInt8Array,
        UInt16Array,
        UInt32Array,
        UInt64Array
    );
    None
}

/// A HUGEINT or UHUGEINT: `Decimal128(38, 0)` by default, 16 little-endian bytes with
/// `arrow_lossless_conversion`.
fn wide(arr: &dyn Array, row: usize) -> Option<i128> {
    let any = arr.as_any();
    if let Some(a) = any.downcast_ref::<Decimal128Array>() {
        return Some(a.value(row));
    }
    if let Some(a) = any.downcast_ref::<FixedSizeBinaryArray>() {
        let bytes: [u8; 16] = a.value(row).try_into().ok()?;
        return Some(i128::from_le_bytes(bytes));
    }
    integer(arr, row)
}

fn float(arr: &dyn Array, row: usize) -> Option<f64> {
    let any = arr.as_any();
    if let Some(a) = any.downcast_ref::<Float64Array>() {
        return Some(a.value(row));
    }
    // A FLOAT keeps its shortest decimal form (0.1, not 0.10000000149011612).
    any.downcast_ref::<Float32Array>()
        .map(|a| a.value(row).to_string().parse().unwrap_or(f64::NAN))
}

fn decimal(arr: &dyn Array, row: usize) -> Option<String> {
    let any = arr.as_any();
    let (value, scale) = match arr.data_type() {
        DataType::Decimal128(_, s) => (any.downcast_ref::<Decimal128Array>()?.value(row), *s),
        DataType::Decimal64(_, s) => (any.downcast_ref::<Decimal64Array>()?.value(row) as i128, *s),
        DataType::Decimal32(_, s) => (any.downcast_ref::<Decimal32Array>()?.value(row) as i128, *s),
        _ => return integer(arr, row).map(|n| n.to_string()),
    };
    Some(decimal_text(value, scale))
}

/// A scaled integer as decimal text: `12345` with scale 2 is `123.45`.
pub fn decimal_text(value: i128, scale: i8) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + 3);
    if negative {
        out.push('-');
    }
    if scale <= 0 {
        out.push_str(&digits);
        for _ in 0..scale.unsigned_abs() {
            out.push('0');
        }
        return out;
    }
    let scale = scale as usize;
    if digits.len() <= scale {
        out.push_str("0.");
        for _ in digits.len()..scale {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let (int, frac) = digits.split_at(digits.len() - scale);
        out.push_str(int);
        out.push('.');
        out.push_str(frac);
    }
    out
}

fn text(arr: &dyn Array, row: usize) -> Option<&str> {
    let any = arr.as_any();
    if let Some(a) = any.downcast_ref::<StringArray>() {
        return Some(a.value(row));
    }
    if let Some(a) = any.downcast_ref::<LargeStringArray>() {
        return Some(a.value(row));
    }
    any.downcast_ref::<StringViewArray>().map(|a| a.value(row))
}

fn binary(arr: &dyn Array, row: usize) -> Option<&[u8]> {
    let any = arr.as_any();
    if let Some(a) = any.downcast_ref::<BinaryArray>() {
        return Some(a.value(row));
    }
    if let Some(a) = any.downcast_ref::<LargeBinaryArray>() {
        return Some(a.value(row));
    }
    if let Some(a) = any.downcast_ref::<BinaryViewArray>() {
        return Some(a.value(row));
    }
    any.downcast_ref::<FixedSizeBinaryArray>()
        .map(|a| a.value(row))
}

/// A UUID: text by default, 16 bytes in network order with `arrow_lossless_conversion`.
fn uuid(arr: &dyn Array, row: usize) -> Option<String> {
    if let Some(s) = text(arr, row) {
        return Some(s.to_string());
    }
    let b = binary(arr, row)?;
    uuid::Uuid::from_slice(b)
        .ok()
        .map(|u| u.hyphenated().to_string())
}

fn enum_label(arr: &dyn Array, row: usize) -> Option<String> {
    let any = arr.as_any();
    macro_rules! read {
        ($($k:ty),*) => {
            $(if let Some(d) = any.downcast_ref::<DictionaryArray<$k>>() {
                let key = d.key(row)?;
                let values = d.values();
                if key >= values.len() {
                    return None;
                }
                return text(values.as_ref(), key).map(str::to_string);
            })*
        };
    }
    read!(
        UInt8Type, UInt16Type, UInt32Type, UInt64Type, Int8Type, Int16Type, Int32Type, Int64Type
    );
    text(arr, row).map(str::to_string)
}

/// DuckDB's BIT layout: a byte with the count of padding bits, then the bits, the padding (set
/// to 1) at the start of the first byte.
fn bit_text(b: &[u8]) -> Option<String> {
    let (&padding, data) = b.split_first()?;
    if padding > 7 || (data.is_empty() && padding > 0) {
        return None;
    }
    let mut out = String::with_capacity(data.len() * 8);
    for (i, byte) in data.iter().enumerate() {
        let skip = if i == 0 { padding } else { 0 };
        for bit in (0..8 - skip).rev() {
            out.push(if byte >> bit & 1 == 1 { '1' } else { '0' });
        }
    }
    Some(out)
}

/// DuckDB's BIGNUM layout: a 3-byte header (the sign in the top bit, set for positive values,
/// and the byte count), then the magnitude big-endian (bytes inverted for negative values).
fn bignum_text(b: &[u8]) -> Option<String> {
    if b.len() < 4 {
        return None;
    }
    let positive = b[0] & 0x80 != 0;
    let data: Vec<u8> = if positive {
        b[3..].to_vec()
    } else {
        b[3..].iter().map(|x| !x).collect()
    };
    // Base 256 to base 10, by repeated division of the big-endian magnitude.
    let mut digits = Vec::new();
    let mut num = data;
    while num.iter().any(|&x| x != 0) {
        let mut rem: u32 = 0;
        for x in num.iter_mut() {
            let cur = (rem << 8) | *x as u32;
            *x = (cur / 10) as u8;
            rem = cur % 10;
        }
        digits.push(b'0' + rem as u8);
    }
    if digits.is_empty() {
        return Some("0".into());
    }
    if !positive {
        digits.push(b'-');
    }
    digits.reverse();
    String::from_utf8(digits).ok()
}

// --- Temporal values -----------------------------------------------------------------------

const MICROS_PER_SEC: i64 = 1_000_000;

/// The proleptic Gregorian (year, month, day) of a day count from 1970-01-01, for any count
/// (Howard Hinnant's `civil_from_days`; DuckDB's dates reach past chrono's ±262,143 years).
fn civil_from_days(days: i64) -> Option<(i64, u32, u32)> {
    let z = days.checked_add(719_468)?;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = era.checked_mul(400)?.checked_add(yoe)? + i64::from(month <= 2);
    Some((year, month, day))
}

/// A date from days since 1970-01-01, as DuckDB prints it (`0044-03-15 (BC)` for year -43).
pub fn date_text(days: i64) -> Option<String> {
    let (year, month, day) = civil_from_days(days)?;
    Some(if year <= 0 {
        format!("{:04}-{month:02}-{day:02} (BC)", 1 - year)
    } else {
        format!("{year:04}-{month:02}-{day:02}")
    })
}

/// DuckDB's TIMETZ bits (`arrow_lossless_conversion`): the time in µs above 24 bits that hold
/// the offset, stored as 15:59:59 minus the offset in seconds.
fn time_tz_bits(arr: &dyn Array, row: usize) -> Option<u64> {
    if let Some(n) = integer(arr, row) {
        return Some(n as u64);
    }
    let b = binary(arr, row)?;
    Some(u64::from_le_bytes(b.try_into().ok()?))
}

/// A TIMETZ as DuckDB prints it: `12:00:00+02`, `08:30:00-03:30`.
fn time_tz_text(bits: u64) -> String {
    const MAX_OFFSET: i64 = 16 * 60 * 60 - 1;
    let micros = (bits >> 24) as i64;
    let offset = MAX_OFFSET - (bits & 0xFF_FFFF) as i64;
    let mut s = time_text(micros, MICROS_PER_SEC, 6);
    let abs = offset.abs();
    let (h, m, sec) = (abs / 3600, abs / 60 % 60, abs % 60);
    let _ = write!(s, "{}{h:02}", if offset < 0 { '-' } else { '+' });
    if m != 0 || sec != 0 {
        let _ = write!(s, ":{m:02}");
    }
    if sec != 0 {
        let _ = write!(s, ":{sec:02}");
    }
    s
}

/// A time of day from a count of `per_sec` units, with as many fractional digits as it needs.
fn time_text(units: i64, per_sec: i64, digits: usize) -> String {
    let secs = units.div_euclid(per_sec);
    let frac = units.rem_euclid(per_sec);
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    let mut out = format!("{h:02}:{m:02}:{s:02}");
    push_fraction(&mut out, frac, digits);
    out
}

fn push_fraction(out: &mut String, frac: i64, digits: usize) {
    if frac == 0 {
        return;
    }
    let text = format!("{frac:0digits$}");
    out.push('.');
    out.push_str(text.trim_end_matches('0'));
}

fn date(arr: &dyn Array, row: usize) -> Option<String> {
    let days = arr.as_any().downcast_ref::<Date32Array>()?.value(row);
    match days {
        i32::MAX => Some("infinity".into()),
        i32::MIN | -2_147_483_647 => Some("-infinity".into()),
        d => date_text(d as i64),
    }
}

fn time(arr: &dyn Array, row: usize) -> Option<String> {
    let any = arr.as_any();
    if let Some(a) = any.downcast_ref::<Time64MicrosecondArray>() {
        return Some(time_text(a.value(row), MICROS_PER_SEC, 6));
    }
    any.downcast_ref::<Time64NanosecondArray>()
        .map(|a| time_text(a.value(row), 1_000_000_000, 9))
}

fn is_infinite(s: &str) -> bool {
    s == "infinity" || s == "-infinity"
}

fn timestamp(arr: &dyn Array, row: usize) -> Option<String> {
    let any = arr.as_any();
    let (value, unit) = match arr.data_type() {
        DataType::Timestamp(TimeUnit::Second, _) => (
            any.downcast_ref::<TimestampSecondArray>()?.value(row),
            TsUnit::Seconds,
        ),
        DataType::Timestamp(TimeUnit::Millisecond, _) => (
            any.downcast_ref::<TimestampMillisecondArray>()?.value(row),
            TsUnit::Millis,
        ),
        DataType::Timestamp(TimeUnit::Microsecond, _) => (
            any.downcast_ref::<TimestampMicrosecondArray>()?.value(row),
            TsUnit::Micros,
        ),
        DataType::Timestamp(TimeUnit::Nanosecond, _) => (
            any.downcast_ref::<TimestampNanosecondArray>()?.value(row),
            TsUnit::Nanos,
        ),
        _ => return None,
    };
    timestamp_text(value, unit)
}

/// A timestamp from units since 1970-01-01, as DuckDB prints it.
pub fn timestamp_text(value: i64, unit: TsUnit) -> Option<String> {
    // DuckDB's infinities are the extreme values of each precision.
    if value == i64::MAX {
        return Some("infinity".into());
    }
    if value == -i64::MAX || value == i64::MIN {
        return Some("-infinity".into());
    }
    let (per_sec, digits): (i64, usize) = match unit {
        TsUnit::Seconds => (1, 0),
        TsUnit::Millis => (1_000, 3),
        TsUnit::Micros => (MICROS_PER_SEC, 6),
        TsUnit::Nanos => (1_000_000_000, 9),
    };
    let per_day = per_sec * 86_400;
    let days = value.div_euclid(per_day);
    let rest = value.rem_euclid(per_day);
    let date = date_text(days)?;
    let time = time_text(rest, per_sec, digits);
    // DuckDB writes `290309-12-22 (BC) 00:00:00`: the era stays with the date.
    Some(format!("{date} {time}"))
}

fn interval(arr: &dyn Array, row: usize) -> Option<String> {
    let v = arr
        .as_any()
        .downcast_ref::<IntervalMonthDayNanoArray>()?
        .value(row);
    Some(interval_text(v.months, v.days, v.nanoseconds / 1_000))
}

/// An interval as DuckDB prints it: `1 year 2 months 3 days 04:05:06.789`, `-01:30:00`,
/// `00:00:00` for zero.
pub fn interval_text(months: i32, days: i32, micros: i64) -> String {
    let mut parts: Vec<String> = Vec::new();
    let plural = |n: i64, unit: &str| {
        if n == 1 || n == -1 {
            format!("{n} {unit}")
        } else {
            format!("{n} {unit}s")
        }
    };
    let years = months / 12;
    let months = months % 12;
    if years != 0 {
        parts.push(plural(years as i64, "year"));
    }
    if months != 0 {
        parts.push(plural(months as i64, "month"));
    }
    if days != 0 {
        parts.push(plural(days as i64, "day"));
    }
    if micros != 0 || parts.is_empty() {
        let sign = if micros < 0 { "-" } else { "" };
        let abs = micros.unsigned_abs();
        let secs = abs / MICROS_PER_SEC as u64;
        let frac = abs % MICROS_PER_SEC as u64;
        let mut t = format!(
            "{sign}{:02}:{:02}:{:02}",
            secs / 3600,
            secs / 60 % 60,
            secs % 60
        );
        push_fraction(&mut t, frac as i64, 6);
        parts.push(t);
    }
    parts.join(" ")
}

// --- Nested values as JSON -----------------------------------------------------------------

/// Nesting deeper than this is written as Arrow formats it.
const MAX_JSON_DEPTH: usize = 64;

fn json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The value at `row` of a nested array as JSON: lists and arrays are arrays, structs objects
/// in member order, maps objects keyed by the keys' text, a union its member's value.
fn json(out: &mut String, arr: &dyn Array, row: usize, ty: &DuckType, depth: usize) {
    if row >= arr.len() || arr.is_null(row) || matches!(arr.data_type(), DataType::Null) {
        out.push_str("null");
        return;
    }
    if depth > MAX_JSON_DEPTH {
        json_string(out, &formatted(arr, row).unwrap_or_default());
        return;
    }
    let any = arr.as_any();
    let elem = |t: &DuckType| match t {
        DuckType::List(e) | DuckType::Array(e, _) => (**e).clone(),
        _ => DuckType::Other(String::new()),
    };
    let items = match arr.data_type() {
        DataType::List(_) => any.downcast_ref::<ListArray>().map(|a| a.value(row)),
        DataType::LargeList(_) => any.downcast_ref::<LargeListArray>().map(|a| a.value(row)),
        DataType::FixedSizeList(..) => any
            .downcast_ref::<FixedSizeListArray>()
            .map(|a| a.value(row)),
        DataType::ListView(_) => any.downcast_ref::<ListViewArray>().map(|a| a.value(row)),
        DataType::LargeListView(_) => any
            .downcast_ref::<LargeListViewArray>()
            .map(|a| a.value(row)),
        _ => None,
    };
    if let Some(items) = items {
        let t = elem(ty);
        json_list(out, &items, &t, depth);
        return;
    }
    if let Some(s) = any.downcast_ref::<StructArray>() {
        let members: Vec<DuckType> = match ty {
            DuckType::Struct(m) => m.iter().map(|(_, t)| t.clone()).collect(),
            _ => Vec::new(),
        };
        out.push('{');
        for (i, (field, col)) in s.fields().iter().zip(s.columns()).enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            json_string(out, field.name());
            out.push_str(": ");
            let t = members
                .get(i)
                .cloned()
                .unwrap_or(DuckType::Other(String::new()));
            json(out, col.as_ref(), row, &t, depth + 1);
        }
        out.push('}');
        return;
    }
    if let Some(m) = any.downcast_ref::<MapArray>() {
        let (kt, vt) = match ty {
            DuckType::Map(k, v) => ((**k).clone(), (**v).clone()),
            _ => (
                DuckType::Other(String::new()),
                DuckType::Other(String::new()),
            ),
        };
        let entries = m.value(row);
        let (keys, values) = match (entries.columns().first(), entries.columns().get(1)) {
            (Some(k), Some(v)) => (k.clone(), v.clone()),
            _ => {
                out.push_str("{}");
                return;
            }
        };
        out.push('{');
        for i in 0..keys.len().min(values.len()) {
            if i > 0 {
                out.push_str(", ");
            }
            json_string(out, &key_text(keys.as_ref(), i, &kt));
            out.push_str(": ");
            json(out, values.as_ref(), i, &vt, depth + 1);
        }
        out.push('}');
        return;
    }
    if let Some(u) = any.downcast_ref::<UnionArray>() {
        json_union(out, u, row, ty, depth);
        return;
    }
    json_scalar(out, arr, row, ty);
}

fn json_list(out: &mut String, items: &ArrayRef, t: &DuckType, depth: usize) {
    out.push('[');
    for i in 0..items.len() {
        if i > 0 {
            out.push_str(", ");
        }
        json(out, items.as_ref(), i, t, depth + 1);
    }
    out.push(']');
}

fn json_union(out: &mut String, u: &UnionArray, row: usize, ty: &DuckType, depth: usize) {
    let DataType::Union(fields, _) = u.data_type() else {
        out.push_str("null");
        return;
    };
    let tid = u.type_ids().get(row).copied();
    let Some((pos, _)) =
        tid.and_then(|tid| fields.iter().enumerate().find(|(_, (id, _))| *id == tid))
    else {
        out.push_str("null");
        return;
    };
    let member = match ty {
        DuckType::Union(m) => m.get(pos).map(|(_, t)| t.clone()),
        _ => None,
    }
    .unwrap_or(DuckType::Other(String::new()));
    let value = u.value(row);
    json(out, value.as_ref(), 0, &member, depth + 1);
}

/// A map key as object key text: strings as they are, anything else as its JSON.
fn key_text(keys: &dyn Array, i: usize, t: &DuckType) -> String {
    if let Some(s) = text(keys, i) {
        return s.to_string();
    }
    let mut s = String::new();
    json(&mut s, keys, i, t, MAX_JSON_DEPTH);
    s.trim_matches('"').to_string()
}

/// A scalar inside a nested value: numbers and booleans as JSON literals, everything else as a
/// string of its text (dates, UUIDs, blobs as `\xAA` escapes, intervals).
fn json_scalar(out: &mut String, arr: &dyn Array, row: usize, ty: &DuckType) {
    let value = match ty {
        // A child of unknown type, or a nested type whose Arrow shape is not nested (which
        // would come back here): go by the Arrow shape.
        DuckType::Other(name) if name.is_empty() => shape_value(arr, row),
        DuckType::List(_)
        | DuckType::Array(..)
        | DuckType::Struct(_)
        | DuckType::Map(..)
        | DuckType::Union(_) => shape_value(arr, row),
        _ => cell(arr, row, ty, Bytes::Preview),
    };
    match value {
        Value::Null | Value::Default => out.push_str("null"),
        Value::Bool(b) => out.push_str(if b { "true" } else { "false" }),
        Value::Int(n) => {
            let _ = write!(out, "{n}");
        }
        Value::BigInt(s) | Value::Decimal(s) => out.push_str(&s),
        Value::Float(f) => {
            let _ = write!(out, "{f}");
        }
        Value::Json(s) => out.push_str(&s),
        Value::Bytes(_) => {
            let b = binary(arr, row).unwrap_or_default();
            json_string(out, &blob_text(b));
        }
        other => json_string(out, &other.as_text().unwrap_or_default()),
    }
}

/// A child value whose DuckDB type the column's logical type did not tell, by its Arrow shape.
fn shape_value(arr: &dyn Array, row: usize) -> Value {
    let ty = match arr.data_type() {
        DataType::Boolean => DuckType::Boolean,
        DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64 => DuckType::BigInt,
        DataType::Float32 | DataType::Float64 => DuckType::Double,
        DataType::Decimal32(..) | DataType::Decimal64(..) | DataType::Decimal128(..) => {
            DuckType::Decimal { width: 0, scale: 0 }
        }
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => DuckType::Varchar,
        DataType::Binary | DataType::LargeBinary | DataType::BinaryView => DuckType::Blob,
        DataType::Date32 => DuckType::Date,
        DataType::Time64(_) => DuckType::Time,
        DataType::Timestamp(_, None) => DuckType::Timestamp(TsUnit::Micros),
        DataType::Timestamp(_, Some(_)) => DuckType::TimestampTz,
        DataType::Interval(_) => DuckType::Interval,
        DataType::Dictionary(..) => DuckType::Enum(None),
        DataType::List(_)
        | DataType::LargeList(_)
        | DataType::FixedSizeList(..)
        | DataType::ListView(_)
        | DataType::LargeListView(_)
        | DataType::Struct(_)
        | DataType::Map(..)
        | DataType::Union(..) => {
            let mut out = String::new();
            json(
                &mut out,
                arr,
                row,
                &DuckType::Other(String::new()),
                MAX_JSON_DEPTH,
            );
            return Value::Json(out);
        }
        _ => return Value::Text(formatted(arr, row).unwrap_or_default()),
    };
    cell(arr, row, &ty, Bytes::Preview)
}

/// A BLOB as DuckDB prints it: printable ASCII as is, every other byte as `\xAA`.
pub fn blob_text(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len());
    for &byte in b {
        if (0x20..0x7f).contains(&byte) && byte != b'\\' && byte != b'\'' && byte != b'"' {
            s.push(byte as char);
        } else {
            let _ = write!(s, "\\x{byte:02X}");
        }
    }
    s
}

// --- Bind parameters -----------------------------------------------------------------------

/// A `Value` as a DuckDB parameter. Typed values go as text, which DuckDB casts to the type the
/// statement expects of the parameter; bytes as a BLOB, integers as integers.
pub fn to_param(v: &Value) -> Result<::duckdb::types::Value, AppError> {
    use ::duckdb::types::Value as D;
    Ok(match v {
        Value::Null => D::Null,
        Value::Bool(b) => D::Boolean(*b),
        Value::Int(n) => D::BigInt(*n),
        Value::BigInt(s) => match s.trim().parse::<i128>() {
            Ok(n) => match i64::try_from(n) {
                Ok(n) => D::BigInt(n),
                Err(_) => D::HugeInt(n),
            },
            Err(_) => D::Text(s.clone()),
        },
        Value::Float(f) => D::Double(*f),
        Value::Bytes(b) => D::Blob(
            base64::engine::general_purpose::STANDARD
                .decode(&b.base64)
                .map_err(|e| AppError::InvalidInput(format!("bad bytes value: {e}")))?,
        ),
        Value::Array(items) => D::Text(list_literal(items)),
        Value::Default => {
            return Err(AppError::internal("DEFAULT cannot be bound as a parameter"));
        }
        other => D::Text(other.as_text().unwrap_or_default()),
    })
}

/// A list as the text DuckDB casts to a LIST (`[1, 'a''b', NULL]`).
pub fn list_literal(items: &[Value]) -> String {
    let mut out = String::from("[");
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        match item {
            Value::Null | Value::Default => out.push_str("NULL"),
            Value::Array(inner) => out.push_str(&list_literal(inner)),
            Value::Bool(_)
            | Value::Int(_)
            | Value::BigInt(_)
            | Value::Float(_)
            | Value::Decimal(_) => out.push_str(&item.as_text().unwrap_or_default()),
            other => {
                let text = other.as_text().unwrap_or_default();
                out.push('\'');
                out.push_str(&text.replace('\'', "''"));
                out.push('\'');
            }
        }
    }
    out.push(']');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals() {
        assert_eq!(decimal_text(12345, 2), "123.45");
        assert_eq!(decimal_text(-5, 3), "-0.005");
        assert_eq!(decimal_text(0, 2), "0.00");
        assert_eq!(decimal_text(7, 0), "7");
        assert_eq!(decimal_text(7, -2), "700");
        assert_eq!(
            decimal_text(i128::MIN, 0),
            "-170141183460469231731687303715884105728"
        );
    }

    #[test]
    fn temporal_text() {
        assert_eq!(date_text(0).as_deref(), Some("1970-01-01"));
        assert_eq!(date_text(19_753).as_deref(), Some("2024-01-31"));
        // 44 BC: year -43 in the proleptic calendar; chrono agrees within its range.
        use chrono::Datelike as _;
        let bc = chrono::NaiveDate::from_ymd_opt(-43, 3, 15).unwrap();
        let days = bc.num_days_from_ce() as i64 - 719_163;
        assert_eq!(date_text(days).as_deref(), Some("0044-03-15 (BC)"));
        for d in [-1_000_000i64, -1, 59, 60, 11_016, 2_932_896] {
            let c = chrono::NaiveDate::from_num_days_from_ce_opt((d + 719_163) as i32).unwrap();
            assert_eq!(
                civil_from_days(d),
                Some((c.year() as i64, c.month(), c.day())),
                "{d}"
            );
        }
        // DuckDB's extremes, beyond chrono (checked against DuckDB's own text in `testing`).
        assert!(date_text(-2_147_483_646).is_some_and(|d| d.ends_with("(BC)")));
        assert_eq!(date_text(i64::MAX), None);
        // TIMETZ bits: 12:00:00 at +02:00, and -03:30.
        let bits = |micros: u64, offset: i64| (micros << 24) | (57_599 - offset) as u64;
        assert_eq!(time_tz_text(bits(43_200_000_000, 7200)), "12:00:00+02");
        assert_eq!(
            time_tz_text(bits(30_600_000_000, -12_600)),
            "08:30:00-03:30"
        );
        assert_eq!(time_text(0, MICROS_PER_SEC, 6), "00:00:00");
        assert_eq!(
            time_text(12 * 3_600_000_000 + 500_000, MICROS_PER_SEC, 6),
            "12:00:00.5"
        );
        assert_eq!(
            time_text(86_400 * MICROS_PER_SEC, MICROS_PER_SEC, 6),
            "24:00:00"
        );
        assert_eq!(
            timestamp_text(1_706_702_400_123_456, TsUnit::Micros).as_deref(),
            Some("2024-01-31 12:00:00.123456")
        );
        assert_eq!(
            timestamp_text(-1, TsUnit::Seconds).as_deref(),
            Some("1969-12-31 23:59:59")
        );
        assert_eq!(
            timestamp_text(1, TsUnit::Nanos).as_deref(),
            Some("1970-01-01 00:00:00.000000001")
        );
        assert_eq!(
            timestamp_text(i64::MAX, TsUnit::Micros).as_deref(),
            Some("infinity")
        );
        assert_eq!(
            timestamp_text(-i64::MAX, TsUnit::Micros).as_deref(),
            Some("-infinity")
        );
        assert_eq!(
            interval_text(14, 3, 14_706_789_000),
            "1 year 2 months 3 days 04:05:06.789"
        );
        assert_eq!(interval_text(0, 1, 0), "1 day");
        assert_eq!(interval_text(0, 0, -5_400_000_000), "-01:30:00");
        assert_eq!(interval_text(0, 0, 0), "00:00:00");
        assert_eq!(interval_text(-25, -2, 0), "-2 years -1 month -2 days");
    }

    #[test]
    fn bits_and_bignums() {
        // BIT '10101': 3 padding bits (set), then 10101.
        assert_eq!(bit_text(&[3, 0b1111_0101]).as_deref(), Some("10101"));
        assert_eq!(
            bit_text(&[0, 0xff, 0x00]).as_deref(),
            Some("1111111100000000")
        );
        assert_eq!(bit_text(&[9, 0]), None);
        assert_eq!(bit_text(&[]), None);
        assert_eq!(bignum_text(&[0x80, 0, 1, 42]).as_deref(), Some("42"));
        assert_eq!(
            bignum_text(&[0x7f, 0xff, 0xfe, !42]).as_deref(),
            Some("-42")
        );
        assert_eq!(bignum_text(&[0x80, 0, 2, 1, 0]).as_deref(), Some("256"));
        assert_eq!(bignum_text(&[1]), None);
    }

    #[test]
    fn integers_past_two_to_the_53_are_text() {
        assert_eq!(int_value(42), Value::Int(42));
        assert_eq!(
            int_value(9_007_199_254_740_993),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            uint_value(u128::MAX),
            Value::BigInt("340282366920938463463374607431768211455".into())
        );
    }

    #[test]
    fn params() {
        use ::duckdb::types::Value as D;
        assert_eq!(to_param(&Value::Int(1)).unwrap(), D::BigInt(1));
        assert_eq!(
            to_param(&Value::BigInt(
                "170141183460469231731687303715884105727".into()
            ))
            .unwrap(),
            D::HugeInt(i128::MAX)
        );
        assert_eq!(
            to_param(&Value::Decimal("1.50".into())).unwrap(),
            D::Text("1.50".into())
        );
        assert_eq!(
            to_param(&Value::Array(vec![
                Value::Int(1),
                Value::Text("a'b".into()),
                Value::Null
            ]))
            .unwrap(),
            D::Text("[1, 'a''b', NULL]".into())
        );
        assert!(to_param(&Value::Default).is_err());
        assert_eq!(blob_text(b"ab\x00'"), "ab\\x00\\x27");
    }
}
