//! Streaming export against real servers. Gated per driver on `TABLER_TEST_PG_URL`,
//! `TABLER_TEST_MYSQL_URL` and `TABLER_TEST_SQLITE_PATH`; each test prints a skip notice and
//! passes when its variable is unset. Scratch tables are named `tabler_export_<random>` and
//! dropped again; SQLite tests work on a temporary copy of the fixture.

// `AppError` carries the full server diagnostic (same allowance as the library).
#![allow(clippy::result_large_err)]

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures::{FutureExt as _, SinkExt as _};
use mysql_async::prelude::Queryable as _;
use tabler_lib::drivers::{ConnectTarget, DbSession, connect};
use tabler_lib::error::AppError;
use tabler_lib::export::run_export;
use tabler_lib::types::browse::{Conjunction, FilterGroup, FilterNode, FilterOp};
use tabler_lib::types::export::{ExportFormat, ExportRequest, ExportResult, ExportSource};
use tabler_lib::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
use tabler_lib::types::schema::TableRef;
use tabler_lib::types::value::{BytesValue, Value};
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

fn base_profile(driver: Driver) -> ConnectionProfile {
    ConnectionProfile {
        id: uuid::Uuid::new_v4(),
        name: "export-test".into(),
        color: None,
        group: None,
        driver,
        host: None,
        port: None,
        user: None,
        database: None,
        ssl: SslMode::Disable,
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
    }
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
    let mut profile = base_profile(driver);
    profile.host = Some(host);
    profile.port = port;
    profile.user = user;
    profile.database = db;
    if driver == Driver::Postgres {
        profile.ssl = SslMode::Prefer;
    }
    (profile, pass)
}

async fn open_server(driver: Driver, env: &str) -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var(env) else {
        println!("skipped: {env} unset");
        return None;
    };
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let (profile, password) = profile_from_url(driver, &url);
    let session = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .expect("connect");
    Some(session)
}

async fn open_pg() -> Option<Arc<dyn DbSession>> {
    open_server(Driver::Postgres, "TABLER_TEST_PG_URL").await
}

async fn open_mysql() -> Option<Arc<dyn DbSession>> {
    open_server(Driver::Mysql, "TABLER_TEST_MYSQL_URL").await
}

/// A session on a temporary copy of the SQLite fixture, plus the copy's path.
async fn open_sqlite() -> Option<(Arc<dyn DbSession>, tempfile::TempDir, PathBuf)> {
    const ENV: &str = "TABLER_TEST_SQLITE_PATH";
    let Ok(rel) = std::env::var(ENV) else {
        println!("skipped: {ENV} unset");
        return None;
    };
    let fixture = {
        let p = PathBuf::from(&rel);
        if p.is_absolute() {
            p
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(p)
        }
    };
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("sample.sqlite");
    std::fs::copy(&fixture, &copy).unwrap_or_else(|e| panic!("copy {}: {e}", fixture.display()));
    let mut profile = base_profile(Driver::Sqlite);
    profile.sqlite_path = Some(copy.to_string_lossy().into_owned());
    let session = connect(ConnectTarget {
        profile: &profile,
        password: None,
        host_override: None,
    })
    .await
    .expect("open sqlite");
    Some((session, dir, copy))
}

/// A plain client on the PostgreSQL test database (for `COPY FROM STDIN` and session settings
/// the app's sessions never use).
async fn pg_raw() -> tokio_postgres::Client {
    let url = std::env::var("TABLER_TEST_PG_URL").expect("TABLER_TEST_PG_URL");
    let (client, conn) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
        .await
        .expect("raw postgres connect");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    client
}

/// A plain connection to the MySQL test database (session `sql_mode`, multi-statement scripts).
async fn mysql_raw() -> mysql_async::Conn {
    let url = std::env::var("TABLER_TEST_MYSQL_URL").expect("TABLER_TEST_MYSQL_URL");
    mysql_async::Conn::from_url(url)
        .await
        .expect("raw mysql connect")
}

/// Runs `body`, then `cleanup` (also when `body` panics), then re-raises a panic.
async fn with_cleanup(body: impl Future<Output = ()>, cleanup: impl Future<Output = ()>) {
    let outcome = AssertUnwindSafe(body).catch_unwind().await;
    cleanup.await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

fn scratch_name() -> String {
    format!(
        "tabler_export_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10]
    )
}

// ---------------------------------------------------------------------------------------------
// Export helpers
// ---------------------------------------------------------------------------------------------

fn table(schema: Option<&str>, name: &str) -> ExportSource {
    ExportSource::Table {
        table: TableRef::new(schema, name),
        filter: None,
        sort: vec![],
        columns: None,
    }
}

fn sql(text: &str) -> ExportSource {
    ExportSource::Sql { sql: text.into() }
}

fn csv() -> ExportFormat {
    ExportFormat::Csv {
        delimiter: ",".into(),
        header: true,
        quote_all: false,
        null_as: String::new(),
    }
}

fn csv_with(quote_all: bool, null_as: &str) -> ExportFormat {
    ExportFormat::Csv {
        delimiter: ",".into(),
        header: true,
        quote_all,
        null_as: null_as.into(),
    }
}

fn ndjson() -> ExportFormat {
    ExportFormat::Json {
        ndjson: true,
        pretty: false,
    }
}

fn json() -> ExportFormat {
    ExportFormat::Json {
        ndjson: false,
        pretty: false,
    }
}

async fn export(
    session: &dyn DbSession,
    source: ExportSource,
    format: ExportFormat,
    path: &Path,
) -> Result<ExportResult, AppError> {
    let req = ExportRequest {
        export_id: "it".into(),
        source,
        format,
        path: path.to_string_lossy().into_owned(),
        max_rows: None,
    };
    run_export(session, &req, None, |_| {}, CancellationToken::new()).await
}

fn files_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// CSV with a header line and 100000 rows; JSON with 100000 objects. `typed`: the server has
/// real decimal and JSON column types (not SQLite).
async fn check_events_big(session: &dyn DbSession, schema: Option<&str>, typed: bool) {
    let dir = tempfile::tempdir().unwrap();

    let csv_path = dir.path().join("events.csv");
    let started = Instant::now();
    let result = export(session, table(schema, "events_big"), csv(), &csv_path)
        .await
        .expect("csv export");
    println!("csv: {result:?} in {:?}", started.elapsed());
    assert_eq!(result.rows, 100_000);
    let text = std::fs::read_to_string(&csv_path).unwrap();
    assert_eq!(result.bytes, text.len() as u64);
    assert_eq!(text.matches('\n').count(), 100_001);
    assert!(
        text.starts_with("id,ts,kind,value,payload\r\n"),
        "{}",
        &text[..80]
    );
    let mut rdr = csv::Reader::from_reader(text.as_bytes());
    let records: Vec<csv::StringRecord> = rdr.records().map(|r| r.unwrap()).collect();
    assert_eq!(records.len(), 100_000);
    // Sorted by the primary key; every id once.
    let ids: Vec<i64> = records.iter().map(|r| r[0].parse().unwrap()).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]));

    let json_path = dir.path().join("events.json");
    let result = export(
        session,
        sql("SELECT id, ts, kind, value, payload FROM events_big ORDER BY id"),
        json(),
        &json_path,
    )
    .await
    .expect("json export");
    assert_eq!(result.rows, 100_000);
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
    let rows = parsed.as_array().unwrap();
    assert_eq!(rows.len(), 100_000);
    let first = &rows[0];
    assert!(first["id"].is_i64(), "{first}");
    assert!(first["ts"].is_string());
    if typed {
        assert!(first["value"].is_string(), "decimals stay strings: {first}");
        assert!(
            rows.iter()
                .any(|r| r["payload"].is_object() || r["payload"].is_array()),
            "json payloads are embedded"
        );
    }

    let nd_path = dir.path().join("events.ndjson");
    let result = export(
        session,
        table(schema, "events_big"),
        ExportFormat::Json {
            ndjson: true,
            pretty: false,
        },
        &nd_path,
    )
    .await
    .expect("ndjson export");
    assert_eq!(result.rows, 100_000);
    let text = std::fs::read_to_string(&nd_path).unwrap();
    assert_eq!(text.lines().count(), 100_000);
    for line in text.lines().take(100) {
        serde_json::from_str::<serde_json::Value>(line).unwrap();
    }

    let mut names = files_in(dir.path());
    names.sort();
    assert_eq!(names, ["events.csv", "events.json", "events.ndjson"]);
}

/// Cancelling from the progress callback stops the export and removes its temporary file.
async fn check_cancel_mid_export(session: &dyn DbSession, schema: Option<&str>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cancelled.csv");
    let req = ExportRequest {
        export_id: "cancel".into(),
        source: table(schema, "events_big"),
        format: csv(),
        path: path.to_string_lossy().into_owned(),
        max_rows: None,
    };
    let token = CancellationToken::new();
    let trigger = token.clone();
    let mut seen = 0;
    let err = run_export(
        session,
        &req,
        None,
        |p| {
            seen = p.rows;
            if p.rows > 0 {
                trigger.cancel();
            }
        },
        token,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
    assert!(seen > 0 && seen < 100_000, "rows at cancel: {seen}");
    assert!(
        files_in(dir.path()).is_empty(),
        "{:?}",
        files_in(dir.path())
    );
    // The worker connection is usable afterwards.
    session.ping().await.expect("ping after cancel");
    let again = export(
        session,
        sql("SELECT 1 AS one"),
        csv(),
        &dir.path().join("one.csv"),
    )
    .await
    .expect("export after cancel");
    assert_eq!(again.rows, 1);
}

/// A statement that produces nothing for a long time is cancelled server-side.
async fn check_cancel_running_statement(session: &dyn DbSession, sleep_sql: &str) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sleep.csv");
    let req = ExportRequest {
        export_id: "sleep".into(),
        source: sql(sleep_sql),
        format: csv(),
        path: path.to_string_lossy().into_owned(),
        max_rows: None,
    };
    let token = CancellationToken::new();
    let trigger = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        trigger.cancel();
    });
    let started = Instant::now();
    let err = run_export(session, &req, None, |_| {}, token)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
    assert!(files_in(dir.path()).is_empty());
}

/// A statement the drivers would drain to its end when the sink stops it (the string literal
/// `'delete'` makes `cancel_safe_on_stop` refuse it) is still aborted promptly on cancel.
async fn check_cancel_drained_statement(session: &dyn DbSession, slow_sql: &str) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slow.csv");
    let req = ExportRequest {
        export_id: "slow".into(),
        source: sql(slow_sql),
        format: csv(),
        path: path.to_string_lossy().into_owned(),
        max_rows: None,
    };
    let token = CancellationToken::new();
    let trigger = token.clone();
    let mut cancelled_at = None;
    let err = run_export(
        session,
        &req,
        None,
        |p| {
            if p.rows > 0 && cancelled_at.is_none() {
                cancelled_at = Some(Instant::now());
                trigger.cancel();
            }
        },
        token,
    )
    .await
    .unwrap_err();
    let after_cancel = cancelled_at.expect("progress before the end").elapsed();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
    assert!(
        after_cancel < Duration::from_secs(5),
        "returned {after_cancel:?} after the cancel"
    );
    assert!(files_in(dir.path()).is_empty());
    // The worker connection is usable afterwards.
    let again = export(
        session,
        sql("SELECT 1 AS one"),
        csv(),
        &dir.path().join("one.csv"),
    )
    .await
    .expect("export after cancel");
    assert_eq!(again.rows, 1);
}

/// Large blobs come out in full from a table export; a query export never writes them
/// truncated.
async fn check_large_blob(
    session: &dyn DbSession,
    qualified: &str,
    name: &str,
    schema: Option<&str>,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("blobs.json");
    let result = export(session, table(schema, name), json(), &path)
        .await
        .expect("blob export");
    assert_eq!(result.rows, 3);
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let rows = parsed.as_array().unwrap();
    let decode = |v: &serde_json::Value| {
        base64::engine::general_purpose::STANDARD
            .decode(v.as_str().unwrap())
            .unwrap()
    };
    assert_eq!(decode(&rows[0]["data"]), vec![0xAB; 100_000]);
    assert_eq!(decode(&rows[1]["data"]), vec![1, 2]);
    assert!(rows[2]["data"].is_null());

    let path = dir.path().join("blobs.sql");
    match export(
        session,
        sql(&format!("SELECT data FROM {qualified} ORDER BY id")),
        json(),
        &path,
    )
    .await
    {
        Ok(r) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(r.rows, 3);
            assert_eq!(decode(&parsed[0]["data"]).len(), 100_000);
        }
        Err(e) => {
            assert!(matches!(e, AppError::Unsupported(_)), "{e:?}");
            assert!(!path.exists());
        }
    }
}

// ---------------------------------------------------------------------------------------------
// PostgreSQL
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn pg_exports_events_big() {
    let Some(s) = open_pg().await else {
        return;
    };
    check_events_big(s.as_ref(), Some("public"), true).await;
}

#[tokio::test]
async fn pg_cancel_removes_the_temporary_file() {
    let Some(s) = open_pg().await else {
        return;
    };
    check_cancel_mid_export(s.as_ref(), Some("public")).await;
    check_cancel_running_statement(s.as_ref(), "SELECT pg_sleep(30)").await;
    check_cancel_drained_statement(
        s.as_ref(),
        "SELECT g, repeat('x', 100) AS pad, 'delete' AS tag, \
         CASE WHEN g % 100 = 0 THEN pg_sleep(0.01)::text END AS s \
         FROM generate_series(1, 100000) g",
    )
    .await;
}

#[tokio::test]
async fn pg_large_bytea_and_insert_export() {
    let Some(s) = open_pg().await else {
        return;
    };
    let name = scratch_name();
    let copy = format!("{name}_c");
    let victim = format!("{name}_v");
    let q = format!("public.\"{name}\"");
    // An identity GENERATED ALWAYS key (needs OVERRIDING SYSTEM VALUE) and a generated column
    // (left out of the INSERTs).
    let ddl = |table: &str| {
        format!(
            "CREATE TABLE public.\"{table}\" (id int GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \
             data bytea, note text, tail text, total int GENERATED ALWAYS AS (id * 2) STORED)"
        )
    };
    s.execute(&ddl(&name), &[])
        .await
        .expect("create scratch table");
    let body = async {
        // Row 3: a value ending in a backslash followed by attacker-controlled text, which a
        // plain '…' literal would turn into SQL under standard_conforming_strings = off.
        s.execute(
            &format!(
                "INSERT INTO {q} (id, data, note, tail) OVERRIDING SYSTEM VALUE VALUES \
                 (1, decode(repeat('ab', 100000), 'hex'), 'it''s', NULL), \
                 (2, '\\x0102'::bytea, E'a\\r\\nb', 'x'), \
                 (3, NULL, E'ends with \\\\', ', NULL); DROP TABLE public.\"{victim}\"; --')"
            ),
            &[],
        )
        .await
        .unwrap();
        check_large_blob(s.as_ref(), &q, &name, Some("public")).await;

        // The INSERT export loads into a table with the same DDL, in a session with
        // standard_conforming_strings off.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rows.sql");
        let r = export(
            s.as_ref(),
            table(Some("public"), &name),
            ExportFormat::SqlInsert {
                table_name: copy.clone(),
                batch_size: 2,
            },
            &path,
        )
        .await
        .unwrap();
        assert_eq!(r.rows, 3);
        let script = std::fs::read_to_string(&path).unwrap();
        assert_eq!(script.matches("INSERT INTO").count(), 2);
        assert_eq!(script.matches("OVERRIDING SYSTEM VALUE").count(), 2);
        assert!(!script.contains("\"total\""), "{}", &script[..200]);
        s.execute(&ddl(&copy), &[]).await.unwrap();
        s.execute(&format!("CREATE TABLE public.\"{victim}\" (x int)"), &[])
            .await
            .unwrap();
        let raw = pg_raw().await;
        raw.batch_execute("SET standard_conforming_strings = off; SET escape_string_warning = off")
            .await
            .unwrap();
        raw.batch_execute(&script).await.expect("load the script");
        let victim_left: i64 = raw
            .query_one(
                "SELECT count(*) FROM pg_tables WHERE schemaname = 'public' AND tablename = $1",
                &[&victim],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(victim_left, 1, "the data ran as SQL");
        let diff: i64 = raw
            .query_one(
                &format!(
                    "SELECT count(*) FROM ((SELECT id, data, note, tail FROM {q} \
                     EXCEPT SELECT id, data, note, tail FROM public.\"{copy}\") UNION ALL \
                     (SELECT id, data, note, tail FROM public.\"{copy}\" \
                     EXCEPT SELECT id, data, note, tail FROM {q})) d"
                ),
                &[],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(diff, 0);
    };
    let cleanup = async {
        for t in [&copy, &victim, &name] {
            let _ = s
                .execute(&format!("DROP TABLE IF EXISTS public.\"{t}\""), &[])
                .await;
        }
    };
    with_cleanup(body, cleanup).await;
}

/// NULL and '' (and a value spelled like the NULL marker) come back as they were when the CSV
/// is loaded with PostgreSQL's COPY, with the matching NULL option, quoteAll or not.
#[tokio::test]
async fn pg_csv_reloads_nulls_and_empty_strings_with_copy() {
    let Some(s) = open_pg().await else {
        return;
    };
    let name = scratch_name();
    let q = format!("public.\"{name}\"");
    let raw = pg_raw().await;
    raw.batch_execute(&format!(
        "CREATE TABLE {q} (id int NOT NULL, n int, t text NOT NULL, u text, m text)"
    ))
    .await
    .expect("create scratch table");
    let body = async {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rows.csv");
        let query = "SELECT 1::int AS id, NULL::int AS n, ''::text AS t, NULL::text AS u, \
                     '\\N'::text AS m \
                     UNION ALL SELECT 2, 7, 'NULL', '', 'x\"y' ORDER BY 1";
        for (quote_all, null_as) in [(false, ""), (true, ""), (true, "\\N"), (false, "NULL")] {
            raw.batch_execute(&format!("TRUNCATE {q}")).await.unwrap();
            export(s.as_ref(), sql(query), csv_with(quote_all, null_as), &path)
                .await
                .unwrap();
            let data = std::fs::read(&path).unwrap();
            let sink = raw
                .copy_in(&format!(
                    "COPY {q} FROM STDIN (FORMAT csv, HEADER true, NULL '{}')",
                    null_as.replace('\'', "''")
                ))
                .await
                .unwrap();
            futures::pin_mut!(sink);
            sink.send(std::io::Cursor::new(data)).await.unwrap();
            sink.finish()
                .await
                .unwrap_or_else(|e| panic!("COPY quoteAll={quote_all} NULL {null_as:?}: {e:?}"));
            let rows = raw
                .query(&format!("SELECT id, n, t, u, m FROM {q} ORDER BY id"), &[])
                .await
                .unwrap();
            let get = |r: usize| {
                let row = &rows[r];
                (
                    row.get::<_, i32>(0),
                    row.get::<_, Option<i32>>(1),
                    row.get::<_, String>(2),
                    row.get::<_, Option<String>>(3),
                    row.get::<_, Option<String>>(4),
                )
            };
            let case = format!("quoteAll={quote_all} NULL {null_as:?}");
            assert_eq!(
                get(0),
                (1, None, String::new(), None, Some("\\N".into())),
                "{case}"
            );
            assert_eq!(
                get(1),
                (
                    2,
                    Some(7),
                    "NULL".into(),
                    Some(String::new()),
                    Some("x\"y".into())
                ),
                "{case}"
            );
        }
    };
    let cleanup = async {
        let _ = raw
            .batch_execute(&format!("DROP TABLE IF EXISTS {q}"))
            .await;
    };
    with_cleanup(body, cleanup).await;
}

/// Array columns of a table export have typed elements, as in a query export.
#[tokio::test]
async fn pg_table_export_types_array_elements() {
    let Some(s) = open_pg().await else {
        return;
    };
    let name = scratch_name();
    let q = format!("public.\"{name}\"");
    s.execute(
        &format!(
            "CREATE TABLE {q} (id int PRIMARY KEY, flags boolean[], nums int[], big bigint[], \
             docs jsonb[], words text[])"
        ),
        &[],
    )
    .await
    .expect("create scratch table");
    let body = async {
        s.execute(
            &format!(
                "INSERT INTO {q} VALUES (1, '{{t,f,NULL}}', '{{1,2}}', '{{9007199254740993}}', \
                 ARRAY['{{\"a\": 1}}'::jsonb, NULL], '{{x,\"y z\"}}')"
            ),
            &[],
        )
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let read = |p: &Path| -> serde_json::Value {
            serde_json::from_str(std::fs::read_to_string(p).unwrap().trim()).unwrap()
        };
        let from_table = dir.path().join("table.ndjson");
        export(
            s.as_ref(),
            table(Some("public"), &name),
            ndjson(),
            &from_table,
        )
        .await
        .unwrap();
        let from_sql = dir.path().join("sql.ndjson");
        export(
            s.as_ref(),
            sql(&format!(
                "SELECT id, flags, nums, big, docs, words FROM {q}"
            )),
            ndjson(),
            &from_sql,
        )
        .await
        .unwrap();
        let (t, q_) = (read(&from_table), read(&from_sql));
        assert_eq!(t["flags"], serde_json::json!([true, false, null]));
        assert_eq!(t["nums"], serde_json::json!([1, 2]));
        assert_eq!(t["big"], serde_json::json!(["9007199254740993"]));
        assert_eq!(t["docs"], serde_json::json!([{"a": 1}, null]));
        assert_eq!(t["words"], serde_json::json!(["x", "y z"]));
        assert_eq!(t, q_, "table and query exports agree");
    };
    let cleanup = async {
        let _ = s.execute(&format!("DROP TABLE IF EXISTS {q}"), &[]).await;
    };
    with_cleanup(body, cleanup).await;
}

// ---------------------------------------------------------------------------------------------
// MySQL
// ---------------------------------------------------------------------------------------------

async fn mysql_db(s: &dyn DbSession) -> String {
    s.server_info()
        .await
        .expect("server_info")
        .current_database
        .expect("a default database in TABLER_TEST_MYSQL_URL")
}

#[tokio::test]
async fn mysql_exports_events_big() {
    let Some(s) = open_mysql().await else {
        return;
    };
    let db = mysql_db(s.as_ref()).await;
    check_events_big(s.as_ref(), Some(&db), true).await;
}

#[tokio::test]
async fn mysql_cancel_removes_the_temporary_file() {
    let Some(s) = open_mysql().await else {
        return;
    };
    let db = mysql_db(s.as_ref()).await;
    check_cancel_mid_export(s.as_ref(), Some(&db)).await;
    check_cancel_running_statement(s.as_ref(), "SELECT SLEEP(30)").await;
    check_cancel_drained_statement(
        s.as_ref(),
        "WITH RECURSIVE d(n) AS (SELECT 0 UNION ALL SELECT n + 1 FROM d WHERE n < 99) \
         SELECT a.n * 10000 + b.n * 100 + c.n AS n, REPEAT('x', 100) AS pad, 'delete' AS tag, \
         IF(c.n = 0, SLEEP(0.01), 0) AS s FROM d a, d b, d c LIMIT 100000",
    )
    .await;
}

#[tokio::test]
async fn mysql_large_blob_and_insert_export() {
    let Some(s) = open_mysql().await else {
        return;
    };
    let db = mysql_db(s.as_ref()).await;
    let name = scratch_name();
    let copy = format!("{name}_c");
    let victim = format!("{name}_v");
    let q = format!("`{db}`.`{name}`");
    s.execute(
        &format!(
            "CREATE TABLE {q} (id int PRIMARY KEY, data longblob, note text, flags bit(5), doc json)"
        ),
        &[],
    )
    .await
    .expect("create scratch table");
    let body = async {
        // Row 3 holds SQL that a backslash-escaped literal (`'\'); DROP …`) would run when the
        // script is loaded under NO_BACKSLASH_ESCAPES.
        s.execute(
            &format!(
                "INSERT INTO {q} VALUES \
                 (1, UNHEX(REPEAT('AB', 100000)), 'it''s \\\\ ok', b'10101', \
                  '{{\"p\": \"a\\\\\\\\b\"}}'), \
                 (2, X'0102', 'a\\r\\nb', b'0', '[1, \"x\"]'), \
                 (3, NULL, '''); DROP TABLE `{db}`.`{victim}`; -- ', NULL, NULL)"
            ),
            &[],
        )
        .await
        .unwrap();
        check_large_blob(s.as_ref(), &q, &name, Some(&db)).await;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rows.sql");
        let r = export(
            s.as_ref(),
            table(Some(&db), &name),
            ExportFormat::SqlInsert {
                table_name: copy.clone(),
                batch_size: 2,
            },
            &path,
        )
        .await
        .unwrap();
        assert_eq!(r.rows, 3);
        let script = std::fs::read_to_string(&path).unwrap();
        assert!(
            !script.contains('\\'),
            "no backslash escapes: {script:.300}"
        );
        s.execute(&format!("CREATE TABLE `{db}`.`{copy}` LIKE {q}"), &[])
            .await
            .unwrap();
        s.execute(&format!("CREATE TABLE `{db}`.`{victim}` (x int)"), &[])
            .await
            .unwrap();
        // Load the script as `mysql < rows.sql` would, in a session with NO_BACKSLASH_ESCAPES
        // (the script names the copy unqualified: it runs in the test database).
        let mut raw = mysql_raw().await;
        raw.query_drop("SET SESSION sql_mode = CONCAT(@@sql_mode, ',NO_BACKSLASH_ESCAPES')")
            .await
            .unwrap();
        raw.query_drop(&script).await.expect("load the script");
        let victim_left: Option<i64> = raw
            .exec_first(
                "SELECT count(*) FROM information_schema.TABLES \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
                (db.clone(), victim.clone()),
            )
            .await
            .unwrap();
        assert_eq!(victim_left, Some(1), "the data ran as SQL");
        let same: Option<i64> = raw
            .query_first(format!(
                "SELECT count(*) FROM {q} a JOIN `{db}`.`{copy}` b ON a.id = b.id \
                 AND a.data <=> b.data AND a.note <=> b.note AND a.flags <=> b.flags \
                 AND a.doc <=> b.doc"
            ))
            .await
            .unwrap();
        assert_eq!(same, Some(3));
        let _ = raw.disconnect().await;
    };
    let cleanup = async {
        for t in [&copy, &victim, &name] {
            let _ = s
                .execute(&format!("DROP TABLE IF EXISTS `{db}`.`{t}`"), &[])
                .await;
        }
    };
    with_cleanup(body, cleanup).await;
}

/// A FLOAT column exports the same text whether the table export has a filter (bound
/// parameters: binary protocol, f32 values) or not (text protocol).
#[tokio::test]
async fn mysql_float_columns_export_the_same_with_and_without_a_filter() {
    let Some(s) = open_mysql().await else {
        return;
    };
    let db = mysql_db(s.as_ref()).await;
    let name = scratch_name();
    let q = format!("`{db}`.`{name}`");
    s.execute(
        &format!("CREATE TABLE {q} (id int PRIMARY KEY, f float, d double)"),
        &[],
    )
    .await
    .expect("create scratch table");
    let body = async {
        s.execute(
            &format!("INSERT INTO {q} VALUES (1, 1.1, 1.1), (2, 0.3, 0.3), (3, NULL, -2.5)"),
            &[],
        )
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let source = |filter: Option<FilterGroup>| ExportSource::Table {
            table: TableRef::new(Some(&db), &name),
            filter,
            sort: vec![],
            columns: None,
        };
        let id_ge_1 = FilterGroup {
            conjunction: Conjunction::And,
            items: vec![FilterNode::Condition {
                column: "id".into(),
                op: FilterOp::Ge,
                values: vec![Value::Int(1)],
            }],
        };
        for (format, ext) in [(csv(), "csv"), (ndjson(), "ndjson")] {
            let plain = dir.path().join(format!("plain.{ext}"));
            let with_filter = dir.path().join(format!("filtered.{ext}"));
            export(s.as_ref(), source(None), format.clone(), &plain)
                .await
                .unwrap();
            export(
                s.as_ref(),
                source(Some(id_ge_1.clone())),
                format,
                &with_filter,
            )
            .await
            .unwrap();
            let plain = std::fs::read_to_string(&plain).unwrap();
            assert_eq!(plain, std::fs::read_to_string(&with_filter).unwrap());
            assert!(
                plain.contains("1.1") && !plain.contains("1.100000"),
                "{plain}"
            );
        }
    };
    let cleanup = async {
        let _ = s.execute(&format!("DROP TABLE IF EXISTS {q}"), &[]).await;
    };
    with_cleanup(body, cleanup).await;
}

// ---------------------------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------------------------

/// Every fixture table goes through an INSERT export into a fresh database with the same
/// schema and comes back identical.
#[tokio::test]
async fn sqlite_insert_export_round_trips_fixture_tables() {
    let Some((s, dir, copy)) = open_sqlite().await else {
        return;
    };
    let tables = [
        "users",
        "posts",
        "tags",
        "post_tags",
        "audit_log",
        "events_big",
        "types_zoo",
        "types_zoo_affinity",
        "Order Items",
        "CamelCase",
    ];
    let fresh_path = dir.path().join("fresh.sqlite");
    let fresh = rusqlite::Connection::open(&fresh_path).unwrap();
    let src =
        rusqlite::Connection::open_with_flags(&copy, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    for name in tables {
        let ddl: String = src
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [name],
                |r| r.get(0),
            )
            .unwrap();
        fresh.execute_batch(&ddl).unwrap();

        let path = dir.path().join(format!("{name}.sql"));
        let result = export(
            s.as_ref(),
            table(Some("main"), name),
            ExportFormat::SqlInsert {
                table_name: String::new(),
                batch_size: 500,
            },
            &path,
        )
        .await
        .unwrap_or_else(|e| panic!("export {name}: {e:?}"));
        let expected: i64 = src
            .query_row(&format!("SELECT count(*) FROM \"{name}\""), [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(result.rows, expected as u64, "{name}");

        let script = std::fs::read_to_string(&path).unwrap();
        fresh
            .execute_batch(&script)
            .unwrap_or_else(|e| panic!("load {name}: {e}"));
        let loaded: i64 = fresh
            .query_row(&format!("SELECT count(*) FROM \"{name}\""), [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(loaded, expected, "{name}");
    }
    drop(src);

    // Same values with the same storage classes, row for row (EXCEPT alone treats 5 and 5.0
    // as equal, so every column's typeof() is compared too).
    fresh
        .execute("ATTACH DATABASE ?1 AS src", [copy.to_string_lossy()])
        .unwrap();
    for name in tables {
        let columns: Vec<String> = fresh
            .prepare(&format!("SELECT name FROM pragma_table_info('{name}')"))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|c| c.unwrap())
            .collect();
        let kinds = columns
            .iter()
            .map(|c| format!("typeof(\"{}\")", c.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(", ");
        for (a, b) in [("src", "main"), ("main", "src")] {
            let diff: i64 = fresh
                .query_row(
                    &format!(
                        "SELECT count(*) FROM (SELECT *, {kinds} FROM {a}.\"{name}\" EXCEPT \
                         SELECT *, {kinds} FROM {b}.\"{name}\")"
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(diff, 0, "{name}: rows of {a} missing from {b}");
        }
    }

    // Integral REAL values stay REAL in columns without type affinity.
    let name = scratch_name();
    let ddl = format!("CREATE TABLE \"{name}\" (id INTEGER PRIMARY KEY, v, w BLOB, n NUMERIC)");
    s.execute(&ddl, &[]).await.unwrap();
    s.execute(
        &format!(
            "INSERT INTO \"{name}\" VALUES (1, 5.0, 5.0, 5.0), (2, 2.5, 2.5, 2.5), \
             (3, 1e20, 1e20, 1e20), (4, -0.0, 3.0, 7.0)"
        ),
        &[],
    )
    .await
    .unwrap();
    let path = dir.path().join("reals.sql");
    export(
        s.as_ref(),
        table(Some("main"), &name),
        ExportFormat::SqlInsert {
            table_name: String::new(),
            batch_size: 10,
        },
        &path,
    )
    .await
    .unwrap();
    s.execute(&format!("DROP TABLE \"{name}\""), &[])
        .await
        .unwrap();
    fresh.execute_batch(&ddl).unwrap();
    fresh
        .execute_batch(&std::fs::read_to_string(&path).unwrap())
        .unwrap();
    let kinds: String = fresh
        .query_row(
            &format!(
                "SELECT group_concat(typeof(v) || '/' || typeof(w) || '/' || typeof(n), ' ') \
                 FROM (SELECT * FROM \"{name}\" ORDER BY id)"
            ),
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        kinds,
        "real/real/integer real/real/real real/real/real real/real/integer"
    );
}

#[tokio::test]
async fn sqlite_exports_events_big_and_large_blobs() {
    let Some((s, _dir, _copy)) = open_sqlite().await else {
        return;
    };
    check_events_big(s.as_ref(), Some("main"), false).await;
    check_cancel_mid_export(s.as_ref(), Some("main")).await;

    let name = scratch_name();
    let q = format!("main.\"{name}\"");
    s.execute(
        &format!("CREATE TABLE {q} (id INTEGER PRIMARY KEY, data BLOB, note TEXT)"),
        &[],
    )
    .await
    .unwrap();
    let full = |bytes: &[u8]| {
        Value::Bytes(BytesValue {
            base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            len: bytes.len() as u64,
            truncated: false,
        })
    };
    s.execute(
        &format!("INSERT INTO {q} VALUES (1, ?1, 'x'), (2, ?2, NULL), (3, NULL, NULL)"),
        &[full(&[0xAB; 100_000]), full(&[1, 2])],
    )
    .await
    .unwrap();
    check_large_blob(s.as_ref(), &q, &name, Some("main")).await;
    s.execute(&format!("DROP TABLE {q}"), &[]).await.unwrap();

    // Any SQLite column can hold a blob: one without a declared type, and one declared TEXT.
    let name = scratch_name();
    let q = format!("main.\"{name}\"");
    s.execute(
        &format!("CREATE TABLE {q} (id INTEGER PRIMARY KEY, name TEXT, data, t TEXT)"),
        &[],
    )
    .await
    .unwrap();
    let big: Vec<u8> = (0..100_000u32).map(|i| (i % 253) as u8).collect();
    s.execute(
        &format!(
            "INSERT INTO {q} VALUES (1, 'a', ?1, ?1), (2, 'b', 'text', ?2), (3, NULL, NULL, NULL)"
        ),
        &[full(&big), full(&[7, 8])],
    )
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("untyped.json");
    let result = export(s.as_ref(), table(Some("main"), &name), json(), &path).await;
    s.execute(&format!("DROP TABLE {q}"), &[]).await.unwrap();
    assert_eq!(result.expect("untyped blob export").rows, 3);
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let decode = |v: &serde_json::Value| {
        base64::engine::general_purpose::STANDARD
            .decode(v.as_str().unwrap())
            .unwrap()
    };
    assert_eq!(decode(&parsed[0]["data"]), big);
    assert_eq!(decode(&parsed[0]["t"]), big);
    assert_eq!(parsed[1]["data"], "text");
    assert_eq!(decode(&parsed[1]["t"]), vec![7, 8]);
    assert!(parsed[2]["data"].is_null());
}

/// A query export refuses statements without rows before running them: a `BEGIN` would stay
/// open on the worker connection and break every later grid commit.
#[tokio::test]
async fn sqlite_query_export_never_runs_transaction_control() {
    let Some((s, dir, copy)) = open_sqlite().await else {
        return;
    };
    for statement in [
        "BEGIN",
        "BEGIN IMMEDIATE",
        "PRAGMA foreign_keys = OFF",
        "DELETE FROM tags",
    ] {
        let err = export(s.as_ref(), sql(statement), csv(), &dir.path().join("x.csv"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::InvalidInput(_)),
            "{statement}: {err:?}"
        );
    }
    // Worker statements still commit at once: another connection sees the new table.
    let name = scratch_name();
    s.execute(&format!("CREATE TABLE \"{name}\" (x)"), &[])
        .await
        .unwrap();
    let other = rusqlite::Connection::open(&copy).unwrap();
    let (seen, tags): (i64, i64) = other
        .query_row(
            &format!(
                "SELECT (SELECT count(*) FROM sqlite_master WHERE name = '{name}'), \
                 (SELECT count(*) FROM tags)"
            ),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(seen, 1);
    assert!(tags > 0, "the DELETE did not run");
    drop(other);
    s.execute(&format!("DROP TABLE \"{name}\""), &[])
        .await
        .unwrap();
}
