//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md): a DuckDB database
//! file (or `:memory:`) opened in this process, read-only when the profile is, with the
//! profile's password as the encryption key of an encrypted file. No host, TLS or SSH: the
//! descriptor offers none.

use std::sync::Arc;

use serde::Deserialize;

use super::engine::{self, Instance, Location};
use super::params::{self, DuckdbParams};
use super::session::{DuckdbSession, LOGGER_REFUSED, SessionOptions};
use crate::conn_options::effective_read_only;
use crate::drivers::native::{CallKind, unknown_call};
use crate::drivers::registry::{self, UtilInput};
use crate::drivers::{ConnectTarget, Session};
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::sqlgen::dialect::duckdb::switches_logger;
use crate::types::caps::SafeConfirm;
use crate::types::profile::{ConnectionProfile, Driver, PasswordMode};

const DRIVER: Driver = Driver::Duckdb;

/// The connection form's calls: `checkFile` (is the path a DuckDB database?) and `createFile`
/// (New…: a new, empty database file at the path).
pub const CHECK_FILE: &str = "checkFile";
pub const CREATE_FILE: &str = "createFile";

/// Opens a session on the profile's database: hardened in the MCP process.
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    let session = open_session(&target, registry::unattended()).await?;
    Ok(Session::Sql(Arc::new(session)))
}

/// The encryption key of a profile: none without a password, a Command Line's output, or the
/// password the connect resolved.
async fn encryption_key(
    profile: &ConnectionProfile,
    password: Option<&str>,
) -> Result<Option<String>, AppError> {
    if profile.options.password_mode() == PasswordMode::None {
        return Ok(None);
    }
    if let Some(command) = CommandPassword::for_profile(profile, password) {
        return command.password().await.map(Some);
    }
    Ok(password.map(str::to_string))
}

/// `open` with the hardening of an unattended process given (`registry::unattended` is
/// process-wide and never goes back, so tests pass it).
pub async fn open_session(
    target: &ConnectTarget<'_>,
    unattended: bool,
) -> Result<DuckdbSession, AppError> {
    let profile = target.profile;
    validate(profile)?;
    let key = encryption_key(profile, target.password).await?;
    let config = params::engine_config(profile, key.as_deref(), unattended)?;
    // DuckDB would create a missing file: a moved or mistyped path must not "connect". A file
    // this process has open is DuckDB's already (and on Windows not readable beside it).
    if let Location::File(path) = &config.location
        && engine::open_instance(path).is_none()
    {
        engine::check_database_file(&path.to_string_lossy())?;
    }
    let instance = tokio::task::spawn_blocking(move || Instance::open(config))
        .await
        .map_err(|e| AppError::Internal(format!("open duckdb: {e}")))??;
    let confirm = if profile.options.password_mode() == PasswordMode::Command {
        SafeConfirm::Passcode
    } else {
        SafeConfirm::Password
    };
    DuckdbSession::start(
        instance,
        SessionOptions {
            read_only: effective_read_only(profile),
            database: profile.database.clone(),
            confirm,
        },
    )
    .await
}

/// The profile's DuckDB settings (`DuckdbParams`), a read-only profile on a file, and startup
/// SQL that leaves the logger alone.
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    DuckdbParams::from_driver_params(profile.options.driver_params.as_ref())?;
    let location = Location::parse(profile.sqlite_path.as_deref().unwrap_or_default());
    if location == Location::Memory && effective_read_only(profile) {
        return Err(AppError::InvalidInput(
            "An in-memory DuckDB database cannot be opened read-only: choose a file.".into(),
        ));
    }
    if profile
        .options
        .startup_sql
        .as_deref()
        .is_some_and(switches_logger)
    {
        return Err(AppError::InvalidInput(format!(
            "Startup SQL: {LOGGER_REFUSED}"
        )));
    }
    Ok(())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PathArgs {
    path: Option<String>,
}

/// The path a call names, or the profile's file.
fn path_of(input: &UtilInput<'_>, args: serde_json::Value) -> Result<String, AppError> {
    let args: PathArgs =
        serde_json::from_value(args).map_err(|e| AppError::InvalidInput(format!("DuckDB: {e}")))?;
    args.path
        .or_else(|| input.profile.sqlite_path.clone())
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .ok_or_else(|| AppError::InvalidInput("DuckDB: choose a database file".into()))
}

/// The connection form's calls (`CHECK_FILE`, `CREATE_FILE`), each answering `{ "path": … }`.
pub async fn util(
    input: UtilInput<'_>,
    method: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    let path = match method {
        CHECK_FILE | CREATE_FILE => path_of(&input, args)?,
        _ => return Err(unknown_call(DRIVER, method)),
    };
    if method == CREATE_FILE && input.secret.is_some_and(|s| !s.is_empty()) {
        // DuckDB 1.5's built-in crypto only decrypts (writing needs httpfs's OpenSSL).
        return Err(AppError::InvalidInput(
            "DuckDB in Tabler cannot create encrypted files: clear the encryption key, or \
             create the file with the DuckDB CLI."
                .into(),
        ));
    }
    let checked = path.clone();
    let method = method.to_string();
    tokio::task::spawn_blocking(move || {
        if method == CREATE_FILE {
            engine::create_database_file(&checked)
        } else if engine::open_instance(std::path::Path::new(&checked)).is_some() {
            // Open in this Tabler: DuckDB read it already.
            Ok(())
        } else {
            engine::check_database_file(&checked)
        }
    })
    .await
    .map_err(|e| AppError::Internal(format!("duckdb file: {e}")))??;
    Ok(serde_json::json!({ "path": path }))
}

/// `checkFile` only reads; anything else (`createFile`, an unknown call) changes something.
pub fn util_kind(method: &str) -> CallKind {
    match method {
        CHECK_FILE => CallKind::Read,
        _ => CallKind::Write,
    }
}
