use std::collections::BTreeMap;
use std::time::Instant;

use tauri::State;
use uuid::Uuid;

use crate::commands::browse::cached_meta;
use crate::drivers::TxStatement;
use crate::drivers::postgres::decode::text_to_value;
use crate::error::AppError;
use crate::sqlgen::mutate::{TableSpec, build_change, ordered};
use crate::sqlgen::select::inline_params;
use crate::state::AppState;
use crate::types::mutate::{ApplyRequest, ApplyResult, RowResult};
use crate::types::profile::Driver;
use crate::types::query::ExecOutcome;
use crate::types::schema::ColumnMeta;
use crate::types::value::Value;

/// Statements for a request, in execution order, with the change they came from.
async fn plan(
    state: &AppState,
    connection_id: Uuid,
    req: &ApplyRequest,
) -> Result<
    (
        Vec<(usize, TxStatement)>,
        Vec<ColumnMeta>,
        Driver,
        crate::sqlgen::Dialect,
    ),
    AppError,
> {
    let conn = state.connection(connection_id).await?;
    if conn.profile.options.read_only {
        return Err(AppError::Unsupported("this connection is read-only".into()));
    }
    let session = conn.session().await;
    let meta = cached_meta(&conn, &req.table).await?;
    let dialect = session.dialect();
    let spec = TableSpec {
        dialect,
        table: &meta.table,
        columns: &meta.columns,
    };
    let mut out = Vec::with_capacity(req.changes.len());
    for change in ordered(&req.changes) {
        let index = req
            .changes
            .iter()
            .position(|c| std::ptr::eq(c, change))
            .unwrap_or(0);
        let (built, expect) = build_change(&spec, change, req.returning)?;
        let mut st = TxStatement::new(built.sql, built.params);
        st.expect_affected = expect;
        out.push((index, st));
    }
    Ok((out, meta.columns.clone(), session.driver(), dialect))
}

#[tauri::command]
pub async fn preview_changes(
    state: State<'_, AppState>,
    connection_id: Uuid,
    req: ApplyRequest,
) -> Result<Vec<String>, AppError> {
    let (statements, _, _, dialect) = plan(&state, connection_id, &req).await?;
    Ok(statements
        .iter()
        .map(|(_, st)| inline_params(dialect, &st.sql, &st.params))
        .collect())
}

fn returned_row(
    driver: Driver,
    columns: &[ColumnMeta],
    outcome: &ExecOutcome,
) -> Option<BTreeMap<String, Value>> {
    let rs = outcome.rows.as_ref()?;
    let row = rs.rows.first()?;
    let mut map = BTreeMap::new();
    for (i, rc) in rs.columns.iter().enumerate() {
        let v = row.get(i).cloned().unwrap_or(Value::Null);
        let v = match (driver, v, columns.iter().find(|c| c.name == rc.name)) {
            (Driver::Postgres, Value::Text(text), Some(col)) => {
                text_to_value(col.kind, &col.data_type, &text)
            }
            (_, v, _) => v,
        };
        map.insert(rc.name.clone(), v);
    }
    Some(map)
}

#[tauri::command]
pub async fn apply_changes(
    state: State<'_, AppState>,
    connection_id: Uuid,
    req: ApplyRequest,
) -> Result<ApplyResult, AppError> {
    let started = Instant::now();
    let (statements, columns, driver, dialect) = plan(&state, connection_id, &req).await?;
    let sql: Vec<String> = statements
        .iter()
        .map(|(_, st)| inline_params(dialect, &st.sql, &st.params))
        .collect();
    if statements.is_empty() {
        return Ok(ApplyResult {
            success: true,
            results: vec![],
            sql,
            error: None,
            duration_ms: 0,
        });
    }
    let session = state.session(connection_id).await?;
    log::info!(
        "apply_changes {} statement(s) on {}",
        statements.len(),
        req.table.name
    );
    match session
        .transaction(statements.iter().map(|(_, st)| st.clone()).collect())
        .await
    {
        Ok(outcomes) => {
            let results = statements
                .iter()
                .zip(outcomes.iter())
                .map(|((index, _), outcome)| {
                    let change = &req.changes[*index];
                    RowResult {
                        row_id: change.row_id().to_string(),
                        op: change.op_name().to_string(),
                        affected: outcome.affected,
                        returned: returned_row(driver, &columns, outcome),
                        error: None,
                    }
                })
                .collect();
            Ok(ApplyResult {
                success: true,
                results,
                sql,
                error: None,
                duration_ms: started.elapsed().as_millis() as u64,
            })
        }
        Err(err) => {
            let payload = err.to_payload();
            let failed = payload.statement_index;
            let results = statements
                .iter()
                .enumerate()
                .map(|(i, (index, _))| {
                    let change = &req.changes[*index];
                    RowResult {
                        row_id: change.row_id().to_string(),
                        op: change.op_name().to_string(),
                        affected: 0,
                        returned: None,
                        error: if failed == Some(i) {
                            Some(payload.clone())
                        } else {
                            None
                        },
                    }
                })
                .collect();
            Ok(ApplyResult {
                success: false,
                results,
                sql,
                error: Some(payload),
                duration_ms: started.elapsed().as_millis() as u64,
            })
        }
    }
}
