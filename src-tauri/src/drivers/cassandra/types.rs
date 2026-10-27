//! CQL types: the text `system_schema.columns.type` spells (`frozen<map<text, int>>`) and the
//! `ColumnType` of result metadata, each as Tabler's `ValueKind` and as type text.

use ::scylla::frame::response::result::{CollectionType, ColumnType, NativeType};

use crate::types::schema::ValueKind;

/// A CQL type as `system_schema` spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CqlType {
    /// A built-in type, lower case (`text`, `bigint`, …; `varchar` reads as `text`).
    Native(String),
    List(Box<CqlType>),
    Set(Box<CqlType>),
    Map(Box<CqlType>, Box<CqlType>),
    Tuple(Vec<CqlType>),
    Vector(Box<CqlType>, u32),
    Frozen(Box<CqlType>),
    /// A user-defined type, by name (unquoted).
    Udt(String),
    /// `'org.apache.cassandra.db.marshal.…'`, or text this parser does not know.
    Custom(String),
}

const NATIVE: &[&str] = &[
    "ascii",
    "bigint",
    "blob",
    "boolean",
    "counter",
    "date",
    "decimal",
    "double",
    "duration",
    "float",
    "inet",
    "int",
    "smallint",
    "text",
    "time",
    "timestamp",
    "timeuuid",
    "tinyint",
    "uuid",
    "varint",
];

/// Nesting beyond this is refused: no real schema comes close, and the parser recurses.
const MAX_DEPTH: usize = 64;

impl CqlType {
    /// Parses a type text; text it cannot read is `Custom` (a kind of `Other`), never an error.
    pub fn parse(text: &str) -> CqlType {
        let mut p = Parser {
            chars: text.chars().collect(),
            at: 0,
        };
        match p.ty(0) {
            Some(t) if p.rest_is_blank() => t,
            _ => CqlType::Custom(text.trim().to_string()),
        }
    }

    /// The type without its `frozen<…>`.
    pub fn unfrozen(&self) -> &CqlType {
        match self {
            CqlType::Frozen(inner) => inner.unfrozen(),
            t => t,
        }
    }

    pub fn is_counter(&self) -> bool {
        matches!(self.unfrozen(), CqlType::Native(n) if n == "counter")
    }

    /// Lists, sets, maps, tuples, UDTs and vectors: shown as JSON.
    pub fn is_compound(&self) -> bool {
        !matches!(self.unfrozen(), CqlType::Native(_) | CqlType::Custom(_))
    }

    pub fn kind(&self) -> ValueKind {
        match self.unfrozen() {
            CqlType::Native(n) => native_kind(n),
            CqlType::Custom(_) => ValueKind::Other,
            _ => ValueKind::Json,
        }
    }
}

/// The kind of a built-in type, by its lower-case name.
pub fn native_kind(name: &str) -> ValueKind {
    match name {
        "ascii" | "text" | "varchar" => ValueKind::Text,
        "int" | "smallint" | "tinyint" => ValueKind::Int,
        // May pass 2^53: `Value::from_i64` decides per value.
        "bigint" | "counter" | "varint" => ValueKind::BigInt,
        "decimal" => ValueKind::Decimal,
        "float" | "double" => ValueKind::Float,
        "boolean" => ValueKind::Bool,
        "blob" => ValueKind::Bytes,
        "uuid" | "timeuuid" => ValueKind::Uuid,
        // Milliseconds since the epoch, in UTC.
        "timestamp" => ValueKind::TimestampTz,
        "date" => ValueKind::Date,
        "time" => ValueKind::Time,
        "duration" => ValueKind::Interval,
        _ => ValueKind::Other,
    }
}

struct Parser {
    chars: Vec<char>,
    at: usize,
}

impl Parser {
    fn blank(&mut self) {
        while self.chars.get(self.at).is_some_and(|c| c.is_whitespace()) {
            self.at += 1;
        }
    }

    fn rest_is_blank(&mut self) -> bool {
        self.blank();
        self.at == self.chars.len()
    }

    fn eat(&mut self, c: char) -> bool {
        self.blank();
        if self.chars.get(self.at) == Some(&c) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    /// A bare word, a `"quoted"` name (`""` doubled) or a `'custom'` class (`''` doubled), with
    /// whether it was double-quoted.
    fn name(&mut self) -> Option<(String, bool)> {
        self.blank();
        let c = *self.chars.get(self.at)?;
        if c == '"' || c == '\'' {
            let mut out = String::new();
            self.at += 1;
            loop {
                let x = *self.chars.get(self.at)?;
                self.at += 1;
                if x == c {
                    if self.chars.get(self.at) == Some(&c) {
                        self.at += 1;
                    } else {
                        break;
                    }
                }
                out.push(x);
            }
            return Some((if c == '\'' { format!("'{out}'") } else { out }, c == '"'));
        }
        let start = self.at;
        while self
            .chars
            .get(self.at)
            .is_some_and(|&x| x.is_alphanumeric() || x == '_' || x == '.')
        {
            self.at += 1;
        }
        (self.at > start).then(|| (self.chars[start..self.at].iter().collect(), false))
    }

    fn ty(&mut self, depth: usize) -> Option<CqlType> {
        if depth > MAX_DEPTH {
            return None;
        }
        let (name, quoted) = self.name()?;
        if name.starts_with('\'') {
            return Some(CqlType::Custom(name));
        }
        if quoted {
            return Some(CqlType::Udt(name));
        }
        let lower = name.to_lowercase();
        let args = |p: &mut Parser, n: usize| -> Option<Vec<CqlType>> {
            if !p.eat('<') {
                return None;
            }
            let mut out = Vec::new();
            loop {
                out.push(p.ty(depth + 1)?);
                if p.eat('>') {
                    break;
                }
                if !p.eat(',') {
                    return None;
                }
            }
            (n == 0 || out.len() == n).then_some(out)
        };
        let one = |mut v: Vec<CqlType>| v.pop().map(Box::new);
        Some(match lower.as_str() {
            "frozen" => CqlType::Frozen(one(args(self, 1)?)?),
            "list" => CqlType::List(one(args(self, 1)?)?),
            "set" => CqlType::Set(one(args(self, 1)?)?),
            "map" => {
                let mut kv = args(self, 2)?;
                let v = kv.pop()?;
                let k = kv.pop()?;
                CqlType::Map(Box::new(k), Box::new(v))
            }
            "tuple" => CqlType::Tuple(args(self, 0)?),
            "vector" => {
                if !self.eat('<') {
                    return None;
                }
                let elem = self.ty(depth + 1)?;
                if !self.eat(',') {
                    return None;
                }
                let (n, _) = self.name()?;
                if !self.eat('>') {
                    return None;
                }
                CqlType::Vector(Box::new(elem), n.parse().ok()?)
            }
            "varchar" => CqlType::Native("text".into()),
            n if NATIVE.contains(&n) => CqlType::Native(n.into()),
            // An unquoted UDT name is stored lower case already.
            _ => CqlType::Udt(name),
        })
    }
}

/// The type text of a result column (`list<text>`, `frozen<address>`, `vector<float, 3>`).
/// Result metadata never says frozen, so nested collections read as plain ones.
pub fn column_type_text(t: &ColumnType<'_>) -> String {
    match t {
        ColumnType::Native(n) => native_name(n).to_string(),
        ColumnType::Collection { frozen, typ } => {
            let inner = match typ {
                CollectionType::List(e) => format!("list<{}>", column_type_text(e)),
                CollectionType::Set(e) => format!("set<{}>", column_type_text(e)),
                CollectionType::Map(k, v) => {
                    format!("map<{}, {}>", column_type_text(k), column_type_text(v))
                }
                _ => "collection".to_string(),
            };
            frozen_text(*frozen, inner)
        }
        ColumnType::Vector { typ, dimensions } => {
            format!("vector<{}, {dimensions}>", column_type_text(typ))
        }
        ColumnType::UserDefinedType { frozen, definition } => {
            frozen_text(*frozen, udt_name(&definition.name))
        }
        ColumnType::Tuple(items) => {
            let items: Vec<String> = items.iter().map(column_type_text).collect();
            format!("tuple<{}>", items.join(", "))
        }
        _ => "unknown".to_string(),
    }
}

fn frozen_text(frozen: bool, inner: String) -> String {
    if frozen {
        format!("frozen<{inner}>")
    } else {
        inner
    }
}

/// A UDT name as CQL spells it: bare when it is a lower-case name, quoted otherwise.
pub fn udt_name(name: &str) -> String {
    let plain = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if plain {
        name.to_string()
    } else {
        (crate::sqlgen::dialect::cql::SPEC.quote_ident)(name)
    }
}

/// The kind of a result column.
pub fn column_kind(t: &ColumnType<'_>) -> ValueKind {
    match t {
        ColumnType::Native(n) => native_kind(native_name(n)),
        ColumnType::Collection { .. }
        | ColumnType::Vector { .. }
        | ColumnType::UserDefinedType { .. }
        | ColumnType::Tuple(_) => ValueKind::Json,
        _ => ValueKind::Other,
    }
}

pub fn native_name(n: &NativeType) -> &'static str {
    match n {
        NativeType::Ascii => "ascii",
        NativeType::Boolean => "boolean",
        NativeType::Blob => "blob",
        NativeType::Counter => "counter",
        NativeType::Date => "date",
        NativeType::Decimal => "decimal",
        NativeType::Double => "double",
        NativeType::Duration => "duration",
        NativeType::Float => "float",
        NativeType::Int => "int",
        NativeType::BigInt => "bigint",
        NativeType::Text => "text",
        NativeType::Timestamp => "timestamp",
        NativeType::Inet => "inet",
        NativeType::SmallInt => "smallint",
        NativeType::TinyInt => "tinyint",
        NativeType::Time => "time",
        NativeType::Timeuuid => "timeuuid",
        NativeType::Uuid => "uuid",
        NativeType::Varint => "varint",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::Arc;

    use ::scylla::frame::response::result::UserDefinedType;

    use super::*;

    fn native(n: &str) -> CqlType {
        CqlType::Native(n.into())
    }

    #[test]
    fn parses_schema_type_texts() {
        assert_eq!(CqlType::parse("int"), native("int"));
        assert_eq!(CqlType::parse("varchar"), native("text"));
        assert_eq!(
            CqlType::parse("frozen<map<text, frozen<list<int>>>>"),
            CqlType::Frozen(Box::new(CqlType::Map(
                Box::new(native("text")),
                Box::new(CqlType::Frozen(Box::new(CqlType::List(Box::new(native(
                    "int"
                ))))))
            )))
        );
        assert_eq!(
            CqlType::parse("tuple<int, text, frozen<address>>"),
            CqlType::Tuple(vec![
                native("int"),
                native("text"),
                CqlType::Frozen(Box::new(CqlType::Udt("address".into())))
            ])
        );
        assert_eq!(
            CqlType::parse("frozen<\"Mixed\"\"Type\">"),
            CqlType::Frozen(Box::new(CqlType::Udt("Mixed\"Type".into())))
        );
        assert_eq!(
            CqlType::parse("vector<float, 3>"),
            CqlType::Vector(Box::new(native("float")), 3)
        );
        assert_eq!(
            CqlType::parse("'org.apache.cassandra.db.marshal.DynamicCompositeType(s=>x)'"),
            CqlType::Custom("'org.apache.cassandra.db.marshal.DynamicCompositeType(s=>x)'".into())
        );
        for bad in [
            "",
            "list<",
            "map<int>",
            "list<int, int>",
            "frozen<>",
            "int int",
            "<",
        ] {
            assert!(matches!(CqlType::parse(bad), CqlType::Custom(_)), "{bad:?}");
        }
        let deep = format!("{}int{}", "list<".repeat(500), ">".repeat(500));
        assert!(matches!(CqlType::parse(&deep), CqlType::Custom(_)));
    }

    #[test]
    fn kinds_of_schema_types() {
        let kind = |t: &str| CqlType::parse(t).kind();
        assert_eq!(kind("text"), ValueKind::Text);
        assert_eq!(kind("bigint"), ValueKind::BigInt);
        assert_eq!(kind("varint"), ValueKind::BigInt);
        assert_eq!(kind("decimal"), ValueKind::Decimal);
        assert_eq!(kind("timestamp"), ValueKind::TimestampTz);
        assert_eq!(kind("timeuuid"), ValueKind::Uuid);
        assert_eq!(kind("duration"), ValueKind::Interval);
        assert_eq!(kind("inet"), ValueKind::Other);
        assert_eq!(kind("frozen<list<int>>"), ValueKind::Json);
        assert_eq!(kind("address"), ValueKind::Json);
        assert_eq!(kind("'org.x.Custom'"), ValueKind::Other);
        assert!(CqlType::parse("counter").is_counter());
        assert!(CqlType::parse("frozen<set<int>>").is_compound());
        assert!(!CqlType::parse("blob").is_compound());
    }

    #[test]
    fn result_column_types() {
        let udt = ColumnType::UserDefinedType {
            frozen: true,
            definition: Arc::new(UserDefinedType {
                name: Cow::Borrowed("Address"),
                keyspace: Cow::Borrowed("ks"),
                field_types: vec![(Cow::Borrowed("city"), ColumnType::Native(NativeType::Text))],
            }),
        };
        let map = ColumnType::Collection {
            frozen: false,
            typ: CollectionType::Map(
                Box::new(ColumnType::Native(NativeType::Text)),
                Box::new(udt.clone()),
            ),
        };
        assert_eq!(column_type_text(&map), "map<text, frozen<\"Address\">>");
        assert_eq!(column_kind(&map), ValueKind::Json);
        let tuple = ColumnType::Tuple(vec![
            ColumnType::Native(NativeType::Int),
            ColumnType::Vector {
                typ: Box::new(ColumnType::Native(NativeType::Float)),
                dimensions: 2,
            },
        ]);
        assert_eq!(column_type_text(&tuple), "tuple<int, vector<float, 2>>");
        assert_eq!(
            column_kind(&ColumnType::Native(NativeType::Timestamp)),
            ValueKind::TimestampTz
        );
        assert_eq!(udt_name("address_v2"), "address_v2");
    }
}
