use std::collections::BTreeMap;
use std::time::Instant;

use tauri::State;
use uuid::Uuid;

use crate::commands::browse::cached_meta;
use crate::drivers::TxStatement;
use crate::drivers::postgres::decode::text_to_value;
use crate::error::{AppError, ErrorPayload};
use crate::sqlgen::SqlDialect;
use crate::sqlgen::mutate::ordered;
use crate::sqlgen::sql::{self, ColumnForm, TableTarget};
use crate::state::{AppState, Connection};
use crate::types::mutate::{ApplyRequest, ApplyResult, RowResult};
use crate::types::profile::Driver;
use crate::types::query::ExecOutcome;
use crate::types::schema::ColumnMeta;
use crate::types::value::Value;

/// A request planned against the table's cached metadata: its statements in execution order,
/// each with the index of the change it came from.
pub struct PlannedRows {
    pub statements: Vec<(usize, TxStatement)>,
    pub columns: Vec<ColumnMeta>,
    pub driver: Driver,
    pub dialect: SqlDialect,
}

impl PlannedRows {
    /// The statements with their literals inlined (what the preview shows).
    pub fn sql(&self) -> Vec<String> {
        self.statements
            .iter()
            .map(|(_, st)| sql::inline_params(self.dialect, &st.sql, &st.params))
            .collect()
    }

    /// One result per statement after the transaction ran (`outcomes` in statement order).
    pub fn results(&self, req: &ApplyRequest, outcomes: &[ExecOutcome]) -> Vec<RowResult> {
        self.statements
            .iter()
            .zip(outcomes.iter())
            .map(|((index, _), outcome)| {
                let change = &req.changes[*index];
                RowResult {
                    row_id: change.row_id().to_string(),
                    op: change.op_name().to_string(),
                    affected: outcome.affected,
                    returned: returned_row(self.dialect, &self.columns, outcome),
                    error: None,
                }
            })
            .collect()
    }

    /// One result per statement after the transaction failed: `failed` (an index into the
    /// statements) carries `payload`.
    pub fn failed_results(
        &self,
        req: &ApplyRequest,
        failed: Option<usize>,
        payload: &ErrorPayload,
    ) -> Vec<RowResult> {
        self.statements
            .iter()
            .enumerate()
            .map(|(i, (index, _))| {
                let change = &req.changes[*index];
                RowResult {
                    row_id: change.row_id().to_string(),
                    op: change.op_name().to_string(),
                    affected: 0,
                    returned: None,
                    error: (failed == Some(i)).then(|| payload.clone()),
                }
            })
            .collect()
    }
}

/// Plans a request on a connection (refused on a read-only one).
pub async fn plan_rows(conn: &Connection, req: &ApplyRequest) -> Result<PlannedRows, AppError> {
    if conn.profile.options.read_only {
        return Err(AppError::Unsupported("this connection is read-only".into()));
    }
    let session = conn.session().await;
    let meta = cached_meta(conn, &req.table).await?;
    let dialect = session.dialect();
    let target = TableTarget {
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
        let (built, expect) = sql::change(dialect, &target, change, req.returning)?;
        let mut st = TxStatement::new(built.sql, built.params);
        st.expect_affected = expect;
        out.push((index, st));
    }
    Ok(PlannedRows {
        statements: out,
        columns: meta.columns.clone(),
        driver: session.driver(),
        dialect,
    })
}

async fn plan(
    state: &AppState,
    connection_id: Uuid,
    req: &ApplyRequest,
) -> Result<PlannedRows, AppError> {
    let conn = state.connection(connection_id).await?;
    plan_rows(&conn, req).await
}

#[tauri::command]
pub async fn preview_changes(
    state: State<'_, AppState>,
    connection_id: Uuid,
    req: ApplyRequest,
) -> Result<Vec<String>, AppError> {
    Ok(plan(&state, connection_id, &req).await?.sql())
}

fn returned_row(
    dialect: SqlDialect,
    columns: &[ColumnMeta],
    outcome: &ExecOutcome,
) -> Option<BTreeMap<String, Value>> {
    let rs = outcome.rows.as_ref()?;
    let row = rs.rows.first()?;
    let mut map = BTreeMap::new();
    for (i, rc) in rs.columns.iter().enumerate() {
        let v = row.get(i).cloned().unwrap_or(Value::Null);
        // RETURNING lists the columns as the select does (PG: as text).
        let v = match (v, columns.iter().find(|c| c.name == rc.name)) {
            (Value::Text(text), Some(col))
                if sql::column_form(dialect, col) != ColumnForm::Native =>
            {
                text_to_value(col.kind, &col.data_type, &text)
            }
            (v, _) => v,
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
    let planned = plan(&state, connection_id, &req).await?;
    let sql = planned.sql();
    if planned.statements.is_empty() {
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
        planned.statements.len(),
        req.table.name
    );
    match session
        .transaction(
            planned
                .statements
                .iter()
                .map(|(_, st)| st.clone())
                .collect(),
        )
        .await
    {
        Ok(outcomes) => Ok(ApplyResult {
            success: true,
            results: planned.results(&req, &outcomes),
            sql,
            error: None,
            duration_ms: started.elapsed().as_millis() as u64,
        }),
        Err(err) => {
            let payload = err.to_payload();
            let results = planned.failed_results(&req, payload.statement_index, &payload);
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
