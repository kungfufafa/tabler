//! The entry points `drivers::registry` calls. The foundation owns this file until the lane is
//! handed over; until then it only reports that the driver is not implemented, and the lane's
//! other work goes in other files of this module.

use std::sync::Arc;

use crate::drivers::native::CallKind;
use crate::drivers::registry::UtilInput;
use crate::drivers::{ConnectTarget, DbSession};
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, Driver};

const DRIVER: Driver = Driver::Mongodb;

/// Opens a session on the profile's server.
pub async fn open(_target: ConnectTarget<'_>) -> Result<Arc<dyn DbSession>, AppError> {
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
