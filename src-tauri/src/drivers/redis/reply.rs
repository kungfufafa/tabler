//! A console command's reply as a result grid. A scalar is one `value` cell; an array is
//! `(#, value)` rows numbered from 1 like redis-cli; a map (RESP3) or the flat pairs of `HGETALL`
//! / `CONFIG GET` (RESP2) are `(field, value)`; `… WITHSCORES` and `ZPOPMIN` are
//! `(member, score)` in both protocols; a `SCAN` family reply is its items, with the next cursor
//! as a notice; `INFO` is `(section, field, value)`; nested replies are JSON cells.

use crate::error::AppError;
use crate::types::query::ResultColumn;
use crate::types::schema::ValueKind;
use crate::types::value::Value;

use super::cli::command_name;
use super::info::Info;
use super::values::{as_items, as_pairs, as_scored, as_text, cell, key_value, number_cell};

type Resp = ::redis::Value;

/// A shaped reply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shaped {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Value>>,
    /// More rows than `max_rows`; the rest were dropped.
    pub truncated: bool,
    pub notices: Vec<String>,
}

fn column(name: &str, data_type: &str, kind: ValueKind) -> ResultColumn {
    ResultColumn {
        name: name.to_string(),
        data_type: data_type.to_string(),
        kind,
        nullable: Some(true),
        table: None,
        origin: None,
    }
}

/// Commands whose RESP2 array reply is field / value pairs.
fn flat_pairs(name: &str, argv: &[Vec<u8>]) -> bool {
    let sub = |s: &str| {
        argv.get(1)
            .is_some_and(|a| a.eq_ignore_ascii_case(s.as_bytes()))
    };
    match name {
        "HGETALL" => true,
        "CONFIG" => sub("GET"),
        "XINFO" => sub("STREAM"),
        "MEMORY" => sub("STATS"),
        "HRANDFIELD" => has_word(argv, "WITHVALUES"),
        _ => false,
    }
}

fn has_word(argv: &[Vec<u8>], word: &str) -> bool {
    argv.iter()
        .skip(1)
        .any(|a| a.eq_ignore_ascii_case(word.as_bytes()))
}

fn scored(name: &str, argv: &[Vec<u8>]) -> bool {
    matches!(name, "ZPOPMIN" | "ZPOPMAX") || (name.starts_with('Z') && has_word(argv, "WITHSCORES"))
}

/// Shapes a reply of the command `argv`; a server error is the command's error.
pub fn shape(argv: &[Vec<u8>], reply: &Resp, max_rows: usize) -> Result<Shaped, AppError> {
    let name = command_name(argv).unwrap_or_default();
    let mut out = match reply {
        Resp::ServerError(e) => return Err(super::errors::server_error(e)),
        Resp::Attribute { data, .. } => return shape(argv, data, max_rows),
        _ if name == "INFO" && as_text(reply).is_some() => info_rows(reply),
        _ if matches!(name.as_str(), "SCAN" | "SSCAN" | "HSCAN" | "ZSCAN") => {
            scan_rows(&name, argv, reply).unwrap_or_else(|| indexed(reply))
        }
        Resp::Map(_) => pair_rows(reply, "field"),
        Resp::Array(_) if scored(&name, argv) => {
            scored_rows(reply).unwrap_or_else(|| indexed(reply))
        }
        Resp::Array(_) if flat_pairs(&name, argv) => match as_pairs(reply) {
            Some(_) => pair_rows(reply, "field"),
            None => indexed(reply),
        },
        Resp::Array(_) | Resp::Set(_) => indexed(reply),
        scalar => Shaped {
            columns: vec![column("value", "reply", ValueKind::Other)],
            rows: vec![vec![cell(scalar)]],
            ..Shaped::default()
        },
    };
    if out.rows.len() > max_rows {
        out.rows.truncate(max_rows);
        out.truncated = true;
    }
    Ok(out)
}

fn indexed(reply: &Resp) -> Shaped {
    let items = as_items(reply).unwrap_or_default();
    Shaped {
        columns: vec![
            column("#", "integer", ValueKind::Int),
            column("value", "reply", ValueKind::Other),
        ],
        rows: items
            .iter()
            .enumerate()
            .map(|(i, v)| vec![Value::from_u64(i as u64 + 1), cell(v)])
            .collect(),
        ..Shaped::default()
    }
}

fn pair_rows(reply: &Resp, first: &str) -> Shaped {
    let pairs = as_pairs(reply).unwrap_or_default();
    Shaped {
        columns: vec![
            column(first, "reply", ValueKind::Text),
            column("value", "reply", ValueKind::Other),
        ],
        rows: pairs.iter().map(|(k, v)| vec![cell(k), cell(v)]).collect(),
        ..Shaped::default()
    }
}

fn scored_rows(reply: &Resp) -> Option<Shaped> {
    // ZPOPMIN without a count answers one pair, not a list of them (RESP3).
    let pairs = as_scored(reply)?;
    Some(Shaped {
        columns: vec![
            column("member", "reply", ValueKind::Text),
            column("score", "double", ValueKind::Float),
        ],
        rows: pairs
            .iter()
            .map(|(m, s)| vec![cell(m), number_cell(s)])
            .collect(),
        ..Shaped::default()
    })
}

fn scan_rows(name: &str, argv: &[Vec<u8>], reply: &Resp) -> Option<Shaped> {
    let [cursor, items] = as_items(reply)? else {
        return None;
    };
    let cursor = as_text(cursor)?;
    let mut out = match name {
        "HSCAN" if !has_word(argv, "NOVALUES") => pair_rows(items, "field"),
        "ZSCAN" => scored_rows(items)?,
        _ => {
            let label = match name {
                "SCAN" => "key",
                "HSCAN" => "field",
                _ => "member",
            };
            Shaped {
                columns: vec![column(label, "reply", ValueKind::Text)],
                rows: as_items(items)?
                    .iter()
                    .map(|k| {
                        vec![match k {
                            Resp::BulkString(b) => key_value(b),
                            other => cell(other),
                        }]
                    })
                    .collect(),
                ..Shaped::default()
            }
        }
    };
    out.notices.push(if cursor == "0" {
        "cursor 0: the scan is complete".to_string()
    } else {
        format!("next cursor: {cursor}")
    });
    Some(out)
}

fn info_rows(reply: &Resp) -> Shaped {
    let info = Info::from_reply(reply);
    Shaped {
        columns: vec![
            column("section", "reply", ValueKind::Text),
            column("field", "reply", ValueKind::Text),
            column("value", "reply", ValueKind::Text),
        ],
        rows: info
            .sections
            .iter()
            .flat_map(|(section, fields)| {
                fields.iter().map(move |(k, v)| {
                    vec![
                        Value::Text(section.clone()),
                        Value::Text(k.clone()),
                        Value::Text(v.clone()),
                    ]
                })
            })
            .collect(),
        ..Shaped::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::redis::cli::tokenize;
    use ::redis::parse_redis_value;

    fn run(line: &str, reply: &[u8]) -> Shaped {
        shape(
            &tokenize(line).unwrap(),
            &parse_redis_value(reply).unwrap(),
            100,
        )
        .unwrap()
    }

    fn names(s: &Shaped) -> Vec<&str> {
        s.columns.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn scalars() {
        let s = run("SET k v", b"+OK\r\n");
        assert_eq!(names(&s), ["value"]);
        assert_eq!(s.rows, [[Value::Text("OK".into())]]);
        assert_eq!(run("INCR n", b":3\r\n").rows, [[Value::Int(3)]]);
        assert_eq!(run("GET nope", b"$-1\r\n").rows, [[Value::Null]]);
        assert_eq!(run("GET nope", b"_\r\n").rows, [[Value::Null]]);
        assert_eq!(run("ZSCORE z m", b",1.5\r\n").rows, [[Value::Float(1.5)]]);
    }

    #[test]
    fn arrays_are_numbered() {
        let s = run("LRANGE l 0 -1", b"*3\r\n$1\r\na\r\n$-1\r\n*1\r\n:1\r\n");
        assert_eq!(names(&s), ["#", "value"]);
        assert_eq!(
            s.rows,
            [
                vec![Value::Int(1), Value::Text("a".into())],
                vec![Value::Int(2), Value::Null],
                vec![Value::Int(3), Value::Json("[1]".into())],
            ]
        );
        let set = run("SMEMBERS s", b"~2\r\n$1\r\na\r\n$1\r\nb\r\n");
        assert_eq!(set.rows.len(), 2);
        // An EXEC reply keeps each command's error as a cell.
        let exec = run("EXEC", b"*2\r\n+OK\r\n-ERR boom\r\n");
        assert_eq!(exec.rows[1][1], Value::other("error", "ERR boom"));
    }

    #[test]
    fn pairs_in_both_protocols() {
        let resp2 = run(
            "HGETALL h",
            b"*4\r\n$1\r\nf\r\n$1\r\nv\r\n$1\r\ng\r\n$1\r\nw\r\n",
        );
        let resp3 = run(
            "HGETALL h",
            b"%2\r\n$1\r\nf\r\n$1\r\nv\r\n$1\r\ng\r\n$1\r\nw\r\n",
        );
        assert_eq!(resp2, resp3);
        assert_eq!(names(&resp2), ["field", "value"]);
        assert_eq!(
            resp2.rows[1],
            [Value::Text("g".into()), Value::Text("w".into())]
        );
        let config = run(
            "config get maxmemory*",
            b"*2\r\n$9\r\nmaxmemory\r\n$1\r\n0\r\n",
        );
        assert_eq!(names(&config), ["field", "value"]);
        // An odd array for a pairs command stays numbered.
        assert_eq!(
            names(&run("HGETALL h", b"*1\r\n$1\r\nf\r\n")),
            ["#", "value"]
        );
        // LRANGE of an even list is not pairs.
        assert_eq!(
            names(&run("LRANGE l 0 1", b"*2\r\n$1\r\na\r\n$1\r\nb\r\n")),
            ["#", "value"]
        );
    }

    #[test]
    fn scores_in_both_protocols() {
        let resp2 = run(
            "ZRANGE z 0 -1 WITHSCORES",
            b"*4\r\n$1\r\na\r\n$1\r\n1\r\n$1\r\nb\r\n$3\r\n2.5\r\n",
        );
        let resp3 = run(
            "zrange z 0 -1 withscores",
            b"*2\r\n*2\r\n$1\r\na\r\n,1\r\n*2\r\n$1\r\nb\r\n,2.5\r\n",
        );
        assert_eq!(resp2, resp3);
        assert_eq!(names(&resp2), ["member", "score"]);
        assert_eq!(resp2.rows[1], [Value::Text("b".into()), Value::Float(2.5)]);
        assert_eq!(
            names(&run("ZPOPMIN z", b"*2\r\n$1\r\na\r\n$1\r\n1\r\n")),
            ["member", "score"]
        );
        assert_eq!(
            names(&run("ZRANGE z 0 -1", b"*1\r\n$1\r\na\r\n")),
            ["#", "value"]
        );
    }

    #[test]
    fn scans() {
        let s = run(
            "SCAN 0 COUNT 2",
            b"*2\r\n$2\r\n12\r\n*2\r\n$1\r\na\r\n$2\r\n\xff\x00\r\n",
        );
        assert_eq!(names(&s), ["key"]);
        assert_eq!(s.rows[0], [Value::Text("a".into())]);
        assert!(matches!(&s.rows[1][0], Value::Bytes(b) if b.len == 2));
        assert_eq!(s.notices, ["next cursor: 12"]);
        let h = run(
            "HSCAN h 0",
            b"*2\r\n$1\r\n0\r\n*2\r\n$1\r\nf\r\n$1\r\nv\r\n",
        );
        assert_eq!(names(&h), ["field", "value"]);
        assert_eq!(h.notices, ["cursor 0: the scan is complete"]);
        let hn = run("HSCAN h 0 NOVALUES", b"*2\r\n$1\r\n0\r\n*1\r\n$1\r\nf\r\n");
        assert_eq!(names(&hn), ["field"]);
        let z = run(
            "ZSCAN z 0",
            b"*2\r\n$1\r\n0\r\n*2\r\n$1\r\nm\r\n$1\r\n3\r\n",
        );
        assert_eq!(z.rows, [[Value::Text("m".into()), Value::Float(3.0)]]);
        assert_eq!(
            names(&run("SSCAN s 0", b"*2\r\n$1\r\n0\r\n*0\r\n")),
            ["member"]
        );
        // A reply of another shape is shown as it is.
        assert_eq!(names(&run("SCAN 0", b"*1\r\n:1\r\n")), ["#", "value"]);
    }

    #[test]
    fn info_and_errors_and_caps() {
        let s = run(
            "INFO",
            b"$39\r\n# Server\r\nredis_version:7.2.4\r\nos:Linux\r\n",
        );
        assert_eq!(names(&s), ["section", "field", "value"]);
        assert_eq!(s.rows.len(), 2);
        assert_eq!(s.rows[0][0], Value::Text("Server".into()));
        let v = run(
            "INFO",
            b"=39\r\ntxt:# Server\r\nredis_version:7.2.4\r\nos:x\r\n",
        );
        assert_eq!(v.rows.len(), 2);
        let err = shape(
            &tokenize("GET h").unwrap(),
            &parse_redis_value(
                b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n",
            )
            .unwrap(),
            10,
        )
        .unwrap_err();
        assert!(err.to_string().starts_with("WRONGTYPE"));
        let many = shape(
            &tokenize("LRANGE l 0 -1").unwrap(),
            &parse_redis_value(b"*3\r\n:1\r\n:2\r\n:3\r\n").unwrap(),
            2,
        )
        .unwrap();
        assert_eq!(many.rows.len(), 2);
        assert!(many.truncated);
        let attr = run("GET k", b"|1\r\n+ttl\r\n:3\r\n$1\r\nv\r\n");
        assert_eq!(attr.rows, [[Value::Text("v".into())]]);
        assert_eq!(
            names(&shape(&[], &::redis::Value::Int(1), 1).unwrap()),
            ["value"]
        );
    }
}
