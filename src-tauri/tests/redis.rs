//! Redis and Valkey against a real server: connect, the sidebar, the key browser page by page,
//! counts, whole cells, a commit with one failing change and the inserts the grid sends, the
//! console, a cancel and a time out, verified TLS, and a read-only profile's refusals on every
//! write path (docs/drivers/redis.md). Built only with `drv-redis` (`required-features` in
//! Cargo.toml) and run with `node dev/scripts/lane.mjs redis it`, which loads `.env`. Each test
//! skips itself while `TABLER_TEST_REDIS_URL` is unset (`redis://[user:password@]host:port/db`;
//! `rediss://` for TLS without a certificate check; `rediss://…?verify=full` to check the
//! certificate and the host name, `&ca=<path to the CA's PEM>` when a private CA signed it), and
//! is `#[ignore]`d so that a plain `cargo test --all-features` never reaches a server.
//!
//! The tests write only keys under a prefix of their own (`tabler:it:<uuid>:`) in the URL's
//! database, and delete them at the end. The read-only checks, too, ask only for writes of keys
//! under that prefix.

#![cfg(feature = "drv-redis")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tabler_lib::drivers::native::NativeSession;
use tabler_lib::drivers::{CancelSlot, CollectSink, ConnectTarget, Session, request_cancel};
use tabler_lib::error::AppError;
use tabler_lib::types::admin::KillScope;
use tabler_lib::types::browse::{
    BrowseRequest, Conjunction, FilterGroup, FilterNode, FilterOp, RowKey,
};
use tabler_lib::types::mutate::{ApplyRequest, StagedChange};
use tabler_lib::types::profile::{ConnectionProfile, SslMode};
use tabler_lib::types::query::RunOptions;
use tabler_lib::types::schema::{TableMeta, TableRef};
use tabler_lib::types::value::Value;

const VAR: &str = "TABLER_TEST_REDIS_URL";

/// The profile and password of the URL: `rediss://` is TLS, checked with `verify=full` (against
/// the CA in `ca=` when there is one).
fn target_of(url: &str) -> (ConnectionProfile, Option<String>) {
    let tls = url.starts_with("rediss://");
    let query: Vec<(&str, &str)> = url
        .split_once('?')
        .map(|(_, q)| q.split('&').filter_map(|p| p.split_once('=')).collect())
        .unwrap_or_default();
    let param = |name: &str| query.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);
    let plain = url.replacen("rediss://", "redis://", 1);
    let (mut profile, password) = common::profile_from_url(&plain);
    profile.ssl = match (tls, param("verify")) {
        (false, _) => SslMode::Disable,
        (true, Some("full")) => SslMode::VerifyFull,
        (true, _) => SslMode::Require,
    };
    profile.ssl_ca_path = param("ca").map(str::to_string);
    (profile, password)
}

/// Opens a session; its error as text.
async fn connect(
    profile: &ConnectionProfile,
    password: Option<&str>,
) -> Result<Arc<dyn NativeSession>, String> {
    common::install_crypto();
    match tabler_lib::drivers::open(ConnectTarget {
        profile,
        password,
        host_override: None,
    })
    .await
    .map_err(|e| e.to_string())?
    {
        Session::Native(native) => Ok(native),
        Session::Sql(_) => panic!("Redis opens a native session"),
    }
}

async fn open(profile: &ConnectionProfile, password: Option<&str>) -> Arc<dyn NativeSession> {
    connect(profile, password)
        .await
        .unwrap_or_else(|e| panic!("{VAR}: connect failed: {e}"))
}

/// One console command; its error as text.
async fn console(s: &dyn NativeSession, command: &str) -> Result<CollectSink, String> {
    let mut sink = CollectSink::default();
    s.run_console(command, 1000, &mut sink, CancelSlot::default())
        .await
        .map_err(|e| e.to_string())?;
    Ok(sink)
}

/// The first cell of a console command's reply.
async fn reply(s: &dyn NativeSession, command: &str) -> Value {
    let sink = console(s, command).await.unwrap();
    sink.sets[0].rows[0][0].clone()
}

fn by_key(op: FilterOp, text: &str) -> FilterGroup {
    FilterGroup {
        conjunction: Conjunction::And,
        items: vec![FilterNode::Condition {
            column: "key".into(),
            op,
            values: vec![Value::Text(text.into())],
        }],
    }
}

fn browse_request(table: &TableRef, filter: FilterGroup, limit: u32) -> BrowseRequest {
    BrowseRequest {
        table: table.clone(),
        columns: None,
        filter: Some(filter),
        sort: Vec::new(),
        limit,
        offset: 0,
        cursor: None,
    }
}

fn pk(key: &str, ty: &str) -> RowKey {
    RowKey::Pk(
        [
            ("key".to_string(), Value::Text(key.into())),
            ("type".to_string(), Value::Text(ty.into())),
        ]
        .into(),
    )
}

/// An inserted row as the grid sends it: a value for every column of the table.
fn insert(meta: &TableMeta, row: &str, cells: Vec<Value>) -> StagedChange {
    StagedChange::Insert {
        row_id: row.into(),
        values: meta
            .columns
            .iter()
            .map(|c| c.name.clone())
            .zip(cells)
            .collect(),
        exprs: None,
    }
}

#[tokio::test]
#[ignore = "set TABLER_TEST_REDIS_URL"]
async fn real_server() {
    let Some(url) = common::url(VAR) else {
        return;
    };
    let (profile, password) = target_of(&url);
    let s = open(&profile, password.as_deref()).await;
    let info = s.server_info().await.unwrap();
    println!("server: {}", info.version);
    s.ping().await.unwrap();
    let db_name = info
        .current_database
        .clone()
        .unwrap_or_else(|| "db0".into());
    let table = TableRef::new(None, &db_name);
    let tables = s.list_tables("").await.unwrap();
    assert!(tables.iter().any(|t| t.name == db_name));
    let meta = s.table_meta(&table).await.unwrap();
    assert_eq!(meta.primary_key, ["key", "type"]);

    let prefix = format!("tabler:it:{}:", uuid::Uuid::new_v4());
    let k = |name: &str| format!("{prefix}{name}");
    for command in [
        format!("SET {} hello", k("str")),
        format!("HSET {} f v g w", k("hash")),
        format!("RPUSH {} a b c", k("list")),
        format!("SADD {} m n", k("set")),
        format!("ZADD {} 1 a 2.5 b", k("zset")),
        format!("XADD {} * f 1", k("stream")),
        format!("SET {} 1 EX 600", k("ttl")),
    ] {
        console(s.as_ref(), &command).await.unwrap();
    }

    // Page by page with a page size of 2 through the test's keys.
    let mut cursor = None;
    let mut seen = std::collections::HashSet::new();
    loop {
        let req = BrowseRequest {
            cursor: cursor.clone(),
            ..browse_request(&table, by_key(FilterOp::HasPrefix, &prefix), 2)
        };
        let page = s.browse(&meta, &req, CancelSlot::default()).await.unwrap();
        for row in page.rows {
            seen.insert(format!("{:?}", row[0]));
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 7);
    let count = s
        .count(
            &meta,
            Some(&by_key(FilterOp::HasPrefix, &prefix)),
            true,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!((count.count, count.exact), (7, true));
    assert_eq!(
        s.fetch_cell(&meta, &pk(&k("hash"), "hash"), "value")
            .await
            .unwrap(),
        Value::Json(r#"{"f":"v","g":"w"}"#.into())
    );

    // A commit: a value edit applies (keeping the TTL), an insert onto an existing key fails,
    // and the inserts the grid sends apply: a new row whose untyped cells are missing, and a
    // duplicated row with every cell of the original (its type, TTL, size and memory too).
    let missing = Value::other("missing", "");
    let mut new_row = vec![missing.clone(); meta.columns.len()];
    new_row[0] = Value::Text(k("new"));
    new_row[5] = Value::Text("typed in".into());
    let original = s
        .browse(
            &meta,
            &browse_request(&table, by_key(FilterOp::Eq, &k("ttl")), 10),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    let mut copy = original.rows[0].clone();
    copy[0] = Value::Text(k("dup"));
    let req = ApplyRequest {
        table: table.clone(),
        changes: vec![
            StagedChange::Update {
                row_id: "a".into(),
                key: pk(&k("ttl"), "string"),
                changes: [("value".to_string(), Value::Text("2".into()))].into(),
                exprs: None,
            },
            StagedChange::Insert {
                row_id: "b".into(),
                values: [("key".to_string(), Value::Text(k("set")))].into(),
                exprs: None,
            },
            insert(&meta, "c", new_row),
            insert(&meta, "d", copy),
        ],
        returning: false,
    };
    let plan = s.plan_changes(&meta, &req).unwrap();
    let result = s.apply_changes(&meta, &req, plan).await;
    assert!(!result.success);
    let failed: Vec<bool> = result.results.iter().map(|r| r.error.is_some()).collect();
    assert_eq!(failed, [false, true, false, false], "{:?}", result.results);
    for (key, ttl_wanted) in [(k("ttl"), true), (k("new"), false), (k("dup"), true)] {
        let ttl = s
            .fetch_cell(&meta, &pk(&key, "string"), "ttl")
            .await
            .unwrap();
        if ttl_wanted {
            assert!(matches!(ttl, Value::Int(1..=600)), "{key}: {ttl:?}");
        } else {
            assert_eq!(ttl, Value::Null, "{key}");
        }
    }
    assert_eq!(
        reply(s.as_ref(), &format!("GET {}", k("new"))).await,
        Value::Text("typed in".into())
    );
    assert_eq!(
        reply(s.as_ref(), &format!("GET {}", k("dup"))).await,
        Value::Text("1".into())
    );

    // The value editor's calls.
    let page = s
        .call(
            "keyPage",
            json!({"db": db_index(&db_name), "key": {"t": "text", "v": k("list")}, "type": "list"}),
        )
        .await
        .unwrap();
    assert_eq!(page["elements"].as_array().unwrap().len(), 3);

    // Cancel and time out, where the server allows DEBUG (Redis 7 needs enable-debug-command).
    if console(s.as_ref(), "DEBUG SLEEP 0").await.is_ok() {
        let slot = CancelSlot::default();
        let run = {
            let (s, slot) = (s.clone(), slot.clone());
            tokio::spawn(async move {
                let mut sink = CollectSink::default();
                s.run_console("DEBUG SLEEP 2", 10, &mut sink, slot).await
            })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        if let Some(handle) = request_cancel(&slot) {
            s.cancel(handle).await.unwrap();
        }
        let out = run.await.unwrap();
        assert!(matches!(out, Err(AppError::Cancelled)), "{out:?}");
        let options = RunOptions {
            timeout_secs: 1,
            ..RunOptions::default()
        };
        let mut emit = |_e: tabler_lib::types::query::QueryEvent| {};
        let results = tabler_lib::commands::console::run_console_inner(
            s.as_ref(),
            "DEBUG SLEEP 3",
            &options,
            &CancelSlot::default(),
            &mut emit,
        )
        .await;
        assert_eq!(
            results[0].error.as_ref().map(|e| e.code.as_str()),
            Some("QUERY_TIMEOUT")
        );
    } else {
        println!("skipped cancel and time out: DEBUG is disabled on this server");
    }

    // Verified TLS: the session above checked the server's certificate and host name against the
    // CA; without that CA the same profile is refused.
    match (profile.ssl, profile.ssl_ca_path.is_some()) {
        (SslMode::VerifyFull, true) => {
            let mut no_ca = profile.clone();
            no_ca.ssl_ca_path = None;
            assert!(
                connect(&no_ca, password.as_deref()).await.is_err(),
                "a certificate the system roots do not trust is refused"
            );
        }
        (SslMode::VerifyFull, false) => println!("TLS verified against the system roots"),
        _ => println!("verified TLS not exercised: use rediss://…?verify=full[&ca=<path>]"),
    }

    // A read-only profile refuses every write path (each of a key under the prefix), and reads.
    let mut ro = profile.clone();
    ro.options.read_only = true;
    let r = open(&ro, password.as_deref()).await;
    let refused = |e: &str| e.contains("read-only");
    let err = console(r.as_ref(), &format!("DEL {}", k("str")))
        .await
        .expect_err("DEL is refused");
    assert!(refused(&err), "{err}");
    // Writes that Redis Stack's modules flag `readonly` in this server's COMMAND table.
    for write in [
        format!("FT.ALIASUPDATE {} {}", k("alias"), k("idx")),
        format!("FT.DICTADD {} w", k("dict")),
        format!("CF.COMPACT {}", k("cf")),
        format!("GRAPH.SLOWLOG {} RESET", k("graph")),
    ] {
        let err = console(r.as_ref(), &write).await.expect_err(&write);
        assert!(refused(&err), "{write}: {err}");
    }
    assert!(r.plan_changes(&meta, &req).is_err());
    // A plan made by a writable session is refused at apply too.
    let ro_insert = ApplyRequest {
        table: table.clone(),
        changes: vec![StagedChange::Insert {
            row_id: "e".into(),
            values: [
                ("key".to_string(), Value::Text(k("ro:apply"))),
                ("value".to_string(), Value::Text("x".into())),
            ]
            .into(),
            exprs: None,
        }],
        returning: false,
    };
    let plan = s.plan_changes(&meta, &ro_insert).unwrap();
    let result = r.apply_changes(&meta, &ro_insert, plan).await;
    assert!(!result.success);
    let db = db_index(&db_name);
    let text = |v: &str| json!({"t": "text", "v": v});
    for (method, args) in [
        (
            "newKey",
            json!({"db": db, "key": text(&k("ro:new")), "value": {"type": "string", "value": text("x")}}),
        ),
        (
            "copyKey",
            json!({"db": db, "from": text(&k("str")), "to": text(&k("ro:copy"))}),
        ),
        (
            "editKey",
            json!({"db": db, "key": text(&k("list")), "type": "list",
                   "edits": [{"op": "listPush", "values": [text("ro")], "head": false}]}),
        ),
    ] {
        let err = r.call(method, args).await.expect_err(method);
        assert!(refused(&err.to_string()), "{method}: {err}");
    }
    // No client has this id: even a regression would kill nothing.
    let killed = r
        .kill_processes(&[i64::MAX.to_string()], KillScope::Connection)
        .await
        .unwrap();
    assert!(!killed[0].ok);
    assert!(
        killed[0].message.as_deref().is_some_and(refused),
        "{killed:?}"
    );
    let mut boot = ro.clone();
    boot.options.startup_sql = Some(format!("SET {} 1", k("ro:boot")));
    assert!(connect(&boot, password.as_deref()).await.is_err());
    assert_eq!(
        reply(r.as_ref(), &format!("GET {}", k("str"))).await,
        Value::Text("hello".into())
    );
    r.close().await;
    // Nothing of it was written.
    let ro_keys = ["ro:apply", "ro:new", "ro:copy", "ro:boot"].map(k);
    assert_eq!(
        reply(s.as_ref(), &format!("EXISTS {}", ro_keys.join(" "))).await,
        Value::Int(0)
    );
    assert_eq!(
        reply(s.as_ref(), &format!("LLEN {}", k("list"))).await,
        Value::Int(3)
    );

    // Clean up (the read-only keys too, should one have been written after all).
    let keys: Vec<String> = [
        "str", "hash", "list", "set", "zset", "stream", "ttl", "new", "dup",
    ]
    .iter()
    .map(|n| k(n))
    .chain(ro_keys)
    .collect();
    console(s.as_ref(), &format!("DEL {}", keys.join(" ")))
        .await
        .unwrap();
    s.close().await;
}

fn db_index(name: &str) -> u32 {
    name.trim_start_matches("db").parse().unwrap_or(0)
}
