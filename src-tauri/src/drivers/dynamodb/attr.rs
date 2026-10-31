//! DynamoDB `AttributeValue`s (`{"S": "x"}`, `{"N": "1.5"}`, …) and Tabler's `Value`s.
//!
//! Decoding is total: nothing a server sends makes it panic. Each attribute keeps its type, so an
//! edit writes back what was read:
//!
//! - `S` → `Text`; `N` → `Int` (integral and within 2^53), `BigInt` (integral beyond), else
//!   `Decimal` with the server's exact text, never a float; `B` → `Bytes`; `BOOL` → `Bool`;
//!   `NULL` → `Null`;
//! - `M` and `L` → `Json`: DynamoDB JSON of the content (`{"a":{"N":"1"}}`, `[{"S":"x"}]`), which
//!   keeps every nested type; longer than [`JSON_CELL_CAP`] a cut `Other{jsonPreview}`;
//! - `SS`, `NS`, `BS` → `Array` of `Text`, numbers or `Bytes`;
//! - an attribute an item does not have → `Other{missing}` (not the same as `NULL`).
//!
//! Encoding ([`from_value`]) takes the attribute's type as a hint: text typed into an `N` cell is
//! a number, into an `S` cell a string (`'02134'` stays a string), into a set cell a JSON array.
//! Json cells are read as DynamoDB JSON when every value is typed, otherwise as plain JSON
//! (objects maps, arrays lists, numbers `N` with their exact text).

use std::collections::{BTreeMap, HashSet};

use base64::Engine as _;

use crate::error::AppError;
use crate::types::schema::ValueKind;
use crate::types::value::Value;

/// `Other` type of an attribute the item does not have.
pub const TY_MISSING: &str = "missing";
/// `Other` type of a map or list too long for a grid cell: a cut preview, never written back.
pub const TY_PREVIEW: &str = "jsonPreview";
/// `Other` type the shared cell editor gives text typed into a cell of an `other` column.
pub const TY_TEXT: &str = "other";
/// Maps and lists longer than this as JSON text are cut in grid cells; `fetch_cell` loads them.
pub const JSON_CELL_CAP: usize = 64 * 1024;
/// DynamoDB nests at most 32 levels; deeper replies are refused, not recursed into.
pub const MAX_DEPTH: usize = 64;
/// Significant digits a number may have.
pub const MAX_NUMBER_DIGITS: usize = 38;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attr {
    S(String),
    /// The number's text as DynamoDB keeps it.
    N(String),
    B(Vec<u8>),
    Bool(bool),
    Null,
    M(BTreeMap<String, Attr>),
    L(Vec<Attr>),
    Ss(Vec<String>),
    Ns(Vec<String>),
    Bs(Vec<Vec<u8>>),
}

/// An item: attribute name → value.
pub type Item = BTreeMap<String, Attr>;

/// At most the memory an item (or an `M`'s content) holds: its tree's nodes, its names and
/// what its values hold. What the budgets of items kept count.
pub fn item_size(item: &Item) -> usize {
    let n = item.len();
    let nodes = match n {
        0 => 0,
        // A node has room for 11 entries: a map of one costs a whole node.
        1..=11 => alloc(LEAF),
        // Every node but the root holds at least 5; an internal node is over at least 5.
        _ => n.div_ceil(5) * alloc(LEAF) + n.div_ceil(25) * alloc(INTERNAL),
    };
    nodes
        + item
            .iter()
            .map(|(k, v)| alloc(k.len()) + v.heap())
            .sum::<usize>()
}

const VALUE: usize = std::mem::size_of::<Attr>();
/// A `String`, and a `Vec<u8>` of a binary set.
const STRING: usize = std::mem::size_of::<String>();
/// A leaf of an item's tree: 11 names and values, its parent link, index and length.
const LEAF: usize = 11 * (STRING + VALUE) + 16;
/// An internal node: a leaf and its 12 edges.
const INTERNAL: usize = LEAF + 12 * std::mem::size_of::<usize>();

/// An allocation of `n` bytes with the allocator's header and rounding (its size classes are at
/// most an eighth apart).
fn alloc(n: usize) -> usize {
    if n == 0 { 0 } else { n + n / 8 + 32 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AttrType {
    S,
    N,
    B,
    Bool,
    Null,
    M,
    L,
    Ss,
    Ns,
    Bs,
}

impl AttrType {
    pub const ALL: [AttrType; 10] = [
        AttrType::S,
        AttrType::N,
        AttrType::B,
        AttrType::Bool,
        AttrType::Null,
        AttrType::M,
        AttrType::L,
        AttrType::Ss,
        AttrType::Ns,
        AttrType::Bs,
    ];

    /// The tag DynamoDB uses (`S`, `BOOL`, `SS`…).
    pub fn as_str(self) -> &'static str {
        match self {
            AttrType::S => "S",
            AttrType::N => "N",
            AttrType::B => "B",
            AttrType::Bool => "BOOL",
            AttrType::Null => "NULL",
            AttrType::M => "M",
            AttrType::L => "L",
            AttrType::Ss => "SS",
            AttrType::Ns => "NS",
            AttrType::Bs => "BS",
        }
    }

    pub fn parse(tag: &str) -> Option<AttrType> {
        AttrType::ALL.into_iter().find(|t| t.as_str() == tag)
    }

    /// The grid kind of a column of this type alone.
    pub fn kind(self) -> ValueKind {
        match self {
            AttrType::S => ValueKind::Text,
            AttrType::N => ValueKind::Decimal,
            AttrType::B => ValueKind::Bytes,
            AttrType::Bool => ValueKind::Bool,
            AttrType::Null => ValueKind::Other,
            AttrType::M | AttrType::L => ValueKind::Json,
            AttrType::Ss | AttrType::Ns | AttrType::Bs => ValueKind::Array,
        }
    }

    pub fn is_set(self) -> bool {
        matches!(self, AttrType::Ss | AttrType::Ns | AttrType::Bs)
    }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(text: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .ok()
}

fn invalid(why: impl Into<String>) -> AppError {
    AppError::InvalidInput(why.into())
}

fn protocol(why: impl std::fmt::Display) -> AppError {
    AppError::db(format!("unexpected attribute value from DynamoDB: {why}"))
}

/// Checks DynamoDB's number syntax (an optional sign, digits with an optional fraction, an
/// optional exponent; at most 38 significant digits) and returns the text.
pub fn check_number(text: &str) -> Result<String, AppError> {
    let bad = || invalid(format!("\u{201c}{text}\u{201d} is not a DynamoDB number"));
    let s = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (mantissa, exponent) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    if let Some(e) = exponent {
        let digits = e.strip_prefix(['-', '+']).unwrap_or(e);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
    }
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (int.is_empty() && frac.is_empty())
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(bad());
    }
    let digits: String = format!("{int}{frac}");
    let significant = digits.trim_start_matches('0').trim_end_matches('0');
    if significant.len() > MAX_NUMBER_DIGITS {
        return Err(invalid(format!(
            "\u{201c}{text}\u{201d} has more than {MAX_NUMBER_DIGITS} significant digits, which DynamoDB cannot store"
        )));
    }
    Ok(text.to_string())
}

impl Attr {
    pub fn ty(&self) -> AttrType {
        match self {
            Attr::S(_) => AttrType::S,
            Attr::N(_) => AttrType::N,
            Attr::B(_) => AttrType::B,
            Attr::Bool(_) => AttrType::Bool,
            Attr::Null => AttrType::Null,
            Attr::M(_) => AttrType::M,
            Attr::L(_) => AttrType::L,
            Attr::Ss(_) => AttrType::Ss,
            Attr::Ns(_) => AttrType::Ns,
            Attr::Bs(_) => AttrType::Bs,
        }
    }

    /// At most the memory the value holds outside itself (its own place is its map's node or
    /// its list's buffer). Lists and sets collected from a reply may have twice the room they
    /// use.
    fn heap(&self) -> usize {
        match self {
            Attr::S(s) | Attr::N(s) => alloc(s.len()),
            Attr::B(b) => alloc(b.len()),
            Attr::Bool(_) | Attr::Null => 0,
            Attr::M(m) => item_size(m),
            Attr::L(l) => alloc(2 * l.len() * VALUE) + l.iter().map(Attr::heap).sum::<usize>(),
            Attr::Ss(s) | Attr::Ns(s) => {
                alloc(2 * s.len() * STRING) + s.iter().map(|x| alloc(x.len())).sum::<usize>()
            }
            Attr::Bs(b) => {
                alloc(2 * b.len() * STRING) + b.iter().map(|x| alloc(x.len())).sum::<usize>()
            }
        }
    }

    /// An AttributeValue of a reply or a request.
    pub fn from_json(v: &serde_json::Value) -> Result<Attr, AppError> {
        Attr::from_json_at(v, 0)
    }

    fn from_json_at(v: &serde_json::Value, depth: usize) -> Result<Attr, AppError> {
        if depth > MAX_DEPTH {
            return Err(protocol("nested too deep"));
        }
        let obj = v
            .as_object()
            .ok_or_else(|| protocol(format!("{v} is not an object")))?;
        let mut entries = obj.iter();
        let (Some((tag, inner)), None) = (entries.next(), entries.next()) else {
            return Err(protocol("an attribute value must have exactly one type"));
        };
        let string = |x: &serde_json::Value| {
            x.as_str()
                .map(str::to_string)
                .ok_or_else(|| protocol(format!("{tag} holds {x}, not a string")))
        };
        let strings = |x: &serde_json::Value| -> Result<Vec<String>, AppError> {
            x.as_array()
                .ok_or_else(|| protocol(format!("{tag} holds {x}, not an array")))?
                .iter()
                .map(string)
                .collect()
        };
        let bytes = |s: String| unb64(&s).ok_or_else(|| protocol(format!("{tag} is not base64")));
        let number =
            |s: String| check_number(&s).map_err(|_| protocol(format!("N \u{201c}{s}\u{201d}")));
        Ok(match tag.as_str() {
            "S" => Attr::S(string(inner)?),
            "N" => Attr::N(number(string(inner)?)?),
            "B" => Attr::B(bytes(string(inner)?)?),
            "BOOL" => Attr::Bool(
                inner
                    .as_bool()
                    .ok_or_else(|| protocol(format!("BOOL holds {inner}")))?,
            ),
            "NULL" => match inner.as_bool() {
                Some(true) => Attr::Null,
                _ => return Err(protocol(format!("NULL holds {inner}"))),
            },
            "M" => Attr::M(
                inner
                    .as_object()
                    .ok_or_else(|| protocol(format!("M holds {inner}")))?
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), Attr::from_json_at(v, depth + 1)?)))
                    .collect::<Result<_, AppError>>()?,
            ),
            "L" => Attr::L(
                inner
                    .as_array()
                    .ok_or_else(|| protocol(format!("L holds {inner}")))?
                    .iter()
                    .map(|v| Attr::from_json_at(v, depth + 1))
                    .collect::<Result<_, AppError>>()?,
            ),
            "SS" => Attr::Ss(strings(inner)?),
            "NS" => Attr::Ns(
                strings(inner)?
                    .into_iter()
                    .map(number)
                    .collect::<Result<_, _>>()?,
            ),
            "BS" => Attr::Bs(
                strings(inner)?
                    .into_iter()
                    .map(bytes)
                    .collect::<Result<_, _>>()?,
            ),
            other => return Err(protocol(format!("unknown type {other}"))),
        })
    }

    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::{Value as J, json};
        match self {
            Attr::S(s) => json!({ "S": s }),
            Attr::N(n) => json!({ "N": n }),
            Attr::B(b) => json!({ "B": b64(b) }),
            Attr::Bool(b) => json!({ "BOOL": b }),
            Attr::Null => json!({ "NULL": true }),
            Attr::M(m) => json!({ "M": map_json(m) }),
            Attr::L(l) => json!({ "L": J::Array(l.iter().map(Attr::to_json).collect()) }),
            Attr::Ss(s) => json!({ "SS": s }),
            Attr::Ns(n) => json!({ "NS": n }),
            Attr::Bs(b) => json!({ "BS": b.iter().map(|x| b64(x)).collect::<Vec<_>>() }),
        }
    }
}

/// A map of AttributeValues as JSON (an item, a key, `M`'s content).
pub fn map_json(m: &BTreeMap<String, Attr>) -> serde_json::Value {
    serde_json::Value::Object(m.iter().map(|(k, v)| (k.clone(), v.to_json())).collect())
}

/// An item (or a key) of a reply.
pub fn item_from_json(v: &serde_json::Value) -> Result<Item, AppError> {
    v.as_object()
        .ok_or_else(|| protocol(format!("an item must be an object, not {v}")))?
        .iter()
        .map(|(k, v)| Ok((k.clone(), Attr::from_json(v)?)))
        .collect()
}

// --- decoding to grid values -----------------------------------------------------------------

/// The `Other` value of an attribute the item does not have.
pub fn missing() -> Value {
    Value::other(TY_MISSING, "")
}

#[cfg(test)]
pub fn is_missing(v: &Value) -> bool {
    matches!(v, Value::Other(o) if o.ty == TY_MISSING)
}

/// An `N`'s text as a value: `Int` or `BigInt` when it is a plain integer, else `Decimal`.
pub fn number_value(text: &str) -> Value {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let plain_integer = !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'));
    if !plain_integer {
        return Value::Decimal(text.to_string());
    }
    match text.parse::<i64>() {
        Ok(n) => Value::from_i64(n),
        Err(_) => Value::BigInt(text.to_string()),
    }
}

/// How much of a value a cell gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    /// A grid cell: binaries and long maps or lists are cut.
    Grid,
    /// `fetch_cell`: the whole value.
    Full,
}

/// The JSON text of a map's or list's content (DynamoDB JSON, every nested type kept).
pub fn content_json(a: &Attr) -> String {
    match a {
        Attr::M(m) => map_json(m).to_string(),
        Attr::L(l) => serde_json::Value::Array(l.iter().map(Attr::to_json).collect()).to_string(),
        other => other.to_json().to_string(),
    }
}

pub fn to_value(a: &Attr, cap: Cap) -> Value {
    let bytes = |b: &[u8]| match cap {
        Cap::Grid => Value::from_bytes(b),
        Cap::Full => Value::from_bytes_uncapped(b),
    };
    match a {
        Attr::S(s) => Value::Text(s.clone()),
        Attr::N(n) => number_value(n),
        Attr::B(b) => bytes(b),
        Attr::Bool(b) => Value::Bool(*b),
        Attr::Null => Value::Null,
        Attr::M(_) | Attr::L(_) => {
            let text = content_json(a);
            if cap == Cap::Grid && text.len() > JSON_CELL_CAP {
                let mut end = JSON_CELL_CAP;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                Value::other(TY_PREVIEW, format!("{}…", &text[..end]))
            } else {
                Value::Json(text)
            }
        }
        Attr::Ss(s) => Value::Array(s.iter().cloned().map(Value::Text).collect()),
        Attr::Ns(n) => Value::Array(n.iter().map(|x| number_value(x)).collect()),
        Attr::Bs(b) => Value::Array(b.iter().map(|x| bytes(x)).collect()),
    }
}

/// The value of attribute `name` of an item (`Other{missing}` when it has none).
pub fn cell(item: &Item, name: &str, cap: Cap) -> Value {
    item.get(name).map_or_else(missing, |a| to_value(a, cap))
}

// --- a JSON reader that keeps numbers' text ---------------------------------------------------

/// JSON as written: numbers keep their text (serde_json would round them through f64), object
/// keys their order; a key repeated in an object is an error, so what is sent is what was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

pub fn parse_json(text: &str) -> Result<Json, AppError> {
    let mut r = Reader {
        s: text.as_bytes(),
        text,
        i: 0,
    };
    r.ws();
    let v = r.value(0)?;
    r.ws();
    if r.i < r.s.len() {
        return Err(r.err("unexpected text after the JSON value"));
    }
    Ok(v)
}

struct Reader<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
}

impl Reader<'_> {
    fn err(&self, why: &str) -> AppError {
        invalid(format!("invalid JSON at character {}: {why}", self.i + 1))
    }

    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, word: &str) -> bool {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, AppError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested too deep"));
        }
        match self.s.get(self.i) {
            None => Err(self.err("a value is missing")),
            Some(b'{') => {
                self.i += 1;
                let mut entries: Vec<(String, Json)> = Vec::new();
                self.ws();
                if self.eat("}") {
                    return Ok(Json::Obj(entries));
                }
                loop {
                    self.ws();
                    if self.s.get(self.i) != Some(&b'"') {
                        return Err(self.err("a key must be a string"));
                    }
                    let key = self.string()?;
                    if entries.iter().any(|(k, _)| *k == key) {
                        return Err(self.err(&format!("the key \u{201c}{key}\u{201d} is repeated")));
                    }
                    self.ws();
                    if !self.eat(":") {
                        return Err(self.err("expected ':'"));
                    }
                    self.ws();
                    let v = self.value(depth + 1)?;
                    entries.push((key, v));
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("}") {
                        return Ok(Json::Obj(entries));
                    }
                    return Err(self.err("expected ',' or '}'"));
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.eat("]") {
                    return Ok(Json::Arr(items));
                }
                loop {
                    self.ws();
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("]") {
                        return Ok(Json::Arr(items));
                    }
                    return Err(self.err("expected ',' or ']'"));
                }
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') if self.eat("true") => Ok(Json::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Json::Bool(false)),
            Some(b'n') if self.eat("null") => Ok(Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.i;
        while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
            self.i += 1;
        }
        self.i - start
    }

    fn number(&mut self) -> Result<Json, AppError> {
        let start = self.i;
        self.eat("-");
        let int_start = self.i;
        if self.digits() == 0 {
            return Err(self.err("a number needs digits"));
        }
        if self.s[int_start] == b'0' && self.i - int_start > 1 {
            return Err(self.err("a number cannot start with 0"));
        }
        if self.eat(".") && self.digits() == 0 {
            return Err(self.err("a fraction needs digits"));
        }
        if self.i < self.s.len() && matches!(self.s[self.i], b'e' | b'E') {
            self.i += 1;
            if self.i < self.s.len() && matches!(self.s[self.i], b'+' | b'-') {
                self.i += 1;
            }
            if self.digits() == 0 {
                return Err(self.err("an exponent needs digits"));
            }
        }
        Ok(Json::Num(self.text[start..self.i].to_string()))
    }

    fn hex4(&mut self) -> Result<u32, AppError> {
        let hex = self
            .text
            .get(self.i..self.i + 4)
            .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| self.err("a \\u escape needs four hex digits"))?;
        let n = u32::from_str_radix(hex, 16).map_err(|_| self.err("bad \\u escape"))?;
        self.i += 4;
        Ok(n)
    }

    fn string(&mut self) -> Result<String, AppError> {
        self.i += 1; // the opening quote
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.s.len()
                && !matches!(self.s[self.i], b'"' | b'\\')
                && self.s[self.i] >= 0x20
            {
                self.i += 1;
            }
            out.push_str(&self.text[start..self.i]);
            match self.s.get(self.i) {
                None => return Err(self.err("a string is not closed")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let Some(&c) = self.s.get(self.i) else {
                        return Err(self.err("a string is not closed"));
                    };
                    self.i += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                if !self.eat("\\u") {
                                    return Err(self.err("a lone surrogate"));
                                }
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(self.err("a lone surrogate"));
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            out.push(
                                char::from_u32(code).ok_or_else(|| self.err("a lone surrogate"))?,
                            );
                        }
                        _ => return Err(self.err("bad escape")),
                    }
                }
                Some(_) => return Err(self.err("a control character in a string")),
            }
        }
    }
}

// --- encoding from values -----------------------------------------------------------------------

/// An AttributeValue written as DynamoDB JSON (`{"S": "x"}`), when `j` is one.
fn typed(j: &Json, depth: usize) -> Option<Attr> {
    if depth > MAX_DEPTH {
        return None;
    }
    let Json::Obj(entries) = j else { return None };
    let [(tag, inner)] = entries.as_slice() else {
        return None;
    };
    let strings = |x: &Json| match x {
        Json::Arr(items) => items
            .iter()
            .map(|i| match i {
                Json::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>(),
        _ => None,
    };
    Some(match (tag.as_str(), inner) {
        ("S", Json::Str(s)) => Attr::S(s.clone()),
        ("N", Json::Str(s)) => Attr::N(check_number(s).ok()?),
        ("B", Json::Str(s)) => Attr::B(unb64(s)?),
        ("BOOL", Json::Bool(b)) => Attr::Bool(*b),
        ("NULL", Json::Bool(true)) => Attr::Null,
        ("M", Json::Obj(m)) => Attr::M(
            m.iter()
                .map(|(k, v)| Some((k.clone(), typed(v, depth + 1)?)))
                .collect::<Option<_>>()?,
        ),
        ("L", Json::Arr(l)) => Attr::L(
            l.iter()
                .map(|v| typed(v, depth + 1))
                .collect::<Option<_>>()?,
        ),
        ("SS", x) => Attr::Ss(strings(x)?),
        ("NS", x) => Attr::Ns(
            strings(x)?
                .iter()
                .map(|n| check_number(n).ok())
                .collect::<Option<_>>()?,
        ),
        ("BS", x) => Attr::Bs(
            strings(x)?
                .iter()
                .map(|b| unb64(b))
                .collect::<Option<_>>()?,
        ),
        _ => return None,
    })
}

/// Plain JSON as attributes: objects maps, arrays lists, strings `S`, numbers `N`.
fn plain(j: &Json) -> Result<Attr, AppError> {
    Ok(match j {
        Json::Null => Attr::Null,
        Json::Bool(b) => Attr::Bool(*b),
        Json::Num(n) => Attr::N(check_number(n)?),
        Json::Str(s) => Attr::S(s.clone()),
        Json::Arr(items) => Attr::L(items.iter().map(plain).collect::<Result<_, _>>()?),
        Json::Obj(entries) => Attr::M(
            entries
                .iter()
                .map(|(k, v)| Ok((k.clone(), plain(v)?)))
                .collect::<Result<_, AppError>>()?,
        ),
    })
}

/// A map's or list's content: DynamoDB JSON when every value is typed, else plain JSON.
fn content(j: &Json) -> Result<Attr, AppError> {
    match j {
        Json::Obj(entries) => {
            let all_typed: Option<BTreeMap<String, Attr>> = entries
                .iter()
                .map(|(k, v)| Some((k.clone(), typed(v, 1)?)))
                .collect();
            match all_typed {
                Some(m) => Ok(Attr::M(m)),
                None => plain(j),
            }
        }
        Json::Arr(items) => {
            let all_typed: Option<Vec<Attr>> = items.iter().map(|v| typed(v, 1)).collect();
            match all_typed {
                Some(l) => Ok(Attr::L(l)),
                None => plain(j),
            }
        }
        other => plain(other),
    }
}

/// A set, refusing what DynamoDB refuses: no members, or a member twice.
fn check_set<T: Eq + std::hash::Hash>(items: Vec<T>, ty: AttrType) -> Result<Vec<T>, AppError> {
    if items.is_empty() {
        return Err(invalid(format!(
            "a DynamoDB set ({}) cannot be empty: set the attribute to NULL or remove it",
            ty.as_str()
        )));
    }
    // Hashed: a set a server sent (a duplicated row's) can hold many thousands of members.
    let mut seen = HashSet::with_capacity(items.len());
    if !items.iter().all(|a| seen.insert(a)) {
        return Err(invalid(format!(
            "a DynamoDB set ({}) cannot hold a member twice",
            ty.as_str()
        )));
    }
    Ok(items)
}

fn make_set(ty: AttrType, members: Vec<Attr>) -> Result<Attr, AppError> {
    let wrong = |a: &Attr| {
        invalid(format!(
            "a {} set cannot hold a {} value",
            ty.as_str(),
            a.ty().as_str()
        ))
    };
    Ok(match ty {
        AttrType::Ss => Attr::Ss(check_set(
            members
                .into_iter()
                .map(|a| match a {
                    Attr::S(s) => Ok(s),
                    other => Err(wrong(&other)),
                })
                .collect::<Result<_, _>>()?,
            ty,
        )?),
        AttrType::Ns => Attr::Ns(check_set(
            members
                .into_iter()
                .map(|a| match a {
                    Attr::N(n) => Ok(n),
                    Attr::S(s) => check_number(s.trim()),
                    other => Err(wrong(&other)),
                })
                .collect::<Result<_, _>>()?,
            ty,
        )?),
        _ => Attr::Bs(check_set(
            members
                .into_iter()
                .map(|a| match a {
                    Attr::B(b) => Ok(b),
                    Attr::S(s) => unb64(&s).ok_or_else(|| invalid("a BS member is not base64")),
                    other => Err(wrong(&other)),
                })
                .collect::<Result<_, _>>()?,
            ty,
        )?),
    })
}

/// JSON text typed or staged for a cell of type `hint`. A map or list cell holds its content
/// (`{"a": …}`, `[…]`); any other cell may hold one typed value (`{"SS": ["a"]}`), which says its
/// own type, so a map with a single key named like a type is read as that type there.
fn json_as(text: &str, hint: Option<AttrType>) -> Result<Attr, AppError> {
    let j = parse_json(text)?;
    match hint {
        Some(AttrType::M) => match j {
            Json::Obj(_) => content(&j),
            _ => Err(invalid("a map (M) is written as a JSON object")),
        },
        Some(AttrType::L) => match j {
            Json::Arr(_) => content(&j),
            _ => Err(invalid("a list (L) is written as a JSON array")),
        },
        Some(t @ (AttrType::Ss | AttrType::Ns | AttrType::Bs)) => match &j {
            Json::Arr(items) => make_set(t, items.iter().map(plain).collect::<Result<_, _>>()?),
            _ => match typed(&j, 0) {
                Some(a) if a.ty() == t => Ok(a),
                _ => Err(invalid(format!(
                    "a {} set is written as a JSON array",
                    t.as_str()
                ))),
            },
        },
        _ => match typed(&j, 0) {
            Some(a) => Ok(a),
            None => content(&j),
        },
    }
}

/// Text typed or staged for a cell of type `hint` (none, or a null or missing cell: a string).
pub fn text_as(text: &str, hint: Option<AttrType>) -> Result<Attr, AppError> {
    match hint {
        None | Some(AttrType::S | AttrType::Null) => Ok(Attr::S(text.to_string())),
        Some(AttrType::N) => Ok(Attr::N(check_number(text.trim())?)),
        Some(AttrType::B) => unb64(text)
            .map(Attr::B)
            .ok_or_else(|| invalid("a binary (B) value is written in base64")),
        Some(AttrType::Bool) => match text.trim().to_ascii_lowercase().as_str() {
            "true" => Ok(Attr::Bool(true)),
            "false" => Ok(Attr::Bool(false)),
            _ => Err(invalid(format!(
                "\u{201c}{text}\u{201d} is not true or false"
            ))),
        },
        Some(t) => json_as(text, Some(t)),
    }
}

/// The shortest text of a finite float DynamoDB reads back as the same number.
fn float_text(f: f64) -> Result<String, AppError> {
    if !f.is_finite() {
        return Err(invalid(format!("{f} is not a number DynamoDB can store")));
    }
    if f.fract() == 0.0 && f.abs() < 1e21 {
        Ok(format!("{f:.0}"))
    } else {
        let plain = f.to_string();
        Ok(if plain.len() <= 24 {
            plain
        } else {
            format!("{f:e}")
        })
    }
}

/// The attribute to write for `v` in a cell of type `hint` (the type it had when loaded, else
/// its column's); `None` removes the attribute (`Other{missing}`).
pub fn from_value(v: &Value, hint: Option<AttrType>) -> Result<Option<Attr>, AppError> {
    let text = |s: &str| text_as(s, hint).map(Some);
    match v {
        Value::Null => Ok(Some(Attr::Null)),
        Value::Default => Err(invalid("DynamoDB has no default values")),
        Value::Bool(b) => Ok(Some(Attr::Bool(*b))),
        Value::Int(n) => Ok(Some(Attr::N(n.to_string()))),
        Value::BigInt(s) | Value::Decimal(s) => Ok(Some(Attr::N(check_number(s.trim())?))),
        Value::Float(f) => Ok(Some(Attr::N(float_text(*f)?))),
        Value::Bytes(b) if b.truncated => Err(invalid(
            "the binary value was cut for display; load the full value before editing it",
        )),
        Value::Bytes(b) => unb64(&b.base64)
            .map(|bytes| Some(Attr::B(bytes)))
            .ok_or_else(|| invalid("a binary value is not base64")),
        Value::Json(s) => json_as(s, hint).map(Some),
        Value::Array(items) => array(items, hint).map(Some),
        Value::Text(s)
        | Value::Date(s)
        | Value::Time(s)
        | Value::Timestamp(s)
        | Value::TimestampTz(s)
        | Value::Interval(s)
        | Value::Uuid(s)
        | Value::Enum(s) => text(s),
        Value::Other(o) if o.ty == TY_MISSING => Ok(None),
        Value::Other(o) if o.ty == TY_PREVIEW => Err(invalid(
            "the value was cut for display; load the full value before editing it",
        )),
        Value::Other(o) if o.ty == TY_TEXT => text(&o.text),
        Value::Other(o) => match AttrType::parse(&o.ty) {
            Some(t) => text_as(&o.text, Some(t)).map(Some),
            None => Err(invalid(format!(
                "a {} value cannot be written to DynamoDB",
                o.ty
            ))),
        },
    }
}

/// An `Array` value: a set of the hinted (or its members') type, or a list.
fn array(items: &[Value], hint: Option<AttrType>) -> Result<Attr, AppError> {
    let members = items
        .iter()
        .map(|i| {
            from_value(i, None)?.ok_or_else(|| invalid("a list or set member cannot be missing"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let ty = match hint {
        Some(t) if t.is_set() || t == AttrType::L => t,
        _ => {
            let first = members.first().map(Attr::ty);
            match first {
                Some(t @ (AttrType::S | AttrType::N | AttrType::B))
                    if members.iter().all(|m| m.ty() == t) =>
                {
                    match t {
                        AttrType::S => AttrType::Ss,
                        AttrType::N => AttrType::Ns,
                        _ => AttrType::Bs,
                    }
                }
                _ => AttrType::L,
            }
        }
    };
    if ty == AttrType::L {
        Ok(Attr::L(members))
    } else {
        make_set(ty, members)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn round_trip(j: serde_json::Value) {
        let a = Attr::from_json(&j).unwrap();
        assert_eq!(a.to_json(), j);
    }

    #[test]
    fn every_type_round_trips_as_json() {
        round_trip(json!({"S": "02134"}));
        round_trip(json!({"N": "12345678901234567890123456789012345678"}));
        round_trip(json!({"N": "-1.5E-130"}));
        round_trip(json!({"B": "AAEC/w=="}));
        round_trip(json!({"BOOL": false}));
        round_trip(json!({"NULL": true}));
        round_trip(json!({"M": {"a": {"N": "1"}, "b": {"L": [{"S": "x"}, {"NULL": true}]}}}));
        round_trip(json!({"SS": ["a", "b"]}));
        round_trip(json!({"NS": ["1", "2.5"]}));
        round_trip(json!({"BS": ["AA==", "AQ=="]}));
    }

    #[test]
    fn bad_replies_are_errors_not_panics() {
        for bad in [
            json!("x"),
            json!({}),
            json!({"S": "a", "N": "1"}),
            json!({"S": 1}),
            json!({"N": "1x"}),
            json!({"N": "123456789012345678901234567890123456789"}),
            json!({"B": "%%"}),
            json!({"NULL": false}),
            json!({"BOOL": "true"}),
            json!({"M": []}),
            json!({"L": {}}),
            json!({"SS": [1]}),
            json!({"NS": ["x"]}),
            json!({"X": "?"}),
        ] {
            assert!(Attr::from_json(&bad).is_err(), "{bad}");
        }
        let mut deep = json!({"S": "x"});
        for _ in 0..(MAX_DEPTH + 5) {
            deep = json!({ "L": [deep] });
        }
        assert!(Attr::from_json(&deep).is_err());
        assert!(item_from_json(&json!([])).is_err());
    }

    #[test]
    fn numbers() {
        assert_eq!(number_value("42"), Value::Int(42));
        assert_eq!(
            number_value("-9007199254740991"),
            Value::Int(-9_007_199_254_740_991)
        );
        assert_eq!(
            number_value("9007199254740993"),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            number_value("123456789012345678901234567890"),
            Value::BigInt("123456789012345678901234567890".into())
        );
        assert_eq!(number_value("1.50"), Value::Decimal("1.50".into()));
        assert_eq!(number_value("1E+2"), Value::Decimal("1E+2".into()));
        assert_eq!(number_value("007"), Value::Decimal("007".into()));
        for ok in [
            "0",
            "-0.5",
            ".5",
            "5.",
            "+1",
            "1e-130",
            "1000000000000000000000000000000000000000000",
        ] {
            assert!(check_number(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", "-", ".", "1e", "1e+", "0x10", "1 ", "NaN", "Infinity", "1.2.3", "１",
        ] {
            assert!(check_number(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn grid_values() {
        let item = item_from_json(&json!({
            "s": {"S": "02134"}, "n": {"N": "7"}, "b": {"B": "AQI="}, "t": {"BOOL": true},
            "z": {"NULL": true}, "m": {"M": {"k": {"N": "1.0"}}}, "l": {"L": [{"S": "x"}]},
            "ss": {"SS": ["a"]}, "ns": {"NS": ["1", "2.5"]}, "bs": {"BS": ["AQ=="]}
        }))
        .unwrap();
        assert_eq!(cell(&item, "s", Cap::Grid), Value::Text("02134".into()));
        assert_eq!(cell(&item, "n", Cap::Grid), Value::Int(7));
        assert!(
            matches!(cell(&item, "b", Cap::Grid), Value::Bytes(ref b) if b.base64 == "AQI=" && b.len == 2)
        );
        assert_eq!(cell(&item, "t", Cap::Grid), Value::Bool(true));
        assert_eq!(cell(&item, "z", Cap::Grid), Value::Null);
        assert_eq!(
            cell(&item, "m", Cap::Grid),
            Value::Json(r#"{"k":{"N":"1.0"}}"#.into())
        );
        assert_eq!(
            cell(&item, "l", Cap::Grid),
            Value::Json(r#"[{"S":"x"}]"#.into())
        );
        assert_eq!(
            cell(&item, "ss", Cap::Grid),
            Value::Array(vec![Value::Text("a".into())])
        );
        assert_eq!(
            cell(&item, "ns", Cap::Grid),
            Value::Array(vec![Value::Int(1), Value::Decimal("2.5".into())])
        );
        assert!(is_missing(&cell(&item, "nope", Cap::Grid)));
        let big = Attr::L(vec![Attr::S("x".repeat(JSON_CELL_CAP)); 2]);
        assert!(matches!(to_value(&big, Cap::Grid), Value::Other(ref o) if o.ty == TY_PREVIEW));
        assert!(matches!(to_value(&big, Cap::Full), Value::Json(_)));
    }

    /// Sizes are at least the memory taken: a tree node for every map however small, the
    /// values' own room in lists and sets, every text with its allocation.
    #[test]
    fn sizes_count_every_nested_value() {
        assert_eq!(VALUE, 32);
        assert_eq!(item_size(&Item::new()), 0);
        let small = item_from_json(&json!({"a": {"S": "xy"}})).unwrap();
        assert_eq!(item_size(&small), alloc(LEAF) + alloc(1) + alloc(2));
        assert!(item_size(&small) >= LEAF + 2 * 16);
        let nested = item_from_json(&json!({
            "m": {"M": {"k": {"L": [{"N": "123"}, {"B": "AAEC"}, {"NULL": true}]}}},
            "ss": {"SS": ["a", "bc"]},
        }))
        .unwrap();
        let list = alloc(6 * VALUE) + alloc(3) + alloc(3);
        let m = alloc(LEAF) + alloc(1) + list;
        let ss = alloc(4 * STRING) + alloc(1) + alloc(2);
        assert_eq!(
            item_size(&nested),
            alloc(LEAF) + alloc(1) + m + alloc(2) + ss
        );
        // 30 maps of one entry each: 30 nodes.
        let mut deep = json!({"NULL": true});
        for _ in 0..30 {
            deep = json!({"M": {"a": deep}});
        }
        let deep = item_from_json(&json!({ "a": deep })).unwrap();
        assert!(item_size(&deep) >= 31 * LEAF);
        // Wide maps: a node per 5 entries at the most.
        let wide: Item = (0..1000).map(|i| (format!("a{i}"), Attr::Null)).collect();
        assert!(item_size(&wide) >= 1000 * (STRING + VALUE));
        assert!(item_size(&wide) <= 1000 * (LEAF / 5 + 2 * LEAF / 25 + 48));
    }

    #[test]
    fn a_set_twice_a_member_is_refused_however_large() {
        let members: Vec<String> = (0..50_000).map(|i| i.to_string()).collect();
        assert!(check_set(members.clone(), AttrType::Ss).is_ok());
        let mut twice = members;
        twice.push("7".into());
        assert!(check_set(twice, AttrType::Ss).is_err());
    }

    /// Decoding then encoding with the attribute's own type gives the attribute back.
    #[test]
    fn values_round_trip_with_their_type() {
        let attrs = [
            Attr::S("02134".into()),
            Attr::S("".into()),
            Attr::N("12345678901234567890.123456789012345678".into()),
            Attr::N("9007199254740993".into()),
            Attr::N("-3".into()),
            Attr::B(vec![0, 1, 255]),
            Attr::Bool(true),
            Attr::Null,
            Attr::M(BTreeMap::from([
                ("S".to_string(), Attr::S("x".into())),
                (
                    "n".to_string(),
                    Attr::N("1.000000000000000000000000000001".into()),
                ),
                ("set".to_string(), Attr::Ss(vec!["a".into()])),
                ("bin".to_string(), Attr::B(vec![9])),
            ])),
            Attr::L(vec![
                Attr::N("1".into()),
                Attr::S("1".into()),
                Attr::Bs(vec![vec![1]]),
            ]),
            Attr::M(BTreeMap::new()),
            Attr::L(vec![]),
            Attr::Ss(vec!["b".into(), "a".into()]),
            Attr::Ns(vec!["1".into(), "2.50".into()]),
            Attr::Bs(vec![vec![1], vec![2]]),
        ];
        for a in attrs {
            let v = to_value(&a, Cap::Full);
            let back = from_value(&v, Some(a.ty())).unwrap().unwrap();
            assert_eq!(back, a, "{v:?}");
            // Text staged by the cell editor reads back the same.
            if let Some(t) = v
                .as_text()
                .filter(|_| !matches!(v, Value::Array(_) | Value::Bytes(_)))
            {
                assert_eq!(text_as(&t, Some(a.ty())).unwrap(), a, "{t}");
            }
        }
    }

    #[test]
    fn hints_decide_the_type_of_typed_text() {
        assert_eq!(
            text_as("02134", Some(AttrType::S)).unwrap(),
            Attr::S("02134".into())
        );
        assert_eq!(text_as("02134", None).unwrap(), Attr::S("02134".into()));
        assert_eq!(
            text_as(" 5 ", Some(AttrType::N)).unwrap(),
            Attr::N("5".into())
        );
        assert!(text_as("five", Some(AttrType::N)).is_err());
        assert_eq!(
            text_as("abc", Some(AttrType::Null)).unwrap(),
            Attr::S("abc".into())
        );
        assert_eq!(
            text_as("TRUE", Some(AttrType::Bool)).unwrap(),
            Attr::Bool(true)
        );
        assert!(text_as("yes", Some(AttrType::Bool)).is_err());
        assert_eq!(
            text_as(r#"["a","b"]"#, Some(AttrType::Ss)).unwrap(),
            Attr::Ss(vec!["a".into(), "b".into()])
        );
        assert_eq!(
            text_as(r#"[1, "2.5"]"#, Some(AttrType::Ns)).unwrap(),
            Attr::Ns(vec!["1".into(), "2.5".into()])
        );
        assert!(text_as("[]", Some(AttrType::Ss)).is_err());
        assert!(text_as(r#"["a","a"]"#, Some(AttrType::Ss)).is_err());
        assert!(text_as(r#"{"a":1}"#, Some(AttrType::L)).is_err());
        // Plain JSON in a map cell: numbers keep their digits.
        assert_eq!(
            text_as(
                r#"{"a": 1.000000000000000000000000000001, "b": [true, null, "x"]}"#,
                Some(AttrType::M)
            )
            .unwrap(),
            Attr::M(BTreeMap::from([
                (
                    "a".to_string(),
                    Attr::N("1.000000000000000000000000000001".into())
                ),
                (
                    "b".to_string(),
                    Attr::L(vec![Attr::Bool(true), Attr::Null, Attr::S("x".into())])
                ),
            ]))
        );
        // A map whose values are all typed is DynamoDB JSON.
        assert_eq!(
            text_as(r#"{"a": {"N": "1"}}"#, Some(AttrType::M)).unwrap(),
            Attr::M(BTreeMap::from([("a".to_string(), Attr::N("1".into()))]))
        );
        // A map cell holds content: a key named like a type is a key.
        assert_eq!(
            text_as(r#"{"M": {"M": {"a": {"S": "x"}}}}"#, Some(AttrType::M)).unwrap(),
            Attr::M(BTreeMap::from([(
                "M".to_string(),
                Attr::M(BTreeMap::from([("a".to_string(), Attr::S("x".into()))]))
            )]))
        );
        // Without a hint, a typed value says its type.
        assert_eq!(
            from_value(&Value::Json(r#"{"SS":["x"]}"#.into()), None).unwrap(),
            Some(Attr::Ss(vec!["x".into()]))
        );
        assert_eq!(
            from_value(&Value::Json(r#"{"N":"2"}"#.into()), None).unwrap(),
            Some(Attr::N("2".into()))
        );
    }

    #[test]
    fn value_variants() {
        assert_eq!(from_value(&missing(), Some(AttrType::S)).unwrap(), None);
        assert_eq!(
            from_value(&Value::Null, Some(AttrType::S)).unwrap(),
            Some(Attr::Null)
        );
        assert!(from_value(&Value::Default, None).is_err());
        assert_eq!(
            from_value(&Value::Float(2.0), None).unwrap(),
            Some(Attr::N("2".into()))
        );
        assert_eq!(
            from_value(&Value::Float(0.1), None).unwrap(),
            Some(Attr::N("0.1".into()))
        );
        assert!(from_value(&Value::Float(f64::NAN), None).is_err());
        assert_eq!(
            from_value(&Value::Decimal("1e2".into()), Some(AttrType::S)).unwrap(),
            Some(Attr::N("1e2".into()))
        );
        assert_eq!(
            from_value(&Value::Uuid("u".into()), None).unwrap(),
            Some(Attr::S("u".into()))
        );
        assert_eq!(
            from_value(&Value::other(TY_TEXT, "9"), Some(AttrType::N)).unwrap(),
            Some(Attr::N("9".into()))
        );
        assert_eq!(
            from_value(&Value::other("NS", "[\"1\"]"), None).unwrap(),
            Some(Attr::Ns(vec!["1".into()]))
        );
        assert!(from_value(&Value::other(TY_PREVIEW, "{…"), None).is_err());
        assert!(from_value(&Value::other("objectId", "x"), None).is_err());
        let cut = Value::Bytes(crate::types::value::BytesValue {
            base64: "AA==".into(),
            len: 99,
            truncated: true,
        });
        assert!(from_value(&cut, None).is_err());
        // Arrays: sets by their members, or by the hint; mixed members a list.
        assert_eq!(
            from_value(
                &Value::Array(vec![Value::Int(1), Value::Decimal("2.5".into())]),
                None
            )
            .unwrap(),
            Some(Attr::Ns(vec!["1".into(), "2.5".into()]))
        );
        assert_eq!(
            from_value(
                &Value::Array(vec![Value::Int(1), Value::Text("a".into())]),
                None
            )
            .unwrap(),
            Some(Attr::L(vec![Attr::N("1".into()), Attr::S("a".into())]))
        );
        assert_eq!(
            from_value(
                &Value::Array(vec![Value::Text("1".into())]),
                Some(AttrType::Ns)
            )
            .unwrap(),
            Some(Attr::Ns(vec!["1".into()]))
        );
        assert!(from_value(&Value::Array(vec![]), Some(AttrType::Ss)).is_err());
        assert!(from_value(&Value::Array(vec![missing()]), None).is_err());
    }

    #[test]
    fn json_reader() {
        assert_eq!(
            parse_json(r#" {"a": [1, -2.5e3, "x\u00e9\ud83d\ude00\n"], "b": {}} "#).unwrap(),
            Json::Obj(vec![
                (
                    "a".into(),
                    Json::Arr(vec![
                        Json::Num("1".into()),
                        Json::Num("-2.5e3".into()),
                        Json::Str("xé😀\n".into())
                    ])
                ),
                ("b".into(), Json::Obj(vec![]))
            ])
        );
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\":1,\"a\":2}",
            "01",
            "1.",
            "-",
            "\"\\ud800\"",
            "\"a",
            "tru",
            "{1:2}",
            "[1] x",
            "\"\u{1}\"",
            "1e",
            "\"\\x\"",
        ] {
            assert!(parse_json(bad).is_err(), "{bad}");
        }
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));
        assert!(parse_json(&deep).is_err());
    }
}
