//! The session against the fake server, the way the app drives it: connect (RESP3 and RESP2, ACL
//! users, TLS with a CA and a client certificate, through a tunnel's address), the sidebar, the
//! key browser page by page, counts, whole cells, a commit with a failing change, the grid's
//! insert payloads, edits under an ACL without transactions or after a SELECT that timed out,
//! the console, the value editor's calls, the process list, a cancel and a time out, and every
//! write path refused on a read-only profile.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use super::session::{RedisSession, lock};
use super::testing::{Data, FakeRedis, Options, tls_requiring_client_cert};
use crate::drivers::native::{CallKind, NativeSession, TableSource};
use crate::drivers::{CancelSlot, CollectSink, ConnectTarget, request_cancel};
use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::native::redis::col;
use crate::types::browse::{BrowseRequest, Conjunction, FilterGroup, FilterNode, FilterOp, RowKey};
use crate::types::caps::{CountMode, Feature, Paging, TxMode};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::{ApplyRequest, StagedChange};
use crate::types::profile::{ConnectionProfile, SslMode};
use crate::types::query::RunOptions;
use crate::types::schema::{TableMeta, TableRef};
use crate::types::value::Value;

fn profile(fake: &FakeRedis, db: u32) -> ConnectionProfile {
    serde_json::from_value(json!({
        "id": uuid::Uuid::new_v4(), "name": "redis-test", "driver": "redis",
        "host": "127.0.0.1", "port": fake.addr.port(), "user": null,
        "database": db.to_string(), "sslCaPath": null, "sqlitePath": null, "ssh": null,
        "lastConnectedAt": null, "ssl": "disable",
        "options": { "connectTimeoutSecs": 5 },
    }))
    .expect("a profile")
}

async fn open(p: &ConnectionProfile, password: Option<&str>) -> Result<RedisSession, AppError> {
    RedisSession::connect(ConnectTarget {
        profile: p,
        password,
        host_override: None,
    })
    .await
}

async fn session(fake: &FakeRedis, db: u32) -> RedisSession {
    open(&profile(fake, db), None).await.expect("connected")
}

/// Every key type in db 1, a binary key, keys with TTLs and twelve `user:NN` strings.
fn seed(fake: &FakeRedis) {
    let ttl = Some(Duration::from_secs(100));
    fake.seed(1, b"str:plain", Data::str("hello"), None);
    fake.seed(1, b"str:big", Data::Str(vec![b'x'; 40_000]), ttl);
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

fn db(n: u32) -> TableRef {
    TableRef::new(None, &format!("db{n}"))
}

async fn meta(s: &RedisSession, n: u32) -> TableMeta {
    s.table_meta(&db(n)).await.expect("the table")
}

fn request(
    n: u32,
    limit: u32,
    cursor: Option<String>,
    filter: Option<FilterGroup>,
) -> BrowseRequest {
    BrowseRequest {
        table: db(n),
        columns: None,
        filter,
        sort: Vec::new(),
        limit,
        offset: 0,
        cursor,
    }
}

fn filter(column: &str, op: FilterOp, v: &str) -> Option<FilterGroup> {
    Some(FilterGroup {
        conjunction: Conjunction::And,
        items: vec![FilterNode::Condition {
            column: column.into(),
            op,
            values: vec![Value::Text(v.into())],
        }],
    })
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

/// Runs one console command into a sink.
async fn console(s: &RedisSession, command: &str) -> Result<CollectSink, AppError> {
    let mut sink = CollectSink::default();
    s.run_console(command, 100, &mut sink, CancelSlot::default())
        .await?;
    Ok(sink)
}

fn first_cell(sink: &CollectSink) -> Value {
    sink.sets[0].rows[0].last().cloned().unwrap_or(Value::Null)
}

#[tokio::test]
async fn connects_in_both_protocols_and_lists_the_databases() {
    let fake = FakeRedis::with(Options {
        user: Some("app".into()),
        password: Some("s3cret".into()),
        product: Some(("valkey".into(), "8.0.1".into())),
        databases: 32,
        ..Options::default()
    })
    .await;
    seed(&fake);
    let mut p = profile(&fake, 1);
    p.user = Some("app".into());
    let s = open(&p, Some("s3cret")).await.unwrap();
    assert!(fake.log().iter().any(|l| l == "HELLO 3 AUTH app s3cret"));
    assert!(
        fake.log()
            .iter()
            .any(|l| l == "CLIENT SETINFO LIB-NAME tabler")
    );
    let info = s.server_info().await.unwrap();
    assert_eq!(info.version, "Valkey 8.0.1");
    assert_eq!(info.current_database.as_deref(), Some("db1"));
    assert_eq!(info.current_user.as_deref(), Some("app"));
    s.ping().await.unwrap();
    assert_eq!(s.list_databases().await.unwrap().len(), 32);
    assert!(s.list_schemas().await.unwrap().is_empty());
    let tables = s.list_tables("").await.unwrap();
    assert_eq!(tables.len(), 32);
    assert_eq!(
        (tables[1].name.as_str(), tables[1].row_estimate),
        ("db1", Some(22))
    );
    let m = meta(&s, 1).await;
    assert_eq!(m.primary_key, ["key", "type"]);
    assert_eq!(m.row_estimate, Some(22));
    assert!(m.columns.iter().any(|c| c.name == "type" && c.is_generated));
    let def = s.definition(&db(1)).await.unwrap().unwrap();
    assert!(
        def.contains("\"keys\": 22") && def.contains("RESP3"),
        "{def}"
    );
    assert!(matches!(
        s.table_meta(&db(32)).await,
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        s.table_meta(&TableRef::new(None, "keys")).await,
        Err(AppError::NotFound(_))
    ));

    // Wrong credentials are an authentication error.
    let err = open(&p, Some("nope")).await.err().unwrap();
    assert!(matches!(err, AppError::Auth(_)), "{err:?}");

    // A server before 6.0 has no HELLO: the session speaks RESP2 after one try.
    let old = FakeRedis::with(Options {
        version: "5.0.14".into(),
        password: Some("pw".into()),
        refuse_config: true,
        ..Options::default()
    })
    .await;
    let s = open(&profile(&old, 0), Some("pw")).await.unwrap();
    assert_eq!(s.server_info().await.unwrap().version, "Redis 5.0.14");
    // CONFIG refused (a managed service): 16 databases.
    assert_eq!(s.list_tables("").await.unwrap().len(), 16);
    console(&s, "PING").await.unwrap();
    let hellos = old.log().iter().filter(|l| l.starts_with("HELLO")).count();
    assert_eq!(hellos, 1, "{:?}", old.log());
    assert!(old.log().iter().any(|l| l == "AUTH pw"));

    // A DB index the server does not have.
    assert!(open(&profile(&old, 20), Some("pw")).await.is_err());
    // MCP refuses native stores.
    let err = RedisSession::connect_as(
        ConnectTarget {
            profile: &profile(&old, 0),
            password: Some("pw"),
            host_override: None,
        },
        true,
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(err, AppError::Unsupported(_)), "{err:?}");
}

#[tokio::test]
async fn caps_grant_what_the_session_implements() {
    let fake = FakeRedis::start().await;
    let s = session(&fake, 0).await;
    let caps = s.capabilities();
    for f in [
        Feature::StructureView,
        Feature::Editor,
        Feature::EditRows,
        Feature::InsertRows,
        Feature::DeleteRows,
        Feature::ExactCount,
        Feature::ViewDefinition,
        Feature::ProcessList,
        Feature::KillProcess,
        Feature::StartupCommands,
    ] {
        assert!(caps.has(f), "{f:?}");
    }
    for f in [
        Feature::CancelOnServer,
        Feature::Mcp,
        Feature::EditWithoutKey,
        Feature::ServerExpressions,
        Feature::WorkspaceCommit,
        Feature::SwitchDatabase,
        Feature::Explain,
    ] {
        assert!(!caps.has(f), "{f:?}");
    }
    assert_eq!(caps.dialect, None);
    assert_eq!(caps.data.paging, Paging::Cursor);
    assert_eq!(caps.data.count, CountMode::Estimate);
    assert_eq!(caps.data.transactions, TxMode::Batch);
    assert!(!caps.data.sort);
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    assert_eq!(caps.session.keepalive_secs, Some(30));
    assert!(!caps.session.ping_billed);
    assert_eq!(caps.namespaces.labels.tables, "Databases");
}

#[tokio::test]
async fn browses_page_by_page_counts_and_fetches_cells() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    let s = session(&fake, 0).await;
    let m = meta(&s, 1).await;
    // Page by page: the cursor of each page leads to the next one.
    let mut cursor = None;
    let mut keys = std::collections::HashSet::new();
    let mut pages = 0;
    loop {
        let page = s
            .browse(
                &m,
                &request(1, 5, cursor.clone(), None),
                CancelSlot::default(),
            )
            .await
            .unwrap();
        pages += 1;
        assert_eq!(page.pk_columns, ["key", "type"]);
        assert!(page.sql.starts_with("SCAN "), "{}", page.sql);
        for row in &page.rows {
            assert_eq!(row.len(), page.columns.len());
            assert!(keys.insert(format!("{:?}", row[0])), "a key came twice");
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(keys.len(), 22);
    assert!(pages >= 4, "{pages} pages");
    // A filtered page scans on until it holds a page of matches.
    let page = s
        .browse(
            &m,
            &request(1, 10, None, filter("key", FilterOp::HasPrefix, "user:")),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    // SCAN steps return what they matched whole: a page may hold a few more.
    assert!((10..=12).contains(&page.rows.len()), "{}", page.rows.len());
    // An exact key is read without a SCAN.
    fake.clear_log();
    let page = s
        .browse(
            &m,
            &request(1, 10, None, filter("key", FilterOp::Eq, "hash:1")),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0][5], Value::Json(r#"{"f":"v","g":"w"}"#.into()));
    assert_eq!(page.next_cursor, None);
    assert!(fake.log().iter().all(|l| !l.starts_with("SCAN")));
    // The commands are in the console log of the app's own queries.
    let traced = s.drain_trace();
    assert!(
        traced.iter().any(|t| t.sql.starts_with("TYPE hash:1")),
        "{traced:?}"
    );

    // Counts: DBSIZE, an exact key, a pattern (exact, or scanned for a moment), a type.
    let count = |f: Option<FilterGroup>, exact: bool| {
        let m = m.clone();
        let s = &s;
        async move {
            s.count(&m, f.as_ref(), exact, CancelSlot::default())
                .await
                .unwrap()
        }
    };
    let all = count(None, false).await;
    assert_eq!((all.count, all.exact), (22, true));
    let users = count(filter("key", FilterOp::HasPrefix, "user:"), true).await;
    assert_eq!((users.count, users.exact), (12, true));
    // The small keyspace is scanned whole within the moment: exact too.
    let users = count(filter("key", FilterOp::HasPrefix, "user:"), false).await;
    assert_eq!((users.count, users.exact), (12, true));
    assert_eq!(
        count(filter("key", FilterOp::Eq, "list:1"), false)
            .await
            .count,
        1
    );
    assert_eq!(
        count(filter("key", FilterOp::Eq, "nope"), false)
            .await
            .count,
        0
    );
    assert_eq!(
        count(filter("type", FilterOp::Eq, "hash"), true)
            .await
            .count,
        1
    );

    // Whole cells: a string cut in the grid, a container, the TTL, the exact memory.
    let big = s
        .browse(
            &m,
            &request(1, 1, None, filter("key", FilterOp::Eq, "str:big")),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert!(matches!(&big.rows[0][5], Value::Bytes(b) if b.truncated && b.len == 40_000));
    let whole = s
        .fetch_cell(&m, &pk("str:big", "string"), col::VALUE)
        .await
        .unwrap();
    assert_eq!(whole, Value::Text("x".repeat(40_000)));
    assert_eq!(
        s.fetch_cell(&m, &pk("zset:1", "zset"), col::VALUE)
            .await
            .unwrap(),
        Value::Json(r#"{"a":1.0,"b":2.5}"#.into())
    );
    assert_eq!(
        s.fetch_cell(&m, &pk("hash:1", "hash"), col::VALUE)
            .await
            .unwrap(),
        Value::Json(r#"{"f":"v","g":"w"}"#.into())
    );
    assert!(matches!(
        s.fetch_cell(&m, &pk("str:big", "string"), col::TTL)
            .await
            .unwrap(),
        Value::Int(95..=100)
    ));
    assert!(matches!(
        s.fetch_cell(&m, &pk("list:1", "list"), col::MEMORY)
            .await
            .unwrap(),
        Value::Int(_)
    ));
    assert!(
        fake.log()
            .iter()
            .any(|l| l == "MEMORY USAGE list:1 SAMPLES 0")
    );
    // A key whose type changed, a key that is gone.
    let err = s
        .fetch_cell(&m, &pk("list:1", "string"), col::VALUE)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("changed by someone else"), "{err}");
    assert!(matches!(
        s.fetch_cell(&m, &pk("gone", "string"), col::VALUE).await,
        Err(AppError::NotFound(_))
    ));
    // A module type has no standard read.
    assert!(matches!(
        s.fetch_cell(&m, &pk("ts:1", "TSDB-TYPE"), col::VALUE).await,
        Err(AppError::Unsupported(_))
    ));
}

#[tokio::test]
async fn a_commit_reports_each_change_and_one_fails() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    let s = session(&fake, 0).await;
    let m = meta(&s, 1).await;
    let req = ApplyRequest {
        table: db(1),
        changes: vec![
            StagedChange::Update {
                row_id: "a".into(),
                key: pk("str:big", "string"),
                changes: [(col::VALUE.to_string(), Value::Text("short".into()))].into(),
                exprs: None,
            },
            // An insert onto an existing key fails, and changes nothing.
            StagedChange::Insert {
                row_id: "b".into(),
                values: [
                    (col::KEY.to_string(), Value::Text("set:1".into())),
                    (col::VALUE.to_string(), Value::Text("x".into())),
                ]
                .into(),
                exprs: None,
            },
            StagedChange::Delete {
                row_id: "c".into(),
                key: pk("zset:1", "zset"),
            },
            StagedChange::Update {
                row_id: "d".into(),
                key: pk("str:plain", "string"),
                changes: [
                    (col::TTL.to_string(), Value::Int(60)),
                    (col::KEY.to_string(), Value::Text("str:renamed".into())),
                ]
                .into(),
                exprs: None,
            },
        ],
        returning: false,
    };
    let plan = s.plan_changes(&m, &req).unwrap();
    let preview: Vec<&str> = plan.preview.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        preview.contains(&"SET str:big short XX KEEPTTL"),
        "{preview:?}"
    );
    assert!(preview.contains(&"WATCH zset:1"), "{preview:?}");
    let result = s.apply_changes(&m, &req, plan).await;
    assert!(!result.success);
    let errors: Vec<Option<String>> = result
        .results
        .iter()
        .map(|r| r.error.as_ref().map(|e| e.message.clone()))
        .collect();
    assert_eq!(
        errors,
        [
            None,
            Some("a key with this name already exists".into()),
            None,
            None
        ]
    );
    assert_eq!(result.results[0].affected, 1);
    assert_eq!(result.results[1].affected, 0);
    assert_eq!(result.results[1].row_id, "b");
    assert_eq!(fake.data(1, b"str:big"), Some(Data::str("short")));
    assert!(fake.ttl(1, b"str:big").is_some(), "the TTL was kept");
    assert_eq!(fake.data(1, b"set:1"), Some(Data::set(&["m"])));
    assert_eq!(fake.data(1, b"zset:1"), None);
    assert_eq!(fake.data(1, b"str:renamed"), Some(Data::str("hello")));
    assert!(fake.ttl(1, b"str:renamed").is_some());
    assert!(result.sql.iter().any(|l| l == "MULTI"));
    // The edit connection selected db 1 for it; another database's edits SELECT again.
    let m0 = meta(&s, 0).await;
    let req0 = ApplyRequest {
        table: db(0),
        changes: vec![StagedChange::Insert {
            row_id: "e".into(),
            values: [
                (col::KEY.to_string(), Value::Text("in0".into())),
                (col::VALUE.to_string(), Value::Text("zero".into())),
                (col::TTL.to_string(), Value::Int(30)),
            ]
            .into(),
            exprs: None,
        }],
        returning: false,
    };
    let plan = s.plan_changes(&m0, &req0).unwrap();
    assert!(s.apply_changes(&m0, &req0, plan).await.success);
    assert_eq!(fake.data(0, b"in0"), Some(Data::str("zero")));
    assert!(fake.log().iter().any(|l| l == "SELECT 0"));
    // A plan the session did not make is refused, not guessed at.
    let forged = crate::drivers::native::NativePlan {
        preview: Vec::new(),
        ops: Box::new(42u8),
    };
    assert!(!s.apply_changes(&m0, &req0, forged).await.success);
}

#[tokio::test]
async fn the_console_keeps_its_database_and_transaction() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    let s = session(&fake, 0).await;
    assert_eq!(
        console(&s, "GET str:plain").await.unwrap().sets[0].rows[0][0],
        Value::Null
    );
    console(&s, "SELECT 1").await.unwrap();
    assert_eq!(
        first_cell(&console(&s, "GET \"str:plain\"").await.unwrap()),
        Value::Text("hello".into())
    );
    let hash = console(&s, "HGETALL hash:1").await.unwrap();
    assert_eq!(hash.sets[0].columns[0].name, "field");
    assert_eq!(hash.sets[0].rows.len(), 2);
    // The row cap.
    let mut sink = CollectSink::default();
    let outcome = s
        .run_console("LRANGE list:1 0 -1", 2, &mut sink, CancelSlot::default())
        .await
        .unwrap();
    assert!(outcome.truncated);
    assert_eq!(sink.sets[0].rows.len(), 2);
    // MULTI holds across commands, EXEC answers for each.
    assert_eq!(s.in_transaction(), Some(false));
    console(&s, "MULTI").await.unwrap();
    assert_eq!(s.in_transaction(), Some(true));
    assert_eq!(
        first_cell(&console(&s, "SET tx:1 one").await.unwrap()),
        Value::Text("QUEUED".into())
    );
    let exec = console(&s, "EXEC").await.unwrap();
    assert_eq!(exec.sets[0].rows.len(), 1);
    assert_eq!(s.in_transaction(), Some(false));
    assert_eq!(fake.data(1, b"tx:1"), Some(Data::str("one")));
    // A SELECT queued in a discarded MULTI leaves the console's database as it was.
    console(&s, "MULTI").await.unwrap();
    console(&s, "SELECT 0").await.unwrap();
    console(&s, "DISCARD").await.unwrap();
    assert_eq!(s.in_transaction(), Some(false));
    assert_eq!(*lock(&s.console_db), 1);
    // A server error is the command's error; blocking and pub/sub commands are refused.
    let err = console(&s, "LPUSH hash:1 x").await.err().unwrap();
    assert!(err.to_string().starts_with("WRONGTYPE"), "{err}");
    for refused in ["SUBSCRIBE ch", "BLPOP q 0", "HELLO 2", "MONITOR"] {
        assert!(
            matches!(console(&s, refused).await, Err(AppError::Unsupported(_))),
            "{refused}"
        );
    }
    assert!(matches!(
        console(&s, "GET \"open").await,
        Err(AppError::InvalidInput(_))
    ));
    // Splitting and classifying the way the renderer's Safe Mode does.
    let split = s.split_console("GET a; SET b \"x;y\"\nDEL c");
    assert_eq!(split.len(), 3);
    assert_eq!(split[1].sql, "SET b \"x;y\"");
    assert_eq!(s.classify_console("GET a"), StatementClass::Read);
    assert_eq!(s.classify_console("SELECT 2"), StatementClass::Read);
    assert_eq!(s.classify_console("DEL c"), StatementClass::Write);
    assert_eq!(
        s.classify_console("EVAL \"return 1\" 0"),
        StatementClass::Write
    );
    assert_eq!(s.classify_console("FOO.BAR x"), StatementClass::Write);
    // The console runs on a connection of its own: the grid still reads its database.
    let m = meta(&s, 0).await;
    let page = s
        .browse(&m, &request(0, 10, None, None), CancelSlot::default())
        .await
        .unwrap();
    assert!(page.rows.is_empty());
    assert!(s.console_lock().is_some());
}

#[tokio::test]
async fn a_cancel_stops_the_console_and_a_time_out_is_reported() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    let s = Arc::new(session(&fake, 0).await);
    console(&s, "SELECT 1").await.unwrap();
    let slot = CancelSlot::default();
    let run = {
        let (s, slot) = (s.clone(), slot.clone());
        tokio::spawn(async move {
            let mut sink = CollectSink::default();
            s.run_console("DEBUG SLEEP 30", 10, &mut sink, slot).await
        })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    let started = Instant::now();
    let handle = request_cancel(&slot).expect("the command is running");
    s.cancel(handle).await.unwrap();
    let out = tokio::time::timeout(Duration::from_secs(3), run)
        .await
        .expect("the run ends")
        .expect("the task");
    assert!(matches!(out, Err(AppError::Cancelled)), "{out:?}");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(
        fake.log().iter().any(|l| l.starts_with("CLIENT KILL ID")),
        "{:?}",
        fake.log()
    );
    // The next command opens a new console connection on the database it had selected.
    assert_eq!(
        first_cell(&console(&s, "GET str:plain").await.unwrap()),
        Value::Text("hello".into())
    );

    // The query time out of a console run: the command is cancelled and reported as timed out.
    let options = RunOptions {
        timeout_secs: 1,
        ..RunOptions::default()
    };
    let mut events = Vec::new();
    let mut emit = |e: crate::types::query::QueryEvent| events.push(e);
    let results = crate::commands::console::run_console_inner(
        s.as_ref(),
        "DEBUG SLEEP 30\nGET str:plain",
        &options,
        &CancelSlot::default(),
        &mut emit,
    )
    .await;
    assert_eq!(results.len(), 1, "the run stops at the time out");
    let error = results[0].error.as_ref().expect("timed out");
    assert_eq!(error.code, "QUERY_TIMEOUT");

    // A grid read under the time out stops waiting (its connection is shared, so not killed).
    fake.set_slow(Some(("SCAN", Duration::from_millis(1500))));
    let m = meta(&s, 1).await;
    let slot = CancelSlot::default();
    let (out, expired) = crate::commands::query::with_timeout_by(
        |h| s.cancel(h),
        &slot,
        Some(Duration::from_millis(200)),
        s.browse(&m, &request(1, 5, None, None), slot.clone()),
    )
    .await;
    assert!(
        expired && matches!(out, Err(AppError::Cancelled)),
        "{out:?}"
    );
    fake.set_slow(None);
    let page = s
        .browse(&m, &request(1, 5, None, None), CancelSlot::default())
        .await
        .unwrap();
    assert!(!page.rows.is_empty());
}

#[tokio::test]
async fn a_read_only_profile_refuses_every_write_path() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    let mut p = profile(&fake, 1);
    p.options.read_only = true;
    let s = open(&p, None).await.unwrap();
    let m = meta(&s, 1).await;
    let ro = |r: Result<(), AppError>| matches!(r, Err(AppError::InvalidInput(m)) if m.contains("read-only"));
    let req = ApplyRequest {
        table: db(1),
        changes: vec![StagedChange::Delete {
            row_id: "a".into(),
            key: pk("str:plain", "string"),
        }],
        returning: false,
    };
    assert!(ro(s.plan_changes(&m, &req).map(|_| ())));
    // A plan made elsewhere is refused at apply too.
    let edits =
        crate::sqlgen::native::redis::plan_changes(&req.changes, &Default::default()).unwrap();
    let forged = crate::drivers::native::NativePlan {
        preview: Vec::new(),
        ops: Box::new(super::apply::Plan { db: 1, edits }),
    };
    let result = s.apply_changes(&m, &req, forged).await;
    assert!(!result.success);
    // The server's flags were read: they add a core read, never a module's command.
    assert_eq!(s.classify_console("NEWREAD k"), StatementClass::Read);
    for write in [
        "SET a 1",
        "DEL str:plain",
        "FLUSHDB",
        "EVAL \"return 1\" 0",
        "MULTI",
        "KEYS *",
        // Writes the fake's COMMAND flags `readonly`, as RediSearch before 8, RedisBloom and
        // RedisGraph do, and a module's command flagged `readonly`.
        "FT.ALIASUPDATE alias idx2",
        "FT.DICTADD dict word",
        "FT.CURSOR DEL idx 1",
        "CF.COMPACT cf",
        "GRAPH.SLOWLOG g RESET",
        "MODREAD k",
        // The server reads options with strcasecmp, which stops at the first NUL.
        "SORT list:1 \"STORE\\x00\" dst",
        "SORT src \"store\\x00junk\" dst",
        "GEORADIUS g 0 0 1 km \"STORE\\x00\" dst",
        "GEORADIUSBYMEMBER g m 1 km \"STOREDIST\\x00\" dst",
        "GRAPH.SLOWLOG g \"RESET\\x00\"",
    ] {
        assert_eq!(s.classify_console(write), StatementClass::Write, "{write}");
        assert!(ro(console(&s, write).await.map(|_| ())), "{write}");
    }
    assert_eq!(
        first_cell(&console(&s, "GET str:plain").await.unwrap()),
        Value::Text("hello".into())
    );
    for (method, args) in [
        ("flushDb", json!({"db": 1})),
        (
            "copyKey",
            json!({"db": 1, "from": {"t": "text", "v": "str:plain"}, "to": {"t": "text", "v": "c"}}),
        ),
        (
            "editKey",
            json!({"db": 1, "key": {"t": "text", "v": "hash:1"}, "type": "hash", "edits": []}),
        ),
        (
            "newKey",
            json!({"db": 1, "key": {"t": "text", "v": "n"}, "value": {"type": "string", "value": {"t": "text", "v": "x"}}}),
        ),
        ("somethingNew", json!({})),
    ] {
        assert_eq!(s.call_kind(method), CallKind::Write);
        assert!(ro(s.call(method, args).await.map(|_| ())), "{method}");
    }
    assert!(s.call("serverInfo", json!({})).await.is_ok());
    let killed = s
        .kill_processes(&["1".into()], crate::types::admin::KillScope::Connection)
        .await
        .unwrap();
    assert!(!killed[0].ok);
    // Bootstrap commands that write keep a read-only profile from connecting.
    p.options.startup_sql = Some("SET boot 1".into());
    assert!(open(&p, None).await.is_err());
    // Nothing was written.
    let log = fake.log();
    assert!(
        log.iter().all(|l| ![
            "SET",
            "DEL",
            "UNLINK",
            "FLUSHDB",
            "EVAL",
            "MULTI",
            "KEYS",
            "COPY",
            "FT.ALIASUPDATE",
            "FT.DICTADD",
            "FT.CURSOR",
            "CF.COMPACT",
            "GRAPH.SLOWLOG",
            "MODREAD"
        ]
        .contains(&l.split(' ').next().unwrap_or(""))),
        "{log:?}"
    );
    assert_eq!(fake.data(1, b"str:plain"), Some(Data::str("hello")));
}

#[tokio::test]
async fn tls_with_a_ca_a_client_certificate_and_a_tunnel() {
    crate::testkit::install_crypto();
    let fake = FakeRedis::with(Options {
        tls: Some(crate::testkit::tls::server_config()),
        ..Options::default()
    })
    .await;
    fake.seed(0, b"k", Data::str("v"), None);
    let ca = crate::testkit::tls::ca_file();
    let mut p = profile(&fake, 0);
    p.ssl = SslMode::VerifyFull;
    p.ssl_ca_path = Some(ca.path().to_string_lossy().into_owned());
    let s = open(&p, None).await.unwrap();
    assert_eq!(
        first_cell(&console(&s, "GET k").await.unwrap()),
        Value::Text("v".into())
    );
    // Without the CA the certificate is not trusted; `require` encrypts without checking.
    p.ssl_ca_path = None;
    let err = open(&p, None).await.err().unwrap();
    assert!(err.to_string().contains("tls"), "{err}");
    p.ssl = SslMode::Require;
    open(&p, None).await.unwrap();
    // Off speaks plain RESP to a TLS port, which never answers it.
    p.ssl = SslMode::Disable;
    p.options.connect_timeout_secs = Some(1);
    assert!(open(&p, None).await.is_err());

    // Through a tunnel: the socket goes to the tunnel's address, the certificate is checked for
    // the profile's host.
    let mut tunneled = profile(&fake, 0);
    tunneled.host = Some("db.example.test".into());
    tunneled.ssl = SslMode::VerifyFull;
    tunneled.ssl_ca_path = Some(ca.path().to_string_lossy().into_owned());
    let s = RedisSession::connect(ConnectTarget {
        profile: &tunneled,
        password: None,
        host_override: Some(("127.0.0.1".into(), fake.addr.port())),
    })
    .await
    .unwrap();
    s.ping().await.unwrap();

    // A server that wants a client certificate.
    let mtls = FakeRedis::with(Options {
        tls: Some(tls_requiring_client_cert()),
        ..Options::default()
    })
    .await;
    let mut p = profile(&mtls, 0);
    p.ssl = SslMode::VerifyCa;
    p.ssl_ca_path = Some(ca.path().to_string_lossy().into_owned());
    p.options.connect_timeout_secs = Some(3);
    assert!(open(&p, None).await.is_err(), "no client certificate");
    let pem = |text: &str| {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(text.as_bytes()).unwrap();
        f
    };
    let (cert, key) = (
        pem(crate::testkit::tls::CLIENT_CERT_PEM),
        pem(crate::testkit::tls::CLIENT_KEY_PEM),
    );
    p.options.ssl_cert_path = Some(cert.path().to_string_lossy().into_owned());
    p.options.ssl_key_path = Some(key.path().to_string_lossy().into_owned());
    let s = open(&p, None).await.unwrap();
    s.ping().await.unwrap();
}

#[tokio::test]
async fn command_passwords_and_bootstrap_commands_on_every_connection() {
    let fake = FakeRedis::with(Options {
        password: Some("tok3n".into()),
        ..Options::default()
    })
    .await;
    let mut p = profile(&fake, 0);
    p.options.password_mode = Some(crate::types::profile::PasswordMode::Command);
    p.options.password_command = Some("printf tok3n".into());
    p.options.startup_sql = Some("CLIENT SETNAME tabler-app".into());
    // What the command printed for this connect (the app runs it before `open`).
    let s = open(&p, Some("tok3n")).await.unwrap();
    console(&s, "PING").await.unwrap();
    let m = meta(&s, 2).await;
    s.browse(&m, &request(2, 5, None, None), CancelSlot::default())
        .await
        .unwrap();
    let log = fake.log();
    let named = log
        .iter()
        .filter(|l| *l == "CLIENT SETNAME tabler-app")
        .count();
    // The grid connections of db 0 and db 2 and the console's.
    assert_eq!(named, 3, "{log:?}");
    assert!(
        log.iter()
            .filter(|l| l.starts_with("HELLO 3 AUTH default tok3n"))
            .count()
            >= 3
    );
    // A bootstrap command that fails fails the connect, with its text.
    p.options.startup_sql = Some("CONFIG SET nope 1".into());
    let err = open(&p, Some("tok3n")).await.err().unwrap();
    assert!(err.to_string().contains("CONFIG SET nope 1"), "{err}");
}

#[tokio::test]
async fn calls_serve_the_value_editor_and_the_server_panel() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    let entry: &[(&str, &str)] = &[("n", "x")];
    fake.seed(
        1,
        b"stream:2",
        Data::stream(&[
            ("1-0", entry),
            ("2-0", entry),
            ("3-0", entry),
            ("4-0", entry),
            ("5-0", entry),
        ]),
        None,
    );
    let s = session(&fake, 0).await;
    let key = |k: &str, ty: &str| json!({"db": 1, "key": {"t": "text", "v": k}, "type": ty});
    let page = |args: serde_json::Value| {
        let s = &s;
        async move { s.call("keyPage", args).await.unwrap() }
    };
    let with = |mut base: serde_json::Value, extra: serde_json::Value| {
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        base
    };
    let hash = page(with(key("hash:1", "hash"), json!({"count": 1}))).await;
    assert_eq!(hash["size"], json!({"t": "int", "v": 2}));
    assert_eq!(hash["elements"].as_array().unwrap().len(), 1);
    assert!(hash["nextCursor"].is_string());
    let list = page(with(
        key("list:1", "list"),
        json!({"count": 2, "cursor": "1"}),
    ))
    .await;
    assert_eq!(list["elements"][0]["name"], json!({"t": "int", "v": 1}));
    assert_eq!(list["elements"][1]["value"], json!({"t": "text", "v": "c"}));
    assert_eq!(list["nextCursor"], serde_json::Value::Null);
    let zset = page(key("zset:1", "zset")).await;
    assert_eq!(
        zset["elements"][1]["value"],
        json!({"t": "float", "v": 2.5})
    );
    let stream = page(with(key("stream:2", "stream"), json!({"count": 2}))).await;
    assert_eq!(
        stream["elements"][0]["name"],
        json!({"t": "text", "v": "5-0"})
    );
    assert_eq!(stream["nextCursor"], json!("4-0"));
    let older = page(with(
        key("stream:2", "stream"),
        json!({"count": 2, "cursor": "4-0"}),
    ))
    .await;
    assert_eq!(
        older["elements"][0]["name"],
        json!({"t": "text", "v": "3-0"})
    );
    let string = page(key("str:plain", "string")).await;
    assert_eq!(string["value"], json!({"t": "text", "v": "hello"}));
    let json_doc = page(key("json:1", "ReJSON-RL")).await;
    assert_eq!(
        json_doc["value"],
        json!({"t": "json", "v": "{\"a\":[1,2]}"})
    );
    // The type changed since the editor loaded it.
    assert!(s.call("keyPage", key("list:1", "hash")).await.is_err());

    let text = |v: &str| json!({"t": "text", "v": v});
    let edits = json!([
        {"op": "hashRename", "from": text("f"), "to": text("f2"), "value": text("v")},
        {"op": "hashAdd", "field": text("g"), "value": text("dup")},
        {"op": "hashSet", "field": text("g"), "value": text("w2")},
    ]);
    let preview = s
        .call(
            "previewEdits",
            with(key("hash:1", "hash"), json!({"edits": edits.clone()})),
        )
        .await
        .unwrap();
    assert!(
        preview
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l == "WATCH hash:1")
    );
    assert!(
        fake.log().iter().all(|l| !l.starts_with("HSETNX")),
        "a preview sends nothing"
    );
    let done = s
        .call(
            "editKey",
            with(key("hash:1", "hash"), json!({"edits": edits})),
        )
        .await
        .unwrap();
    let oks: Vec<bool> = done["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["ok"].as_bool().unwrap())
        .collect();
    assert_eq!(oks, [true, false, true]);
    assert_eq!(
        done["results"][1]["error"],
        json!("the field already exists")
    );
    assert_eq!(
        fake.data(1, b"hash:1"),
        Some(Data::hash(&[("g", "w2"), ("f2", "v")]))
    );
    let list_edits = json!([
        {"op": "listDelete", "index": 1, "old": text("b")},
        {"op": "listInsert", "before": true, "pivot": text("zz"), "value": text("q")},
        {"op": "listPush", "values": [text("d")], "head": false},
    ]);
    let done = s
        .call(
            "editKey",
            with(key("list:1", "list"), json!({"edits": list_edits})),
        )
        .await
        .unwrap();
    assert_eq!(done["results"][1]["ok"], json!(false));
    assert_eq!(fake.data(1, b"list:1"), Some(Data::list(&["a", "c", "d"])));
    let made = s
        .call(
            "newKey",
            json!({"db": 1, "key": text("z:new"), "ttl": {"t": "int", "v": 60},
                   "value": {"type": "zset", "members": [{"member": text("m"), "score": "1.5"}]}}),
        )
        .await
        .unwrap();
    assert_eq!(made["results"][0]["ok"], json!(true));
    assert!(fake.ttl(1, b"z:new").is_some());
    let again = s
        .call(
            "newKey",
            json!({"db": 1, "key": text("z:new"),
                   "value": {"type": "set", "members": [text("x")]}}),
        )
        .await
        .unwrap();
    assert_eq!(
        again["results"][0]["error"],
        json!("a key with this name already exists")
    );
    let copied = s
        .call(
            "copyKey",
            json!({"db": 1, "from": text("str:plain"), "to": text("str:copy")}),
        )
        .await
        .unwrap();
    assert_eq!(copied["results"][0]["ok"], json!(true));
    assert_eq!(fake.data(1, b"str:copy"), Some(Data::str("hello")));
    s.call("flushDb", json!({"db": 1})).await.unwrap();
    assert_eq!(fake.data(1, b"str:plain"), None);
    assert!(s.call("flushDb", json!({"db": 99})).await.is_err());

    let info = s.call("serverInfo", json!({})).await.unwrap();
    assert_eq!(info["product"], json!("Redis"));
    assert_eq!(info["protocol"], json!(3));
    assert!(
        info["sections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "Server")
    );
    let slow = s.call("slowLog", json!({})).await.unwrap();
    assert_eq!(slow[0]["command"], json!("KEYS *"));
    assert_eq!(slow[0]["durationUs"], json!({"t": "int", "v": 15000}));
    let err = s.call("dropEverything", json!({})).await.unwrap_err();
    assert!(err.to_string().contains("unknown call"), "{err}");
    assert!(s.call("keyPage", json!({"db": 1})).await.is_err());
}

/// Keys and values travel as argv: protocol bytes, quotes, `;` and line breaks inside them stay
/// inside, whether the grid writes them or the console reads them back.
#[tokio::test]
async fn injection_stays_inside_the_arguments() {
    let fake = FakeRedis::start().await;
    fake.seed(0, b"victim", Data::str("safe"), None);
    let s = session(&fake, 0).await;
    let m = meta(&s, 0).await;
    let evil_key = "k\r\n*1\r\n$8\r\nFLUSHALL\r\n";
    let evil_value = "v\"; FLUSHDB\nDEL victim";
    let req = ApplyRequest {
        table: db(0),
        changes: vec![StagedChange::Insert {
            row_id: "x".into(),
            values: [
                (col::KEY.to_string(), Value::Text(evil_key.into())),
                (col::VALUE.to_string(), Value::Text(evil_value.into())),
            ]
            .into(),
            exprs: None,
        }],
        returning: false,
    };
    let plan = s.plan_changes(&m, &req).unwrap();
    // The preview quotes what it shows, and reads back as the same bytes.
    let shown = &plan.preview[0].1;
    assert_eq!(
        super::cli::tokenize(shown).unwrap(),
        [
            b"SET".to_vec(),
            evil_key.as_bytes().to_vec(),
            evil_value.as_bytes().to_vec(),
            b"NX".to_vec()
        ]
    );
    assert!(s.apply_changes(&m, &req, plan).await.success);
    assert_eq!(
        fake.data(0, evil_key.as_bytes()),
        Some(Data::str(evil_value))
    );
    assert_eq!(fake.data(0, b"victim"), Some(Data::str("safe")));
    let log = fake.log();
    assert!(
        log.iter()
            .all(|l| !l.starts_with("FLUSH") && !l.starts_with("DEL")),
        "{log:?}"
    );
    // The exact-key filter finds it byte for byte; the console reads it through quotes.
    let page = s
        .browse(
            &m,
            &request(0, 5, None, filter("key", FilterOp::Eq, evil_key)),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(page.rows[0][5], Value::Text(evil_value.into()));
    let quoted = crate::sqlgen::native::redis::quote_arg(evil_key.as_bytes());
    assert_eq!(
        first_cell(&console(&s, &format!("GET {quoted}")).await.unwrap()),
        Value::Text(evil_value.into())
    );
    // A glob in a contains filter is matched literally: of the two keys, only the evil one holds
    // a `*`, and neither a `?` (as globs, both would match both).
    for (needle, want) in [("*", 1), ("?", 0)] {
        let n = s
            .count(
                &m,
                filter("key", FilterOp::Contains, needle).as_ref(),
                true,
                CancelSlot::default(),
            )
            .await
            .unwrap();
        assert_eq!((n.count, n.exact), (want, true), "{needle}");
    }
}

#[tokio::test]
async fn the_process_list_shows_and_kills_clients() {
    let fake = FakeRedis::start().await;
    let s = session(&fake, 0).await;
    console(&s, "PING").await.unwrap();
    let other = session(&fake, 0).await;
    let rows = s.list_processes().await.unwrap().unwrap();
    assert!(rows.len() >= 3, "{rows:?}");
    assert_eq!(rows.iter().filter(|r| r.current).count(), 1);
    assert!(rows.iter().filter(|r| r.own).count() >= 2);
    let theirs: Vec<String> = rows
        .iter()
        .filter(|r| !r.own)
        .map(|r| r.id.clone())
        .collect();
    assert!(!theirs.is_empty());
    let killed = s
        .kill_processes(
            &[theirs[0].clone(), "999999".into(), "x".into()],
            crate::types::admin::KillScope::Query,
        )
        .await
        .unwrap();
    assert!(killed[0].ok, "{killed:?}");
    assert!(!killed[1].ok && !killed[2].ok);
    // The killed session finds its connection gone and opens a new one for the next read.
    other.ping().await.unwrap();
    s.close().await;
    assert!(matches!(s.ping().await, Err(AppError::NotConnected)));
    assert!(console(&s, "PING").await.is_err());
}

#[tokio::test]
async fn grid_inserts_take_the_rows_the_renderer_stages() {
    let fake = FakeRedis::start().await;
    seed(&fake);
    fake.seed(
        1,
        b"dup:src",
        Data::str("copied"),
        Some(Duration::from_secs(100)),
    );
    let s = session(&fake, 0).await;
    let m = meta(&s, 1).await;
    let names: Vec<String> = m.columns.iter().map(|c| c.name.clone()).collect();
    let insert = |row: &str, cells: Vec<Value>| StagedChange::Insert {
        row_id: row.into(),
        values: names.iter().cloned().zip(cells).collect(),
        exprs: None,
    };
    // A new row: every cell starts missing; the key and the value were typed in.
    let missing = Value::other("missing", "");
    let mut new_row = vec![missing.clone(); names.len()];
    new_row[0] = Value::Text("new:typed".into());
    new_row[5] = Value::Text("typed in".into());
    // A duplicated row copies every cell of the browsed one (its type, TTL, size and memory
    // too) and gets a new name.
    let page = s
        .browse(
            &m,
            &request(1, 10, None, filter(col::KEY, FilterOp::Eq, "dup:src")),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    let mut copy = page.rows[0].clone();
    assert_eq!(copy[1], Value::Text("string".into()));
    assert!(matches!(copy[2], Value::Int(n) if n > 90), "{:?}", copy[2]);
    assert!(matches!(copy[3], Value::Int(6)), "{:?}", copy[3]);
    assert!(matches!(copy[4], Value::Int(_)), "{:?}", copy[4]);
    copy[0] = Value::Text("dup:copy".into());
    // A new row whose name was never typed in.
    let mut nameless = vec![missing.clone(); names.len()];
    nameless[5] = Value::Text("v".into());
    let req = ApplyRequest {
        table: db(1),
        changes: vec![insert("a", new_row), insert("b", copy)],
        returning: false,
    };
    let plan = s.plan_changes(&m, &req).unwrap();
    let preview: Vec<&str> = plan.preview.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(preview[0], "SET new:typed \"typed in\" NX");
    assert!(
        preview[1].starts_with("SET dup:copy copied NX PX "),
        "{preview:?}"
    );
    let result = s.apply_changes(&m, &req, plan).await;
    assert!(result.success, "{:?}", result.error);
    assert_eq!(fake.data(1, b"new:typed"), Some(Data::str("typed in")));
    assert!(fake.ttl(1, b"new:typed").is_none());
    assert_eq!(fake.data(1, b"dup:copy"), Some(Data::str("copied")));
    assert!(fake.ttl(1, b"dup:copy").is_some(), "the TTL was copied");
    let refused = ApplyRequest {
        table: db(1),
        changes: vec![insert("c", nameless)],
        returning: false,
    };
    let err = s.plan_changes(&m, &refused).err().expect("refused");
    assert!(err.to_string().contains("needs a name"), "{err}");
}

#[tokio::test]
async fn an_edit_connection_whose_select_timed_out_is_not_used_again() {
    let fake = FakeRedis::start().await;
    fake.seed(1, b"a", Data::str("one"), None);
    fake.seed(2, b"a", Data::str("two"), None);
    let s = session(&fake, 0).await;
    let set = |db: u32, v: &str| {
        json!({"db": db, "key": {"t": "text", "v": "a"}, "type": "string",
               "edits": [{"op": "stringSet", "value": {"t": "text", "v": v}}]})
    };
    let m2 = meta(&s, 2).await;
    // The edit connection is on db 1.
    let r = s.call("editKey", set(1, "uno")).await.unwrap();
    assert_eq!(r["results"][0]["ok"], json!(true), "{r}");
    // An edit of db 2 whose SELECT is answered after the reply time out: nothing of it was sent.
    tokio::time::pause();
    fake.set_slow(Some(("SELECT", Duration::from_secs(61))));
    let r = s.call("editKey", set(2, "never")).await.unwrap();
    assert_eq!(r["results"][0]["ok"], json!(false), "{r}");
    let error = r["results"][0]["error"].as_str().unwrap_or_default();
    assert!(error.starts_with("not applied"), "{error}");
    // The grid's apply says so too (its new connection's SELECT is as slow).
    let req = ApplyRequest {
        table: db(2),
        changes: vec![StagedChange::Update {
            row_id: "a".into(),
            key: pk("a", "string"),
            changes: [(col::VALUE.to_string(), Value::Text("never".into()))].into(),
            exprs: None,
        }],
        returning: false,
    };
    let plan = s.plan_changes(&m2, &req).unwrap();
    let result = s.apply_changes(&m2, &req, plan).await;
    assert!(!result.success);
    let error = result.results[0].error.as_ref().map(|e| e.message.as_str());
    assert!(
        error.is_some_and(|e| e.starts_with("not applied")),
        "{error:?}"
    );
    // The server runs that SELECT late; the next edit of db 1 does not land in db 2.
    tokio::time::sleep(Duration::from_secs(5)).await;
    fake.set_slow(None);
    tokio::time::resume();
    let r = s.call("editKey", set(1, "right")).await.unwrap();
    assert_eq!(r["results"][0]["ok"], json!(true), "{r}");
    assert_eq!(fake.data(1, b"a"), Some(Data::str("right")));
    assert_eq!(fake.data(2, b"a"), Some(Data::str("two")));
}

#[tokio::test]
async fn a_server_refusing_transactions_gets_no_unguarded_writes() {
    let fake = FakeRedis::start().await;
    fake.seed(0, b"l", Data::list(&["a"]), None);
    fake.seed(0, b"s1", Data::str("old"), None);
    let s = session(&fake, 0).await;
    let push = json!({"db": 0, "key": {"t": "text", "v": "l"}, "type": "list",
        "edits": [{"op": "listPush", "values": [{"t": "text", "v": "x"}], "head": false}]});
    let rpushes = || fake.log().iter().filter(|l| l.starts_with("RPUSH")).count();
    // An ACL user without @transaction: WATCH is refused, so nothing of the edit is sent.
    fake.deny(&["WATCH", "MULTI", "EXEC", "UNWATCH", "DISCARD"]);
    for _ in 0..2 {
        let r = s.call("editKey", push.clone()).await.unwrap();
        assert_eq!(r["results"][0]["ok"], json!(false), "{r}");
        let error = r["results"][0]["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("refused WATCH") && error.contains("NOPERM"),
            "{error}"
        );
    }
    assert_eq!(rpushes(), 0, "{:?}", fake.log());
    assert_eq!(fake.data(0, b"l"), Some(Data::list(&["a"])));
    // The grid's value edit too.
    let m = meta(&s, 0).await;
    let req = ApplyRequest {
        table: db(0),
        changes: vec![StagedChange::Update {
            row_id: "a".into(),
            key: pk("s1", "string"),
            changes: [(col::VALUE.to_string(), Value::Text("new".into()))].into(),
            exprs: None,
        }],
        returning: false,
    };
    let plan = s.plan_changes(&m, &req).unwrap();
    let result = s.apply_changes(&m, &req, plan).await;
    assert!(!result.success);
    assert_eq!(result.results[0].affected, 0);
    assert!(
        !fake.log().iter().any(|l| l.starts_with("SET ")),
        "{:?}",
        fake.log()
    );
    assert_eq!(fake.data(0, b"s1"), Some(Data::str("old")));
    // WATCH allowed, MULTI refused: the edit is refused before its commands are sent.
    fake.deny(&["MULTI"]);
    let r = s.call("editKey", push.clone()).await.unwrap();
    let error = r["results"][0]["error"].as_str().unwrap_or_default();
    assert!(error.contains("refused MULTI"), "{r}");
    assert_eq!(rpushes(), 0, "{:?}", fake.log());
    // EXEC refused: the push was only queued, so it did not run; the connection that is left
    // inside MULTI is not used again.
    fake.deny(&["EXEC"]);
    let r = s.call("editKey", push.clone()).await.unwrap();
    assert_eq!(r["results"][0]["ok"], json!(false), "{r}");
    assert_eq!(fake.data(0, b"l"), Some(Data::list(&["a"])));
    fake.deny(&[]);
    let r = s.call("editKey", push).await.unwrap();
    assert_eq!(r["results"][0]["ok"], json!(true), "{r}");
    assert_eq!(fake.data(0, b"l"), Some(Data::list(&["a", "x"])));
}

#[tokio::test]
async fn the_console_says_where_it_reopens_after_a_select_in_exec() {
    let fake = FakeRedis::start().await;
    fake.seed(0, b"x", Data::str("db0"), None);
    fake.seed(2, b"x", Data::str("db2"), None);
    let s = session(&fake, 0).await;
    for c in ["MULTI", "SELECT 2", "EXEC"] {
        console(&s, c).await.unwrap();
    }
    assert_eq!(*lock(&s.console_db), 2);
    assert_eq!(
        first_cell(&console(&s, "GET x").await.unwrap()),
        Value::Text("db2".into())
    );
    // The connection is lost (the server's idle time out, a kill from elsewhere).
    if let Some(l) = lock(&s.console).as_ref() {
        l.kill();
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut sink = CollectSink::default();
    let out = s
        .run_console("SET x changed", 10, &mut sink, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(
        out.notices,
        ["The console's connection was opened again, on db2."]
    );
    assert_eq!(fake.data(2, b"x"), Some(Data::str("changed")));
    assert_eq!(fake.data(0, b"x"), Some(Data::str("db0")));
    // A SELECT whose EXEC was refused (a watched key changed) leaves the database as it was;
    // a MULTI lost with the connection is named in the notice.
    console(&s, "WATCH x").await.unwrap();
    fake.seed(2, b"x", Data::str("touched"), None);
    for c in ["MULTI", "SELECT 0"] {
        console(&s, c).await.unwrap();
    }
    console(&s, "EXEC").await.unwrap();
    assert_eq!(*lock(&s.console_db), 2);
    assert_eq!(
        first_cell(&console(&s, "GET x").await.unwrap()),
        Value::Text("touched".into())
    );
    console(&s, "MULTI").await.unwrap();
    if let Some(l) = lock(&s.console).as_ref() {
        l.kill();
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut sink = CollectSink::default();
    let out = s
        .run_console("GET x", 10, &mut sink, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(
        out.notices,
        ["The console's connection was opened again, on db2: its MULTI and WATCH were discarded."]
    );
    assert_eq!(s.in_transaction(), Some(false));
}
