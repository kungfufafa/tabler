use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use tauri::State;
use uuid::Uuid;

use crate::commands::browse::cached_meta;
use crate::drivers::native::{NativePlan, NativeSession};
use crate::drivers::postgres::decode::text_to_value;
use crate::drivers::{Session, TxStatement};
use crate::error::{AppError, ErrorPayload};
use crate::sqlgen::SqlDialect;
use crate::sqlgen::mutate::ordered;
use crate::sqlgen::sql::{self, ColumnForm, TableTarget};
use crate::state::{AppState, Connection};
use crate::types::browse::RowKey;
use crate::types::caps::{Feature, TxMode};
use crate::types::mutate::{ApplyRequest, ApplyResult, RowResult, ServerExpr, StagedChange};
use crate::types::profile::Driver;
use crate::types::query::ExecOutcome;
use crate::types::schema::{ColumnMeta, TableMeta};
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

    /// One result per statement after a session that applies changes one by one (`TxMode` other
    /// than `Atomic` and `DataOnly`) stopped at `failed`: the statements before it were applied
    /// and say so with no error, so the renderer unstages their changes; `failed` carries
    /// `payload` and the ones after it were not sent. When the failure has no statement every
    /// change keeps the error (none is known to be applied).
    pub fn partial_results(
        &self,
        req: &ApplyRequest,
        failed: Option<usize>,
        payload: &ErrorPayload,
    ) -> Vec<RowResult> {
        let not_sent =
            AppError::InvalidInput("not applied: an earlier change failed".into()).to_payload();
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
                    error: match failed {
                        Some(f) if i < f => None,
                        Some(f) if i > f => Some(not_sent.clone()),
                        _ => Some(payload.clone()),
                    },
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

/// The features the changes of `req` need: their kind, a key other than the primary key (the
/// all-columns match or SQLite's rowid), and DEFAULT or server expressions.
pub fn change_features(req: &ApplyRequest) -> Vec<Feature> {
    let keyless = |key: &RowKey| matches!(key, RowKey::Rowid(_) | RowKey::AllColumns(_));
    let expressions = |exprs: &Option<BTreeMap<String, ServerExpr>>| {
        exprs.as_ref().is_some_and(|e| !e.is_empty())
    };
    let mut out = Vec::new();
    for change in &req.changes {
        match change {
            StagedChange::Update { key, exprs, .. } => {
                out.push(Feature::EditRows);
                if keyless(key) {
                    out.push(Feature::EditWithoutKey);
                }
                if expressions(exprs) {
                    out.push(Feature::ServerExpressions);
                }
            }
            StagedChange::Insert { exprs, .. } => {
                out.push(Feature::InsertRows);
                if expressions(exprs) {
                    out.push(Feature::ServerExpressions);
                }
            }
            StagedChange::Delete { key, .. } => {
                out.push(Feature::DeleteRows);
                if keyless(key) {
                    out.push(Feature::EditWithoutKey);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Plans a request on a connection (refused on a read-only one, and before anything is read
/// when the session lacks a feature the changes need). Rows are asked back only from a session
/// that can return them (`Returning`); the others apply the same changes without.
pub async fn plan_rows(conn: &Connection, req: &ApplyRequest) -> Result<PlannedRows, AppError> {
    if conn.profile.options.read_only {
        return Err(AppError::Unsupported("this connection is read-only".into()));
    }
    let session = conn.session().await?;
    let caps = session.capabilities();
    caps.require_all(&change_features(req))?;
    let returning = req.returning && caps.has(Feature::Returning);
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
        let (built, expect) = sql::change(dialect, &target, change, returning)?;
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

/// Plans a request on a store that speaks no SQL, with the same refusals as `plan_rows`: the
/// store's own plan, whose preview is what its `apply_changes` sends.
pub async fn plan_native(
    conn: &Connection,
    store: &dyn NativeSession,
    req: &ApplyRequest,
) -> Result<(Arc<TableMeta>, NativePlan), AppError> {
    if conn.profile.options.read_only {
        return Err(AppError::Unsupported("this connection is read-only".into()));
    }
    store.capabilities().require_all(&change_features(req))?;
    let meta = cached_meta(conn, &req.table).await?;
    let plan = store.plan_changes(&meta, req)?;
    Ok((meta, plan))
}

/// The preview text of a store's plan, in its order.
fn native_preview(plan: &NativePlan) -> Vec<String> {
    plan.preview.iter().map(|(_, text)| text.clone()).collect()
}

#[tauri::command]
pub async fn preview_changes(
    state: State<'_, AppState>,
    connection_id: Uuid,
    req: ApplyRequest,
) -> Result<Vec<String>, AppError> {
    preview_changes_on(&state, connection_id, &req).await
}

/// The body of [`preview_changes`], callable without a Tauri runtime.
pub(crate) async fn preview_changes_on(
    state: &AppState,
    connection_id: Uuid,
    req: &ApplyRequest,
) -> Result<Vec<String>, AppError> {
    let conn = state.connection(connection_id).await?;
    if let Session::Native(store) = conn.any_session().await {
        let (_, plan) = plan_native(&conn, store.as_ref(), req).await?;
        return Ok(native_preview(&plan));
    }
    Ok(plan_rows(&conn, req).await?.sql())
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
    apply_changes_on(&state, connection_id, req).await
}

/// Applies a request on a store that speaks no SQL: the store reports each change's outcome, so
/// after a partial failure the renderer unstages the changes that were applied.
async fn apply_native(
    conn: &Connection,
    store: &dyn NativeSession,
    req: &ApplyRequest,
) -> Result<ApplyResult, AppError> {
    let started = Instant::now();
    let (meta, plan) = plan_native(conn, store, req).await?;
    if req.changes.is_empty() {
        return Ok(ApplyResult {
            success: true,
            results: vec![],
            sql: native_preview(&plan),
            error: None,
            duration_ms: 0,
        });
    }
    log::info!(
        "apply_changes {} change(s) on {}",
        req.changes.len(),
        req.table.name
    );
    let mut result = store.apply_changes(&meta, req, plan).await;
    result.duration_ms = started.elapsed().as_millis() as u64;
    Ok(result)
}

/// The body of [`apply_changes`], callable without a Tauri runtime.
pub(crate) async fn apply_changes_on(
    state: &AppState,
    connection_id: Uuid,
    req: ApplyRequest,
) -> Result<ApplyResult, AppError> {
    let conn = state.connection(connection_id).await?;
    if let Session::Native(store) = conn.any_session().await {
        return apply_native(&conn, store.as_ref(), &req).await;
    }
    let started = Instant::now();
    let planned = plan_rows(&conn, &req).await?;
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
            let results = match session.capabilities().data.transactions {
                TxMode::Atomic | TxMode::DataOnly => {
                    planned.failed_results(&req, payload.statement_index, &payload)
                }
                TxMode::Batch | TxMode::Sequential | TxMode::None => {
                    planned.partial_results(&req, payload.statement_index, &payload)
                }
            };
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
