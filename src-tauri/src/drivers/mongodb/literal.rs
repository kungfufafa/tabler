//! The values of the mongosh subset: JavaScript object literals and Extended JSON, read into BSON
//! by hand (no JavaScript engine).
//!
//! - JSON plus what mongosh users type: unquoted and `$` keys (dotted ones too), single-quoted
//!   and backtick strings with JavaScript escapes, trailing commas, `//` and `/* */` comments,
//!   hex and `123n` integers, `NaN` / `Infinity`, `undefined` (null, as mongosh stores it) and
//!   `/re/flags` literals.
//! - The shell helpers: `ObjectId()`, `ISODate()` / `new Date()`, `NumberLong` / `Long`,
//!   `NumberInt` / `Int32`, `Double`, `NumberDecimal` / `Decimal128`, `UUID()`, `BinData()`,
//!   `HexData()`, `MD5()`, `Timestamp()`, `MinKey` / `MaxKey`, `Code()`, `RegExp()`, `DBRef()`.
//! - Extended JSON v2, canonical and relaxed (`{"$oid": …}`, `{"$date": …}`,
//!   `{"$numberLong": …}`…), and its legacy `{"$binary": …, "$type": …}`. The legacy
//!   `{"$regex": …, "$options": …}` stays a document: it is also the query operator.
//!
//! Integers follow Extended JSON: Int32 when they fit, then Int64, then Double; a number with a
//! fraction or an exponent is a Double (`1.0` stays a double, unlike in JavaScript).

use ::mongodb::bson::spec::BinarySubtype;
use ::mongodb::bson::{
    Binary, Bson, DateTime, Document, JavaScriptCodeWithScope, Regex, Timestamp, oid::ObjectId,
};
use base64::Engine as _;

use crate::drivers::mongodb::values::{parse_date_millis, parse_decimal};
use crate::error::{AppError, ErrorPos};

/// Deeper nesting is refused (a server stores at most 100 levels).
pub const MAX_DEPTH: usize = 128;

/// A syntax error at a byte offset of the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub message: String,
}

impl ParseError {
    pub fn new(offset: usize, message: impl Into<String>) -> ParseError {
        ParseError {
            offset,
            message: message.into(),
        }
    }

    /// Where it is in `text`, as the editor counts (characters, 1-based line and column).
    pub fn position(&self, text: &str) -> ErrorPos {
        let end = self.offset.min(text.len());
        let chars = text.char_indices().take_while(|(i, _)| *i < end).count();
        ErrorPos::from_offset(text, chars)
    }

    pub fn into_app(self, text: &str) -> AppError {
        let pos = self.position(text);
        AppError::InvalidInput(format!(
            "{} (line {}, column {})",
            self.message, pos.line, pos.column
        ))
    }
}

type Result<T> = std::result::Result<T, ParseError>;

/// The whole text as one value.
pub fn parse_value(text: &str) -> Result<Bson> {
    let mut lx = Lexer::new(text);
    let v = lx.value()?;
    lx.end()?;
    Ok(v)
}

/// The whole text as one document.
pub fn parse_document(text: &str) -> Result<Document> {
    let mut lx = Lexer::new(text);
    let start = lx.peek_pos()?;
    match lx.value()? {
        Bson::Document(d) => {
            lx.end()?;
            Ok(d)
        }
        _ => Err(ParseError::new(start, "expected a document `{ … }`")),
    }
}

/// A cursor over a text: skips white space and comments, reads names, punctuation and values.
pub struct Lexer<'a> {
    pub src: &'a str,
    pub pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Lexer<'a> {
        Lexer { src, pos: 0 }
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    /// The next character, without skipping anything.
    pub fn peek_raw(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek_raw()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    /// Skips white space and comments.
    pub fn skip_trivia(&mut self) -> Result<()> {
        loop {
            let rest = self.rest();
            if let Some(c) = rest
                .chars()
                .next()
                .filter(|c| c.is_whitespace() || *c == '\u{feff}')
            {
                self.pos += c.len_utf8();
            } else if rest.starts_with("//") {
                let n = rest.find(['\n', '\r']).unwrap_or(rest.len());
                self.pos += n;
            } else if let Some(body) = rest.strip_prefix("/*") {
                match body.find("*/") {
                    Some(n) => self.pos += n + 4,
                    None => return Err(ParseError::new(self.pos, "unterminated comment")),
                }
            } else {
                return Ok(());
            }
        }
    }

    /// The next character after white space and comments.
    pub fn peek(&mut self) -> Result<Option<char>> {
        self.skip_trivia()?;
        Ok(self.peek_raw())
    }

    /// The offset of the next token.
    pub fn peek_pos(&mut self) -> Result<usize> {
        self.skip_trivia()?;
        Ok(self.pos)
    }

    /// Consumes `c` when it comes next.
    pub fn eat(&mut self, c: char) -> Result<bool> {
        if self.peek()? == Some(c) {
            self.pos += c.len_utf8();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn expect(&mut self, c: char) -> Result<()> {
        if self.eat(c)? {
            Ok(())
        } else {
            Err(self.unexpected(&format!("`{c}`")))
        }
    }

    /// An error naming what was found where `wanted` was expected.
    pub fn unexpected(&self, wanted: &str) -> ParseError {
        match self.peek_raw() {
            Some(c) => ParseError::new(self.pos, format!("expected {wanted}, found `{c}`")),
            None => ParseError::new(self.pos, format!("expected {wanted}, found the end")),
        }
    }

    /// Only white space, comments and `;` are left.
    pub fn end(&mut self) -> Result<()> {
        while self.eat(';')? {}
        match self.peek()? {
            None => Ok(()),
            Some(_) => Err(self.unexpected("the end of the statement")),
        }
    }

    pub fn at_end(&mut self) -> Result<bool> {
        Ok(self.peek()?.is_none())
    }

    /// A JavaScript name (`db`, `find`, `$set`), if one comes next.
    pub fn ident(&mut self) -> Result<Option<String>> {
        self.skip_trivia()?;
        let rest = self.rest();
        let mut chars = rest.char_indices();
        match chars.next() {
            Some((_, c)) if is_ident_start(c) => {}
            _ => return Ok(None),
        }
        let end = chars
            .find(|(_, c)| !is_ident_char(*c))
            .map_or(rest.len(), |(i, _)| i);
        self.pos += end;
        Ok(Some(rest[..end].to_string()))
    }

    /// The rest of the line (for `use x`, `show dbs`), trimmed.
    pub fn rest_of_text(&mut self) -> &'a str {
        let rest = self.rest();
        self.pos = self.src.len();
        rest.trim()
    }

    /// A quoted string (`'…'`, `"…"` or a backtick string without `${}`).
    pub fn string(&mut self) -> Result<String> {
        self.skip_trivia()?;
        let start = self.pos;
        let quote = match self.bump() {
            Some(q @ ('\'' | '"' | '`')) => q,
            _ => {
                self.pos = start;
                return Err(self.unexpected("a string"));
            }
        };
        let mut out = String::new();
        loop {
            let at = self.pos;
            let Some(c) = self.bump() else {
                return Err(ParseError::new(start, "unterminated string"));
            };
            match c {
                c if c == quote => return Ok(out),
                '\\' => self.escape(&mut out)?,
                '\n' | '\r' if quote != '`' => {
                    return Err(ParseError::new(start, "unterminated string"));
                }
                '$' if quote == '`' && self.peek_raw() == Some('{') => {
                    return Err(ParseError::new(
                        at,
                        "template expressions `${…}` are not supported",
                    ));
                }
                c => out.push(c),
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<()> {
        let at = self.pos - 1;
        let Some(c) = self.bump() else {
            return Err(ParseError::new(at, "unterminated string"));
        };
        match c {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'v' => out.push('\u{b}'),
            '0' if !self.peek_raw().is_some_and(|d| d.is_ascii_digit()) => out.push('\0'),
            'x' => {
                let n = self.hex_digits(2, at)?;
                out.push(char::from_u32(n).ok_or_else(|| ParseError::new(at, "invalid escape"))?);
            }
            'u' => {
                let cp = self.unicode_escape(at)?;
                if (0xD800..0xDC00).contains(&cp) {
                    // A high surrogate must be followed by a low one.
                    if self.rest().starts_with("\\u") {
                        self.pos += 1;
                        self.bump();
                        let lo = self.unicode_escape(at)?;
                        if (0xDC00..0xE000).contains(&lo) {
                            let c = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                            out.push(
                                char::from_u32(c)
                                    .ok_or_else(|| ParseError::new(at, "invalid escape"))?,
                            );
                            return Ok(());
                        }
                    }
                    return Err(ParseError::new(at, "unpaired surrogate in a \\u escape"));
                }
                out.push(
                    char::from_u32(cp).ok_or_else(|| ParseError::new(at, "invalid \\u escape"))?,
                );
            }
            // A line continuation.
            '\r' => {
                if self.peek_raw() == Some('\n') {
                    self.pos += 1;
                }
            }
            '\n' | '\u{2028}' | '\u{2029}' => {}
            c => out.push(c),
        }
        Ok(())
    }

    fn unicode_escape(&mut self, at: usize) -> Result<u32> {
        if self.peek_raw() == Some('{') {
            self.pos += 1;
            let rest = self.rest();
            let end = rest
                .find('}')
                .ok_or_else(|| ParseError::new(at, "invalid \\u{…} escape"))?;
            let digits = &rest[..end];
            if digits.is_empty() || digits.len() > 6 {
                return Err(ParseError::new(at, "invalid \\u{…} escape"));
            }
            let n = u32::from_str_radix(digits, 16)
                .map_err(|_| ParseError::new(at, "invalid \\u{…} escape"))?;
            self.pos += end + 1;
            return Ok(n);
        }
        self.hex_digits(4, at)
    }

    fn hex_digits(&mut self, n: usize, at: usize) -> Result<u32> {
        let rest = self.rest();
        let digits = rest
            .get(..n)
            .filter(|d| d.chars().all(|c| c.is_ascii_hexdigit()));
        let digits = digits.ok_or_else(|| ParseError::new(at, "invalid escape"))?;
        self.pos += n;
        u32::from_str_radix(digits, 16).map_err(|_| ParseError::new(at, "invalid escape"))
    }

    /// One value.
    pub fn value(&mut self) -> Result<Bson> {
        self.value_at(0)
    }

    fn value_at(&mut self, depth: usize) -> Result<Bson> {
        let start = self.peek_pos()?;
        if depth > MAX_DEPTH {
            return Err(ParseError::new(start, "the value is nested too deeply"));
        }
        match self.peek_raw() {
            Some('{') => self.object(depth),
            Some('[') => {
                self.pos += 1;
                let items = self.list(']', depth)?;
                Ok(Bson::Array(items))
            }
            Some('\'' | '"' | '`') => Ok(Bson::String(self.string()?)),
            Some('/') => self.regex(),
            Some(c) if c.is_ascii_digit() || c == '.' || c == '-' || c == '+' => self.number(),
            Some(c) if is_ident_start(c) => self.word(depth),
            _ => Err(self.unexpected("a value")),
        }
    }

    /// Values separated by commas up to `close` (already past the opening bracket); a trailing
    /// comma is allowed.
    pub fn list(&mut self, close: char, depth: usize) -> Result<Vec<Bson>> {
        let mut items = Vec::new();
        loop {
            if self.eat(close)? {
                return Ok(items);
            }
            items.push(self.value_at(depth + 1)?);
            if !self.eat(',')? {
                self.expect(close)?;
                return Ok(items);
            }
        }
    }

    /// The arguments of a call, after its `(`.
    pub fn args(&mut self) -> Result<Vec<Bson>> {
        self.list(')', 0)
    }

    fn object(&mut self, depth: usize) -> Result<Bson> {
        let start = self.pos;
        self.pos += 1;
        let mut doc = Document::new();
        loop {
            if self.eat('}')? {
                break;
            }
            let key = self.key()?;
            self.expect(':')?;
            let v = self.value_at(depth + 1)?;
            doc.insert(key, v);
            if !self.eat(',')? {
                self.expect('}')?;
                break;
            }
        }
        match extended(&doc) {
            Some(r) => r.map_err(|m| ParseError::new(start, m)),
            None => Ok(Bson::Document(doc)),
        }
    }

    fn key(&mut self) -> Result<String> {
        match self.peek()? {
            Some('\'' | '"' | '`') => self.string(),
            Some(c) if c.is_ascii_digit() => {
                let rest = self.rest();
                let end = rest
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '.')
                    .unwrap_or(rest.len());
                self.pos += end;
                Ok(rest[..end].to_string())
            }
            Some(c) if is_ident_start(c) => {
                // Dotted keys are taken unquoted too (`{a.b: 1}`), as Compass users type them.
                let rest = self.rest();
                let end = rest
                    .find(|c: char| !is_ident_char(c) && c != '.')
                    .unwrap_or(rest.len());
                self.pos += end;
                Ok(rest[..end].to_string())
            }
            _ => Err(self.unexpected("a field name")),
        }
    }

    fn regex(&mut self) -> Result<Bson> {
        let start = self.pos;
        self.pos += 1;
        let mut pattern = String::new();
        let mut class = false;
        loop {
            let Some(c) = self.bump() else {
                return Err(ParseError::new(start, "unterminated regular expression"));
            };
            match c {
                '\n' | '\r' => {
                    return Err(ParseError::new(start, "unterminated regular expression"));
                }
                '\\' => {
                    pattern.push('\\');
                    match self.bump() {
                        Some(n) if n != '\n' && n != '\r' => pattern.push(n),
                        _ => return Err(ParseError::new(start, "unterminated regular expression")),
                    }
                }
                '[' => {
                    class = true;
                    pattern.push(c);
                }
                ']' => {
                    class = false;
                    pattern.push(c);
                }
                '/' if !class => break,
                c => pattern.push(c),
            }
        }
        let flags_at = self.pos;
        let rest = self.rest();
        let end = rest.find(|c: char| !is_ident_char(c)).unwrap_or(rest.len());
        let flags = &rest[..end];
        self.pos += end;
        regex(pattern, flags).map_err(|m| ParseError::new(flags_at, m))
    }

    fn number(&mut self) -> Result<Bson> {
        let start = self.pos;
        let rest = self.rest();
        let (negative, body) = match rest.as_bytes().first() {
            Some(b'-') => (true, &rest[1..]),
            Some(b'+') => (false, &rest[1..]),
            _ => (false, rest),
        };
        let sign_len = rest.len() - body.len();
        for (word, f) in [("Infinity", f64::INFINITY), ("NaN", f64::NAN)] {
            if body.starts_with(word) && !body[word.len()..].starts_with(is_ident_char) {
                self.pos += sign_len + word.len();
                return Ok(Bson::Double(if negative { -f } else { f }));
            }
        }
        let end = body
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_'))
            .unwrap_or(body.len());
        // An exponent sign belongs to the number.
        let mut end = end;
        while body[..end].ends_with(['e', 'E'])
            && body[end..].starts_with(['+', '-'])
            && !body[..end].starts_with("0x")
            && !body[..end].starts_with("0X")
        {
            let more = body[end + 1..]
                .find(|c: char| !c.is_ascii_alphanumeric())
                .unwrap_or(body.len() - end - 1);
            end += 1 + more;
        }
        let token = &body[..end];
        self.pos += sign_len + end;
        number(token, negative).map_err(|m| ParseError::new(start, m))
    }

    fn word(&mut self, depth: usize) -> Result<Bson> {
        let start = self.pos;
        let Some(mut name) = self.ident()? else {
            return Err(self.unexpected("a value"));
        };
        let mut new = false;
        if name == "new" {
            new = true;
            name = self
                .ident()?
                .ok_or_else(|| self.unexpected("a constructor name after `new`"))?;
        }
        if self.eat('(')? {
            let args = self.list(')', depth)?;
            return helper(&name, new, args).map_err(|m| ParseError::new(start, m));
        }
        if new {
            return Err(ParseError::new(
                start,
                format!("expected `(` after `new {name}`"),
            ));
        }
        match name.as_str() {
            "true" => Ok(Bson::Boolean(true)),
            "false" => Ok(Bson::Boolean(false)),
            "null" | "undefined" => Ok(Bson::Null),
            "NaN" => Ok(Bson::Double(f64::NAN)),
            "Infinity" => Ok(Bson::Double(f64::INFINITY)),
            "MinKey" => Ok(Bson::MinKey),
            "MaxKey" => Ok(Bson::MaxKey),
            _ => Err(ParseError::new(
                start,
                format!(
                    "unknown name `{name}`: the console reads values only (no JavaScript variables)"
                ),
            )),
        }
    }
}

pub fn is_ident_start(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphabetic()
}

pub fn is_ident_char(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphanumeric()
}

fn number(token: &str, negative: bool) -> std::result::Result<Bson, String> {
    let bad = || {
        format!(
            "invalid number `{}{token}`",
            if negative { "-" } else { "" }
        )
    };
    if token.is_empty() || token.contains('_') {
        return Err(bad());
    }
    let int = |digits: &str, radix: u32| -> std::result::Result<Option<i64>, String> {
        let n = u64::from_str_radix(digits, radix).map_err(|_| bad());
        match n {
            Ok(n) if negative && n <= i64::MAX as u64 + 1 => {
                Ok(Some(0i64.wrapping_sub_unsigned(n)))
            }
            Ok(n) if !negative && n <= i64::MAX as u64 => Ok(Some(n as i64)),
            // Too large for 64 bits: a Double, as Extended JSON reads it.
            Ok(_) | Err(_) if digits.chars().all(|c| c.is_digit(radix)) && !digits.is_empty() => {
                Ok(None)
            }
            _ => Err(bad()),
        }
    };
    let width = |n: i64| match i32::try_from(n) {
        Ok(i) => Bson::Int32(i),
        Err(_) => Bson::Int64(n),
    };
    if let Some(hex) = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
    {
        let (hex, big) = match hex.strip_suffix('n') {
            Some(h) => (h, true),
            None => (hex, false),
        };
        return match int(hex, 16)? {
            Some(n) if big => Ok(Bson::Int64(n)),
            Some(n) => Ok(width(n)),
            None => Err(format!("{} does not fit 64 bits", bad())),
        };
    }
    if let Some(digits) = token.strip_suffix('n') {
        if !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(bad());
        }
        return match int(digits, 10)? {
            Some(n) => Ok(Bson::Int64(n)),
            None => Err(format!("{} does not fit 64 bits", bad())),
        };
    }
    let float_like = token.contains(['.', 'e', 'E']);
    if !float_like {
        if !token.chars().all(|c| c.is_ascii_digit()) {
            return Err(bad());
        }
        if let Some(n) = int(token, 10)? {
            return Ok(width(n));
        }
    }
    let valid = token
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
        && token.chars().any(|c| c.is_ascii_digit());
    let f: f64 = if valid {
        token.parse().map_err(|_| bad())?
    } else {
        return Err(bad());
    };
    Ok(Bson::Double(if negative { -f } else { f }))
}

/// A regular expression with JavaScript flags as BSON options (sorted, as BSON wants them).
fn regex(pattern: String, flags: &str) -> std::result::Result<Bson, String> {
    let mut options: Vec<char> = Vec::new();
    for c in flags.chars() {
        if !matches!(c, 'i' | 'm' | 's' | 'u' | 'x' | 'l') {
            return Err(format!("unsupported regular expression flag `{c}`"));
        }
        if !options.contains(&c) {
            options.push(c);
        }
    }
    options.sort_unstable();
    Ok(Bson::RegularExpression(Regex {
        pattern,
        options: options.into_iter().collect(),
    }))
}

/// The value of an Extended JSON wrapper document (`{"$oid": …}`…); `None` when `doc` is not
/// one, an error when it is one with a wrong value.
fn extended(doc: &Document) -> Option<std::result::Result<Bson, String>> {
    let mut keys = doc.keys();
    let first = keys.next()?.as_str();
    let second = keys.next().map(String::as_str);
    if keys.next().is_some() {
        return None;
    }
    let v = doc.get(first)?;
    let text = |what: &str| match v {
        Bson::String(s) => Ok(s.clone()),
        _ => Err(format!("{what} takes a string")),
    };
    let r = match (first, second) {
        ("$oid", None) => text("$oid").and_then(|s| {
            ObjectId::parse_str(&s)
                .map(Bson::ObjectId)
                .map_err(|_| format!("invalid ObjectId `{s}`"))
        }),
        ("$symbol", None) => text("$symbol").map(Bson::Symbol),
        ("$numberInt", None) => text("$numberInt").and_then(|s| {
            s.parse::<i32>()
                .map(Bson::Int32)
                .map_err(|_| format!("invalid $numberInt `{s}`"))
        }),
        ("$numberLong", None) => text("$numberLong").and_then(|s| {
            s.parse::<i64>()
                .map(Bson::Int64)
                .map_err(|_| format!("invalid $numberLong `{s}`"))
        }),
        ("$numberDouble", None) => text("$numberDouble").and_then(|s| {
            match s.as_str() {
                "NaN" | "-NaN" => Ok(f64::NAN),
                "Infinity" => Ok(f64::INFINITY),
                "-Infinity" => Ok(f64::NEG_INFINITY),
                t => t
                    .parse::<f64>()
                    .map_err(|_| format!("invalid $numberDouble `{s}`")),
            }
            .map(Bson::Double)
        }),
        ("$numberDecimal", None) => text("$numberDecimal").and_then(|s| decimal(&s)),
        ("$binary", None) => match v {
            Bson::Document(b) => binary_v2(b),
            _ => Err("$binary takes {base64, subType}".into()),
        },
        ("$binary", Some("$type")) | ("$type", Some("$binary")) => {
            match (doc.get("$binary"), doc.get("$type")) {
                (Some(Bson::String(b64)), Some(Bson::String(t))) => binary(b64, t),
                _ => return None,
            }
        }
        ("$uuid", None) => text("$uuid").and_then(|s| uuid(&s)),
        ("$code", None) => text("$code").map(Bson::JavaScriptCode),
        ("$code", Some("$scope")) | ("$scope", Some("$code")) => {
            match (doc.get("$code"), doc.get("$scope")) {
                (Some(Bson::String(code)), Some(Bson::Document(scope))) => {
                    Ok(Bson::JavaScriptCodeWithScope(JavaScriptCodeWithScope {
                        code: code.clone(),
                        scope: scope.clone(),
                    }))
                }
                _ => Err("$code with $scope takes a string and a document".into()),
            }
        }
        ("$timestamp", None) => match v {
            Bson::Document(t) => match (t.get("t").and_then(u32_of), t.get("i").and_then(u32_of)) {
                (Some(time), Some(increment)) if t.len() == 2 => {
                    Ok(Bson::Timestamp(Timestamp { time, increment }))
                }
                _ => Err("$timestamp takes {t, i}".into()),
            },
            _ => Err("$timestamp takes {t, i}".into()),
        },
        ("$regularExpression", None) => match v {
            Bson::Document(r) => match (r.get("pattern"), r.get("options")) {
                (Some(Bson::String(p)), Some(Bson::String(o))) if r.len() == 2 => {
                    regex(p.clone(), o)
                }
                _ => Err("$regularExpression takes {pattern, options}".into()),
            },
            _ => Err("$regularExpression takes {pattern, options}".into()),
        },
        ("$date", None) => match v {
            Bson::String(s) => parse_date_millis(s)
                .map(|ms| Bson::DateTime(DateTime::from_millis(ms)))
                .ok_or_else(|| format!("invalid $date `{s}`")),
            // `{"$numberLong": …}` inside was read already.
            Bson::Int64(ms) => Ok(Bson::DateTime(DateTime::from_millis(*ms))),
            Bson::Int32(ms) => Ok(Bson::DateTime(DateTime::from_millis(i64::from(*ms)))),
            Bson::Double(f) if f.fract() == 0.0 && f.abs() < 9.2e18 => {
                Ok(Bson::DateTime(DateTime::from_millis(*f as i64)))
            }
            _ => Err("$date takes an ISO-8601 string or {\"$numberLong\": …}".into()),
        },
        ("$minKey", None) => Ok(Bson::MinKey),
        ("$maxKey", None) => Ok(Bson::MaxKey),
        ("$undefined", None) => Ok(Bson::Undefined),
        ("$dbPointer", None) => db_pointer(v),
        _ => return None,
    };
    Some(r)
}

fn u32_of(b: &Bson) -> Option<u32> {
    match b {
        Bson::Int32(n) => u32::try_from(*n).ok(),
        Bson::Int64(n) => u32::try_from(*n).ok(),
        Bson::Double(f) if f.fract() == 0.0 && *f >= 0.0 && *f <= f64::from(u32::MAX) => {
            Some(*f as u32)
        }
        _ => None,
    }
}

fn decimal(s: &str) -> std::result::Result<Bson, String> {
    parse_decimal(s)
        .map(Bson::Decimal128)
        .ok_or_else(|| format!("invalid decimal `{s}`"))
}

fn binary_v2(b: &Document) -> std::result::Result<Bson, String> {
    match (b.get("base64"), b.get("subType")) {
        (Some(Bson::String(b64)), Some(Bson::String(t))) if b.len() == 2 => binary(b64, t),
        _ => Err("$binary takes {base64, subType}".into()),
    }
}

fn binary(b64: &str, subtype_hex: &str) -> std::result::Result<Bson, String> {
    let subtype = u8::from_str_radix(subtype_hex, 16)
        .ok()
        .filter(|_| (1..=2).contains(&subtype_hex.len()))
        .ok_or_else(|| format!("invalid binary subtype `{subtype_hex}`"))?;
    binary_bytes(b64, subtype)
}

fn binary_bytes(b64: &str, subtype: u8) -> std::result::Result<Bson, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| "invalid base64".to_string())?;
    Ok(Bson::Binary(Binary {
        subtype: BinarySubtype::from(subtype),
        bytes,
    }))
}

fn uuid(s: &str) -> std::result::Result<Bson, String> {
    let u = uuid::Uuid::parse_str(s.trim()).map_err(|_| format!("invalid UUID `{s}`"))?;
    Ok(Bson::Binary(Binary {
        subtype: BinarySubtype::Uuid,
        bytes: u.as_bytes().to_vec(),
    }))
}

fn hex_bytes(s: &str) -> std::result::Result<Vec<u8>, String> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("invalid hex `{s}`"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| format!("invalid hex `{s}`")))
        .collect()
}

fn db_pointer(v: &Bson) -> std::result::Result<Bson, String> {
    let Bson::Document(p) = v else {
        return Err("$dbPointer takes {$ref, $id}".into());
    };
    match (p.get("$ref"), p.get("$id")) {
        (Some(Bson::String(ns)), Some(Bson::ObjectId(id))) if p.len() == 2 => {
            // The crate builds DbPointer values only from Extended JSON.
            let json =
                serde_json::json!({ "$dbPointer": { "$ref": ns, "$id": { "$oid": id.to_hex() } } });
            Bson::try_from(json).map_err(|e| e.to_string())
        }
        _ => Err("$dbPointer takes {$ref: string, $id: ObjectId}".into()),
    }
}

/// A shell helper call: `ObjectId("…")`, `ISODate("…")`, `NumberLong(5)`…
fn helper(name: &str, new: bool, args: Vec<Bson>) -> std::result::Result<Bson, String> {
    let arity = |min: usize, max: usize| {
        if args.len() < min || args.len() > max {
            Err(format!("{name}() takes {}", plural_args(min, max)))
        } else {
            Ok(())
        }
    };
    let str_arg = |i: usize| match args.get(i) {
        Some(Bson::String(s)) => Ok(s.clone()),
        _ => Err(format!("{name}() takes a string")),
    };
    match name {
        "ObjectId" | "ObjectID" => {
            arity(0, 1)?;
            match args.first() {
                None => Ok(Bson::ObjectId(ObjectId::new())),
                Some(Bson::String(s)) => ObjectId::parse_str(s)
                    .map(Bson::ObjectId)
                    .map_err(|_| format!("invalid ObjectId `{s}`")),
                Some(Bson::ObjectId(o)) => Ok(Bson::ObjectId(*o)),
                Some(_) => Err("ObjectId() takes a 24-digit hex string".into()),
            }
        }
        "ISODate" | "Date" => {
            arity(0, 1)?;
            match args.first() {
                None => Ok(Bson::DateTime(DateTime::now())),
                Some(Bson::String(s)) => parse_date_millis(s)
                    .map(|ms| Bson::DateTime(DateTime::from_millis(ms)))
                    .ok_or_else(|| format!("invalid date `{s}`")),
                Some(n @ (Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_))) => int_of(n)
                    .map(|ms| Bson::DateTime(DateTime::from_millis(ms)))
                    .ok_or_else(|| format!("{name}() takes whole milliseconds")),
                Some(d @ Bson::DateTime(_)) => Ok(d.clone()),
                Some(_) => Err(format!("{name}() takes a date string or milliseconds")),
            }
        }
        "NumberLong" | "Long" => {
            arity(1, 1)?;
            match &args[0] {
                Bson::String(s) => s
                    .trim()
                    .parse::<i64>()
                    .map(Bson::Int64)
                    .map_err(|_| format!("invalid {name} `{s}`")),
                n => int_of(n)
                    .map(Bson::Int64)
                    .ok_or_else(|| format!("{name}() takes an integer")),
            }
        }
        "NumberInt" | "Int32" => {
            arity(1, 1)?;
            match &args[0] {
                Bson::String(s) => s
                    .trim()
                    .parse::<i32>()
                    .map(Bson::Int32)
                    .map_err(|_| format!("invalid {name} `{s}`")),
                n => int_of(n)
                    .and_then(|n| i32::try_from(n).ok())
                    .map(Bson::Int32)
                    .ok_or_else(|| format!("{name}() takes a 32-bit integer")),
            }
        }
        "Double" | "Number" => {
            arity(1, 1)?;
            match &args[0] {
                Bson::Double(f) => Ok(Bson::Double(*f)),
                Bson::Int32(n) => Ok(Bson::Double(f64::from(*n))),
                Bson::Int64(n) => Ok(Bson::Double(*n as f64)),
                Bson::String(s) => s
                    .trim()
                    .parse::<f64>()
                    .map(Bson::Double)
                    .map_err(|_| format!("invalid {name} `{s}`")),
                _ => Err(format!("{name}() takes a number")),
            }
        }
        "NumberDecimal" | "Decimal128" => {
            arity(1, 1)?;
            match &args[0] {
                Bson::String(s) => decimal(s),
                Bson::Int32(n) => decimal(&n.to_string()),
                Bson::Int64(n) => decimal(&n.to_string()),
                Bson::Double(f) if f.is_finite() => decimal(&f.to_string()),
                _ => Err(format!("{name}() takes a decimal string")),
            }
        }
        "UUID" => {
            arity(0, 1)?;
            match args.first() {
                None => Ok(Bson::Binary(Binary {
                    subtype: BinarySubtype::Uuid,
                    bytes: uuid::Uuid::new_v4().as_bytes().to_vec(),
                })),
                Some(_) => uuid(&str_arg(0)?),
            }
        }
        "BinData" => {
            arity(2, 2)?;
            let subtype = int_of(&args[0])
                .and_then(|n| u8::try_from(n).ok())
                .ok_or("BinData() takes a subtype from 0 to 255")?;
            binary_bytes(&str_arg(1)?, subtype)
        }
        "HexData" => {
            arity(2, 2)?;
            let subtype = int_of(&args[0])
                .and_then(|n| u8::try_from(n).ok())
                .ok_or("HexData() takes a subtype from 0 to 255")?;
            Ok(Bson::Binary(Binary {
                subtype: BinarySubtype::from(subtype),
                bytes: hex_bytes(&str_arg(1)?)?,
            }))
        }
        "MD5" => {
            arity(1, 1)?;
            Ok(Bson::Binary(Binary {
                subtype: BinarySubtype::Md5,
                bytes: hex_bytes(&str_arg(0)?)?,
            }))
        }
        "Timestamp" => {
            arity(0, 2)?;
            let (t, i) = match args.as_slice() {
                [] => (Some(0), Some(0)),
                [Bson::Document(d)] => (d.get("t").and_then(u32_of), d.get("i").and_then(u32_of)),
                [t, i] => (u32_of(t), u32_of(i)),
                _ => (None, None),
            };
            match (t, i) {
                (Some(time), Some(increment)) => Ok(Bson::Timestamp(Timestamp { time, increment })),
                _ => Err("Timestamp() takes (seconds, increment) or {t, i}".into()),
            }
        }
        "MinKey" => arity(0, 0).map(|_| Bson::MinKey),
        "MaxKey" => arity(0, 0).map(|_| Bson::MaxKey),
        "Code" => {
            arity(1, 2)?;
            let code = str_arg(0)?;
            match args.get(1) {
                None => Ok(Bson::JavaScriptCode(code)),
                Some(Bson::Document(scope)) => {
                    Ok(Bson::JavaScriptCodeWithScope(JavaScriptCodeWithScope {
                        code,
                        scope: scope.clone(),
                    }))
                }
                Some(_) => Err("Code() takes a scope document".into()),
            }
        }
        "RegExp" => {
            arity(1, 2)?;
            let flags = match args.get(1) {
                Some(Bson::String(f)) => f.clone(),
                None => String::new(),
                Some(_) => return Err("RegExp() takes string flags".into()),
            };
            match &args[0] {
                Bson::String(p) => regex(p.clone(), &flags),
                Bson::RegularExpression(r) if args.len() == 1 => {
                    Ok(Bson::RegularExpression(r.clone()))
                }
                _ => Err("RegExp() takes a pattern string".into()),
            }
        }
        "DBRef" => {
            arity(2, 3)?;
            let mut d = Document::new();
            d.insert("$ref", str_arg(0)?);
            d.insert("$id", args[1].clone());
            if args.len() == 3 {
                d.insert("$db", str_arg(2)?);
            }
            Ok(Bson::Document(d))
        }
        _ => Err(format!(
            "{}{name}() is not supported: the console runs a subset of mongosh (no JavaScript)",
            if new { "new " } else { "" }
        )),
    }
}

fn plural_args(min: usize, max: usize) -> String {
    match (min, max) {
        (0, 0) => "no arguments".into(),
        (a, b) if a == b => format!("{a} argument{}", if a == 1 { "" } else { "s" }),
        (a, b) => format!("{a} to {b} arguments"),
    }
}

/// A whole number from an Int32, an Int64 or an integral Double.
pub fn int_of(b: &Bson) -> Option<i64> {
    match b {
        Bson::Int32(n) => Some(i64::from(*n)),
        Bson::Int64(n) => Some(*n),
        Bson::Double(f) if f.fract() == 0.0 && f.abs() < 9.2e18 => Some(*f as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use ::mongodb::bson::doc;

    use super::*;

    fn v(text: &str) -> Bson {
        parse_value(text).unwrap_or_else(|e| panic!("{text}: {e:?}"))
    }

    fn err(text: &str) -> String {
        parse_value(text).expect_err(text).message
    }

    #[test]
    fn json_is_read_as_extended_json() {
        assert_eq!(v("1"), Bson::Int32(1));
        assert_eq!(v("-2147483648"), Bson::Int32(i32::MIN));
        assert_eq!(v("2147483648"), Bson::Int64(2_147_483_648));
        assert_eq!(v("-9223372036854775808"), Bson::Int64(i64::MIN));
        assert_eq!(
            v("9223372036854775808"),
            Bson::Double(9.223_372_036_854_776e18)
        );
        assert_eq!(v("1.0"), Bson::Double(1.0));
        assert_eq!(v("1e3"), Bson::Double(1000.0));
        assert_eq!(v("-1.5E-3"), Bson::Double(-0.0015));
        assert_eq!(v(".5"), Bson::Double(0.5));
        assert_eq!(v("\"a\\nb\""), Bson::String("a\nb".into()));
        assert_eq!(v("null"), Bson::Null);
        assert_eq!(
            v(r#"{"a": [1, true, null], "b": {"c": "d"}}"#),
            Bson::Document(doc! { "a": [1, true, null], "b": { "c": "d" } })
        );
    }

    #[test]
    fn shell_literals() {
        assert_eq!(
            v("{a: 1, 'b': \"x\", $set: {c: 2,}, d: [1, 2,], `e`: 3, 7: 8}"),
            Bson::Document(
                doc! { "a": 1, "b": "x", "$set": { "c": 2 }, "d": [1, 2], "e": 3, "7": 8 }
            )
        );
        assert_eq!(v("{a.b: 1}"), Bson::Document(doc! { "a.b": 1 }));
        assert_eq!(
            v("{ /* c */ a: 1, // line\n b: 2 }"),
            Bson::Document(doc! { "a": 1, "b": 2 })
        );
        assert_eq!(v("0x1F"), Bson::Int32(31));
        assert_eq!(v("5n"), Bson::Int64(5));
        assert_eq!(v("-5n"), Bson::Int64(-5));
        assert_eq!(v("+3"), Bson::Int32(3));
        assert_eq!(v("undefined"), Bson::Null);
        assert!(matches!(v("NaN"), Bson::Double(f) if f.is_nan()));
        assert_eq!(v("-Infinity"), Bson::Double(f64::NEG_INFINITY));
        assert_eq!(v("MinKey"), Bson::MinKey);
        assert_eq!(v("MaxKey()"), Bson::MaxKey);
        assert_eq!(v("'it\\'s'"), Bson::String("it's".into()));
        assert_eq!(
            v("'\\x41\\u0042\\u{43}\\u{1F600}'"),
            Bson::String("ABC😀".into())
        );
        assert_eq!(v("'\\uD83D\\uDE00'"), Bson::String("😀".into()));
        assert_eq!(v("'a\\\nb'"), Bson::String("ab".into()));
        assert_eq!(v("`multi\nline`"), Bson::String("multi\nline".into()));
    }

    #[test]
    fn regex_literals() {
        assert_eq!(
            v("/^a\\/b[/]c/mi"),
            Bson::RegularExpression(Regex {
                pattern: "^a\\/b[/]c".into(),
                options: "im".into()
            })
        );
        assert!(err("/abc/g").contains("flag"));
        assert!(err("/abc").contains("unterminated"));
        assert_eq!(
            v("RegExp('a.c', 'si')"),
            Bson::RegularExpression(Regex {
                pattern: "a.c".into(),
                options: "is".into()
            })
        );
    }

    #[test]
    fn shell_helpers() {
        let oid = ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap();
        assert_eq!(
            v("ObjectId('65f1a2b3c4d5e6f708192a3b')"),
            Bson::ObjectId(oid)
        );
        assert_eq!(
            v("new ObjectId(\"65f1a2b3c4d5e6f708192a3b\")"),
            Bson::ObjectId(oid)
        );
        assert!(matches!(v("ObjectId()"), Bson::ObjectId(_)));
        assert_eq!(
            v("ISODate('2024-05-01T10:00:00Z')"),
            Bson::DateTime(DateTime::from_millis(1_714_557_600_000))
        );
        assert_eq!(v("new Date(0)"), Bson::DateTime(DateTime::from_millis(0)));
        assert!(matches!(v("new Date()"), Bson::DateTime(_)));
        assert_eq!(v("NumberLong(5)"), Bson::Int64(5));
        assert_eq!(
            v("NumberLong('9223372036854775807')"),
            Bson::Int64(i64::MAX)
        );
        assert_eq!(v("Long('-3')"), Bson::Int64(-3));
        assert_eq!(v("NumberInt('7')"), Bson::Int32(7));
        assert_eq!(v("Int32(7)"), Bson::Int32(7));
        assert_eq!(v("Double(7)"), Bson::Double(7.0));
        assert_eq!(
            v("NumberDecimal('1.10')"),
            Bson::Decimal128("1.10".parse().unwrap())
        );
        assert_eq!(
            v("Decimal128('-0.0001')"),
            Bson::Decimal128("-0.0001".parse().unwrap())
        );
        let u = uuid::Uuid::parse_str("0f8fad5b-d9cb-469f-a165-70867728950e").unwrap();
        assert_eq!(
            v("UUID('0f8fad5bd9cb469fa16570867728950e')"),
            Bson::Binary(Binary {
                subtype: BinarySubtype::Uuid,
                bytes: u.as_bytes().to_vec()
            })
        );
        assert_eq!(
            v("BinData(0, 'AAEC')"),
            Bson::Binary(Binary {
                subtype: BinarySubtype::Generic,
                bytes: vec![0, 1, 2]
            })
        );
        assert_eq!(
            v("HexData(5, 'ff00')"),
            Bson::Binary(Binary {
                subtype: BinarySubtype::Md5,
                bytes: vec![255, 0]
            })
        );
        assert_eq!(
            v("MD5('00ff')"),
            Bson::Binary(Binary {
                subtype: BinarySubtype::Md5,
                bytes: vec![0, 255]
            })
        );
        assert_eq!(
            v("Timestamp(1700000000, 3)"),
            Bson::Timestamp(Timestamp {
                time: 1_700_000_000,
                increment: 3
            })
        );
        assert_eq!(
            v("Timestamp({t: 5, i: 6})"),
            Bson::Timestamp(Timestamp {
                time: 5,
                increment: 6
            })
        );
        assert_eq!(v("Code('x()')"), Bson::JavaScriptCode("x()".into()));
        assert_eq!(
            v("DBRef('users', 5)"),
            Bson::Document(doc! { "$ref": "users", "$id": 5 })
        );
    }

    #[test]
    fn helper_errors() {
        assert!(err("ObjectId('xyz')").contains("invalid ObjectId"));
        assert!(err("NumberInt(3000000000)").contains("32-bit"));
        assert!(err("NumberLong('abc')").contains("invalid NumberLong"));
        assert!(err("ISODate('soon')").contains("invalid date"));
        assert!(err("foo(1)").contains("not supported"));
        assert!(err("new Map()").contains("not supported"));
        assert!(err("x").contains("no JavaScript variables"));
        assert!(err("BinData(300, 'AA')").contains("subtype"));
        assert!(err("UUID('nope')").contains("invalid UUID"));
        // Text without a digit is not a decimal (bson would read it as 0); NaN and the infinities are.
        for t in ["", " ", ".", "-", "e5"] {
            assert!(
                err(&format!("NumberDecimal('{t}')")).contains("invalid decimal"),
                "{t:?}"
            );
        }
        assert!(matches!(
            v("NumberDecimal('-Infinity')"),
            Bson::Decimal128(_)
        ));
        assert!(matches!(v("Decimal128('NaN')"), Bson::Decimal128(_)));
    }

    #[test]
    fn extended_json_wrappers() {
        let oid = ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap();
        assert_eq!(
            v(r#"{"$oid": "65f1a2b3c4d5e6f708192a3b"}"#),
            Bson::ObjectId(oid)
        );
        assert_eq!(v(r#"{"$numberInt": "5"}"#), Bson::Int32(5));
        assert_eq!(v(r#"{"$numberLong": "5"}"#), Bson::Int64(5));
        assert_eq!(v(r#"{"$numberDouble": "1.0"}"#), Bson::Double(1.0));
        assert_eq!(
            v(r#"{"$numberDouble": "-Infinity"}"#),
            Bson::Double(f64::NEG_INFINITY)
        );
        assert_eq!(
            v(r#"{"$numberDecimal": "1.5"}"#),
            Bson::Decimal128("1.5".parse().unwrap())
        );
        assert_eq!(
            v(r#"{"$date": "1970-01-01T00:00:00.001Z"}"#),
            Bson::DateTime(DateTime::from_millis(1))
        );
        assert_eq!(
            v(r#"{"$date": {"$numberLong": "-5"}}"#),
            Bson::DateTime(DateTime::from_millis(-5))
        );
        assert_eq!(
            v(r#"{"$binary": {"base64": "AAE=", "subType": "80"}}"#),
            Bson::Binary(Binary {
                subtype: BinarySubtype::from(0x80),
                bytes: vec![0, 1]
            })
        );
        assert_eq!(
            v(r#"{"$binary": "AAE=", "$type": "0"}"#),
            Bson::Binary(Binary {
                subtype: BinarySubtype::Generic,
                bytes: vec![0, 1]
            })
        );
        assert_eq!(
            v(r#"{"$timestamp": {"t": 1, "i": 2}}"#),
            Bson::Timestamp(Timestamp {
                time: 1,
                increment: 2
            })
        );
        assert_eq!(
            v(r#"{"$regularExpression": {"pattern": "a", "options": "mi"}}"#),
            Bson::RegularExpression(Regex {
                pattern: "a".into(),
                options: "im".into()
            })
        );
        assert_eq!(v(r#"{"$minKey": 1}"#), Bson::MinKey);
        assert_eq!(v(r#"{"$maxKey": 1}"#), Bson::MaxKey);
        assert_eq!(v(r#"{"$undefined": true}"#), Bson::Undefined);
        assert_eq!(v(r#"{"$symbol": "s"}"#), Bson::Symbol("s".into()));
        assert_eq!(v(r#"{"$code": "c"}"#), Bson::JavaScriptCode("c".into()));
        assert!(matches!(
            v(r#"{"$code": "c", "$scope": {"x": 1}}"#),
            Bson::JavaScriptCodeWithScope(_)
        ));
        assert!(matches!(
            v(r#"{"$dbPointer": {"$ref": "db.c", "$id": {"$oid": "65f1a2b3c4d5e6f708192a3b"}}}"#),
            Bson::DbPointer(_)
        ));
        assert_eq!(
            v(r#"{"$uuid": "0f8fad5b-d9cb-469f-a165-70867728950e"}"#),
            uuid("0f8fad5b-d9cb-469f-a165-70867728950e").unwrap()
        );
        // Wrong values of a wrapper are errors, not documents.
        assert!(err(r#"{"$oid": 5}"#).contains("$oid"));
        assert!(err(r#"{"$numberInt": "3000000000"}"#).contains("$numberInt"));
        for t in ["", ".", "-", "+", "e5"] {
            let text = format!(r#"{{"$numberDecimal": "{t}"}}"#);
            assert!(err(&text).contains("invalid decimal"), "{t:?}");
        }
        assert!(err(r#"{"$binary": {"base64": "!!", "subType": "00"}}"#).contains("base64"));
    }

    #[test]
    fn query_operators_stay_documents() {
        assert_eq!(
            v(r#"{"$regex": "^a", "$options": "i"}"#),
            Bson::Document(doc! { "$regex": "^a", "$options": "i" })
        );
        assert_eq!(
            v("{$type: 'string'}"),
            Bson::Document(doc! { "$type": "string" })
        );
        assert_eq!(
            v("{$oid: 'x', other: 1}"),
            Bson::Document(doc! { "$oid": "x", "other": 1 })
        );
        assert_eq!(v("{$gt: 5}"), Bson::Document(doc! { "$gt": 5 }));
    }

    #[test]
    fn syntax_errors_have_positions() {
        let e = parse_value("{a: 1,\n  b: }").unwrap_err();
        assert!(e.message.contains("expected a value"), "{e:?}");
        let pos = e.position("{a: 1,\n  b: }");
        assert_eq!((pos.line, pos.column), (2, 6));
        assert!(err("{a 1}").contains("`:`"));
        assert!(err("[1 2]").contains("`]`"));
        assert!(err("'abc").contains("unterminated"));
        assert!(err("'a\nb'").contains("unterminated"));
        assert!(err("/* x").contains("unterminated comment"));
        assert!(err("1 2").contains("end of the statement"));
        assert!(err("12abc").contains("invalid number"));
        assert!(err("1_000").contains("invalid number"));
        assert!(err("--1").contains("invalid number"));
        assert!(err("`${x}`").contains("template"));
        assert!(err("'\\uD800'").contains("surrogate"));
        assert!(err("").contains("expected a value"));
        let app = parse_value("{a: }").unwrap_err().into_app("{a: }");
        assert!(app.to_string().contains("line 1, column 5"), "{app}");
    }

    #[test]
    fn positions_count_characters() {
        let text = "{é: 'ü', x: }";
        let e = parse_value(text).unwrap_err();
        assert_eq!(e.position(text).column, 13);
    }

    #[test]
    fn deep_nesting_is_refused_without_overflow() {
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(err(&deep).contains("nested too deeply"));
        let ok = format!("{}1{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse_value(&ok).is_ok());
    }

    #[test]
    fn documents_only() {
        assert_eq!(parse_document("{a: 1}").unwrap(), doc! { "a": 1 });
        assert!(parse_document("[1]").is_err());
        // A wrapper is a value, not a document.
        assert!(parse_document(r#"{"$oid": "65f1a2b3c4d5e6f708192a3b"}"#).is_err());
    }

    #[test]
    fn never_panics_on_garbage() {
        let pieces = [
            "{", "}", "[", "]", "(", ")", "'", "\"", "`", "\\", "/", "*", "$", "a", "1", ".", "e",
            "-", "+", "0x", "n", ":", ",", "ObjectId", "new", " ", "\n", "é", "\\u", "{$date:",
            "NaN", "/*", "//", "\u{2028}",
        ];
        let mut seed: u64 = 0x5eed;
        for _ in 0..4000 {
            let mut text = String::new();
            for _ in 0..(seed % 12) {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                text.push_str(pieces[(seed >> 33) as usize % pieces.len()]);
            }
            let _ = parse_value(&text);
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        }
    }
}
