//! `run_export` against a fake in-memory session: files, formats, decoding, limits, cancel.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

use async_trait::async_trait;

use super::*;
use crate::drivers::{
    CancelHandle, RowChunker, ServerInfo, StatementOutcome, TxStatement, arm_cancel,
    cancel_requested,
};
use crate::sqlgen::SqlDialect;
use crate::types::browse::SortSpec;
use crate::types::export::{
    CsvQuote, ExportLayout, ExportResultItem, ExportResultsRequest, ExportTablesRequest,
};
use crate::types::profile::Driver;
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, ForeignKeyMeta, SchemaInfo, TableKind, TableRef,
    TableSummary, TriggerMeta,
};

type Set = (Vec<ResultColumn>, Vec<Vec<Value>>);

/// Streams canned result sets; records the statements it was asked to run.
struct FakeSession {
    driver: Driver,
    dialect: SqlDialect,
    sets: Vec<Set>,
    meta: Option<TableMeta>,
    /// Never delivers anything; waits until the cancel slot is cancelled.
    block_until_cancel: bool,
    /// After the sink stops the stream, keeps "draining" the statement for this long (as the
    /// drivers do with a statement `cancel_safe_on_stop` refuses) unless it is cancelled.
    drain_after_stop: Option<Duration>,
    /// Answer of the time zone query (`session_time_zone`); `None` = the fake cannot answer it.
    zone: Option<Vec<Value>>,
    seen: Mutex<Vec<(String, Vec<Value>)>>,
    cancels: AtomicUsize,
}

impl FakeSession {
    fn new(dialect: Dialect, sets: Vec<Set>) -> FakeSession {
        FakeSession {
            driver: match dialect {
                Dialect::Postgres => Driver::Postgres,
                Dialect::Mysql => Driver::Mysql,
                Dialect::Sqlite => Driver::Sqlite,
            },
            dialect: dialect.into(),
            sets,
            meta: None,
            block_until_cancel: false,
            drain_after_stop: None,
            zone: None,
            seen: Mutex::new(Vec::new()),
            cancels: AtomicUsize::new(0),
        }
    }

    /// The row the session's time zone query answers.
    fn with_zone(mut self, row: Vec<Value>) -> FakeSession {
        self.zone = Some(row);
        self
    }

    fn with_meta(mut self, meta: TableMeta) -> FakeSession {
        self.meta = Some(meta);
        self
    }

    fn last_sql(&self) -> String {
        self.seen.lock().unwrap().last().cloned().unwrap().0
    }
}

fn unsupported<T>() -> Result<T, AppError> {
    Err(AppError::Unsupported("fake session".into()))
}

#[async_trait]
impl DbSession for FakeSession {
    fn driver(&self) -> Driver {
        self.driver
    }
    fn dialect(&self) -> SqlDialect {
        self.dialect
    }
    async fn server_info(&self) -> Result<ServerInfo, AppError> {
        Ok(ServerInfo::default())
    }
    async fn ping(&self) -> Result<(), AppError> {
        Ok(())
    }
    async fn close(&self) {}
    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        unsupported()
    }
    async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        unsupported()
    }
    async fn list_tables(&self, _schema: &str) -> Result<Vec<TableSummary>, AppError> {
        unsupported()
    }
    async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        self.meta
            .clone()
            .ok_or_else(|| AppError::NotFound(table.name.clone()))
    }
    async fn view_definition(&self, _table: &TableRef) -> Result<Option<String>, AppError> {
        unsupported()
    }
    async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        unsupported()
    }
    async fn query(&self, sql: &str, _params: &[Value]) -> Result<ResultSet, AppError> {
        match (
            &self.zone,
            sql.contains("time_zone") || sql.contains("TimeZone"),
        ) {
            (Some(row), true) => Ok(ResultSet {
                columns: Vec::new(),
                rows: vec![row.clone()],
                truncated: false,
            }),
            _ => unsupported(),
        }
    }
    async fn execute(&self, _sql: &str, _params: &[Value]) -> Result<ExecOutcome, AppError> {
        unsupported()
    }
    async fn transaction(
        &self,
        _statements: Vec<TxStatement>,
    ) -> Result<Vec<ExecOutcome>, AppError> {
        unsupported()
    }
    async fn run_statement(
        &self,
        _sql: &str,
        _max_rows: usize,
        _sink: &mut dyn RowSink,
        _cancel: CancelSlot,
    ) -> Result<StatementOutcome, AppError> {
        unsupported()
    }
    async fn stream_query(
        &self,
        sql: &str,
        params: &[Value],
        sink: &mut dyn RowSink,
        cancel: CancelSlot,
    ) -> Result<u64, AppError> {
        self.seen
            .lock()
            .unwrap()
            .push((sql.to_string(), params.to_vec()));
        let _armed = arm_cancel(&cancel, CancelHandle::Mysql { conn_id: 1 })?;
        if self.block_until_cancel {
            loop {
                if cancel_requested(&cancel) {
                    return Err(AppError::Cancelled);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        let mut chunker = RowChunker::new(sink);
        let mut stopped = false;
        'sets: for (columns, rows) in &self.sets {
            if chunker.begin_set(columns.clone()).is_break() {
                stopped = true;
                break;
            }
            for row in rows {
                if chunker.push(row.clone()).is_break() {
                    stopped = true;
                    break 'sets;
                }
            }
        }
        stopped |= chunker.flush().is_break();
        let total = chunker.total_rows;
        if stopped && let Some(drain) = self.drain_after_stop {
            let started = Instant::now();
            while started.elapsed() < drain {
                if cancel_requested(&cancel) {
                    return Err(AppError::Cancelled);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        Ok(total)
    }
    async fn cancel(&self, _handle: CancelHandle) -> Result<(), AppError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn in_transaction(&self) -> Option<bool> {
        Some(false)
    }
}

fn rcol(name: &str, kind: ValueKind, data_type: &str) -> ResultColumn {
    ResultColumn {
        name: name.into(),
        data_type: data_type.into(),
        kind,
        nullable: None,
        table: None,
        origin: None,
    }
}

fn mcol(name: &str, data_type: &str, kind: ValueKind, pk: bool) -> ColumnMeta {
    ColumnMeta {
        name: name.into(),
        ordinal: 1,
        data_type: data_type.into(),
        kind,
        nullable: !pk,
        default_expr: None,
        is_pk: pk,
        pk_ordinal: pk.then_some(1),
        is_auto_increment: false,
        is_generated: false,
        is_unsigned: false,
        comment: None,
        enum_values: None,
        char_max_len: None,
        numeric_precision: None,
        numeric_scale: None,
        collation: None,
        array_elem_type: None,
        extra: None,
        base_type: None,
    }
}

fn table_meta(schema: &str, name: &str, columns: Vec<ColumnMeta>) -> TableMeta {
    let primary_key = columns
        .iter()
        .filter(|c| c.is_pk)
        .map(|c| c.name.clone())
        .collect();
    TableMeta {
        table: TableRef::new(Some(schema), name),
        kind: TableKind::Table,
        columns,
        primary_key,
        indexes: vec![],
        foreign_keys: vec![],
        triggers: vec![],
        comment: None,
        row_estimate: None,
        ddl: None,
        checks: vec![],
        options: None,
    }
}

fn csv_format() -> ExportFormat {
    ExportFormat::Csv {
        delimiter: ",".into(),
        header: true,
        quote: CsvQuote::IfNeeded,
        null_as: String::new(),
        line_break: "\r\n".into(),
        decimal: ".".into(),
        line_break_to_space: false,
    }
}

fn insert_format(table_name: &str, batch_size: u32) -> ExportFormat {
    ExportFormat::SqlInsert {
        table_name: table_name.into(),
        batch_size,
        drop_table: false,
        structure: false,
        content: true,
        gzip: false,
    }
}

fn json_format() -> ExportFormat {
    ExportFormat::Json {
        ndjson: false,
        pretty: false,
    }
}

fn request(
    source: ExportSource,
    format: ExportFormat,
    path: &Path,
    max_rows: Option<u64>,
) -> ExportRequest {
    ExportRequest {
        export_id: "test".into(),
        source,
        format,
        path: path.to_string_lossy().into_owned(),
        max_rows,
        encoding: Default::default(),
    }
}

fn sql_source(sql: &str) -> ExportSource {
    ExportSource::Sql { sql: sql.into() }
}

fn table_source(name: &str) -> ExportSource {
    ExportSource::Table {
        table: TableRef::new(Some("main"), name),
        filter: None,
        sort: vec![],
        columns: None,
        offset: None,
    }
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn int_rows(n: i64) -> Vec<Vec<Value>> {
    (0..n)
        .map(|i| vec![Value::Int(i), Value::Text(format!("row {i}"))])
        .collect()
}

fn id_name_set(n: i64) -> Set {
    (
        vec![
            rcol("id", ValueKind::Int, "INTEGER"),
            rcol("name", ValueKind::Text, "TEXT"),
        ],
        int_rows(n),
    )
}

#[tokio::test]
async fn sql_export_writes_csv_and_reports_progress() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.csv");
    let fake = FakeSession::new(Dialect::Sqlite, vec![id_name_set(3)]);
    let req = request(
        sql_source("select id, name from t;"),
        csv_format(),
        &dest,
        None,
    );
    let mut reports = Vec::new();
    let result = run_export(
        &fake,
        &req,
        None,
        |p| reports.push(p),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let text = std::fs::read_to_string(&dest).unwrap();
    assert_eq!(text, "id,name\r\n0,row 0\r\n1,row 1\r\n2,row 2\r\n");
    assert_eq!(result.rows, 3);
    assert_eq!(result.bytes, text.len() as u64);
    assert_eq!(result.path, dest.to_string_lossy());
    assert_eq!(
        reports.last(),
        Some(&ExportProgress {
            rows: 3,
            bytes: text.len() as u64
        })
    );
    // The statement is streamed without its terminator; no temporary file is left behind.
    assert_eq!(fake.last_sql(), "select id, name from t");
    assert_eq!(files_in(dir.path()), ["out.csv"]);
}

/// Exports are written by the core generators: a session of an extension dialect, or of a driver
/// beyond the core three, is refused before anything runs or is written.
#[tokio::test]
async fn sessions_without_the_core_generators_export_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.csv");
    for (driver, dialect) in [
        (Driver::Postgres, SqlDialect::Redshift),
        (Driver::Libsql, SqlDialect::Sqlite),
    ] {
        let fake = FakeSession {
            driver,
            dialect,
            ..FakeSession::new(Dialect::Sqlite, vec![id_name_set(3)])
        };
        for format in [csv_format(), insert_format("t", 100)] {
            let req = request(sql_source("select id, name from t"), format, &dest, None);
            let err = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
                .await
                .unwrap_err();
            assert!(matches!(err, AppError::Unsupported(_)), "{err:?}");
        }
        assert!(fake.seen.lock().unwrap().is_empty());
    }
    assert!(files_in(dir.path()).is_empty());
}

#[tokio::test]
async fn progress_is_throttled_by_rows() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.ndjson");
    let fake = FakeSession::new(Dialect::Mysql, vec![id_name_set(20_000)]);
    let req = request(
        sql_source("select * from t"),
        ExportFormat::Json {
            ndjson: true,
            pretty: false,
        },
        &dest,
        None,
    );
    let mut reports = Vec::new();
    let result = run_export(
        &fake,
        &req,
        None,
        |p| reports.push(p),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(result.rows, 20_000);
    // Every 5000 rows (or 100 ms) plus the final report; never per chunk of 500.
    assert!(reports.len() >= 5, "{} reports", reports.len());
    assert!(reports.len() <= 40, "{} reports", reports.len());
    assert!(reports.windows(2).all(|w| w[0].rows <= w[1].rows));
    assert_eq!(reports.last().unwrap().rows, 20_000);
    let text = std::fs::read_to_string(&dest).unwrap();
    assert_eq!(text.lines().count(), 20_000);
    assert_eq!(
        text.lines().next().unwrap(),
        "{\"id\":0,\"name\":\"row 0\"}"
    );
}

#[tokio::test]
async fn postgres_table_export_decodes_text_cells() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("t.json");
    let meta = table_meta(
        "public",
        "t",
        vec![
            mcol("id", "bigint", ValueKind::BigInt, true),
            mcol("amount", "numeric(12,2)", ValueKind::Decimal, false),
            mcol("data", "bytea", ValueKind::Bytes, false),
            mcol("doc", "jsonb", ValueKind::Json, false),
            mcol("flag", "boolean", ValueKind::Bool, false),
            mcol("tags", "text[]", ValueKind::Array, false),
            mcol("note", "text", ValueKind::Text, false),
        ],
    );
    let big: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let hex: String = big.iter().map(|b| format!("{b:02x}")).collect();
    let columns: Vec<ResultColumn> = meta
        .columns
        .iter()
        .map(|c| rcol(&c.name, ValueKind::Text, "text"))
        .collect();
    let rows = vec![
        vec![
            Value::Text("1".into()),
            Value::Text("12.50".into()),
            Value::Text(format!("\\x{hex}")),
            Value::Text("{\"a\": 1}".into()),
            Value::Text("t".into()),
            Value::Text("{a,\"b c\",NULL}".into()),
            Value::Text("hi".into()),
        ],
        vec![
            Value::Text("9007199254740993".into()),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Text("f".into()),
            Value::Text("{}".into()),
            Value::Null,
        ],
    ];
    let fake = FakeSession::new(Dialect::Postgres, vec![(columns, rows)]);
    let mut source = table_source("t");
    if let ExportSource::Table { table, .. } = &mut source {
        table.schema = Some("public".into());
    }
    let req = request(source, json_format(), &dest, None);
    let result = run_export(&fake, &req, Some(&meta), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.rows, 2);
    let sql = fake.last_sql();
    assert!(
        sql.starts_with(
            "SELECT \"id\"::text AS \"_tabler_c0\", \"amount\"::text AS \"_tabler_c1\""
        ),
        "{sql}"
    );
    assert!(
        sql.ends_with("FROM \"public\".\"t\" ORDER BY \"id\" ASC"),
        "{sql}"
    );

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&dest).unwrap()).unwrap();
    let rows = json.as_array().unwrap();
    assert_eq!(rows[0]["id"], 1);
    assert_eq!(rows[0]["amount"], "12.50");
    let data = base64::engine::general_purpose::STANDARD
        .decode(rows[0]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(data, big, "bytea is exported in full");
    assert_eq!(rows[0]["doc"], serde_json::json!({"a": 1}));
    assert_eq!(rows[0]["flag"], true);
    assert_eq!(rows[0]["tags"], serde_json::json!(["a", "b c", null]));
    assert_eq!(rows[0]["note"], "hi");
    assert_eq!(rows[1]["id"], "9007199254740993");
    assert_eq!(rows[1]["amount"], serde_json::Value::Null);
    assert_eq!(rows[1]["flag"], false);
    assert_eq!(rows[1]["tags"], serde_json::json!([]));
}

#[tokio::test]
async fn postgres_table_export_types_array_elements() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("arrays.ndjson");
    let array = |name: &str, data_type: &str, elem: &str| {
        let mut c = mcol(name, data_type, ValueKind::Array, false);
        c.array_elem_type = Some(elem.into());
        c
    };
    let meta = table_meta(
        "public",
        "arrays",
        vec![
            mcol("id", "integer", ValueKind::Int, true),
            array("flags", "boolean[]", "bool"),
            array("nums", "integer[]", "int4"),
            array("docs", "jsonb[]", "jsonb"),
            array("blobs", "bytea[]", "bytea"),
            array("words", "text[]", "text"),
        ],
    );
    let big: Vec<u8> = (0..(BYTES_PREVIEW_CAP as u32 + 10))
        .map(|i| (i % 256) as u8)
        .collect();
    let big_hex: String = big.iter().map(|b| format!("{b:02x}")).collect();
    let columns: Vec<ResultColumn> = meta
        .columns
        .iter()
        .map(|c| rcol(&c.name, ValueKind::Text, "text"))
        .collect();
    let rows = vec![vec![
        Value::Text("1".into()),
        Value::Text("{t,f,NULL}".into()),
        Value::Text("{1,-2}".into()),
        Value::Text("{\"{\\\"a\\\": 1}\",NULL}".into()),
        Value::Text(format!("{{\"\\\\x0102\",\"\\\\x{big_hex}\"}}")),
        Value::Text("{x,\"y z\"}".into()),
    ]];
    let fake = FakeSession::new(Dialect::Postgres, vec![(columns, rows)]);
    let mut source = table_source("arrays");
    if let ExportSource::Table { table, .. } = &mut source {
        table.schema = Some("public".into());
    }
    let req = request(
        source,
        ExportFormat::Json {
            ndjson: true,
            pretty: false,
        },
        &dest,
        None,
    );
    run_export(&fake, &req, Some(&meta), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let row: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&dest).unwrap().trim()).unwrap();
    assert_eq!(row["flags"], serde_json::json!([true, false, null]));
    assert_eq!(row["nums"], serde_json::json!([1, -2]));
    assert_eq!(row["docs"], serde_json::json!([{"a": 1}, null]));
    assert_eq!(row["words"], serde_json::json!(["x", "y z"]));
    let blobs = row["blobs"].as_array().unwrap();
    let decode = |v: &serde_json::Value| {
        base64::engine::general_purpose::STANDARD
            .decode(v.as_str().unwrap())
            .unwrap()
    };
    assert_eq!(decode(&blobs[0]), vec![1, 2]);
    assert_eq!(decode(&blobs[1]), big, "bytea elements are not capped");
}

#[tokio::test]
async fn postgres_insert_export_overrides_identity_always() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("t.sql");
    let mut id = mcol("id", "integer", ValueKind::Int, true);
    id.is_auto_increment = true;
    id.extra = Some("ALWAYS".into());
    let meta = table_meta(
        "public",
        "t",
        vec![id, mcol("note", "text", ValueKind::Text, false)],
    );
    let columns = vec![
        rcol("id", ValueKind::Text, "text"),
        rcol("note", ValueKind::Text, "text"),
    ];
    let rows = vec![vec![Value::Text("1".into()), Value::Text("a".into())]];
    let fake = FakeSession::new(Dialect::Postgres, vec![(columns, rows)]);
    let mut source = table_source("t");
    if let ExportSource::Table { table, .. } = &mut source {
        table.schema = Some("public".into());
    }
    let insert = insert_format("", 10);
    let req = request(source.clone(), insert.clone(), &dest, None);
    run_export(&fake, &req, Some(&meta), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "SET TIME ZONE 'UTC';\n\
         INSERT INTO \"t\" (\"id\", \"note\") OVERRIDING SYSTEM VALUE VALUES\n(1, 'a');\n\
         RESET TIME ZONE;\n"
    );

    // An identity BY DEFAULT column takes explicit values without the clause; so does a
    // column list that leaves the identity out.
    let mut by_default = meta.clone();
    by_default.columns[0].extra = Some("BY DEFAULT".into());
    run_export(
        &fake,
        &req,
        Some(&by_default),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(
        !std::fs::read_to_string(&dest)
            .unwrap()
            .contains("OVERRIDING")
    );
    let q = table_query(
        Dialect::Postgres,
        &meta,
        Some(&["note".into()]),
        None,
        &[],
        None,
        0,
        true,
    )
    .unwrap();
    assert!(!q.identity_always);
}

#[tokio::test]
async fn mysql_float_columns_export_their_shortest_text() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("f.csv");
    let meta = table_meta(
        "app",
        "m",
        vec![
            mcol("id", "int", ValueKind::Int, true),
            mcol("f", "float", ValueKind::Float, false),
            mcol("d", "double", ValueKind::Float, false),
        ],
    );
    // Binary protocol (filtered export): FLOAT 1.1 arrives widened from f32.
    let columns = vec![
        rcol("id", ValueKind::Int, "int"),
        rcol("f", ValueKind::Float, "float"),
        rcol("d", ValueKind::Float, "double"),
    ];
    let rows = vec![
        vec![
            Value::Int(1),
            Value::Float(1.1f32 as f64),
            Value::Float(1.1),
        ],
        vec![Value::Int(2), Value::Float(0.3), Value::Float(0.3)],
        vec![Value::Int(3), Value::Null, Value::Float(1e300)],
    ];
    let fake = FakeSession::new(Dialect::Mysql, vec![(columns, rows)]);
    let req = request(
        ExportSource::Table {
            table: TableRef::new(Some("app"), "m"),
            filter: None,
            sort: vec![],
            columns: None,
            offset: None,
        },
        csv_format(),
        &dest,
        None,
    );
    run_export(&fake, &req, Some(&meta), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "id,f,d\r\n1,1.1,1.1\r\n2,0.3,0.3\r\n3,,1e300\r\n"
    );
    assert_eq!(narrow_float(1.1f32 as f64), 1.1);
    assert_eq!(narrow_float(3.4028235e38), 3.4028235e38);
    assert_eq!(narrow_float(-0.0), 0.0);
}

#[tokio::test]
async fn table_export_restores_large_blobs_from_hex() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("blobs.csv");
    // In SQLite any column holds blobs: one declared BLOB, one without a type.
    let meta = table_meta(
        "main",
        "blobs",
        vec![
            mcol("id", "INTEGER", ValueKind::Int, true),
            mcol("data", "BLOB", ValueKind::Bytes, false),
            mcol("misc", "", ValueKind::Other, false),
        ],
    );
    let big: Vec<u8> = (0..(BYTES_PREVIEW_CAP as u32 + 1000))
        .map(|i| (i % 256) as u8)
        .collect();
    let hex: String = big.iter().map(|b| format!("{b:02X}")).collect();
    let columns = vec![
        rcol("id", ValueKind::Int, "INTEGER"),
        rcol("data", ValueKind::Bytes, "BLOB"),
        rcol("misc", ValueKind::Other, ""),
        rcol("_tabler_hex_0", ValueKind::Text, ""),
        rcol("_tabler_hex_1", ValueKind::Text, ""),
        rcol("_tabler_hex_2", ValueKind::Text, ""),
    ];
    let rows = vec![
        vec![
            Value::Int(1),
            Value::from_bytes(&big),
            Value::from_bytes(&big),
            Value::Null,
            Value::Text(hex.clone()),
            Value::Text(hex),
        ],
        vec![
            Value::Int(2),
            Value::from_bytes(b"small"),
            Value::Text("text".into()),
            Value::Null,
            Value::Null,
            Value::Null,
        ],
        vec![
            Value::Int(3),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        ],
    ];
    let fake = FakeSession::new(Dialect::Sqlite, vec![(columns, rows)]).with_meta(meta);
    let req = request(table_source("blobs"), csv_format(), &dest, None);
    // No metadata passed: `run_export` fetches it.
    let result = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.rows, 3);
    let hex_of = |c: &str, n: usize| {
        format!(
            "CASE WHEN typeof(\"{c}\") = 'blob' AND length(\"{c}\") > 65536 THEN hex(\"{c}\") END \
             AS \"_tabler_hex_{n}\""
        )
    };
    assert_eq!(
        fake.last_sql(),
        format!(
            "SELECT \"id\", \"data\", \"misc\", {}, {}, {}\nFROM \"main\".\"blobs\" ORDER BY \"id\" ASC",
            hex_of("id", 0),
            hex_of("data", 1),
            hex_of("misc", 2)
        )
    );
    let mut rdr = csv::Reader::from_path(&dest).unwrap();
    let recs: Vec<csv::StringRecord> = rdr.records().map(|r| r.unwrap()).collect();
    assert_eq!(recs.len(), 3);
    let decode = |s: &str| base64::engine::general_purpose::STANDARD.decode(s).unwrap();
    assert_eq!(decode(&recs[0][1]), big);
    assert_eq!(decode(&recs[0][2]), big, "untyped column restored too");
    assert_eq!(&recs[1][1], "c21hbGw=");
    assert_eq!(&recs[1][2], "text");
    assert_eq!(&recs[2][1], "");
}

#[test]
fn mysql_table_query_adds_hex_for_blobs_only() {
    let meta = table_meta(
        "app",
        "files",
        vec![
            mcol("id", "bigint", ValueKind::BigInt, true),
            mcol("name", "varchar(20)", ValueKind::Text, false),
            mcol("body", "longblob", ValueKind::Bytes, false),
        ],
    );
    let q = table_query(Dialect::Mysql, &meta, None, None, &[], Some(10), 0, false).unwrap();
    assert_eq!(
        q.sql,
        "SELECT `id`, `name`, `body`, CASE WHEN LENGTH(`body`) > 65536 THEN HEX(`body`) END AS \
         `_tabler_hex_0`\nFROM `app`.`files` ORDER BY `id` ASC\nLIMIT 10 OFFSET 0"
    );
    assert_eq!(q.blob_hex, vec![2]);
    let q = table_query(
        Dialect::Mysql,
        &meta,
        Some(&["name".into(), "id".into()]),
        None,
        &[],
        None,
        0,
        false,
    )
    .unwrap();
    assert_eq!(
        q.sql,
        "SELECT `name`, `id`\nFROM `app`.`files` ORDER BY `id` ASC"
    );
    assert!(q.blob_hex.is_empty());
    assert_eq!(q.columns.unwrap()[0].name, "name");
    assert!(table_query(Dialect::Mysql, &meta, Some(&[]), None, &[], None, 0, false).is_err());
    assert!(
        table_query(
            Dialect::Mysql,
            &meta,
            Some(&["nope".into()]),
            None,
            &[],
            None,
            0,
            false
        )
        .is_err()
    );
}

#[tokio::test]
async fn max_rows_stops_the_stream() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.json");
    let fake = FakeSession::new(Dialect::Sqlite, vec![id_name_set(10_000)]);
    let req = request(
        sql_source("select * from t"),
        json_format(),
        &dest,
        Some(1234),
    );
    let result = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.rows, 1234);
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&dest).unwrap()).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1234);

    // Zero rows: header only.
    let dest = dir.path().join("zero.csv");
    let req = request(sql_source("select * from t"), csv_format(), &dest, Some(0));
    let result = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.rows, 0);
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "id,name\r\n");
}

#[tokio::test]
async fn cancel_mid_export_removes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.csv");
    let fake = FakeSession::new(Dialect::Sqlite, vec![id_name_set(50_000)]);
    let req = request(sql_source("select * from t"), csv_format(), &dest, None);
    let token = CancellationToken::new();
    let trigger = token.clone();
    let mut last_rows = 0;
    let err = run_export(
        &fake,
        &req,
        None,
        |p| {
            last_rows = p.rows;
            trigger.cancel();
        },
        token,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
    assert!(last_rows > 0 && last_rows < 50_000);
    assert!(
        files_in(dir.path()).is_empty(),
        "{:?}",
        files_in(dir.path())
    );
    // The sink stopped the stream itself: no driver-level cancel was needed.
    assert_eq!(fake.cancels.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancel_aborts_a_statement_the_driver_would_drain() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.csv");
    let mut fake = FakeSession::new(Dialect::Postgres, vec![id_name_set(50_000)]);
    fake.drain_after_stop = Some(Duration::from_secs(30));
    let req = request(
        sql_source("select * from audit where action = 'delete'"),
        csv_format(),
        &dest,
        None,
    );
    let token = CancellationToken::new();
    let trigger = token.clone();
    let started = Instant::now();
    let err = run_export(&fake, &req, None, |_| trigger.cancel(), token)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(fake.cancels.load(Ordering::SeqCst), 1);
    assert!(files_in(dir.path()).is_empty());

    // Stopping at max_rows is not a cancel: the statement is left to finish.
    fake.drain_after_stop = Some(Duration::from_millis(400));
    let req = request(sql_source("select * from t"), csv_format(), &dest, Some(10));
    let result = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.rows, 10);
    assert_eq!(fake.cancels.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancel_while_the_statement_runs_uses_the_cancel_handle() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.csv");
    let mut fake = FakeSession::new(Dialect::Mysql, vec![]);
    fake.block_until_cancel = true;
    let req = request(sql_source("select sleep(60)"), csv_format(), &dest, None);
    let token = CancellationToken::new();
    let trigger = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        trigger.cancel();
    });
    let started = Instant::now();
    let err = run_export(&fake, &req, None, |_| {}, token)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(fake.cancels.load(Ordering::SeqCst), 1);
    assert!(files_in(dir.path()).is_empty());

    // Cancelled before it starts: nothing runs, nothing is written.
    let token = CancellationToken::new();
    token.cancel();
    let fake = FakeSession::new(Dialect::Mysql, vec![id_name_set(1)]);
    let err = run_export(&fake, &req, None, |_| {}, token)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Cancelled));
    assert!(fake.seen.lock().unwrap().is_empty());
    assert!(files_in(dir.path()).is_empty());
}

#[tokio::test]
async fn rejects_bad_requests_without_touching_files() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.csv");
    let fake = FakeSession::new(Dialect::Postgres, vec![id_name_set(1)]);
    for sql in [
        "select 1; select 2",
        "",
        "  -- just a comment",
        "\\d users",
        "BEGIN",
        "start transaction",
        "COMMIT",
        "SET search_path = other",
        "set role admin",
        "LOCK TABLE t IN ACCESS EXCLUSIVE MODE",
        "LISTEN chan",
        "DELETE FROM t",
        "update t set a = 1",
        "PRAGMA foreign_keys = OFF",
        "PRAGMA table_info(t)",
        "ATTACH DATABASE 'x.db' AS x",
        "DECLARE c CURSOR FOR SELECT 1",
        "CREATE TABLE t2 AS SELECT 1",
    ] {
        let req = request(sql_source(sql), csv_format(), &dest, None);
        let err = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)), "{sql}: {err:?}");
    }
    let req = request(
        sql_source("select 1"),
        ExportFormat::Csv {
            delimiter: "ab".into(),
            header: true,
            quote: CsvQuote::IfNeeded,
            null_as: String::new(),
            line_break: "\n".into(),
            decimal: ".".into(),
            line_break_to_space: false,
        },
        &dest,
        None,
    );
    assert!(
        run_export(&fake, &req, None, |_| {}, CancellationToken::new())
            .await
            .is_err()
    );
    let req = request(
        sql_source("select 1"),
        csv_format(),
        Path::new("rel.csv"),
        None,
    );
    assert!(
        run_export(&fake, &req, None, |_| {}, CancellationToken::new())
            .await
            .is_err()
    );
    assert!(fake.seen.lock().unwrap().is_empty());
    assert!(files_in(dir.path()).is_empty());
}

#[test]
fn query_exports_accept_statements_that_return_rows() {
    for sql in [
        "select 1",
        "  /* c */ (SELECT 1) UNION (SELECT 2)",
        "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
        "values (1), (2)",
        "TABLE t",
        "show search_path",
        "EXPLAIN SELECT 1",
        "describe t",
        "CALL two_sets()",
        "DELETE FROM t WHERE id = 1 RETURNING *",
        "insert into t default values returning id",
    ] {
        let q = sql_query(sql, Dialect::Postgres).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
        assert!(sql.ends_with(&q.sql), "{sql}");
    }
    let err = sql_query("PRAGMA table_info(t)", Dialect::Sqlite).unwrap_err();
    assert!(err.to_string().contains("pragma_"), "{err}");
}

#[tokio::test]
async fn failures_remove_the_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.json");
    std::fs::write(&dest, "previous").unwrap();

    // A capped blob from a query export fails instead of writing truncated data.
    let big = vec![7u8; BYTES_PREVIEW_CAP + 1];
    let fake = FakeSession::new(
        Dialect::Mysql,
        vec![(
            vec![rcol("b", ValueKind::Bytes, "blob")],
            vec![vec![Value::from_bytes(&big)]],
        )],
    );
    let req = request(sql_source("select b from t"), json_format(), &dest, None);
    let err = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Unsupported(_)), "{err:?}");

    // A statement without a result set.
    let fake = FakeSession::new(Dialect::Mysql, vec![]);
    let req = request(sql_source("call no_sets()"), json_format(), &dest, None);
    let err = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::InvalidInput(_)), "{err:?}");

    // The existing file is untouched and nothing else is left behind.
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "previous");
    assert_eq!(files_in(dir.path()), ["out.json"]);
}

#[tokio::test]
async fn exports_only_the_first_result_set_and_replaces_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.csv");
    std::fs::write(&dest, "previous").unwrap();
    let fake = FakeSession::new(
        Dialect::Mysql,
        vec![
            id_name_set(2),
            (
                vec![rcol("x", ValueKind::Int, "int")],
                vec![vec![Value::Int(9)]],
            ),
        ],
    );
    let req = request(sql_source("call two_sets()"), csv_format(), &dest, None);
    let result = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.rows, 2);
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "id,name\r\n0,row 0\r\n1,row 1\r\n"
    );
    assert_eq!(files_in(dir.path()), ["out.csv"]);
}

#[tokio::test]
async fn insert_export_of_a_table_skips_generated_columns() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.sql");
    let mut total = mcol("total", "INTEGER", ValueKind::Int, false);
    total.is_generated = true;
    let meta = table_meta(
        "main",
        "items",
        vec![
            mcol("id", "INTEGER", ValueKind::Int, true),
            mcol("name", "TEXT", ValueKind::Text, false),
            total,
        ],
    );
    let fake = FakeSession::new(
        Dialect::Sqlite,
        vec![(
            vec![
                rcol("id", ValueKind::Int, "INTEGER"),
                rcol("name", ValueKind::Text, "TEXT"),
                rcol("_tabler_hex_0", ValueKind::Text, ""),
                rcol("_tabler_hex_1", ValueKind::Text, ""),
            ],
            vec![
                vec![
                    Value::Int(1),
                    Value::Text("a'b".into()),
                    Value::Null,
                    Value::Null,
                ],
                vec![Value::Int(2), Value::Null, Value::Null, Value::Null],
                vec![
                    Value::Int(3),
                    Value::Text("c".into()),
                    Value::Null,
                    Value::Null,
                ],
            ],
        )],
    );
    let req = request(table_source("items"), insert_format("", 2), &dest, None);
    run_export(&fake, &req, Some(&meta), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let sql = fake.last_sql();
    assert!(sql.starts_with("SELECT \"id\", \"name\", CASE"), "{sql}");
    assert!(!sql.contains("total"), "{sql}");
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "INSERT INTO \"items\" (\"id\", \"name\") VALUES\n(1, 'a''b'),\n(2, NULL);\n\
         INSERT INTO \"items\" (\"id\", \"name\") VALUES\n(3, 'c');\n"
    );
}

fn rows_source(set: Set, table: Option<&str>) -> ExportSource {
    ExportSource::Rows {
        columns: set.0,
        rows: set.1,
        table: table.map(str::to_string),
    }
}

#[tokio::test]
async fn rows_export_writes_the_given_rows_without_running_anything() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeSession::new(Dialect::Sqlite, vec![]);

    // CSV.
    let dest = dir.path().join("result.csv");
    let req = request(
        rows_source(id_name_set(3), Some("t")),
        csv_format(),
        &dest,
        None,
    );
    let mut reports = Vec::new();
    let result = run_export(
        &fake,
        &req,
        None,
        |p| reports.push(p),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let text = std::fs::read_to_string(&dest).unwrap();
    assert_eq!(text, "id,name\r\n0,row 0\r\n1,row 1\r\n2,row 2\r\n");
    assert_eq!(result.rows, 3);
    assert_eq!(
        reports.last(),
        Some(&ExportProgress {
            rows: 3,
            bytes: text.len() as u64
        })
    );

    // JSON, with NULLs and a row cap.
    let dest = dir.path().join("result.json");
    let mut set = id_name_set(3);
    set.1[1][1] = Value::Null;
    let req = request(rows_source(set, None), json_format(), &dest, Some(2));
    let result = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.rows, 2);
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&dest).unwrap()).unwrap();
    assert_eq!(
        json,
        serde_json::json!([{"id": 0, "name": "row 0"}, {"id": 1, "name": null}])
    );

    // INSERT statements into the result's table by default…
    let dest = dir.path().join("result.sql");
    let insert = |table_name: &str| insert_format(table_name, 2);
    let req = request(
        rows_source(id_name_set(3), Some("users")),
        insert(""),
        &dest,
        None,
    );
    run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES\n(0, 'row 0'),\n(1, 'row 1');\n\
         INSERT INTO \"users\" (\"id\", \"name\") VALUES\n(2, 'row 2');\n"
    );
    // …or the one the user named; without either the INSERTs go into `untitled`.
    let req = request(
        rows_source(id_name_set(1), Some("users")),
        insert("archive"),
        &dest,
        None,
    );
    run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert!(
        std::fs::read_to_string(&dest)
            .unwrap()
            .starts_with("INSERT INTO \"archive\" ")
    );
    let req = request(rows_source(id_name_set(1), None), insert(""), &dest, None);
    run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert!(
        std::fs::read_to_string(&dest)
            .unwrap()
            .starts_with("INSERT INTO \"untitled\" (")
    );

    // The database was never asked for anything; no temporary file is left behind.
    assert!(fake.seen.lock().unwrap().is_empty());
    assert_eq!(
        files_in(dir.path()),
        ["result.csv", "result.json", "result.sql"]
    );
}

#[tokio::test]
async fn rows_export_keeps_postgres_values_as_they_are() {
    // A PostgreSQL query export decodes text cells by type; the editor's rows are values
    // already (a text value in an int column stays text).
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.json");
    let fake = FakeSession::new(Dialect::Postgres, vec![]);
    let set = (
        vec![
            rcol("n", ValueKind::Int, "int4"),
            rcol("d", ValueKind::Decimal, "numeric"),
        ],
        vec![vec![
            Value::Text("7".into()),
            Value::Decimal("12.50".into()),
        ]],
    );
    let req = request(rows_source(set, None), json_format(), &dest, None);
    run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&dest).unwrap()).unwrap();
    assert_eq!(json, serde_json::json!([{"n": "7", "d": "12.50"}]));
}

#[tokio::test]
async fn rows_export_refuses_truncated_binary_values() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.csv");
    let fake = FakeSession::new(Dialect::Sqlite, vec![]);
    let capped = Value::Bytes(BytesValue {
        base64: String::new(),
        len: (BYTES_PREVIEW_CAP + 1) as u64,
        truncated: true,
    });
    let set = (
        vec![
            rcol("id", ValueKind::Int, "INTEGER"),
            rcol("data", ValueKind::Bytes, "BLOB"),
        ],
        vec![
            vec![Value::Int(1), Value::Null],
            vec![Value::Int(2), capped],
        ],
    );
    let req = request(rows_source(set, None), csv_format(), &dest, None);
    let err = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap_err();
    // SQLite (and MySQL) cap blobs in query exports too: only a table export reads them in full.
    match err {
        AppError::InvalidInput(msg) => assert_eq!(
            msg,
            "a binary value in this result was truncated to 64 KiB; export its table instead"
        ),
        other => panic!("unexpected {other:?}"),
    }
    // A PostgreSQL query export decodes bytea in full.
    let pg = FakeSession::new(Dialect::Postgres, vec![]);
    let err = run_export(&pg, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap_err();
    match err {
        AppError::InvalidInput(msg) => assert_eq!(
            msg,
            "a binary value in this result was truncated to 64 KiB; export the query instead"
        ),
        other => panic!("unexpected {other:?}"),
    }
    // A ragged row is refused too; neither leaves a file.
    let set = (
        vec![rcol("id", ValueKind::Int, "INTEGER")],
        vec![vec![Value::Int(1), Value::Int(2)]],
    );
    let req = request(rows_source(set, None), csv_format(), &dest, None);
    let err = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::InvalidInput(_)), "{err:?}");
    assert!(files_in(dir.path()).is_empty());
}

#[tokio::test]
async fn rows_export_can_be_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.csv");
    let fake = FakeSession::new(Dialect::Sqlite, vec![]);
    let token = CancellationToken::new();
    let req = request(
        rows_source(id_name_set(20_000), None),
        csv_format(),
        &dest,
        None,
    );
    let mut reports = 0;
    let cancel = token.clone();
    let err = run_export(
        &fake,
        &req,
        None,
        |_| {
            // Cancel once the first chunk has been written.
            reports += 1;
            cancel.cancel();
        },
        token,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
    assert!(reports >= 1);
    assert!(files_in(dir.path()).is_empty());
}

// ---- SQL scripts, Excel, multi-table ---------------------------------------------------------

/// What a SQLite table export of `id, name` streams: the values plus the blob hex columns.
fn lite_set(n: i64) -> Set {
    (
        vec![
            rcol("id", ValueKind::Int, "INTEGER"),
            rcol("name", ValueKind::Text, "TEXT"),
            rcol("_tabler_hex_0", ValueKind::Text, ""),
            rcol("_tabler_hex_1", ValueKind::Text, ""),
        ],
        (0..n)
            .map(|i| {
                vec![
                    Value::Int(i + 1),
                    Value::Text(format!("n{i}")),
                    Value::Null,
                    Value::Null,
                ]
            })
            .collect(),
    )
}

fn lite_meta(name: &str, references: Option<&str>) -> TableMeta {
    let mut meta = table_meta(
        "main",
        name,
        vec![
            mcol("id", "INTEGER", ValueKind::Int, true),
            mcol("name", "TEXT", ValueKind::Text, false),
        ],
    );
    meta.ddl = Some(match references {
        Some(parent) => format!(
            "CREATE TABLE {name} (id INTEGER PRIMARY KEY REFERENCES {parent}(id), name TEXT)"
        ),
        None => format!("CREATE TABLE {name} (id INTEGER PRIMARY KEY, name TEXT)"),
    });
    if let Some(parent) = references {
        meta.foreign_keys = vec![
            serde_json::from_value(serde_json::json!({
                "name": "fk", "columns": ["id"],
                "refTable": {"schema": "main", "name": parent}, "refColumns": ["id"],
                "onUpdate": "NO ACTION", "onDelete": "NO ACTION"
            }))
            .unwrap(),
        ];
    }
    meta
}

fn script(drop_table: bool, structure: bool, content: bool, gzip: bool) -> ExportFormat {
    ExportFormat::SqlInsert {
        table_name: String::new(),
        batch_size: 2,
        drop_table,
        structure,
        content,
        gzip,
    }
}

fn gunzip(path: &Path) -> String {
    use std::io::Read as _;
    let mut out = String::new();
    flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap())
        .read_to_string(&mut out)
        .unwrap();
    out
}

#[tokio::test]
async fn sql_export_with_structure_drop_and_gzip() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("items.sql.gz");
    let mut meta = lite_meta("items", None);
    meta.indexes = vec![
        serde_json::from_value(serde_json::json!({
            "name": "items_name", "columns": ["name"], "unique": false, "primary": false,
            "definition": "CREATE INDEX items_name ON items (name)"
        }))
        .unwrap(),
    ];
    let fake = FakeSession::new(Dialect::Sqlite, vec![lite_set(3)]);
    let req = request(
        table_source("items"),
        script(true, true, true, true),
        &dest,
        None,
    );
    let result = run_export(&fake, &req, Some(&meta), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.rows, 3);
    assert_eq!(result.bytes, std::fs::metadata(&dest).unwrap().len());
    let text = gunzip(&dest);
    assert_eq!(
        text,
        "DROP TABLE IF EXISTS \"items\";\n\
         CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);\n\
         CREATE INDEX items_name ON items (name);\n\n\
         INSERT INTO \"items\" (\"id\", \"name\") VALUES\n(1, 'n0'),\n(2, 'n1');\n\
         INSERT INTO \"items\" (\"id\", \"name\") VALUES\n(3, 'n2');\n"
    );
    // The script loads, also over an existing table.
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch(&text).unwrap();
    db.execute_batch(&text).unwrap();
    let n: i64 = db
        .query_row("SELECT count(*) FROM items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 3);

    // Structure only: nothing is queried.
    let dest = dir.path().join("structure.sql");
    let fake = FakeSession::new(Dialect::Sqlite, vec![lite_set(3)]);
    let req = request(
        table_source("items"),
        script(false, true, false, false),
        &dest,
        None,
    );
    let result = run_export(&fake, &req, Some(&meta), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.rows, 0);
    assert!(fake.seen.lock().unwrap().is_empty());
    assert!(
        std::fs::read_to_string(&dest)
            .unwrap()
            .starts_with("CREATE TABLE items")
    );
    // Neither structure nor content: refused.
    let req = request(
        table_source("items"),
        script(true, false, false, false),
        &dest,
        None,
    );
    let req_nothing = request(
        table_source("items"),
        script(false, false, false, false),
        &dest,
        None,
    );
    assert!(
        run_export(&fake, &req, Some(&meta), |_| {}, CancellationToken::new())
            .await
            .is_ok()
    );
    assert!(
        run_export(
            &fake,
            &req_nothing,
            Some(&meta),
            |_| {},
            CancellationToken::new()
        )
        .await
        .is_err()
    );

    // MySQL: the time zone is set and foreign key checks are off while the script loads, both
    // restored at its end (the fake session cannot tell its zone: UTC is assumed).
    let dest = dir.path().join("my.sql");
    let mut my = table_meta(
        "app",
        "items",
        vec![
            mcol("id", "int", ValueKind::Int, true),
            mcol("name", "varchar(20)", ValueKind::Text, false),
        ],
    );
    my.ddl = Some("CREATE TABLE `items` (\n  `id` int NOT NULL\n)".into());
    let fake = FakeSession::new(Dialect::Mysql, vec![id_name_set(1)]);
    let mut source = table_source("items");
    if let ExportSource::Table { table, .. } = &mut source {
        table.schema = Some("app".into());
    }
    let req = request(source, script(true, true, true, false), &dest, None);
    run_export(&fake, &req, Some(&my), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "SET @OLD_TIME_ZONE=@@TIME_ZONE;\n\
         SET TIME_ZONE='+00:00';\n\
         SET @OLD_FOREIGN_KEY_CHECKS = @@FOREIGN_KEY_CHECKS, FOREIGN_KEY_CHECKS = 0;\n\
         DROP TABLE IF EXISTS `items`;\n\
         CREATE TABLE `items` (\n  `id` int NOT NULL\n);\n\n\
         INSERT INTO `items` (`id`, `name`) VALUES\n(0, 'row 0');\n\
         SET FOREIGN_KEY_CHECKS = @OLD_FOREIGN_KEY_CHECKS;\n\
         SET TIME_ZONE=@OLD_TIME_ZONE;\n"
    );
}

#[tokio::test]
async fn xlsx_export_of_a_query() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("q.xlsx");
    let fake = FakeSession::new(Dialect::Sqlite, vec![id_name_set(2)]);
    let req = request(
        sql_source("select id, name from t"),
        ExportFormat::Xlsx { header: true },
        &dest,
        None,
    );
    let result = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.rows, 2);
    let files = crate::export::xlsx::tests::unzip(&std::fs::read(&dest).unwrap());
    let sheet = String::from_utf8(files["xl/worksheets/sheet1.xml"].clone()).unwrap();
    assert!(sheet.contains("<c r=\"A3\"><v>1</v></c>"), "{sheet}");
    assert!(sheet.contains(">row 1</t>"));
    let wb = String::from_utf8(files["xl/workbook.xml"].clone()).unwrap();
    assert!(wb.contains("name=\"Result\""));
}

#[tokio::test]
async fn html_and_encoded_exports_of_a_query() {
    use crate::types::export::ExportEncoding;
    let dir = tempfile::tempdir().unwrap();
    let set = (
        vec![
            rcol("id", ValueKind::Int, "int"),
            rcol("name", ValueKind::Text, "text"),
        ],
        vec![
            vec![Value::Int(1), Value::Text("Zoë <b>&</b> €".into())],
            vec![Value::Int(2), Value::Text("世界".into())],
            vec![Value::Int(3), Value::Null],
        ],
    );

    // HTML: escaped, NULL marked, numbers right-aligned.
    let dest = dir.path().join("q.html");
    let fake = FakeSession::new(Dialect::Sqlite, vec![set.clone()]);
    let req = request(sql_source("select 1"), ExportFormat::Html, &dest, None);
    let result = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.rows, 3);
    assert_eq!(result.replaced, None);
    let html = std::fs::read_to_string(&dest).unwrap();
    assert!(html.contains("<meta charset=\"utf-8\">"));
    assert!(html.contains("<th>id</th><th>name</th>"));
    assert!(
        html.contains("<td>Zoë &lt;b&gt;&amp;&lt;/b&gt; €</td>"),
        "{html}"
    );
    assert!(html.contains("<td class=\"null\">NULL</td>"));

    // CSV in Windows-1252: what it cannot hold becomes `?`, and is counted.
    let dest = dir.path().join("q.csv");
    let fake = FakeSession::new(Dialect::Sqlite, vec![set.clone()]);
    let mut req = request(sql_source("select 1"), csv_format(), &dest, None);
    req.encoding = ExportEncoding::Windows1252;
    let result = run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.replaced, Some(2));
    let bytes = std::fs::read(&dest).unwrap();
    assert_eq!(
        bytes,
        b"id,name\r\n1,Zo\xEB <b>&</b> \x80\r\n2,??\r\n3,\r\n".to_vec()
    );
    assert_eq!(result.bytes, bytes.len() as u64);

    // HTML in Latin-1 declares it; JSON in UTF-8 with a BOM starts with one.
    let dest = dir.path().join("l.html");
    let fake = FakeSession::new(Dialect::Sqlite, vec![set.clone()]);
    let mut req = request(sql_source("select 1"), ExportFormat::Html, &dest, None);
    req.encoding = ExportEncoding::Latin1;
    run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let bytes = std::fs::read(&dest).unwrap();
    let text: String = bytes.iter().map(|&b| b as char).collect();
    assert!(text.contains("<meta charset=\"iso-8859-1\">"));
    assert!(text.contains("Zo\u{EB} &lt;b&gt;"));
    let dest = dir.path().join("q.json");
    let fake = FakeSession::new(Dialect::Sqlite, vec![set.clone()]);
    let mut req = request(sql_source("select 1"), json_format(), &dest, None);
    req.encoding = ExportEncoding::Utf8Bom;
    run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let bytes = std::fs::read(&dest).unwrap();
    assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
    let parsed: serde_json::Value = serde_json::from_slice(&bytes[3..]).unwrap();
    assert_eq!(parsed[1]["name"], "世界");

    // A UTF-16 SQL script reads back; a workbook is never transcoded.
    let dest = dir.path().join("q.sql");
    let fake = FakeSession::new(Dialect::Sqlite, vec![set.clone()]);
    let mut req = request(sql_source("select 1"), insert_format("t", 10), &dest, None);
    req.encoding = ExportEncoding::Utf16Le;
    run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let bytes = std::fs::read(&dest).unwrap();
    assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
    let units: Vec<u16> = bytes[2..]
        .chunks(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect();
    let sql = String::from_utf16(&units).unwrap();
    assert!(sql.contains("(2, '世界')"), "{sql}");
    let dest = dir.path().join("q.xlsx");
    let fake = FakeSession::new(Dialect::Sqlite, vec![set]);
    let mut req = request(
        sql_source("select 1"),
        ExportFormat::Xlsx { header: true },
        &dest,
        None,
    );
    req.encoding = ExportEncoding::Utf16Le;
    run_export(&fake, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert!(
        crate::export::xlsx::tests::unzip(&std::fs::read(&dest).unwrap())
            .contains_key("xl/workbook.xml")
    );
}

fn tables_request(
    tables: &[&str],
    format: ExportFormat,
    layout: ExportLayout,
    target: &Path,
) -> ExportTablesRequest {
    ExportTablesRequest {
        export_id: "multi".into(),
        tables: tables
            .iter()
            .map(|t| TableRef::new(Some("main"), t))
            .collect(),
        format,
        layout,
        target: target.to_string_lossy().into_owned(),
        encoding: Default::default(),
        database: None,
        create_folder: false,
    }
}

#[tokio::test]
async fn multi_table_exports() {
    use crate::export::tables::run_export_tables;
    let dir = tempfile::tempdir().unwrap();
    // `child` references `parent`; requested child first.
    let metas = vec![
        Arc::new(lite_meta("child", Some("parent"))),
        Arc::new(lite_meta("parent", None)),
    ];

    // One CSV per table, with progress per table.
    let folder = dir.path().join("csv");
    std::fs::create_dir(&folder).unwrap();
    let fake = FakeSession::new(Dialect::Sqlite, vec![lite_set(3)]);
    let mut seen = Vec::new();
    let result = run_export_tables(
        &fake,
        &tables_request(
            &["child", "parent"],
            csv_format(),
            ExportLayout::FilePerTable,
            &folder,
        ),
        &metas,
        |p| seen.push(p),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(files_in(&folder), ["child.csv", "parent.csv"]);
    assert_eq!(result.rows, 6);
    assert_eq!(result.paths.len(), 2);
    assert_eq!(result.tables[0].table.name, "child");
    assert_eq!(result.tables[1].rows, 3);
    assert_eq!(
        std::fs::read_to_string(folder.join("parent.csv")).unwrap(),
        "id,name\r\n1,n0\r\n2,n1\r\n3,n2\r\n"
    );
    assert!(
        seen.iter()
            .any(|p| p.table_index == 1 && p.table == "parent")
    );
    assert!(seen.iter().all(|p| p.table_count == 2));

    // One SQL script: parents first, loadable.
    let file = dir.path().join("all.sql");
    let fake = FakeSession::new(Dialect::Sqlite, vec![lite_set(2)]);
    run_export_tables(
        &fake,
        &tables_request(
            &["child", "parent"],
            script(true, true, true, false),
            ExportLayout::SingleFile,
            &file,
        ),
        &metas,
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let parent_at = text.find("CREATE TABLE parent").unwrap();
    let child_at = text.find("CREATE TABLE child").unwrap();
    assert!(parent_at < child_at, "{text}");
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch(&text).unwrap();
    let n: i64 = db
        .query_row("SELECT count(*) FROM child", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2);

    // One workbook, a sheet per table in the order asked.
    let book = dir.path().join("all.xlsx");
    let fake = FakeSession::new(Dialect::Sqlite, vec![lite_set(1)]);
    run_export_tables(
        &fake,
        &tables_request(
            &["child", "parent"],
            ExportFormat::Xlsx { header: true },
            ExportLayout::SingleFile,
            &book,
        ),
        &metas,
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let files = crate::export::xlsx::tests::unzip(&std::fs::read(&book).unwrap());
    let wb = String::from_utf8(files["xl/workbook.xml"].clone()).unwrap();
    let (c, p) = (
        wb.find("name=\"child\"").unwrap(),
        wb.find("name=\"parent\"").unwrap(),
    );
    assert!(c < p);
    assert!(files.contains_key("xl/worksheets/sheet2.xml"));

    // One HTML page with a table per table, in the order asked; or a page per table in a
    // folder the export creates.
    let page = dir.path().join("all.html");
    let fake = FakeSession::new(Dialect::Sqlite, vec![lite_set(2)]);
    let result = run_export_tables(
        &fake,
        &tables_request(
            &["child", "parent"],
            ExportFormat::Html,
            ExportLayout::SingleFile,
            &page,
        ),
        &metas,
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(result.rows, 4);
    let html = std::fs::read_to_string(&page).unwrap();
    assert_eq!(html.matches("<table>").count(), 2);
    let (c, p) = (
        html.find("<h2>main.child</h2>").unwrap(),
        html.find("<h2>main.parent</h2>").unwrap(),
    );
    assert!(c < p);
    assert!(html.contains("<title>all</title>"));
    let folder = dir.path().join("db").join("html");
    let fake = FakeSession::new(Dialect::Sqlite, vec![lite_set(1)]);
    let mut req = tables_request(
        &["child", "parent"],
        ExportFormat::Html,
        ExportLayout::FilePerTable,
        &folder,
    );
    req.create_folder = true;
    run_export_tables(&fake, &req, &metas, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(files_in(&folder), ["child.html", "parent.html"]);
    assert!(
        std::fs::read_to_string(folder.join("parent.html"))
            .unwrap()
            .contains("<title>parent</title>")
    );

    // Cancelled: no file at all, not even the first table's.
    let folder = dir.path().join("cancelled");
    std::fs::create_dir(&folder).unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let fake = FakeSession::new(Dialect::Sqlite, vec![lite_set(3)]);
    let err = run_export_tables(
        &fake,
        &tables_request(
            &["child", "parent"],
            json_format(),
            ExportLayout::FilePerTable,
            &folder,
        ),
        &metas,
        |_| {},
        cancel,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Cancelled));
    assert!(files_in(&folder).is_empty());
}

// ---- Script layout: DROPs, tables, rows, then keys and triggers ------------------------------

fn fk(name: &str, columns: &[&str], ref_schema: &str, ref_table: &str) -> ForeignKeyMeta {
    ForeignKeyMeta {
        name: name.into(),
        columns: columns.iter().map(|c| c.to_string()).collect(),
        ref_table: TableRef::new(Some(ref_schema), ref_table),
        ref_columns: vec!["id".into()],
        on_update: "NO ACTION".into(),
        on_delete: "NO ACTION".into(),
        definition: None,
    }
}

#[tokio::test]
async fn postgres_script_loads_keys_triggers_and_sequences_after_the_rows() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("emp.sql");
    let mut id = mcol("id", "integer", ValueKind::Int, true);
    id.is_auto_increment = true;
    id.extra = Some("BY DEFAULT".into());
    let mut meta = table_meta(
        "public",
        "emp",
        vec![id, mcol("boss", "integer", ValueKind::Int, false)],
    );
    // a key referencing the table itself: a row may point at a later row
    meta.foreign_keys = vec![fk("emp_boss_fk", &["boss"], "public", "emp")];
    meta.triggers = vec![TriggerMeta {
        name: "emp_audit".into(),
        timing: Some("AFTER".into()),
        events: vec!["INSERT".into()],
        enabled: true,
        definition: "CREATE TRIGGER emp_audit AFTER INSERT ON public.emp FOR EACH ROW EXECUTE FUNCTION audit()".into(),
    }];
    let columns = vec![
        rcol("id", ValueKind::Text, "text"),
        rcol("boss", ValueKind::Text, "text"),
    ];
    let rows = vec![
        vec![Value::Text("1".into()), Value::Text("2".into())],
        vec![Value::Text("2".into()), Value::Null],
    ];
    // (the fake answers no catalog query: the script has what `TableMeta` says)
    let fake = FakeSession::new(Dialect::Postgres, vec![(columns, rows)]);
    let mut source = table_source("emp");
    if let ExportSource::Table { table, .. } = &mut source {
        table.schema = Some("public".into());
    }
    let req = request(source, script(true, true, true, false), &dest, None);
    run_export(&fake, &req, Some(&meta), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "SET TIME ZONE 'UTC';\n\
         DROP TABLE IF EXISTS \"public\".\"emp\";\n\
         CREATE TABLE \"public\".\"emp\" (\n  \"id\" integer GENERATED BY DEFAULT AS IDENTITY NOT NULL,\n  \"boss\" integer,\n  PRIMARY KEY (\"id\")\n);\n\n\
         INSERT INTO \"public\".\"emp\" (\"id\", \"boss\") VALUES\n(1, 2),\n(2, NULL);\n\n\
         SELECT setval(pg_get_serial_sequence('\"public\".\"emp\"', 'id'), GREATEST(MAX(\"id\"), 1)) FROM \"public\".\"emp\" HAVING MAX(\"id\") IS NOT NULL;\n\n\
         ALTER TABLE \"public\".\"emp\" ADD CONSTRAINT \"emp_boss_fk\" FOREIGN KEY (\"boss\") REFERENCES \"public\".\"emp\" (\"id\");\n\n\
         CREATE TRIGGER emp_audit AFTER INSERT ON public.emp FOR EACH ROW EXECUTE FUNCTION audit();\n\
         RESET TIME ZONE;\n"
    );

    // Structure only: no reset (there are no rows), the keys right after the table.
    let req = request(
        table_source("emp"),
        script(false, true, false, false),
        &dest,
        None,
    );
    run_export(&fake, &req, Some(&meta), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let text = std::fs::read_to_string(&dest).unwrap();
    assert!(!text.contains("setval"), "{text}");
    assert!(
        text.contains(");\n\nALTER TABLE \"public\".\"emp\" ADD CONSTRAINT"),
        "{text}"
    );
}

#[test]
fn script_layout_orders_like_pg_dump_and_loads_partitions_once() {
    use crate::export::structure::{ScriptTable, script_layout};
    use crate::sqlgen::ddl::{PgPartition, PgTableContext};

    let col = |name: &str| mcol(name, "integer", ValueKind::Int, name == "id");
    // `events` is partitioned: `events_2024` is exported too, `events_2025` and the
    // sub-partitioned `events_eu` (with `events_eu_1`) are not.
    let mut events = table_meta("public", "events", vec![col("id"), col("ts")]);
    events.kind = TableKind::Partitioned;
    let events_2024 = table_meta("public", "events_2024", vec![col("id"), col("ts")]);
    let mut audit = table_meta("public", "audit", vec![col("id"), col("event_id")]);
    audit.foreign_keys = vec![fk("audit_event_fk", &["event_id"], "public", "events")];
    let part = |name: &str, parent: &str, bound: &str, key: Option<&str>| PgPartition {
        table: TableRef::new(Some("public"), name),
        parent: TableRef::new(Some("public"), parent),
        bound: bound.into(),
        key: key.map(str::to_string),
        objects: Vec::new(),
    };
    let events_context = PgTableContext {
        partition_key: Some("RANGE (ts)".into()),
        partitions: vec![
            part("events_2024", "events", "FOR VALUES FROM (0) TO (10)", None),
            part(
                "events_2025",
                "events",
                "FOR VALUES FROM (10) TO (20)",
                None,
            ),
            part(
                "events_eu",
                "events",
                "FOR VALUES FROM (20) TO (30)",
                Some("LIST (id)"),
            ),
            part("events_eu_1", "events_eu", "FOR VALUES IN (1)", None),
        ],
        ..PgTableContext::default()
    };
    let child_context = PgTableContext {
        partition_bound: Some("FOR VALUES FROM (0) TO (10)".into()),
        ancestors: vec![TableRef::new(Some("public"), "events")],
        ..PgTableContext::default()
    };
    // requested: the partition, the referencing table, then the partitioned one
    let tables = [
        ScriptTable {
            meta: &events_2024,
            context: child_context,
        },
        ScriptTable {
            meta: &audit,
            context: PgTableContext::default(),
        },
        ScriptTable {
            meta: &events,
            context: events_context,
        },
    ];
    let layout = script_layout(
        Dialect::Postgres,
        &tables,
        SqlScript {
            drop_table: true,
            structure: true,
            content: true,
            gzip: false,
        },
        true,
        None,
    );
    // referenced and partitioned tables first
    assert_eq!(layout.order, [2, 0, 1]);
    // the partition's rows come through its partitioned table
    assert_eq!(layout.rows, [false, true, true]);
    let text = &layout.opening;
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing in\n{text}"))
    };
    // DROPs first, dependants first
    assert!(
        at("DROP TABLE IF EXISTS \"public\".\"audit\"")
            < at("DROP TABLE IF EXISTS \"public\".\"events\";")
    );
    assert!(
        at("DROP TABLE IF EXISTS \"public\".\"events_2024\"")
            < at("DROP TABLE IF EXISTS \"public\".\"events\";")
    );
    assert!(at("DROP TABLE IF EXISTS \"public\".\"events\";") < at("CREATE TABLE"));
    // the partitioned table with the partitions not exported on their own, then the partition
    // as a table of its own, attached
    assert!(
        at(") PARTITION BY RANGE (ts);") < at("\"events_2025\" PARTITION OF \"public\".\"events\"")
    );
    assert!(
        at(
            "\"events_eu\" PARTITION OF \"public\".\"events\" FOR VALUES FROM (20) TO (30) PARTITION BY LIST (id);"
        ) < at("\"events_eu_1\" PARTITION OF \"public\".\"events_eu\"")
    );
    assert!(!text.contains("\"events_2024\" PARTITION OF"));
    assert!(
        at("CREATE TABLE \"public\".\"events_2024\" (")
            < at("ATTACH PARTITION \"public\".\"events_2024\" FOR VALUES FROM (0) TO (10);")
    );
    assert!(at("ATTACH PARTITION") < at("CREATE TABLE \"public\".\"audit\""));
    // keys after the rows
    assert!(!text.contains("FOREIGN KEY"));
    assert_eq!(layout.closing.len(), 1);
    assert!(
        layout.closing[0]
            .starts_with("ALTER TABLE \"public\".\"audit\" ADD CONSTRAINT \"audit_event_fk\"")
    );
}

#[test]
fn script_layout_recreates_the_own_indexes_and_triggers_of_partitions() {
    use crate::export::structure::{ScriptTable, script_layout};
    use crate::sqlgen::ddl::{
        PgPartition, PgPartitionObject, PgPartitionObjectKind, PgTableContext,
    };

    let col = |name: &str| mcol(name, "integer", ValueKind::Int, false);
    let object = |kind, name: &str, definition: &str, disabled| PgPartitionObject {
        kind,
        name: name.into(),
        definition: definition.into(),
        disabled,
    };
    // `events` (exported) has the partitioned index `events_ts_idx`. Its partitions are
    // recreated with it: `events_2025` with an index, a UNIQUE constraint and a disabled trigger
    // of its own; the sub-partitioned `events_eu` with an index of its own (which reaches
    // `events_eu_1`) and `events_eu_1` with another one. `events_2024` is exported on its own.
    let mut events = table_meta("public", "events", vec![col("id"), col("ts")]);
    events.kind = TableKind::Partitioned;
    events.indexes = vec![crate::types::schema::IndexMeta {
        name: "events_ts_idx".into(),
        columns: vec!["ts".into()],
        unique: false,
        primary: false,
        method: Some("btree".into()),
        predicate: None,
        definition: Some(
            "CREATE INDEX events_ts_idx ON ONLY public.events USING btree (ts)".into(),
        ),
        comment: None,
        include: vec![],
    }];
    events.triggers = vec![TriggerMeta {
        name: "events_audit".into(),
        timing: Some("AFTER".into()),
        events: vec!["INSERT".into()],
        enabled: true,
        definition: "CREATE TRIGGER events_audit AFTER INSERT ON public.events FOR EACH ROW EXECUTE FUNCTION audit()".into(),
    }];
    let events_2024 = table_meta("public", "events_2024", vec![col("id"), col("ts")]);
    let partition =
        |name: &str, parent: &str, bound: &str, key: Option<&str>, objects| PgPartition {
            table: TableRef::new(Some("public"), name),
            parent: TableRef::new(Some("public"), parent),
            bound: bound.into(),
            key: key.map(str::to_string),
            objects,
        };
    let events_context = PgTableContext {
        partition_key: Some("RANGE (ts)".into()),
        partitions: vec![
            partition(
                "events_2024",
                "events",
                "FOR VALUES FROM (0) TO (10)",
                None,
                vec![object(
                    PgPartitionObjectKind::Index,
                    "events_2024_held",
                    "CREATE INDEX events_2024_held ON public.events_2024 USING btree (id)",
                    false,
                )],
            ),
            partition(
                "events_2025",
                "events",
                "FOR VALUES FROM (10) TO (20)",
                None,
                vec![
                    object(
                        PgPartitionObjectKind::Constraint,
                        "events_2025_id_key",
                        "UNIQUE (id)",
                        false,
                    ),
                    object(
                        PgPartitionObjectKind::Index,
                        "events_2025_local",
                        "CREATE INDEX events_2025_local ON public.events_2025 USING btree (id) INCLUDE (ts)",
                        false,
                    ),
                    object(
                        PgPartitionObjectKind::Trigger,
                        "events_2025_touch",
                        "CREATE TRIGGER events_2025_touch BEFORE UPDATE ON public.events_2025 FOR EACH ROW EXECUTE FUNCTION touch()",
                        true,
                    ),
                ],
            ),
            partition(
                "events_eu",
                "events",
                "FOR VALUES FROM (20) TO (30)",
                Some("LIST (id)"),
                vec![object(
                    PgPartitionObjectKind::Index,
                    "events_eu_id_idx",
                    "CREATE INDEX events_eu_id_idx ON ONLY public.events_eu USING btree (id)",
                    false,
                )],
            ),
            partition(
                "events_eu_1",
                "events_eu",
                "FOR VALUES IN (1)",
                None,
                vec![object(
                    PgPartitionObjectKind::Index,
                    "events_eu_1_ts",
                    "CREATE INDEX events_eu_1_ts ON public.events_eu_1 USING btree (ts)",
                    false,
                )],
            ),
        ],
        ..PgTableContext::default()
    };
    let child_context = PgTableContext {
        partition_bound: Some("FOR VALUES FROM (0) TO (10)".into()),
        ancestors: vec![TableRef::new(Some("public"), "events")],
        ..PgTableContext::default()
    };
    let tables = [
        ScriptTable {
            meta: &events,
            context: events_context,
        },
        ScriptTable {
            meta: &events_2024,
            context: child_context,
        },
    ];
    let layout = script_layout(
        Dialect::Postgres,
        &tables,
        SqlScript {
            drop_table: false,
            structure: true,
            content: true,
            gzip: false,
        },
        true,
        None,
    );
    let text = &layout.opening;
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing in\n{text}"))
    };
    // the parent's index, every partition, then the partitions' own indexes (parents first)
    assert!(at("CREATE INDEX events_ts_idx ON ONLY public.events") < at("PARTITION OF"));
    let last_partition = at("\"events_eu_1\" PARTITION OF \"public\".\"events_eu\"");
    let unique = at(
        "ALTER TABLE \"public\".\"events_2025\" ADD CONSTRAINT \"events_2025_id_key\" UNIQUE (id);",
    );
    let local =
        at("CREATE INDEX events_2025_local ON public.events_2025 USING btree (id) INCLUDE (ts);");
    // `ON ONLY` would leave the index of a partitioned partition invalid, without its partitions
    let eu = at("CREATE INDEX events_eu_id_idx ON public.events_eu USING btree (id);");
    let eu_1 = at("CREATE INDEX events_eu_1_ts ON public.events_eu_1 USING btree (ts);");
    assert!(last_partition < unique && unique < local && local < eu && eu < eu_1);
    assert!(!text.contains("ON ONLY public.events_eu "), "{text}");
    // the partition exported on its own brings its own indexes
    assert!(!text.contains("events_2024_held"), "{text}");
    assert!(!text.contains("CREATE TRIGGER"), "{text}");
    // triggers after the rows: the parent's, then the partition's own, disabled again
    let triggers = layout.closing.last().unwrap();
    let parent = triggers.find("CREATE TRIGGER events_audit").unwrap();
    let own = triggers.find("CREATE TRIGGER events_2025_touch").unwrap();
    let disable = triggers
        .find("ALTER TABLE \"public\".\"events_2025\" DISABLE TRIGGER \"events_2025_touch\";")
        .unwrap();
    assert!(parent < own && own < disable, "{triggers}");
}

#[test]
fn script_layout_creates_and_resets_a_shared_sequence_once() {
    use crate::export::structure::{ScriptTable, script_layout};
    use crate::sqlgen::ddl::{PgSequence, PgTableContext};

    let serial = |name: &str, seq: &str| {
        let mut c = mcol(name, "integer", ValueKind::Int, true);
        c.default_expr = Some(format!("nextval('{seq}'::regclass)"));
        c.is_auto_increment = true;
        c
    };
    let sequence = |column: &str, name: &str, owned: bool| PgSequence {
        column: column.into(),
        schema: "public".into(),
        name: name.into(),
        owned,
        data_type: Some("integer".into()),
        start: Some(1),
        increment: Some(1),
        min: Some(1),
        max: Some(i32::MAX.into()),
        cache: Some(1),
        cycle: false,
    };
    // `orders` owns `doc_seq`; `invoices` draws from it too; `notes` has a serial of its own.
    // Requested: invoices first.
    let invoices = table_meta("public", "invoices", vec![serial("id", "doc_seq")]);
    let orders = table_meta("public", "orders", vec![serial("id", "doc_seq")]);
    let notes = table_meta("public", "notes", vec![serial("id", "notes_id_seq")]);
    let tables = [
        ScriptTable {
            meta: &invoices,
            context: PgTableContext {
                sequences: vec![sequence("id", "doc_seq", false)],
                ..PgTableContext::default()
            },
        },
        ScriptTable {
            meta: &orders,
            context: PgTableContext {
                sequences: vec![sequence("id", "doc_seq", true)],
                ..PgTableContext::default()
            },
        },
        ScriptTable {
            meta: &notes,
            context: PgTableContext {
                sequences: vec![sequence("id", "notes_id_seq", true)],
                ..PgTableContext::default()
            },
        },
    ];
    let layout = script_layout(
        Dialect::Postgres,
        &tables,
        SqlScript {
            drop_table: true,
            structure: true,
            content: true,
            gzip: false,
        },
        true,
        None,
    );
    // the owner first (created first, dropped last)
    assert_eq!(layout.order, [1, 0, 2]);
    let text = &layout.opening;
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing in\n{text}"))
    };
    assert!(
        at("DROP TABLE IF EXISTS \"public\".\"invoices\"")
            < at("DROP TABLE IF EXISTS \"public\".\"orders\"")
    );
    assert_eq!(
        text.matches("CREATE SEQUENCE IF NOT EXISTS \"public\".\"doc_seq\"")
            .count(),
        1,
        "{text}"
    );
    assert!(
        at("CREATE SEQUENCE IF NOT EXISTS \"public\".\"doc_seq\"")
            < at("CREATE TABLE \"public\".\"orders\"")
    );
    assert!(
        text.contains(
            "ALTER SEQUENCE \"public\".\"doc_seq\" OWNED BY \"public\".\"orders\".\"id\";"
        )
    );
    // one reset per sequence, the shared one over both tables
    let resets = &layout.closing[0];
    assert_eq!(resets.matches("setval").count(), 2, "{resets}");
    assert!(resets.contains(
        "SELECT setval(pg_get_serial_sequence('\"public\".\"notes\"', 'id'), GREATEST(MAX(\"id\"), 1)) FROM \"public\".\"notes\" HAVING MAX(\"id\") IS NOT NULL;"
    ));
    assert!(resets.contains(
        "SELECT setval('\"public\".\"doc_seq\"', GREATEST(v, 1)) FROM (SELECT GREATEST((SELECT MAX(\"id\") FROM \"public\".\"invoices\"), (SELECT MAX(\"id\") FROM \"public\".\"orders\")) AS v) AS s WHERE v IS NOT NULL;"
    ), "{resets}");

    // Structure only: no resets at all.
    let layout = script_layout(
        Dialect::Postgres,
        &tables,
        SqlScript {
            drop_table: false,
            structure: true,
            content: false,
            gzip: false,
        },
        true,
        None,
    );
    assert!(layout.closing.iter().all(|s| !s.contains("setval")));
}

#[tokio::test]
async fn multi_table_sql_script_reloads_over_an_existing_copy_with_foreign_keys_on() {
    use crate::export::tables::run_export_tables;
    let dir = tempfile::tempdir().unwrap();
    let metas = vec![
        Arc::new(lite_meta("parent", None)),
        Arc::new(lite_meta("child", Some("parent"))),
    ];
    let file = dir.path().join("all.sql");
    let fake = FakeSession::new(Dialect::Sqlite, vec![lite_set(2)]);
    run_export_tables(
        &fake,
        &tables_request(
            &["parent", "child"],
            script(true, true, true, false),
            ExportLayout::SingleFile,
            &file,
        ),
        &metas,
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    // every DROP first, the referencing table first
    assert!(
        text.starts_with("DROP TABLE IF EXISTS \"child\";\nDROP TABLE IF EXISTS \"parent\";\n\n"),
        "{text}"
    );
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    db.execute_batch(&text).unwrap();
    // a second load drops the referencing table before the one it references
    db.execute_batch(&text).unwrap();
    let n: i64 = db
        .query_row("SELECT count(*) FROM child", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2);
}

// ---------------------------------------------------------------------------------------------
// Session time zone of SQL scripts
// ---------------------------------------------------------------------------------------------

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

#[tokio::test]
async fn the_time_zone_is_read_from_the_exporting_session() {
    let zone = |dialect: Dialect, row: Vec<Value>| {
        let fake = FakeSession::new(dialect, vec![]).with_zone(row);
        async move { session_time_zone(&fake, dialect).await }
    };
    // MySQL: the session's own zone, or (SYSTEM) the server clock's offset from UTC.
    assert_eq!(
        zone(Dialect::Mysql, vec![text("+07:00"), text("07:00:00")]).await,
        Some("+07:00".into())
    );
    assert_eq!(
        zone(Dialect::Mysql, vec![text("Asia/Jakarta"), text("00:00:00")]).await,
        Some("Asia/Jakarta".into())
    );
    assert_eq!(
        zone(Dialect::Mysql, vec![text("SYSTEM"), text("-05:30:00")]).await,
        Some("-05:30".into())
    );
    assert_eq!(
        zone(Dialect::Mysql, vec![text("SYSTEM"), text("00:00:00")]).await,
        Some("+00:00".into())
    );
    assert_eq!(
        zone(Dialect::Mysql, vec![text("SYSTEM"), Value::Null]).await,
        None
    );
    // PostgreSQL: the TimeZone setting.
    assert_eq!(
        zone(Dialect::Postgres, vec![text("Europe/Berlin")]).await,
        Some("Europe/Berlin".into())
    );
    // SQLite has none; a server that cannot say leaves it to the script's default.
    assert_eq!(zone(Dialect::Sqlite, vec![text("UTC")]).await, None);
    let mute = FakeSession::new(Dialect::Mysql, vec![]);
    assert_eq!(session_time_zone(&mute, Dialect::Mysql).await, None);
    assert_eq!(offset_from_timediff("07:00:00"), Some("+07:00".into()));
    assert_eq!(offset_from_timediff("-03:00:00"), Some("-03:00".into()));
    assert_eq!(offset_from_timediff("-838:59:59"), None);
    assert_eq!(offset_from_timediff("nonsense"), None);
}

#[tokio::test]
async fn sql_exports_set_the_session_time_zone_and_put_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let meta = table_meta(
        "app",
        "events",
        vec![
            mcol("id", "int", ValueKind::Int, true),
            mcol("name", "varchar(20)", ValueKind::Text, false),
        ],
    );
    let mysql_session = || {
        FakeSession::new(Dialect::Mysql, vec![id_name_set(2)])
            .with_zone(vec![text("+07:00"), text("07:00:00")])
    };
    let mut source = table_source("events");
    if let ExportSource::Table { table, .. } = &mut source {
        table.schema = Some("app".into());
    }
    let expect = |file: &Path| {
        let sql = std::fs::read_to_string(file).unwrap();
        assert!(
            sql.starts_with(
                "SET @OLD_TIME_ZONE=@@TIME_ZONE;\nSET TIME_ZONE='+07:00';\nINSERT INTO"
            ),
            "{sql}"
        );
        assert!(
            sql.ends_with(");\nSET TIME_ZONE=@OLD_TIME_ZONE;\n"),
            "{sql}"
        );
        // No foreign key statements around the rows of one table.
        assert!(!sql.contains("FOREIGN_KEY_CHECKS"), "{sql}");
    };

    // A table (rows only), a query and the rows of a shown result: all set the zone.
    let file = dir.path().join("table.sql");
    let req = request(source.clone(), insert_format("", 10), &file, None);
    run_export(
        &mysql_session(),
        &req,
        Some(&meta),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    expect(&file);
    let file = dir.path().join("query.sql");
    let req = request(sql_source("select 1"), insert_format("t", 10), &file, None);
    run_export(
        &mysql_session(),
        &req,
        None,
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    expect(&file);
    let file = dir.path().join("rows.sql");
    let req = request(
        rows_source(id_name_set(2), Some("events")),
        insert_format("", 10),
        &file,
        None,
    );
    run_export(
        &mysql_session(),
        &req,
        None,
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    expect(&file);

    // PostgreSQL: SET TIME ZONE … RESET TIME ZONE.
    let file = dir.path().join("pg.sql");
    let pg = FakeSession::new(
        Dialect::Postgres,
        vec![(
            vec![rcol("id", ValueKind::Int, "int4")],
            vec![vec![text("1")]],
        )],
    )
    .with_zone(vec![text("Europe/Berlin")]);
    let req = request(sql_source("select 1"), insert_format("t", 10), &file, None);
    run_export(&pg, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let sql = std::fs::read_to_string(&file).unwrap();
    assert!(
        sql.starts_with("SET TIME ZONE 'Europe/Berlin';\nINSERT INTO"),
        "{sql}"
    );
    assert!(sql.ends_with(");\nRESET TIME ZONE;\n"), "{sql}");

    // SQLite and the other formats write nothing of the kind.
    let file = dir.path().join("lite.sql");
    let lite = FakeSession::new(Dialect::Sqlite, vec![id_name_set(1)]);
    let req = request(sql_source("select 1"), insert_format("t", 10), &file, None);
    run_export(&lite, &req, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .starts_with("INSERT INTO")
    );
    let file = dir.path().join("my.csv");
    let req = request(sql_source("select 1"), csv_format(), &file, None);
    run_export(
        &mysql_session(),
        &req,
        None,
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(
        !std::fs::read_to_string(&file)
            .unwrap()
            .contains("TIME_ZONE")
    );
}

#[tokio::test]
async fn multi_table_scripts_set_the_session_time_zone_around_everything() {
    use crate::export::tables::run_export_tables;
    let dir = tempfile::tempdir().unwrap();
    let metas: Vec<Arc<TableMeta>> = ["a", "b"]
        .iter()
        .map(|n| {
            Arc::new(table_meta(
                "app",
                n,
                vec![
                    mcol("id", "int", ValueKind::Int, true),
                    mcol("name", "varchar(20)", ValueKind::Text, false),
                ],
            ))
        })
        .collect();
    let fake = FakeSession::new(Dialect::Mysql, vec![id_name_set(1)])
        .with_zone(vec![text("-03:00"), text("-03:00:00")]);
    let file = dir.path().join("all.sql");
    let mut req = tables_request(
        &["a", "b"],
        insert_format("", 10),
        ExportLayout::SingleFile,
        &file,
    );
    for t in &mut req.tables {
        t.schema = Some("app".into());
    }
    run_export_tables(&fake, &req, &metas, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let sql = std::fs::read_to_string(&file).unwrap();
    assert!(
        sql.starts_with(
            "SET @OLD_TIME_ZONE=@@TIME_ZONE;\nSET TIME_ZONE='-03:00';\n\
             SET @OLD_FOREIGN_KEY_CHECKS = @@FOREIGN_KEY_CHECKS, FOREIGN_KEY_CHECKS = 0;\n"
        ),
        "{sql}"
    );
    assert!(
        sql.ends_with(
            "SET FOREIGN_KEY_CHECKS = @OLD_FOREIGN_KEY_CHECKS;\nSET TIME_ZONE=@OLD_TIME_ZONE;\n"
        ),
        "{sql}"
    );
    // One table, rows only: the zone, and no foreign key statements.
    let file = dir.path().join("one.sql");
    let mut req = tables_request(
        &["a"],
        insert_format("", 10),
        ExportLayout::SingleFile,
        &file,
    );
    req.tables[0].schema = Some("app".into());
    run_export_tables(&fake, &req, &metas[..1], |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let sql = std::fs::read_to_string(&file).unwrap();
    assert!(
        sql.starts_with("SET @OLD_TIME_ZONE=@@TIME_ZONE;\nSET TIME_ZONE='-03:00';\nINSERT"),
        "{sql}"
    );
    assert!(!sql.contains("FOREIGN_KEY_CHECKS"), "{sql}");
    // Structure only (no rows, no timestamps): no zone either.
    let file = dir.path().join("structure.sql");
    let req = tables_request(
        &["a"],
        script(false, true, false, false),
        ExportLayout::SingleFile,
        &file,
    );
    let mut req = req;
    req.tables[0].schema = Some("app".into());
    run_export_tables(&fake, &req, &metas[..1], |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert!(
        !std::fs::read_to_string(&file)
            .unwrap()
            .contains("TIME_ZONE")
    );
}

// ---------------------------------------------------------------------------------------------
// Table exports from an offset (a table tab's later pages)
// ---------------------------------------------------------------------------------------------

fn page_meta(dialect: Dialect, keyed: bool) -> TableMeta {
    let mut meta = table_meta(
        "app",
        "items",
        vec![
            mcol("id", "int", ValueKind::Int, keyed),
            mcol("name", "text", ValueKind::Text, false),
            mcol("doc", "json", ValueKind::Json, false),
        ],
    );
    if dialect == Dialect::Sqlite {
        meta.table = TableRef::new(Some("main"), "items");
    }
    meta
}

fn asc(column: &str) -> SortSpec {
    SortSpec {
        column: column.into(),
        dir: crate::types::browse::SortDir::Asc,
        nulls: None,
    }
}

#[test]
fn table_query_starts_at_an_offset_with_a_deterministic_order() {
    let names = ["id".to_string(), "name".to_string()];
    // The key orders a page; the offset follows the limit.
    let q = table_query(
        Dialect::Postgres,
        &page_meta(Dialect::Postgres, true),
        Some(&names),
        None,
        &[],
        Some(10),
        10,
        false,
    )
    .unwrap();
    assert!(
        q.sql
            .ends_with("FROM \"app\".\"items\" ORDER BY \"id\" ASC\nLIMIT 10 OFFSET 10"),
        "{}",
        q.sql
    );
    assert_eq!(q.columns.as_ref().unwrap().len(), 2);
    // A sort that does not decide the order of equal rows gets the key as tie-break.
    let q = table_query(
        Dialect::Mysql,
        &page_meta(Dialect::Mysql, true),
        Some(&names),
        None,
        &[asc("name")],
        Some(10),
        20,
        false,
    )
    .unwrap();
    assert!(
        q.sql
            .ends_with("ORDER BY `name` ASC, `id` ASC\nLIMIT 10 OFFSET 20"),
        "{}",
        q.sql
    );
    // A sort that already names the key is left alone.
    let q = table_query(
        Dialect::Mysql,
        &page_meta(Dialect::Mysql, true),
        None,
        None,
        &[asc("id")],
        Some(10),
        20,
        false,
    )
    .unwrap();
    assert!(
        q.sql.contains("ORDER BY `id` ASC\nLIMIT 10 OFFSET 20"),
        "{}",
        q.sql
    );
    // An offset without a page size reads to the end of the table.
    let q = table_query(
        Dialect::Mysql,
        &page_meta(Dialect::Mysql, true),
        Some(&names),
        None,
        &[],
        None,
        10,
        false,
    )
    .unwrap();
    assert!(q.sql.ends_with("LIMIT 4294967295 OFFSET 10"), "{}", q.sql);
    // No key: SQLite orders by rowid, the others by the selected columns (by position).
    let q = table_query(
        Dialect::Sqlite,
        &page_meta(Dialect::Sqlite, false),
        Some(&names),
        None,
        &[],
        Some(5),
        5,
        false,
    )
    .unwrap();
    assert!(
        q.sql.contains(" ORDER BY rowid ASC\nLIMIT 5 OFFSET 5"),
        "{}",
        q.sql
    );
    let q = table_query(
        Dialect::Sqlite,
        &page_meta(Dialect::Sqlite, false),
        Some(&names),
        None,
        &[asc("name")],
        Some(5),
        5,
        false,
    )
    .unwrap();
    assert!(
        q.sql
            .contains("ORDER BY \"name\" ASC, rowid ASC\nLIMIT 5 OFFSET 5"),
        "{}",
        q.sql
    );
    let q = table_query(
        Dialect::Postgres,
        &page_meta(Dialect::Postgres, false),
        None,
        None,
        &[],
        Some(5),
        5,
        false,
    )
    .unwrap();
    assert!(
        q.sql.contains(" ORDER BY 1, 2, 3\nLIMIT 5 OFFSET 5"),
        "{}",
        q.sql
    );
    // Not a page: no paging clause and no extra ordering (the whole table, in the table's order).
    let q = table_query(
        Dialect::Postgres,
        &page_meta(Dialect::Postgres, false),
        None,
        None,
        &[],
        None,
        0,
        false,
    )
    .unwrap();
    assert!(
        !q.sql.contains("ORDER BY") && !q.sql.contains("LIMIT"),
        "{}",
        q.sql
    );
}

#[tokio::test]
async fn a_table_export_reads_the_page_after_its_offset() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("page.csv");
    let fake = FakeSession::new(
        Dialect::Mysql,
        vec![(
            vec![rcol("name", ValueKind::Text, "varchar(20)")],
            vec![vec![text("row 10")]],
        )],
    );
    let meta = table_meta(
        "app",
        "items",
        vec![
            mcol("id", "int", ValueKind::Int, true),
            mcol("name", "varchar(20)", ValueKind::Text, false),
        ],
    );
    let source = ExportSource::Table {
        table: TableRef::new(Some("app"), "items"),
        filter: None,
        sort: vec![],
        columns: Some(vec!["name".into()]),
        offset: Some(10),
    };
    let req = request(source, csv_format(), &dest, Some(10));
    run_export(&fake, &req, Some(&meta), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let sql = fake.last_sql();
    assert!(
        sql.starts_with("SELECT `name`\nFROM `app`.`items`"),
        "{sql}"
    );
    assert!(
        sql.ends_with("ORDER BY `id` ASC\nLIMIT 10 OFFSET 10"),
        "{sql}"
    );
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "name\r\nrow 10\r\n"
    );
}

// ---------------------------------------------------------------------------------------------
// Several results at once
// ---------------------------------------------------------------------------------------------

fn result_item(label: &str, table: Option<&str>, n: i64) -> ExportResultItem {
    let (columns, rows) = id_name_set(n);
    ExportResultItem {
        label: label.into(),
        columns,
        rows,
        table: table.map(str::to_string),
    }
}

fn results_request(
    results: Vec<ExportResultItem>,
    format: ExportFormat,
    layout: ExportLayout,
    target: &Path,
) -> ExportResultsRequest {
    ExportResultsRequest {
        export_id: "results".into(),
        results,
        format,
        layout,
        target: target.to_string_lossy().into_owned(),
        encoding: Default::default(),
        name: None,
    }
}

#[tokio::test]
async fn all_results_go_into_one_workbook_with_a_sheet_each() {
    use crate::export::results::run_export_results;
    use crate::export::xlsx::tests::unzip;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("results.xlsx");
    let fake = FakeSession::new(Dialect::Sqlite, vec![]);
    let mut seen = Vec::new();
    let result = run_export_results(
        &fake,
        &results_request(
            vec![
                result_item("Result 1", Some("users"), 3),
                result_item("Result 2", None, 2),
            ],
            ExportFormat::Xlsx { header: true },
            ExportLayout::SingleFile,
            &file,
        ),
        |p| seen.push(p),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(result.rows, 5);
    assert_eq!(result.paths, [file.to_string_lossy().into_owned()]);
    assert_eq!(result.results[1].label, "Result 2");
    assert_eq!(result.results[1].rows, 2);
    assert_eq!(result.bytes, std::fs::metadata(&file).unwrap().len());
    assert!(seen.iter().all(|p| p.table_count == 2));
    assert!(
        seen.iter()
            .any(|p| p.table_index == 1 && p.table == "Result 2")
    );
    // The workbook: a sheet per result, the header row plus the rows in each.
    let files = unzip(&std::fs::read(&file).unwrap());
    let workbook = String::from_utf8(files["xl/workbook.xml"].clone()).unwrap();
    assert!(workbook.contains("name=\"Result 1\""), "{workbook}");
    assert!(workbook.contains("name=\"Result 2\""), "{workbook}");
    let rows_of = |sheet: &str| {
        String::from_utf8(files[sheet].clone())
            .unwrap()
            .matches("<row ")
            .count()
    };
    assert_eq!(rows_of("xl/worksheets/sheet1.xml"), 4);
    assert_eq!(rows_of("xl/worksheets/sheet2.xml"), 3);
    assert_eq!(files_in(dir.path()), ["results.xlsx"]);
}

#[tokio::test]
async fn all_results_of_csv_and_json_go_into_a_file_each() {
    use crate::export::results::run_export_results;
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeSession::new(Dialect::Sqlite, vec![]);
    let items = || {
        vec![
            result_item("Result 1", Some("users"), 2),
            result_item("Result 2", None, 1),
            result_item("Result 3", None, 0),
        ]
    };
    let folder = dir.path().to_path_buf();
    let mut req = results_request(items(), csv_format(), ExportLayout::FilePerTable, &folder);
    req.name = Some("query".into());
    let result = run_export_results(&fake, &req, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        files_in(&folder),
        ["query-1.csv", "query-2.csv", "query-3.csv"]
    );
    assert_eq!(result.paths.len(), 3);
    assert_eq!(
        std::fs::read_to_string(folder.join("query-1.csv")).unwrap(),
        "id,name\r\n0,row 0\r\n1,row 1\r\n"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("query-3.csv")).unwrap(),
        "id,name\r\n"
    );
    // Without a name: result-N; JSON the same.
    let json_dir = tempfile::tempdir().unwrap();
    let req = results_request(
        items(),
        json_format(),
        ExportLayout::FilePerTable,
        json_dir.path(),
    );
    run_export_results(&fake, &req, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        files_in(json_dir.path()),
        ["result-1.json", "result-2.json", "result-3.json"]
    );
    // One CSV file cannot hold several results.
    let one = dir.path().join("all.csv");
    let req = results_request(items(), csv_format(), ExportLayout::SingleFile, &one);
    assert!(
        run_export_results(&fake, &req, |_| {}, CancellationToken::new())
            .await
            .is_err()
    );
    assert!(!one.exists());
}

#[tokio::test]
async fn all_results_of_html_and_sql_go_into_one_file() {
    use crate::export::results::run_export_results;
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeSession::new(Dialect::Sqlite, vec![]);
    let items = vec![
        result_item("Result 1", Some("users"), 2),
        result_item("Result 2", None, 1),
    ];
    // HTML: one page, a heading and a table per result.
    let page = dir.path().join("all.html");
    let req = results_request(
        items.clone(),
        ExportFormat::Html,
        ExportLayout::SingleFile,
        &page,
    );
    run_export_results(&fake, &req, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let html = std::fs::read_to_string(&page).unwrap();
    assert_eq!(html.matches("<table>").count(), 2);
    assert!(html.find("<h2>Result 1</h2>").unwrap() < html.find("<h2>Result 2</h2>").unwrap());

    // SQL: one script; each result's INSERTs name its table, or `untitled`.
    let sql_file = dir.path().join("all.sql");
    let req = results_request(
        items.clone(),
        insert_format("ignored", 10),
        ExportLayout::SingleFile,
        &sql_file,
    );
    run_export_results(&fake, &req, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&sql_file).unwrap(),
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES\n(0, 'row 0'),\n(1, 'row 1');\n\n\
         INSERT INTO \"untitled\" (\"id\", \"name\") VALUES\n(0, 'row 0');\n\n"
    );
    // A script of results has no structure: content is required.
    let req = results_request(
        items.clone(),
        script(false, true, false, false),
        ExportLayout::SingleFile,
        &sql_file,
    );
    assert!(
        run_export_results(&fake, &req, |_| {}, CancellationToken::new())
            .await
            .is_err()
    );
    // On MySQL the script sets the session's time zone once, around both results.
    let my =
        FakeSession::new(Dialect::Mysql, vec![]).with_zone(vec![text("+07:00"), text("07:00:00")]);
    let req = results_request(
        items,
        insert_format("", 10),
        ExportLayout::SingleFile,
        &sql_file,
    );
    run_export_results(&my, &req, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let sql = std::fs::read_to_string(&sql_file).unwrap();
    assert!(
        sql.starts_with(
            "SET @OLD_TIME_ZONE=@@TIME_ZONE;\nSET TIME_ZONE='+07:00';\nINSERT INTO `users`"
        ),
        "{sql}"
    );
    assert!(sql.contains("INSERT INTO `untitled`"), "{sql}");
    assert!(sql.ends_with("SET TIME_ZONE=@OLD_TIME_ZONE;\n"), "{sql}");
    assert_eq!(sql.matches("SET TIME_ZONE='").count(), 1);
}

#[tokio::test]
async fn all_results_refuse_what_cannot_be_written_and_leave_no_file() {
    use crate::export::results::run_export_results;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("r.xlsx");
    let fake = FakeSession::new(Dialect::Sqlite, vec![]);
    // A capped binary preview in the second result: nothing is written.
    let mut bad = result_item("Result 2", None, 1);
    bad.columns.push(rcol("b", ValueKind::Bytes, "blob"));
    bad.rows[0].push(Value::Bytes(BytesValue {
        base64: String::new(),
        len: 70_000,
        truncated: true,
    }));
    let req = results_request(
        vec![result_item("Result 1", None, 1), bad],
        ExportFormat::Xlsx { header: true },
        ExportLayout::SingleFile,
        &file,
    );
    let err = run_export_results(&fake, &req, |_| {}, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Result 2"), "{err}");
    assert!(files_in(dir.path()).is_empty());
    // A cancelled export leaves no file, nor a half-written set.
    let token = CancellationToken::new();
    token.cancel();
    let req = results_request(
        vec![result_item("Result 1", None, 1)],
        csv_format(),
        ExportLayout::FilePerTable,
        dir.path(),
    );
    let err = run_export_results(&fake, &req, |_| {}, token)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Cancelled));
    assert!(files_in(dir.path()).is_empty());
    // No results at all.
    let req = results_request(vec![], csv_format(), ExportLayout::FilePerTable, dir.path());
    assert!(
        run_export_results(&fake, &req, |_| {}, CancellationToken::new())
            .await
            .is_err()
    );
}
