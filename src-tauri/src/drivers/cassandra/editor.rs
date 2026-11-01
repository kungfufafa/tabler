//! The editor's runs: cqlsh's session commands (`CONSISTENCY`, `TRACING`, `PAGING`, …) change
//! the session's settings and are never sent, `USE` switches every connection's keyspace, and
//! anything else is sent at the editor's consistency, page by page up to the row cap. A traced
//! statement's trace comes back as notices.

use super::cqlsh;
use super::params::MAX_PAGE_SIZE;
use super::session::{
    CassandraSession, Paged, applied, cancellable, changes_schema, lock, use_target,
};
use crate::drivers::{CancelSlot, RowChunker, RowSink, StatementOutcome, cancel_requested};
use crate::error::AppError;
use crate::types::query::StatementKind;

pub(super) async fn run(
    session: &CassandraSession,
    sql: &str,
    max_rows: usize,
    sink: &mut dyn RowSink,
    cancel: CancelSlot,
) -> Result<StatementOutcome, AppError> {
    if cancel_requested(&cancel) {
        return Err(AppError::Cancelled);
    }
    if let Some(command) = cqlsh::parse(sql) {
        return Ok(notice(session.apply_cqlsh(command?)));
    }
    if let Some(name) = use_target(sql) {
        let keyspace = cancellable(&cancel, session.use_keyspace(&name)).await?;
        return Ok(notice(format!("Now using keyspace {keyspace}.")));
    }
    session.refuse_write(sql)?;
    let settings = *lock(&session.settings);
    let page_size = if settings.paging {
        settings.page_size
    } else {
        MAX_PAGE_SIZE
    };
    let mut chunker = RowChunker::new(sink);
    let (truncated, reply) = session
        .page_through(
            sql,
            &[],
            Paged {
                options: settings.options,
                page_size: i32::try_from(page_size).unwrap_or(i32::MAX),
                max_rows: Some(max_rows as u64),
            },
            &mut chunker,
            &cancel,
        )
        .await?;
    let _ = chunker.flush();
    let wire = session.wire()?;
    if changes_schema(sql) {
        cancellable(&cancel, wire.await_schema_agreement()).await?;
    }
    let mut notices = reply.warnings.clone();
    if let Some(id) = reply.tracing_id {
        match wire.tracing(id).await {
            Ok(lines) => notices.extend(lines),
            Err(e) => notices.push(format!("Tracing session {id}: {e}")),
        }
    }
    let rows = reply.is_rows;
    Ok(StatementOutcome {
        kind: if rows {
            StatementKind::Rows
        } else {
            StatementKind::Empty
        },
        result_sets: u32::from(rows),
        row_count: chunker.total_rows,
        truncated,
        affected: applied(&reply).map(u64::from),
        last_insert_id: None,
        notices,
    })
}

/// A statement the session answered itself.
fn notice(text: String) -> StatementOutcome {
    StatementOutcome {
        kind: StatementKind::Empty,
        notices: vec![text],
        ..StatementOutcome::default()
    }
}
