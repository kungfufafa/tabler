//! App lock (Settings ▸ Security ▸ Passcode, Window ▸ Lock Tabler), Touch ID, and the security
//! details behind the status bar's popover and Copy as URL. Logic lives in `passcode`,
//! `biometrics` and `sqlgen::security`.

use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::biometrics::{self, EVENT_SYSTEM_LOCK};
use crate::error::AppError;
use crate::passcode;
use crate::secrets::SecretKind;
use crate::sqlgen::security;
use crate::state::AppState;
use crate::types::security::{PasscodeStatus, SessionDetails};

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::internal(format!("background task failed: {e}")))?
}

/// Shows the system Touch ID / account password dialog ("Tabler is trying to <reason>.") on a
/// background thread. True when the owner authenticated, false when cancelled.
#[tauri::command]
pub async fn authenticate_biometric(reason: String) -> Result<bool, AppError> {
    let ok = blocking(move || biometrics::authenticate(&reason)).await?;
    log::info!(
        "touch id: {}",
        if ok { "authenticated" } else { "cancelled" }
    );
    Ok(ok)
}

/// Whether a passcode is set and whether Touch ID (or the account password) can be asked.
#[tauri::command]
pub async fn passcode_status(state: State<'_, AppState>) -> Result<PasscodeStatus, AppError> {
    let enabled = passcode::load(&state.appdb)?.is_some();
    let biometrics = blocking(|| Ok(biometrics::available())).await?;
    Ok(PasscodeStatus {
        enabled,
        biometrics,
    })
}

/// Sets (`passcode`), changes or turns off (`passcode: None`) the app passcode. With one set,
/// `current` must match it (`AUTH_FAILED` otherwise). Only a salted PBKDF2 hash is stored.
#[tauri::command]
pub async fn set_passcode(
    state: State<'_, AppState>,
    current: Option<String>,
    passcode: Option<String>,
) -> Result<(), AppError> {
    let appdb = state.appdb.clone();
    let enabling = passcode.is_some();
    blocking(move || {
        passcode::change(
            &appdb,
            current.as_deref(),
            passcode.as_deref(),
            passcode::ITERATIONS,
        )
    })
    .await?;
    log::info!(
        "app passcode {}",
        if enabling { "set" } else { "turned off" }
    );
    Ok(())
}

/// Checks the passcode typed on the lock screen. False when none is set.
#[tauri::command]
pub async fn verify_passcode(
    state: State<'_, AppState>,
    passcode: String,
) -> Result<bool, AppError> {
    let appdb = state.appdb.clone();
    blocking(move || {
        Ok(passcode::load(&appdb)?.is_some_and(|stored| passcode::verify(&stored, &passcode)))
    })
    .await
}

/// Window ▸ Lock Tabler: every window locks (`EVENT_SYSTEM_LOCK`, reason `user`).
#[tauri::command]
pub async fn lock_app(app: AppHandle) -> Result<(), AppError> {
    app.emit(EVENT_SYSTEM_LOCK, "user")
        .map_err(|e| AppError::internal(e.to_string()))
}

/// The connection's DB password for "Copy as URL" (the renderer asked the safe-mode / Touch ID
/// checks first): the stored one, else the one typed at connect for "Ask every time". None when
/// the profile has neither.
#[tauri::command]
pub async fn reveal_connection_password(
    state: State<'_, AppState>,
    profile_id: Uuid,
) -> Result<Option<String>, AppError> {
    if state.appdb.get_profile(profile_id)?.is_none() {
        return Err(AppError::NotFound(format!("profile {profile_id}")));
    }
    let stored = state
        .secrets
        .get(profile_id, SecretKind::DbPassword)
        .await?;
    let password = stored.or_else(|| state.session_password(profile_id));
    log::info!("copy as url: password of profile {profile_id} revealed");
    Ok(password)
}

/// TLS state (version, cipher) and character set of an open session. A query the server does
/// not support leaves its fields empty; the error is returned only when nothing could be read.
#[tauri::command]
pub async fn session_details(
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<SessionDetails, AppError> {
    let session = state.session(connection_id).await?;
    let dialect = session.dialect();
    let mut details = SessionDetails::default();
    let mut first_error = None;
    if let Some(sql) = security::tls_sql(dialect) {
        match session.query(sql, &[]).await {
            Ok(rows) => security::read_tls(dialect, &rows, &mut details),
            Err(e) => {
                log::debug!("session details: no TLS state: {e}");
                first_error = Some(e);
            }
        }
    }
    match session.query(security::encoding_sql(dialect), &[]).await {
        Ok(rows) => details.encoding = security::read_encoding(&rows),
        Err(e) => {
            log::debug!("session details: no encoding: {e}");
            if let Some(first) = first_error {
                return Err(first);
            }
        }
    }
    Ok(details)
}
