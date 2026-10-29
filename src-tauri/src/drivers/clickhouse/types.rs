//! ClickHouse type names (`Nullable(LowCardinality(String))`, `DateTime64(3, 'Asia/Jakarta')`,
//! `Map(String, Array(Tuple(a UInt8, b Enum8('x' = 1))))`…) parsed into `ChType`, and what the
//! grid makes of them (`ValueKind`, nullability, enum labels).
//!
//! The parser is total: a type it cannot read becomes `ChType::Other` with its text, never a
//! panic, and nesting is bounded. Most of TablePlus's ClickHouse crashes were type names its
//! decoder did not expect (LowCardinality(Nullable), Map, Nested with an Enum, UInt256…).

use crate::types::schema::ValueKind;

/// How deep types may nest before the rest is kept as text.
const MAX_DEPTH: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChType {
    Nullable(Box<ChType>),
    LowCardinality(Box<ChType>),
    /// `Int8` … `Int256`, `UInt8` … `UInt256`.
    Int {
        bits: u16,
        signed: bool,
    },
    /// `Float32`, `Float64`, `BFloat16`.
    Float {
        bits: u8,
    },
    Decimal {
        precision: u8,
        scale: u8,
    },
    Bool,
    String,
    FixedString(u32),
    Uuid,
    Date,
    Date32,
    DateTime {
        tz: Option<String>,
    },
    DateTime64 {
        precision: u8,
        tz: Option<String>,
    },
    /// `Time`, `Time64(p)`.
    Time {
        precision: Option<u8>,
    },
    /// `Enum8` / `Enum16`: labels and their numbers, as declared.
    Enum {
        bits: u8,
        values: Vec<(String, i64)>,
    },
    Ipv4,
    Ipv6,
    Array(Box<ChType>),
    /// Elements with their names (a named tuple) or without.
    Tuple(Vec<(Option<String>, ChType)>),
    Map(Box<ChType>, Box<ChType>),
    Nested(Vec<(String, ChType)>),
    /// `JSON(…)`, `Object('json')`.
    Json,
    Dynamic,
    Variant(Vec<ChType>),
    /// `IntervalDay`… (the unit).
    Interval(String),
    /// `Point`, `Ring`, `LineString`, `MultiLineString`, `Polygon`, `MultiPolygon`, `Geometry`.
    Geo(String),
    /// `SimpleAggregateFunction(f, T)`: stored and sent as `T`.
    SimpleAggregate(String, Box<ChType>),
    /// `AggregateFunction(…)`: a binary state the grid cannot show (see `finalizeAggregation`).
    AggregateFunction(String),
    Nothing,
    /// Anything else, as written.
    Other(String),
}

impl ChType {
    /// Parses a type name; never fails.
    pub fn parse(text: &str) -> ChType {
        let tokens = tokenize(text);
        let mut p = Parser {
            tokens: &tokens,
            at: 0,
        };
        match p.ty(0) {
            Some(t) if p.at == tokens.len() => t,
            _ => ChType::Other(text.trim().to_string()),
        }
    }

    /// The type with `Nullable`, `LowCardinality` and `SimpleAggregateFunction` taken off.
    pub fn base(&self) -> &ChType {
        match self {
            ChType::Nullable(t) | ChType::LowCardinality(t) | ChType::SimpleAggregate(_, t) => {
                t.base()
            }
            t => t,
        }
    }

    /// Whether a value of this type may be NULL.
    pub fn is_nullable(&self) -> bool {
        match self {
            ChType::Nullable(_) => true,
            ChType::LowCardinality(t) | ChType::SimpleAggregate(_, t) => t.is_nullable(),
            // Dynamic and Variant hold NULL without a Nullable around them.
            ChType::Dynamic | ChType::Variant(_) | ChType::Nothing => true,
            _ => false,
        }
    }

    pub fn kind(&self) -> ValueKind {
        match self.base() {
            ChType::Int { bits, .. } if *bits <= 32 => ValueKind::Int,
            ChType::Int { .. } => ValueKind::BigInt,
            ChType::Float { .. } => ValueKind::Float,
            ChType::Decimal { .. } => ValueKind::Decimal,
            ChType::Bool => ValueKind::Bool,
            ChType::String | ChType::FixedString(_) => ValueKind::Text,
            ChType::Uuid => ValueKind::Uuid,
            ChType::Date | ChType::Date32 => ValueKind::Date,
            ChType::DateTime { .. } | ChType::DateTime64 { .. } => ValueKind::Timestamp,
            ChType::Time { .. } => ValueKind::Time,
            ChType::Enum { .. } => ValueKind::Enum,
            ChType::Array(_) => ValueKind::Array,
            ChType::Tuple(_)
            | ChType::Map(..)
            | ChType::Nested(_)
            | ChType::Json
            | ChType::Dynamic
            | ChType::Variant(_) => ValueKind::Json,
            // Intervals come as bare numbers, their unit only in the type.
            ChType::Interval(_) => ValueKind::Other,
            ChType::Geo(_) => ValueKind::Geometry,
            ChType::Ipv4
            | ChType::Ipv6
            | ChType::AggregateFunction(_)
            | ChType::Nothing
            | ChType::Other(_)
            | ChType::Nullable(_)
            | ChType::LowCardinality(_)
            | ChType::SimpleAggregate(..) => ValueKind::Other,
        }
    }

    /// The labels of an enum, in declaration order.
    pub fn enum_labels(&self) -> Option<Vec<String>> {
        match self.base() {
            ChType::Enum { values, .. } => Some(values.iter().map(|(l, _)| l.clone()).collect()),
            _ => None,
        }
    }

    pub fn is_unsigned(&self) -> bool {
        matches!(self.base(), ChType::Int { signed: false, .. })
    }

    /// `(precision, scale)` of a decimal.
    pub fn decimal(&self) -> Option<(u8, u8)> {
        match self.base() {
            ChType::Decimal { precision, scale } => Some((*precision, *scale)),
            _ => None,
        }
    }

    /// A state the grid cannot show as it is (`AggregateFunction`).
    pub fn is_aggregate_state(&self) -> bool {
        matches!(self.base(), ChType::AggregateFunction(_))
    }

    /// The short name used in `Value::Other` (`IPv4`, `Point`, `Float64`…).
    pub fn label(&self) -> String {
        match self.base() {
            ChType::Int { bits, signed } => {
                format!("{}Int{bits}", if *signed { "" } else { "U" })
            }
            ChType::Float { bits: 16 } => "BFloat16".into(),
            ChType::Float { bits } => format!("Float{bits}"),
            ChType::Decimal { .. } => "Decimal".into(),
            ChType::Bool => "Bool".into(),
            ChType::String => "String".into(),
            ChType::FixedString(_) => "FixedString".into(),
            ChType::Uuid => "UUID".into(),
            ChType::Date => "Date".into(),
            ChType::Date32 => "Date32".into(),
            ChType::DateTime { .. } => "DateTime".into(),
            ChType::DateTime64 { .. } => "DateTime64".into(),
            ChType::Time { precision: None } => "Time".into(),
            ChType::Time { .. } => "Time64".into(),
            ChType::Enum { bits, .. } => format!("Enum{bits}"),
            ChType::Ipv4 => "IPv4".into(),
            ChType::Ipv6 => "IPv6".into(),
            ChType::Array(_) => "Array".into(),
            ChType::Tuple(_) => "Tuple".into(),
            ChType::Map(..) => "Map".into(),
            ChType::Nested(_) => "Nested".into(),
            ChType::Json => "JSON".into(),
            ChType::Dynamic => "Dynamic".into(),
            ChType::Variant(_) => "Variant".into(),
            ChType::Interval(unit) => format!("Interval{unit}"),
            ChType::Geo(name) => name.clone(),
            ChType::AggregateFunction(_) => "AggregateFunction".into(),
            ChType::Nothing => "Nothing".into(),
            ChType::Other(text) => text.split('(').next().unwrap_or(text).trim().to_string(),
            ChType::Nullable(_) | ChType::LowCardinality(_) | ChType::SimpleAggregate(..) => {
                "Other".into()
            }
        }
    }
}

// --- Tokens ----------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// A name or a number (`UInt8`, `3`, `-1` comes as `Punct('-')` then `3`).
    Word(String),
    /// A quoted string (`'UTC'`), unescaped.
    Str(String),
    /// A quoted name (`` `a b` `` in a named tuple), unescaped.
    Quoted(String),
    Punct(char),
    /// Text the tokenizer could not read (an unterminated quote): the parse fails.
    Bad,
}

fn tokenize(text: &str) -> Vec<Tok> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_alphanumeric() || c == '_' || c == '.' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.')
            {
                i += 1;
            }
            out.push(Tok::Word(chars[start..i].iter().collect()));
        } else if c == '\'' || c == '`' || c == '"' {
            match quoted(&chars, i) {
                Some((value, next)) => {
                    out.push(if c == '\'' {
                        Tok::Str(value)
                    } else {
                        Tok::Quoted(value)
                    });
                    i = next;
                }
                None => {
                    out.push(Tok::Bad);
                    break;
                }
            }
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    out
}

/// The text of the quoted token at `start` (backslash escapes and the quote doubled) and the
/// index after it; None when it does not end.
fn quoted(chars: &[char], start: usize) -> Option<(String, usize)> {
    let q = chars[start];
    let mut out = String::new();
    let mut i = start + 1;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            let next = *chars.get(i + 1)?;
            out.push(match next {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '0' => '\0',
                other => other,
            });
            i += 2;
        } else if c == q {
            if chars.get(i + 1) == Some(&q) {
                out.push(q);
                i += 2;
            } else {
                return Some((out, i + 1));
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    None
}

// --- Parser ----------------------------------------------------------------------------------

struct Parser<'t> {
    tokens: &'t [Tok],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.at)
    }

    fn punct(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Punct(c)) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn word(&mut self) -> Option<String> {
        match self.peek()? {
            Tok::Word(w) => {
                let w = w.clone();
                self.at += 1;
                Some(w)
            }
            _ => None,
        }
    }

    fn number(&mut self) -> Option<i64> {
        let negative = self.punct('-');
        let w = self.word()?;
        let n: i64 = w.parse().ok()?;
        Some(if negative { -n } else { n })
    }

    fn string(&mut self) -> Option<String> {
        match self.peek()? {
            Tok::Str(s) => {
                let s = s.clone();
                self.at += 1;
                Some(s)
            }
            _ => None,
        }
    }

    /// The text of the tokens from `from` to the current position, for `Other` and
    /// `AggregateFunction` (spacing normalized).
    fn text_since(&self, from: usize) -> String {
        let mut out = String::new();
        for t in &self.tokens[from..self.at] {
            match t {
                Tok::Word(w) => {
                    if out.ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                        out.push(' ');
                    }
                    out.push_str(w);
                }
                Tok::Str(s) => {
                    out.push('\'');
                    out.push_str(&s.replace('\\', "\\\\").replace('\'', "\\'"));
                    out.push('\'');
                }
                Tok::Quoted(s) => {
                    out.push('`');
                    out.push_str(&s.replace('\\', "\\\\").replace('`', "\\`"));
                    out.push('`');
                }
                Tok::Punct(',') => out.push_str(", "),
                Tok::Punct(c) => out.push(*c),
                Tok::Bad => {}
            }
        }
        out
    }

    /// Skips balanced arguments (after the `(` was read) up to and including their `)`.
    fn skip_args(&mut self) -> Option<()> {
        let mut depth = 1usize;
        while depth > 0 {
            match self.peek()? {
                Tok::Punct('(') => depth += 1,
                Tok::Punct(')') => depth -= 1,
                Tok::Bad => return None,
                _ => {}
            }
            self.at += 1;
        }
        Some(())
    }

    fn ty(&mut self, depth: usize) -> Option<ChType> {
        if depth > MAX_DEPTH {
            return None;
        }
        let start = self.at;
        let name = self.word()?;
        let has_args = self.punct('(');
        let lower = name.to_ascii_lowercase();
        let t = match (lower.as_str(), has_args) {
            ("nullable", true) => ChType::Nullable(Box::new(self.inner(depth)?)),
            ("lowcardinality", true) => ChType::LowCardinality(Box::new(self.inner(depth)?)),
            ("array", true) => ChType::Array(Box::new(self.inner(depth)?)),
            ("map", true) => {
                let k = self.ty(depth + 1)?;
                if !self.punct(',') {
                    return None;
                }
                let v = self.ty(depth + 1)?;
                self.close()?;
                ChType::Map(Box::new(k), Box::new(v))
            }
            ("tuple", true) => ChType::Tuple(self.elements(depth)?),
            ("tuple", false) => ChType::Tuple(Vec::new()),
            ("nested", true) => {
                let elements = self.elements(depth)?;
                let named: Option<Vec<(String, ChType)>> =
                    elements.into_iter().map(|(n, t)| Some((n?, t))).collect();
                ChType::Nested(named?)
            }
            ("variant", true) => {
                let mut items = vec![self.ty(depth + 1)?];
                while self.punct(',') {
                    items.push(self.ty(depth + 1)?);
                }
                self.close()?;
                ChType::Variant(items)
            }
            ("simpleaggregatefunction", true) => {
                let f = self.text_of_function()?;
                if !self.punct(',') {
                    return None;
                }
                let t = self.ty(depth + 1)?;
                self.close()?;
                ChType::SimpleAggregate(f, Box::new(t))
            }
            ("aggregatefunction", true) => {
                self.skip_args()?;
                ChType::AggregateFunction(self.text_since(start))
            }
            ("decimal" | "numeric" | "dec", _) => {
                if has_args {
                    let p = self.number()?;
                    let s = if self.punct(',') { self.number()? } else { 0 };
                    self.close()?;
                    decimal(p, s)?
                } else {
                    decimal(10, 0)?
                }
            }
            ("decimal32" | "decimal64" | "decimal128" | "decimal256", true) => {
                let s = self.number()?;
                self.close()?;
                let p = match lower.as_str() {
                    "decimal32" => 9,
                    "decimal64" => 18,
                    "decimal128" => 38,
                    _ => 76,
                };
                decimal(p, s)?
            }
            ("datetime", _) => {
                let tz = if has_args {
                    let tz = self.string()?;
                    self.close()?;
                    Some(tz)
                } else {
                    None
                };
                ChType::DateTime { tz }
            }
            ("datetime64", true) => {
                let p = self.number()?;
                let tz = if self.punct(',') {
                    Some(self.string()?)
                } else {
                    None
                };
                self.close()?;
                ChType::DateTime64 {
                    precision: u8::try_from(p).ok().filter(|p| *p <= 9)?,
                    tz,
                }
            }
            ("time64", true) => {
                let p = self.number()?;
                self.close()?;
                ChType::Time {
                    precision: Some(u8::try_from(p).ok().filter(|p| *p <= 9)?),
                }
            }
            ("enum8" | "enum16" | "enum", true) => {
                let mut values = Vec::new();
                let mut next = 1i64;
                loop {
                    let label = self.string()?;
                    let n = if self.punct('=') {
                        self.number()?
                    } else {
                        next
                    };
                    next = n.saturating_add(1);
                    values.push((label, n));
                    if !self.punct(',') {
                        break;
                    }
                }
                self.close()?;
                let bits = if lower == "enum8" { 8 } else { 16 };
                ChType::Enum { bits, values }
            }
            ("fixedstring", true) => {
                let n = self.number()?;
                self.close()?;
                ChType::FixedString(u32::try_from(n).ok()?)
            }
            ("json" | "object" | "dynamic" | "qbit", true) => {
                self.skip_args()?;
                match lower.as_str() {
                    "json" | "object" => ChType::Json,
                    "dynamic" => ChType::Dynamic,
                    _ => ChType::Other(self.text_since(start)),
                }
            }
            (_, true) => {
                // An unknown parametric type: kept whole.
                self.skip_args()?;
                ChType::Other(self.text_since(start))
            }
            (_, false) => simple(&name),
        };
        Some(t)
    }

    fn inner(&mut self, depth: usize) -> Option<ChType> {
        let t = self.ty(depth + 1)?;
        self.close()?;
        Some(t)
    }

    fn close(&mut self) -> Option<()> {
        self.punct(')').then_some(())
    }

    /// The elements of a `Tuple(…)` or `Nested(…)`: `T` or `name T`, the name possibly quoted.
    fn elements(&mut self, depth: usize) -> Option<Vec<(Option<String>, ChType)>> {
        let mut out = Vec::new();
        loop {
            let name = match (self.tokens.get(self.at), self.tokens.get(self.at + 1)) {
                (Some(Tok::Quoted(n)), _) => {
                    let n = n.clone();
                    self.at += 1;
                    Some(n)
                }
                // `name Type`: a word followed by another word.
                (Some(Tok::Word(n)), Some(Tok::Word(_))) => {
                    let n = n.clone();
                    self.at += 1;
                    Some(n)
                }
                _ => None,
            };
            out.push((name, self.ty(depth + 1)?));
            if !self.punct(',') {
                break;
            }
        }
        self.close()?;
        Some(out)
    }

    /// The aggregate function of a `SimpleAggregateFunction` (`sum`, `anyLast`,
    /// `groupArrayArray`, `sumMap`…), possibly with parameters.
    fn text_of_function(&mut self) -> Option<String> {
        let start = self.at;
        self.word()?;
        if self.punct('(') {
            self.skip_args()?;
        }
        Some(self.text_since(start))
    }
}

fn decimal(p: i64, s: i64) -> Option<ChType> {
    let precision = u8::try_from(p).ok().filter(|p| (1..=76).contains(p))?;
    let scale = u8::try_from(s).ok().filter(|s| *s <= precision)?;
    Some(ChType::Decimal { precision, scale })
}

/// A type without arguments, by its name (case-insensitive aliases included).
fn simple(name: &str) -> ChType {
    let lower = name.to_ascii_lowercase();
    let int = |bits: u16, signed: bool| ChType::Int { bits, signed };
    match lower.as_str() {
        "int8" | "tinyint" | "int1" | "byte" => int(8, true),
        "int16" | "smallint" | "int2" => int(16, true),
        "int32" | "int" | "integer" | "int4" => int(32, true),
        "int64" | "bigint" | "int8_t" => int(64, true),
        "int128" => int(128, true),
        "int256" => int(256, true),
        "uint8" => int(8, false),
        "uint16" => int(16, false),
        "uint32" => int(32, false),
        "uint64" => int(64, false),
        "uint128" => int(128, false),
        "uint256" => int(256, false),
        "float32" | "float" | "real" | "single" => ChType::Float { bits: 32 },
        "float64" | "double" => ChType::Float { bits: 64 },
        "bfloat16" => ChType::Float { bits: 16 },
        "bool" | "boolean" => ChType::Bool,
        "string" | "text" | "varchar" | "char" | "blob" | "longtext" => ChType::String,
        "uuid" => ChType::Uuid,
        "date" => ChType::Date,
        "date32" => ChType::Date32,
        "datetime" | "timestamp" => ChType::DateTime { tz: None },
        "time" => ChType::Time { precision: None },
        "ipv4" => ChType::Ipv4,
        "ipv6" => ChType::Ipv6,
        "json" => ChType::Json,
        "dynamic" => ChType::Dynamic,
        "nothing" => ChType::Nothing,
        "point" | "ring" | "linestring" | "multilinestring" | "polygon" | "multipolygon"
        | "geometry" => ChType::Geo(name.to_string()),
        _ => match lower.strip_prefix("interval") {
            Some(unit) if !unit.is_empty() => {
                ChType::Interval(name["Interval".len()..].to_string())
            }
            _ => ChType::Other(name.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(bits: u16, signed: bool) -> ChType {
        ChType::Int { bits, signed }
    }

    #[test]
    fn plain_and_wrapped_types() {
        assert_eq!(ChType::parse("UInt64"), int(64, false));
        assert_eq!(ChType::parse(" Int256 "), int(256, true));
        assert_eq!(
            ChType::parse("Nullable(String)"),
            ChType::Nullable(Box::new(ChType::String))
        );
        // TablePlus #3772 / #3608.
        let lc = ChType::parse("LowCardinality(Nullable(String))");
        assert_eq!(
            lc,
            ChType::LowCardinality(Box::new(ChType::Nullable(Box::new(ChType::String))))
        );
        assert!(lc.is_nullable());
        assert_eq!(lc.base(), &ChType::String);
        assert_eq!(lc.kind(), ValueKind::Text);
        assert_eq!(ChType::parse("Bool").kind(), ValueKind::Bool);
        assert_eq!(ChType::parse("UInt32").kind(), ValueKind::Int);
        assert_eq!(ChType::parse("Int64").kind(), ValueKind::BigInt);
        assert!(ChType::parse("UInt8").is_unsigned());
        assert_eq!(ChType::parse("FixedString(16)"), ChType::FixedString(16));
        assert_eq!(ChType::parse("IPv6").kind(), ValueKind::Other);
        assert_eq!(ChType::parse("IntervalDay"), ChType::Interval("Day".into()));
        assert_eq!(ChType::parse("Point").kind(), ValueKind::Geometry);
        assert_eq!(ChType::parse("Nothing"), ChType::Nothing);
    }

    #[test]
    fn decimals_and_times() {
        assert_eq!(
            ChType::parse("Decimal(38, 10)"),
            ChType::Decimal {
                precision: 38,
                scale: 10
            }
        );
        assert_eq!(ChType::parse("Decimal(9)").decimal(), Some((9, 0)));
        assert_eq!(ChType::parse("Decimal64(4)").decimal(), Some((18, 4)));
        assert_eq!(ChType::parse("Decimal256(70)").decimal(), Some((76, 70)));
        assert!(matches!(ChType::parse("Decimal(80, 2)"), ChType::Other(_)));
        assert_eq!(
            ChType::parse("DateTime('Asia/Jakarta')"),
            ChType::DateTime {
                tz: Some("Asia/Jakarta".into())
            }
        );
        // TablePlus #3604.
        assert_eq!(
            ChType::parse("DateTime64(3, 'Asia/Jakarta')"),
            ChType::DateTime64 {
                precision: 3,
                tz: Some("Asia/Jakarta".into())
            }
        );
        assert_eq!(ChType::parse("DateTime64(6)").kind(), ValueKind::Timestamp);
        assert_eq!(
            ChType::parse("Time64(3)"),
            ChType::Time { precision: Some(3) }
        );
    }

    #[test]
    fn enums_keep_their_labels() {
        let e = ChType::parse("Enum8('a' = 1, 'it\\'s' = -2, 'x=y' = 3)");
        assert_eq!(
            e,
            ChType::Enum {
                bits: 8,
                values: vec![("a".into(), 1), ("it's".into(), -2), ("x=y".into(), 3)]
            }
        );
        assert_eq!(e.enum_labels().unwrap(), ["a", "it's", "x=y"]);
        assert_eq!(e.kind(), ValueKind::Enum);
        // Values may be left out (numbered from 1).
        assert_eq!(
            ChType::parse("Enum('p', 'q')"),
            ChType::Enum {
                bits: 16,
                values: vec![("p".into(), 1), ("q".into(), 2)]
            }
        );
    }

    #[test]
    fn composite_types() {
        // TablePlus #3811 (Map) and #3700 (Nested with an Enum).
        let m = ChType::parse("Map(LowCardinality(String), Array(Nullable(UInt256)))");
        assert_eq!(m.kind(), ValueKind::Json);
        let ChType::Map(k, v) = &m else {
            panic!("{m:?}")
        };
        assert_eq!(k.base(), &ChType::String);
        assert_eq!(
            **v,
            ChType::Array(Box::new(ChType::Nullable(Box::new(int(256, false)))))
        );
        let n = ChType::parse("Nested(id UInt32, kind Enum8('a' = 1, 'b' = 2))");
        let ChType::Nested(fields) = &n else {
            panic!("{n:?}")
        };
        assert_eq!(fields[0], ("id".into(), int(32, false)));
        assert!(matches!(fields[1].1, ChType::Enum { bits: 8, .. }));
        let t = ChType::parse("Tuple(a UInt8, `b c` String, Float64)");
        assert_eq!(
            t,
            ChType::Tuple(vec![
                (Some("a".into()), int(8, false)),
                (Some("b c".into()), ChType::String),
                (None, ChType::Float { bits: 64 }),
            ])
        );
        assert_eq!(
            ChType::parse("Tuple(String, UInt8)").kind(),
            ValueKind::Json
        );
        assert_eq!(
            ChType::parse("Array(Array(String))").kind(),
            ValueKind::Array
        );
        assert_eq!(
            ChType::parse("Variant(String, UInt64)"),
            ChType::Variant(vec![ChType::String, int(64, false)])
        );
        assert_eq!(
            ChType::parse("JSON(max_dynamic_paths = 10, a.b UInt32, SKIP c)"),
            ChType::Json
        );
        assert_eq!(ChType::parse("Object('json')"), ChType::Json);
        assert_eq!(ChType::parse("Dynamic(max_types = 8)"), ChType::Dynamic);
    }

    #[test]
    fn aggregate_types() {
        // TablePlus #3284.
        let s = ChType::parse("SimpleAggregateFunction(anyLast, Nullable(String))");
        assert_eq!(s.base(), &ChType::String);
        assert!(s.is_nullable());
        let q = ChType::parse("SimpleAggregateFunction(quantiles(0.5, 0.9), Array(Float64))");
        assert_eq!(q.kind(), ValueKind::Array);
        // TablePlus #3417.
        let a = ChType::parse("AggregateFunction(uniq, UInt64)");
        assert!(a.is_aggregate_state());
        assert_eq!(a.kind(), ValueKind::Other);
        let ChType::AggregateFunction(text) = a else {
            panic!()
        };
        assert_eq!(text, "AggregateFunction(uniq, UInt64)");
    }

    #[test]
    fn anything_unreadable_is_kept_as_text() {
        for bad in [
            "",
            "(",
            ")",
            "Nullable(",
            "Nullable(String",
            "Map(String)",
            "Enum8('a' = )",
            "DateTime('unterminated",
            "Decimal(x, y)",
            "Array(String) extra",
            "FixedString(-1)",
            "DateTime64(42)",
        ] {
            assert!(matches!(ChType::parse(bad), ChType::Other(_)), "{bad:?}");
        }
        assert_eq!(
            ChType::parse("QBit(Float32, 8)"),
            ChType::Other("QBit(Float32, 8)".into())
        );
        assert_eq!(
            ChType::parse("SomethingNew(1, 'x')"),
            ChType::Other("SomethingNew(1, 'x')".into())
        );
        // Deep nesting stops at the limit instead of overflowing the stack.
        let deep = format!("{}String{}", "Array(".repeat(5000), ")".repeat(5000));
        assert!(matches!(ChType::parse(&deep), ChType::Other(_)));
        let ok = format!("{}String{}", "Array(".repeat(20), ")".repeat(20));
        assert_eq!(ChType::parse(&ok).kind(), ValueKind::Array);
    }
}
