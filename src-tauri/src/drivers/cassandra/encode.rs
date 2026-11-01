//! Tabler's `Value`s as the CQL values a prepared statement binds. The driver checks each value
//! against the type the server gave its marker, so the value is built for that type: an edited
//! `smallint` cell (an `Int`) becomes a `SmallInt`, a decimal's text an exact `CqlDecimal`, a
//! timestamp's text its milliseconds, a collection's JSON (as the grid shows it, `toJson()`'s
//! shape) the collection.
//!
//! Total: anything that does not fit the type is an `InvalidInput` naming it, never a panic.

use std::net::IpAddr;
use std::str::FromStr;

use ::scylla::frame::response::result::{CollectionType, ColumnType, NativeType};
use ::scylla::value::{
    Counter, CqlDate, CqlDecimal, CqlDuration, CqlTime, CqlTimestamp, CqlTimeuuid, CqlValue,
    CqlVarint,
};
use base64::Engine as _;
use bigdecimal::BigDecimal;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use num_bigint::BigInt;
use uuid::Uuid;

use super::types::column_type_text;
use crate::error::AppError;
use crate::types::value::{Value, decode_hex};

/// The bind value of `v` for a marker of type `ty`; `None` binds NULL.
pub fn encode(v: &Value, ty: &ColumnType<'_>) -> Result<Option<CqlValue>, AppError> {
    match v {
        Value::Null => Ok(None),
        Value::Default => Err(AppError::InvalidInput(
            "CQL has no DEFAULT: enter the value explicitly".into(),
        )),
        v => match ty {
            ColumnType::Native(n) => native(v, n, ty).map(Some),
            _ => {
                let text = composite_text(v, ty)?;
                let json: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|e| mismatch(ty, &format!("its JSON does not parse ({e})")))?;
                from_json(&json, ty)
            }
        },
    }
}

/// The JSON text of a collection, tuple, UDT or vector value.
fn composite_text(v: &Value, ty: &ColumnType<'_>) -> Result<String, AppError> {
    match v {
        Value::Json(s) | Value::Text(s) => Ok(s.clone()),
        _ => Err(mismatch(ty, "it takes JSON text")),
    }
}

fn mismatch(ty: &ColumnType<'_>, why: &str) -> AppError {
    AppError::InvalidInput(format!(
        "not a value for a {} column: {why}",
        column_type_text(ty)
    ))
}

/// The text of a scalar value, for the types read from text.
fn text_of(v: &Value, ty: &ColumnType<'_>) -> Result<String, AppError> {
    match v {
        Value::Bytes(_) | Value::Array(_) => Err(mismatch(ty, "it is not text")),
        v => v
            .as_text()
            .map(|s| s.trim().to_string())
            .ok_or_else(|| mismatch(ty, "it is empty")),
    }
}

fn native(v: &Value, n: &NativeType, ty: &ColumnType<'_>) -> Result<CqlValue, AppError> {
    let int = |min: i64, max: i64| -> Result<i64, AppError> {
        let n = integer(v, ty)?;
        i64::try_from(&n)
            .ok()
            .filter(|i| (min..=max).contains(i))
            .ok_or_else(|| mismatch(ty, "it is out of range"))
    };
    Ok(match n {
        NativeType::Ascii => {
            let s = plain_text(v, ty)?;
            if !s.is_ascii() {
                return Err(mismatch(ty, "it is not ASCII"));
            }
            CqlValue::Ascii(s)
        }
        NativeType::Text => CqlValue::Text(plain_text(v, ty)?),
        NativeType::Boolean => match v {
            Value::Bool(b) => CqlValue::Boolean(*b),
            v => match text_of(v, ty)?.to_ascii_lowercase().as_str() {
                "true" => CqlValue::Boolean(true),
                "false" => CqlValue::Boolean(false),
                _ => return Err(mismatch(ty, "it is not true or false")),
            },
        },
        NativeType::TinyInt => CqlValue::TinyInt(int(i8::MIN.into(), i8::MAX.into())? as i8),
        NativeType::SmallInt => CqlValue::SmallInt(int(i16::MIN.into(), i16::MAX.into())? as i16),
        NativeType::Int => CqlValue::Int(int(i32::MIN.into(), i32::MAX.into())? as i32),
        NativeType::BigInt => CqlValue::BigInt(int(i64::MIN, i64::MAX)?),
        NativeType::Counter => CqlValue::Counter(Counter(int(i64::MIN, i64::MAX)?)),
        NativeType::Varint => CqlValue::Varint(CqlVarint::from_signed_bytes_be(
            integer(v, ty)?.to_signed_bytes_be(),
        )),
        NativeType::Decimal => {
            let d = BigDecimal::from_str(&text_of(v, ty)?)
                .map_err(|_| mismatch(ty, "it is not a number"))?;
            let (unscaled, scale) = d.as_bigint_and_exponent();
            let scale = i32::try_from(scale).map_err(|_| mismatch(ty, "its scale is too large"))?;
            CqlValue::Decimal(CqlDecimal::from_signed_be_bytes_slice_and_exponent(
                &unscaled.to_signed_bytes_be(),
                scale,
            ))
        }
        NativeType::Float => CqlValue::Float(float(v, ty)? as f32),
        NativeType::Double => CqlValue::Double(float(v, ty)?),
        NativeType::Blob => CqlValue::Blob(blob(v, ty)?),
        NativeType::Uuid => CqlValue::Uuid(uuid(v, ty)?),
        NativeType::Timeuuid => {
            let u = uuid(v, ty)?;
            if u.get_version_num() != 1 {
                return Err(mismatch(ty, "it is not a time-based (version 1) UUID"));
            }
            CqlValue::Timeuuid(CqlTimeuuid::from(u))
        }
        NativeType::Inet => CqlValue::Inet(
            IpAddr::from_str(&text_of(v, ty)?)
                .map_err(|_| mismatch(ty, "it is not an IP address"))?,
        ),
        NativeType::Date => CqlValue::Date(CqlDate(date(&text_of(v, ty)?, ty)?)),
        NativeType::Time => CqlValue::Time(CqlTime(time(&text_of(v, ty)?, ty)?)),
        NativeType::Timestamp => CqlValue::Timestamp(CqlTimestamp(match v {
            Value::Int(ms) => *ms,
            v => timestamp(&text_of(v, ty)?, ty)?,
        })),
        NativeType::Duration => CqlValue::Duration(duration(&text_of(v, ty)?, ty)?),
        _ => return Err(mismatch(ty, "Tabler cannot write this type")),
    })
}

/// Text as it is (no trimming: spaces are part of a text value).
fn plain_text(v: &Value, ty: &ColumnType<'_>) -> Result<String, AppError> {
    match v {
        Value::Text(s) | Value::Json(s) | Value::Enum(s) => Ok(s.clone()),
        Value::Bytes(_) | Value::Array(_) => Err(mismatch(ty, "it is not text")),
        v => v.as_text().ok_or_else(|| mismatch(ty, "it is empty")),
    }
}

fn integer(v: &Value, ty: &ColumnType<'_>) -> Result<BigInt, AppError> {
    match v {
        Value::Int(n) => Ok(BigInt::from(*n)),
        Value::Float(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Ok(BigInt::from(*f as i64)),
        v => {
            let text = text_of(v, ty)?;
            let text = text.strip_prefix('+').unwrap_or(&text);
            BigInt::from_str(text).map_err(|_| mismatch(ty, "it is not an integer"))
        }
    }
}

fn float(v: &Value, ty: &ColumnType<'_>) -> Result<f64, AppError> {
    match v {
        Value::Float(f) => Ok(*f),
        Value::Int(n) => Ok(*n as f64),
        v => {
            let text = text_of(v, ty)?;
            match text.as_str() {
                "NaN" => Ok(f64::NAN),
                "Infinity" => Ok(f64::INFINITY),
                "-Infinity" => Ok(f64::NEG_INFINITY),
                t => t
                    .parse::<f64>()
                    .map_err(|_| mismatch(ty, "it is not a number")),
            }
        }
    }
}

fn blob(v: &Value, ty: &ColumnType<'_>) -> Result<Vec<u8>, AppError> {
    match v {
        Value::Bytes(b) if b.truncated => Err(mismatch(
            ty,
            "only a preview of it was loaded; load the full value first",
        )),
        Value::Bytes(b) => base64::engine::general_purpose::STANDARD
            .decode(&b.base64)
            .map_err(|_| mismatch(ty, "its bytes do not decode")),
        v => {
            let text = text_of(v, ty)?;
            let hex = text
                .strip_prefix("0x")
                .or_else(|| text.strip_prefix("0X"))
                .ok_or_else(|| mismatch(ty, "a blob is written 0x and hex digits"))?;
            decode_hex(hex).ok_or_else(|| mismatch(ty, "a blob is written 0x and hex digits"))
        }
    }
}

fn uuid(v: &Value, ty: &ColumnType<'_>) -> Result<Uuid, AppError> {
    Uuid::parse_str(&text_of(v, ty)?).map_err(|_| mismatch(ty, "it is not a UUID"))
}

/// Days since the epoch, as CQL stores them: offset by 2^31.
const DATE_EPOCH: i64 = 1 << 31;

fn epoch_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default()
}

/// `YYYY-MM-DD`, or the days as CQL counts them.
fn date(text: &str, ty: &ColumnType<'_>) -> Result<u32, AppError> {
    let days = match NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        Ok(d) => d.signed_duration_since(epoch_date()).num_days() + DATE_EPOCH,
        Err(_) => text
            .parse::<i64>()
            .map_err(|_| mismatch(ty, "a date is written YYYY-MM-DD"))?,
    };
    u32::try_from(days).map_err(|_| mismatch(ty, "it is out of range"))
}

/// `HH:MM:SS[.fffffffff]` as nanoseconds since midnight.
fn time(text: &str, ty: &ColumnType<'_>) -> Result<i64, AppError> {
    let t = NaiveTime::parse_from_str(text, "%H:%M:%S%.f")
        .or_else(|_| NaiveTime::parse_from_str(text, "%H:%M"))
        .map_err(|_| mismatch(ty, "a time is written HH:MM:SS.fff"))?;
    if t.nanosecond() >= 1_000_000_000 {
        return Err(mismatch(ty, "it is out of range"));
    }
    Ok(i64::from(t.num_seconds_from_midnight()) * 1_000_000_000 + i64::from(t.nanosecond()))
}

/// A timestamp's text (`2024-05-01 10:00:00.123+0000`, ISO 8601 with `T`, `Z` or an offset, a
/// date alone, or milliseconds since the epoch) as milliseconds since the epoch. A time without
/// an offset is UTC.
fn timestamp(text: &str, ty: &ColumnType<'_>) -> Result<i64, AppError> {
    if let Ok(ms) = text.parse::<i64>() {
        return Ok(ms);
    }
    let normalized = text.replacen('T', " ", 1);
    let normalized = normalized.trim_end_matches('Z').trim_end_matches('z');
    let zoned = [
        "%Y-%m-%d %H:%M:%S%.f%z",
        "%Y-%m-%d %H:%M:%S%.f%:z",
        "%Y-%m-%d %H:%M%z",
    ]
    .iter()
    .find_map(|f| DateTime::parse_from_str(normalized, f).ok())
    .map(|d| d.timestamp_millis());
    let naive = || {
        ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M"]
            .iter()
            .find_map(|f| NaiveDateTime::parse_from_str(normalized, f).ok())
            .or_else(|| {
                NaiveDate::parse_from_str(normalized, "%Y-%m-%d")
                    .ok()
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
            })
            .map(|d| d.and_utc().timestamp_millis())
    };
    zoned
        .or_else(naive)
        .ok_or_else(|| mismatch(ty, "a timestamp is written YYYY-MM-DD HH:MM:SS.fff+0000"))
}

/// Cassandra's duration form (`1y2mo3d4h5m6s7ms8us9ns`, `-1d`, also `µs`), or ISO 8601's
/// `P1Y2M3DT4H5M6S` / `P2W`.
fn duration(text: &str, ty: &ColumnType<'_>) -> Result<CqlDuration, AppError> {
    let bad = || mismatch(ty, "a duration is written like 1y2mo3d4h5m6s");
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (mut months, mut days, mut nanos) = (0i64, 0i64, 0i64);
    if let Some(iso) = body.strip_prefix('P').or_else(|| body.strip_prefix('p')) {
        let mut in_time = false;
        let mut number = String::new();
        for c in iso.chars() {
            match c {
                '0'..='9' => number.push(c),
                'T' | 't' => in_time = true,
                unit => {
                    let n: i64 = number.parse().map_err(|_| bad())?;
                    number.clear();
                    match (in_time, unit.to_ascii_uppercase()) {
                        (false, 'Y') => months += n * 12,
                        (false, 'M') => months += n,
                        (false, 'W') => days += n * 7,
                        (false, 'D') => days += n,
                        (true, 'H') => nanos += n * 3_600_000_000_000,
                        (true, 'M') => nanos += n * 60_000_000_000,
                        (true, 'S') => nanos += n * 1_000_000_000,
                        _ => return Err(bad()),
                    }
                }
            }
        }
        if !number.is_empty() {
            return Err(bad());
        }
    } else {
        let lower = body.to_ascii_lowercase().replace('µ', "u");
        let mut rest = lower.as_str();
        if rest.is_empty() {
            return Err(bad());
        }
        while !rest.is_empty() {
            let digits = rest.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
            let n: i64 = rest[..digits].parse().map_err(|_| bad())?;
            rest = &rest[digits..];
            let unit_len = rest
                .find(|c: char| c.is_ascii_digit())
                .unwrap_or(rest.len());
            let unit = &rest[..unit_len];
            rest = &rest[unit_len..];
            let checked = |a: i64, mul: i64| a.checked_add(n.checked_mul(mul)?);
            match unit {
                "y" => months = checked(months, 12).ok_or_else(bad)?,
                "mo" => months = checked(months, 1).ok_or_else(bad)?,
                "w" => days = checked(days, 7).ok_or_else(bad)?,
                "d" => days = checked(days, 1).ok_or_else(bad)?,
                "h" => nanos = checked(nanos, 3_600_000_000_000).ok_or_else(bad)?,
                "m" => nanos = checked(nanos, 60_000_000_000).ok_or_else(bad)?,
                "s" => nanos = checked(nanos, 1_000_000_000).ok_or_else(bad)?,
                "ms" => nanos = checked(nanos, 1_000_000).ok_or_else(bad)?,
                "us" => nanos = checked(nanos, 1000).ok_or_else(bad)?,
                "ns" => nanos = checked(nanos, 1).ok_or_else(bad)?,
                _ => return Err(bad()),
            }
        }
    }
    let sign = if negative { -1 } else { 1 };
    Ok(CqlDuration {
        months: i32::try_from(months * sign).map_err(|_| bad())?,
        days: i32::try_from(days * sign).map_err(|_| bad())?,
        nanoseconds: nanos * sign,
    })
}

/// A JSON value (the grid's text of a collection, tuple, UDT or vector) as a value of `ty`.
fn from_json(j: &serde_json::Value, ty: &ColumnType<'_>) -> Result<Option<CqlValue>, AppError> {
    use serde_json::Value as J;
    if j.is_null() {
        return Ok(None);
    }
    let element = |j: &J, ty: &ColumnType<'_>| -> Result<CqlValue, AppError> {
        from_json(j, ty)?.ok_or_else(|| mismatch(ty, "a collection cannot hold null"))
    };
    let items = |j: &J| -> Result<Vec<J>, AppError> {
        j.as_array()
            .cloned()
            .ok_or_else(|| mismatch(ty, "it takes a JSON array"))
    };
    Ok(Some(match ty {
        ColumnType::Native(n) => {
            let v = match j {
                J::Bool(b) => Value::Bool(*b),
                J::Number(n) => Value::Decimal(n.to_string()),
                J::String(s) => Value::Text(s.clone()),
                _ => return Err(mismatch(ty, "it is not a scalar")),
            };
            native(&v, n, ty)?
        }
        ColumnType::Collection { typ, .. } => match typ {
            CollectionType::List(e) => CqlValue::List(
                items(j)?
                    .iter()
                    .map(|i| element(i, e))
                    .collect::<Result<_, _>>()?,
            ),
            CollectionType::Set(e) => CqlValue::Set(
                items(j)?
                    .iter()
                    .map(|i| element(i, e))
                    .collect::<Result<_, _>>()?,
            ),
            CollectionType::Map(k, v) => {
                let object = j
                    .as_object()
                    .ok_or_else(|| mismatch(ty, "it takes a JSON object"))?;
                let mut pairs = Vec::with_capacity(object.len());
                for (key, value) in object {
                    pairs.push((map_key(key, k)?, element(value, v)?));
                }
                CqlValue::Map(pairs)
            }
            _ => return Err(mismatch(ty, "Tabler cannot write this type")),
        },
        ColumnType::Tuple(types) => {
            let values = items(j)?;
            if values.len() != types.len() {
                return Err(mismatch(ty, "it has another number of elements"));
            }
            CqlValue::Tuple(
                values
                    .iter()
                    .zip(types)
                    .map(|(v, t)| from_json(v, t))
                    .collect::<Result<_, _>>()?,
            )
        }
        ColumnType::Vector { typ, dimensions } => {
            let values = items(j)?;
            if values.len() != usize::from(*dimensions) {
                return Err(mismatch(ty, "it has another number of elements"));
            }
            CqlValue::Vector(
                values
                    .iter()
                    .map(|v| element(v, typ))
                    .collect::<Result<_, _>>()?,
            )
        }
        ColumnType::UserDefinedType { definition, .. } => {
            let object = j
                .as_object()
                .ok_or_else(|| mismatch(ty, "it takes a JSON object"))?;
            if let Some(unknown) = object
                .keys()
                .find(|k| !definition.field_types.iter().any(|(n, _)| n == *k))
            {
                return Err(mismatch(
                    ty,
                    &format!("it has no field \u{201c}{unknown}\u{201d}"),
                ));
            }
            let mut fields = Vec::with_capacity(definition.field_types.len());
            for (name, field_ty) in &definition.field_types {
                let value = match object.get(name.as_ref()) {
                    Some(v) => from_json(v, field_ty)?,
                    None => None,
                };
                fields.push((name.to_string(), value));
            }
            CqlValue::UserDefinedType {
                keyspace: definition.keyspace.to_string(),
                name: definition.name.to_string(),
                fields,
            }
        }
        _ => return Err(mismatch(ty, "Tabler cannot write this type")),
    }))
}

/// A map key as `toJson()` writes it: the value's own text for a scalar, the JSON text of a
/// tuple or collection.
fn map_key(key: &str, ty: &ColumnType<'_>) -> Result<CqlValue, AppError> {
    let value = match ty {
        ColumnType::Native(_) => from_json(&serde_json::Value::String(key.to_string()), ty)?,
        _ => {
            let json: serde_json::Value = serde_json::from_str(key)
                .map_err(|_| mismatch(ty, "a map key of this type is JSON text"))?;
            from_json(&json, ty)?
        }
    };
    value.ok_or_else(|| mismatch(ty, "a map key cannot be null"))
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::Arc;

    use ::scylla::frame::response::result::UserDefinedType;

    use super::*;
    use crate::drivers::cassandra::values::decode_value;

    fn n(t: NativeType) -> ColumnType<'static> {
        ColumnType::Native(t)
    }

    fn enc(v: Value, ty: &ColumnType<'_>) -> CqlValue {
        encode(&v, ty).unwrap().unwrap()
    }

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn integers_take_the_width_of_their_column() {
        assert_eq!(
            enc(Value::Int(7), &n(NativeType::SmallInt)),
            CqlValue::SmallInt(7)
        );
        assert_eq!(
            enc(text("-128"), &n(NativeType::TinyInt)),
            CqlValue::TinyInt(-128)
        );
        assert!(encode(&Value::Int(40_000), &n(NativeType::SmallInt)).is_err());
        // 2^53 + 1 arrives as BigInt text and is written exactly.
        let big = Value::BigInt("9007199254740993".into());
        assert_eq!(
            enc(big.clone(), &n(NativeType::BigInt)),
            CqlValue::BigInt(9_007_199_254_740_993)
        );
        assert_eq!(
            decode_value(&enc(big, &n(NativeType::BigInt))),
            Value::BigInt("9007199254740993".into())
        );
        let huge = "-123456789012345678901234567890";
        assert_eq!(
            decode_value(&enc(text(huge), &n(NativeType::Varint))),
            Value::BigInt(huge.into())
        );
        assert!(encode(&text("1.5"), &n(NativeType::Int)).is_err());
    }

    #[test]
    fn decimals_round_trip_exactly() {
        for d in [
            "-0.05",
            "10.00",
            "123456789012345678901234567890.123456789",
            "1E+300",
        ] {
            let v = enc(Value::Decimal(d.into()), &n(NativeType::Decimal));
            let back = decode_value(&v);
            let Value::Decimal(back) = back else {
                panic!("{back:?}");
            };
            assert_eq!(
                BigDecimal::from_str(&back).unwrap(),
                BigDecimal::from_str(d).unwrap(),
                "{d}"
            );
        }
        assert_eq!(
            decode_value(&enc(
                Value::Decimal("-0.05".into()),
                &n(NativeType::Decimal)
            )),
            Value::Decimal("-0.05".into())
        );
    }

    #[test]
    fn temporal_texts_round_trip() {
        let ts = "2024-05-01 10:00:00.123+0000";
        let v = enc(Value::TimestampTz(ts.into()), &n(NativeType::Timestamp));
        assert_eq!(decode_value(&v), Value::TimestampTz(ts.into()));
        for (text_form, ms) in [
            ("2024-05-01T10:00:00.123Z", 1_714_557_600_123),
            ("2024-05-01 12:00:00.123+02:00", 1_714_557_600_123),
            ("2024-05-01", 1_714_521_600_000),
            ("1714557600123", 1_714_557_600_123),
            ("1969-12-31 23:59:59.999+0000", -1),
        ] {
            assert_eq!(
                enc(text(text_form), &n(NativeType::Timestamp)),
                CqlValue::Timestamp(CqlTimestamp(ms)),
                "{text_form}"
            );
        }
        let date = enc(Value::Date("1969-12-31".into()), &n(NativeType::Date));
        assert_eq!(date, CqlValue::Date(CqlDate((1u32 << 31) - 1)));
        assert_eq!(decode_value(&date), Value::Date("1969-12-31".into()));
        let t = enc(
            Value::Time("10:11:12.000000123".into()),
            &n(NativeType::Time),
        );
        assert_eq!(decode_value(&t), Value::Time("10:11:12.000000123".into()));
        let d = enc(
            Value::Interval("1y2mo3d4h5m6s7ms8us9ns".into()),
            &n(NativeType::Duration),
        );
        assert_eq!(
            decode_value(&d),
            Value::Interval("1y2mo3d4h5m6s7ms8us9ns".into())
        );
        assert_eq!(
            enc(text("-P1DT2H"), &n(NativeType::Duration)),
            CqlValue::Duration(CqlDuration {
                months: 0,
                days: -1,
                nanoseconds: -7_200_000_000_000
            })
        );
        for bad in ["yesterday", "2024-13-01"] {
            assert!(
                encode(&text(bad), &n(NativeType::Timestamp)).is_err(),
                "{bad}"
            );
        }
        assert!(encode(&text("1x"), &n(NativeType::Duration)).is_err());
    }

    #[test]
    fn scalars_of_other_kinds() {
        assert_eq!(
            enc(text("true"), &n(NativeType::Boolean)),
            CqlValue::Boolean(true)
        );
        assert_eq!(
            enc(text("0xff00"), &n(NativeType::Blob)),
            CqlValue::Blob(vec![255, 0])
        );
        assert!(encode(&text("ff00"), &n(NativeType::Blob)).is_err());
        let truncated = Value::Bytes(crate::types::value::BytesValue {
            base64: "AA==".into(),
            len: 99,
            truncated: true,
        });
        assert!(encode(&truncated, &n(NativeType::Blob)).is_err());
        assert_eq!(
            enc(
                Value::Uuid("6f9619ff-8b86-d011-b42d-00c04fc964ff".into()),
                &n(NativeType::Uuid)
            ),
            CqlValue::Uuid(Uuid::parse_str("6f9619ff-8b86-d011-b42d-00c04fc964ff").unwrap())
        );
        // A version 4 UUID is no timeuuid.
        assert!(
            encode(
                &Value::Uuid("6f9619ff-8b86-4011-b42d-00c04fc964ff".into()),
                &n(NativeType::Timeuuid)
            )
            .is_err()
        );
        assert_eq!(
            enc(Value::other("inet", "10.0.0.1"), &n(NativeType::Inet)),
            CqlValue::Inet("10.0.0.1".parse().unwrap())
        );
        assert!(encode(&text("é"), &n(NativeType::Ascii)).is_err());
        assert_eq!(
            enc(text(" a "), &n(NativeType::Text)),
            CqlValue::Text(" a ".into())
        );
        assert_eq!(encode(&Value::Null, &n(NativeType::Text)).unwrap(), None);
        assert!(encode(&Value::Default, &n(NativeType::Text)).is_err());
    }

    #[test]
    fn collections_tuples_and_udts_from_their_json() {
        let list = ColumnType::Collection {
            frozen: false,
            typ: CollectionType::List(Box::new(n(NativeType::Int))),
        };
        assert_eq!(
            enc(Value::Json("[1, 2]".into()), &list),
            CqlValue::List(vec![CqlValue::Int(1), CqlValue::Int(2)])
        );
        assert!(encode(&Value::Json("[1, null]".into()), &list).is_err());
        assert!(encode(&Value::Json("{".into()), &list).is_err());
        let map = ColumnType::Collection {
            frozen: false,
            typ: CollectionType::Map(Box::new(n(NativeType::Int)), Box::new(n(NativeType::Text))),
        };
        // `toJson()` quotes every key; a number key is read back from its text.
        let v = enc(Value::Json(r#"{"1": "a", "2": "b"}"#.into()), &map);
        assert_eq!(
            decode_value(&v),
            Value::Json(r#"{"1": "a", "2": "b"}"#.into())
        );
        let tuple = ColumnType::Tuple(vec![n(NativeType::Int), n(NativeType::Text)]);
        assert_eq!(
            enc(Value::Json(r#"[1, null]"#.into()), &tuple),
            CqlValue::Tuple(vec![Some(CqlValue::Int(1)), None])
        );
        assert!(encode(&Value::Json("[1]".into()), &tuple).is_err());
        let udt = ColumnType::UserDefinedType {
            frozen: true,
            definition: Arc::new(UserDefinedType {
                name: Cow::Borrowed("address"),
                keyspace: Cow::Borrowed("shop"),
                field_types: vec![
                    (Cow::Borrowed("street"), n(NativeType::Text)),
                    (Cow::Borrowed("zip"), n(NativeType::Int)),
                ],
            }),
        };
        assert_eq!(
            enc(Value::Json(r#"{"street": "Main"}"#.into()), &udt),
            CqlValue::UserDefinedType {
                keyspace: "shop".into(),
                name: "address".into(),
                fields: vec![
                    ("street".into(), Some(CqlValue::Text("Main".into()))),
                    ("zip".into(), None)
                ],
            }
        );
        assert!(encode(&Value::Json(r#"{"city": "X"}"#.into()), &udt).is_err());
        let vector = ColumnType::Vector {
            typ: Box::new(n(NativeType::Float)),
            dimensions: 2,
        };
        assert_eq!(
            enc(Value::Json("[0.5, 1]".into()), &vector),
            CqlValue::Vector(vec![CqlValue::Float(0.5), CqlValue::Float(1.0)])
        );
        assert!(encode(&Value::Int(1), &list).is_err());
    }
}
