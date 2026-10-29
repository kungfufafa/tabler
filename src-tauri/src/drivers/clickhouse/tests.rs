//! The client against the scripted server (`testing::FakeClickhouse`), and its requests and
//! replies over a real socket against `testkit::fake_http`.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use crate::drivers::{CollectSink, RowSink, TxStatement};
use crate::error::AppError;
use crate::testkit::{FakeServer, Piece, Reply, Route};
use crate::types::query::ResultColumn;
use crate::types::schema::TableKind;
use crate::types::value::Value;

use super::catalog;
use super::client::{Client, READONLY_PROBE, SETTINGS_PROBE};
use super::request::{ConnSettings, Credentials, Endpoint, HttpSession, OUTPUT_FORMAT, Query};
use super::testing::{
    FakeClickhouse, FakeReply, ReqwestTransport, exception_block, exception_text,
};
use super::transport::header;

fn endpoint() -> Endpoint {
    Endpoint {
        tls: false,
        host: "127.0.0.1".into(),
        port: 8123,
    }
}

fn creds() -> Credentials {
    Credentials::Password {
        user: "alice".into(),
        password: "secret".into(),
    }
}

fn client(fake: &Arc<FakeClickhouse>, read_only: bool) -> Client {
    Client::new(
        fake.clone(),
        endpoint(),
        creds(),
        ConnSettings {
            database: Some("default".into()),
            read_only,
            output_settings: true,
            ..ConnSettings::default()
        },
    )
}

fn q(sql: &str) -> Query<'_> {
    Query {
        sql,
        ..Query::default()
    }
}

#[tokio::test]
async fn a_select_streams_its_rows_with_their_types() {
    let fake = Arc::new(FakeClickhouse::new());
    fake.on(
        "FROM events",
        FakeReply::rows(
            &["id", "amount", "tags", "attrs"],
            &[
                "UInt64",
                "Decimal(38, 4)",
                "Array(LowCardinality(String))",
                "Map(String, UInt256)",
            ],
            &[
                r#"["9007199254740993","1.5000",["a","b"],{"k":"115792089237316195423570985008687907853269984665640564039457584007913129639935"}]"#,
                r#"["2","-0.0001",[],{}]"#,
            ],
        ),
    );
    let c = client(&fake, false);
    let mut sink = CollectSink::default();
    let out = c.run(&q("SELECT * FROM events"), &mut sink).await.unwrap();
    assert!(out.result_set && !out.stopped);
    assert_eq!(out.rows, 2);
    assert_eq!(out.timezone.as_deref(), Some("UTC"));
    assert!(out.query_id.starts_with("tabler-"));
    let set = &sink.sets[0];
    assert_eq!(set.columns[1].data_type, "Decimal(38, 4)");
    assert_eq!(
        set.rows[0],
        [
            Value::BigInt("9007199254740993".into()),
            Value::Decimal("1.5000".into()),
            Value::Array(vec![Value::Text("a".into()), Value::Text("b".into())]),
            Value::Json(
                r#"{"k":"115792089237316195423570985008687907853269984665640564039457584007913129639935"}"#
                    .into()
            ),
        ]
    );
    let req = &fake.requests()[0];
    assert_eq!(req.param("query_id"), Some(out.query_id.clone()));
    assert_eq!(req.param("database").as_deref(), Some("default"));
    assert_eq!(req.param("readonly"), None);
    assert_eq!(req.header(header::KEY), Some("secret"));
}

#[tokio::test]
async fn errors_before_and_after_the_rows() {
    let fake = Arc::new(FakeClickhouse::new());
    fake.on(
        "nope",
        FakeReply::error(60, "Table default.nope does not exist"),
    );
    fake.on(
        "throwIf",
        FakeReply::FailAfter {
            names: vec!["x".into()],
            types: vec!["UInt8".into()],
            rows: vec!["[1]".into(), "[2]".into()],
            code: 395,
            message: "Value passed to 'throwIf' function is non-zero".into(),
        },
    );
    let c = client(&fake, false);
    let mut sink = CollectSink::default();
    match c.run(&q("SELECT * FROM nope"), &mut sink).await {
        Err(AppError::Db {
            vendor_code: Some(60),
            message,
            ..
        }) => assert!(message.contains("does not exist"), "{message}"),
        other => panic!("{other:?}"),
    }
    let mut sink = CollectSink::default();
    let err = c
        .run(&q("SELECT throwIf(number > 1) FROM numbers(3)"), &mut sink)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        AppError::Db {
            vendor_code: Some(395),
            ..
        }
    ));
    // The rows before the error reached the sink; the error says the rest is missing.
    assert_eq!(sink.sets[0].rows.len(), 2);

    let mut wrong = FakeClickhouse::new();
    wrong.credentials = Some(("alice".into(), "other".into()));
    let wrong = Arc::new(wrong);
    let err = client(&wrong, false)
        .run(&q("SELECT 1"), &mut CollectSink::default())
        .await;
    assert!(matches!(err, Err(AppError::Auth(_))), "{err:?}");
}

#[tokio::test]
async fn read_only_sessions_send_readonly_2_and_the_server_refuses_writes() {
    let fake = Arc::new(FakeClickhouse::new());
    fake.on("SELECT", FakeReply::rows(&["x"], &["UInt8"], &["[1]"]));
    let c = client(&fake, true);
    assert!(c.read_only() && c.hardened());
    c.rows(&q("SELECT 1")).await.unwrap();
    for sql in [
        "INSERT INTO t VALUES (1)",
        "CREATE TABLE x (a UInt8) ENGINE = Memory",
        "ALTER TABLE t UPDATE a = 1 WHERE 1",
    ] {
        let err = c.execute(sql, &[], &[]).await.unwrap_err();
        assert!(
            matches!(
                err,
                AppError::Db {
                    vendor_code: Some(164),
                    ..
                }
            ),
            "{sql}: {err:?}"
        );
    }
    for req in fake.requests() {
        assert_eq!(req.query_pairs()[0], ("readonly".into(), "2".into()));
    }
}

#[tokio::test]
async fn a_readonly_1_profile_gets_no_settings_after_the_first_refusal() {
    let mut fake = FakeClickhouse::new();
    fake.profile_readonly = true;
    let fake = Arc::new(fake);
    fake.on(
        "SELECT version()",
        FakeReply::rows(
            &["v", "db", "u", "tz", "ro"],
            &["String", "String", "String", "String", "String"],
            &[r#"["24.3.2.23","default","alice","UTC","1"]"#],
        ),
    );
    let c = client(&fake, true);
    let info = c.server_info().await.unwrap();
    assert_eq!(info.readonly, 1);
    assert_eq!(info.release(), Some((24, 3)));
    let reqs = fake.requests();
    let sql: Vec<String> = reqs.iter().map(|r| r.sql()).collect();
    assert_eq!(
        sql,
        [
            catalog::SERVER_INFO,
            SETTINGS_PROBE,
            READONLY_PROBE,
            catalog::SERVER_INFO
        ],
        "refused, the settings alone refused too, the profile asked, then sent without settings"
    );
    assert_eq!(reqs[0].param("readonly").as_deref(), Some("2"));
    assert_eq!(reqs[1].param("readonly").as_deref(), Some("2"));
    assert_eq!(reqs[1].param("dialect").as_deref(), Some("clickhouse"));
    for r in &reqs[2..] {
        assert_eq!(r.param("readonly"), None);
        assert_eq!(r.param("output_format_json_quote_64bit_integers"), None);
        assert_eq!(r.param("dialect"), None);
        assert_eq!(r.param("session_id"), None);
    }
    // Still read-only: the server's readonly=1 holds.
    assert!(c.read_only() && c.hardened());
    assert!(c.execute("DROP TABLE t", &[], &[]).await.is_err());
}

#[tokio::test]
async fn every_request_pins_the_language_and_keeps_stream_reads_off() {
    let fake = Arc::new(FakeClickhouse::new());
    fake.on("SELECT", FakeReply::rows(&["x"], &["UInt8"], &["[1]"]));
    fake.on("SET", FakeReply::Done { written_rows: 0 });
    let c = client(&fake, false);
    let session = HttpSession {
        id: "s-1".into(),
        timeout_secs: None,
    };
    let in_session = |sql: &'static str| Query {
        sql,
        session: Some(&session),
        ..Query::default()
    };
    // A SET the user confirmed changes the session, not what the next request reads.
    c.rows(&in_session("SET dialect = 'polyglot'"))
        .await
        .unwrap();
    c.rows(&in_session("SELECT 1")).await.unwrap();
    c.execute("SELECT 2", &[], &[("max_threads", "1")])
        .await
        .unwrap();
    let reqs = fake.requests();
    assert_eq!(reqs.len(), 3);
    for r in &reqs {
        assert_eq!(
            r.param("dialect").as_deref(),
            Some("clickhouse"),
            "{}",
            r.sql()
        );
        assert_eq!(
            r.param("stream_like_engine_allow_direct_select").as_deref(),
            Some("0")
        );
    }
    // After the output settings (and the profile's), before the statement's own.
    let keys: Vec<String> = reqs[2].query_pairs().into_iter().map(|(k, _)| k).collect();
    let at = |name: &str| keys.iter().position(|k| k == name).unwrap();
    assert!(at("cancel_http_readonly_queries_on_client_close") < at("dialect"));
    assert!(at("dialect") < at("max_threads"));
    // A connection's own settings may not change them.
    let c = Client::new(
        fake.clone(),
        endpoint(),
        creds(),
        ConnSettings {
            settings: vec![("dialect".into(), "kusto".into())],
            ..ConnSettings::default()
        },
    );
    assert!(matches!(
        c.rows(&q("SELECT 1")).await,
        Err(AppError::InvalidInput(_))
    ));
    assert_eq!(fake.requests().len(), 3, "refused before it was sent");
}

#[tokio::test]
async fn a_statements_own_readonly_setting_keeps_readonly_2() {
    let fake = Arc::new(FakeClickhouse::new());
    fake.on("SELECT", FakeReply::rows(&["x"], &["UInt8"], &["[1]"]));
    let c = client(&fake, true);
    // Refused for the statement's SETTINGS clause, not for the driver's settings.
    let err = c
        .rows(&q("SELECT 1 SETTINGS readonly = 0"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AppError::Db {
                vendor_code: Some(164),
                ..
            }
        ),
        "{err:?}"
    );
    let err = c.execute("DROP TABLE t", &[], &[]).await.unwrap_err();
    assert!(err.to_string().contains("Cannot execute query"), "{err}");
    let reqs = fake.requests();
    let sql: Vec<String> = reqs.iter().map(|r| r.sql()).collect();
    assert_eq!(
        sql,
        [
            "SELECT 1 SETTINGS readonly = 0",
            SETTINGS_PROBE,
            "DROP TABLE t"
        ],
        "no retry and no look at the profile"
    );
    for r in &reqs {
        assert_eq!(
            r.query_pairs()[0],
            ("readonly".into(), "2".into()),
            "{}",
            r.sql()
        );
        assert_eq!(
            r.param("output_format_json_quote_64bit_integers")
                .as_deref(),
            Some("1")
        );
    }
    assert!(c.read_only() && c.hardened());
}

#[tokio::test]
async fn a_constraint_on_an_output_setting_drops_only_the_output_settings() {
    let mut fake = FakeClickhouse::new();
    fake.fixed_settings = vec!["output_format_json_quote_decimals".into()];
    let fake = Arc::new(fake);
    fake.on("SELECT", FakeReply::rows(&["x"], &["UInt8"], &["[1]"]));
    let c = client(&fake, true);
    assert_eq!(c.rows(&q("SELECT 2")).await.unwrap(), [[Value::Int(1)]]);
    c.rows(&q("SELECT 3")).await.unwrap();
    let reqs = fake.requests();
    let sql: Vec<String> = reqs.iter().map(|r| r.sql()).collect();
    assert_eq!(sql, ["SELECT 2", SETTINGS_PROBE, "SELECT 2", "SELECT 3"]);
    for r in &reqs {
        assert_eq!(r.query_pairs()[0], ("readonly".into(), "2".into()));
    }
    for r in &reqs[2..] {
        assert_eq!(r.param("output_format_json_quote_decimals"), None);
    }
    assert!(c.hardened());
}

#[tokio::test]
async fn a_statements_own_constraint_violation_keeps_the_output_settings() {
    let mut fake = FakeClickhouse::new();
    // Named by the statement only: the URL does not change it.
    fake.fixed_settings = vec!["max_threads".into()];
    let fake = Arc::new(fake);
    fake.on("SELECT", FakeReply::rows(&["x"], &["UInt8"], &["[1]"]));
    let c = client(&fake, false);
    let err = c
        .rows(&q("SELECT 1 SETTINGS max_threads = 8"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AppError::Db {
                vendor_code: Some(452),
                ..
            }
        ),
        "{err:?}"
    );
    c.rows(&q("SELECT 2")).await.unwrap();
    let reqs = fake.requests();
    let sql: Vec<String> = reqs.iter().map(|r| r.sql()).collect();
    assert_eq!(
        sql,
        [
            "SELECT 1 SETTINGS max_threads = 8",
            SETTINGS_PROBE,
            "SELECT 2"
        ]
    );
    assert_eq!(
        reqs[2]
            .param("output_format_json_quote_decimals")
            .as_deref(),
        Some("1")
    );
}

#[tokio::test]
async fn a_refusal_the_settings_alone_do_not_explain_is_returned_as_it_is() {
    // The driver's own statement settings (an edit's mutations_sync) under a constraint: not an
    // output setting, so nothing is dropped and the error stands.
    let mut fake = FakeClickhouse::new();
    fake.fixed_settings = vec!["mutations_sync".into()];
    let fake = Arc::new(fake);
    let c = client(&fake, true);
    let err = c
        .execute(
            "ALTER TABLE t DELETE WHERE 1",
            &[],
            &[("mutations_sync", "2")],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AppError::Db {
                vendor_code: Some(452),
                ..
            }
        ),
        "{err:?}"
    );
    let reqs = fake.requests();
    assert_eq!(reqs.len(), 2, "the request and the probe, no retry");
    assert_eq!(reqs[1].param("mutations_sync").as_deref(), Some("2"));
    assert!(
        reqs.iter()
            .all(|r| r.param("readonly").as_deref() == Some("2"))
    );
}

struct Capped {
    inner: CollectSink,
    cap: usize,
}

impl RowSink for Capped {
    fn begin_set(&mut self, columns: Vec<ResultColumn>) {
        self.inner.begin_set(columns);
    }

    fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        let _ = self.inner.rows(rows);
        if self.inner.sets.last().map_or(0, |s| s.rows.len()) >= self.cap {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

#[tokio::test]
async fn a_full_sink_closes_the_connection() {
    let fake = Arc::new(FakeClickhouse::new());
    let rows: Vec<String> = (0..2000).map(|i| format!("[{i}]")).collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    fake.on("numbers", FakeReply::rows(&["n"], &["UInt32"], &rows));
    let c = client(&fake, false);
    let mut sink = Capped {
        inner: CollectSink::default(),
        cap: 500,
    };
    let out = c
        .run(&q("SELECT number FROM numbers(2000)"), &mut sink)
        .await
        .unwrap();
    assert!(out.stopped);
    assert!(out.rows >= 500 && out.rows < 2000, "{}", out.rows);
    assert_eq!(fake.dropped(), [out.query_id]);
}

#[tokio::test]
async fn kill_stops_a_running_query_by_its_id() {
    let fake = Arc::new(FakeClickhouse::new());
    fake.on(
        "sleepEachRow",
        FakeReply::Hang {
            names: vec!["x".into()],
            types: vec!["UInt8".into()],
        },
    );
    let c = Arc::new(client(&fake, true));
    let runner = {
        let c = c.clone();
        tokio::spawn(async move {
            let mut sink = CollectSink::default();
            c.run(
                &Query {
                    sql: "SELECT sleepEachRow(1) FROM numbers(100)",
                    query_id: Some("q-42"),
                    ..Query::default()
                },
                &mut sink,
            )
            .await
        })
    };
    // Wait until the query is under way.
    for _ in 0..200 {
        if !fake.requests().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    c.kill("q-42").await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), runner)
        .await
        .expect("the run ends")
        .unwrap();
    assert!(matches!(result, Err(AppError::Cancelled)), "{result:?}");
    assert_eq!(fake.killed(), ["q-42"]);
    let kill = fake
        .requests()
        .into_iter()
        .find(|r| r.sql().starts_with("KILL"))
        .unwrap();
    assert_eq!(kill.sql(), "KILL QUERY WHERE query_id = {id:String} ASYNC");
    assert_eq!(kill.param("param_id").as_deref(), Some("q-42"));
    assert_eq!(kill.param("session_id"), None);
}

#[tokio::test]
async fn edits_are_guarded_by_a_count_of_their_key() {
    let fake = Arc::new(FakeClickhouse::new());
    let update = TxStatement::new(
        "ALTER TABLE `db`.`t` UPDATE `name` = {p1:String} WHERE `id` = {p2:UInt64}",
        vec![Value::Text("x".into()), Value::Int(7)],
    )
    .expecting(1);
    let c = client(&fake, false);

    // A key that matches two rows: nothing is changed.
    fake.once(
        "count()",
        FakeReply::rows(&["c"], &["UInt64"], &[r#"["2"]"#]),
    );
    let err = c.apply(&update, 3).await.unwrap_err();
    assert!(err.to_string().contains("matches 2 row(s)"), "{err}");
    assert!(matches!(
        err,
        AppError::Db {
            statement_index: Some(3),
            ..
        }
    ));
    assert_eq!(
        fake.statements(),
        ["SELECT count() FROM `db`.`t` WHERE `id` = {p2:UInt64}"]
    );
    // The count carries only the condition's value.
    let count = &fake.requests()[0];
    assert_eq!(count.param("param_p2").as_deref(), Some("7"));
    assert_eq!(count.param("param_p1"), None);

    // One row: the mutation runs, and waits.
    fake.once(
        "count()",
        FakeReply::rows(&["c"], &["UInt64"], &[r#"["1"]"#]),
    );
    fake.once("ALTER TABLE", FakeReply::Done { written_rows: 0 });
    let out = c.apply(&update, 0).await.unwrap();
    assert_eq!(out.affected, 1);
    assert!(!out.affected_unknown);
    let reqs = fake.requests();
    let alter = reqs.last().unwrap();
    assert!(alter.sql().starts_with("ALTER TABLE"));
    assert_eq!(alter.param("mutations_sync").as_deref(), Some("2"));
    assert_eq!(alter.param("param_p1").as_deref(), Some("x"));

    // A delete whose row is gone.
    let delete = TxStatement::new(
        "DELETE FROM `db`.`t` WHERE `id` = {p1:UInt64}",
        vec![Value::Int(7)],
    )
    .expecting(1);
    fake.once(
        "count()",
        FakeReply::rows(&["c"], &["UInt64"], &[r#"["0"]"#]),
    );
    assert!(c.apply(&delete, 0).await.is_err());
    assert!(!fake.statements().last().unwrap().starts_with("DELETE"));

    // An insert into a table with a materialized view: the summary counts both writes.
    let insert = TxStatement::new(
        "INSERT INTO `db`.`t` (`id`) SELECT {p1:UInt64}",
        vec![Value::Int(8)],
    )
    .expecting(1);
    fake.once("INSERT", FakeReply::Done { written_rows: 2 });
    assert_eq!(c.apply(&insert, 0).await.unwrap().affected, 1);
    // A deduplicated insert wrote nothing.
    fake.once("INSERT", FakeReply::Done { written_rows: 0 });
    assert!(c.apply(&insert, 0).await.is_err());

    // A mutation with a subquery is counted by its own condition.
    let other = TxStatement::new("ALTER TABLE t DELETE WHERE 1 IN (SELECT 1)", vec![]).expecting(1);
    let before = fake.requests().len();
    fake.once(
        "count()",
        FakeReply::rows(&["c"], &["UInt64"], &[r#"["1"]"#]),
    );
    fake.once("ALTER", FakeReply::Done { written_rows: 0 });
    assert!(c.apply(&other, 0).await.is_ok());
    assert_eq!(fake.requests().len(), before + 2);
    // One the guard cannot count is refused before it is sent.
    let before = fake.requests().len();
    let uncountable = TxStatement::new("DELETE FROM t", vec![]).expecting(1);
    assert!(c.apply(&uncountable, 0).await.is_err());
    assert_eq!(fake.requests().len(), before);
    // A statement whose count nobody reports fails the guard.
    let optimize = TxStatement::new("OPTIMIZE TABLE t FINAL", vec![]).expecting(1);
    fake.once("OPTIMIZE", FakeReply::Done { written_rows: 0 });
    assert!(c.apply(&optimize, 0).await.is_err());
}

#[tokio::test]
async fn execute_reports_written_rows_and_unknown_counts() {
    let fake = Arc::new(FakeClickhouse::new());
    fake.on("INSERT", FakeReply::Done { written_rows: 3 });
    fake.on("DELETE", FakeReply::Done { written_rows: 0 });
    fake.on("CREATE", FakeReply::Done { written_rows: 0 });
    let c = client(&fake, false);
    let out = c
        .execute("INSERT INTO t SELECT * FROM s", &[], &[])
        .await
        .unwrap();
    assert_eq!((out.affected, out.affected_unknown), (3, false));
    let out = c
        .execute("DELETE FROM t WHERE a = 1", &[], &[])
        .await
        .unwrap();
    assert!(out.affected_unknown);
    let out = c
        .execute("CREATE TABLE x (a UInt8) ENGINE = Memory", &[], &[])
        .await
        .unwrap();
    assert!(!out.affected_unknown && out.rows.is_none());
}

#[tokio::test]
async fn a_format_clause_that_changes_the_output_is_refused() {
    let fake = Arc::new(FakeClickhouse::new());
    fake.on(
        "FORMAT CSV",
        FakeReply::Raw {
            format: "CSV".into(),
            body: "1,\"a\"\n".into(),
        },
    );
    let err = client(&fake, false)
        .rows(&q("SELECT 1, 'a' FORMAT CSV"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("FORMAT clause"), "{err}");
}

#[tokio::test]
async fn catalog_queries_bind_their_names() {
    let fake = Arc::new(FakeClickhouse::new());
    fake.on(
        "FROM system.tables WHERE database",
        FakeReply::rows(
            &[
                "name",
                "engine",
                "total_rows",
                "total_bytes",
                "comment",
                "is_temporary",
            ],
            &[
                "String",
                "String",
                "Nullable(UInt64)",
                "Nullable(UInt64)",
                "String",
                "UInt8",
            ],
            &[
                r#"["hits","MergeTree","100","2048","",0]"#,
                r#"["v","View",null,null,"a view",0]"#,
            ],
        ),
    );
    let c = client(&fake, true);
    let rows = c
        .catalog(catalog::TABLES, &[("db", "it's db")])
        .await
        .unwrap();
    let tables = catalog::parse_tables("it's db", &rows);
    assert_eq!(tables[0].row_estimate, Some(100));
    assert_eq!(tables[1].kind, TableKind::View);
    assert_eq!(tables[1].schema.as_deref(), Some("it's db"));
    let req = &fake.requests()[0];
    assert_eq!(req.param("param_db").as_deref(), Some("it's db"));
    assert!(req.sql().contains("{db:String}"));
}

#[tokio::test]
async fn ping_needs_no_credentials() {
    let mut fake = FakeClickhouse::new();
    fake.credentials = Some(("x".into(), "y".into()));
    let fake = Arc::new(fake);
    client(&fake, false).ping().await.unwrap();
    assert_eq!(fake.requests()[0].header(header::USER), None);
}

// --- Over a socket ---------------------------------------------------------------------------

fn socket_client(server: &FakeServer, read_only: bool) -> Client {
    Client::new(
        Arc::new(ReqwestTransport::new()),
        Endpoint {
            tls: false,
            host: "127.0.0.1".into(),
            port: server.addr().port(),
        },
        creds(),
        ConnSettings {
            read_only,
            output_settings: true,
            ..ConnSettings::default()
        },
    )
}

#[tokio::test]
async fn over_http_the_headers_and_a_chunked_error_arrive() {
    let tag = "0123456789abcdef";
    let text = exception_text(241, "Memory limit (total) exceeded");
    let server = FakeServer::start(vec![
        Route::new("POST", "/").reply(
            Reply::ok()
                .header("x-clickhouse-exception-tag", tag)
                .header("x-clickhouse-format", OUTPUT_FORMAT)
                .header("x-clickhouse-timezone", "Asia/Jakarta")
                .chunked(vec![
                    Piece::Data(b"[\"n\"]\n[\"UInt64\"]\n[\"1\"]\n[\"2".to_vec()),
                    Piece::Wait(Duration::from_millis(20)),
                    Piece::Data(exception_block(tag, &text).into_bytes()),
                ]),
        ),
        Route::new("GET", "/ping").reply(Reply::text(200, "Ok.\n")),
    ])
    .await;
    let c = socket_client(&server, true);
    let mut sink = CollectSink::default();
    let err = c
        .run(
            &Query {
                sql: "SELECT number FROM numbers(1e9) WHERE name = {p1:String}",
                params: &[Value::Text("a b&c\td".into())],
                ..Query::default()
            },
            &mut sink,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AppError::Db {
                vendor_code: Some(241),
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(sink.sets[0].rows, [vec![Value::Int(1)]]);
    c.ping().await.unwrap();
    let reqs = server
        .wait_until(Duration::from_secs(5), |r| r.len() == 2)
        .await;
    let post = &reqs[0];
    assert_eq!(post.header("x-clickhouse-user"), Some("alice"));
    assert_eq!(post.header("x-clickhouse-key"), Some("secret"));
    assert_eq!(post.header("x-clickhouse-format"), Some(OUTPUT_FORMAT));
    assert_eq!(
        post.text(),
        "SELECT number FROM numbers(1e9) WHERE name = {p1:String}"
    );
    let query = post.query.clone().unwrap_or_default();
    assert!(query.starts_with("readonly=2&"), "{query}");
    assert!(query.contains("param_p1=a%20b%26c%5Ctd"), "{query}");
    assert!(!query.contains("secret"));
}

#[tokio::test]
async fn over_http_an_error_status_carries_the_servers_code() {
    let server = FakeServer::start(vec![Route::new("POST", "/").reply(
        Reply::text(
            500,
            "Code: 62. DB::Exception: Syntax error: failed at position 8. (SYNTAX_ERROR) (version 25.8.1.1)\n",
        )
        .header("x-clickhouse-exception-code", "62"),
    )])
    .await;
    let err = socket_client(&server, false)
        .rows(&q("SELECT FROM"))
        .await
        .unwrap_err();
    match err {
        AppError::Db {
            vendor_code,
            message,
            ..
        } => {
            assert_eq!(vendor_code, Some(62));
            assert_eq!(
                message,
                "Syntax error: failed at position 8. (SYNTAX_ERROR)"
            );
        }
        other => panic!("{other:?}"),
    }
}
