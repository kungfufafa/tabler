//! `_source` values as `Value`s and back, keeping each value's JSON type: a string stays text
//! (a date as `TimestampTz`, an ip as `Other{ip}`, a binary field as bytes), a number stays a
//! number (integers beyond 2^53 as `BigInt`, more digits than a double holds as `Decimal`),
//! arrays and objects are JSON text, a field the document does not have is `Other{missing}`.
//!
//! A dotted column (`user.name`) is found both under nested objects (`{"user": {"name": …}}`)
//! and under a literal dotted key (`{"user.name": …}`), as Elasticsearch reads it; through an
//! array of objects it collects every match into a JSON array.

use base64::Engine as _;

use crate::error::AppError;
use crate::types::value::{OtherValue, Value};

use super::catalog::{self, Field};
use super::json::{self, Json};

pub const TY_MISSING: &str = "missing";
pub const TY_IP: &str = "ip";

/// The cell of a field the document does not have.
pub fn missing() -> Value {
    Value::other(TY_MISSING, "")
}

pub fn is_missing(v: &Value) -> bool {
    matches!(v, Value::Other(OtherValue { ty, .. }) if ty == TY_MISSING)
}

/// Significant digits a decimal literal may have and still read back exactly from a double.
const EXACT_DIGITS: usize = 15;

/// A JSON number as a `Value` (total).
pub fn number_value(n: &str) -> Value {
    if json::is_integer_text(n) {
        if let Ok(i) = n.parse::<i64>() {
            return Value::from_i64(i);
        }
        if let Ok(u) = n.parse::<u64>() {
            return Value::from_u64(u);
        }
        return Value::BigInt(n.to_string());
    }
    let digits = n
        .split(['e', 'E'])
        .next()
        .unwrap_or(n)
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    let significant = digits.trim_start_matches('0').len();
    match n.parse::<f64>() {
        Ok(f) if f.is_finite() && significant <= EXACT_DIGITS => Value::Float(f),
        _ => Value::Decimal(n.to_string()),
    }
}

/// A `_source` value of a field of type `es_type` (`None`: unmapped).
pub fn to_value(v: &Json, es_type: Option<&str>) -> Value {
    match v {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Num(n) => number_value(n),
        Json::Arr(_) | Json::Obj(_) => Value::Json(v.to_text()),
        Json::Str(s) => match es_type {
            Some("date" | "date_nanos") => Value::TimestampTz(s.clone()),
            Some("ip") => Value::other(TY_IP, s.clone()),
            Some("binary") => match base64::engine::general_purpose::STANDARD.decode(s) {
                Ok(bytes) => Value::from_bytes(&bytes),
                Err(_) => Value::Text(s.clone()),
            },
            _ => Value::Text(s.clone()),
        },
    }
}

/// The values at a dotted path of a `_source` object, and whether an array was crossed to get
/// there (the field then holds an array).
pub fn lookup<'a>(source: &'a Json, path: &str) -> (Vec<&'a Json>, bool) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut out = Vec::new();
    let mut crossed = false;
    collect(source, &parts, &mut out, &mut crossed, 0);
    (out, crossed)
}

fn collect<'a>(
    v: &'a Json,
    parts: &[&str],
    out: &mut Vec<&'a Json>,
    crossed: &mut bool,
    depth: usize,
) {
    if parts.is_empty() {
        out.push(v);
        return;
    }
    if depth > json::MAX_DEPTH {
        return;
    }
    match v {
        Json::Obj(pairs) => {
            for k in 1..=parts.len() {
                let key = parts[..k].join(".");
                for (name, val) in pairs {
                    if *name == key {
                        collect(val, &parts[k..], out, crossed, depth + 1);
                    }
                }
            }
        }
        Json::Arr(items) => {
            *crossed = true;
            for item in items {
                collect(item, parts, out, crossed, depth + 1);
            }
        }
        _ => {}
    }
}

/// The cell of `path` in a `_source`.
pub fn field_value(source: &Json, path: &str, es_type: Option<&str>) -> Value {
    let (found, crossed) = lookup(source, path);
    match (found.as_slice(), crossed) {
        ([], _) => missing(),
        ([one], false) => to_value(one, es_type),
        (many, _) => Value::Json(Json::Arr(many.iter().map(|v| (*v).clone()).collect()).to_text()),
    }
}

/// One search hit as a grid row for `columns` (metadata columns from the hit, the rest from its
/// `_source`). A hit without `_source` (disabled, or filtered out) gives `missing` cells.
pub fn hit_row(hit: &Json, columns: &[crate::types::schema::ColumnMeta]) -> Vec<Value> {
    let source = hit.get("_source");
    columns
        .iter()
        .map(|c| match c.name.as_str() {
            catalog::ID | catalog::INDEX | catalog::ROUTING => match hit.get(&c.name) {
                Some(Json::Str(s)) => Value::Text(s.clone()),
                Some(other) => to_value(other, None),
                None => Value::Null,
            },
            catalog::SEQ_NO | catalog::PRIMARY_TERM | catalog::SCORE => {
                hit.get(&c.name).map_or(Value::Null, |v| to_value(v, None))
            }
            path => match source {
                Some(src) => field_value(src, path, Some(c.data_type.as_str())),
                None => missing(),
            },
        })
        .collect()
}

/// A staged value as the JSON written for a field of type `es_type` (`None`: unmapped, or a
/// metadata field). Text staged for a number, boolean or JSON field is written as that type
/// when it reads as one; values that cannot be written (missing, DEFAULT, a cut preview) are
/// refused.
pub fn to_json(v: &Value, field: &str, es_type: Option<&str>) -> Result<Json, AppError> {
    let refuse = |why: &str| AppError::InvalidInput(format!("{field}: {why}"));
    Ok(match v {
        Value::Null => Json::Null,
        Value::Default => {
            return Err(refuse(
                "Elasticsearch has no column defaults; leave the field out instead",
            ));
        }
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(n) => Json::int(*n),
        Value::Float(f) => {
            if !f.is_finite() {
                return Err(refuse("JSON has no NaN or infinity"));
            }
            let mut text = f.to_string();
            if !text.contains(['.', 'e', 'E']) && is_float_type(es_type) {
                text.push_str(".0");
            }
            Json::Num(text)
        }
        Value::BigInt(s) | Value::Decimal(s) => {
            if json::is_number(s) {
                Json::Num(s.clone())
            } else {
                return Err(refuse(&format!("\u{201c}{s}\u{201d} is not a number")));
            }
        }
        Value::Text(s) => text_as(s, es_type),
        Value::Json(text) => json::parse(text)
            .map_err(|e| refuse(&format!("not valid JSON: {}", e.describe(text))))?,
        Value::Bytes(b) => {
            if b.truncated {
                return Err(refuse(
                    "only a preview of this value is loaded; load the whole value before editing",
                ));
            }
            Json::Str(b.base64.clone())
        }
        Value::Date(s)
        | Value::Time(s)
        | Value::Timestamp(s)
        | Value::TimestampTz(s)
        | Value::Interval(s)
        | Value::Uuid(s)
        | Value::Enum(s) => Json::Str(s.clone()),
        Value::Array(items) => Json::Arr(
            items
                .iter()
                .map(|i| to_json(i, field, es_type))
                .collect::<Result<_, _>>()?,
        ),
        Value::Other(o) if o.ty == TY_MISSING => {
            return Err(refuse("a missing field has no value to write"));
        }
        Value::Other(o) => text_as(&o.text, es_type),
    })
}

fn is_float_type(es_type: Option<&str>) -> bool {
    matches!(
        es_type,
        Some("double" | "float" | "half_float" | "scaled_float")
    )
}

/// Text staged in the grid, in the field's own JSON type when it reads as one.
fn text_as(s: &str, es_type: Option<&str>) -> Json {
    let t = s.trim();
    match es_type.map(catalog::kind_of) {
        Some(
            crate::types::schema::ValueKind::Int
            | crate::types::schema::ValueKind::BigInt
            | crate::types::schema::ValueKind::Float,
        ) if json::is_number(t) => Json::Num(t.to_string()),
        Some(crate::types::schema::ValueKind::Bool) if t == "true" || t == "false" => {
            Json::Bool(t == "true")
        }
        Some(crate::types::schema::ValueKind::Json | crate::types::schema::ValueKind::Geometry)
            if t.starts_with(['{', '[']) =>
        {
            json::parse(t).unwrap_or_else(|_| Json::Str(s.to_string()))
        }
        _ => Json::Str(s.to_string()),
    }
}

/// The field of a column, for writing.
pub fn es_type_of<'a>(fields: &'a [Field], column: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|f| f.path == column)
        .map(|f| f.es_type.as_str())
        .filter(|t| !t.contains('|'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::elasticsearch::catalog::{ColumnsFor, columns, flatten_mapping};
    use crate::drivers::elasticsearch::json::parse;

    #[test]
    fn numbers_keep_their_type_and_precision() {
        assert_eq!(number_value("42"), Value::Int(42));
        assert_eq!(
            number_value("9007199254740993"),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            number_value("18446744073709551615"),
            Value::BigInt("18446744073709551615".into())
        );
        assert_eq!(
            number_value("123456789012345678901234567890"),
            Value::BigInt("123456789012345678901234567890".into())
        );
        assert_eq!(number_value("1.5"), Value::Float(1.5));
        assert_eq!(number_value("-2.5e3"), Value::Float(-2500.0));
        assert_eq!(
            number_value("0.12345678901234567890"),
            Value::Decimal("0.12345678901234567890".into())
        );
        assert_eq!(number_value("1E+400"), Value::Decimal("1E+400".into()));
        assert_eq!(number_value("0.000001"), Value::Float(0.000001));
    }

    #[test]
    fn strings_by_mapping_type() {
        let s = Json::str("2024-05-01T10:00:00.123456789Z");
        assert_eq!(
            to_value(&s, Some("date_nanos")),
            Value::TimestampTz("2024-05-01T10:00:00.123456789Z".into())
        );
        assert_eq!(
            to_value(&Json::str("10.0.0.1"), Some("ip")),
            Value::other("ip", "10.0.0.1")
        );
        match to_value(&Json::str("aGVsbG8="), Some("binary")) {
            Value::Bytes(b) => assert_eq!(b.len, 5),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            to_value(&Json::str("not base64!"), Some("binary")),
            Value::Text("not base64!".into())
        );
        // A number in a keyword field stays a number; a string in a long field stays text.
        assert_eq!(
            to_value(&Json::Num("7".into()), Some("keyword")),
            Value::Int(7)
        );
        assert_eq!(
            to_value(&Json::str("7"), Some("long")),
            Value::Text("7".into())
        );
        // Epoch millis in a date field stay a number.
        assert_eq!(
            to_value(&Json::Num("1700000000000".into()), Some("date")),
            Value::Int(1_700_000_000_000)
        );
    }

    #[test]
    fn dotted_paths_find_nested_and_literal_keys() {
        let src = parse(
            r#"{"user":{"name":"ann","geo":{"loc":{"lat":1,"lon":2}}},"host.name":"h1","tags":[{"k":"a"},{"k":"b"}],"list":[1,2],"n":null}"#,
        )
        .unwrap();
        assert_eq!(
            field_value(&src, "user.name", Some("keyword")),
            Value::Text("ann".into())
        );
        assert_eq!(
            field_value(&src, "user.geo.loc", Some("geo_point")),
            Value::Json(r#"{"lat":1,"lon":2}"#.into())
        );
        assert_eq!(
            field_value(&src, "host.name", None),
            Value::Text("h1".into())
        );
        assert_eq!(
            field_value(&src, "tags.k", None),
            Value::Json(r#"["a","b"]"#.into())
        );
        assert_eq!(
            field_value(&src, "list", Some("long")),
            Value::Json("[1,2]".into())
        );
        assert_eq!(field_value(&src, "n", None), Value::Null);
        assert!(is_missing(&field_value(&src, "user.age", None)));
        assert!(is_missing(&field_value(&src, "nope", None)));
    }

    #[test]
    fn hits_become_rows() {
        let mapping = flatten_mapping(
            &parse(
                r#"{"properties":{"a":{"type":"long"},"b":{"properties":{"c":{"type":"date"}}}}}"#,
            )
            .unwrap(),
        );
        let cols = columns(
            &mapping,
            ColumnsFor {
                spans_indices: true,
                seq_no_guards: true,
            },
        );
        let hit = parse(
            r#"{"_index":"logs-1","_id":"x1","_seq_no":12,"_primary_term":3,"_score":null,"_source":{"a":9007199254740993,"b":{"c":"2024-01-01"}}}"#,
        )
        .unwrap();
        let row = hit_row(&hit, &cols);
        assert_eq!(
            row,
            vec![
                Value::Text("logs-1".into()),
                Value::Text("x1".into()),
                Value::Int(12),
                Value::Int(3),
                Value::BigInt("9007199254740993".into()),
                Value::TimestampTz("2024-01-01".into()),
            ]
        );
        let bare = parse(r#"{"_index":"i","_id":"y"}"#).unwrap();
        let row = hit_row(&bare, &cols);
        assert_eq!(row[2], Value::Null);
        assert!(is_missing(&row[4]));
    }

    #[test]
    fn staged_values_are_written_in_the_field_type() {
        assert_eq!(
            to_json(&Value::Text("42".into()), "f", Some("long")).unwrap(),
            Json::Num("42".into())
        );
        assert_eq!(
            to_json(&Value::Text("4x".into()), "f", Some("long")).unwrap(),
            Json::str("4x")
        );
        assert_eq!(
            to_json(&Value::Text("true".into()), "f", Some("boolean")).unwrap(),
            Json::Bool(true)
        );
        assert_eq!(
            to_json(&Value::Text("{\"a\":1}".into()), "f", Some("object"))
                .unwrap()
                .to_text(),
            "{\"a\":1}"
        );
        assert_eq!(
            to_json(&Value::Text("42".into()), "f", Some("keyword")).unwrap(),
            Json::str("42")
        );
        assert_eq!(
            to_json(&Value::Float(2.0), "f", Some("double")).unwrap(),
            Json::Num("2.0".into())
        );
        assert_eq!(
            to_json(&Value::Float(2.5), "f", None).unwrap(),
            Json::Num("2.5".into())
        );
        assert_eq!(
            to_json(
                &Value::BigInt("18446744073709551615".into()),
                "f",
                Some("unsigned_long")
            )
            .unwrap(),
            Json::Num("18446744073709551615".into())
        );
        assert_eq!(
            to_json(&Value::Json("[1, {\"b\": 2}]".into()), "f", None)
                .unwrap()
                .to_text(),
            "[1,{\"b\":2}]"
        );
        assert_eq!(
            to_json(&Value::other("ip", "::1"), "f", Some("ip")).unwrap(),
            Json::str("::1")
        );
        assert_eq!(
            to_json(&Value::TimestampTz("2024-01-01".into()), "f", Some("date")).unwrap(),
            Json::str("2024-01-01")
        );
        for bad in [
            Value::Default,
            missing(),
            Value::Json("{".into()),
            Value::BigInt("12x".into()),
        ] {
            assert!(to_json(&bad, "f", None).is_err(), "{bad:?}");
        }
        let cut = Value::from_bytes(&vec![0u8; crate::types::value::BYTES_PREVIEW_CAP + 1]);
        assert!(to_json(&cut, "f", Some("binary")).is_err());
    }
}
