//! Bound values on the wire. The dialect writes a typed placeholder per value (`{p1:Int64}`,
//! `sqlgen::dialect::clickhouse`), and the request carries the value as `param_p1=<text>`, which
//! the server parses as that type from its "escaped" text form (the form of TabSeparated): a
//! NULL is `\N`, a backslash, tab or line break in a string is escaped, and the elements of an
//! array, map or tuple are written as in the "quoted" form (`['a\'b', NULL]`, `{'k': 1}`,
//! `('x', 2)`). The value is never part of the SQL text.

use crate::error::AppError;
use crate::types::value::Value;

use super::json::{self, JsonVal};
use super::types::ChType;

/// The placeholders of `sql` with their type text, by number, in order of appearance
/// (`{p1:Nullable(String)}` → `(1, "Nullable(String)")`). Strings, quoted names and comments
/// are skipped; a placeholder used twice is listed once.
pub fn placeholder_types(sql: &str) -> Vec<(usize, String)> {
    let c: Vec<char> = sql.chars().collect();
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            q @ ('\'' | '"' | '`') => i = skip_quoted(&c, i, q),
            '-' if c.get(i + 1) == Some(&'-') => i = skip_line(&c, i),
            '#' => i = skip_line(&c, i),
            '/' if c.get(i + 1) == Some(&'/') => i = skip_line(&c, i),
            '/' if c.get(i + 1) == Some(&'*') => i = skip_block(&c, i),
            '{' => {
                if let Some((n, ty, next)) = placeholder_at(&c, i) {
                    if !out.iter().any(|(k, _)| *k == n) {
                        out.push((n, ty));
                    }
                    i = next;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

fn skip_quoted(c: &[char], start: usize, q: char) -> usize {
    let mut i = start + 1;
    while i < c.len() {
        if c[i] == '\\' {
            i += 2;
        } else if c[i] == q {
            if c.get(i + 1) == Some(&q) {
                i += 2;
            } else {
                return i + 1;
            }
        } else {
            i += 1;
        }
    }
    c.len()
}

fn skip_line(c: &[char], start: usize) -> usize {
    c[start..]
        .iter()
        .position(|&x| x == '\n')
        .map_or(c.len(), |p| start + p + 1)
}

fn skip_block(c: &[char], start: usize) -> usize {
    let mut depth = 0usize;
    let mut i = start;
    while i + 1 < c.len() {
        if c[i] == '/' && c[i + 1] == '*' {
            depth += 1;
            i += 2;
        } else if c[i] == '*' && c[i + 1] == '/' {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += 1;
        }
    }
    c.len()
}

/// `{pN:Type}` at `start`: N, the type text and the index after the `}`.
fn placeholder_at(c: &[char], start: usize) -> Option<(usize, String, usize)> {
    let mut i = start + 1;
    if c.get(i) != Some(&'p') {
        return None;
    }
    i += 1;
    let digits_start = i;
    while c.get(i).is_some_and(char::is_ascii_digit) {
        i += 1;
    }
    let n: usize = c[digits_start..i].iter().collect::<String>().parse().ok()?;
    if c.get(i) != Some(&':') {
        return None;
    }
    i += 1;
    let ty_start = i;
    let mut depth = 0usize;
    while i < c.len() {
        match c[i] {
            q @ ('\'' | '"' | '`') => {
                i = skip_quoted(c, i, q);
                continue;
            }
            '(' => depth += 1,
            ')' => depth = depth.checked_sub(1)?,
            '}' if depth == 0 => {
                let ty: String = c[ty_start..i].iter().collect();
                return Some((n, ty.trim().to_string(), i + 1));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The `param_pN` text of `value` for a placeholder of type `ty`.
pub fn param_text(value: &Value, ty: &ChType) -> Result<String, AppError> {
    if value.is_null() {
        return Ok("\\N".into());
    }
    let base = ty.base();
    match base {
        ChType::Array(_) | ChType::Map(..) | ChType::Tuple(_) => {
            let v = to_json(value)?;
            literal(&v, base, 0)
        }
        ChType::Json | ChType::Dynamic | ChType::Variant(_) => {
            let text = match value {
                Value::Json(t) => t.clone(),
                other => to_json(other)?.to_text(),
            };
            Ok(escape_top(&text))
        }
        ChType::Nested(_) | ChType::AggregateFunction(_) | ChType::Nothing => Err(unsupported(ty)),
        _ => Ok(escape_top(&scalar_text(value, ty)?)),
    }
}

fn unsupported(ty: &ChType) -> AppError {
    AppError::Unsupported(format!(
        "ClickHouse: values of type {} cannot be edited here",
        ty.label()
    ))
}

/// The text of a scalar value for a column of type `ty`.
fn scalar_text(value: &Value, ty: &ChType) -> Result<String, AppError> {
    Ok(match value {
        Value::Bool(b) => match ty.base() {
            // An integer column takes 0/1.
            ChType::Int { .. } => if *b { "1" } else { "0" }.to_string(),
            _ => if *b { "true" } else { "false" }.to_string(),
        },
        Value::Bytes(b) => {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&b.base64)
                .map_err(|_| AppError::InvalidInput("a binary value is not valid base64".into()))?;
            String::from_utf8(bytes).map_err(|_| {
                AppError::Unsupported(
                    "ClickHouse: binary data that is not UTF-8 cannot be sent as a parameter"
                        .into(),
                )
            })?
        }
        Value::Array(_) => return Err(unsupported(ty)),
        Value::Default => {
            return Err(AppError::InvalidInput(
                "DEFAULT is not a value of a parameter".into(),
            ));
        }
        other => other.as_text().unwrap_or_default(),
    })
}

/// A string in the escaped form: backslash, tab, line breaks and NUL written as escapes.
fn escape_top(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out
}

/// `'text'` in the quoted form.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// A value as JSON, the common shape of the grid's composite values (`Value::Json` text, or an
/// array of values).
fn to_json(v: &Value) -> Result<JsonVal, AppError> {
    Ok(match v {
        Value::Null => JsonVal::Null,
        Value::Bool(b) => JsonVal::Bool(*b),
        Value::Int(n) => JsonVal::Num(n.to_string()),
        Value::Float(f) => JsonVal::Num(f.to_string()),
        Value::BigInt(s) | Value::Decimal(s) => JsonVal::Num(s.clone()),
        Value::Json(text) => {
            json::parse(text).map_err(|e| AppError::InvalidInput(format!("not valid JSON: {e}")))?
        }
        Value::Array(items) => JsonVal::Arr(items.iter().map(to_json).collect::<Result<_, _>>()?),
        Value::Default => {
            return Err(AppError::InvalidInput(
                "DEFAULT is not a value of a parameter".into(),
            ));
        }
        other => JsonVal::Str(other.as_text().unwrap_or_default()),
    })
}

/// How deep a composite value may nest.
const MAX_DEPTH: usize = 64;

/// `v` in the quoted form of type `ty` (an element of a composite, or the composite itself).
fn literal(v: &JsonVal, ty: &ChType, depth: usize) -> Result<String, AppError> {
    if depth > MAX_DEPTH {
        return Err(AppError::InvalidInput("the value nests too deep".into()));
    }
    let mismatch = || {
        AppError::InvalidInput(format!(
            "{} is not a value of type {}",
            v.to_text(),
            ty.label()
        ))
    };
    if matches!(v, JsonVal::Null) {
        return Ok("NULL".into());
    }
    match ty.base() {
        ChType::Array(elem) => match v {
            JsonVal::Arr(items) => {
                let parts: Vec<String> = items
                    .iter()
                    .map(|i| literal(i, elem, depth + 1))
                    .collect::<Result<_, _>>()?;
                Ok(format!("[{}]", parts.join(", ")))
            }
            _ => Err(mismatch()),
        },
        ChType::Map(k, val) => match v {
            JsonVal::Obj(fields) => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(key, value)| {
                        let key = literal(&key_value(key, k), k, depth + 1)?;
                        Ok(format!("{key}: {}", literal(value, val, depth + 1)?))
                    })
                    .collect::<Result<_, AppError>>()?;
                Ok(format!("{{{}}}", parts.join(", ")))
            }
            // `[[k, v], …]`: the map as an array of pairs.
            JsonVal::Arr(pairs) => {
                let parts: Vec<String> = pairs
                    .iter()
                    .map(|pair| match pair {
                        JsonVal::Arr(kv) if kv.len() == 2 => Ok(format!(
                            "{}: {}",
                            literal(&kv[0], k, depth + 1)?,
                            literal(&kv[1], val, depth + 1)?
                        )),
                        _ => Err(mismatch()),
                    })
                    .collect::<Result<_, AppError>>()?;
                Ok(format!("{{{}}}", parts.join(", ")))
            }
            _ => Err(mismatch()),
        },
        ChType::Tuple(elems) => {
            let values: Vec<&JsonVal> = match v {
                JsonVal::Arr(items) if items.len() == elems.len() => items.iter().collect(),
                // A named tuple as an object: its fields by name.
                JsonVal::Obj(_) if elems.iter().all(|(n, _)| n.is_some()) => elems
                    .iter()
                    .map(|(n, _)| v.get(n.as_deref().unwrap_or_default()).ok_or_else(mismatch))
                    .collect::<Result<_, _>>()?,
                _ => return Err(mismatch()),
            };
            let parts: Vec<String> = values
                .iter()
                .zip(elems)
                .map(|(item, (_, t))| literal(item, t, depth + 1))
                .collect::<Result<_, _>>()?;
            // A one-element tuple needs its comma.
            let comma = if parts.len() == 1 { "," } else { "" };
            Ok(format!("({}{comma})", parts.join(", ")))
        }
        ChType::Json | ChType::Dynamic | ChType::Variant(_) => Ok(quote(&v.to_text())),
        ChType::Int { .. } | ChType::Float { .. } | ChType::Decimal { .. } => match v {
            JsonVal::Num(n) if json::is_json_number(n) => Ok(n.clone()),
            JsonVal::Num(n) | JsonVal::Str(n) => Ok(quote(n)),
            JsonVal::Bool(b) => Ok(if *b { "1" } else { "0" }.into()),
            _ => Err(mismatch()),
        },
        ChType::Bool => match v {
            JsonVal::Bool(b) => Ok(b.to_string()),
            JsonVal::Num(n) | JsonVal::Str(n) => Ok(quote(n)),
            _ => Err(mismatch()),
        },
        ChType::Nested(_) | ChType::AggregateFunction(_) | ChType::Nothing => Err(unsupported(ty)),
        _ => match v {
            JsonVal::Str(s) | JsonVal::Num(s) => Ok(quote(s)),
            JsonVal::Bool(b) => Ok(quote(&b.to_string())),
            _ => Err(mismatch()),
        },
    }
}

/// A JSON object key (always text) as a value of the map's key type.
fn key_value(key: &str, ty: &ChType) -> JsonVal {
    match ty.base() {
        ChType::Int { .. } | ChType::Float { .. } | ChType::Decimal { .. }
            if json::is_json_number(key) =>
        {
            JsonVal::Num(key.to_string())
        }
        _ => JsonVal::Str(key.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: Value, ty: &str) -> String {
        param_text(&v, &ChType::parse(ty)).unwrap()
    }

    #[test]
    fn placeholders_are_found_outside_strings_and_comments() {
        let sql = "ALTER TABLE `t{p9:X}` UPDATE `a` = {p1:Nullable(String)} WHERE `id` = \
                   {p2:DateTime64(3, 'Asia/Jakarta')} AND x = '{p3:String}' -- {p4:Int8}\n \
                   AND y = {p1:Nullable(String)} /* {p5:UInt8} /* */ */ AND z = {p6:Enum8('}' = 1)}";
        assert_eq!(
            placeholder_types(sql),
            [
                (1, "Nullable(String)".to_string()),
                (2, "DateTime64(3, 'Asia/Jakarta')".to_string()),
                (6, "Enum8('}' = 1)".to_string()),
            ]
        );
        assert!(
            placeholder_types("SELECT {x:String}, {p:String}, {p1 String}, {p1:Int8").is_empty()
        );
    }

    #[test]
    fn scalars_are_escaped_text() {
        assert_eq!(
            text(Value::Text("a\tb\\c\nd'e".into()), "String"),
            "a\\tb\\\\c\\nd'e"
        );
        assert_eq!(text(Value::Null, "Nullable(String)"), "\\N");
        // A string that reads `\N` stays text.
        assert_eq!(text(Value::Text("\\N".into()), "Nullable(String)"), "\\\\N");
        assert_eq!(text(Value::Int(-5), "Int32"), "-5");
        assert_eq!(
            text(Value::BigInt("18446744073709551615".into()), "UInt64"),
            "18446744073709551615"
        );
        assert_eq!(text(Value::Decimal("1.50".into()), "Decimal(9, 2)"), "1.50");
        assert_eq!(text(Value::Bool(true), "Bool"), "true");
        assert_eq!(text(Value::Bool(true), "UInt8"), "1");
        assert_eq!(
            text(
                Value::Timestamp("2024-01-02 03:04:05.678".into()),
                "DateTime64(3)"
            ),
            "2024-01-02 03:04:05.678"
        );
        assert_eq!(
            text(Value::Enum("it's".into()), "Enum8('it\\'s' = 1)"),
            "it's"
        );
        assert_eq!(text(Value::other("IPv4", "10.0.0.1"), "IPv4"), "10.0.0.1");
        assert_eq!(text(Value::Float(1.5), "Float64"), "1.5");
        assert_eq!(text(Value::from_bytes(b"hi"), "String"), "hi");
        assert!(param_text(&Value::from_bytes(&[0xff]), &ChType::String).is_err());
        assert!(param_text(&Value::Default, &ChType::String).is_err());
    }

    #[test]
    fn composites_use_the_quoted_form() {
        assert_eq!(
            text(
                Value::Array(vec![
                    Value::Text("a'b".into()),
                    Value::Null,
                    Value::Text("c\\".into())
                ]),
                "Array(Nullable(String))"
            ),
            "['a\\'b', NULL, 'c\\\\']"
        );
        assert_eq!(
            text(Value::Json("[[1, 2], []]".into()), "Array(Array(UInt8))"),
            "[[1, 2], []]"
        );
        assert_eq!(
            text(
                Value::Json("{\"b\": 2, \"a\": null}".into()),
                "Map(String, Nullable(UInt64))"
            ),
            "{'b': 2, 'a': NULL}"
        );
        assert_eq!(
            text(Value::Json("{\"1\": \"x\"}".into()), "Map(UInt8, String)"),
            "{1: 'x'}"
        );
        assert_eq!(
            text(Value::Json("[\"x\", 2]".into()), "Tuple(String, UInt8)"),
            "('x', 2)"
        );
        assert_eq!(
            text(
                Value::Json("{\"b\": \"y\", \"a\": 1}".into()),
                "Tuple(a UInt8, b String)"
            ),
            "(1, 'y')"
        );
        assert_eq!(text(Value::Json("[7]".into()), "Tuple(UInt8)"), "(7,)");
        assert_eq!(
            text(Value::Json("{\"a\": {\"b\": [1]}}".into()), "JSON"),
            "{\"a\": {\"b\": [1]}}"
        );
        for (v, ty) in [
            (Value::Json("{\"a\": 1}".into()), "Array(UInt8)"),
            (Value::Json("[1, 2]".into()), "Tuple(UInt8)"),
            (Value::Json("not json".into()), "Map(String, String)"),
            (Value::Json("[[1]]".into()), "Nested(a UInt8)"),
            (Value::Text("x".into()), "AggregateFunction(uniq, UInt64)"),
        ] {
            assert!(param_text(&v, &ChType::parse(ty)).is_err(), "{v:?} {ty}");
        }
    }
}
