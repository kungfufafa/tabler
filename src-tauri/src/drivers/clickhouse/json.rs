//! A small JSON reader for the rows ClickHouse sends: numbers keep their text, so a 256-bit
//! integer or a decimal that the server did not quote (a user whose profile forbids changing the
//! output settings) arrives exactly, and objects keep their key order. `serde_json` without
//! `arbitrary_precision` would turn such numbers into floats.
//!
//! Total: malformed text is an error, never a panic, and nesting is bounded.

/// Nesting deeper than this is refused instead of overflowing the stack.
const MAX_DEPTH: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub enum JsonVal {
    Null,
    Bool(bool),
    /// The number as written (`-1.5e3`, `340282366920938463463374607431768211455`), or one of
    /// the non-finite words ClickHouse may write bare (`nan`, `inf`, `-inf`).
    Num(String),
    Str(String),
    Arr(Vec<JsonVal>),
    Obj(Vec<(String, JsonVal)>),
}

impl JsonVal {
    /// Compact JSON text; numbers as received.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            JsonVal::Null => out.push_str("null"),
            JsonVal::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            // Non-finite words are not JSON: they go out as strings.
            JsonVal::Num(n) if is_json_number(n) => out.push_str(n),
            JsonVal::Num(n) => write_str(n, out),
            JsonVal::Str(s) => write_str(s, out),
            JsonVal::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            JsonVal::Obj(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    /// A field of an object.
    pub fn get(&self, key: &str) -> Option<&JsonVal> {
        match self {
            JsonVal::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The text of a string or a number.
    pub fn as_scalar_text(&self) -> Option<&str> {
        match self {
            JsonVal::Str(s) | JsonVal::Num(s) => Some(s),
            _ => None,
        }
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Whether `n` is a JSON number (not `nan`, `inf`…).
pub fn is_json_number(n: &str) -> bool {
    let b = n.as_bytes();
    let mut i = usize::from(b.first() == Some(&b'-'));
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > start
    };
    if !digits(&mut i) {
        return false;
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        if !digits(&mut i) {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == b.len()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub at: usize,
    pub what: &'static str,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.what, self.at)
    }
}

/// Parses one JSON value that fills `text` (surrounding whitespace allowed).
pub fn parse(text: &str) -> Result<JsonVal, JsonError> {
    let mut r = Reader {
        b: text.as_bytes(),
        text,
        at: 0,
    };
    let v = r.value(0)?;
    r.space();
    if r.at != r.b.len() {
        return Err(r.err("text after the value"));
    }
    Ok(v)
}

struct Reader<'a> {
    b: &'a [u8],
    text: &'a str,
    at: usize,
}

impl Reader<'_> {
    fn err(&self, what: &'static str) -> JsonError {
        JsonError { at: self.at, what }
    }

    fn space(&mut self) {
        while self.at < self.b.len() && matches!(self.b[self.at], b' ' | b'\t' | b'\n' | b'\r') {
            self.at += 1;
        }
    }

    fn eat(&mut self, word: &str) -> bool {
        if self.b[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<JsonVal, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        self.space();
        let Some(&c) = self.b.get(self.at) else {
            return Err(self.err("unexpected end"));
        };
        match c {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => Ok(JsonVal::Str(self.string()?)),
            b't' if self.eat("true") => Ok(JsonVal::Bool(true)),
            b'f' if self.eat("false") => Ok(JsonVal::Bool(false)),
            b'n' if self.eat("null") => Ok(JsonVal::Null),
            b'n' if self.eat("nan") => Ok(JsonVal::Num("nan".into())),
            b'i' if self.eat("inf") => Ok(JsonVal::Num("inf".into())),
            b'+' if self.eat("+inf") => Ok(JsonVal::Num("inf".into())),
            b'+' if self.eat("+nan") => Ok(JsonVal::Num("nan".into())),
            b'-' if self.eat("-inf") => Ok(JsonVal::Num("-inf".into())),
            b'-' if self.eat("-nan") => Ok(JsonVal::Num("nan".into())),
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(self.err("unexpected character")),
        }
    }

    fn number(&mut self) -> Result<JsonVal, JsonError> {
        let start = self.at;
        while self.at < self.b.len()
            && matches!(
                self.b[self.at],
                b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
            )
        {
            self.at += 1;
        }
        let n = &self.text[start..self.at];
        if is_json_number(n) {
            Ok(JsonVal::Num(n.to_string()))
        } else {
            Err(JsonError {
                at: start,
                what: "malformed number",
            })
        }
    }

    fn array(&mut self, depth: usize) -> Result<JsonVal, JsonError> {
        self.at += 1;
        let mut items = Vec::new();
        self.space();
        if self.b.get(self.at) == Some(&b']') {
            self.at += 1;
            return Ok(JsonVal::Arr(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.space();
            match self.b.get(self.at) {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(JsonVal::Arr(items));
                }
                _ => return Err(self.err("expected , or ]")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<JsonVal, JsonError> {
        self.at += 1;
        let mut fields = Vec::new();
        self.space();
        if self.b.get(self.at) == Some(&b'}') {
            self.at += 1;
            return Ok(JsonVal::Obj(fields));
        }
        loop {
            self.space();
            if self.b.get(self.at) != Some(&b'"') {
                return Err(self.err("expected a key"));
            }
            let key = self.string()?;
            self.space();
            if self.b.get(self.at) != Some(&b':') {
                return Err(self.err("expected :"));
            }
            self.at += 1;
            fields.push((key, self.value(depth + 1)?));
            self.space();
            match self.b.get(self.at) {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(JsonVal::Obj(fields));
                }
                _ => return Err(self.err("expected , or }")),
            }
        }
    }

    /// A string at `"`; escapes decoded, a lone surrogate replaced.
    fn string(&mut self) -> Result<String, JsonError> {
        self.at += 1;
        let mut out = String::new();
        loop {
            // Copy the run up to the next quote or backslash (always at a char boundary: both
            // are ASCII).
            let start = self.at;
            while self.at < self.b.len() && !matches!(self.b[self.at], b'"' | b'\\') {
                self.at += 1;
            }
            out.push_str(&self.text[start..self.at]);
            match self.b.get(self.at) {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(_) => {
                    self.at += 1;
                    let Some(&e) = self.b.get(self.at) else {
                        return Err(self.err("unterminated escape"));
                    };
                    self.at += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode()?),
                        _ => return Err(self.err("unknown escape")),
                    }
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let digits = self
            .text
            .get(self.at..self.at + 4)
            .ok_or_else(|| self.err("short \\u escape"))?;
        let n = u32::from_str_radix(digits, 16).map_err(|_| self.err("bad \\u escape"))?;
        self.at += 4;
        Ok(n)
    }

    fn unicode(&mut self) -> Result<char, JsonError> {
        let hi = self.hex4()?;
        if (0xD800..0xDC00).contains(&hi) && self.b[self.at..].starts_with(b"\\u") {
            let save = self.at;
            self.at += 2;
            let lo = self.hex4()?;
            if (0xDC00..0xE000).contains(&lo) {
                let c = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                return Ok(char::from_u32(c).unwrap_or('\u{FFFD}'));
            }
            self.at = save;
        }
        Ok(char::from_u32(hi).unwrap_or('\u{FFFD}'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(n: &str) -> JsonVal {
        JsonVal::Num(n.into())
    }

    #[test]
    fn numbers_keep_their_text() {
        let big = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        let v = parse(&format!("[{big}, -1.50, 1e-3, 0, \"2\"]")).unwrap();
        assert_eq!(
            v,
            JsonVal::Arr(vec![
                num(big),
                num("-1.50"),
                num("1e-3"),
                num("0"),
                JsonVal::Str("2".into())
            ])
        );
        assert_eq!(v.to_text(), format!("[{big},-1.50,1e-3,0,\"2\"]"));
        for bad in ["01x", "1.", ".5", "-", "1e", "--1", "1.2.3"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert_eq!(
            parse("[nan, inf, -inf]").unwrap().to_text(),
            r#"["nan","inf","-inf"]"#
        );
    }

    #[test]
    fn strings_objects_and_order() {
        let v = parse(r#"{"z": "a\"b\\c\n\u00e9\ud83d\ude00", "a": [true, false, null], "e": {}}"#)
            .unwrap();
        assert_eq!(
            v.get("z"),
            Some(&JsonVal::Str("a\"b\\c\n\u{e9}\u{1F600}".into()))
        );
        // Key order survives.
        assert_eq!(
            v.to_text(),
            r#"{"z":"a\"b\\c\né😀","a":[true,false,null],"e":{}}"#
        );
        // A lone surrogate becomes U+FFFD.
        assert_eq!(
            parse(r#""\ud800x""#).unwrap(),
            JsonVal::Str("\u{FFFD}x".into())
        );
        assert_eq!(parse(" [ ] ").unwrap(), JsonVal::Arr(vec![]));
    }

    #[test]
    fn malformed_text_is_an_error() {
        for bad in [
            "",
            "[",
            "[1,",
            "[1 2]",
            "{\"a\" 1}",
            "{1: 2}",
            "\"abc",
            "\"\\x\"",
            "\"\\u12\"",
            "[1] x",
            "tru",
            "{\"a\":1,}",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        assert_eq!(parse(&deep).unwrap_err().what, "nesting too deep");
    }
}
