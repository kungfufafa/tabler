//! Tests of the duckdb lane against the real engine (DuckDB runs in the test process): the
//! sample database, instances and their sharing, the read-only and hardened settings, streaming
//! execution and cancel, every value type, and the catalog parsers.
//!
//! Every test closes what it opened (workers, then the instance) before its `TempDir` goes, so
//! the file is unlocked and removable; several tests check exactly that.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::catalog;
use super::conn::{self, Worker, query_all};
use super::engine::{self, EngineConfig, Instance};
use super::errors;
use crate::drivers::{CancelSlot, CollectSink, TxState, cancel_portable, request_cancel};
use crate::error::AppError;
use crate::types::query::StatementKind;
use crate::types::schema::{TableKind, TableRef, ValueKind};
use crate::types::value::Value;

/// The sample database's SQL (also what `dev/fixtures/sample.duckdb` holds).
pub const SAMPLE_SQL: &str = include_str!("../../../tests/fixtures/duckdb/sample.sql");

/// A new database file at `path` holding the sample, closed again.
pub fn build_sample(path: &Path) {
    let conn = ::duckdb::Connection::open(path).expect("create the sample file");
    conn.execute_batch(SAMPLE_SQL).expect("run sample.sql");
    conn.execute_batch("CHECKPOINT").expect("checkpoint");
    conn.close().map_err(|(_, e)| e).expect("close the sample");
}

/// A temporary folder with the sample database in it.
pub fn sample() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("sample.duckdb");
    build_sample(&path);
    (dir, path)
}

/// A DuckDB string literal, as the dialect writes it.
pub fn lit(s: &str) -> String {
    (crate::sqlgen::dialect::duckdb::SPEC.quote_literal_exec)(s)
}

/// A path as a DuckDB string literal (forward slashes, which DuckDB takes on Windows too).
pub fn sql_path(p: &Path) -> String {
    lit(&p.to_string_lossy().replace('\\', "/"))
}

pub fn rows(conn: &::duckdb::Connection, sql: &str) -> Vec<Vec<Value>> {
    query_all(conn, sql, &[])
        .map(|(_, r)| r)
        .unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

fn rows_p(conn: &::duckdb::Connection, sql: &str, params: &[Value]) -> Vec<Vec<Value>> {
    query_all(conn, sql, params)
        .map(|(_, r)| r)
        .unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

fn exec(conn: &::duckdb::Connection, sql: &str) -> Result<(), AppError> {
    query_all(conn, sql, &[]).map(|_| ())
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn message(e: &AppError) -> String {
    match e {
        AppError::Db { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

/// Builds the sample at a path given by `TABLER_DUCKDB_SAMPLE_OUT` (the dev fixture script runs
/// it: `lane.mjs duckdb test drivers::duckdb::testing::write_sample_file -- --ignored`).
#[test]
#[ignore = "set TABLER_DUCKDB_SAMPLE_OUT"]
fn write_sample_file() {
    let Ok(out) = std::env::var("TABLER_DUCKDB_SAMPLE_OUT") else {
        return;
    };
    let out = PathBuf::from(out);
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(out.with_extension("duckdb.wal"));
    build_sample(&out);
}

// --- Instances, sharing and closing --------------------------------------------------------

#[test]
fn one_instance_per_file_and_settings() {
    let (dir, path) = sample();
    let a = Instance::open(EngineConfig::file(&path)).unwrap();
    // The same file under another spelling, same settings: the same instance.
    let other_spelling = dir.path().join(".").join("sample.duckdb");
    let b = Instance::open(EngineConfig::file(&other_spelling)).unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    let err = Instance::open(EngineConfig::file(&path).read_only()).unwrap_err();
    assert!(message(&err).contains("other settings"), "{err:?}");
    assert!(engine::open_instance(&path).is_some());
    drop((a, b));
    // Closed: the file opens again, with other settings, and its lock is gone.
    assert!(engine::open_instance(&path).is_none());
    let ro = Instance::open(EngineConfig::file(&path).read_only()).unwrap();
    let conn = ro.connect().unwrap();
    assert_eq!(
        rows(&conn, "SELECT count(*) FROM orders"),
        vec![vec![Value::Int(3)]]
    );
    drop(conn);
    drop(ro);
    drop(dir);
}

#[test]
fn workers_close_their_connection_and_the_file() {
    let (dir, path) = sample();
    let inst = Instance::open(EngineConfig::file(&path)).unwrap();
    let w1 = Worker::spawn(inst.clone()).unwrap();
    let w2 = Worker::spawn(inst.clone()).unwrap();
    drop(inst);
    w1.close();
    w2.close();
    drop((w1, w2));
    // Nothing holds the file: a raw open (another "process" view of it) works, then the folder
    // goes away.
    let raw = ::duckdb::Connection::open(&path).expect("the file is unlocked");
    raw.close().map_err(|(_, e)| e).unwrap();
    dir.close().expect("the temp dir is removable");
}

#[test]
fn a_second_open_of_a_held_file_names_the_way_out() {
    let (_dir, path) = sample();
    let inst = Instance::open(EngineConfig::file(&path)).unwrap();
    // Outside the registry (as another program would). Windows refuses the second open of the
    // file (a sharing violation naming this process), read-only too; POSIX record locks belong
    // to the process, so on macOS and Linux a second open in the same process may succeed.
    for flags in [
        ::duckdb::Config::default(),
        ::duckdb::Config::default()
            .access_mode(::duckdb::AccessMode::ReadOnly)
            .unwrap(),
    ] {
        match ::duckdb::Connection::open_with_flags(&path, flags) {
            Ok(c) if !cfg!(windows) => drop(c),
            Ok(_) => panic!("Windows opened a held file twice"),
            Err(e) => {
                let text = errors::open_error(&path.to_string_lossy(), e).to_string();
                assert!(text.contains(errors::FILE_LOCK_HINT), "{text}");
            }
        }
    }
    drop(inst);
}

#[test]
fn in_memory_databases_are_their_own() {
    let a = Instance::open(EngineConfig::memory()).unwrap();
    let b = Instance::open(EngineConfig::memory()).unwrap();
    assert!(!Arc::ptr_eq(&a, &b));
    let err = Instance::open(EngineConfig::memory().read_only()).unwrap_err();
    assert!(matches!(err, AppError::InvalidInput(_)));
}

#[test]
fn database_files_are_checked_and_created() {
    let (dir, path) = sample();
    engine::check_database_file(&path.to_string_lossy()).unwrap();
    let missing = dir.path().join("missing.duckdb");
    assert!(engine::check_database_file(&missing.to_string_lossy()).is_err());
    assert!(engine::check_database_file(&dir.path().to_string_lossy()).is_err());
    let not_duck = dir.path().join("x.duckdb");
    std::fs::write(&not_duck, b"SQLite format 3\0 and more bytes").unwrap();
    assert!(engine::check_database_file(&not_duck.to_string_lossy()).is_err());
    let empty = dir.path().join("empty.duckdb");
    std::fs::write(&empty, b"").unwrap();
    engine::check_database_file(&empty.to_string_lossy()).unwrap();

    let new = dir.path().join("new.duckdb");
    engine::create_database_file(&new.to_string_lossy()).unwrap();
    engine::check_database_file(&new.to_string_lossy()).unwrap();
    assert!(engine::create_database_file(&new.to_string_lossy()).is_err());
    // It is closed: an instance opens it.
    let inst = Instance::open(EngineConfig::file(&new)).unwrap();
    let conn = inst.connect().unwrap();
    assert_eq!(rows(&conn, "SELECT 1"), vec![vec![Value::Int(1)]]);
}

#[test]
fn startup_sql_runs_once_per_instance_and_use_per_connection() {
    let (dir, path) = sample();
    let other = dir.path().join("other.duckdb");
    engine::create_database_file(&other.to_string_lossy()).unwrap();
    let mut config = EngineConfig::file(&path);
    config.startup_sql = Some(format!(
        "ATTACH {} AS other; CREATE TABLE other.t AS SELECT 7 AS x; USE other",
        sql_path(&other)
    ));
    let inst = Instance::open(config).unwrap();
    let c1 = inst.connect().unwrap();
    let c2 = inst.connect().unwrap();
    for c in [&c1, &c2] {
        assert_eq!(
            rows(c, "SELECT current_database()"),
            vec![vec![text("other")]]
        );
        assert_eq!(rows(c, "SELECT x FROM t"), vec![vec![Value::Int(7)]]);
    }
    let bad = {
        let mut c = EngineConfig::file(dir.path().join("third.duckdb"));
        c.startup_sql = Some("SELEC 1".into());
        Instance::open(c).unwrap_err()
    };
    assert!(matches!(bad, AppError::Connect(_)), "{bad:?}");
}

// --- Read-only and hardened sessions -------------------------------------------------------

/// The folder DuckDB takes as its temp directory when none is set (`<db>.tmp`, or `.tmp` under
/// the working directory for an in-memory database), with a file planted in it. DuckDB lets SQL
/// into its temp directory once external access is off, so hardened instances set none.
/// Removes what it made.
struct TempFolder {
    dir: PathBuf,
    /// The folder as SQL names it here.
    sql: String,
    planted: String,
    created: bool,
}

impl TempFolder {
    fn of_file(db: &Path) -> TempFolder {
        let mut name = db.as_os_str().to_owned();
        name.push(".tmp");
        let dir = PathBuf::from(name);
        let sql = dir.to_string_lossy().replace('\\', "/");
        TempFolder::plant(dir, sql)
    }

    /// `.tmp` under the working directory, named relative as DuckDB names it.
    fn of_memory() -> TempFolder {
        TempFolder::plant(PathBuf::from(".tmp"), ".tmp".into())
    }

    fn plant(dir: PathBuf, sql: String) -> TempFolder {
        let created = !dir.exists();
        std::fs::create_dir_all(&dir).unwrap();
        // The working directory is shared by the tests and other runs.
        let planted = format!("tabler_planted_{}.csv", std::process::id());
        std::fs::write(dir.join(&planted), "a,b\n1,2\n").unwrap();
        TempFolder {
            dir,
            sql,
            planted,
            created,
        }
    }

    /// `name` in the folder as a string literal.
    fn sql_file(&self, name: &str) -> String {
        lit(&format!("{}/{name}", self.sql))
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.dir.join(&self.planted));
        if self.created {
            let _ = std::fs::remove_dir(&self.dir);
        }
    }
}

/// `path` with `suffix` added to its file name (`x.duckdb.wal`).
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[test]
fn read_only_refuses_writes_and_files() {
    let (dir, path) = sample();
    let csv = dir.path().join("secret.csv");
    std::fs::write(&csv, "a,b\n1,2\n").unwrap();
    let tmp = TempFolder::of_file(&path);
    let inst = Instance::open(EngineConfig::file(&path).read_only()).unwrap();
    assert!(inst.is_read_only() && inst.is_hardened() && inst.local_files_off());
    let conn = inst.connect().unwrap();
    assert_eq!(
        rows(&conn, "SELECT count(*) FROM orders"),
        vec![vec![Value::Int(3)]]
    );
    for sql in [
        "INSERT INTO customers VALUES (9, 'x')",
        "UPDATE orders SET total = 0",
        "CREATE TABLE x (a INTEGER)",
        "DROP TABLE events",
        "SELECT nextval('order_seq')",
    ] {
        assert!(
            exec(&conn, sql).is_err(),
            "{sql} ran on a read-only database"
        );
    }
    // Temporary tables live in memory and go with the instance; DuckDB takes them here.
    exec(&conn, "CREATE TEMP TABLE scratch AS SELECT 1 AS a").unwrap();
    assert_hardened(&conn, dir.path(), &csv, &tmp, Some(&path), true);
    assert_eq!(
        rows(&conn, "SELECT count(*) FROM customers"),
        vec![vec![Value::Int(3)]]
    );
}

#[test]
fn hardened_sessions_refuse_files_and_keep_their_settings() {
    let (dir, path) = sample();
    let csv = dir.path().join("secret.csv");
    std::fs::write(&csv, "a,b\n1,2\n").unwrap();
    let tmp = TempFolder::of_file(&path);
    // Control: an ordinary session reads the file both ways.
    {
        let plain = Instance::open(EngineConfig::memory()).unwrap();
        let conn = plain.connect().unwrap();
        let q = format!("SELECT count(*) FROM {}", sql_path(&csv));
        assert_eq!(rows(&conn, &q), vec![vec![Value::Int(1)]]);
        let q = format!("SELECT count(*) FROM read_csv({})", sql_path(&csv));
        assert_eq!(rows(&conn, &q), vec![vec![Value::Int(1)]]);
    }
    // Hardened but writable (an MCP or plugin session on a read-write profile): the database
    // takes writes, files and settings do not. Its WAL needs the file system, which stays on.
    let inst = Instance::open(EngineConfig::file(&path).hardened()).unwrap();
    assert!(!inst.local_files_off() && !inst.is_read_only());
    let conn = inst.connect().unwrap();
    exec(&conn, "INSERT INTO customers VALUES (9, 'x')").unwrap();
    assert_hardened(&conn, dir.path(), &csv, &tmp, Some(&path), false);
    // A second connection of the instance is just as locked.
    let conn2 = inst.connect().unwrap();
    assert_hardened(&conn2, dir.path(), &csv, &tmp, Some(&path), false);
}

#[test]
fn hardened_in_memory_instances_reach_no_files() {
    let dir = tempfile::tempdir().unwrap();
    let csv = dir.path().join("secret.csv");
    std::fs::write(&csv, "a,b\n1,2\n").unwrap();
    let tmp = TempFolder::of_memory();
    // Control: an ordinary in-memory session reads the planted file by the relative name.
    {
        let plain = Instance::open(EngineConfig::memory()).unwrap();
        let conn = plain.connect().unwrap();
        let q = format!("SELECT count(*) FROM {}", tmp.sql_file(&tmp.planted));
        assert_eq!(rows(&conn, &q), vec![vec![Value::Int(1)]]);
    }
    let mut config = EngineConfig::memory().hardened();
    config.startup_sql = Some(SAMPLE_SQL.into());
    let inst = Instance::open(config).unwrap();
    assert!(inst.local_files_off() && !inst.is_read_only());
    let conn = inst.connect().unwrap();
    exec(&conn, "INSERT INTO customers VALUES (9, 'x')").unwrap();
    assert_hardened(&conn, dir.path(), &csv, &tmp, None, true);
}

/// Startup SQL that asks for files (a folder, a temp directory) keeps the file system on; the
/// session is then no read-only one for `ReadOnlyMode::Server`.
#[test]
fn startup_sql_that_allows_files_keeps_the_file_system() {
    let (dir, path) = sample();
    let data = dir.path().join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::write(data.join("x.csv"), "a\n1\n").unwrap();
    let outside = dir.path().join("outside.csv");
    std::fs::write(&outside, "a\n1\n").unwrap();
    for startup in [
        format!("SET allowed_directories = [{}]", sql_path(&data)),
        format!(
            "SET temp_directory = {}",
            sql_path(&dir.path().join("spill"))
        ),
    ] {
        let mut config = EngineConfig::file(&path).read_only();
        config.startup_sql = Some(startup.clone());
        let inst = Instance::open(config).unwrap();
        assert!(
            inst.is_hardened() && !inst.local_files_off() && !inst.is_read_only(),
            "{startup}"
        );
        let conn = inst.connect().unwrap();
        if startup.contains("allowed_directories") {
            let q = format!("SELECT count(*) FROM {}", sql_path(&data.join("x.csv")));
            assert_eq!(rows(&conn, &q), vec![vec![Value::Int(1)]]);
        }
        assert!(exec(&conn, &format!("SELECT * FROM {}", sql_path(&outside))).is_err());
    }
}

/// What every hardened connection refuses, and what it still allows. `db` is the database file
/// (None in memory), `tmp` the folder DuckDB would take as its temp directory.
fn assert_hardened(
    conn: &::duckdb::Connection,
    dir: &Path,
    csv: &Path,
    tmp: &TempFolder,
    db: Option<&Path>,
    local_files_off: bool,
) {
    let out = dir.join("out.csv");
    let other = dir.join("other.duckdb");
    let refused = [
        format!("SELECT * FROM {}", sql_path(csv)),
        format!("SELECT * FROM read_csv({})", sql_path(csv)),
        format!("SELECT * FROM read_text({})", sql_path(csv)),
        format!("SELECT * FROM glob({})", sql_path(&dir.join("*"))),
        format!("SELECT * FROM {}", sql_path(&dir.join("*.csv"))),
        format!("DESCRIBE {}", sql_path(csv)),
        format!("COPY customers TO {}", sql_path(&out)),
        format!("COPY (SELECT 1) TO {}", sql_path(&out)),
        format!("ATTACH {} AS other", sql_path(&other)),
        format!("EXPORT DATABASE {}", sql_path(&dir.join("export"))),
        // The folder DuckDB allows when it has a temp directory.
        format!("SELECT * FROM {}", tmp.sql_file(&tmp.planted)),
        format!("SELECT * FROM read_csv({})", tmp.sql_file(&tmp.planted)),
        format!("SELECT * FROM glob({})", tmp.sql_file("*")),
        format!("COPY (SELECT 'w' AS a) TO {}", tmp.sql_file("written.csv")),
        format!("EXPORT DATABASE {}", tmp.sql_file("exp")),
        format!("ATTACH {} AS z", tmp.sql_file("z.duckdb")),
        "INSTALL httpfs".into(),
        "LOAD httpfs".into(),
        "SET enable_external_access = true".into(),
        "SET GLOBAL enable_external_access = true".into(),
        "RESET enable_external_access".into(),
        "SET lock_configuration = false".into(),
        "SET autoload_known_extensions = true".into(),
        "SET allowed_directories = ['/']".into(),
        format!("SET temp_directory = {}", sql_path(dir)),
        "RESET temp_directory".into(),
        "SET disabled_filesystems = ''".into(),
        "RESET disabled_filesystems".into(),
        format!("PRAGMA profiling_output = {}", sql_path(&out)),
        format!("SET profiling_output = {}", sql_path(&out)),
    ];
    for sql in &refused {
        assert!(exec(conn, sql).is_err(), "a hardened session ran {sql}");
    }
    assert!(!out.exists() && !other.exists());
    for name in ["written.csv", "exp", "z.duckdb"] {
        assert!(!tmp.dir.join(name).exists(), "{name}");
    }
    let settings = rows(
        conn,
        "SELECT current_setting('enable_external_access'), current_setting('temp_directory'), \
         len(current_setting('allowed_directories'))",
    );
    assert_eq!(
        settings,
        vec![vec![Value::Bool(false), text(""), Value::Int(0)]]
    );
    // With the local file system off nothing is reached, not even the paths DuckDB allows
    // itself beside the database (COPY would create a WAL there). A hardened instance that
    // takes writes keeps them for its WAL (module doc of `engine`).
    if let (Some(db), true) = (db, local_files_off) {
        for suffix in [".wal", ".wal.checkpoint", ".wal.recovery"] {
            let target = with_suffix(db, suffix);
            let sql = format!("COPY (SELECT 'w' AS a) TO {}", sql_path(&target));
            assert!(exec(conn, &sql).is_err(), "a hardened session ran {sql}");
            assert!(!target.exists(), "{suffix}");
        }
        let sql = format!("SELECT * FROM read_blob({})", sql_path(db));
        assert!(exec(conn, &sql).is_err(), "a hardened session ran {sql}");
    }
    // A plan binds the SQL in its string, so its scanners' binders run, and are refused (the
    // classifier calls `json_serialize_plan` a write besides).
    let inner = format!("SELECT * FROM read_csv({})", sql_path(csv));
    let plan = rows(
        conn,
        &format!("SELECT json_serialize_plan({})", lit(&inner)),
    );
    let plan = format!("{plan:?}");
    assert!(plan.contains("disabled by configuration"), "{plan}");
    // The lock covers SET and RESET only: the profiler's table function runs, but saves nothing
    // (the connection's statements fail until profiling is off).
    for profile in [
        sql_path(&dir.join("profile.json")),
        tmp.sql_file("profile.json"),
    ] {
        let _ = exec(
            conn,
            &format!("CALL enable_profiling(format := 'json', save_location := {profile})"),
        );
        let _ = exec(conn, "SELECT 1");
        exec(conn, "CALL disable_profiling()").unwrap();
    }
    assert!(!dir.join("profile.json").exists() && !tmp.dir.join("profile.json").exists());
    // Still allowed: schema switches, reads, the catalog.
    exec(conn, "USE crm").unwrap();
    assert_eq!(
        rows(conn, "SELECT count(*) FROM customers"),
        vec![vec![Value::Int(2)]]
    );
    exec(conn, "USE main").unwrap();
    rows(conn, "SELECT * FROM duckdb_tables()");
}

/// `enable_logging` is a table function the configuration lock does not cover either. With the
/// local file system off (read-only and in-memory hardened instances) DuckDB refuses file
/// storage, and the instance goes on.
#[test]
fn file_logging_is_refused_without_the_local_file_system() {
    let (dir, path) = sample();
    let logs = dir.path().join("logs");
    let call = format!(
        "CALL enable_logging(storage := 'file', storage_path := {})",
        sql_path(&logs)
    );
    for config in [
        EngineConfig::memory().hardened(),
        EngineConfig::file(&path).read_only(),
    ] {
        let inst = Instance::open(config).unwrap();
        assert!(inst.local_files_off());
        let conn = inst.connect().unwrap();
        let err = exec(&conn, &call).unwrap_err();
        assert!(message(&err).contains("LocalFileSystem"), "{err:?}");
        let other = inst.connect().unwrap();
        for c in [&conn, &other] {
            assert_eq!(rows(c, "SELECT 1"), vec![vec![Value::Int(1)]]);
        }
    }
    assert!(!logs.exists());
}

/// Where a hardened instance keeps its file system (its startup SQL set a temp directory here),
/// file storage outside the allowed paths writes nothing, but every later statement of the
/// instance fails on the log folder, `disable_logging` included, and closing a connection of it
/// then aborts the process (DuckDB throws from a destructor). So nothing here is ever closed,
/// and a session that lets writes through must refuse the call (brief, Limits).
#[test]
fn file_logging_writes_nothing_and_wedges_a_hardened_instance_with_files() {
    use std::mem::ManuallyDrop;
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let mut config = EngineConfig::memory().hardened();
    config.startup_sql = Some(format!(
        "SET temp_directory = {}",
        sql_path(&dir.path().join("spill"))
    ));
    // Never dropped: closing them would abort the test process.
    let inst = ManuallyDrop::new(Instance::open(config).unwrap());
    assert!(!inst.local_files_off());
    let conn = ManuallyDrop::new(inst.connect().unwrap());
    exec(
        &conn,
        &format!(
            "CALL enable_logging(storage := 'file', storage_path := {})",
            sql_path(&logs)
        ),
    )
    .unwrap();
    let other = ManuallyDrop::new(inst.connect().unwrap());
    for (c, sql) in [
        (&*conn, "SELECT 1"),
        (&*other, "SELECT 1"),
        (&*conn, "CALL disable_logging()"),
    ] {
        let err = exec(c, sql).unwrap_err();
        assert!(message(&err).contains("Permission Error"), "{sql}: {err:?}");
    }
    assert!(!logs.exists());
}

#[test]
fn select_into_and_inspections_never_write() {
    let inst = Instance::open(EngineConfig::memory()).unwrap();
    let conn = inst.connect().unwrap();
    exec(
        &conn,
        "CREATE TABLE t (a INTEGER); INSERT INTO t VALUES (1)",
    )
    .unwrap();
    // DuckDB has no SELECT … INTO (the classifier has no INTO keyword for that reason).
    assert!(exec(&conn, "SELECT 1 AS a INTO t2").is_err());
    assert!(exec(&conn, "SELECT * INTO t2 FROM t").is_err());
    assert!(
        rows(
            &conn,
            "SELECT * FROM duckdb_tables() WHERE table_name = 't2'"
        )
        .is_empty()
    );
    // Inspections take no statement that writes, and EXPLAIN only plans.
    for sql in [
        "DESCRIBE INSERT INTO t VALUES (2)",
        "SUMMARIZE INSERT INTO t VALUES (2)",
        "SHOW INSERT INTO t VALUES (2)",
        "DESCRIBE (INSERT INTO t VALUES (2) RETURNING *)",
        "SUMMARIZE (DELETE FROM t RETURNING *)",
        "SELECT * FROM (DELETE FROM t RETURNING *)",
        "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
        "EXPLAIN INSERT INTO t VALUES (2)",
        "EXPLAIN DELETE FROM t",
        "UNPIVOT t ON a INTO NAME n VALUE v",
    ] {
        let _ = exec(&conn, sql);
        assert_eq!(
            rows(&conn, "SELECT count(*) FROM t"),
            vec![vec![Value::Int(1)]],
            "{sql}"
        );
    }
}

/// A result column's type name parses back as the same type, whatever its members are called.
#[test]
fn type_names_parse_back() {
    use crate::sqlgen::dialect::duckdb::RESERVED_WORDS;
    let inst = Instance::open(EngineConfig::memory()).unwrap();
    let conn = inst.connect().unwrap();
    let engine: Vec<Value> = rows(
        &conn,
        "SELECT keyword_name FROM duckdb_keywords() \
         WHERE keyword_category IN ('reserved', 'type_function') ORDER BY 1",
    )
    .into_iter()
    .flatten()
    .collect();
    let mut ours: Vec<&str> = RESERVED_WORDS.split_ascii_whitespace().collect();
    ours.sort_unstable();
    assert_eq!(engine, ours.into_iter().map(text).collect::<Vec<_>>());
    for member in ["order", "key", "select", "x1", "_a", "B c", "a\"b", "1a"] {
        let sql = format!("SELECT {{{}: [1]}} AS s", lit(member));
        let (cols, _) = query_all(&conn, &sql, &[]).unwrap();
        let name = cols[0].data_type.clone();
        let (back, _) =
            query_all(&conn, &format!("SELECT CAST(NULL AS {name}) AS s"), &[]).unwrap();
        assert_eq!(back[0].data_type, name, "{member:?}");
    }
}

#[test]
fn encrypted_files_open_with_their_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("enc.duckdb");
    {
        let c = ::duckdb::Connection::open_in_memory().unwrap();
        // Writing one needs httpfs's crypto; the test fixture may use the unsafe fallback.
        c.execute_batch(&format!(
            "SET force_mbedtls_unsafe = true; ATTACH {} AS e (ENCRYPTION_KEY 'k3y'); \
             CREATE TABLE e.t AS SELECT 42 AS x; DETACH e;",
            sql_path(&path)
        ))
        .unwrap();
    }
    let mut config = EngineConfig::file(&path);
    config.encryption_key = Some("k3y".into());
    {
        let inst = Instance::open(config.clone()).unwrap();
        let conn = inst.connect().unwrap();
        assert_eq!(rows(&conn, "SELECT x FROM t"), vec![vec![Value::Int(42)]]);
        assert_eq!(
            rows(&conn, "SELECT current_database()"),
            vec![vec![text("enc")]]
        );
        // The file is read-only whatever the profile says; the instance is not (it is not
        // hardened): other files attach and take writes, and so does its in-memory database.
        assert!(inst.is_file_read_only() && !inst.is_read_only());
        assert!(exec(&conn, "INSERT INTO t VALUES (43)").is_err());
        let other = dir.path().join("other.duckdb");
        exec(&conn, &format!("ATTACH {} AS o2", sql_path(&other))).unwrap();
        exec(&conn, "CREATE TABLE o2.z AS SELECT 1 AS a").unwrap();
        exec(&conn, "DETACH o2").unwrap();
        exec(&conn, "CREATE TABLE memory.main.q AS SELECT 1 AS a").unwrap();
    }
    {
        let inst = Instance::open(config.clone().read_only()).unwrap();
        assert!(inst.is_read_only() && inst.is_file_read_only() && inst.local_files_off());
        let conn = inst.connect().unwrap();
        // No file beside the attached one either (DuckDB allows its WAL paths).
        let wal = with_suffix(&path, ".wal");
        let sql = format!("COPY (SELECT 1 AS a) TO {}", sql_path(&wal));
        assert!(exec(&conn, &sql).is_err() && !wal.exists());
        assert_eq!(
            rows(&conn, "SELECT count(*) FROM t"),
            vec![vec![Value::Int(1)]]
        );
        assert!(exec(&conn, "INSERT INTO t VALUES (44)").is_err());
        let other = dir.path().join("other2.duckdb");
        assert!(exec(&conn, &format!("ATTACH {} AS o3", sql_path(&other))).is_err());
        assert!(!other.exists());
        // In-memory scratch still takes writes, and goes with the instance (`is_read_only`).
        exec(&conn, "CREATE TABLE memory.main.q AS SELECT 1 AS a").unwrap();
    }
    let mut wrong = config.clone();
    wrong.encryption_key = Some("nope".into());
    assert!(Instance::open(wrong).is_err());
    assert!(Instance::open(EngineConfig::file(&path)).is_err());
}

// --- Execution -----------------------------------------------------------------------------

#[test]
fn statements_report_rows_counts_and_nothing() {
    let inst = Instance::open(EngineConfig::memory()).unwrap();
    let conn = inst.connect().unwrap();
    let slot = CancelSlot::default();
    let run = |sql: &str, cap: Option<usize>| {
        let mut sink = CollectSink::default();
        let out = conn::execute_streaming(&conn, sql, &[], cap, &mut sink, &slot)
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"));
        (out, sink)
    };
    let (out, _) = run("CREATE TABLE t (id INTEGER PRIMARY KEY, a VARCHAR)", None);
    assert_eq!((out.kind, out.affected), (StatementKind::Empty, None));
    let (out, _) = run(
        "INSERT INTO t SELECT i, 'x' || i FROM range(5000) r(i)",
        None,
    );
    assert_eq!(
        (out.kind, out.affected),
        (StatementKind::Affected, Some(5000))
    );
    let (out, _) = run("UPDATE t SET a = 'y' WHERE id < 10", None);
    assert_eq!(out.affected, Some(10));
    let (out, _) = run("DELETE FROM t WHERE id < 0", None);
    assert_eq!((out.kind, out.affected), (StatementKind::Affected, Some(0)));
    let (out, sink) = run("DELETE FROM t WHERE id = 1 RETURNING id", None);
    assert_eq!(out.kind, StatementKind::Rows);
    assert_eq!(sink.sets[0].rows, vec![vec![Value::Int(1)]]);
    let (out, _) = run("CREATE TABLE u AS SELECT * FROM t", None);
    assert_eq!(out.affected, Some(4999));
    let (out, _) = run("SET threads = 2", None);
    assert_eq!(out.kind, StatementKind::Empty);
    let (out, sink) = run("SELECT * FROM t ORDER BY id", Some(3000));
    assert!(out.truncated);
    assert_eq!(out.row_count, 3000);
    assert_eq!(sink.sets[0].rows.len(), 3000);
    assert_eq!(sink.sets[0].columns[1].data_type, "VARCHAR");
    let (out, sink) = run("SELECT count(*) AS \"Count\" FROM t", None);
    assert_eq!(out.kind, StatementKind::Rows);
    assert_eq!(sink.sets[0].rows, vec![vec![Value::Int(4999)]]);
    let (out, sink) = run("PRAGMA table_info('t')", None);
    assert_eq!(out.kind, StatementKind::Rows);
    assert_eq!(sink.sets[0].rows.len(), 2);
    // Parameters arrive as text and DuckDB casts them to what the statement expects.
    let r = rows_p(
        &conn,
        "SELECT a FROM t WHERE id = $1 AND $2::DATE > DATE '2000-01-01'",
        &[text("7"), Value::Date("2024-01-31".into())],
    );
    assert_eq!(r, vec![vec![text("y")]]);
    let r = rows_p(
        &conn,
        "SELECT $1 + 1",
        &[Value::BigInt("9007199254740993".into())],
    );
    assert_eq!(r, vec![vec![Value::BigInt("9007199254740994".into())]]);
    // A syntax error is a Db error with DuckDB's message.
    let mut sink = CollectSink::default();
    let err = conn::execute_streaming(&conn, "SELEC 1", &[], None, &mut sink, &slot).unwrap_err();
    assert!(message(&err).starts_with("Parser Error"), "{err:?}");
}

#[test]
fn cancelled_slots_refuse_to_start() {
    let inst = Instance::open(EngineConfig::memory()).unwrap();
    let conn = inst.connect().unwrap();
    let slot = CancelSlot::default();
    request_cancel(&slot);
    let mut sink = CollectSink::default();
    assert!(matches!(
        conn::execute_streaming(&conn, "SELECT 1", &[], None, &mut sink, &slot),
        Err(AppError::Cancelled)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workers_stream_cancel_and_track_transactions() {
    let inst = Instance::open(EngineConfig::memory()).unwrap();
    let worker = Worker::spawn(inst).unwrap();
    let tx = Arc::new(TxState::new(Some(false)));
    let mut sink = CollectSink::default();
    let out = conn::run(
        &worker,
        "SELECT * FROM range(10000)",
        &[],
        Some(2500),
        &mut sink,
        CancelSlot::default(),
        Some(tx.clone()),
    )
    .await
    .unwrap();
    assert!(out.truncated);
    assert_eq!(sink.sets[0].rows.len(), 2500);
    assert_eq!(tx.get(), Some(false));

    let mut sink = CollectSink::default();
    conn::run(
        &worker,
        "BEGIN TRANSACTION",
        &[],
        None,
        &mut sink,
        CancelSlot::default(),
        Some(tx.clone()),
    )
    .await
    .unwrap();
    assert_eq!(tx.get(), Some(true));
    conn::run(
        &worker,
        "ROLLBACK",
        &[],
        None,
        &mut sink,
        CancelSlot::default(),
        Some(tx.clone()),
    )
    .await
    .unwrap();
    assert_eq!(tx.get(), Some(false));

    // A long query, interrupted through the slot's handle.
    let slot = CancelSlot::default();
    let canceller = {
        let slot = slot.clone();
        tokio::spawn(async move {
            for _ in 0..200 {
                tokio::time::sleep(Duration::from_millis(25)).await;
                if let Some(handle) = request_cancel(&slot) {
                    return cancel_portable(handle).await.is_some();
                }
            }
            false
        })
    };
    let mut sink = CollectSink::default();
    let started = std::time::Instant::now();
    let result = conn::run(
        &worker,
        "SELECT count(*) FROM range(100000000000) r(i) WHERE i % 7 = 3",
        &[],
        None,
        &mut sink,
        slot,
        None,
    )
    .await;
    assert!(canceller.await.unwrap(), "no handle was armed");
    assert!(matches!(result, Err(AppError::Cancelled)), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(30));
    // The connection works afterwards.
    let mut sink = CollectSink::default();
    conn::run(
        &worker,
        "SELECT 1",
        &[],
        None,
        &mut sink,
        CancelSlot::default(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(sink.sets[0].rows, vec![vec![Value::Int(1)]]);
    worker.close();
    let mut sink = CollectSink::default();
    let err = conn::run(
        &worker,
        "SELECT 1",
        &[],
        None,
        &mut sink,
        CancelSlot::default(),
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::NotConnected));
}

// --- Values --------------------------------------------------------------------------------

#[test]
fn every_type_of_the_sample_decodes() {
    let (_dir, path) = sample();
    let inst = Instance::open(EngineConfig::file(&path)).unwrap();
    let conn = inst.connect().unwrap();
    let (cols, r) = query_all(&conn, "SELECT * FROM all_types ORDER BY id", &[]).unwrap();
    let col = |name: &str| cols.iter().position(|c| c.name == name).unwrap();
    let v = |name: &str| r[0][col(name)].clone();
    assert_eq!(v("b"), Value::Bool(true));
    assert_eq!(v("ti"), Value::Int(-8));
    assert_eq!(v("bi"), Value::BigInt("9007199254740993".into()));
    assert_eq!(
        v("hi"),
        Value::BigInt("-170141183460469231731687303715884105728".into())
    );
    assert_eq!(v("ubi"), Value::BigInt("18446744073709551615".into()));
    assert_eq!(
        v("uhi"),
        Value::BigInt("340282366920938463463374607431768211455".into())
    );
    assert_eq!(v("f"), Value::Float(0.5));
    assert_eq!(v("d"), Value::Float(0.1));
    assert_eq!(v("num"), Value::Decimal("12345.678".into()));
    assert_eq!(v("v"), text("héllo"));
    assert_eq!(v("bl"), Value::from_bytes(&[0xAA, 0x00, b'a', b'b']));
    assert_eq!(v("bt"), Value::other("bit", "10101"));
    assert_eq!(v("dt"), Value::Date("2024-01-31".into()));
    assert_eq!(v("tm"), Value::Time("12:00:00.5".into()));
    // Arrow carries a TIMETZ as its local time: the offset is not known here (the grid reads
    // these columns as text).
    assert_eq!(v("ttz"), Value::Time("12:00:00".into()));
    assert_eq!(v("ts"), Value::Timestamp("2024-01-31 12:00:00.5".into()));
    assert_eq!(v("ts_s"), Value::Timestamp("2024-01-31 12:00:00".into()));
    assert_eq!(
        v("ts_ms"),
        Value::Timestamp("2024-01-31 12:00:00.123".into())
    );
    assert_eq!(
        v("ts_ns"),
        Value::Timestamp("2024-01-31 12:00:00.123456789".into())
    );
    assert_eq!(
        v("tstz"),
        Value::TimestampTz("2024-01-31 10:00:00+00".into())
    );
    assert_eq!(
        v("iv"),
        Value::Interval("1 year 2 months 3 days 04:05:06.789".into())
    );
    assert_eq!(
        v("u"),
        Value::Uuid("6bea4f37-ddba-4e31-95b0-15bb71efdb48".into())
    );
    assert_eq!(v("j"), Value::Json("{\"a\": [1, 2]}".into()));
    assert_eq!(v("e"), Value::Enum("happy".into()));
    assert_eq!(v("l"), Value::Json("[1, 2, null]".into()));
    assert_eq!(v("a"), Value::Json("[\"x\", \"y\"]".into()));
    assert_eq!(v("s"), Value::Json("{\"a\": 1, \"b\": \"it's\"}".into()));
    assert_eq!(v("m"), Value::Json("{\"k\": 1, \"j\": 2}".into()));
    assert_eq!(v("un"), Value::Json("\"u\"".into()));
    assert_eq!(v("bn"), Value::BigInt("-42".into()));
    // The all-NULL row.
    assert!(r[1][1..].iter().all(Value::is_null), "{:?}", r[1]);
    // Column types as the catalog spells them.
    let ty = |name: &str| (cols[col(name)].data_type.clone(), cols[col(name)].kind);
    assert_eq!(ty("num"), ("DECIMAL(18,3)".into(), ValueKind::Decimal));
    assert_eq!(ty("a"), ("VARCHAR[2]".into(), ValueKind::Json));
    assert_eq!(
        ty("s"),
        ("STRUCT(a INTEGER, b VARCHAR)".into(), ValueKind::Json)
    );
    assert_eq!(
        ty("tstz"),
        ("TIMESTAMP WITH TIME ZONE".into(), ValueKind::TimestampTz)
    );
    assert_eq!(ty("e"), ("ENUM".into(), ValueKind::Enum));
    assert_eq!(ty("j"), ("JSON".into(), ValueKind::Json));
    // A whole blob for "load full value".
    let whole = conn::fetch_whole(&conn, "SELECT from_hex(repeat('FF', 70000))", &[]).unwrap();
    match whole {
        Some(Value::Bytes(b)) => assert!(!b.truncated && b.len == 70000),
        other => panic!("{other:?}"),
    }
}

/// `test_all_types()` but its VARIANT columns, which the duckdb crate refuses to decode (the
/// grid reads them as text; the editor reports the crate's error).
const ALL_TYPES: &str = "SELECT COLUMNS(c -> c NOT ILIKE '%variant%') FROM test_all_types()";

/// Every type DuckDB has, at its extremes and NULL (`test_all_types()`), decodes without a
/// panic, also with the Arrow export settings a user may change (string and list views,
/// lossless extension types, large buffers).
#[test]
fn test_all_types_decode_under_every_arrow_setting() {
    let inst = Instance::open(EngineConfig::memory()).unwrap();
    let conn = inst.connect().unwrap();
    let settings = [
        "SET produce_arrow_string_view = false",
        "SET arrow_lossless_conversion = true",
        "SET produce_arrow_string_view = true",
        "SET arrow_output_list_view = true",
        "SET arrow_large_buffer_size = true",
    ];
    let check = |label: &str| {
        let (cols, r) =
            query_all(&conn, ALL_TYPES, &[]).unwrap_or_else(|e| panic!("{label}: {e:?}"));
        assert_eq!(r.len(), 3, "{label}");
        assert!(cols.len() > 40, "{label}");
        let col = |name: &str| cols.iter().position(|c| c.name == name);
        if let Some(i) = col("hugeint") {
            assert_eq!(
                r[0][i],
                Value::BigInt("-170141183460469231731687303715884105728".into()),
                "{label}"
            );
        }
        if let Some(i) = col("uuid") {
            assert!(matches!(&r[0][i], Value::Uuid(_)), "{label}: {:?}", r[0][i]);
        }
        if let Some(i) = col("varchar") {
            assert!(matches!(&r[0][i], Value::Text(_)), "{label}: {:?}", r[0][i]);
        }
        if let Some(i) = col("int_array") {
            assert!(matches!(&r[0][i], Value::Json(_)), "{label}: {:?}", r[0][i]);
        }
        // The NULL row is NULL everywhere.
        assert!(r[2].iter().all(Value::is_null), "{label}: {:?}", r[2]);
        // Nested values are JSON text serde can read.
        for row in &r {
            for v in row {
                if let Value::Json(s) = v {
                    serde_json::from_str::<serde_json::Value>(s)
                        .unwrap_or_else(|e| panic!("{label}: {s}: {e}"));
                }
            }
        }
    };
    check("defaults");
    for s in settings {
        exec(&conn, s).unwrap();
        check(s);
    }
}

/// The decoders write what DuckDB's own text cast writes, for every value of
/// `test_all_types()` (extremes, infinities, BC dates) of the types that travel as text.
#[test]
fn decoded_text_matches_duckdbs_own() {
    let inst = Instance::open(EngineConfig::memory()).unwrap();
    let conn = inst.connect().unwrap();
    let columns = [
        "bigint",
        "ubigint",
        "hugeint",
        "uhugeint",
        "date",
        "time",
        "timestamp",
        "timestamp_s",
        "timestamp_ms",
        "timestamp_ns",
        "timestamp_tz",
        "interval",
        "dec_4_1",
        "dec_9_4",
        "dec_18_6",
        "dec38_10",
        "uuid",
        "bit",
        "varint",
        "bignum",
        "small_enum",
        "varchar",
    ];
    let present: Vec<String> = rows(
        &conn,
        "SELECT column_name FROM (DESCRIBE SELECT * FROM test_all_types())",
    )
    .into_iter()
    .filter_map(|r| r.first().and_then(Value::as_text))
    .collect();
    let mut checked = 0;
    for c in columns {
        if !present.iter().any(|p| p == c) {
            continue;
        }
        let q = format!("SELECT \"{c}\", \"{c}\"::VARCHAR FROM test_all_types()");
        for r in rows(&conn, &q) {
            let (Some(ours), Some(theirs)) = (r[0].as_text(), r[1].as_text()) else {
                assert!(r[0].is_null() && r[1].is_null(), "{c}: {r:?}");
                continue;
            };
            assert_eq!(ours, theirs, "{c}");
            checked += 1;
        }
    }
    assert!(checked >= 40, "only {checked} values compared");
}

// --- Catalog -------------------------------------------------------------------------------

#[test]
fn the_catalog_of_the_sample() {
    let (_dir, path) = sample();
    let inst = Instance::open(EngineConfig::file(&path).read_only()).unwrap();
    let conn = inst.connect().unwrap();

    let info = catalog::parse_server_info(&rows(&conn, catalog::SERVER_INFO)).unwrap();
    assert!(info.version.starts_with("v1.5"), "{info:?}");
    assert_eq!(info.database.as_deref(), Some("sample"));
    assert_eq!(info.schema.as_deref(), Some("main"));

    let dbs = catalog::parse_databases(&rows(&conn, catalog::DATABASES));
    assert_eq!(dbs.len(), 1, "{dbs:?}");
    assert!(dbs[0].is_current && dbs[0].read_only && dbs[0].path.is_some());

    let schemas = catalog::parse_schemas(&rows(&conn, catalog::SCHEMAS));
    let names: Vec<&str> = schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["crm", "main"]);
    assert!(schemas[1].is_default && !schemas[0].is_default);

    let tables = catalog::parse_tables(&rows_p(&conn, catalog::TABLES, &[text("main")]), "main");
    let names: Vec<(&str, TableKind)> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(
        names,
        vec![
            ("all_types", TableKind::Table),
            ("customers", TableKind::Table),
            ("events", TableKind::Table),
            ("order_items", TableKind::Table),
            ("orders", TableKind::Table),
            ("recent_orders", TableKind::View),
        ]
    );
    let orders = tables.iter().find(|t| t.name == "orders").unwrap();
    assert_eq!(orders.row_estimate, Some(3));
    let view = tables.iter().find(|t| t.name == "recent_orders").unwrap();
    assert_eq!(view.comment.as_deref(), Some("Orders of 2024 on"));

    let meta = table_meta(&conn, "main", "orders");
    assert_eq!(meta.primary_key, vec!["id"]);
    let id = &meta.columns[0];
    assert!(id.is_pk && id.is_auto_increment && id.pk_ordinal == Some(1));
    let total = meta.columns.iter().find(|c| c.name == "total").unwrap();
    assert_eq!(
        (total.data_type.as_str(), total.kind, total.nullable),
        ("DECIMAL(12,2)", ValueKind::Decimal, false)
    );
    assert_eq!(
        (total.numeric_precision, total.numeric_scale),
        (Some(12), Some(2))
    );
    assert_eq!(meta.foreign_keys.len(), 1);
    assert_eq!(
        meta.foreign_keys[0].ref_table,
        TableRef::new(Some("main"), "customers")
    );
    assert_eq!(meta.foreign_keys[0].columns, vec!["customer_id"]);
    assert_eq!(meta.checks.len(), 1);
    assert!(
        meta.checks[0].expression.contains("total >= 0"),
        "{:?}",
        meta.checks
    );
    let index_names: Vec<&str> = meta.indexes.iter().map(|i| i.name.as_str()).collect();
    assert!(index_names.contains(&"orders_placed"), "{index_names:?}");
    let unique = meta
        .indexes
        .iter()
        .find(|i| i.name == "orders_customer_status")
        .unwrap();
    assert!(unique.unique);
    assert_eq!(unique.columns, vec!["customer_id", "lower(status)"]);
    assert!(
        meta.indexes
            .iter()
            .any(|i| i.primary && i.columns == ["id"])
    );
    assert!(
        meta.ddl
            .as_deref()
            .is_some_and(|d| d.starts_with("CREATE TABLE orders"))
    );

    let items = table_meta(&conn, "main", "order_items");
    assert_eq!(items.primary_key, vec!["order_id", "line"]);
    let events = table_meta(&conn, "main", "events");
    assert!(events.primary_key.is_empty());
    assert_eq!(events.columns[2].kind, ValueKind::Json);

    let crm = table_meta(&conn, "crm", "customers");
    assert_eq!(crm.comment.as_deref(), Some("People who buy"));
    let email = crm.columns.iter().find(|c| c.name == "email").unwrap();
    assert_eq!(email.comment.as_deref(), Some("Login"));
    let feeling = crm.columns.iter().find(|c| c.name == "feeling").unwrap();
    assert_eq!(feeling.kind, ValueKind::Enum);
    assert_eq!(
        feeling.enum_values.as_deref(),
        Some(&["sad".to_string(), "ok".to_string(), "happy".to_string()][..])
    );
    let tags = crm.columns.iter().find(|c| c.name == "tags").unwrap();
    assert_eq!(tags.array_elem_type.as_deref(), Some("VARCHAR"));
    assert!(
        crm.indexes
            .iter()
            .any(|i| i.unique && !i.primary && i.columns == ["email"])
    );

    let view = catalog::parse_definition(&rows_p(
        &conn,
        catalog::VIEW_DEFINITION,
        &[text("main"), text("recent_orders")],
    ));
    assert!(view.is_some_and(|v| v.to_uppercase().starts_with("CREATE VIEW")));
    let ddl = catalog::parse_table_ddl(&rows_p(
        &conn,
        catalog::TABLE_DDL,
        &[text("main"), text("orders")],
    ))
    .unwrap();
    assert!(ddl.contains("CREATE INDEX orders_placed"), "{ddl}");

    let seqs = catalog::parse_sequences(
        &rows_p(&conn, catalog::SEQUENCES, &[text("main")]),
        &rows_p(&conn, catalog::SEQUENCE_USERS, &[text("main")]),
        "main",
    );
    assert_eq!(seqs.len(), 1);
    assert_eq!(seqs[0].owned_by.as_deref(), Some("orders.id"));
    let seq_ddl = catalog::parse_definition(&rows_p(
        &conn,
        catalog::SEQUENCE_DEFINITION,
        &[text("main"), text("order_seq")],
    ));
    assert!(seq_ddl.is_some_and(|d| d.contains("SEQUENCE")));

    let macros = catalog::parse_macros(&rows_p(&conn, catalog::MACROS, &[text("main")]), "main");
    let names: Vec<(&str, Option<&str>)> = macros
        .iter()
        .map(|m| (m.name.as_str(), m.return_type.as_deref()))
        .collect();
    assert_eq!(
        names,
        vec![("add_tax", Some("MACRO")), ("top_orders", Some("TABLE"))]
    );
    let def = catalog::parse_macro_definition(
        &rows_p(
            &conn,
            catalog::MACRO_DEFINITION,
            &[text("main"), text("top_orders")],
        ),
        "main",
        "top_orders",
    )
    .unwrap();
    assert!(
        def.starts_with("CREATE MACRO \"main\".\"top_orders\"(\"n\") AS TABLE"),
        "{def}"
    );
    // The text recreates the macro (elsewhere: this instance is read-only).
    {
        let scratch = Instance::open(EngineConfig::memory()).unwrap();
        let c = scratch.connect().unwrap();
        exec(
            &c,
            "CREATE TABLE orders AS SELECT 1 AS total UNION ALL SELECT 2",
        )
        .unwrap();
        exec(&c, &def).unwrap();
        assert_eq!(
            rows(&c, "SELECT total FROM top_orders(1)"),
            vec![vec![Value::Int(2)]]
        );
    }

    let enums = catalog::parse_enum_types(&rows(&conn, catalog::ENUM_TYPES));
    assert_eq!(catalog::user_types(&enums)[0].name, "mood");

    let completion =
        catalog::parse_completion(&rows(&conn, catalog::COMPLETION), Some("main".into()));
    assert_eq!(completion.schemas["crm"]["customers"][0], "id");
    assert!(completion.schemas["main"].contains_key("recent_orders"));
}

fn table_meta(
    conn: &::duckdb::Connection,
    schema: &str,
    table: &str,
) -> crate::types::schema::TableMeta {
    let params = [text(schema), text(table)];
    let r = TableRef::new(Some(schema), table);
    let info = catalog::parse_table(&rows_p(conn, catalog::TABLE, &params), &r).unwrap();
    let enums = catalog::parse_enum_types(&rows(conn, catalog::ENUM_TYPES));
    let columns = catalog::parse_columns(&rows_p(conn, catalog::COLUMNS, &params), &enums);
    let constraints =
        catalog::parse_constraints(&rows_p(conn, catalog::CONSTRAINTS, &params), schema, table);
    let indexes = catalog::parse_indexes(&rows_p(conn, catalog::INDEXES, &params));
    catalog::table_meta(r, info, columns, constraints, indexes)
}
