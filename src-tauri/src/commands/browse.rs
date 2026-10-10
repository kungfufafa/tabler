use std::sync::Arc;
use std::time::Instant;

use tauri::State;
use uuid::Uuid;

use crate::drivers::postgres::decode::text_to_value;
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::mutate::{TableSpec, row_key_where};
use crate::sqlgen::select::{ROWID_ALIAS, SelectSpec, build_count, build_select, inline_params};
use crate::state::{AppState, Connection};
use crate::types::browse::{BrowsePage, BrowseRequest, FilterGroup, RowCount, RowIdentity, RowKey};
use crate::types::profile::Driver;
use crate::types::schema::{ColumnMeta, TableMeta, TableRef};
use crate::types::value::Value;

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

/// Fetches one full cell (used for blobs/long text beyond the preview cap and for refreshes).
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
    let dialect = session.dialect();
    let meta = cached_meta(&conn, &table).await?;
    let col = meta
        .columns
        .iter()
        .find(|c| c.name == column)
        .ok_or_else(|| AppError::InvalidInput(format!("unknown column \"{column}\"")))?;
    let mut params = Vec::new();
    let spec = TableSpec {
        dialect,
        table: &meta.table,
        columns: &meta.columns,
    };
    let w = row_key_where(&spec, &key, &mut params)?;
    let where_sql = format!("{}{}", w.condition, w.suffix);
    let expr = match dialect {
        Dialect::Postgres => format!("{}::text", dialect.quote_ident(&col.name)),
        _ => dialect.quote_ident(&col.name),
    };
    let sql = format!(
        "SELECT {expr} FROM {} WHERE {where_sql} LIMIT 1",
        dialect.qualify(meta.table.schema.as_deref(), &meta.table.name)
    );
    let rs = session.query(&sql, &params).await?;
    let cell = rs
        .rows
        .into_iter()
        .next()
        .and_then(|r| r.into_iter().next())
        .unwrap_or(Value::Null);
    Ok(match (session.driver(), cell) {
        (Driver::Postgres, Value::Text(text)) => text_to_value(col.kind, &col.data_type, &text),
        (_, v) => v,
    })
}
