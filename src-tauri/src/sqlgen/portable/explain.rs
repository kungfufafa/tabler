//! EXPLAIN of the extension dialects, as their `SqlKnobs::explain` writes it.

use crate::error::AppError;
use crate::sqlgen::dialect::spec::{DialectSpec, ExplainForm};
use crate::types::query::ExplainMode;

fn mode_name(mode: ExplainMode) -> &'static str {
    match mode {
        ExplainMode::Plan => "Explain",
        ExplainMode::Analyze => "Explain Analyze",
    }
}

/// The text put in front of a statement for `mode`, or why there is none.
pub fn explain_prefix(spec: &DialectSpec, mode: ExplainMode) -> Result<&'static str, AppError> {
    match (spec.knobs.explain)(mode) {
        ExplainForm::Prefix(prefix) => Ok(prefix),
        ExplainForm::Unsupported => Err(AppError::Unsupported(format!(
            "{}: {} is not available",
            spec.label,
            mode_name(mode)
        ))),
    }
}

/// Wraps one statement in the dialect's EXPLAIN. A trailing `;` (and whitespace) is dropped.
pub fn wrap_explain(spec: &DialectSpec, sql: &str, mode: ExplainMode) -> Result<String, AppError> {
    Ok(format!(
        "{} {}",
        explain_prefix(spec, mode)?,
        strip_terminator(sql)
    ))
}

/// Characters `wrap_explain` adds before the statement (0 when there is no EXPLAIN).
pub fn explain_offset(spec: &DialectSpec, mode: ExplainMode) -> usize {
    explain_prefix(spec, mode).map_or(0, |p| p.chars().count() + 1)
}

/// What an executing Explain Analyze runs in so that its changes are undone (see
/// `sqlgen::explain::analyze_wrapper`): the dialect's `analyze_tx` around it when no transaction
/// is open. These dialects have no savepoint form, so inside a transaction, or with the state
/// unknown, or without `analyze_tx`, a statement that writes is refused and anything else runs
/// as is.
pub fn analyze_wrapper(
    spec: &DialectSpec,
    in_tx: Option<bool>,
    writes: bool,
) -> Result<Option<(String, Vec<String>)>, AppError> {
    match (spec.knobs.analyze_tx, in_tx) {
        (Some((open, close)), Some(false)) => Ok(Some((open.to_string(), vec![close.to_string()]))),
        _ if writes => Err(AppError::Unsupported(format!(
            "{}: Explain Analyze would really run this statement, and its changes could not be \
             undone here; use Explain instead",
            spec.label
        ))),
        _ => Ok(None),
    }
}

fn strip_terminator(sql: &str) -> &str {
    let mut s = sql.trim_end();
    while let Some(rest) = s.strip_suffix(';') {
        s = rest.trim_end();
    }
    s
}
