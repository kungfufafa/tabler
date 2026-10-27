//! The workspace commit (TablePlus §9: "pending changes across multiple tabs of the same
//! workspace are committed together in one transaction"). ⌘S sends every pending GUI change of
//! a connection as groups (the staged rows of its table tabs and query results, the staged DDL
//! of its structure views, its pending sidebar operations) and they run in TablePlus's order:
//! data edits, then structure, then object operations.
//!
//! PostgreSQL and SQLite run everything in ONE transaction while every plan is transactional:
//! a failure rolls all of it back. MySQL does too while only row edits are pending (they run in
//! one transaction whatever the number of tabs). Otherwise (MySQL with structure or object
//! operations, whose DDL commits implicitly, or a plan that cannot run in a transaction such as
//! a SQLite table rebuild) the data edits run first in a transaction of their own, then each
//! structure change / operation, stopping at the first failure; the result says precisely what
//! stayed applied. A failing data transaction there leaves nothing applied (the structure never
//! ran), so the result is atomic too: "All changes were reverted".

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::time::Instant;

use futures::FutureExt as _;
use tauri::State;
use uuid::Uuid;

use crate::commands::ddl::{DDL_LOCK_TIMEOUT_SECS, apply_plan, explain_lock_wait, plan_for};
use crate::commands::mutate::{PlannedRows, plan_rows};
use crate::drivers::{DbSession, TxStatement};
use crate::error::{AppError, ErrorPayload};
use crate::objects;
use crate::sqlgen::Dialect;
use crate::sqlgen::definition::{
    create_statement, definition_plan, qualify_routine, without_definer,
};
use crate::state::{AppState, Connection};
use crate::types::caps::Feature;
use crate::types::ddl::{DdlPlan, DdlResult};
use crate::types::mutate::{
    ApplyRequest, DefinitionRestore, GroupPlan, GroupResult, GroupStatus, WorkspaceGroup,
    WorkspacePlan, WorkspaceResult,
};
use crate::types::objects::RoutineRef;

/// A group ready to run.
pub enum PlannedGroup {
    /// Row edits: parameterised statements guarded by their expected row counts.
    Rows {
        req: ApplyRequest,
        planned: PlannedRows,
    },
    /// Structure changes, a definition (`created`: the routine its text creates), or (`object`)
    /// a sidebar operation. `replaces` is the routine a MySQL definition drops before it creates
    /// the new text: MySQL has no transactional DDL, so the old version is read first and put
    /// back when the CREATE fails (`run_mysql_replacement`).
    Plan {
        plan: DdlPlan,
        object: bool,
        created: Option<RoutineRef>,
        replaces: Option<RoutineRef>,
    },
}

impl PlannedGroup {
    fn sql(&self) -> Vec<String> {
        match self {
            PlannedGroup::Rows { planned, .. } => planned.sql(),
            PlannedGroup::Plan { plan, .. } => plan.statements.clone(),
        }
    }

    fn transactional(&self) -> bool {
        match self {
            PlannedGroup::Rows { .. } => true,
            PlannedGroup::Plan { plan, .. } => plan.transactional,
        }
    }

    /// TablePlus's order: data edits, then structure (and definitions), then object operations.
    fn rank(&self) -> u8 {
        match self {
            PlannedGroup::Rows { .. } => 0,
            PlannedGroup::Plan { object: false, .. } => 1,
            PlannedGroup::Plan { object: true, .. } => 2,
        }
    }
}

/// The order the groups run in (indexes into `groups`): by `rank`, request order within one.
pub fn run_order(groups: &[PlannedGroup]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by_key(|&i| groups[i].rank());
    order
}

/// Whether everything can run in one transaction: every plan is transactional and, on MySQL
/// (whose DDL commits on its own), nothing but row edits is pending.
pub fn is_atomic(dialect: Dialect, groups: &[PlannedGroup]) -> bool {
    atomic_for(dialect, groups.iter())
}

fn atomic_for<'a>(dialect: Dialect, mut groups: impl Iterator<Item = &'a PlannedGroup>) -> bool {
    groups.all(|g| {
        g.transactional() && (dialect != Dialect::Mysql || matches!(g, PlannedGroup::Rows { .. }))
    })
}

/// A definition saved from its SQL tab as a planned group. On MySQL the routine it drops is
/// remembered (`PlannedGroup::Plan::replaces`) so a failing CREATE can be undone.
pub fn plan_definition(
    dialect: Dialect,
    drop: Option<&RoutineRef>,
    sql: &str,
) -> Result<PlannedGroup, AppError> {
    let def = definition_plan(dialect, drop, sql)?;
    Ok(PlannedGroup::Plan {
        plan: def.plan,
        object: false,
        created: def.created,
        replaces: def.dropped.filter(|_| dialect == Dialect::Mysql),
    })
}

async fn plan_group(
    conn: &Connection,
    session: &dyn DbSession,
    group: &WorkspaceGroup,
) -> Result<PlannedGroup, AppError> {
    Ok(match group {
        WorkspaceGroup::Rows { request } => PlannedGroup::Rows {
            planned: plan_rows(conn, request).await?,
            req: request.clone(),
        },
        WorkspaceGroup::Ddl { table, ops } => PlannedGroup::Plan {
            plan: plan_for(conn, table.as_ref(), ops).await?,
            object: false,
            created: None,
            replaces: None,
        },
        WorkspaceGroup::Object { op } => PlannedGroup::Plan {
            plan: objects::plan_op(session, op).await?,
            object: true,
            created: None,
            replaces: None,
        },
        WorkspaceGroup::Definition { drop, sql } => plan_definition(
            session.core_dialect(Feature::WorkspaceCommit)?,
            drop.as_ref(),
            sql,
        )?,
    })
}

fn group_result(status: GroupStatus, sql: Vec<String>) -> GroupResult {
    let executed = if status == GroupStatus::Applied {
        sql.clone()
    } else {
        Vec::new()
    };
    GroupResult {
        status,
        sql,
        executed,
        failed_index: None,
        rows: Vec::new(),
        routine: None,
        restore: None,
        error: None,
    }
}

/// The failing statement of a batch as (group, index in the group), when the driver said.
fn origin_of(origins: &[Option<(usize, usize)>], payload: &ErrorPayload) -> Option<(usize, usize)> {
    payload
        .statement_index
        .and_then(|i| origins.get(i).copied().flatten())
}

/// Marks the groups of a failed transaction: the failing one `Failed`, those before it in the
/// run order `RolledBack`, those after it `NotRun` (all `RolledBack` when it is unknown).
fn fail_batch(
    results: &mut [GroupResult],
    in_batch: &[usize],
    groups: &[PlannedGroup],
    failed: Option<(usize, usize)>,
    payload: &ErrorPayload,
) {
    let mut after = false;
    for &g in in_batch {
        let r = &mut results[g];
        r.executed.clear();
        match failed {
            Some((fg, local)) if fg == g => {
                r.status = GroupStatus::Failed;
                r.failed_index = Some(local);
                let mut p = payload.clone();
                p.statement_index = Some(local);
                r.error = Some(p.clone());
                if let PlannedGroup::Rows { req, planned } = &groups[g] {
                    r.rows = planned.failed_results(req, Some(local), &p);
                }
                after = true;
            }
            _ => {
                r.status = if after {
                    GroupStatus::NotRun
                } else {
                    GroupStatus::RolledBack
                };
            }
        }
    }
}

/// Runs `in_batch` (indexes into `groups`, in order) as one `DbSession::transaction`. With
/// `structure`, PG bounds lock waits (`SET LOCAL` ends with the transaction) and SQLite commits
/// inside the batch, so a failing COMMIT is rolled back by the driver.
async fn run_batch(
    session: &dyn DbSession,
    dialect: Dialect,
    groups: &[PlannedGroup],
    in_batch: &[usize],
    results: &mut [GroupResult],
) -> Result<(), ErrorPayload> {
    let structure = in_batch
        .iter()
        .any(|&g| matches!(groups[g], PlannedGroup::Plan { .. }));
    let mut batch: Vec<TxStatement> = Vec::new();
    let mut origins: Vec<Option<(usize, usize)>> = Vec::new();
    if structure && dialect == Dialect::Postgres {
        batch.push(TxStatement::new(
            format!("SET LOCAL lock_timeout = '{DDL_LOCK_TIMEOUT_SECS}s'"),
            Vec::new(),
        ));
        origins.push(None);
    }
    for &g in in_batch {
        match &groups[g] {
            PlannedGroup::Rows { planned, .. } => {
                for (local, (_, st)) in planned.statements.iter().enumerate() {
                    batch.push(st.clone());
                    origins.push(Some((g, local)));
                }
            }
            PlannedGroup::Plan { plan, .. } => {
                for (local, sql) in plan.statements.iter().enumerate() {
                    batch.push(TxStatement::new(sql.clone(), Vec::new()));
                    origins.push(Some((g, local)));
                }
            }
        }
    }
    if origins.iter().all(Option::is_none) {
        for &g in in_batch {
            results[g].status = GroupStatus::Applied;
        }
        return Ok(());
    }
    if structure && dialect == Dialect::Sqlite {
        batch.push(TxStatement::new("COMMIT", Vec::new()));
        batch.push(TxStatement::new("BEGIN", Vec::new()));
        origins.extend([None, None]);
    }
    match session.transaction(batch).await {
        Ok(outcomes) => {
            for &g in in_batch {
                let r = &mut results[g];
                r.status = GroupStatus::Applied;
                r.executed = r.sql.clone();
                if let PlannedGroup::Rows { req, planned } = &groups[g] {
                    let own: Vec<_> = origins
                        .iter()
                        .zip(outcomes.iter())
                        .filter(|(o, _)| matches!(o, Some((og, _)) if *og == g))
                        .map(|(_, out)| out.clone())
                        .collect();
                    r.rows = planned.results(req, &own);
                }
            }
            Ok(())
        }
        Err(err) => {
            let mut payload = err.to_payload();
            explain_lock_wait(session, dialect, &mut payload);
            // A batch of one statement failed at that statement.
            let single = (origins.iter().flatten().count() == 1)
                .then(|| origins.iter().flatten().next().copied())
                .flatten();
            let failed = origin_of(&origins, &payload).or(single);
            fail_batch(results, in_batch, groups, failed, &payload);
            payload.statement_index = failed.map(|(_, local)| local);
            Err(payload)
        }
    }
}

/// Runs planned groups (see the module docs). Never fails: the outcome says what happened. A
/// session without the workspace commit runs none of them.
pub async fn run_workspace(session: &dyn DbSession, groups: &[PlannedGroup]) -> WorkspaceResult {
    let started = Instant::now();
    let mut results: Vec<GroupResult> = groups
        .iter()
        .map(|g| group_result(GroupStatus::NotRun, g.sql()))
        .collect();
    let dialect = match session.core_dialect(Feature::WorkspaceCommit) {
        Ok(dialect) => dialect,
        Err(e) => {
            return WorkspaceResult {
                success: false,
                // Nothing ran: "All changes were reverted" is what stayed.
                atomic: true,
                groups: results,
                failed_group: None,
                error: Some(e.to_payload()),
                duration_ms: 0,
            };
        }
    };
    let atomic = is_atomic(dialect, groups);
    let order = run_order(groups);
    let finish = |mut results: Vec<GroupResult>, error: Option<ErrorPayload>, atomic: bool| {
        for (r, g) in results.iter_mut().zip(groups) {
            if let (GroupStatus::Applied, PlannedGroup::Plan { created, .. }) = (r.status, g) {
                r.routine = created.clone();
            }
        }
        let failed_group = results.iter().position(|r| r.status == GroupStatus::Failed);
        WorkspaceResult {
            success: error.is_none(),
            atomic,
            groups: results,
            failed_group,
            error,
            duration_ms: started.elapsed().as_millis() as u64,
        }
    };

    if atomic {
        let error = run_batch(session, dialect, groups, &order, &mut results)
            .await
            .err();
        return finish(results, error, true);
    }

    // Data edits first, in one transaction of their own.
    let rows: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&g| matches!(groups[g], PlannedGroup::Rows { .. }))
        .collect();
    if !rows.is_empty()
        && let Err(payload) = run_batch(session, dialect, groups, &rows, &mut results).await
    {
        // The data transaction rolled back and the structure never ran: nothing stayed, which is
        // what "atomic" tells the renderer ("All changes were reverted").
        return finish(results, Some(payload), true);
    }
    // Then each plan on its own, stopping at the first failure.
    for &g in order.iter().filter(|g| !rows.contains(g)) {
        let PlannedGroup::Plan {
            plan,
            object,
            replaces,
            ..
        } = &groups[g]
        else {
            continue;
        };
        let (outcome, restore) = match replaces {
            Some(routine) if dialect == Dialect::Mysql => {
                run_mysql_replacement(session, plan, routine).await
            }
            _ if *object && dialect == Dialect::Mysql => {
                (objects::run_mysql_plan(session, plan).await, None)
            }
            _ => (apply_plan(session, plan).await, None),
        };
        let r = &mut results[g];
        r.restore = restore;
        r.executed = outcome.executed;
        match outcome.error {
            None => r.status = GroupStatus::Applied,
            Some(payload) => {
                r.status = GroupStatus::Failed;
                r.failed_index = outcome.failed_index.or(payload.statement_index);
                r.error = Some(payload.clone());
                return finish(results, Some(payload), false);
            }
        }
    }
    finish(results, None, false)
}

/// What was in the MySQL routine a definition is about to replace.
enum Previous {
    /// There is none (`DROP … IF EXISTS` has nothing to drop).
    Absent,
    /// Its `CREATE` text, as `SHOW CREATE` prints it.
    Text(String),
    /// The server would not show it (a routine of another definer, no `SHOW_ROUTINE`: the
    /// `Create Procedure` column is NULL): it could not be put back. Boxed because the payload is
    /// big, more so in builds where serde_json keeps key order (bson asks for it: drv-mongodb).
    Unreadable(Box<ErrorPayload>),
}

/// ER_SP_DOES_NOT_EXIST
const MYSQL_NO_SUCH_ROUTINE: i64 = 1305;
/// ER_SPECIFIC_ACCESS_DENIED_ERROR: creating a routine for a definer the account may not assume.
const MYSQL_CANNOT_ASSUME_DEFINER: i64 = 1227;

async fn read_previous(session: &dyn DbSession, routine: &RoutineRef) -> Previous {
    match objects::routine_definition(session, routine).await {
        Ok(script) => match create_statement(Dialect::Mysql, &script) {
            Some(create) => Previous::Text(create),
            None => Previous::Unreadable(Box::new(
                AppError::internal(format!("unexpected definition text of {}", routine.name))
                    .to_payload(),
            )),
        },
        Err(AppError::Db {
            vendor_code: Some(MYSQL_NO_SUCH_ROUTINE),
            ..
        }) => Previous::Absent,
        Err(e) => Previous::Unreadable(Box::new(e.to_payload())),
    }
}

/// Runs one statement the way a plan of its own runs.
async fn run_statement_plan(session: &dyn DbSession, sql: &str) -> DdlResult {
    let plan = DdlPlan {
        statements: vec![sql.to_string()],
        warnings: Vec::new(),
        transactional: false,
    };
    apply_plan(session, &plan).await
}

/// Creates `previous` again where the old routine lived (qualified with its schema: the pooled
/// connection may have another database selected). A definer the account cannot assume (the
/// reason a re-created routine can fail) is dropped from the text on a second try.
async fn restore_routine(
    session: &dyn DbSession,
    routine: &RoutineRef,
    previous: String,
) -> DefinitionRestore {
    let sql = match routine.schema.as_deref() {
        Some(schema) => qualify_routine(Dialect::Mysql, &previous, schema),
        None => previous,
    };
    let mut result = run_statement_plan(session, &sql).await;
    let mut redefined = false;
    if result
        .error
        .as_ref()
        .is_some_and(|e| e.vendor_code == Some(MYSQL_CANNOT_ASSUME_DEFINER))
        && let Some(plain) = without_definer(&sql)
    {
        let retry = run_statement_plan(session, &plain).await;
        if retry.error.is_none() {
            redefined = true;
            result = retry;
        }
    }
    DefinitionRestore {
        // Named with its schema, so creating it again from this text lands where it lived.
        previous: sql,
        restored: result.error.is_none(),
        without_definer: redefined,
        error: result.error,
    }
}

/// Why a MySQL replacement is not started: the old version cannot be read, so it could not be put
/// back if the new one fails (the DROP is committed on its own and cannot be undone).
fn unreadable_routine(routine: &RoutineRef, cause: ErrorPayload) -> ErrorPayload {
    let mut payload = AppError::InvalidInput(format!(
        "The current definition of {} cannot be read by this account, so it could not be put back \
         if the new one fails to create. Nothing was changed.",
        routine.name
    ))
    .to_payload();
    payload.detail = Some(cause.message);
    payload.hint = Some(
        "Grant the account SHOW_ROUTINE (or save as the routine's definer), or run the statements \
         in a SQL tab if you accept losing the old version on a failure."
            .into(),
    );
    payload
}

/// Runs the DROP and the CREATE of a MySQL definition (`plan`) as `apply_plan` would, after
/// reading the old version of `routine`. MySQL commits the DROP on its own, so a CREATE that then
/// fails (a syntax error, a definer the account cannot assume…) would lose the routine for good:
/// the old version is created again from what was read, and the outcome says how that went (a
/// restored routine leaves nothing in `executed`: its effects did not persist; one that could not
/// be restored hands its text back in the `DefinitionRestore`). When the old version cannot be
/// read at all, nothing is dropped and the group fails with that reason.
pub async fn run_mysql_replacement(
    session: &dyn DbSession,
    plan: &DdlPlan,
    routine: &RoutineRef,
) -> (DdlResult, Option<DefinitionRestore>) {
    let previous = match read_previous(session, routine).await {
        Previous::Unreadable(cause) => {
            let outcome = DdlResult {
                executed: Vec::new(),
                failed_index: None,
                error: Some(unreadable_routine(routine, *cause)),
                duration_ms: 0,
            };
            return (outcome, None);
        }
        Previous::Absent => None,
        Previous::Text(text) => Some(text),
    };
    let mut outcome = apply_plan(session, plan).await;
    // Only a failure after the DROP ran has something to undo.
    let Some(text) = previous.filter(|_| outcome.error.is_some() && !outcome.executed.is_empty())
    else {
        return (outcome, None);
    };
    let restore = restore_routine(session, routine, text).await;
    if restore.restored {
        outcome.executed.clear();
    }
    (outcome, Some(restore))
}

fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

/// Awaits `planning`, turning a panic inside it into an error: a bug in planning one group must
/// not take every window down with it (nor leave the commit UI waiting for an answer that never
/// comes). It catches only where panics unwind; the release profile sets `panic = "abort"`, so
/// there the planners (`definition_plan`'s tokenizer above all) must not panic in the first place.
pub async fn catch_planning_panic<T>(
    planning: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    match AssertUnwindSafe(planning).catch_unwind().await {
        Ok(planned) => planned,
        Err(panic) => {
            let message = panic_text(panic.as_ref());
            log::error!("planning a workspace group panicked: {message}");
            Err(AppError::internal(format!(
                "planning this change failed unexpectedly ({message}); nothing was run"
            )))
        }
    }
}

/// Plans every group with `plan`; one that cannot be planned (or whose planning panics) keeps its
/// error, the others are planned as usual.
pub async fn plan_each<'a, F, Fut>(
    groups: &'a [WorkspaceGroup],
    plan: F,
) -> Vec<Result<PlannedGroup, AppError>>
where
    F: Fn(&'a WorkspaceGroup) -> Fut,
    Fut: Future<Output = Result<PlannedGroup, AppError>>,
{
    let mut out = Vec::with_capacity(groups.len());
    for g in groups {
        out.push(catch_planning_panic(plan(g)).await);
    }
    out
}

/// Plans every group; one that cannot be planned keeps its error.
async fn plan_all(
    conn: &Connection,
    groups: &[WorkspaceGroup],
) -> Vec<Result<PlannedGroup, AppError>> {
    let session = conn.session().await;
    plan_each(groups, |g| plan_group(conn, session.as_ref(), g)).await
}

#[tauri::command]
pub async fn preview_workspace_changes(
    state: State<'_, AppState>,
    connection_id: Uuid,
    groups: Vec<WorkspaceGroup>,
) -> Result<WorkspacePlan, AppError> {
    let conn = state.connection(connection_id).await?;
    let dialect = conn
        .session()
        .await
        .core_dialect(Feature::WorkspaceCommit)?;
    let planned = plan_all(&conn, &groups).await;
    let atomic = planned.iter().all(Result::is_ok) && atomic_for(dialect, planned.iter().flatten());
    let groups = planned
        .into_iter()
        .map(|p| match p {
            Ok(g) => GroupPlan {
                statements: g.sql(),
                warnings: match &g {
                    PlannedGroup::Plan { plan, .. } => plan.warnings.clone(),
                    PlannedGroup::Rows { .. } => Vec::new(),
                },
                transactional: g.transactional(),
                error: None,
            },
            Err(e) => GroupPlan {
                statements: Vec::new(),
                warnings: Vec::new(),
                transactional: true,
                error: Some(e.to_payload()),
            },
        })
        .collect();
    Ok(WorkspacePlan { groups, atomic })
}

#[tauri::command]
pub async fn apply_workspace_changes(
    state: State<'_, AppState>,
    connection_id: Uuid,
    groups: Vec<WorkspaceGroup>,
) -> Result<WorkspaceResult, AppError> {
    let conn = state.connection(connection_id).await?;
    if conn.profile.options.read_only {
        return Err(AppError::Unsupported("this connection is read-only".into()));
    }
    let started = Instant::now();
    let planned = plan_all(&conn, &groups).await;
    // Nothing runs while a group cannot be planned.
    if let Some(bad) = planned.iter().position(Result::is_err) {
        let results = planned
            .iter()
            .enumerate()
            .map(|(i, p)| match p {
                Ok(g) => group_result(GroupStatus::NotRun, g.sql()),
                Err(e) => GroupResult {
                    error: Some(e.to_payload()),
                    ..group_result(
                        if i == bad {
                            GroupStatus::Failed
                        } else {
                            GroupStatus::NotRun
                        },
                        Vec::new(),
                    )
                },
            })
            .collect::<Vec<_>>();
        let error = results[bad].error.clone();
        return Ok(WorkspaceResult {
            success: false,
            atomic: false,
            groups: results,
            failed_group: Some(bad),
            error,
            duration_ms: started.elapsed().as_millis() as u64,
        });
    }
    let planned: Vec<PlannedGroup> = planned.into_iter().flatten().collect();
    let session = conn.session().await;
    log::info!(
        "apply_workspace_changes: {} group(s), {} statement(s)",
        planned.len(),
        planned.iter().map(|g| g.sql().len()).sum::<usize>()
    );
    let mut result = run_workspace(session.as_ref(), &planned).await;
    if planned
        .iter()
        .any(|g| matches!(g, PlannedGroup::Plan { .. }))
    {
        conn.invalidate(None).await;
    }
    if let Some(err) = &result.error {
        log::info!(
            "apply_workspace_changes failed in group {:?}: {}",
            result.failed_group,
            err.message
        );
    }
    result.duration_ms = started.elapsed().as_millis() as u64;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::drivers::sqlite::LiteSession;
    use crate::sqlgen::mutate::{TableSpec, build_change, ordered};
    use crate::types::browse::RowKey;
    use crate::types::mutate::StagedChange;
    use crate::types::objects::ObjectOp;
    use crate::types::profile::{ConnectionProfile, Driver, ProfileOptions, SslMode};
    use crate::types::schema::TableRef;
    use crate::types::value::Value;

    fn profile(path: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: Uuid::new_v4(),
            name: "commit3".into(),
            color: None,
            group: None,
            driver: Driver::Sqlite,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: Some(path.into()),
            ssh: None,
            options: ProfileOptions {
                sqlite_foreign_keys: true,
                ..ProfileOptions::default()
            },
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    async fn session() -> (tempfile::TempDir, LiteSession) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws.sqlite").to_string_lossy().into_owned();
        crate::drivers::sqlite::create_database_file(&path)
            .await
            .unwrap();
        let s = LiteSession::open(&profile(&path)).await.unwrap();
        s.execute(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, seen TEXT)",
            &[],
        )
        .await
        .unwrap();
        s.execute("INSERT INTO t (id, name) VALUES (1, 'a'), (2, 'b')", &[])
            .await
            .unwrap();
        s.execute("CREATE TABLE gone (x INTEGER)", &[])
            .await
            .unwrap();
        (dir, s)
    }

    async fn rows(s: &LiteSession, changes: Vec<StagedChange>) -> PlannedGroup {
        let table = TableRef::new(None, "t");
        let meta = s.table_meta(&table).await.unwrap();
        let spec = TableSpec {
            dialect: Dialect::Sqlite,
            table: &meta.table,
            columns: &meta.columns,
        };
        let req = ApplyRequest {
            table,
            changes,
            returning: true,
        };
        let statements = ordered(&req.changes)
            .into_iter()
            .map(|change| {
                let index = req
                    .changes
                    .iter()
                    .position(|c| std::ptr::eq(c, change))
                    .unwrap();
                let (b, expect) = build_change(&spec, change, true).unwrap();
                let mut st = TxStatement::new(b.sql, b.params);
                st.expect_affected = expect;
                (index, st)
            })
            .collect();
        PlannedGroup::Rows {
            planned: PlannedRows {
                statements,
                columns: meta.columns.clone(),
                driver: Driver::Sqlite,
                dialect: Dialect::Sqlite.into(),
            },
            req,
        }
    }

    fn rename(name: &str) -> StagedChange {
        StagedChange::Update {
            row_id: "r1".into(),
            key: RowKey::Pk(BTreeMap::from([("id".to_string(), Value::Int(1))])),
            changes: BTreeMap::from([("name".to_string(), Value::Text(name.into()))]),
            exprs: Some(BTreeMap::from([(
                "seen".to_string(),
                crate::types::mutate::ServerExpr::Now,
            )])),
        }
    }

    async fn names(s: &LiteSession) -> Vec<String> {
        s.query("SELECT name FROM t ORDER BY id", &[])
            .await
            .unwrap()
            .rows
            .into_iter()
            .filter_map(|r| r.first().and_then(Value::as_text))
            .collect()
    }

    async fn has_table(s: &LiteSession, name: &str) -> bool {
        !s.query(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            &[Value::Text(name.into())],
        )
        .await
        .unwrap()
        .rows
        .is_empty()
    }

    #[test]
    fn runs_data_then_structure_then_objects() {
        let groups = vec![
            PlannedGroup::Plan {
                plan: DdlPlan {
                    statements: vec!["DROP TABLE gone".into()],
                    warnings: Vec::new(),
                    transactional: true,
                },
                object: true,
                created: None,
                replaces: None,
            },
            PlannedGroup::Plan {
                plan: DdlPlan {
                    statements: vec!["ALTER TABLE t ADD c INT".into()],
                    warnings: Vec::new(),
                    transactional: true,
                },
                object: false,
                created: None,
                replaces: None,
            },
        ];
        assert_eq!(run_order(&groups), [1, 0]);
        assert!(is_atomic(Dialect::Postgres, &groups));
        assert!(!is_atomic(Dialect::Mysql, &groups));
    }

    /// A rows group that holds no statement (only its kind matters to `is_atomic`).
    fn empty_rows(dialect: Dialect) -> PlannedGroup {
        PlannedGroup::Rows {
            planned: PlannedRows {
                statements: Vec::new(),
                columns: Vec::new(),
                driver: match dialect {
                    Dialect::Mysql => Driver::Mysql,
                    Dialect::Postgres => Driver::Postgres,
                    Dialect::Sqlite => Driver::Sqlite,
                },
                dialect: dialect.into(),
            },
            req: ApplyRequest {
                table: TableRef::new(None, "t"),
                changes: Vec::new(),
                returning: true,
            },
        }
    }

    fn ddl(transactional: bool, object: bool) -> PlannedGroup {
        PlannedGroup::Plan {
            plan: DdlPlan {
                statements: vec!["ALTER TABLE t ADD c INT".into()],
                warnings: Vec::new(),
                transactional,
            },
            object,
            created: None,
            replaces: None,
        }
    }

    #[test]
    fn mysql_is_atomic_while_only_row_edits_are_pending() {
        // Row edits run in ONE transaction on MySQL, however many tabs they come from.
        assert!(is_atomic(Dialect::Mysql, &[empty_rows(Dialect::Mysql)]));
        assert!(is_atomic(
            Dialect::Mysql,
            &[empty_rows(Dialect::Mysql), empty_rows(Dialect::Mysql)]
        ));
        // Its DDL commits on its own: structure, an object operation or a definition is not.
        for plan in [ddl(true, false), ddl(true, true), ddl(false, false)] {
            assert!(!is_atomic(
                Dialect::Mysql,
                &[empty_rows(Dialect::Mysql), plan]
            ));
        }
        assert!(!is_atomic(Dialect::Mysql, &[ddl(true, false)]));
        assert!(!is_atomic(
            Dialect::Mysql,
            &[plan_definition(Dialect::Mysql, None, "CREATE VIEW v AS SELECT 1").unwrap()]
        ));
        // The other dialects: atomic while every plan is transactional.
        for dialect in [Dialect::Postgres, Dialect::Sqlite] {
            assert!(is_atomic(dialect, &[empty_rows(dialect), ddl(true, false)]));
            assert!(!is_atomic(
                dialect,
                &[empty_rows(dialect), ddl(false, false)]
            ));
        }
        // Nothing pending is trivially atomic on every dialect.
        assert!(is_atomic(Dialect::Mysql, &[]));
    }

    #[tokio::test]
    async fn a_failing_data_transaction_reverts_everything_even_when_a_plan_runs_outside_one() {
        // The MySQL shape: rows, then plans of their own. The rows fail first, so no plan ran and
        // nothing stayed: the result is atomic ("All changes were reverted").
        let (_dir, s) = session().await;
        let missing = StagedChange::Delete {
            row_id: "r9".into(),
            key: RowKey::Pk(BTreeMap::from([("id".to_string(), Value::Int(9))])),
        };
        let groups = vec![
            PlannedGroup::Plan {
                plan: DdlPlan {
                    statements: vec!["DROP TABLE gone".into()],
                    warnings: Vec::new(),
                    transactional: false,
                },
                object: true,
                created: None,
                replaces: None,
            },
            rows(&s, vec![rename("z")]).await,
            rows(&s, vec![missing]).await,
        ];
        let r = run_workspace(&s, &groups).await;
        assert!(!r.success);
        assert!(r.atomic, "nothing was applied, so everything was reverted");
        assert_eq!(r.failed_group, Some(2));
        assert_eq!(r.groups[0].status, GroupStatus::NotRun);
        assert_eq!(r.groups[1].status, GroupStatus::RolledBack);
        assert_eq!(r.groups[2].status, GroupStatus::Failed);
        assert!(r.groups.iter().all(|g| g.executed.is_empty()));
        assert_eq!(names(&s).await, ["a", "b"]);
        assert!(has_table(&s, "gone").await);
    }

    #[tokio::test]
    async fn everything_commits_in_one_transaction() {
        let (_dir, s) = session().await;
        let groups = vec![
            PlannedGroup::Plan {
                plan: DdlPlan {
                    statements: vec!["DROP TABLE gone".into()],
                    warnings: Vec::new(),
                    transactional: true,
                },
                object: true,
                created: None,
                replaces: None,
            },
            rows(&s, vec![rename("z")]).await,
            PlannedGroup::Plan {
                plan: DdlPlan {
                    statements: vec!["ALTER TABLE t ADD COLUMN c INTEGER".into()],
                    warnings: Vec::new(),
                    transactional: true,
                },
                object: false,
                created: None,
                replaces: None,
            },
        ];
        let r = run_workspace(&s, &groups).await;
        assert!(r.success, "{:?}", r.error);
        assert!(r.atomic);
        assert!(r.groups.iter().all(|g| g.status == GroupStatus::Applied));
        assert_eq!(r.groups[1].rows.len(), 1);
        assert_eq!(r.groups[1].rows[0].row_id, "r1");
        // RETURNING * gives the stored row back, NOW() included.
        let returned = r.groups[1].rows[0].returned.as_ref().unwrap();
        assert!(matches!(returned.get("seen"), Some(Value::Text(t)) if t.len() == 19));
        assert_eq!(names(&s).await, ["z", "b"]);
        assert!(!has_table(&s, "gone").await);
    }

    #[tokio::test]
    async fn a_failure_reverts_every_tab() {
        let (_dir, s) = session().await;
        let groups = vec![
            rows(&s, vec![rename("z")]).await,
            PlannedGroup::Plan {
                plan: DdlPlan {
                    statements: vec![
                        "ALTER TABLE t ADD COLUMN c INTEGER".into(),
                        "ALTER TABLE nope ADD COLUMN d INTEGER".into(),
                    ],
                    warnings: Vec::new(),
                    transactional: true,
                },
                object: false,
                created: None,
                replaces: None,
            },
            PlannedGroup::Plan {
                plan: DdlPlan {
                    statements: vec!["DROP TABLE gone".into()],
                    warnings: Vec::new(),
                    transactional: true,
                },
                object: true,
                created: None,
                replaces: None,
            },
        ];
        let r = run_workspace(&s, &groups).await;
        assert!(!r.success);
        assert!(r.atomic);
        assert!(r.error.is_some());
        assert!(r.groups.iter().all(|g| g.executed.is_empty()));
        assert!(r.groups.iter().all(|g| g.status != GroupStatus::Applied));
        // Nothing stayed: the row, the new column and the dropped table are as before.
        assert_eq!(names(&s).await, ["a", "b"]);
        assert!(has_table(&s, "gone").await);
        let cols = s.query("SELECT * FROM t LIMIT 0", &[]).await.unwrap();
        assert_eq!(cols.columns.len(), 3);
    }

    #[tokio::test]
    async fn a_row_guard_names_the_failing_group() {
        let (_dir, s) = session().await;
        let missing = StagedChange::Delete {
            row_id: "r9".into(),
            key: RowKey::Pk(BTreeMap::from([("id".to_string(), Value::Int(9))])),
        };
        let groups = vec![
            rows(&s, vec![rename("z")]).await,
            rows(&s, vec![missing]).await,
        ];
        let r = run_workspace(&s, &groups).await;
        assert!(!r.success);
        assert_eq!(r.failed_group, Some(1));
        assert_eq!(r.groups[0].status, GroupStatus::RolledBack);
        assert_eq!(r.groups[1].failed_index, Some(0));
        assert_eq!(r.groups[1].rows[0].row_id, "r9");
        assert!(r.groups[1].rows[0].error.is_some());
        assert_eq!(names(&s).await, ["a", "b"]);
    }

    #[tokio::test]
    async fn a_plan_outside_transactions_runs_after_the_committed_data() {
        let (_dir, s) = session().await;
        let groups = vec![
            rows(&s, vec![rename("z")]).await,
            PlannedGroup::Plan {
                plan: DdlPlan {
                    statements: vec!["DROP TABLE gone".into(), "DROP TABLE nope".into()],
                    warnings: Vec::new(),
                    transactional: false,
                },
                object: true,
                created: None,
                replaces: None,
            },
        ];
        let r = run_workspace(&s, &groups).await;
        assert!(!r.success);
        assert!(!r.atomic);
        assert_eq!(r.groups[0].status, GroupStatus::Applied);
        assert_eq!(r.groups[1].status, GroupStatus::Failed);
        assert_eq!(r.groups[1].failed_index, Some(1));
        assert_eq!(r.groups[1].executed, ["DROP TABLE gone"]);
        assert_eq!(names(&s).await, ["z", "b"]);
        assert!(!has_table(&s, "gone").await);
    }

    #[tokio::test]
    async fn a_definition_runs_after_the_data_edits_and_reports_in_request_order() {
        let (_dir, s) = session().await;
        let groups = vec![
            plan_definition(
                Dialect::Sqlite,
                None,
                "CREATE VIEW names AS SELECT name FROM t;",
            )
            .unwrap(),
            rows(&s, vec![rename("z")]).await,
        ];
        assert_eq!(run_order(&groups), [1, 0]);
        let r = run_workspace(&s, &groups).await;
        assert!(r.success, "{:?}", r.error);
        assert!(r.groups[0].sql[0].starts_with("CREATE VIEW names"));
        assert_eq!(r.groups[1].rows[0].row_id, "r1");
        let view = s
            .query("SELECT name FROM names ORDER BY name", &[])
            .await
            .unwrap();
        assert_eq!(view.rows.len(), 2);
    }

    #[test]
    fn object_ops_keep_their_variant() {
        // The renderer's payload for a pending sidebar operation.
        let g: WorkspaceGroup = serde_json::from_str(
            r#"{"kind":"object","op":{"op":"dropSequences","sequences":[{"schema":"public","name":"s"}],"cascade":false}}"#,
        )
        .unwrap();
        assert!(matches!(
            g,
            WorkspaceGroup::Object {
                op: ObjectOp::DropSequences { .. }
            }
        ));
        let g: WorkspaceGroup = serde_json::from_str(
            r#"{"kind":"definition","drop":null,"sql":"CREATE VIEW v AS SELECT 1"}"#,
        )
        .unwrap();
        assert!(matches!(g, WorkspaceGroup::Definition { drop: None, .. }));
    }

    #[tokio::test]
    async fn a_view_with_text_outside_ascii_is_saved() {
        // The text that panicked the tokenizer (a one-byte step inside a multi-byte character).
        let (_dir, s) = session().await;
        let groups = vec![
            plan_definition(
                Dialect::Sqlite,
                None,
                "CREATE VIEW cidade AS SELECT 'São Paulo' AS nome, name AS 名前 FROM t;",
            )
            .unwrap(),
        ];
        let r = run_workspace(&s, &groups).await;
        assert!(r.success, "{:?}", r.error);
        let rows = s
            .query("SELECT nome FROM cidade ORDER BY 名前 LIMIT 1", &[])
            .await
            .unwrap()
            .rows;
        assert_eq!(rows[0][0].as_text().as_deref(), Some("São Paulo"));
    }

    /// A group per `sql`: the text `boom` stands for a planner that panics.
    async fn plan_texts(texts: &[&str]) -> Vec<Result<PlannedGroup, AppError>> {
        let groups: Vec<WorkspaceGroup> = texts
            .iter()
            .map(|t| WorkspaceGroup::Definition {
                drop: None,
                sql: (*t).to_string(),
            })
            .collect();
        plan_each(&groups, |g| async move {
            let WorkspaceGroup::Definition { sql, .. } = g else {
                unreachable!()
            };
            if sql == "boom" {
                panic!("tokenizer bug on {sql}");
            }
            plan_definition(Dialect::Sqlite, None, sql)
        })
        .await
    }

    #[tokio::test]
    async fn a_panic_while_planning_becomes_that_groups_error() {
        let planned = plan_texts(&[
            "CREATE VIEW a AS SELECT 1",
            "boom",
            "CREATE VIEW b AS SELECT 2",
        ])
        .await;
        assert_eq!(planned.len(), 3);
        assert!(planned[0].is_ok());
        assert!(planned[2].is_ok());
        let err = planned[1]
            .as_ref()
            .err()
            .expect("the panicking group fails");
        assert_eq!(err.code(), "INTERNAL");
        let message = err.to_payload().message;
        assert!(message.contains("tokenizer bug on boom"), "{message}");
        assert!(message.contains("nothing was run"), "{message}");
        // An ordinary planning error is untouched.
        let planned = plan_texts(&["DROP TABLE users"]).await;
        assert_eq!(planned[0].as_ref().err().unwrap().code(), "INVALID_INPUT");
    }
}
