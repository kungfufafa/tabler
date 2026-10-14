use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::State;
use uuid::Uuid;

use super::query::{query_limited, query_time_limit};
use crate::drivers::DbSession;
use crate::drivers::postgres::decode::text_to_value;
use crate::error::AppError;
use crate::sqlgen::mutate::TableSpec;
use crate::sqlgen::select::{
    ROWID_ALIAS, SelectSpec, build_count, build_fetch_cell, build_select, inline_params,
};
use crate::state::{AppState, Connection};
use crate::types::browse::{
    BrowsePage, BrowseRequest, FilterGroup, RowCount, RowIdentity, RowKey, TracedSql,
};
use crate::types::profile::Driver;
use crate::types::schema::{ColumnMeta, TableMeta, TableRef, ValueKind};
use crate::types::value::{Value, decode_hex};

/// Table metadata from the per-connection cache (fetching it on a miss).
pub async fn cached_meta(conn: &Connection, table: &TableRef) -> Result<Arc<TableMeta>, AppError> {
    if let Some(meta) = conn.schema_cache.read().await.get(table) {
        return Ok(meta.clone());
    }
    let meta = Arc::new(conn.session().await.table_meta(table).await?);
    conn.schema_cache
        .write()
        .await
        .insert(table.clone(), meta.clone());
    Ok(meta)
}

fn identity_for(driver: Driver, meta: &TableMeta) -> RowIdentity {
    if !meta.primary_key.is_empty() {
        RowIdentity::Pk
    } else if driver == Driver::Sqlite && meta.kind == crate::types::schema::TableKind::Table {
        RowIdentity::Rowid
    } else {
        RowIdentity::None
    }
}

/// PG rows arrive as text (sqlgen casts every column); decode them by the column's kind.
fn decode_rows(driver: Driver, columns: &[&ColumnMeta], rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
    if driver != Driver::Postgres {
        return rows;
    }
    rows.into_iter()
        .map(|row| {
            row.into_iter()
                .enumerate()
                .map(|(i, v)| match (columns.get(i), v) {
                    (Some(col), Value::Text(text)) => {
                        text_to_value(col.kind, &col.data_type, &text)
                    }
                    (_, v) => v,
                })
                .collect()
        })
        .collect()
}

/// Loads a page of a table's rows. The load runs under the query time out (Settings ▸ Connections;
/// `timeout_secs` overrides it, 0 = no limit): a statement still running past it is cancelled on
/// the server and the call fails with `QUERY_TIMEOUT`.
#[tauri::command]
pub async fn browse_table(
    state: State<'_, AppState>,
    connection_id: Uuid,
    req: BrowseRequest,
    timeout_secs: Option<u32>,
) -> Result<BrowsePage, AppError> {
    if req.limit == 0 || req.limit > 10_000 {
        return Err(AppError::InvalidInput(
            "limit must be between 1 and 10000".into(),
        ));
    }
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await;
    log::debug!(
        "browse_table {}.{} limit={} offset={}",
        req.table.schema.as_deref().unwrap_or(""),
        req.table.name,
        req.limit,
        req.offset
    );
    let meta = cached_meta(&conn, &req.table).await?;
    let limit = query_time_limit(&state.appdb, timeout_secs);
    browse_page(session.as_ref(), &meta, &req, limit).await
}

/// Whether a failed select addressed by `identity` is worth running again addressing the rows by
/// all their columns: the first attempt only, and only when the server's message is about the
/// rowid (a table without one reports `no such column: rowid`). Any other failure would fail the
/// same way and, for a statement that ran into the query time out, wait for the limit again.
fn retry_by_all_columns(identity: RowIdentity, attempt: u32, error: &AppError) -> bool {
    identity == RowIdentity::Rowid
        && attempt == 1
        && matches!(error, AppError::Db { message, .. } if message.to_lowercase().contains("rowid"))
}

/// `browse_table` without Tauri state: one page of `meta`'s table, loaded under `limit`.
pub async fn browse_page(
    session: &dyn DbSession,
    meta: &TableMeta,
    req: &BrowseRequest,
    limit: Option<Duration>,
) -> Result<BrowsePage, AppError> {
    let driver = session.driver();
    let dialect = session.dialect();
    let mut identity = identity_for(driver, meta);

    let selected: Vec<&ColumnMeta> = match &req.columns {
        Some(names) => names
            .iter()
            .map(|n| {
                meta.columns
                    .iter()
                    .find(|c| &c.name == n)
                    .ok_or_else(|| AppError::InvalidInput(format!("unknown column \"{n}\"")))
            })
            .collect::<Result<_, _>>()?,
        None => meta.columns.iter().collect(),
    };

    let started = Instant::now();
    let mut attempt = 0;
    loop {
        attempt += 1;
        let built = build_select(&SelectSpec {
            dialect,
            table: &meta.table,
            columns: &meta.columns,
            selected: req.columns.as_deref(),
            filter: req.filter.as_ref(),
            sort: &req.sort,
            limit: req.limit,
            offset: req.offset,
            pk: &meta.primary_key,
            identity,
        })?;
        match query_limited(session, &built.sql, &built.params, limit).await {
            Ok(rs) => {
                let mut rows = rs.rows;
                let mut row_ids = None;
                if identity == RowIdentity::Rowid {
                    // The synthetic rowid column is last; split it off.
                    let idx = rs.columns.iter().position(|c| c.name == ROWID_ALIAS);
                    if let Some(idx) = idx {
                        let mut ids = Vec::with_capacity(rows.len());
                        for row in &mut rows {
                            let v = row.remove(idx);
                            ids.push(match v {
                                Value::Int(i) => i,
                                Value::BigInt(s) => s.parse().unwrap_or(0),
                                _ => 0,
                            });
                        }
                        row_ids = Some(ids);
                    }
                }
                let rows = decode_rows(driver, &selected, rows);
                return Ok(BrowsePage {
                    columns: selected.iter().map(|c| (*c).clone()).collect(),
                    rows,
                    pk_columns: meta.primary_key.clone(),
                    identity,
                    row_ids,
                    offset: req.offset,
                    limit: req.limit,
                    sql: inline_params(dialect, &built.sql, &built.params),
                    duration_ms: started.elapsed().as_millis() as u64,
                });
            }
            // WITHOUT ROWID tables have no rowid: retry addressing rows by all columns.
            Err(e) if retry_by_all_columns(identity, attempt, &e) => {
                log::debug!("rowid select failed ({e}); falling back to all-columns identity");
                identity = RowIdentity::None;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Rows above which an unfiltered count uses the planner's estimate (TablePlus's default for
/// "Estimate count when table has more than … rows").
pub const DEFAULT_ESTIMATE_ABOVE: i64 = 500_000;

/// The planner estimate to report instead of counting, when it applies: no exact count asked for,
/// no filter, and a table estimated to hold more than `estimate_above` rows.
fn estimated_count(
    meta: &TableMeta,
    filtered: bool,
    exact: bool,
    estimate_above: i64,
) -> Option<RowCount> {
    let est = meta.row_estimate?;
    (!exact && !filtered && est > estimate_above).then_some(RowCount {
        count: est,
        exact: false,
    })
}

/// Counts a table's rows (`filter` applied). An unfiltered table estimated above `estimate_above`
/// rows (default 500,000) reports the planner estimate, unless `exact` is set. The count runs
/// under the query time out (see `browse_table`).
#[tauri::command]
pub async fn count_table_rows(
    state: State<'_, AppState>,
    connection_id: Uuid,
    table: TableRef,
    filter: Option<FilterGroup>,
    exact: Option<bool>,
    estimate_above: Option<i64>,
    timeout_secs: Option<u32>,
) -> Result<RowCount, AppError> {
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await;
    let meta = cached_meta(&conn, &table).await?;
    if let Some(estimate) = estimated_count(
        &meta,
        filter.is_some(),
        exact.unwrap_or(false),
        estimate_above.unwrap_or(DEFAULT_ESTIMATE_ABOVE),
    ) {
        return Ok(estimate);
    }
    let built = build_count(
        session.dialect(),
        &meta.table,
        &meta.columns,
        filter.as_ref(),
    )?;
    let limit = query_time_limit(&state.appdb, timeout_secs);
    let rs = query_limited(session.as_ref(), &built.sql, &built.params, limit).await?;
    let count = match rs.rows.first().and_then(|r| r.first()) {
        Some(Value::Int(i)) => *i,
        Some(Value::BigInt(s)) | Some(Value::Decimal(s)) | Some(Value::Text(s)) => {
            s.parse().unwrap_or(0)
        }
        _ => 0,
    };
    Ok(RowCount { count, exact: true })
}

/// Largest value `fetch_cell` returns; bigger ones should be exported instead.
const FULL_CELL_MAX: usize = 64 * 1024 * 1024;

fn too_large() -> AppError {
    AppError::InvalidInput("value is too large to show; export it instead".into())
}

/// Text form of a hex-encoded blob cell, whatever type the driver decoded it as.
fn hex_text(v: Value) -> Option<String> {
    match v {
        Value::Bytes(b) => {
            use base64::Engine as _;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(b.base64)
                .ok()?;
            String::from_utf8(raw).ok()
        }
        other => other.as_text(),
    }
}

/// Fetches one full cell (the inspector's "Load full value" for blobs beyond the preview cap),
/// under the query time out (see `browse_table`).
#[tauri::command]
pub async fn fetch_cell(
    state: State<'_, AppState>,
    connection_id: Uuid,
    table: TableRef,
    key: RowKey,
    column: String,
    timeout_secs: Option<u32>,
) -> Result<Value, AppError> {
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await;
    let meta = cached_meta(&conn, &table).await?;
    let limit = query_time_limit(&state.appdb, timeout_secs);
    fetch_cell_limited(session.as_ref(), &meta, &column, &key, limit).await
}

/// The statements the connection's session ran for the app since the last call (introspection,
/// table loads, counts, writes, exports), oldest first, for the console log: the renderer calls
/// this after each of its own calls and logs what it gets. A connection that is gone has none.
#[tauri::command]
pub async fn drain_sql_trace(
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<Vec<TracedSql>, AppError> {
    let conn = state.connections.read().await.get(&connection_id).cloned();
    match conn {
        Some(conn) => Ok(conn.session().await.drain_trace()),
        None => Ok(Vec::new()),
    }
}

/// `fetch_cell` without Tauri state or time limit, for tests.
pub async fn fetch_cell_inner(
    session: &dyn DbSession,
    meta: &TableMeta,
    column: &str,
    key: &RowKey,
) -> Result<Value, AppError> {
    fetch_cell_limited(session, meta, column, key, None).await
}

/// `fetch_cell` without Tauri state, under `limit`.
pub async fn fetch_cell_limited(
    session: &dyn DbSession,
    meta: &TableMeta,
    column: &str,
    key: &RowKey,
    limit: Option<Duration>,
) -> Result<Value, AppError> {
    let col = meta
        .columns
        .iter()
        .find(|c| c.name == column)
        .ok_or_else(|| AppError::InvalidInput(format!("unknown column \"{column}\"")))?;
    let spec = TableSpec {
        dialect: session.dialect(),
        table: &meta.table,
        columns: &meta.columns,
    };
    let (sql, params) = build_fetch_cell(&spec, col, key)?;
    let rs = query_limited(session, &sql, &params, limit).await?;
    fetched_value(
        session.driver(),
        col,
        rs.rows.into_iter().next().unwrap_or_default(),
    )
}

/// Decodes the row of a `build_fetch_cell` query (see its result shape). A blob always comes back
/// whole — the grid's preview cap does not apply — and a NULL stays NULL.
fn fetched_value(driver: Driver, col: &ColumnMeta, row: Vec<Value>) -> Result<Value, AppError> {
    let mut cells = row.into_iter();
    let first = cells.next().unwrap_or(Value::Null);
    let (value, hex) = match driver {
        // `<column>, <its hex when the storage class is blob>`: SQLite blobs may sit in any column.
        Driver::Sqlite => (first, cells.next().unwrap_or(Value::Null)),
        _ if col.kind == ValueKind::Bytes => (Value::Null, first),
        _ => (first, Value::Null),
    };
    if !hex.is_null() {
        let hex =
            hex_text(hex).ok_or_else(|| AppError::Internal("unexpected blob encoding".into()))?;
        if hex.len() / 2 > FULL_CELL_MAX {
            return Err(too_large());
        }
        let bytes = decode_hex(&hex)
            .ok_or_else(|| AppError::Internal("unexpected blob encoding".into()))?;
        return Ok(Value::from_bytes_uncapped(&bytes));
    }
    if value.as_text().is_some_and(|t| t.len() > FULL_CELL_MAX) {
        return Err(too_large());
    }
    Ok(match (driver, value) {
        (Driver::Postgres, Value::Text(text)) => text_to_value(col.kind, &col.data_type, &text),
        (_, v) => v,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::drivers::{ConnectTarget, connect};
    use crate::types::profile::{ConnectionProfile, ProfileOptions, SslMode};
    use crate::types::schema::TableKind;
    use crate::types::value::BYTES_PREVIEW_CAP;

    fn big_blob() -> Vec<u8> {
        (0..BYTES_PREVIEW_CAP + 4_464)
            .map(|i| (i % 251) as u8)
            .collect()
    }

    /// A real SQLite session on a temp file holding a view, an untyped column and a TEXT column
    /// that store blobs larger than the grid's 64 KiB preview.
    async fn sqlite_session() -> (Arc<dyn DbSession>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fetch.sqlite");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE files (id INTEGER PRIMARY KEY, name TEXT, data BLOB);
             CREATE VIEW files_v AS SELECT id, name, data FROM files;
             CREATE TABLE cache (key, value);
             CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT);
             CREATE TABLE fx3_wr (a TEXT PRIMARY KEY, b TEXT) WITHOUT ROWID;
             INSERT INTO fx3_wr VALUES ('x', '1'), ('y', '2');",
        )
        .unwrap();
        let big = big_blob();
        db.execute(
            "INSERT INTO files VALUES (1, 'a', ?1), (2, NULL, NULL)",
            [&big],
        )
        .unwrap();
        db.execute(
            "INSERT INTO cache VALUES ('k', ?1), ('n', NULL), ('t', 'plain')",
            [&big],
        )
        .unwrap();
        db.execute("INSERT INTO notes VALUES (1, ?1)", [&big])
            .unwrap();
        drop(db);
        (open_sqlite(&path).await, dir)
    }

    async fn open_sqlite(path: &std::path::Path) -> Arc<dyn DbSession> {
        let profile = ConnectionProfile {
            id: Uuid::new_v4(),
            name: "fetch".into(),
            color: None,
            group: None,
            driver: Driver::Sqlite,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: Some(path.to_string_lossy().into_owned()),
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        };
        connect(ConnectTarget {
            profile: &profile,
            password: None,
            host_override: None,
        })
        .await
        .unwrap()
    }

    async fn meta(session: &dyn DbSession, name: &str) -> TableMeta {
        session
            .table_meta(&TableRef::new(Some("main"), name))
            .await
            .unwrap()
    }

    fn assert_full(v: &Value) {
        assert_eq!(*v, Value::from_bytes_uncapped(&big_blob()));
        assert!(matches!(v, Value::Bytes(b) if !b.truncated));
    }

    /// Views have no primary key, rowid or ctid: the renderer keys them by every non-blob column,
    /// and the fetch must not address the row through a `rowid` subquery.
    #[tokio::test]
    async fn loads_full_blobs_from_a_keyless_sqlite_view() {
        let (session, _dir) = sqlite_session().await;
        let view = meta(session.as_ref(), "files_v").await;
        assert_eq!(view.kind, TableKind::View);
        assert_eq!(identity_for(Driver::Sqlite, &view), RowIdentity::None);
        let key = |id: i64, name: Value| {
            RowKey::AllColumns(BTreeMap::from([
                ("id".to_string(), Value::Int(id)),
                ("name".to_string(), name),
            ]))
        };
        let full = fetch_cell_inner(
            session.as_ref(),
            &view,
            "data",
            &key(1, Value::Text("a".into())),
        )
        .await
        .unwrap();
        assert_full(&full);
        let null = fetch_cell_inner(session.as_ref(), &view, "data", &key(2, Value::Null))
            .await
            .unwrap();
        assert_eq!(null, Value::Null);
    }

    /// SQLite is dynamically typed: a blob in an untyped or TEXT column must also load whole, and a
    /// NULL in a BLOB column must stay NULL (`hex(NULL)` is `''`, not NULL).
    #[tokio::test]
    async fn loads_full_blobs_whatever_the_declared_sqlite_type() {
        let (session, _dir) = sqlite_session().await;
        let cache = meta(session.as_ref(), "cache").await;
        assert_eq!(
            cache
                .columns
                .iter()
                .find(|c| c.name == "value")
                .unwrap()
                .kind,
            ValueKind::Other
        );
        let s = session.as_ref();
        assert_full(
            &fetch_cell_inner(s, &cache, "value", &RowKey::Rowid(1))
                .await
                .unwrap(),
        );
        assert_eq!(
            fetch_cell_inner(s, &cache, "value", &RowKey::Rowid(2))
                .await
                .unwrap(),
            Value::Null
        );
        assert_eq!(
            fetch_cell_inner(s, &cache, "value", &RowKey::Rowid(3))
                .await
                .unwrap(),
            Value::Text("plain".into())
        );

        let notes = meta(s, "notes").await;
        let pk = |id: i64| RowKey::Pk(BTreeMap::from([("id".to_string(), Value::Int(id))]));
        assert_full(&fetch_cell_inner(s, &notes, "body", &pk(1)).await.unwrap());

        let files = meta(s, "files").await;
        assert_full(&fetch_cell_inner(s, &files, "data", &pk(1)).await.unwrap());
        assert_eq!(
            fetch_cell_inner(s, &files, "data", &pk(2)).await.unwrap(),
            Value::Null
        );
        assert_eq!(
            fetch_cell_inner(s, &files, "name", &pk(1)).await.unwrap(),
            Value::Text("a".into())
        );
    }

    fn db_error() -> AppError {
        AppError::db("no such column: rowid")
    }

    #[test]
    fn only_a_missing_rowid_on_the_first_attempt_retries_by_all_columns() {
        use RowIdentity::{None as NoKey, Pk, Rowid};
        assert!(retry_by_all_columns(Rowid, 1, &db_error()));
        // Not twice, and not for a table that was never addressed by its rowid.
        assert!(!retry_by_all_columns(Rowid, 2, &db_error()));
        assert!(!retry_by_all_columns(Pk, 1, &db_error()));
        assert!(!retry_by_all_columns(NoKey, 1, &db_error()));
        // A statement stopped by the query time out (or by a cancel, a lost connection…) would run
        // into the limit again, and any other failure of the select fails the same way.
        for e in [
            AppError::QueryTimeout { secs: 300 },
            AppError::Cancelled,
            AppError::Timeout,
            AppError::NotConnected,
            AppError::Io("broken pipe".into()),
            AppError::db("database is locked"),
            AppError::db("no such table: main.t"),
        ] {
            assert!(!retry_by_all_columns(Rowid, 1, &e), "{e:?}");
        }
    }

    fn page_request(table: &str) -> BrowseRequest {
        BrowseRequest {
            table: TableRef::new(Some("main"), table),
            columns: None,
            filter: None,
            sort: Vec::new(),
            limit: 5,
            offset: 0,
        }
    }

    /// A table without a primary key is browsed by its rowid; when the load ran into the query time
    /// out that is the answer after one wait of the limit, not a second identical select that
    /// waits for the limit again (and scans the table twice).
    #[tokio::test]
    async fn a_page_load_that_hits_the_query_time_out_runs_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("slow.sqlite");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE fx3_slow (a INTEGER);
             INSERT INTO fx3_slow WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 2000000)
               SELECT x FROM c;",
        )
        .unwrap();
        drop(db);
        let session = open_sqlite(&path).await;
        let meta = meta(session.as_ref(), "fx3_slow").await;
        assert_eq!(identity_for(Driver::Sqlite, &meta), RowIdentity::Rowid);
        session.drain_trace();

        let mut req = page_request("fx3_slow");
        // Sorting a table without an index reads every row.
        req.sort = vec![crate::types::browse::SortSpec {
            column: "a".into(),
            dir: crate::types::browse::SortDir::Desc,
            nulls: None,
        }];
        let started = Instant::now();
        let err = browse_page(
            session.as_ref(),
            &meta,
            &req,
            Some(Duration::from_millis(30)),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, AppError::QueryTimeout { .. }),
            "unexpected {err:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(20));
        let selects: Vec<_> = session
            .drain_trace()
            .into_iter()
            .filter(|t| t.sql.contains("fx3_slow"))
            .collect();
        assert_eq!(
            selects.len(),
            1,
            "the select ran more than once: {selects:#?}"
        );
        assert!(selects[0].error.is_some());
    }

    /// A table the app could not key is addressed by its rowid first; one that has none (WITHOUT
    /// ROWID) answers `no such column: rowid`, and the page loads once more addressed by all its
    /// columns.
    #[tokio::test]
    async fn a_table_without_a_rowid_loads_addressed_by_all_columns() {
        let (session, _dir) = sqlite_session().await;
        let mut meta = meta(session.as_ref(), "fx3_wr").await;
        meta.primary_key.clear();
        assert_eq!(identity_for(Driver::Sqlite, &meta), RowIdentity::Rowid);
        session.drain_trace();
        let page = browse_page(session.as_ref(), &meta, &page_request("fx3_wr"), None)
            .await
            .unwrap();
        assert_eq!(page.identity, RowIdentity::None);
        assert_eq!(page.row_ids, None);
        assert_eq!(page.rows.len(), 2);
        let trace = session.drain_trace();
        assert_eq!(trace.len(), 2, "{trace:#?}");
        assert!(trace[0].error.as_deref().unwrap().contains("rowid"));
        assert!(trace[1].error.is_none());
    }

    /// An ordinary page load is untouched, and still returns the rowids of a keyless table.
    #[tokio::test]
    async fn a_keyless_sqlite_table_still_loads_with_its_rowids() {
        let (session, _dir) = sqlite_session().await;
        let meta = meta(session.as_ref(), "cache").await;
        let page = browse_page(session.as_ref(), &meta, &page_request("cache"), None)
            .await
            .unwrap();
        assert_eq!(page.identity, RowIdentity::Rowid);
        assert_eq!(page.row_ids, Some(vec![1, 2, 3]));
        assert_eq!(page.rows.len(), 3);
    }

    fn column(kind: ValueKind, data_type: &str) -> ColumnMeta {
        ColumnMeta {
            name: "c".into(),
            ordinal: 1,
            data_type: data_type.into(),
            kind,
            nullable: true,
            default_expr: None,
            is_pk: false,
            pk_ordinal: None,
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

    #[test]
    fn estimates_only_big_unfiltered_tables() {
        let meta = |est: Option<i64>| TableMeta {
            table: TableRef::new(None, "t"),
            kind: TableKind::Table,
            columns: vec![],
            primary_key: vec![],
            indexes: vec![],
            foreign_keys: vec![],
            triggers: vec![],
            comment: None,
            row_estimate: est,
            ddl: None,
            checks: vec![],
            options: None,
        };
        let big = meta(Some(600_000));
        assert_eq!(
            estimated_count(&big, false, false, DEFAULT_ESTIMATE_ABOVE).map(|c| (c.count, c.exact)),
            Some((600_000, false))
        );
        // An exact count, a filter, a higher threshold or no estimate at all: count.
        assert!(estimated_count(&big, false, true, DEFAULT_ESTIMATE_ABOVE).is_none());
        assert!(estimated_count(&big, true, false, DEFAULT_ESTIMATE_ABOVE).is_none());
        assert!(estimated_count(&big, false, false, 1_000_000).is_none());
        assert!(estimated_count(&meta(None), false, false, 0).is_none());
        assert!(estimated_count(&meta(Some(500_000)), false, false, 500_000).is_none());
    }

    #[test]
    fn decodes_fetched_cells_per_engine() {
        let bytes = column(ValueKind::Bytes, "bytea");
        let hex = Value::Text("00ff".into());
        let whole = Value::from_bytes_uncapped(&[0, 255]);
        for driver in [Driver::Postgres, Driver::Mysql] {
            assert_eq!(
                fetched_value(driver, &bytes, vec![hex.clone()]).unwrap(),
                whole
            );
            assert_eq!(
                fetched_value(driver, &bytes, vec![Value::Null]).unwrap(),
                Value::Null
            );
        }
        // MySQL may hand HEX() back as binary.
        assert_eq!(
            fetched_value(Driver::Mysql, &bytes, vec![Value::from_bytes(b"00ff")]).unwrap(),
            whole
        );
        // SQLite: the second column carries the hex only for blobs, whatever the declared kind.
        let other = column(ValueKind::Other, "");
        let preview = Value::from_bytes(&[0, 255]);
        assert_eq!(
            fetched_value(Driver::Sqlite, &other, vec![preview, hex]).unwrap(),
            whole
        );
        assert_eq!(
            fetched_value(Driver::Sqlite, &bytes, vec![Value::Null, Value::Null]).unwrap(),
            Value::Null
        );
        assert_eq!(
            fetched_value(Driver::Sqlite, &other, vec![Value::Int(3), Value::Null]).unwrap(),
            Value::Int(3)
        );
        // No row at all reads as NULL.
        assert_eq!(
            fetched_value(Driver::Sqlite, &other, vec![]).unwrap(),
            Value::Null
        );
        // PG text is decoded by the column kind.
        let int = column(ValueKind::Int, "integer");
        assert_eq!(
            fetched_value(Driver::Postgres, &int, vec![Value::Text("42".into())]).unwrap(),
            Value::Int(42)
        );
    }
}
