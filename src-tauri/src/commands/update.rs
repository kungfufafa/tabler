//! Check for Updates…: the app menu's item, the quiet check at launch and the update dialog.

use tauri::ipc::Channel;
use tauri::{AppHandle, State};

use crate::commands::locations::restart_decision;
use crate::error::AppError;
use crate::shell::QuitState;
use crate::types::update::{UpdateCheck, UpdateProgress};
use crate::updater::{self, UpdaterState};

/// Looks for a newer build of this build's channel. `launch`: the quiet check a window makes when
/// it opens (only the first window of a run gets an answer other than `skipped`).
#[tauri::command]
pub async fn update_check(
    app: AppHandle,
    state: State<'_, UpdaterState>,
    launch: bool,
) -> Result<UpdateCheck, AppError> {
    updater::check(&app, &state, launch).await
}

/// Downloads the update the last check found and verifies its signature.
#[tauri::command]
pub async fn update_download(
    state: State<'_, UpdaterState>,
    on_progress: Channel<UpdateProgress>,
) -> Result<(), AppError> {
    updater::download(&state, move |progress| {
        let _ = on_progress.send(progress);
    })
    .await
}

/// Stops the download under way.
#[tauri::command]
pub fn update_cancel(state: State<'_, UpdaterState>) {
    updater::cancel(&state);
}

/// Installs the downloaded update and restarts into it. Refused while a window has unsaved work,
/// like `restart_app` (the renderer checks its own window first). The restart goes through the
/// exit events, so the cleanup of `RunEvent::Exit` runs; on Windows the installer ends the
/// process itself, after `updater::before_installer_exit`.
#[tauri::command]
pub async fn update_install(
    app: AppHandle,
    state: State<'_, UpdaterState>,
    quit: State<'_, QuitState>,
) -> Result<(), AppError> {
    restart_decision(quit.has_unsaved())?;
    updater::install(&state).await?;
    log::info!("restart requested (update installed)");
    // The exit guard lets this exit through: the renderer already cleared it.
    quit.confirm();
    app.request_restart();
    Ok(())
}
