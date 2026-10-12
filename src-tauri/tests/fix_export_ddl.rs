//! SQL export scripts reloaded into a fresh database, and DDL that must not lose an object,
//! against real servers. Gated on `TABLER_TEST_PG_URL` / `TABLER_TEST_MYSQL_URL` (each test prints
//! a skip notice and passes when its variable is unset). Everything happens in scratch databases
//! named `fix_export_<random>`, dropped again.

// `AppError` carries the full server diagnostic (same allowance as the library).
#![allow(clippy::result_large_err)]

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Arc;

use futures::FutureExt as _;
use mysql_async::prelude::Queryable as _;
use tabler_lib::commands::ddl::{apply_plan, plan_on};
use tabler_lib::drivers::{ConnectTarget, DbSession, connect};
use tabler_lib::export::run_export;
use tabler_lib::export::structure::pg_table_context;
use tabler_lib::export::tables::run_export_tables;
use tabler_lib::sqlgen::Dialect;
use tabler_lib::sqlgen::ddl::create_table_sql_with;
use tabler_lib::types::ddl::{CheckDef, DdlOp, IndexDef};
use tabler_lib::types::export::{
    ExportFormat, ExportLayout, ExportRequest, ExportSource, ExportTablesRequest,
};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::schema::{TableMeta, TableRef};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------------------------

fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `url` with its database replaced by `db`.
fn with_database(url: &str, db: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let host = rest.split('/').next().unwrap_or(rest);
    let query = rest
        .split_once('?')
        .map(|(_, q)| format!("?{q}"))
        .unwrap_or_default();
    format!("{scheme}://{host}/{db}{query}")
}

/// Parses `scheme://user[:pass]@host[:port]/db` into a profile plus the password.
fn profile_from_url(driver: Driver, url: &str) -> (ConnectionProfile, Option<String>) {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let (creds, hostdb) = match rest.rsplit_once('@') {
        Some((c, h)) => (Some(c), h),
        None => (None, rest),
    };
    let (user, pass) = match creds {
        Some(c) => match c.split_once(':') {
            Some((u, p)) => (Some(pct_decode(u)), Some(pct_decode(p))),
            None => (Some(pct_decode(c)), None),
        },
        None => (None, None),
    };
    let (hostport, db) = match hostdb.split_once('/') {
        Some((h, d)) => (
            h,
            Some(d.split('?').next().unwrap_or("").to_string()).filter(|d| !d.is_empty()),
        ),
        None => (hostdb, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (hostport.to_string(), None),
    };
    let profile = ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "fix-export".into(),
        color: None,
        group: None,
        driver,
        host: Some(host),
        port,
        user,
        database: db,
        ssl: if driver == Driver::Postgres {
            SslMode::Prefer
        } else {
            SslMode::Disable
        },
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions {
            connect_timeout_secs: Some(10),
            ..ProfileOptions::default()
        },
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    };
    (profile, pass)
}

async fn session(driver: Driver, url: &str) -> Arc<dyn DbSession> {
    session_with(driver, url, None).await
}

async fn session_with(driver: Driver, url: &str, startup_sql: Option<&str>) -> Arc<dyn DbSession> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let (mut profile, password) = profile_from_url(driver, url);
    profile.options.startup_sql = startup_sql.map(str::to_string);
    connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .expect("connect")
}

async fn pg_raw(url: &str) -> tokio_postgres::Client {
    let (client, conn) = tokio_postgres::connect(url, tokio_postgres::NoTls)
        .await
        .expect("raw postgres connect");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    client
}

/// Runs `body`, then `cleanup` (also when `body` panics), then re-raises a panic.
async fn with_cleanup(body: impl Future<Output = ()>, cleanup: impl Future<Output = ()>) {
    let outcome = AssertUnwindSafe(body).catch_unwind().await;
    cleanup.await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

fn scratch() -> String {
    format!(
        "fix_export_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10]
    )
}

fn script_format() -> ExportFormat {
    ExportFormat::SqlInsert {
        table_name: String::new(),
        batch_size: 3,
        drop_table: true,
        structure: true,
        content: true,
        gzip: false,
    }
}

async fn export_tables(s: &dyn DbSession, schema: &str, names: &[&str], file: &Path) -> String {
    let tables: Vec<TableRef> = names
        .iter()
        .map(|n| TableRef::new(Some(schema), n))
        .collect();
    let mut metas: Vec<Arc<TableMeta>> = Vec::new();
    for t in &tables {
        metas.push(Arc::new(s.table_meta(t).await.expect("meta")));
    }
    let req = ExportTablesRequest {
        export_id: "fix-export".into(),
        tables,
        format: script_format(),
        layout: ExportLayout::SingleFile,
        target: file.to_string_lossy().into_owned(),
    };
    run_export_tables(s, &req, &metas, |_| {}, CancellationToken::new())
        .await
        .expect("export tables");
    std::fs::read_to_string(file).unwrap()
}

async fn export_table(s: &dyn DbSession, schema: &str, name: &str, file: &Path) -> String {
    let req = ExportRequest {
        export_id: "fix-export-one".into(),
        source: ExportSource::Table {
            table: TableRef::new(Some(schema), name),
            filter: None,
            sort: vec![],
            columns: None,
        },
        format: script_format(),
        path: file.to_string_lossy().into_owned(),
        max_rows: None,
    };
    run_export(s, &req, None, |_| {}, CancellationToken::new())
        .await
        .expect("export table");
    std::fs::read_to_string(file).unwrap()
}

async fn pg_int(c: &tokio_postgres::Client, sql: &str) -> i64 {
    let row = c
        .query_one(sql, &[])
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    row.get::<_, i64>(0)
}

// ---------------------------------------------------------------------------------------------
// PostgreSQL
// ---------------------------------------------------------------------------------------------

const PG_SOURCE: &str = r#"
CREATE TABLE ser (id serial PRIMARY KEY, v text);
INSERT INTO ser (v) SELECT 'r' || g FROM generate_series(1, 5) g;
DELETE FROM ser WHERE id = 5;
CREATE TABLE idt (id int GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, v text);
INSERT INTO idt (v) VALUES ('a'), ('b'), ('c');
CREATE TABLE emp (id int PRIMARY KEY, boss int REFERENCES emp (id));
INSERT INTO emp VALUES (2, NULL), (1, 2);
CREATE TABLE audit_log (n int);
CREATE FUNCTION emp_audit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN INSERT INTO audit_log VALUES (NEW.id); RETURN NEW; END $$;
CREATE TRIGGER emp_audit AFTER INSERT ON emp FOR EACH ROW EXECUTE FUNCTION emp_audit();
CREATE TABLE meas (id int GENERATED BY DEFAULT AS IDENTITY, ts int NOT NULL, x int,
  PRIMARY KEY (id, ts)) PARTITION BY RANGE (ts);
CREATE INDEX meas_x_idx ON meas (x);
CREATE TABLE meas_a PARTITION OF meas FOR VALUES FROM (0) TO (10);
CREATE TABLE meas_b PARTITION OF meas FOR VALUES FROM (10) TO (20) PARTITION BY LIST (ts);
CREATE TABLE meas_b1 PARTITION OF meas_b DEFAULT;
CREATE TABLE meas_rest PARTITION OF meas DEFAULT;
INSERT INTO meas (ts, x) SELECT g, g FROM generate_series(0, 25) g;
CREATE TABLE refx (id int PRIMARY KEY, mid int, mts int, FOREIGN KEY (mid, mts) REFERENCES meas (id, ts));
INSERT INTO refx VALUES (1, 1, 0);
CREATE TABLE s2 (id serial PRIMARY KEY, v text);
INSERT INTO s2 (v) VALUES ('a'), ('b'), ('c');
CREATE TABLE s3 (id serial PRIMARY KEY, v text);
INSERT INTO s3 (v) VALUES ('a'), ('b'), ('c');
"#;

/// What the target database needs that a table script does not carry (functions).
const PG_TARGET: &str = r#"
CREATE TABLE audit_log (n int);
CREATE FUNCTION emp_audit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN INSERT INTO audit_log VALUES (NEW.id); RETURN NEW; END $$;
"#;

#[tokio::test]
async fn pg_scripts_reload_and_serial_becomes_identity() {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return;
    };
    let admin = pg_raw(&url).await;
    let (src, dst) = (scratch(), scratch());
    for db in [&src, &dst] {
        admin
            .batch_execute(&format!("CREATE DATABASE {db}"))
            .await
            .expect("create scratch database");
    }
    let (src_url, dst_url) = (with_database(&url, &src), with_database(&url, &dst));
    let body = async {
        pg_raw(&src_url)
            .await
            .batch_execute(PG_SOURCE)
            .await
            .expect("source");
        let target = pg_raw(&dst_url).await;
        target.batch_execute(PG_TARGET).await.expect("target");
        let s = session(Driver::Postgres, &src_url).await;
        let dir = tempfile::tempdir().unwrap();

        // 1. Several tables (a partition among them, the others recreated with their parent),
        //    loaded twice: into the empty database, then over the copy.
        let script = export_tables(
            s.as_ref(),
            "public",
            &["refx", "meas_a", "emp", "ser", "idt", "meas"],
            &dir.path().join("all.sql"),
        )
        .await;
        println!("{script}");
        target.batch_execute(&script).await.expect("first load");
        target
            .batch_execute(&script)
            .await
            .expect("load over the copy");
        assert_eq!(pg_int(&target, "SELECT count(*) FROM meas").await, 26);
        assert_eq!(pg_int(&target, "SELECT count(*) FROM meas_a").await, 10);
        assert_eq!(pg_int(&target, "SELECT count(*) FROM meas_b1").await, 10);
        assert_eq!(
            pg_int(
                &target,
                "SELECT count(*) FROM pg_inherits WHERE inhparent = 'meas'::regclass"
            )
            .await,
            3
        );
        assert_eq!(
            pg_int(
                &target,
                "SELECT count(*) FROM pg_index WHERE NOT indisvalid"
            )
            .await,
            0
        );
        // the sequences continue after the rows
        assert_eq!(
            pg_int(
                &target,
                "INSERT INTO ser (v) VALUES ('n') RETURNING id::int8"
            )
            .await,
            5
        );
        assert_eq!(
            pg_int(
                &target,
                "INSERT INTO idt (v) VALUES ('d') RETURNING id::int8"
            )
            .await,
            4
        );
        assert_eq!(
            pg_int(
                &target,
                "INSERT INTO meas (ts, x) VALUES (5, 5) RETURNING id::int8"
            )
            .await,
            27
        );
        // the trigger did not fire on the load, and exists
        assert_eq!(pg_int(&target, "SELECT count(*) FROM audit_log").await, 0);
        target
            .batch_execute("INSERT INTO emp VALUES (3, 1)")
            .await
            .unwrap();
        assert_eq!(pg_int(&target, "SELECT count(*) FROM audit_log").await, 1);
        // the keys exist (once: no clones of the key that references a partitioned table)
        assert!(
            target
                .batch_execute("INSERT INTO emp VALUES (9, 99)")
                .await
                .is_err()
        );
        assert!(
            target
                .batch_execute("INSERT INTO refx VALUES (9, 99, 0)")
                .await
                .is_err()
        );
        assert_eq!(
            pg_int(
                &target,
                "SELECT count(*) FROM pg_constraint WHERE conrelid = 'refx'::regclass AND contype = 'f' AND conparentid = 0"
            )
            .await,
            1
        );

        // 2. One serial table, alone, over the copy.
        let script = export_table(s.as_ref(), "public", "ser", &dir.path().join("ser.sql")).await;
        target.batch_execute(&script).await.expect("load ser");
        assert_eq!(
            pg_int(
                &target,
                "INSERT INTO ser (v) VALUES ('n') RETURNING id::int8"
            )
            .await,
            5
        );

        // 3. A partitioned table alone: its partitions come with it, its rows once.
        target.batch_execute("DROP TABLE refx").await.unwrap();
        let script = export_table(s.as_ref(), "public", "meas", &dir.path().join("meas.sql")).await;
        target.batch_execute(&script).await.expect("load meas");
        assert_eq!(pg_int(&target, "SELECT count(*) FROM meas").await, 26);
        assert_eq!(pg_int(&target, "SELECT count(*) FROM meas_b1").await, 10);

        // 4. Show DDL of a table referencing a partitioned one: one foreign key.
        let refx = s
            .table_meta(&TableRef::new(Some("public"), "refx"))
            .await
            .unwrap();
        assert!(
            refx.foreign_keys.len() > 1,
            "the clones are in the metadata"
        );
        let context = pg_table_context(s.as_ref(), &refx.table, false)
            .await
            .unwrap();
        let ddl = create_table_sql_with(Dialect::Postgres, &refx, &context);
        assert_eq!(ddl.matches("FOREIGN KEY").count(), 1, "{ddl}");

        // 5. serial → identity: the identity sequence continues after the rows, also when the
        //    auto-increment was turned off first.
        let source = pg_raw(&src_url).await;
        for (table, steps) in [("s2", vec![true]), ("s3", vec![false, true])] {
            for on in steps {
                let t = TableRef::new(Some("public"), table);
                let meta = s.table_meta(&t).await.unwrap();
                let op = DdlOp::AlterColumnAutoIncrement {
                    table: t.clone(),
                    name: "id".into(),
                    auto_increment: on,
                };
                let plan = plan_on(s.as_ref(), Some(&meta), &[op]).await.unwrap();
                let result = apply_plan(s.as_ref(), &plan).await;
                assert!(result.error.is_none(), "{table}: {:?}", result.error);
            }
            let next = pg_int(
                &source,
                &format!("INSERT INTO {table} (v) VALUES ('d') RETURNING id::int8"),
            )
            .await;
            assert_eq!(next, 4, "{table}");
        }
        s.close().await;
    };
    let cleanup = async {
        for db in [&src, &dst] {
            let _ = admin
                .batch_execute(&format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"))
                .await;
        }
    };
    with_cleanup(body, cleanup).await;
}

// ---------------------------------------------------------------------------------------------
// MySQL
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn mysql_index_and_check_edits_keep_the_original_and_scripts_reload() {
    let Ok(url) = std::env::var("TABLER_TEST_MYSQL_URL") else {
        println!("skipped: TABLER_TEST_MYSQL_URL unset");
        return;
    };
    let mut admin = mysql_async::Conn::from_url(url.as_str())
        .await
        .expect("raw mysql connect");
    let (src, dst) = (scratch(), scratch());
    admin
        .query_drop(format!("CREATE DATABASE {src}"))
        .await
        .unwrap();
    admin
        .query_drop(format!("CREATE DATABASE {dst}"))
        .await
        .unwrap();
    let (src_url, dst_url) = (with_database(&url, &src), with_database(&url, &dst));
    let body = async {
        let mut raw = mysql_async::Conn::from_url(src_url.as_str()).await.unwrap();
        raw.query_drop(
            "CREATE TABLE t (id int AUTO_INCREMENT PRIMARY KEY, a int, CONSTRAINT ck CHECK (a > 0), \
             INDEX ix (a)); \
             INSERT INTO t (a) VALUES (1), (1), (2); \
             CREATE TABLE c (id int PRIMARY KEY, t_id int, FOREIGN KEY (t_id) REFERENCES t (id)); \
             INSERT INTO c VALUES (1, 3);",
        )
        .await
        .unwrap();
        let s = session(Driver::Mysql, &src_url).await;
        let t = TableRef::new(Some(&src), "t");
        let meta = s.table_meta(&t).await.unwrap();

        // An index made UNIQUE over duplicates: the ALTER fails as a whole, the index stays.
        let ops = [
            DdlOp::DropIndex {
                table: t.clone(),
                name: "ix".into(),
            },
            DdlOp::AddIndex {
                table: t.clone(),
                index: IndexDef {
                    name: "ix".into(),
                    columns: vec!["a".into()],
                    unique: true,
                    method: None,
                    predicate: None,
                    comment: None,
                },
            },
        ];
        let plan = plan_on(s.as_ref(), Some(&meta), &ops).await.unwrap();
        assert_eq!(plan.statements.len(), 1, "{:?}", plan.statements);
        assert!(apply_plan(s.as_ref(), &plan).await.error.is_some());
        let after = s.table_meta(&t).await.unwrap();
        let ix = after
            .indexes
            .iter()
            .find(|i| i.name == "ix")
            .expect("ix kept");
        assert!(!ix.unique);

        // A CHECK the rows violate: the old one stays.
        let ops = [
            DdlOp::DropCheck {
                table: t.clone(),
                name: "ck".into(),
            },
            DdlOp::AddCheck {
                table: t.clone(),
                check: CheckDef {
                    name: Some("ck".into()),
                    expression: "a > 5".into(),
                },
            },
        ];
        let plan = plan_on(s.as_ref(), Some(&after), &ops).await.unwrap();
        assert_eq!(plan.statements.len(), 1, "{:?}", plan.statements);
        assert!(apply_plan(s.as_ref(), &plan).await.error.is_some());
        let after = s.table_meta(&t).await.unwrap();
        assert_eq!(after.checks.len(), 1);
        assert!(
            after.checks[0].expression.contains("> 0"),
            "{:?}",
            after.checks
        );

        // A valid edit applies.
        let ops = [
            DdlOp::DropIndex {
                table: t.clone(),
                name: "ix".into(),
            },
            DdlOp::AddIndex {
                table: t.clone(),
                index: IndexDef {
                    name: "ix".into(),
                    columns: vec!["a".into(), "id".into()],
                    unique: true,
                    method: None,
                    predicate: None,
                    comment: None,
                },
            },
        ];
        let plan = plan_on(s.as_ref(), Some(&after), &ops).await.unwrap();
        let result = apply_plan(s.as_ref(), &plan).await;
        assert!(result.error.is_none(), "{:?}", result.error);

        // A script of both tables loads twice; AUTO_INCREMENT continues after the rows.
        let dir = tempfile::tempdir().unwrap();
        let script =
            export_tables(s.as_ref(), &src, &["c", "t"], &dir.path().join("all.sql")).await;
        assert!(script.contains("FOREIGN_KEY_CHECKS = 0"), "{script}");
        let mut target = mysql_async::Conn::from_url(dst_url.as_str()).await.unwrap();
        target
            .query_drop(script.as_str())
            .await
            .expect("first load");
        target
            .query_drop(script.as_str())
            .await
            .expect("load over the copy");
        target
            .query_drop("INSERT INTO t (a) VALUES (7)")
            .await
            .unwrap();
        let id: Option<i64> = target.query_first("SELECT MAX(id) FROM t").await.unwrap();
        assert_eq!(id, Some(4));
        s.close().await;

        // Comments with a backslash and a quote are stored exactly, whatever the session's
        // sql_mode says about backslashes.
        let comment = r"C:\temp it's";
        for startup in [
            None,
            Some("SET SESSION sql_mode = CONCAT(@@SESSION.sql_mode, ',NO_BACKSLASH_ESCAPES')"),
        ] {
            let s = session_with(Driver::Mysql, &src_url, startup).await;
            let meta = s.table_meta(&t).await.unwrap();
            let ops = [DdlOp::SetTableComment {
                table: t.clone(),
                comment: Some(comment.into()),
            }];
            let plan = plan_on(s.as_ref(), Some(&meta), &ops).await.unwrap();
            let doubled = plan.statements[0].contains(r"C:\\temp");
            assert_eq!(doubled, startup.is_none(), "{:?}", plan.statements);
            let result = apply_plan(s.as_ref(), &plan).await;
            assert!(result.error.is_none(), "{startup:?}: {:?}", result.error);
            let stored: Option<String> = raw
                .exec_first(
                    "SELECT TABLE_COMMENT FROM information_schema.TABLES \
                     WHERE TABLE_SCHEMA = ? AND TABLE_NAME = 't'",
                    (src.as_str(),),
                )
                .await
                .unwrap();
            assert_eq!(
                stored.as_deref(),
                Some(comment),
                "{startup:?}: {:?}",
                plan.statements
            );
            s.close().await;
        }
    };
    let cleanup = async {
        let _ = admin
            .query_drop(format!("DROP DATABASE IF EXISTS {src}"))
            .await;
        let _ = admin
            .query_drop(format!("DROP DATABASE IF EXISTS {dst}"))
            .await;
    };
    with_cleanup(body, cleanup).await;
}
