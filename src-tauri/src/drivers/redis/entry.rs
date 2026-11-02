//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md).

use std::sync::Arc;

use crate::drivers::native::{CallKind, NativeSession, unknown_call};
use crate::drivers::registry::UtilInput;
use crate::drivers::{ConnectTarget, Session};
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, Driver};

use super::conn::startup_commands;
use super::params::{RedisParams, database_index};
use super::session::RedisSession;

const DRIVER: Driver = Driver::Redis;

/// Opens a session on the profile's server (a native store: it speaks no SQL).
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    let session = RedisSession::connect(target).await?;
    Ok(Session::Native(Arc::new(session) as Arc<dyn NativeSession>))
}

/// The driver's own checks of a profile being saved or tested, after the registry's common
/// ones: its settings, its DB index and its bootstrap commands.
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    RedisParams::from_driver_params(profile.options.driver_params.as_ref())?;
    database_index(profile)?;
    startup_commands(profile, crate::conn_options::effective_read_only(profile))?;
    Ok(())
}

/// The connection form has no call of its own for Redis.
pub async fn util(
    _input: UtilInput<'_>,
    method: &str,
    _args: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    Err(unknown_call(DRIVER, method))
}

/// Whether a `util` call only reads; there is none, and anything unknown changes something.
pub fn util_kind(_method: &str) -> CallKind {
    CallKind::Write
}
