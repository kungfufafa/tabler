//! File ▸ Open Recent: the renderer reports what it opened; the native submenu follows.

use tauri::{AppHandle, State};

use crate::error::AppError;
use crate::recent::{self, RecentEntry, RecentItems};
use crate::state::AppState;

/// The recently opened connections and `.sql` files, newest first.
#[tauri::command]
pub fn recent_list(state: State<'_, AppState>) -> RecentItems {
    recent::load(&state.appdb)
}

/// A connection was opened or an `.sql` file opened / saved: it moves to the top of Open Recent.
#[tauri::command]
pub fn recent_add(
    app: AppHandle,
    state: State<'_, AppState>,
    entry: RecentEntry,
) -> Result<RecentItems, AppError> {
    let items = recent::update(&state.appdb, |items| items.push(&entry))?;
    recent::refresh_menu(&app);
    Ok(items)
}

/// Open Recent ▸ Clear Menu.
#[tauri::command]
pub fn recent_clear(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    recent::update(&state.appdb, |items| {
        *items = RecentItems::default();
        Ok(())
    })?;
    recent::refresh_menu(&app);
    Ok(())
}

/// Rebuilds the submenu (a saved connection was renamed or deleted).
#[tauri::command]
pub fn recent_refresh_menu(app: AppHandle) {
    recent::refresh_menu(&app);
}
