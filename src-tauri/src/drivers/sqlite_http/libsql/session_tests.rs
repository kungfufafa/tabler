//! The LibSQL driver end to end: `entry::open` through the registry, over the shared HTTP client,
//! against the Hrana fake served on a socket, and the app's command bodies on the session.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::entry::{self, LibsqlParams};
use super::error::STREAM_LOST_HINT;
use super::session;
use super::testing::FakeHrana;
use crate::commands::browse::{DEFAULT_ESTIMATE_ABOVE, browse_rows, count_rows, fetch_cell_on};
use crate::commands::ddl::table_ddl_on;
use crate::commands::lock::session_details_of;
use crate::commands::mutate::apply_changes_on;
use crate::commands::query::{explain_inner, query_limited, run_sql_inner};
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::native::CallKind;
use crate::drivers::registry::{self, UtilInput};
use crate::drivers::sqlite_http::testing::{Answers, Sink, app_connection, profile, serve};
use crate::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, Session, TxStatement, request_cancel,
};
use crate::error::AppError;
use crate::testkit::FakeServer;
use crate::types::browse::{BrowseRequest, RowIdentity, RowKey};
use crate::types::caps::{CountMode, Feature};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::{ApplyRequest, ServerExpr, StagedChange};
use crate::types::profile::{ConnectionProfile, Driver};
use crate::types::query::{ExplainMode, RunOptions, StatementKind, StatementResult};
use crate::types::schema::TableRef;
use crate::types::value::Value;

const TOKEN: &str = "tok";

fn seeded() -> Arc<FakeHrana> {
    let fake = Arc::new(FakeHrana::new());
    fake.seed(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT UNIQUE, n NUMERIC, at DATETIME, doc JSON,
                         big INTEGER, b BLOB);
         INSERT INTO t (v, n, at, doc, big, b) VALUES
           ('a', 1.5, '2026-10-30 12:00:00', '{\"k\":[1,2]}', 9007199254740993, x'0102'),
           ('b', NULL, NULL, NULL, -1, NULL), ('c', 3, NULL, NULL, 0, NULL),
           ('d', 4, NULL, NULL, 0, NULL), ('e', 5, NULL, NULL, 0, NULL);
         CREATE TABLE nokey (x TEXT);
         INSERT INTO nokey VALUES ('p'), ('p'), ('q');
         CREATE VIEW tv AS SELECT id, v FROM t;
         CREATE TABLE libsql_wasm_func_table (name TEXT);",
    );
    fake
}

fn count(fake: &FakeHrana, table: &str) -> i64 {
    fake.connect()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

/// A profile for the fake at `server`, with `fields` over it.
fn libsql_profile(server: &FakeServer, fields: serde_json::Value) -> ConnectionProfile {
    let p = profile(
        Driver::Libsql,
        json!({"host": "127.0.0.1", "port": server.addr().port()}),
    );
    let mut patched = serde_json::to_value(&p).unwrap();
    for (k, v) in fields.as_object().unwrap() {
        if k == "options" {
            for (ok, ov) in v.as_object().unwrap() {
                patched["options"][ok] = ov.clone();
            }
        } else {
            patched[k] = v.clone();
        }
    }
    serde_json::from_value(patched).unwrap()
}

async fn open(profile: &ConnectionProfile, password: Option<&str>) -> Result<Session, AppError> {
    registry::open(ConnectTarget {
        profile,
        password,
        host_override: None,
    })
    .await
}

/// The fake on a socket requiring the token, and a session opened on it with `fields`.
async fn connected(
    fake: &Arc<FakeHrana>,
    fields: serde_json::Value,
) -> (FakeServer, ConnectionProfile, Arc<dyn DbSession>) {
    let server = serve(
        fake.clone() as Arc<dyn Answers>,
        Some(&format!("Bearer {TOKEN}")),
    )
    .await;
    let p = libsql_profile(&server, fields);
    let session = open(&p, Some(TOKEN)).await.unwrap().sql().unwrap();
    (server, p, session)
}

fn t() -> TableRef {
    TableRef::new(Some("main"), "t")
}

fn pk(id: i64) -> RowKey {
    RowKey::Pk(BTreeMap::from([("id".to_string(), Value::Int(id))]))
}

fn text(v: &str) -> Value {
    Value::Text(v.into())
}

async fn run(session: &dyn DbSession, sql: &str, max_rows: u32) -> Vec<StatementResult> {
    let options = RunOptions {
        max_rows,
        stop_on_error: true,
        ..RunOptions::default()
    };
    let mut emit = |_| {};
    run_sql_inner(session, sql, &options, &CancelSlot::default(), &mut emit).await
}

#[tokio::test]
async fn connects_over_http_and_reads_the_catalog() {
    let fake = seeded();
    let (server, _, s) = connected(&fake, json!({})).await;
    let info = s.server_info().await.unwrap();
    assert!(
        info.version.starts_with("LibSQL (SQLite 3."),
        "{}",
        info.version
    );
    assert_eq!(info.current_database.as_deref(), Some("127.0.0.1"));
    let dbs = s.list_databases().await.unwrap();
    assert_eq!((dbs.len(), dbs[0].is_current), (1, true));
    assert_eq!(s.list_schemas().await.unwrap()[0].name, "main");
    let tables: Vec<String> = s
        .list_tables("main")
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(tables, ["nokey", "t", "tv"]);
    assert!(s.list_tables("temp").await.unwrap().is_empty());
    let meta = s.table_meta(&t()).await.unwrap();
    assert_eq!(meta.columns.len(), 7);
    assert_eq!(meta.primary_key, ["id"]);
    assert!(meta.indexes.iter().any(|i| i.unique && i.columns == ["v"]));
    assert_eq!(
        s.view_definition(&TableRef::new(None, "tv")).await.unwrap(),
        Some("CREATE VIEW tv AS SELECT id, v FROM t".into())
    );
    let completion = s.completion_metadata().await.unwrap();
    assert_eq!(completion.schemas["main"]["tv"], ["id", "v"]);
    assert!(
        s.table_ddl(&t())
            .await
            .unwrap()
            .unwrap()
            .starts_with("CREATE TABLE t (")
    );
    s.ping().await.unwrap();
    // Every request carried the token.
    assert!(
        server
            .requests()
            .iter()
            .all(|r| r.header("authorization") == Some("Bearer tok"))
    );

    let caps = s.capabilities();
    assert_eq!(caps, session::capabilities(false));
    assert!(caps.has(Feature::EditWithoutKey) && caps.has(Feature::Returning));
    assert!(!caps.has(Feature::StructureEdit) && !caps.has(Feature::CancelOnServer));
    assert_eq!(caps.data.count, CountMode::Costly);
    // A writable session never asks whether the server takes `query_only`.
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    assert_eq!(caps.session.keepalive_secs, None);

    // After close the session sends nothing more.
    s.close().await;
    assert!(matches!(s.ping().await, Err(AppError::NotConnected)));
    assert!(matches!(
        s.query("SELECT 1", &[]).await,
        Err(AppError::NotConnected)
    ));
}

#[tokio::test]
async fn refused_tokens_and_failing_startup_commands_fail_the_connect() {
    let fake = seeded();
    let server = serve(fake.clone() as Arc<dyn Answers>, Some("Bearer tok")).await;
    let p = libsql_profile(&server, json!({}));
    assert!(matches!(
        open(&p, Some("nope")).await,
        Err(AppError::Auth(_))
    ));
    assert!(matches!(open(&p, None).await, Err(AppError::Auth(_))));
    // Without a token no authorization header goes out at all.
    assert_eq!(
        server.requests().pop().unwrap().header("authorization"),
        None
    );
    let broken = libsql_profile(
        &server,
        json!({"options": {"startupSql": "SELECT * FROM nope"}}),
    );
    let e = open(&broken, Some(TOKEN)).await.err().unwrap();
    assert!(e.to_string().contains("startup statements failed"), "{e}");
}

#[tokio::test]
async fn basic_auth_sends_the_user_and_the_password() {
    let fake = seeded();
    // base64("u:p")
    let server = serve(fake.clone() as Arc<dyn Answers>, Some("Basic dTpw")).await;
    let p = libsql_profile(
        &server,
        json!({"user": "u", "options": {"driverParams": {"basicAuth": true}}}),
    );
    let s = open(&p, Some("p")).await.unwrap();
    s.ping().await.unwrap();
}

#[tokio::test]
async fn an_ssh_tunnel_keeps_the_servers_name() {
    let fake = seeded();
    let server = serve(fake.clone() as Arc<dyn Answers>, None).await;
    let p = profile(
        Driver::Libsql,
        json!({"host": "db.example.test", "port": 8080}),
    );
    let s = registry::open(ConnectTarget {
        profile: &p,
        password: None,
        host_override: Some(("127.0.0.1".into(), server.addr().port())),
    })
    .await
    .unwrap();
    s.ping().await.unwrap();
    let requests = server.requests();
    assert!(!requests.is_empty());
    assert!(
        requests
            .iter()
            .all(|r| r.header("host") == Some("db.example.test:8080")),
        "{requests:?}"
    );
}

#[tokio::test]
async fn tls_checks_the_certificate_the_profile_trusts() {
    use crate::testkit::tls::{TlsServer, ca_file};
    fn through(p: &ConnectionProfile, port: u16) -> ConnectTarget<'_> {
        ConnectTarget {
            profile: p,
            password: None,
            host_override: Some(("127.0.0.1".into(), port)),
        }
    }
    let server = TlsServer::start();
    let port = server.addr().port();
    let ca = ca_file();
    let mut p = profile(
        Driver::Libsql,
        json!({"host": "db.example.test", "port": 443, "ssl": "verify-full"}),
    );
    // Without the test CA the certificate is refused before anything is sent.
    let e = registry::open(through(&p, port)).await.err().unwrap();
    assert!(matches!(e, AppError::Connect(_)), "{e:?}");
    assert!(server.requests().is_empty());
    // With it the handshake passes, for the server's name; the server is no LibSQL one.
    p.ssl_ca_path = Some(ca.path().to_string_lossy().into_owned());
    let e = registry::open(through(&p, port)).await.err().unwrap();
    assert!(e.to_string().contains("unexpected"), "{e}");
    let seen = server.requests();
    assert_eq!(seen[0].sni.as_deref(), Some("db.example.test"));
    assert_eq!(seen[0].line, "GET /v3 HTTP/1.1");
}

#[tokio::test]
async fn the_grid_reads_counts_fetches_and_commits_through_the_commands() {
    let fake = seeded();
    let (_server, p, s) = connected(&fake, json!({})).await;
    let (state, conn) = app_connection(p, Session::Sql(s.clone())).await;
    let page = |table: &str, offset: u64| BrowseRequest {
        table: TableRef::new(Some("main"), table),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit: 2,
        offset,
        cursor: None,
    };
    let first = browse_rows(&conn, &page("t", 0), None).await.unwrap();
    assert_eq!(first.identity, RowIdentity::Pk);
    assert_eq!(first.rows.len(), 2);
    let row = &first.rows[0];
    assert_eq!(row[0], Value::Int(1));
    assert_eq!(row[1], text("a"));
    assert_eq!(row[3], Value::Timestamp("2026-10-30 12:00:00".into()));
    assert_eq!(row[4], Value::Json("{\"k\":[1,2]}".into()));
    assert_eq!(row[5], Value::BigInt("9007199254740993".into()));
    assert!(matches!(row[6], Value::Bytes(_)));
    let third = browse_rows(&conn, &page("t", 4), None).await.unwrap();
    assert_eq!(third.rows.len(), 1);
    assert_eq!(third.rows[0][1], text("e"));
    let keyless = browse_rows(&conn, &page("nokey", 0), None).await.unwrap();
    assert_eq!(keyless.identity, RowIdentity::Rowid);
    assert_eq!(keyless.row_ids, Some(vec![1, 2]));

    let counted = count_rows(&conn, &t(), None, true, DEFAULT_ESTIMATE_ABOVE, None)
        .await
        .unwrap();
    assert_eq!((counted.count, counted.exact), (5, true));
    let blob = fetch_cell_on(&conn, &t(), &pk(1), "b", None).await.unwrap();
    assert_eq!(blob, Value::from_bytes(&[1, 2]));

    // One failing change (a duplicate key) and nothing is applied.
    let failing = ApplyRequest {
        table: t(),
        changes: vec![
            StagedChange::Update {
                row_id: "r1".into(),
                key: pk(1),
                changes: BTreeMap::from([("v".into(), text("z"))]),
                exprs: None,
            },
            StagedChange::Insert {
                row_id: "r2".into(),
                values: BTreeMap::from([("v".into(), text("b"))]),
                exprs: None,
            },
        ],
        returning: false,
    };
    let result = apply_changes_on(&state, conn.id, failing).await.unwrap();
    assert!(!result.success);
    assert!(result.results.iter().any(|r| r.error.is_some()));
    assert_eq!(count(&fake, "t WHERE v = 'z'"), 0);

    // A new row with a server expression, read back.
    let insert = ApplyRequest {
        table: t(),
        changes: vec![StagedChange::Insert {
            row_id: "r3".into(),
            values: BTreeMap::from([("v".into(), text("f"))]),
            exprs: Some(BTreeMap::from([(
                "at".into(),
                ServerExpr::CurrentTimestamp,
            )])),
        }],
        returning: true,
    };
    let result = apply_changes_on(&state, conn.id, insert).await.unwrap();
    assert!(result.success, "{:?}", result.error);
    let returned = result.results[0].returned.as_ref().expect("the row back");
    assert_eq!(returned["id"], Value::Int(6));
    assert_eq!(count(&fake, "t WHERE v = 'f' AND at IS NOT NULL"), 1);

    // Without a key: by rowid, and an all-columns match of two rows rolls back.
    let by_rowid = ApplyRequest {
        table: TableRef::new(Some("main"), "nokey"),
        changes: vec![StagedChange::Update {
            row_id: "k3".into(),
            key: RowKey::Rowid(3),
            changes: BTreeMap::from([("x".into(), text("r"))]),
            exprs: None,
        }],
        returning: false,
    };
    assert!(
        apply_changes_on(&state, conn.id, by_rowid)
            .await
            .unwrap()
            .success
    );
    assert_eq!(count(&fake, "nokey WHERE x = 'r'"), 1);
    // A duplicate row goes alone (SQLite deletes one row of the match).
    let duplicate = ApplyRequest {
        table: TableRef::new(Some("main"), "nokey"),
        changes: vec![StagedChange::Delete {
            row_id: "k1".into(),
            key: RowKey::AllColumns(BTreeMap::from([("x".into(), text("p"))])),
        }],
        returning: false,
    };
    let result = apply_changes_on(&state, conn.id, duplicate).await.unwrap();
    assert!(result.success, "{:?}", result.error);
    assert_eq!(count(&fake, "nokey WHERE x = 'p'"), 1);
    // A row someone else deleted fails the guard, and the batch with it.
    let gone = ApplyRequest {
        table: t(),
        changes: vec![
            StagedChange::Delete {
                row_id: "r4".into(),
                key: pk(2),
            },
            StagedChange::Update {
                row_id: "r5".into(),
                key: pk(99),
                changes: BTreeMap::from([("v".into(), text("y"))]),
                exprs: None,
            },
        ],
        returning: false,
    };
    let result = apply_changes_on(&state, conn.id, gone).await.unwrap();
    assert!(!result.success);
    let error = result.error.unwrap();
    assert!(
        error
            .message
            .starts_with("expected to affect 1 row(s) but matched 0"),
        "{error:?}"
    );
    assert_eq!(count(&fake, "t WHERE id = 2"), 1);
    let e = s
        .transaction(vec![
            TxStatement::new("DELETE FROM nokey", vec![]).expecting(1),
        ])
        .await
        .unwrap_err();
    assert!(e.to_string().contains("matched 2"), "{e}");
    assert_eq!(count(&fake, "nokey"), 2);

    assert!(
        table_ddl_on(&conn, &t())
            .await
            .unwrap()
            .contains("CREATE TABLE t (")
    );
    let details = session_details_of(s.as_ref()).await.unwrap();
    assert!(!details.tls);
    assert_eq!(details.encoding.as_deref(), Some("UTF-8"));
}

#[tokio::test]
async fn the_editor_streams_caps_and_keeps_its_transaction() {
    let fake = seeded();
    let (_server, _, s) = connected(&fake, json!({})).await;
    let results = run(
        s.as_ref(),
        "BEGIN; INSERT INTO t (v) VALUES ('x'); SELECT v FROM t ORDER BY id",
        3,
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(results[1].kind, StatementKind::Affected);
    assert_eq!(results[1].affected, Some(1));
    assert_eq!(results[1].last_insert_id.as_deref(), Some("6"));
    assert_eq!(results[2].kind, StatementKind::Rows);
    assert_eq!((results[2].row_count, results[2].truncated), (3, true));
    assert_eq!(s.in_transaction(), Some(true));
    // Another stream does not see the row yet; the capped read kept the stream and its
    // transaction.
    assert_eq!(count(&fake, "t"), 5);
    let results = run(s.as_ref(), "COMMIT", 3).await;
    assert!(results[0].error.is_none(), "{results:?}");
    assert_eq!(s.in_transaction(), Some(false));
    assert_eq!(count(&fake, "t"), 6);

    // Outside a transaction a capped read lets the stream go, and nothing counts as lost.
    let results = run(s.as_ref(), "SELECT * FROM t; SELECT 1", 2).await;
    assert!(results[0].truncated);
    assert!(results[1].notices.is_empty(), "{results:?}");
    let last = fake.request_bodies().pop().unwrap();
    assert_eq!(last["baton"], json!(null));

    // A failing statement keeps the stream; DDL reports no stale count.
    let results = run(s.as_ref(), "SELECT * FROM nope", 10).await;
    assert!(results[0].error.is_some());
    let results = run(s.as_ref(), "CREATE TABLE more (a)", 10).await;
    assert_eq!(
        (results[0].kind, results[0].affected),
        (StatementKind::Empty, None)
    );

    let plan = explain_inner(
        s.as_ref(),
        "SELECT * FROM t WHERE id = 1",
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert!(!plan.rows.is_empty());

    // The worker streams its rows through a cursor too.
    let mut sink = CollectSink::default();
    let n = s
        .stream_query(
            "SELECT id FROM t WHERE id > ?1",
            &[Value::Int(1)],
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(n, 5);
    assert_eq!(sink.sets[0].rows.len(), 5);
}

#[tokio::test]
async fn hrana_2_servers_run_the_editor_in_pipelines() {
    let fake = Arc::new(FakeHrana::v2_only());
    fake.seed("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t (v) VALUES ('a');");
    let (_server, _, s) = connected(&fake, json!({})).await;
    let results = run(
        s.as_ref(),
        "BEGIN; INSERT INTO t (v) VALUES ('b'), ('c'); SELECT v FROM t",
        2,
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(results[1].affected, Some(2));
    assert!(results[2].truncated);
    assert_eq!(s.in_transaction(), Some(true));
    run(s.as_ref(), "ROLLBACK", 10).await;
    assert_eq!(s.in_transaction(), Some(false));
    assert_eq!(count(&fake, "t"), 1);
    let mut sink = Sink::default();
    s.stream_query("SELECT v FROM t", &[], &mut sink, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(sink.rows, [[text("a")]]);
}

#[tokio::test]
async fn a_cancel_drops_the_request_and_the_time_out_reports_it() {
    let fake = seeded();
    let (_server, _, s) = connected(&fake, json!({})).await;
    run(s.as_ref(), "BEGIN", 10).await;
    fake.delay_replies(Some(Duration::from_secs(30)));
    let limited = query_limited(
        s.as_ref(),
        "SELECT 1",
        &[],
        Some(Duration::from_millis(200)),
    )
    .await;
    assert!(
        matches!(limited, Err(AppError::QueryTimeout { .. })),
        "{limited:?}"
    );

    let slot = CancelSlot::default();
    let mut sink = Sink::default();
    let statement = s.run_statement(
        "INSERT INTO t (v) VALUES ('late')",
        10,
        &mut sink,
        slot.clone(),
    );
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let handle = request_cancel(&slot).expect("the statement is running");
        s.cancel(handle).await
    };
    let (result, cancelled) = tokio::join!(statement, cancel);
    cancelled.unwrap();
    assert!(matches!(result, Err(AppError::Cancelled)), "{result:?}");
    fake.delay_replies(None);
    // The stream went with the dropped request, and its transaction with it: the next statement
    // runs on a new stream and says so.
    let results = run(s.as_ref(), "SELECT 1", 10).await;
    assert!(results[0].error.is_none(), "{results:?}");
    assert_eq!(results[0].notices, [STREAM_LOST_HINT]);
    assert_eq!(s.in_transaction(), Some(false));
}

/// The writes a read-only session refuses before sending them, on every path, and the reads it
/// still runs.
async fn refuses_every_write(fake: &FakeHrana, server: &FakeServer, s: &dyn DbSession) {
    let sent = server.requests().len();
    let refused = |r: Result<(), AppError>| match r {
        Err(AppError::InvalidInput(m)) => assert_eq!(m, READ_ONLY_MESSAGE),
        other => panic!("{other:?}"),
    };
    let insert = "INSERT INTO t (v) VALUES ('w')";
    refused(s.execute(insert, &[]).await.map(|_| ()));
    refused(
        s.execute_cancellable(insert, &[], CancelSlot::default())
            .await
            .map(|_| ()),
    );
    refused(s.query("DELETE FROM t RETURNING id", &[]).await.map(|_| ()));
    refused(
        s.query_cancellable("UPDATE t SET v = 'q'", &[], CancelSlot::default())
            .await
            .map(|_| ()),
    );
    let e = s
        .transaction(vec![
            TxStatement::new("SELECT 1", vec![]),
            TxStatement::new("DELETE FROM t", vec![]),
        ])
        .await
        .unwrap_err();
    assert_eq!(e.to_string(), READ_ONLY_MESSAGE);
    let mut sink = Sink::default();
    // `query_only` stops no PRAGMA: SQLite applies one while preparing it, under EXPLAIN too.
    for sql in [
        insert,
        "PRAGMA query_only = 0",
        "PRAGMA soft_heap_limit = 7340033",
        "EXPLAIN QUERY PLAN PRAGMA hard_heap_limit = 7340033",
        "SELECT $a(--) ; DELETE FROM t",
        "nonsense",
    ] {
        refused(
            s.run_statement(sql, 10, &mut sink, CancelSlot::default())
                .await
                .map(|_| ()),
        );
    }
    refused(
        s.stream_query(
            "INSERT INTO t (v) VALUES ('w') RETURNING id",
            &[],
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .map(|_| ()),
    );
    // The Explain button and MCP's `explain_query`: the commands layer lets a plan through.
    refused(
        explain_inner(
            s,
            "PRAGMA soft_heap_limit = 7340033",
            ExplainMode::Plan,
            &CancelSlot::default(),
        )
        .await
        .map(|_| ()),
    );
    assert_eq!(server.requests().len(), sent, "nothing reached the server");
    // Reads still go out, and nothing was written.
    assert_eq!(s.query("SELECT v FROM t", &[]).await.unwrap().rows.len(), 5);
    let plan = explain_inner(
        s,
        "SELECT * FROM t WHERE id = 1",
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert!(!plan.rows.is_empty());
    let mut sink = Sink::default();
    s.run_statement("SELECT v FROM t", 10, &mut sink, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(sink.rows.len(), 5);
    let n = s
        .stream_query(
            "SELECT id FROM t",
            &[],
            &mut Sink::default(),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(n, 5);
    assert_eq!(count(fake, "t"), 5);
}

#[tokio::test]
async fn read_only_sessions_refuse_writes_on_every_path() {
    let fake = seeded();
    let (server, _, s) = connected(&fake, json!({"options": {"readOnly": true}})).await;
    // This server takes `query_only`.
    assert_eq!(s.capabilities(), session::capabilities(true));
    assert_eq!(s.capabilities().safety.read_only, ReadOnlyMode::PerRequest);
    refuses_every_write(&fake, &server, s.as_ref()).await;
    // Every batch and cursor started with the pragma, so the server refuses what the session
    // might let through.
    for req in server.requests().iter().filter(|r| r.method == "POST") {
        let body = req.json();
        let batch = match body["requests"].as_array() {
            Some(requests) => requests[0]["batch"].clone(),
            None => body["batch"].clone(),
        };
        if batch.is_null() {
            continue; // a close
        }
        assert_eq!(
            batch["steps"][0]["stmt"]["sql"], "PRAGMA query_only = 1",
            "{body}"
        );
    }
}

#[tokio::test]
async fn on_sqld_the_classifier_alone_keeps_a_session_read_only() {
    // `sqld` (and Turso) refuses `PRAGMA query_only = 1` as an unsupported statement.
    let fake = Arc::new(FakeHrana::sqld());
    fake.seed(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);
         INSERT INTO t (v) VALUES ('a'), ('b'), ('c'), ('d'), ('e');",
    );
    let (server, _, s) = connected(&fake, json!({"options": {"readOnly": true}})).await;
    assert_eq!(
        s.capabilities().safety.read_only,
        ReadOnlyMode::ClassifierOnly
    );
    // Asked once at connect, never again.
    let sent_query_only = |server: &FakeServer| {
        server
            .requests()
            .iter()
            .filter(|r| r.text().contains("query_only"))
            .count()
    };
    assert_eq!(sent_query_only(&server), 1);
    refuses_every_write(&fake, &server, s.as_ref()).await;
    assert_eq!(sent_query_only(&server), 1);

    // The fake refuses as `sqld` does: the whole batch, before anything of it runs.
    let (_w, _, writable) = connected(&fake, json!({})).await;
    let e = writable
        .execute("PRAGMA soft_heap_limit = 7340033", &[])
        .await
        .unwrap_err();
    assert!(e.to_string().contains("unsupported statement"), "{e}");
}

#[tokio::test]
async fn an_idle_stream_the_server_forgot_is_replaced_unless_it_held_a_transaction() {
    let fake = seeded();
    let (_server, _, s) = connected(&fake, json!({})).await;
    run(s.as_ref(), "SELECT 1", 10).await;
    fake.expire_streams();
    let results = run(s.as_ref(), "SELECT 2", 10).await;
    assert!(results[0].error.is_none(), "{results:?}");
    assert!(results[0].notices.is_empty());

    run(s.as_ref(), "BEGIN; DELETE FROM t WHERE id = 1", 10).await;
    assert_eq!(s.in_transaction(), Some(true));
    fake.expire_streams();
    let results = run(s.as_ref(), "DELETE FROM t WHERE id = 2", 10).await;
    let error = results[0].error.as_ref().expect("the transaction is gone");
    assert_eq!(error.hint.as_deref(), Some(STREAM_LOST_HINT));
    assert_eq!(s.in_transaction(), Some(false));
    assert_eq!(count(&fake, "t"), 5);
}

#[tokio::test]
async fn startup_commands_and_foreign_keys_run_on_every_new_stream() {
    let fake = seeded();
    let (_server, _, s) = connected(
        &fake,
        json!({"options": {"sqliteForeignKeys": true,
                           "startupSql": "CREATE TEMP TABLE boot (x); INSERT INTO boot VALUES (1);"}}),
    )
    .await;
    let rs = s
        .query("SELECT x FROM boot, pragma_foreign_keys", &[])
        .await
        .unwrap();
    assert_eq!(rs.rows, [[Value::Int(1)]]);
    let mut sink = Sink::default();
    s.run_statement(
        "SELECT foreign_keys FROM pragma_foreign_keys",
        10,
        &mut sink,
        CancelSlot::default(),
    )
    .await
    .unwrap();
    assert_eq!(sink.rows, [[Value::Int(1)]]);
}

#[tokio::test]
async fn the_trace_logs_what_the_app_sent() {
    let fake = seeded();
    let (_server, _, s) = connected(&fake, json!({})).await;
    s.drain_trace();
    s.table_meta(&t()).await.unwrap();
    s.query("SELECT v FROM t WHERE id = ?1", &[Value::Int(1)])
        .await
        .unwrap();
    s.transaction(vec![
        TxStatement::new("UPDATE t SET v = 'y' WHERE id = 1", vec![]).expecting(1),
    ])
    .await
    .unwrap();
    let mut sink = Sink::default();
    s.run_statement("SELECT 1", 10, &mut sink, CancelSlot::default())
        .await
        .unwrap();
    let logged = s.drain_trace();
    assert_eq!(logged.len(), 3, "{logged:?}");
    assert_eq!(logged[0].sql.matches(";\n").count(), 4);
    assert_eq!(logged[1].sql, "SELECT v FROM t WHERE id = 1");
    assert_eq!(logged[2].affected, Some(1));
}

#[test]
fn profiles_are_checked_and_name_their_server() {
    let p = |fields: serde_json::Value| profile(Driver::Libsql, fields);
    assert_eq!(
        entry::validate(&p(json!({}))).unwrap_err().to_string(),
        "Host is required"
    );
    entry::validate(&p(json!({"host": "db-org.turso.io"}))).unwrap();
    let url = |u: &str| p(json!({"options": {"driverParams": {"url": u}}}));
    entry::validate(&url("libsql://db-org.turso.io")).unwrap();
    for secret in [
        "libsql://db.turso.io?authToken=eyJ",
        "https://u:pw@sqld.example.com",
        "https://:pw@h/db",
        "https://h/db?authToken=SECRET&authToken=",
        "https://h/db?authToken=",
        "https://h/db?AuthToken=S",
    ] {
        let e = entry::validate(&url(secret)).unwrap_err().to_string();
        assert!(e.contains("Auth Token"), "{e}");
    }
    let mut tunnelled = url("https://sqld.example.com/db");
    tunnelled.ssh = serde_json::from_value(json!({
        "host": "bastion", "port": 22, "user": "me", "auth": "agent", "keyPath": null,
        "localPort": null, "keepaliveSecs": null
    }))
    .unwrap();
    assert!(entry::validate(&tunnelled).is_err());
    let basic = p(json!({"host": "h", "options": {"driverParams": {"basicAuth": true}}}));
    assert!(entry::validate(&basic).is_err());

    // Plain HTTP is `sqld`'s 8080; HTTPS leaves its 443 out.
    assert_eq!(
        entry::base_of(&p(json!({"host": "127.0.0.1"}))).unwrap(),
        ("http://127.0.0.1:8080".to_string(), false)
    );
    assert_eq!(
        entry::base_of(&p(json!({"host": "db-org.turso.io", "ssl": "verify-full"}))).unwrap(),
        ("https://db-org.turso.io".to_string(), true)
    );
    assert_eq!(
        entry::base_of(&url("http://127.0.0.1:9000/db/")).unwrap(),
        ("http://127.0.0.1:9000/db".to_string(), false)
    );
    assert_eq!(
        LibsqlParams::from_driver_params(None).unwrap(),
        LibsqlParams::default()
    );
}

#[tokio::test]
async fn the_form_has_no_calls_of_its_own() {
    let p = profile(Driver::Libsql, json!({}));
    let e = entry::util(
        UtilInput {
            profile: &p,
            secret: None,
        },
        "anything",
        json!(null),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code(), "UNSUPPORTED");
    assert_eq!(entry::util_kind("anything"), CallKind::Write);
}

#[tokio::test]
async fn https_checks_the_certificate_unless_told_not_to() {
    use crate::testkit::tls::TlsServer;
    let server = TlsServer::start();
    let port = server.addr().port();
    let open_with = |fields: serde_json::Value| async move {
        let p = profile(Driver::Libsql, fields);
        registry::open(ConnectTarget {
            profile: &p,
            password: Some("secret-token"),
            host_override: Some(("127.0.0.1".into(), port)),
        })
        .await
    };
    let host = |ssl: &str| json!({"host": "db.example.test", "port": 443, "ssl": ssl});
    // A new profile's `prefer`, `verify-full`, and a URL whatever the SSL mode: the untrusted
    // certificate is refused before the token is sent.
    let url =
        json!({"ssl": "disable", "options": {"driverParams": {"url": "libsql://db.example.test"}}});
    for fields in [host("prefer"), host("verify-full"), url] {
        let e = open_with(fields.clone()).await.err().unwrap();
        assert!(matches!(e, AppError::Connect(_)), "{fields}: {e:?}");
    }
    assert!(server.requests().is_empty());
    assert_eq!(
        entry::certificate_check(crate::types::profile::SslMode::VerifyCa),
        crate::types::profile::SslMode::VerifyCa
    );
    // Plain "HTTPS" (`require`) is the choice of a server whose certificate cannot be checked.
    let e = open_with(host("require")).await.err().unwrap();
    assert!(e.to_string().contains("unexpected"), "{e}");
    let seen = server.requests();
    assert_eq!(seen[0].header("authorization"), Some("Bearer secret-token"));
}

#[tokio::test]
async fn read_only_startup_commands_cannot_write() {
    let fake = seeded();
    let server = serve(fake.clone() as Arc<dyn Answers>, Some("Bearer tok")).await;
    // Refused before anything is sent, also one that only turns `query_only` off.
    for startup in [
        "PRAGMA query_only = 0; INSERT INTO t (v) VALUES ('boot')",
        "PRAGMA query_only = 0",
        "SELECT 1; CREATE TEMP TABLE boot (x)",
    ] {
        let p = libsql_profile(
            &server,
            json!({"options": {"readOnly": true, "startupSql": startup}}),
        );
        let e = open(&p, Some(TOKEN)).await.err().unwrap();
        assert!(
            e.to_string().starts_with(READ_ONLY_MESSAGE),
            "{startup}: {e}"
        );
        assert!(e.to_string().contains("startup"), "{e}");
    }
    assert!(server.requests().is_empty());
    // Reads connect, and run after `query_only` on every new stream.
    let reading = libsql_profile(
        &server,
        json!({"options": {"readOnly": true, "startupSql": "SELECT 1"}}),
    );
    let s = open(&reading, Some(TOKEN)).await.unwrap().sql().unwrap();
    assert!(s.execute("DELETE FROM t", &[]).await.is_err());
    let body = fake.request_bodies().pop().unwrap();
    let steps = &body["requests"][0]["batch"]["steps"];
    assert_eq!(steps[0]["stmt"]["sql"], "PRAGMA query_only = 1");
    assert_eq!(steps[1]["stmt"]["sql"], "PRAGMA foreign_keys = OFF");
    assert_eq!(steps[2]["stmt"]["sql"], "SELECT 1");
    assert_eq!(steps[3]["stmt"]["sql"], "PRAGMA query_only = 1");
    assert_eq!(count(&fake, "t"), 5);
}

#[tokio::test]
async fn hrana_2_savepoints_count_as_a_transaction() {
    let fake = Arc::new(FakeHrana::v2_only());
    fake.seed("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t (v) VALUES ('a');");
    let (_server, _, s) = connected(&fake, json!({})).await;
    run(s.as_ref(), "BEGIN; COMMIT", 10).await;
    assert_eq!(s.in_transaction(), Some(false));
    // SQLite's SAVEPOINT outside a transaction opens one.
    let results = run(
        s.as_ref(),
        "SAVEPOINT a; INSERT INTO t (v) VALUES ('s')",
        10,
    )
    .await;
    assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
    assert_eq!(s.in_transaction(), Some(true));
    // The server forgets the stream: the insert is gone, and the next statement says so rather
    // than run quietly on a new stream.
    fake.expire_streams();
    let results = run(s.as_ref(), "SELECT count(*) FROM t", 10).await;
    let error = results[0].error.as_ref().expect("the transaction is gone");
    assert_eq!(error.hint.as_deref(), Some(STREAM_LOST_HINT));
    assert_eq!(s.in_transaction(), Some(false));
    assert_eq!(count(&fake, "t"), 1);
    // ROLLBACK TO keeps the transaction; after RELEASE whether one is open is unknown, so a
    // forgotten stream is reported too.
    run(s.as_ref(), "SAVEPOINT b; ROLLBACK TO b", 10).await;
    assert_eq!(s.in_transaction(), Some(true));
    run(s.as_ref(), "RELEASE b", 10).await;
    assert_eq!(s.in_transaction(), None);
    fake.expire_streams();
    let results = run(s.as_ref(), "SELECT 1", 10).await;
    let error = results[0].error.as_ref().expect("the stream is gone");
    assert_eq!(error.hint.as_deref(), Some(STREAM_LOST_HINT));
    assert_eq!(s.in_transaction(), Some(false));
}

#[tokio::test]
async fn hostile_names_and_values_are_quoted_or_bound() {
    let fake = Arc::new(FakeHrana::new());
    fake.seed(
        r#"CREATE TABLE "we""ird'; --" (id INTEGER PRIMARY KEY, "c""ol" TEXT, "x'y" INTEGER);
           INSERT INTO "we""ird'; --" ("c""ol", "x'y") VALUES ('a', 1), ('b', 2), ('c', 3);
           CREATE INDEX "i""dx'" ON "we""ird'; --" ("c""ol");
           CREATE VIEW "v'iew" AS SELECT 1 AS one;
           CREATE TABLE t (id INTEGER PRIMARY KEY);"#,
    );
    let (_server, p, s) = connected(&fake, json!({})).await;
    let name = "we\"ird'; --";
    let tables: Vec<String> = s
        .list_tables("main")
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert!(tables.contains(&name.to_string()), "{tables:?}");
    let table = TableRef::new(Some("main"), name);
    let meta = s.table_meta(&table).await.unwrap();
    assert_eq!(meta.columns.len(), 3);
    assert!(
        meta.indexes.iter().any(|i| i.name == "i\"dx'"),
        "{:?}",
        meta.indexes
    );
    let ddl = s.table_ddl(&table).await.unwrap().unwrap();
    assert!(ddl.contains(r#"CREATE INDEX "i""dx'""#), "{ddl}");
    assert!(
        s.view_definition(&TableRef::new(None, "v'iew"))
            .await
            .unwrap()
            .is_some()
    );
    let (state, conn) = app_connection(p, Session::Sql(s.clone())).await;
    let page = browse_rows(
        &conn,
        &BrowseRequest {
            table: table.clone(),
            columns: None,
            filter: None,
            sort: Vec::new(),
            limit: 2,
            offset: 1,
            cursor: None,
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.rows[0][1], text("b"));
    let counted = count_rows(&conn, &table, None, true, DEFAULT_ESTIMATE_ABOVE, None)
        .await
        .unwrap();
    assert_eq!(counted.count, 3);
    let cell = fetch_cell_on(&conn, &table, &pk(1), "c\"ol", None)
        .await
        .unwrap();
    assert_eq!(cell, text("a"));
    let result = apply_changes_on(
        &state,
        conn.id,
        ApplyRequest {
            table: table.clone(),
            changes: vec![
                StagedChange::Update {
                    row_id: "r1".into(),
                    key: pk(1),
                    changes: BTreeMap::from([("c\"ol".into(), text("z'; DROP TABLE t; --"))]),
                    exprs: None,
                },
                StagedChange::Insert {
                    row_id: "r2".into(),
                    values: BTreeMap::from([("x'y".into(), Value::Int(9))]),
                    exprs: None,
                },
                StagedChange::Delete {
                    row_id: "r3".into(),
                    key: pk(3),
                },
            ],
            returning: true,
        },
    )
    .await
    .unwrap();
    assert!(result.success, "{:?}", result.error);
    let quoted = r#""we""ird'; --""#;
    let edited = format!(r#"{quoted} WHERE "c""ol" = 'z''; DROP TABLE t; --'"#);
    assert_eq!(count(&fake, &edited), 1);
    assert_eq!(count(&fake, &format!(r#"{quoted} WHERE "x'y" = 9"#)), 1);
    assert_eq!(count(&fake, quoted), 3);
    assert_eq!(count(&fake, "t"), 0, "t is still there");
}

#[tokio::test]
async fn a_command_token_runs_again_after_the_server_refuses_it() {
    let fake = seeded();
    // The server takes only what the command prints now: what it printed at connect expired.
    let server = serve(fake.clone() as Arc<dyn Answers>, Some("Bearer fresh-token")).await;
    let p = libsql_profile(
        &server,
        json!({"options": {"passwordMode": "command", "passwordCommand": "echo fresh-token"}}),
    );
    let s = open(&p, Some("stale-token")).await.unwrap().sql().unwrap();
    s.query("SELECT 1", &[]).await.unwrap();
    let sent: Vec<Option<String>> = server
        .requests()
        .iter()
        .map(|r| r.header("authorization").map(str::to_string))
        .collect();
    assert_eq!(sent[0].as_deref(), Some("Bearer stale-token"));
    assert!(
        sent[1..]
            .iter()
            .all(|a| a.as_deref() == Some("Bearer fresh-token")),
        "{sent:?}"
    );
}
