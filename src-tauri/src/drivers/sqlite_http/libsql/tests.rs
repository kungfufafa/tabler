//! The LibSQL client against the in-process Hrana server (`testing::FakeHrana`), and its wire
//! format through reqwest against `testkit::fake_http`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::client::{Cursor, CursorEvent, HranaClient, Prelude, StreamState, Version};
use super::error::STREAM_LOST_HINT;
use super::hrana::{Batch, BatchStep, Stmt};
use super::testing::FakeHrana;
use crate::drivers::TxStatement;
use crate::drivers::sqlite_http::catalog::{
    Remote, list_tables_query, parse_table_meta, parse_tables, table_meta_queries,
};
use crate::drivers::sqlite_http::testing::ReqwestTransport;
use crate::drivers::sqlite_http::transport::{HttpResponse, Transport};
use crate::error::AppError;
use crate::testkit::{FakeServer, Piece, Reply, Route};
use crate::types::schema::{TableRef, ValueKind};
use crate::types::value::Value;

const BASE: &str = "http://fake.test:8080";

fn client(fake: &Arc<FakeHrana>, prelude: Prelude) -> HranaClient {
    HranaClient::new(fake.clone() as Arc<dyn Transport>, BASE, prelude)
}

fn seeded() -> Arc<FakeHrana> {
    let fake = Arc::new(FakeHrana::new());
    fake.seed(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT UNIQUE, n INTEGER, ok BOOLEAN, at DATE, data BLOB);
         INSERT INTO t (id, v) VALUES (1, 'a'), (2, 'b'), (3, 'c');",
    );
    fake
}

fn count(fake: &FakeHrana) -> i64 {
    fake.connect()
        .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .unwrap()
}

fn one(sql: &str) -> Batch {
    Batch {
        steps: vec![BatchStep {
            condition: None,
            stmt: Stmt::new(sql, vec![]),
        }],
    }
}

#[tokio::test]
async fn the_version_is_probed_once() {
    let fake = seeded();
    let c = client(&fake, Prelude::default());
    assert_eq!(c.version().await.unwrap(), Version::V3);
    assert_eq!(c.version().await.unwrap(), Version::V3);
    let urls: Vec<String> = fake.requests().iter().map(|r| r.url.clone()).collect();
    assert_eq!(urls, [format!("{BASE}/v3")]);

    let old = Arc::new(FakeHrana::v2_only());
    let c = client(&old, Prelude::default());
    assert_eq!(c.version().await.unwrap(), Version::V2);
    c.execute("SELECT 1", &[]).await.unwrap();
    let paths: Vec<String> = old
        .requests()
        .iter()
        .map(|r| r.path().to_string())
        .collect();
    assert_eq!(paths, ["/v3", "/v2", "/v2/pipeline"]);

    let refused = Arc::new(FakeHrana::new());
    refused.answer_status(Some(401));
    let e = client(&refused, Prelude::default())
        .version()
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::Auth(_)), "{e:?}");
}

#[tokio::test]
async fn worker_calls_run_the_prelude_and_close_their_stream() {
    let fake = seeded();
    let c = client(
        &fake,
        Prelude {
            foreign_keys: Some(true),
            startup: vec!["PRAGMA cache_size = 50".into()],
            read_only: false,
        },
    );
    let fk = c.execute("PRAGMA foreign_keys", &[]).await.unwrap();
    assert_eq!(
        fk.rows[0][0],
        super::hrana::HranaValue::Integer { value: "1".into() }
    );
    assert_eq!(fake.open_streams(), 0);
    let body = fake.request_bodies().pop().unwrap();
    assert_eq!(body["baton"], json!(null));
    assert_eq!(body["requests"][1], json!({"type": "close"}));
    let steps = &body["requests"][0]["batch"]["steps"];
    assert_eq!(steps[0]["stmt"]["sql"], "PRAGMA foreign_keys = ON");
    assert_eq!(steps[1]["stmt"]["sql"], "PRAGMA cache_size = 50");
    assert_eq!(steps[2]["condition"], json!({"type": "ok", "step": 1}));
}

#[tokio::test]
async fn values_round_trip() {
    let fake = seeded();
    let c = client(&fake, Prelude::default());
    let big = Value::BigInt("9007199254740993".into());
    c.execute(
        "INSERT INTO t (id, v, n, ok, at, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        &[
            Value::Int(10),
            Value::Text("é".into()),
            big.clone(),
            Value::Bool(true),
            Value::Date("2026-10-27".into()),
            Value::from_bytes(&[0, 1, 255]),
        ],
    )
    .await
    .unwrap();
    let r = c
        .execute(
            "SELECT v, n, ok, at, data, 1.5, NULL FROM t WHERE id = ?1",
            &[Value::Int(10)],
        )
        .await
        .unwrap();
    let set = super::decode::result_set(&r, None, None);
    assert_eq!(set.columns[2].kind, ValueKind::Bool);
    assert_eq!(
        set.rows[0],
        [
            Value::Text("é".into()),
            big,
            Value::Bool(true),
            Value::Date("2026-10-27".into()),
            Value::from_bytes(&[0, 1, 255]),
            Value::Float(1.5),
            Value::Null
        ]
    );
    // Named parameters.
    let mut stmt = Stmt::new("SELECT :x + 1", vec![]);
    stmt.named_args = vec![super::hrana::NamedArg {
        name: ":x".into(),
        value: super::hrana::HranaValue::Integer { value: "41".into() },
    }];
    let steps = c
        .run(
            None,
            Batch {
                steps: vec![BatchStep {
                    condition: None,
                    stmt,
                }],
            },
        )
        .await
        .unwrap();
    let r = steps.into_iter().next().unwrap().into_result().unwrap();
    assert_eq!(super::decode::plain_rows(&r), [[Value::Int(42)]]);
}

#[tokio::test]
async fn read_only_streams_refuse_writes_and_a_failed_prelude_stops_the_work() {
    let fake = seeded();
    let c = client(
        &fake,
        Prelude {
            read_only: true,
            ..Prelude::default()
        },
    );
    let e = c.execute("DELETE FROM t", &[]).await.unwrap_err();
    assert!(
        matches!(
            e,
            AppError::Db {
                vendor_code: Some(8),
                ..
            }
        ),
        "query_only refuses writes: {e:?}"
    );
    assert_eq!(count(&fake), 3);
    assert_eq!(
        c.execute("SELECT * FROM t", &[]).await.unwrap().rows.len(),
        3
    );

    // Startup statements of a read-only session cannot write either.
    let sneaky = client(
        &fake,
        Prelude {
            read_only: true,
            startup: vec!["DELETE FROM t".into()],
            ..Prelude::default()
        },
    );
    let e = sneaky.execute("SELECT 1", &[]).await.unwrap_err();
    assert!(e.to_string().contains("startup statements failed"), "{e}");
    assert_eq!(count(&fake), 3);

    // Nor turn `query_only` off for the work after them.
    let undo = client(
        &fake,
        Prelude {
            read_only: true,
            startup: vec!["PRAGMA query_only = 0".into()],
            ..Prelude::default()
        },
    );
    let e = undo.execute("DELETE FROM t", &[]).await.unwrap_err();
    assert!(
        matches!(
            e,
            AppError::Db {
                vendor_code: Some(8),
                ..
            }
        ),
        "{e:?}"
    );
    let mut held = StreamState::default();
    let e = undo
        .execute_on(&mut held, "DELETE FROM t", &[])
        .await
        .unwrap_err();
    assert!(matches!(e, AppError::Db { .. }), "{e:?}");
    let mut cursor = undo.cursor(None, one("DELETE FROM t")).await.unwrap();
    let events = drain(&mut cursor).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, CursorEvent::StepError { step: 0, .. })),
        "{events:?}"
    );
    assert_eq!(count(&fake), 3);

    // A startup statement that fails: the write after it never runs.
    let broken = client(
        &fake,
        Prelude {
            startup: vec!["SELEC 1".into()],
            ..Prelude::default()
        },
    );
    let e = broken.execute("DELETE FROM t", &[]).await.unwrap_err();
    assert!(e.to_string().contains("startup statements failed"), "{e}");
    assert_eq!(count(&fake), 3);
}

#[tokio::test]
async fn the_editor_stream_keeps_its_transaction() {
    let fake = seeded();
    let c = client(&fake, Prelude::default());
    let mut s = StreamState::default();
    c.execute_on(&mut s, "BEGIN", &[]).await.unwrap();
    assert!(s.is_open());
    assert_eq!(s.autocommit, Some(false));
    let ins = c
        .execute_on(&mut s, "INSERT INTO t (v) VALUES ('d')", &[])
        .await
        .unwrap();
    assert_eq!(ins.affected_row_count, 1);
    // Another stream does not see it yet.
    assert_eq!(count(&fake), 3);
    c.execute_on(&mut s, "COMMIT", &[]).await.unwrap();
    assert_eq!(s.autocommit, Some(true));
    assert_eq!(count(&fake), 4);
    assert!(!s.take_lost());

    // Every request carried the baton of the reply before it.
    let bodies = fake.request_bodies();
    // bodies[0] is the version probe.
    let batons: Vec<_> = bodies.iter().map(|b| b["baton"].clone()).collect();
    assert_eq!(batons[1], json!(null));
    assert!(batons[2].is_string() && batons[3].is_string());
    assert_ne!(batons[2], batons[3]);
    assert!(
        bodies[1]["requests"]
            .as_array()
            .unwrap()
            .contains(&json!({"type": "get_autocommit"}))
    );
    c.close(&mut s).await;
    assert!(!s.is_open());
    assert_eq!(fake.open_streams(), 0);
}

#[tokio::test]
async fn hrana_2_streams_have_no_autocommit_flag() {
    let fake = Arc::new(FakeHrana::v2_only());
    let c = client(&fake, Prelude::default());
    let mut s = StreamState::default();
    c.execute_on(&mut s, "BEGIN", &[]).await.unwrap();
    assert_eq!(s.autocommit, None);
    let body = fake.request_bodies().pop().unwrap();
    assert_eq!(body["requests"].as_array().unwrap().len(), 1);
    c.execute_on(&mut s, "ROLLBACK", &[]).await.unwrap();
    let e = c.cursor(None, one("SELECT 1")).await.err().unwrap();
    assert!(matches!(e, AppError::Unsupported(_)));
}

#[tokio::test]
async fn an_expired_stream_is_lost_with_its_transaction() {
    let fake = seeded();
    let c = client(
        &fake,
        Prelude {
            foreign_keys: Some(true),
            ..Prelude::default()
        },
    );
    let mut s = StreamState::default();
    c.execute_on(&mut s, "BEGIN", &[]).await.unwrap();
    c.execute_on(&mut s, "DELETE FROM t", &[]).await.unwrap();
    fake.expire_streams();
    let e = c.execute_on(&mut s, "COMMIT", &[]).await.unwrap_err();
    match &e {
        AppError::Db { hint, .. } => assert_eq!(hint.as_deref(), Some(STREAM_LOST_HINT)),
        other => panic!("{other:?}"),
    }
    assert!(!s.is_open());
    assert!(s.take_lost());
    assert!(!s.take_lost());
    // The rows are back, and the next call opens a new stream with the prelude.
    assert_eq!(count(&fake), 3);
    c.execute_on(&mut s, "SELECT 1", &[]).await.unwrap();
    let body = fake.request_bodies().pop().unwrap();
    assert_eq!(body["baton"], json!(null));
    assert_eq!(
        body["requests"][0]["batch"]["steps"][0]["stmt"]["sql"],
        "PRAGMA foreign_keys = ON"
    );
}

#[tokio::test]
async fn a_reply_never_read_drops_the_stream() {
    let fake = seeded();
    let c = client(&fake, Prelude::default());
    let mut s = StreamState::default();
    c.execute_on(&mut s, "BEGIN", &[]).await.unwrap();
    fake.delay_replies(Some(Duration::from_secs(30)));
    let gave_up = tokio::time::timeout(
        Duration::from_millis(50),
        c.execute_on(&mut s, "INSERT INTO t (v) VALUES ('x')", &[]),
    )
    .await;
    assert!(gave_up.is_err());
    fake.delay_replies(None);
    // The server moved the baton on: the old one is never sent again.
    c.execute_on(&mut s, "SELECT 1", &[]).await.unwrap();
    assert!(s.take_lost());
    assert_eq!(fake.request_bodies().pop().unwrap()["baton"], json!(null));
}

#[tokio::test]
async fn the_base_url_a_server_names_is_followed_on_its_origin_only() {
    let fake = seeded();
    fake.set_base_url(Some("/replica/db"));
    let c = client(&fake, Prelude::default());
    let mut s = StreamState::default();
    c.execute_on(&mut s, "SELECT 1", &[]).await.unwrap();
    c.execute_on(&mut s, "SELECT 2", &[]).await.unwrap();
    // A cursor on the held stream goes there too, and keeps it.
    let mut cursor = c.cursor(Some(&mut s), one("SELECT 3")).await.unwrap();
    drain(&mut cursor).await;
    cursor.update(&mut s);
    // Another origin would get the token: the stream stays where it is.
    fake.set_base_url(Some("http://replica.test:9000/db"));
    c.execute_on(&mut s, "SELECT 4", &[]).await.unwrap();
    c.execute_on(&mut s, "SELECT 5", &[]).await.unwrap();
    let mut cursor = c.cursor(Some(&mut s), one("SELECT 6")).await.unwrap();
    drain(&mut cursor).await;
    cursor.update(&mut s);
    c.execute_on(&mut s, "SELECT 7", &[]).await.unwrap();
    let urls: Vec<String> = fake.requests().iter().map(|r| r.url.clone()).collect();
    let replica = format!("{BASE}/replica/db");
    assert_eq!(
        urls,
        [
            format!("{BASE}/v3"),
            format!("{BASE}/v3/pipeline"),
            format!("{replica}/v3/pipeline"),
            format!("{replica}/v3/cursor"),
            format!("{replica}/v3/pipeline"),
            format!("{replica}/v3/pipeline"),
            format!("{replica}/v3/cursor"),
            format!("{replica}/v3/pipeline"),
        ]
    );
    // Once the stream is closed the next one starts at the profile's base.
    c.close(&mut s).await;
    c.execute_on(&mut s, "SELECT 8", &[]).await.unwrap();
    assert_eq!(
        fake.requests().pop().unwrap().url,
        format!("{BASE}/v3/pipeline")
    );
}

#[tokio::test]
async fn transactions_commit_whole_or_not_at_all() {
    let fake = seeded();
    let c = client(&fake, Prelude::default());
    let ok = c
        .transaction(&[
            TxStatement::new(
                "UPDATE t SET v = ?1 WHERE id = ?2",
                vec![Value::Text("z".into()), Value::Int(1)],
            )
            .expecting(1),
            TxStatement::new("INSERT INTO t (v) VALUES ('d')", vec![]),
        ])
        .await
        .unwrap();
    assert_eq!(ok[0].affected, 1);
    assert_eq!(ok[1].last_insert_id.as_deref(), Some("4"));
    assert_eq!(count(&fake), 4);

    // A guard that does not hold rolls back the statements before it.
    let e = c
        .transaction(&[
            TxStatement::new("INSERT INTO t (v) VALUES ('e')", vec![]),
            TxStatement::new("DELETE FROM t WHERE id = 99", vec![]).expecting(1),
        ])
        .await
        .unwrap_err();
    assert!(
        e.to_string()
            .starts_with("expected to affect 1 row(s) but matched 0"),
        "{e}"
    );
    assert!(matches!(
        e,
        AppError::Db {
            statement_index: Some(1),
            ..
        }
    ));
    assert_eq!(count(&fake), 4);

    // So does a failing statement, which the error names.
    let e = c
        .transaction(&[
            TxStatement::new("DELETE FROM t WHERE id = 2", vec![]).expecting(1),
            TxStatement::new("INSERT INTO t (v) VALUES ('c')", vec![]),
        ])
        .await
        .unwrap_err();
    match e {
        AppError::Db {
            statement_index,
            vendor_code,
            ..
        } => {
            assert_eq!(statement_index, Some(1));
            assert_eq!(vendor_code, Some(2067));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(count(&fake), 4);
    // Nothing was left open or locked.
    assert_eq!(fake.open_streams(), 0);
    c.execute("DELETE FROM t WHERE id = 2", &[]).await.unwrap();
    assert_eq!(count(&fake), 3);
}

#[tokio::test]
async fn catalog_calls_take_one_round_trip() {
    let fake = seeded();
    fake.seed(
        "CREATE TABLE libsql_wasm_func_table (name TEXT);
         CREATE INDEX t_n ON t (n) WHERE n > 0;
         CREATE TRIGGER t_touch AFTER UPDATE ON t BEGIN SELECT 1; END;",
    );
    let c = client(&fake, Prelude::default());
    let before = fake.requests().len();
    let t = TableRef::new(None, "t");
    let rows = c.catalog(&table_meta_queries(&t)).await.unwrap();
    assert_eq!(
        fake.requests().len(),
        before + 2,
        "the probe and one pipeline"
    );
    let meta = parse_table_meta(&t, &rows).unwrap();
    assert_eq!(meta.primary_key, ["id"]);
    assert_eq!(meta.columns.len(), 6);
    assert_eq!(meta.columns[3].kind, ValueKind::Bool);
    let live = meta.indexes.iter().find(|i| i.name == "t_n").unwrap();
    assert_eq!(live.predicate.as_deref(), Some("n > 0"));
    assert_eq!(meta.triggers[0].timing.as_deref(), Some("AFTER"));

    let tables = c
        .catalog(&[list_tables_query(Remote::Libsql)])
        .await
        .unwrap();
    let names: Vec<String> = parse_tables(&tables[0])
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["t"]);

    let bad = c
        .catalog(&[crate::drivers::sqlite_http::catalog::CatalogQuery {
            sql: "SELECT * FROM nope",
            params: vec![],
        }])
        .await
        .unwrap_err();
    assert!(bad.to_string().contains("no such table"), "{bad}");
}

async fn drain(cursor: &mut Cursor) -> Vec<CursorEvent> {
    let mut events = Vec::new();
    while let Some(e) = cursor.next_event().await.unwrap() {
        events.push(e);
    }
    events
}

#[tokio::test]
async fn cursors_stream_rows_and_hide_the_prelude() {
    let fake = seeded();
    let c = client(
        &fake,
        Prelude {
            foreign_keys: Some(false),
            ..Prelude::default()
        },
    );
    let work = Batch {
        steps: vec![
            BatchStep {
                condition: None,
                stmt: Stmt::new("SELECT id FROM t ORDER BY id", vec![]),
            },
            BatchStep {
                condition: None,
                stmt: Stmt::new("SELECT * FROM nope", vec![]),
            },
        ],
    };
    let mut cursor = c.cursor(None, work).await.unwrap();
    let events = drain(&mut cursor).await;
    assert!(cursor.head().unwrap().baton.is_some());
    assert!(matches!(&events[0], CursorEvent::Begin { step: 0, cols } if cols.len() == 1));
    let rows = events
        .iter()
        .filter(|e| matches!(e, CursorEvent::Row(_)))
        .count();
    assert_eq!(rows, 3);
    assert!(matches!(events[4], CursorEvent::End { .. }));
    assert!(
        matches!(&events[5], CursorEvent::StepError { step: 1, error } if error.message.contains("no such table"))
    );
    assert_eq!(events.len(), 6);

    // The editor's stream goes on after a cursor once it has the new baton.
    let mut s = StreamState::default();
    c.execute_on(&mut s, "BEGIN", &[]).await.unwrap();
    let mut cursor = c
        .cursor(Some(&mut s), one("DELETE FROM t RETURNING id"))
        .await
        .unwrap();
    let events = drain(&mut cursor).await;
    assert_eq!(events.len(), 5);
    cursor.update(&mut s);
    c.execute_on(&mut s, "ROLLBACK", &[]).await.unwrap();
    assert!(!s.take_lost());
    assert_eq!(count(&fake), 3);

    // A cursor whose prelude fails is an error.
    let broken = client(
        &fake,
        Prelude {
            startup: vec!["SELEC".into()],
            ..Prelude::default()
        },
    );
    let mut cursor = broken.cursor(None, one("SELECT 1")).await.unwrap();
    let e = cursor.next_event().await.unwrap_err();
    assert!(e.to_string().contains("startup statements failed"), "{e}");
}

#[tokio::test]
async fn servers_that_refuse_or_break() {
    let fake = seeded();
    let c = client(&fake, Prelude::default());
    c.version().await.unwrap();
    fake.fail_next(HttpResponse::text(401, "Unauthorized"));
    assert!(matches!(
        c.execute("SELECT 1", &[]).await.unwrap_err(),
        AppError::Auth(_)
    ));
    fake.fail_next(HttpResponse::text(404, "namespace not found"));
    assert!(matches!(
        c.execute("SELECT 1", &[]).await.unwrap_err(),
        AppError::NotFound(_)
    ));
    fake.fail_next(HttpResponse {
        status: 502,
        retry_after: None,
        content_type: Some("text/html".into()),
        body: b"<html><body>Bad Gateway</body></html>".to_vec(),
    });
    let e = c.execute("SELECT 1", &[]).await.unwrap_err();
    assert!(
        matches!(&e, AppError::Connect(m) if m.contains("Bad Gateway")),
        "{e:?}"
    );
    fake.fail_next(HttpResponse::json(
        200,
        &json!({"baton": null, "results": []}),
    ));
    let e = c.execute("SELECT 1", &[]).await.unwrap_err();
    assert!(e.to_string().contains("0 result(s) to 2 request(s)"), "{e}");
    fake.fail_next(HttpResponse::text(200, "{\"results\": ["));
    assert!(c.execute("SELECT 1", &[]).await.is_err());
    let described = c.describe("DELETE FROM t WHERE id = ?1").await.unwrap();
    assert!(!described.is_readonly);
    assert_eq!(described.params.len(), 1);
    assert!(c.describe("SELECT 1").await.unwrap().is_readonly);
}

#[tokio::test]
async fn the_wire_format_over_http() {
    let pipeline = json!({
        "baton": null,
        "base_url": null,
        "results": [
            {"type": "ok", "response": {"type": "batch", "result": {
                "step_results": [{"cols": [{"name": "1", "decltype": null}],
                                  "rows": [[{"type": "integer", "value": "1"}]],
                                  "affected_row_count": 0, "last_insert_rowid": null}],
                "step_errors": [null]}}},
            {"type": "ok", "response": {"type": "close"}}
        ]
    });
    let cursor_body = concat!(
        "{\"baton\":\"b\",\"base_url\":null}\n{\"type\":\"step_begin\",\"step\":0,\"cols\":[]}\n",
        "{\"type\":\"row\",\"row\":[{\"type\":\"text\",\"value\":\"x\"}]}\n",
        "{\"type\":\"step_end\",\"affected_row_count\":0,\"last_insert_rowid\":null}\n"
    );
    let server = FakeServer::start(vec![
        Route::new("GET", "/v3")
            .auth("Bearer tok")
            .reply(Reply::ok()),
        Route::new("POST", "/v3/pipeline")
            .auth("Bearer tok")
            .reply(Reply::json(200, &pipeline)),
        Route::new("POST", "/v3/cursor")
            .auth("Bearer tok")
            .reply(Reply::ok().chunked(vec![
                Piece::Data(cursor_body.as_bytes()[..40].to_vec()),
                Piece::Wait(Duration::from_millis(20)),
                Piece::Data(cursor_body.as_bytes()[40..].to_vec()),
            ])),
    ])
    .await;
    let base = server.url("");
    let c = HranaClient::new(
        Arc::new(ReqwestTransport::new(Some("tok"))),
        base.clone(),
        Prelude::default(),
    );
    let r = c.execute("SELECT 1", &[]).await.unwrap();
    assert_eq!(r.rows.len(), 1);
    let mut cursor = c.cursor(None, one("SELECT 'x'")).await.unwrap();
    let events = drain(&mut cursor).await;
    assert_eq!(events.len(), 3);
    let requests = server.requests();
    assert_eq!(requests[1].header("content-type"), Some("application/json"));
    assert_eq!(requests[1].header("authorization"), Some("Bearer tok"));
    assert_eq!(
        requests[1].json()["requests"][0]["batch"]["steps"][0]["stmt"],
        json!({"sql": "SELECT 1", "want_rows": true})
    );

    // Without a token no authorization header goes out (an open `sqld` refuses an empty one).
    let open = HranaClient::new(
        Arc::new(ReqwestTransport::new(Some(""))),
        base,
        Prelude::default(),
    );
    assert!(matches!(
        open.version().await.unwrap_err(),
        AppError::Auth(_)
    ));
    let last = server.requests().pop().unwrap();
    assert_eq!(last.header("authorization"), None);
}
