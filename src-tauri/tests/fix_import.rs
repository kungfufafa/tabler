//! Import fixes against real servers: parameter casts that no longer cut values (PostgreSQL),
//! sequences after imported ids, MySQL value-change warnings as row errors, UPDATE ON DUPLICATE
//! with repeated keys, SQL dumps that end inside their transaction, `\connect`, and mysqldump's
//! raw `_binary` bytes. Gated on `TABLER_TEST_PG_URL` / `TABLER_TEST_MYSQL_URL`
//! (`scheme://user[:pass]@host[:port]/db`); every object is a `fix_import_*` table, dropped at
//! the end.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use tabler_lib::drivers::{CancelSlot, ConnectTarget, DbSession, connect};
use tabler_lib::import::{run_csv_import, run_sql_import};
use tabler_lib::sqlgen::Dialect;
use tabler_lib::sqlgen::mutate::{TableSpec, build_update};
use tabler_lib::sqlgen::select::build_count;
use tabler_lib::types::browse::{Conjunction, FilterGroup, FilterNode, FilterOp, RowKey};
use tabler_lib::types::import::{
    CsvFormat, CsvImportReport, CsvImportRequest, ImportColumnDef, ImportTarget, ImportType,
    InsertMode, SqlImportReport, SqlImportRequest, TextEncoding,
};
use tabler_lib::types::profile::ConnectionProfile;
use tabler_lib::types::schema::{TableMeta, TableRef};
use tabler_lib::types::value::Value;

/// A session for `scheme://user[:pass]@host[:port]/db` in the environment variable `env`.
async fn open(env: &str, driver: &str, scheme: &str) -> Option<Arc<dyn DbSession>> {
    let Ok(url) = std::env::var(env) else {
        println!("skipped: {env} unset");
        return None;
    };
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let rest = url.strip_prefix(scheme).expect("URL scheme");
    let (auth, hostdb) = rest
        .rsplit_once('@')
        .map_or((None, rest), |(a, h)| (Some(a), h));
    let (user, password) = match auth.map(|a| a.split_once(':').unwrap_or((a, ""))) {
        Some((u, p)) => (
            Some(u.to_string()),
            Some(p.to_string()).filter(|p| !p.is_empty()),
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
        "name": "fix import",
        "driver": driver,
        "host": host,
        "port": port,
        "user": user,
        "database": db.split('?').next().filter(|d| !d.is_empty()),
        "ssl": "disable",
    }))
    .expect("profile");
    let session = connect(ConnectTarget {
        profile: &profile,
        password: password.as_deref(),
        host_override: None,
    })
    .await
    .expect("connect");
    Some(session)
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> String {
    let path = dir.join(name);
    std::fs::File::create(&path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
    path.to_string_lossy().into_owned()
}

fn csv_request(
    path: &str,
    target: ImportTarget,
    mapping: &[&str],
    mode: InsertMode,
) -> CsvImportRequest {
    CsvImportRequest {
        import_id: "fix".into(),
        path: path.into(),
        format: CsvFormat {
            delimiter: Some(",".into()),
            quote: "\"".into(),
            encoding: Some(TextEncoding::Utf8),
            header: true,
        },
        target,
        mapping: mapping.iter().map(|m| Some(m.to_string())).collect(),
        empty_as_null: true,
        mode,
    }
}

async fn import_csv(
    s: &dyn DbSession,
    dir: &Path,
    text: &str,
    meta: &TableMeta,
    mapping: &[&str],
    mode: InsertMode,
) -> CsvImportReport {
    let path = write(dir, "data.csv", text.as_bytes());
    let req = csv_request(
        &path,
        ImportTarget::Existing {
            table: meta.table.clone(),
        },
        mapping,
        mode,
    );
    run_csv_import(s, &req, Some(meta), |_| {}, CancellationToken::new())
        .await
        .expect("import")
}

async fn import_dump(
    s: &dyn DbSession,
    dir: &Path,
    dump: &[u8],
    stop_on_error: bool,
) -> SqlImportReport {
    let path = write(dir, "dump.sql", dump);
    let req = SqlImportRequest {
        import_id: "fix-dump".into(),
        path,
        stop_on_error,
    };
    run_sql_import(
        s,
        &req,
        CancelSlot::default(),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .expect("dump")
}

async fn scalar(s: &dyn DbSession, sql: &str) -> Value {
    s.query(sql, &[]).await.expect(sql).rows.remove(0).remove(0)
}

fn sqlstate(e: &tabler_lib::error::AppError) -> Option<&str> {
    match e {
        tabler_lib::error::AppError::Db { sqlstate, .. } => sqlstate.as_deref(),
        _ => None,
    }
}

#[tokio::test]
async fn postgres_import_fixes() {
    let Some(s) = open("TABLER_TEST_PG_URL", "postgres", "postgres://").await else {
        return;
    };
    let name = format!("fix_import_{}", uuid::Uuid::new_v4().simple());
    let table = TableRef::new(Some("public"), &name);
    let q = format!("public.\"{name}\"");
    s.execute(
        &format!("CREATE TABLE {q} (id serial PRIMARY KEY, code varchar(5), flag char(3), amount numeric(10,2))"),
        &[],
    )
    .await
    .unwrap();
    let result = std::panic::AssertUnwindSafe(postgres_steps(s.as_ref(), &table, &q));
    let outcome = futures::FutureExt::catch_unwind(result).await;
    s.execute(&format!("DROP TABLE IF EXISTS {q}"), &[])
        .await
        .unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn postgres_steps(s: &dyn DbSession, table: &TableRef, q: &str) {
    let dir = tempfile::tempdir().unwrap();
    let meta = s.table_meta(table).await.unwrap();

    // CSV with its own ids: the sequence continues after them, and never moves back.
    let report = import_csv(
        s,
        dir.path(),
        "id,code\n10,a\n20,b\n",
        &meta,
        &["id", "code"],
        InsertMode::StopOnError,
    )
    .await;
    assert_eq!(
        (report.rows_imported, report.failure.clone()),
        (2, None),
        "{report:?}"
    );
    let next = format!("INSERT INTO {q} (code) VALUES ('n') RETURNING id::text");
    assert_eq!(scalar(s, &next).await, Value::Text("21".into()));
    let report = import_csv(
        s,
        dir.path(),
        "id,code\n5,z\n",
        &meta,
        &["id", "code"],
        InsertMode::StopOnError,
    )
    .await;
    assert_eq!(report.rows_imported, 1);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    assert_eq!(scalar(s, &next).await, Value::Text("22".into()));

    // An over-long value fails (22001) instead of being cut to the column's size.
    let report = import_csv(
        s,
        dir.path(),
        "id,code\n30,abcdefghij\n",
        &meta,
        &["id", "code"],
        InsertMode::StopOnError,
    )
    .await;
    let failure = report.failure.expect("value too long");
    assert!(failure.message.contains("too long"), "{failure:?}");
    let report = import_csv(
        s,
        dir.path(),
        "id,flag\n31,abcd\n",
        &meta,
        &["id", "flag"],
        InsertMode::StopOnError,
    )
    .await;
    assert!(
        report
            .failure
            .expect("char(3)")
            .message
            .contains("too long")
    );

    // A grid edit too.
    let spec = TableSpec {
        dialect: Dialect::Postgres,
        table,
        columns: &meta.columns,
    };
    let key = RowKey::Pk(BTreeMap::from([("id".to_string(), Value::Int(10))]));
    let edit = build_update(
        &spec,
        &key,
        &BTreeMap::from([("code".to_string(), Value::Text("abcdefghij".into()))]),
        false,
    )
    .unwrap();
    let err = s.execute(&edit.sql, &edit.params).await.unwrap_err();
    assert_eq!(sqlstate(&err), Some("22001"), "{err}");
    let edit = build_update(
        &spec,
        &key,
        &BTreeMap::from([("amount".to_string(), Value::Text("12345678901.5".into()))]),
        false,
    )
    .unwrap();
    assert_eq!(
        sqlstate(&s.execute(&edit.sql, &edit.params).await.unwrap_err()),
        Some("22003")
    );

    // A filter compares the value as typed, not cut to varchar(5).
    let count = |value: &str| {
        let filter = FilterGroup {
            conjunction: Conjunction::And,
            items: vec![FilterNode::Condition {
                column: "code".into(),
                op: FilterOp::Eq,
                values: vec![Value::Text(value.into())],
            }],
        };
        build_count(Dialect::Postgres, table, &meta.columns, Some(&filter)).unwrap()
    };
    s.execute(&format!("UPDATE {q} SET code = 'abcde' WHERE id = 10"), &[])
        .await
        .unwrap();
    for (value, n) in [("abcdeZZZZ", 0), ("abcde", 1)] {
        let built = count(value);
        let rows = s.query(&built.sql, &built.params).await.unwrap().rows;
        assert_eq!(rows[0][0], Value::Int(n), "{value}");
    }

    // UPDATE ON DUPLICATE with a key twice in one batch: the last row wins.
    let report = import_csv(
        s,
        dir.path(),
        "id,code\n10,first\n40,x\n10,last\n",
        &meta,
        &["id", "code"],
        InsertMode::UpdateOnDuplicate,
    )
    .await;
    assert_eq!(report.failure, None);
    assert_eq!(
        scalar(s, &format!("SELECT code FROM {q} WHERE id = 10")).await,
        Value::Text("last".into())
    );

    // A dump whose transaction fails: PostgreSQL ignores the rest and its COMMIT rolls back.
    let dump = format!(
        "BEGIN;\nINSERT INTO {q} (id, code) VALUES (100, 'a');\nINSERT INTO fix_import_nope VALUES (1);\nINSERT INTO {q} (id, code) VALUES (101, 'b');\nCOMMIT;\n"
    );
    let report = import_dump(s, dir.path(), dump.as_bytes(), false).await;
    assert_eq!((report.executed, report.failed), (3, 2), "{report:?}");
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.rolled_back, Some(1));
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("The COMMIT on line 5 rolled the transaction back")),
        "{:?}",
        report.notes
    );
    assert!(report.notes.iter().any(|n| n.starts_with("1 statement failed because the transaction had already failed")), "{:?}", report.notes);
    assert_eq!(s.in_transaction(), Some(false));
    assert_eq!(
        scalar(
            s,
            &format!("SELECT count(*)::text FROM {q} WHERE id >= 100")
        )
        .await,
        Value::Text("0".into())
    );
    // Stopped inside it: rolled back, and the session is out of the transaction.
    let report = import_dump(s, dir.path(), dump.as_bytes(), true).await;
    assert!(report.stopped);
    assert_eq!(report.rolled_back, Some(1));
    assert_eq!(s.in_transaction(), Some(false));

    // `\connect` to the same database is skipped; to another one stops the import.
    let db = match scalar(s, "SELECT current_database()::text").await {
        Value::Text(db) => db,
        other => panic!("{other:?}"),
    };
    let dump = format!(
        "\\connect {db}\nINSERT INTO {q} (id, code) VALUES (200, 'a');\n\\connect fix_import_elsewhere\nINSERT INTO {q} (id, code) VALUES (201, 'b');\n"
    );
    let report = import_dump(s, dir.path(), dump.as_bytes(), false).await;
    assert!(report.stopped);
    assert_eq!((report.executed, report.failed, report.skipped), (1, 1, 1));
    assert!(
        report.errors[0].message.contains("fix_import_elsewhere"),
        "{:?}",
        report.errors
    );
    assert_eq!(report.errors[0].line, 3);
}

#[tokio::test]
async fn mysql_import_fixes() {
    let Some(s) = open("TABLER_TEST_MYSQL_URL", "mysql", "mysql://").await else {
        return;
    };
    let name = format!("fix_import_{}", uuid::Uuid::new_v4().simple());
    let created = format!("fix_import_new_{}", uuid::Uuid::new_v4().simple());
    let schema = match scalar(s.as_ref(), "SELECT DATABASE()").await {
        Value::Text(db) => db,
        other => panic!("{other:?}"),
    };
    let table = TableRef::new(Some(&schema), &name);
    let q = format!("`{name}`");
    s.execute(
        &format!("CREATE TABLE {q} (id INT AUTO_INCREMENT PRIMARY KEY, d DECIMAL(10,2), day DATE, b BLOB)"),
        &[],
    )
    .await
    .unwrap();
    let result = std::panic::AssertUnwindSafe(mysql_steps(s.as_ref(), &table, &q, &created));
    let outcome = futures::FutureExt::catch_unwind(result).await;
    s.execute(&format!("DROP TABLE IF EXISTS {q}"), &[])
        .await
        .unwrap();
    s.execute(&format!("DROP TABLE IF EXISTS `{created}`"), &[])
        .await
        .unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn mysql_steps(s: &dyn DbSession, table: &TableRef, q: &str, created: &str) {
    let dir = tempfile::tempdir().unwrap();
    let meta = s.table_meta(table).await.unwrap();

    // A DECIMAL rounded to its scale is a note only: the import refuses it, naming the row.
    let report = import_csv(
        s,
        dir.path(),
        "id,d\n1,1.5\n2,1.239\n3,2\n",
        &meta,
        &["id", "d"],
        InsertMode::StopOnError,
    )
    .await;
    let failure = report.failure.expect("rounding is an error");
    assert_eq!(
        failure.message,
        "Data truncated for column 'd': MySQL would store a changed value"
    );
    assert_eq!(
        (failure.first_row, failure.last_row, failure.line),
        (Some(2), Some(2), Some(3))
    );
    assert_eq!(
        scalar(s, &format!("SELECT COUNT(*) FROM {q}")).await,
        Value::Int(0)
    );
    // A date that would lose its time.
    let report = import_csv(
        s,
        dir.path(),
        "id,day\n1,2026-01-02\n2,2026-01-02 10:20:30\n",
        &meta,
        &["id", "day"],
        InsertMode::StopOnError,
    )
    .await;
    let failure = report.failure.expect("time dropped");
    assert!(
        failure.message.contains("Incorrect date value"),
        "{failure:?}"
    );
    assert_eq!(failure.first_row, Some(2), "{failure:?}");

    // Values that fit go in; ON DUPLICATE KEY UPDATE's deprecation warning is no error.
    let report = import_csv(
        s,
        dir.path(),
        "id,d\n1,1.5\n50,2\n",
        &meta,
        &["id", "d"],
        InsertMode::StopOnError,
    )
    .await;
    assert_eq!((report.rows_imported, report.failure), (2, None));
    let report = import_csv(
        s,
        dir.path(),
        "id,d\n1,2.25\n1,3.5\n",
        &meta,
        &["id", "d"],
        InsertMode::UpdateOnDuplicate,
    )
    .await;
    assert_eq!(report.failure, None, "{report:?}");
    assert_eq!(
        scalar(s, &format!("SELECT CAST(d AS CHAR) FROM {q} WHERE id = 1")).await,
        Value::Text("3.50".into())
    );
    // AUTO_INCREMENT moves past imported ids by itself.
    s.execute(&format!("INSERT INTO {q} (d) VALUES (1)"), &[])
        .await
        .unwrap();
    assert_eq!(
        scalar(s, &format!("SELECT MAX(id) FROM {q}")).await,
        Value::Int(51)
    );

    // A new table with offsets: DATETIME holding UTC, said so.
    let path = write(
        dir.path(),
        "tz.csv",
        b"at\n2026-01-02 03:20:30+07:00\n2026-01-02T10:00:00Z\n",
    );
    let mut req = csv_request(
        &path,
        ImportTarget::Create {
            table: TableRef::new(None, created),
            columns: vec![ImportColumnDef {
                name: "at".into(),
                ty: ImportType::TimestampTz,
                precision: None,
                scale: None,
            }],
        },
        &["at"],
        InsertMode::StopOnError,
    );
    req.import_id = "fix-tz".into();
    let report = run_csv_import(s, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!((report.rows_imported, report.failure), (2, None));
    assert!(
        report.notes[0].starts_with("2 times with a UTC offset were converted to UTC"),
        "{:?}",
        report.notes
    );
    assert_eq!(
        scalar(
            s,
            &format!("SELECT GROUP_CONCAT(CAST(at AS CHAR) ORDER BY at) FROM `{created}`")
        )
        .await,
        Value::Text("2026-01-01 20:20:30,2026-01-02 10:00:00".into())
    );

    // mysqldump without --hex-blob: raw bytes keep their values; text after them stays UTF-8.
    let mut dump = Vec::new();
    dump.extend_from_slice(b"SET NAMES utf8mb4;\nINSERT INTO ");
    dump.extend_from_slice(q.as_bytes());
    dump.extend_from_slice(b" (id, b) VALUES (7, _binary '\xFF\\0\\'\xC3\xA9\x80');\n");
    let report = import_dump(s, dir.path(), &dump, true).await;
    assert_eq!((report.executed, report.failed), (2, 0), "{report:?}");
    assert_eq!(report.invalid_utf8, 2);
    assert_eq!(
        scalar(s, &format!("SELECT HEX(b) FROM {q} WHERE id = 7")).await,
        Value::Text("FF0027C3A980".into())
    );
    // Stopped inside the dump's transaction: rolled back, the editor out of it.
    let dump = format!(
        "START TRANSACTION;\nINSERT INTO {q} (id) VALUES (300);\nINSERT INTO fix_import_nope VALUES (1);\nCOMMIT;\n"
    );
    let report = import_dump(s, dir.path(), dump.as_bytes(), true).await;
    assert_eq!(report.rolled_back, Some(1), "{report:?}");
    assert_eq!(s.in_transaction(), Some(false));
    assert_eq!(
        scalar(s, &format!("SELECT COUNT(*) FROM {q} WHERE id = 300")).await,
        Value::Int(0)
    );
}
