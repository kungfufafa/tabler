//! tiberius values as Tabler's `Value`s, and back as bind parameters.
//!
//! Every decoder is total: a value the server should never send (a day count past 9999-12-31, a
//! time scale above 7) becomes `Other` with what is known of it, never a panic. Temporal values
//! are written the way SQL Server prints them (`CONVERT(…, 121)`): datetime with three decimals,
//! datetime2 and time with as many as their scale, datetimeoffset in its own offset.

use std::borrow::Cow;

use ::tiberius::numeric::Numeric;
use ::tiberius::{Column, ColumnData, ColumnType, Row, Uuid};
use base64::Engine as _;
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

use crate::error::AppError;
use crate::types::query::ResultColumn;
use crate::types::schema::ValueKind;
use crate::types::value::Value;

/// How much of a binary value a cell keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bytes {
    /// The grid's preview (`Value::from_bytes`).
    Preview,
    /// All of it (`fetch_cell`'s "load full value").
    Whole,
}

/// The type name a result column shows and the kind of its values. The TDS metadata tells
/// money from smallmoney and datetime from smalldatetime only for NOT NULL columns.
pub fn type_info(ty: ColumnType) -> (&'static str, ValueKind) {
    use ColumnType as T;
    match ty {
        T::Null => ("null", ValueKind::Other),
        T::Bit | T::Bitn => ("bit", ValueKind::Bool),
        T::Int1 => ("tinyint", ValueKind::Int),
        T::Int2 => ("smallint", ValueKind::Int),
        T::Int4 | T::Intn => ("int", ValueKind::Int),
        T::Int8 => ("bigint", ValueKind::BigInt),
        T::Float4 => ("real", ValueKind::Float),
        T::Float8 | T::Floatn => ("float", ValueKind::Float),
        T::Money => ("money", ValueKind::Decimal),
        T::Money4 => ("smallmoney", ValueKind::Decimal),
        T::Decimaln => ("decimal", ValueKind::Decimal),
        T::Numericn => ("numeric", ValueKind::Decimal),
        T::Datetime4 => ("smalldatetime", ValueKind::Timestamp),
        T::Datetime | T::Datetimen => ("datetime", ValueKind::Timestamp),
        T::Daten => ("date", ValueKind::Date),
        T::Timen => ("time", ValueKind::Time),
        T::Datetime2 => ("datetime2", ValueKind::Timestamp),
        T::DatetimeOffsetn => ("datetimeoffset", ValueKind::TimestampTz),
        T::Guid => ("uniqueidentifier", ValueKind::Uuid),
        T::BigVarBin => ("varbinary", ValueKind::Bytes),
        T::BigBinary => ("binary", ValueKind::Bytes),
        T::Image => ("image", ValueKind::Bytes),
        T::BigVarChar => ("varchar", ValueKind::Text),
        T::BigChar => ("char", ValueKind::Text),
        T::NVarchar => ("nvarchar", ValueKind::Text),
        T::NChar => ("nchar", ValueKind::Text),
        T::Text => ("text", ValueKind::Text),
        T::NText => ("ntext", ValueKind::Text),
        T::Xml => ("xml", ValueKind::Text),
        // CLR types (hierarchyid, geometry, geography, assemblies) arrive serialized.
        T::Udt => ("udt", ValueKind::Bytes),
        T::SSVariant => ("sql_variant", ValueKind::Other),
    }
}

/// A result column as the grid shows it. TDS metadata names no base table, so result sets are
/// not editable by origin.
pub fn result_column(c: &Column) -> ResultColumn {
    let (data_type, kind) = type_info(c.column_type());
    ResultColumn {
        name: c.name().to_string(),
        data_type: data_type.to_string(),
        kind,
        nullable: None,
        table: None,
        origin: None,
    }
}

pub fn result_columns(columns: &[Column]) -> Vec<ResultColumn> {
    columns.iter().map(result_column).collect()
}

/// A row's values, in column order.
pub fn decode_row(row: Row, bytes: Bytes) -> Vec<Value> {
    let types: Vec<ColumnType> = row.columns().iter().map(Column::column_type).collect();
    row.into_iter()
        .zip(types)
        .map(|(data, ty)| decode_cell(ty, data, bytes))
        .collect()
}

/// One value of a column of type `ty`.
pub fn decode_cell(ty: ColumnType, data: ColumnData<'_>, bytes: Bytes) -> Value {
    match data {
        ColumnData::U8(v) => v.map_or(Value::Null, |n| Value::Int(i64::from(n))),
        ColumnData::I16(v) => v.map_or(Value::Null, |n| Value::Int(i64::from(n))),
        ColumnData::I32(v) => v.map_or(Value::Null, |n| Value::Int(i64::from(n))),
        ColumnData::I64(v) => v.map_or(Value::Null, Value::from_i64),
        // The shortest text that reads back as the same f32 (0.1, not 0.100000001490116).
        ColumnData::F32(v) => v.map_or(Value::Null, |f| {
            Value::from_f64("real", f.to_string().parse().unwrap_or(f64::from(f)))
        }),
        ColumnData::F64(v) => v.map_or(Value::Null, |f| match ty {
            ColumnType::Money | ColumnType::Money4 => money(f),
            _ => Value::from_f64("float", f),
        }),
        ColumnData::Bit(v) => v.map_or(Value::Null, Value::Bool),
        ColumnData::String(v) => v.map_or(Value::Null, |s| Value::Text(s.into_owned())),
        ColumnData::Guid(v) => v.map_or(Value::Null, |u| Value::Uuid(guid_text(u))),
        ColumnData::Binary(v) => v.map_or(Value::Null, |b| match bytes {
            Bytes::Preview => Value::from_bytes(&b),
            Bytes::Whole => Value::from_bytes_uncapped(&b),
        }),
        ColumnData::Numeric(v) => v.map_or(Value::Null, |n| {
            Value::Decimal(numeric_text(n.value(), n.scale()))
        }),
        ColumnData::Xml(v) => v.map_or(Value::Null, |x| Value::Text(x.into_owned().into_string())),
        ColumnData::DateTime(v) => v.map_or(Value::Null, |dt| {
            datetime_text(dt.days(), dt.seconds_fragments())
                .map(Value::Timestamp)
                .unwrap_or_else(|| {
                    bad(
                        "datetime",
                        format!("{} days, {} ticks", dt.days(), dt.seconds_fragments()),
                    )
                })
        }),
        ColumnData::SmallDateTime(v) => v.map_or(Value::Null, |dt| {
            smalldatetime_text(dt.days(), dt.seconds_fragments())
                .map(Value::Timestamp)
                .unwrap_or_else(|| {
                    bad(
                        "smalldatetime",
                        format!("{} days, {} minutes", dt.days(), dt.seconds_fragments()),
                    )
                })
        }),
        ColumnData::Time(v) => v.map_or(Value::Null, |t| {
            time_text(t.increments(), t.scale())
                .map(Value::Time)
                .unwrap_or_else(|| {
                    bad("time", format!("{} at scale {}", t.increments(), t.scale()))
                })
        }),
        ColumnData::Date(v) => v.map_or(Value::Null, |d| {
            date_of(d.days())
                .map(|d| Value::Date(date_text(d)))
                .unwrap_or_else(|| bad("date", format!("{} days", d.days())))
        }),
        ColumnData::DateTime2(v) => v.map_or(Value::Null, |dt| {
            let (date, time) = (dt.date(), dt.time());
            datetime2_text(date.days(), time.increments(), time.scale())
                .map(Value::Timestamp)
                .unwrap_or_else(|| {
                    bad(
                        "datetime2",
                        format!(
                            "{} days, {} at scale {}",
                            date.days(),
                            time.increments(),
                            time.scale()
                        ),
                    )
                })
        }),
        ColumnData::DateTimeOffset(v) => v.map_or(Value::Null, |dto| {
            let dt = dto.datetime2();
            let (date, time) = (dt.date(), dt.time());
            datetimeoffset_text(date.days(), time.increments(), time.scale(), dto.offset())
                .map(Value::TimestampTz)
                .unwrap_or_else(|| {
                    bad(
                        "datetimeoffset",
                        format!(
                            "{} days, {} at scale {}, offset {} min",
                            date.days(),
                            time.increments(),
                            time.scale(),
                            dto.offset()
                        ),
                    )
                })
        }),
    }
}

/// A value no valid server sends, kept as what is known of it.
fn bad(ty: &str, what: String) -> Value {
    Value::other(ty, format!("(invalid {ty}: {what})"))
}

/// tiberius reads money as `value / 10^4` in a float: exact up to about 900 billion, where four
/// decimals still recover the stored value. Browse queries send money as text (`dialect::tsql`)
/// for the rest.
pub fn money(f: f64) -> Value {
    if !f.is_finite() {
        return Value::from_f64("money", f);
    }
    let text = format!("{f:.4}");
    Value::Decimal(match text.strip_prefix('-') {
        Some(rest) if rest.bytes().all(|b| b == b'0' || b == b'.') => rest.to_string(),
        _ => text,
    })
}

/// A decimal from its unscaled value and scale (`12345`, 2 → `123.45`).
pub fn numeric_text(value: i128, scale: u8) -> String {
    let digits = value.unsigned_abs().to_string();
    let scale = usize::from(scale);
    let sign = if value < 0 { "-" } else { "" };
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let padded = format!("{digits:0>width$}", width = scale + 1);
    let (int, frac) = padded.split_at(padded.len() - scale);
    format!("{sign}{int}.{frac}")
}

/// GUIDs as SQL Server prints them: upper case.
pub fn guid_text(u: Uuid) -> String {
    u.hyphenated().to_string().to_uppercase()
}

/// `date` counts days from 0001-01-01.
fn date_of(days: u32) -> Option<NaiveDate> {
    let days = i32::try_from(days).ok()?.checked_add(1)?;
    NaiveDate::from_num_days_from_ce_opt(days).filter(|d| d.year() <= 9999)
}

/// `datetime` and `smalldatetime` count days from 1900-01-01 (datetime back to 1753).
fn legacy_date_of(days: i64) -> Option<NaiveDate> {
    let base = NaiveDate::from_ymd_opt(1900, 1, 1)?;
    base.checked_add_signed(Duration::try_days(days)?)
        .filter(|d| (1753..=9999).contains(&d.year()))
}

fn date_text(d: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// Ticks of 10^-scale seconds past midnight, as `hh:mm:ss[.f…]` with `scale` decimals.
pub fn time_text(increments: u64, scale: u8) -> Option<String> {
    let (secs, frac) = split_increments(increments, scale)?;
    Some(clock_text(secs, frac, scale))
}

/// Seconds past midnight and the fraction's digits, for a valid time of day.
fn split_increments(increments: u64, scale: u8) -> Option<(u32, u64)> {
    if scale > 7 {
        return None;
    }
    let per_second = 10u64.pow(u32::from(scale));
    let secs = increments / per_second;
    if secs >= 86_400 {
        return None;
    }
    Some((secs as u32, increments % per_second))
}

fn clock_text(secs: u32, frac: u64, scale: u8) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if scale == 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!(
            "{h:02}:{m:02}:{s:02}.{frac:0width$}",
            width = usize::from(scale)
        )
    }
}

pub fn datetime2_text(days: u32, increments: u64, scale: u8) -> Option<String> {
    let date = date_of(days)?;
    let (secs, frac) = split_increments(increments, scale)?;
    Some(format!(
        "{} {}",
        date_text(date),
        clock_text(secs, frac, scale)
    ))
}

/// The wire holds the UTC time and the offset in minutes; SQL Server prints the local time
/// with its offset (`2024-01-02 15:41:12.1234567 +05:30`).
pub fn datetimeoffset_text(days: u32, increments: u64, scale: u8, offset: i16) -> Option<String> {
    if !(-840..=840).contains(&offset) {
        return None;
    }
    let date = date_of(days)?;
    let (secs, frac) = split_increments(increments, scale)?;
    let utc = NaiveDateTime::new(
        date,
        NaiveTime::from_num_seconds_from_midnight_opt(secs, 0)?,
    );
    let local = utc.checked_add_signed(Duration::try_minutes(i64::from(offset))?)?;
    if !(1..=9999).contains(&local.year()) {
        return None;
    }
    let sign = if offset < 0 { '-' } else { '+' };
    let abs = offset.unsigned_abs();
    Some(format!(
        "{} {} {sign}{:02}:{:02}",
        date_text(local.date()),
        clock_text(local.time().num_seconds_from_midnight(), frac, scale),
        abs / 60,
        abs % 60
    ))
}

/// `datetime`: days from 1900-01-01 and 1/300 s ticks, printed with milliseconds rounded as
/// SQL Server rounds them (.000, .003, .007).
pub fn datetime_text(days: i32, ticks: u32) -> Option<String> {
    const TICKS_PER_DAY: u32 = 300 * 86_400;
    if ticks >= TICKS_PER_DAY {
        return None;
    }
    let date = legacy_date_of(i64::from(days))?;
    let ms = (u64::from(ticks) * 20 + 3) / 6;
    let secs = (ms / 1000) as u32;
    Some(format!(
        "{} {}.{:03}",
        date_text(date),
        clock_text(secs, 0, 0),
        ms % 1000
    ))
}

/// `smalldatetime`: days from 1900-01-01 and minutes past midnight.
pub fn smalldatetime_text(days: u16, minutes: u16) -> Option<String> {
    if minutes >= 24 * 60 {
        return None;
    }
    let date = legacy_date_of(i64::from(days))?;
    Some(format!(
        "{} {}",
        date_text(date),
        clock_text(u32::from(minutes) * 60, 0, 0)
    ))
}

// --- Bind parameters -----------------------------------------------------------------------------

/// A `Value` as a bind parameter. Temporal values, JSON and the rest go as text, which the
/// server converts to the column's type (a `datetime` column compared with a datetime2 parameter
/// would not match its own value). `kind`, the column's kind when known, types a NULL: an
/// nvarchar NULL does not convert to the binary types.
pub fn to_param(v: &Value, kind: Option<ValueKind>) -> Result<ColumnData<'static>, AppError> {
    Ok(match v {
        Value::Null => match kind {
            Some(ValueKind::Bytes) => ColumnData::Binary(None),
            _ => ColumnData::String(None),
        },
        Value::Default => {
            return Err(AppError::Internal(
                "DEFAULT cannot be bound as a parameter".into(),
            ));
        }
        Value::Bool(b) => ColumnData::Bit(Some(*b)),
        Value::Int(n) => match i32::try_from(*n) {
            Ok(n) => ColumnData::I32(Some(n)),
            Err(_) => ColumnData::I64(Some(*n)),
        },
        Value::BigInt(s) => match s.trim().parse::<i64>() {
            Ok(n) => ColumnData::I64(Some(n)),
            Err(_) => numeric_param(s).unwrap_or_else(|| text(s)),
        },
        Value::Float(f) => ColumnData::F64(Some(*f)),
        Value::Decimal(s) => numeric_param(s).unwrap_or_else(|| text(s)),
        Value::Bytes(b) => {
            if b.truncated {
                return Err(AppError::InvalidInput(
                    "a shortened binary value cannot be written back; load it whole first".into(),
                ));
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&b.base64)
                .map_err(|e| AppError::InvalidInput(format!("binary value: {e}")))?;
            ColumnData::Binary(Some(Cow::Owned(bytes)))
        }
        Value::Uuid(s) => match Uuid::parse_str(s.trim()) {
            Ok(u) => ColumnData::Guid(Some(u)),
            Err(_) => text(s),
        },
        Value::Text(s)
        | Value::Date(s)
        | Value::Time(s)
        | Value::Timestamp(s)
        | Value::TimestampTz(s)
        | Value::Interval(s)
        | Value::Json(s)
        | Value::Enum(s) => text(s),
        Value::Array(_) | Value::Other(_) => text(&v.as_text().unwrap_or_default()),
    })
}

fn text(s: &str) -> ColumnData<'static> {
    ColumnData::String(Some(Cow::Owned(s.to_string())))
}

/// A decimal written as digits with an optional sign and point, up to 38 digits (SQL Server's
/// precision); None for anything else (exponents, more digits), which goes as text.
pub fn numeric_param(s: &str) -> Option<ColumnData<'static>> {
    let s = s.trim();
    let (negative, unsigned) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (int, frac) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if (int.is_empty() && frac.is_empty()) || !digits(int) || !digits(frac) {
        return None;
    }
    let int = int.trim_start_matches('0');
    if int.len() + frac.len() > 38 {
        return None;
    }
    let scale = u8::try_from(frac.len()).ok()?;
    let mut value: i128 = format!("{int}{frac}").parse().unwrap_or(0);
    if negative {
        value = -value;
    }
    Some(ColumnData::Numeric(Some(Numeric::new_with_scale(
        value, scale,
    ))))
}

#[cfg(test)]
mod tests {
    use ::tiberius::time::{Date, DateTime, DateTime2, DateTimeOffset, SmallDateTime, Time};
    use ::tiberius::xml::XmlData;

    use super::*;
    use crate::types::value::BYTES_PREVIEW_CAP;

    fn cell(ty: ColumnType, data: ColumnData<'static>) -> Value {
        decode_cell(ty, data, Bytes::Preview)
    }

    #[test]
    fn integers_keep_the_safe_range() {
        assert_eq!(
            cell(ColumnType::Int1, ColumnData::U8(Some(255))),
            Value::Int(255)
        );
        assert_eq!(
            cell(ColumnType::Int2, ColumnData::I16(Some(-2))),
            Value::Int(-2)
        );
        assert_eq!(cell(ColumnType::Int4, ColumnData::I32(None)), Value::Null);
        assert_eq!(
            cell(ColumnType::Int8, ColumnData::I64(Some((1 << 53) + 1))),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            cell(ColumnType::Int8, ColumnData::I64(Some(i64::MIN))),
            Value::BigInt(i64::MIN.to_string())
        );
        assert_eq!(
            cell(ColumnType::Bitn, ColumnData::Bit(Some(true))),
            Value::Bool(true)
        );
    }

    #[test]
    fn floats_money_and_decimals() {
        assert_eq!(
            cell(ColumnType::Float4, ColumnData::F32(Some(0.1))),
            Value::Float(0.1)
        );
        assert_eq!(
            cell(ColumnType::Float8, ColumnData::F64(Some(1.5))),
            Value::Float(1.5)
        );
        assert_eq!(
            cell(ColumnType::Float8, ColumnData::F64(Some(f64::INFINITY))),
            Value::other("float", "inf")
        );
        assert_eq!(
            cell(ColumnType::Money, ColumnData::F64(Some(12.3456))),
            Value::Decimal("12.3456".into())
        );
        assert_eq!(
            cell(ColumnType::Money4, ColumnData::F64(Some(-214748.3648))),
            Value::Decimal("-214748.3648".into())
        );
        assert_eq!(money(-0.0), Value::Decimal("0.0000".into()));
        assert_eq!(money(-0.00001), Value::Decimal("0.0000".into()));
        assert_eq!(
            cell(
                ColumnType::Numericn,
                ColumnData::Numeric(Some(Numeric::new_with_scale(-5, 3)))
            ),
            Value::Decimal("-0.005".into())
        );
        assert_eq!(numeric_text(12345, 2), "123.45");
        assert_eq!(numeric_text(7, 0), "7");
        assert_eq!(numeric_text(i128::MIN, 38).len(), 41);
        let max = 10i128.pow(38) - 1;
        assert_eq!(numeric_text(max, 0), "9".repeat(38));
    }

    #[test]
    fn text_guid_xml_and_bytes() {
        assert_eq!(
            cell(ColumnType::NVarchar, ColumnData::String(Some("é".into()))),
            Value::Text("é".into())
        );
        let u = Uuid::parse_str("6f9619ff-8b86-d011-b42d-00c04fc964ff").unwrap();
        assert_eq!(
            cell(ColumnType::Guid, ColumnData::Guid(Some(u))),
            Value::Uuid("6F9619FF-8B86-D011-B42D-00C04FC964FF".into())
        );
        assert_eq!(
            cell(
                ColumnType::Xml,
                ColumnData::Xml(Some(Cow::Owned(XmlData::new("<a/>"))))
            ),
            Value::Text("<a/>".into())
        );
        let big = vec![7u8; BYTES_PREVIEW_CAP + 1];
        match cell(
            ColumnType::BigVarBin,
            ColumnData::Binary(Some(Cow::Owned(big.clone()))),
        ) {
            Value::Bytes(b) => assert!(b.truncated && b.len == big.len() as u64),
            other => panic!("{other:?}"),
        }
        match decode_cell(
            ColumnType::Image,
            ColumnData::Binary(Some(Cow::Owned(big.clone()))),
            Bytes::Whole,
        ) {
            Value::Bytes(b) => assert!(!b.truncated),
            other => panic!("{other:?}"),
        }
        // A NULL sql_variant arrives as a NULL string.
        assert_eq!(
            cell(ColumnType::SSVariant, ColumnData::String(None)),
            Value::Null
        );
        // A sql_variant holding an int arrives as that int.
        assert_eq!(
            cell(ColumnType::SSVariant, ColumnData::I32(Some(3))),
            Value::Int(3)
        );
    }

    #[test]
    fn temporal_values_print_like_the_server() {
        // 2024-01-02 is day 738886 from 0001-01-01.
        let day = 738_886u32;
        assert_eq!(
            cell(ColumnType::Daten, ColumnData::Date(Some(Date::new(day)))),
            Value::Date("2024-01-02".into())
        );
        assert_eq!(
            cell(ColumnType::Daten, ColumnData::Date(Some(Date::new(0)))),
            Value::Date("0001-01-01".into())
        );
        let t = Time::new(367_221_234_567, 7);
        assert_eq!(
            cell(ColumnType::Timen, ColumnData::Time(Some(t))),
            Value::Time("10:12:02.1234567".into())
        );
        assert_eq!(time_text(36_000, 0).as_deref(), Some("10:00:00"));
        assert_eq!(time_text(36_000_120, 3).as_deref(), Some("10:00:00.120"));
        assert_eq!(
            cell(
                ColumnType::Datetime2,
                ColumnData::DateTime2(Some(DateTime2::new(
                    Date::new(day),
                    Time::new(3_600_005, 3)
                )))
            ),
            Value::Timestamp("2024-01-02 01:00:00.005".into())
        );
        // 10:00 UTC at +05:30 is 15:30 local; -08:00 crosses back a day.
        let utc = DateTime2::new(Date::new(day), Time::new(36_000, 0));
        assert_eq!(
            cell(
                ColumnType::DatetimeOffsetn,
                ColumnData::DateTimeOffset(Some(DateTimeOffset::new(utc, 330)))
            ),
            Value::TimestampTz("2024-01-02 15:30:00 +05:30".into())
        );
        assert_eq!(
            datetimeoffset_text(day, 3_600, 0, -480).as_deref(),
            Some("2024-01-01 17:00:00 -08:00")
        );
        // datetime: day 45291 from 1900-01-01 is 2024-01-02; 1/300 s ticks round to ms.
        assert_eq!(
            cell(
                ColumnType::Datetimen,
                ColumnData::DateTime(Some(DateTime::new(45_291, 2)))
            ),
            Value::Timestamp("2024-01-02 00:00:00.007".into())
        );
        assert_eq!(
            datetime_text(45_291, 300 * 86_400 - 1).as_deref(),
            Some("2024-01-02 23:59:59.997")
        );
        assert_eq!(
            datetime_text(-53_690, 0).as_deref(),
            Some("1753-01-01 00:00:00.000")
        );
        assert_eq!(
            cell(
                ColumnType::Datetime4,
                ColumnData::SmallDateTime(Some(SmallDateTime::new(45_291, 61)))
            ),
            Value::Timestamp("2024-01-02 01:01:00".into())
        );
    }

    #[test]
    fn impossible_temporal_values_are_kept_not_panicked_on() {
        assert!(matches!(
            cell(ColumnType::Timen, ColumnData::Time(Some(Time::new(1, 9)))),
            Value::Other(o) if o.ty == "time"
        ));
        assert!(matches!(
            cell(
                ColumnType::Timen,
                ColumnData::Time(Some(Time::new(86_400, 0)))
            ),
            Value::Other(_)
        ));
        assert!(matches!(
            cell(ColumnType::Daten, ColumnData::Date(Some(Date::new(0xFF_FFFF)))),
            Value::Other(o) if o.ty == "date"
        ));
        assert!(matches!(
            cell(
                ColumnType::Datetimen,
                ColumnData::DateTime(Some(DateTime::new(i32::MIN, 0)))
            ),
            Value::Other(_)
        ));
        assert!(matches!(
            cell(
                ColumnType::Datetimen,
                ColumnData::DateTime(Some(DateTime::new(0, u32::MAX)))
            ),
            Value::Other(_)
        ));
        assert!(datetimeoffset_text(738_886, 0, 0, 900).is_none());
        assert!(datetimeoffset_text(3_652_058, 86_399, 0, 840).is_none());
        assert!(smalldatetime_text(0, 1440).is_none());
    }

    #[test]
    fn result_columns_name_types() {
        let row = Row::builder()
            .column("m", ColumnType::Money, ColumnData::F64(Some(1.25)))
            .column(
                "t",
                ColumnType::DatetimeOffsetn,
                ColumnData::DateTimeOffset(None),
            )
            .build();
        let cols = result_columns(row.columns());
        assert_eq!(cols[0].data_type, "money");
        assert_eq!(cols[0].kind, ValueKind::Decimal);
        assert_eq!(cols[1].kind, ValueKind::TimestampTz);
        assert_eq!(
            decode_row(row, Bytes::Preview),
            vec![Value::Decimal("1.2500".into()), Value::Null]
        );
    }

    #[test]
    fn parameters_are_typed_where_that_is_safe() {
        assert_eq!(
            to_param(&Value::Int(5), None).unwrap(),
            ColumnData::I32(Some(5))
        );
        assert_eq!(
            to_param(&Value::Int(1 << 40), None).unwrap(),
            ColumnData::I64(Some(1 << 40))
        );
        assert_eq!(
            to_param(&Value::BigInt("9007199254740993".into()), None).unwrap(),
            ColumnData::I64(Some(9_007_199_254_740_993))
        );
        assert_eq!(
            to_param(&Value::Decimal("-12.50".into()), None).unwrap(),
            ColumnData::Numeric(Some(Numeric::new_with_scale(-1250, 2)))
        );
        assert_eq!(
            to_param(&Value::Decimal("1e5".into()), None).unwrap(),
            ColumnData::String(Some("1e5".into()))
        );
        assert!(numeric_param(&"9".repeat(39)).is_none());
        assert!(numeric_param(".").is_none() && numeric_param("").is_none());
        assert!(numeric_param("0.5").is_some() && numeric_param(".5").is_some());
        assert_eq!(
            to_param(&Value::Timestamp("2024-01-02 10:11:12.123".into()), None).unwrap(),
            ColumnData::String(Some("2024-01-02 10:11:12.123".into()))
        );
        assert_eq!(
            to_param(&Value::Null, Some(ValueKind::Bytes)).unwrap(),
            ColumnData::Binary(None)
        );
        assert_eq!(
            to_param(&Value::Null, None).unwrap(),
            ColumnData::String(None)
        );
        assert_eq!(
            to_param(&Value::from_bytes(&[1, 2]), None).unwrap(),
            ColumnData::Binary(Some(Cow::Owned(vec![1, 2])))
        );
        assert!(to_param(&Value::from_bytes(&vec![0; BYTES_PREVIEW_CAP + 1]), None).is_err());
        assert!(to_param(&Value::Default, None).is_err());
        assert!(matches!(
            to_param(
                &Value::Uuid("6f9619ff-8b86-d011-b42d-00c04fc964ff".into()),
                None
            )
            .unwrap(),
            ColumnData::Guid(Some(_))
        ));
    }
}
