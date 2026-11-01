//! The ClickHouse driver end to end: `entry::open` through the registry over the shared HTTP
//! client against the scripted server on a socket, and the session (on the in-process fake)
//! under the app's command bodies: the grid, commits, the editor, cancel, the time out, the
//! process list and read-only refusals.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::entry::{self, ClickhouseParams, ClickhouseSetting};
use super::request::{ConnSettings, Credentials, Endpoint};
use super::session::{ClickhouseSession, FEATURES, SessionOptions};
use super::testing::{FakeClickhouse, FakeReply, app_connection, http_transport, profile, serve};
use super::transport::header;
use crate::commands::browse::{DEFAULT_ESTIMATE_ABOVE, browse_rows, count_rows, fetch_cell_on};
use crate::commands::ddl::table_ddl_on;
use crate::commands::lock::session_details_of;
use crate::commands::mutate::apply_changes_on;
use crate::commands::query::query_limited;
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::common::http::Credential;
use crate::drivers::common::token::TokenSource;
use crate::drivers::native::CallKind;
use crate::drivers::registry::{self, UtilInput};
use crate::drivers::{
    CancelSlot, CollectSink, ConnectTarget, DbSession, Session, TxStatement, request_cancel,
};
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::types::admin::KillScope;
use crate::types::browse::{BrowseRequest, RowIdentity, RowKey};
use crate::types::caps::{CountMode, Feature, SafeConfirm, TxMode};
use crate::types::driver::ReadOnlyMode;
use crate::types::mutate::{ApplyRequest, ServerExpr, StagedChange};
use crate::types::query::StatementKind;
use crate::types::schema::{TableKind, TableRef};
use crate::types::value::Value;

const SERVER_INFO_NAMES: [&str; 5] = ["version()", "db", "user", "tz", "readonly"];
const STRINGS5: [&str; 5] = ["String"; 5];

fn server_info(version: &str) -> FakeReply {
    let row = format!(r#"["{version}","default","alice","UTC","0"]"#);
    FakeReply::rows(&SERVER_INFO_NAMES, &STRINGS5, &[row.as_str()])
}

/// A server with database `default` holding table `t` (`id` its key; a decimal, a time, a map),
/// view `v` and dictionary `d`.
fn seeded() -> Arc<FakeClickhouse> {
    let fake = FakeClickhouse::new();
    fake.on("SELECT version()", server_info("25.8.2.29"));
    fake.on(
        "FROM system.databases",
        FakeReply::rows(
            &["name", "engine", "comment", "is_current"],
            &["String", "String", "String", "UInt8"],
            &[
                r#"["default","Atomic","",1]"#,
                r#"["system","Atomic","",0]"#,
            ],
        ),
    );
    fake.on(
        "engine_full",
        FakeReply::rows(
            &[
                "engine",
                "engine_full",
                "partition_key",
                "sorting_key",
                "primary_key",
                "sampling_key",
                "create_table_query",
                "as_select",
                "comment",
                "total_rows",
            ],
            &[
                "String",
                "String",
                "String",
                "String",
                "String",
                "String",
                "String",
                "String",
                "String",
                "Nullable(UInt64)",
            ],
            &[
                r#"["MergeTree","MergeTree ORDER BY id","","id","id","","CREATE TABLE default.t (`id` UInt64, `name` String) ENGINE = MergeTree ORDER BY id","","","5"]"#,
            ],
        ),
    );
    fake.on(
        "is_temporary",
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
                r#"["d","Dictionary",null,null,"",0]"#,
                r#"["t","MergeTree","5","1024","",0]"#,
                r#"["v","View",null,null,"",0]"#,
            ],
        ),
    );
    let column_types = [
        "String",
        "String",
        "UInt64",
        "String",
        "String",
        "String",
        "UInt8",
        "UInt8",
        "UInt8",
        "UInt8",
        "String",
        "Nullable(UInt64)",
        "Nullable(UInt64)",
    ];
    let column_names = [
        "name",
        "type",
        "position",
        "default_kind",
        "default_expression",
        "comment",
        "is_in_primary_key",
        "is_in_sorting_key",
        "is_in_partition_key",
        "is_in_sampling_key",
        "compression_codec",
        "numeric_precision",
        "numeric_scale",
    ];
    fake.on(
        "FROM system.columns WHERE database = {db:String} AND table",
        FakeReply::rows(
            &column_names,
            &column_types,
            &[
                r#"["id","UInt64","1","","","",1,1,0,0,"","64","0"]"#,
                r#"["name","String","2","","","",0,0,0,0,"",null,null]"#,
                r#"["amount","Decimal(10, 2)","3","","","",0,0,0,0,"","10","2"]"#,
                r#"["at","DateTime('UTC')","4","","","",0,0,0,0,"",null,null]"#,
                r#"["tags","Map(String, UInt8)","5","","","",0,0,0,0,"",null,null]"#,
            ],
        ),
    );
    fake.on(
        "data_skipping_indices",
        FakeReply::rows(
            &["name", "type", "expr", "granularity"],
            &STRINGS5[..4],
            &[],
        ),
    );
    fake.on(
        "system.projections",
        FakeReply::rows(&["name", "query"], &STRINGS5[..2], &[]),
    );
    fake.on(
        "WHERE database NOT IN",
        FakeReply::rows(
            &["database", "table", "name"],
            &STRINGS5[..3],
            &[r#"["default","t","id"]"#, r#"["default","t","name"]"#],
        ),
    );
    Arc::new(fake)
}

/// The page reply of `t`: rows `ids`, with values that cover the decoders.
fn page(ids: &[u64]) -> FakeReply {
    let rows: Vec<String> = ids
        .iter()
        .map(|id| {
            let id = if *id == 1 { 9007199254740993 } else { *id };
            format!(r#"["{id}","n{id}","1.50","2026-10-30 12:00:00",{{"a":1}}]"#)
        })
        .collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    FakeReply::rows(
        &["id", "name", "amount", "at", "tags"],
        &[
            "UInt64",
            "String",
            "Decimal(10, 2)",
            "DateTime('UTC')",
            "Map(String, UInt8)",
        ],
        &rows,
    )
}

fn count_reply(n: u64) -> FakeReply {
    let row = format!(r#"["{n}"]"#);
    FakeReply::rows(&["count()"], &["UInt64"], &[row.as_str()])
}

fn conn_settings(read_only: bool) -> ConnSettings {
    ConnSettings {
        database: Some("default".into()),
        read_only,
        output_settings: true,
        ..ConnSettings::default()
    }
}

async fn session_on(fake: &Arc<FakeClickhouse>, read_only: bool) -> ClickhouseSession {
    ClickhouseSession::start(
        fake.clone(),
        Endpoint {
            tls: false,
            host: "ch.example".into(),
            port: 8123,
        },
        Credentials::Password {
            user: "alice".into(),
            password: "secret".into(),
        },
        conn_settings(read_only),
        SessionOptions {
            tls: false,
            confirm: SafeConfirm::Password,
            session_timeout_secs: Some(600),
        },
    )
    .await
    .expect("a session")
}

fn t() -> TableRef {
    TableRef::new(Some("default"), "t")
}

fn pk(id: i64) -> RowKey {
    RowKey::Pk(BTreeMap::from([("id".into(), Value::Int(id))]))
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn ch_profile(port: u16, fields: serde_json::Value) -> crate::types::profile::ConnectionProfile {
    let mut base =
        json!({ "host": "127.0.0.1", "port": port, "user": "alice", "database": "default" });
    if let (Some(b), Some(extra)) = (base.as_object_mut(), fields.as_object()) {
        for (k, v) in extra {
            b.insert(k.clone(), v.clone());
        }
    }
    profile(base)
}

#[tokio::test]
async fn open_connects_over_the_shared_client_with_the_profiles_settings() {
    let mut fake = FakeClickhouse::new();
    fake.credentials = Some(("alice".into(), "secret".into()));
    fake.on("SELECT version()", server_info("25.8.2.29"));
    let fake = Arc::new(fake);
    let server = serve(fake.clone()).await;
    let p = ch_profile(
        server.addr().port(),
        json!({ "options": { "driverParams": {
            "roles": ["analyst"],
            "settings": [{ "name": "max_threads", "value": "2" }],
            "sessionTimeoutSecs": 600
        } } }),
    );
    let target = ConnectTarget {
        profile: &p,
        password: Some("secret"),
        host_override: None,
    };
    let Session::Sql(s) = registry::open(target).await.expect("a session") else {
        panic!("a SQL session");
    };
    let info = s.server_info().await.unwrap();
    assert_eq!(info.version, "ClickHouse 25.8.2.29");
    assert_eq!(info.current_user.as_deref(), Some("alice"));
    s.ping().await.unwrap();
    let caps = s.capabilities();
    assert_eq!(caps.safety.read_only, ReadOnlyMode::PerRequest);
    assert!(!caps.safety.hardened);
    let sent = fake.requests();
    let first = &sent[0];
    assert_eq!(first.header(header::USER), Some("alice"));
    assert_eq!(first.header(header::KEY), Some("secret"));
    assert_eq!(first.param("database").as_deref(), Some("default"));
    assert_eq!(first.param("role").as_deref(), Some("analyst"));
    assert_eq!(first.param("max_threads").as_deref(), Some("2"));
    assert_eq!(first.param("readonly"), None);
    assert!(sent.iter().any(|r| r.path() == "/ping"));

    // A wrong password is an authentication error.
    let target = ConnectTarget {
        profile: &p,
        password: Some("wrong"),
        host_override: None,
    };
    assert!(matches!(
        registry::open(target).await,
        Err(AppError::Auth(_))
    ));

    // A read-only profile sends `readonly=2` first on every request, and says so.
    let ro = ch_profile(
        server.addr().port(),
        json!({ "options": { "readOnly": true } }),
    );
    let target = ConnectTarget {
        profile: &ro,
        password: Some("secret"),
        host_override: None,
    };
    let Session::Sql(s) = registry::open(target).await.unwrap() else {
        panic!("a SQL session");
    };
    assert!(s.capabilities().safety.hardened);
    let last = fake.requests().pop().unwrap();
    assert!(last.url.contains("?readonly=2&"), "{}", last.url);
}

#[tokio::test]
async fn a_command_password_goes_through_the_transport() {
    let mut fake = FakeClickhouse::new();
    fake.credentials = Some(("bob".into(), "from-command".into()));
    fake.on("SELECT 1", FakeReply::rows(&["1"], &["UInt8"], &["[1]"]));
    let fake = Arc::new(fake);
    let server = serve(fake.clone()).await;
    let command = CommandPassword::new("print-password", Duration::from_secs(600))
        .seeded(Some("from-command"));
    let (creds, credential) = entry::credentials(
        &ClickhouseParams::default(),
        "bob",
        TokenSource::Command(Arc::new(command)),
    );
    assert_eq!(creds, Credentials::Key { user: "bob".into() });
    assert!(matches!(credential, Credential::Header(h, _) if *h == *header::KEY));
    let client = super::client::Client::new(
        Arc::new(http_transport(&server, credential)),
        Endpoint {
            tls: false,
            host: "127.0.0.1".into(),
            port: server.addr().port(),
        },
        creds,
        conn_settings(false),
    );
    let rows = client
        .rows(&super::request::Query {
            sql: "SELECT 1",
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(rows, [vec![Value::Int(1)]]);
    let sent = &fake.requests()[0];
    assert_eq!(sent.header(header::KEY), Some("from-command"));

    // A stored password travels in the request's own header; a certificate user sends none.
    let (creds, credential) = entry::credentials(
        &ClickhouseParams::default(),
        "u",
        TokenSource::from_secret(Some("pw")),
    );
    assert!(matches!(credential, Credential::None));
    assert_eq!(
        creds,
        Credentials::Password {
            user: "u".into(),
            password: "pw".into()
        }
    );
    let cert = ClickhouseParams {
        certificate_auth: true,
        ..ClickhouseParams::default()
    };
    let (creds, credential) = entry::credentials(&cert, "u", TokenSource::from_secret(Some("pw")));
    assert_eq!(creds, Credentials::Certificate { user: "u".into() });
    assert!(matches!(credential, Credential::None));
}

/// The Client over the shared HTTP client to `server` with `token` as the profile's secret.
fn client_with(server: &crate::testkit::FakeServer, token: TokenSource) -> super::client::Client {
    let params = ClickhouseParams::default();
    let refresh = entry::refresh_of(&params, &token);
    let (creds, credential) = entry::credentials(&params, "bob", token);
    super::client::Client::new(
        Arc::new(http_transport(server, credential).refreshing(refresh)),
        Endpoint {
            tls: false,
            host: "127.0.0.1".into(),
            port: server.addr().port(),
        },
        creds,
        conn_settings(false),
    )
}

#[tokio::test]
async fn a_refused_command_password_runs_the_command_again_once() {
    let mut fake = FakeClickhouse::new();
    fake.credentials = Some(("bob".into(), "rotated".into()));
    fake.on("SELECT 1", FakeReply::rows(&["1"], &["UInt8"], &["[1]"]));
    let fake = Arc::new(fake);
    let server = serve(fake.clone()).await;
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let command = {
        let runs = runs.clone();
        CommandPassword::with_fetch(Duration::from_secs(600), move || {
            runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok("rotated".to_string()) })
        })
        .seeded(Some("stale"))
    };
    let client = client_with(&server, TokenSource::Command(Arc::new(command)));
    let q = super::request::Query {
        sql: "SELECT 1",
        ..Default::default()
    };
    // The server refuses the stale password with a 403 (516): the command runs, the statement
    // goes again with its output.
    assert_eq!(client.rows(&q).await.unwrap(), [vec![Value::Int(1)]]);
    let keys: Vec<_> = fake
        .requests()
        .iter()
        .map(|r| r.header(header::KEY).map(str::to_string))
        .collect();
    assert_eq!(
        keys,
        [Some("stale".to_string()), Some("rotated".to_string())]
    );
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);

    // A command that keeps printing a refused password runs once per request, not in a loop.
    let mut wrong = FakeClickhouse::new();
    wrong.credentials = Some(("bob".into(), "other".into()));
    let wrong = Arc::new(wrong);
    let server = serve(wrong.clone()).await;
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let command = {
        let runs = runs.clone();
        CommandPassword::with_fetch(Duration::from_secs(600), move || {
            runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok("rotated".to_string()) })
        })
        .seeded(Some("stale"))
    };
    let client = client_with(&server, TokenSource::Command(Arc::new(command)));
    assert!(matches!(client.rows(&q).await, Err(AppError::Auth(_))));
    assert_eq!(wrong.requests().len(), 2);
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);

    // A stored password has nothing to run again: one request, the refusal.
    let client = client_with(&server, TokenSource::from_secret(Some("pw")));
    assert!(matches!(client.rows(&q).await, Err(AppError::Auth(_))));
    assert_eq!(wrong.requests().len(), 3);

    // An unknown user (192) is no stale password: the command does not run again.
    let nobody = FakeClickhouse::new();
    nobody.on(
        "SELECT 1",
        FakeReply::Error {
            code: 192,
            message: "Unknown user bob".into(),
        },
    );
    let nobody = Arc::new(nobody);
    let server = serve(nobody.clone()).await;
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let command = {
        let runs = runs.clone();
        CommandPassword::with_fetch(Duration::from_secs(600), move || {
            runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok("rotated".to_string()) })
        })
        .seeded(Some("stale"))
    };
    let client = client_with(&server, TokenSource::Command(Arc::new(command)));
    assert!(matches!(client.rows(&q).await, Err(AppError::Auth(_))));
    assert_eq!(nobody.requests().len(), 1);
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_refusal_after_the_query_started_does_not_send_the_statement_again() {
    let sql = "INSERT INTO t SELECT * FROM remote('a,b', db.x)";
    let remote = "Received from shard2:9000. DB::Exception: default: Authentication failed: \
                  password is incorrect, or there is no user with such name";
    // A 516 raised while the insert runs (a remote's password, with the query id set), a
    // started query's own 516 (query id only) and a remote's message without the query id.
    for reply in [
        FakeReply::error(516, remote),
        FakeReply::error(516, "default: Authentication failed"),
        FakeReply::Refused {
            code: 516,
            message: remote.into(),
        },
    ] {
        let fake = FakeClickhouse::new();
        fake.on("INSERT INTO t", reply);
        let fake = Arc::new(fake);
        let server = serve(fake.clone()).await;
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let command = {
            let runs = runs.clone();
            CommandPassword::with_fetch(Duration::from_secs(600), move || {
                runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async { Ok("rotated".to_string()) })
            })
            .seeded(Some("stale"))
        };
        let client = client_with(&server, TokenSource::Command(Arc::new(command)));
        assert!(matches!(
            client.execute(sql, &[], &[]).await,
            Err(AppError::Auth(_))
        ));
        let inserts = fake
            .requests()
            .iter()
            .filter(|r| r.sql().contains("INSERT INTO t"))
            .count();
        assert_eq!(inserts, 1);
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

#[test]
fn validate_checks_the_params_and_the_certificate_sign_in() {
    let ok = profile(json!({ "host": "ch.example" }));
    assert!(entry::validate(&ok).is_ok());
    assert!(registry::validate(&ok).is_ok());
    let bad = |fields: serde_json::Value| {
        let p = profile(fields);
        assert!(
            entry::validate(&p).is_err(),
            "{:?}",
            p.options.driver_params
        );
    };
    bad(json!({ "host": " " }));
    bad(
        json!({ "host": "h", "options": { "driverParams": { "settings": [
        { "name": "readonly", "value": "0" }
    ] } } }),
    );
    bad(
        json!({ "host": "h", "options": { "driverParams": { "settings": [
        { "name": "max threads", "value": "1" }
    ] } } }),
    );
    bad(json!({ "host": "h", "options": { "driverParams": { "sessionTimeoutSecs": 0 } } }));
    bad(json!({ "host": "h", "options": { "driverParams": { "sessionTimeoutSecs": 7200 } } }));
    bad(json!({ "host": "h", "options": { "driverParams": { "roles": ["a\nb"] } } }));
    bad(json!({ "host": "h", "options": { "driverParams": { "certificateAuth": "yes" } } }));
    bad(json!({ "host": "h", "options": { "sslCertPath": "c.pem" } }));
    // A certificate sign-in needs HTTPS and both files.
    bad(json!({ "host": "h", "options": { "driverParams": { "certificateAuth": true } } }));
    bad(json!({ "host": "h", "ssl": "verify-full",
                "options": { "driverParams": { "certificateAuth": true } } }));
    let cert = profile(json!({ "host": "h", "ssl": "verify-full", "options": {
        "sslCertPath": "c.pem", "sslKeyPath": "k.pem",
        "driverParams": { "certificateAuth": true }
    } }));
    assert!(entry::validate(&cert).is_ok());
    let params = ClickhouseParams::from_driver_params(cert.options.driver_params.as_ref()).unwrap();
    assert!(params.certificate_auth);

    // HTTPS by the SSL mode, on its port; `prefer` checks the certificate fully.
    let https = profile(json!({ "host": " h ", "ssl": "prefer" }));
    let e = entry::endpoint_of(&https);
    assert_eq!((e.tls, e.host.as_str(), e.port), (true, "h", 8443));
    assert_eq!(e.base_url(), "https://h:8443/");
    assert_eq!(entry::endpoint_of(&ok).port, 8123);
    use crate::types::profile::SslMode;
    assert_eq!(
        entry::certificate_check(SslMode::Prefer),
        SslMode::VerifyFull
    );
    assert_eq!(entry::certificate_check(SslMode::Require), SslMode::Require);

    // The MCP process (unattended) is read-only like a read-only profile.
    let params = ClickhouseParams {
        settings: vec![ClickhouseSetting {
            name: " max_threads ".into(),
            value: "4".into(),
        }],
        roles: vec![" r ".into(), " ".into()],
        ..ClickhouseParams::default()
    };
    let c = entry::conn_settings(&ok, &params, false);
    assert!(!c.read_only);
    assert_eq!(c.settings, [("max_threads".to_string(), "4".to_string())]);
    assert_eq!(c.roles, ["r"]);
    assert!(entry::conn_settings(&ok, &params, true).read_only);
}

#[tokio::test]
async fn util_knows_no_call_and_counts_it_a_write() {
    let p = profile(json!({ "host": "h" }));
    let input = UtilInput {
        profile: &p,
        secret: None,
    };
    assert!(matches!(
        entry::util(input, "listDatabases", serde_json::Value::Null).await,
        Err(AppError::Unsupported(_))
    ));
    assert_eq!(entry::util_kind("anything"), CallKind::Write);
}

#[tokio::test]
async fn a_server_older_than_23_3_is_refused() {
    // A version that does not parse proves nothing either.
    for version in ["22.8.5.29", "", "x.y", "23"] {
        a_server_is_refused(version).await;
    }
}

async fn a_server_is_refused(version: &str) {
    let fake = FakeClickhouse::new();
    fake.on("SELECT version()", server_info(version));
    let fake = Arc::new(fake);
    let e = ClickhouseSession::start(
        fake,
        Endpoint {
            tls: false,
            host: "h".into(),
            port: 8123,
        },
        Credentials::Key { user: "u".into() },
        conn_settings(false),
        SessionOptions {
            tls: false,
            confirm: SafeConfirm::Passcode,
            session_timeout_secs: None,
        },
    )
    .await
    .err()
    .expect("refused");
    assert!(
        matches!(e, AppError::Unsupported(ref m) if m.contains("23.3")),
        "{version}: {e:?}"
    );
}

#[tokio::test]
async fn caps_grant_what_the_session_does() {
    let fake = seeded();
    let s = session_on(&fake, false).await;
    let caps = s.capabilities();
    for f in FEATURES {
        assert!(caps.features.contains(&f), "{f:?}");
    }
    for f in [Feature::Editor, Feature::StructureView] {
        assert!(caps.features.contains(&f), "{f:?}");
    }
    for f in [
        Feature::EditWithoutKey,
        Feature::Returning,
        Feature::ExplainAnalyze,
        Feature::StructureEdit,
        Feature::WorkspaceCommit,
        Feature::SwitchDatabase,
        Feature::ExportTables,
        Feature::CopyAsInsert,
        Feature::StartupCommands,
    ] {
        assert!(!caps.features.contains(&f), "{f:?}");
    }
    assert_eq!(caps.data.transactions, TxMode::Sequential);
    assert_eq!(caps.data.count, CountMode::Estimate);
    assert!(!caps.data.default_keyword);
    assert_eq!(caps.namespaces.default_schema.as_deref(), Some("default"));
    assert_eq!(caps.namespaces.labels.schema, "Database");
    assert_eq!(caps.session.keepalive_secs, Some(30));
    assert_eq!(caps.safety.read_only, ReadOnlyMode::PerRequest);
    assert!(!caps.safety.hardened);
    assert_eq!(s.in_transaction(), Some(false));
    let ro = session_on(&fake, true).await;
    assert!(ro.capabilities().safety.hardened);
}

#[tokio::test]
async fn the_sidebar_lists_databases_tables_and_structure() {
    let fake = seeded();
    let s = session_on(&fake, false).await;
    let dbs = s.list_databases().await.unwrap();
    assert_eq!(dbs.len(), 1);
    assert_eq!(dbs[0].name, "ch.example");
    let schemas = s.list_schemas().await.unwrap();
    let names: Vec<&str> = schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["default", "system"]);
    assert!(schemas[0].is_default);
    let tables = s.list_tables("default").await.unwrap();
    let kinds: Vec<(&str, TableKind)> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(
        kinds,
        [
            ("d", TableKind::ForeignTable),
            ("t", TableKind::Table),
            ("v", TableKind::View)
        ]
    );
    assert_eq!(tables[1].row_estimate, Some(5));
    let tables_req = fake
        .requests()
        .into_iter()
        .find(|r| r.sql().contains("is_temporary"))
        .unwrap();
    assert_eq!(tables_req.param("param_db").as_deref(), Some("default"));
    // Catalog queries go outside the editor's session.
    assert_eq!(tables_req.param("session_id"), None);

    let meta = s.table_meta(&t()).await.unwrap();
    assert_eq!(meta.primary_key, ["id"]);
    assert_eq!(meta.row_estimate, Some(5));
    assert_eq!(
        meta.options.as_ref().and_then(|o| o.sort_key.as_deref()),
        Some("id")
    );
    let ddl = s.table_ddl(&t()).await.unwrap().unwrap();
    assert!(ddl.starts_with("CREATE TABLE default.t"), "{ddl}");
    let completion = s.completion_metadata().await.unwrap();
    assert_eq!(completion.schemas["default"]["t"], ["id", "name"]);
    assert_eq!(completion.default_schema.as_deref(), Some("default"));
    // Every catalog query shows in the console log, as structure.
    let traced = s.drain_trace();
    assert!(traced.iter().any(|t| t.sql.contains("system.columns")));
}

#[tokio::test]
async fn the_grid_reads_counts_fetches_and_commits_through_the_commands() {
    let fake = seeded();
    let s: Arc<dyn DbSession> = Arc::new(session_on(&fake, false).await);
    let p = profile(json!({ "host": "ch.example" }));
    let (state, conn) = app_connection(p, Session::Sql(s.clone())).await;
    let req = |offset: u64| BrowseRequest {
        table: t(),
        columns: None,
        filter: None,
        sort: Vec::new(),
        limit: 2,
        offset,
        cursor: None,
    };
    fake.once("FROM `default`.`t`", page(&[1, 2]));
    let first = browse_rows(&conn, &req(0), None).await.unwrap();
    assert_eq!(first.identity, RowIdentity::Pk);
    assert_eq!(first.rows.len(), 2);
    let row = &first.rows[0];
    // 2^53 + 1 stays exact, a decimal is its text, a time its text, a map JSON.
    assert_eq!(row[0], Value::BigInt("9007199254740993".into()));
    assert_eq!(row[1], text("n9007199254740993"));
    assert_eq!(row[2], Value::Decimal("1.50".into()));
    assert!(
        matches!(&row[3], Value::Timestamp(t) if t == "2026-10-30 12:00:00"),
        "{:?}",
        row[3]
    );
    assert!(
        matches!(&row[4], Value::Json(j) if j.contains("\"a\"")),
        "{:?}",
        row[4]
    );
    fake.once("FROM `default`.`t`", page(&[5]));
    let last = browse_rows(&conn, &req(4), None).await.unwrap();
    assert_eq!(last.rows.len(), 1);
    let pages: Vec<String> = fake
        .statements()
        .into_iter()
        .filter(|s| s.contains("FROM `default`.`t`"))
        .collect();
    assert_eq!(pages.len(), 2);
    assert!(pages[0].contains("LIMIT"), "{}", pages[0]);
    assert!(pages[1].contains("OFFSET"), "{}", pages[1]);

    fake.once("SELECT count()", count_reply(5));
    let counted = count_rows(&conn, &t(), None, true, DEFAULT_ESTIMATE_ABOVE, None)
        .await
        .unwrap();
    assert_eq!((counted.count, counted.exact), (5, true));

    fake.once(
        "`name`",
        FakeReply::rows(&["name"], &["String"], &[r#"["whole"]"#]),
    );
    let cell = fetch_cell_on(&conn, &t(), &pk(2), "name", None)
        .await
        .unwrap();
    assert_eq!(cell, text("whole"));

    // Two updates: the first key matches one row and is applied, the second matches two and
    // is refused, so it stays staged with its error.
    fake.once("SELECT count()", count_reply(1));
    fake.once("SELECT count()", count_reply(2));
    fake.on("ALTER TABLE", FakeReply::Done { written_rows: 0 });
    let result = apply_changes_on(
        &state,
        conn.id,
        ApplyRequest {
            table: t(),
            changes: vec![
                StagedChange::Update {
                    row_id: "r1".into(),
                    key: pk(2),
                    changes: BTreeMap::from([("name".into(), text("x"))]),
                    exprs: None,
                },
                StagedChange::Update {
                    row_id: "r2".into(),
                    key: pk(3),
                    changes: BTreeMap::from([("name".into(), text("y"))]),
                    exprs: None,
                },
            ],
            returning: false,
        },
    )
    .await
    .unwrap();
    assert!(!result.success);
    assert!(result.results[0].error.is_none(), "{:?}", result.results);
    let failed = result.results[1].error.as_ref().expect("the second fails");
    assert!(failed.message.contains("matches 2 row(s)"), "{failed:?}");
    let alters: Vec<_> = fake
        .requests()
        .into_iter()
        .filter(|r| r.sql().starts_with("ALTER TABLE"))
        .collect();
    assert_eq!(alters.len(), 1, "the refused update was never sent");
    assert_eq!(alters[0].param("mutations_sync").as_deref(), Some("2"));
    assert_eq!(alters[0].param("param_p1").as_deref(), Some("x"));

    // An insert with a clock, counted by the summary, sent without buffering; a delete
    // guarded by its count.
    fake.on("INSERT INTO", FakeReply::Done { written_rows: 1 });
    fake.once("SELECT count()", count_reply(1));
    fake.on("DELETE FROM", FakeReply::Done { written_rows: 0 });
    let result = apply_changes_on(
        &state,
        conn.id,
        ApplyRequest {
            table: t(),
            changes: vec![
                StagedChange::Insert {
                    row_id: "r3".into(),
                    values: BTreeMap::from([
                        ("id".into(), Value::Int(9)),
                        ("name".into(), text("z")),
                    ]),
                    exprs: Some(BTreeMap::from([("at".into(), ServerExpr::Now)])),
                },
                StagedChange::Delete {
                    row_id: "r4".into(),
                    key: pk(4),
                },
            ],
            returning: false,
        },
    )
    .await
    .unwrap();
    assert!(result.success, "{:?}", result.error);
    let insert = fake
        .requests()
        .into_iter()
        .find(|r| r.sql().starts_with("INSERT INTO"))
        .unwrap();
    assert!(insert.sql().contains("now()"), "{}", insert.sql());
    assert_eq!(insert.param("async_insert").as_deref(), Some("0"));

    // Show DDL and the session details go through the hooks.
    let ddl = table_ddl_on(&conn, &t()).await.unwrap();
    assert!(ddl.contains("ENGINE = MergeTree"), "{ddl}");
    let details = session_details_of(s.as_ref()).await.unwrap();
    assert!(!details.tls);
    assert_eq!(details.encoding.as_deref(), Some("UTF-8"));
}

#[tokio::test]
async fn the_editor_runs_in_its_session_and_stops_at_the_row_cap() {
    let fake = seeded();
    let s = session_on(&fake, false).await;
    fake.on(
        "FROM numbers(5)",
        FakeReply::rows(
            &["number"],
            &["UInt64"],
            &[r#"["0"]"#, r#"["1"]"#, r#"["2"]"#, r#"["3"]"#, r#"["4"]"#],
        ),
    );
    let mut sink = CollectSink::default();
    let out = s
        .run_statement(
            "SELECT number FROM numbers(5)",
            2,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(out.kind, StatementKind::Rows);
    assert_eq!((out.row_count, out.truncated), (2, true));
    assert_eq!(
        sink.sets[0].rows,
        [vec![Value::Int(0)], vec![Value::Int(1)]]
    );

    fake.on("SET max_threads", FakeReply::Done { written_rows: 0 });
    fake.on("INSERT INTO t", FakeReply::Done { written_rows: 3 });
    let mut sink = CollectSink::default();
    let set = s
        .run_statement("SET max_threads = 1", 100, &mut sink, CancelSlot::default())
        .await
        .unwrap();
    assert_eq!(set.kind, StatementKind::Empty);
    let insert = s
        .run_statement(
            "INSERT INTO t SELECT * FROM t2",
            100,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        (insert.kind, insert.affected),
        (StatementKind::Affected, Some(3))
    );
    // Every editor statement goes in the same HTTP session; the app's own work outside it.
    let sessions: Vec<Option<String>> = fake
        .requests()
        .iter()
        .filter(|r| {
            let sql = r.sql();
            sql.starts_with("SELECT number") || sql.starts_with("SET") || sql.starts_with("INSERT")
        })
        .map(|r| r.param("session_id"))
        .collect();
    assert_eq!(sessions.len(), 3);
    assert!(sessions[0].is_some());
    assert!(sessions.iter().all(|id| id == &sessions[0]));
    let timeout = fake
        .requests()
        .iter()
        .find(|r| r.sql().starts_with("SET"))
        .and_then(|r| r.param("session_timeout"));
    assert_eq!(timeout.as_deref(), Some("600"));
    assert!(s.editor_run_lock().is_some());

    // An error reaches the editor with the server's code.
    fake.on("SELECT bad", FakeReply::error(47, "Unknown identifier bad"));
    let e = s
        .run_statement("SELECT bad", 10, &mut sink, CancelSlot::default())
        .await
        .unwrap_err();
    assert!(
        matches!(
            e,
            AppError::Db {
                vendor_code: Some(47),
                ..
            }
        ),
        "{e:?}"
    );
}

#[tokio::test]
async fn cancel_drops_the_request_and_kills_the_query() {
    cancel_kills(false).await;
}

/// A read-only session stops its own query the same way, with `readonly=2` on the kill too.
#[tokio::test]
async fn a_read_only_session_cancels_its_own_query() {
    cancel_kills(true).await;
}

async fn cancel_kills(read_only: bool) {
    let fake = seeded();
    fake.on(
        "sleepEachRow",
        FakeReply::Hang {
            names: vec!["x".into()],
            types: vec!["UInt8".into()],
        },
    );
    let s = Arc::new(session_on(&fake, read_only).await);
    let slot = CancelSlot::default();
    let run = {
        let s = s.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut sink = CollectSink::default();
            s.run_statement(
                "SELECT sleepEachRow(1) FROM numbers(100)",
                10,
                &mut sink,
                slot,
            )
            .await
        })
    };
    for _ in 0..400 {
        if slot.is_armed() && fake.statements().iter().any(|q| q.contains("sleepEachRow")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let handle = request_cancel(&slot).expect("a running statement");
    s.cancel(handle).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("the run ends")
        .unwrap();
    assert!(matches!(result, Err(AppError::Cancelled)), "{result:?}");
    let running = fake
        .requests()
        .into_iter()
        .find(|r| r.sql().contains("sleepEachRow"))
        .unwrap();
    let id = running.param("query_id").unwrap();
    assert_eq!(fake.killed(), [id]);
    // The kill goes outside the editor's session, which the running query holds.
    let kill = fake
        .requests()
        .into_iter()
        .find(|r| r.sql().starts_with("KILL"))
        .unwrap();
    assert_eq!(kill.param("session_id"), None);
    assert_eq!(kill.param("readonly").is_some(), read_only);
}

#[tokio::test]
async fn a_query_past_the_time_out_is_killed() {
    let fake = seeded();
    fake.on(
        "sleepEachRow",
        FakeReply::Hang {
            names: vec!["x".into()],
            types: vec!["UInt8".into()],
        },
    );
    let s = session_on(&fake, false).await;
    let e = query_limited(
        &s,
        "SELECT sleepEachRow(1) FROM numbers(100)",
        &[],
        Some(Duration::from_millis(100)),
    )
    .await
    .unwrap_err();
    assert!(matches!(e, AppError::QueryTimeout { .. }), "{e:?}");
    assert_eq!(fake.killed().len(), 1);
}

#[tokio::test]
async fn processes_are_listed_and_killed_by_query_id() {
    let fake = seeded();
    fake.on(
        "FROM system.processes",
        FakeReply::rows(
            &[
                "query_id",
                "user",
                "address",
                "elapsed",
                "read_rows",
                "memory_usage",
                "current_database",
                "query",
            ],
            &[
                "String", "String", "String", "Float64", "UInt64", "Int64", "String", "String",
            ],
            &[
                r#"["q-1","bob","::1",1.5,"10","2048","default","SELECT 1"]"#,
                r#"["tabler-x","alice","::1",0.01,"0","0","default","SELECT …"]"#,
            ],
        ),
    );
    let s = session_on(&fake, false).await;
    let rows = s.list_processes().await.unwrap().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "q-1");
    assert_eq!(rows[0].duration_ms, Some(1500));
    assert!(!rows[0].own && rows[1].own);
    let out = s
        .kill_processes(&["q-1".into()], KillScope::Query)
        .await
        .unwrap();
    assert!(out[0].ok, "{out:?}");
    assert_eq!(fake.killed(), ["q-1"]);
    let out = s
        .kill_processes(&["q-1".into()], KillScope::Connection)
        .await
        .unwrap();
    assert!(!out[0].ok);
}

#[tokio::test]
async fn a_read_only_session_refuses_every_write_before_sending() {
    let fake = seeded();
    fake.on("SELECT 1", FakeReply::rows(&["1"], &["UInt8"], &["[1]"]));
    let s = session_on(&fake, true).await;
    let refused = |r: Result<(), AppError>| match r {
        Err(e) => assert_eq!(e.to_string(), READ_ONLY_MESSAGE),
        Ok(()) => panic!("a write went through"),
    };
    refused(s.execute("INSERT INTO t VALUES (1)", &[]).await.map(|_| ()));
    refused(
        s.execute_cancellable("ALTER TABLE t DELETE WHERE 1", &[], CancelSlot::default())
            .await
            .map(|_| ()),
    );
    refused(
        s.query("SELECT * FROM url('http://x', CSV)", &[])
            .await
            .map(|_| ()),
    );
    let mut sink = CollectSink::default();
    refused(
        s.run_statement(
            "CREATE TABLE x (a UInt8) ENGINE = Memory",
            10,
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .map(|_| ()),
    );
    refused(
        s.stream_query(
            "SELECT * FROM file('/etc/passwd')",
            &[],
            &mut sink,
            CancelSlot::default(),
        )
        .await
        .map(|_| ()),
    );
    let e = s
        .transaction(vec![
            TxStatement::new("SELECT 1", vec![]),
            TxStatement::new("DELETE FROM t WHERE id = 1", vec![]).expecting(1),
        ])
        .await
        .unwrap_err();
    assert_eq!(e.to_string(), READ_ONLY_MESSAGE);
    assert!(matches!(e, AppError::InvalidInput(_)));
    let out = s
        .kill_processes(&["q".into()], KillScope::Query)
        .await
        .unwrap();
    assert!(!out[0].ok);
    // Nothing that writes reached the server; reads still go, with `readonly=2` first.
    for sql in fake.statements() {
        let upper = sql.to_ascii_uppercase();
        for w in [
            "INSERT", "ALTER", "CREATE", "DELETE", "KILL", "URL(", "FILE(",
        ] {
            assert!(!upper.contains(w), "{sql}");
        }
    }
    s.query("SELECT 1", &[]).await.unwrap();
    let last = fake.requests().pop().unwrap();
    assert!(last.url.contains("?readonly=2&"), "{}", last.url);
    assert!(fake.killed().is_empty());
}

#[tokio::test]
async fn a_closed_session_sends_nothing() {
    let fake = seeded();
    let s = session_on(&fake, false).await;
    let before = fake.requests().len();
    s.close().await;
    assert!(matches!(s.ping().await, Err(AppError::NotConnected)));
    assert!(matches!(
        s.list_schemas().await,
        Err(AppError::NotConnected)
    ));
    assert_eq!(fake.requests().len(), before);
}

#[tokio::test]
async fn a_commit_names_the_statement_that_failed_whatever_its_error() {
    let fake = seeded();
    fake.on("INSERT INTO t2", FakeReply::Done { written_rows: 1 });
    let s = session_on(&fake, false).await;
    // The second statement's count cannot be confirmed (a mutation without WHERE): the first
    // was applied, and the error says which one failed.
    let e = s
        .transaction(vec![
            TxStatement::new("INSERT INTO t2 SELECT 1", vec![]).expecting(1),
            TxStatement::new("ALTER TABLE t2 UPDATE a = 1", vec![]).expecting(1),
        ])
        .await
        .unwrap_err();
    assert!(
        matches!(
            e,
            AppError::Db {
                statement_index: Some(1),
                ..
            }
        ),
        "{e:?}"
    );
    assert!(e.to_string().contains("cannot confirm"), "{e}");
    assert!(!fake.statements().iter().any(|q| q.starts_with("ALTER")));
    // Each statement of the commit shows in the console log.
    let traced = s.drain_trace();
    assert!(
        traced
            .iter()
            .any(|t| t.sql.starts_with("INSERT INTO t2") && t.affected == Some(1))
    );
    assert!(
        traced
            .iter()
            .any(|t| t.sql.starts_with("ALTER TABLE t2") && t.error.is_some())
    );
}

#[tokio::test]
async fn explain_shows_the_plan_and_views_their_definition() {
    use crate::commands::query::explain_inner;
    use crate::types::query::ExplainMode;
    let fake = seeded();
    fake.on(
        "EXPLAIN SELECT",
        FakeReply::rows(
            &["explain"],
            &["String"],
            &[
                r#"["Expression ((Project names + Projection))"]"#,
                r#"["  ReadFromMergeTree (default.t)"]"#,
            ],
        ),
    );
    let s = session_on(&fake, false).await;
    let plan = explain_inner(
        &s,
        "SELECT * FROM t",
        ExplainMode::Plan,
        &CancelSlot::default(),
    )
    .await
    .unwrap();
    assert_eq!(plan.rows.len(), 2);
    assert!(
        fake.statements()
            .iter()
            .any(|q| q == "EXPLAIN SELECT * FROM t"),
        "{:?}",
        fake.statements()
    );
    // Explain Analyze is not offered.
    assert!(
        explain_inner(
            &s,
            "SELECT * FROM t",
            ExplainMode::Analyze,
            &CancelSlot::default()
        )
        .await
        .is_err()
    );
    // A view's definition is the SELECT the server keeps (here the whole CREATE text).
    let def = s
        .view_definition(&TableRef::new(Some("default"), "v"))
        .await
        .unwrap()
        .unwrap();
    assert!(def.starts_with("CREATE TABLE"), "{def}");
}

/// The caps of a session (a stored password, a Command Line password, and a read-only profile),
/// kept in `tests/fixtures/clickhouse/caps.json`: the renderer's mock tests read the same file,
/// so the mock's caps cannot drift from these. `TABLER_BLESS=1` rewrites it.
#[tokio::test]
async fn caps_match_the_recorded_fixture() {
    let fake = seeded();
    let mut all = serde_json::Map::new();
    for (name, read_only, confirm) in [
        ("password", false, SafeConfirm::Password),
        ("command", false, SafeConfirm::Passcode),
        ("readOnly", true, SafeConfirm::Password),
    ] {
        let s = ClickhouseSession::start(
            fake.clone(),
            Endpoint {
                tls: false,
                host: "ch.example".into(),
                port: 8123,
            },
            Credentials::Password {
                user: "alice".into(),
                password: "secret".into(),
            },
            conn_settings(read_only),
            SessionOptions {
                tls: false,
                confirm,
                session_timeout_secs: None,
            },
        )
        .await
        .expect("a session");
        all.insert(
            name.into(),
            serde_json::to_value(s.capabilities()).expect("caps"),
        );
    }
    let got = serde_json::Value::Object(all);
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/clickhouse/caps.json");
    if std::env::var("TABLER_BLESS").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("folder");
        let text = serde_json::to_string_pretty(&got).expect("json");
        std::fs::write(&path, text + "\n").expect("written");
        return;
    }
    let want: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    assert!(
        got == want,
        "{} differs (TABLER_BLESS=1 rewrites it); now:\n{}",
        path.display(),
        serde_json::to_string_pretty(&got).expect("json")
    );
}
