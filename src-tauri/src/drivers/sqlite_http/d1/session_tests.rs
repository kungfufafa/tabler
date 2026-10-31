//! The D1 driver end to end: `entry::open` through the registry, over the shared HTTP client,
//! against the D1 fake served on a socket, and the app's command bodies on the session.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::entry::{self, D1Params, LIST_DATABASES};
use super::session;
use super::testing::{ACCOUNT, FakeD1, MAIN_DB};
use crate::commands::browse::{DEFAULT_ESTIMATE_ABOVE, browse_rows, count_rows, fetch_cell_on};
use crate::commands::connections::switch_database_on;
use crate::commands::ddl::table_ddl_on;
use crate::commands::lock::session_details_of;
use crate::commands::mutate::apply_changes_on;
use crate::commands::query::{explain_inner, query_limited, run_sql_inner};
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::native::CallKind;
use crate::drivers::registry::{self, UtilInput};
use crate::drivers::sqlite_http::session::hides_code_in_a_variable;
use crate::drivers::sqlite_http::testing::{Answers, Sink, app_connection, profile, serve};
use crate::drivers::{CancelSlot, ConnectTarget, DbSession, Session, TxStatement, request_cancel};
use crate::error::AppError;
use crate::secrets::SecretKind;
use crate::testkit::FakeServer;
use crate::types::browse::{
    BrowseRequest, Conjunction, FilterGroup, FilterNode, RowIdentity, RowKey,
};
use crate::types::caps::{CountMode, Feature};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::{ApplyRequest, ServerExpr, StagedChange};
use crate::types::profile::{ConnectionProfile, Driver};
use crate::types::query::{ExplainMode, RunOptions, StatementKind, StatementResult};
use crate::types::schema::TableRef;
use crate::types::value::Value;

const TOKEN: &str = "cf-token";

fn seeded() -> Arc<FakeD1> {
    let fake = Arc::new(FakeD1::new());
    fake.seed(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT UNIQUE, n INTEGER, price REAL,
                         at DATETIME, doc JSON, data BLOB);
         INSERT INTO t (id, v, n, price, at, doc, data) VALUES
           (1, 'a', 9007199254740993, 2.0, '2026-10-30 12:00:00', '{\"k\":1}', x'0001ff'),
           (2, 'b', 5, 2.5, NULL, NULL, NULL), (3, 'c', NULL, NULL, NULL, NULL, NULL);
         CREATE VIEW tv AS SELECT id, v FROM t;
         CREATE TABLE d1_migrations (id INTEGER PRIMARY KEY, name TEXT);",
    );
    fake
}

fn count(fake: &FakeD1, sql: &str) -> i64 {
    fake.with_db(MAIN_DB, |c| {
        c.query_row(&format!("SELECT count(*) FROM {sql}"), [], |r| r.get(0))
            .unwrap()
    })
}

/// A profile of the fake at `server`'s account and database `database`.
fn d1_profile(server: &FakeServer, database: &str, read_only: bool) -> ConnectionProfile {
    profile(
        Driver::D1,
        json!({"database": database, "options": {"readOnly": read_only, "driverParams": {
            "accountId": ACCOUNT, "apiBase": server.url("/client/v4")}}}),
    )
}

async fn open(profile: &ConnectionProfile, password: Option<&str>) -> Result<Session, AppError> {
    registry::open(ConnectTarget {
        profile,
        password,
        host_override: None,
    })
    .await
}

async fn connected(
    fake: &Arc<FakeD1>,
    read_only: bool,
) -> (FakeServer, ConnectionProfile, Arc<dyn DbSession>) {
    let server = serve(
        fake.clone() as Arc<dyn Answers>,
        Some(&format!("Bearer {TOKEN}")),
    )
    .await;
    let p = d1_profile(&server, "main-db", read_only);
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
        stop_on_error: false,
        ..RunOptions::default()
    };
    let mut emit = |_| {};
    run_sql_inner(session, sql, &options, &CancelSlot::default(), &mut emit).await
}

#[tokio::test]
async fn connects_by_name_or_uuid_and_reads_the_catalog() {
    let fake = seeded();
    fake.add_database("other");
    let (server, _, s) = connected(&fake, false).await;
    let info = s.server_info().await.unwrap();
    assert!(
        info.version.starts_with("Cloudflare D1 (SQLite 3."),
        "{}",
        info.version
    );
    assert_eq!(info.current_database.as_deref(), Some("main-db"));
    let dbs = s.list_databases().await.unwrap();
    let listed: Vec<(&str, bool)> = dbs
        .iter()
        .map(|d| (d.name.as_str(), d.is_current))
        .collect();
    assert_eq!(listed, [("main-db", true), ("other", false)]);
    let tables: Vec<String> = s
        .list_tables("main")
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(tables, ["d1_migrations", "t", "tv"]);
    let meta = s.table_meta(&t()).await.unwrap();
    assert_eq!(meta.primary_key, ["id"]);
    assert_eq!(meta.columns.len(), 7);
    assert!(
        s.view_definition(&TableRef::new(None, "tv"))
            .await
            .unwrap()
            .is_some()
    );
    let completion = s.completion_metadata().await.unwrap();
    assert!(!completion.schemas["main"].contains_key("_cf_KV"));
    assert!(
        s.table_ddl(&t())
            .await
            .unwrap()
            .unwrap()
            .starts_with("CREATE TABLE t (")
    );
    // Nothing to ping: no connection, and every API call counts against the rate limit.
    let sent = server.requests().len();
    s.ping().await.unwrap();
    assert_eq!(server.requests().len(), sent);
    assert!(
        server
            .requests()
            .iter()
            .all(|r| r.header("authorization") == Some("Bearer cf-token"))
    );

    let caps = s.capabilities();
    assert_eq!(caps, session::capabilities());
    assert!(caps.has(Feature::EditRows) && caps.has(Feature::SwitchDatabase));
    assert!(!caps.has(Feature::EditWithoutKey) && !caps.has(Feature::Returning));
    assert!(caps.namespaces.databases);
    assert_eq!(caps.data.max_params, 100);
    assert_eq!(caps.data.count, CountMode::Costly);
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    assert_eq!(s.in_transaction(), Some(false));

    // By UUID too; an unknown name, a malformed account or a wrong token fail the connect.
    let by_id = d1_profile(&server, MAIN_DB, false);
    open(&by_id, Some(TOKEN)).await.unwrap();
    let unknown = d1_profile(&server, "nope", false);
    assert!(matches!(
        open(&unknown, Some(TOKEN)).await,
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        open(&by_id, Some("wrong")).await,
        Err(AppError::Auth(_))
    ));
    let mut bad_account = by_id.clone();
    bad_account.options.driver_params = Some(crate::types::profile::DriverParams(
        json!({"accountId": "023e105f", "apiBase": server.url("/client/v4")})
            .as_object()
            .unwrap()
            .clone(),
    ));
    assert!(matches!(
        open(&bad_account, Some(TOKEN)).await,
        Err(AppError::InvalidInput(_))
    ));

    s.close().await;
    assert!(matches!(
        s.list_tables("main").await,
        Err(AppError::NotConnected)
    ));
}

#[tokio::test]
async fn the_grid_reads_counts_fetches_and_commits_through_the_commands() {
    let fake = seeded();
    let (_server, p, s) = connected(&fake, false).await;
    let (state, conn) = app_connection(p, Session::Sql(s.clone())).await;
    let page = |offset: u64| BrowseRequest {
        table: t(),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit: 2,
        offset,
        cursor: None,
    };
    let first = browse_rows(&conn, &page(0), None).await.unwrap();
    assert_eq!(first.identity, RowIdentity::Pk);
    let row = &first.rows[0];
    // Read as text by the narrowed select, so exact.
    assert_eq!(row[2], Value::BigInt("9007199254740993".into()));
    // D1's JavaScript sends 2.0 as 2: a whole REAL reads as an integer.
    assert_eq!(row[3], Value::Int(2));
    assert_eq!(first.rows[1][3], Value::Float(2.5));
    assert_eq!(row[4], text("2026-10-30 12:00:00"));
    assert!(matches!(row[6], Value::Bytes(_)));
    let second = browse_rows(&conn, &page(2), None).await.unwrap();
    assert_eq!(second.rows.len(), 1);
    assert_eq!(second.rows[0][1], text("c"));

    let counted = count_rows(&conn, &t(), None, true, DEFAULT_ESTIMATE_ABOVE, None)
        .await
        .unwrap();
    assert_eq!((counted.count, counted.exact), (3, true));
    let blob = fetch_cell_on(&conn, &t(), &pk(1), "data", None)
        .await
        .unwrap();
    assert_eq!(blob, Value::from_bytes(&[0, 1, 255]));

    // One failing change (a duplicate key): the batch is rolled back whole.
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
        returning: true,
    };
    let result = apply_changes_on(&state, conn.id, failing).await.unwrap();
    assert!(!result.success);
    assert_eq!(count(&fake, "t WHERE v = 'z'"), 0);

    // A blob (written into the statement: D1 binds none), a server expression, no rows back.
    let edits = ApplyRequest {
        table: t(),
        changes: vec![
            StagedChange::Update {
                row_id: "r3".into(),
                key: pk(2),
                changes: BTreeMap::from([("data".into(), Value::from_bytes(&[9, 8]))]),
                exprs: None,
            },
            StagedChange::Insert {
                row_id: "r4".into(),
                values: BTreeMap::from([("v".into(), text("d"))]),
                exprs: Some(BTreeMap::from([(
                    "at".into(),
                    ServerExpr::CurrentTimestamp,
                )])),
            },
        ],
        returning: true,
    };
    let result = apply_changes_on(&state, conn.id, edits).await.unwrap();
    assert!(result.success, "{:?}", result.error);
    assert!(result.results.iter().all(|r| r.returned.is_none()));
    assert_eq!(count(&fake, "t WHERE id = 2 AND data = x'0908'"), 1);
    assert_eq!(count(&fake, "t WHERE v = 'd' AND at IS NOT NULL"), 1);

    // Rows are addressed by primary key only.
    let keyless = ApplyRequest {
        table: t(),
        changes: vec![StagedChange::Delete {
            row_id: "r5".into(),
            key: RowKey::AllColumns(BTreeMap::from([("v".into(), text("c"))])),
        }],
        returning: false,
    };
    assert!(matches!(
        apply_changes_on(&state, conn.id, keyless).await,
        Err(AppError::Unsupported(_))
    ));

    assert!(
        table_ddl_on(&conn, &t())
            .await
            .unwrap()
            .contains("CREATE TABLE t (")
    );
    let details = session_details_of(s.as_ref()).await.unwrap();
    assert!(!details.tls, "the fake API is plain HTTP");
}

#[tokio::test]
async fn the_editor_runs_one_request_per_statement() {
    let fake = seeded();
    let (_server, _, s) = connected(&fake, false).await;
    let results = run(
        s.as_ref(),
        "SELECT * FROM t; INSERT INTO t (v) VALUES ('x'); BEGIN; CREATE TABLE more (a)",
        2,
    )
    .await;
    assert_eq!(results[0].kind, StatementKind::Rows);
    assert_eq!((results[0].row_count, results[0].truncated), (2, true));
    assert_eq!(results[1].kind, StatementKind::Affected);
    assert_eq!(results[1].affected, Some(1));
    assert_eq!(results[1].last_insert_id.as_deref(), Some("4"));
    // D1 has no transactions of SQL.
    let refused = results[2].error.as_ref().expect("BEGIN is refused");
    assert!(refused.message.contains("transaction"), "{refused:?}");
    assert_eq!(
        (results[3].kind, results[3].affected),
        (StatementKind::Empty, None)
    );
    assert_eq!(s.in_transaction(), Some(false));

    let plan = explain_inner(
        s.as_ref(),
        "SELECT * FROM t WHERE id = 1",
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert!(!plan.rows.is_empty());
    let mut sink = Sink::default();
    let n = s
        .stream_query(
            "SELECT v FROM t WHERE id < ?1",
            &[Value::Int(3)],
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(n, 2);
    assert_eq!(sink.rows, [[text("a")], [text("b")]]);
}

#[tokio::test]
async fn a_cancel_drops_the_request_and_the_time_out_reports_it() {
    let fake = seeded();
    let (_server, _, s) = connected(&fake, false).await;
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
    let statement = s.run_statement("SELECT * FROM t", 10, &mut sink, slot.clone());
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let handle = request_cancel(&slot).expect("the statement is running");
        s.cancel(handle).await
    };
    let (result, cancelled) = tokio::join!(statement, cancel);
    cancelled.unwrap();
    assert!(matches!(result, Err(AppError::Cancelled)), "{result:?}");
    fake.delay_replies(None);
    assert!(run(s.as_ref(), "SELECT 1", 10).await[0].error.is_none());
}

#[tokio::test]
async fn read_only_sessions_refuse_writes_before_sending_them() {
    let fake = seeded();
    let (server, _, s) = connected(&fake, true).await;
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
    for sql in [
        insert,
        "PRAGMA defer_foreign_keys = on",
        "VACUUM",
        "nonsense",
    ] {
        refused(
            s.run_statement(sql, 10, &mut sink, CancelSlot::default())
                .await
                .map(|_| ()),
        );
    }
    refused(
        s.stream_query(insert, &[], &mut sink, CancelSlot::default())
            .await
            .map(|_| ()),
    );
    // An explain plan passes the commands layer, but SQLite applies a PRAGMA while preparing it.
    refused(
        explain_inner(
            s.as_ref(),
            "PRAGMA defer_foreign_keys = ON",
            ExplainMode::Plan,
            &CancelSlot::default(),
        )
        .await
        .map(|_| ()),
    );
    assert_eq!(server.requests().len(), sent, "nothing reached the API");
    // Reads go out.
    assert_eq!(s.query("SELECT v FROM t", &[]).await.unwrap().rows.len(), 3);
    s.run_statement("SELECT 1", 10, &mut sink, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(count(&fake, "t"), 3);
}

#[tokio::test]
async fn switching_databases_reconnects_by_name() {
    let fake = seeded();
    let other = fake.add_database("other");
    fake.with_db(&other, |c| {
        c.execute_batch("CREATE TABLE elsewhere (a)").unwrap()
    });
    let (_server, p, s) = connected(&fake, false).await;
    let (state, conn) = app_connection(p, Session::Sql(s)).await;
    state
        .secrets
        .set(conn.profile.id, SecretKind::DbPassword, TOKEN)
        .await
        .unwrap();
    let info = switch_database_on(&state, conn.id, "other".into())
        .await
        .unwrap();
    assert_eq!(info.current_database.as_deref(), Some("other"));
    let switched = conn.session().await.unwrap();
    let tables = switched.list_tables("main").await.unwrap();
    assert_eq!(tables[0].name, "elsewhere");
}

#[tokio::test]
async fn the_picker_lists_the_accounts_databases() {
    let fake = seeded();
    fake.add_database("other");
    let server = serve(fake.clone() as Arc<dyn Answers>, Some("Bearer cf-token")).await;
    let p = d1_profile(&server, "", false);
    let input = || UtilInput {
        profile: &p,
        secret: Some(TOKEN),
    };
    let found = entry::util(input(), LIST_DATABASES, json!(null))
        .await
        .unwrap();
    let names: Vec<&str> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["main-db", "other"]);
    assert_eq!(found[0]["uuid"], MAIN_DB);
    assert_eq!(entry::util_kind(LIST_DATABASES), CallKind::Read);
    let e = entry::util(input(), "dropEverything", json!(null))
        .await
        .unwrap_err();
    assert_eq!(e.code(), "UNSUPPORTED");
    assert_eq!(entry::util_kind("dropEverything"), CallKind::Write);
}

#[test]
fn params_are_checked() {
    let with = |params: serde_json::Value| {
        profile(
            Driver::D1,
            json!({"database": "db", "options": {"driverParams": params}}),
        )
    };
    entry::validate(&with(json!({"accountId": ACCOUNT}))).unwrap();
    for ok in [
        "https://gateway.example.com/client/v4",
        "http://127.0.0.1:8787/client/v4",
        "http://localhost/v4",
        "http://[::1]:9/v4",
    ] {
        entry::validate(&with(json!({"accountId": ACCOUNT, "apiBase": ok}))).unwrap();
    }
    // The token would travel in clear text to another machine.
    let e = entry::validate(&with(
        json!({"accountId": ACCOUNT, "apiBase": "http://d1.example.com/v4"}),
    ))
    .unwrap_err();
    assert!(e.to_string().contains("https://"), "{e}");
    // A credential in the base would be stored as plain text and sent as Basic auth.
    for secret in [
        "https://u:SECRET@gw.example/client/v4",
        "https://SECRET@gw.example/client/v4",
        "https://:SECRET@gw.example/client/v4",
        "https://gw.example/?key=SECRET",
        "https://gw.example/v4?",
        "https://gw.example/v4#SECRET",
        "http://u:SECRET@127.0.0.1:8787/v4",
    ] {
        let e = entry::validate(&with(json!({"accountId": ACCOUNT, "apiBase": secret})))
            .unwrap_err()
            .to_string();
        assert!(e.contains("API Token field"), "{secret}: {e}");
        assert!(!e.contains("SECRET"), "{e}");
    }
    assert!(entry::validate(&with(json!({"accountId": 7}))).is_err());
    let parsed = D1Params::from_driver_params(None).unwrap();
    assert_eq!(
        parsed.api_base().unwrap(),
        "https://api.cloudflare.com/client/v4"
    );
}

/// Texts SQLite reads as a variable (`$a(--)` is one token) where a lexer that sees a comment or
/// a quote in it misses a DELETE: the shared classifier calls them reads (a change request).
const HIDDEN_WRITES: [&str; 11] = [
    "SELECT $a(--) ; DELETE FROM t",
    "SELECT $a(') ; DELETE FROM t ; SELECT $b(')",
    "SELECT $a(/*) ; DELETE FROM t ; SELECT 1 /* */",
    "SELECT :a(--) ; DELETE FROM t",
    "SELECT @a(--) ; DELETE FROM t",
    "SELECT $a::b(--) ; DELETE FROM t",
    "WITH x AS (SELECT $a(--)) DELETE FROM t",
    "WITH x AS (SELECT $a(')) DELETE FROM t WHERE $b(') IS NULL",
    "WITH x AS (SELECT $a(/*)) DELETE FROM t /* */",
    "SELECT $a(\") ; DELETE FROM t ; SELECT $b(\")",
    "SELECT $a([) ; DELETE FROM t ; SELECT ]",
];

#[test]
fn variables_that_hide_code_are_found() {
    for sql in HIDDEN_WRITES {
        assert!(hides_code_in_a_variable(sql), "{sql}");
    }
    for sql in [
        "SELECT * FROM t WHERE id = ?1 AND v = :v AND n = @n AND at = $at",
        "SELECT json_extract(doc, '$.k[0]'), '12:30:00', x$a(1) FROM t",
        "SELECT $a(1), :b::c(x) FROM t",
        "SELECT 'a:b(c)', '$', ':', ''",
    ] {
        assert!(!hides_code_in_a_variable(sql), "{sql}");
    }
    // Inside a string it is still refused: the scan reads the raw text.
    assert!(hides_code_in_a_variable("SELECT ':a(--)'"));
}

#[tokio::test]
async fn variables_that_hide_code_never_write() {
    let fake = seeded();
    // Read-only: refused before anything is sent, on the editor's and MCP's path too.
    let (server, _, ro) = connected(&fake, true).await;
    let sent = server.requests().len();
    let mut sink = Sink::default();
    for sql in HIDDEN_WRITES {
        let r = ro
            .run_statement(sql, 10, &mut sink, CancelSlot::default())
            .await;
        assert!(
            matches!(&r, Err(AppError::InvalidInput(m)) if m == READ_ONLY_MESSAGE),
            "{sql}: {r:?}"
        );
        assert!(
            run(ro.as_ref(), sql, 10)
                .await
                .iter()
                .all(|r| r.error.is_some()),
            "{sql}"
        );
    }
    assert_eq!(server.requests().len(), sent, "nothing reached the API");
    // Writable: D1 refuses a variable it is given no value for (the editor binds none).
    let (_server, _, rw) = connected(&fake, false).await;
    for sql in HIDDEN_WRITES {
        let r = rw
            .run_statement(sql, 10, &mut sink, CancelSlot::default())
            .await;
        assert!(r.is_err(), "{sql}: {r:?}");
    }
    assert_eq!(count(&fake, "t"), 3);
}

#[tokio::test]
async fn a_raw_filter_cannot_add_a_statement() {
    let fake = seeded();
    let (server, p, s) = connected(&fake, false).await;
    let (_state, conn) = app_connection(p, Session::Sql(s.clone())).await;
    let raw = |sql: &str| FilterGroup {
        conjunction: Conjunction::And,
        items: vec![FilterNode::Raw { sql: sql.into() }],
    };
    let sent = server.requests().len();
    let browse = BrowseRequest {
        table: t(),
        columns: None,
        filter: Some(raw("1) ; DELETE FROM t; SELECT * FROM t WHERE (1")),
        sort: Vec::new(),
        limit: 2,
        offset: 0,
        cursor: None,
    };
    let e = browse_rows(&conn, &browse, None).await.unwrap_err();
    assert!(e.to_string().contains("one statement at a time"), "{e}");
    let counted = count_rows(
        &conn,
        &t(),
        Some(&raw(
            "1) ; DELETE FROM t WHERE id = 3; SELECT count(*) FROM t WHERE (1",
        )),
        true,
        DEFAULT_ESTIMATE_ABOVE,
        None,
    )
    .await;
    assert!(counted.is_err(), "{counted:?}");
    // Every path that sends one statement checks it, the staged commit's too.
    let two = "SELECT 1; DELETE FROM t";
    assert!(s.query(two, &[]).await.is_err());
    assert!(s.execute(two, &[]).await.is_err());
    // A writable session too: the DELETE behind a variable's suffix is not sent.
    let hidden = "SELECT $a(--) ; DELETE FROM t";
    let e = s.query(hidden, &[]).await.unwrap_err();
    assert!(e.to_string().contains("one statement at a time"), "{e}");
    assert!(s.execute(hidden, &[]).await.is_err());
    let mut sink = Sink::default();
    assert!(
        s.run_statement(two, 10, &mut sink, CancelSlot::default())
            .await
            .is_err()
    );
    assert!(
        s.stream_query(two, &[], &mut sink, CancelSlot::default())
            .await
            .is_err()
    );
    let e = s
        .transaction(vec![
            TxStatement::new("UPDATE t SET v = 'x' WHERE id = 1", vec![]),
            TxStatement::new(two, vec![]),
        ])
        .await
        .unwrap_err();
    assert!(e.to_string().contains("one statement at a time"), "{e}");
    // Only the grid's catalog reads went out.
    let sent: Vec<String> = server.requests()[sent..]
        .iter()
        .map(|r| r.json().to_string())
        .collect();
    assert!(sent.iter().all(|body| !body.contains("DELETE")), "{sent:?}");
    assert_eq!(count(&fake, "t"), 3);
    // A filter that is one condition still works.
    let ok = BrowseRequest {
        filter: Some(raw("v IN ('a', 'b')")),
        ..browse
    };
    assert_eq!(browse_rows(&conn, &ok, None).await.unwrap().rows.len(), 2);
}

#[tokio::test]
async fn hostile_names_and_values_are_quoted_or_bound() {
    let fake = Arc::new(FakeD1::new());
    fake.seed(
        r#"CREATE TABLE "we""ird'; --" (id INTEGER PRIMARY KEY, "c""ol" TEXT, "x'y" INTEGER);
           INSERT INTO "we""ird'; --" ("c""ol", "x'y")
             VALUES ('a', 9007199254740993), ('b', 2), ('c', 3);
           CREATE INDEX "i""dx'" ON "we""ird'; --" ("c""ol");
           CREATE VIEW "v'iew" AS SELECT 1 AS one;
           CREATE TABLE t (id INTEGER PRIMARY KEY);"#,
    );
    let (_server, p, s) = connected(&fake, false).await;
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
            offset: 0,
            cursor: None,
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.rows[0][2], Value::BigInt("9007199254740993".into()));
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
                    values: BTreeMap::from([(
                        "x'y".into(),
                        Value::BigInt("9007199254740995".into()),
                    )]),
                    exprs: None,
                },
                StagedChange::Delete {
                    row_id: "r3".into(),
                    key: pk(3),
                },
            ],
            returning: false,
        },
    )
    .await
    .unwrap();
    assert!(result.success, "{:?}", result.error);
    let quoted = r#""we""ird'; --""#;
    let edited = format!(r#"{quoted} WHERE "c""ol" = 'z''; DROP TABLE t; --'"#);
    assert_eq!(count(&fake, &edited), 1);
    let inserted = format!(r#"{quoted} WHERE "x'y" = 9007199254740995"#);
    assert_eq!(count(&fake, &inserted), 1);
    assert_eq!(count(&fake, quoted), 3);
    assert_eq!(count(&fake, "t"), 0, "t is still there");
}
