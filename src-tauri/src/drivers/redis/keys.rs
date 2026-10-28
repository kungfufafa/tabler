//! A database's keys as a table: the key types, the synthetic columns of a `dbN` table and the
//! value previews of each type. The commands are built in `sqlgen::native::redis`.

use crate::sqlgen::native::redis::{self as cmd, Command, TYPE_JSON, TYPE_STRING, col};
use crate::types::schema::{ColumnMeta, TableKind, TableMeta, TableRef, ValueKind};
use crate::types::value::Value;

use super::values::{
    as_bytes, as_int, as_items, as_pairs, as_scored, as_text, bytes_value, json, string_preview,
    write_number_json,
};

type Resp = ::redis::Value;

/// A key's type as `TYPE` names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyType {
    String,
    List,
    Set,
    ZSet,
    Hash,
    Stream,
    /// RedisJSON (`ReJSON-RL`).
    Json,
    /// A module's type (`TSDB-TYPE`, `MBbloom--`, `vectorset`, …): shown, not previewed.
    Other(String),
    /// The key does not exist (any more).
    None,
}

/// The types the `type` column lists (its enum values).
pub const KNOWN_TYPES: &[&str] = &[
    TYPE_STRING,
    "list",
    "set",
    "zset",
    "hash",
    "stream",
    TYPE_JSON,
];

impl KeyType {
    pub fn parse(name: &str) -> KeyType {
        match name {
            TYPE_STRING => KeyType::String,
            "list" => KeyType::List,
            "set" => KeyType::Set,
            "zset" => KeyType::ZSet,
            "hash" => KeyType::Hash,
            "stream" => KeyType::Stream,
            TYPE_JSON => KeyType::Json,
            "none" | "" => KeyType::None,
            other => KeyType::Other(other.to_string()),
        }
    }

    /// From a `TYPE` reply; anything unreadable is an unknown type, not a missing key.
    pub fn from_reply(v: &Resp) -> KeyType {
        match as_text(v) {
            Some(name) => KeyType::parse(&name),
            None => KeyType::Other("unknown".into()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            KeyType::String => TYPE_STRING,
            KeyType::List => "list",
            KeyType::Set => "set",
            KeyType::ZSet => "zset",
            KeyType::Hash => "hash",
            KeyType::Stream => "stream",
            KeyType::Json => TYPE_JSON,
            KeyType::Other(name) => name,
            KeyType::None => "none",
        }
    }

    pub fn exists(&self) -> bool {
        *self != KeyType::None
    }

    /// The command that reads the key's size (`STRLEN`, `LLEN`, …).
    pub fn size_command(&self, key: &[u8]) -> Option<Command> {
        cmd::size_of(self.name(), key)
    }

    /// The command that reads the value preview.
    pub fn preview_command(&self, key: &[u8], limits: &Limits) -> Option<Command> {
        cmd::preview_of(self.name(), key, limits.bytes, limits.elements)
    }
}

/// How much of a value the grid loads per key (the profile's "value preview cap").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Bytes of a string.
    pub bytes: u64,
    /// Elements of a container.
    pub elements: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            bytes: 16 * 1024,
            elements: 50,
        }
    }
}

/// The value cell from a preview reply read with `limits`. `size` is the key's size (`STRLEN`
/// for a string, which with the cap tells whether the preview is the whole value: see
/// `string_preview`). Containers become JSON: a list or set an array, a hash an object, a sorted
/// set an object of member to score, a stream an array of `{"id", "fields"}` entries (newest
/// first). A server error or an unexpected shape is NULL.
pub fn preview_cell(ty: &KeyType, reply: &Resp, size: Option<i64>, limits: &Limits) -> Value {
    if matches!(reply, Resp::ServerError(_)) {
        return Value::Null;
    }
    match ty {
        KeyType::String => match as_bytes(reply) {
            Some(b) => {
                let total = size.and_then(|n| u64::try_from(n).ok());
                // `preview_of` reads at least one byte.
                string_preview(&b, total, limits.bytes.max(1))
            }
            None => Value::Null,
        },
        KeyType::List => match as_items(reply) {
            Some(_) => Value::Json(json(reply)),
            None => Value::Null,
        },
        KeyType::Set => match scan_items(reply) {
            Some(items) => Value::Json(json(&Resp::Array(items.to_vec()))),
            None => Value::Null,
        },
        KeyType::Hash => match scan_items(reply) {
            Some(items) if items.len().is_multiple_of(2) => Value::Json(pairs_json(items)),
            _ => Value::Null,
        },
        KeyType::ZSet => match as_scored(reply) {
            Some(pairs) => Value::Json(scored_json(&pairs)),
            None => Value::Null,
        },
        KeyType::Stream => match stream_json(reply) {
            Some(text) => Value::Json(text),
            None => Value::Null,
        },
        KeyType::Json => match as_bytes(reply) {
            Some(b) if serde_json::from_slice::<serde::de::IgnoredAny>(&b).is_ok() => {
                Value::Json(String::from_utf8_lossy(&b).into_owned())
            }
            Some(b) => bytes_value(&b),
            None => Value::Null,
        },
        KeyType::Other(_) | KeyType::None => Value::Null,
    }
}

/// The elements of a `[cursor, [items…]]` scan reply.
fn scan_items(reply: &Resp) -> Option<&[Resp]> {
    let parts = as_items(reply)?;
    match parts {
        [_, items] => as_items(items),
        _ => None,
    }
}

/// A flat field / value list (`HSCAN`) as an object.
fn pairs_json(items: &[Resp]) -> String {
    let pairs: Vec<(Resp, Resp)> = items
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[k, v]| (k.clone(), v.clone()))
        .collect();
    json(&Resp::Map(pairs))
}

fn scored_json(pairs: &[(&Resp, &Resp)]) -> String {
    let as_map: Vec<(Resp, Resp)> = pairs
        .iter()
        .map(|(m, s)| ((*m).clone(), (*s).clone()))
        .collect();
    // An object of member to score when every member is text, else pairs; scores are numbers.
    if as_map.iter().all(|(m, _)| as_text(m).is_some()) {
        let mut out = String::from("{");
        for (i, (m, s)) in as_map.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&json(m));
            out.push(':');
            write_number_json(s, &mut out);
        }
        out.push('}');
        out
    } else {
        let mut out = String::from("[");
        for (i, (m, s)) in as_map.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push('[');
            out.push_str(&json(m));
            out.push(',');
            write_number_json(s, &mut out);
            out.push(']');
        }
        out.push(']');
        out
    }
}

/// `XRANGE` / `XREVRANGE` entries `[id, [field, value, …]]` (or a RESP3 map of fields) as
/// `[{"id": …, "fields": {…}}]`.
pub fn stream_json(reply: &Resp) -> Option<String> {
    let entries = as_items(reply)?;
    let mut out = String::from("[");
    for (i, entry) in entries.iter().enumerate() {
        let [id, fields] = as_items(entry)? else {
            return None;
        };
        let id = as_text(id)?;
        let pairs = match fields {
            Resp::Nil => Vec::new(),
            other => as_pairs(other)?,
        };
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"id\":");
        out.push_str(&json(&Resp::SimpleString(id)));
        out.push_str(",\"fields\":");
        let map: Vec<(Resp, Resp)> = pairs
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.push_str(&json(&Resp::Map(map)));
        out.push('}');
    }
    out.push(']');
    Some(out)
}

/// The size cell from its reply (`STRLEN`, `LLEN`, …).
pub fn size_cell(reply: Option<&Resp>) -> Option<i64> {
    reply.and_then(as_int)
}

fn column(name: &str, ordinal: u32, data_type: &str, kind: ValueKind) -> ColumnMeta {
    ColumnMeta {
        name: name.to_string(),
        ordinal,
        data_type: data_type.to_string(),
        kind,
        nullable: true,
        default_expr: None,
        is_pk: false,
        pk_ordinal: None,
        is_auto_increment: false,
        is_generated: false,
        is_unsigned: false,
        comment: None,
        enum_values: None,
        char_max_len: None,
        numeric_precision: None,
        numeric_scale: None,
        collation: None,
        array_elem_type: None,
        extra: None,
        base_type: None,
    }
}

/// The columns of a `dbN` table: the key and its type identify a row (the type lets an edit
/// write the value the right way and the apply prove the key was not replaced meanwhile); TTL,
/// size and memory are computed; the value is a preview.
pub fn columns() -> Vec<ColumnMeta> {
    let mut key = column(col::KEY, 1, "key", ValueKind::Text);
    key.nullable = false;
    key.is_pk = true;
    key.pk_ordinal = Some(1);
    key.comment = Some("text, or bytes when the name is not UTF-8".into());
    let mut ty = column(col::TYPE, 2, "type", ValueKind::Enum);
    ty.nullable = false;
    ty.is_pk = true;
    ty.pk_ordinal = Some(2);
    ty.is_generated = true;
    ty.enum_values = Some(KNOWN_TYPES.iter().map(|t| t.to_string()).collect());
    let mut ttl = column(col::TTL, 3, "ttl (seconds)", ValueKind::Int);
    ttl.comment = Some("seconds until the key expires; empty for no expiry".into());
    let mut size = column(col::SIZE, 4, "size", ValueKind::Int);
    size.is_generated = true;
    size.comment = Some("bytes of a string, elements of a container".into());
    let mut memory = column(col::MEMORY, 5, "memory (bytes)", ValueKind::Int);
    memory.is_generated = true;
    let mut value = column(col::VALUE, 6, "value", ValueKind::Text);
    value.comment = Some("a string's value; a preview of a container's elements".into());
    vec![key, ty, ttl, size, memory, value]
}

/// The primary key columns of a `dbN` table.
pub fn primary_key() -> Vec<String> {
    vec![col::KEY.to_string(), col::TYPE.to_string()]
}

/// A `dbN` table's meta (`keys` from `INFO keyspace` when known).
pub fn table_meta(table: &TableRef, keys: Option<i64>) -> TableMeta {
    TableMeta {
        table: table.clone(),
        kind: TableKind::Table,
        columns: columns(),
        primary_key: primary_key(),
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        triggers: Vec::new(),
        comment: None,
        row_estimate: keys,
        ddl: None,
        checks: Vec::new(),
        options: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::redis::parse_redis_value;

    fn p(s: &str) -> Resp {
        parse_redis_value(s.as_bytes()).unwrap()
    }

    #[test]
    fn types() {
        for name in KNOWN_TYPES {
            assert_eq!(KeyType::parse(name).name(), *name);
        }
        assert_eq!(KeyType::parse("none"), KeyType::None);
        assert!(!KeyType::parse("none").exists());
        assert_eq!(
            KeyType::parse("TSDB-TYPE"),
            KeyType::Other("TSDB-TYPE".into())
        );
        assert_eq!(KeyType::from_reply(&p("+hash\r\n")), KeyType::Hash);
        assert_eq!(KeyType::from_reply(&p("$6\r\nstring\r\n")), KeyType::String);
        assert_eq!(
            KeyType::from_reply(&p(":1\r\n")),
            KeyType::Other("unknown".into())
        );
        assert_eq!(
            KeyType::Hash
                .size_command(b"h")
                .map(|c| c.text())
                .as_deref(),
            Some("HLEN h")
        );
        assert!(KeyType::Json.size_command(b"j").is_none());
        assert_eq!(
            KeyType::String
                .preview_command(b"s", &Limits::default())
                .map(|c| c.text())
                .as_deref(),
            Some("GETRANGE s 0 16383")
        );
        assert!(
            KeyType::Other("x".into())
                .preview_command(b"s", &Limits::default())
                .is_none()
        );
    }

    #[test]
    fn previews_of_every_type() {
        let l = Limits::default();
        assert_eq!(
            preview_cell(&KeyType::String, &p("$5\r\nhello\r\n"), Some(5), &l),
            Value::Text("hello".into())
        );
        assert!(matches!(
            preview_cell(&KeyType::String, &p("$5\r\nhello\r\n"), Some(9), &l),
            Value::Bytes(b) if b.truncated && b.len == 9
        ));
        assert_eq!(
            preview_cell(
                &KeyType::List,
                &p("*2\r\n$1\r\na\r\n$1\r\nb\r\n"),
                Some(2),
                &l
            ),
            Value::Json(r#"["a","b"]"#.into())
        );
        assert_eq!(
            preview_cell(
                &KeyType::Set,
                &p("*2\r\n$1\r\n0\r\n*1\r\n$1\r\nm\r\n"),
                Some(1),
                &l
            ),
            Value::Json(r#"["m"]"#.into())
        );
        assert_eq!(
            preview_cell(
                &KeyType::Hash,
                &p("*2\r\n$1\r\n0\r\n*4\r\n$1\r\nf\r\n$1\r\nv\r\n$1\r\ng\r\n$1\r\nw\r\n"),
                Some(2),
                &l
            ),
            Value::Json(r#"{"f":"v","g":"w"}"#.into())
        );
        // RESP2 and RESP3 sorted sets preview alike.
        let z2 = p("*4\r\n$1\r\na\r\n$1\r\n1\r\n$1\r\nb\r\n$3\r\ninf\r\n");
        let z3 = p("*2\r\n*2\r\n$1\r\na\r\n,1\r\n*2\r\n$1\r\nb\r\n,inf\r\n");
        assert_eq!(
            preview_cell(&KeyType::ZSet, &z2, None, &l),
            preview_cell(&KeyType::ZSet, &z3, None, &l)
        );
        assert_eq!(
            preview_cell(&KeyType::ZSet, &z2, None, &l),
            Value::Json(r#"{"a":1.0,"b":"inf"}"#.into())
        );
        let binary_member = Resp::Array(vec![
            Resp::BulkString(vec![0xff]),
            Resp::BulkString(b"2".to_vec()),
        ]);
        assert_eq!(
            preview_cell(&KeyType::ZSet, &binary_member, None, &l),
            Value::Json(r#"[[{"$base64":"/w=="},2.0]]"#.into())
        );
        let stream = p("*1\r\n*2\r\n$3\r\n2-0\r\n*2\r\n$1\r\nf\r\n$1\r\nv\r\n");
        assert_eq!(
            preview_cell(&KeyType::Stream, &stream, Some(1), &l),
            Value::Json(r#"[{"id":"2-0","fields":{"f":"v"}}]"#.into())
        );
        assert_eq!(
            preview_cell(&KeyType::Json, &p("$7\r\n{\"a\":1}\r\n"), None, &l),
            Value::Json(r#"{"a":1}"#.into())
        );
        assert_eq!(
            preview_cell(&KeyType::Json, &p("$3\r\n{no\r\n"), None, &l),
            Value::Text("{no".into())
        );
        // Errors, other shapes and module types are NULL.
        assert_eq!(
            preview_cell(&KeyType::List, &p("-WRONGTYPE x\r\n"), None, &l),
            Value::Null
        );
        assert_eq!(
            preview_cell(&KeyType::Hash, &p(":1\r\n"), None, &l),
            Value::Null
        );
        assert_eq!(
            preview_cell(&KeyType::Stream, &p("*1\r\n:1\r\n"), None, &l),
            Value::Null
        );
        assert_eq!(
            preview_cell(&KeyType::Other("TSDB-TYPE".into()), &p(":1\r\n"), None, &l),
            Value::Null
        );
        assert_eq!(
            preview_cell(&KeyType::String, &p("_\r\n"), None, &l),
            Value::Null
        );
    }

    #[test]
    fn a_string_preview_that_may_be_cut_is_not_editable() {
        let l = Limits {
            bytes: 16,
            elements: 10,
        };
        let filled = Resp::BulkString(vec![b'x'; 16]);
        let cut = |v: Value| matches!(v, Value::Bytes(b) if b.truncated);
        // STRLEN failed (an ACL without it), or disagrees with the read (a write in between).
        assert!(cut(preview_cell(&KeyType::String, &filled, None, &l)));
        assert!(cut(preview_cell(&KeyType::String, &filled, Some(5), &l)));
        assert!(cut(preview_cell(&KeyType::String, &filled, Some(-1), &l)));
        assert!(cut(preview_cell(&KeyType::String, &filled, Some(40), &l)));
        assert_eq!(
            preview_cell(&KeyType::String, &filled, Some(16), &l),
            Value::Text("x".repeat(16))
        );
        // A read that stopped short of the cap is the whole value, with or without STRLEN.
        let short = Resp::BulkString(b"hello".to_vec());
        assert_eq!(
            preview_cell(&KeyType::String, &short, None, &l),
            Value::Text("hello".into())
        );
        // A cap of 0 reads one byte (`preview_of`).
        let one = Limits { bytes: 0, ..l };
        assert!(cut(preview_cell(
            &KeyType::String,
            &Resp::BulkString(b"h".to_vec()),
            None,
            &one
        )));
    }

    #[test]
    fn table_shape() {
        let cols = columns();
        let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["key", "type", "ttl", "size", "memory", "value"]);
        assert_eq!(primary_key(), ["key", "type"]);
        let meta = table_meta(&TableRef::new(Some("db0"), "db0"), Some(12));
        assert_eq!(meta.row_estimate, Some(12));
        assert_eq!(meta.columns[1].enum_values.as_ref().map(Vec::len), Some(7));
        assert!(meta.columns.iter().all(|c| c.ordinal > 0));
        assert_eq!(size_cell(Some(&p(":3\r\n"))), Some(3));
        assert_eq!(size_cell(None), None);
    }
}
