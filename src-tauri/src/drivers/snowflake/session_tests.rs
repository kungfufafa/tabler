//! The session against the scripted Snowflake of `testing`: connect, introspection, the grid's
//! pages, counts and cells, commits, the editor, cancel, the time out, expired sessions and
//! read-only refusals on every write path.

use std::sync::Arc;
use std::time::Duration;

use super::session::{SnowflakeSession, capabilities};
use super::signin::SignIn;
use super::testing::{Answer, FakeColumn, FakeSnowflake, Login, fixture_path};
use crate::commands::browse::{browse_page, fetch_cell_inner};
use crate::commands::query::query_limited;
use crate::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, TxStatement, request_cancel,
};
use crate::error::AppError;
use crate::sqlgen::dialect::snowflake as sf;
use crate::sqlgen::narrow::Narrowing;
use crate::sqlgen::sql;
use crate::types::admin::KillScope;
use crate::types::browse::{BrowseRequest, RowKey};
use crate::types::caps::{CountMode, Feature};
use crate::types::driver::ReadOnlyMode;
use crate::types::objects::{RoutineKind, RoutineRef};
use crate::types::profile::ConnectionProfile;
use crate::types::query::StatementKind;
use crate::types::schema::{TableKind, TableRef, ValueKind};
use crate::types::value::Value;

const ACCOUNT: &str = "myorg-acct";
const USER: &str = "tabler_user";

fn profile(
    fake: &FakeSnowflake,
    params: serde_json::Value,
    options: serde_json::Value,
) -> ConnectionProfile {
    let mut driver_params = serde_json::json!({
        "account": ACCOUNT,
        "privateKeyPath": fixture_path("rsa_key.p8"),
        "baseUrl": fake.url(),
    });
    for (k, v) in params.as_object().expect("params") {
        driver_params[k] = v.clone();
    }
    let mut opts = serde_json::json!({ "driverParams": driver_params });
    for (k, v) in options.as_object().expect("options") {
        opts[k] = v.clone();
    }
    serde_json::from_value(serde_json::json!({
        "id": uuid::Uuid::new_v4(), "name": "sf", "driver": "snowflake",
        "user": USER, "database": "DB", "options": opts,
    }))
    .expect("a profile")
}

async fn fake(login: Login) -> FakeSnowflake {
    crate::testkit::install_crypto();
    let fake = FakeSnowflake::start(ACCOUNT, USER, login).await;
    fake.server_info();
    fake.editor_state(Some("DB"), None);
    fake.status(&sf::PIN_SESSION);
    fake
}

async fn open_with(
    fake: &FakeSnowflake,
    params: serde_json::Value,
    options: serde_json::Value,
    password: Option<&str>,
) -> Result<SnowflakeSession, AppError> {
    let p = profile(fake, params, options);
    let target = ConnectTarget {
        profile: &p,
        password,
        host_override: None,
    };
    SnowflakeSession::open(&target).await
}

async fn open(fake: &FakeSnowflake) -> SnowflakeSession {
    open_with(fake, serde_json::json!({}), serde_json::json!({}), None)
        .await
        .expect("the session opens")
}

async fn read_only(fake: &FakeSnowflake) -> SnowflakeSession {
    open_with(
        fake,
        serde_json::json!({}),
        serde_json::json!({ "readOnly": true }),
        None,
    )
    .await
    .expect("the session opens")
}

/// Runs `f`, and when the fake knew no answer, answers the statement it got with `answer` and
/// runs `f` again: the test then needs no copy of the SQL the app builds.
async fn learn<T, F, Fut>(fake: &FakeSnowflake, answer: Answer, f: F) -> Result<T, AppError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, AppError>>,
{
    match f().await {
        Err(AppError::Db { message, .. }) if message.contains("knows no answer") => {
            let (_, sql) = fake.sent().pop().expect("a statement was sent");
            fake.answer(&sql, answer);
            f().await
        }
        other => other,
    }
}

const SHOW_TABLES_COLS: &[&str] = &[
    "created_on",
    "name",
    "database_name",
    "schema_name",
    "kind",
    "comment",
    "rows",
    "bytes",
    "is_external",
    "is_hybrid",
];

/// The fake knows table DB.PUBLIC.ITEMS (ID primary key, NAME, PRICE, DOC) and view V_ITEMS.
fn describe_items(fake: &FakeSnowflake) {
    fake.show(
        &sf::show_table_like("DB", "PUBLIC", "ITEMS"),
        SHOW_TABLES_COLS,
        &[&[
            Some("2024-01-01"),
            Some("ITEMS"),
            Some("DB"),
            Some("PUBLIC"),
            Some("TABLE"),
            Some("the items"),
            Some("2500000"),
            Some("1048576"),
            Some("N"),
            Some("N"),
        ]],
    );
    let col = |name: &str, ty: &str, null: &str| -> [Option<String>; 6] {
        [
            Some("ITEMS".into()),
            Some(name.into()),
            Some(ty.into()),
            Some(null.into()),
            None,
            None,
        ]
    };
    let rows = [
        col(
            "ID",
            r#"{"type":"FIXED","precision":38,"scale":0,"nullable":false}"#,
            "false",
        ),
        col(
            "NAME",
            r#"{"type":"TEXT","length":100,"nullable":true}"#,
            "true",
        ),
        col(
            "PRICE",
            r#"{"type":"FIXED","precision":10,"scale":2,"nullable":true}"#,
            "true",
        ),
        col("DOC", r#"{"type":"VARIANT","nullable":true}"#, "true"),
    ];
    let refs: Vec<Vec<Option<&str>>> = rows
        .iter()
        .map(|r| r.iter().map(|c| c.as_deref()).collect())
        .collect();
    let slices: Vec<&[Option<&str>]> = refs.iter().map(Vec::as_slice).collect();
    fake.show(
        &sf::show_in_table(sf::TableObjects::Columns, "DB", "PUBLIC", "ITEMS"),
        &[
            "table_name",
            "column_name",
            "data_type",
            "null?",
            "default",
            "autoincrement",
        ],
        &slices,
    );
    fake.show(
        &sf::show_in_table(sf::TableObjects::PrimaryKeys, "DB", "PUBLIC", "ITEMS"),
        &["column_name", "key_sequence", "constraint_name"],
        &[&[Some("ID"), Some("1"), Some("PK_ITEMS")]],
    );
    fake.show(
        &sf::show_in_table(sf::TableObjects::UniqueKeys, "DB", "PUBLIC", "ITEMS"),
        &["column_name", "key_sequence", "constraint_name"],
        &[&[Some("NAME"), Some("1"), Some("UQ_NAME")]],
    );
    fake.show(
        &sf::show_in_table(sf::TableObjects::ImportedKeys, "DB", "PUBLIC", "ITEMS"),
        &[
            "pk_schema_name",
            "pk_table_name",
            "pk_column_name",
            "fk_column_name",
            "key_sequence",
            "fk_name",
        ],
        &[],
    );
}

#[tokio::test]
async fn connects_twice_and_reads_where_it_stands() {
    let fake = fake(Login::key_pair()).await;
    let s = open(&fake).await;
    // The editor's session and the app's: two sign-ins.
    assert_eq!(fake.logins().len(), 2);
    let info = s.server_info().await.unwrap();
    assert_eq!(info.version, "Snowflake 9.30.1");
    assert_eq!(info.current_database.as_deref(), Some("DB"));
    assert_eq!(info.current_user.as_deref(), Some("TABLER_USER"));
    // SERVER_INFO ran on the editor's session at open, on the app's after.
    let sent = fake.sent();
    assert_eq!(sent[0], (1, sf::SERVER_INFO.to_string()));
    assert_eq!(sent[1], (2, sf::SERVER_INFO.to_string()));
    // Ping: SELECT 1, no warehouse.
    fake.show(sf::PING, &["1"], &[&[Some("1")]]);
    s.ping().await.unwrap();
    let details = s.session_details().await.unwrap().unwrap();
    // The fake is plain http on this machine.
    assert!(!details.tls);
    assert_eq!(details.encoding.as_deref(), Some("UTF-8"));
    s.close().await;
}

#[tokio::test]
async fn caps_grant_what_the_driver_does() {
    let caps = capabilities(Some("PUBLIC".into()));
    for f in [
        Feature::Editor,
        Feature::Explain,
        Feature::CancelOnServer,
        Feature::Format,
        Feature::EditRows,
        Feature::InsertRows,
        Feature::DeleteRows,
        Feature::ExactCount,
        Feature::StructureView,
        Feature::TableDdl,
        Feature::ViewDefinition,
        Feature::Routines,
        Feature::SwitchDatabase,
        Feature::ProcessList,
        Feature::KillProcess,
        Feature::SessionDetails,
        Feature::StartupCommands,
        Feature::Mcp,
    ] {
        assert!(caps.has(f), "{f:?}");
    }
    for f in [
        Feature::Returning,
        Feature::EditWithoutKey,
        Feature::ServerExpressions,
        Feature::StructureEdit,
        Feature::ExplainAnalyze,
        Feature::Users,
        Feature::Sequences,
        Feature::Backup,
        Feature::CopyAsInsert,
    ] {
        assert!(!caps.has(f), "{f:?}");
    }
    assert_eq!(caps.data.count, CountMode::Costly);
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    assert!(!caps.safety.hardened);
    assert_eq!(caps.session.keepalive_secs, None);
    assert!(!caps.session.ping_billed);
    assert_eq!(caps.namespaces.default_schema.as_deref(), Some("PUBLIC"));
    assert_eq!(caps.data.max_params, 1000);
    // The descriptor says the same about read-only.
    assert_eq!(
        crate::types::profile::Driver::Snowflake
            .descriptor()
            .read_only,
        ReadOnlyMode::ClassifierOnly
    );
}

#[tokio::test]
async fn lists_and_describes_through_show() {
    let fake = fake(Login::key_pair()).await;
    let s = open(&fake).await;
    fake.show(
        sf::SHOW_DATABASES,
        &["name", "owner", "is_current"],
        &[
            &[Some("DB"), Some("SYSADMIN"), Some("N")],
            &[Some("OTHER"), None, Some("N")],
        ],
    );
    let dbs = s.list_databases().await.unwrap();
    assert_eq!(dbs.len(), 2);
    assert!(dbs[0].is_current && !dbs[1].is_current);

    fake.show(
        &sf::show_schemas("DB", None),
        &["name", "is_current"],
        &[
            &[Some("INFORMATION_SCHEMA"), Some("N")],
            &[Some("PUBLIC"), Some("N")],
        ],
    );
    let schemas = s.list_schemas().await.unwrap();
    assert!(schemas.iter().any(|x| x.name == "PUBLIC" && x.is_default));

    fake.show(
        &sf::show_in_schema(sf::SchemaObjects::Tables, "DB", "PUBLIC", None),
        SHOW_TABLES_COLS,
        &[&[
            None,
            Some("ITEMS"),
            Some("DB"),
            Some("PUBLIC"),
            Some("TABLE"),
            None,
            Some("3"),
            Some("2048"),
            Some("N"),
            Some("N"),
        ]],
    );
    fake.show(
        &sf::show_in_schema(sf::SchemaObjects::Views, "DB", "PUBLIC", None),
        &["name", "schema_name", "is_materialized", "text"],
        &[&[
            Some("V_ITEMS"),
            Some("PUBLIC"),
            Some("false"),
            Some("create view V_ITEMS as select 1"),
        ]],
    );
    let tables = s.list_tables("PUBLIC").await.unwrap();
    assert_eq!(tables.len(), 2);
    assert_eq!(tables[0].row_estimate, Some(3));
    assert_eq!(tables[1].kind, TableKind::View);

    describe_items(&fake);
    let meta = s
        .table_meta(&TableRef {
            schema: Some("PUBLIC".into()),
            name: "ITEMS".into(),
        })
        .await
        .unwrap();
    assert_eq!(meta.kind, TableKind::Table);
    assert_eq!(meta.primary_key, vec!["ID"]);
    assert_eq!(meta.row_estimate, Some(2_500_000));
    assert_eq!(meta.comment.as_deref(), Some("the items"));
    assert_eq!(meta.columns.len(), 4);
    assert_eq!(meta.columns[0].kind, ValueKind::BigInt);
    assert!(meta.columns[0].is_pk && !meta.columns[0].nullable);
    assert_eq!(meta.columns[2].data_type, "NUMBER(10,2)");
    assert_eq!(meta.columns[3].kind, ValueKind::Json);
    assert_eq!(meta.indexes.len(), 2);

    fake.show(
        &sf::show_views_like("DB", "PUBLIC", "V_ITEMS"),
        &["name", "is_materialized", "text"],
        &[&[
            Some("V_ITEMS"),
            Some("false"),
            Some("create view V_ITEMS as select 1"),
        ]],
    );
    let view = TableRef {
        schema: Some("PUBLIC".into()),
        name: "V_ITEMS".into(),
    };
    assert_eq!(
        s.view_definition(&view).await.unwrap().as_deref(),
        Some("create view V_ITEMS as select 1")
    );

    // Show DDL: GET_DDL of the table.
    fake.show(
        &sf::get_ddl(
            sf::DdlKind::Table,
            &sf::qualified(&["DB", "PUBLIC", "ITEMS"]),
        ),
        &["GET_DDL"],
        &[&[Some(
            "create or replace TABLE ITEMS (ID NUMBER(38,0) NOT NULL);",
        )]],
    );
    let ddl = s
        .table_ddl(&TableRef {
            schema: Some("PUBLIC".into()),
            name: "ITEMS".into(),
        })
        .await
        .unwrap()
        .unwrap();
    assert!(ddl.starts_with("create or replace TABLE ITEMS"));

    // Completion: the session's schema.
    fake.show(
        &sf::show_in_schema(sf::SchemaObjects::Columns, "DB", "PUBLIC", None),
        &["table_name", "column_name"],
        &[&[Some("ITEMS"), Some("ID")], &[Some("ITEMS"), Some("NAME")]],
    );
    let completion = s.completion_metadata().await.unwrap();
    assert_eq!(completion.schemas["PUBLIC"]["ITEMS"], vec!["ID", "NAME"]);

    // Routines and their DDL by argument types.
    fake.show(
        &sf::show_in_schema(sf::SchemaObjects::UserFunctions, "DB", "PUBLIC", None),
        &["name", "schema_name", "arguments", "is_builtin", "language"],
        &[&[
            Some("ADD_ONE"),
            Some("PUBLIC"),
            Some("ADD_ONE(NUMBER) RETURN NUMBER"),
            Some("N"),
            Some("SQL"),
        ]],
    );
    fake.show(
        &sf::show_in_schema(sf::SchemaObjects::Procedures, "DB", "PUBLIC", None),
        &["name", "schema_name", "arguments", "is_builtin"],
        &[],
    );
    let routines = s.list_routines("PUBLIC").await.unwrap().unwrap();
    assert_eq!(routines.len(), 1);
    assert_eq!(routines[0].identity_arguments, "NUMBER");
    fake.show(
        &sf::get_ddl(
            sf::DdlKind::Function,
            "\"DB\".\"PUBLIC\".\"ADD_ONE\"(NUMBER)",
        ),
        &["GET_DDL"],
        &[&[Some(
            "CREATE FUNCTION ADD_ONE(X NUMBER) RETURNS NUMBER AS 'X + 1'",
        )]],
    );
    let text = s
        .routine_definition(&RoutineRef {
            schema: Some("PUBLIC".into()),
            name: "ADD_ONE".into(),
            kind: RoutineKind::Function,
            identity_arguments: "NUMBER".into(),
        })
        .await
        .unwrap()
        .unwrap();
    assert!(text.starts_with("CREATE FUNCTION"));

    // Introspection ran on the app's session, never the editor's.
    assert!(fake.sent().iter().skip(1).all(|(session, _)| *session == 2));
}

#[tokio::test]
async fn a_show_past_10000_names_pages_on() {
    let fake = fake(Login::key_pair()).await;
    let s = open(&fake).await;
    let names: Vec<String> = (0..sf::SHOW_PAGE).map(|i| format!("S{i:05}")).collect();
    let first: Vec<Vec<Option<&str>>> = names.iter().map(|n| vec![Some(n.as_str())]).collect();
    let first: Vec<&[Option<&str>]> = first.iter().map(Vec::as_slice).collect();
    fake.show(&sf::show_schemas("DB", None), &["name"], &first);
    fake.show(
        &sf::show_schemas("DB", Some("S09999")),
        &["name"],
        &[&[Some("ZZ")]],
    );
    let schemas = s.list_schemas().await.unwrap();
    assert_eq!(schemas.len(), sf::SHOW_PAGE as usize + 1);
    assert_eq!(schemas.last().unwrap().name, "ZZ");
}

fn items() -> TableRef {
    TableRef {
        schema: Some("PUBLIC".into()),
        name: "ITEMS".into(),
    }
}

#[tokio::test]
async fn browses_with_paging_counts_and_fetches_a_cell() {
    let fake = fake(Login::key_pair()).await;
    let s = open(&fake).await;
    describe_items(&fake);
    let meta = s.table_meta(&items()).await.unwrap();
    let columns = vec![
        FakeColumn::new("ID", "fixed").precision(38, 0),
        FakeColumn::new("NAME", "text").length(100),
        FakeColumn::new("PRICE", "fixed").precision(10, 2),
        FakeColumn::new("DOC", "variant"),
    ];
    let page = |id: &str| {
        vec![
            Some(id.to_string()),
            Some("pen".to_string()),
            Some("1.50".to_string()),
            Some("{\"a\":[1,2]}".to_string()),
        ]
    };
    let req = BrowseRequest {
        table: items(),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit: 2,
        offset: 2,
        cursor: None,
    };
    let got = learn(
        &fake,
        Answer::rows(columns.clone(), vec![page("9007199254740993"), page("4")]),
        || browse_page(&s, &meta, &req, None),
    )
    .await
    .unwrap();
    let (_, sql) = fake.sent().pop().unwrap();
    assert!(sql.contains("LIMIT 2 OFFSET 2"), "{sql}");
    assert!(sql.contains("\"PUBLIC\".\"ITEMS\""), "{sql}");
    assert_eq!(got.rows.len(), 2);
    assert_eq!(got.rows[0][0], Value::BigInt("9007199254740993".into()));
    assert_eq!(got.rows[0][2], Value::Decimal("1.50".into()));
    assert_eq!(got.rows[0][3], Value::Json("{\"a\":[1,2]}".into()));
    assert_eq!(got.rows[1][0], Value::Int(4));

    // An exact count, as the count button asks it.
    let built = sql::count_on(
        &Narrowing::for_caps(&s.capabilities()),
        s.dialect(),
        &meta.table,
        &meta.columns,
        None,
    )
    .unwrap();
    fake.answer(
        &built.sql,
        Answer::rows(
            vec![FakeColumn::new("COUNT(*)", "fixed").precision(18, 0)],
            vec![vec![Some("2500017".into())]],
        ),
    );
    let rs = s.query(&built.sql, &built.params).await.unwrap();
    assert_eq!(rs.rows[0][0], Value::Int(2_500_017));

    // One whole cell by its key.
    let key = RowKey::Pk([("ID".to_string(), Value::Int(4))].into_iter().collect());
    let cell = learn(
        &fake,
        Answer::rows(
            vec![FakeColumn::new("DOC", "variant")],
            vec![vec![Some("{\"big\":true}".into())]],
        ),
        || fetch_cell_inner(&s, &meta, "DOC", &key),
    )
    .await
    .unwrap();
    assert_eq!(cell, Value::Json("{\"big\":true}".into()));
    let last = fake.queries().pop().unwrap();
    assert_eq!(last.body["bindings"]["1"]["type"], "FIXED");
    assert_eq!(last.body["bindings"]["1"]["value"], "4");
}

#[tokio::test]
async fn a_commit_checks_every_count_before_it_commits() {
    let fake = fake(Login::key_pair()).await;
    let s = open(&fake).await;
    for st in [sf::BEGIN, sf::COMMIT, sf::ROLLBACK] {
        fake.status(st);
    }
    let one = "UPDATE \"PUBLIC\".\"ITEMS\" SET \"NAME\" = ? WHERE \"ID\" = ?";
    let dup = "DELETE FROM \"PUBLIC\".\"ITEMS\" WHERE \"ID\" = ?";
    fake.counts(one, "updated", 1);
    fake.counts(dup, "deleted", 2);
    let ok = s
        .transaction(vec![
            TxStatement::new(one, vec![Value::Text("ink".into()), Value::Int(4)]).expecting(1),
        ])
        .await
        .unwrap();
    assert_eq!(ok[0].affected, 1);
    let texts: Vec<String> = fake.sent().into_iter().map(|(_, s)| s).collect();
    assert_eq!(texts[texts.len() - 3..], [sf::BEGIN, one, sf::COMMIT]);

    // A key that is only informational matched two rows: everything is rolled back.
    let err = s
        .transaction(vec![
            TxStatement::new(one, vec![Value::Text("ink".into()), Value::Int(4)]).expecting(1),
            TxStatement::new(dup, vec![Value::Int(5)]).expecting(1),
        ])
        .await
        .unwrap_err();
    match &err {
        AppError::Db {
            message,
            statement_index,
            ..
        } => {
            assert!(message.contains("matched 2"), "{message}");
            assert_eq!(*statement_index, Some(1));
        }
        other => panic!("{other:?}"),
    }
    let sent = fake.sent();
    let texts: Vec<&str> = sent.iter().map(|(_, s)| s.as_str()).collect();
    assert_eq!(
        texts[texts.len() - 4..],
        [sf::BEGIN, one, dup, sf::ROLLBACK]
    );
    // All of it on the app's session.
    assert!(sent.iter().skip(2).all(|(session, _)| *session == 2));

    // A failing statement rolls back too, with its index.
    fake.answer(dup, Answer::error("100132", "JavaScript execution error"));
    let err = s
        .transaction(vec![TxStatement::new(dup, vec![Value::Int(5)])])
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AppError::Db {
                statement_index: Some(0),
                vendor_code: Some(100132),
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(fake.sent().pop().unwrap().1, sf::ROLLBACK);
    // The trace has the statements of the commits.
    let trace = s.drain_trace();
    assert!(
        trace
            .iter()
            .any(|t| t.error.as_deref().is_some_and(|e| e.contains("matched 2")))
    );
}

async fn run(
    s: &SnowflakeSession,
    sql: &str,
    max_rows: usize,
) -> Result<(crate::drivers::StatementOutcome, CollectSink), AppError> {
    let mut sink = CollectSink::default();
    let out = s
        .run_statement(sql, max_rows, &mut sink, CancelSlot::default())
        .await?;
    Ok((out, sink))
}

#[tokio::test]
async fn the_editor_keeps_its_session_and_counts_dml() {
    let fake = fake(Login::key_pair()).await;
    let s = open(&fake).await;
    let rows: Vec<Vec<Option<String>>> = (0..5).map(|i| vec![Some(i.to_string())]).collect();
    fake.answer(
        "SELECT n FROM t",
        Answer::rows(
            vec![FakeColumn::new("N", "fixed").precision(9, 0)],
            rows[..2].to_vec(),
        )
        .chunk(rows[2..].to_vec()),
    );
    let (out, sink) = run(&s, "SELECT n FROM t", 3).await.unwrap();
    assert_eq!(out.kind, StatementKind::Rows);
    assert_eq!(out.row_count, 3);
    assert!(out.truncated);
    assert_eq!(sink.sets[0].rows.len(), 3);
    let (out, _) = run(&s, "SELECT n FROM t", 100).await.unwrap();
    assert!(!out.truncated && out.row_count == 5);

    fake.counts("INSERT INTO t VALUES (1), (2)", "inserted", 2);
    let (out, sink) = run(&s, "INSERT INTO t VALUES (1), (2)", 100).await.unwrap();
    assert_eq!(out.kind, StatementKind::Affected);
    assert_eq!(out.affected, Some(2));
    assert!(sink.sets.is_empty());

    // A transaction lasts across runs, and DDL ends it.
    fake.status("BEGIN");
    fake.status("COMMIT");
    fake.status("CREATE TABLE t2 (a INT)");
    assert_eq!(s.in_transaction(), Some(false));
    run(&s, "BEGIN", 10).await.unwrap();
    assert_eq!(s.in_transaction(), Some(true));
    run(&s, "COMMIT", 10).await.unwrap();
    assert_eq!(s.in_transaction(), Some(false));
    run(&s, "BEGIN", 10).await.unwrap();
    let (ddl, sink) = run(&s, "CREATE TABLE t2 (a INT)", 10).await.unwrap();
    assert_eq!(ddl.kind, StatementKind::Rows);
    assert_eq!(sink.sets[0].columns[0].name, "status");
    assert_eq!(s.in_transaction(), Some(false));

    // Errors keep their place in the statement.
    fake.answer(
        "SELECT 1\nFORM t",
        Answer::error(
            "001003",
            "SQL compilation error:\nsyntax error line 2 at position 0 unexpected 'FORM'.",
        ),
    );
    let err = run(&s, "SELECT 1\nFORM t", 10).await.unwrap_err();
    assert!(
        matches!(
            err,
            AppError::Db {
                position: Some(_),
                vendor_code: Some(1003),
                ..
            }
        ),
        "{err:?}"
    );

    // Everything ran on the editor's session.
    assert!(fake.sent().iter().skip(1).all(|(session, _)| *session == 1));
}

#[tokio::test]
async fn explain_runs_the_dialects_form() {
    let fake = fake(Login::key_pair()).await;
    let s = open(&fake).await;
    // The session has no form of its own: the dialect's.
    assert!(
        s.explain_form(crate::types::query::ExplainMode::Plan)
            .is_none()
    );
    let explained = sql::wrap_explain(
        s.dialect(),
        "SELECT * FROM t",
        crate::types::query::ExplainMode::Plan,
        false,
    )
    .unwrap();
    assert_eq!(explained, "EXPLAIN USING TEXT SELECT * FROM t");
    fake.show(&explained, &["content"], &[&[Some("GlobalStats: …")]]);
    let (out, sink) = run(&s, &explained, 100).await.unwrap();
    assert_eq!(out.kind, StatementKind::Rows);
    assert_eq!(sink.sets[0].rows.len(), 1);
}

#[tokio::test]
async fn cancel_stops_the_editor_statement_on_the_server() {
    let fake = fake(Login::key_pair()).await;
    let s = Arc::new(open(&fake).await);
    fake.answer("SELECT SYSTEM$WAIT(60)", Answer::Hang);
    let slot = CancelSlot::default();
    let running = {
        let s = Arc::clone(&s);
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut sink = CollectSink::default();
            s.run_statement("SELECT SYSTEM$WAIT(60)", 100, &mut sink, slot)
                .await
        })
    };
    let before = fake.queries().len();
    fake.wait_for_queries(before + 1).await;
    // Wait until the statement has armed the slot.
    for _ in 0..100 {
        if slot.is_armed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let handle = request_cancel(&slot).expect("the run armed the slot");
    s.cancel(handle).await.unwrap();
    let err = running.await.unwrap().unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
    assert_eq!(fake.aborts().len(), 1);
}

#[tokio::test]
async fn the_query_time_out_cancels_on_the_server() {
    let fake = fake(Login::key_pair()).await;
    let s = open(&fake).await;
    fake.answer("SELECT * FROM big", Answer::Hang);
    let err = query_limited(
        &s,
        "SELECT * FROM big",
        &[],
        Some(Duration::from_millis(500)),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::QueryTimeout { .. }), "{err:?}");
    assert_eq!(fake.aborts().len(), 1);
    // The session goes on.
    fake.show(sf::PING, &["1"], &[&[Some("1")]]);
    s.ping().await.unwrap();
}

#[tokio::test]
async fn a_read_holding_another_statement_is_refused_everywhere() {
    let fake = fake(Login::key_pair()).await;
    let s = open(&fake).await;
    let sent = fake.sent().len();
    for sql in [
        "SELECT * FROM t WHERE (1=1); DELETE FROM t WHERE (1=1)",
        "SELECT * FROM t WHERE (1=1) ->> DELETE FROM t",
    ] {
        assert!(s.query(sql, &[]).await.is_err(), "{sql}");
        let mut sink = CollectSink::default();
        assert!(
            s.stream_query(sql, &[], &mut sink, CancelSlot::default())
                .await
                .is_err(),
            "{sql}"
        );
    }
    assert_eq!(fake.sent().len(), sent);
}

#[tokio::test]
async fn read_only_refuses_every_write_path_before_sending() {
    let fake = fake(Login::key_pair()).await;
    let s = read_only(&fake).await;
    let sent = fake.sent().len();
    let ro = |r: Result<(), AppError>| {
        assert!(
            matches!(&r, Err(AppError::InvalidInput(m)) if m.contains("read-only")),
            "{r:?}"
        );
    };
    for sql in [
        "DELETE FROM t",
        "SELECT SYSTEM$CANCEL_ALL_QUERIES(1)",
        "CALL p()",
        "EXECUTE IMMEDIATE 'DROP TABLE t'",
        "USE ROLE ACCOUNTADMIN",
        "SELECT * FROM TABLE(p())",
        "PUT file:///x @s",
        "frobnicate",
        "SELECT seq.\"NEXTVAL\"",
        "SELECT \"SEQ\".\"NEXTVAL\" FROM TABLE(GENERATOR(ROWCOUNT => 1000000))",
    ] {
        ro(s.query(sql, &[]).await.map(|_| ()));
        ro(s.query_cancellable(sql, &[], CancelSlot::default())
            .await
            .map(|_| ()));
        ro(s.execute(sql, &[]).await.map(|_| ()));
        ro(s.execute_cancellable(sql, &[], CancelSlot::default())
            .await
            .map(|_| ()));
        ro(s.transaction(vec![TxStatement::new(sql, Vec::new())])
            .await
            .map(|_| ()));
        ro(run(&s, sql, 10).await.map(|_| ()));
        let mut sink = CollectSink::default();
        ro(s.stream_query(sql, &[], &mut sink, CancelSlot::default())
            .await
            .map(|_| ()));
    }
    let kills = s
        .kill_processes(&["01b0-query".into()], KillScope::Query)
        .await
        .unwrap();
    assert!(
        !kills[0].ok
            && kills[0]
                .message
                .as_deref()
                .is_some_and(|m| m.contains("read-only"))
    );
    // An empty commit sends nothing (no BEGIN).
    assert!(s.transaction(Vec::new()).await.unwrap().is_empty());
    assert_eq!(fake.sent().len(), sent, "nothing reached the server");

    // Reads still run.
    fake.show("SELECT 1 AS one", &["ONE"], &[&[Some("1")]]);
    assert!(s.query("SELECT 1 AS one", &[]).await.is_ok());
    assert!(run(&s, "SELECT 1 AS one", 10).await.is_ok());
    assert_eq!(
        s.capabilities().safety.read_only,
        ReadOnlyMode::ClassifierOnly
    );

    // Startup commands that write are refused before any sign-in.
    let before = fake.logins().len();
    let err = open_with(
        &fake,
        serde_json::json!({}),
        serde_json::json!({ "readOnly": true, "startupSql": "ALTER SESSION SET TIMEZONE = 'UTC';" }),
        None,
    )
    .await
    .err()
    .expect("refused");
    assert!(err.to_string().contains("read-only"), "{err}");
    assert_eq!(fake.logins().len(), before);
}

#[tokio::test]
async fn processes_list_and_kill_by_query_id() {
    let fake = fake(Login::key_pair()).await;
    let s = open(&fake).await;
    fake.show(
        sf::RUNNING_QUERIES,
        &[
            "QUERY_ID",
            "SESSION_ID",
            "USER_NAME",
            "WAREHOUSE_NAME",
            "EXECUTION_STATUS",
            "START_TIME",
            "QUERY_TEXT",
        ],
        &[&[
            Some("01b0-q1"),
            Some("123"),
            Some("TABLER_USER"),
            Some("COMPUTE_WH"),
            Some("RUNNING"),
            Some("2026-11-01 10:00:00"),
            Some("SELECT SYSTEM$WAIT(60)"),
        ]],
    );
    let rows = s.list_processes().await.unwrap().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "01b0-q1");
    assert_eq!(rows[0].state.as_deref(), Some("RUNNING"));
    fake.show(
        &sf::cancel_query("01b0-q1"),
        &["SYSTEM$CANCEL_QUERY('01b0-q1')"],
        &[&[Some("query [01b0-q1] terminated.")]],
    );
    let out = s
        .kill_processes(&["01b0-q1".into()], KillScope::Query)
        .await
        .unwrap();
    assert!(out[0].ok, "{out:?}");
    assert_eq!(fake.sent().pop().unwrap().1, sf::cancel_query("01b0-q1"));
    let out = s
        .kill_processes(&["123".into()], KillScope::Connection)
        .await
        .unwrap();
    assert!(!out[0].ok);
}

#[tokio::test]
async fn an_expired_session_signs_in_again_and_is_set_up_as_before() {
    let fake = fake(Login::key_pair()).await;
    fake.status("ALTER SESSION SET QUERY_TAG = 'tabler'");
    fake.status("USE SCHEMA \"DB\".\"OTHER\"");
    let s = open_with(
        &fake,
        serde_json::json!({}),
        serde_json::json!({ "startupSql": "ALTER SESSION SET QUERY_TAG = 'tabler'" }),
        None,
    )
    .await
    .unwrap();
    fake.show(sf::PING, &["1"], &[&[Some("1")]]);
    fake.show("SELECT 2", &["2"], &[&[Some("2")]]);
    // The app's session: signed in again, the statement sent again.
    fake.expire_session();
    s.ping().await.unwrap();
    assert_eq!(fake.logins().len(), 3);
    let sent = fake.sent();
    let n = sent.len();
    assert_eq!(sent[n - 4], (2, sf::PING.to_string()));
    assert_eq!(
        sent[n - 3],
        (3, "ALTER SESSION SET QUERY_TAG = 'tabler'".to_string())
    );
    // The app's session sets its parameters back after the startup commands.
    assert_eq!(sent[n - 2], (3, sf::PIN_SESSION.to_string()));
    assert_eq!(sent[n - 1], (3, sf::PING.to_string()));

    // The editor: its USE runs again on the new session, and the run says what was lost.
    run(&s, "USE SCHEMA \"DB\".\"OTHER\"", 10).await.unwrap();
    fake.expire_session();
    let (out, _) = run(&s, "SELECT 2", 10).await.unwrap();
    assert!(
        out.notices.iter().any(|n| n.contains("signed in again")),
        "{out:?}"
    );
    let sent = fake.sent();
    let tail: Vec<&str> = sent[sent.len() - 3..]
        .iter()
        .map(|(_, s)| s.as_str())
        .collect();
    assert_eq!(
        tail,
        [
            "ALTER SESSION SET QUERY_TAG = 'tabler'",
            "USE SCHEMA \"DB\".\"OTHER\"",
            "SELECT 2"
        ]
    );
    assert!(
        sent[sent.len() - 3..]
            .iter()
            .all(|(session, _)| *session == 4)
    );

    // An open transaction cannot survive: the run fails instead of going on without it.
    fake.status("BEGIN");
    run(&s, "BEGIN", 10).await.unwrap();
    fake.expire_session();
    let err = run(&s, "SELECT 2", 10).await.unwrap_err();
    assert!(err.to_string().contains("transaction"), "{err}");
    assert_eq!(s.in_transaction(), Some(false));
}

#[tokio::test]
async fn an_expiry_after_snowflake_accepted_a_statement_never_sends_it_again() {
    let fake = fake(Login::key_pair()).await;
    let s = open(&fake).await;
    // Accepted (`333334`, a query id), then the poll finds the session expired: the statement
    // goes on in Snowflake, so neither the app nor the editor sends it again.
    let expired_while_polled = || {
        Answer::Async(Box::new(Answer::error(
            "390112",
            "Your session has expired. Please login again.",
        )))
    };
    let insert = "INSERT INTO t VALUES (1)";
    fake.answer(insert, expired_while_polled());
    let times = |sql: &str| fake.sent().iter().filter(|(_, s)| s == sql).count();
    let logins = fake.logins().len();
    let may_have = |e: AppError| {
        assert!(e.to_string().contains("may have completed"), "{e}");
    };

    may_have(s.execute(insert, &[]).await.unwrap_err());
    assert_eq!(times(insert), 1);
    assert_eq!(fake.logins().len(), logins + 1, "signed in again");
    may_have(
        s.execute_cancellable(insert, &[], CancelSlot::default())
            .await
            .unwrap_err(),
    );
    assert_eq!(times(insert), 2);
    may_have(run(&s, insert, 10).await.unwrap_err());
    assert_eq!(times(insert), 3);
    assert_eq!(fake.logins().len(), logins + 3);

    // A read Snowflake had accepted is not sent again either.
    fake.answer("SELECT 2", expired_while_polled());
    may_have(s.query("SELECT 2", &[]).await.unwrap_err());
    assert_eq!(times("SELECT 2"), 1);

    // A write refused before Snowflake accepted it (no query id) is not sent again: whether it
    // ran cannot be told from the client.
    fake.expire_session();
    may_have(s.execute(insert, &[]).await.unwrap_err());
    assert_eq!(times(insert), 4);

    // An expired master token (`390114`) is an expired session too: signed in again.
    let logins = fake.logins().len();
    fake.answer(
        "SELECT 3",
        Answer::error("390114", "Master token has expired."),
    );
    may_have(s.query("SELECT 3", &[]).await.unwrap_err());
    assert_eq!(fake.logins().len(), logins + 1);
    fake.show(sf::PING, &["1"], &[&[Some("1")]]);
    s.ping().await.unwrap();
}

#[tokio::test]
async fn an_mfa_sign_in_happens_once_and_both_share_it() {
    let fake = fake(Login::Password {
        password: "pw".into(),
        passcode: None,
    })
    .await;
    let s = open_with(
        &fake,
        serde_json::json!({ "auth": "password", "mfa": true, "privateKeyPath": null }),
        serde_json::json!({}),
        Some("pw"),
    )
    .await
    .unwrap();
    assert_eq!(fake.logins().len(), 1);
    fake.status(sf::BEGIN);
    fake.status(sf::COMMIT);
    fake.status("BEGIN");
    fake.counts("DELETE FROM t WHERE id = ?", "deleted", 1);
    // A transaction the user opened in the editor would take the commit in: refused.
    run(&s, "BEGIN", 10).await.unwrap();
    let err = s
        .transaction(vec![
            TxStatement::new("DELETE FROM t WHERE id = ?", vec![Value::Int(1)]).expecting(1),
        ])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("transaction open"), "{err}");
    // So would a statement the app runs (DDL would commit it): refused too, nothing sent.
    let sent = fake.sent().len();
    for err in [
        s.execute("DELETE FROM t WHERE id = 2", &[])
            .await
            .unwrap_err(),
        s.execute_cancellable("DROP TABLE t", &[], CancelSlot::default())
            .await
            .unwrap_err(),
    ] {
        assert!(err.to_string().contains("transaction open"), "{err}");
    }
    assert_eq!(fake.sent().len(), sent);
    fake.status("ROLLBACK");
    run(&s, "ROLLBACK", 10).await.unwrap();
    s.transaction(vec![
        TxStatement::new("DELETE FROM t WHERE id = ?", vec![Value::Int(1)]).expecting(1),
    ])
    .await
    .unwrap();
    assert!(fake.sent().iter().all(|(session, _)| *session == 1));
    // Unattended (MCP), nobody can approve the push: refused before sending.
    let p = profile(
        &fake,
        serde_json::json!({ "auth": "password", "mfa": true }),
        serde_json::json!({}),
    );
    let mut inputs = super::auth::AuthInputs {
        secret: Some("pw".into()),
        mfa: true,
        unattended: true,
        ..Default::default()
    };
    assert!(super::auth::auth_config(super::params::SnowflakeAuth::Password, &inputs).is_err());
    inputs.unattended = false;
    assert!(super::auth::auth_config(super::params::SnowflakeAuth::Password, &inputs).is_ok());
    assert!(
        SignIn::for_target(&ConnectTarget {
            profile: &p,
            password: Some("pw"),
            host_override: None
        })
        .unwrap()
        .asks_mfa()
    );
}

#[tokio::test]
async fn a_pat_longer_than_the_credential_manager_keeps_signs_in() {
    let pat: String = "p".repeat(2_000);
    let fake = fake(Login::Password {
        password: pat.clone(),
        passcode: None,
    })
    .await;
    let s = open_with(
        &fake,
        serde_json::json!({ "auth": "pat", "privateKeyPath": null }),
        serde_json::json!({}),
        Some(&pat),
    )
    .await
    .unwrap();
    assert_eq!(fake.logins().len(), 2);
    assert_eq!(fake.logins()[0].body["data"]["PASSWORD"], pat.as_str());
    s.close().await;
}

#[tokio::test]
async fn an_encrypted_key_takes_the_passphrase_secret_of_the_connect() {
    use crate::secrets::ExtraSecrets;
    use crate::types::driver::ExtraSecret;
    let fake = fake(Login::key_pair()).await;
    let params = serde_json::json!({ "privateKeyPath": fixture_path("rsa_key_encrypted.p8") });
    // Without it: an Auth error before anything is sent.
    let err = open_with(&fake, params.clone(), serde_json::json!({}), None)
        .await
        .err()
        .expect("refused");
    assert!(matches!(err, AppError::Auth(_)), "{err:?}");
    assert!(fake.logins().is_empty());
    let mut extras = ExtraSecrets::default();
    extras.set(
        ExtraSecret::KeyPassphrase,
        super::testing::KEY_PASSPHRASE.into(),
    );
    let s = extras
        .scope(open_with(&fake, params, serde_json::json!({}), None))
        .await
        .unwrap();
    assert_eq!(fake.logins().len(), 2);
    // A later sign-in (an expired session) keeps what the connect read.
    fake.show(sf::PING, &["1"], &[&[Some("1")]]);
    fake.expire_session();
    s.ping().await.unwrap();
    assert_eq!(fake.logins().len(), 3);
}

#[tokio::test]
async fn startup_commands_run_on_both_sessions() {
    let fake = fake(Login::key_pair()).await;
    fake.status("ALTER SESSION SET TIMEZONE = 'UTC'");
    fake.status("USE WAREHOUSE \"ETL\"");
    let _s = open_with(
        &fake,
        serde_json::json!({}),
        serde_json::json!({ "startupSql": "ALTER SESSION SET TIMEZONE = 'UTC';\nUSE WAREHOUSE \"ETL\";" }),
        None,
    )
    .await
    .unwrap();
    let sent = fake.sent();
    for session in [1, 2] {
        let mine: Vec<&str> = sent
            .iter()
            .filter(|(s, _)| *s == session)
            .map(|(_, q)| q.as_str())
            .take(2)
            .collect();
        assert_eq!(
            mine,
            [
                "ALTER SESSION SET TIMEZONE = 'UTC'",
                "USE WAREHOUSE \"ETL\""
            ]
        );
    }
    // A startup command (a procedure, say) may change the app's parameters unseen: its session
    // (the second sign-in) sets them back after them; the editor's own session is the user's.
    let after = |session: usize| {
        sent.iter()
            .filter(|(s, _)| *s == session)
            .nth(2)
            .map(|(_, q)| q.clone())
    };
    assert_eq!(after(2).as_deref(), Some(sf::PIN_SESSION.as_str()));
    assert_ne!(after(1).as_deref(), Some(sf::PIN_SESSION.as_str()));
}

#[tokio::test]
async fn a_refused_sign_in_is_an_auth_error() {
    let fake = fake(Login::Password {
        password: "right".into(),
        passcode: None,
    })
    .await;
    let err = open_with(
        &fake,
        serde_json::json!({ "auth": "pat", "privateKeyPath": null }),
        serde_json::json!({}),
        Some("wrong"),
    )
    .await
    .err()
    .expect("refused");
    assert!(matches!(err, AppError::Auth(_)), "{err:?}");
}

#[tokio::test]
async fn an_expiry_after_snowflake_accepted_a_commit_says_it_may_have_committed() {
    let fake = fake(Login::key_pair()).await;
    let s = open(&fake).await;
    for st in ["BEGIN", sf::BEGIN, sf::ROLLBACK] {
        fake.status(st);
    }
    let insert = "INSERT INTO t VALUES (1)";
    fake.counts(insert, "inserted", 1);
    let expired_while_polled = || {
        Answer::Async(Box::new(Answer::error(
            "390112",
            "Your session has expired. Please login again.",
        )))
    };
    let may_have = |e: AppError| {
        assert!(e.to_string().contains("may have completed"), "{e}");
    };

    // The editor's COMMIT, accepted: the transaction may be committed, so it is not reported as
    // rolled back (running it again would insert the row twice).
    fake.answer(sf::COMMIT, expired_while_polled());
    run(&s, "BEGIN", 10).await.unwrap();
    run(&s, insert, 10).await.unwrap();
    may_have(run(&s, sf::COMMIT, 10).await.unwrap_err());
    assert_eq!(s.in_transaction(), Some(false));

    // DDL commits the transaction implicitly: the same.
    let ddl = "CREATE TABLE u (a INT)";
    fake.answer(ddl, expired_while_polled());
    run(&s, "BEGIN", 10).await.unwrap();
    run(&s, insert, 10).await.unwrap();
    may_have(run(&s, ddl, 10).await.unwrap_err());
    assert_eq!(s.in_transaction(), Some(false));

    // A ROLLBACK or a write inside the transaction: rolled back with the session.
    for sql in [sf::ROLLBACK, insert] {
        fake.answer(sql, expired_while_polled());
        run(&s, "BEGIN", 10).await.unwrap();
        let err = run(&s, sql, 10).await.unwrap_err();
        assert!(err.to_string().contains("rolled back"), "{sql}: {err}");
        assert_eq!(s.in_transaction(), Some(false));
    }

    // The grid's commit: the same COMMIT, accepted, may have gone through.
    let update = "UPDATE \"PUBLIC\".\"ITEMS\" SET \"NAME\" = ? WHERE \"ID\" = ?";
    fake.counts(update, "updated", 1);
    fake.status(sf::ROLLBACK);
    let staged = || {
        vec![TxStatement::new(update, vec![Value::Text("ink".into()), Value::Int(4)]).expecting(1)]
    };
    may_have(s.transaction(staged()).await.unwrap_err());
    // Signed in again, the same changes commit (an expired BEGIN is sent again).
    fake.status(sf::COMMIT);
    fake.expire_session();
    let logins = fake.logins().len();
    assert!(s.transaction(staged()).await.is_ok());
    assert_eq!(fake.logins().len(), logins + 1);
}

async fn open_mfa(fake: &FakeSnowflake) -> SnowflakeSession {
    open_with(
        fake,
        serde_json::json!({ "auth": "password", "mfa": true, "privateKeyPath": null }),
        serde_json::json!({}),
        Some("pw"),
    )
    .await
    .expect("the session opens")
}

#[tokio::test]
async fn on_a_shared_mfa_session_the_app_never_follows_the_editor_to_another_database() {
    let fake = fake(Login::Password {
        password: "pw".into(),
        passcode: None,
    })
    .await;
    let s = open_mfa(&fake).await;
    // The grid's SQL names schema and table only.
    let update = "UPDATE \"PUBLIC\".\"ITEMS\" SET \"NAME\" = ? WHERE \"ID\" = ?";
    let args = || vec![Value::Text("ink".into()), Value::Int(4)];
    fake.counts(update, "updated", 1);
    fake.status(sf::BEGIN);
    fake.status(sf::COMMIT);
    let staged = || vec![TxStatement::new(update, args()).expecting(1)];
    let count = "SELECT COUNT(*) FROM \"PUBLIC\".\"ITEMS\"";
    fake.answer(
        count,
        Answer::rows(
            vec![FakeColumn::new("COUNT(*)", "fixed").precision(18, 0)],
            vec![vec![Some("3".into())]],
        ),
    );
    let back = "USE DATABASE \"DB\"";
    fake.status(back);

    for (moves, now_in) in [
        ("USE DATABASE \"PROD\"", "PROD"),
        ("USE SCHEMA PROD2.PUBLIC", "PROD2"),
        ("USE PROD", "PROD"),
        // Moves without saying USE: CREATE DATABASE makes the new one current, and a
        // procedure can run USE.
        ("CREATE DATABASE SCRATCH", "SCRATCH"),
        ("CALL use_prod()", "PROD"),
    ] {
        fake.status(moves);
        fake.editor_state(Some(now_in), None);
        run(&s, moves, 10).await.unwrap();
        let sent = fake.sent().len();
        for err in [
            s.transaction(staged()).await.unwrap_err(),
            s.execute(update, &args()).await.unwrap_err(),
            s.query(count, &[]).await.unwrap_err(),
        ] {
            assert!(
                err.to_string().contains(&format!("database {now_in}")),
                "{moves}: {err}"
            );
            assert!(err.to_string().contains(back), "{moves}: {err}");
        }
        assert_eq!(fake.sent().len(), sent, "{moves}: nothing was sent");
        // Back in the session's database, the grid works again.
        fake.editor_state(Some("DB"), None);
        run(&s, back, 10).await.unwrap();
        assert_eq!(s.transaction(staged()).await.unwrap().len(), 1, "{moves}");
        s.query(count, &[]).await.unwrap();
    }
    // A read changes nothing, so nothing is read back after it.
    let sent = fake.sent().len();
    run(&s, count, 10).await.unwrap();
    assert_eq!(fake.sent().len(), sent + 1);

    // Signed in again, the replayed USE statements may land elsewhere: read back before the
    // statement is sent again.
    fake.editor_state(Some("PROD"), None);
    fake.expire_session();
    let err = s.query(count, &[]).await.unwrap_err();
    assert!(err.to_string().contains("database PROD"), "{err}");
    let times = fake.sent().iter().filter(|(_, q)| q == count).count();
    fake.editor_state(Some("DB"), None);
    run(&s, back, 10).await.unwrap();
    s.query(count, &[]).await.unwrap();
    assert_eq!(
        fake.sent().iter().filter(|(_, q)| q == count).count(),
        times + 1
    );
    // The same before a commit's BEGIN on the new session.
    fake.editor_state(Some("PROD"), None);
    fake.expire_session();
    let err = s.transaction(staged()).await.unwrap_err();
    assert!(err.to_string().contains("database PROD"), "{err}");
    assert_ne!(fake.sent().last().map(|(_, q)| q.as_str()), Some(sf::BEGIN));
    assert!(!fake.sent().iter().rev().take(3).any(|(_, q)| q == update));

    // With a session of its own (no MFA), the app never shares the editor's USE.
    let fake = fake_with_key().await;
    let s = open(&fake).await;
    fake.status("USE DATABASE \"PROD\"");
    fake.counts(update, "updated", 1);
    fake.status(sf::BEGIN);
    fake.status(sf::COMMIT);
    fake.editor_state(Some("PROD"), None);
    run(&s, "USE DATABASE \"PROD\"", 10).await.unwrap();
    assert_eq!(s.transaction(staged()).await.unwrap().len(), 1);
    assert!(
        fake.sent()
            .iter()
            .filter(|(_, q)| q == update || q == sf::BEGIN)
            .all(|(session, _)| *session == 2)
    );
}

#[tokio::test]
async fn on_a_shared_mfa_session_the_apps_new_sign_in_under_the_editors_transaction_is_reported() {
    let fake = fake(Login::Password {
        password: "pw".into(),
        passcode: None,
    })
    .await;
    let s = open_mfa(&fake).await;
    let insert = "INSERT INTO t VALUES (1)";
    let again = "INSERT INTO t VALUES (2)";
    let count = "SELECT COUNT(*) FROM \"PUBLIC\".\"ITEMS\"";
    fake.status("BEGIN");
    fake.status(sf::ROLLBACK);
    fake.counts(insert, "inserted", 1);
    fake.counts(again, "inserted", 1);
    fake.answer(
        count,
        Answer::rows(
            vec![FakeColumn::new("COUNT(*)", "fixed").precision(18, 0)],
            vec![vec![Some("3".into())]],
        ),
    );
    fake.show(sf::PING, &["1"], &[&[Some("1")]]);
    let open_tx = || Some("1700000000000000001");

    // Each of the app's paths that signs in again: a grid read (allowed while the editor's
    // transaction is open) and a ping.
    for path in ["grid read", "ping"] {
        fake.editor_state(Some("DB"), None);
        run(&s, "BEGIN", 10).await.unwrap();
        fake.editor_state(Some("DB"), open_tx());
        run(&s, insert, 10).await.unwrap();
        assert_eq!(s.in_transaction(), Some(true), "{path}");

        // Snowflake ends the idle session, and the app meets it first.
        fake.expire_session();
        fake.editor_state(Some("DB"), None);
        let logins = fake.logins().len();
        if path == "ping" {
            s.ping().await.unwrap();
        } else {
            assert_eq!(s.query(count, &[]).await.unwrap().rows.len(), 1);
        }
        assert_eq!(fake.logins().len(), logins + 1, "{path}: signed in again");

        // The editor's next statement would autocommit on the new session: it is refused, with
        // nothing sent, and says the transaction is gone.
        let sent = fake.sent().len();
        let err = run(&s, again, 10).await.unwrap_err();
        assert!(
            err.to_string().contains("run the transaction again"),
            "{path}: {err}"
        );
        assert_eq!(fake.sent().len(), sent, "{path}: nothing was sent");
        assert_eq!(s.in_transaction(), Some(false), "{path}");
        // Said once: the next statement runs on the new session.
        run(&s, sf::ROLLBACK, 10).await.unwrap();
        assert_eq!(s.in_transaction(), Some(false), "{path}");
        assert_eq!(
            fake.sent().len(),
            sent + 3,
            "{path}: ROLLBACK, the app's parameters set back, and its read-back"
        );
    }

    // With no transaction open, the app's new sign-in leaves the editor alone.
    fake.expire_session();
    s.ping().await.unwrap();
    let (out, _) = run(&s, again, 10).await.unwrap();
    assert!(out.notices.is_empty(), "{:?}", out.notices);
}

#[tokio::test]
async fn on_a_shared_mfa_session_a_new_sign_in_while_the_editors_begin_runs_is_reported() {
    let fake = fake(Login::Password {
        password: "pw".into(),
        passcode: None,
    })
    .await;
    let s = Arc::new(open_mfa(&fake).await);
    let insert = "INSERT INTO t VALUES (1)";
    fake.counts(insert, "inserted", 1);
    fake.editor_state(Some("DB"), None);
    // The BEGIN is answered late; the app's catalog read (which waits for no editor statement)
    // meets the expiry meanwhile and signs in again before the editor reads the BEGIN back.
    fake.answer(
        "BEGIN",
        Answer::Slow(
            Duration::from_millis(600),
            Box::new(Answer::rows(
                vec![FakeColumn::new("status", "text")],
                vec![vec![Some("Statement executed successfully.".into())]],
            )),
        ),
    );
    fake.answer(
        sf::SHOW_DATABASES,
        Answer::error("390112", "Your session has expired. Please login again."),
    );
    let logins = fake.logins().len();
    let before = fake.queries().len();
    let begin = {
        let s = Arc::clone(&s);
        tokio::spawn(async move { run(&s, "BEGIN", 10).await.map(|_| ()) })
    };
    fake.wait_for_queries(before + 1).await;
    let _ = s.list_databases().await;
    assert!(fake.logins().len() > logins, "signed in again");
    assert!(!begin.is_finished(), "the BEGIN was still running");
    begin.await.unwrap().unwrap();
    assert!(
        fake.sent()
            .iter()
            .any(|(n, q)| *n == 1 && q == sf::EDITOR_STATE),
        "read back from the session the BEGIN ran on: {:?}",
        fake.sent()
    );

    // The transaction went with the old session: the INSERT would autocommit on the new one, so
    // it is refused with nothing sent.
    let sent = fake.sent().len();
    let err = run(&s, insert, 10).await.unwrap_err();
    assert!(
        err.to_string().contains("run the transaction again"),
        "{err}"
    );
    assert_eq!(fake.sent().len(), sent, "nothing was sent");
    assert_eq!(s.in_transaction(), Some(false));
    // Said once: the next statement runs on the new session.
    run(&s, insert, 10).await.unwrap();
    assert!(fake.sent().iter().any(|(n, q)| *n > 1 && q == insert));
}

#[tokio::test]
async fn on_a_shared_mfa_session_an_editor_write_waiting_on_a_read_that_signs_in_again_is_refused()
{
    let fake = fake(Login::Password {
        password: "pw".into(),
        passcode: None,
    })
    .await;
    let s = Arc::new(open_mfa(&fake).await);
    let insert = "INSERT INTO t VALUES (1)";
    let again = "INSERT INTO t VALUES (2)";
    let count = "SELECT COUNT(*) FROM \"PUBLIC\".\"ITEMS\"";
    fake.status("BEGIN");
    fake.counts(insert, "inserted", 1);
    fake.counts(again, "inserted", 1);
    fake.editor_state(Some("DB"), None);
    run(&s, "BEGIN", 10).await.unwrap();
    fake.editor_state(Some("DB"), Some("1700000000000000001"));
    run(&s, insert, 10).await.unwrap();
    assert_eq!(s.in_transaction(), Some(true));

    // A grid read holds the session (`moving` for reading) and meets the expiry only after the
    // editor's INSERT has started waiting for it.
    fake.answer(
        count,
        Answer::Slow(
            Duration::from_millis(600),
            Box::new(Answer::error(
                "390112",
                "Your session has expired. Please login again.",
            )),
        ),
    );
    let logins = fake.logins().len();
    let reading = {
        let s = Arc::clone(&s);
        tokio::spawn(async move { s.query(count, &[]).await.map(|_| ()) })
    };
    let before = fake.queries().len();
    fake.wait_for_queries(before + 1).await;
    let writing = {
        let s = Arc::clone(&s);
        tokio::spawn(async move { run(&s, again, 10).await.map(|_| ()) })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!writing.is_finished(), "the INSERT waits for the read");
    fake.editor_state(Some("DB"), None);
    fake.answer(
        count,
        Answer::rows(
            vec![FakeColumn::new("COUNT(*)", "fixed").precision(18, 0)],
            vec![vec![Some("3".into())]],
        ),
    );
    // Whatever the read answers, it signed in again under the editor's transaction.
    let _ = reading.await.unwrap();
    assert_eq!(fake.logins().len(), logins + 1, "signed in again");

    // The INSERT would autocommit on the new session: refused, never sent, and said at once.
    let err = writing.await.unwrap().unwrap_err();
    assert!(
        err.to_string().contains("run the transaction again"),
        "{err}"
    );
    assert!(!fake.sent().iter().any(|(_, q)| q == again), "nothing sent");
    assert_eq!(s.in_transaction(), Some(false));
}

async fn fake_with_key() -> FakeSnowflake {
    fake(Login::key_pair()).await
}

#[tokio::test]
async fn autocommit_stays_on_and_a_transaction_the_text_hides_is_seen() {
    let fake = fake(Login::Password {
        password: "pw".into(),
        passcode: None,
    })
    .await;
    let s = open_mfa(&fake).await;
    // The sign-in sets it, whatever the profile, the user or the account says.
    assert_eq!(
        fake.logins()[0].body["data"]["SESSION_PARAMETERS"]["AUTOCOMMIT"],
        true
    );
    let sent = fake.sent().len();
    for sql in [
        "ALTER SESSION SET AUTOCOMMIT = FALSE",
        "alter session set timezone = 'UTC', autocommit = false",
        "/* off */ ALTER SESSION UNSET AUTOCOMMIT",
        "EXECUTE IMMEDIATE 'ALTER SESSION SET AUTOCOMMIT = FALSE'",
    ] {
        let err = run(&s, sql, 10).await.unwrap_err();
        assert!(err.to_string().contains("AUTOCOMMIT"), "{sql}: {err}");
    }
    assert_eq!(fake.sent().len(), sent, "nothing was sent");
    assert_eq!(s.in_transaction(), Some(false));

    // A procedure that turned it off: the INSERT after it leaves a transaction open, read back
    // after the statement.
    let call = "CALL legacy_setup()";
    let insert = "INSERT INTO t VALUES (1)";
    fake.status(call);
    fake.counts(insert, "inserted", 1);
    run(&s, call, 10).await.unwrap();
    assert_eq!(s.in_transaction(), Some(false));
    fake.editor_state(Some("DB"), Some("1700000000000000001"));
    run(&s, insert, 10).await.unwrap();
    assert_eq!(s.in_transaction(), Some(true));
    // The grid's commit would take the INSERT in (or its ROLLBACK throw it away): refused.
    let sent = fake.sent().len();
    let err = s
        .transaction(vec![
            TxStatement::new("DELETE FROM t WHERE id = ?", vec![Value::Int(1)]).expecting(1),
        ])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("transaction open"), "{err}");
    assert_eq!(fake.sent().len(), sent);
    fake.status(sf::COMMIT);
    fake.editor_state(Some("DB"), None);
    run(&s, sf::COMMIT, 10).await.unwrap();
    assert_eq!(s.in_transaction(), Some(false));

    // When the state cannot be read back, the transaction is not known (and shared writes wait).
    fake.answer(
        sf::EDITOR_STATE,
        Answer::error("000603", "SQL execution internal error."),
    );
    run(&s, insert, 10).await.unwrap();
    assert_eq!(s.in_transaction(), None);
    fake.editor_state(Some("DB"), None);

    // Startup commands that change it refuse the connect before signing in.
    let logins = fake.logins().len();
    let err = open_with(
        &fake,
        serde_json::json!({}),
        serde_json::json!({ "startupSql": "USE WAREHOUSE W; ALTER SESSION SET AUTOCOMMIT = FALSE" }),
        None,
    )
    .await
    .err()
    .expect("refused");
    assert!(err.to_string().contains("AUTOCOMMIT"), "{err}");
    assert_eq!(fake.logins().len(), logins);
}

#[tokio::test]
async fn quoted_names_keep_their_case_on_the_apps_session() {
    let fake = fake(Login::Password {
        password: "pw".into(),
        passcode: None,
    })
    .await;
    // The sign-in turns it off, whatever the profile says.
    let s = open_mfa(&fake).await;
    assert_eq!(
        fake.logins()[0].body["data"]["SESSION_PARAMETERS"]["QUOTED_IDENTIFIERS_IGNORE_CASE"],
        false
    );
    // On the shared (MFA) session the editor may not turn it back on.
    let sent = fake.sent().len();
    for sql in [
        "ALTER SESSION SET QUOTED_IDENTIFIERS_IGNORE_CASE = TRUE",
        "alter session unset quoted_identifiers_ignore_case",
        "EXECUTE IMMEDIATE 'ALTER SESSION SET QUOTED_IDENTIFIERS_IGNORE_CASE = TRUE'",
    ] {
        let err = run(&s, sql, 10).await.unwrap_err();
        assert!(
            err.to_string().contains("QUOTED_IDENTIFIERS_IGNORE_CASE"),
            "{sql}: {err}"
        );
    }
    assert_eq!(fake.sent().len(), sent, "nothing was sent");
    // The grid's dates are written as ISO text, which only AUTO reads as such.
    for sql in [
        "ALTER SESSION SET DATE_INPUT_FORMAT = 'YYYY-DD-MM'",
        "alter session set timestamp_input_format = 'DD/MM/YYYY HH24:MI'",
    ] {
        let err = run(&s, sql, 10).await.unwrap_err();
        assert!(err.to_string().contains("INPUT_FORMAT"), "{sql}: {err}");
    }
    assert_eq!(fake.sent().len(), sent, "nothing was sent");
    // Text that hides the parameter (a session variable, a procedure with the caller's rights)
    // runs, and the session's parameters are set back before it is read back.
    for sql in ["EXECUTE IMMEDIATE $q", "CALL util.setup_session()"] {
        fake.status(sql);
        let before = fake.sent().len();
        run(&s, sql, 10).await.unwrap();
        let after: Vec<String> = fake.sent()[before..]
            .iter()
            .map(|(_, q)| q.clone())
            .collect();
        assert_eq!(
            after,
            [sql, sf::PIN_SESSION.as_str(), sf::EDITOR_STATE],
            "{sql}"
        );
    }
    // A read changes nothing: nothing is set back after it.
    let read = "SELECT 2";
    fake.answer(
        read,
        Answer::rows(
            vec![FakeColumn::new("2", "fixed").precision(1, 0)],
            vec![vec![Some("2".into())]],
        ),
    );
    let before = fake.sent().len();
    run(&s, read, 10).await.unwrap();
    assert_eq!(fake.sent().len(), before + 1);

    // When setting them back fails, the grid's next statement sets them back first, and is not
    // sent while that fails.
    let count = "SELECT COUNT(*) FROM \"PUBLIC\".\"ITEMS\"";
    fake.answer(
        count,
        Answer::rows(
            vec![FakeColumn::new("COUNT(*)", "fixed").precision(18, 0)],
            vec![vec![Some("3".into())]],
        ),
    );
    fake.answer(
        &sf::PIN_SESSION,
        Answer::error("000603", "SQL execution internal error."),
    );
    let sql = "CALL util.setup_session()";
    run(&s, sql, 10).await.unwrap();
    assert!(s.query(count, &[]).await.is_err());
    assert!(!fake.sent().iter().any(|(_, q)| q == count), "not sent");
    fake.status(&sf::PIN_SESSION);
    s.query(count, &[]).await.unwrap();
    let tail: Vec<String> = fake
        .sent()
        .iter()
        .rev()
        .take(3)
        .rev()
        .map(|(_, q)| q.clone())
        .collect();
    assert_eq!(tail, [sf::PIN_SESSION.as_str(), sf::EDITOR_STATE, count]);

    // The editor's own session (no MFA) is the user's to set.
    let fake = fake_with_key().await;
    let s = open(&fake).await;
    let set = "ALTER SESSION SET QUOTED_IDENTIFIERS_IGNORE_CASE = TRUE";
    fake.status(set);
    run(&s, set, 10).await.unwrap();
    assert!(fake.sent().iter().any(|(_, q)| q == set));
    assert!(
        !fake
            .sent()
            .iter()
            .any(|(_, q)| q == sf::PIN_SESSION.as_str())
    );
    // The sign-in sets the grid's input formats, whatever the profile says.
    let login = &fake.logins()[0].body["data"]["SESSION_PARAMETERS"];
    assert_eq!(login["DATE_INPUT_FORMAT"], "AUTO");
    assert_eq!(login["TIMESTAMP_INPUT_FORMAT"], "AUTO");

    // Startup commands run on the app's session too: refused before signing in.
    let logins = fake.logins().len();
    let err = open_with(
        &fake,
        serde_json::json!({}),
        serde_json::json!({ "startupSql": "ALTER SESSION SET QUOTED_IDENTIFIERS_IGNORE_CASE = TRUE" }),
        None,
    )
    .await
    .err()
    .expect("refused");
    assert!(
        err.to_string().contains("QUOTED_IDENTIFIERS_IGNORE_CASE"),
        "{err}"
    );
    assert_eq!(fake.logins().len(), logins);
}

#[tokio::test]
async fn an_imported_profile_never_opens_a_share_or_sends_the_key_elsewhere() {
    let fake = fake_with_key().await;
    // A connections file is saved without the form's checks: the connect checks again.
    for (params, want) in [
        (
            serde_json::json!({ "privateKeyPath": "\\\\attacker\\share\\k.p8" }),
            "not a network share",
        ),
        (
            serde_json::json!({ "privateKeyPath": "/\\attacker\\share\\k.p8" }),
            "not a network share",
        ),
        (
            serde_json::json!({ "baseUrl": "https://attacker.example" }),
            "a key pair signs in only",
        ),
    ] {
        let err = open_with(&fake, params.clone(), serde_json::json!({}), None)
            .await
            .err()
            .expect("refused");
        assert!(err.to_string().contains(want), "{params}: {err}");
    }
    assert!(fake.logins().is_empty(), "nothing signed in");
}

#[tokio::test]
async fn an_alter_session_in_a_transaction_leaves_it_open() {
    let fake = fake(Login::Password {
        password: "pw".into(),
        passcode: None,
    })
    .await;
    let s = open_mfa(&fake).await;
    let insert = "INSERT INTO t VALUES (1)";
    let alter = "ALTER SESSION SET TIMEZONE = 'UTC'";
    fake.status("BEGIN");
    fake.counts(insert, "inserted", 1);
    fake.status(alter);
    run(&s, "BEGIN", 10).await.unwrap();
    assert_eq!(s.in_transaction(), Some(true));
    fake.editor_state(Some("DB"), Some("1700000000000000001"));
    run(&s, insert, 10).await.unwrap();
    // Not DDL: Snowflake keeps the transaction open, and the read-back says so.
    run(&s, alter, 10).await.unwrap();
    assert_eq!(s.in_transaction(), Some(true));
    let sent = fake.sent().len();
    let err = s
        .transaction(vec![
            TxStatement::new("DELETE FROM t WHERE id = ?", vec![Value::Int(1)]).expecting(1),
        ])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("transaction open"), "{err}");
    assert_eq!(fake.sent().len(), sent, "nothing was sent");

    // The read-back wins over the text: DDL the server says left a transaction open.
    let create = "CREATE TEMPORARY TABLE scratch (a INT)";
    fake.status(create);
    run(&s, create, 10).await.unwrap();
    assert_eq!(s.in_transaction(), Some(true));
    // The text decides only when there is no read-back.
    fake.answer(
        sf::EDITOR_STATE,
        Answer::error("000603", "SQL execution internal error."),
    );
    fake.status(sf::COMMIT);
    run(&s, sf::COMMIT, 10).await.unwrap();
    assert_eq!(s.in_transaction(), Some(false));
}

#[tokio::test]
async fn on_a_shared_mfa_session_reads_never_overlap_an_editor_move() {
    let fake = fake(Login::Password {
        password: "pw".into(),
        passcode: None,
    })
    .await;
    let s = Arc::new(open_mfa(&fake).await);
    let count = "SELECT COUNT(*) FROM \"PUBLIC\".\"ITEMS\"";
    let use_prod = "USE DATABASE \"PROD\"";
    fake.answer(use_prod, Answer::Hang);
    // Cancelled, the USE leaves the session where it was.
    fake.editor_state(Some("DB"), None);

    // While the editor's USE runs, a read is refused without waiting.
    let slot = CancelSlot::default();
    let moving = {
        let s = Arc::clone(&s);
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut sink = CollectSink::default();
            s.run_statement(use_prod, 10, &mut sink, slot).await
        })
    };
    let before = fake.queries().len();
    fake.wait_for_queries(before + 1).await;
    for _ in 0..100 {
        if slot.is_armed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let sent = fake.sent().len();
    let err = s.query(count, &[]).await.unwrap_err();
    assert!(err.to_string().contains("try again when it ends"), "{err}");
    let mut sink = CollectSink::default();
    let err = s
        .stream_query(count, &[], &mut sink, CancelSlot::default())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("try again when it ends"), "{err}");
    assert_eq!(fake.sent().len(), sent, "nothing was sent");
    let handle = request_cancel(&slot).expect("the run armed the slot");
    s.cancel(handle).await.unwrap();
    assert!(matches!(
        moving.await.unwrap().unwrap_err(),
        AppError::Cancelled
    ));

    // A read in flight holds the editor's USE back until it has been answered.
    let back = "USE DATABASE \"DB\"";
    fake.status(back);
    fake.answer(count, Answer::Hang);
    let reading = {
        let s = Arc::clone(&s);
        tokio::spawn(async move {
            query_limited(&*s, count, &[], Some(Duration::from_millis(500)))
                .await
                .map(|_| ())
        })
    };
    let before = fake.queries().len();
    fake.wait_for_queries(before + 1).await;
    let moving = {
        let s = Arc::clone(&s);
        tokio::spawn(async move { run(&s, back, 10).await.map(|_| ()) })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!fake.sent().iter().any(|(_, q)| q == back), "the USE waits");
    let err = reading.await.unwrap().unwrap_err();
    assert!(matches!(err, AppError::QueryTimeout { .. }), "{err:?}");
    moving.await.unwrap().unwrap();
    let sent = fake.sent();
    let read_at = sent.iter().position(|(_, q)| q == count);
    let use_at = sent.iter().position(|(_, q)| q == back);
    assert!(read_at < use_at, "{sent:?}");
}

#[tokio::test]
async fn on_a_shared_mfa_session_cancel_ends_an_editor_move_waiting_for_a_read() {
    let fake = fake(Login::Password {
        password: "pw".into(),
        passcode: None,
    })
    .await;
    let s = Arc::new(open_mfa(&fake).await);
    let count = "SELECT COUNT(*) FROM \"PUBLIC\".\"ITEMS\"";
    let use_prod = "USE DATABASE \"PROD\"";
    fake.answer(count, Answer::Hang);
    fake.status(use_prod);
    // A count on a warehouse may take minutes, and holds the editor's USE back.
    let read_slot = CancelSlot::default();
    let reading = {
        let s = Arc::clone(&s);
        let slot = read_slot.clone();
        tokio::spawn(async move { s.query_cancellable(count, &[], slot).await.map(|_| ()) })
    };
    let before = fake.queries().len();
    fake.wait_for_queries(before + 1).await;

    // The user's Cancel ends the waiting run: nothing was armed, and nothing is sent.
    let slot = CancelSlot::default();
    let moving = {
        let s = Arc::clone(&s);
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut sink = CollectSink::default();
            s.run_statement(use_prod, 10, &mut sink, slot).await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!moving.is_finished(), "the USE waits for the read");
    assert!(request_cancel(&slot).is_none());
    let err = tokio::time::timeout(Duration::from_secs(3), moving)
        .await
        .expect("the run ended on Cancel")
        .unwrap()
        .unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");

    // So does the query time out.
    let slot = CancelSlot::default();
    let (result, timed_out) = tokio::time::timeout(
        Duration::from_secs(3),
        crate::commands::query::with_timeout(&*s, &slot, Some(Duration::from_millis(200)), async {
            let mut sink = CollectSink::default();
            s.run_statement(use_prod, 10, &mut sink, slot.clone()).await
        }),
    )
    .await
    .expect("the run ended on the time out");
    assert!(timed_out);
    assert!(matches!(result, Err(AppError::Cancelled)), "{result:?}");
    assert!(
        !fake.sent().iter().any(|(_, q)| q == use_prod),
        "nothing sent"
    );

    // The read was never touched; once it ends, the USE runs.
    let handle = request_cancel(&read_slot).expect("the read armed its slot");
    s.cancel(handle).await.unwrap();
    assert!(matches!(
        reading.await.unwrap().unwrap_err(),
        AppError::Cancelled
    ));
    fake.editor_state(Some("PROD"), None);
    run(&s, use_prod, 10).await.unwrap();
    assert!(fake.sent().iter().any(|(_, q)| q == use_prod));
}

#[tokio::test]
async fn on_a_shared_mfa_session_a_failed_commit_never_lets_the_grid_join_its_transaction() {
    let fake = fake(Login::Password {
        password: "pw".into(),
        passcode: None,
    })
    .await;
    let s = open_mfa(&fake).await;
    let insert = "INSERT INTO t VALUES (1)";
    let count = "SELECT COUNT(*) FROM \"PUBLIC\".\"ITEMS\"";
    let delete = "DELETE FROM t WHERE id = ?";
    let staged = || vec![TxStatement::new(delete, vec![Value::Int(1)]).expecting(1)];
    let open_tx = Some("1700000000000000001");
    let state_fails = || {
        fake.answer(
            sf::EDITOR_STATE,
            Answer::error("000603", "SQL execution internal error."),
        )
    };
    fake.status("BEGIN");
    fake.counts(insert, "inserted", 1);
    fake.counts(delete, "deleted", 1);
    fake.status(sf::BEGIN);
    fake.answer(
        count,
        Answer::rows(
            vec![FakeColumn::new("COUNT(*)", "fixed").precision(18, 0)],
            vec![vec![Some("3".into())]],
        ),
    );
    run(&s, "BEGIN", 10).await.unwrap();
    fake.editor_state(Some("DB"), open_tx);
    run(&s, insert, 10).await.unwrap();
    assert_eq!(s.in_transaction(), Some(true));

    // The COMMIT fails, and so does its read-back: Snowflake may still hold the transaction.
    fake.answer(
        sf::COMMIT,
        Answer::error("000603", "SQL execution internal error."),
    );
    state_fails();
    run(&s, sf::COMMIT, 10).await.unwrap_err();
    assert_eq!(s.in_transaction(), Some(true));

    // A grid read reads the session again, sees the transaction open and keeps it.
    fake.editor_state(Some("DB"), open_tx);
    s.query(count, &[]).await.unwrap();
    assert_eq!(s.in_transaction(), Some(true));
    // The app's commit would commit the editor's INSERT with its own: refused, nothing sent.
    let sent = fake.sent().len();
    let err = s.transaction(staged()).await.unwrap_err();
    assert!(err.to_string().contains("transaction open"), "{err}");
    assert_eq!(fake.sent().len(), sent);

    // A failed statement outside a transaction whose read-back fails leaves it not known...
    fake.status(sf::COMMIT);
    fake.editor_state(Some("DB"), None);
    run(&s, sf::COMMIT, 10).await.unwrap();
    assert_eq!(s.in_transaction(), Some(false));
    let call = "CALL p()";
    fake.answer(call, Answer::error("100132", "JavaScript execution error"));
    state_fails();
    run(&s, call, 10).await.unwrap_err();
    assert_eq!(s.in_transaction(), None);
    // ...until the next read of the app's finds it open (the procedure ran BEGIN).
    fake.editor_state(Some("DB"), open_tx);
    s.query(count, &[]).await.unwrap();
    assert_eq!(s.in_transaction(), Some(true));
    let sent = fake.sent().len();
    assert!(s.transaction(staged()).await.is_err());
    assert_eq!(fake.sent().len(), sent);

    // Found closed, the app's commit goes ahead.
    fake.editor_state(Some("DB"), None);
    run(&s, sf::COMMIT, 10).await.unwrap();
    state_fails();
    run(&s, call, 10).await.unwrap_err();
    assert_eq!(s.in_transaction(), None);
    fake.editor_state(Some("DB"), None);
    assert_eq!(s.transaction(staged()).await.unwrap().len(), 1);
    assert_eq!(s.in_transaction(), Some(false));
}

#[tokio::test]
async fn on_a_shared_mfa_session_a_commit_that_outruns_the_apps_new_sign_in_is_not_reported_lost() {
    let fake = fake(Login::Password {
        password: "pw".into(),
        passcode: None,
    })
    .await;
    let s = Arc::new(open_mfa(&fake).await);
    let insert = "INSERT INTO t VALUES (1)";
    let select = "SELECT 1";
    fake.status("BEGIN");
    fake.counts(insert, "inserted", 1);
    fake.show(select, &["1"], &[&[Some("1")]]);
    fake.answer(
        sf::SHOW_DATABASES,
        Answer::error("390112", "Your session has expired. Please login again."),
    );
    let done = || {
        Answer::rows(
            vec![FakeColumn::new("status", "text")],
            vec![vec![Some("Statement executed successfully.".into())]],
        )
    };

    // Each statement that ends the transaction itself: it was not lost, so nothing says to run
    // it again (a COMMIT run twice applies its writes twice).
    for end in [sf::COMMIT, sf::ROLLBACK, "CREATE TABLE t2 (a INT)"] {
        fake.editor_state(Some("DB"), None);
        run(&s, "BEGIN", 10).await.unwrap();
        fake.editor_state(Some("DB"), Some("1700000000000000001"));
        run(&s, insert, 10).await.unwrap();
        assert_eq!(s.in_transaction(), Some(true), "{end}");

        // The statement is answered late; the app's catalog read meets an expiry meanwhile and
        // signs in again under the transaction the statement is ending.
        fake.answer(
            end,
            Answer::Slow(Duration::from_millis(600), Box::new(done())),
        );
        fake.editor_state(Some("DB"), None);
        let logins = fake.logins().len();
        let before = fake.queries().len();
        let ending = {
            let s = Arc::clone(&s);
            let end = end.to_string();
            tokio::spawn(async move { run(&s, &end, 10).await.map(|_| ()) })
        };
        fake.wait_for_queries(before + 1).await;
        let _ = s.list_databases().await;
        assert!(fake.logins().len() > logins, "{end}: signed in again");
        assert!(!ending.is_finished(), "{end}: still running");
        // Sign-ins are numbered from 1: the editor's was the last before the app's.
        let ran_on = logins;
        ending.await.unwrap().unwrap();
        assert!(
            fake.sent().iter().any(|(n, q)| *n == ran_on && q == end),
            "{end}: ran on the old session: {:?}",
            fake.sent()
        );
        assert_eq!(s.in_transaction(), Some(false), "{end}");

        // The next statement runs, on the new session, with no notice.
        let (out, _) = run(&s, select, 10).await.unwrap();
        assert!(out.notices.is_empty(), "{end}: {:?}", out.notices);
        assert_eq!(
            fake.sent().last().map(|(n, q)| (*n, q.as_str())),
            Some((fake.logins().len(), select)),
            "{end}"
        );
    }
}
