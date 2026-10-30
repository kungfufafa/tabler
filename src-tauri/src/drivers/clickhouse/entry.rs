//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md). The lane owns this
//! file; until its driver is done, `open` reports that it is not implemented yet.

use crate::drivers::native::CallKind;
use crate::drivers::registry::UtilInput;
use crate::drivers::{ConnectTarget, Session};
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, Driver};

const DRIVER: Driver = Driver::Clickhouse;

/// Opens a session on the profile's server: `Session::Sql` for a SQL engine, `Session::Native`
/// for a store that speaks no SQL.
pub async fn open(_target: ConnectTarget<'_>) -> Result<Session, AppError> {
    Err(AppError::Unsupported(format!(
        "{} driver: not implemented yet",
        DRIVER.label()
    )))
}

/// The driver's own checks of a profile being saved or tested (its `driver_params`), after the
/// registry's common ones.
pub fn validate(_profile: &ConnectionProfile) -> Result<(), AppError> {
    Ok(())
}

/// A call of the connection form that needs no session (pickers, probes, creating a file).
pub async fn util(
    _input: UtilInput<'_>,
    method: &str,
    _args: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    Err(AppError::Unsupported(format!(
        "{}: unknown call \u{201c}{method}\u{201d}",
        DRIVER.label()
    )))
}

/// Whether a `util` call only reads; anything unknown changes something.
pub fn util_kind(_method: &str) -> CallKind {
    CallKind::Write
}
