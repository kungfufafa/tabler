//! The session against the scripted server (`testing::sql_script`): connect and setup, TLS, the
//! catalog, the grid, commits, the editor, cancel and time out, read-only and the admin hooks.

use std::sync::Arc;
use std::time::Duration;

use super::catalog;
use super::session::{FEATURES, VerticaSession};
use super::testing::{self, Answer, Params, Request, Script, Seen, failure, rows};
use super::types;
use crate::commands::browse::{browse_page, fetch_cell_inner};
use crate::commands::query::query_limited;
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, TxStatement, request_cancel,
};
use crate::error::AppError;
use crate::sqlgen::dialect::{SqlDialect, vertica};
use crate::sqlgen::narrow::Narrowing;
use crate::sqlgen::portable::testing::{map, text};
use crate::sqlgen::sql::{self, TableTarget};
use crate::types::admin::KillScope;
use crate::types::browse::{BrowseRequest, RowKey};
use crate::types::caps::Feature;
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::StagedChange;
use crate::types::objects::{RoutineKind, RoutineRef};
use crate::types::profile::ConnectionProfile;
use crate::types::query::StatementKind;
use crate::types::schema::{TableKind, TableMeta, TableRef, ValueKind};
use crate::types::value::Value;

const BIG: &str = "9007199254740993";

fn profile(port: u16, fields: serde_json::Value) -> ConnectionProfile {
    let mut base = serde_json::json!({
        "id": "6f9619ff-8b86-d011-b42d-00c04fc964ff", "name": "ve", "color": null,
        "group": null, "driver": "vertica", "host": "127.0.0.1", "port": port,
        "user": "dbadmin", "database": "VMart", "ssl": "disable", "sslCaPath": null,
        "sqlitePath": null, "ssh": null, "lastConnectedAt": null,
        "options": { "connectTimeoutSecs": 5 }
    });
    for (k, v) in fields.as_object().expect("fields") {
        if k == "options" {
            for (ok, ov) in v.as_object().expect("options") {
                base["options"][ok] = ov.clone();
            }
        } else {
            base[k] = v.clone();
        }
    }
    serde_json::from_value(base).expect("a profile")
}

async fn open(p: &ConnectionProfile, password: Option<&str>) -> Result<VerticaSession, AppError> {
    VerticaSession::open(&ConnectTarget {
        profile: p,
        password,
        host_override: None,
    })
    .await
}

/// A fake answering `handler`, and a session on it (`fields` change the profile).
async fn session_with(
    fields: serde_json::Value,
    handler: impl Fn(&str, Params<'_>) -> Option<Answer> + Send + Sync + 'static,
) -> (VerticaSession, Seen, ConnectionProfile) {
    let (port, seen) = testing::serve(testing::sql_script(handler)).await;
    let p = profile(port, fields);
    let s = open(&p, None).await.expect("the session opens");
    (s, seen, p)
}

fn queries(seen: &Seen) -> Vec<String> {
    seen.requests()
        .into_iter()
        .filter_map(|r| match r {
            Request::Query(sql) => Some(sql),
            Request::Execute { sql, .. } => Some(sql),
            _ => None,
        })
        .collect()
}

fn sent(seen: &Seen, needle: &str) -> bool {
    queries(seen).iter().any(|q| q.contains(needle))
}

async fn until(seen: &Seen, needle: &str) {
    for _ in 0..500 {
        if sent(seen, needle) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("{needle} was never sent: {:?}", seen.requests());
}

// --- The orders table ---------------------------------------------------------------------------

const BOOL: u32 = types::BOOL;
const INT: u32 = types::INT8;
const VC: u32 = types::VARCHAR;

/// The catalog's answers about `public.orders` (id INT primary key, name VARCHAR(80), data
/// VARBINARY) and its grid.
fn orders(sql: &str, params: Params<'_>) -> Option<Answer> {
    let p = |i: usize| {
        params
            .and_then(|p| p.get(i).cloned().flatten())
            .unwrap_or_default()
    };
    if sql == catalog::TABLE {
        if params.is_some_and(|_| p(1) != "orders") {
            return Some(rows(&[("n", VC), ("k", VC), ("c", VC)], &[]));
        }
        return Some(rows(
            &[("n", VC), ("k", VC), ("c", VC)],
            &[&["orders", "table", "Every order"]],
        ));
    }
    if sql == catalog::TABLES {
        return Some(rows(
            &[("n", VC), ("k", VC), ("c", VC)],
            &[
                &["orders", "table", "NULL"],
                &["recent", "view", "NULL"],
                &["staging", "flex", "NULL"],
            ],
        ));
    }
    if sql == catalog::TABLE_SIZES {
        return Some(rows(
            &[("n", VC), ("rows", INT), ("bytes", INT)],
            &[&["orders", "12", "4096"]],
        ));
    }
    if sql == catalog::COLUMNS {
        let cols = [
            ("column_name", VC),
            ("data_type", VC),
            ("data_type_id", INT),
            ("len", INT),
            ("prec", INT),
            ("scale", INT),
            ("is_nullable", BOOL),
            ("column_default", VC),
            ("is_identity", BOOL),
            ("ordinal_position", INT),
            ("column_set_using", VC),
        ];
        return Some(rows(
            &cols,
            &[
                &[
                    "id", "int", "6", "NULL", "NULL", "NULL", "f", "NULL", "t", "1", "NULL",
                ],
                &[
                    "name",
                    "varchar(80)",
                    "9",
                    "80",
                    "NULL",
                    "NULL",
                    "t",
                    "NULL",
                    "f",
                    "2",
                    "NULL",
                ],
                &[
                    "data",
                    "varbinary(80)",
                    "17",
                    "80",
                    "NULL",
                    "NULL",
                    "t",
                    "NULL",
                    "f",
                    "3",
                    "NULL",
                ],
            ],
        ));
    }
    if sql == catalog::COLUMN_COMMENTS {
        return Some(rows(
            &[("o", VC), ("c", VC)],
            &[&["orders.name", "Who ordered"]],
        ));
    }
    if sql == catalog::PRIMARY_KEY {
        return Some(rows(
            &[("c", VC), ("col", VC), ("pos", VC), ("on", BOOL)],
            &[&["C_PRIMARY", "id", "1", "t"]],
        ));
    }
    if sql == catalog::FOREIGN_KEYS || sql == catalog::CONSTRAINTS {
        return Some(rows(&[("x", VC)], &[]));
    }
    if sql == catalog::PROJECTIONS {
        return Some(rows(
            &[
                ("p", VC),
                ("super", BOOL),
                ("seg", BOOL),
                ("expr", VC),
                ("col", VC),
                ("sort", INT),
                ("pos", INT),
            ],
            &[&["orders_super", "t", "t", "hash(orders.id)", "id", "0", "0"]],
        ));
    }
    if sql.starts_with("SELECT") && sql.contains("\"public\".\"orders\"") {
        if sql.to_ascii_uppercase().contains("COUNT") {
            return Some(rows(&[("count", INT)], &[&["12"]]));
        }
        if !sql.contains("WHERE") {
            return Some(rows(
                &[("id", INT), ("name", VC), ("data", types::VARBINARY)],
                &[&[BIG, "b", "\\001\\002"], &["4", "NULL", "NULL"]],
            ));
        }
        return Some(rows(&[("data", types::VARBINARY)], &[&["\\001\\002"]]));
    }
    None
}

async fn orders_meta(s: &VerticaSession) -> TableMeta {
    s.table_meta(&TableRef {
        schema: Some("public".into()),
        name: "orders".into(),
    })
    .await
    .expect("the orders table")
}

// --- Connect -------------------------------------------------------------------------------------

#[tokio::test]
async fn connects_sets_up_each_connection_and_reports_the_server() {
    let (s, seen, _) = session_with(
        serde_json::json!({
            "options": {
                "timezone": "Asia/Jakarta",
                "startupSql": "SET SEARCH_PATH TO app, public;",
                "driverParams": { "clientLabel": "reports", "workload": "etl" }
            }
        }),
        |_, _| None,
    )
    .await;
    let info = s.server_info().await.unwrap();
    assert_eq!(info.version, "Vertica Analytic Database 24.1.0-0");
    assert_eq!(info.current_database.as_deref(), Some("VMart"));
    assert_eq!(info.current_user.as_deref(), Some("dbadmin"));
    s.ping().await.unwrap();
    // The editor and one worker logged in, each set up the same way, in autocommit.
    let startups = seen.startups();
    assert_eq!(startups.len(), 2, "{startups:?}");
    for st in &startups {
        assert_eq!(st["user"], "dbadmin");
        assert_eq!(st["database"], "VMart");
        assert_eq!(st["client_label"], "reports");
        assert_eq!(st["client_type"], "Tabler");
        assert_eq!(st["workload"], "etl");
        assert_eq!(st["autocommit"], "on");
    }
    let q = queries(&seen);
    let setup = [
        "SET SEARCH_PATH TO app, public",
        "SET TIME ZONE TO 'Asia/Jakarta'",
    ];
    assert_eq!(q.iter().filter(|x| x.as_str() == setup[0]).count(), 2);
    assert_eq!(q.iter().filter(|x| x.as_str() == setup[1]).count(), 2);
    assert!(!sent(&seen, vertica::READ_ONLY_SESSION));
    assert_eq!(s.in_transaction(), Some(false));
}

#[tokio::test]
async fn the_caps_grant_what_the_driver_implements() {
    let (s, _, _) = session_with(serde_json::json!({}), |_, _| None).await;
    let caps = s.capabilities();
    for f in FEATURES {
        assert!(caps.has(f), "{f:?}");
    }
    for f in [
        Feature::EditWithoutKey,
        Feature::Returning,
        Feature::ExplainAnalyze,
        Feature::ServerExpressions,
        Feature::StructureEdit,
        Feature::SwitchDatabase,
        Feature::Users,
        Feature::Sequences,
    ] {
        assert!(!caps.has(f), "{f:?}");
    }
    assert_eq!(caps.dialect, Some(SqlDialect::Vertica));
    assert_eq!(caps.safety.read_only, ReadOnlyMode::Server);
    assert!(!caps.safety.hardened);
    assert_eq!(caps.session.keepalive_secs, Some(30));
    assert_eq!(caps.namespaces.default_schema.as_deref(), Some("public"));
    assert_eq!(caps.data.max_params, 1000);
}

#[tokio::test]
async fn a_failed_login_and_a_lost_server_are_reported() {
    let script = testing::sql_script(|_, _| None);
    let script = Script {
        auth: testing::AuthPlan::Sha512 {
            salt: [1, 2, 3, 4],
            user_salt: [9; 16],
        },
        password: "right".into(),
        ..script
    };
    let (port, seen) = testing::serve(script).await;
    let p = profile(port, serde_json::json!({}));
    let err = open(&p, Some("wrong")).await.err().expect("refused");
    assert!(matches!(err, AppError::Auth(_)), "{err:?}");
    let s = open(&p, Some("right")).await.expect("logged in");
    assert_eq!(seen.passwords().len(), 2);
    s.close().await;

    // Nothing listens: a connect error, not a hang.
    let dead = profile(1, serde_json::json!({}));
    assert!(matches!(open(&dead, None).await, Err(AppError::Connect(_))));
}

#[tokio::test]
async fn an_oauth_token_goes_in_the_startup_message_and_the_answer_over_tls_only() {
    let oauth = |script: Script| Script {
        auth: testing::AuthPlan::OAuth,
        password: "tok3n".into(),
        ..script
    };
    let script = oauth(testing::sql_script(|_, _| None).with_tls(server_tls(false)));
    let (port, seen) = testing::serve(script).await;
    let p = profile(
        port,
        serde_json::json!({ "ssl": "require", "options": { "driverParams": { "auth": "oauth" } } }),
    );
    let s = open(&p, Some("tok3n\n")).await.expect("the token is taken");
    assert_eq!(seen.tls().len(), 1);
    let st = &seen.startups()[0];
    assert_eq!(st["oauth_access_token"], "tok3n");
    assert_eq!(st["auth_category"], "OAuth");
    assert_eq!(seen.passwords()[0], b"tok3n");
    s.close().await;

    // `prefer` against a server that says no to TLS: validate refuses the profile, and the
    // connection itself never sends the token in clear.
    let (port, seen) = testing::serve(oauth(testing::sql_script(|_, _| None))).await;
    let p = profile(
        port,
        serde_json::json!({ "ssl": "prefer", "options": { "driverParams": { "auth": "oauth" } } }),
    );
    assert!(super::entry::validate(&p).is_err());
    let err = open(&p, Some("tok3n")).await.err().expect("refused");
    assert!(
        matches!(err, AppError::Auth(ref m) if m.contains("TLS")),
        "{err:?}"
    );
    assert!(seen.tls().is_empty());
    assert!(!seen.startups()[0].contains_key("oauth_access_token"));
    assert!(
        seen.passwords()
            .iter()
            .all(|p| !p.windows(5).any(|w| w == b"tok3n"))
    );
}

// --- TLS -----------------------------------------------------------------------------------------

fn pem_file(text: &str) -> tempfile::NamedTempFile {
    use std::io::Write as _;
    let mut f = tempfile::NamedTempFile::new().expect("a temp file");
    f.write_all(text.as_bytes()).expect("written");
    f
}

/// The test server's TLS, asking for a client certificate the test CA signed.
fn server_tls(client_auth: bool) -> Arc<rustls::ServerConfig> {
    use crate::testkit::tls::{CA_PEM, SERVER_CERT_PEM, SERVER_KEY_PEM, cert_der};
    use rustls::pki_types::PrivateKeyDer;
    use rustls::pki_types::pem::PemObject as _;
    if !client_auth {
        return crate::testkit::tls::server_config();
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert_der(CA_PEM)).expect("the CA");
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()
    .expect("a client verifier");
    let key = PrivateKeyDer::from_pem_slice(SERVER_KEY_PEM.as_bytes()).expect("a key");
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![cert_der(SERVER_CERT_PEM), cert_der(CA_PEM)], key)
        .expect("a server config");
    Arc::new(config)
}

#[tokio::test]
async fn tls_checks_the_server_and_shows_the_client_certificate() {
    use crate::testkit::tls::{CA_PEM, CLIENT_CERT_PEM, CLIENT_KEY_PEM};
    let script = testing::sql_script(|_, _| None).with_tls(server_tls(true));
    let (port, seen) = testing::serve(script).await;
    let (ca, cert, key) = (
        pem_file(CA_PEM),
        pem_file(CLIENT_CERT_PEM),
        pem_file(CLIENT_KEY_PEM),
    );
    let path = |f: &tempfile::NamedTempFile| f.path().to_string_lossy().into_owned();
    let p = profile(
        port,
        serde_json::json!({
            "host": "db.example.test", "ssl": "verify-full", "sslCaPath": path(&ca),
            "options": { "sslCertPath": path(&cert), "sslKeyPath": path(&key) }
        }),
    );
    // Through a "tunnel": the socket goes to 127.0.0.1, the certificate is checked for the
    // profile's host.
    let s = VerticaSession::open(&ConnectTarget {
        profile: &p,
        password: None,
        host_override: Some(("127.0.0.1".into(), port)),
    })
    .await
    .expect("TLS with a client certificate");
    let tls = seen.tls();
    assert_eq!(tls.len(), 1);
    assert_eq!(tls[0], (Some("db.example.test".into()), true));
    let details = s.session_details().await.unwrap().unwrap();
    assert!(details.tls);
    assert!(details.tls_version.is_some_and(|v| v.starts_with("TLSv1")));
    assert!(details.tls_cipher.is_some());
    s.close().await;

    // A name the certificate does not hold fails verify-full.
    let wrong = profile(
        port,
        serde_json::json!({ "host": "elsewhere.test", "ssl": "verify-full", "sslCaPath": path(&ca) }),
    );
    let err = VerticaSession::open(&ConnectTarget {
        profile: &wrong,
        password: None,
        host_override: Some(("127.0.0.1".into(), port)),
    })
    .await
    .err()
    .expect("refused");
    assert!(matches!(err, AppError::Connect(_)), "{err:?}");
}

#[tokio::test]
async fn prefer_goes_on_without_tls_and_require_does_not() {
    let (port, seen) = testing::serve(testing::sql_script(|_, _| None)).await;
    let s = open(&profile(port, serde_json::json!({ "ssl": "prefer" })), None)
        .await
        .expect("prefer falls back to plain");
    assert!(!s.session_details().await.unwrap().unwrap().tls);
    assert_eq!(*seen.ssl_requests.lock().unwrap(), 1);
    let err = open(
        &profile(port, serde_json::json!({ "ssl": "require" })),
        None,
    )
    .await
    .err()
    .expect("require refuses plain");
    assert!(matches!(err, AppError::Connect(m) if m.contains("TLS")));
    // TLS on without a check: the server's certificate is taken as it is.
    let script = testing::sql_script(|_, _| None).with_tls(server_tls(false));
    let (port, _) = testing::serve(script).await;
    let s = open(
        &profile(port, serde_json::json!({ "ssl": "require" })),
        None,
    )
    .await
    .expect("require over TLS");
    assert!(s.session_details().await.unwrap().unwrap().tls);
}

// --- Catalog -------------------------------------------------------------------------------------

#[tokio::test]
async fn lists_the_database_schemas_tables_and_structure() {
    let (s, seen, _) = session_with(serde_json::json!({}), |sql, params| {
        if sql == catalog::DATABASES {
            return Some(rows(&[("d", VC), ("o", VC)], &[&["VMart", "dbadmin"]]));
        }
        if sql == catalog::SCHEMAS {
            return Some(rows(
                &[("s", VC), ("sys", BOOL)],
                &[&["public", "f"], &["v_catalog", "t"]],
            ));
        }
        if sql == catalog::VIEW_DEFINITION {
            return Some(rows(&[("v", VC)], &[&["SELECT * FROM orders"]]));
        }
        if sql.starts_with("SELECT EXPORT_OBJECTS") {
            return Some(rows(
                &[("ddl", VC)],
                &[&["CREATE TABLE public.orders (id int);"]],
            ));
        }
        if sql == catalog::COMPLETION {
            return Some(rows(
                &[("s", VC), ("t", VC), ("c", VC), ("o", INT)],
                &[&["public", "orders", "id", "1"]],
            ));
        }
        orders(sql, params)
    })
    .await;
    let dbs = s.list_databases().await.unwrap();
    assert_eq!(dbs.len(), 1);
    assert!(dbs[0].is_current);
    let schemas = s.list_schemas().await.unwrap();
    assert!(schemas[0].is_default && schemas[0].name == "public");
    let tables = s.list_tables("public").await.unwrap();
    assert_eq!(tables.len(), 3);
    assert_eq!(tables[0].row_estimate, Some(12));
    assert_eq!(tables[0].size_bytes, Some(4096));
    assert_eq!(tables[1].kind, TableKind::View);

    let meta = orders_meta(&s).await;
    assert_eq!(meta.kind, TableKind::Table);
    assert_eq!(meta.comment.as_deref(), Some("Every order"));
    assert_eq!(meta.primary_key, ["id"]);
    assert_eq!(meta.row_estimate, Some(12));
    assert_eq!(meta.columns.len(), 3);
    assert!(meta.columns[0].is_pk && meta.columns[0].is_auto_increment);
    assert_eq!(meta.columns[1].comment.as_deref(), Some("Who ordered"));
    assert_eq!(meta.columns[2].kind, ValueKind::Bytes);
    assert_eq!(meta.indexes.len(), 1, "the super projection");
    // Names travel as bind parameters, never in the text.
    let Some(Request::Execute { values, .. }) = seen
        .requests()
        .into_iter()
        .find(|r| matches!(r, Request::Execute { sql, .. } if sql == catalog::TABLE))
    else {
        panic!("no table lookup");
    };
    assert_eq!(values[1].as_deref(), Some(&b"orders"[..]));
    let missing = s
        .table_meta(&TableRef {
            schema: None,
            name: "nope".into(),
        })
        .await;
    assert!(matches!(missing, Err(AppError::NotFound(_))), "{missing:?}");

    let view = TableRef {
        schema: Some("public".into()),
        name: "recent".into(),
    };
    assert_eq!(
        s.view_definition(&view).await.unwrap().as_deref(),
        Some("SELECT * FROM orders")
    );
    let ddl = s.table_ddl(&meta.table).await.unwrap().unwrap();
    assert!(ddl.starts_with("CREATE TABLE"));
    assert!(sent(
        &seen,
        "SELECT EXPORT_OBJECTS('', '\"public\".\"orders\"', false)"
    ));
    let completion = s.completion_metadata().await.unwrap();
    assert_eq!(completion.default_schema.as_deref(), Some("public"));
    // The console log shows the catalog reads.
    assert!(!s.drain_trace().is_empty());
}

#[tokio::test]
async fn routines_and_their_text() {
    let (s, _, _) = session_with(serde_json::json!({}), |sql, params| {
        if sql == catalog::ROUTINES {
            return Some(rows(
                &[
                    ("n", VC),
                    ("t", VC),
                    ("a", VC),
                    ("r", VC),
                    ("l", VC),
                    ("c", VC),
                ],
                &[
                    &[
                        "add1",
                        "User Defined SQL Function",
                        "x int",
                        "int",
                        "NULL",
                        "NULL",
                    ],
                    &["cleanup", "Stored Procedure", "", "NULL", "PL/vSQL", "NULL"],
                ],
            ));
        }
        if sql == catalog::FUNCTION_DEFINITION {
            return Some(rows(&[("d", VC)], &[&["RETURN (x + 1)"]]));
        }
        if sql.starts_with("SELECT EXPORT_OBJECTS") && sql.contains("cleanup") {
            return Some(failure("42883", "not exportable", "3457"));
        }
        let _ = params;
        None
    })
    .await;
    let routines = s.list_routines("public").await.unwrap().unwrap();
    assert_eq!(routines.len(), 2);
    assert_eq!(routines[1].kind, RoutineKind::Procedure);
    let function = RoutineRef {
        schema: Some("public".into()),
        name: "add1".into(),
        kind: RoutineKind::Function,
        identity_arguments: String::new(),
    };
    assert_eq!(
        s.routine_definition(&function).await.unwrap().unwrap(),
        "RETURN (x + 1)"
    );
    let procedure = RoutineRef {
        name: "cleanup".into(),
        kind: RoutineKind::Procedure,
        ..function
    };
    let text = s.routine_definition(&procedure).await.unwrap().unwrap();
    assert!(
        text.starts_with("-- The definition is not available"),
        "{text}"
    );
}

// --- Grid ----------------------------------------------------------------------------------------

#[tokio::test]
async fn browses_with_paging_counts_and_fetches_a_cell() {
    let (s, seen, _) = session_with(serde_json::json!({}), orders).await;
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
    // 2^53+1 stays exact.
    assert_eq!(page.rows[0][0], Value::BigInt(BIG.into()));
    assert_eq!(page.rows[0][1], Value::Text("b".into()));
    assert_eq!(page.rows[0][2], Value::from_bytes(&[1, 2]));
    assert_eq!(page.rows[1], [Value::Int(4), Value::Null, Value::Null]);
    let browse = queries(&seen)
        .into_iter()
        .find(|q| q.contains("LIMIT") || q.contains("OFFSET"))
        .expect("the page's SELECT");
    assert!(browse.contains("\"public\".\"orders\""), "{browse}");

    let narrowing = Narrowing::for_caps(&s.capabilities());
    let count = sql::count_on(
        &narrowing,
        SqlDialect::Vertica,
        &meta.table,
        &meta.columns,
        None,
    )
    .unwrap();
    let counted = s
        .query_cancellable(&count.sql, &count.params, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(counted.rows, [[Value::Int(12)]]);

    let key = RowKey::Pk(map([("id", Value::BigInt(BIG.into()))]));
    let cell = fetch_cell_inner(&s, &meta, "data", &key).await.unwrap();
    assert_eq!(cell, Value::from_bytes(&[1, 2]));
    let Some(Request::Execute { values, .. }) = seen
        .requests()
        .into_iter()
        .rev()
        .find(|r| matches!(r, Request::Execute { .. }))
    else {
        panic!("the fetch was not bound");
    };
    assert_eq!(values, [Some(BIG.as_bytes().to_vec())]);
}

#[tokio::test]
async fn values_map_to_the_app_s_variants() {
    let (s, _, _) = session_with(serde_json::json!({}), |sql, _| {
        (sql == "SELECT v FROM t").then(|| {
            rows(
                &[
                    ("big", INT),
                    ("num", types::NUMERIC),
                    ("ts", types::TIMESTAMP),
                    ("tz", types::TIMESTAMPTZ),
                    ("ok", BOOL),
                    ("f", types::FLOAT8),
                ],
                &[&[
                    BIG,
                    "12345678901234567890.50",
                    "2024-01-31 12:00:00.5",
                    "2024-01-31 12:00:00+07",
                    "t",
                    "1.5",
                ]],
            )
        })
    })
    .await;
    let rs = s.query("SELECT v FROM t", &[]).await.unwrap();
    let row = &rs.rows[0];
    assert_eq!(row[0], Value::BigInt(BIG.into()));
    assert_eq!(row[1], Value::Decimal("12345678901234567890.50".into()));
    assert_eq!(row[2], Value::Timestamp("2024-01-31 12:00:00.5".into()));
    assert!(
        matches!(&row[3], Value::Timestamp(t) | Value::TimestampTz(t) if t == "2024-01-31 12:00:00+07"),
        "{:?}",
        row[3]
    );
    assert_eq!(row[4], Value::Bool(true));
    assert_eq!(row[5], Value::Float(1.5));
    assert_eq!(rs.columns[1].kind, ValueKind::Decimal);
}

fn update(meta: &TableMeta, id: i64, name: &str) -> TxStatement {
    let narrowing = Narrowing::for_caps(&crate::types::caps::SessionCaps::for_driver(
        crate::types::profile::Driver::Vertica,
    ));
    let target = TableTarget {
        table: &meta.table,
        columns: &meta.columns,
    };
    let change = StagedChange::Update {
        row_id: "r".into(),
        key: RowKey::Pk(map([("id", Value::Int(id))])),
        changes: map([("name", text(name))]),
        exprs: None,
    };
    let (built, expect) =
        sql::change_on(&narrowing, SqlDialect::Vertica, &target, &change, false).unwrap();
    let mut st = TxStatement::new(built.sql, built.params);
    st.expect_affected = expect;
    st
}

#[tokio::test]
async fn a_commit_with_one_failing_change_rolls_everything_back() {
    let (s, seen, _) = session_with(serde_json::json!({}), |sql, params| {
        if !sql.starts_with("UPDATE") {
            return orders(sql, params);
        }
        let id = params.and_then(|p| p.last().cloned().flatten());
        Some(match id.as_deref() {
            Some("2") => failure("23505", "Duplicate key values: 'name=b'", "6745"),
            Some("3") => Answer::Count("UPDATE".into(), 2),
            _ => Answer::Count("UPDATE".into(), 1),
        })
    })
    .await;
    let meta = orders_meta(&s).await;
    s.drain_trace();
    let ok = s
        .transaction(vec![update(&meta, 1, "a"), update(&meta, 4, "d")])
        .await
        .unwrap();
    assert_eq!(ok.iter().map(|o| o.affected).collect::<Vec<_>>(), [1, 1]);
    let q = queries(&seen);
    let tail: Vec<&str> = q[q.len() - 5..].iter().map(String::as_str).collect();
    assert_eq!(tail[0], vertica::AUTOCOMMIT_OFF);
    assert!(tail[1].starts_with("UPDATE") && tail[2].starts_with("UPDATE"));
    assert_eq!(&tail[3..], ["COMMIT", vertica::AUTOCOMMIT_ON]);

    let err = s
        .transaction(vec![update(&meta, 1, "a"), update(&meta, 2, "b")])
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AppError::Db {
                vendor_code: Some(6745),
                statement_index: Some(1),
                ..
            }
        ),
        "{err:?}"
    );
    let q = queries(&seen);
    assert_eq!(&q[q.len() - 2..], ["ROLLBACK", vertica::AUTOCOMMIT_ON]);
    // A change that matched two rows is rolled back too.
    let err = s
        .transaction(vec![update(&meta, 3, "c")])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("expected to affect 1"), "{err}");
    let q = queries(&seen);
    assert_eq!(&q[q.len() - 2..], ["ROLLBACK", vertica::AUTOCOMMIT_ON]);
    let trace = s.drain_trace();
    assert!(
        trace
            .iter()
            .any(|t| t.error.as_deref().is_some_and(|e| e.contains("Duplicate")))
    );
    // The worker is still good.
    s.ping().await.unwrap();
}

#[tokio::test]
async fn an_insert_and_a_delete_are_counted() {
    let (s, _, _) = session_with(serde_json::json!({}), |sql, _| {
        if sql.starts_with("INSERT") {
            return Some(Answer::Count("INSERT".into(), 1));
        }
        if sql.starts_with("DELETE") {
            return Some(Answer::Count("DELETE".into(), 3));
        }
        None
    })
    .await;
    let out = s
        .execute("INSERT INTO t (a) VALUES (?)", &[Value::Int(1)])
        .await
        .unwrap();
    assert_eq!(out.affected, 1);
    assert!(out.rows.is_none() && !out.affected_unknown);
    let out = s.execute("DELETE FROM t", &[]).await.unwrap();
    assert_eq!(out.affected, 3);
}

// --- Editor --------------------------------------------------------------------------------------

#[tokio::test]
async fn the_editor_streams_counts_and_tracks_the_transaction() {
    let (s, seen, _) = session_with(serde_json::json!({}), |sql, _| match sql {
        "SELECT n FROM five" => Some(rows(
            &[("n", INT)],
            &[&["1"], &["2"], &["3"], &["4"], &["5"]],
        )),
        "UPDATE t SET a = 1" => Some(Answer::Count("UPDATE".into(), 7)),
        "CREATE TABLE x (a int)" => Some(Answer::Done("CREATE TABLE".into())),
        _ => None,
    })
    .await;
    let run = |sql: &'static str, max: usize| {
        let s = &s;
        async move {
            let mut sink = CollectSink::default();
            let out = s
                .run_statement(sql, max, &mut sink, CancelSlot::default())
                .await;
            (out, sink)
        }
    };
    let (out, sink) = run("SELECT n FROM five", 100).await;
    let out = out.unwrap();
    assert_eq!(out.kind, StatementKind::Rows);
    assert_eq!(out.row_count, 5);
    assert_eq!(sink.sets[0].rows.len(), 5);
    assert_eq!(sink.sets[0].columns[0].name, "n");

    let (out, sink) = run("UPDATE t SET a = 1", 100).await;
    let out = out.unwrap();
    assert_eq!(out.kind, StatementKind::Affected);
    assert_eq!(out.affected, Some(7));
    assert!(
        sink.sets.is_empty(),
        "the OUTPUT row is a count, not a result"
    );

    let (out, _) = run("CREATE TABLE x (a int)", 100).await;
    assert_eq!(out.unwrap().kind, StatementKind::Empty);

    let (out, _) = run("BEGIN", 100).await;
    out.unwrap();
    assert_eq!(s.in_transaction(), Some(true));
    let (out, _) = run("COMMIT", 100).await;
    out.unwrap();
    assert_eq!(s.in_transaction(), Some(false));

    // At the row cap a read is stopped on the server.
    let (out, sink) = run("SELECT n FROM five", 2).await;
    let out = out.unwrap();
    assert!(out.truncated);
    assert_eq!(sink.sets[0].rows.len(), 2);
    assert_eq!(seen.cancels().len(), 1);

    // An error keeps its code and the editor goes on.
    let (out, _) = run("SELEC 1", 100).await;
    assert!(matches!(out, Err(AppError::Db { sqlstate: Some(s), .. }) if s == "42601"));
    let (out, _) = run("SELECT 1", 100).await;
    out.unwrap();
    // One statement at a time.
    let (out, _) = run("SELECT 1; SELECT 2", 100).await;
    assert!(matches!(out, Err(AppError::InvalidInput(_))), "{out:?}");
}

#[tokio::test]
async fn manual_commit_opens_the_editor_without_autocommit() {
    let (s, seen, _) = session_with(
        serde_json::json!({ "options": { "driverParams": { "manualCommit": true } } }),
        |_, _| None,
    )
    .await;
    assert_eq!(seen.startups()[0]["autocommit"], "off");
    s.ping().await.unwrap();
    assert_eq!(
        seen.startups()[1]["autocommit"],
        "on",
        "workers commit on their own"
    );
}

#[tokio::test]
async fn cancel_stops_the_editor_statement_on_the_server() {
    const SLOW: &str = "SELECT SLEEP(3600)";
    let (s, seen, _) = session_with(serde_json::json!({}), |sql, _| {
        (sql == SLOW).then_some(Answer::Hang)
    })
    .await;
    let s = Arc::new(s);
    let slot = CancelSlot::default();
    let running = {
        let s = Arc::clone(&s);
        let slot = slot.clone();
        tokio::spawn(async move {
            s.run_statement(SLOW, 100, &mut CollectSink::default(), slot)
                .await
        })
    };
    until(&seen, SLOW).await;
    let handle = request_cancel(&slot).expect("the statement armed the slot");
    s.cancel(handle).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("the cancel returns")
        .expect("the task ends");
    assert!(matches!(outcome, Err(AppError::Cancelled)), "{outcome:?}");
    // The request named the editor's connection (the first login).
    assert_eq!(seen.cancels(), [(1001, testing::key_of(1001))]);
    s.run_statement(
        "SELECT 1",
        10,
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_time_out_cancels_the_grid_read_on_the_server() {
    let (s, seen, _) = session_with(serde_json::json!({}), |sql, _| {
        (sql == "SELECT slowly FROM t").then_some(Answer::Hang)
    })
    .await;
    let outcome = query_limited(
        &s,
        "SELECT slowly FROM t",
        &[],
        Some(Duration::from_millis(200)),
    )
    .await;
    assert!(
        matches!(outcome, Err(AppError::QueryTimeout { .. })),
        "{outcome:?}"
    );
    assert_eq!(seen.cancels().len(), 1);
    // The worker was not broken by it: it serves the next read.
    let logins = seen.logins();
    s.query("SELECT 1", &[]).await.unwrap();
    assert_eq!(seen.logins(), logins);
}

#[tokio::test]
async fn a_lost_editor_ends_the_session() {
    let (s, _, _) = session_with(serde_json::json!({}), |sql, _| {
        (sql == "SELECT boom").then_some(Answer::Close)
    })
    .await;
    let out = s
        .run_statement(
            "SELECT boom",
            10,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await;
    assert!(matches!(out, Err(AppError::NotConnected)), "{out:?}");
    assert!(matches!(s.ping().await, Err(AppError::NotConnected)));
    assert_eq!(s.in_transaction(), None);
}

#[tokio::test]
async fn close_ends_every_connection_quickly() {
    let (s, seen, _) = session_with(serde_json::json!({}), |_, _| None).await;
    s.ping().await.unwrap();
    let started = std::time::Instant::now();
    s.close().await;
    assert!(started.elapsed() < Duration::from_secs(3));
    for _ in 0..200 {
        if seen.terminated() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        seen.terminated(),
        2,
        "the editor and the worker said goodbye"
    );
    assert!(matches!(s.ping().await, Err(AppError::NotConnected)));
}

// --- Read-only and safety ------------------------------------------------------------------------

#[tokio::test]
async fn read_only_is_the_server_s_and_every_write_is_refused_before_it_is_sent() {
    let (s, seen, _) = session_with(
        serde_json::json!({ "options": { "readOnly": true } }),
        orders,
    )
    .await;
    // Every connection is read-only on the server, set up before any startup command.
    s.ping().await.unwrap();
    let setups = queries(&seen)
        .into_iter()
        .filter(|q| q == vertica::READ_ONLY_SESSION)
        .count();
    assert_eq!(setups, 2);
    assert_eq!(s.capabilities().safety.read_only, ReadOnlyMode::Server);
    let before = seen.requests().len();
    let refused = |r: Result<(), AppError>, what: &str| match r {
        Err(AppError::InvalidInput(m)) => assert_eq!(m, READ_ONLY_MESSAGE, "{what}"),
        other => panic!("{what}: {other:?}"),
    };
    refused(s.execute("DELETE FROM t", &[]).await.map(|_| ()), "execute");
    refused(
        s.execute_cancellable("UPDATE t SET a = 1", &[], CancelSlot::default())
            .await
            .map(|_| ()),
        "execute_cancellable",
    );
    let meta = orders_meta(&s).await;
    let before_tx = seen.requests().len();
    refused(
        s.transaction(vec![update(&meta, 1, "a")]).await.map(|_| ()),
        "transaction",
    );
    assert_eq!(seen.requests().len(), before_tx);
    for sql in [
        "INSERT INTO t VALUES (1)",
        "SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE",
        "SELECT CLOSE_SESSION('x')",
        "COPY t FROM '/tmp/x'",
        "EXPORT TO PARQUET (directory = '/tmp/x') AS SELECT 1",
        "SELECT NEXTVAL('s')",
        "SELECT my_seq.NEXTVAL",
        "SELECT \"my_seq\".\"NEXTVAL\" FROM t",
        r#"SELECT s.U&"NEXTVAL""#,
        r#"SELECT s.u&"nextval""#,
        r#"SELECT s.U&"\004EEXTVAL""#,
        r#"SELECT s.U&"!004EEXTVAL" UESCAPE '!'"#,
        "PROFILE SELECT 1",
        "COMMIT",
        // The KELVIN SIGN folds to `k`, as Vertica compares names.
        "SELECT \u{212A}afkaListTopics('evil:9092')",
        "SELECT LDAP_LIN\u{212A}_SYNC_START()",
        "SELECT \u{212A}MEANS('m','t','*',2)",
    ] {
        refused(
            s.run_statement(sql, 10, &mut CollectSink::default(), CancelSlot::default())
                .await
                .map(|_| ()),
            sql,
        );
        refused(s.query(sql, &[]).await.map(|_| ()), sql);
        refused(
            s.stream_query(sql, &[], &mut CollectSink::default(), CancelSlot::default())
                .await
                .map(|_| ()),
            sql,
        );
    }
    let kills = s
        .kill_processes(&["v_vmart_node0001-9:0x2".into()], KillScope::Connection)
        .await
        .unwrap();
    assert!(!kills[0].ok);
    assert_eq!(kills[0].message.as_deref(), Some(READ_ONLY_MESSAGE));
    // Nothing of it reached the server (the table lookup aside).
    let writes: Vec<String> = queries(&seen)
        .into_iter()
        .filter(|q| {
            !q.starts_with("SELECT")
                || q.contains("CLOSE_SESSION")
                || q.to_ascii_uppercase().contains("NEXTVAL")
                || q.contains("U&")
                || q.contains('\u{212A}')
        })
        .filter(|q| q != vertica::READ_ONLY_SESSION)
        .collect();
    assert!(writes.is_empty(), "{writes:?}");
    assert!(seen.requests().len() > before, "the table lookup ran");
    // Reads still work.
    s.run_statement(
        "SELECT 1",
        10,
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_read_only_session_refuses_startup_commands_that_write() {
    let (port, seen) = testing::serve(testing::sql_script(|_, _| None)).await;
    let p = profile(
        port,
        serde_json::json!({
            "options": { "readOnly": true, "startupSql": "SET SEARCH_PATH TO app" }
        }),
    );
    let err = open(&p, None).await.err().expect("refused");
    assert!(matches!(err, AppError::InvalidInput(m) if m.contains("read-only")));
    assert!(seen.startups().is_empty(), "refused before connecting");
}

#[tokio::test]
async fn a_startup_command_s_credential_stays_out_of_its_errors() {
    const SQL: &str = "ALTER SESSION SET AWSAuth = 'AKIAX:SECRETX'";
    // Read-only: refused before connecting (the MCP process always opens read-only).
    let (port, _) = testing::serve(testing::sql_script(|_, _| None)).await;
    let p = profile(
        port,
        serde_json::json!({ "options": { "readOnly": true, "startupSql": SQL } }),
    );
    let err = open(&p, None).await.err().expect("refused").to_string();
    assert!(err.contains("read-only"), "{err}");
    assert!(!err.contains("SECRETX"), "{err}");
    // Read-write: the server refuses it and quotes the value back.
    let (port, _) = testing::serve(testing::sql_script(|sql, _| {
        sql.starts_with("ALTER SESSION SET AWSAuth")
            .then(|| failure("22023", "bad AWSAuth value 'AKIAX:SECRETX'", "1234"))
    }))
    .await;
    let p = profile(
        port,
        serde_json::json!({ "options": { "startupSql": SQL } }),
    );
    let err = open(&p, None).await.err().expect("refused").to_string();
    assert!(err.contains("bad AWSAuth value"), "{err}");
    assert!(!err.contains("SECRETX"), "{err}");
}

#[tokio::test]
async fn a_read_only_session_is_read_only_before_its_startup_commands_run() {
    let (port, seen) = testing::serve(testing::sql_script(|sql, _| {
        (sql == "SELECT 42").then(|| Answer::Done("SELECT".into()))
    }))
    .await;
    let p = profile(
        port,
        serde_json::json!({ "options": { "readOnly": true, "startupSql": "SELECT 42" } }),
    );
    let _s = open(&p, None).await.unwrap();
    let sent = queries(&seen);
    let at = |q: &str| sent.iter().position(|s| s == q);
    assert!(
        matches!((at(vertica::READ_ONLY_SESSION), at("SELECT 42")), (Some(r), Some(s)) if r < s),
        "{sent:?}"
    );
}

#[tokio::test]
async fn a_raw_filter_cannot_add_a_statement_or_write() {
    let (s, seen, _) = session_with(serde_json::json!({}), |_, _| None).await;
    for sql in [
        "SELECT * FROM t WHERE (1=1); DELETE FROM t WHERE (1=1)",
        "SELECT * FROM t WHERE 1=1 /* x */; SELECT 2",
    ] {
        let r = s.query(sql, &[]).await;
        assert!(matches!(r, Err(AppError::InvalidInput(_))), "{sql}: {r:?}");
    }
    for sql in [
        "SELECT * FROM t WHERE a = (SELECT DROP_PARTITIONS('t', 1, 2))",
        "SELECT * FROM t WHERE a = (SELECT KMEANS('m', 't', '*', 2))",
    ] {
        let r = s.query(sql, &[]).await;
        assert!(matches!(r, Err(AppError::InvalidInput(_))), "{sql}: {r:?}");
    }
    assert!(!sent(&seen, "DELETE") && !sent(&seen, "DROP_PARTITIONS") && !sent(&seen, "MEANS"));
}

// --- Processes -----------------------------------------------------------------------------------

#[tokio::test]
async fn lists_and_kills_sessions() {
    let (s, seen, _) = session_with(serde_json::json!({}), |sql, _| {
        if sql == catalog::SESSIONS {
            let cols: Vec<(&str, u32)> = [
                "session_id",
                "user_name",
                "client_hostname",
                "client_label",
                "login_timestamp",
                "statement_start",
                "current_statement",
                "statement_id",
                "transaction_id",
                "node_name",
                "client_type",
            ]
            .iter()
            .map(|c| {
                (
                    *c,
                    if c.ends_with("_id") && *c != "session_id" {
                        INT
                    } else {
                        VC
                    },
                )
            })
            .collect();
            return Some(rows(
                &cols,
                &[
                    &[
                        "v_vmart_node0001-31:0x1",
                        "dbadmin",
                        "10.0.0.2",
                        "Tabler",
                        "2024-01-01 10:00:00",
                        "NULL",
                        "NULL",
                        "NULL",
                        "NULL",
                        "v_vmart_node0001",
                        "Tabler",
                    ],
                    &[
                        "v_vmart_node0001-9:0x2",
                        "etl",
                        "10.0.0.3",
                        "loader",
                        "2024-01-01 09:00:00",
                        "2024-01-01 10:01:00",
                        "SELECT COUNT(*) FROM big",
                        "5",
                        "45035996273704961",
                        "v_vmart_node0001",
                        "JDBC Driver",
                    ],
                ],
            ));
        }
        if sql.starts_with("SELECT INTERRUPT_STATEMENT") || sql.starts_with("SELECT CLOSE_SESSION")
        {
            return Some(rows(&[("r", VC)], &[&["ok"]]));
        }
        None
    })
    .await;
    let list = s.list_processes().await.unwrap().unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[0].current && list[0].own);
    assert_eq!(list[1].id, "v_vmart_node0001-9:0x2");
    assert_eq!(list[1].query.as_deref(), Some("SELECT COUNT(*) FROM big"));
    assert_eq!(list[1].state.as_deref(), Some("active"));
    let ids = ["v_vmart_node0001-9:0x2".to_string()];
    let out = s.kill_processes(&ids, KillScope::Query).await.unwrap();
    assert!(out[0].ok, "{out:?}");
    assert!(sent(
        &seen,
        "SELECT INTERRUPT_STATEMENT('v_vmart_node0001-9:0x2', 5)"
    ));
    let out = s.kill_processes(&ids, KillScope::Connection).await.unwrap();
    assert!(out[0].ok);
    assert!(sent(
        &seen,
        "SELECT CLOSE_SESSION('v_vmart_node0001-9:0x2')"
    ));
    // The idle session runs no statement to interrupt.
    let idle = ["v_vmart_node0001-31:0x1".to_string()];
    let out = s.kill_processes(&idle, KillScope::Query).await.unwrap();
    assert!(!out[0].ok);
}

// --- Dropped futures, read-only extras, EXPLAIN and MCP -----------------------------------------

/// Answers the first `SELECT 1` (a ping) only when cancelled, and `SELECT '<x>'` with `<x>` in
/// upper case.
fn first_ping_hangs() -> impl Fn(&str, Params<'_>) -> Option<Answer> + Send + Sync + 'static {
    let pings = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    move |sql, _| {
        if sql == "SELECT 1" && pings.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            return Some(Answer::Hang);
        }
        let x = sql.strip_prefix("SELECT '")?.strip_suffix('\'')?;
        Some(rows(&[("x", VC)], &[&[&x.to_ascii_uppercase()]]))
    }
}

#[tokio::test]
async fn a_dropped_ping_does_not_hand_its_reply_to_the_next_statement() {
    let (s, seen, _) = session_with(serde_json::json!({}), first_ping_hangs()).await;
    // The app's holder check: the ping is given up after its time out.
    let dropped = tokio::time::timeout(Duration::from_millis(200), s.ping()).await;
    assert!(dropped.is_err(), "the ping was still waiting");
    let logins = seen.logins();
    // Each statement gets its own answer: the worker that still owed the ping's was closed.
    for x in ["a", "b"] {
        let rs = s.query(&format!("SELECT '{x}'"), &[]).await.unwrap();
        assert_eq!(rs.rows, [vec![text(&x.to_ascii_uppercase())]], "{x}");
    }
    assert_eq!(seen.logins(), logins + 1, "one new worker");
    s.ping().await.unwrap();
}

#[tokio::test]
async fn a_dropped_transaction_does_not_pool_its_worker() {
    let (s, seen, _) = session_with(serde_json::json!({}), |sql, params| {
        if sql.starts_with("UPDATE") {
            return Some(Answer::Hang);
        }
        orders(sql, params)
    })
    .await;
    let meta = orders_meta(&s).await;
    let logins = seen.logins();
    let dropped = tokio::time::timeout(
        Duration::from_millis(200),
        s.transaction(vec![update(&meta, 1, "a")]),
    )
    .await;
    assert!(dropped.is_err(), "the UPDATE was still waiting");
    s.ping().await.unwrap();
    assert_eq!(
        seen.logins(),
        logins + 1,
        "the worker left in autocommit off was closed"
    );
    let commits = queries(&seen)
        .iter()
        .filter(|q| *q == "COMMIT" || *q == vertica::AUTOCOMMIT_ON)
        .count();
    assert_eq!(commits, 0, "nothing committed what was left open");
}

#[tokio::test]
async fn a_dropped_editor_run_loses_the_editor() {
    const SLOW: &str = "SELECT SLEEP(3600)";
    let (s, _, _) = session_with(serde_json::json!({}), |sql, _| {
        (sql == SLOW).then_some(Answer::Hang)
    })
    .await;
    let mut sink = CollectSink::default();
    let dropped = tokio::time::timeout(
        Duration::from_millis(200),
        s.run_statement(SLOW, 10, &mut sink, CancelSlot::default()),
    )
    .await;
    assert!(dropped.is_err(), "the statement was still waiting");
    // Its replies are still due: the editor is lost, not reused out of step.
    assert!(matches!(s.ping().await, Err(AppError::NotConnected)));
    let out = s
        .run_statement(
            "SELECT 1",
            10,
            &mut CollectSink::default(),
            CancelSlot::default(),
        )
        .await;
    assert!(matches!(out, Err(AppError::NotConnected)), "{out:?}");
    assert_eq!(s.in_transaction(), None);
}

#[tokio::test]
async fn a_read_only_session_refuses_a_startup_nextval_and_commits_on_its_own() {
    let (port, seen) = testing::serve(testing::sql_script(|_, _| None)).await;
    for startup in ["SELECT NEXTVAL('s')", "SELECT s.NEXTVAL"] {
        let p = profile(
            port,
            serde_json::json!({ "options": { "readOnly": true, "startupSql": startup } }),
        );
        let err = open(&p, None).await.err().expect("refused");
        assert!(
            matches!(err, AppError::InvalidInput(ref m) if m.contains("read-only")),
            "{startup}: {err:?}"
        );
    }
    assert!(seen.startups().is_empty(), "refused before connecting");
    // Manual commit on a read-only profile: COMMIT is refused there, so the editor commits on
    // its own rather than keep a transaction its first SELECT opened.
    let p = profile(
        port,
        serde_json::json!({ "options": { "readOnly": true, "driverParams": { "manualCommit": true } } }),
    );
    let s = open(&p, None).await.unwrap();
    assert_eq!(seen.startups()[0]["autocommit"], "on");
    s.run_statement(
        "SELECT 1",
        10,
        &mut CollectSink::default(),
        CancelSlot::default(),
    )
    .await
    .unwrap();
    assert_eq!(s.in_transaction(), Some(false));
}

#[tokio::test]
async fn explain_returns_the_plan_rows() {
    use crate::commands::query::explain_inner;
    use crate::types::query::ExplainMode;
    let (s, seen, _) = session_with(
        serde_json::json!({ "options": { "readOnly": true } }),
        |sql, _| {
            sql.starts_with("EXPLAIN ").then(|| {
                rows(
                    &[("QUERY PLAN", VC)],
                    &[
                        &["Access Path:"],
                        &["+-STORAGE ACCESS for orders [Cost: 10, Rows: 3] (PATH ID: 1)"],
                    ],
                )
            })
        },
    )
    .await;
    let expected = sql::wrap_explain(
        SqlDialect::Vertica,
        "SELECT * FROM orders",
        ExplainMode::Plan,
        false,
    )
    .unwrap();
    assert!(expected.starts_with("EXPLAIN "), "{expected}");
    let plan = explain_inner(
        &s,
        "SELECT * FROM orders",
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert!(sent(&seen, &expected));
    assert_eq!(plan.rows.len(), 2);
    assert_eq!(plan.columns[0].name, "QUERY PLAN");
    assert_eq!(plan.rows[0], [text("Access Path:")]);
    // A plan never runs the statement, so a read-only session explains a write too.
    let rs = s.query("EXPLAIN DELETE FROM orders", &[]).await.unwrap();
    assert_eq!(rs.rows.len(), 2);
    // Not EXPLAIN ANALYZE: PROFILE runs the statement.
    let analyze = explain_inner(
        &s,
        "SELECT * FROM orders",
        ExplainMode::Analyze,
        &CancelSlot::default(),
    )
    .await;
    assert!(analyze.is_err());
}

#[test]
fn the_mcp_process_reads_only_like_a_read_only_profile() {
    // `registry::unattended()` is process-wide, so its effect is tested through the function
    // `open` hands it to; a read-only session's refusals are tested above.
    let p = profile(1, serde_json::json!({}));
    assert!(!super::session::read_only_of(&p, false));
    assert!(super::session::read_only_of(&p, true));
    let ro = profile(1, serde_json::json!({ "options": { "readOnly": true } }));
    assert!(super::session::read_only_of(&ro, false));
}
