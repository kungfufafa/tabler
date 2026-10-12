use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Largest integer JavaScript can represent exactly (2^53 - 1).
pub const JS_SAFE_INT: i64 = 9_007_199_254_740_991;

/// A cell value crossing the IPC boundary. Tagged as `{ "t": "...", "v": ... }` so the renderer can
/// render and edit every value losslessly: integers beyond 2^53 and decimals travel as strings,
/// temporal values keep the database's textual form, JSON stays raw text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "t", content = "v", rename_all = "camelCase")]
#[ts(export)]
pub enum Value {
    Null,
    Bool(bool),
    /// Only when |n| <= 2^53 - 1.
    Int(i64),
    /// Integers outside the JS safe range (i64/u64/i128), as a decimal string.
    BigInt(String),
    /// Finite floats only; NaN/Infinity become `Other`.
    Float(f64),
    /// numeric / decimal, exact textual form.
    Decimal(String),
    Text(String),
    Bytes(BytesValue),
    Date(String),
    Time(String),
    Timestamp(String),
    TimestampTz(String),
    Interval(String),
    Uuid(String),
    /// Raw JSON text (preserves key order and numeric precision).
    Json(String),
    Array(Vec<Value>),
    Enum(String),
    /// Anything the app does not model natively: inet, money, ranges, geometry, bit strings…
    Other(OtherValue),
    /// Write-only: emits the `DEFAULT` keyword in INSERT/UPDATE.
    Default,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BytesValue {
    pub base64: String,
    pub len: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OtherValue {
    pub ty: String,
    pub text: String,
}

/// Blob preview cap sent to the renderer; the full value is available through `fetch_cell`.
pub const BYTES_PREVIEW_CAP: usize = 64 * 1024;

impl Value {
    pub fn from_i64(n: i64) -> Value {
        if (-JS_SAFE_INT..=JS_SAFE_INT).contains(&n) {
            Value::Int(n)
        } else {
            Value::BigInt(n.to_string())
        }
    }

    pub fn from_u64(n: u64) -> Value {
        if n <= JS_SAFE_INT as u64 {
            Value::Int(n as i64)
        } else {
            Value::BigInt(n.to_string())
        }
    }

    pub fn from_f64(ty: &str, f: f64) -> Value {
        if f.is_finite() {
            Value::Float(f)
        } else {
            Value::Other(OtherValue {
                ty: ty.to_string(),
                text: f.to_string(),
            })
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Value {
        use base64::Engine as _;
        let truncated = bytes.len() > BYTES_PREVIEW_CAP;
        let slice = if truncated {
            &bytes[..BYTES_PREVIEW_CAP]
        } else {
            bytes
        };
        Value::Bytes(BytesValue {
            base64: base64::engine::general_purpose::STANDARD.encode(slice),
            len: bytes.len() as u64,
            truncated,
        })
    }

    /// A whole blob, never truncated (`fetch_cell`'s "load full value"; callers bound the size).
    pub fn from_bytes_uncapped(bytes: &[u8]) -> Value {
        use base64::Engine as _;
        Value::Bytes(BytesValue {
            base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            len: bytes.len() as u64,
            truncated: false,
        })
    }

    pub fn other(ty: &str, text: impl Into<String>) -> Value {
        Value::Other(OtherValue {
            ty: ty.to_string(),
            text: text.into(),
        })
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Textual form suitable for sending as a bind parameter or showing in a preview.
    pub fn as_text(&self) -> Option<String> {
        Some(match self {
            Value::Null | Value::Default => return None,
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => f.to_string(),
            Value::BigInt(s)
            | Value::Decimal(s)
            | Value::Text(s)
            | Value::Date(s)
            | Value::Time(s)
            | Value::Timestamp(s)
            | Value::TimestampTz(s)
            | Value::Interval(s)
            | Value::Uuid(s)
            | Value::Json(s)
            | Value::Enum(s) => s.clone(),
            Value::Bytes(b) => b.base64.clone(),
            Value::Array(items) => pg_array_literal(items),
            Value::Other(o) => o.text.clone(),
        })
    }
}

/// Decodes hex text (`encode(x, 'hex')`, `HEX(x)`, `hex(x)`; either case). `None` when malformed.
pub fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim().as_bytes();
    let (pairs, rest) = hex.as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    pairs
        .iter()
        .map(|[hi, lo]| Some((nibble(*hi)? << 4) | nibble(*lo)?))
        .collect()
}

/// Renders a one-dimensional array as a PostgreSQL array literal: `{"a","b",NULL}`.
pub fn pg_array_literal(items: &[Value]) -> String {
    let mut out = String::from("{");
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        match item {
            Value::Null | Value::Default => out.push_str("NULL"),
            Value::Array(inner) => out.push_str(&pg_array_literal(inner)),
            other => {
                let text = other.as_text().unwrap_or_default();
                out.push('"');
                for ch in text.chars() {
                    if ch == '"' || ch == '\\' {
                        out.push('\\');
                    }
                    out.push(ch);
                }
                out.push('"');
            }
        }
    }
    out.push('}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ints_outside_js_safe_range_become_strings() {
        assert_eq!(Value::from_i64(42), Value::Int(42));
        assert_eq!(Value::from_i64(JS_SAFE_INT), Value::Int(JS_SAFE_INT));
        assert_eq!(
            Value::from_i64(JS_SAFE_INT + 2),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            Value::from_i64(i64::MIN),
            Value::BigInt(i64::MIN.to_string())
        );
        assert_eq!(
            Value::from_u64(u64::MAX),
            Value::BigInt(u64::MAX.to_string())
        );
    }

    #[test]
    fn non_finite_floats_are_other() {
        assert_eq!(Value::from_f64("float8", 1.5), Value::Float(1.5));
        assert_eq!(
            Value::from_f64("float8", f64::NAN),
            Value::other("float8", "NaN")
        );
    }

    #[test]
    fn bytes_are_capped() {
        let big = vec![0u8; BYTES_PREVIEW_CAP + 10];
        match Value::from_bytes(&big) {
            Value::Bytes(b) => {
                assert!(b.truncated);
                assert_eq!(b.len, (BYTES_PREVIEW_CAP + 10) as u64);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn uncapped_bytes_keep_everything() {
        let big = vec![7u8; BYTES_PREVIEW_CAP + 10];
        match Value::from_bytes_uncapped(&big) {
            Value::Bytes(b) => {
                assert!(!b.truncated);
                assert_eq!(b.len, (BYTES_PREVIEW_CAP + 10) as u64);
                use base64::Engine as _;
                let back = base64::engine::general_purpose::STANDARD
                    .decode(b.base64)
                    .unwrap();
                assert_eq!(back, big);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn hex_decoding() {
        assert_eq!(decode_hex("00ff1A"), Some(vec![0, 255, 26]));
        assert_eq!(decode_hex(""), Some(vec![]));
        assert_eq!(decode_hex("abc"), None);
        assert_eq!(decode_hex("zz"), None);
    }

    #[test]
    fn array_literal_quotes_and_escapes() {
        let v = vec![
            Value::Text("a\"b".into()),
            Value::Null,
            Value::Int(3),
            Value::Text("x\\y".into()),
        ];
        assert_eq!(pg_array_literal(&v), "{\"a\\\"b\",NULL,\"3\",\"x\\\\y\"}");
    }

    #[test]
    fn serde_shape() {
        let json = serde_json::to_string(&Value::Null).unwrap();
        assert_eq!(json, r#"{"t":"null"}"#);
        let json = serde_json::to_string(&Value::BigInt("1".into())).unwrap();
        assert_eq!(json, r#"{"t":"bigInt","v":"1"}"#);
        let v: Value = serde_json::from_str(r#"{"t":"default"}"#).unwrap();
        assert_eq!(v, Value::Default);
    }
}
