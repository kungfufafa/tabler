//! The lane's codecs and builders against the fake server, through the redis crate's own client
//! (so the fake's RESP and the decoders meet real parsing): the handshake in RESP2 and RESP3,
//! introspection, browsing every key type page by page, filters, edits applied the way the
//! session will (WATCH, TYPE, MULTI … EXEC, a result per change), the console, and cancel.

use std::time::Duration;

use ::redis::AsyncConnectionConfig;
use ::redis::Value as Resp;
use ::redis::aio::{ConnectionLike, MultiplexedConnection};

use super::classify::{ServerReads, classify_script};
use super::cli::{split_console, tokenize};
use super::errors::redis_error;
use super::info::{self, Info, Server};
use super::keys::Limits;
use super::reply::shape;
use super::scan::{
    KeyFilter, KeyMatch, assemble, cursor, detail_commands, key_filter, meta_commands, next_token,
    parse_meta, parse_scan, scan_keys,
};
use super::testing::{Data, FakeRedis, Options};
use super::values::as_bytes;
use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::native::redis::{
    self as cmd, Command, Expect, Gates, Guard, KeyEdit, col, plan_changes,
};
use crate::types::browse::{Conjunction, FilterGroup, FilterNode, FilterOp, RowKey};
use crate::types::mutate::StagedChange;
use crate::types::value::Value;

fn config() -> AsyncConnectionConfig {
    // redis-rs gives up on a reply after 500 ms by default; the session sets its own limit.
    AsyncConnectionConfig::new()
        .set_response_timeout(Some(Duration::from_secs(10)))
        .set_connection_timeout(Some(Duration::from_secs(5)))
}

async fn try_connect(
    fake: &FakeRedis,
    db: u32,
    resp3: bool,
) -> Result<MultiplexedConnection, ::redis::RedisError> {
    ::redis::Client::open(fake.url(db, resp3))?
        .get_multiplexed_async_connection_with_config(&config())
        .await
}

async fn connect(fake: &FakeRedis, db: u32, resp3: bool) -> MultiplexedConnection {
    try_connect(fake, db, resp3)
        .await
        .expect("connect to the fake")
}

fn packed(c: &Command) -> ::redis::Cmd {
    let mut out = ::redis::cmd(&String::from_utf8_lossy(&c.argv[0]));
    for a in &c.argv[1..] {
        out.arg(a.as_slice());
    }
    out
}

/// One command, its reply raw (a server error stays a value).
async fn send(con: &mut MultiplexedConnection, c: &Command) -> Resp {
    con.req_packed_command(&packed(c)).await.expect("a reply")
}

/// Commands pipelined, their replies raw.
async fn pipeline(con: &mut MultiplexedConnection, cmds: &[Command]) -> Vec<Resp> {
    if cmds.is_empty() {
        return Vec::new();
    }
    let mut p = ::redis::pipe();
    for c in cmds {
        p.add_command(packed(c));
    }
    con.req_packed_commands(&p, 0, cmds.len())
        .await
        .expect("replies")
}

/// Every key type in db 1, plus a binary key and keys with TTLs.
fn seed(fake: &FakeRedis) {
    let ttl = Some(Duration::from_secs(100));
    fake.seed(1, b"str:plain", Data::str("hello"), None);
    fake.seed(1, b"str:big", Data::Str(vec![b'x'; 100]), ttl);
    fake.seed(1, b"hash:1", Data::hash(&[("f", "v"), ("g", "w")]), None);
    fake.seed(1, b"list:1", Data::list(&["a", "b", "c"]), None);
    fake.seed(1, b"set:1", Data::set(&["m"]), None);
    fake.seed(1, b"zset:1", Data::zset(&[("a", 1.0), ("b", 2.5)]), None);
    fake.seed(
        1,
        b"stream:1",
        Data::stream(&[("1-0", &[("f", "1")]), ("2-0", &[("f", "2")])]),
        None,
    );
    fake.seed(1, b"json:1", Data::Json("{\"a\":[1,2]}".into()), None);
    fake.seed(1, b"ts:1", Data::Module("TSDB-TYPE".into()), None);
    fake.seed(1, &[0xff, b':', 0x00], Data::str("bin"), None);
    for i in 0..12 {
        fake.seed(
            1,
            format!("user:{i:02}").as_bytes(),
            Data::str(&i.to_string()),
            None,
        );
    }
}

/// Browses db 1 page by page the way the session will: SCAN, then TYPE + PTTL, then size +
/// preview, until the cursor is back at 0.
async fn browse_all(
    con: &mut MultiplexedConnection,
    filter: &KeyFilter,
    gates: &Gates,
) -> (Vec<Vec<Value>>, usize) {
    let limits = Limits {
        bytes: 16,
        elements: 10,
    };
    let mut token: Option<String> = None;
    let mut rows = Vec::new();
    let mut pages = 0;
    loop {
        let c = cursor(token.as_deref()).unwrap();
        let keys = match &filter.keys {
            KeyMatch::Exact(k) => vec![k.clone()],
            _ => {
                let scan = filter.scan_command(&c, 5, gates).unwrap();
                let reply = parse_scan(&send(con, &scan).await).unwrap();
                token = next_token(&reply.cursor);
                scan_keys(&reply)
            }
        };
        pages += 1;
        let meta = pipeline(con, &meta_commands(&keys)).await;
        let found = parse_meta(&keys, &meta, filter);
        let plan = detail_commands(&found, &limits, true);
        let details = pipeline(con, &plan.commands).await;
        rows.extend(assemble(&found, &plan, &details));
        if token.is_none() || matches!(filter.keys, KeyMatch::Exact(_)) {
            break;
        }
    }
    (rows, pages)
}

fn row<'a>(rows: &'a [Vec<Value>], key: &str) -> &'a [Value] {
    rows.iter()
        .find(|r| r[0] == Value::Text(key.into()))
        .unwrap_or_else(|| panic!("no row {key}"))
}

#[tokio::test]
async fn handshake_in_both_protocols_with_acl_auth() {
    let fake = FakeRedis::with(Options {
        user: Some("app".into()),
        password: Some("s3cret".into()),
        ..Options::default()
    })
    .await;
    for resp3 in [false, true] {
        let mut con = connect(&fake, 2, resp3).await;
        assert_eq!(
            send(&mut con, &cmd::ping()).await,
            Resp::SimpleString("PONG".into())
        );
        // SELECT 2 was part of the handshake.
        fake.seed(2, b"here", Data::str("x"), None);
        assert_eq!(send(&mut con, &cmd::dbsize()).await, Resp::Int(1));
    }
    let log = fake.log();
    assert!(log.iter().any(|l| l == "AUTH app s3cret"), "{log:?}");
    assert!(
        log.iter().any(|l| l == "HELLO 3 AUTH app s3cret"),
        "{log:?}"
    );
    assert!(log.iter().any(|l| l == "SELECT 2"));
}

#[tokio::test]
async fn bad_credentials_are_auth_errors() {
    let fake = FakeRedis::with(Options {
        password: Some("right".into()),
        ..Options::default()
    })
    .await;
    for resp3 in [false, true] {
        let url = format!(
            "redis://:wrong@{}/0{}",
            fake.addr,
            if resp3 { "?protocol=resp3" } else { "" }
        );
        let err = ::redis::Client::open(url)
            .unwrap()
            .get_multiplexed_async_connection_with_config(&config())
            .await
            .expect_err("refused");
        assert!(matches!(redis_error(&err), AppError::Auth(_)), "{err:?}");
    }
    // Without a password the server answers NOAUTH.
    let err = ::redis::Client::open(format!("redis://{}/0", fake.addr))
        .unwrap()
        .get_multiplexed_async_connection_with_config(&config())
        .await;
    let mut con = match err {
        Ok(con) => con,
        Err(e) => panic!("{e:?}"),
    };
    let Resp::ServerError(e) = send(&mut con, &cmd::dbsize()).await else {
        panic!()
    };
    assert!(matches!(super::errors::server_error(&e), AppError::Auth(_)));
}

#[tokio::test]
async fn introspection_with_fallbacks() {
    let fake = FakeRedis::with(Options {
        product: Some(("valkey".into(), "8.0.1".into())),
        databases: 32,
        ..Options::default()
    })
    .await;
    seed(&fake);
    let mut con = connect(&fake, 0, true).await;
    let info = Info::from_reply(&send(&mut con, &cmd::info(None)).await);
    let server = Server::identify(&info);
    assert_eq!(server.label(), "Valkey 8.0.1");
    assert_eq!(server.gates(), Gates::default());
    let ks = info::keyspace(&info);
    assert_eq!(ks.len(), 1);
    assert_eq!((ks[0].db, ks[0].keys, ks[0].expires), (1, 22, 1));
    let config = info::config_databases(&send(&mut con, &cmd::config_get("databases")).await);
    assert_eq!(info::database_count(config, &ks), 32);
    let tables = info::tables(info::database_count(config, &ks), &ks, "db0");
    assert_eq!(tables[1].row_estimate, Some(22));
    let reads = ServerReads::from_reply(&send(&mut con, &cmd::command_table()).await);
    assert!(reads.contains("ts.range") && reads.contains("object|freq"));
    assert!(!reads.contains("pfcount") && !reads.contains("set"));

    // A managed service: CONFIG refused, an old version.
    let managed = FakeRedis::with(Options {
        refuse_config: true,
        version: "5.0.14".into(),
        ..Options::default()
    })
    .await;
    let mut con = connect(&managed, 0, false).await;
    let refused = send(&mut con, &cmd::config_get("databases")).await;
    assert!(matches!(refused, Resp::ServerError(_)));
    assert_eq!(
        info::database_count(info::config_databases(&refused), &[]),
        16
    );
    let server = Server::identify(&Info::from_reply(
        &send(&mut con, &cmd::info(Some("server"))).await,
    ));
    assert_eq!(server.label(), "Redis 5.0.14");
    assert!(!server.gates().keepttl);
    // RESP3 needs HELLO (6.0): the client reports it.
    let err = try_connect(&managed, 0, true).await.expect_err("no RESP3");
    assert!(
        matches!(
            redis_error(&err),
            AppError::Connect(_) | AppError::Db { .. }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn browsing_every_type_in_both_protocols() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    let gates = Gates::default();
    let all = key_filter(None).unwrap();
    let mut by_protocol = Vec::new();
    for resp3 in [false, true] {
        let mut con = connect(&fake, 1, resp3).await;
        let (rows, pages) = browse_all(&mut con, &all, &gates).await;
        assert_eq!(rows.len(), 22);
        assert!(pages >= 5, "pages {pages}");
        assert_eq!(row(&rows, "str:plain")[5], Value::Text("hello".into()));
        assert_eq!(row(&rows, "str:plain")[2], Value::Null);
        assert_eq!(row(&rows, "str:plain")[3], Value::Int(5));
        let big = row(&rows, "str:big");
        // 100 s from the seed, give or take a slow machine.
        assert!(matches!(big[2], Value::Int(95..=100)), "{:?}", big[2]);
        assert!(matches!(&big[5], Value::Bytes(b) if b.truncated && b.len == 100));
        assert_eq!(
            row(&rows, "hash:1")[5],
            Value::Json(r#"{"f":"v","g":"w"}"#.into())
        );
        assert_eq!(
            row(&rows, "list:1")[5],
            Value::Json(r#"["a","b","c"]"#.into())
        );
        assert_eq!(row(&rows, "set:1")[5], Value::Json(r#"["m"]"#.into()));
        assert_eq!(
            row(&rows, "zset:1")[5],
            Value::Json(r#"{"a":1.0,"b":2.5}"#.into())
        );
        assert_eq!(
            row(&rows, "stream:1")[5],
            Value::Json(
                r#"[{"id":"2-0","fields":{"f":"2"}},{"id":"1-0","fields":{"f":"1"}}]"#.into()
            )
        );
        assert_eq!(row(&rows, "stream:1")[3], Value::Int(2));
        assert_eq!(
            row(&rows, "json:1")[5],
            Value::Json(r#"{"a":[1,2]}"#.into())
        );
        assert_eq!(row(&rows, "ts:1")[1], Value::Text("TSDB-TYPE".into()));
        assert_eq!(row(&rows, "ts:1")[5], Value::Null);
        assert!(row(&rows, "hash:1")[4] != Value::Null);
        let binary = rows
            .iter()
            .find(|r| matches!(&r[0], Value::Bytes(_)))
            .expect("the binary key");
        assert_eq!(binary[5], Value::Text("bin".into()));
        // The TTLs count down between the two runs.
        let without_ttl: Vec<Vec<Value>> = rows
            .into_iter()
            .map(|mut r| {
                r.remove(2);
                r
            })
            .collect();
        by_protocol.push(without_ttl);
    }
    assert_eq!(by_protocol[0], by_protocol[1]);
}

#[tokio::test]
async fn filters_scan_by_pattern_type_or_exact_key() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    let mut con = connect(&fake, 1, false).await;
    let g = Gates::default();
    let filter = |items: Vec<FilterNode>| {
        key_filter(Some(&FilterGroup {
            conjunction: Conjunction::And,
            items,
        }))
        .unwrap()
    };
    let cond = |column: &str, op, v: &str| FilterNode::Condition {
        column: column.into(),
        op,
        values: vec![Value::Text(v.into())],
    };
    let (rows, _) = browse_all(
        &mut con,
        &filter(vec![cond("key", FilterOp::HasPrefixCase, "USER:0")]),
        &g,
    )
    .await;
    assert_eq!(rows.len(), 10);
    let (rows, _) = browse_all(
        &mut con,
        &filter(vec![cond("type", FilterOp::Eq, "hash")]),
        &g,
    )
    .await;
    assert_eq!(rows.len(), 1);
    fake.clear_log();
    let (rows, pages) = browse_all(
        &mut con,
        &filter(vec![cond("key", FilterOp::Eq, "list:1")]),
        &g,
    )
    .await;
    assert_eq!((rows.len(), pages), (1, 1));
    assert!(
        fake.log().iter().all(|l| !l.starts_with("SCAN")),
        "{:?}",
        fake.log()
    );
    let (rows, _) = browse_all(
        &mut con,
        &filter(vec![cond("key", FilterOp::Eq, "nope")]),
        &g,
    )
    .await;
    assert!(rows.is_empty());
    let (rows, _) = browse_all(
        &mut con,
        &filter(vec![FilterNode::Raw { sql: "*:1".into() }]),
        &g,
    )
    .await;
    assert_eq!(rows.len(), 7);

    // Before 6.0 the type is filtered after TYPE, from a SCAN without TYPE.
    let old = FakeRedis::with(Options {
        version: "5.0.14".into(),
        ..Options::default()
    })
    .await;
    seed(&old);
    let mut con = connect(&old, 1, false).await;
    let old_gates =
        Server::identify(&Info::from_reply(&send(&mut con, &cmd::info(None)).await)).gates();
    let (rows, _) = browse_all(
        &mut con,
        &filter(vec![cond("type", FilterOp::Eq, "zset")]),
        &old_gates,
    )
    .await;
    assert_eq!(rows.len(), 1);
    assert!(
        old.log()
            .iter()
            .filter(|l| l.starts_with("SCAN"))
            .all(|l| !l.contains("TYPE"))
    );
}

/// Applies a plan the way the session will: per change WATCH its guard's key and send the
/// guard's checks; only when each answers as it expects, MULTI … EXEC. The change's error is
/// the first failed check (nothing is sent), the first failed expectation inside EXEC, or a nil
/// EXEC.
async fn apply(con: &mut MultiplexedConnection, plan: &[KeyEdit]) -> Vec<Result<(), String>> {
    let mut out = Vec::new();
    for edit in plan {
        if let Some(key) = edit.guard.key() {
            send(con, &cmd::watch(&[key])).await;
            let checks = edit.guard.checks();
            let replies = pipeline(con, &checks).await;
            let refused = checks
                .iter()
                .zip(&replies)
                .find_map(|(c, r)| check(c, r).err());
            if let Some(e) = refused {
                send(con, &cmd::unwatch()).await;
                out.push(Err(e));
                continue;
            }
        }
        let mut cmds = vec![cmd::multi()];
        cmds.extend(edit.commands.iter().cloned());
        cmds.push(cmd::exec());
        let replies = pipeline(con, &cmds).await;
        let result = match replies.last() {
            Some(Resp::Array(results)) => edit
                .commands
                .iter()
                .zip(results)
                .find_map(|(c, r)| check(c, r).err())
                .map_or(Ok(()), Err),
            Some(Resp::Nil) => Err(cmd::CHANGED.to_string()),
            other => Err(format!("{other:?}")),
        };
        out.push(result);
    }
    out
}

fn check(c: &Command, reply: &Resp) -> Result<(), String> {
    match (&c.expect, reply) {
        (_, Resp::ServerError(e)) => Err(super::values::error_text(e)),
        (Expect::Any, _) => Ok(()),
        (Expect::Ok(_), Resp::Okay) => Ok(()),
        (Expect::Int(n, _), Resp::Int(m)) if n == m => Ok(()),
        (Expect::Bytes(want, _), r) if as_bytes(r).as_ref() == Some(want) => Ok(()),
        (Expect::Ok(msg) | Expect::Int(_, msg) | Expect::Bytes(_, msg), _) => Err(msg.to_string()),
    }
}

fn pk(key: &str, ty: &str) -> RowKey {
    RowKey::Pk(
        [
            (col::KEY.to_string(), Value::Text(key.into())),
            (col::TYPE.to_string(), Value::Text(ty.into())),
        ]
        .into(),
    )
}

#[tokio::test]
async fn edits_apply_per_change_with_guards() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    let mut con = connect(&fake, 1, true).await;
    let changes = vec![
        // A value edit keeps the TTL.
        StagedChange::Update {
            row_id: "a".into(),
            key: pk("str:big", "string"),
            changes: [(col::VALUE.to_string(), Value::Text("short".into()))].into(),
            exprs: None,
        },
        // A rename onto an existing key fails, after its TTL edit applied (no rollback).
        StagedChange::Update {
            row_id: "b".into(),
            key: pk("str:plain", "string"),
            changes: [
                (col::TTL.to_string(), Value::Int(50)),
                (col::KEY.to_string(), Value::Text("hash:1".into())),
            ]
            .into(),
            exprs: None,
        },
        // A key whose type changed since it was loaded is not touched.
        StagedChange::Update {
            row_id: "c".into(),
            key: pk("list:1", "string"),
            changes: [(col::VALUE.to_string(), Value::Text("x".into()))].into(),
            exprs: None,
        },
        StagedChange::Insert {
            row_id: "d".into(),
            values: [
                (col::KEY.to_string(), Value::Text("new:1".into())),
                (col::VALUE.to_string(), Value::Text("fresh".into())),
            ]
            .into(),
            exprs: None,
        },
        // An insert onto an existing key fails.
        StagedChange::Insert {
            row_id: "e".into(),
            values: [(col::KEY.to_string(), Value::Text("set:1".into()))].into(),
            exprs: None,
        },
        StagedChange::Delete {
            row_id: "f".into(),
            key: pk("zset:1", "zset"),
        },
        StagedChange::Update {
            row_id: "g".into(),
            key: pk("json:1", "ReJSON-RL"),
            changes: [(col::VALUE.to_string(), Value::Json("{\"b\":true}".into()))].into(),
            exprs: None,
        },
    ];
    let plan = plan_changes(&changes, &Gates::default()).unwrap();
    let results = apply(&mut con, &plan).await;
    assert_eq!(results[0], Ok(()));
    assert_eq!(fake.data(1, b"str:big"), Some(Data::str("short")));
    assert!(fake.ttl(1, b"str:big").is_some());
    assert_eq!(
        results[1],
        Err("a key with this name already exists".into())
    );
    assert!(
        fake.ttl(1, b"str:plain")
            .is_some_and(|t| t <= Duration::from_secs(50))
    );
    assert_eq!(results[2], Err(cmd::CHANGED.into()));
    assert_eq!(fake.data(1, b"list:1"), Some(Data::list(&["a", "b", "c"])));
    assert_eq!(results[3], Ok(()));
    assert_eq!(fake.data(1, b"new:1"), Some(Data::str("fresh")));
    assert_eq!(
        results[4],
        Err("a key with this name already exists".into())
    );
    assert_eq!(results[5], Ok(()));
    assert_eq!(fake.data(1, b"zset:1"), None);
    assert_eq!(results[6], Ok(()));
    assert_eq!(
        fake.data(1, b"json:1"),
        Some(Data::Json("{\"b\":true}".into()))
    );

    // A change between WATCH and EXEC aborts the transaction (nil EXEC).
    let plan = plan_changes(
        &[StagedChange::Update {
            row_id: "h".into(),
            key: pk("str:plain", "string"),
            changes: [(col::VALUE.to_string(), Value::Text("mine".into()))].into(),
            exprs: None,
        }],
        &Gates::default(),
    )
    .unwrap();
    send(&mut con, &cmd::watch(&[b"str:plain".as_slice()])).await;
    let mut other = connect(&fake, 1, false).await;
    send(
        &mut other,
        &cmd::set_string(b"str:plain", b"theirs", &Gates::default()),
    )
    .await;
    let mut cmds = vec![cmd::multi()];
    cmds.extend(plan[0].commands.iter().cloned());
    cmds.push(cmd::exec());
    let replies = pipeline(&mut con, &cmds).await;
    assert_eq!(replies.last(), Some(&Resp::Nil));
    assert_eq!(fake.data(1, b"str:plain"), Some(Data::str("theirs")));

    // Before 6.0 the value edit keeps the TTL through the script; the fake has no EVAL, which
    // shows as the change's error.
    let old = Gates {
        keepttl: false,
        ..Gates::default()
    };
    let plan = plan_changes(&changes[..1], &old).unwrap();
    let results = apply(&mut con, &plan).await;
    assert!(
        results[0]
            .as_ref()
            .is_err_and(|e| e.contains("unknown command"))
    );
}

#[tokio::test]
async fn container_edits_and_new_keys() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    let mut con = connect(&fake, 1, false).await;
    let replies = pipeline(
        &mut con,
        &[
            cmd::hash_set(b"hash:1", b"f", b"v2"),
            cmd::hash_add(b"hash:1", b"g", b"dup"),
            cmd::list_set(b"list:1", -1, b"z"),
            cmd::set_add(b"set:1", b"n"),
            cmd::zset_score(b"zset:1", "9", b"a").unwrap(),
            cmd::stream_add(b"stream:1", None, &[(b"k".to_vec(), b"v".to_vec())]).unwrap(),
            cmd::stream_delete(b"stream:1", "1-0").unwrap(),
        ],
    )
    .await;
    assert_eq!(
        check(&cmd::hash_add(b"hash:1", b"g", b"dup"), &replies[1]),
        Err("the field already exists".into())
    );
    assert_eq!(
        fake.data(1, b"hash:1"),
        Some(Data::hash(&[("f", "v2"), ("g", "w")]))
    );
    assert_eq!(fake.data(1, b"list:1"), Some(Data::list(&["a", "b", "z"])));
    assert_eq!(fake.data(1, b"set:1"), Some(Data::set(&["m", "n"])));
    assert_eq!(
        fake.data(1, b"zset:1"),
        Some(Data::zset(&[("b", 2.5), ("a", 9.0)]))
    );
    assert_eq!(replies[5], Resp::BulkString(b"3-0".to_vec()));
    // Guarded edits go through the apply: WATCH, checks, MULTI … EXEC.
    let edits = [
        cmd::list_delete_at(b"list:1", 1, b"b", b"__tabler:x").edit(0),
        cmd::hash_rename(b"hash:1", b"g", b"g2", b"w")
            .unwrap()
            .edit(1),
        cmd::create_key(
            b"z2",
            &cmd::NewKey::ZSet(vec![("1".into(), b"m".to_vec())]),
            Some(60_000),
        )
        .unwrap()
        .edit(2),
    ];
    assert_eq!(apply(&mut con, &edits).await, [Ok(()), Ok(()), Ok(())]);
    assert_eq!(fake.data(1, b"list:1"), Some(Data::list(&["a", "z"])));
    assert_eq!(
        fake.data(1, b"hash:1"),
        Some(Data::hash(&[("f", "v2"), ("g2", "w")]))
    );
    assert!(fake.ttl(1, b"z2").is_some());
    // A new key of a type that adds to an existing key is refused before MULTI.
    let again = cmd::create_key(
        b"z2",
        &cmd::NewKey::ZSet(vec![("5".into(), b"other".to_vec())]),
        None,
    )
    .unwrap()
    .edit(0);
    assert_eq!(
        apply(&mut con, &[again]).await,
        [Err("a key with this name already exists".to_string())]
    );
    assert_eq!(fake.data(1, b"z2"), Some(Data::zset(&[("m", 1.0)])));
    let copy = cmd::copy_key(b"z2", b"z3", &Gates::default()).unwrap();
    assert_eq!(check(&copy, &send(&mut con, &copy).await), Ok(()));
    assert_eq!(
        check(&copy, &send(&mut con, &copy).await),
        Err("a key with this name already exists".into())
    );
    let wrong = cmd::list_set(b"hash:1", 0, b"x");
    assert!(
        check(&wrong, &send(&mut con, &wrong).await).is_err_and(|e| e.starts_with("WRONGTYPE"))
    );
}

/// A RedisJSON insert with a TTL is `JSON.SET … NX` plus `PEXPIRE`. Inside MULTI the PEXPIRE ran
/// after the refused JSON.SET and expired the key that was there; the key is now proved absent
/// before MULTI.
#[tokio::test]
async fn a_refused_insert_leaves_the_existing_key_alone() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    let mut con = connect(&fake, 1, true).await;
    let json_insert = |row: &str, key: &str| StagedChange::Insert {
        row_id: row.into(),
        values: [
            (col::KEY.to_string(), Value::Text(key.into())),
            (col::TYPE.to_string(), Value::Text("ReJSON-RL".into())),
            (col::VALUE.to_string(), Value::Json("{\"n\":1}".into())),
            (col::TTL.to_string(), Value::Int(5)),
        ]
        .into(),
        exprs: None,
    };
    let plan = plan_changes(
        &[
            json_insert("a", "json:1"),
            json_insert("b", "hash:1"),
            json_insert("c", "json:new"),
        ],
        &Gates::default(),
    )
    .unwrap();
    assert!(plan.iter().all(|e| matches!(e.guard, Guard::Absent { .. })));
    fake.clear_log();
    let results = apply(&mut con, &plan).await;
    let exists = Err("a key with this name already exists".to_string());
    assert_eq!(results, [exists.clone(), exists, Ok(())]);
    assert_eq!(fake.ttl(1, b"json:1"), None);
    assert_eq!(fake.ttl(1, b"hash:1"), None);
    assert_eq!(
        fake.data(1, b"json:1"),
        Some(Data::Json("{\"a\":[1,2]}".into()))
    );
    assert_eq!(
        fake.data(1, b"hash:1"),
        Some(Data::hash(&[("f", "v"), ("g", "w")]))
    );
    assert_eq!(
        fake.data(1, b"json:new"),
        Some(Data::Json("{\"n\":1}".into()))
    );
    assert!(fake.ttl(1, b"json:new").is_some());
    // No write of the refused changes was sent: every JSON.SET and PEXPIRE is the new key's.
    let log = fake.log();
    assert!(
        log.iter()
            .filter(|l| l.starts_with("JSON.SET") || l.starts_with("PEXPIRE"))
            .all(|l| l.contains("json:new")),
        "{log:?}"
    );
    assert_eq!(
        log.iter()
            .filter(|l| l.starts_with("JSON.SET") || l.starts_with("PEXPIRE"))
            .count(),
        2
    );
}

/// Renames inside a container are two commands (`HSETNX` + `HDEL`, `SADD` + `SREM`): inside
/// MULTI a refused first one did not stop the second, which lost the field. The guard's checks
/// now refuse the change before MULTI, and nothing changes.
#[tokio::test]
async fn container_renames_are_refused_before_multi() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    fake.seed(1, b"set:2", Data::set(&["m", "n"]), None);
    let mut con = connect(&fake, 1, false).await;
    let hash = Data::hash(&[("f", "v"), ("g", "w")]);
    let refused = [
        // Onto an existing field: HDEL would have dropped f after the refused HSETNX.
        (
            cmd::hash_rename(b"hash:1", b"f", b"g", b"v").unwrap(),
            "the field already exists",
        ),
        // The old field holds another value than the one loaded.
        (
            cmd::hash_rename(b"hash:1", b"f", b"h", b"stale").unwrap(),
            "the field changed or no longer exists (changed by someone else)",
        ),
        // A field that is gone.
        (
            cmd::hash_rename(b"hash:1", b"nope", b"h", b"v").unwrap(),
            "the field changed or no longer exists (changed by someone else)",
        ),
        // Onto an existing member, and a member that is gone.
        (
            cmd::set_rename(b"set:2", b"m", b"n").unwrap(),
            "the member already exists",
        ),
        (
            cmd::set_rename(b"set:2", b"gone", b"o").unwrap(),
            "the member no longer exists",
        ),
        // A position that holds another element than the one loaded.
        (
            cmd::list_delete_at(b"list:1", 0, b"b", b"__tabler:x"),
            "the element changed or moved (changed by someone else)",
        ),
        (
            cmd::list_replace(b"list:1", 5, b"a", b"x"),
            "the element changed or moved (changed by someone else)",
        ),
    ];
    for (g, want) in refused {
        let text = format!("{:?}", g.commands);
        assert_eq!(
            apply(&mut con, &[g.edit(0)]).await,
            [Err(want.to_string())],
            "{text}"
        );
    }
    // Another type: the check's own error.
    let wrong = cmd::hash_rename(b"list:1", b"a", b"b", b"v")
        .unwrap()
        .edit(0);
    assert!(
        apply(&mut con, &[wrong]).await[0]
            .as_ref()
            .is_err_and(|e| e.starts_with("WRONGTYPE"))
    );
    assert_eq!(fake.data(1, b"hash:1"), Some(hash));
    assert_eq!(fake.data(1, b"set:2"), Some(Data::set(&["m", "n"])));
    assert_eq!(fake.data(1, b"list:1"), Some(Data::list(&["a", "b", "c"])));

    // The same edits where the checks hold.
    let applied = [
        cmd::hash_rename(b"hash:1", b"f", b"h", b"v")
            .unwrap()
            .edit(0),
        cmd::set_rename(b"set:2", b"m", b"o").unwrap().edit(1),
        cmd::list_delete_at(b"list:1", 1, b"b", b"__tabler:x").edit(2),
        cmd::list_replace(b"list:1", -1, b"c", b"z").edit(3),
    ];
    assert_eq!(
        apply(&mut con, &applied).await,
        [Ok(()), Ok(()), Ok(()), Ok(())]
    );
    assert_eq!(
        fake.data(1, b"hash:1"),
        Some(Data::hash(&[("g", "w"), ("h", "v")]))
    );
    assert_eq!(fake.data(1, b"set:2"), Some(Data::set(&["n", "o"])));
    assert_eq!(fake.data(1, b"list:1"), Some(Data::list(&["a", "z"])));
}

#[tokio::test]
async fn console_runs_and_shapes_in_both_protocols() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    let script = "HGETALL hash:1\nZRANGE zset:1 0 -1 WITHSCORES; SCAN 0 MATCH user:* COUNT 5\nGET \"str:plain\"\nINFO keyspace";
    assert_eq!(classify_script(script, None), StatementClass::Read);
    let mut shaped = Vec::new();
    for resp3 in [false, true] {
        let mut con = connect(&fake, 1, resp3).await;
        let mut out = Vec::new();
        for st in split_console(script) {
            let argv = tokenize(&st.sql).unwrap();
            let c = Command {
                argv: argv.clone(),
                expect: Expect::Any,
            };
            out.push(shape(&argv, &send(&mut con, &c).await, 100).unwrap());
        }
        shaped.push(out);
    }
    assert_eq!(shaped[0], shaped[1]);
    let s = &shaped[0];
    assert_eq!(s[0].rows.len(), 2);
    assert_eq!(s[1].rows[1], [Value::Text("b".into()), Value::Float(2.5)]);
    assert_eq!(s[2].notices, ["next cursor: 5"]);
    assert_eq!(s[3].rows, [[Value::Text("hello".into())]]);
    assert_eq!(s[4].rows[0][1], Value::Text("db1".into()));
    // A write is classified as one, and a server error is the command's error.
    assert_eq!(classify_script("GET a\nDEL a", None), StatementClass::Write);
    let mut con = connect(&fake, 1, false).await;
    let argv = tokenize("LPUSH hash:1 x").unwrap();
    let reply = send(
        &mut con,
        &Command {
            argv: argv.clone(),
            expect: Expect::Any,
        },
    )
    .await;
    assert!(
        shape(&argv, &reply, 10)
            .unwrap_err()
            .to_string()
            .starts_with("WRONGTYPE")
    );
}

#[tokio::test]
async fn cancel_kills_the_running_command() {
    let fake = FakeRedis::start().await;
    let mut busy = connect(&fake, 0, false).await;
    let Resp::Int(id) = send(&mut busy, &cmd::client_id()).await else {
        panic!()
    };
    let sleeper = tokio::spawn(async move {
        busy.req_packed_command(::redis::cmd("DEBUG").arg("SLEEP").arg("30"))
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut control = connect(&fake, 0, false).await;
    assert_eq!(
        send(&mut control, &cmd::client_kill(id)).await,
        Resp::Int(1)
    );
    let outcome = tokio::time::timeout(Duration::from_secs(3), sleeper)
        .await
        .expect("the killed command returns")
        .expect("the task");
    let err = outcome.expect_err("killed");
    assert!(
        matches!(redis_error(&err), AppError::NotConnected),
        "{err:?}"
    );
    assert_eq!(
        send(&mut control, &cmd::client_kill(id)).await,
        Resp::Int(0)
    );
}

#[tokio::test]
async fn a_response_timeout_is_a_timeout() {
    let fake = FakeRedis::start().await;
    let mut con = ::redis::Client::open(fake.url(0, false))
        .unwrap()
        .get_multiplexed_async_connection_with_config(
            &AsyncConnectionConfig::new().set_response_timeout(Some(Duration::from_millis(100))),
        )
        .await
        .unwrap();
    let err = con
        .req_packed_command(::redis::cmd("DEBUG").arg("SLEEP").arg("2"))
        .await
        .expect_err("timed out");
    assert!(matches!(redis_error(&err), AppError::Timeout), "{err:?}");
}
