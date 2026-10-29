//! Safe mode: the password check behind Safe Mode 1 / 2 (and lowering a Safe level), and the
//! backend's own refusal to run writes on a read-only connection (defence in depth: the
//! renderer's guard already blocks them, and the session itself is read-only on the server).

use std::hint::black_box;

use tauri::State;
use uuid::Uuid;

use crate::drivers::native::{CallKind, NativeSession};
use crate::error::AppError;
use crate::passcode;
use crate::secrets::{SecretKind, Secrets};
use crate::sqlgen::SqlDialect;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::split::is_meta_command;
use crate::sqlgen::sql::{split_statements, writes};
use crate::state::{AppState, Connection};
use crate::types::profile::{ConnectionProfile, PasswordMode};
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

/// Whether Safe Mode confirms a write on `profile` with the app passcode (or, without one, the
/// connection's name) instead of its password: its secret is a token nobody types (an API key,
/// an access token), or a driver beyond the core three gets it from a command. The core
/// profiles keep their password prompt. Mirrors `passwordSecret` in the renderer's
/// `safety/guard.ts`.
pub fn confirms_with_passcode(profile: &ConnectionProfile) -> bool {
    profile.driver.descriptor().secret_is_token
        || (!profile.driver.is_core() && profile.options.password_mode() == PasswordMode::Command)
}

/// Whether `answer` is what the Safe Mode prompt of `profile_id` asks for: the app passcode when
/// [`confirms_with_passcode`] and one is set, else the connection's name; for every other
/// profile its DB password, as [`verify_password`] checks it (the prompt asks for the name only
/// when there is no password, and checks that itself).
pub async fn verify_safe_mode(
    state: &AppState,
    profile_id: Uuid,
    answer: &str,
) -> Result<bool, AppError> {
    let profile = state
        .appdb
        .get_profile(profile_id)?
        .ok_or_else(|| AppError::NotFound(format!("profile {profile_id}")))?;
    if !confirms_with_passcode(&profile) {
        let session = state.session_password(profile_id);
        return verify_password(&state.secrets, session, profile_id, answer).await;
    }
    let Some(stored) = passcode::load(&state.appdb)? else {
        let name = profile.name.trim();
        return Ok(!name.is_empty() && constant_time_eq(name.as_bytes(), answer.trim().as_bytes()));
    };
    // PBKDF2 at the passcode's cost: off the async workers.
    let answer = answer.to_string();
    tokio::task::spawn_blocking(move || passcode::verify(&stored, &answer))
        .await
        .map_err(|e| AppError::internal(format!("background task failed: {e}")))
}

/// Checks what was typed in a safe-mode prompt (see [`verify_safe_mode`]).
#[tauri::command]
pub async fn verify_connection_password(
    state: State<'_, AppState>,
    profile_id: Uuid,
    password: String,
) -> Result<bool, AppError> {
    let ok = verify_safe_mode(&state, profile_id, &password).await?;
    if !ok {
        log::info!("safe mode: wrong answer for profile {profile_id}");
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

/// `run_sql` on a store that speaks no SQL: refuses the whole run on a read-only connection when
/// any command writes, as the store classifies it (a command it does not know writes).
pub fn refuse_read_only_console(
    conn: &Connection,
    store: &dyn NativeSession,
    text: &str,
) -> Result<(), AppError> {
    refuse(conn.profile.options.read_only, || {
        store
            .split_console(text)
            .iter()
            .any(|c| store.classify_console(&c.sql) == StatementClass::Write)
    })
}

/// `session_call` and `driver_util`: refuses a call that may change something (`CallKind::Write`,
/// which is what a driver says of a call it does not know) on a read-only connection.
pub fn refuse_read_only_call(read_only: bool, kind: CallKind) -> Result<(), AppError> {
    refuse(read_only, || kind == CallKind::Write)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appdb::AppDb;
    use crate::secrets::Backend;
    use crate::sqlgen::Dialect;
    use crate::types::profile::{Driver, ProfileOptions, SslMode};

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

    fn safety_state() -> AppState {
        let appdb = AppDb::open_in_memory().unwrap();
        AppState::new(appdb.clone(), Secrets::new(Backend::File, appdb))
    }

    fn saved(state: &AppState, driver: Driver, mode: PasswordMode) -> ConnectionProfile {
        let profile = ConnectionProfile {
            id: Uuid::new_v4(),
            name: "Billing prod".into(),
            color: None,
            group: None,
            driver,
            host: Some("db".into()),
            port: None,
            user: Some("app".into()),
            database: None,
            ssl: SslMode::Require,
            ssl_ca_path: None,
            sqlite_path: None,
            ssh: None,
            options: ProfileOptions {
                password_mode: Some(mode),
                password_command: Some("printf tok".into()),
                ..ProfileOptions::default()
            },
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        };
        state.appdb.upsert_profile(&profile).unwrap()
    }

    fn set_passcode(state: &AppState, code: &str) {
        let stored = passcode::hash(code, 1_000).unwrap();
        passcode::store(&state.appdb, Some(&stored)).unwrap();
    }

    #[test]
    fn only_token_and_non_core_command_secrets_confirm_with_the_passcode() {
        let state = safety_state();
        for mode in [
            PasswordMode::Keychain,
            PasswordMode::Ask,
            PasswordMode::Command,
        ] {
            for core in [Driver::Postgres, Driver::Mysql, Driver::Sqlite] {
                assert!(
                    !confirms_with_passcode(&saved(&state, core, mode)),
                    "{core:?}"
                );
            }
            // An API token is never typed again; a password is, unless a command prints it.
            assert!(confirms_with_passcode(&saved(&state, Driver::D1, mode)));
            assert_eq!(
                confirms_with_passcode(&saved(&state, Driver::Clickhouse, mode)),
                mode == PasswordMode::Command
            );
        }
    }

    #[tokio::test]
    async fn safe_mode_of_a_token_profile_takes_the_passcode_else_the_name() {
        let state = safety_state();
        let d1 = saved(&state, Driver::D1, PasswordMode::Keychain);
        // The stored token is not what the prompt asks for.
        state
            .secrets
            .set(d1.id, SecretKind::DbPassword, "api-token")
            .await
            .unwrap();
        // No passcode: the connection's name (trimmed), nothing else.
        assert!(
            verify_safe_mode(&state, d1.id, "Billing prod")
                .await
                .unwrap()
        );
        assert!(
            verify_safe_mode(&state, d1.id, "  Billing prod ")
                .await
                .unwrap()
        );
        assert!(
            !verify_safe_mode(&state, d1.id, "billing prod")
                .await
                .unwrap()
        );
        assert!(!verify_safe_mode(&state, d1.id, "api-token").await.unwrap());
        assert!(!verify_safe_mode(&state, d1.id, "").await.unwrap());
        // With a passcode: the passcode only.
        set_passcode(&state, "4321");
        assert!(verify_safe_mode(&state, d1.id, "4321").await.unwrap());
        assert!(
            !verify_safe_mode(&state, d1.id, "Billing prod")
                .await
                .unwrap()
        );
        assert!(!verify_safe_mode(&state, d1.id, "api-token").await.unwrap());
        // A command's token on a non-core driver: the same.
        let iam = saved(&state, Driver::Clickhouse, PasswordMode::Command);
        assert!(verify_safe_mode(&state, iam.id, "4321").await.unwrap());
        assert!(!verify_safe_mode(&state, iam.id, "tok").await.unwrap());
        let err = verify_safe_mode(&state, Uuid::new_v4(), "4321")
            .await
            .unwrap_err();
        assert_eq!(err.code(), "NOT_FOUND");
    }

    #[tokio::test]
    async fn safe_mode_of_a_core_profile_is_unchanged_by_a_passcode() {
        let state = safety_state();
        set_passcode(&state, "4321");
        let pg = saved(&state, Driver::Postgres, PasswordMode::Keychain);
        state
            .secrets
            .set(pg.id, SecretKind::DbPassword, "pw")
            .await
            .unwrap();
        assert!(verify_safe_mode(&state, pg.id, "pw").await.unwrap());
        assert!(!verify_safe_mode(&state, pg.id, "4321").await.unwrap());
        assert!(
            !verify_safe_mode(&state, pg.id, "Billing prod")
                .await
                .unwrap()
        );
        // "Ask every time": the password typed at connect.
        let ask = saved(&state, Driver::Mysql, PasswordMode::Ask);
        assert!(!verify_safe_mode(&state, ask.id, "typed").await.unwrap());
        state.set_session_password(ask.id, Some("typed".into()));
        assert!(verify_safe_mode(&state, ask.id, "typed").await.unwrap());
        assert!(!verify_safe_mode(&state, ask.id, "4321").await.unwrap());
        // A core command profile has no password to check: the prompt asks for the name and
        // checks it itself, as before.
        let iam = saved(&state, Driver::Postgres, PasswordMode::Command);
        assert!(!verify_safe_mode(&state, iam.id, "4321").await.unwrap());
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
