//! Structure / export / import round 2 against a real PostgreSQL: domain columns that fail
//! instead of being cut on CSV import and edits, trigger enable / disable, INCLUDE columns of
//! indexes, and export scripts that recreate the partitions' own indexes and triggers and reset a
//! shared sequence once. Gated on `TABLER_TEST_PG_URL` (`postgres://user[:pass]@host[:port]/db`;
//! each test prints a skip notice and passes when it is unset). Every object is named
//! `struct2_*` or `fx_diag_*` (scratch databases and schemas too) and dropped at the end.

// `AppError` carries the full server diagnostic (same allowance as the library).
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;
use std::future::Future;
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Arc;

use futures::FutureExt as _;
use tokio_util::sync::CancellationToken;

use tabler_lib::commands::ddl::{apply_plan, plan_on};
use tabler_lib::drivers::{ConnectTarget, DbSession, connect};
use tabler_lib::error::AppError;
use tabler_lib::export::tables::run_export_tables;
use tabler_lib::import::run_csv_import;
use tabler_lib::sqlgen::Dialect;
use tabler_lib::sqlgen::mutate::{TableSpec, build_update};
use tabler_lib::types::browse::RowKey;
use tabler_lib::types::ddl::{DdlOp, IndexDef};
use tabler_lib::types::export::{ExportFormat, ExportLayout, ExportTablesRequest};
use tabler_lib::types::import::{
    CsvFormat, CsvImportReport, CsvImportRequest, ImportTarget, InsertMode, TextEncoding,
};
use tabler_lib::types::profile::ConnectionProfile;
use tabler_lib::types::schema::{TableMeta, TableRef};
use tabler_lib::types::value::Value;

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

/// A session for `postgres://user[:pass]@host[:port]/db`.
async fn session(url: &str) -> Arc<dyn DbSession> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let (auth, hostdb) = rest
        .rsplit_once('@')
        .map_or((None, rest), |(a, h)| (Some(a), h));
    let (user, password) = match auth.map(|a| a.split_once(':').unwrap_or((a, ""))) {
        Some((u, p)) => (
            Some(pct_decode(u)),
            Some(pct_decode(p)).filter(|p| !p.is_empty()),
        ),
        None => (None, None),
    };
    let (hostport, db) = hostdb.split_once('/').unwrap_or((hostdb, ""));
    let (host, port) = hostport
        .rsplit_once(':')
        .map_or((hostport, None), |(h, p)| (h, p.parse::<u16>().ok()));
    // Only what a session needs; the rest defaults (robust to new profile fields).
    let profile: ConnectionProfile = serde_json::from_value(serde_json::json!({
        "id": uuid::Uuid::new_v4(),
        "name": "struct2",
        "driver": "postgres",
        "host": host,
        "port": port,
        "user": user,
        "database": db.split('?').next().filter(|d| !d.is_empty()),
        "ssl": "disable",
    }))
    .expect("profile");
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

fn suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..10].to_string()
}

fn sqlstate(e: &AppError) -> Option<&str> {
    match e {
        AppError::Db { sqlstate, .. } => sqlstate.as_deref(),
        _ => None,
    }
}

async fn pg_int(c: &tokio_postgres::Client, sql: &str) -> i64 {
    let row = c
        .query_one(sql, &[])
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    row.get::<_, i64>(0)
}

// ---------------------------------------------------------------------------------------------
// CSV import and edits through domains
// ---------------------------------------------------------------------------------------------

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::File::create(&path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    path.to_string_lossy().into_owned()
}

async fn import_csv(
    s: &dyn DbSession,
    dir: &Path,
    meta: &TableMeta,
    text: &str,
    mapping: &[&str],
) -> CsvImportReport {
    let req = CsvImportRequest {
        import_id: "struct2".into(),
        path: write(dir, "data.csv", text),
        format: CsvFormat {
            delimiter: Some(",".into()),
            quote: "\"".into(),
            encoding: Some(TextEncoding::Utf8),
            header: true,
        },
        target: ImportTarget::Existing {
            table: meta.table.clone(),
        },
        mapping: mapping.iter().map(|m| Some(m.to_string())).collect(),
        empty_as_null: true,
        mode: InsertMode::StopOnError,
    };
    run_csv_import(s, &req, Some(meta), |_| {}, CancellationToken::new())
        .await
        .expect("import")
}

#[tokio::test]
async fn struct2_pg_domain_columns_fail_instead_of_cutting() {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return;
    };
    let s = session(&url).await;
    let id = suffix();
    let (code, tag, items) = (
        format!("struct2_code_{id}"),
        format!("struct2_tag_{id}"),
        format!("struct2_items_{id}"),
    );
    for sql in [
        format!("CREATE DOMAIN public.{code} AS varchar(3)"),
        format!("CREATE DOMAIN public.{tag} AS public.{code} CHECK (VALUE <> 'x')"),
        format!(
            "CREATE TABLE public.{items} (id int PRIMARY KEY, code public.{code}, \
             tag public.{tag}, tags public.{code}[])"
        ),
    ] {
        s.execute(&sql, &[]).await.expect(&sql);
    }
    let table = TableRef::new(Some("public"), &items);
    let body = async {
        let meta = s.table_meta(&table).await.unwrap();
        let col = |name: &str| meta.columns.iter().find(|c| c.name == name).unwrap();
        // the structure keeps showing the domain; casts go to what it is over
        assert_eq!(col("code").data_type, code);
        assert_eq!(col("code").base_type.as_deref(), Some("character varying"));
        assert_eq!(col("tag").base_type.as_deref(), Some("character varying"));
        assert_eq!(
            col("tags").base_type.as_deref(),
            Some("character varying[]")
        );
        assert_eq!(col("id").base_type, None);

        // A value too long for the domain's varchar(3) fails (22001) instead of being cut.
        let dir = tempfile::tempdir().unwrap();
        for column in ["code", "tag"] {
            let report = import_csv(
                s.as_ref(),
                dir.path(),
                &meta,
                &format!("id,{column}\n1,abcdef\n"),
                &["id", column],
            )
            .await;
            assert_eq!(report.rows_imported, 0, "{report:?}");
            let failure = report.failure.expect("a failure");
            assert!(
                failure
                    .message
                    .contains("value too long for type character varying(3)"),
                "{failure:?}"
            );
        }
        let report = import_csv(
            s.as_ref(),
            dir.path(),
            &meta,
            "id,code,tag\n3,abc,xyz\n",
            &["id", "code", "tag"],
        )
        .await;
        assert_eq!(report.rows_imported, 1, "{report:?}");

        // The same for an edit of the cell.
        let spec = TableSpec {
            dialect: Dialect::Postgres,
            table: &table,
            columns: &meta.columns,
        };
        let key = RowKey::Pk(BTreeMap::from([("id".to_string(), Value::Int(3))]));
        let edit = build_update(
            &spec,
            &key,
            &BTreeMap::from([("tag".to_string(), Value::Text("abcdef".into()))]),
            false,
        )
        .unwrap();
        let err = s.execute(&edit.sql, &edit.params).await.unwrap_err();
        assert_eq!(sqlstate(&err), Some("22001"), "{err}");
        let stored = s
            .query(
                &format!("SELECT tag::text FROM public.{items} WHERE id = 3"),
                &[],
            )
            .await
            .unwrap();
        assert_eq!(stored.rows[0][0], Value::Text("xyz".into()));
    };
    let cleanup = async {
        for sql in [
            format!("DROP TABLE IF EXISTS public.{items}"),
            format!("DROP DOMAIN IF EXISTS public.{tag}"),
            format!("DROP DOMAIN IF EXISTS public.{code}"),
        ] {
            s.execute(&sql, &[]).await.expect(&sql);
        }
    };
    with_cleanup(body, cleanup).await;
}

// ---------------------------------------------------------------------------------------------
// Triggers, INCLUDE columns and export scripts
// ---------------------------------------------------------------------------------------------

const SOURCE: &str = r#"
CREATE FUNCTION struct2_touch() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RETURN NEW; END$$;
CREATE SEQUENCE doc_seq AS integer;
CREATE TABLE docs (id int PRIMARY KEY DEFAULT nextval('doc_seq'), title text, body text);
ALTER SEQUENCE doc_seq OWNED BY docs.id;
CREATE TABLE notes (id int PRIMARY KEY DEFAULT nextval('doc_seq'), txt text);
INSERT INTO docs (title) VALUES ('a'), ('b'), ('c');
INSERT INTO notes (txt) VALUES ('x'), ('y');
CREATE INDEX docs_title_cover ON docs (title) INCLUDE (body);
CREATE TRIGGER docs_touch BEFORE UPDATE ON docs FOR EACH ROW EXECUTE FUNCTION struct2_touch();
CREATE TABLE ev (id int, ts int NOT NULL, region text) PARTITION BY RANGE (ts);
CREATE INDEX ev_ts_idx ON ev (ts);
CREATE TRIGGER ev_trg BEFORE INSERT ON ev FOR EACH ROW EXECUTE FUNCTION struct2_touch();
CREATE TABLE ev_1 PARTITION OF ev FOR VALUES FROM (0) TO (10);
CREATE TABLE ev_2 PARTITION OF ev FOR VALUES FROM (10) TO (20) PARTITION BY LIST (region);
CREATE TABLE ev_2a PARTITION OF ev_2 FOR VALUES IN ('a');
CREATE TABLE ev_2b PARTITION OF ev_2 DEFAULT;
CREATE INDEX ev_1_local ON ev_1 (id) INCLUDE (region);
ALTER TABLE ev_1 ADD CONSTRAINT ev_1_uq UNIQUE (id);
CREATE INDEX ev_2_local ON ev_2 (id);
CREATE INDEX ev_2a_local ON ev_2a (region);
CREATE TRIGGER ev_1_own AFTER UPDATE ON ev_1 FOR EACH ROW EXECUTE FUNCTION struct2_touch();
ALTER TABLE ev_1 DISABLE TRIGGER ev_1_own;
INSERT INTO ev SELECT g, g, CASE WHEN g % 2 = 0 THEN 'a' ELSE 'b' END
  FROM generate_series(0, 19) g;
"#;

/// What the target database needs that a table script does not carry (the trigger function).
const TARGET: &str = r#"
CREATE FUNCTION struct2_touch() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RETURN NEW; END$$;
"#;

async fn apply(s: &dyn DbSession, table: &TableRef, ops: Vec<DdlOp>) -> Vec<String> {
    let meta = s.table_meta(table).await.unwrap();
    let plan = plan_on(s, Some(&meta), &ops).await.unwrap();
    let result = apply_plan(s, &plan).await;
    assert!(result.error.is_none(), "{:?}", result.error);
    plan.statements
}

#[tokio::test]
async fn struct2_pg_triggers_include_columns_and_partition_scripts() {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return;
    };
    let admin = pg_raw(&url).await;
    let (src, dst) = (
        format!("struct2_src_{}", suffix()),
        format!("struct2_dst_{}", suffix()),
    );
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
            .batch_execute(SOURCE)
            .await
            .expect("source");
        let target = pg_raw(&dst_url).await;
        target.batch_execute(TARGET).await.expect("target");
        let s = session(&src_url).await;
        let docs = TableRef::new(Some("public"), "docs");

        // INCLUDE columns are not key columns.
        let meta = s.table_meta(&docs).await.unwrap();
        let cover = meta
            .indexes
            .iter()
            .find(|i| i.name == "docs_title_cover")
            .unwrap();
        assert_eq!(cover.columns, ["title"]);
        assert_eq!(cover.include, ["body"]);
        let ev_1 = s
            .table_meta(&TableRef::new(Some("public"), "ev_1"))
            .await
            .unwrap();
        let local = ev_1
            .indexes
            .iter()
            .find(|i| i.name == "ev_1_local")
            .unwrap();
        assert_eq!(local.columns, ["id"]);
        assert_eq!(local.include, ["region"]);

        // … and the designer creates them.
        apply(
            s.as_ref(),
            &docs,
            vec![DdlOp::AddIndex {
                table: docs.clone(),
                index: IndexDef {
                    name: "docs_id_cover".into(),
                    columns: vec!["id".into()],
                    unique: true,
                    method: None,
                    predicate: None,
                    comment: None,
                    include: vec!["title".into(), "body".into()],
                },
            }],
        )
        .await;
        let meta = s.table_meta(&docs).await.unwrap();
        let added = meta
            .indexes
            .iter()
            .find(|i| i.name == "docs_id_cover")
            .unwrap();
        assert_eq!(added.columns, ["id"]);
        assert_eq!(added.include, ["title", "body"]);
        assert!(added.unique);

        // Disable / enable a trigger.
        let trigger_enabled = |m: &TableMeta| {
            m.triggers
                .iter()
                .find(|t| t.name == "docs_touch")
                .unwrap()
                .enabled
        };
        assert!(trigger_enabled(&meta));
        let statements = apply(
            s.as_ref(),
            &docs,
            vec![DdlOp::DisableTrigger {
                table: docs.clone(),
                name: "docs_touch".into(),
            }],
        )
        .await;
        assert_eq!(
            statements,
            [r#"ALTER TABLE "public"."docs" DISABLE TRIGGER "docs_touch""#]
        );
        assert!(!trigger_enabled(&s.table_meta(&docs).await.unwrap()));
        apply(
            s.as_ref(),
            &docs,
            vec![DdlOp::EnableTrigger {
                table: docs.clone(),
                name: "docs_touch".into(),
            }],
        )
        .await;
        assert!(trigger_enabled(&s.table_meta(&docs).await.unwrap()));
        apply(
            s.as_ref(),
            &docs,
            vec![DdlOp::DisableTrigger {
                table: docs.clone(),
                name: "docs_touch".into(),
            }],
        )
        .await;

        // A script of the partitioned table and the tables sharing `doc_seq` (the owner last),
        // loaded into the empty database, then over the copy.
        let names = ["notes", "ev", "docs"];
        let tables: Vec<TableRef> = names
            .iter()
            .map(|n| TableRef::new(Some("public"), n))
            .collect();
        let mut metas: Vec<Arc<TableMeta>> = Vec::new();
        for t in &tables {
            metas.push(Arc::new(s.table_meta(t).await.expect("meta")));
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("all.sql");
        let req = ExportTablesRequest {
            export_id: "struct2".into(),
            tables,
            format: ExportFormat::SqlInsert {
                table_name: String::new(),
                batch_size: 5,
                drop_table: true,
                structure: true,
                content: true,
                gzip: false,
            },
            layout: ExportLayout::SingleFile,
            target: file.to_string_lossy().into_owned(),
        };
        run_export_tables(s.as_ref(), &req, &metas, |_| {}, CancellationToken::new())
            .await
            .expect("export");
        let script = std::fs::read_to_string(&file).unwrap();
        println!("{script}");
        assert_eq!(
            script
                .matches(r#"CREATE SEQUENCE IF NOT EXISTS "public"."doc_seq""#)
                .count(),
            1
        );
        assert_eq!(script.matches("setval").count(), 1, "one reset for doc_seq");
        target.batch_execute(&script).await.expect("first load");
        target
            .batch_execute(&script)
            .await
            .expect("load over the copy");

        assert_eq!(pg_int(&target, "SELECT count(*) FROM ev").await, 20);
        assert_eq!(pg_int(&target, "SELECT count(*) FROM ev_2a").await, 5);
        // the partitions' own indexes, the sub-partitioned one reaching its partitions
        assert_eq!(
            pg_int(
                &target,
                "SELECT count(*) FROM pg_indexes WHERE indexname = 'ev_1_local' \
                 AND indexdef LIKE '%(id) INCLUDE (region)'"
            )
            .await,
            1
        );
        assert_eq!(
            pg_int(
                &target,
                "SELECT count(*) FROM pg_constraint WHERE conname = 'ev_1_uq' AND contype = 'u'"
            )
            .await,
            1
        );
        assert_eq!(
            pg_int(
                &target,
                "SELECT count(*) FROM pg_inherits WHERE inhparent = 'ev_2_local'::regclass"
            )
            .await,
            2
        );
        assert_eq!(
            pg_int(
                &target,
                "SELECT count(*) FROM pg_indexes WHERE indexname = 'ev_2a_local'"
            )
            .await,
            1
        );
        assert_eq!(
            pg_int(
                &target,
                "SELECT count(*) FROM pg_index WHERE NOT indisvalid"
            )
            .await,
            0
        );
        // the parent's index on every partition, and nothing twice
        assert_eq!(
            pg_int(
                &target,
                "SELECT count(*) FROM pg_index ix JOIN pg_class c ON c.oid = ix.indrelid \
                 WHERE c.relname LIKE 'ev%'"
            )
            .await,
            pg_int(
                &pg_raw(&src_url).await,
                "SELECT count(*) FROM pg_index ix JOIN pg_class c ON c.oid = ix.indrelid \
                 WHERE c.relname LIKE 'ev%'"
            )
            .await
        );
        // the partition's own trigger, disabled again; the disabled table trigger too
        assert_eq!(
            pg_int(
                &target,
                "SELECT count(*) FROM pg_trigger WHERE tgname = 'ev_1_own' AND tgenabled = 'D'"
            )
            .await,
            1
        );
        assert_eq!(
            pg_int(
                &target,
                "SELECT count(*) FROM pg_trigger WHERE tgname = 'docs_touch' AND tgenabled = 'D'"
            )
            .await,
            1
        );
        // the INCLUDE index of the table itself
        assert_eq!(
            pg_int(
                &target,
                "SELECT count(*) FROM pg_indexes WHERE indexname = 'docs_id_cover' \
                 AND indexdef LIKE '%(id) INCLUDE (title, body)'"
            )
            .await,
            1
        );
        // the shared sequence continues after the rows of both tables
        assert_eq!(
            pg_int(
                &target,
                "INSERT INTO docs (title) VALUES ('d') RETURNING id::int8"
            )
            .await,
            6
        );
        assert_eq!(
            pg_int(
                &target,
                "INSERT INTO notes (txt) VALUES ('z') RETURNING id::int8"
            )
            .await,
            7
        );
    };
    let cleanup = async {
        for db in [&src, &dst] {
            admin
                .batch_execute(&format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"))
                .await
                .expect("drop scratch database");
        }
    };
    with_cleanup(body, cleanup).await;
}

/// INCLUDE columns come back by name, so a covering index on mixed-case / spaced columns can be
/// edited (dropped and created again) and its columns renamed. Objects live in a scratch schema
/// `fx_diag_*` of the test database.
#[tokio::test]
async fn struct2_pg_include_columns_keep_their_names() {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return;
    };
    let admin = pg_raw(&url).await;
    let schema = format!("fx_diag_{}", suffix());
    admin
        .batch_execute(&format!(
            r#"CREATE SCHEMA {schema};
            CREATE TABLE {schema}."People" (id bigint PRIMARY KEY, "Email" text, "first name" text);
            CREATE INDEX "People_cover" ON {schema}."People" (id) INCLUDE ("Email", "first name");"#
        ))
        .await
        .expect("setup");
    let body = async {
        let s = session(&url).await;
        let people = TableRef::new(Some(&schema), "People");
        let cover = |m: &TableMeta| {
            m.indexes
                .iter()
                .find(|i| i.name == "People_cover")
                .cloned()
                .unwrap_or_else(|| panic!("People_cover in {:?}", m.indexes))
        };
        let index = cover(&s.table_meta(&people).await.unwrap());
        assert_eq!(index.columns, ["id"]);
        assert_eq!(index.include, ["Email", "first name"]);

        // Edit it: unique now, same columns.
        let statements = apply(
            s.as_ref(),
            &people,
            vec![
                DdlOp::DropIndex {
                    table: people.clone(),
                    name: index.name.clone(),
                },
                DdlOp::AddIndex {
                    table: people.clone(),
                    index: IndexDef {
                        name: index.name.clone(),
                        columns: index.columns.clone(),
                        unique: true,
                        method: index.method.clone(),
                        predicate: None,
                        comment: None,
                        include: index.include.clone(),
                    },
                },
            ],
        )
        .await;
        assert!(
            statements[1].ends_with(r#"INCLUDE ("Email", "first name")"#),
            "{statements:?}"
        );
        let edited = cover(&s.table_meta(&people).await.unwrap());
        assert!(edited.unique);
        assert_eq!(edited.include, ["Email", "first name"]);

        // Renaming an INCLUDE column renames it in the index.
        apply(
            s.as_ref(),
            &people,
            vec![DdlOp::RenameColumn {
                table: people.clone(),
                name: "Email".into(),
                new_name: "E-mail".into(),
            }],
        )
        .await;
        let renamed = cover(&s.table_meta(&people).await.unwrap());
        assert_eq!(renamed.include, ["E-mail", "first name"]);
    };
    let cleanup = async {
        admin
            .batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
            .await
            .expect("drop scratch schema");
    };
    with_cleanup(body, cleanup).await;
}
