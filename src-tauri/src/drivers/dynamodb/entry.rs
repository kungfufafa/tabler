//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md): opening a session,
//! checking a profile before it is saved or tested, and the connection form's calls.

use std::sync::Arc;

use serde_json::json;

use super::auth;
use super::params::{DynamoAuth, DynamoParams, check_region};
use super::session::DynamoSession;
use crate::drivers::native::{CallKind, unknown_call};
use crate::drivers::registry::{UtilInput, unattended};
use crate::drivers::{ConnectTarget, Session};
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, Driver, PasswordMode};

const DRIVER: Driver = Driver::Dynamodb;

/// The regions the form offers (any other name in AWS's spelling is taken too: new regions,
/// emulators).
pub const REGIONS: &[&str] = &[
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
    "af-south-1",
    "ap-east-1",
    "ap-east-2",
    "ap-south-1",
    "ap-south-2",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ap-southeast-4",
    "ap-southeast-5",
    "ap-southeast-7",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ca-central-1",
    "ca-west-1",
    "eu-central-1",
    "eu-central-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "eu-south-1",
    "eu-south-2",
    "eu-north-1",
    "il-central-1",
    "me-central-1",
    "me-south-1",
    "mx-central-1",
    "sa-east-1",
    "us-gov-east-1",
    "us-gov-west-1",
    "cn-north-1",
    "cn-northwest-1",
];

/// Opens a session of the profile's region (or endpoint) and proves its credentials. The MCP
/// process gets none: native stores are not offered to MCP clients in v1.
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    open_as(target, unattended()).await
}

/// [`open`] in the MCP server's process (`unattended`) or in the app's.
pub(crate) async fn open_as(
    target: ConnectTarget<'_>,
    unattended: bool,
) -> Result<Session, AppError> {
    if unattended {
        return Err(AppError::Unsupported(format!(
            "{} connections are not available to MCP clients",
            DRIVER.label()
        )));
    }
    Ok(Session::Native(Arc::new(
        DynamoSession::open(&target).await?,
    )))
}

/// The profile's `DynamoParams`, its region, and the password's Command Line mode for the
/// Command source. A missing access key or secret is the connect's error (`Auth`): a profile is
/// saved before its secret is.
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    let params = DynamoParams::from_driver_params(profile.options.driver_params.as_ref())?;
    check_region(profile.database.as_deref().unwrap_or_default())?;
    let command = profile.options.password_mode() == PasswordMode::Command;
    match params.auth {
        DynamoAuth::Command if !command => Err(AppError::InvalidInput(
            "DynamoDB: the Command source reads the password's Command Line mode; choose it \
             for the secret access key"
                .into(),
        )),
        _ => Ok(()),
    }
}

/// The form's calls: `regions` (the region picker) and `awsProfiles` (the names of this
/// machine's AWS profiles).
pub async fn util(
    _input: UtilInput<'_>,
    method: &str,
    _args: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    match method {
        "regions" => Ok(json!(REGIONS)),
        "awsProfiles" => Ok(json!(auth::profile_names(&|k| std::env::var(k).ok()).await)),
        _ => Err(unknown_call(DRIVER, method)),
    }
}

/// Both of the form's calls only read; anything unknown changes something.
pub fn util_kind(method: &str) -> CallKind {
    match method {
        "regions" | "awsProfiles" => CallKind::Read,
        _ => CallKind::Write,
    }
}
