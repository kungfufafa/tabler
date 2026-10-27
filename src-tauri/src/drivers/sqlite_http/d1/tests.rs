//! The D1 client against the in-process REST API (`testing::FakeD1`), and its wire format
//! through reqwest against `testkit::fake_http`.

use std::sync::Arc;

use serde_json::json;

use super::api::{D1Target, MAX_PARAMS};
use super::client::{D1Client, list_databases, resolve_database};
use super::decode::ParamEncoding;
use super::testing::{ACCOUNT, API_BASE, FakeD1, MAIN_DB};
use crate::drivers::TxStatement;
use crate::drivers::sqlite_http::catalog::{
    Remote, completion_query, list_tables_query, parse_completion, parse_table_meta, parse_tables,
    table_meta_queries,
};
use crate::drivers::sqlite_http::testing::ReqwestTransport;
use crate::drivers::sqlite_http::transport::{HttpResponse, Transport};
use crate::error::AppError;
use crate::testkit::{FakeServer, Reply, Route};
use crate::types::schema::{TableRef, ValueKind};
use crate::types::value::Value;

fn client(fake: &Arc<FakeD1>, encoding: ParamEncoding) -> D1Client {
    D1Client::new(
        fake.clone() as Arc<dyn Transport>,
        D1Target::new(Some(API_BASE), ACCOUNT, MAIN_DB).unwrap(),
        encoding,
    )
}

fn seeded() -> Arc<FakeD1> {
    let fake = Arc::new(FakeD1::new());
    fake.seed(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT UNIQUE, n INTEGER, price REAL, ok BOOLEAN, data BLOB);
         INSERT INTO t (id, v, n, price, ok, data) VALUES
           (1, 'a', 9007199254740993, 2.0, 1, x'0001ff'), (2, 'b', 5, 2.5, 0, NULL), (3, 'c', NULL, NULL, NULL, NULL);
         CREATE TABLE d1_migrations (id INTEGER PRIMARY KEY, name TEXT);",
    );
    fake
}

fn count(fake: &FakeD1) -> i64 {
    fake.with_db(MAIN_DB, |c| {
        c.query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap()
    })
}

#[tokio::test]
async fn databases_are_listed_page_by_page_and_names_resolve() {
    let fake = seeded();
    let analytics = fake.add_database("analytics");
    fake.add_database("other");
    let all = list_databases(fake.as_ref(), API_BASE, ACCOUNT, None, 2)
        .await
        .unwrap();
    let names: Vec<&str> = all.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["main-db", "analytics", "other"]);
    let pages: Vec<String> = fake
        .requests()
        .iter()
        .map(|r| r.query().unwrap_or_default().to_string())
        .collect();
    assert_eq!(pages, ["page=1&per_page=2", "page=2&per_page=2"]);

    assert_eq!(
        resolve_database(fake.as_ref(), API_BASE, ACCOUNT, "analytics")
            .await
            .unwrap(),
        analytics
    );
    let before = fake.requests().len();
    assert_eq!(
        resolve_database(fake.as_ref(), API_BASE, ACCOUNT, MAIN_DB)
            .await
            .unwrap(),
        MAIN_DB
    );
    assert_eq!(fake.requests().len(), before, "a UUID needs no lookup");
    assert!(matches!(
        resolve_database(fake.as_ref(), API_BASE, ACCOUNT, "missing").await,
        Err(AppError::NotFound(_))
    ));

    let c = client(&fake, ParamEncoding::Native);
    let info = c.database().await.unwrap();
    assert_eq!(
        (info.name.as_str(), info.version.as_deref()),
        ("main-db", Some("production"))
    );
    assert_eq!(c.list_databases(Some("other")).await.unwrap().len(), 1);
    let gone = D1Client::new(
        fake.clone(),
        c.target()
            .with_database("00000000-0000-4000-8000-000000000000"),
        ParamEncoding::Native,
    );
    assert!(matches!(gone.database().await, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn values_as_d1_sends_them() {
    let fake = seeded();
    let c = client(&fake, ParamEncoding::Native);
    let kinds = [
        ValueKind::Int,
        ValueKind::Text,
        ValueKind::Int,
        ValueKind::Float,
        ValueKind::Bool,
        ValueKind::Bytes,
    ];
    let set = c
        .query(
            "SELECT id, v, n, price, ok, data FROM t ORDER BY id",
            &[],
            Some(&kinds),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        set.rows[0],
        [
            Value::Int(1),
            Value::Text("a".into()),
            // Rounded by D1's JavaScript on the way.
            Value::BigInt("9007199254740992".into()),
            Value::Float(2.0),
            Value::Bool(true),
            Value::from_bytes(&[0, 1, 255])
        ]
    );
    assert_eq!(set.rows[1][3], Value::Float(2.5));
    assert_eq!(set.rows[2][2], Value::Null);
    // Read as text beyond 2^53, the integer is exact.
    let exact = c
        .query(
            "SELECT CASE WHEN typeof(n) = 'integer' AND abs(n) > 9007199254740991 THEN CAST(n AS TEXT) ELSE n END FROM t ORDER BY id",
            &[],
            Some(&[ValueKind::Int]),
            Some(2),
        )
        .await
        .unwrap();
    assert_eq!(
        exact.rows,
        [[Value::BigInt("9007199254740993".into())], [Value::Int(5)]]
    );
    assert!(exact.truncated);
    // Without kinds (an editor result) a whole REAL shows as an integer, and both id columns
    // of a join stay.
    let joined = c
        .query(
            "SELECT a.id, b.id FROM t a JOIN t b ON b.id = a.id + 1 ORDER BY a.id",
            &[],
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(joined.columns.len(), 2);
    assert_eq!(joined.rows[0], [Value::Int(1), Value::Int(2)]);
}

#[tokio::test]
async fn parameters_native_or_as_text() {
    let fake = seeded();
    let native = client(&fake, ParamEncoding::Native);
    let out = native
        .execute(
            "INSERT INTO t (v, n, price, ok) VALUES (?1, ?2, ?3, ?4)",
            &[
                Value::Text("d".into()),
                Value::BigInt("9007199254740993".into()),
                Value::Float(1.25),
                Value::Bool(true),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        (out.affected, out.last_insert_id.as_deref()),
        (1, Some("4"))
    );
    let stored: (String, i64, f64) = fake.with_db(MAIN_DB, |c| {
        c.query_row("SELECT typeof(n), n, price FROM t WHERE v = 'd'", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap()
    });
    assert_eq!(stored, ("integer".into(), 9_007_199_254_740_993, 1.25));

    // A server that takes text only: numbers go as text, which the column's affinity converts.
    fake.strict_params(true);
    let refused = native
        .execute(
            "UPDATE t SET n = ?1 WHERE id = ?2",
            &[Value::Int(7), Value::Int(2)],
        )
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("params must be strings"),
        "{refused}"
    );
    let text = client(&fake, ParamEncoding::Strings);
    let out = text
        .execute(
            "UPDATE t SET n = ?1, price = ?2 WHERE id = ?3",
            &[Value::Int(7), Value::Float(0.5), Value::Int(2)],
        )
        .await
        .unwrap();
    assert_eq!(out.affected, 1);
    let n: (String, i64) = fake.with_db(MAIN_DB, |c| {
        c.query_row("SELECT typeof(n), n FROM t WHERE id = 2", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap()
    });
    assert_eq!(n, ("integer".into(), 7));

    // Blobs and too many parameters never leave the client.
    let before = fake.requests().len();
    assert!(matches!(
        text.execute("UPDATE t SET data = ?1", &[Value::from_bytes(&[1])])
            .await,
        Err(AppError::Unsupported(_))
    ));
    let many = vec![Value::Int(1); MAX_PARAMS + 1];
    assert!(matches!(
        text.query("SELECT 1", &many, None, None).await,
        Err(AppError::InvalidInput(_))
    ));
    assert_eq!(fake.requests().len(), before);
}

#[tokio::test]
async fn the_catalog_is_one_batch_without_cf_kv() {
    let fake = seeded();
    fake.seed("CREATE INDEX t_n ON t (n) WHERE n > 0; CREATE VIEW tv AS SELECT id FROM t;");
    let c = client(&fake, ParamEncoding::Native);
    let tables = c.catalog(&[list_tables_query(Remote::D1)]).await.unwrap();
    let names: Vec<String> = parse_tables(&tables[0])
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["d1_migrations", "t", "tv"]);

    let t = TableRef::new(Some("main"), "t");
    let before = fake.requests().len();
    let rows = c.catalog(&table_meta_queries(&t)).await.unwrap();
    assert_eq!(fake.requests().len(), before + 1);
    let body = fake.requests().pop().unwrap().json().unwrap();
    assert_eq!(body["batch"].as_array().unwrap().len(), 5);
    let meta = parse_table_meta(&t, &rows).unwrap();
    assert_eq!(meta.primary_key, ["id"]);
    assert_eq!(meta.columns[3].kind, ValueKind::Float);
    assert_eq!(meta.columns[0].extra.as_deref(), Some("rowid alias"));
    let partial = meta.indexes.iter().find(|i| i.name == "t_n").unwrap();
    assert_eq!(partial.predicate.as_deref(), Some("n > 0"));
    assert!(meta.indexes.iter().any(|i| i.unique && i.columns == ["v"]));

    let completion = c.catalog(&[completion_query(Remote::D1)]).await.unwrap();
    let meta = parse_completion(&completion[0]).unwrap();
    assert_eq!(meta.schemas["main"]["tv"], ["id"]);
    assert!(!meta.schemas["main"].contains_key("_cf_KV"));
}

#[tokio::test]
async fn staged_commits_are_one_guarded_batch() {
    let fake = seeded();
    let c = client(&fake, ParamEncoding::Native);
    let out = c
        .transaction(&[
            TxStatement::new(
                "UPDATE t SET v = ?1 WHERE id = ?2",
                vec![Value::Text("z".into()), Value::Int(1)],
            )
            .expecting(1),
            TxStatement::new("INSERT INTO t (v) VALUES ('d') RETURNING id", vec![]),
        ])
        .await
        .unwrap();
    assert_eq!(out[0].affected, 1);
    assert_eq!(out[1].rows.as_ref().unwrap().rows, [[Value::Int(4)]]);
    assert_eq!(count(&fake), 4);
    let body = fake.requests().pop().unwrap().json().unwrap();
    assert_eq!(
        body["batch"].as_array().unwrap().len(),
        3,
        "a guard after the update"
    );

    // A guard that fails rolls the whole batch back and names its statement.
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

    // So does any failing statement.
    let e = c
        .transaction(&[
            TxStatement::new("DELETE FROM t WHERE id = 2", vec![]).expecting(1),
            TxStatement::new("INSERT INTO t (v) VALUES ('c')", vec![]),
        ])
        .await
        .unwrap_err();
    assert!(
        matches!(&e, AppError::Db { vendor_code: Some(2067), message, .. } if message.contains("UNIQUE")),
        "{e:?}"
    );
    assert_eq!(count(&fake), 4);
}

#[tokio::test]
async fn cascades_and_triggers_do_not_fail_a_committed_batch() {
    let fake = seeded();
    fake.seed(
        "CREATE TABLE parent (id INTEGER PRIMARY KEY, name TEXT);
         CREATE TABLE child (id INTEGER PRIMARY KEY,
           parent_id INTEGER REFERENCES parent (id) ON DELETE CASCADE ON UPDATE CASCADE);
         CREATE TABLE audit (what TEXT);
         CREATE TRIGGER parent_audit AFTER UPDATE ON parent
           BEGIN INSERT INTO audit VALUES ('updated ' || new.id); END;
         INSERT INTO parent VALUES (1, 'a'), (2, 'b');
         INSERT INTO child (parent_id) VALUES (1), (1), (2);",
    );
    let c = client(&fake, ParamEncoding::Native);
    // `meta.changes` of the delete is 3 (the cascade) and of the update 2 (the trigger), while
    // each guard sees `changes()` = 1.
    let out = c
        .transaction(&[
            TxStatement::new("DELETE FROM parent WHERE id = ?1", vec![Value::Int(1)]).expecting(1),
            TxStatement::new(
                "UPDATE parent SET name = 'z' WHERE id = ?1",
                vec![Value::Int(2)],
            )
            .expecting(1),
            TxStatement::new("INSERT INTO child (parent_id) VALUES (2)", vec![]),
        ])
        .await
        .unwrap();
    assert_eq!(
        out.iter().map(|o| o.affected).collect::<Vec<_>>(),
        [1, 1, 1]
    );
    let counts: (i64, i64, i64) = fake.with_db(MAIN_DB, |c| {
        c.query_row(
            "SELECT (SELECT count(*) FROM parent), (SELECT count(*) FROM child), \
             (SELECT count(*) FROM audit)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    });
    assert_eq!(counts, (1, 2, 1), "the batch committed");

    // Outside a staged commit the count stays D1's rough one.
    let rough = c
        .execute("DELETE FROM parent WHERE id = ?1", &[Value::Int(2)])
        .await
        .unwrap();
    assert_eq!(rough.affected, 3);
}

#[tokio::test]
async fn what_d1_refuses_comes_back_as_errors() {
    let fake = seeded();
    let c = client(&fake, ParamEncoding::Native);
    for sql in [
        "BEGIN",
        "PRAGMA database_list",
        "SELECT * FROM _cf_KV",
        "ATTACH 'x' AS y",
    ] {
        let e = c.execute(sql, &[]).await.unwrap_err();
        assert!(
            matches!(
                e,
                AppError::Db {
                    vendor_code: Some(23),
                    ..
                }
            ),
            "{sql}: {e:?}"
        );
    }
    match c.execute("SELEC 1", &[]).await.unwrap_err() {
        AppError::Db {
            position, message, ..
        } => {
            assert!(message.contains("syntax error"), "{message}");
            assert_eq!(position.map(|p| p.offset), Some(0));
        }
        other => panic!("{other:?}"),
    }
    c.execute("PRAGMA foreign_keys = ON", &[]).await.unwrap();
}

#[tokio::test]
async fn refused_tokens_rate_limits_and_broken_replies() {
    let fake = seeded();
    let c = client(&fake, ParamEncoding::Native);
    fake.answer_status(Some(403));
    assert!(matches!(c.database().await, Err(AppError::Auth(_))));
    fake.answer_status(None);
    fake.fail_next(HttpResponse {
        retry_after: Some(60),
        ..HttpResponse::json(
            429,
            &json!({"success": false, "errors": [{"code": 971, "message": "Please wait and consider throttling your request speed"}]}),
        )
    });
    match c.execute("SELECT 1", &[]).await.unwrap_err() {
        AppError::Db { hint, .. } => assert_eq!(hint.as_deref(), Some("Retry in 60 s.")),
        other => panic!("{other:?}"),
    }
    fake.fail_next(HttpResponse::json(
        200,
        &json!({"success": true, "errors": [], "result": [{"success": false}]}),
    ));
    assert!(c.execute("SELECT 1", &[]).await.is_err());
    fake.fail_next(HttpResponse::json(
        200,
        &json!({"success": true, "result": []}),
    ));
    assert!(c.execute("SELECT 1", &[]).await.is_err());
    fake.fail_next(HttpResponse::json(
        200,
        &json!({"success": true, "result": []}),
    ));
    assert!(c.catalog(&[list_tables_query(Remote::D1)]).await.is_err());
    let wrong = D1Client::new(
        fake.clone(),
        D1Target::new(Some(API_BASE), "ffffffffffffffffffffffffffffffff", MAIN_DB).unwrap(),
        ParamEncoding::Native,
    );
    assert!(matches!(
        wrong.execute("SELECT 1", &[]).await,
        Err(AppError::Auth(_))
    ));
}

#[tokio::test]
async fn the_wire_format_over_http() {
    let reply = json!({"success": true, "errors": [], "messages": [], "result": [{
        "results": {"columns": ["x"], "rows": [[1]]}, "meta": {"changes": 0}, "success": true}]});
    let path = format!("/client/v4/accounts/{ACCOUNT}/d1/database/{MAIN_DB}/raw");
    let server = FakeServer::start(vec![
        Route::new("POST", &path)
            .auth("Bearer cf-token")
            .reply(Reply::json(200, &reply)),
    ])
    .await;
    let c = D1Client::new(
        Arc::new(ReqwestTransport::new(Some("cf-token"))),
        D1Target::new(Some(&server.url("/client/v4")), ACCOUNT, MAIN_DB).unwrap(),
        ParamEncoding::Native,
    );
    let set = c
        .query("SELECT ?1 AS x", &[Value::Int(1)], None, None)
        .await
        .unwrap();
    assert_eq!(set.rows, [[Value::Int(1)]]);
    let req = server.requests().pop().unwrap();
    assert_eq!(req.header("authorization"), Some("Bearer cf-token"));
    assert_eq!(req.header("content-type"), Some("application/json"));
    assert_eq!(req.json(), json!({"sql": "SELECT ?1 AS x", "params": [1]}));
}
