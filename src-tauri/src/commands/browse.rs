use std::sync::Arc;
use std::time::Instant;

use tauri::State;
use uuid::Uuid;

use crate::drivers::DbSession;
use crate::drivers::postgres::decode::text_to_value;
use crate::error::AppError;
use crate::sqlgen::mutate::TableSpec;
use crate::sqlgen::select::{
    ROWID_ALIAS, SelectSpec, build_count, build_fetch_cell, build_select, inline_params,
};
use crate::state::{AppState, Connection};
use crate::types::browse::{BrowsePage, BrowseRequest, FilterGroup, RowCount, RowIdentity, RowKey};
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

#[tauri::command]
pub async fn browse_table(
    state: State<'_, AppState>,
    connection_id: Uuid,
    req: BrowseRequest,
) -> Result<BrowsePage, AppError> {
    if req.limit == 0 || req.limit > 10_000 {
        return Err(AppError::InvalidInput(
            "limit must be between 1 and 10000".into(),
        ));
    }
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await;
    let driver = session.driver();
    log::debug!(
        "browse_table {}.{} limit={} offset={}",
        req.table.schema.as_deref().unwrap_or(""),
        req.table.name,
        req.limit,
        req.offset
    );
    let dialect = session.dialect();
    let meta = cached_meta(&conn, &req.table).await?;
    let mut identity = identity_for(driver, &meta);

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
        match session.query(&built.sql, &built.params).await {
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
            Err(e) if identity == RowIdentity::Rowid && attempt == 1 => {
                log::debug!("rowid select failed ({e}); falling back to all-columns identity");
                identity = RowIdentity::None;
            }
            Err(e) => return Err(e),
        }
    }
}

#[tauri::command]
pub async fn count_table_rows(
    state: State<'_, AppState>,
    connection_id: Uuid,
    table: TableRef,
    filter: Option<FilterGroup>,
    exact: Option<bool>,
) -> Result<RowCount, AppError> {
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await;
    let meta = cached_meta(&conn, &table).await?;
    let exact = exact.unwrap_or(false);
    // Large unfiltered tables: use the planner estimate unless an exact count was requested.
    if !exact
        && filter.is_none()
        && let Some(est) = meta.row_estimate
        && est > 200_000
    {
        return Ok(RowCount {
            count: est,
            exact: false,
        });
    }
    let built = build_count(
        session.dialect(),
        &meta.table,
        &meta.columns,
        filter.as_ref(),
    )?;
    let rs = session.query(&built.sql, &built.params).await?;
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

/// Fetches one full cell (the inspector's "Load full value" for blobs beyond the preview cap).
#[tauri::command]
pub async fn fetch_cell(
    state: State<'_, AppState>,
    connection_id: Uuid,
    table: TableRef,
    key: RowKey,
    column: String,
) -> Result<Value, AppError> {
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await;
    let meta = cached_meta(&conn, &table).await?;
    fetch_cell_inner(session.as_ref(), &meta, &column, &key).await
}

/// `fetch_cell` without Tauri state, for tests.
pub async fn fetch_cell_inner(
    session: &dyn DbSession,
    meta: &TableMeta,
    column: &str,
    key: &RowKey,
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
    let rs = session.query(&sql, &params).await?;
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
             CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT);",
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
        let session = connect(ConnectTarget {
            profile: &profile,
            password: None,
            host_override: None,
        })
        .await
        .unwrap();
        (session, dir)
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
        }
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
