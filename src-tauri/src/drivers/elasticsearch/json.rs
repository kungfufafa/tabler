//! JSON as Elasticsearch sends and takes it. Numbers keep their text and objects keep their key
//! order, so a `_source` shows exactly what was indexed (an `unsigned_long`, a decimal with more
//! digits than a double holds, `1.50` stays `1.50`), and `serde_json` features of other crates
//! (`preserve_order`, `arbitrary_precision`) do not change what a cell holds.
//!
//! [`parse`] is strict JSON (server replies). [`parse_console`] also takes what the Kibana
//! console takes in a body: `#`, `//` and `/* */` comments between tokens and `"""…"""` strings
//! that keep their text verbatim. Both are total: malformed text is an error, never a panic, and
//! nesting is bounded.

use std::fmt::Write as _;

/// Nesting deeper than this is refused instead of overflowing the stack (Jackson, which
/// Elasticsearch parses with, stops at 1000; real documents stay far below).
pub const MAX_DEPTH: usize = 512;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Json {
    #[default]
    Null,
    Bool(bool),
    /// The number as written (`-1.5e3`, `18446744073709551615`).
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    /// In document order; a repeated key is kept as sent.
    Obj(Vec<(String, Json)>),
}

/// Where and why text is not JSON. `pos` is a byte offset into the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub message: String,
    pub pos: usize,
}

impl JsonError {
    /// 1-based line and column (in characters) of the error in `text`.
    pub fn line_col(&self, text: &str) -> (usize, usize) {
        let upto = text.get(..self.pos.min(text.len())).unwrap_or(text);
        let line = upto.matches('\n').count() + 1;
        let col = upto.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        (line, col)
    }

    /// The message with its line and column in `text`.
    pub fn describe(&self, text: &str) -> String {
        let (line, col) = self.line_col(text);
        format!("{} (line {line}, column {col})", self.message)
    }
}

impl Json {
    pub fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }

    pub fn int(n: i64) -> Json {
        Json::Num(n.to_string())
    }

    pub fn uint(n: u64) -> Json {
        Json::Num(n.to_string())
    }

    /// An object from pairs.
    pub fn obj<K: Into<String>>(pairs: impl IntoIterator<Item = (K, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// The value of `key` in an object (the first, if repeated).
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Follows object keys.
    pub fn at(&self, path: &[&str]) -> Option<&Json> {
        path.iter().try_fold(self, |v, k| v.get(k))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// An integer, also from a numeric string (the `_cat` APIs send numbers as strings).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) | Json::Str(n) => n.trim().parse().ok(),
            _ => None,
        }
    }

    /// A non-negative integer, also from a numeric string.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Num(n) | Json::Str(n) => n.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) | Json::Str(n) => n.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Obj(pairs) => Some(pairs),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    /// Text of a scalar (`"a"` → `a`, `1` → `1`, `true` → `true`); `None` for null, arrays and
    /// objects.
    pub fn scalar_text(&self) -> Option<String> {
        match self {
            Json::Str(s) | Json::Num(s) => Some(s.clone()),
            Json::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }

    /// Compact JSON text.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, None, 0);
        out
    }

    /// Indented JSON text (two spaces), for definitions and the console.
    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, Some(2), 0);
        out
    }

    fn write(&self, out: &mut String, indent: Option<usize>, level: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) if is_number(n) => out.push_str(n),
            // A number that is not one (built by hand from text) goes out as a string.
            Json::Num(n) | Json::Str(n) => write_str(out, n),
            Json::Arr(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, indent, level + 1);
                    item.write(out, indent, level + 1);
                }
                newline(out, indent, level);
                out.push(']');
            }
            Json::Obj(pairs) => {
                if pairs.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, indent, level + 1);
                    write_str(out, k);
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    v.write(out, indent, level + 1);
                }
                newline(out, indent, level);
                out.push('}');
            }
        }
    }
}

fn newline(out: &mut String, indent: Option<usize>, level: usize) {
    if let Some(n) = indent {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', n * level));
    }
}

/// A string as a JSON string literal.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    write_str(&mut out, s);
    out
}

fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Whether `s` is a JSON number literal.
pub fn is_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if b.first() == Some(&b'-') {
        i += 1;
    }
    match b.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => {
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        }
        _ => return false,
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == b.len()
}

/// Whether a number literal is an integer (no fraction or exponent).
pub fn is_integer_text(s: &str) -> bool {
    is_number(s) && !s.contains(['.', 'e', 'E'])
}

/// Strict JSON: one value, surrounded by whitespace only.
pub fn parse(text: &str) -> Result<Json, JsonError> {
    let mut p = Parser::new(text, false);
    let v = p.value(0)?;
    p.skip_space()?;
    if p.i < p.b.len() {
        return Err(p.err("unexpected text after the JSON value"));
    }
    Ok(v)
}

/// Strict JSON from bytes (a reply body); invalid UTF-8 is an error.
pub fn parse_bytes(body: &[u8]) -> Result<Json, JsonError> {
    let text = std::str::from_utf8(body).map_err(|e| JsonError {
        message: "the reply is not UTF-8 text".into(),
        pos: e.valid_up_to(),
    })?;
    parse(text.strip_prefix('\u{feff}').unwrap_or(text))
}

/// Every value of a console body, in order (one for most requests; one per line for `_bulk`
/// and `_msearch`), with comments and `"""…"""` strings allowed.
pub fn parse_console(text: &str) -> Result<Vec<Json>, JsonError> {
    let mut p = Parser::new(text, true);
    let mut out = Vec::new();
    loop {
        p.skip_space()?;
        if p.i >= p.b.len() {
            return Ok(out);
        }
        out.push(p.value(0)?);
    }
}

/// Values of newline-delimited JSON (`_bulk` replies, `_msearch` bodies), strictly.
pub fn parse_ndjson(text: &str) -> Result<Vec<Json>, JsonError> {
    let mut p = Parser::new(text, false);
    let mut out = Vec::new();
    loop {
        p.skip_space()?;
        if p.i >= p.b.len() {
            return Ok(out);
        }
        out.push(p.value(0)?);
    }
}

struct Parser<'a> {
    text: &'a str,
    b: &'a [u8],
    i: usize,
    console: bool,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str, console: bool) -> Parser<'a> {
        Parser {
            text,
            b: text.as_bytes(),
            i: 0,
            console,
        }
    }

    fn err(&self, message: &str) -> JsonError {
        JsonError {
            message: message.to_string(),
            pos: self.i,
        }
    }

    fn skip_space(&mut self) -> Result<(), JsonError> {
        loop {
            match self.b.get(self.i) {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.i += 1,
                Some(b'#') if self.console => self.skip_line(),
                Some(b'/') if self.console && self.b.get(self.i + 1) == Some(&b'/') => {
                    self.skip_line()
                }
                Some(b'/') if self.console && self.b.get(self.i + 1) == Some(&b'*') => {
                    let start = self.i;
                    match self.text[self.i + 2..].find("*/") {
                        Some(end) => self.i += 2 + end + 2,
                        None => {
                            self.i = start;
                            return Err(self.err("unterminated /* comment"));
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn skip_line(&mut self) {
        while let Some(&c) = self.b.get(self.i) {
            self.i += 1;
            if c == b'\n' {
                break;
            }
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.err("JSON nested too deeply"));
        }
        self.skip_space()?;
        match self.b.get(self.i) {
            None => Err(self.err("expected a JSON value")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.word("true", Json::Bool(true)),
            Some(b'f') => self.word("false", Json::Bool(false)),
            Some(b'n') => self.word("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("expected a JSON value")),
        }
    }

    fn word(&mut self, w: &str, v: Json) -> Result<Json, JsonError> {
        if self.b[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            Ok(v)
        } else {
            Err(self.err("expected a JSON value"))
        }
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.i;
        while matches!(
            self.b.get(self.i),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.i += 1;
        }
        let text = &self.text[start..self.i];
        if is_number(text) {
            Ok(Json::Num(text.to_string()))
        } else {
            self.i = start;
            Err(self.err("malformed number"))
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.i += 1;
        let mut pairs = Vec::new();
        self.skip_space()?;
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(Json::Obj(pairs));
        }
        loop {
            self.skip_space()?;
            if self.b.get(self.i) != Some(&b'"') {
                return Err(self.err("expected a quoted key"));
            }
            let key = self.string()?;
            self.skip_space()?;
            if self.b.get(self.i) != Some(&b':') {
                return Err(self.err("expected `:` after the key"));
            }
            self.i += 1;
            let v = self.value(depth + 1)?;
            pairs.push((key, v));
            self.skip_space()?;
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Json::Obj(pairs));
                }
                _ => return Err(self.err("expected `,` or `}`")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.i += 1;
        let mut items = Vec::new();
        self.skip_space()?;
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.skip_space()?;
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(self.err("expected `,` or `]`")),
            }
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        let start = self.i;
        if self.console && self.b[self.i..].starts_with(b"\"\"\"") {
            return match self.text[self.i + 3..].find("\"\"\"") {
                Some(end) => {
                    let s = self.text[self.i + 3..self.i + 3 + end].to_string();
                    self.i += 3 + end + 3;
                    Ok(s)
                }
                None => Err(self.err("unterminated \"\"\" string")),
            };
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            // Copy the run of plain characters at once (the text is UTF-8 already).
            let run = self.b[self.i..]
                .iter()
                .position(|&c| c == b'"' || c == b'\\')
                .map_or(self.b.len(), |n| self.i + n);
            out.push_str(&self.text[self.i..run]);
            self.i = run;
            match self.b.get(self.i) {
                None => {
                    self.i = start;
                    return Err(self.err("unterminated string"));
                }
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(_) => {
                    self.i += 1;
                    let esc = self.b.get(self.i).copied();
                    self.i += 1;
                    match esc {
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'/') => out.push('/'),
                        Some(b'b') => out.push('\u{8}'),
                        Some(b'f') => out.push('\u{c}'),
                        Some(b'n') => out.push('\n'),
                        Some(b'r') => out.push('\r'),
                        Some(b't') => out.push('\t'),
                        Some(b'u') => {
                            let hi = self.hex4()?;
                            let c = if (0xD800..0xDC00).contains(&hi)
                                && self.b[self.i..].starts_with(b"\\u")
                            {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00))
                                } else {
                                    self.i = save;
                                    None
                                }
                            } else {
                                char::from_u32(hi)
                            };
                            // A lone surrogate has no character: keep the text readable.
                            out.push(c.unwrap_or('\u{fffd}'));
                        }
                        _ => {
                            self.i -= 1;
                            return Err(self.err("invalid escape in a string"));
                        }
                    }
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let digits = self
            .text
            .get(self.i..self.i + 4)
            .filter(|d| d.bytes().all(|c| c.is_ascii_hexdigit()))
            .ok_or_else(|| self.err("invalid \\u escape"))?;
        let n = u32::from_str_radix(digits, 16).map_err(|_| self.err("invalid \\u escape"))?;
        self.i += 4;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_key_order_survive() {
        let text = r#"{"z":1.50,"a":18446744073709551615,"m":-0.0,"e":1E+400,"big":123456789012345678901234567890}"#;
        let v = parse(text).unwrap();
        assert_eq!(v.to_text(), text);
        assert_eq!(v.get("a").and_then(Json::as_u64), Some(u64::MAX));
        let keys: Vec<&str> = v
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(keys, ["z", "a", "m", "e", "big"]);
    }

    #[test]
    fn strings_unescape_and_requote() {
        let v = parse(r#""a\"b\\c\/d\n\u00e9\ud83d\ude00\u0001""#).unwrap();
        assert_eq!(v, Json::str("a\"b\\c/d\né😀\u{1}"));
        assert_eq!(v.to_text(), r#""a\"b\\c/d\né😀\u0001""#);
        // A lone surrogate is replaced, never a panic.
        assert_eq!(parse(r#""\ud800x""#).unwrap(), Json::str("\u{fffd}x"));
        assert_eq!(parse(r#""\ud800\u0041""#).unwrap(), Json::str("\u{fffd}A"));
    }

    #[test]
    fn malformed_text_is_an_error_with_a_position() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\" 1}",
            "01",
            "1.",
            "-",
            "1e",
            "tru",
            "\"x",
            "\"\\q\"",
            "{} x",
            "\"\\u12\"",
            "[1 2]",
            "{1:2}",
            "nul",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let e = parse("{\n  \"a\": x\n}").unwrap_err();
        assert_eq!(e.line_col("{\n  \"a\": x\n}"), (2, 8));
    }

    #[test]
    fn nesting_is_bounded() {
        let deep = "[".repeat(MAX_DEPTH + 10) + &"]".repeat(MAX_DEPTH + 10);
        assert!(parse(&deep).unwrap_err().message.contains("deeply"));
        let ok = "[".repeat(100) + &"]".repeat(100);
        assert!(parse(&ok).is_ok());
    }

    #[test]
    fn console_bodies_take_comments_and_triple_quotes() {
        let body = "# a comment\n{\n  // line\n  \"query\": \"\"\"SELECT \"x\" FROM t\n\"\"\", /* c */\n  \"n\": 1\n}\n{\"b\": 2}";
        let vals = parse_console(body).unwrap();
        assert_eq!(vals.len(), 2);
        assert_eq!(
            vals[0].get("query").and_then(Json::as_str),
            Some("SELECT \"x\" FROM t\n")
        );
        assert_eq!(vals[1].to_text(), r#"{"b":2}"#);
        // Strict parsing takes neither.
        assert!(parse("# x\n{}").is_err());
        assert!(parse(r#"{"a":"""x"""}"#).is_err());
        assert!(parse_console("{} /* open").is_err());
        assert!(parse_console(r#"{"a": """x"#).is_err());
        assert_eq!(parse_console("  \n# only a comment\n").unwrap(), vec![]);
    }

    #[test]
    fn ndjson_reads_one_value_per_line() {
        let v = parse_ndjson("{\"index\":{}}\n{\"a\":1}\n").unwrap();
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn pretty_text_is_indented() {
        let v = parse(r#"{"a":[1,{"b":null}],"c":{}}"#).unwrap();
        assert_eq!(
            v.to_pretty(),
            "{\n  \"a\": [\n    1,\n    {\n      \"b\": null\n    }\n  ],\n  \"c\": {}\n}"
        );
    }

    #[test]
    fn helpers_read_cat_style_numbers() {
        let v = parse(r#"{"docs.count":"12","size":null,"n":3}"#).unwrap();
        assert_eq!(v.get("docs.count").and_then(Json::as_i64), Some(12));
        assert_eq!(v.get("size").and_then(Json::as_i64), None);
        assert_eq!(v.get("n").and_then(Json::as_u64), Some(3));
        assert!(is_integer_text("-12") && !is_integer_text("1.0") && !is_integer_text("x"));
        // A Num that is not a number is written as a string, so the output stays JSON.
        assert_eq!(Json::Num("NaN".into()).to_text(), "\"NaN\"");
    }

    #[test]
    fn bytes_must_be_utf8() {
        assert!(parse_bytes(b"\xff\xfe").is_err());
        assert_eq!(
            parse_bytes("\u{feff}[]".as_bytes()).unwrap(),
            Json::Arr(vec![])
        );
    }
}
