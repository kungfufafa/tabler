//! Snowflake's values as `Value`s. The JSON result format sends every cell as text (or null),
//! whatever its type: NUMBER as its decimal digits, DATE as days since the epoch, TIME and
//! TIMESTAMP_NTZ / LTZ as seconds since midnight or the epoch with the column's fraction digits,
//! TIMESTAMP_TZ as those seconds and an offset index (minutes + 1440), BINARY as hex, VARIANT /
//! OBJECT / ARRAY as JSON text, GEOGRAPHY / GEOMETRY in the session's output format (GeoJSON by
//! default), VECTOR as a JSON array. The decoders are total: text a type does not expect becomes
//! `Other` with the text as sent, never a panic.
//!
//! Temporal values keep the server's precision and read as Snowflake's own input accepts them:
//! `2024-05-01`, `13:45:00.123`, `2024-05-01 13:45:00.123456789`, and TIMESTAMP_TZ with its
//! offset (`… +07:00`). TIMESTAMP_LTZ arrives as an instant only (the session's time zone is not
//! in the result), so it shows in UTC with `+00:00`: the same instant, which Snowflake reads back
//! exactly.

use std::sync::Arc;

use chrono::{DateTime, Days, FixedOffset, NaiveDate, NaiveTime, Utc};
use snowflake_connector_rs::decode::{PlanBuildResult, RowDecodeResult};
use snowflake_connector_rs::{Column, ColumnType, FromRow, RowPlanContext, RowRef};

use crate::types::query::ResultColumn;
use crate::types::schema::ValueKind;
use crate::types::value::Value;

/// A column type as the decoders and the catalog see it: the connector's `ColumnType` (from a
/// result's row type) or the `data_type` JSON of `SHOW COLUMNS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SfType {
    /// NUMBER / DECIMAL / INT: FIXED with a precision and scale.
    Fixed {
        precision: Option<u8>,
        scale: Option<u8>,
    },
    /// FLOAT / DOUBLE / REAL.
    Real,
    /// DECFLOAT: up to 38 significant digits and an exponent.
    Decfloat,
    Text {
        length: Option<u32>,
    },
    Binary {
        length: Option<u32>,
    },
    Boolean,
    Date,
    Time {
        scale: Option<u8>,
    },
    TimestampNtz {
        scale: Option<u8>,
    },
    TimestampLtz {
        scale: Option<u8>,
    },
    TimestampTz {
        scale: Option<u8>,
    },
    Variant,
    Object,
    Array,
    /// A structured MAP.
    Map,
    Geography,
    Geometry,
    Vector,
    /// A FILE value (an object describing a staged file).
    File,
    /// A type this build does not know, as the server names it (lower case).
    Other(String),
}

impl SfType {
    /// The type of a result column's row type.
    pub fn of(ty: &ColumnType) -> SfType {
        match ty {
            ColumnType::Fixed {
                precision, scale, ..
            } => SfType::Fixed {
                precision: *precision,
                scale: *scale,
            },
            ColumnType::Real => SfType::Real,
            ColumnType::Text { length, .. } => SfType::Text { length: *length },
            ColumnType::Binary { length, .. } => SfType::Binary { length: *length },
            ColumnType::Boolean => SfType::Boolean,
            ColumnType::Date => SfType::Date,
            ColumnType::Time { scale, .. } => SfType::Time { scale: *scale },
            ColumnType::TimestampNtz { scale, .. } => SfType::TimestampNtz { scale: *scale },
            ColumnType::TimestampLtz { scale, .. } => SfType::TimestampLtz { scale: *scale },
            ColumnType::TimestampTz { scale, .. } => SfType::TimestampTz { scale: *scale },
            ColumnType::Variant => SfType::Variant,
            ColumnType::Object => SfType::Object,
            ColumnType::Array => SfType::Array,
            ColumnType::Geography => SfType::Geography,
            ColumnType::Geometry => SfType::Geometry,
            ColumnType::Vector => SfType::Vector,
            other => SfType::named(other.as_str(), None, None, None),
        }
    }

    /// The type a row type or `SHOW COLUMNS` names (`fixed`, `TEXT`, `timestamp_ntz`, …), with
    /// the length, precision and scale given beside it.
    pub fn named(
        name: &str,
        length: Option<u32>,
        precision: Option<u8>,
        scale: Option<u8>,
    ) -> SfType {
        match name.trim().to_ascii_lowercase().as_str() {
            "fixed" | "number" | "decimal" | "numeric" | "int" | "integer" | "bigint"
            | "smallint" | "tinyint" | "byteint" => SfType::Fixed { precision, scale },
            "real" | "float" | "float4" | "float8" | "double" | "double precision" => SfType::Real,
            "decfloat" => SfType::Decfloat,
            "text" | "varchar" | "char" | "character" | "string" => SfType::Text { length },
            "binary" | "varbinary" => SfType::Binary { length },
            "boolean" => SfType::Boolean,
            "date" => SfType::Date,
            "time" => SfType::Time { scale },
            "timestamp_ntz" | "datetime" => SfType::TimestampNtz { scale },
            "timestamp_ltz" => SfType::TimestampLtz { scale },
            "timestamp_tz" => SfType::TimestampTz { scale },
            "variant" => SfType::Variant,
            "object" => SfType::Object,
            "array" => SfType::Array,
            "map" => SfType::Map,
            "geography" => SfType::Geography,
            "geometry" => SfType::Geometry,
            "vector" => SfType::Vector,
            "file" => SfType::File,
            other => SfType::Other(other.to_string()),
        }
    }

    /// The type as Snowflake SQL spells it (`NUMBER(38,0)`, `VARCHAR(16777216)`,
    /// `TIMESTAMP_NTZ(9)`).
    pub fn sql_name(&self) -> String {
        let scaled = |name: &str, scale: &Option<u8>| match scale {
            Some(s) => format!("{name}({s})"),
            None => name.to_string(),
        };
        let sized = |name: &str, length: &Option<u32>| match length {
            Some(n) => format!("{name}({n})"),
            None => name.to_string(),
        };
        match self {
            SfType::Fixed { precision, scale } => match (precision, scale) {
                (Some(p), Some(s)) => format!("NUMBER({p},{s})"),
                (Some(p), None) => format!("NUMBER({p})"),
                _ => "NUMBER".into(),
            },
            SfType::Real => "FLOAT".into(),
            SfType::Decfloat => "DECFLOAT".into(),
            SfType::Text { length } => sized("VARCHAR", length),
            SfType::Binary { length } => sized("BINARY", length),
            SfType::Boolean => "BOOLEAN".into(),
            SfType::Date => "DATE".into(),
            SfType::Time { scale } => scaled("TIME", scale),
            SfType::TimestampNtz { scale } => scaled("TIMESTAMP_NTZ", scale),
            SfType::TimestampLtz { scale } => scaled("TIMESTAMP_LTZ", scale),
            SfType::TimestampTz { scale } => scaled("TIMESTAMP_TZ", scale),
            SfType::Variant => "VARIANT".into(),
            SfType::Object => "OBJECT".into(),
            SfType::Array => "ARRAY".into(),
            SfType::Map => "MAP".into(),
            SfType::Geography => "GEOGRAPHY".into(),
            SfType::Geometry => "GEOMETRY".into(),
            SfType::Vector => "VECTOR".into(),
            SfType::File => "FILE".into(),
            SfType::Other(name) => name.to_ascii_uppercase(),
        }
    }

    pub fn kind(&self) -> ValueKind {
        match self {
            SfType::Fixed { scale, precision } => match scale {
                Some(s) if *s > 0 => ValueKind::Decimal,
                // NUMBER(38,0), the type of every INT column, outgrows a JS number.
                _ if precision.unwrap_or(38) <= 15 => ValueKind::Int,
                _ => ValueKind::BigInt,
            },
            SfType::Real => ValueKind::Float,
            SfType::Decfloat => ValueKind::Decimal,
            SfType::Text { .. } => ValueKind::Text,
            SfType::Binary { .. } => ValueKind::Bytes,
            SfType::Boolean => ValueKind::Bool,
            SfType::Date => ValueKind::Date,
            SfType::Time { .. } => ValueKind::Time,
            SfType::TimestampNtz { .. } => ValueKind::Timestamp,
            SfType::TimestampLtz { .. } | SfType::TimestampTz { .. } => ValueKind::TimestampTz,
            SfType::Variant | SfType::Object | SfType::Array | SfType::Map | SfType::File => {
                ValueKind::Json
            }
            SfType::Geography | SfType::Geometry => ValueKind::Geometry,
            SfType::Vector => ValueKind::Array,
            SfType::Other(_) => ValueKind::Other,
        }
    }

    /// The name an `Other` value of this type carries.
    fn other_name(&self) -> String {
        self.sql_name().to_ascii_lowercase()
    }
}

/// A result column as the editor shows it.
pub fn result_column(col: &Column) -> ResultColumn {
    let ty = SfType::of(col.ty());
    ResultColumn {
        name: col.name().to_string(),
        data_type: ty.sql_name(),
        kind: ty.kind(),
        nullable: Some(col.is_nullable()),
        table: None,
        origin: None,
    }
}

/// One cell: NULL, or the text the server sent for a value of `ty`.
pub fn cell(ty: &SfType, raw: Option<&str>) -> Value {
    let Some(raw) = raw else {
        return Value::Null;
    };
    let other = || Value::other(&ty.other_name(), raw);
    match ty {
        SfType::Fixed { scale, .. } => match scale {
            Some(s) if *s > 0 => decimal(raw).unwrap_or_else(other),
            _ => integer(raw).or_else(|| decimal(raw)).unwrap_or_else(other),
        },
        SfType::Real => match raw.trim().parse::<f64>() {
            Ok(f) => Value::from_f64("float", f),
            Err(_) => other(),
        },
        SfType::Decfloat => decimal_with_exponent(raw).unwrap_or_else(other),
        SfType::Text { .. } => Value::Text(raw.to_string()),
        SfType::Binary { .. } => match hex(raw) {
            Some(bytes) => Value::from_bytes(&bytes),
            None => other(),
        },
        SfType::Boolean => match raw.trim() {
            "1" => Value::Bool(true),
            "0" => Value::Bool(false),
            t if t.eq_ignore_ascii_case("true") => Value::Bool(true),
            t if t.eq_ignore_ascii_case("false") => Value::Bool(false),
            _ => other(),
        },
        SfType::Date => date(raw).map(Value::Date).unwrap_or_else(other),
        SfType::Time { scale } => time(raw, digits(*scale))
            .map(Value::Time)
            .unwrap_or_else(other),
        SfType::TimestampNtz { scale } => epoch(raw)
            .map(|dt| Value::Timestamp(timestamp_text(&dt.naive_utc(), digits(*scale))))
            .unwrap_or_else(other),
        SfType::TimestampLtz { scale } => epoch(raw)
            .map(|dt| {
                Value::TimestampTz(format!(
                    "{} +00:00",
                    timestamp_text(&dt.naive_utc(), digits(*scale))
                ))
            })
            .unwrap_or_else(other),
        SfType::TimestampTz { scale } => timestamp_tz(raw, digits(*scale))
            .map(Value::TimestampTz)
            .unwrap_or_else(other),
        SfType::Variant | SfType::Object | SfType::Array | SfType::Map | SfType::File => {
            // NaN and Infinity in a VARIANT are no JSON; keep the text as it came.
            if serde_json::from_str::<serde_json::Value>(raw).is_ok() {
                Value::Json(raw.to_string())
            } else {
                other()
            }
        }
        SfType::Geography | SfType::Geometry => other(),
        SfType::Vector => vector(raw).unwrap_or_else(other),
        SfType::Other(_) => other(),
    }
}

/// Fraction digits of a time type; Snowflake's default is 9.
fn digits(scale: Option<u8>) -> usize {
    usize::from(scale.unwrap_or(9).min(9))
}

/// An integer as `Int` (within 2^53) or `BigInt`, with its sign and digits only.
fn integer(raw: &str) -> Option<Value> {
    let t = raw.trim();
    let digits = t.strip_prefix(['-', '+']).unwrap_or(t);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(match t.parse::<i64>() {
        Ok(n) => Value::from_i64(n),
        Err(_) => Value::BigInt(t.trim_start_matches('+').to_string()),
    })
}

/// A decimal as the server wrote it (`-12.50`), when it is one.
fn decimal(raw: &str) -> Option<Value> {
    let t = raw.trim();
    let body = t.strip_prefix(['-', '+']).unwrap_or(t);
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let ok = !(int.is_empty() && frac.is_empty())
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| Value::Decimal(t.trim_start_matches('+').to_string()))
}

/// A DECFLOAT, which may carry an exponent (`1.5E+40`).
fn decimal_with_exponent(raw: &str) -> Option<Value> {
    let t = raw.trim();
    let (mantissa, exp) = match t.find(['e', 'E']) {
        Some(at) => (&t[..at], Some(&t[at + 1..])),
        None => (t, None),
    };
    let exp_ok = exp.is_none_or(|e| {
        let d = e.strip_prefix(['-', '+']).unwrap_or(e);
        !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())
    });
    (exp_ok && decimal(mantissa).is_some()).then(|| Value::Decimal(t.to_string()))
}

fn hex(raw: &str) -> Option<Vec<u8>> {
    let b = raw.trim().as_bytes();
    if !b.len().is_multiple_of(2) {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    b.chunks(2)
        .map(|p| Some((nibble(p[0])? << 4) | nibble(p[1])?))
        .collect()
}

fn epoch_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default()
}

/// Days since 1970-01-01 as `YYYY-MM-DD`.
fn date(raw: &str) -> Option<String> {
    let days: i64 = raw.trim().parse().ok()?;
    let d = if days >= 0 {
        epoch_date().checked_add_days(Days::new(days.unsigned_abs()))?
    } else {
        epoch_date().checked_sub_days(Days::new(days.unsigned_abs()))?
    };
    Some(d.format("%Y-%m-%d").to_string())
}

/// `[-]seconds[.fraction]` as whole seconds (rounded down) and nanoseconds.
fn seconds(raw: &str) -> Option<(i64, u32)> {
    let t = raw.trim();
    let (negative, body) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if int.is_empty()
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
        || frac.len() > 9
    {
        return None;
    }
    let whole: i128 = int.parse().ok()?;
    let mut nanos: i128 = 0;
    for (i, b) in frac.bytes().enumerate() {
        nanos += i128::from(b - b'0') * 10i128.pow(8 - i as u32);
    }
    let mut total = whole.checked_mul(1_000_000_000)?.checked_add(nanos)?;
    if negative {
        total = -total;
    }
    let secs = i64::try_from(total.div_euclid(1_000_000_000)).ok()?;
    let nanos = u32::try_from(total.rem_euclid(1_000_000_000)).ok()?;
    Some((secs, nanos))
}

fn epoch(raw: &str) -> Option<DateTime<Utc>> {
    let (secs, nanos) = seconds(raw)?;
    DateTime::from_timestamp(secs, nanos)
}

/// `.123` with `digits` fraction digits (none for 0).
fn fraction(nanos: u32, digits: usize) -> String {
    if digits == 0 {
        return String::new();
    }
    let all = format!("{:09}", nanos.min(999_999_999));
    format!(".{}", &all[..digits])
}

fn timestamp_text(dt: &chrono::NaiveDateTime, digits: usize) -> String {
    use chrono::Timelike as _;
    format!(
        "{}{}",
        dt.format("%Y-%m-%d %H:%M:%S"),
        fraction(dt.nanosecond(), digits)
    )
}

/// Seconds since midnight as `HH:MM:SS[.fff]`.
fn time(raw: &str, digits: usize) -> Option<String> {
    let (secs, nanos) = seconds(raw)?;
    let t = NaiveTime::from_num_seconds_from_midnight_opt(u32::try_from(secs).ok()?, nanos)?;
    Some(format!(
        "{}{}",
        t.format("%H:%M:%S"),
        fraction(nanos, digits)
    ))
}

/// `epoch offset` (the offset index is minutes + 1440), or the older single number holding
/// `epoch * 16384 + index`, as local time with its offset.
fn timestamp_tz(raw: &str, digits: usize) -> Option<String> {
    let mut parts = raw.split_whitespace();
    let first = parts.next()?;
    let (utc, index) = match (parts.next(), parts.next()) {
        (Some(index), None) => (epoch(first)?, index.parse::<i64>().ok()?),
        (None, None) => {
            // Packed: the epoch in the column's scale, then the index in the low 14 bits.
            let packed: i128 = first.parse().ok()?;
            let scaled = packed.div_euclid(16_384);
            let index = i64::try_from(packed.rem_euclid(16_384)).ok()?;
            let unit = 10i128.pow(u32::try_from(digits).ok()?);
            let secs = i64::try_from(scaled.div_euclid(unit)).ok()?;
            let rest = scaled.rem_euclid(unit) * 10i128.pow(9 - u32::try_from(digits).ok()?);
            (
                DateTime::from_timestamp(secs, u32::try_from(rest).ok()?)?,
                index,
            )
        }
        _ => return None,
    };
    if !(0..=2880).contains(&index) {
        return None;
    }
    let minutes = i32::try_from(index - 1440).ok()?;
    let offset = FixedOffset::east_opt(minutes * 60)?;
    // `naive_local()` panics when the offset carries the instant past chrono's range.
    let local = utc.naive_utc().checked_add_offset(offset)?;
    let sign = if minutes < 0 { '-' } else { '+' };
    let m = minutes.unsigned_abs();
    Some(format!(
        "{} {sign}{:02}:{:02}",
        timestamp_text(&local, digits),
        m / 60,
        m % 60
    ))
}

/// `[1,2,3]` or `[1.5,inf]` as an array of numbers.
fn vector(raw: &str) -> Option<Value> {
    let inner = raw.trim().strip_prefix('[')?.strip_suffix(']')?.trim();
    if inner.is_empty() {
        return Some(Value::Array(Vec::new()));
    }
    inner
        .split(',')
        .map(|t| {
            let t = t.trim();
            if let Ok(n) = t.parse::<i64>() {
                Some(Value::from_i64(n))
            } else {
                t.parse::<f64>().ok().map(|f| Value::from_f64("float", f))
            }
        })
        .collect::<Option<Vec<_>>>()
        .map(Value::Array)
}

/// A result row decoded into `Value`s (the connector's `FromRow`, with the column types as its
/// plan).
#[derive(Debug, Clone, PartialEq)]
pub struct ValueRow(pub Vec<Value>);

impl FromRow for ValueRow {
    type Plan = Arc<[SfType]>;

    fn build_plan(ctx: RowPlanContext<'_>) -> PlanBuildResult<Self::Plan> {
        Ok(ctx
            .schema()
            .columns()
            .iter()
            .map(|c| SfType::of(c.ty()))
            .collect())
    }

    fn from_row_with_plan(row: RowRef<'_>, plan: &Self::Plan) -> RowDecodeResult<Self> {
        let mut out = Vec::with_capacity(plan.len());
        for (i, ty) in plan.iter().enumerate() {
            let c = row.cell_at(i)?;
            out.push(cell(ty, c.raw()));
        }
        Ok(ValueRow(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(scale: u8) -> SfType {
        SfType::Fixed {
            precision: Some(38),
            scale: Some(scale),
        }
    }

    #[test]
    fn numbers_keep_every_digit() {
        assert_eq!(cell(&fixed(0), Some("42")), Value::Int(42));
        assert_eq!(
            cell(&fixed(0), Some("-9007199254740991")),
            Value::Int(-9007199254740991)
        );
        // 2^53 + 1 and beyond i64 stay exact.
        assert_eq!(
            cell(&fixed(0), Some("9007199254740993")),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            cell(&fixed(0), Some("-99999999999999999999999999999999999999")),
            Value::BigInt("-99999999999999999999999999999999999999".into())
        );
        assert_eq!(
            cell(&fixed(2), Some("-12.50")),
            Value::Decimal("-12.50".into())
        );
        assert_eq!(cell(&fixed(2), Some(".5")), Value::Decimal(".5".into()));
        // A FIXED without a scale that still carries one, and junk, are not lost.
        assert_eq!(
            cell(&SfType::named("fixed", None, None, None), Some("1.5")),
            Value::Decimal("1.5".into())
        );
        assert_eq!(
            cell(&fixed(0), Some("12a")),
            Value::other("number(38,0)", "12a")
        );
        assert_eq!(
            cell(&fixed(2), Some("1.2.3")),
            Value::other("number(38,2)", "1.2.3")
        );
        assert_eq!(cell(&fixed(0), Some("")), Value::other("number(38,0)", ""));
        assert_eq!(cell(&fixed(0), None), Value::Null);
        assert_eq!(
            cell(&SfType::Decfloat, Some("1.5E+40")),
            Value::Decimal("1.5E+40".into())
        );
        assert_eq!(
            cell(&SfType::Decfloat, Some("1e")),
            Value::other("decfloat", "1e")
        );
        assert_eq!(cell(&SfType::Real, Some("1.5")), Value::Float(1.5));
        assert_eq!(cell(&SfType::Real, Some("1e300")), Value::Float(1e300));
        assert_eq!(
            cell(&SfType::Real, Some("NaN")),
            Value::other("float", "NaN")
        );
        assert_eq!(
            cell(&SfType::Real, Some("inf")),
            Value::other("float", "inf")
        );
        assert_eq!(
            cell(&SfType::Real, Some("-inf")),
            Value::other("float", "-inf")
        );
        assert_eq!(cell(&SfType::Real, Some("x")), Value::other("float", "x"));
    }

    #[test]
    fn text_bool_and_binary() {
        let text = SfType::Text { length: Some(16) };
        assert_eq!(
            cell(&text, Some("日本語 ✓")),
            Value::Text("日本語 ✓".into())
        );
        assert_eq!(cell(&SfType::Boolean, Some("1")), Value::Bool(true));
        assert_eq!(cell(&SfType::Boolean, Some("false")), Value::Bool(false));
        assert_eq!(
            cell(&SfType::Boolean, Some("maybe")),
            Value::other("boolean", "maybe")
        );
        let bin = SfType::Binary { length: None };
        assert_eq!(
            cell(&bin, Some("00FFa0")),
            Value::from_bytes(&[0, 255, 160])
        );
        assert_eq!(cell(&bin, Some("")), Value::from_bytes(&[]));
        assert_eq!(cell(&bin, Some("0")), Value::other("binary", "0"));
        assert_eq!(cell(&bin, Some("zz")), Value::other("binary", "zz"));
    }

    #[test]
    fn dates_times_and_timestamps_as_text() {
        assert_eq!(
            cell(&SfType::Date, Some("0")),
            Value::Date("1970-01-01".into())
        );
        assert_eq!(
            cell(&SfType::Date, Some("19844")),
            Value::Date("2024-05-01".into())
        );
        assert_eq!(
            cell(&SfType::Date, Some("-1")),
            Value::Date("1969-12-31".into())
        );
        assert_eq!(
            cell(&SfType::Date, Some("99999999999")),
            Value::other("date", "99999999999")
        );
        let t3 = SfType::Time { scale: Some(3) };
        assert_eq!(
            cell(&t3, Some("49500.123")),
            Value::Time("13:45:00.123".into())
        );
        assert_eq!(
            cell(&SfType::Time { scale: Some(0) }, Some("0")),
            Value::Time("00:00:00".into())
        );
        assert_eq!(
            cell(&SfType::Time { scale: None }, Some("86399.999999999")),
            Value::Time("23:59:59.999999999".into())
        );
        assert_eq!(cell(&t3, Some("86400")), Value::other("time(3)", "86400"));
        assert_eq!(cell(&t3, Some("-1")), Value::other("time(3)", "-1"));

        let ntz = SfType::TimestampNtz { scale: Some(9) };
        assert_eq!(
            cell(&ntz, Some("1714571100.123456789")),
            Value::Timestamp("2024-05-01 13:45:00.123456789".into())
        );
        assert_eq!(
            cell(
                &SfType::TimestampNtz { scale: Some(3) },
                Some("1714571100.5")
            ),
            Value::Timestamp("2024-05-01 13:45:00.500".into())
        );
        // Before 1970: the seconds are negative and the fraction counts up from them.
        assert_eq!(
            cell(&SfType::TimestampNtz { scale: Some(1) }, Some("-1.5")),
            Value::Timestamp("1969-12-31 23:59:58.5".into())
        );
        assert_eq!(
            cell(&ntz, Some("1e9")),
            Value::other("timestamp_ntz(9)", "1e9")
        );
        assert_eq!(
            cell(&ntz, Some("99999999999999999999")),
            Value::other("timestamp_ntz(9)", "99999999999999999999")
        );
        assert_eq!(
            cell(
                &SfType::TimestampLtz { scale: Some(3) },
                Some("1714571100.000")
            ),
            Value::TimestampTz("2024-05-01 13:45:00.000 +00:00".into())
        );
        let tz = SfType::TimestampTz { scale: Some(3) };
        // 1440 + 420: UTC+07:00.
        assert_eq!(
            cell(&tz, Some("1714571100.000 1860")),
            Value::TimestampTz("2024-05-01 20:45:00.000 +07:00".into())
        );
        assert_eq!(
            cell(&tz, Some("1714571100.000 1110")),
            Value::TimestampTz("2024-05-01 08:15:00.000 -05:30".into())
        );
        // The packed form: epoch in milliseconds * 16384 + index.
        let packed = (1_714_571_100_000i128 * 16_384 + 1440).to_string();
        assert_eq!(
            cell(&tz, Some(&packed)),
            Value::TimestampTz("2024-05-01 13:45:00.000 +00:00".into())
        );
        for bad in ["1714571100.000 9999", "x 1440", "1 2 3", ""] {
            assert_eq!(cell(&tz, Some(bad)), Value::other("timestamp_tz(3)", bad));
        }
    }

    #[test]
    fn nested_spatial_and_vectors() {
        let json = "{\n  \"a\": [1, 2.5e300, 12345678901234567890]\n}";
        assert_eq!(cell(&SfType::Variant, Some(json)), Value::Json(json.into()));
        assert_eq!(cell(&SfType::Array, Some("[]")), Value::Json("[]".into()));
        assert_eq!(
            cell(&SfType::Object, Some("\"s\"")),
            Value::Json("\"s\"".into())
        );
        assert_eq!(
            cell(&SfType::Variant, Some("NaN")),
            Value::other("variant", "NaN")
        );
        let geo = "{\"coordinates\": [1, 2], \"type\": \"Point\"}";
        assert_eq!(
            cell(&SfType::Geography, Some(geo)),
            Value::other("geography", geo)
        );
        assert_eq!(
            cell(&SfType::Geometry, Some("POINT(1 2)")),
            Value::other("geometry", "POINT(1 2)")
        );
        assert_eq!(
            cell(&SfType::Vector, Some("[1,2,9007199254740993]")),
            Value::Array(vec![
                Value::Int(1),
                Value::Int(2),
                Value::BigInt("9007199254740993".into())
            ])
        );
        assert_eq!(
            cell(&SfType::Vector, Some("[1.5,inf]")),
            Value::Array(vec![Value::Float(1.5), Value::other("float", "inf")])
        );
        assert_eq!(
            cell(&SfType::Vector, Some("[a]")),
            Value::other("vector", "[a]")
        );
        assert_eq!(
            cell(&SfType::Other("weird".into()), Some("x")),
            Value::other("weird", "x")
        );
    }

    #[test]
    fn types_name_and_kind() {
        assert_eq!(fixed(0).sql_name(), "NUMBER(38,0)");
        assert_eq!(fixed(0).kind(), ValueKind::BigInt);
        assert_eq!(
            SfType::Fixed {
                precision: Some(9),
                scale: Some(0)
            }
            .kind(),
            ValueKind::Int
        );
        assert_eq!(fixed(2).kind(), ValueKind::Decimal);
        assert_eq!(
            SfType::named("TEXT", Some(16_777_216), None, None).sql_name(),
            "VARCHAR(16777216)"
        );
        assert_eq!(
            SfType::named("timestamp_tz", None, Some(0), Some(9)).sql_name(),
            "TIMESTAMP_TZ(9)"
        );
        assert_eq!(
            SfType::named("GEOGRAPHY", None, None, None).kind(),
            ValueKind::Geometry
        );
        assert_eq!(
            SfType::named("vector", None, None, None).kind(),
            ValueKind::Array
        );
        assert_eq!(
            SfType::named("decfloat", None, None, None).kind(),
            ValueKind::Decimal
        );
        assert_eq!(
            SfType::named("map", None, None, None).kind(),
            ValueKind::Json
        );
        assert_eq!(
            SfType::named("Mystery", None, None, None),
            SfType::Other("mystery".into())
        );
    }

    #[test]
    fn decoders_never_panic_on_odd_text() {
        let types = [
            fixed(0),
            fixed(5),
            SfType::Real,
            SfType::Decfloat,
            SfType::Binary { length: None },
            SfType::Boolean,
            SfType::Date,
            SfType::Time { scale: Some(12) },
            SfType::TimestampNtz { scale: None },
            SfType::TimestampLtz { scale: Some(0) },
            SfType::TimestampTz { scale: Some(9) },
            SfType::TimestampTz { scale: Some(0) },
            SfType::Variant,
            SfType::Vector,
        ];
        let odd = [
            "",
            " ",
            "-",
            "+",
            ".",
            "-.",
            "1.",
            "--1",
            "1e",
            "١٢",
            "\u{0}",
            "9".repeat(60).leak(),
            "-9223372036854775808",
            "18446744073709551616",
            "1.0000000001",
            "1 1440 1",
            "-99999999999999999999.5 1440",
            "[",
            "]",
            "[,]",
            "{",
            "\"",
            "ff",
            "0x10",
            // Instants at chrono's ends that an offset carries past them.
            "8210266876799 1860",
            "8210266876798 1860",
            "8210266876799.999999999 2880",
            "-8334601228800 1000",
            "-8334601228800 0",
            "134517012509476676",
            "-136554106532658200",
        ];
        for ty in &types {
            for raw in odd {
                let _ = cell(ty, Some(raw));
            }
        }
        // Past the end with its offset is no local time: the text as sent.
        let tz = SfType::TimestampTz { scale: Some(0) };
        for raw in [
            "8210266876799 1860",
            "-8334601228800 1000",
            "134517012509476676",
            "-136554106532658200",
        ] {
            assert_eq!(cell(&tz, Some(raw)), Value::other("timestamp_tz(0)", raw));
        }
    }
}
