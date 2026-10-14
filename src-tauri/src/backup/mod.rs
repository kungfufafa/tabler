//! Backup / restore through the native dump binaries (TablePlus "Backup…" / "Restore…"):
//! - [`discover`]: the pg_dump / pg_restore / psql / mysqldump / mysql binaries on this Mac;
//! - [`version`]: their `--version` and the server's version;
//! - [`args`]: their command lines (the password only ever goes in the environment);
//! - [`detect`]: what a restore file holds (plain SQL, `PGDMP` custom archive, tar; gzip);
//! - [`run`]: running one, streaming its output, gzip in and out, cancel;
//! - [`sqlite`]: SQLite has no dump binary: its database file is copied;
//! - [`brew`]: "Install with Homebrew…" when no (recent enough) binary is installed.

pub mod args;
pub mod brew;
pub mod detect;
pub mod discover;
pub mod run;
pub mod sqlite;
pub mod version;

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use tokio_util::sync::CancellationToken;

use crate::error::AppError;

pub use run::shutdown;

static JOBS: LazyLock<Mutex<HashMap<String, CancellationToken>>> = LazyLock::new(Default::default);

fn jobs() -> std::sync::MutexGuard<'static, HashMap<String, CancellationToken>> {
    JOBS.lock().unwrap_or_else(|p| p.into_inner())
}

/// Keeps a job's cancellation token registered while it runs.
pub struct JobGuard {
    id: String,
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        jobs().remove(&self.id);
    }
}

/// Registers job `id`; refuses an id that is already running.
pub fn register_job(id: &str) -> Result<(JobGuard, CancellationToken), AppError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(AppError::InvalidInput("jobId is required".into()));
    }
    let mut map = jobs();
    if map.contains_key(id) {
        return Err(AppError::InvalidInput(format!(
            "job \"{id}\" is already running"
        )));
    }
    let token = CancellationToken::new();
    map.insert(id.to_string(), token.clone());
    Ok((JobGuard { id: id.to_string() }, token))
}

/// Cancels job `id`; false when no such job runs.
pub fn cancel_job(id: &str) -> bool {
    match jobs().get(id.trim()) {
        Some(token) => {
            token.cancel();
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_and_cancels_jobs() {
        let (guard, token) = register_job("backup-test-job").unwrap();
        assert!(register_job("backup-test-job").is_err());
        assert!(register_job("  ").is_err());
        assert!(cancel_job("backup-test-job"));
        assert!(token.is_cancelled());
        drop(guard);
        assert!(!cancel_job("backup-test-job"));
        assert!(register_job("backup-test-job").is_ok());
    }
}
