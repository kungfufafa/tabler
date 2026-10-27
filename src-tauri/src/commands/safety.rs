//! Safe mode: the password check behind Safe Mode 1 / 2 (and lowering a Safe level), and the
//! backend's own refusal to run writes on a read-only connection (defence in depth: the
//! renderer's guard already blocks them, and the session itself is read-only on the server).

use std::hint::black_box;

use tauri::State;
use uuid::Uuid;

use crate::error::AppError;
use crate::secrets::{SecretKind, Secrets};
use crate::sqlgen::SqlDialect;
use crate::sqlgen::split::is_meta_command;
use crate::sqlgen::sql::{split_statements, writes};
use crate::state::{AppState, Connection};
use crate::types::query::ExplainMode;

/// Message of the error returned for a write on a read-only connection.
pub const READ_ONLY_MESSAGE: &str = "This connection is read-only";

/// Compares two byte strings in time that depends only on their lengths, not on where they
/// differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= black_box(usize::from(x ^ y));
    }
    black_box(diff) == 0
}

/// Whether `password` is the profile's stored DB password or, for an "Ask every time" profile,
/// the `session` password typed at connect. False when there is neither.
pub async fn verify_password(
    secrets: &Secrets,
    session: Option<String>,
    profile_id: Uuid,
    password: &str,
) -> Result<bool, AppError> {
    let stored = secrets.get(profile_id, SecretKind::DbPassword).await?;
    Ok(stored
        .or(session)
        .is_some_and(|s| constant_time_eq(s.as_bytes(), password.as_bytes())))
}

/// Checks the password typed in a safe-mode prompt against the connection's DB password.
#[tauri::command]
pub async fn verify_connection_password(
    state: State<'_, AppState>,
    profile_id: Uuid,
    password: String,
) -> Result<bool, AppError> {
    if state.appdb.get_profile(profile_id)?.is_none() {
        return Err(AppError::NotFound(format!("profile {profile_id}")));
    }
    let session = state.session_password(profile_id);
    let ok = verify_password(&state.secrets, session, profile_id, &password).await?;
    if !ok {
        log::info!("safe mode: wrong password for profile {profile_id}");
    }
    Ok(ok)
}

/// Whether running `sql` (split like the editor does; meta-commands are refused anyway) writes.
/// `explain`: each statement is wrapped in EXPLAIN (only planned: never a write) or EXPLAIN
/// ANALYZE (runs it: a write when the statement is one). `first_only`: only the first statement
/// is run (`explain`).
pub fn run_writes(
    sql: &str,
    dialect: impl Into<SqlDialect>,
    explain: Option<ExplainMode>,
    first_only: bool,
) -> bool {
    if explain == Some(ExplainMode::Plan) {
        return false;
    }
    let dialect = dialect.into();
    let statements = split_statements(dialect, sql);
    let take = if first_only { 1 } else { statements.len() };
    statements
        .iter()
        .take(take)
        .filter(|st| !is_meta_command(&st.sql))
        .any(|st| writes(dialect, &st.sql))
}

fn refuse(read_only: bool, writes: impl FnOnce() -> bool) -> Result<(), AppError> {
    if read_only && writes() {
        Err(AppError::InvalidInput(READ_ONLY_MESSAGE.into()))
    } else {
        Ok(())
    }
}

/// `run_sql`: refuses the whole run on a read-only connection when any statement writes.
pub fn refuse_read_only_run(
    conn: &Connection,
    sql: &str,
    dialect: SqlDialect,
    explain: Option<ExplainMode>,
) -> Result<(), AppError> {
    refuse(conn.profile.options.read_only, || {
        run_writes(sql, dialect, explain, false)
    })
}

/// `explain`: refuses an EXPLAIN ANALYZE of a write on a read-only connection.
pub fn refuse_read_only_explain(
    conn: &Connection,
    sql: &str,
    dialect: SqlDialect,
    mode: ExplainMode,
) -> Result<(), AppError> {
    refuse(conn.profile.options.read_only, || {
        run_writes(sql, dialect, Some(mode), true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appdb::AppDb;
    use crate::secrets::Backend;
    use crate::sqlgen::Dialect;

    #[test]
    fn constant_time_eq_compares_whole_strings() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secret1"));
        assert!(!constant_time_eq(b"secret", b""));
        assert!(!constant_time_eq(b"", b"x"));
    }

    #[tokio::test]
    async fn verifies_against_the_stored_db_password() {
        let secrets = Secrets::new(Backend::File, AppDb::open_in_memory().unwrap());
        let id = Uuid::new_v4();
        assert!(!verify_password(&secrets, None, id, "").await.unwrap());
        assert!(!verify_password(&secrets, None, id, "pw").await.unwrap());
        secrets.set(id, SecretKind::DbPassword, "pw").await.unwrap();
        // The SSH password is not the connection's password.
        secrets
            .set(id, SecretKind::SshPassword, "ssh")
            .await
            .unwrap();
        assert!(verify_password(&secrets, None, id, "pw").await.unwrap());
        assert!(!verify_password(&secrets, None, id, "PW").await.unwrap());
        assert!(!verify_password(&secrets, None, id, "ssh").await.unwrap());
        assert!(!verify_password(&secrets, None, id, "").await.unwrap());
        // The stored password wins over a session one.
        let session = Some("typed".to_string());
        assert!(
            !verify_password(&secrets, session.clone(), id, "typed")
                .await
                .unwrap()
        );
        // "Ask every time": only the password typed at connect.
        let other = Uuid::new_v4();
        assert!(
            verify_password(&secrets, session.clone(), other, "typed")
                .await
                .unwrap()
        );
        assert!(
            !verify_password(&secrets, session, other, "pw")
                .await
                .unwrap()
        );
    }

    #[test]
    fn run_writes_classifies_every_statement() {
        let pg = Dialect::Postgres;
        assert!(!run_writes("SELECT 1; SHOW search_path", pg, None, false));
        assert!(run_writes("SELECT 1; DELETE FROM t", pg, None, false));
        assert!(run_writes(
            "-- just a comment\nUPDATE t SET a = 1",
            pg,
            None,
            false
        ));
        // Meta-commands never reach the server (run_sql refuses them on its own).
        assert!(!run_writes("\\d users\nSELECT 1", pg, None, false));
        // A DELIMITER block is one splitter statement but holds two server statements.
        let block = "DELIMITER //\nSELECT 1; INSERT INTO t VALUES (1)//\nDELIMITER ;";
        assert!(run_writes(block, Dialect::Mysql, None, false));
    }

    #[test]
    fn run_writes_follows_the_explain_mode() {
        let sl = Dialect::Sqlite;
        assert!(!run_writes(
            "DELETE FROM t",
            sl,
            Some(ExplainMode::Plan),
            false
        ));
        assert!(run_writes(
            "DELETE FROM t",
            sl,
            Some(ExplainMode::Analyze),
            false
        ));
        assert!(!run_writes(
            "SELECT 1",
            sl,
            Some(ExplainMode::Analyze),
            false
        ));
        // `explain` only runs the first statement.
        assert!(!run_writes(
            "SELECT 1; DELETE FROM t",
            sl,
            Some(ExplainMode::Analyze),
            true
        ));
        assert!(run_writes(
            "SELECT 1; DELETE FROM t",
            sl,
            Some(ExplainMode::Analyze),
            false
        ));
    }

    #[test]
    fn refuse_only_on_read_only_connections() {
        assert!(refuse(false, || true).is_ok());
        assert!(refuse(true, || false).is_ok());
        let err = refuse(true, || true).unwrap_err();
        assert_eq!(err.to_string(), READ_ONLY_MESSAGE);
    }
}
