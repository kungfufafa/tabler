//! Schemas and rows of the REST API as Tabler's columns and `Value`s.
//!
//! Rows arrive as `{"f": [{"v": …}, …]}` with every scalar a string; a RECORD cell is another
//! `{"f": […]}` and a REPEATED one `[{"v": …}, …]`. The schema says what each string is. The
//! decoders are total: a cell that does not fit its type becomes `Other` with its text, a cell
//! the row lacks `Other{ty: "missing"}`, and nothing panics on what the server sends.
//!
//! INT64 beyond 2^53 is `BigInt`, NUMERIC and BIGNUMERIC are `Decimal` as sent, FLOAT64 NaN and
//! infinities `Other`, BYTES are base64, TIMESTAMP is read from int64 microseconds (the client
//! asks for `useInt64Timestamp`) or float seconds and written like `CAST(ts AS STRING)`
//! (`2024-01-02 03:04:05.123456+00`), DATE, TIME, DATETIME and INTERVAL keep BigQuery's text,
//! GEOGRAPHY is WKT, JSON stays raw JSON text, RANGE is `Other`, and RECORD and REPEATED cells
//! are JSON text that keeps field order and exact numbers.

use base64::Engine as _;
use serde::{Deserialize, Deserializer};

use crate::sqlgen::dialect::spec::backticked_backslash;
use crate::types::query::ResultColumn;
use crate::types::schema::{ColumnMeta, ValueKind};
use crate::types::value::Value;

/// An int64 the API sends as a string (or a number); anything else is `None`.
pub fn int_of(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Serde helper for the API's int64 fields (strings in JSON); a malformed one is `None`.
pub fn lenient_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.as_ref().and_then(int_of))
}

/// Serde helper for unsigned int64 fields (row counts, byte counts).
pub fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.and_then(|v| match v {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TableSchema {
    pub fields: Vec<FieldSchema>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RangeElementType {
    #[serde(rename = "type")]
    pub ty: String,
}

/// A column of a table or a result (`TableFieldSchema`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FieldSchema {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    /// `NULLABLE` (also when absent), `REQUIRED` or `REPEATED`.
    pub mode: Option<String>,
    /// A RECORD's fields.
    pub fields: Vec<FieldSchema>,
    pub description: Option<String>,
    #[serde(deserialize_with = "lenient_i64")]
    pub max_length: Option<i64>,
    #[serde(deserialize_with = "lenient_i64")]
    pub precision: Option<i64>,
    #[serde(deserialize_with = "lenient_i64")]
    pub scale: Option<i64>,
    pub default_value_expression: Option<String>,
    pub collation: Option<String>,
    pub range_element_type: Option<RangeElementType>,
}

/// The types of the REST API, legacy names included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BqType {
    String,
    Bytes,
    Int64,
    Float64,
    Numeric,
    BigNumeric,
    Bool,
    Timestamp,
    Date,
    Time,
    Datetime,
    Geography,
    Json,
    Interval,
    Range,
    Struct,
    Unknown,
}

impl BqType {
    pub fn parse(ty: &str) -> BqType {
        match ty.trim().to_ascii_uppercase().as_str() {
            "STRING" => BqType::String,
            "BYTES" => BqType::Bytes,
            "INTEGER" | "INT64" => BqType::Int64,
            "FLOAT" | "FLOAT64" => BqType::Float64,
            "NUMERIC" | "DECIMAL" => BqType::Numeric,
            "BIGNUMERIC" | "BIGDECIMAL" => BqType::BigNumeric,
            "BOOLEAN" | "BOOL" => BqType::Bool,
            "TIMESTAMP" => BqType::Timestamp,
            "DATE" => BqType::Date,
            "TIME" => BqType::Time,
            "DATETIME" => BqType::Datetime,
            "GEOGRAPHY" => BqType::Geography,
            "JSON" => BqType::Json,
            "INTERVAL" => BqType::Interval,
            "RANGE" => BqType::Range,
            "RECORD" | "STRUCT" => BqType::Struct,
            _ => BqType::Unknown,
        }
    }

    /// GoogleSQL's name (`INT64`, not the legacy `INTEGER`).
    fn name(self) -> Option<&'static str> {
        Some(match self {
            BqType::String => "STRING",
            BqType::Bytes => "BYTES",
            BqType::Int64 => "INT64",
            BqType::Float64 => "FLOAT64",
            BqType::Numeric => "NUMERIC",
            BqType::BigNumeric => "BIGNUMERIC",
            BqType::Bool => "BOOL",
            BqType::Timestamp => "TIMESTAMP",
            BqType::Date => "DATE",
            BqType::Time => "TIME",
            BqType::Datetime => "DATETIME",
            BqType::Geography => "GEOGRAPHY",
            BqType::Json => "JSON",
            BqType::Interval => "INTERVAL",
            BqType::Range => "RANGE",
            BqType::Struct => "STRUCT",
            BqType::Unknown => return None,
        })
    }
}

impl FieldSchema {
    pub fn bq_type(&self) -> BqType {
        BqType::parse(&self.ty)
    }

    pub fn repeated(&self) -> bool {
        self.mode
            .as_deref()
            .is_some_and(|m| m.eq_ignore_ascii_case("REPEATED"))
    }

    pub fn required(&self) -> bool {
        self.mode
            .as_deref()
            .is_some_and(|m| m.eq_ignore_ascii_case("REQUIRED"))
    }

    /// NULL can be stored: neither REQUIRED nor an array (an array is empty, never NULL).
    pub fn nullable(&self) -> bool {
        !self.required() && !self.repeated()
    }

    /// The type of one value, as GoogleSQL spells it: `STRING(10)`, `NUMERIC(10, 2)`,
    /// `STRUCT<a INT64, b ARRAY<STRING>>`, `RANGE<DATE>`.
    pub fn element_type(&self) -> String {
        let t = self.bq_type();
        let base = t
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| self.ty.trim().to_ascii_uppercase());
        match t {
            BqType::String | BqType::Bytes => match self.max_length {
                Some(n) => format!("{base}({n})"),
                None => base,
            },
            BqType::Numeric | BqType::BigNumeric => match (self.precision, self.scale) {
                (Some(p), Some(s)) => format!("{base}({p}, {s})"),
                (Some(p), None) => format!("{base}({p})"),
                _ => base,
            },
            BqType::Range => match &self.range_element_type {
                Some(e) if !e.ty.is_empty() => format!("RANGE<{}>", e.ty.to_ascii_uppercase()),
                _ => base,
            },
            BqType::Struct => {
                let parts: Vec<String> = self
                    .fields
                    .iter()
                    .map(|f| format!("{} {}", field_name(&f.name), f.data_type()))
                    .collect();
                format!("STRUCT<{}>", parts.join(", "))
            }
            _ => base,
        }
    }

    /// The column's type: `ARRAY<…>` around the element type of a REPEATED field.
    pub fn data_type(&self) -> String {
        if self.repeated() {
            format!("ARRAY<{}>", self.element_type())
        } else {
            self.element_type()
        }
    }

    pub fn kind(&self) -> ValueKind {
        if self.repeated() {
            return ValueKind::Json;
        }
        match self.bq_type() {
            BqType::String => ValueKind::Text,
            BqType::Bytes => ValueKind::Bytes,
            BqType::Int64 => ValueKind::BigInt,
            BqType::Float64 => ValueKind::Float,
            BqType::Numeric | BqType::BigNumeric => ValueKind::Decimal,
            BqType::Bool => ValueKind::Bool,
            BqType::Timestamp => ValueKind::TimestampTz,
            BqType::Date => ValueKind::Date,
            BqType::Time => ValueKind::Time,
            BqType::Datetime => ValueKind::Timestamp,
            BqType::Geography => ValueKind::Geometry,
            BqType::Json | BqType::Struct => ValueKind::Json,
            BqType::Interval => ValueKind::Interval,
            BqType::Range | BqType::Unknown => ValueKind::Other,
        }
    }
}

/// A STRUCT field's name inside a type: plain when it is a plain name, backticked otherwise.
fn field_name(name: &str) -> String {
    let plain = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        name.to_string()
    } else {
        backticked_backslash(name)
    }
}

/// The table column of a schema field (1-based `ordinal`); keys are set by the caller.
pub fn column_meta(field: &FieldSchema, ordinal: u32) -> ColumnMeta {
    let t = field.bq_type();
    let sized = matches!(t, BqType::String | BqType::Bytes) && !field.repeated();
    let numeric = matches!(t, BqType::Numeric | BqType::BigNumeric) && !field.repeated();
    let narrow = |n: Option<i64>| n.and_then(|n| i32::try_from(n).ok());
    ColumnMeta {
        name: field.name.clone(),
        ordinal,
        data_type: field.data_type(),
        kind: field.kind(),
        nullable: field.nullable(),
        default_expr: field.default_value_expression.clone(),
        is_pk: false,
        pk_ordinal: None,
        is_auto_increment: false,
        is_generated: false,
        is_unsigned: false,
        comment: field.description.clone().filter(|d| !d.is_empty()),
        enum_values: None,
        char_max_len: if sized { field.max_length } else { None },
        numeric_precision: if numeric {
            narrow(field.precision)
        } else {
            None
        },
        numeric_scale: if numeric { narrow(field.scale) } else { None },
        collation: field.collation.clone().filter(|c| !c.is_empty()),
        array_elem_type: field.repeated().then(|| field.element_type()),
        extra: None,
        base_type: None,
    }
}

/// The result column of a schema field.
pub fn result_column(field: &FieldSchema) -> ResultColumn {
    ResultColumn {
        name: field.name.clone(),
        data_type: field.data_type(),
        kind: field.kind(),
        nullable: Some(field.nullable()),
        table: None,
        origin: None,
    }
}

/// The rows of a reply, one `Value` per field of `fields`.
pub fn decode_rows(fields: &[FieldSchema], rows: &[serde_json::Value]) -> Vec<Vec<Value>> {
    rows.iter().map(|r| decode_row(fields, r)).collect()
}

/// One `{"f": [...]}` row. Cells beyond the schema are ignored; missing ones are `missing`.
pub fn decode_row(fields: &[FieldSchema], row: &serde_json::Value) -> Vec<Value> {
    let cells = row.get("f").and_then(serde_json::Value::as_array);
    fields
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let cell = cells.and_then(|c| c.get(i));
            match cell.and_then(|c| c.get("v")) {
                Some(v) => decode_cell(field, v),
                None => Value::other("missing", ""),
            }
        })
        .collect()
}

/// The value of one cell (`v`) of `field`.
pub fn decode_cell(field: &FieldSchema, v: &serde_json::Value) -> Value {
    if v.is_null() {
        return Value::Null;
    }
    if field.repeated() || field.bq_type() == BqType::Struct {
        let mut out = String::new();
        write_json(field, v, true, &mut out);
        return Value::Json(out);
    }
    let text = match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => return Value::other(&field.data_type(), other.to_string()),
    };
    scalar(field, text)
}

fn scalar(field: &FieldSchema, s: String) -> Value {
    let t = field.bq_type();
    let other = |s: String| Value::other(&field.element_type(), s);
    match t {
        BqType::String => Value::Text(s),
        BqType::Bytes => match base64::engine::general_purpose::STANDARD.decode(s.trim()) {
            Ok(bytes) => Value::from_bytes(&bytes),
            Err(_) => other(s),
        },
        BqType::Int64 => match s.trim().parse::<i64>() {
            Ok(n) => Value::from_i64(n),
            Err(_) => other(s),
        },
        BqType::Float64 => match s.trim().parse::<f64>() {
            Ok(f) if f.is_finite() => Value::Float(f),
            // NaN, Infinity and -Infinity keep BigQuery's spelling.
            _ => other(s),
        },
        BqType::Numeric | BqType::BigNumeric if is_json_number(s.trim()) => {
            Value::Decimal(s.trim().to_string())
        }
        BqType::Bool if s.eq_ignore_ascii_case("true") => Value::Bool(true),
        BqType::Bool if s.eq_ignore_ascii_case("false") => Value::Bool(false),
        BqType::Timestamp => match timestamp_micros(&s).and_then(format_timestamp) {
            Some(text) => Value::TimestampTz(text),
            None => other(s),
        },
        BqType::Date => Value::Date(s),
        BqType::Time => Value::Time(s),
        BqType::Datetime => Value::Timestamp(s),
        BqType::Geography => Value::other("geometry", s),
        BqType::Json => Value::Json(s),
        BqType::Interval => Value::Interval(s),
        _ => other(s),
    }
}

/// `v` of `field` as JSON text. `whole`: `v` is the field's whole cell (an array of `{"v"}`
/// items when REPEATED), not one item of it.
fn write_json(field: &FieldSchema, v: &serde_json::Value, whole: bool, out: &mut String) {
    if v.is_null() {
        out.push_str("null");
        return;
    }
    if whole && field.repeated() {
        match v.as_array() {
            Some(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    let inner = item.get("v").unwrap_or(&serde_json::Value::Null);
                    write_json(field, inner, false, out);
                }
                out.push(']');
            }
            None => push_string(&text_of(v), out),
        }
        return;
    }
    if field.bq_type() == BqType::Struct {
        match v.get("f").and_then(serde_json::Value::as_array) {
            Some(cells) => {
                out.push('{');
                for (i, sub) in field.fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    push_string(&sub.name, out);
                    out.push(':');
                    let inner = cells.get(i).and_then(|c| c.get("v"));
                    write_json(sub, inner.unwrap_or(&serde_json::Value::Null), true, out);
                }
                out.push('}');
            }
            None => push_string(&text_of(v), out),
        }
        return;
    }
    let s = text_of(v);
    let t = s.trim();
    match field.bq_type() {
        BqType::Int64 if t.parse::<i64>().is_ok() => out.push_str(t),
        BqType::Float64 => match t.parse::<f64>().ok().and_then(serde_json::Number::from_f64) {
            Some(n) => out.push_str(&n.to_string()),
            None => push_string(&s, out),
        },
        BqType::Numeric | BqType::BigNumeric if is_json_number(t) => out.push_str(t),
        BqType::Bool if t.eq_ignore_ascii_case("true") => out.push_str("true"),
        BqType::Bool if t.eq_ignore_ascii_case("false") => out.push_str("false"),
        BqType::Json if serde_json::from_str::<serde_json::Value>(t).is_ok() => out.push_str(t),
        BqType::Timestamp => match timestamp_micros(&s).and_then(format_timestamp) {
            Some(text) => push_string(&text, out),
            None => push_string(&s, out),
        },
        _ => push_string(&s, out),
    }
}

/// A string cell as it is; anything else as its JSON text.
fn text_of(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn push_string(s: &str, out: &mut String) {
    out.push_str(&serde_json::Value::String(s.to_string()).to_string());
}

/// Whether `s` is a number in JSON's grammar (`-?(0|[1-9]d*)(.d+)?([eE][+-]?d+)?`), which is
/// how BigQuery writes NUMERIC and BIGNUMERIC.
pub fn is_json_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let start = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i - start
    };
    let start = i;
    let n = digits(&mut i);
    if n == 0 || (n > 1 && b[start] == b'0') {
        return false;
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        if digits(&mut i) == 0 {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return false;
        }
    }
    i == b.len()
}

/// Microseconds since the epoch from a TIMESTAMP cell: int64 microseconds (`useInt64Timestamp`)
/// or seconds as a decimal or scientific number (`1.700000000123456E9`, the API's default),
/// read exactly and rounded to the microsecond.
pub fn timestamp_micros(s: &str) -> Option<i64> {
    let s = s.trim();
    if !s.is_empty()
        && s.trim_start_matches('-')
            .bytes()
            .all(|c| c.is_ascii_digit())
    {
        return s.parse().ok();
    }
    let (negative, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(i) => (&body[..i], body[i + 1..].parse::<i32>().ok()?),
        None => (body, 0),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let all = format!("{int_part}{frac_part}");
    if all.is_empty() || all.len() > 36 || !all.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let digits: i128 = all.parse().ok()?;
    let frac_len = i32::try_from(frac_part.len()).ok()?;
    // micros = digits × 10^(exponent − frac_len + 6)
    let power = exponent.checked_sub(frac_len)?.checked_add(6)?;
    let micros = if power >= 0 {
        digits.checked_mul(10i128.checked_pow(u32::try_from(power).ok()?)?)?
    } else {
        let div = 10i128.checked_pow(u32::try_from(-power).ok()?)?;
        (digits + div / 2) / div
    };
    let micros = if negative { -micros } else { micros };
    i64::try_from(micros).ok()
}

/// A TIMESTAMP as `CAST(ts AS STRING)` writes it: UTC, `+00`, the fraction only when there is
/// one and without trailing zeros. `None` outside chrono's range.
pub fn format_timestamp(micros: i64) -> Option<String> {
    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp_micros(micros)?;
    let mut out = dt.format("%Y-%m-%d %H:%M:%S").to_string();
    let frac = micros.rem_euclid(1_000_000);
    if frac != 0 {
        let digits = format!("{frac:06}");
        out.push('.');
        out.push_str(digits.trim_end_matches('0'));
    }
    out.push_str("+00");
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::value::OtherValue;
    use serde_json::json;

    fn schema(v: serde_json::Value) -> Vec<FieldSchema> {
        serde_json::from_value::<TableSchema>(v).unwrap().fields
    }

    fn row(cells: serde_json::Value) -> serde_json::Value {
        json!({ "f": cells.as_array().unwrap().iter().map(|c| json!({"v": c})).collect::<Vec<_>>() })
    }

    fn other(ty: &str, text: &str) -> Value {
        Value::Other(OtherValue {
            ty: ty.into(),
            text: text.into(),
        })
    }

    #[test]
    fn scalars_of_every_type() {
        let fields = schema(json!({"fields": [
            {"name": "i", "type": "INTEGER"},
            {"name": "big", "type": "INT64"},
            {"name": "neg", "type": "INT64"},
            {"name": "f", "type": "FLOAT"},
            {"name": "nan", "type": "FLOAT64"},
            {"name": "inf", "type": "FLOAT64"},
            {"name": "n", "type": "NUMERIC"},
            {"name": "bn", "type": "BIGNUMERIC"},
            {"name": "b", "type": "BOOLEAN"},
            {"name": "s", "type": "STRING"},
            {"name": "by", "type": "BYTES"},
            {"name": "d", "type": "DATE"},
            {"name": "t", "type": "TIME"},
            {"name": "dt", "type": "DATETIME"},
            {"name": "ts", "type": "TIMESTAMP"},
            {"name": "g", "type": "GEOGRAPHY"},
            {"name": "j", "type": "JSON"},
            {"name": "iv", "type": "INTERVAL"},
            {"name": "r", "type": "RANGE", "rangeElementType": {"type": "DATE"}},
            {"name": "x", "type": "NEWTYPE"},
            {"name": "nul", "type": "STRING"}
        ]}));
        let r = row(json!([
            "42",
            "9007199254740993",
            "-9223372036854775808",
            "1.5",
            "NaN",
            "-Infinity",
            "123456789012345678901234567890.123456789",
            "-1.5E+40",
            "true",
            "héllo",
            "AP8=",
            "2024-02-29",
            "23:59:59.999999",
            "2024-01-02T03:04:05.123456",
            "1704164645123456",
            "POINT(106.8 -6.2)",
            "{\"a\":[1,2.50,{\"b\":null}]}",
            "1-2 3 4:5:6.789",
            "[2024-01-01, UNBOUNDED)",
            "zz",
            null
        ]));
        let v = decode_row(&fields, &r);
        assert_eq!(v[0], Value::Int(42));
        assert_eq!(v[1], Value::BigInt("9007199254740993".into()));
        assert_eq!(v[2], Value::BigInt("-9223372036854775808".into()));
        assert_eq!(v[3], Value::Float(1.5));
        assert_eq!(v[4], other("FLOAT64", "NaN"));
        assert_eq!(v[5], other("FLOAT64", "-Infinity"));
        assert_eq!(
            v[6],
            Value::Decimal("123456789012345678901234567890.123456789".into())
        );
        assert_eq!(v[7], Value::Decimal("-1.5E+40".into()));
        assert_eq!(v[8], Value::Bool(true));
        assert_eq!(v[9], Value::Text("héllo".into()));
        assert_eq!(v[10], Value::from_bytes(&[0x00, 0xff]));
        assert_eq!(v[11], Value::Date("2024-02-29".into()));
        assert_eq!(v[12], Value::Time("23:59:59.999999".into()));
        assert_eq!(v[13], Value::Timestamp("2024-01-02T03:04:05.123456".into()));
        assert_eq!(
            v[14],
            Value::TimestampTz("2024-01-02 03:04:05.123456+00".into())
        );
        assert_eq!(v[15], other("geometry", "POINT(106.8 -6.2)"));
        assert_eq!(v[16], Value::Json("{\"a\":[1,2.50,{\"b\":null}]}".into()));
        assert_eq!(v[17], Value::Interval("1-2 3 4:5:6.789".into()));
        assert_eq!(v[18], other("RANGE<DATE>", "[2024-01-01, UNBOUNDED)"));
        assert_eq!(v[19], other("NEWTYPE", "zz"));
        assert_eq!(v[20], Value::Null);
    }

    #[test]
    fn integers_at_the_js_safe_limit() {
        let f = schema(json!({"fields": [{"name": "i", "type": "INT64"}]}));
        let at = |s: &str| decode_row(&f, &row(json!([s])))[0].clone();
        assert_eq!(at("9007199254740991"), Value::Int(9_007_199_254_740_991));
        assert_eq!(
            at("9007199254740992"),
            Value::BigInt("9007199254740992".into())
        );
        assert_eq!(
            at("-9007199254740992"),
            Value::BigInt("-9007199254740992".into())
        );
        assert_eq!(
            at("9223372036854775807"),
            Value::BigInt("9223372036854775807".into())
        );
    }

    #[test]
    fn malformed_cells_never_panic() {
        let fields = schema(json!({"fields": [
            {"name": "i", "type": "INT64"},
            {"name": "f", "type": "FLOAT64"},
            {"name": "n", "type": "NUMERIC"},
            {"name": "b", "type": "BOOL"},
            {"name": "by", "type": "BYTES"},
            {"name": "ts", "type": "TIMESTAMP"},
            {"name": "rec", "type": "RECORD", "fields": [{"name": "a", "type": "INT64"}]},
            {"name": "arr", "type": "INT64", "mode": "REPEATED"},
            {"name": "s", "type": "STRING"},
            {"name": "gone", "type": "STRING"}
        ]}));
        let r = json!({"f": [
            {"v": "1e3"}, {"v": "fast"}, {"v": "01"}, {"v": "yes"}, {"v": "@@@"},
            {"v": "99999999999999999999999"}, {"v": "flat"}, {"v": {"x": 1}},
            {"v": ["not", "a", "string"]}
        ]});
        let v = decode_row(&fields, &r);
        assert_eq!(v[0], other("INT64", "1e3"));
        assert_eq!(v[1], other("FLOAT64", "fast"));
        assert_eq!(v[2], other("NUMERIC", "01"));
        assert_eq!(v[3], other("BOOL", "yes"));
        assert_eq!(v[4], other("BYTES", "@@@"));
        assert_eq!(v[5], other("TIMESTAMP", "99999999999999999999999"));
        assert_eq!(v[6], Value::Json("\"flat\"".into()));
        assert_eq!(v[7], Value::Json("\"{\\\"x\\\":1}\"".into()));
        assert_eq!(v[8], other("STRING", "[\"not\",\"a\",\"string\"]"));
        assert_eq!(v[9], other("missing", ""));
        // Rows that are no rows at all.
        for junk in [
            json!(null),
            json!(7),
            json!({"f": "x"}),
            json!({"f": [1, 2]}),
        ] {
            let v = decode_row(&fields, &junk);
            assert_eq!(v.len(), fields.len());
            assert!(v.iter().all(|c| matches!(c, Value::Other(_))), "{junk}");
        }
    }

    #[test]
    fn records_and_arrays_are_json_text_in_field_order() {
        let fields = schema(json!({"fields": [
            {"name": "tags", "type": "STRING", "mode": "REPEATED"},
            {"name": "addr", "type": "RECORD", "fields": [
                {"name": "zip", "type": "STRING"},
                {"name": "geo", "type": "RECORD", "fields": [
                    {"name": "lat", "type": "FLOAT64"},
                    {"name": "big", "type": "INT64"}
                ]},
                {"name": "at", "type": "TIMESTAMP"}
            ]},
            {"name": "items", "type": "RECORD", "mode": "REPEATED", "fields": [
                {"name": "sku", "type": "STRING"},
                {"name": "qty", "type": "INTEGER"},
                {"name": "price", "type": "NUMERIC"},
                {"name": "ok", "type": "BOOLEAN"},
                {"name": "doc", "type": "JSON"},
                {"name": "w", "type": "FLOAT64"},
                {"name": "raw", "type": "BYTES"}
            ]},
            {"name": "empty", "type": "INT64", "mode": "REPEATED"}
        ]}));
        let r = json!({"f": [
            {"v": [{"v": "a"}, {"v": "b\"c"}]},
            {"v": {"f": [{"v": "12345"}, {"v": {"f": [{"v": "-6.2"}, {"v": "9007199254740993"}]}}, {"v": "0"}]}},
            {"v": [{"v": {"f": [{"v": "x"}, {"v": "2"}, {"v": "1.50"}, {"v": "true"},
                                 {"v": "{\"k\": [1]}"}, {"v": "Infinity"}, {"v": "AP8="}]}},
                   {"v": {"f": [{"v": null}, {"v": null}, {"v": null}, {"v": null},
                                 {"v": null}, {"v": null}, {"v": null}]}}]},
            {"v": []}
        ]});
        let v = decode_row(&fields, &r);
        assert_eq!(v[0], Value::Json("[\"a\",\"b\\\"c\"]".into()));
        assert_eq!(
            v[1],
            Value::Json(
                "{\"zip\":\"12345\",\"geo\":{\"lat\":-6.2,\"big\":9007199254740993},\"at\":\"1970-01-01 00:00:00+00\"}"
                    .into()
            )
        );
        assert_eq!(
            v[2],
            Value::Json(
                "[{\"sku\":\"x\",\"qty\":2,\"price\":1.50,\"ok\":true,\"doc\":{\"k\": [1]},\"w\":\"Infinity\",\"raw\":\"AP8=\"},\
                 {\"sku\":null,\"qty\":null,\"price\":null,\"ok\":null,\"doc\":null,\"w\":null,\"raw\":null}]"
                    .into()
            )
        );
        assert_eq!(v[3], Value::Json("[]".into()));
        // Every one is valid JSON.
        for c in &v {
            if let Value::Json(t) = c {
                serde_json::from_str::<serde_json::Value>(t).expect(t);
            }
        }
    }

    #[test]
    fn columns_spell_googlesql_types() {
        let fields = schema(json!({"fields": [
            {"name": "id", "type": "INTEGER", "mode": "REQUIRED", "description": "key"},
            {"name": "code", "type": "STRING", "maxLength": "10", "collation": "und:ci",
             "defaultValueExpression": "'x'"},
            {"name": "amount", "type": "NUMERIC", "precision": "10", "scale": "2"},
            {"name": "tags", "type": "STRING", "mode": "REPEATED"},
            {"name": "rec", "type": "RECORD", "fields": [
                {"name": "a", "type": "INT64"},
                {"name": "b c", "type": "STRING", "mode": "REPEATED"}
            ]},
            {"name": "period", "type": "RANGE", "rangeElementType": {"type": "DATETIME"}},
            {"name": "ts", "type": "TIMESTAMP"},
            {"name": "g", "type": "GEOGRAPHY"}
        ]}));
        let cols: Vec<ColumnMeta> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| column_meta(f, i as u32 + 1))
            .collect();
        assert_eq!(cols[0].data_type, "INT64");
        assert_eq!(cols[0].kind, ValueKind::BigInt);
        assert!(!cols[0].nullable);
        assert_eq!(cols[0].comment.as_deref(), Some("key"));
        assert_eq!(cols[1].data_type, "STRING(10)");
        assert_eq!(cols[1].char_max_len, Some(10));
        assert_eq!(cols[1].collation.as_deref(), Some("und:ci"));
        assert_eq!(cols[1].default_expr.as_deref(), Some("'x'"));
        assert_eq!(cols[2].data_type, "NUMERIC(10, 2)");
        assert_eq!(
            (cols[2].numeric_precision, cols[2].numeric_scale),
            (Some(10), Some(2))
        );
        assert_eq!(cols[3].data_type, "ARRAY<STRING>");
        assert_eq!(cols[3].array_elem_type.as_deref(), Some("STRING"));
        assert_eq!(cols[3].kind, ValueKind::Json);
        assert!(!cols[3].nullable, "an array is empty, never NULL");
        assert_eq!(cols[4].data_type, "STRUCT<a INT64, `b c` ARRAY<STRING>>");
        assert_eq!(cols[4].kind, ValueKind::Json);
        assert_eq!(cols[5].data_type, "RANGE<DATETIME>");
        assert_eq!(cols[5].kind, ValueKind::Other);
        assert_eq!(cols[6].kind, ValueKind::TimestampTz);
        assert_eq!(cols[7].kind, ValueKind::Geometry);
        assert_eq!(cols[7].ordinal, 8);
        let rc = result_column(&fields[3]);
        assert_eq!(
            (rc.data_type.as_str(), rc.nullable),
            ("ARRAY<STRING>", Some(false))
        );
        // Junk sizes are no sizes.
        let junk = schema(json!({"fields": [{"name": "s", "type": "STRING", "maxLength": "ten"}]}));
        assert_eq!(junk[0].max_length, None);
    }

    #[test]
    fn timestamps_from_micros_and_float_seconds() {
        assert_eq!(
            timestamp_micros("1704164645123456"),
            Some(1_704_164_645_123_456)
        );
        assert_eq!(timestamp_micros("-1"), Some(-1));
        assert_eq!(
            timestamp_micros("1.704164645123456E9"),
            Some(1_704_164_645_123_456)
        );
        assert_eq!(
            timestamp_micros("1.7041646451234559E9"),
            Some(1_704_164_645_123_456)
        );
        assert_eq!(timestamp_micros("1.0E9"), Some(1_000_000_000_000_000));
        assert_eq!(
            timestamp_micros("1704164645.5"),
            Some(1_704_164_645_500_000)
        );
        assert_eq!(
            timestamp_micros("-6.21355968E10"),
            Some(-62_135_596_800_000_000)
        );
        for bad in [
            "",
            "-",
            "x",
            "1e",
            "1.2.3",
            "1E999",
            "9".repeat(40).as_str(),
        ] {
            assert_eq!(timestamp_micros(bad), None, "{bad}");
        }
        assert_eq!(
            format_timestamp(0).as_deref(),
            Some("1970-01-01 00:00:00+00")
        );
        assert_eq!(
            format_timestamp(1_704_164_645_120_000).as_deref(),
            Some("2024-01-02 03:04:05.12+00")
        );
        // The first and last instants BigQuery stores.
        assert_eq!(
            format_timestamp(-62_135_596_800_000_000).as_deref(),
            Some("0001-01-01 00:00:00+00")
        );
        assert_eq!(
            format_timestamp(253_402_300_799_999_999).as_deref(),
            Some("9999-12-31 23:59:59.999999+00")
        );
        assert_eq!(
            format_timestamp(-1).as_deref(),
            Some("1969-12-31 23:59:59.999999+00")
        );
        assert_eq!(format_timestamp(i64::MAX), None);
    }

    #[test]
    fn json_numbers() {
        for ok in [
            "0",
            "-1",
            "1.50",
            "1e40",
            "-1.5E+40",
            "123456789012345678901234567890.1",
        ] {
            assert!(is_json_number(ok), "{ok}");
        }
        for bad in ["", "-", "01", "1.", ".5", "1e", "+1", "NaN", "1 ", "0x1"] {
            assert!(!is_json_number(bad), "{bad}");
        }
    }
}
