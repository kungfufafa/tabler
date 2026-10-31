//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md).

use std::sync::Arc;

use crate::drivers::native::{CallKind, NativeSession, unknown_call};
use crate::drivers::registry::{self, UtilInput};
use crate::drivers::{ConnectTarget, Session};
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, Driver};

use super::params::MongoParams;
use super::session::MongoSession;

const DRIVER: Driver = Driver::Mongodb;

/// Opens a MongoDB session (a native store: no SQL). The MCP process is refused: MCP serves no
/// native store in v1.
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    let session = MongoSession::open(target, registry::unattended()).await?;
    Ok(Session::Native(Arc::new(session) as Arc<dyn NativeSession>))
}

/// The profile's MongoDB settings (`MongoParams`), checked against its other fields.
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    MongoParams::of_profile(profile).map(|_| ())
}

/// The connection form has no MongoDB calls.
pub async fn util(
    _input: UtilInput<'_>,
    method: &str,
    _args: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    Err(unknown_call(DRIVER, method))
}

/// Whether a `util` call only reads; there are none, and anything unknown changes something.
pub fn util_kind(_method: &str) -> CallKind {
    CallKind::Write
}
