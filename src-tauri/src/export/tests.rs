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
use crate::types::profile::Driver;
use crate::types::query::{ExecOutcome, ResultSet};
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableKind, TableRef, TableSummary,
};

type Set = (Vec<ResultColumn>, Vec<Vec<Value>>);

/// Streams canned result sets; records the statements it was asked to run.
struct FakeSession {
    dialect: Dialect,
    sets: Vec<Set>,
    meta: Option<TableMeta>,
    /// Never delivers anything; waits until the cancel slot is cancelled.
    block_until_cancel: bool,
    /// After the sink stops the stream, keeps "draining" the statement for this long (as the
    /// drivers do with a statement `cancel_safe_on_stop` refuses) unless it is cancelled.
    drain_after_stop: Option<Duration>,
    seen: Mutex<Vec<(String, Vec<Value>)>>,
    cancels: AtomicUsize,
}

impl FakeSession {
    fn new(dialect: Dialect, sets: Vec<Set>) -> FakeSession {
        FakeSession {
            dialect,
            sets,
            meta: None,
            block_until_cancel: false,
            drain_after_stop: None,
            seen: Mutex::new(Vec::new()),
            cancels: AtomicUsize::new(0),
        }
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
        match self.dialect {
            Dialect::Postgres => Driver::Postgres,
            Dialect::Mysql => Driver::Mysql,
            Dialect::Sqlite => Driver::Sqlite,
        }
    }
    fn dialect(&self) -> Dialect {
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
    async fn query(&self, _sql: &str, _params: &[Value]) -> Result<ResultSet, AppError> {
        unsupported()
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
    }
}

fn csv_format() -> ExportFormat {
    ExportFormat::Csv {
        delimiter: ",".into(),
        header: true,
        quote_all: false,
        null_as: String::new(),
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
    let insert = ExportFormat::SqlInsert {
        table_name: String::new(),
        batch_size: 10,
    };
    let req = request(source.clone(), insert.clone(), &dest, None);
    run_export(&fake, &req, Some(&meta), |_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "INSERT INTO \"t\" (\"id\", \"note\") OVERRIDING SYSTEM VALUE VALUES\n(1, 'a');\n"
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
    let q = table_query(Dialect::Mysql, &meta, None, None, &[], Some(10), false).unwrap();
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
        false,
    )
    .unwrap();
    assert_eq!(
        q.sql,
        "SELECT `name`, `id`\nFROM `app`.`files` ORDER BY `id` ASC"
    );
    assert!(q.blob_hex.is_empty());
    assert_eq!(q.columns.unwrap()[0].name, "name");
    assert!(table_query(Dialect::Mysql, &meta, Some(&[]), None, &[], None, false).is_err());
    assert!(
        table_query(
            Dialect::Mysql,
            &meta,
            Some(&["nope".into()]),
            None,
            &[],
            None,
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
            quote_all: false,
            null_as: String::new(),
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
    let req = request(
        table_source("items"),
        ExportFormat::SqlInsert {
            table_name: String::new(),
            batch_size: 2,
        },
        &dest,
        None,
    );
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
