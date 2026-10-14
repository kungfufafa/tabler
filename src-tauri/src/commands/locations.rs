//! Settings ▸ Locations: where the app database and the saved queries live, plus Show in Finder
//! for them and the restart a data folder change needs.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, State};

use crate::error::AppError;
use crate::locations::Locations;
use crate::shell::QuitState;
use crate::types::locations::{LocationChange, LocationInspection, LocationKind, LocationsInfo};

/// Reveals `path` in Finder (selected in its folder).
pub fn reveal(path: &Path) -> Result<(), AppError> {
    tauri_plugin_opener::reveal_item_in_dir(path)
        .map_err(|e| AppError::Io(format!("{}: {e}", path.display())))
}

#[tauri::command]
pub async fn get_locations(locations: State<'_, Locations>) -> Result<LocationsInfo, AppError> {
    Ok(locations.info())
}

/// Checks a folder picked in the native dialog before the change is confirmed.
#[tauri::command]
pub async fn inspect_location(
    locations: State<'_, Locations>,
    kind: LocationKind,
    path: String,
) -> Result<LocationInspection, AppError> {
    Ok(locations.inspect(kind, Path::new(&path)))
}

/// Chooses the connections & settings folder (`path` = null: the default); used after a restart.
#[tauri::command]
pub async fn set_data_location(
    locations: State<'_, Locations>,
    path: Option<String>,
    change: LocationChange,
) -> Result<LocationsInfo, AppError> {
    locations.set_data(path.as_deref().map(Path::new), change)
}

/// Chooses the queries folder (`path` = null: the default); applies at once.
#[tauri::command]
pub async fn set_queries_location(
    locations: State<'_, Locations>,
    path: Option<String>,
    change: LocationChange,
) -> Result<LocationsInfo, AppError> {
    locations.set_queries(path.as_deref().map(Path::new), change)
}

/// Show in Finder: the app database file (`data`, also the history store) or the queries folder.
#[tauri::command]
pub async fn reveal_location(
    locations: State<'_, Locations>,
    kind: LocationKind,
) -> Result<(), AppError> {
    let path: PathBuf = match kind {
        LocationKind::Data => locations.data_file(),
        LocationKind::Queries => {
            let dir = locations.queries_dir();
            std::fs::create_dir_all(&dir)?;
            dir
        }
    };
    reveal(&path)
}

/// Whether the app may restart now: not while any window holds unsaved work. The renderer checks
/// its own window before it calls `restart_app`; the other windows have their own tabs and
/// staging, which the quit handshake of a normal quit asks about and a restart must not skip.
fn restart_decision(any_window_unsaved: bool) -> Result<(), AppError> {
    if any_window_unsaved {
        return Err(AppError::InvalidInput(
            "Another window has unsaved work. Save or close it first, then restart.".into(),
        ));
    }
    Ok(())
}

/// Restarts the app (a new data folder is used from the next start). Refused while another window
/// has unsaved work. The restart goes through the exit events (`request_restart`), so the cleanup
/// of `RunEvent::Exit` runs: running exports and backups are stopped and their partial files
/// removed, and SQLite over SSH copies are uploaded one last time. (`restart` on the main thread,
/// where this command runs, would skip all of that.)
#[tauri::command]
pub fn restart_app(app: AppHandle, quit: State<'_, QuitState>) -> Result<(), AppError> {
    restart_decision(quit.has_unsaved())?;
    log::info!("restart requested (Settings ▸ Locations)");
    // The exit guard lets this exit through: the renderer already cleared it.
    quit.confirm();
    app.request_restart();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restart_is_refused_while_a_window_has_unsaved_work() {
        let err = restart_decision(true).unwrap_err();
        assert!(
            matches!(&err, AppError::InvalidInput(m) if m
                == "Another window has unsaved work. Save or close it first, then restart."),
            "{err:?}"
        );
        assert!(restart_decision(false).is_ok());
    }

    #[test]
    fn the_quit_state_decides() {
        // What `restart_app` reads: any window reporting unsaved work refuses the restart.
        let quit = QuitState::default();
        assert!(restart_decision(quit.has_unsaved()).is_ok());
        quit.set_unsaved(true);
        assert!(restart_decision(quit.has_unsaved()).is_err());
        quit.set_unsaved(false);
        assert!(restart_decision(quit.has_unsaved()).is_ok());
    }
}
