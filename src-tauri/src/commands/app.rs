use tauri::{AppHandle, State, Webview};
use uuid::Uuid;

use crate::drivers::registry;
use crate::error::AppError;
use crate::shell::QuitState;
use crate::types::app::AppInfo;
use crate::types::window::WindowHandoff;
use crate::windows::{self, WindowRegistry};

#[tauri::command]
pub fn app_info(app: AppHandle) -> AppInfo {
    log::debug!("renderer ready: app_info requested");
    AppInfo {
        version: app.package_info().version.to_string(),
        platform: std::env::consts::OS.to_string(),
        debug: cfg!(debug_assertions),
        drivers: Some(registry::built_drivers()),
        kinds: Some(registry::built_kinds()),
    }
}

/// Quits for real once every window agrees: the calling window has nothing unsaved left (or its
/// user chose to discard it). With other windows still asking their user, the quit waits for them
/// (each answers with `quit_app`, or closes once its work was discarded; `cancel_quit` calls it
/// off).
#[tauri::command]
pub fn quit_app(
    app: AppHandle,
    webview: Webview,
    quit: State<'_, QuitState>,
    registry: State<'_, WindowRegistry>,
) {
    if !registry.ready_to_quit(webview.label(), &windows::open_labels(&app)) {
        log::info!(
            "quit: window {} is ready, waiting for the others",
            webview.label()
        );
        return;
    }
    log::info!("quit confirmed by the renderer");
    quit.confirm();
    app.exit(0);
}

/// The user kept their work in the unsaved-changes dialog: a quit under way is off.
#[tauri::command]
pub fn cancel_quit(registry: State<'_, WindowRegistry>) {
    registry.cancel_quit();
}

/// A window reports whether any of its tabs holds unsaved work, so a Dock / system quit can be
/// answered right away (it asks while any window holds some); it also acknowledges a pending
/// `app:quit-requested`.
#[tauri::command]
pub fn set_unsaved(
    webview: Webview,
    unsaved: bool,
    quit: State<'_, QuitState>,
    registry: State<'_, WindowRegistry>,
) {
    quit.set_unsaved(registry.set_unsaved(webview.label(), unsaved));
}

/// File ▸ New Window (⌘N), "Open in New Window", Move Tab to New Window: opens a window like the
/// main one; `handoff` (what it opens with) waits for its renderer. Returns the new window's label.
/// Async: building a window from a synchronous command can deadlock (WebView2).
#[tauri::command]
pub async fn window_open(
    app: AppHandle,
    handoff: Option<WindowHandoff>,
) -> Result<String, AppError> {
    windows::open_window(&app, handoff).map_err(|e| AppError::Internal(e.to_string()))
}

/// What this window was opened with (once; `None` for the main window and plain new windows).
#[tauri::command]
pub fn window_take_handoff(
    webview: Webview,
    registry: State<'_, WindowRegistry>,
) -> Option<WindowHandoff> {
    registry.take_handoff(webview.label())
}

/// This window's open connections (closed with the window).
#[tauri::command]
pub fn window_set_connections(
    webview: Webview,
    connection_ids: Vec<Uuid>,
    registry: State<'_, WindowRegistry>,
) {
    registry.set_connections(webview.label(), connection_ids);
}

/// This window's label (`main` for the first window).
#[tauri::command]
pub fn window_label(webview: Webview) -> String {
    webview.label().to_string()
}

/// Whether this window keeps the session saved: the main window until it closes, then the oldest
/// window left (which is also told with `window:session-owner`).
#[tauri::command]
pub fn window_is_session_owner(
    app: AppHandle,
    webview: Webview,
    registry: State<'_, WindowRegistry>,
) -> bool {
    registry.is_session_owner(webview.label(), &windows::open_labels(&app))
}
