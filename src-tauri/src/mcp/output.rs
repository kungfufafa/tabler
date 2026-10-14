//! Query results as plain JSON for MCP clients: each row an object keyed by column name, values
//! in their natural JSON form (exact numbers that JSON cannot hold, and dates, stay strings).
//! Rows are capped by count and by the total size of their JSON.

use serde_json::{Map, Value as Json, json};

use crate::types::query::ResultColumn;
use crate::types::value::Value;

/// Row cap of `run_query` (also its largest `limit`).
pub const MAX_ROWS: u32 = 500;
/// `run_query`'s `limit` when the client does not pass one.
pub const DEFAULT_ROWS: u32 = 100;
/// Cap on the JSON size of the rows one `run_query` returns.
pub const MAX_BYTES: usize = 1024 * 1024;
/// Longest text value returned; longer ones are cut (a note says so).
pub const MAX_TEXT_CHARS: usize = 64 * 1024;

/// A cell as JSON.
pub fn value_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(n) => json!(n),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(Json::Number)
            .unwrap_or_else(|| Json::String(f.to_string())),
        Value::BigInt(s)
        | Value::Decimal(s)
        | Value::Date(s)
        | Value::Time(s)
        | Value::Timestamp(s)
        | Value::TimestampTz(s)
        | Value::Interval(s)
        | Value::Uuid(s)
        | Value::Enum(s) => Json::String(s.clone()),
        Value::Text(s) => Json::String(cut(s)),
        // Raw JSON text stays JSON (numbers keep their precision only as far as f64 goes, so very
        // large ones come back as text instead).
        Value::Json(s) => match serde_json::from_str::<Json>(s) {
            Ok(parsed) if !has_lossy_number(&parsed) && s.len() <= MAX_TEXT_CHARS => parsed,
            _ => Json::String(cut(s)),
        },
        Value::Bytes(b) => {
            let mut text = format!("base64:{}", b.base64);
            if b.truncated {
                text.push_str(&format!(
                    " (first {} of {} bytes)",
                    b.base64.len() / 4 * 3,
                    b.len
                ));
            }
            Json::String(text)
        }
        Value::Array(items) => Json::Array(items.iter().map(value_json).collect()),
        Value::Other(o) => Json::String(cut(&o.text)),
        Value::Default => Json::String("DEFAULT".into()),
    }
}

fn has_lossy_number(v: &Json) -> bool {
    match v {
        Json::Number(n) => {
            n.is_f64()
                && n.as_f64()
                    .is_some_and(|f| f.abs() >= 9_007_199_254_740_992.0)
        }
        Json::Array(a) => a.iter().any(has_lossy_number),
        Json::Object(o) => o.values().any(has_lossy_number),
        _ => false,
    }
}

fn cut(s: &str) -> String {
    match s.char_indices().nth(MAX_TEXT_CHARS) {
        Some((byte, _)) => format!(
            "{}… (truncated, {} characters in all)",
            &s[..byte],
            s.chars().count()
        ),
        None => s.to_string(),
    }
}

/// Object keys for the columns: duplicate names (`SELECT a.id, b.id`) get ` (2)`, ` (3)`…
pub fn column_keys(columns: &[ResultColumn]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::with_capacity(columns.len());
    for c in columns {
        let mut key = c.name.clone();
        let mut n = 1;
        while keys.contains(&key) {
            n += 1;
            key = format!("{} ({n})", c.name);
        }
        keys.push(key);
    }
    keys
}

/// The columns as `{name, type}` objects.
pub fn columns_json(columns: &[ResultColumn]) -> Json {
    Json::Array(
        column_keys(columns)
            .into_iter()
            .zip(columns)
            .map(|(name, c)| json!({ "name": name, "type": c.data_type }))
            .collect(),
    )
}

/// What is left of a call's row and byte allowance.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub rows: usize,
    pub bytes: usize,
}

impl Budget {
    pub fn new(rows: u32) -> Budget {
        Budget {
            rows: rows.clamp(1, MAX_ROWS) as usize,
            bytes: MAX_BYTES,
        }
    }
}

/// Converts rows into objects while the budget lasts. Returns them and whether rows were left
/// out because the budget ran out.
pub fn rows_json(
    columns: &[ResultColumn],
    rows: &[Vec<Value>],
    budget: &mut Budget,
) -> (Vec<Json>, bool) {
    let keys = column_keys(columns);
    let mut out = Vec::with_capacity(rows.len().min(budget.rows));
    for row in rows {
        if budget.rows == 0 {
            return (out, true);
        }
        let mut obj = Map::with_capacity(keys.len());
        for (i, key) in keys.iter().enumerate() {
            obj.insert(
                key.clone(),
                row.get(i).map(value_json).unwrap_or(Json::Null),
            );
        }
        let obj = Json::Object(obj);
        let size = serde_json::to_string(&obj)
            .map(|s| s.len() + 1)
            .unwrap_or(0);
        if size > budget.bytes {
            budget.bytes = 0;
            budget.rows = 0;
            return (out, true);
        }
        budget.bytes -= size;
        budget.rows -= 1;
        out.push(obj);
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::schema::ValueKind;
    use crate::types::value::{BytesValue, OtherValue};

    fn col(name: &str) -> ResultColumn {
        ResultColumn {
            name: name.into(),
            data_type: "text".into(),
            kind: ValueKind::Text,
            nullable: None,
            table: None,
            origin: None,
        }
    }

    #[test]
    fn values_become_natural_json() {
        assert_eq!(value_json(&Value::Null), Json::Null);
        assert_eq!(value_json(&Value::Int(42)), json!(42));
        assert_eq!(value_json(&Value::Float(1.5)), json!(1.5));
        assert_eq!(
            value_json(&Value::BigInt("9007199254740993".into())),
            json!("9007199254740993")
        );
        assert_eq!(value_json(&Value::Decimal("1.10".into())), json!("1.10"));
        assert_eq!(
            value_json(&Value::Json(r#"{"a":[1,true]}"#.into())),
            json!({"a": [1, true]})
        );
        assert_eq!(value_json(&Value::Json("{bad".into())), json!("{bad"));
        assert_eq!(
            value_json(&Value::Json("123456789012345678901234567890".into())),
            json!("123456789012345678901234567890")
        );
        assert_eq!(
            value_json(&Value::Array(vec![Value::Int(1), Value::Text("x".into())])),
            json!([1, "x"])
        );
        assert_eq!(
            value_json(&Value::Bytes(BytesValue {
                base64: "AAE=".into(),
                len: 2,
                truncated: false
            })),
            json!("base64:AAE=")
        );
        assert_eq!(
            value_json(&Value::Other(OtherValue {
                ty: "inet".into(),
                text: "10.0.0.1".into()
            })),
            json!("10.0.0.1")
        );
        let long = "x".repeat(MAX_TEXT_CHARS + 5);
        let Json::String(s) = value_json(&Value::Text(long)) else {
            panic!()
        };
        assert!(s.contains("truncated"));
    }

    #[test]
    fn duplicate_column_names_get_numbered() {
        let cols = [col("id"), col("id"), col("name"), col("id")];
        assert_eq!(column_keys(&cols), ["id", "id (2)", "name", "id (3)"]);
    }

    #[test]
    fn rows_stop_at_the_row_and_byte_caps() {
        let cols = [col("a")];
        let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::Int(i)]).collect();
        let mut budget = Budget::new(4);
        let (out, cut) = rows_json(&cols, &rows, &mut budget);
        assert_eq!(out.len(), 4);
        assert!(cut);
        assert_eq!(out[0], json!({"a": 0}));
        // Budget is shared across result sets.
        let (more, cut) = rows_json(&cols, &rows, &mut budget);
        assert!(more.is_empty() && cut);

        let mut budget = Budget::new(MAX_ROWS);
        let big = vec![vec![Value::Text("y".repeat(60_000))]; 40];
        let (out, cut) = rows_json(&cols, &big, &mut budget);
        assert!(cut);
        let total: usize = out
            .iter()
            .map(|r| serde_json::to_string(r).unwrap().len() + 1)
            .sum();
        assert!(
            total <= MAX_BYTES && out.len() == 17,
            "{} rows, {total} bytes",
            out.len()
        );

        // `limit` is clamped to 1..=500.
        assert_eq!(Budget::new(0).rows, 1);
        assert_eq!(Budget::new(10_000).rows, MAX_ROWS as usize);
    }
}
