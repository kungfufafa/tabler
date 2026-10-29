//! CSV import and SQL dump import against a real SQLite session (a temporary file).

use std::io::Write;
use std::path::Path;

use tokio_util::sync::CancellationToken;

use super::csvimport::{import_with_chunks, run_csv_import};
use super::dump::run_sql_import;
use crate::drivers::sqlite::LiteSession;
use crate::drivers::{CancelSlot, DbSession};
use crate::types::import::{
    CsvFormat, CsvImportRequest, ImportColumnDef, ImportTarget, ImportType, InsertMode,
    SqlImportRequest, TextEncoding,
};
use crate::types::profile::ConnectionProfile;
use crate::types::schema::TableRef;
use crate::types::value::Value;

async fn session(dir: &Path) -> LiteSession {
    // Only the fields a SQLite session needs; the rest default (robust to new profile fields).
    let profile: ConnectionProfile = serde_json::from_value(serde_json::json!({
        "id": uuid::Uuid::new_v4(),
        "name": "import test",
        "driver": "sqlite",
        "sqlitePath": dir.join("db.sqlite").to_string_lossy(),
    }))
    .unwrap();
    LiteSession::open(&profile).await.unwrap()
}

fn write(dir: &Path, name: &str, text: &[u8]) -> String {
    let path = dir.join(name);
    std::fs::File::create(&path)
        .unwrap()
        .write_all(text)
        .unwrap();
    path.to_string_lossy().into_owned()
}

async fn rows(s: &LiteSession, sql: &str) -> Vec<Vec<Value>> {
    s.query(sql, &[]).await.unwrap().rows
}

async fn count(s: &LiteSession, table: &str) -> i64 {
    match rows(s, &format!("SELECT count(*) FROM {table}")).await[0][0] {
        Value::Int(n) => n,
        ref other => panic!("count is {other:?}"),
    }
}

fn request(
    path: &str,
    table: &str,
    mapping: &[Option<&str>],
    mode: InsertMode,
) -> CsvImportRequest {
    CsvImportRequest {
        import_id: "csv".into(),
        path: path.into(),
        format: CsvFormat {
            delimiter: Some(",".into()),
            quote: "\"".into(),
            encoding: Some(TextEncoding::Utf8),
            header: true,
        },
        target: ImportTarget::Existing {
            table: TableRef::new(Some("main"), table),
        },
        mapping: mapping.iter().map(|m| m.map(str::to_string)).collect(),
        empty_as_null: true,
        mode,
    }
}

async fn people(s: &LiteSession) -> crate::types::schema::TableMeta {
    s.execute(
        "CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER, active BOOLEAN)",
        &[],
    )
    .await
    .unwrap();
    s.table_meta(&TableRef::new(Some("main"), "people"))
        .await
        .unwrap()
}

const PEOPLE_MAP: [Option<&str>; 4] = [Some("id"), Some("name"), Some("age"), Some("active")];

#[tokio::test]
async fn imports_into_an_existing_table() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path()).await;
    let meta = people(&s).await;
    let csv = write(
        dir.path(),
        "p.csv",
        b"ID,Name,age,active\n1,Ann,30,yes\n2,\"Bo, \"\"B\"\"\",,0\n3,\"\",41,true\n",
    );
    let mut seen = Vec::new();
    let report = run_csv_import(
        &s,
        &request(&csv, "people", &PEOPLE_MAP, InsertMode::StopOnError),
        Some(&meta),
        |p| seen.push(p),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.failure, None);
    assert_eq!((report.rows_imported, report.rows_read), (3, 3));
    assert!(report.single_transaction);
    assert!(!seen.is_empty());
    assert_eq!(seen.last().unwrap().rows_imported, 3);
    let got = rows(&s, "SELECT id, name, age, active FROM people ORDER BY id").await;
    assert_eq!(
        got,
        vec![
            vec![
                Value::Int(1),
                Value::Text("Ann".into()),
                Value::Int(30),
                Value::Bool(true)
            ],
            // The empty unquoted age is NULL; `0` a boolean false.
            vec![
                Value::Int(2),
                Value::Text("Bo, \"B\"".into()),
                Value::Null,
                Value::Bool(false)
            ],
            // A quoted "" stays an empty string (NOT NULL holds).
            vec![
                Value::Int(3),
                Value::Text(String::new()),
                Value::Int(41),
                Value::Bool(true)
            ],
        ]
    );

    // UPDATE ON DUPLICATE updates by primary key; a column left out keeps its value.
    let csv = write(
        dir.path(),
        "u.csv",
        b"id,name,x\n2,Bob,ignored\n4,Dee,ignored\n",
    );
    let report = run_csv_import(
        &s,
        &request(
            &csv,
            "people",
            &[Some("id"), Some("name"), None],
            InsertMode::UpdateOnDuplicate,
        ),
        Some(&meta),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.rows_imported, 2);
    let got = rows(
        &s,
        "SELECT id, name, active FROM people WHERE id IN (2, 4) ORDER BY id",
    )
    .await;
    assert_eq!(
        got,
        vec![
            vec![Value::Int(2), Value::Text("Bob".into()), Value::Bool(false)],
            vec![Value::Int(4), Value::Text("Dee".into()), Value::Null],
        ]
    );

    // REPLACE replaces the whole row.
    let csv = write(dir.path(), "r.csv", b"id,name\n1,Ann R\n");
    run_csv_import(
        &s,
        &request(
            &csv,
            "people",
            &[Some("id"), Some("name")],
            InsertMode::Replace,
        ),
        Some(&meta),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let got = rows(&s, "SELECT name, age FROM people WHERE id = 1").await;
    assert_eq!(got, vec![vec![Value::Text("Ann R".into()), Value::Null]]);
    assert_eq!(count(&s, "people").await, 4);

    // UPDATE ON DUPLICATE without the key column is refused before anything runs.
    let err = run_csv_import(
        &s,
        &request(
            &csv,
            "people",
            &[None, Some("name")],
            InsertMode::UpdateOnDuplicate,
        ),
        Some(&meta),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("primary key"), "{err}");
}

/// UPDATE ON DUPLICATE where the server has no upsert (Greenplum 6, Redshift: `DataCaps::upsert`)
/// is refused before anything reaches it.
#[tokio::test]
async fn an_upsert_is_refused_where_the_server_has_none() {
    use crate::sqlgen::Dialect;
    use crate::testkit::FakeSession;
    use crate::types::caps::SessionCaps;
    use crate::types::profile::Driver;

    let dir = tempfile::tempdir().unwrap();
    let meta = people(&session(dir.path()).await).await;
    let csv = write(
        dir.path(),
        "u.csv",
        b"id,name,age,active
1,Ann,30,true
",
    );
    let upsert = request(&csv, "people", &PEOPLE_MAP, InsertMode::UpdateOnDuplicate);
    let mut caps = SessionCaps::core(Driver::Sqlite, Dialect::Sqlite, None);
    caps.data.upsert = false;
    let none = FakeSession::new(caps);
    let err = run_csv_import(
        &none,
        &upsert,
        Some(&meta),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported: SQLite: updating the rows whose key exists is not available"
    );
    assert!(none.calls().is_empty(), "{:?}", none.calls());
    // Plain inserts go on there, and the upsert where the server has one.
    let insert = request(&csv, "people", &PEOPLE_MAP, InsertMode::StopOnError);
    run_csv_import(
        &none,
        &insert,
        Some(&meta),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let has = FakeSession::new(SessionCaps::core(Driver::Sqlite, Dialect::Sqlite, None));
    run_csv_import(&has, &upsert, Some(&meta), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert!(
        has.calls().iter().any(|c| c.contains("ON CONFLICT")),
        "{:?}",
        has.calls()
    );
}

#[tokio::test]
async fn stop_on_error_rolls_everything_back_and_names_the_rows() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path()).await;
    let meta = people(&s).await;
    let mut text = String::from("id,name,age,active\n");
    for i in 1..=2500 {
        // Row 1500 has no name: NOT NULL fails in the second statement (rows 1001–2000).
        let name = if i == 1500 {
            String::new()
        } else {
            format!("p{i}")
        };
        text.push_str(&format!("{i},{name},{},no\n", i % 90));
    }
    let csv = write(dir.path(), "big.csv", text.as_bytes());
    let report = run_csv_import(
        &s,
        &request(&csv, "people", &PEOPLE_MAP, InsertMode::StopOnError),
        Some(&meta),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let failure = report.failure.expect("the import fails");
    assert!(failure.message.contains("NOT NULL"), "{}", failure.message);
    assert_eq!(
        (failure.first_row, failure.last_row),
        (Some(1001), Some(2000))
    );
    assert_eq!(failure.line, Some(1002));
    assert_eq!(report.rows_imported, 0);
    assert!(report.single_transaction);
    assert_eq!(count(&s, "people").await, 0);
}

#[tokio::test]
async fn large_files_commit_in_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path()).await;
    let meta = people(&s).await;
    let mut text = String::from("id,name\n");
    for i in 1..=10 {
        let name = if i == 7 {
            String::new()
        } else {
            format!("p{i}")
        };
        text.push_str(&format!("{i},{name}\n"));
    }
    let csv = write(dir.path(), "c.csv", text.as_bytes());
    // 8 values = 4 rows of 2 columns per transaction.
    let report = import_with_chunks(
        &s,
        &request(
            &csv,
            "people",
            &[Some("id"), Some("name")],
            InsertMode::StopOnError,
        ),
        Some(&meta),
        8,
    )
    .await
    .unwrap();
    assert!(!report.single_transaction);
    // Rows 1–4 were committed; the chunk with row 7 (rows 5–8) rolled back.
    assert_eq!(report.rows_imported, 4);
    assert_eq!(count(&s, "people").await, 4);
    let failure = report.failure.unwrap();
    assert_eq!((failure.first_row, failure.last_row), (Some(5), Some(8)));
}

#[tokio::test]
async fn creates_a_new_table() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path()).await;
    let csv = write(
        dir.path(),
        "n.csv",
        b"code,price,when\n007,1.50,2026-01-02\n,2,\n",
    );
    let col = |name: &str, ty| ImportColumnDef {
        name: name.into(),
        ty,
        precision: None,
        scale: None,
    };
    let mut req = request(
        &csv,
        "x",
        &[Some("code"), Some("price"), Some("when")],
        InsertMode::StopOnError,
    );
    req.target = ImportTarget::Create {
        table: TableRef::new(None, "prices"),
        columns: vec![
            col("code", ImportType::Text),
            col("price", ImportType::Numeric),
            col("when", ImportType::Date),
        ],
    };
    let report = run_csv_import(&s, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(report.failure, None);
    assert!(report.created_table);
    assert_eq!(report.rows_imported, 2);
    let got = rows(&s, "SELECT code, \"when\" FROM prices ORDER BY rowid").await;
    assert_eq!(
        got,
        vec![
            vec![Value::Text("007".into()), Value::Date("2026-01-02".into())],
            vec![Value::Null, Value::Null],
        ]
    );
    // The table already exists now: creating it again fails, and nothing changes.
    let report = run_csv_import(&s, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert!(report.failure.unwrap().message.contains("already exists"));
    assert_eq!(count(&s, "prices").await, 2);
}

#[tokio::test]
async fn a_cancelled_import_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path()).await;
    let meta = people(&s).await;
    let csv = write(dir.path(), "p.csv", b"id,name\n1,a\n");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let report = run_csv_import(
        &s,
        &request(
            &csv,
            "people",
            &[Some("id"), Some("name")],
            InsertMode::StopOnError,
        ),
        Some(&meta),
        |_| {},
        cancel,
    )
    .await
    .unwrap();
    assert!(report.cancelled);
    assert_eq!(count(&s, "people").await, 0);
}

fn dump_request(path: &str, stop_on_error: bool) -> SqlImportRequest {
    SqlImportRequest {
        import_id: "sql".into(),
        path: path.into(),
        stop_on_error,
    }
}

const DUMP: &str = "-- a dump\n\
    PRAGMA foreign_keys=OFF;\n\
    BEGIN TRANSACTION;\n\
    CREATE TABLE t (id INTEGER PRIMARY KEY, note TEXT);\n\
    INSERT INTO t VALUES (1, 'semi;colon');\n\
    INSERT INTO t VALUES (1, 'duplicate');\n\
    INSERT INTO t VALUES (2, 'two');\n\
    COMMIT;\n";

#[tokio::test]
async fn runs_a_sql_dump_statement_by_statement() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path()).await;
    let path = write(dir.path(), "d.sql", DUMP.as_bytes());
    let mut seen = Vec::new();
    let report = run_sql_import(
        &s,
        &dump_request(&path, false),
        CancelSlot::default(),
        |p| seen.push(p),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!((report.executed, report.failed, report.skipped), (6, 1, 0));
    assert!(!report.stopped && !report.cancelled);
    assert_eq!(report.errors.len(), 1);
    let err = &report.errors[0];
    assert_eq!((err.index, err.line), (4, 6));
    assert!(err.message.contains("UNIQUE"), "{}", err.message);
    assert_eq!(seen.last().unwrap().statements, 7);
    assert_eq!(seen.last().unwrap().total_bytes, DUMP.len() as u64);
    assert_eq!(count(&s, "t").await, 2);
    assert_eq!(s.in_transaction(), Some(false));
}

#[tokio::test]
async fn stop_on_error_ends_a_dump_at_the_first_error() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path()).await;
    // Compressed, and decoded as Windows-1252 (not valid UTF-8).
    let path = dir.path().join("d.sql.gz");
    let mut gz = flate2::write::GzEncoder::new(
        std::fs::File::create(&path).unwrap(),
        flate2::Compression::fast(),
    );
    gz.write_all(b"CREATE TABLE u (v TEXT);\nINSERT INTO u VALUES ('caf\xE9');\nINSERT INTO nope VALUES (1);\nINSERT INTO u VALUES ('after');\n")
        .unwrap();
    gz.finish().unwrap();
    let report = run_sql_import(
        &s,
        &dump_request(&path.to_string_lossy(), true),
        CancelSlot::default(),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!((report.executed, report.failed), (2, 1));
    assert!(report.stopped);
    assert_eq!(report.encoding, TextEncoding::Windows1252);
    assert_eq!(
        rows(&s, "SELECT v FROM u").await,
        vec![vec![Value::Text("café".into())]]
    );

    // Cancelled before it starts: nothing runs.
    let cancel = CancellationToken::new();
    cancel.cancel();
    let report = run_sql_import(
        &s,
        &dump_request(&path.to_string_lossy(), false),
        CancelSlot::default(),
        |_| {},
        cancel,
    )
    .await
    .unwrap();
    assert!(report.cancelled);
    assert_eq!(report.executed, 0);
}

#[tokio::test]
async fn rows_of_another_width_stop_or_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path()).await;
    let meta = people(&s).await;
    let csv = write(
        dir.path(),
        "w.csv",
        b"id,name\n1,Ann\n2\n3,Cy,extra\n4,Dee\n",
    );
    let map = [Some("id"), Some("name"), None];
    // STOP ON ERROR: the short row stops the import, which writes nothing.
    let report = run_csv_import(
        &s,
        &request(&csv, "people", &map, InsertMode::StopOnError),
        Some(&meta),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let failure = report.failure.expect("a row error");
    assert_eq!(failure.message, "The row has 1 field, the header has 2");
    assert_eq!(
        (failure.first_row, failure.last_row, failure.line),
        (Some(2), Some(2), Some(3))
    );
    assert_eq!(report.rows_imported, 0);
    assert_eq!(count(&s, "people").await, 0);
    // REPLACE: the other rows go in, the two odd ones are counted and listed.
    let report = run_csv_import(
        &s,
        &request(&csv, "people", &map, InsertMode::Replace),
        Some(&meta),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.failure, None);
    assert_eq!((report.rows_imported, report.rows_skipped), (2, 2));
    assert_eq!(
        report
            .skipped
            .iter()
            .map(|r| (r.row, r.line))
            .collect::<Vec<_>>(),
        [(2, 3), (3, 4)]
    );
    assert_eq!(
        rows(&s, "SELECT id, name FROM people ORDER BY id").await,
        vec![
            vec![Value::Int(1), Value::Text("Ann".into())],
            vec![Value::Int(4), Value::Text("Dee".into())],
        ]
    );
}

#[tokio::test]
async fn sqlite_rowids_continue_after_imported_ids() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path()).await;
    s.execute(
        "CREATE TABLE seq (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)",
        &[],
    )
    .await
    .unwrap();
    let meta = s
        .table_meta(&TableRef::new(Some("main"), "seq"))
        .await
        .unwrap();
    let csv = write(dir.path(), "s.csv", b"id,name\n10,a\n20,b\n");
    let report = run_csv_import(
        &s,
        &request(
            &csv,
            "seq",
            &[Some("id"), Some("name")],
            InsertMode::StopOnError,
        ),
        Some(&meta),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.rows_imported, 2);
    s.execute("INSERT INTO seq (name) VALUES ('next')", &[])
        .await
        .unwrap();
    assert_eq!(
        rows(&s, "SELECT id FROM seq WHERE name = 'next'").await,
        vec![vec![Value::Int(21)]]
    );
}

const TX_DUMP: &str = "CREATE TABLE tx (id INTEGER PRIMARY KEY);\n\
    BEGIN;\n\
    INSERT INTO tx VALUES (1);\n\
    INSERT INTO tx VALUES (2);\n\
    INSERT INTO nope VALUES (3);\n\
    INSERT INTO tx VALUES (4);\n\
    COMMIT;\n";

#[tokio::test]
async fn a_dump_stopped_inside_its_transaction_rolls_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path()).await;
    let path = write(dir.path(), "tx.sql", TX_DUMP.as_bytes());
    let report = run_sql_import(
        &s,
        &dump_request(&path, true),
        CancelSlot::default(),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(report.stopped);
    assert_eq!((report.executed, report.failed), (4, 1));
    assert_eq!(report.rolled_back, Some(2));
    assert!(
        report.notes[0].contains("rolled back, and 2 statements were undone"),
        "{:?}",
        report.notes
    );
    assert_eq!(s.in_transaction(), Some(false));
    assert_eq!(count(&s, "tx").await, 0);

    // Without "Stop on error" SQLite keeps the transaction and its COMMIT keeps the rows.
    s.execute("DROP TABLE tx", &[]).await.unwrap();
    let report = run_sql_import(
        &s,
        &dump_request(&path, false),
        CancelSlot::default(),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.rolled_back, None);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    assert_eq!(count(&s, "tx").await, 3);

    // A dump without its COMMIT.
    let path = write(
        dir.path(),
        "open.sql",
        b"BEGIN;\nINSERT INTO tx VALUES (9);\n",
    );
    let report = run_sql_import(
        &s,
        &dump_request(&path, false),
        CancelSlot::default(),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.rolled_back, Some(1));
    assert_eq!(s.in_transaction(), Some(false));
    assert_eq!(count(&s, "tx").await, 3);
}

#[tokio::test]
async fn a_dump_reports_bytes_that_are_not_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path()).await;
    let path = write(
        dir.path(),
        "l.sql",
        b"CREATE TABLE l (v TEXT);\nINSERT INTO l VALUES ('caf\xE9');\nINSERT INTO l VALUES ('\xC3\xA9t\xC3\xA9');\n",
    );
    let report = run_sql_import(
        &s,
        &dump_request(&path, true),
        CancelSlot::default(),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!((report.executed, report.failed), (3, 0));
    assert_eq!(
        (report.invalid_utf8, report.first_invalid_line),
        (1, Some(2))
    );
    assert!(report.notes[0].starts_with("1 byte sequence was not UTF-8 (the first on line 2)"));
    // Only the invalid byte is Windows-1252: the UTF-8 after it stays UTF-8.
    assert_eq!(
        rows(&s, "SELECT v FROM l ORDER BY rowid").await,
        vec![
            vec![Value::Text("café".into())],
            vec![Value::Text("été".into())]
        ]
    );
    let preview = crate::import::preview_dump(&path, crate::sqlgen::Dialect::Sqlite).unwrap();
    assert_eq!(
        (preview.invalid_utf8, preview.first_invalid_line),
        (1, Some(2))
    );
    assert_eq!(preview.statements[1], "INSERT INTO l VALUES ('café')");
}
