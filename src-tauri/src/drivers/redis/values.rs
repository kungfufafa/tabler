//! RESP replies as Tabler values. Total over every `redis::Value` variant, RESP2 and RESP3:
//! nil is NULL, integers beyond 2^53 are `BigInt`, byte strings are text when they are UTF-8 and
//! bytes otherwise, doubles are floats (`inf` / `nan` are `Other`), RESP3 big numbers keep their
//! digits, errors inside a reply (an EXEC's) are `Other{ty:"error"}`, and nested replies are JSON
//! text.

use base64::Engine as _;

use crate::types::value::{BYTES_PREVIEW_CAP, BytesValue, OtherValue, Value};

type Resp = ::redis::Value;

/// A byte string: text when it is UTF-8, otherwise bytes (capped like every blob preview).
pub fn bytes_value(b: &[u8]) -> Value {
    match std::str::from_utf8(b) {
        Ok(s) => Value::Text(s.to_string()),
        Err(_) => Value::from_bytes(b),
    }
}

/// A key name, never truncated: it is the row's identity and must round-trip byte for byte.
pub fn key_value(b: &[u8]) -> Value {
    match std::str::from_utf8(b) {
        Ok(s) => Value::Text(s.to_string()),
        Err(_) => Value::from_bytes_uncapped(b),
    }
}

/// The digits of a RESP3 big number, or `None` when the server sent something else.
fn big_number_text(b: &[u8]) -> Option<&str> {
    let s = std::str::from_utf8(b).ok()?;
    let digits = s.strip_prefix('-').unwrap_or(s);
    (!digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit())).then_some(s)
}

fn verbatim_text(text: &str) -> &str {
    // redis-rs keeps the `txt:` prefix out of `text`; older servers' replies may still carry it.
    text.strip_prefix("txt:")
        .or_else(|| text.strip_prefix("mkd:"))
        .unwrap_or(text)
}

/// The text of a server error inside a reply, as redis-cli prints it (`WRONGTYPE …`).
pub fn error_text(e: &::redis::ServerError) -> String {
    match e.details() {
        Some(d) => format!("{} {d}", e.code()),
        None => e.code().to_string(),
    }
}

/// One reply as a cell.
pub fn cell(v: &Resp) -> Value {
    match v {
        Resp::Nil => Value::Null,
        Resp::Int(n) => Value::from_i64(*n),
        Resp::BulkString(b) => bytes_value(b),
        Resp::SimpleString(s) => Value::Text(s.clone()),
        Resp::Okay => Value::Text("OK".into()),
        Resp::Double(f) => Value::from_f64("double", *f),
        Resp::Boolean(b) => Value::Bool(*b),
        Resp::VerbatimString { text, .. } => Value::Text(verbatim_text(text).to_string()),
        Resp::BigNumber(b) => match big_number_text(b) {
            Some(s) => Value::BigInt(s.to_string()),
            None => Value::other("bigNumber", String::from_utf8_lossy(b)),
        },
        Resp::Attribute { data, .. } => cell(data),
        Resp::ServerError(e) => Value::other("error", error_text(e)),
        Resp::Array(_) | Resp::Set(_) | Resp::Map(_) | Resp::Push { .. } => Value::Json(json(v)),
        // `redis::Value` is non-exhaustive: a variant added later still shows.
        other => Value::other("reply", format!("{other:?}")),
    }
}

/// A reply as JSON text: arrays and sets are arrays, maps objects (or `[key, value]` pairs when
/// a key is not text), integers and big numbers exact, binary strings `{"$base64": "…"}`,
/// non-finite doubles strings, errors `{"error": "…"}`.
pub fn json(v: &Resp) -> String {
    let mut out = String::new();
    write_json(v, &mut out);
    out
}

fn json_str(s: &str, out: &mut String) {
    // Serializing a str cannot fail.
    out.push_str(&serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()));
}

fn write_bytes_json(b: &[u8], out: &mut String) {
    match std::str::from_utf8(b) {
        Ok(s) => json_str(s, out),
        Err(_) => {
            out.push_str("{\"$base64\":");
            json_str(&base64::engine::general_purpose::STANDARD.encode(b), out);
            out.push('}');
        }
    }
}

fn write_seq<'a>(items: impl Iterator<Item = &'a Resp>, out: &mut String) {
    out.push('[');
    for (i, item) in items.enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_json(item, out);
    }
    out.push(']');
}

/// A map key as an object key, when it is text or a number.
fn key_text(k: &Resp) -> Option<String> {
    match k {
        Resp::BulkString(b) => std::str::from_utf8(b).ok().map(str::to_string),
        Resp::SimpleString(s) => Some(s.clone()),
        Resp::VerbatimString { text, .. } => Some(verbatim_text(text).to_string()),
        Resp::Okay => Some("OK".into()),
        Resp::Int(n) => Some(n.to_string()),
        _ => None,
    }
}

fn write_map(pairs: &[(Resp, Resp)], out: &mut String) {
    let keys: Option<Vec<String>> = pairs.iter().map(|(k, _)| key_text(k)).collect();
    match keys {
        Some(keys) => {
            out.push('{');
            for (i, (key, (_, value))) in keys.iter().zip(pairs).enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json_str(key, out);
                out.push(':');
                write_json(value, out);
            }
            out.push('}');
        }
        None => {
            out.push('[');
            for (i, (k, v)) in pairs.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('[');
                write_json(k, out);
                out.push(',');
                write_json(v, out);
                out.push(']');
            }
            out.push(']');
        }
    }
}

fn write_json(v: &Resp, out: &mut String) {
    match v {
        Resp::Nil => out.push_str("null"),
        Resp::Int(n) => out.push_str(&n.to_string()),
        Resp::BulkString(b) => write_bytes_json(b, out),
        Resp::SimpleString(s) => json_str(s, out),
        Resp::Okay => out.push_str("\"OK\""),
        Resp::Double(f) => write_double(*f, out),
        Resp::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
        Resp::VerbatimString { text, .. } => json_str(verbatim_text(text), out),
        Resp::BigNumber(b) => match big_number_text(b) {
            Some(s) => out.push_str(s),
            None => json_str(&String::from_utf8_lossy(b), out),
        },
        Resp::Array(items) | Resp::Set(items) => write_seq(items.iter(), out),
        Resp::Map(pairs) => write_map(pairs, out),
        Resp::Attribute { data, .. } => write_json(data, out),
        Resp::Push { kind, data } => {
            out.push_str("{\"push\":");
            json_str(&format!("{kind:?}"), out);
            out.push_str(",\"data\":");
            write_seq(data.iter(), out);
            out.push('}');
        }
        Resp::ServerError(e) => {
            out.push_str("{\"error\":");
            json_str(&error_text(e), out);
            out.push('}');
        }
        other => json_str(&format!("{other:?}"), out),
    }
}

fn write_double(f: f64, out: &mut String) {
    match serde_json::Number::from_f64(f) {
        Some(n) => out.push_str(&n.to_string()),
        None if f.is_nan() => out.push_str("\"nan\""),
        None if f > 0.0 => out.push_str("\"inf\""),
        None => out.push_str("\"-inf\""),
    }
}

/// A score or other number a RESP2 server sends as a byte string (`"1.5"`, `"inf"`), or a RESP3
/// double, as JSON.
pub fn write_number_json(v: &Resp, out: &mut String) {
    match v {
        Resp::BulkString(b) | Resp::BigNumber(b) => {
            let parsed = std::str::from_utf8(b)
                .ok()
                .and_then(|s| s.parse::<f64>().ok());
            match parsed {
                Some(f) => write_double(f, out),
                None => write_bytes_json(b, out),
            }
        }
        other => write_json(other, out),
    }
}

/// A score as a cell: a float from a RESP3 double or a RESP2 string.
pub fn number_cell(v: &Resp) -> Value {
    match v {
        Resp::BulkString(b) => match std::str::from_utf8(b)
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
        {
            Some(f) => Value::from_f64("double", f),
            None => bytes_value(b),
        },
        other => cell(other),
    }
}

/// The value cell of a string key from `GETRANGE key 0 cap-1` and its length (`STRLEN`, `None`
/// when that failed): the whole value as text or bytes, or, when it may be only a prefix, bytes
/// marked truncated, which the grid shows as a preview and does not edit until the whole value
/// is loaded (`fetch_cell`). Saving a prefix as the value (`SET … XX KEEPTTL`) would drop the
/// rest, so the read counts as whole only when its length is `STRLEN`'s, or, without a length,
/// when it stopped short of the cap. A length that disagrees with the read (a write between
/// the two) is a preview too. `len` is the larger of the two, a lower bound without `STRLEN`.
pub fn string_preview(read: &[u8], total_len: Option<u64>, cap: u64) -> Value {
    let read_len = read.len() as u64;
    let whole = match total_len {
        Some(total) => read_len == total,
        None => read_len < cap,
    };
    if whole {
        return bytes_value(read);
    }
    let shown = &read[..read.len().min(BYTES_PREVIEW_CAP)];
    Value::Bytes(BytesValue {
        base64: base64::engine::general_purpose::STANDARD.encode(shown),
        len: total_len.unwrap_or(0).max(read_len),
        truncated: true,
    })
}

/// A TTL in seconds from `PTTL` milliseconds, rounded like Redis's own `TTL`; NULL without one
/// (-1) and for a key that is gone (-2).
pub fn ttl_cell(pttl_ms: i64) -> Value {
    if pttl_ms < 0 {
        return Value::Null;
    }
    Value::from_i64(pttl_ms.saturating_add(500) / 1000)
}

/// An integer reply (`STRLEN`, `MEMORY USAGE`) as a cell; NULL for nil, errors and other shapes.
pub fn int_cell(v: &Resp) -> Value {
    match v {
        Resp::Int(n) => Value::from_i64(*n),
        Resp::BulkString(b) => std::str::from_utf8(b)
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .map_or(Value::Null, Value::from_i64),
        _ => Value::Null,
    }
}

/// An integer reply as `i64`.
pub fn as_int(v: &Resp) -> Option<i64> {
    match v {
        Resp::Int(n) => Some(*n),
        Resp::BulkString(b) => std::str::from_utf8(b).ok()?.parse().ok(),
        Resp::SimpleString(s) => s.parse().ok(),
        Resp::Attribute { data, .. } => as_int(data),
        _ => None,
    }
}

/// A reply's text (bulk, simple or verbatim string), when it is UTF-8.
pub fn as_text(v: &Resp) -> Option<String> {
    match v {
        Resp::BulkString(b) => String::from_utf8(b.clone()).ok(),
        Resp::SimpleString(s) => Some(s.clone()),
        Resp::VerbatimString { text, .. } => Some(verbatim_text(text).to_string()),
        Resp::Okay => Some("OK".into()),
        Resp::Attribute { data, .. } => as_text(data),
        _ => None,
    }
}

/// A reply's bytes (bulk or simple string).
pub fn as_bytes(v: &Resp) -> Option<Vec<u8>> {
    match v {
        Resp::BulkString(b) => Some(b.clone()),
        Resp::SimpleString(s) => Some(s.clone().into_bytes()),
        Resp::VerbatimString { text, .. } => Some(verbatim_text(text).as_bytes().to_vec()),
        Resp::Attribute { data, .. } => as_bytes(data),
        _ => None,
    }
}

/// The elements of an array or set reply.
pub fn as_items(v: &Resp) -> Option<&[Resp]> {
    match v {
        Resp::Array(a) | Resp::Set(a) => Some(a),
        Resp::Attribute { data, .. } => as_items(data),
        _ => None,
    }
}

/// Field / value pairs from a RESP3 map or a RESP2 flat array (`HGETALL`, `CONFIG GET`);
/// `None` for other shapes (an odd-length array).
pub fn as_pairs(v: &Resp) -> Option<Vec<(&Resp, &Resp)>> {
    match v {
        Resp::Map(pairs) => Some(pairs.iter().map(|(k, v)| (k, v)).collect()),
        Resp::Attribute { data, .. } => as_pairs(data),
        Resp::Array(items) if items.len().is_multiple_of(2) => Some(
            items
                .as_chunks::<2>()
                .0
                .iter()
                .map(|[k, v]| (k, v))
                .collect(),
        ),
        _ => None,
    }
}

/// `[member, score]` pairs of `ZRANGE … WITHSCORES`: RESP2 flat, RESP3 nested pairs.
pub fn as_scored(v: &Resp) -> Option<Vec<(&Resp, &Resp)>> {
    let items = as_items(v)?;
    if !items.is_empty()
        && items
            .iter()
            .all(|i| as_items(i).is_some_and(|p| p.len() == 2))
    {
        return Some(
            items
                .iter()
                .filter_map(as_items)
                .map(|p| (&p[0], &p[1]))
                .collect(),
        );
    }
    as_pairs(v)
}

/// A plain other value (for module types without a preview).
pub fn other(ty: &str, text: &str) -> Value {
    Value::Other(OtherValue {
        ty: ty.to_string(),
        text: text.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::value::JS_SAFE_INT;
    use ::redis::parse_redis_value;

    fn p(s: &str) -> Resp {
        parse_redis_value(s.as_bytes()).unwrap()
    }

    #[test]
    fn scalars() {
        assert_eq!(cell(&p("_\r\n")), Value::Null);
        assert_eq!(cell(&p("$-1\r\n")), Value::Null);
        assert_eq!(cell(&p(":42\r\n")), Value::Int(42));
        assert_eq!(
            cell(&Resp::Int(JS_SAFE_INT + 2)),
            Value::BigInt("9007199254740993".into())
        );
        assert_eq!(
            cell(&Resp::Int(i64::MIN)),
            Value::BigInt(i64::MIN.to_string())
        );
        assert_eq!(cell(&p("+OK\r\n")), Value::Text("OK".into()));
        assert_eq!(cell(&p("+PONG\r\n")), Value::Text("PONG".into()));
        assert_eq!(cell(&p("$5\r\nhello\r\n")), Value::Text("hello".into()));
        assert_eq!(cell(&p("$0\r\n\r\n")), Value::Text(String::new()));
        assert_eq!(cell(&p(",1.5\r\n")), Value::Float(1.5));
        assert_eq!(cell(&p(",inf\r\n")), Value::other("double", "inf"));
        assert_eq!(cell(&p(",-inf\r\n")), Value::other("double", "-inf"));
        assert!(matches!(cell(&p(",nan\r\n")), Value::Other(o) if o.ty == "double"));
        assert_eq!(cell(&p("#t\r\n")), Value::Bool(true));
        assert_eq!(cell(&p("#f\r\n")), Value::Bool(false));
        assert_eq!(
            cell(&p("(3492890328409238509324850943850943825024385\r\n")),
            Value::BigInt("3492890328409238509324850943850943825024385".into())
        );
        assert_eq!(cell(&p("(-12\r\n")), Value::BigInt("-12".into()));
        assert_eq!(
            cell(&p("=15\r\ntxt:Some string\r\n")),
            Value::Text("Some string".into())
        );
    }

    #[test]
    fn binary_strings_are_bytes() {
        let v = cell(&Resp::BulkString(vec![0xff, 0x00, b'a']));
        let Value::Bytes(b) = v else { panic!("{v:?}") };
        assert_eq!(b.len, 3);
        assert!(!b.truncated);
        assert_eq!(b.base64, "/wBh");
        // Keys are never truncated.
        let big = vec![0xffu8; BYTES_PREVIEW_CAP + 1];
        let Value::Bytes(k) = key_value(&big) else {
            panic!()
        };
        assert!(!k.truncated);
        assert_eq!(key_value(b"user:1"), Value::Text("user:1".into()));
    }

    #[test]
    fn errors_inside_replies() {
        let exec = p(
            "*3\r\n+OK\r\n-WRONGTYPE Operation against a key holding the wrong kind of value\r\n:1\r\n",
        );
        let Resp::Array(items) = &exec else { panic!() };
        assert_eq!(
            cell(&items[1]),
            Value::other(
                "error",
                "WRONGTYPE Operation against a key holding the wrong kind of value"
            )
        );
        assert_eq!(
            json(&exec),
            r#"["OK",{"error":"WRONGTYPE Operation against a key holding the wrong kind of value"},1]"#
        );
        assert_eq!(
            cell(&p("!21\r\nSYNTAX invalid syntax\r\n")),
            Value::other("error", "SYNTAX invalid syntax")
        );
    }

    #[test]
    fn nested_replies_are_json() {
        // RESP2 XRANGE: entries of [id, [field, value, …]].
        let xrange = parse_redis_value(
            b"*1\r\n*2\r\n$3\r\n1-1\r\n*4\r\n$1\r\na\r\n$1\r\n1\r\n$1\r\nb\r\n$2\r\n\xff\x00\r\n",
        )
        .unwrap();
        assert_eq!(
            cell(&xrange),
            Value::Json(r#"[["1-1",["a","1","b",{"$base64":"/wA="}]]]"#.into())
        );
        // RESP3 map with text keys, set, double, nil, big number, boolean.
        let map = p(
            "%3\r\n$1\r\na\r\n~2\r\n:1\r\n,2.5\r\n+b\r\n_\r\n:7\r\n(123456789012345678901234567890\r\n",
        );
        assert_eq!(
            json(&map),
            r#"{"a":[1,2.5],"b":null,"7":123456789012345678901234567890}"#
        );
        // Keys that are not text make a list of pairs.
        let odd = Resp::Map(vec![(Resp::Array(vec![]), Resp::Boolean(true))]);
        assert_eq!(json(&odd), "[[[],true]]");
        assert_eq!(json(&p(",inf\r\n")), "\"inf\"");
        assert_eq!(json(&p(",-inf\r\n")), "\"-inf\"");
        assert_eq!(json(&p(",nan\r\n")), "\"nan\"");
        assert_eq!(json(&Resp::Double(1e300)), "1e+300");
        assert_eq!(json(&p("|1\r\n+key-popularity\r\n:1\r\n:5\r\n")), "5");
        assert_eq!(json(&Resp::Int(i64::MAX)), i64::MAX.to_string());
        assert_eq!(json(&Resp::SimpleString("a\"\n".into())), r#""a\"\n""#);
        let push = Resp::Push {
            kind: ::redis::PushKind::Message,
            data: vec![Resp::Int(1)],
        };
        assert_eq!(json(&push), r#"{"push":"Message","data":[1]}"#);
        assert!(matches!(cell(&push), Value::Json(_)));
        assert_eq!(
            cell(&p("|1\r\n+a\r\n:1\r\n$1\r\nx\r\n")),
            Value::Text("x".into())
        );
    }

    #[test]
    fn numbers_and_scores() {
        assert_eq!(number_cell(&p("$3\r\n1.5\r\n")), Value::Float(1.5));
        assert_eq!(
            number_cell(&p("$3\r\ninf\r\n")),
            Value::other("double", "inf")
        );
        assert_eq!(number_cell(&p(",2\r\n")), Value::Float(2.0));
        assert_eq!(number_cell(&p("$1\r\nx\r\n")), Value::Text("x".into()));
        let mut out = String::new();
        write_number_json(&p("$4\r\n-inf\r\n"), &mut out);
        assert_eq!(out, "\"-inf\"");
        let mut out = String::new();
        write_number_json(&p("$2\r\n10\r\n"), &mut out);
        assert_eq!(out, "10.0");
        assert_eq!(int_cell(&p(":5\r\n")), Value::Int(5));
        assert_eq!(int_cell(&p("_\r\n")), Value::Null);
        assert_eq!(int_cell(&p("-ERR x\r\n")), Value::Null);
        assert_eq!(as_int(&p("$2\r\n12\r\n")), Some(12));
        assert_eq!(as_int(&p("$2\r\nab\r\n")), None);
    }

    #[test]
    fn string_previews() {
        let cap = 16;
        assert_eq!(
            string_preview(b"abc", Some(3), cap),
            Value::Text("abc".into())
        );
        let Value::Bytes(b) = string_preview(b"abc", Some(10), cap) else {
            panic!()
        };
        assert!(b.truncated);
        assert_eq!(b.len, 10);
        assert_eq!(b.base64, "YWJj");
        // A preview cut in the middle of a character is still a (truncated) preview, not an error.
        let Value::Bytes(b) = string_preview(&"é".as_bytes()[..1], Some(2), cap) else {
            panic!()
        };
        assert!(b.truncated);
        assert!(matches!(
            string_preview(&[0xff], Some(1), cap),
            Value::Bytes(BytesValue {
                truncated: false,
                ..
            })
        ));
        let big = vec![b'a'; BYTES_PREVIEW_CAP + 10];
        let Value::Bytes(b) = string_preview(&big, Some((BYTES_PREVIEW_CAP * 4) as u64), u64::MAX)
        else {
            panic!()
        };
        assert_eq!(b.len, (BYTES_PREVIEW_CAP * 4) as u64);
    }

    #[test]
    fn a_preview_is_whole_only_when_it_provably_is() {
        let cap = 16;
        let full = [b'x'; 16];
        let truncated = |v: Value, len: u64| {
            assert!(
                matches!(&v, Value::Bytes(b) if b.truncated && b.len == len),
                "{v:?}"
            );
        };
        // Without STRLEN (an ACL that allows GETRANGE only): a read that filled the cap may be a
        // prefix, so it is not editable; one that stopped short of it is the whole value.
        truncated(string_preview(&full, None, cap), 16);
        assert_eq!(
            string_preview(b"short", None, cap),
            Value::Text("short".into())
        );
        assert_eq!(string_preview(b"", None, cap), Value::Text(String::new()));
        // A STRLEN that disagrees with the read (a write between the two) is not trusted.
        truncated(string_preview(&full, Some(5), cap), 16);
        truncated(string_preview(b"abcdefg", Some(5), cap), 7);
        // A value exactly as long as the cap, with its length known, is whole.
        assert_eq!(
            string_preview(&full, Some(16), cap),
            Value::Text("x".repeat(16))
        );
    }

    #[test]
    fn ttls() {
        assert_eq!(ttl_cell(-1), Value::Null);
        assert_eq!(ttl_cell(-2), Value::Null);
        assert_eq!(ttl_cell(0), Value::Int(0));
        assert_eq!(ttl_cell(1499), Value::Int(1));
        assert_eq!(ttl_cell(1500), Value::Int(2));
        assert_eq!(ttl_cell(i64::MAX), Value::from_i64(i64::MAX / 1000));
    }

    #[test]
    fn pair_shapes() {
        let resp2 = p("*4\r\n$1\r\nf\r\n$1\r\nv\r\n$1\r\ng\r\n$1\r\nw\r\n");
        let resp3 = p("%2\r\n$1\r\nf\r\n$1\r\nv\r\n$1\r\ng\r\n$1\r\nw\r\n");
        let flat = |v: &Resp| {
            as_pairs(v)
                .unwrap()
                .iter()
                .map(|(k, v)| (as_text(k).unwrap(), as_text(v).unwrap()))
                .collect::<Vec<_>>()
        };
        assert_eq!(flat(&resp2), flat(&resp3));
        assert!(as_pairs(&p("*1\r\n:1\r\n")).is_none());
        let z2 = p("*4\r\n$1\r\na\r\n$1\r\n1\r\n$1\r\nb\r\n$3\r\n2.5\r\n");
        let z3 = p("*2\r\n*2\r\n$1\r\na\r\n,1\r\n*2\r\n$1\r\nb\r\n,2.5\r\n");
        let scores = |v: &Resp| {
            as_scored(v)
                .unwrap()
                .iter()
                .map(|(m, s)| (as_text(m).unwrap(), number_cell(s)))
                .collect::<Vec<_>>()
        };
        assert_eq!(scores(&z2), scores(&z3));
        assert_eq!(scores(&z3)[1], ("b".to_string(), Value::Float(2.5)));
        assert_eq!(as_bytes(&p("+x\r\n")), Some(b"x".to_vec()));
        assert_eq!(as_bytes(&p(":1\r\n")), None);
        assert!(as_items(&p("~1\r\n:1\r\n")).is_some());
        assert_eq!(other("TSDB-TYPE", "x"), Value::other("TSDB-TYPE", "x"));
    }
}
