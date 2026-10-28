//! Browsing a database's keys: `SCAN` pages with the cursor as the page token (a decimal
//! string, never parsed into a number the renderer could round), the filter bar as a `MATCH`
//! glob or an exact key (never a SCAN for `key = x`, TablePlus #1224), and the rows of a page from
//! two pipelined rounds: `TYPE` + `PTTL` for every key, then its size and value preview.
//!
//! SCAN may return a key twice, pages of uneven size (empty ones too) and keys in no order, so a
//! page is deduplicated and sorting can only reorder the page itself.

use std::collections::HashSet;

use crate::error::AppError;
use crate::sqlgen::native::redis::{self as cmd, Command, Gates, col, value_arg};
use crate::types::browse::{FilterGroup, FilterNode, FilterOp};
use crate::types::value::Value;

use super::keys::{KeyType, Limits, preview_cell, size_cell};
use super::values::{as_bytes, as_int, as_items, as_text, int_cell, key_value, ttl_cell};

type Resp = ::redis::Value;

/// The renderer's page token as a SCAN cursor: missing or empty is the start (`0`); anything
/// but an unsigned 64-bit decimal is refused.
pub fn cursor(token: Option<&str>) -> Result<String, AppError> {
    let t = token.map(str::trim).unwrap_or("");
    if t.is_empty() {
        return Ok("0".into());
    }
    if t.bytes().all(|b| b.is_ascii_digit()) && t.parse::<u64>().is_ok() {
        // Without leading zeros, as the server writes it.
        let s = t.trim_start_matches('0');
        return Ok(if s.is_empty() { "0" } else { s }.to_string());
    }
    Err(AppError::InvalidInput(format!(
        "“{t}” is not a Redis SCAN cursor"
    )))
}

/// The next page's token: `None` when the server's cursor is back at 0 (the scan is complete).
pub fn next_token(server_cursor: &str) -> Option<String> {
    (server_cursor != "0").then(|| server_cursor.to_string())
}

/// A `SCAN` / `HSCAN` / `SSCAN` / `ZSCAN` reply: the next cursor and the items.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanReply {
    pub cursor: String,
    pub items: Vec<Resp>,
}

pub fn parse_scan(reply: &Resp) -> Result<ScanReply, AppError> {
    let bad = || AppError::Internal("the server sent an unexpected SCAN reply".into());
    if let Resp::ServerError(e) = reply {
        return Err(super::errors::server_error(e));
    }
    let [c, items] = as_items(reply).ok_or_else(bad)? else {
        return Err(bad());
    };
    let c = as_text(c).ok_or_else(bad)?;
    let c = cursor(Some(&c)).map_err(|_| bad())?;
    let items = as_items(items).ok_or_else(bad)?.to_vec();
    Ok(ScanReply { cursor: c, items })
}

/// The keys of a SCAN reply, each once, in the server's order (binary keys included; a non-string
/// item is skipped).
pub fn scan_keys(reply: &ScanReply) -> Vec<Vec<u8>> {
    let mut seen = HashSet::new();
    reply
        .items
        .iter()
        .filter_map(as_bytes)
        .filter(|k| seen.insert(k.clone()))
        .collect()
}

/// Which keys a page lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyMatch {
    All,
    /// A `MATCH` glob.
    Pattern(Vec<u8>),
    /// One key (`key = x`): `TYPE` and `PTTL` of it, no SCAN.
    Exact(Vec<u8>),
}

/// The filter bar as a key match and a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyFilter {
    pub keys: KeyMatch,
    pub ty: Option<String>,
}

impl KeyFilter {
    /// The SCAN of one page (`None` for an exact key). Without `SCAN … TYPE` (before 6.0) the
    /// type is filtered after the `TYPE` round (`matches_type`).
    pub fn scan_command(&self, cursor: &str, count: u32, gates: &Gates) -> Option<Command> {
        let pattern = match &self.keys {
            KeyMatch::Exact(_) => return None,
            KeyMatch::All => None,
            KeyMatch::Pattern(p) => Some(p.as_slice()),
        };
        let ty = self.ty.as_deref().filter(|_| gates.scan_type);
        Some(cmd::scan(cursor, pattern, count, ty))
    }

    pub fn matches_type(&self, ty: &KeyType) -> bool {
        self.ty.as_deref().is_none_or(|t| t == ty.name())
    }
}

/// Escapes glob characters (`* ? [ ] \`) so the bytes match literally; with `fold`, ASCII
/// letters match either case (`[aA]`), since MATCH has no case-insensitive mode.
pub fn glob_literal(bytes: &[u8], fold: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 4);
    for &b in bytes {
        push_literal(&mut out, b, fold);
    }
    out
}

fn push_literal(out: &mut Vec<u8>, b: u8, fold: bool) {
    if fold && b.is_ascii_alphabetic() {
        out.extend_from_slice(&[b'[', b.to_ascii_lowercase(), b.to_ascii_uppercase(), b']']);
    } else {
        if matches!(b, b'*' | b'?' | b'[' | b']' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
}

/// A SQL `LIKE` pattern as a glob: `%` is `*`, `_` is `?`, a backslash makes the next character
/// literal, everything else is literal.
pub fn like_glob(pattern: &[u8], fold: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(pattern.len() + 4);
    let mut i = 0;
    while i < pattern.len() {
        match pattern[i] {
            b'%' => out.push(b'*'),
            b'_' => out.push(b'?'),
            b'\\' if i + 1 < pattern.len() => {
                i += 1;
                push_literal(&mut out, pattern[i], fold);
            }
            b => push_literal(&mut out, b, fold),
        }
        i += 1;
    }
    out
}

fn unsupported_filter() -> AppError {
    AppError::Unsupported(
        "Redis filters keys by one name pattern (equals, contains, starts / ends with, like, or a \
         raw glob) and by type"
            .into(),
    )
}

/// The filter bar of a key table as a key match and a type.
pub fn key_filter(filter: Option<&FilterGroup>) -> Result<KeyFilter, AppError> {
    let mut out = KeyFilter {
        keys: KeyMatch::All,
        ty: None,
    };
    if let Some(group) = filter {
        add_group(group, &mut out)?;
    }
    Ok(out)
}

fn add_group(group: &FilterGroup, out: &mut KeyFilter) -> Result<(), AppError> {
    if group.items.len() > 1 && group.conjunction == crate::types::browse::Conjunction::Or {
        return Err(unsupported_filter());
    }
    for item in &group.items {
        match item {
            FilterNode::Group(g) => add_group(g, out)?,
            FilterNode::Raw { sql } => {
                let glob = sql.trim();
                if !glob.is_empty() {
                    set_keys(out, KeyMatch::Pattern(glob.as_bytes().to_vec()))?;
                }
            }
            FilterNode::Condition { column, op, values } if column == col::TYPE => {
                let (FilterOp::Eq, [Value::Text(t) | Value::Enum(t)]) = (op, values.as_slice())
                else {
                    return Err(unsupported_filter());
                };
                if out.ty.replace(t.clone()).is_some() {
                    return Err(unsupported_filter());
                }
            }
            FilterNode::Condition { column, op, values } if column == col::KEY => {
                set_keys(out, key_match(*op, values)?)?;
            }
            // The key is the only text the server can match.
            FilterNode::AnyColumn { op, values } => set_keys(out, key_match(*op, values)?)?,
            FilterNode::Condition { .. } => return Err(unsupported_filter()),
        }
    }
    Ok(())
}

fn set_keys(out: &mut KeyFilter, keys: KeyMatch) -> Result<(), AppError> {
    if out.keys != KeyMatch::All {
        return Err(unsupported_filter());
    }
    out.keys = keys;
    Ok(())
}

fn key_match(op: FilterOp, values: &[Value]) -> Result<KeyMatch, AppError> {
    let [v] = values else {
        return Err(unsupported_filter());
    };
    let v = value_arg(v, "key")?;
    let wrap = |pre: bool, post: bool, fold: bool| {
        let mut g = Vec::new();
        if pre {
            g.push(b'*');
        }
        g.extend(glob_literal(&v, fold));
        if post {
            g.push(b'*');
        }
        KeyMatch::Pattern(g)
    };
    Ok(match op {
        FilterOp::Eq => KeyMatch::Exact(v),
        FilterOp::Like => KeyMatch::Pattern(like_glob(&v, false)),
        FilterOp::Ilike => KeyMatch::Pattern(like_glob(&v, true)),
        FilterOp::Contains => wrap(true, true, false),
        FilterOp::ContainsCase => wrap(true, true, true),
        FilterOp::HasPrefix => wrap(false, true, false),
        FilterOp::HasPrefixCase => wrap(false, true, true),
        FilterOp::HasSuffix => wrap(true, false, false),
        FilterOp::HasSuffixCase => wrap(true, false, true),
        _ => return Err(unsupported_filter()),
    })
}

/// A key of a page after the first round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRow {
    pub key: Vec<u8>,
    pub ty: KeyType,
    /// `PTTL`: milliseconds, -1 without expiry.
    pub pttl: i64,
}

/// Round one: `TYPE` and `PTTL` of every key, in that order, for one pipeline.
pub fn meta_commands(keys: &[Vec<u8>]) -> Vec<Command> {
    keys.iter()
        .flat_map(|k| [cmd::key_type(k), cmd::pttl(k)])
        .collect()
}

/// The rows of round one's replies: keys gone meanwhile (`none`) and keys of another type than
/// the filter's are dropped.
pub fn parse_meta(keys: &[Vec<u8>], replies: &[Resp], filter: &KeyFilter) -> Vec<KeyRow> {
    keys.iter()
        .zip(replies.chunks(2))
        .filter_map(|(key, pair)| {
            let [ty, pttl] = pair else { return None };
            let ty = KeyType::from_reply(ty);
            (ty.exists() && filter.matches_type(&ty)).then(|| KeyRow {
                key: key.clone(),
                ty,
                pttl: as_int(pttl).unwrap_or(-1),
            })
        })
        .collect()
}

/// Round two: per row its size, value preview and (when asked) memory, with where each reply
/// lands.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DetailPlan {
    pub commands: Vec<Command>,
    /// Per row: the reply indexes of its size, preview and memory.
    pub slots: Vec<[Option<usize>; 3]>,
    /// What the previews read, which tells a whole value from a cut one.
    pub limits: Limits,
}

pub fn detail_commands(rows: &[KeyRow], limits: &Limits, with_memory: bool) -> DetailPlan {
    let mut plan = DetailPlan {
        limits: *limits,
        ..DetailPlan::default()
    };
    for row in rows {
        let mut push = |c: Option<Command>| {
            c.map(|c| {
                plan.commands.push(c);
                plan.commands.len() - 1
            })
        };
        let size = push(row.ty.size_command(&row.key));
        let preview = push(row.ty.preview_command(&row.key, limits));
        let memory = push(with_memory.then(|| cmd::memory_usage(&row.key)));
        plan.slots.push([size, preview, memory]);
    }
    plan
}

/// The page's rows (`keys::columns` order: key, type, ttl, size, memory, value). A reply that is
/// missing or an error leaves its cell NULL.
pub fn assemble(rows: &[KeyRow], plan: &DetailPlan, replies: &[Resp]) -> Vec<Vec<Value>> {
    let at = |i: Option<usize>| i.and_then(|i| replies.get(i));
    rows.iter()
        .zip(&plan.slots)
        .map(|(row, [size, preview, memory])| {
            let size = size_cell(at(*size));
            let value = at(*preview).map_or(Value::Null, |r| {
                preview_cell(&row.ty, r, size, &plan.limits)
            });
            vec![
                key_value(&row.key),
                Value::Text(row.ty.name().to_string()),
                ttl_cell(row.pttl),
                size.map_or(Value::Null, Value::from_i64),
                at(*memory).map_or(Value::Null, int_cell),
                value,
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::browse::Conjunction;
    use ::redis::parse_redis_value;

    fn p(s: &str) -> Resp {
        parse_redis_value(s.as_bytes()).unwrap()
    }

    fn cond(column: &str, op: FilterOp, v: &str) -> FilterNode {
        FilterNode::Condition {
            column: column.into(),
            op,
            values: vec![Value::Text(v.into())],
        }
    }

    fn and(items: Vec<FilterNode>) -> FilterGroup {
        FilterGroup {
            conjunction: Conjunction::And,
            items,
        }
    }

    fn glob(f: &KeyFilter) -> String {
        match &f.keys {
            KeyMatch::Pattern(p) => String::from_utf8(p.clone()).unwrap(),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn cursors_are_decimal_strings() {
        assert_eq!(cursor(None).unwrap(), "0");
        assert_eq!(cursor(Some(" ")).unwrap(), "0");
        assert_eq!(cursor(Some("17")).unwrap(), "17");
        assert_eq!(cursor(Some("0017")).unwrap(), "17");
        assert_eq!(cursor(Some("000")).unwrap(), "0");
        assert_eq!(
            cursor(Some("18446744073709551615")).unwrap(),
            "18446744073709551615"
        );
        for bad in ["-1", "1e3", "18446744073709551616", "abc", "1 2", "+5"] {
            assert!(cursor(Some(bad)).is_err(), "{bad}");
        }
        assert_eq!(next_token("0"), None);
        assert_eq!(next_token("42").as_deref(), Some("42"));
    }

    #[test]
    fn scan_replies() {
        let r = parse_scan(&p(
            "*2\r\n$2\r\n17\r\n*3\r\n$1\r\na\r\n$1\r\nb\r\n$1\r\na\r\n",
        ))
        .unwrap();
        assert_eq!(r.cursor, "17");
        assert_eq!(scan_keys(&r), [b"a".to_vec(), b"b".to_vec()]);
        let r = parse_scan(&p("*2\r\n$1\r\n0\r\n*0\r\n")).unwrap();
        assert_eq!((r.cursor.as_str(), r.items.len()), ("0", 0));
        let binary = Resp::Array(vec![
            Resp::BulkString(b"0".to_vec()),
            Resp::Array(vec![Resp::BulkString(vec![0xff, 0x00]), Resp::Int(1)]),
        ]);
        assert_eq!(scan_keys(&parse_scan(&binary).unwrap()), [vec![0xff, 0x00]]);
        for bad in [
            "*1\r\n$1\r\n0\r\n",
            ":1\r\n",
            "*2\r\n$2\r\nxx\r\n*0\r\n",
            "*2\r\n$1\r\n0\r\n:3\r\n",
        ] {
            assert!(parse_scan(&p(bad)).is_err(), "{bad:?}");
        }
        let err = parse_scan(&p("-ERR invalid cursor\r\n")).unwrap_err();
        assert!(err.to_string().contains("invalid cursor"));
    }

    #[test]
    fn filters_become_globs() {
        assert_eq!(key_filter(None).unwrap().keys, KeyMatch::All);
        let f = |n: FilterNode| key_filter(Some(&and(vec![n]))).unwrap();
        assert_eq!(
            f(cond("key", FilterOp::Eq, "user:1")).keys,
            KeyMatch::Exact(b"user:1".to_vec())
        );
        assert_eq!(glob(&f(cond("key", FilterOp::Contains, "a*b"))), "*a\\*b*");
        assert_eq!(
            glob(&f(cond("key", FilterOp::HasPrefix, "user:"))),
            "user:*"
        );
        assert_eq!(
            glob(&f(cond("key", FilterOp::HasSuffix, "[x]"))),
            "*\\[x\\]"
        );
        assert_eq!(
            glob(&f(cond("key", FilterOp::HasPrefixCase, "Ab1"))),
            "[aA][bB]1*"
        );
        assert_eq!(glob(&f(cond("key", FilterOp::ContainsCase, "?"))), "*\\?*");
        assert_eq!(glob(&f(cond("key", FilterOp::HasSuffixCase, "z"))), "*[zZ]");
        assert_eq!(
            glob(&f(cond("key", FilterOp::Like, "user:%:_\\%"))),
            "user:*:?%"
        );
        assert_eq!(glob(&f(cond("key", FilterOp::Ilike, "a%"))), "[aA]*");
        assert_eq!(glob(&f(cond("key", FilterOp::Like, "a\\"))), "a\\\\");
        assert_eq!(
            glob(&f(FilterNode::Raw {
                sql: " session:[0-9]* ".into()
            })),
            "session:[0-9]*"
        );
        assert_eq!(f(FilterNode::Raw { sql: " ".into() }).keys, KeyMatch::All);
        assert_eq!(
            glob(&f(FilterNode::AnyColumn {
                op: FilterOp::Contains,
                values: vec![Value::Text("x".into())]
            })),
            "*x*"
        );
        // Binary values and ints are matched as their bytes.
        assert_eq!(
            f(FilterNode::Condition {
                column: "key".into(),
                op: FilterOp::Eq,
                values: vec![Value::from_bytes(&[0xff])]
            })
            .keys,
            KeyMatch::Exact(vec![0xff])
        );
        let both = key_filter(Some(&and(vec![
            cond("type", FilterOp::Eq, "hash"),
            FilterNode::Group(and(vec![cond("key", FilterOp::HasPrefix, "h:")])),
        ])))
        .unwrap();
        assert_eq!(both.ty.as_deref(), Some("hash"));
        assert_eq!(glob(&both), "h:*");
    }

    #[test]
    fn unsupported_filters() {
        let refused = |g: FilterGroup| key_filter(Some(&g)).is_err();
        assert!(refused(and(vec![cond("ttl", FilterOp::Gt, "5")])));
        assert!(refused(and(vec![cond("value", FilterOp::Contains, "x")])));
        assert!(refused(and(vec![cond("key", FilterOp::Ne, "x")])));
        assert!(refused(and(vec![cond("type", FilterOp::Ne, "hash")])));
        assert!(refused(and(vec![
            cond("key", FilterOp::HasPrefix, "a"),
            cond("key", FilterOp::HasSuffix, "b")
        ])));
        assert!(refused(and(vec![
            cond("type", FilterOp::Eq, "hash"),
            cond("type", FilterOp::Eq, "set")
        ])));
        assert!(refused(FilterGroup {
            conjunction: Conjunction::Or,
            items: vec![
                cond("key", FilterOp::Eq, "a"),
                cond("key", FilterOp::Eq, "b")
            ]
        }));
        assert!(refused(and(vec![FilterNode::Condition {
            column: "key".into(),
            op: FilterOp::In,
            values: vec![Value::Text("a".into()), Value::Text("b".into())]
        }])));
        assert!(refused(and(vec![FilterNode::Condition {
            column: "key".into(),
            op: FilterOp::Eq,
            values: vec![Value::Null]
        }])));
        // One condition in an OR group is fine.
        assert!(!refused(FilterGroup {
            conjunction: Conjunction::Or,
            items: vec![cond("key", FilterOp::Eq, "a")]
        }));
    }

    #[test]
    fn scan_commands() {
        let g = Gates::default();
        let all = key_filter(None).unwrap();
        assert_eq!(
            all.scan_command("0", 100, &g).unwrap().text(),
            "SCAN 0 COUNT 100"
        );
        let typed = KeyFilter {
            keys: KeyMatch::Pattern(b"a b*".to_vec()),
            ty: Some("zset".into()),
        };
        assert_eq!(
            typed.scan_command("9", 50, &g).unwrap().text(),
            "SCAN 9 MATCH \"a b*\" COUNT 50 TYPE zset"
        );
        // Before 6.0 the type is filtered after the TYPE round.
        let old = Gates {
            scan_type: false,
            ..g
        };
        assert_eq!(
            typed.scan_command("9", 50, &old).unwrap().text(),
            "SCAN 9 MATCH \"a b*\" COUNT 50"
        );
        assert!(typed.matches_type(&KeyType::ZSet));
        assert!(!typed.matches_type(&KeyType::Hash));
        let exact = KeyFilter {
            keys: KeyMatch::Exact(b"k".to_vec()),
            ty: None,
        };
        assert!(exact.scan_command("0", 10, &g).is_none());
    }

    #[test]
    fn page_assembly() {
        let keys = vec![
            b"s".to_vec(),
            b"gone".to_vec(),
            b"h".to_vec(),
            b"ts".to_vec(),
            vec![0xff],
        ];
        let meta = meta_commands(&keys);
        assert_eq!(meta.len(), 10);
        assert_eq!(meta[0].text(), "TYPE s");
        assert_eq!(meta[1].text(), "PTTL s");
        let replies = [
            p("+string\r\n"),
            p(":1500\r\n"),
            p("+none\r\n"),
            p(":-2\r\n"),
            p("+hash\r\n"),
            p(":-1\r\n"),
            p("+TSDB-TYPE\r\n"),
            p(":-1\r\n"),
            p("+list\r\n"),
            p("-ERR boom\r\n"),
        ];
        let all = key_filter(None).unwrap();
        let rows = parse_meta(&keys, &replies, &all);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].pttl, 1500);
        assert_eq!(rows[3].pttl, -1);
        let only_hashes = KeyFilter {
            keys: KeyMatch::All,
            ty: Some("hash".into()),
        };
        assert_eq!(parse_meta(&keys, &replies, &only_hashes).len(), 1);
        // Short replies drop the keys they do not cover.
        assert_eq!(parse_meta(&keys, &replies[..3], &all).len(), 1);

        let plan = detail_commands(
            &rows,
            &Limits {
                bytes: 4,
                elements: 2,
            },
            true,
        );
        let texts: Vec<String> = plan.commands.iter().map(Command::text).collect();
        assert_eq!(
            texts,
            [
                "STRLEN s",
                "GETRANGE s 0 3",
                "MEMORY USAGE s SAMPLES 0",
                "HLEN h",
                "HSCAN h 0 COUNT 2",
                "MEMORY USAGE h SAMPLES 0",
                "MEMORY USAGE ts SAMPLES 0",
                "LLEN \"\\xff\"",
                "LRANGE \"\\xff\" 0 1",
                "MEMORY USAGE \"\\xff\" SAMPLES 0",
            ]
        );
        let details = [
            p(":10\r\n"),
            p("$4\r\nhell\r\n"),
            p(":72\r\n"),
            p(":1\r\n"),
            p("*2\r\n$1\r\n0\r\n*2\r\n$1\r\nf\r\n$1\r\nv\r\n"),
            p("-ERR MEMORY disabled\r\n"),
            p(":300\r\n"),
            p(":2\r\n"),
            p("*2\r\n$1\r\na\r\n$1\r\nb\r\n"),
            // The last reply is missing (a short pipeline): its cell stays NULL.
        ];
        let page = assemble(&rows, &plan, &details);
        assert_eq!(page.len(), 4);
        assert_eq!(page[0][0], Value::Text("s".into()));
        assert_eq!(page[0][1], Value::Text("string".into()));
        assert_eq!(page[0][2], Value::Int(2));
        assert_eq!(page[0][3], Value::Int(10));
        assert_eq!(page[0][4], Value::Int(72));
        assert!(matches!(&page[0][5], Value::Bytes(b) if b.truncated && b.len == 10));
        assert_eq!(page[1][2], Value::Null);
        assert_eq!(page[1][4], Value::Null);
        assert_eq!(page[1][5], Value::Json(r#"{"f":"v"}"#.into()));
        assert_eq!(page[2][1], Value::Text("TSDB-TYPE".into()));
        assert_eq!(page[2][3], Value::Null);
        assert_eq!(page[2][4], Value::Int(300));
        assert_eq!(page[2][5], Value::Null);
        assert!(matches!(&page[3][0], Value::Bytes(b) if b.len == 1));
        assert_eq!(page[3][2], Value::Null);
        assert_eq!(page[3][5], Value::Json(r#"["a","b"]"#.into()));
        assert_eq!(page[3][4], Value::Null);
        // STRLEN refused (an ACL): the 4 bytes read fill the cap, so they stay a cut preview.
        let mut no_strlen = details.to_vec();
        no_strlen[0] = p("-NOPERM no permissions to run the 'strlen' command\r\n");
        let page = assemble(&rows, &plan, &no_strlen);
        assert_eq!(page[0][3], Value::Null);
        assert!(matches!(&page[0][5], Value::Bytes(b) if b.truncated && b.len == 4));
        let without_memory = detail_commands(&rows, &Limits::default(), false);
        assert_eq!(without_memory.commands.len(), 6);
        assert!(without_memory.slots.iter().all(|s| s[2].is_none()));
    }
}
