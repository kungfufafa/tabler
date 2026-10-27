//! DDL commands (M5): preview a designer change set as SQL, apply it, and show a table's DDL.
//! Planning lives in `sqlgen::ddl`; `apply_plan` is the driver-agnostic executor.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Instant;

use regex::Regex;
use tauri::State;
use uuid::Uuid;

use crate::admin::mysql_literal_mode;
use crate::commands::browse::cached_meta;
use crate::drivers::{DbSession, TxStatement};
use crate::error::{AppError, ErrorPayload};
use crate::export::structure::pg_table_context;
use crate::sqlgen::Dialect;
use crate::sqlgen::ddl::{
    PlanContext, create_table_sql_with, is_fk_check, is_fk_toggle, pg_owned_sequences_sql,
    plan_ddl_with, session_toggle,
};
use crate::state::{AppState, Connection};
use crate::types::caps::Feature;
use crate::types::ddl::{DdlOp, DdlPlan, DdlResult};
use crate::types::profile::{ConnectionProfile, Driver};
use crate::types::query::ExecOutcome;
use crate::types::schema::{TableKind, TableMeta, TableRef};
use crate::types::value::Value;

/// Start of the error a foreign-key guard raises (see `fk_guard_sql`).
const FK_GUARD_MARKER: &str = "tabler-fk-violations:";

/// How long a DDL statement waits for a lock another session holds before it fails (PG
/// `lock_timeout`, MySQL `lock_wait_timeout`; SQLite connections give up after their 5 s busy
/// timeout). The servers queue every other session's access to the table behind a waiting
/// ALTER, so the wait must stay short.
pub const DDL_LOCK_TIMEOUT_SECS: u64 = 5;

/// Plans `ops` against fresh metadata of `table` (when given).
pub(crate) async fn plan_for(
    conn: &Connection,
    table: Option<&TableRef>,
    ops: &[DdlOp],
) -> Result<DdlPlan, AppError> {
    let session = conn.session().await;
    let meta = match table {
        Some(t) => {
            conn.invalidate(Some(t)).await;
            match cached_meta(conn, t).await {
                Ok(meta) => Some(meta),
                // A table that does not exist yet (create flow): plan without metadata.
                Err(AppError::NotFound(_)) => None,
                Err(e) => return Err(e),
            }
        }
        None => None,
    };
    let mut plan = plan_on(session.as_ref(), meta.as_deref(), ops).await?;
    adapt_to_profile(&mut plan, &conn.profile);
    Ok(plan)
}

/// Plans `ops` against `meta`, after looking up on the server what `TableMeta` does not say
/// (PG: which indexes back a constraint, which sequences the columns own; MySQL: how the
/// session reads backslashes in literals; see `PlanContext`).
pub async fn plan_on(
    session: &dyn DbSession,
    meta: Option<&TableMeta>,
    ops: &[DdlOp],
) -> Result<DdlPlan, AppError> {
    let dialect = session.core_dialect(Feature::StructureEdit)?;
    let mut context = PlanContext::default();
    if dialect == Dialect::Mysql {
        // comments and defaults are spelled for how this session reads backslashes
        context.mysql_mode = mysql_literal_mode(session).await?;
    }
    if dialect == Dialect::Postgres
        && let Some(meta) = meta
    {
        if ops.iter().any(|op| matches!(op, DdlOp::DropIndex { .. })) {
            context.constraint_indexes = pg_constraint_indexes(session, &meta.table).await?;
        }
        if ops.iter().any(|op| {
            matches!(
                op,
                DdlOp::AlterColumnAutoIncrement {
                    auto_increment: true,
                    ..
                }
            )
        }) {
            context.owned_sequences = pg_owned_sequences(session, &meta.table).await?;
        }
    }
    plan_ddl_with(dialect, meta, ops, &context)
}

/// PG: the sequences owned by a column of `table` besides its identity: `(column, sequence)`.
async fn pg_owned_sequences(
    session: &dyn DbSession,
    table: &TableRef,
) -> Result<Vec<(String, TableRef)>, AppError> {
    let query = pg_owned_sequences_sql(table);
    let rows = session.query(&query.sql, &query.params).await?.rows;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let text = |i: usize| row.get(i).and_then(Value::as_text);
            Some((text(0)?, TableRef::new(Some(&text(1)?), &text(2)?)))
        })
        .collect())
}

/// PG: the indexes of `table` that back a PRIMARY KEY, UNIQUE or EXCLUDE constraint.
async fn pg_constraint_indexes(
    session: &dyn DbSession,
    table: &TableRef,
) -> Result<Vec<String>, AppError> {
    let base = "SELECT ci.relname::text FROM pg_catalog.pg_constraint c \
         JOIN pg_catalog.pg_class t ON t.oid = c.conrelid \
         JOIN pg_catalog.pg_namespace n ON n.oid = t.relnamespace \
         JOIN pg_catalog.pg_class ci ON ci.oid = c.conindid \
         WHERE c.contype IN ('p', 'u', 'x') AND t.relname = $1 AND n.nspname = ";
    let mut params = vec![Value::Text(table.name.clone())];
    let sql = match table.schema.as_deref().filter(|s| !s.is_empty()) {
        Some(schema) => {
            params.push(Value::Text(schema.to_string()));
            format!("{base}$2")
        }
        None => format!("{base}current_schema()"),
    };
    let rows = session.query(&sql, &params).await?.rows;
    Ok(rows
        .into_iter()
        .filter_map(|row| match row.into_iter().next() {
            Some(Value::Text(name)) => Some(name),
            _ => None,
        })
        .collect())
}

/// SQLite connections opened with foreign keys disabled keep them disabled: the rebuild's
/// `PRAGMA foreign_keys` toggles would switch enforcement on for the rest of the session, and
/// the post-rebuild check would report violations the user opted out of. Only the wrapper the
/// planner puts around a rebuild (`PRAGMA … OFF`, `BEGIN`, …, check, `COMMIT`, `PRAGMA … ON`)
/// is touched.
pub fn adapt_to_profile(plan: &mut DdlPlan, profile: &ConnectionProfile) {
    let wrapped = plan.statements.first().is_some_and(|s| is_fk_toggle(s))
        && plan.statements.get(1).is_some_and(|s| s == "BEGIN");
    if profile.driver != Driver::Sqlite || profile.options.sqlite_foreign_keys || !wrapped {
        return;
    }
    plan.statements.remove(0);
    if plan.statements.last().is_some_and(|s| is_fk_toggle(s)) {
        plan.statements.pop();
    }
    plan.statements.retain(|s| !is_fk_check(s));
}

#[tauri::command]
pub async fn preview_ddl(
    state: State<'_, AppState>,
    connection_id: Uuid,
    table: Option<TableRef>,
    ops: Vec<DdlOp>,
) -> Result<DdlPlan, AppError> {
    let conn = state.connection(connection_id).await?;
    plan_for(&conn, table.as_ref(), &ops).await
}

#[tauri::command]
pub async fn apply_ddl(
    state: State<'_, AppState>,
    connection_id: Uuid,
    table: Option<TableRef>,
    ops: Vec<DdlOp>,
) -> Result<DdlResult, AppError> {
    let conn = state.connection(connection_id).await?;
    if conn.profile.options.read_only {
        return Err(AppError::Unsupported("this connection is read-only".into()));
    }
    let plan = plan_for(&conn, table.as_ref(), &ops).await?;
    let target = table
        .as_ref()
        .map(|t| t.name.as_str())
        .unwrap_or("(no table)");
    log::info!(
        "apply_ddl {} statement(s) on {target} (transactional: {})",
        plan.statements.len(),
        plan.transactional
    );
    let session = conn.session().await;
    let result = apply_plan(session.as_ref(), &plan).await;
    conn.invalidate(None).await;
    match (&result.error, result.failed_index) {
        (Some(err), index) => log::info!(
            "apply_ddl on {target} failed at statement {index:?} after {} ms{}: {}",
            result.duration_ms,
            // (a transactional plan rolled back; one MySQL ALTER TABLE fails as a whole)
            if plan.transactional && result.executed.is_empty() {
                " (all changes were reverted)"
            } else {
                ""
            },
            err.message
        ),
        (None, _) => log::info!("apply_ddl on {target} done in {} ms", result.duration_ms),
    }
    Ok(result)
}

/// `CREATE` text of a table or view: the server's own DDL where it has one (MySQL, SQLite),
/// a reconstruction for PG tables, `CREATE VIEW … AS` + definition for PG views.
#[tauri::command]
pub async fn get_table_ddl(
    state: State<'_, AppState>,
    connection_id: Uuid,
    table: TableRef,
) -> Result<String, AppError> {
    let conn = state.connection(connection_id).await?;
    let session = conn.session().await;
    // Always describe the current structure, not a cached one.
    conn.invalidate(Some(&table)).await;
    let meta = cached_meta(&conn, &table).await?;
    if let Some(ddl) = meta.ddl.as_deref().filter(|d| !d.trim().is_empty()) {
        return Ok(ddl.to_string());
    }
    let dialect = session.core_dialect(Feature::TableDdl)?;
    match meta.kind {
        TableKind::View | TableKind::MaterializedView | TableKind::SystemView => {
            let definition = session
                .view_definition(&meta.table)
                .await?
                .unwrap_or_default();
            let what = if meta.kind == TableKind::MaterializedView {
                "MATERIALIZED VIEW"
            } else {
                "VIEW"
            };
            let body = definition.trim();
            let body = body.strip_suffix(';').unwrap_or(body).trim_end();
            Ok(format!(
                "CREATE {what} {} AS\n{body};",
                dialect.qualify(meta.table.schema.as_deref(), &meta.table.name)
            ))
        }
        _ => {
            // PG: what the catalog adds (sequences, partitioning, cloned keys and triggers);
            // a server that cannot answer gets the script `TableMeta` alone gives.
            let context = if dialect == Dialect::Postgres {
                pg_table_context(session.as_ref(), &meta.table, false)
                    .await
                    .unwrap_or_else(|e| {
                        log::warn!("get_table_ddl: no catalog context: {e}");
                        Default::default()
                    })
            } else {
                Default::default()
            };
            Ok(create_table_sql_with(dialect, &meta, &context))
        }
    }
}

fn first_word_is(sql: &str, word: &str) -> bool {
    sql.split_whitespace()
        .next()
        .is_some_and(|w| w.trim_end_matches(';').eq_ignore_ascii_case(word))
}

fn violation_error(count: &str, sample: &[String]) -> AppError {
    AppError::Db {
        message: format!("the change would leave {count} row(s) violating foreign keys"),
        detail: Some(format!("table rowid parent fkid:\n{}", sample.join("\n"))),
        hint: Some("Fix or delete the offending rows, then apply again.".into()),
        sqlstate: None,
        vendor_code: None,
        position: None,
        statement_index: None,
    }
}

/// Rows from a foreign-key check (`is_fk_check`) are violations: turn them into an error so the
/// surrounding explicit transaction is rolled back.
fn check_outcome(sql: &str, outcome: &ExecOutcome) -> Result<(), AppError> {
    if !is_fk_check(sql) {
        return Ok(());
    }
    let Some(rows) = outcome.rows.as_ref().filter(|r| !r.rows.is_empty()) else {
        return Ok(());
    };
    let sample: Vec<String> = rows
        .rows
        .iter()
        .take(5)
        .map(|row| {
            row.iter()
                .map(|v| match v {
                    Value::Null => "NULL".to_string(),
                    Value::Int(i) => i.to_string(),
                    Value::Text(s) | Value::BigInt(s) => s.clone(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    Err(violation_error(&rows.rows.len().to_string(), &sample))
}

fn unquote_ident(s: &str) -> String {
    let s = s.trim();
    let inner = |open: char, close: char| s.strip_prefix(open).and_then(|r| r.strip_suffix(close));
    if let Some(i) = inner('"', '"') {
        i.replace("\"\"", "\"")
    } else if let Some(i) = inner('`', '`') {
        i.replace("``", "`")
    } else if let Some(i) = inner('[', ']') {
        i.to_string()
    } else if let Some(i) = inner('\'', '\'') {
        i.replace("''", "'")
    } else {
        s.to_string()
    }
}

/// A foreign-key check as a query that can be nested (`PRAGMA s.foreign_key_check(t)` becomes
/// the table-valued `pragma_foreign_key_check('t', 's')`).
fn check_query(check: &str) -> String {
    static PRAGMA: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?is)^\s*PRAGMA\s+(?:("(?:[^"]|"")*"|`(?:[^`]|``)*`|\[[^\]]*\]|[^\s."`\[(]+)\s*\.\s*)?foreign_key_check\s*(?:\(\s*(.*?)\s*\))?$"#,
        )
        .expect("static regex")
    });
    let check = check.trim().trim_end_matches(';').trim_end();
    let Some(c) = PRAGMA.captures(check) else {
        return check.to_string();
    };
    let lit = |s: &str| Dialect::Sqlite.quote_literal(&unquote_ident(s));
    let table = c.get(2).map(|m| lit(m.as_str()));
    let schema = c.get(1).map(|m| lit(m.as_str()));
    let args = match (table, schema) {
        (Some(t), Some(s)) => format!("{t}, {s}"),
        (Some(t), None) => t,
        (None, Some(s)) => format!("NULL, {s}"),
        (None, None) => String::new(),
    };
    format!("SELECT * FROM pragma_foreign_key_check({args})")
}

/// A foreign-key check that fails by itself, for use inside a transaction (where returned rows
/// cannot stop the commit): with violations it hands `json_extract` an invalid path made of
/// `FK_GUARD_MARKER`, their number and the first five, which SQLite reports as the error.
fn fk_guard_sql(check: &str) -> String {
    let query = check_query(check);
    format!(
        "SELECT json_extract('{{}}', CASE WHEN n > 0 THEN '{FK_GUARD_MARKER}' || n || char(10) || \
         ifnull((SELECT group_concat(line, char(10)) FROM (SELECT \"table\" || ' ' || \
         ifnull(rowid, 'NULL') || ' ' || ifnull(parent, '') || ' ' || ifnull(fkid, '') AS line \
         FROM ({query}) LIMIT 5)), '') ELSE '$' END) FROM (SELECT count(*) AS n FROM ({query}))"
    )
}

/// The violation report for an error raised by `fk_guard_sql`.
fn guard_violation(err: &AppError) -> Option<AppError> {
    let msg = err.to_string();
    let at = msg.find(FK_GUARD_MARKER)?;
    let rest = &msg[at + FK_GUARD_MARKER.len()..];
    // SQLite quotes the path: `bad JSON path: '…'`
    let rest = rest.strip_suffix('\'').unwrap_or(rest).replace("''", "'");
    let (count, sample) = rest.split_once('\n').unwrap_or((&rest, ""));
    let sample: Vec<String> = sample
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    Some(violation_error(count.trim(), &sample))
}

/// One lock per SQLite session, held while a sequential plan runs: statements outside a block
/// (and the restoring toggles after a failure) are separate jobs on the session's connection,
/// and another plan's toggles must not land in between.
fn plan_lock(session: &dyn DbSession) -> Arc<tokio::sync::Mutex<()>> {
    type Locks = HashMap<usize, Weak<tokio::sync::Mutex<()>>>;
    static LOCKS: LazyLock<Mutex<Locks>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    let key = std::ptr::from_ref(session).cast::<()>() as usize;
    let mut locks = LOCKS.lock().unwrap_or_else(|p| p.into_inner());
    locks.retain(|_, l| l.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

/// Index of the `COMMIT` (or `END`) closing the block opened at `begin`.
fn block_end(statements: &[String], begin: usize) -> Option<usize> {
    for (i, s) in statements.iter().enumerate().skip(begin + 1) {
        if first_word_is(s, "BEGIN") {
            return None;
        }
        if first_word_is(s, "COMMIT") || first_word_is(s, "END") {
            return Some(i);
        }
    }
    None
}

/// A SQLite `BEGIN` … `COMMIT` block of a plan together with the connection-setting toggles
/// right around it (`PRAGMA foreign_keys = OFF` before, `… = ON` after a rebuild):
/// `statements[start..begin]` toggles, `statements[begin]` `BEGIN`, `statements[end]` `COMMIT`,
/// `statements[end + 1..stop]` toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Block {
    start: usize,
    begin: usize,
    end: usize,
    stop: usize,
}

/// The block starting at `index` (with its leading toggles), if one does.
fn block_at(statements: &[String], index: usize) -> Option<Block> {
    let toggle = |i: usize| {
        statements
            .get(i)
            .is_some_and(|s| session_toggle(s).is_some())
    };
    let mut begin = index;
    while toggle(begin) {
        begin += 1;
    }
    if !statements
        .get(begin)
        .is_some_and(|s| first_word_is(s, "BEGIN"))
    {
        return None;
    }
    let end = block_end(statements, begin)?;
    let mut stop = end + 1;
    while toggle(stop) {
        stop += 1;
    }
    Some(Block {
        start: index,
        begin,
        end,
        stop,
    })
}

/// Runs a SQLite block as ONE `DbSession::transaction` job on the session's connection: no other
/// work runs inside the block, sees it half done, or runs between the block and its toggles (a
/// grid commit queued while a rebuild copies rows would otherwise run with foreign keys off,
/// skipping its cascades). `PRAGMA foreign_keys` does nothing inside a transaction, so the
/// batch ends the transaction the driver opens before its leading toggles, and opens an empty
/// one again for the driver's closing COMMIT after the trailing ones:
///
/// `[COMMIT, <toggles>, BEGIN,] <block>, COMMIT, <toggles>, BEGIN`
///
/// The block's own COMMIT is a statement of the batch, so when it fails (SQLITE_BUSY keeps the
/// transaction open) the driver rolls the transaction back instead of leaving it open on the
/// connection. Foreign-key checks become guards that fail by themselves. On failure returns
/// the index of the failing statement in the plan, when known.
async fn run_block(
    session: &dyn DbSession,
    statements: &[String],
    block: Block,
) -> Result<(), (Option<usize>, ErrorPayload)> {
    let Block {
        start,
        begin,
        end,
        stop,
    } = block;
    // (statement, its index in the plan)
    let mut batch: Vec<(String, Option<usize>)> = Vec::new();
    if start < begin {
        batch.push(("COMMIT".into(), None));
        batch.extend((start..begin).map(|i| (statements[i].clone(), Some(i))));
        batch.push(("BEGIN".into(), Some(begin)));
    }
    for (i, s) in statements.iter().enumerate().take(end).skip(begin + 1) {
        let sql = if is_fk_check(s) {
            fk_guard_sql(s)
        } else {
            s.clone()
        };
        batch.push((sql, Some(i)));
    }
    batch.push(("COMMIT".into(), Some(end)));
    batch.extend((end + 1..stop).map(|i| (statements[i].clone(), Some(i))));
    batch.push(("BEGIN".into(), None));
    let plan_index: Vec<Option<usize>> = batch.iter().map(|(_, i)| *i).collect();
    let txs = batch
        .into_iter()
        .map(|(sql, _)| TxStatement::new(sql, Vec::new()))
        .collect();
    let err = match session.transaction(txs).await {
        Ok(_) => return Ok(()),
        Err(err) => err,
    };
    let check = (begin + 1..end).find(|&i| is_fk_check(&statements[i]));
    let reported = err
        .to_payload()
        .statement_index
        .and_then(|i| plan_index.get(i).copied().flatten());
    let (err, failed) = match guard_violation(&err) {
        Some(violation) => (violation, check),
        // a child whose key no longer matches the rebuilt parent
        None if check.is_some() && err.to_string().contains("foreign key mismatch") => (err, check),
        None => (err, reported),
    };
    let mut payload = err.to_payload();
    payload.statement_index = failed;
    Err((failed, payload))
}

/// The connection settings `statements` change (`foreign_keys`, `legacy_alter_table`): the last
/// toggle of each setting, in plan order.
fn last_toggles(statements: &[String]) -> Vec<&String> {
    let mut last: Vec<(String, usize)> = Vec::new();
    for (i, s) in statements.iter().enumerate() {
        if let Some(setting) = session_toggle(s) {
            match last.iter_mut().find(|(k, _)| *k == setting) {
                Some(entry) => entry.1 = i,
                None => last.push((setting, i)),
            }
        }
    }
    let mut positions: Vec<usize> = last.into_iter().map(|(_, i)| i).collect();
    positions.sort_unstable();
    positions.into_iter().map(|i| &statements[i]).collect()
}

/// After a failure, re-applies the connection settings the plan changes (`last_toggles` of
/// `rest`).
async fn restore_settings(session: &dyn DbSession, rest: &[String], executed: &mut Vec<String>) {
    for sql in last_toggles(rest) {
        match session.execute(sql, &[]).await {
            Ok(_) => executed.push(sql.clone()),
            Err(e) => log::warn!("restoring session state ({sql}): {e}"),
        }
    }
}

/// A transactional plan as the batch sent to the driver, and the number of statements put in
/// front of the plan's own. PG bounds lock waits (`SET LOCAL` ends with the transaction);
/// SQLite commits inside the batch, so a failed COMMIT is rolled back by the driver (see
/// `run_block`). A MySQL plan is transactional when it is one statement (an `ALTER TABLE` that
/// fails leaves the table as it was); it bounds its metadata-lock wait like `execute_one`.
fn transactional_batch(dialect: Dialect, statements: &[String]) -> (Vec<TxStatement>, usize) {
    let plain = |s: &String| TxStatement::new(s.clone(), Vec::new());
    match dialect {
        Dialect::Postgres => {
            let mut batch = vec![TxStatement::new(
                format!("SET LOCAL lock_timeout = '{DDL_LOCK_TIMEOUT_SECS}s'"),
                Vec::new(),
            )];
            batch.extend(statements.iter().map(plain));
            (batch, 1)
        }
        Dialect::Sqlite => {
            let mut batch: Vec<TxStatement> = statements.iter().map(plain).collect();
            batch.push(TxStatement::new("COMMIT", Vec::new()));
            batch.push(TxStatement::new("BEGIN", Vec::new()));
            (batch, 0)
        }
        Dialect::Mysql => {
            let mut batch = vec![TxStatement::new(
                format!("SET SESSION lock_wait_timeout = {DDL_LOCK_TIMEOUT_SECS}"),
                Vec::new(),
            )];
            batch.extend(statements.iter().map(plain));
            (batch, 1)
        }
    }
}

/// Runs one statement of a sequential plan. MySQL runs it on one pooled connection after
/// bounding its metadata-lock wait (the pool resets session variables when the connection is
/// returned; DDL commits implicitly, so the surrounding transaction changes nothing else).
async fn execute_one(session: &dyn DbSession, dialect: Dialect, sql: &str) -> Result<(), AppError> {
    if dialect == Dialect::Mysql {
        let batch = vec![
            TxStatement::new(
                format!("SET SESSION lock_wait_timeout = {DDL_LOCK_TIMEOUT_SECS}"),
                Vec::new(),
            ),
            TxStatement::new(sql, Vec::new()),
        ];
        return session.transaction(batch).await.map(|_| ());
    }
    let outcome = session.execute(sql, &[]).await?;
    check_outcome(sql, &outcome)
}

/// Explains a failure to get a lock in time: usually an open transaction, very often the SQL
/// editor's own, holds a lock on the table.
pub(crate) fn explain_lock_wait(
    session: &dyn DbSession,
    dialect: Dialect,
    payload: &mut ErrorPayload,
) {
    let lock_wait = match dialect {
        // lock_not_available ("canceling statement due to lock timeout")
        Dialect::Postgres => payload.sqlstate.as_deref() == Some("55P03"),
        // ER_LOCK_WAIT_TIMEOUT
        Dialect::Mysql => payload.vendor_code == Some(1205),
        // SQLITE_BUSY / SQLITE_LOCKED (extended codes keep the primary one in the low byte)
        Dialect::Sqlite => payload
            .vendor_code
            .is_some_and(|c| matches!(c & 0xff, 5 | 6)),
    };
    if !lock_wait {
        return;
    }
    if session.in_transaction() == Some(true) {
        payload.hint = Some(
            "The SQL editor has an open transaction, which may hold a lock on this table. Commit or roll it back, then apply again."
                .into(),
        );
    } else if payload.hint.is_none() {
        payload.hint = Some(
            "Another session holds a lock on this table (for example an open transaction). Apply again once it is done."
                .into(),
        );
    }
}

/// Executes a plan. Transactional plans run in one `DbSession::transaction` (all or nothing);
/// others run statement by statement, stopping at the first failure. Lock waits are bounded
/// (`DDL_LOCK_TIMEOUT_SECS`) on PG transactional plans and on MySQL. On SQLite a plan's own
/// `BEGIN` … `COMMIT` block (a table rebuild) runs together with the toggles around it as one
/// transaction job (`run_block`), with its foreign-key check turned into a guard that fails the
/// block, and plans on one session do not interleave. When a sequential plan fails, its open
/// block is rolled back and the connection settings it toggles (`PRAGMA foreign_keys`,
/// `legacy_alter_table`) are restored. Failures are reported in the result, never as `Err`.
/// The plans are the core planners' (structure, object operations, definitions): a session
/// without the core generators runs none.
pub async fn apply_plan(session: &dyn DbSession, plan: &DdlPlan) -> DdlResult {
    let started = Instant::now();
    let dialect = match session.core_dialect(Feature::StructureEdit) {
        Ok(dialect) => dialect,
        Err(e) => {
            return DdlResult {
                executed: Vec::new(),
                failed_index: None,
                error: Some(e.to_payload()),
                duration_ms: 0,
            };
        }
    };
    let mut result = run_plan(session, dialect, plan).await;
    result.duration_ms = started.elapsed().as_millis() as u64;
    if let Some(payload) = result.error.as_mut() {
        explain_lock_wait(session, dialect, payload);
    }
    result
}

async fn run_plan(session: &dyn DbSession, dialect: Dialect, plan: &DdlPlan) -> DdlResult {
    let done = |executed: Vec<String>, failed_index: Option<usize>, error| DdlResult {
        executed,
        failed_index,
        error,
        duration_ms: 0,
    };
    if plan.statements.is_empty() {
        return done(Vec::new(), None, None);
    }

    if plan.transactional {
        let (batch, offset) = transactional_batch(dialect, &plan.statements);
        return match session.transaction(batch).await {
            Ok(_) => done(plan.statements.clone(), None, None),
            Err(err) => {
                let mut payload = err.to_payload();
                let failed_index = payload
                    .statement_index
                    .and_then(|i| i.checked_sub(offset))
                    .filter(|&i| i < plan.statements.len())
                    .or((plan.statements.len() == 1).then_some(0));
                payload.statement_index = failed_index;
                done(Vec::new(), failed_index, Some(payload))
            }
        };
    }

    // SQLite: keep other plans' toggles out of this one's.
    let lock = (dialect == Dialect::Sqlite).then(|| plan_lock(session));
    let _serialized = match &lock {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    let mut executed: Vec<String> = Vec::new();
    // Position in `executed` where an open BEGIN block (run statement by statement) starts.
    let mut block_start: Option<usize> = None;
    let mut index = 0;
    while index < plan.statements.len() {
        if lock.is_some()
            && let Some(block) = block_at(&plan.statements, index)
        {
            // Queued right behind the block (a connection runs its jobs in order, and both
            // futures queue their jobs when first polled, block first): after a failure they
            // restore the settings before anything else can run there; after a success they
            // change nothing.
            let restore = last_toggles(&plan.statements[block.start..block.stop]);
            let restoring =
                futures::future::join_all(restore.iter().map(|sql| session.execute(sql, &[])));
            let (outcome, restored) =
                futures::future::join(run_block(session, &plan.statements, block), restoring).await;
            match outcome {
                Ok(()) => {
                    executed.extend_from_slice(&plan.statements[block.start..block.stop]);
                    index = block.stop;
                    continue;
                }
                Err((failed_index, payload)) => {
                    executed.extend_from_slice(&plan.statements[block.start..block.begin]);
                    for (sql, outcome) in restore.into_iter().zip(restored) {
                        match outcome {
                            Ok(_) => executed.push(sql.clone()),
                            Err(e) => log::warn!("restoring session state ({sql}): {e}"),
                        }
                    }
                    return done(executed, failed_index, Some(payload));
                }
            }
        }
        let sql = &plan.statements[index];
        match execute_one(session, dialect, sql).await {
            Ok(()) => {
                if first_word_is(sql, "BEGIN") {
                    block_start = Some(executed.len());
                } else if first_word_is(sql, "COMMIT") || first_word_is(sql, "ROLLBACK") {
                    block_start = None;
                }
                executed.push(sql.clone());
            }
            Err(err) => {
                let payload = err.with_statement_index(index).to_payload();
                if let Some(start) = block_start {
                    if let Err(e) = session.execute("ROLLBACK", &[]).await {
                        log::warn!("rollback after failed DDL statement {index}: {e}");
                    }
                    executed.truncate(start);
                }
                restore_settings(session, &plan.statements[index + 1..], &mut executed).await;
                return done(executed, Some(index), Some(payload));
            }
        }
        index += 1;
    }
    done(executed, None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlgen::SqlDialect;
    use crate::types::profile::ProfileOptions;
    use crate::types::query::{ResultColumn, ResultSet};
    use crate::types::schema::ValueKind;

    fn profile(driver: Driver, sqlite_foreign_keys: bool) -> ConnectionProfile {
        ConnectionProfile {
            id: Uuid::nil(),
            name: "t".into(),
            color: None,
            group: None,
            driver,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: Default::default(),
            ssl_ca_path: None,
            sqlite_path: Some(":memory:".into()),
            ssh: None,
            options: ProfileOptions {
                sqlite_foreign_keys,
                ..ProfileOptions::default()
            },
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    fn rebuild_plan() -> DdlPlan {
        DdlPlan {
            statements: vec![
                "PRAGMA foreign_keys = OFF".into(),
                "BEGIN".into(),
                "CREATE TABLE \"t__tabler_new\" (\"a\" INT)".into(),
                "PRAGMA foreign_key_check(\"t\")".into(),
                "COMMIT".into(),
                "PRAGMA foreign_keys = ON".into(),
            ],
            warnings: vec![],
            transactional: false,
        }
    }

    #[test]
    fn keeps_fk_toggles_only_when_the_profile_enforces_foreign_keys() {
        let mut plan = rebuild_plan();
        adapt_to_profile(&mut plan, &profile(Driver::Sqlite, true));
        assert_eq!(plan.statements.len(), 6);

        let mut plan = rebuild_plan();
        adapt_to_profile(&mut plan, &profile(Driver::Sqlite, false));
        assert_eq!(
            plan.statements,
            [
                "BEGIN",
                "CREATE TABLE \"t__tabler_new\" (\"a\" INT)",
                "COMMIT"
            ]
        );

        let mut plan = rebuild_plan();
        adapt_to_profile(&mut plan, &profile(Driver::Postgres, false));
        assert_eq!(plan.statements.len(), 6);

        // A user's own PRAGMA outside a rebuild is left alone.
        let mut plan = DdlPlan {
            statements: vec!["PRAGMA foreign_keys = ON".into()],
            warnings: vec![],
            transactional: false,
        };
        adapt_to_profile(&mut plan, &profile(Driver::Sqlite, false));
        assert_eq!(plan.statements, ["PRAGMA foreign_keys = ON"]);
    }

    #[test]
    fn foreign_key_check_rows_are_an_error() {
        let clean = ExecOutcome {
            affected: 0,
            last_insert_id: None,
            rows: Some(ResultSet::default()),
        };
        assert!(check_outcome("PRAGMA foreign_key_check(\"t\")", &clean).is_ok());
        let dirty = ExecOutcome {
            affected: 0,
            last_insert_id: None,
            rows: Some(ResultSet {
                columns: vec![ResultColumn {
                    name: "table".into(),
                    data_type: "TEXT".into(),
                    kind: ValueKind::Text,
                    nullable: None,
                    table: None,
                    origin: None,
                }],
                rows: vec![vec![Value::Text("child".into())]],
                truncated: false,
            }),
        };
        let err = check_outcome("PRAGMA foreign_key_check(\"t\")", &dirty).unwrap_err();
        assert!(err.to_string().contains("1 row(s)"), "{err}");
        // Other statements never fail on returned rows.
        assert!(check_outcome("SELECT 1", &dirty).is_ok());
    }

    #[test]
    fn recognises_transaction_control_words() {
        assert!(first_word_is("BEGIN", "BEGIN"));
        assert!(first_word_is("  begin;", "BEGIN"));
        assert!(!first_word_is("BEGINNING", "BEGIN"));
        assert!(first_word_is("COMMIT", "COMMIT"));
        let plan = rebuild_plan();
        assert_eq!(block_end(&plan.statements, 1), Some(4));
        let nested: Vec<String> = ["BEGIN", "BEGIN", "COMMIT"].map(String::from).to_vec();
        assert_eq!(block_end(&nested, 0), None);
    }

    #[test]
    fn foreign_key_checks_become_guards() {
        assert_eq!(
            check_query("PRAGMA foreign_key_check(\"t\");"),
            "SELECT * FROM pragma_foreign_key_check('t')"
        );
        assert_eq!(
            check_query("PRAGMA \"aux\".foreign_key_check([it's])"),
            "SELECT * FROM pragma_foreign_key_check('it''s', 'aux')"
        );
        assert_eq!(
            check_query("PRAGMA foreign_key_check"),
            "SELECT * FROM pragma_foreign_key_check()"
        );
        let select = "SELECT k.* FROM pragma_foreign_key_check('t') AS k";
        assert_eq!(check_query(select), select);
        let guard = fk_guard_sql(select);
        assert!(guard.contains(&format!("FROM ({select})")), "{guard}");
        assert!(guard.contains(FK_GUARD_MARKER), "{guard}");

        // what SQLite reports for the guard's path
        let err = AppError::db(format!(
            "bad JSON path: '{FK_GUARD_MARKER}2\nc 1 p 0\nit''s NULL p 0'"
        ));
        let violation = guard_violation(&err).expect("guard error");
        let payload = violation.to_payload();
        assert_eq!(
            payload.message,
            "the change would leave 2 row(s) violating foreign keys"
        );
        assert_eq!(
            payload.detail.as_deref(),
            Some("table rowid parent fkid:\nc 1 p 0\nit's NULL p 0")
        );
        assert!(guard_violation(&AppError::db("bad JSON path: '$x'")).is_none());
    }

    /// A session that only logs what runs, yielding at every call (a SQLite one unless
    /// `session` names its driver and dialect). `fail` makes every transaction fail with that
    /// error.
    #[derive(Default)]
    struct Recorder {
        log: Mutex<Vec<String>>,
        session: Option<(Driver, SqlDialect)>,
        in_tx: Option<bool>,
        fail: Option<AppError>,
    }

    impl Recorder {
        fn of(dialect: Dialect) -> Recorder {
            let driver = match dialect {
                Dialect::Postgres => Driver::Postgres,
                Dialect::Mysql => Driver::Mysql,
                Dialect::Sqlite => Driver::Sqlite,
            };
            Recorder {
                session: Some((driver, dialect.into())),
                ..Recorder::default()
            }
        }

        async fn record(&self, entry: String) {
            self.log.lock().unwrap().push(entry);
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    }

    #[async_trait::async_trait]
    impl DbSession for Recorder {
        fn driver(&self) -> Driver {
            self.session.map_or(Driver::Sqlite, |(driver, _)| driver)
        }
        fn dialect(&self) -> SqlDialect {
            self.session
                .map_or(SqlDialect::Sqlite, |(_, dialect)| dialect)
        }
        async fn server_info(&self) -> Result<crate::drivers::ServerInfo, AppError> {
            unimplemented!()
        }
        async fn ping(&self) -> Result<(), AppError> {
            unimplemented!()
        }
        async fn close(&self) {}
        async fn list_databases(
            &self,
        ) -> Result<Vec<crate::types::schema::DatabaseInfo>, AppError> {
            unimplemented!()
        }
        async fn list_schemas(&self) -> Result<Vec<crate::types::schema::SchemaInfo>, AppError> {
            unimplemented!()
        }
        async fn list_tables(
            &self,
            _schema: &str,
        ) -> Result<Vec<crate::types::schema::TableSummary>, AppError> {
            unimplemented!()
        }
        async fn table_meta(
            &self,
            _table: &TableRef,
        ) -> Result<crate::types::schema::TableMeta, AppError> {
            unimplemented!()
        }
        async fn view_definition(&self, _table: &TableRef) -> Result<Option<String>, AppError> {
            unimplemented!()
        }
        async fn completion_metadata(
            &self,
        ) -> Result<crate::types::schema::CompletionMetadata, AppError> {
            unimplemented!()
        }
        async fn query(&self, _sql: &str, _params: &[Value]) -> Result<ResultSet, AppError> {
            unimplemented!()
        }
        async fn execute(&self, sql: &str, _params: &[Value]) -> Result<ExecOutcome, AppError> {
            self.record(sql.to_string()).await;
            Ok(ExecOutcome {
                affected: 0,
                last_insert_id: None,
                rows: None,
            })
        }
        async fn transaction(
            &self,
            statements: Vec<TxStatement>,
        ) -> Result<Vec<ExecOutcome>, AppError> {
            let sqls: Vec<String> = statements.into_iter().map(|s| s.sql).collect();
            self.record(format!("tx[{}]", sqls.join("; "))).await;
            match &self.fail {
                Some(err) => Err(err.clone()),
                None => Ok(Vec::new()),
            }
        }
        async fn run_statement(
            &self,
            _sql: &str,
            _max_rows: usize,
            _sink: &mut dyn crate::drivers::RowSink,
            _cancel: crate::drivers::CancelSlot,
        ) -> Result<crate::drivers::StatementOutcome, AppError> {
            unimplemented!()
        }
        async fn stream_query(
            &self,
            _sql: &str,
            _params: &[Value],
            _sink: &mut dyn crate::drivers::RowSink,
            _cancel: crate::drivers::CancelSlot,
        ) -> Result<u64, AppError> {
            unimplemented!()
        }
        async fn cancel(&self, _handle: crate::drivers::CancelHandle) -> Result<(), AppError> {
            Ok(())
        }
        fn in_transaction(&self) -> Option<bool> {
            self.in_tx
        }
    }

    /// Plans come from the core planners: a session of an extension dialect, or of a driver
    /// beyond the core three, runs none of them.
    #[tokio::test]
    async fn no_plan_runs_without_the_core_generators() {
        for session in [
            (Driver::Postgres, SqlDialect::Redshift),
            (Driver::Libsql, SqlDialect::Sqlite),
        ] {
            let recorder = Recorder {
                session: Some(session),
                ..Recorder::default()
            };
            let result = apply_plan(&recorder, &rebuild_plan()).await;
            let error = result.error.expect("refused");
            assert_eq!(error.code, "UNSUPPORTED", "{session:?}");
            assert!(result.executed.is_empty());
            assert!(recorder.log.lock().unwrap().is_empty());
        }
    }

    /// The toggles around a rebuild used to be jobs of their own: work queued on the
    /// connection while the block ran (a grid commit) ran before `foreign_keys = ON`, with
    /// cascades skipped. Now toggles and block are one job, followed by the restoring toggles.
    #[tokio::test]
    async fn plans_on_one_sqlite_session_do_not_interleave() {
        let session = Recorder::default();
        let plan_a = rebuild_plan();
        let mut plan_b = rebuild_plan();
        plan_b.statements[2] = "CREATE TABLE \"u__tabler_new\" (\"a\" INT)".into();
        let (a, b) = tokio::join!(apply_plan(&session, &plan_a), apply_plan(&session, &plan_b));
        assert!(a.error.is_none() && b.error.is_none());
        assert_eq!(a.executed, plan_a.statements);
        assert_eq!(b.executed, plan_b.statements);
        let log = session.log.lock().unwrap().clone();
        let tx = |table: &str| {
            format!(
                "tx[COMMIT; PRAGMA foreign_keys = OFF; BEGIN; CREATE TABLE \"{table}__tabler_new\" (\"a\" INT); {}; COMMIT; PRAGMA foreign_keys = ON; BEGIN]",
                fk_guard_sql("PRAGMA foreign_key_check(\"t\")")
            )
        };
        // each block is one job with its toggles, and the second plan waits for the first
        assert_eq!(
            log,
            [
                tx("t"),
                "PRAGMA foreign_keys = ON".to_string(),
                tx("u"),
                "PRAGMA foreign_keys = ON".to_string(),
            ]
        );
    }

    #[test]
    fn finds_blocks_with_their_toggles() {
        let plan = rebuild_plan();
        let block = Block {
            start: 0,
            begin: 1,
            end: 4,
            stop: 6,
        };
        assert_eq!(block_at(&plan.statements, 0), Some(block));
        assert_eq!(
            block_at(&plan.statements, 1),
            Some(Block { start: 1, ..block })
        );
        assert_eq!(block_at(&plan.statements, 2), None);
        let lone: Vec<String> = ["PRAGMA foreign_keys = OFF", "VACUUM"]
            .map(String::from)
            .to_vec();
        assert_eq!(block_at(&lone, 0), None);
        assert_eq!(
            last_toggles(&[
                "PRAGMA foreign_keys = OFF".into(),
                "PRAGMA legacy_alter_table = ON".into(),
                "PRAGMA legacy_alter_table = OFF".into(),
                "PRAGMA foreign_keys = ON".into(),
            ]),
            [
                "PRAGMA legacy_alter_table = OFF",
                "PRAGMA foreign_keys = ON"
            ]
        );
    }

    /// DDL used to wait for locks without limit (PG `lock_timeout` and MySQL
    /// `lock_wait_timeout` default to none / a year), stalling every other client of the table.
    #[tokio::test]
    async fn lock_waits_are_bounded_on_postgres_and_mysql() {
        let pg = Recorder::of(Dialect::Postgres);
        let plan = DdlPlan {
            statements: vec!["ALTER TABLE t ADD COLUMN c INT".into()],
            warnings: vec![],
            transactional: true,
        };
        let result = apply_plan(&pg, &plan).await;
        assert!(result.error.is_none(), "{result:?}");
        assert_eq!(result.executed, plan.statements);
        assert_eq!(
            pg.log.lock().unwrap().clone(),
            ["tx[SET LOCAL lock_timeout = '5s'; ALTER TABLE t ADD COLUMN c INT]"]
        );

        let my = Recorder::of(Dialect::Mysql);
        let plan = DdlPlan {
            statements: vec![
                "ALTER TABLE t ADD COLUMN c INT".into(),
                "DROP TABLE u".into(),
            ],
            warnings: vec![],
            transactional: false,
        };
        let result = apply_plan(&my, &plan).await;
        assert!(result.error.is_none(), "{result:?}");
        assert_eq!(result.executed, plan.statements);
        assert_eq!(
            my.log.lock().unwrap().clone(),
            [
                "tx[SET SESSION lock_wait_timeout = 5; ALTER TABLE t ADD COLUMN c INT]",
                "tx[SET SESSION lock_wait_timeout = 5; DROP TABLE u]",
            ]
        );

        // the failing statement is reported by its index in the plan
        let mut err =
            AppError::db("canceling statement due to lock timeout").with_statement_index(2);
        if let AppError::Db { sqlstate, .. } = &mut err {
            *sqlstate = Some("55P03".into());
        }
        let pg = Recorder {
            fail: Some(err),
            in_tx: Some(true),
            ..Recorder::of(Dialect::Postgres)
        };
        let plan = DdlPlan {
            statements: vec![
                "COMMENT ON TABLE t IS NULL".into(),
                "ALTER TABLE t ADD c INT".into(),
            ],
            warnings: vec![],
            transactional: true,
        };
        let result = apply_plan(&pg, &plan).await;
        assert_eq!(result.failed_index, Some(1), "{result:?}");
        assert!(result.executed.is_empty());
        let payload = result.error.expect("error");
        assert_eq!(payload.statement_index, Some(1));
        assert!(
            payload.hint.as_deref().unwrap_or("").contains("SQL editor"),
            "{payload:?}"
        );
    }

    #[test]
    fn lock_wait_failures_get_a_hint() {
        let payload = |sqlstate: Option<&str>, vendor_code: Option<i64>| ErrorPayload {
            code: "DB_ERROR".into(),
            message: "x".into(),
            detail: None,
            hint: None,
            sqlstate: sqlstate.map(str::to_string),
            vendor_code,
            position: None,
            statement_index: None,
            data: None,
        };
        let cases = [
            (Dialect::Postgres, payload(Some("55P03"), None), true),
            (Dialect::Postgres, payload(Some("23505"), None), false),
            (Dialect::Mysql, payload(None, Some(1205)), true),
            (Dialect::Mysql, payload(None, Some(1060)), false),
            (Dialect::Sqlite, payload(None, Some(5)), true),
            (Dialect::Sqlite, payload(None, Some(517)), true),
            (Dialect::Sqlite, payload(None, Some(19)), false),
        ];
        for (dialect, mut p, hinted) in cases {
            explain_lock_wait(&Recorder::of(dialect), dialect, &mut p);
            assert_eq!(p.hint.is_some(), hinted, "{dialect:?} {p:?}");
            if hinted {
                assert!(p.hint.unwrap().contains("Another session"));
            }
        }
        let editor = Recorder {
            in_tx: Some(true),
            ..Recorder::of(Dialect::Mysql)
        };
        let mut p = payload(None, Some(1205));
        explain_lock_wait(&editor, Dialect::Mysql, &mut p);
        assert!(
            p.hint
                .unwrap()
                .contains("SQL editor has an open transaction")
        );
    }

    #[test]
    fn transactional_batches_per_dialect() {
        let plan: Vec<String> = vec!["A".into(), "B".into()];
        let sqls = |d: Dialect| {
            let (batch, offset) = transactional_batch(d, &plan);
            (batch.into_iter().map(|s| s.sql).collect::<Vec<_>>(), offset)
        };
        assert_eq!(
            sqls(Dialect::Postgres),
            (
                vec![
                    "SET LOCAL lock_timeout = '5s'".to_string(),
                    "A".into(),
                    "B".into()
                ],
                1
            )
        );
        assert_eq!(
            sqls(Dialect::Sqlite),
            (
                vec!["A".to_string(), "B".into(), "COMMIT".into(), "BEGIN".into()],
                0
            )
        );
        assert_eq!(
            sqls(Dialect::Mysql),
            (
                vec![
                    "SET SESSION lock_wait_timeout = 5".to_string(),
                    "A".into(),
                    "B".into()
                ],
                1
            )
        );
    }

    async fn lite_session() -> (Arc<dyn DbSession>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut p = profile(Driver::Sqlite, true);
        p.sqlite_path = Some(dir.path().join("t.sqlite").to_string_lossy().into_owned());
        let session = crate::drivers::connect(crate::drivers::ConnectTarget {
            profile: &p,
            password: None,
            host_override: None,
        })
        .await
        .expect("open sqlite");
        (session, dir)
    }

    async fn count(session: &Arc<dyn DbSession>, sql: &str) -> i64 {
        match session.query(sql, &[]).await.expect(sql).rows[0][0] {
            Value::Int(n) => n,
            ref other => panic!("{sql}: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_sqlite_block_runs_as_one_transaction_and_its_check_can_fail_it() {
        let (session, _dir) = lite_session().await;
        for sql in [
            "CREATE TABLE p (id INTEGER PRIMARY KEY)",
            "CREATE TABLE c (pid INTEGER REFERENCES p(id))",
            "PRAGMA foreign_keys = OFF",
            "INSERT INTO c VALUES (7)",
            "PRAGMA foreign_keys = ON",
        ] {
            session.execute(sql, &[]).await.expect(sql);
        }
        let plan = |check: &str| DdlPlan {
            statements: vec![
                "PRAGMA foreign_keys = OFF".into(),
                "BEGIN".into(),
                "CREATE TABLE x (a)".into(),
                "PRAGMA legacy_alter_table = ON".into(),
                "PRAGMA legacy_alter_table = OFF".into(),
                check.into(),
                "COMMIT".into(),
                "PRAGMA foreign_keys = ON".into(),
            ],
            warnings: vec![],
            transactional: false,
        };
        for check in [
            "PRAGMA foreign_key_check(\"c\")".to_string(),
            crate::sqlgen::ddl::plan_ddl(
                Dialect::Sqlite,
                Some(&session.table_meta(&TableRef::new(None, "p")).await.unwrap()),
                &[DdlOp::AlterColumnType {
                    table: TableRef::new(None, "p"),
                    name: "id".into(),
                    new_type: "INTEGER".into(),
                    using: None,
                }],
            )
            .unwrap()
            .statements
            .into_iter()
            .find(|s| is_fk_check(s))
            .unwrap(),
        ] {
            let plan = plan(&check);
            let result = apply_plan(session.as_ref(), &plan).await;
            assert_eq!(result.failed_index, Some(5), "{result:?}");
            let err = result.error.expect("error");
            assert_eq!(err.statement_index, Some(5));
            assert!(err.message.contains("1 row(s)"), "{err:?}");
            assert_eq!(
                result.executed,
                [
                    "PRAGMA foreign_keys = OFF",
                    "PRAGMA legacy_alter_table = OFF",
                    "PRAGMA foreign_keys = ON"
                ]
            );
            assert_eq!(
                count(
                    &session,
                    "SELECT count(*) FROM sqlite_master WHERE name = 'x'"
                )
                .await,
                0
            );
            assert_eq!(count(&session, "PRAGMA foreign_keys").await, 1);
            assert_eq!(count(&session, "PRAGMA legacy_alter_table").await, 0);
        }
        // once the data is fixed the block commits
        session
            .execute("INSERT INTO p VALUES (7)", &[])
            .await
            .expect("fix");
        let plan = plan("PRAGMA foreign_key_check(\"c\")");
        let result = apply_plan(session.as_ref(), &plan).await;
        assert!(result.error.is_none(), "{result:?}");
        assert_eq!(result.executed, plan.statements);
        assert_eq!(
            count(
                &session,
                "SELECT count(*) FROM sqlite_master WHERE name = 'x'"
            )
            .await,
            1
        );
        session.close().await;
    }

    async fn editor(session: &Arc<dyn DbSession>, sql: &str) {
        let mut sink = crate::drivers::CollectSink::default();
        session
            .run_statement(sql, 100, &mut sink, crate::drivers::CancelSlot::default())
            .await
            .expect(sql);
    }

    /// A rebuild whose COMMIT failed with SQLITE_BUSY (a reader, here the SQL editor's open
    /// transaction, holds the file) used to leave the worker connection inside the open
    /// transaction with foreign keys off: every later commit failed until reconnecting.
    #[tokio::test]
    async fn a_rebuild_whose_commit_is_busy_is_rolled_back() {
        let (session, _dir) = lite_session().await;
        for sql in [
            "CREATE TABLE p (id INTEGER PRIMARY KEY)",
            "CREATE TABLE t (id INTEGER PRIMARY KEY, a TEXT, pid INTEGER REFERENCES p(id))",
            "CREATE TABLE other (v INT)",
            "INSERT INTO p VALUES (1)",
            "INSERT INTO t VALUES (1, '7', 1)",
            // the worker gives up quickly instead of after 5 s
            "PRAGMA busy_timeout = 100",
        ] {
            session.execute(sql, &[]).await.expect(sql);
        }
        let table = TableRef::new(None, "t");
        let meta = session.table_meta(&table).await.expect("meta");
        let plan = crate::sqlgen::ddl::plan_ddl(
            Dialect::Sqlite,
            Some(&meta),
            &[DdlOp::AlterColumnType {
                table: table.clone(),
                name: "a".into(),
                new_type: "INTEGER".into(),
                using: None,
            }],
        )
        .expect("plan");

        editor(&session, "BEGIN").await;
        editor(&session, "SELECT count(*) FROM other").await;
        let result = apply_plan(session.as_ref(), &plan).await;
        let err = result.error.expect("the commit cannot get the lock");
        assert_eq!(err.vendor_code.map(|c| c & 0xff), Some(5), "{err:?}");
        assert!(
            err.hint.as_deref().unwrap_or("").contains("SQL editor"),
            "{err:?}"
        );
        editor(&session, "ROLLBACK").await;

        // the worker connection is usable and enforces foreign keys again
        assert_eq!(count(&session, "PRAGMA foreign_keys").await, 1);
        assert_eq!(count(&session, "PRAGMA legacy_alter_table").await, 0);
        session
            .transaction(vec![TxStatement::new(
                "INSERT INTO other VALUES (1)",
                vec![],
            )])
            .await
            .expect("a later commit");
        let meta = session.table_meta(&table).await.expect("meta");
        assert_eq!(
            meta.columns[1].data_type, "TEXT",
            "the change was rolled back"
        );
        assert_eq!(
            count(
                &session,
                "SELECT count(*) FROM sqlite_master WHERE instr(name, '__tabler') > 0"
            )
            .await,
            0
        );
        // and the same change applies once the reader is gone
        let result = apply_plan(session.as_ref(), &plan).await;
        assert!(result.error.is_none(), "{result:?}");
        let meta = session.table_meta(&table).await.expect("meta");
        assert_eq!(meta.columns[1].data_type, "INTEGER");
        session.close().await;
    }
}
