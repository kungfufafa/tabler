//! BSON values to Tabler's `Value` and back.
//!
//! Decoding is total: nothing here panics on what a server sends. The mapping keeps every type
//! apart so an edit can write back exactly what was read (a string `_id` stays a string, an
//! Int32 stays an Int32):
//!
//! - double → `Float` (NaN and the infinities `Other{double}`), string → `Text`, bool, null;
//! - int → `Int`, long → `Int` or `BigInt` beyond 2^53, decimal → `Decimal` (its exact text);
//! - date → `TimestampTz` (`2024-05-01T10:00:00.000Z`) for the years 0–9999, otherwise
//!   `Other{date}` with the canonical Extended JSON;
//! - objectId → `Other{objectId, <hex>}`; a field a document does not have → `Other{missing}`;
//! - binary subtype 0 → `Bytes`, subtype 4 → `Uuid`, other subtypes `Other{binData}`;
//! - embedded documents and arrays → `Json` (relaxed Extended JSON, `sqlgen::native::mongo`);
//! - regex, timestamp, javascript, symbol, minKey, maxKey, undefined, dbPointer → `Other` named
//!   by their `$type` alias, carrying the canonical Extended JSON.
//!
//! Encoding (`to_bson`) takes the `$type` alias the field had as a hint for the width of numbers;
//! the other variants say their type themselves. `text_as` reads text typed for a field as the
//! field's own type, for cells the shared editor staged by their column's kind.

use ::mongodb::bson::spec::BinarySubtype;
use ::mongodb::bson::{Binary, Bson, DateTime, Decimal128, oid::ObjectId};
use base64::Engine as _;
use chrono::{NaiveDate, NaiveDateTime, TimeZone as _, Utc};

use crate::drivers::mongodb::literal;
use crate::error::AppError;
use crate::sqlgen::native::mongo::{JsonMode, bson_json};
use crate::types::schema::ValueKind;
use crate::types::value::{OtherValue, Value};

/// `Other` type of a field the document does not have (not the same as null).
pub const TY_MISSING: &str = "missing";
/// `Other` type of an ObjectId; its text is the 24 hex digits.
pub const TY_OBJECT_ID: &str = "objectId";
/// `Other` type of a nested value too large for a grid cell: a cut preview, never written back.
pub const TY_PREVIEW: &str = "jsonPreview";
/// `Other` type the shared cell editor gives text typed into a cell of an `other` column whose
/// own value was not an `Other` (a null, a string…): text whose type the editor did not know.
pub const TY_UNKNOWN: &str = "other";

/// Nested values (and binaries of other subtypes) longer than this as JSON text are cut in grid
/// cells; `fetch_cell` loads them whole.
pub const JSON_CELL_CAP: usize = 64 * 1024;

/// Milliseconds of 9999-12-31T23:59:59.999Z, the last instant with a four-digit year.
const MAX_ISO_MILLIS: i64 = 253_402_300_799_999;
/// Milliseconds of 0000-01-01T00:00:00Z.
const MIN_ISO_MILLIS: i64 = -62_167_219_200_000;

/// The `$type` alias of a value (`objectId`, `int`, `long`, `object`…).
pub fn type_name(b: &Bson) -> &'static str {
    match b {
        Bson::Double(_) => "double",
        Bson::String(_) => "string",
        Bson::Document(_) => "object",
        Bson::Array(_) => "array",
        Bson::Binary(_) => "binData",
        Bson::Undefined => "undefined",
        Bson::ObjectId(_) => "objectId",
        Bson::Boolean(_) => "bool",
        Bson::DateTime(_) => "date",
        Bson::Null => "null",
        Bson::RegularExpression(_) => "regex",
        Bson::DbPointer(_) => "dbPointer",
        Bson::JavaScriptCode(_) => "javascript",
        Bson::Symbol(_) => "symbol",
        Bson::JavaScriptCodeWithScope(_) => "javascriptWithScope",
        Bson::Int32(_) => "int",
        Bson::Timestamp(_) => "timestamp",
        Bson::Int64(_) => "long",
        Bson::Decimal128(_) => "decimal",
        Bson::MinKey => "minKey",
        Bson::MaxKey => "maxKey",
    }
}

/// The type label of a value in a column's type list: its `$type` alias, except `uuid` for a
/// subtype-4 binary (shown as a UUID rather than bytes).
pub fn type_label(b: &Bson) -> &'static str {
    match b {
        Bson::Binary(bin) if is_uuid(bin) => "uuid",
        b => type_name(b),
    }
}

/// How a column whose values are mostly of type `label` is shown and edited.
pub fn kind_of(label: &str) -> ValueKind {
    match label {
        "double" => ValueKind::Float,
        "string" | "null" | "missing" => ValueKind::Text,
        "object" | "array" => ValueKind::Json,
        "binData" => ValueKind::Bytes,
        "uuid" => ValueKind::Uuid,
        "bool" => ValueKind::Bool,
        "date" => ValueKind::TimestampTz,
        "int" => ValueKind::Int,
        "long" => ValueKind::BigInt,
        "decimal" => ValueKind::Decimal,
        _ => ValueKind::Other,
    }
}

/// The cell of a field the document does not have.
pub fn missing() -> Value {
    Value::other(TY_MISSING, "")
}

pub fn is_missing(v: &Value) -> bool {
    matches!(v, Value::Other(OtherValue { ty, .. }) if ty == TY_MISSING)
}

/// A value for a grid cell: binaries and nested values are cut at their preview caps.
pub fn to_value(b: &Bson) -> Value {
    decode(b, false)
}

/// A value whole (`fetch_cell`; the caller bounds the size).
pub fn to_value_full(b: &Bson) -> Value {
    decode(b, true)
}

/// A field of a document: `missing` when the document does not have it.
pub fn field_value(b: Option<&Bson>) -> Value {
    b.map_or_else(missing, to_value)
}

fn decode(b: &Bson, full: bool) -> Value {
    match b {
        Bson::Double(f) if f.is_finite() => Value::Float(*f),
        Bson::Double(f) => Value::other("double", special_double(*f)),
        Bson::String(s) => Value::Text(s.clone()),
        Bson::Boolean(v) => Value::Bool(*v),
        Bson::Null => Value::Null,
        Bson::Int32(n) => Value::Int(i64::from(*n)),
        Bson::Int64(n) => Value::from_i64(*n),
        Bson::Decimal128(d) => Value::Decimal(d.to_string()),
        Bson::DateTime(d) => match iso_text(d.timestamp_millis()) {
            Some(text) => Value::TimestampTz(text),
            None => Value::other("date", bson_json(b, JsonMode::Canonical)),
        },
        Bson::ObjectId(oid) => Value::other(TY_OBJECT_ID, oid.to_hex()),
        Bson::Binary(bin) if is_uuid(bin) => Value::Uuid(uuid_text(&bin.bytes)),
        Bson::Binary(bin) if bin.subtype == BinarySubtype::Generic => {
            if full {
                Value::from_bytes_uncapped(&bin.bytes)
            } else {
                Value::from_bytes(&bin.bytes)
            }
        }
        Bson::Document(_) | Bson::Array(_) => {
            capped(bson_json(b, JsonMode::Relaxed), full).unwrap_or_else(Value::Json)
        }
        other => {
            let text = bson_json(other, JsonMode::Canonical);
            capped(text, full).unwrap_or_else(|t| Value::other(type_name(other), t))
        }
    }
}

/// `Ok(preview)` when `text` is over the cell cap (and `full` is false), `Err(text)` otherwise.
fn capped(text: String, full: bool) -> Result<Value, String> {
    if full || text.len() <= JSON_CELL_CAP {
        return Err(text);
    }
    let mut end = JSON_CELL_CAP;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok(Value::other(TY_PREVIEW, format!("{}…", &text[..end])))
}

fn is_uuid(bin: &Binary) -> bool {
    bin.subtype == BinarySubtype::Uuid && bin.bytes.len() == 16
}

fn uuid_text(bytes: &[u8]) -> String {
    uuid::Uuid::from_slice(bytes)
        .map(|u| u.hyphenated().to_string())
        .unwrap_or_default()
}

/// The Extended JSON spelling of a double that is not finite.
pub fn special_double(f: f64) -> &'static str {
    if f.is_nan() {
        "NaN"
    } else if f > 0.0 {
        "Infinity"
    } else {
        "-Infinity"
    }
}

/// `2024-05-01T10:00:00.000Z` for the years 0–9999, `None` outside.
pub fn iso_text(millis: i64) -> Option<String> {
    if !(MIN_ISO_MILLIS..=MAX_ISO_MILLIS).contains(&millis) {
        return None;
    }
    let dt = Utc.timestamp_millis_opt(millis).single()?;
    Some(dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// Milliseconds since the epoch of a date or timestamp text: RFC 3339 / ISO 8601 with a `Z` or
/// an offset, or without one (UTC), with `T` or a space, or a date alone (midnight UTC).
/// Fractions below a millisecond are dropped (BSON dates have milliseconds).
pub fn parse_date_millis(text: &str) -> Option<i64> {
    let mut s = text.trim().to_string();
    if s.len() > 10 && s.as_bytes().get(10) == Some(&b' ') {
        s.replace_range(10..11, "T");
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&s) {
        return Some(dt.timestamp_millis());
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f%z", "%Y-%m-%dT%H:%M%z"] {
        if let Ok(dt) = chrono::DateTime::parse_from_str(&s, fmt) {
            return Some(dt.timestamp_millis());
        }
    }
    let naive = s.strip_suffix('Z').unwrap_or(&s);
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(naive, fmt) {
            return Some(dt.and_utc().timestamp_millis());
        }
    }
    let date = NaiveDate::parse_from_str(naive, "%Y-%m-%d").ok()?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis())
}

/// A cell value as BSON. `hint` is the `$type` alias the field had (the loaded value's type, or
/// the column's dominant one): it picks Int32, Int64, Double or Decimal128 for a number; every
/// other variant names its type itself. `missing` and `Default` are not values (an update unsets
/// a missing field; MongoDB has no defaults) and are refused here.
pub fn to_bson(v: &Value, hint: Option<&str>) -> Result<Bson, AppError> {
    Ok(match v {
        Value::Null => Bson::Null,
        Value::Bool(b) => Bson::Boolean(*b),
        Value::Int(n) => int_bson(*n, hint)?,
        Value::BigInt(s) => match s.trim().parse::<i64>() {
            Ok(n) => int_bson(n, hint)?,
            Err(_) if hint == Some("decimal") => decimal(s)?,
            Err(_) => {
                return Err(AppError::InvalidInput(format!(
                    "{s} does not fit a 64-bit integer (use a decimal)"
                )));
            }
        },
        Value::Float(f) => match hint {
            Some("decimal") => decimal(&f.to_string())?,
            _ => Bson::Double(*f),
        },
        Value::Decimal(s) => match hint {
            Some("double") => Bson::Double(parse_double(s)?),
            _ => decimal(s)?,
        },
        Value::Text(s) | Value::Enum(s) | Value::Interval(s) | Value::Time(s) => {
            Bson::String(s.clone())
        }
        Value::Bytes(b) => {
            if b.truncated {
                return Err(AppError::InvalidInput(
                    "the binary value was cut for display; load the full value before writing it"
                        .into(),
                ));
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&b.base64)
                .map_err(|e| AppError::InvalidInput(format!("invalid binary value: {e}")))?;
            Bson::Binary(Binary {
                subtype: BinarySubtype::Generic,
                bytes,
            })
        }
        Value::Date(s) | Value::Timestamp(s) | Value::TimestampTz(s) => {
            let millis = parse_date_millis(s)
                .ok_or_else(|| AppError::InvalidInput(format!("not a date: {s}")))?;
            Bson::DateTime(DateTime::from_millis(millis))
        }
        Value::Uuid(s) => {
            let u = uuid::Uuid::parse_str(s.trim())
                .map_err(|_| AppError::InvalidInput(format!("not a UUID: {s}")))?;
            Bson::Binary(Binary {
                subtype: BinarySubtype::Uuid,
                bytes: u.as_bytes().to_vec(),
            })
        }
        Value::Json(text) => literal::parse_value(text).map_err(|e| e.into_app(text))?,
        Value::Array(items) => Bson::Array(
            items
                .iter()
                .map(|item| to_bson(item, None))
                .collect::<Result<_, _>>()?,
        ),
        Value::Other(o) => other_bson(o)?,
        Value::Default => {
            return Err(AppError::InvalidInput(
                "MongoDB has no DEFAULT values".into(),
            ));
        }
    })
}

fn int_bson(n: i64, hint: Option<&str>) -> Result<Bson, AppError> {
    Ok(match hint {
        Some("long") => Bson::Int64(n),
        Some("double") => Bson::Double(n as f64),
        Some("decimal") => decimal(&n.to_string())?,
        _ => match i32::try_from(n) {
            Ok(i) => Bson::Int32(i),
            Err(_) => Bson::Int64(n),
        },
    })
}

fn decimal(s: &str) -> Result<Bson, AppError> {
    parse_decimal(s)
        .map(Bson::Decimal128)
        .ok_or_else(|| AppError::InvalidInput(format!("not a decimal: {s}")))
}

/// A Decimal128 from text: a number, NaN or an infinity. bson's parser reads text without a
/// digit (``, `.`, `-`, `e5`) as 0, so a cleared cell would write 0; the text must also read as
/// a double (which takes the same NaN and infinity spellings).
pub fn parse_decimal(s: &str) -> Option<Decimal128> {
    let t = s.trim();
    if t.parse::<f64>().is_err() {
        return None;
    }
    t.parse().ok()
}

/// A double from text: a number, or NaN and the infinities in any case and in the spellings the
/// shared cell editor accepts (`nan`, `inf`, `-Infinity`).
fn parse_double(s: &str) -> Result<f64, AppError> {
    let t = s.trim();
    let (negative, name) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if name.eq_ignore_ascii_case("nan") {
        return Ok(f64::NAN);
    }
    if name.eq_ignore_ascii_case("inf") || name.eq_ignore_ascii_case("infinity") {
        return Ok(if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        });
    }
    t.parse::<f64>()
        .map_err(|_| AppError::InvalidInput(format!("not a number: {s}")))
}

fn other_bson(o: &OtherValue) -> Result<Bson, AppError> {
    match o.ty.as_str() {
        TY_OBJECT_ID => text_as(&o.text, TY_OBJECT_ID)
            .ok_or_else(|| AppError::InvalidInput(format!("not an ObjectId: {}", o.text))),
        TY_MISSING => Err(AppError::InvalidInput(
            "a missing field has no value to write".into(),
        )),
        TY_PREVIEW => Err(AppError::InvalidInput(
            "the value was cut for display; load the full value before writing it".into(),
        )),
        // `float`: the shared cell editor stages a float a JavaScript number cannot hold
        // (`nan`, `inf`) as `Other{float}`.
        "double" | "float" => parse_double(&o.text).map(Bson::Double),
        // The other types carry their Extended JSON (or are typed in shell syntax).
        _ => literal::parse_value(&o.text).map_err(|e| e.into_app(&o.text)),
    }
}

/// Whether `b` is a value of type `ty`, a `$type` alias or a type label (a subtype-4 binary is
/// both `binData` and `uuid`).
pub fn is_type(b: &Bson, ty: &str) -> bool {
    type_name(b) == ty || type_label(b) == ty
}

/// The text a staged value stands for, to read it as another type: numbers in their shortest
/// form (7.0 is `7`), booleans as `true` / `false`, the other variants as staged. Binaries,
/// arrays and cut previews (never written back) have none.
pub fn value_text(v: &Value) -> Option<String> {
    match v {
        Value::Other(o) if o.ty == TY_PREVIEW => None,
        Value::Int(n) => Some(n.to_string()),
        Value::Float(f) => Some(f.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::BigInt(s)
        | Value::Decimal(s)
        | Value::Text(s)
        | Value::Enum(s)
        | Value::Interval(s)
        | Value::Time(s)
        | Value::Date(s)
        | Value::Timestamp(s)
        | Value::TimestampTz(s)
        | Value::Uuid(s)
        | Value::Json(s) => Some(s.clone()),
        Value::Other(o) => Some(o.text.clone()),
        Value::Null | Value::Default | Value::Bytes(_) | Value::Array(_) => None,
    }
}

/// An integer from text, also with a fraction of zeros (`7.0`, `-3.00`).
fn parse_integer(t: &str) -> Option<i64> {
    let whole = match t.split_once('.') {
        Some((whole, frac)) if !frac.is_empty() && frac.bytes().all(|c| c == b'0') => whole,
        Some(_) => return None,
        None => t,
    };
    whole.parse().ok()
}

/// Text typed for a field of type `ty` (a `$type` alias or a column's type label), read as that
/// type: an ObjectId's hex, a date, a number of that width, `true` / `false`, a UUID, or any
/// spelling the console reads that gives that type (`ObjectId("…")`, `{"$date": …}`, `/re/i`,
/// `{a: 1}` for an object). `None` when the text is not a value of that type. An `int` beyond 32
/// bits comes back as a long (callers that keep a field's type check it with `is_type`).
pub fn text_as(text: &str, ty: &str) -> Option<Bson> {
    let t = text.trim();
    let direct = match ty {
        "string" => Some(Bson::String(text.to_string())),
        TY_OBJECT_ID => ObjectId::parse_str(t).ok().map(Bson::ObjectId),
        "date" => parse_date_millis(t).map(|ms| Bson::DateTime(DateTime::from_millis(ms))),
        "int" | "long" => parse_integer(t).and_then(|n| int_bson(n, Some(ty)).ok()),
        "double" => parse_double(t).ok().map(Bson::Double),
        "decimal" => decimal(t).ok(),
        "bool" if t.eq_ignore_ascii_case("true") => Some(Bson::Boolean(true)),
        "bool" if t.eq_ignore_ascii_case("false") => Some(Bson::Boolean(false)),
        "uuid" => uuid::Uuid::parse_str(t).ok().map(|u| {
            Bson::Binary(Binary {
                subtype: BinarySubtype::Uuid,
                bytes: u.as_bytes().to_vec(),
            })
        }),
        _ => None,
    };
    direct.or_else(|| literal::parse_value(text).ok().filter(|b| is_type(b, ty)))
}

#[cfg(test)]
mod tests {
    use ::mongodb::bson::{Regex, Timestamp, doc};

    use super::*;

    fn other(ty: &str, text: &str) -> Value {
        Value::other(ty, text)
    }

    #[test]
    fn scalars_map_to_their_variants() {
        assert_eq!(to_value(&Bson::Double(1.5)), Value::Float(1.5));
        assert_eq!(to_value(&Bson::String("a".into())), Value::Text("a".into()));
        assert_eq!(to_value(&Bson::Boolean(true)), Value::Bool(true));
        assert_eq!(to_value(&Bson::Null), Value::Null);
        assert_eq!(to_value(&Bson::Int32(-7)), Value::Int(-7));
        assert_eq!(to_value(&Bson::Int64(1 << 40)), Value::Int(1 << 40));
        assert_eq!(
            to_value(&Bson::Int64((1 << 53) + 1)),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            to_value(&Bson::Int64(i64::MIN)),
            Value::BigInt("-9223372036854775808".into())
        );
    }

    #[test]
    fn doubles_that_are_not_finite_are_other() {
        assert_eq!(to_value(&Bson::Double(f64::NAN)), other("double", "NaN"));
        assert_eq!(
            to_value(&Bson::Double(f64::INFINITY)),
            other("double", "Infinity")
        );
        assert_eq!(
            to_value(&Bson::Double(f64::NEG_INFINITY)),
            other("double", "-Infinity")
        );
        let back = to_bson(&other("double", "-Infinity"), None).unwrap();
        assert_eq!(back, Bson::Double(f64::NEG_INFINITY));
        assert!(
            matches!(to_bson(&other("double", "NaN"), None).unwrap(), Bson::Double(f) if f.is_nan())
        );
        // The shared cell editor stages what a JavaScript number cannot hold as `Other{float}`,
        // spelled as typed.
        for (text, want) in [
            ("inf", f64::INFINITY),
            ("+INF", f64::INFINITY),
            ("Infinity", f64::INFINITY),
            ("-infinity", f64::NEG_INFINITY),
            ("-Inf", f64::NEG_INFINITY),
        ] {
            assert_eq!(
                to_bson(&other("float", text), None).unwrap(),
                Bson::Double(want),
                "{text}"
            );
        }
        for text in ["nan", "NAN", "-nan", " NaN "] {
            assert!(
                matches!(to_bson(&other("float", text), None).unwrap(), Bson::Double(f) if f.is_nan()),
                "{text}"
            );
        }
        assert!(to_bson(&other("float", "infinite"), None).is_err());
    }

    #[test]
    fn typed_text_reads_as_the_field_type() {
        let oid = ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap();
        let hex = "65f1a2b3c4d5e6f708192a3b";
        assert_eq!(text_as(hex, TY_OBJECT_ID), Some(Bson::ObjectId(oid)));
        assert_eq!(
            text_as(&format!(" ObjectId('{hex}') "), TY_OBJECT_ID),
            Some(Bson::ObjectId(oid))
        );
        assert_eq!(text_as("zz", TY_OBJECT_ID), None);
        // A shell value of another type is not one of this type.
        assert_eq!(text_as("12", TY_OBJECT_ID), None);
        assert_eq!(text_as(hex, "string"), Some(Bson::String(hex.into())));
        assert_eq!(
            text_as("2024-05-01T10:00:00.123Z", "date"),
            Some(Bson::DateTime(DateTime::from_millis(1_714_557_600_123)))
        );
        assert_eq!(
            text_as("ISODate('1970-01-01T00:00:00Z')", "date"),
            Some(Bson::DateTime(DateTime::from_millis(0)))
        );
        assert_eq!(text_as("soon", "date"), None);
        assert_eq!(text_as(" 7 ", "int"), Some(Bson::Int32(7)));
        assert_eq!(text_as("7", "long"), Some(Bson::Int64(7)));
        assert_eq!(text_as("NumberLong(7)", "long"), Some(Bson::Int64(7)));
        assert_eq!(text_as("7.5", "int"), None);
        assert_eq!(text_as("-7.00", "int"), Some(Bson::Int32(-7)));
        assert_eq!(text_as("7.", "int"), None);
        assert_eq!(text_as(".0", "long"), None);
        // Beyond 32 bits an `int` reads as a long, which `is_type` tells apart.
        let wide = text_as("3000000000", "int").unwrap();
        assert_eq!(wide, Bson::Int64(3_000_000_000));
        assert!(!is_type(&wide, "int"));
        assert_eq!(text_as("7", "double"), Some(Bson::Double(7.0)));
        assert!(matches!(text_as("nan", "double"), Some(Bson::Double(f)) if f.is_nan()));
        assert_eq!(
            text_as("7.50", "decimal"),
            Some(Bson::Decimal128("7.50".parse().unwrap()))
        );
        // bson reads text without a digit as 0: a cleared decimal cell is not 0.
        for t in ["", " ", ".", "-", "+", "e5", "-.e5", "sNaN"] {
            assert_eq!(text_as(t, "decimal"), None, "{t:?}");
            assert!(to_bson(&Value::Decimal(t.into()), None).is_err(), "{t:?}");
        }
        for t in ["NaN", "-Infinity", "inf", " 1e5 ", "-0", ".5", "5."] {
            let d = text_as(t, "decimal");
            assert!(matches!(d, Some(Bson::Decimal128(_))), "{t:?}: {d:?}");
        }
        assert_eq!(
            text_as("-Infinity", "decimal"),
            Some(Bson::Decimal128("-Infinity".parse().unwrap()))
        );
        assert_eq!(text_as("TRUE", "bool"), Some(Bson::Boolean(true)));
        assert_eq!(text_as("yes", "bool"), None);
        let u = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let uuid = Bson::Binary(Binary {
            subtype: BinarySubtype::Uuid,
            bytes: uuid::Uuid::parse_str(u).unwrap().as_bytes().to_vec(),
        });
        assert_eq!(text_as(u, "uuid"), Some(uuid.clone()));
        assert_eq!(text_as(&format!("UUID('{u}')"), "binData"), Some(uuid));
        assert_eq!(text_as("AAEC", "binData"), None);
        assert_eq!(
            text_as("{a: 1}", "object"),
            Some(Bson::Document(doc! { "a": 1 }))
        );
        assert_eq!(text_as("[1]", "object"), None);
        assert_eq!(
            text_as("[1]", "array"),
            Some(Bson::Array(vec![Bson::Int32(1)]))
        );
        assert_eq!(
            text_as("/a+/i", "regex"),
            Some(Bson::RegularExpression(Regex {
                pattern: "a+".into(),
                options: "i".into()
            }))
        );
        assert_eq!(text_as("plain", "regex"), None);
    }

    #[test]
    fn staged_values_have_the_text_they_stand_for() {
        assert_eq!(value_text(&Value::Float(7.0)).as_deref(), Some("7"));
        assert_eq!(value_text(&Value::Float(-0.5)).as_deref(), Some("-0.5"));
        assert_eq!(value_text(&Value::Int(-3)).as_deref(), Some("-3"));
        assert_eq!(value_text(&Value::Bool(false)).as_deref(), Some("false"));
        assert_eq!(
            value_text(&Value::Decimal("7.50".into())).as_deref(),
            Some("7.50")
        );
        assert_eq!(value_text(&other("regex", "/a/")).as_deref(), Some("/a/"));
        // A cut preview is never read back, nor are binaries.
        assert_eq!(value_text(&other(TY_PREVIEW, "{\"a\": …")), None);
        assert_eq!(value_text(&Value::from_bytes(&[1, 2])), None);
        assert_eq!(value_text(&Value::Null), None);
    }

    #[test]
    fn decimal128_keeps_its_exact_text() {
        for s in [
            "0.1",
            "-1234567890123456789012345678901234",
            "1.000",
            "NaN",
            "-Infinity",
        ] {
            let d: Decimal128 = s.parse().unwrap();
            let v = to_value(&Bson::Decimal128(d));
            assert_eq!(v, Value::Decimal(d.to_string()));
            assert_eq!(to_bson(&v, None).unwrap(), Bson::Decimal128(d));
        }
        assert_eq!(
            to_value(&Bson::Decimal128("1.000".parse().unwrap())),
            Value::Decimal("1.000".into())
        );
    }

    #[test]
    fn dates_are_utc_text_with_milliseconds() {
        let d = Bson::DateTime(DateTime::from_millis(1_714_557_600_123));
        assert_eq!(
            to_value(&d),
            Value::TimestampTz("2024-05-01T10:00:00.123Z".into())
        );
        // Before 1970.
        let old = Bson::DateTime(DateTime::from_millis(-86_400_001));
        assert_eq!(
            to_value(&old),
            Value::TimestampTz("1969-12-30T23:59:59.999Z".into())
        );
        assert_eq!(to_bson(&to_value(&old), None).unwrap(), old);
        assert_eq!(to_bson(&to_value(&d), None).unwrap(), d);
        // Outside the four-digit years: canonical Extended JSON, and back.
        for ms in [i64::MAX, i64::MIN, MAX_ISO_MILLIS + 1, MIN_ISO_MILLIS - 1] {
            let b = Bson::DateTime(DateTime::from_millis(ms));
            let v = to_value(&b);
            assert_eq!(
                v,
                other(
                    "date",
                    &format!("{{\"$date\": {{\"$numberLong\": \"{ms}\"}}}}")
                )
            );
            assert_eq!(to_bson(&v, None).unwrap(), b);
        }
        assert!(iso_text(MAX_ISO_MILLIS).is_some());
        assert!(iso_text(MIN_ISO_MILLIS).is_some());
    }

    #[test]
    fn date_texts_parse_in_many_spellings() {
        let base = 1_714_557_600_000; // 2024-05-01T10:00:00Z
        for s in [
            "2024-05-01T10:00:00Z",
            "2024-05-01T10:00:00.000Z",
            "2024-05-01 10:00:00",
            "2024-05-01T10:00:00",
            "2024-05-01T10:00",
            "2024-05-01T12:00:00+02:00",
            "2024-05-01T12:00:00+0200",
            " 2024-05-01 10:00:00Z ",
        ] {
            assert_eq!(parse_date_millis(s), Some(base), "{s}");
        }
        assert_eq!(parse_date_millis("2024-05-01"), Some(1_714_521_600_000));
        assert_eq!(
            parse_date_millis("2024-05-01T10:00:00.1239Z"),
            Some(base + 123)
        );
        assert_eq!(parse_date_millis("yesterday"), None);
        assert_eq!(parse_date_millis(""), None);
        assert_eq!(parse_date_millis("2024-13-01"), None);
    }

    #[test]
    fn object_ids_round_trip_and_stay_apart_from_strings() {
        let oid = ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap();
        let v = to_value(&Bson::ObjectId(oid));
        assert_eq!(v, other(TY_OBJECT_ID, "65f1a2b3c4d5e6f708192a3b"));
        assert_eq!(to_bson(&v, None).unwrap(), Bson::ObjectId(oid));
        // The same hex as text stays a string (TablePlus #1183).
        let s = Value::Text("65f1a2b3c4d5e6f708192a3b".into());
        assert_eq!(
            to_bson(&s, Some("objectId")).unwrap(),
            Bson::String("65f1a2b3c4d5e6f708192a3b".into())
        );
        assert!(to_bson(&other(TY_OBJECT_ID, "zz"), None).is_err());
    }

    #[test]
    fn binaries_by_subtype() {
        let generic = Bson::Binary(Binary {
            subtype: BinarySubtype::Generic,
            bytes: vec![0, 1, 2, 255],
        });
        let v = to_value(&generic);
        assert!(matches!(&v, Value::Bytes(b) if b.base64 == "AAEC/w==" && b.len == 4));
        assert_eq!(to_bson(&v, None).unwrap(), generic);

        let u = uuid::Uuid::parse_str("0f8fad5b-d9cb-469f-a165-70867728950e").unwrap();
        let uuid4 = Bson::Binary(Binary {
            subtype: BinarySubtype::Uuid,
            bytes: u.as_bytes().to_vec(),
        });
        let v = to_value(&uuid4);
        assert_eq!(
            v,
            Value::Uuid("0f8fad5b-d9cb-469f-a165-70867728950e".into())
        );
        assert_eq!(to_bson(&v, None).unwrap(), uuid4);

        // Legacy UUIDs (subtype 3) keep their subtype through Extended JSON.
        let uuid3 = Bson::Binary(Binary {
            subtype: BinarySubtype::UuidOld,
            bytes: u.as_bytes().to_vec(),
        });
        let v = to_value(&uuid3);
        assert_eq!(
            v,
            other(
                "binData",
                "{\"$binary\": {\"base64\": \"D4+tW9nLRp+hZXCGdyiVDg==\", \"subType\": \"03\"}}"
            )
        );
        assert_eq!(to_bson(&v, None).unwrap(), uuid3);
        // A subtype-4 value of the wrong length is not a UUID.
        let odd = Bson::Binary(Binary {
            subtype: BinarySubtype::Uuid,
            bytes: vec![1, 2, 3],
        });
        assert!(matches!(to_value(&odd), Value::Other(o) if o.ty == "binData"));
        assert_eq!(to_bson(&to_value(&odd), None).unwrap(), odd);
    }

    #[test]
    fn large_binaries_are_cut_for_cells_but_not_for_fetch() {
        let big = Bson::Binary(Binary {
            subtype: BinarySubtype::Generic,
            bytes: vec![7; crate::types::value::BYTES_PREVIEW_CAP + 10],
        });
        let cell = to_value(&big);
        assert!(matches!(&cell, Value::Bytes(b) if b.truncated));
        assert!(to_bson(&cell, None).is_err());
        let full = to_value_full(&big);
        assert!(matches!(&full, Value::Bytes(b) if !b.truncated));
        assert_eq!(to_bson(&full, None).unwrap(), big);
    }

    #[test]
    fn nested_values_are_json_and_parse_back_exactly() {
        let d = doc! {
            "b": 1, "a": [1_i64, 2.0, "x", null, { "$x": true }],
            "when": DateTime::from_millis(0), "big": 9_007_199_254_740_993_i64,
            "small": 5_i64,
        };
        let v = to_value(&Bson::Document(d.clone()));
        let Value::Json(text) = &v else {
            panic!("{v:?}")
        };
        assert!(text.starts_with("{\"b\": 1, \"a\": ["), "{text}");
        assert_eq!(to_bson(&v, None).unwrap(), Bson::Document(d));
    }

    #[test]
    fn large_nested_values_are_previews_that_refuse_writes() {
        let long = "x".repeat(JSON_CELL_CAP + 5);
        let b = Bson::Array(vec![Bson::String(long)]);
        let cell = to_value(&b);
        assert!(matches!(&cell, Value::Other(o) if o.ty == TY_PREVIEW && o.text.ends_with('…')));
        assert!(to_bson(&cell, None).is_err());
        assert!(matches!(to_value_full(&b), Value::Json(_)));
        // A cut at a multi-byte character stays on a boundary.
        let wide = "é".repeat(JSON_CELL_CAP);
        assert!(matches!(
            to_value(&Bson::Array(vec![wide.into()])),
            Value::Other(_)
        ));
    }

    #[test]
    fn rare_types_carry_canonical_extended_json() {
        let cases = [
            Bson::RegularExpression(Regex {
                pattern: "^a.*".into(),
                options: "im".into(),
            }),
            Bson::Timestamp(Timestamp {
                time: 1_700_000_000,
                increment: 7,
            }),
            Bson::JavaScriptCode("function() { return 1 }".into()),
            Bson::Symbol("sym".into()),
            Bson::MinKey,
            Bson::MaxKey,
            Bson::Undefined,
        ];
        for b in cases {
            let v = to_value(&b);
            let Value::Other(o) = &v else { panic!("{v:?}") };
            assert_eq!(o.ty, type_name(&b));
            assert!(o.text.starts_with("{\"$"), "{}", o.text);
            assert_eq!(to_bson(&v, None).unwrap(), b);
        }
    }

    #[test]
    fn missing_is_not_null() {
        assert!(is_missing(&missing()));
        assert!(!is_missing(&Value::Null));
        assert_eq!(field_value(None), missing());
        assert_eq!(field_value(Some(&Bson::Null)), Value::Null);
        assert!(to_bson(&missing(), None).is_err());
        assert!(to_bson(&Value::Default, None).is_err());
    }

    #[test]
    fn numbers_take_the_width_of_their_hint() {
        assert_eq!(to_bson(&Value::Int(5), None).unwrap(), Bson::Int32(5));
        assert_eq!(
            to_bson(&Value::Int(5), Some("int")).unwrap(),
            Bson::Int32(5)
        );
        assert_eq!(
            to_bson(&Value::Int(5), Some("long")).unwrap(),
            Bson::Int64(5)
        );
        assert_eq!(
            to_bson(&Value::Int(5), Some("double")).unwrap(),
            Bson::Double(5.0)
        );
        assert_eq!(
            to_bson(&Value::Int(5), Some("decimal")).unwrap(),
            Bson::Decimal128("5".parse().unwrap())
        );
        assert_eq!(
            to_bson(&Value::Int(1 << 40), Some("int")).unwrap(),
            Bson::Int64(1 << 40)
        );
        assert_eq!(
            to_bson(&Value::BigInt("9223372036854775807".into()), None).unwrap(),
            Bson::Int64(i64::MAX)
        );
        assert!(to_bson(&Value::BigInt("9223372036854775808".into()), None).is_err());
        assert_eq!(
            to_bson(
                &Value::BigInt("9223372036854775808".into()),
                Some("decimal")
            )
            .unwrap(),
            Bson::Decimal128("9223372036854775808".parse().unwrap())
        );
        assert_eq!(
            to_bson(&Value::Float(2.5), None).unwrap(),
            Bson::Double(2.5)
        );
        assert_eq!(
            to_bson(&Value::Decimal("2.50".into()), None).unwrap(),
            Bson::Decimal128("2.50".parse().unwrap())
        );
        assert_eq!(
            to_bson(&Value::Decimal("2.5".into()), Some("double")).unwrap(),
            Bson::Double(2.5)
        );
        assert!(to_bson(&Value::Decimal("abc".into()), None).is_err());
    }

    #[test]
    fn texts_stay_strings_and_arrays_encode_items() {
        assert_eq!(
            to_bson(&Value::Text("12".into()), Some("int")).unwrap(),
            Bson::String("12".into())
        );
        assert_eq!(
            to_bson(
                &Value::Array(vec![Value::Int(1), Value::Text("a".into())]),
                None
            )
            .unwrap(),
            Bson::Array(vec![Bson::Int32(1), Bson::String("a".into())])
        );
        assert!(to_bson(&Value::Json("{a:".into()), None).is_err());
        assert_eq!(
            to_bson(
                &Value::Json("{a: ObjectId('65f1a2b3c4d5e6f708192a3b')}".into()),
                None
            )
            .unwrap(),
            Bson::Document(doc! { "a": ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap() })
        );
    }

    #[test]
    fn labels_and_kinds() {
        assert_eq!(type_label(&Bson::Int64(1)), "long");
        let u = Bson::Binary(Binary {
            subtype: BinarySubtype::Uuid,
            bytes: vec![0; 16],
        });
        assert_eq!(type_label(&u), "uuid");
        assert_eq!(type_name(&u), "binData");
        assert_eq!(kind_of("uuid"), ValueKind::Uuid);
        assert_eq!(kind_of("objectId"), ValueKind::Other);
        assert_eq!(kind_of("object"), ValueKind::Json);
        assert_eq!(kind_of("long"), ValueKind::BigInt);
        assert_eq!(kind_of("date"), ValueKind::TimestampTz);
    }
}
