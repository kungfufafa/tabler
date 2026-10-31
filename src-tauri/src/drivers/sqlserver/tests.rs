//! The session against the scripted TDS server (`testing`): connect, introspection, the grid,
//! edits, the editor, cancel and time out, read-only refusals and the hooks, each through the
//! shared code that calls them where there is one (`browse_page`, `fetch_cell_inner`,
//! `query_limited`, `explain_inner`).

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use ::tiberius::EncryptionLevel;

use super::conn::Conn;
use super::connect;
use super::introspect as catalog;
use super::params::MssqlParams;
use super::session::{Connect, MssqlSession};
use super::testing::{
    self, Cell, DONE_COUNT, DONE_MORE, Reply, Request, Server, Ty, done, failed, result,
};
use crate::commands::browse::{browse_page, fetch_cell_inner};
use crate::commands::query::{explain_inner, query_limited};
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, LATE_CANCEL_NOTICE, TxStatement,
    request_cancel,
};
use crate::error::AppError;
use crate::sqlgen::dialect::SqlDialect;
use crate::sqlgen::dialect::tsql;
use crate::sqlgen::narrow::Narrowing;
use crate::sqlgen::portable::testing::{map, text};
use crate::sqlgen::sql::{self, TableTarget};
use crate::types::admin::KillScope;
use crate::types::browse::{
    BrowseRequest, Conjunction, FilterGroup, FilterNode, RowKey, TraceCategory,
};
use crate::types::caps::Feature;
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::{ServerExpr, StagedChange};
use crate::types::objects::{RoutineKind, RoutineRef};
use crate::types::profile::ConnectionProfile;
use crate::types::query::{ExplainMode, StatementKind};
use crate::types::schema::{TableKind, TableMeta, TableRef, ValueKind};
use crate::types::value::Value;

const BIG: i64 = (1 << 53) + 1;

fn profile(options: serde_json::Value) -> ConnectionProfile {
    serde_json::from_value(serde_json::json!({
        "id": "6f9619ff-8b86-d011-b42d-00c04fc964ff", "name": "ms", "color": null,
        "group": null, "driver": "sqlserver", "host": "db.example.com", "port": null,
        "user": "sa", "database": "shop", "sslCaPath": null, "sqlitePath": null, "ssh": null,
        "lastConnectedAt": null, "options": options
    }))
    .expect("a profile")
}

fn plain() -> ConnectionProfile {
    profile(serde_json::json!({}))
}

fn read_only() -> ConnectionProfile {
    profile(serde_json::json!({ "readOnly": true }))
}

/// The session's connections reach `server`, logged in and set up as the driver's are.
fn connector(server: &Server, profile: &ConnectionProfile) -> Connect {
    let server = server.clone();
    let startup = connect::startup_sql(profile).map(str::to_string);
    Arc::new(move || {
        let server = server.clone();
        let startup = startup.clone();
        Box::pin(async move {
            let mut client = server.client().await?;
            connect::ready(&mut client, startup.as_deref()).await?;
            Ok(Conn::spawn(client))
        })
    })
}

fn cols(types: &[Ty]) -> Vec<(&'static str, Ty)> {
    types.iter().map(|t| ("", *t)).collect()
}

fn rowcount(n: i64) -> Vec<u8> {
    let mut tokens = done(DONE_COUNT | DONE_MORE, n.unsigned_abs());
    tokens.extend(result(&[("", Ty::Int)], &[vec![Cell::Int(n)]], false));
    tokens
}

/// A plan, as a server answers under `SHOWPLAN_XML`.
fn plan_set() -> Vec<u8> {
    result(
        &[(tsql::SHOWPLAN_COLUMN, Ty::NVarChar)],
        &[vec![Cell::Text("<ShowPlanXML/>")]],
        false,
    )
}

/// What every connection is asked whatever the test: who the server is, the editor's probe, the
/// keep-alive.
fn standard(r: &Request) -> Reply {
    use Ty::{Int, NVarChar};
    match r {
        Request::Rpc { sql, .. } if sql == catalog::SERVER_INFO => Reply::Tokens(result(
            &cols(&[
                NVarChar, NVarChar, Int, NVarChar, NVarChar, NVarChar, NVarChar, Int,
            ]),
            &[vec![
                Cell::Text("16.0.4135.4"),
                Cell::Text("Microsoft SQL Server 2022"),
                Cell::Int(3),
                Cell::Text("Developer Edition (64-bit)"),
                Cell::Text("shop"),
                Cell::Text("sa"),
                Cell::Text("dbo"),
                Cell::Int(53),
            ]],
            false,
        )),
        Request::Batch(s) if s == tsql::AFTER_BATCH => Reply::Tokens(result(
            &cols(&[Int, Int]),
            &[vec![Cell::Int(0), Cell::Int(0)]],
            false,
        )),
        Request::Batch(s) if s == tsql::PING => {
            Reply::Tokens(result(&cols(&[Int]), &[vec![Cell::Int(1)]], false))
        }
        _ => testing::ok(r),
    }
}

/// A server that answers what `handler` knows, and the rest as `standard`.
fn fake(mut handler: impl FnMut(&Request) -> Option<Reply> + Send + 'static) -> Server {
    Server::new(move |r: &Request| handler(r).unwrap_or_else(|| standard(r)))
}

async fn session(server: &Server, profile: &ConnectionProfile) -> MssqlSession {
    MssqlSession::start(profile, &MssqlParams::default(), connector(server, profile))
        .await
        .expect("a session")
}

/// Waits until the server got a request matching `f`.
async fn until(server: &Server, f: impl Fn(&Request) -> bool) {
    for _ in 0..500 {
        if server.seen.requests().iter().any(&f) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "the server never got the request: {:?}",
        server.seen.requests()
    );
}

fn batch(sql: &str) -> Request {
    Request::Batch(sql.into())
}

fn is_batch(r: &Request, sql: &str) -> bool {
    matches!(r, Request::Batch(s) if s == sql)
}

fn sent(server: &Server, needle: &str) -> bool {
    server.seen.requests().iter().any(|r| match r {
        Request::Batch(s) | Request::Rpc { sql: s, .. } => s.contains(needle),
        _ => false,
    })
}

// --- The table the grid tests use: [dbo].[Orders] (id bigint identity key, name, data) ---------

const OBJECT: &str = "[dbo].[Orders]";

fn column(
    id: i64,
    name: &'static str,
    ty: &'static str,
    len: i64,
    key: bool,
) -> Vec<Cell<'static>> {
    let seed = if key { Cell::Text("1") } else { Cell::Null };
    vec![
        Cell::Int(id),
        Cell::Text(name),
        Cell::Text(ty),
        Cell::Null,
        Cell::Int(len),
        Cell::Int(0),
        Cell::Int(0),
        Cell::Bit(!key),
        Cell::Bit(key),
        Cell::Bit(false),
        Cell::Int(0),
        Cell::Null,
        Cell::Null,
        Cell::Null,
        Cell::Null,
        seed,
        seed,
        if key {
            Cell::Text("The key")
        } else {
            Cell::Null
        },
        Cell::Bit(false),
    ]
}

/// The catalog's answers about [dbo].[Orders], or None.
fn orders(r: &Request) -> Option<Reply> {
    use Ty::{BigInt, Bit, Int, NVarChar};
    let Request::Rpc { sql, params } = r else {
        return None;
    };
    if params.first().map(String::as_str) != Some(OBJECT) {
        return None;
    }
    let s = sql.as_str();
    Some(Reply::Tokens(if s == catalog::TABLE {
        result(
            &cols(&[NVarChar, NVarChar, BigInt]),
            &[vec![
                Cell::Text("U "),
                Cell::Text("All orders"),
                Cell::Int(3),
            ]],
            false,
        )
    } else if s == catalog::COLUMNS {
        result(
            &cols(&[
                Int, NVarChar, NVarChar, NVarChar, Int, Int, Int, Bit, Bit, Bit, Int, NVarChar,
                NVarChar, NVarChar, Bit, NVarChar, NVarChar, NVarChar, Bit,
            ]),
            &[
                column(1, "id", "bigint", 8, true),
                column(2, "name", "nvarchar", 100, false),
                column(3, "data", "varbinary", -1, false),
            ],
            false,
        )
    } else if s == catalog::INDEXES {
        result(
            &cols(&[
                Int, NVarChar, NVarChar, Bit, Bit, NVarChar, Bit, NVarChar, NVarChar,
            ]),
            &[vec![
                Cell::Int(1),
                Cell::Text("PK_Orders"),
                Cell::Text("CLUSTERED"),
                Cell::Bit(true),
                Cell::Bit(true),
                Cell::Null,
                Cell::Bit(false),
                Cell::Text("id"),
                Cell::Null,
            ]],
            false,
        )
    } else if s == catalog::DEFINITION {
        result(&cols(&[NVarChar]), &[vec![Cell::Null]], false)
    } else {
        // No foreign keys, checks or triggers.
        done(DONE_COUNT, 0)
    }))
}

async fn orders_meta(session: &MssqlSession) -> TableMeta {
    session
        .table_meta(&TableRef::new(Some("dbo"), "Orders"))
        .await
        .expect("the table's structure")
}

// --- Connect and introspection ------------------------------------------------------------------

#[tokio::test]
async fn connects_runs_the_startup_commands_and_reports_the_server() {
    let server = fake(|_| None);
    let p = profile(serde_json::json!({ "startupSql": "SET LANGUAGE us_english" }));
    let s = session(&server, &p).await;
    let requests = server.seen.requests();
    assert!(matches!(&requests[0], Request::Login { application, .. } if application == "Tabler"));
    assert_eq!(requests[1], batch(tsql::SESSION_SETUP));
    assert_eq!(requests[2], batch("SET LANGUAGE us_english"));
    assert!(matches!(&requests[3], Request::Rpc { sql, .. } if sql == catalog::SERVER_INFO));
    let info = s.server_info().await.unwrap();
    assert_eq!(info.version, "SQL Server 2022 (16.0.4135.4)");
    assert_eq!(info.current_database.as_deref(), Some("shop"));
    assert_eq!(info.current_user.as_deref(), Some("sa"));
    s.ping().await.unwrap();
    // The catalog reads are in the console log as structure.
    let trace = s.drain_trace();
    assert!(
        trace
            .iter()
            .all(|t| t.category == TraceCategory::Structure && t.error.is_none())
    );
    assert_eq!(trace.len(), 2);
    // A startup command that fails ends the connect.
    let broken = fake(|r| match r {
        Request::Batch(s) if s == "SET NOPE ON" => Some(Reply::Tokens(failed(
            195,
            1,
            "'NOPE' is not a recognized SET option.",
        ))),
        _ => None,
    });
    let p = profile(serde_json::json!({ "startupSql": "SET NOPE ON" }));
    let refused = MssqlSession::start(&p, &MssqlParams::default(), connector(&broken, &p)).await;
    assert!(
        matches!(&refused, Err(AppError::Connect(m)) if m.contains("startup commands") && m.contains("NOPE")),
        "{:?}",
        refused.err()
    );
}

#[tokio::test]
async fn the_caps_grant_what_the_driver_implements() {
    let server = fake(|_| None);
    let s = session(&server, &plain()).await;
    let caps = s.capabilities();
    assert_eq!(caps.dialect, Some(SqlDialect::Tsql));
    for f in [
        Feature::Editor,
        Feature::Explain,
        Feature::ExplainAnalyze,
        Feature::CancelOnServer,
        Feature::EditRows,
        Feature::InsertRows,
        Feature::DeleteRows,
        Feature::ServerExpressions,
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
    ] {
        assert!(caps.has(f), "{f:?}");
    }
    assert_eq!(caps.features.len(), 18, "{:?}", caps.features);
    // Neither the core-only features nor what is left out on purpose.
    for f in [
        Feature::Returning,
        Feature::EditWithoutKey,
        Feature::Mcp,
        Feature::WorkspaceCommit,
        Feature::StructureEdit,
        Feature::Users,
        Feature::ExportTables,
        Feature::Backup,
        Feature::Metrics,
    ] {
        assert!(!caps.has(f), "{f:?}");
    }
    assert_eq!(caps.safety.read_only, ReadOnlyMode::ClassifierOnly);
    assert!(!caps.safety.hardened);
    assert_eq!(caps.session.keepalive_secs, Some(30));
    assert_eq!(caps.namespaces.default_schema.as_deref(), Some("dbo"));
    assert!(caps.data.default_keyword && !caps.data.upsert);
    assert_eq!(caps.data.max_params, 2098);
    // The renderer's base caps for the driver say less, never more.
    let base =
        crate::types::caps::SessionCaps::for_driver(crate::types::profile::Driver::Sqlserver);
    assert!(base.features.iter().all(|f| caps.has(*f)));
}

/// The session's connections reach `server` with the login `Dial` builds from the profile
/// (`connect::build_config`), encryption aside: the fake speaks no TLS.
fn dialer(server: &Server, profile: &ConnectionProfile) -> Connect {
    let server = server.clone();
    let profile = profile.clone();
    Arc::new(move || {
        let server = server.clone();
        let profile = profile.clone();
        Box::pin(async move {
            let endpoint = connect::Endpoint::of(&ConnectTarget {
                profile: &profile,
                password: None,
                host_override: None,
            });
            let mut config = connect::build_config(
                &profile,
                &MssqlParams::default(),
                Some("pw"),
                &endpoint,
                Duration::from_secs(5),
            )?;
            config.encryption(EncryptionLevel::NotSupported);
            let mut client = server.client_with(config).await?;
            connect::ready(&mut client, None).await?;
            Ok(Conn::spawn(client))
        })
    })
}

/// Switch Database (`commands::connections::switch_database_on`) opens a new session from the
/// profile with the other database: every connection of it logs in there.
#[tokio::test]
async fn a_switched_session_logs_in_to_the_other_database() {
    let server = fake(|_| None);
    let shop = plain();
    let mut archive = shop.clone();
    archive.database = Some("archive".into());
    let mut none = shop.clone();
    none.database = Some(" ".into());
    for p in [&shop, &archive, &none] {
        let s = MssqlSession::start(p, &MssqlParams::default(), dialer(&server, p))
            .await
            .expect("a session");
        // The editor's connection and a worker's.
        s.query("SELECT 1", &[]).await.unwrap();
        s.close().await;
    }
    let logins: Vec<String> = server
        .seen
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            Request::Login { database, .. } => Some(database),
            _ => None,
        })
        .collect();
    assert_eq!(logins, ["shop", "shop", "archive", "archive", "", ""]);
}

#[tokio::test]
async fn lists_databases_schemas_tables_and_structure() {
    use Ty::{BigInt, Int, NVarChar};
    let server = fake(|r| {
        let Request::Rpc { sql, params } = r else {
            return None;
        };
        if sql == catalog::DATABASES {
            return Some(Reply::Tokens(result(
                &cols(&[NVarChar, NVarChar, NVarChar, Int]),
                &[
                    vec![
                        Cell::Text("master"),
                        Cell::Text("sa"),
                        Cell::Null,
                        Cell::Int(0),
                    ],
                    vec![
                        Cell::Text("shop"),
                        Cell::Text("sa"),
                        Cell::Null,
                        Cell::Int(1),
                    ],
                ],
                false,
            )));
        }
        if sql == catalog::SCHEMAS {
            return Some(Reply::Tokens(result(
                &cols(&[NVarChar, Int]),
                &[vec![Cell::Text("dbo"), Cell::Int(1)]],
                false,
            )));
        }
        if sql == catalog::TABLES && params == &["dbo".to_string()] {
            return Some(Reply::Tokens(result(
                &cols(&[NVarChar, NVarChar, NVarChar, BigInt, BigInt]),
                &[vec![
                    Cell::Text("Orders"),
                    Cell::Text("U "),
                    Cell::Null,
                    Cell::Int(3),
                    Cell::Int(16384),
                ]],
                false,
            )));
        }
        orders(r)
    });
    let s = session(&server, &plain()).await;
    let dbs = s.list_databases().await.unwrap();
    assert_eq!(dbs.len(), 2);
    assert!(dbs[1].is_current && dbs[1].name == "shop");
    let schemas = s.list_schemas().await.unwrap();
    assert!(schemas[0].is_default);
    let tables = s.list_tables("dbo").await.unwrap();
    assert_eq!(tables[0].name, "Orders");
    assert_eq!(tables[0].row_estimate, Some(3));
    let meta = orders_meta(&s).await;
    assert_eq!(meta.kind, TableKind::Table);
    assert_eq!(meta.primary_key, ["id"]);
    assert!(meta.columns[0].is_pk && meta.columns[0].is_auto_increment);
    assert_eq!(meta.columns[1].data_type, "nvarchar(50)");
    assert_eq!(meta.columns[2].kind, ValueKind::Bytes);
    assert_eq!(meta.comment.as_deref(), Some("All orders"));
    // Every catalog query took the table's quoted name.
    let named = server
        .seen
        .requests()
        .iter()
        .filter(|r| matches!(r, Request::Rpc { params, .. } if params == &[OBJECT.to_string()]))
        .count();
    assert_eq!(named, 6);
}

// --- The grid ------------------------------------------------------------------------------------

/// The page the grid reads: its rows as the server sends them, the big integer past 2^53 and a
/// binary value beyond the grid's preview.
fn grid_server() -> Server {
    let blob = vec![7u8; 70_000];
    fake(move |r| {
        let Request::Rpc { sql, params } = r else {
            return None;
        };
        let typed = [
            ("id", Ty::BigInt),
            ("name", Ty::NVarChar),
            ("data", Ty::VarBinaryMax),
        ];
        if sql
            == "SELECT [id], [name], [data]\nFROM [dbo].[Orders] ORDER BY [id] ASC\nOFFSET 2 \
                ROWS FETCH NEXT 2 ROWS ONLY"
        {
            return Some(Reply::Tokens(result(
                &typed,
                &[
                    vec![Cell::Int(BIG), Cell::Text("b"), Cell::Bytes(&blob)],
                    vec![Cell::Int(4), Cell::Null, Cell::Null],
                ],
                false,
            )));
        }
        if sql == "SELECT COUNT_BIG(*) FROM [dbo].[Orders]" {
            return Some(Reply::Tokens(result(
                &[("", Ty::BigInt)],
                &[vec![Cell::Int(12)]],
                false,
            )));
        }
        if sql == "SELECT TOP (1) [data] FROM [dbo].[Orders] WHERE [id] = @P1"
            && params == &[BIG.to_string()]
        {
            return Some(Reply::Tokens(result(
                &[("data", Ty::VarBinaryMax)],
                &[vec![Cell::Bytes(&blob)]],
                false,
            )));
        }
        orders(r)
    })
}

#[tokio::test]
async fn browses_counts_and_fetches_a_whole_cell() {
    let server = grid_server();
    let s = session(&server, &plain()).await;
    let meta = orders_meta(&s).await;
    let req = BrowseRequest {
        table: meta.table.clone(),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit: 2,
        offset: 2,
        cursor: None,
    };
    let page = browse_page(&s, &meta, &req, None).await.unwrap();
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.rows[0][0], Value::BigInt(BIG.to_string()));
    assert_eq!(page.rows[0][1], Value::Text("b".into()));
    match &page.rows[0][2] {
        Value::Bytes(b) => assert!(b.truncated && b.len == 70_000, "the grid's preview"),
        other => panic!("{other:?}"),
    }
    assert_eq!(page.rows[1], [Value::Int(4), Value::Null, Value::Null]);

    let narrowing = Narrowing::for_caps(&s.capabilities());
    let count = sql::count_on(
        &narrowing,
        SqlDialect::Tsql,
        &meta.table,
        &meta.columns,
        None,
    )
    .unwrap();
    let rows = s
        .query_cancellable(&count.sql, &count.params, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(rows.rows, [[Value::Int(12)]]);

    let key = RowKey::Pk(map([("id", Value::BigInt(BIG.to_string()))]));
    match fetch_cell_inner(&s, &meta, "data", &key).await.unwrap() {
        Value::Bytes(b) => assert!(!b.truncated && b.len == 70_000, "the whole value"),
        other => panic!("{other:?}"),
    }
}

fn update(meta: &TableMeta, id: i64, name: &str) -> TxStatement {
    let change = StagedChange::Update {
        row_id: "r".into(),
        key: RowKey::Pk(map([("id", Value::Int(id))])),
        changes: map([("name", text(name))]),
        exprs: None,
    };
    statement(meta, &change)
}

#[tokio::test]
async fn a_commit_with_one_failing_change_rolls_everything_back() {
    let server = fake(|r| match r {
        Request::Rpc { sql, params } if sql.ends_with("SELECT @@ROWCOUNT") => Some(Reply::Tokens(
            if params.get(1).map(String::as_str) == Some("2") {
                failed(
                    547,
                    1,
                    "The UPDATE statement conflicted with the CHECK constraint.",
                )
            } else {
                rowcount(1)
            },
        )),
        _ => orders(r),
    });
    let s = session(&server, &plain()).await;
    let meta = orders_meta(&s).await;
    s.drain_trace();
    let ok = s
        .transaction(vec![update(&meta, 1, "a"), update(&meta, 3, "c")])
        .await
        .unwrap();
    assert_eq!(ok.iter().map(|o| o.affected).collect::<Vec<_>>(), [1, 1]);
    assert_eq!(
        server.seen.requests().last(),
        Some(&Request::Transaction(7))
    );

    let err = s
        .transaction(vec![update(&meta, 1, "a"), update(&meta, 2, "b")])
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AppError::Db {
                vendor_code: Some(547),
                statement_index: Some(1),
                ..
            }
        ),
        "{err:?}"
    );
    let requests = server.seen.requests();
    let last_begin = requests
        .iter()
        .rposition(|r| r == &Request::Transaction(5))
        .unwrap();
    assert!(requests[last_begin..].contains(&Request::Transaction(8)));
    assert!(!requests[last_begin..].contains(&Request::Transaction(7)));
    // The console log shows each statement as it ran, the failed one with its error.
    let trace = s.drain_trace();
    assert_eq!(trace.len(), 4);
    assert_eq!(trace[2].affected, Some(1));
    assert!(
        trace[3]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("CHECK"))
    );
    assert!(trace[3].sql.contains("N'b'"), "{}", trace[3].sql);
}

fn statement(meta: &TableMeta, change: &StagedChange) -> TxStatement {
    let narrowing = Narrowing::for_caps(&crate::types::caps::SessionCaps::for_driver(
        crate::types::profile::Driver::Sqlserver,
    ));
    let target = TableTarget {
        table: &meta.table,
        columns: &meta.columns,
    };
    let (built, expect) =
        sql::change_on(&narrowing, SqlDialect::Tsql, &target, change, false).unwrap();
    let mut st = TxStatement::new(built.sql, built.params);
    st.expect_affected = expect;
    st
}

/// Inserts, deletes and DEFAULT, each guarded by `@@ROWCOUNT` before the commit: a delete that
/// would remove two rows rolls everything back.
#[tokio::test]
async fn inserts_deletes_and_defaults() {
    let server = fake(|r| match r {
        Request::Rpc { sql, params } if sql.ends_with("SELECT @@ROWCOUNT") => {
            let twins =
                sql.starts_with("DELETE") && params.first().map(String::as_str) == Some("9");
            Some(Reply::Tokens(rowcount(if twins { 2 } else { 1 })))
        }
        _ => orders(r),
    });
    let s = session(&server, &plain()).await;
    let meta = orders_meta(&s).await;
    let insert = StagedChange::Insert {
        row_id: "n".into(),
        values: map([("name", text("new")), ("id", Value::Default)]),
        exprs: None,
    };
    let delete = |id: i64| StagedChange::Delete {
        row_id: "d".into(),
        key: RowKey::Pk(map([("id", Value::Int(id))])),
    };
    let default = StagedChange::Update {
        row_id: "u".into(),
        key: RowKey::Pk(map([("id", Value::Int(6))])),
        changes: std::collections::BTreeMap::new(),
        exprs: Some([("name".to_string(), ServerExpr::Default)].into()),
    };
    let done = s
        .transaction(vec![
            statement(&meta, &insert),
            statement(&meta, &delete(5)),
            statement(&meta, &default),
        ])
        .await
        .unwrap();
    assert_eq!(done.len(), 3);
    assert!(sent(
        &server,
        "INSERT INTO [dbo].[Orders] ([name]) VALUES (@P1)"
    ));
    assert!(sent(&server, "DELETE FROM [dbo].[Orders] WHERE [id] = @P1"));
    assert!(sent(&server, "SET [name] = DEFAULT WHERE [id] = @P1"));
    assert!(server.seen.requests().contains(&Request::Transaction(7)));
    let err = s
        .transaction(vec![
            statement(&meta, &insert),
            statement(&meta, &delete(9)),
        ])
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Db { message, statement_index: Some(1), .. } if message.contains("matched 2")),
        "{err:?}"
    );
    assert_eq!(
        server.seen.requests().last(),
        Some(&Request::Batch(tsql::ROLLBACK_IF_OPEN.into()))
    );
    // A row of a table without a key is not edited: the builders address rows by key only.
    let keyless = StagedChange::Delete {
        row_id: "k".into(),
        key: RowKey::AllColumns(map([("id", Value::Int(7))])),
    };
    let target = TableTarget {
        table: &meta.table,
        columns: &meta.columns,
    };
    let narrowing = Narrowing::for_caps(&s.capabilities());
    assert!(sql::change_on(&narrowing, SqlDialect::Tsql, &target, &keyless, false).is_err());
    assert!(!s.capabilities().has(Feature::EditWithoutKey));
}

#[tokio::test]
async fn a_lone_dml_statement_is_counted_by_rowcount() {
    let server = fake(|r| match r {
        Request::Rpc { sql, .. } if sql == "DELETE FROM t WHERE a = @P1;\nSELECT @@ROWCOUNT" => {
            Some(Reply::Tokens(rowcount(2)))
        }
        _ => None,
    });
    let s = session(&server, &plain()).await;
    let out = s
        .execute("DELETE FROM t WHERE a = @P1", &[Value::Int(1)])
        .await
        .unwrap();
    assert_eq!(out.affected, 2);
    assert!(!out.affected_unknown);
    let out = s
        .execute_cancellable("CREATE TABLE u (a int)", &[], CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(out.affected, 0);
    assert!(server.seen.requests().contains(&Request::Rpc {
        sql: "CREATE TABLE u (a int)".into(),
        params: Vec::new(),
    }));
}

// --- The editor ----------------------------------------------------------------------------------

#[tokio::test]
async fn the_editor_runs_batches_and_tracks_the_transaction() {
    let open = Arc::new(AtomicI64::new(0));
    let last = Arc::new(std::sync::Mutex::new(String::new()));
    let server = {
        let open = Arc::clone(&open);
        fake(move |r| {
            let Request::Batch(s) = r else {
                return None;
            };
            let mut last = last.lock().unwrap();
            if s == tsql::AFTER_BATCH {
                let rows = if last.starts_with("UPDATE") { 3 } else { 0 };
                return Some(Reply::Tokens(result(
                    &cols(&[Ty::Int, Ty::Int]),
                    &[vec![
                        Cell::Int(rows),
                        Cell::Int(open.load(Ordering::SeqCst)),
                    ]],
                    false,
                )));
            }
            *last = s.clone();
            match s.as_str() {
                "SELECT 1 AS a\nSELECT 'x' AS b" => {
                    let mut tokens = result(&[("a", Ty::Int)], &[vec![Cell::Int(1)]], true);
                    tokens.extend(result(
                        &[("b", Ty::NVarChar)],
                        &[vec![Cell::Text("x")]],
                        false,
                    ));
                    Some(Reply::Tokens(tokens))
                }
                "BEGIN TRAN\nUPDATE t SET a = 1" => {
                    open.store(1, Ordering::SeqCst);
                    Some(Reply::Tokens(done(DONE_COUNT, 3)))
                }
                "UPDATE t SET a = 1" => Some(Reply::Tokens(done(DONE_COUNT, 3))),
                "COMMIT" => {
                    open.store(0, Ordering::SeqCst);
                    None
                }
                _ => None,
            }
        })
    };
    let s = session(&server, &plain()).await;
    let mut sink = CollectSink::default();
    let out = s
        .run_statement(
            "SELECT 1 AS a\nSELECT 'x' AS b",
            100,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(out.kind, StatementKind::Rows);
    assert_eq!((out.result_sets, out.row_count), (2, 2));
    assert_eq!(sink.sets[1].rows, [[Value::Text("x".into())]]);
    assert_eq!(s.in_transaction(), Some(false));

    // Two statements: no count of their own, but the transaction they opened.
    let out = s
        .run_statement(
            "BEGIN TRAN\nUPDATE t SET a = 1",
            100,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(out.kind, StatementKind::Empty);
    assert_eq!(s.in_transaction(), Some(true));
    // A lone DML statement reports its rows.
    let out = s
        .run_statement(
            "UPDATE t SET a = 1",
            100,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(out.kind, StatementKind::Affected);
    assert_eq!(out.affected, Some(3));
    s.run_statement(
        "COMMIT",
        100,
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .unwrap();
    assert_eq!(s.in_transaction(), Some(false));
    // Each batch went whole, the probe after it, all on the editor's one connection.
    assert_eq!(server.logins(), 1);
    let requests = server.seen.requests();
    let probes = requests
        .iter()
        .filter(|r| is_batch(r, tsql::AFTER_BATCH))
        .count();
    assert_eq!(probes, 4);
}

#[tokio::test]
async fn the_row_cap_stops_a_read_and_drains_anything_else() {
    let rows: Vec<Vec<Cell<'static>>> = (0..5).map(|i| vec![Cell::Int(i)]).collect();
    let server = fake(move |r| match r {
        Request::Batch(s) if s == "SELECT n FROM t" => {
            let mut tokens = testing::columns(&[("n", Ty::Int)]);
            for row in &rows {
                tokens.extend(testing::row(&[Ty::Int], row));
            }
            Some(Reply::Hang(tokens))
        }
        Request::Batch(s) if s == "SELECT n FROM t\nUPDATE t SET n = 0" => {
            let mut tokens = result(&[("n", Ty::Int)], &rows, true);
            tokens.extend(done(DONE_COUNT, 5));
            Some(Reply::Tokens(tokens))
        }
        _ => None,
    });
    let s = session(&server, &plain()).await;
    let mut sink = CollectSink::default();
    let out = s
        .run_statement("SELECT n FROM t", 2, &mut sink, CancelSlot::default())
        .await
        .unwrap();
    assert!(out.truncated);
    assert_eq!(out.row_count, 2);
    assert_eq!(sink.sets[0].rows.len(), 2);
    assert_eq!(
        server.seen.attentions(),
        1,
        "the read stopped on the server"
    );

    let mut sink = CollectSink::default();
    let out = s
        .run_statement(
            "SELECT n FROM t\nUPDATE t SET n = 0",
            2,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert!(out.truncated && out.row_count == 2);
    assert_eq!(
        server.seen.attentions(),
        1,
        "a batch that writes runs to its end"
    );
}

#[tokio::test]
async fn cancel_stops_the_editor_statement_on_the_server() {
    const SLOW: &str = "WAITFOR DELAY '01:00'";
    let server = fake(|r| match r {
        Request::Batch(s) if s == SLOW => Some(Reply::Hang(Vec::new())),
        _ => None,
    });
    let s = Arc::new(session(&server, &plain()).await);
    let slot = CancelSlot::default();
    let running = {
        let s = Arc::clone(&s);
        let slot = slot.clone();
        tokio::spawn(async move {
            s.run_statement(SLOW, 100, &mut CollectSink::default(), slot)
                .await
        })
    };
    until(&server, |r| is_batch(r, SLOW)).await;
    let handle = request_cancel(&slot).expect("the statement armed the slot");
    s.cancel(handle).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("the cancel returns")
        .expect("the task ends");
    assert!(matches!(outcome, Err(AppError::Cancelled)), "{outcome:?}");
    assert_eq!(server.seen.attentions(), 1);
    // The editor goes on.
    s.run_statement(
        "SELECT 1",
        10,
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .unwrap();
    s.ping().await.unwrap();
}

#[tokio::test]
async fn a_time_out_cancels_the_grid_read_on_the_server() {
    let server = fake(|r| match r {
        Request::Rpc { sql, .. } if sql == "SELECT slowly" => Some(Reply::Hang(Vec::new())),
        _ => None,
    });
    let s = session(&server, &plain()).await;
    let outcome = query_limited(&s, "SELECT slowly", &[], Some(Duration::from_millis(200))).await;
    assert!(
        matches!(outcome, Err(AppError::QueryTimeout { .. })),
        "{outcome:?}"
    );
    assert_eq!(server.seen.attentions(), 1);
    // The worker was not broken by it: it serves the next read.
    let logins = server.logins();
    s.query("SELECT 1", &[]).await.unwrap();
    assert_eq!(server.logins(), logins);
}

#[tokio::test]
async fn explain_runs_the_plan_in_batches_of_its_own() {
    use crate::types::query::ExplainMode::{Analyze, Plan};
    let plan = || {
        result(
            &[(tsql::SHOWPLAN_COLUMN, Ty::NVarChar)],
            &[vec![Cell::Text("<ShowPlanXML/>")]],
            false,
        )
    };
    // As a server answers under each option: the plan alone, or the rows and then the plan.
    let mut option = "";
    let server = fake(move |r| {
        let Request::Batch(s) = r else {
            return None;
        };
        match s.as_str() {
            "SET SHOWPLAN_XML ON" => option = "plan",
            "SET STATISTICS XML ON" => option = "statistics",
            "SET SHOWPLAN_XML OFF" | "SET STATISTICS XML OFF" => option = "",
            "SELECT * FROM t" | tsql::PLAN_PROBE if option == "plan" => {
                return Some(Reply::Tokens(plan()));
            }
            "SELECT * FROM t" => {
                let mut tokens = result(&[("a", Ty::Int)], &[vec![Cell::Int(1)]], true);
                if option == "statistics" {
                    tokens.extend(plan());
                } else {
                    tokens.extend(done(DONE_COUNT, 0));
                }
                return Some(Reply::Tokens(tokens));
            }
            "SELECT nope" => {
                return Some(Reply::Tokens(failed(207, 1, "Invalid column name 'nope'.")));
            }
            _ => {}
        }
        None
    });
    let s = session(&server, &plain()).await;
    for mode in [Plan, Analyze] {
        let explained = explain_inner(&s, "SELECT * FROM t", mode, &CancelSlot::default())
            .await
            .unwrap();
        // Only the plan, never the statement's own rows.
        assert_eq!(
            explained.text.as_deref(),
            Some("<ShowPlanXML/>"),
            "{mode:?}"
        );
        assert_eq!(explained.columns.len(), 1);
    }
    let batches: Vec<String> = server
        .seen
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            Request::Batch(s) if s != tsql::AFTER_BATCH => Some(s),
            _ => None,
        })
        .skip(1)
        .collect();
    assert_eq!(
        batches,
        [
            "SET SHOWPLAN_XML ON",
            tsql::PLAN_PROBE,
            "SELECT * FROM t",
            "SET SHOWPLAN_XML OFF",
            "BEGIN TRANSACTION",
            "SET STATISTICS XML ON",
            "SELECT * FROM t",
            "SET STATISTICS XML OFF",
            "ROLLBACK TRANSACTION",
        ]
    );
    // An error points into the statement, past the prefix.
    let err = explain_inner(&s, "SELECT nope", Plan, &CancelSlot::default())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Db { vendor_code: Some(207), position: Some(p), .. } if p.offset == 0),
        "{err:?}"
    );
    // A user's own `SET STATISTICS XML ON <statement>` runs as written.
    s.run_statement(
        "SET STATISTICS XML ON SELECT * FROM t",
        10,
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .unwrap();
    assert!(sent(&server, "SET STATISTICS XML ON SELECT * FROM t"));
}

#[tokio::test]
async fn a_lost_editor_ends_the_session() {
    let server = fake(|r| match r {
        Request::Batch(s) if s == "SELECT boom" => Some(Reply::Close),
        _ => None,
    });
    let s = session(&server, &plain()).await;
    let lost = s
        .run_statement(
            "SELECT boom",
            10,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await;
    assert!(matches!(lost, Err(AppError::NotConnected)), "{lost:?}");
    assert!(matches!(s.ping().await, Err(AppError::NotConnected)));
    let again = s
        .run_statement(
            "SELECT 1",
            10,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await;
    assert!(matches!(again, Err(AppError::NotConnected)));
    assert_eq!(s.in_transaction(), None);
}

/// Plan output that cannot be turned off would leave every later batch only planned: the
/// editor's connection is given up, and the session with it.
#[tokio::test]
async fn an_editor_left_in_plan_output_is_given_up() {
    let server = fake(|r| match r {
        Request::Batch(s) if s == "SET SHOWPLAN_XML OFF" => Some(Reply::Tokens(failed(
            102,
            1,
            "Incorrect syntax near 'OFF'.",
        ))),
        Request::Batch(s) if s == "SELECT * FROM t" || s == tsql::PLAN_PROBE => {
            Some(Reply::Tokens(plan_set()))
        }
        _ => None,
    });
    let s = session(&server, &plain()).await;
    let explained = explain_inner(
        &s,
        "SELECT * FROM t",
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert_eq!(explained.text.as_deref(), Some("<ShowPlanXML/>"));
    // Nothing more went to it, and its socket closed, which ends the server's session.
    let batches: Vec<Request> = server
        .seen
        .requests()
        .into_iter()
        .filter(|r| matches!(r, Request::Batch(_)))
        .collect();
    assert_eq!(batches.last(), Some(&batch("SET SHOWPLAN_XML OFF")));
    for _ in 0..500 {
        if server.seen.ended() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(server.seen.ended(), "the editor's connection is still open");
    assert!(matches!(s.ping().await, Err(AppError::NotConnected)));
    let again = s
        .run_statement(
            "SELECT 1",
            10,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await;
    assert!(matches!(again, Err(AppError::NotConnected)), "{again:?}");
    assert_eq!(server.logins(), 1, "no connection took its place");
}

#[tokio::test]
async fn workers_are_reused_and_a_broken_one_replaced() {
    let server = fake(|r| match r {
        Request::Rpc { sql, .. } if sql == "SELECT broken" => Some(Reply::Close),
        _ => None,
    });
    let s = session(&server, &plain()).await;
    s.query("SELECT 1", &[]).await.unwrap();
    s.query("SELECT 2", &[]).await.unwrap();
    assert_eq!(server.logins(), 2, "the editor and one worker");
    assert!(matches!(
        s.query("SELECT broken", &[]).await,
        Err(AppError::NotConnected)
    ));
    s.query("SELECT 3", &[]).await.unwrap();
    assert_eq!(
        server.logins(),
        3,
        "a new worker took the broken one's place"
    );
}

#[tokio::test]
async fn close_stops_the_editor_and_returns_quickly() {
    const SLOW: &str = "WAITFOR DELAY '01:00'";
    let server = fake(|r| match r {
        Request::Batch(s) if s == SLOW => Some(Reply::Hang(Vec::new())),
        _ => None,
    });
    let s = Arc::new(session(&server, &plain()).await);
    s.query("SELECT 1", &[]).await.unwrap();
    let running = {
        let s = Arc::clone(&s);
        tokio::spawn(async move {
            s.run_statement(
                SLOW,
                100,
                &mut CollectSink::default(),
                CancelSlot::default(),
            )
            .await
        })
    };
    until(&server, |r| is_batch(r, SLOW)).await;
    tokio::time::timeout(Duration::from_millis(2_500), s.close())
        .await
        .expect("close returns within its 2 s");
    let outcome = running.await.expect("the run ends");
    assert!(outcome.is_err(), "{outcome:?}");
    assert_eq!(server.seen.attentions(), 1);
    assert!(matches!(
        s.query("SELECT 1", &[]).await,
        Err(AppError::NotConnected)
    ));
}

/// A server that never answers the Attention holds the editor past the budget: close still
/// returns at 2 s, having closed the idle worker meanwhile.
#[tokio::test(start_paused = true)]
async fn close_keeps_to_its_2_s_when_the_server_stops_answering() {
    const STUCK: &str = "SELECT stuck";
    let server = fake(|r| match r {
        Request::Batch(s) if s == STUCK => Some(Reply::Stuck(Vec::new())),
        _ => None,
    });
    let s = Arc::new(session(&server, &plain()).await);
    s.query("SELECT 1", &[]).await.unwrap();
    let running = {
        let s = Arc::clone(&s);
        tokio::spawn(async move {
            s.run_statement(
                STUCK,
                100,
                &mut CollectSink::default(),
                CancelSlot::default(),
            )
            .await
        })
    };
    until(&server, |r| is_batch(r, STUCK)).await;
    let started = tokio::time::Instant::now();
    s.close().await;
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(2) && took < Duration::from_millis(2_100),
        "{took:?}"
    );
    assert_eq!(server.seen.attentions(), 1);
    // The worker's connection ended; the editor's waits for its Attention.
    assert!(server.seen.ended());
    assert!(!running.is_finished());
    running.abort();
}

/// The probe after a batch answers at once. A cancel that comes while it runs stops it on the
/// server (the batch's outcome stands, its transaction is then unknown); a probe that goes
/// unanswered gives the editor's connection up within 10 s instead of holding the run until TCP
/// gives up.
#[tokio::test(start_paused = true)]
async fn the_probe_after_a_batch_can_be_cancelled_and_gives_up_in_time() {
    const UPDATE: &str = "UPDATE t SET a = 1";
    let probe = Arc::new(AtomicI64::new(0));
    let server = {
        let probe = Arc::clone(&probe);
        fake(move |r| match r {
            Request::Batch(s) if s == tsql::AFTER_BATCH => match probe.load(Ordering::SeqCst) {
                1 => Some(Reply::Hang(Vec::new())),
                2 => Some(Reply::Stuck(Vec::new())),
                _ => None,
            },
            _ => None,
        })
    };
    let s = Arc::new(session(&server, &plain()).await);
    probe.store(1, Ordering::SeqCst);
    let slot = CancelSlot::default();
    let running = {
        let s = Arc::clone(&s);
        let slot = slot.clone();
        tokio::spawn(async move {
            s.run_statement(UPDATE, 10, &mut CollectSink::default(), slot)
                .await
        })
    };
    until(&server, |r| is_batch(r, tsql::AFTER_BATCH)).await;
    let handle = request_cancel(&slot).expect("the probe armed the slot");
    s.cancel(handle).await.unwrap();
    let out = running.await.expect("the run ends").unwrap();
    assert_eq!(out.kind, StatementKind::Affected);
    assert_eq!(out.affected, None, "the probe was stopped");
    assert!(out.notices.iter().any(|n| n == LATE_CANCEL_NOTICE));
    assert_eq!(server.seen.attentions(), 1);
    assert_eq!(s.in_transaction(), None);
    // The connection is clean: the next run reads the state again.
    probe.store(0, Ordering::SeqCst);
    s.run_statement(
        "SELECT 1",
        10,
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .unwrap();
    assert_eq!(s.in_transaction(), Some(false));

    probe.store(2, Ordering::SeqCst);
    let started = tokio::time::Instant::now();
    let out = s
        .run_statement(
            UPDATE,
            10,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(10) && took < Duration::from_secs(11),
        "{took:?}"
    );
    assert_eq!(out.affected, None);
    assert_eq!(s.in_transaction(), None);
    assert!(matches!(s.ping().await, Err(AppError::NotConnected)));
    for _ in 0..100 {
        if server.seen.attentions() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(server.seen.attentions(), 2, "the stalled probe was stopped");
}

/// Plan output that does not go off in time is given up like plan output that cannot go off.
#[tokio::test(start_paused = true)]
async fn a_switch_of_plan_output_that_goes_unanswered_gives_the_editor_up() {
    let server = fake(|r| match r {
        Request::Batch(s) if s == "SET SHOWPLAN_XML OFF" => Some(Reply::Stuck(Vec::new())),
        Request::Batch(s) if s == "SELECT * FROM t" || s == tsql::PLAN_PROBE => {
            Some(Reply::Tokens(plan_set()))
        }
        _ => None,
    });
    let s = session(&server, &plain()).await;
    let started = tokio::time::Instant::now();
    let explained = explain_inner(
        &s,
        "SELECT * FROM t",
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(10) && took < Duration::from_secs(11),
        "{took:?}"
    );
    assert_eq!(explained.text.as_deref(), Some("<ShowPlanXML/>"));
    assert!(matches!(s.ping().await, Err(AppError::NotConnected)));
}

/// A server that takes `SET SHOWPLAN_XML ON` and still runs statements (Babelfish, by default)
/// never gets the statement of a Plan, on a read-only session or a writable one; a Plan that
/// brings no plan back fails.
#[tokio::test]
async fn a_server_that_ignores_plan_output_never_gets_the_statement() {
    let server = fake(|r| match r {
        Request::Batch(s) if s == tsql::PLAN_PROBE => Some(Reply::Tokens(result(
            &[("plan_probe", Ty::Int)],
            &[vec![Cell::Int(1)]],
            false,
        ))),
        Request::Batch(s) if s == "DELETE FROM t" => Some(Reply::Tokens(done(DONE_COUNT, 5))),
        _ => None,
    });
    let plan = format!("{} DELETE FROM t", tsql::explain_prefix(ExplainMode::Plan));
    for profile in [read_only(), plain()] {
        let s = session(&server, &profile).await;
        let err = explain_inner(
            &s,
            "DELETE FROM t",
            ExplainMode::Plan,
            &CancelSlot::default(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, AppError::Unsupported(m) if m.contains("still runs statements")),
            "{err:?}"
        );
        let run = s
            .run_statement(
                &plan,
                10,
                &mut CollectSink::default(),
                CancelSlot::default(),
            )
            .await;
        assert!(matches!(run, Err(AppError::Unsupported(_))), "{run:?}");
        // Plan output went off again, and the editor still serves.
        s.run_statement(
            "SELECT 1",
            10,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await
        .unwrap();
    }
    assert!(!sent(&server, "DELETE"), "the statement was sent");
    let switches: Vec<String> = server
        .seen
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            Request::Batch(s) if s.contains("SHOWPLAN") || s == tsql::PLAN_PROBE => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(
        switches,
        [
            "SET SHOWPLAN_XML ON",
            tsql::PLAN_PROBE,
            "SET SHOWPLAN_XML OFF"
        ]
        .repeat(4)
    );

    // Plan output that is on, and a statement that brings no plan back.
    let server = fake(|r| match r {
        Request::Batch(s) if s == tsql::PLAN_PROBE => Some(Reply::Tokens(plan_set())),
        _ => None,
    });
    let s = session(&server, &plain()).await;
    let err = explain_inner(
        &s,
        "SET NOCOUNT ON",
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, AppError::Unsupported(m) if m.contains("no plan")),
        "{err:?}"
    );
    assert!(sent(&server, "SET SHOWPLAN_XML OFF"));
}

// --- Read-only ------------------------------------------------------------------------------------

#[tokio::test]
async fn read_only_refuses_every_write_before_it_is_sent() {
    let server = fake(|r| match r {
        Request::Batch(s) if s == tsql::PLAN_PROBE || s == "DELETE FROM t" => {
            Some(Reply::Tokens(plan_set()))
        }
        _ => None,
    });
    let s = session(&server, &read_only()).await;
    let refused = |r: Result<(), AppError>, what: &str| match r {
        Err(AppError::InvalidInput(m)) => assert_eq!(m, READ_ONLY_MESSAGE, "{what}"),
        other => panic!("{what}: {other:?}"),
    };
    refused(s.query("DELETE FROM t", &[]).await.map(|_| ()), "query");
    refused(
        s.query_cancellable("SELECT 1 DELETE FROM t", &[], CancelSlot::default())
            .await
            .map(|_| ()),
        "a write after a read",
    );
    refused(
        s.execute("UPDATE t SET a = 1", &[]).await.map(|_| ()),
        "execute",
    );
    refused(
        s.execute_cancellable("TRUNCATE TABLE t", &[], CancelSlot::default())
            .await
            .map(|_| ()),
        "execute_cancellable",
    );
    refused(
        s.transaction(vec![
            TxStatement::new("SELECT 1", Vec::new()),
            TxStatement::new("INSERT INTO t VALUES (1)", Vec::new()),
        ])
        .await
        .map(|_| ()),
        "transaction",
    );
    for sql in [
        "INSERT INTO t VALUES (1)",
        "EXEC sp_who",
        "SELECT * INTO t2 FROM t",
        "SELECT * FROM OPENQUERY(s, 'DELETE FROM t')",
        "SELECT * FROM t WITH (UPDLOCK)",
        "SET IMPLICIT_TRANSACTIONS ON",
    ] {
        refused(
            s.run_statement(sql, 10, &mut CollectSink::default(), CancelSlot::default())
                .await
                .map(|_| ()),
            sql,
        );
    }
    let analyze = format!(
        "{} DELETE FROM t",
        tsql::explain_prefix(ExplainMode::Analyze)
    );
    refused(
        s.run_statement(
            &analyze,
            10,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await
        .map(|_| ()),
        "explain analyze of a write",
    );
    // The plan of a DML statement that reaches another server must read: compiling a distributed
    // query has the linked server or provider describe its text, which some do by running it.
    // Through the `explain` command's path, whose own checks let these through.
    for statement in [
        "DELETE FROM t WHERE id IN (SELECT id FROM \
         OPENQUERY(srv, 'EXEC dbo.purge; SELECT 1 AS id'))",
        "DELETE FROM OPENQUERY(srv, 'SELECT * FROM remote_t')",
        "INSERT t VALUES ((SELECT TOP (1) a FROM OPENROWSET('SQLNCLI', \
         'Server=.;Trusted_Connection=yes', 'SET FMTONLY OFF EXEC dbo.purge SELECT 1 a')))",
        "DELETE FROM t WHERE EXISTS (SELECT 1 FROM OPENDATASOURCE('SQLNCLI', \
         'Data Source=x;Integrated Security=SSPI').db.dbo.t2)",
        "DELETE FROM t WHERE 1 = (SELECT master.dbo.xp_foo())",
    ] {
        refused(
            explain_inner(&s, statement, ExplainMode::Plan, &CancelSlot::default())
                .await
                .map(|_| ()),
            statement,
        );
    }
    // So must the plan of anything but one DML statement: a `SET` among it could take effect.
    for statement in [
        "SET SHOWPLAN_XML OFF DELETE FROM t",
        "EXEC('DELETE FROM t')",
        "DELETE FROM t SHUTDOWN WITH NOWAIT",
        "DELETE FROM t RECONFIGURE WITH OVERRIDE",
        "INSERT t VALUES (1) CHECKPOINT",
        "UPDATE t SET a = 1 SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        "UPDATE t SET a = 1 SET CONTEXT_INFO 0x01",
        "DELETE FROM t REVERT",
        "DELETE FROM t WITH (TABLOCKX)",
    ] {
        let plan = format!("{} {statement}", tsql::explain_prefix(ExplainMode::Plan));
        refused(
            s.run_statement(
                &plan,
                10,
                &mut CollectSink::default(),
                CancelSlot::default(),
            )
            .await
            .map(|_| ()),
            &plan,
        );
    }
    refused(
        s.stream_query(
            "UPDATE t SET a = 1",
            &[],
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await
        .map(|_| ()),
        "stream_query",
    );
    // A read that advances a sequence, which no rollback undoes: refused on every path,
    // re-spaced and re-cased too, as PostgreSQL's read-only transaction refuses `nextval`.
    const NEXT: &str = "SELECT NEXT VALUE FOR dbo.s";
    refused(
        s.query(NEXT, &[]).await.map(|_| ()),
        "query of NEXT VALUE FOR",
    );
    refused(
        query_limited(&s, NEXT, &[], Some(Duration::from_secs(5)))
            .await
            .map(|_| ()),
        "query_limited of NEXT VALUE FOR",
    );
    refused(
        s.execute(NEXT, &[]).await.map(|_| ()),
        "execute of NEXT VALUE FOR",
    );
    refused(
        s.execute_cancellable(NEXT, &[], CancelSlot::default())
            .await
            .map(|_| ()),
        "execute_cancellable of NEXT VALUE FOR",
    );
    refused(
        s.transaction(vec![TxStatement::new(NEXT, Vec::new())])
            .await
            .map(|_| ()),
        "transaction of NEXT VALUE FOR",
    );
    refused(
        s.stream_query(
            NEXT,
            &[],
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await
        .map(|_| ()),
        "stream_query of NEXT VALUE FOR",
    );
    for sql in [
        NEXT,
        "SELECT next   value\n/* x */ for [dbo].[r1]",
        "SELECT a FROM t WHERE id = NEXT VALUE FOR s",
    ] {
        refused(
            s.run_statement(sql, 10, &mut CollectSink::default(), CancelSlot::default())
                .await
                .map(|_| ()),
            sql,
        );
    }
    for mode in [ExplainMode::Plan, ExplainMode::Analyze] {
        refused(
            explain_inner(&s, NEXT, mode, &CancelSlot::default())
                .await
                .map(|_| ()),
            &format!("{mode:?} of NEXT VALUE FOR"),
        );
    }
    let killed = s
        .kill_processes(&["53".to_string()], KillScope::Connection)
        .await
        .unwrap();
    assert!(!killed[0].ok && killed[0].message.as_deref() == Some(READ_ONLY_MESSAGE));
    // None of it reached the server, nor did plan output go on.
    for needle in [
        "DELETE",
        "UPDATE",
        "INSERT",
        "TRUNCATE",
        "EXEC",
        "INTO",
        "IMPLICIT_TRANSACTIONS ON",
        "KILL",
        "OPENQUERY",
        "OPENROWSET",
        "OPENDATASOURCE",
        "xp_foo",
        "SHUTDOWN",
        "RECONFIGURE",
        "CHECKPOINT",
        "CONTEXT_INFO",
        "REVERT",
        "TABLOCKX",
        "SHOWPLAN",
        "NEXT VALUE",
        "next",
    ] {
        assert!(!sent(&server, needle), "{needle} was sent");
    }
    // Reads still run, and so do the wrapper of an Explain Analyze and the plan of a write,
    // which is not run.
    s.query("SELECT 1", &[]).await.unwrap();
    let plan_of_a_write = format!("{} DELETE FROM t", tsql::explain_prefix(ExplainMode::Plan));
    for sql in [
        "SELECT 1",
        "BEGIN TRANSACTION",
        "ROLLBACK TRANSACTION",
        plan_of_a_write.as_str(),
    ] {
        s.run_statement(sql, 10, &mut CollectSink::default(), CancelSlot::default())
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    assert!(sent(&server, "SET SHOWPLAN_XML ON"));
}

#[tokio::test]
async fn a_read_only_session_refuses_startup_commands_that_write() {
    let server = fake(|_| None);
    for startup in ["UPDATE t SET a = 1", "SELECT NEXT VALUE FOR dbo.st"] {
        let p = profile(serde_json::json!({ "readOnly": true, "startupSql": startup }));
        let refused =
            MssqlSession::start(&p, &MssqlParams::default(), connector(&server, &p)).await;
        assert!(
            matches!(&refused, Err(AppError::InvalidInput(m)) if m.contains("read-only")),
            "{startup}"
        );
    }
    assert_eq!(server.logins(), 0, "nothing was opened");
}

/// The grid's raw WHERE goes through the same refusal.
#[tokio::test]
async fn a_read_only_grid_refuses_a_filter_that_advances_a_sequence() {
    let server = grid_server();
    let s = session(&server, &read_only()).await;
    let meta = orders_meta(&s).await;
    let req = BrowseRequest {
        table: meta.table.clone(),
        columns: None,
        filter: Some(FilterGroup {
            conjunction: Conjunction::And,
            items: vec![FilterNode::Raw {
                sql: "[id] = NEXT VALUE FOR dbo.s".into(),
            }],
        }),
        sort: Vec::new(),
        limit: 2,
        offset: 0,
        cursor: None,
    };
    match browse_page(&s, &meta, &req, None).await {
        Err(AppError::InvalidInput(m)) => assert_eq!(m, READ_ONLY_MESSAGE),
        other => panic!("{other:?}"),
    }
    assert!(!sent(&server, "NEXT VALUE"));
    // Without the filter the page reads.
    let req = BrowseRequest {
        filter: None,
        ..req
    };
    browse_page(&s, &meta, &req, None).await.unwrap();
}

/// The grid's raw WHERE is the user's text, and `sp_executesql` runs every statement of it: one
/// that adds a statement is refused on a writable session too (Safe Mode never sees a browse),
/// and so is any read the app sends that holds one.
#[tokio::test]
async fn a_raw_filter_cannot_add_a_statement() {
    let server = grid_server();
    let s = session(&server, &plain()).await;
    let meta = orders_meta(&s).await;
    for raw in [
        concat!(
            "1=1) DELETE FROM [dbo].[Orders] WHERE (1=1) ",
            "SELECT [id], [name], [data] FROM [dbo].[Orders] WHERE (1=1"
        ),
        "1=1 {call dbo.purge}",
        "1=1) DUMP TRANSACTION shop WITH NO_LOG SELECT 1 WHERE (1=1",
    ] {
        let req = BrowseRequest {
            table: meta.table.clone(),
            columns: None,
            filter: Some(FilterGroup {
                conjunction: Conjunction::And,
                items: vec![FilterNode::Raw { sql: raw.into() }],
            }),
            sort: Vec::new(),
            limit: 2,
            offset: 0,
            cursor: None,
        };
        match browse_page(&s, &meta, &req, None).await {
            Err(AppError::InvalidInput(m)) => assert!(m.contains("another statement"), "{m}"),
            other => panic!("{raw}: {other:?}"),
        }
    }
    for sql in [
        "SELECT 1 DELETE FROM t",
        "SELECT 1 {CALL p}",
        "SELECT 1 { ? = call p }",
        "SELECT 1 LOAD DATABASE x",
    ] {
        let r = s.query(sql, &[]).await;
        assert!(matches!(r, Err(AppError::InvalidInput(_))), "{sql}: {r:?}");
    }
    for needle in ["DELETE", "purge", "DUMP", "LOAD", "CALL", "call"] {
        assert!(!sent(&server, needle), "{needle} was sent");
    }
    // A raw filter that only narrows the rows still reads.
    let req = BrowseRequest {
        table: meta.table.clone(),
        columns: None,
        filter: Some(FilterGroup {
            conjunction: Conjunction::And,
            items: vec![FilterNode::Raw {
                sql: "[name] LIKE N'a%'".into(),
            }],
        }),
        sort: Vec::new(),
        limit: 2,
        offset: 0,
        cursor: None,
    };
    browse_page(&s, &meta, &req, None).await.unwrap();
}

/// Exports stream through `stream_query`: a table export carries the grid's raw WHERE, and a
/// query export is the user's text cut only at `GO`. One that adds a statement is refused on a
/// writable session too, before anything is sent.
#[tokio::test]
async fn an_export_cannot_add_a_statement() {
    let server = grid_server();
    let s = session(&server, &plain()).await;
    let meta = orders_meta(&s).await;
    let filter = |raw: &str| FilterGroup {
        conjunction: Conjunction::And,
        items: vec![FilterNode::Raw { sql: raw.into() }],
    };
    let attack = filter("1=1) DELETE FROM [dbo].[Orders] WHERE (1=1");
    let table = crate::export::table_query(
        SqlDialect::Tsql,
        &meta,
        None,
        Some(&attack),
        &[],
        None,
        0,
        false,
    )
    .unwrap();
    let mut queries = vec![table];
    for sql in [
        "SELECT 1;DELETE FROM t",
        "SELECT 1 DELETE FROM [dbo].[Orders]",
    ] {
        queries.push(crate::export::sql_query(sql, SqlDialect::Tsql).unwrap());
    }
    for q in &queries {
        match s
            .stream_query(
                &q.sql,
                &q.params,
                &mut CollectSink::default(),
                CancelSlot::default(),
            )
            .await
        {
            Err(AppError::InvalidInput(m)) => assert!(m.contains("another statement"), "{m}"),
            other => panic!("{}: {other:?}", q.sql),
        }
    }
    assert!(!sent(&server, "DELETE"), "DELETE was sent");
    // An export whose filter only narrows the rows still streams.
    let narrow = filter("[name] LIKE N'a%'");
    let q = crate::export::table_query(
        SqlDialect::Tsql,
        &meta,
        None,
        Some(&narrow),
        &[],
        None,
        0,
        false,
    )
    .unwrap();
    s.stream_query(
        &q.sql,
        &q.params,
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .unwrap();
}

// --- Hooks ----------------------------------------------------------------------------------------

#[tokio::test]
async fn processes_and_kill() {
    use Ty::{Int, NVarChar};
    let server = fake(|r| match r {
        Request::Rpc { sql, .. } if sql == catalog::PROCESSES => Some(Reply::Tokens(result(
            &cols(&[
                Int, NVarChar, NVarChar, NVarChar, NVarChar, NVarChar, NVarChar, NVarChar, Int,
                NVarChar, NVarChar, Int,
            ]),
            &[vec![
                Cell::Int(53),
                Cell::Text("sa"),
                Cell::Text("shop"),
                Cell::Text("Tabler"),
                Cell::Text("WS1"),
                Cell::Text("running"),
                Cell::Text("SELECT"),
                Cell::Text("2024-01-02T10:11:12.123"),
                Cell::Int(3),
                Cell::Null,
                Cell::Text("SELECT 1"),
                Cell::Int(1),
            ]],
            false,
        ))),
        _ => None,
    });
    let s = session(&server, &plain()).await;
    let processes = s.list_processes().await.unwrap().unwrap();
    assert_eq!(processes[0].id, "53");
    assert!(processes[0].current && processes[0].own);
    let killed = s
        .kill_processes(&["53".to_string(), "x".to_string()], KillScope::Connection)
        .await
        .unwrap();
    assert!(killed[0].ok && !killed[1].ok);
    assert!(server.seen.requests().contains(&batch("KILL 53")));
    let query = s
        .kill_processes(&["54".to_string()], KillScope::Query)
        .await
        .unwrap();
    assert!(
        !query[0].ok
            && query[0]
                .message
                .as_deref()
                .is_some_and(|m| m.contains("Kill Connection"))
    );
    assert!(!sent(&server, "KILL 54"));
}

#[tokio::test]
async fn show_ddl_routines_and_session_details() {
    use Ty::{Bit, Int, NVarChar};
    let server = fake(|r| match r {
        Request::Rpc { sql, params }
            if sql == catalog::ROUTINES && params == &["dbo".to_string()] =>
        {
            Some(Reply::Tokens(result(
                &cols(&[NVarChar, NVarChar, NVarChar]),
                &[vec![Cell::Text("load"), Cell::Text("P "), Cell::Null]],
                false,
            )))
        }
        Request::Rpc { sql, .. } if sql == catalog::ROUTINE_PARAMETERS => {
            Some(Reply::Tokens(result(
                &cols(&[NVarChar, Int, NVarChar, NVarChar, Int, Int, Int, Bit, Bit]),
                &[vec![
                    Cell::Text("load"),
                    Cell::Int(1),
                    Cell::Text("@n"),
                    Cell::Text("int"),
                    Cell::Int(4),
                    Cell::Int(10),
                    Cell::Int(0),
                    Cell::Bit(false),
                    Cell::Bit(false),
                ]],
                false,
            )))
        }
        Request::Rpc { sql, params }
            if sql == catalog::DEFINITION && params == &["[dbo].[load]".to_string()] =>
        {
            Some(Reply::Tokens(result(
                &cols(&[NVarChar]),
                &[vec![Cell::Text(
                    "CREATE PROCEDURE load @n int AS SELECT @n",
                )]],
                false,
            )))
        }
        Request::Rpc { sql, .. } if sql == catalog::SESSION_TLS => Some(Reply::Tokens(result(
            &cols(&[NVarChar]),
            &[vec![Cell::Text("TRUE")]],
            false,
        ))),
        _ => orders(r),
    });
    let s = session(&server, &plain()).await;
    let ddl = s
        .table_ddl(&TableRef::new(Some("dbo"), "Orders"))
        .await
        .unwrap()
        .unwrap();
    assert!(
        ddl.starts_with(
            "CREATE TABLE [dbo].[Orders] (\n    [id] bigint IDENTITY(1,1) NOT NULL,\n    \
             [name] nvarchar(50) NULL,\n    [data] varbinary(max) NULL,\n    CONSTRAINT \
             [PK_Orders] PRIMARY KEY CLUSTERED ([id])\n);\n"
        ),
        "{ddl}"
    );
    assert!(ddl.contains("@value = N'All orders'") && ddl.contains("@level2name = N'id'"));
    let routines = s.list_routines("dbo").await.unwrap().unwrap();
    assert_eq!(routines[0].kind, RoutineKind::Procedure);
    assert_eq!(routines[0].arguments, "@n int");
    let routine = RoutineRef {
        schema: Some("dbo".into()),
        name: "load".into(),
        kind: RoutineKind::Procedure,
        identity_arguments: String::new(),
    };
    assert!(
        s.routine_definition(&routine)
            .await
            .unwrap()
            .unwrap()
            .starts_with("CREATE PROCEDURE")
    );
    // A view of the same catalog with no text (WITH ENCRYPTION) says so.
    let orders_ref = RoutineRef {
        name: "Orders".into(),
        ..routine
    };
    assert!(
        s.routine_definition(&orders_ref)
            .await
            .unwrap()
            .unwrap()
            .contains("WITH ENCRYPTION")
    );
    assert_eq!(
        s.view_definition(&TableRef::new(Some("dbo"), "Orders"))
            .await
            .unwrap(),
        None
    );
    let details = s.session_details().await.unwrap().unwrap();
    assert!(details.tls);
    assert_eq!(details.encoding.as_deref(), Some("UTF-16"));
}

const RECENT: &str = "CREATE VIEW dbo.Recent AS SELECT id FROM dbo.Orders WHERE id > 100";

/// The catalog's answers about two views: [dbo].[Recent], whose text the server keeps, and
/// [dbo].[Secret], created WITH ENCRYPTION.
fn views(r: &Request) -> Option<Reply> {
    use Ty::{BigInt, NVarChar};
    let Request::Rpc { sql, params } = r else {
        return None;
    };
    let definition = match params.first().map(String::as_str) {
        Some("[dbo].[Recent]") => Cell::Text(RECENT),
        Some("[dbo].[Secret]") => Cell::Null,
        _ => return None,
    };
    Some(Reply::Tokens(if sql == catalog::TABLE {
        result(
            &cols(&[NVarChar, NVarChar, BigInt]),
            &[vec![Cell::Text("V "), Cell::Null, Cell::Null]],
            false,
        )
    } else if sql == catalog::DEFINITION {
        result(&cols(&[NVarChar]), &[vec![definition]], false)
    } else {
        // Its columns do not matter here.
        done(DONE_COUNT, 0)
    }))
}

#[tokio::test]
async fn show_ddl_and_the_definition_of_a_view() {
    let server = fake(views);
    let s = session(&server, &plain()).await;
    let recent = TableRef::new(Some("dbo"), "Recent");
    assert_eq!(s.table_meta(&recent).await.unwrap().kind, TableKind::View);
    // A view's DDL is its own definition, not a CREATE TABLE.
    assert_eq!(s.table_ddl(&recent).await.unwrap().unwrap(), RECENT);
    assert_eq!(
        s.view_definition(&recent).await.unwrap().as_deref(),
        Some(RECENT)
    );
    let secret = TableRef::new(Some("dbo"), "Secret");
    assert!(
        s.table_ddl(&secret)
            .await
            .unwrap()
            .unwrap()
            .contains("WITH ENCRYPTION")
    );
    assert_eq!(s.view_definition(&secret).await.unwrap(), None);
}
