//! Multi-window bookkeeping (TablePlus: File ▸ New Window ⌘N, "Open in New Window", Window ▸ Move
//! Tab to New Window). Every window is a webview with its own renderer stores; the backend state
//! (open connections) is shared, but each window owns the connections its renderer opened: they are
//! closed when the window goes away.
//!
//! The Settings window ([`SETTINGS_WINDOW`], TablePlus's Preferences / Settings window) is not one
//! of those: it holds no connection, no session and no unsaved work, so it is left out of the
//! session's owner, connection routing, the last-window rules and the quit handshake
//! ([`is_aux_window`]), and menu items reach it only when it can do something with them
//! ([`menu_route`]).
//!
//! [`WindowRegistry`] is plain state (unit-tested without Tauri): the handoffs parked for new
//! windows, the unsaved-work flag of every window (a quit asks while any window holds unsaved
//! work), the windows ready to quit, and which connections each window owns. The Tauri glue
//! (opening a window, routing menu events to the focused one, cleaning up a closed one) is below it.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, EventTarget, Manager, Runtime, WebviewWindowBuilder, Window, WindowEvent,
};
use uuid::Uuid;

use crate::types::window::WindowHandoff;

/// The window the app starts with (tauri.conf.json).
pub const MAIN_WINDOW: &str = "main";
/// Label prefix of the windows opened later: `w-2`, `w-3`… (the window-state plugin keeps each
/// one's size and position under its label, so window N reopens where window N was).
pub const WINDOW_PREFIX: &str = "w-";
/// How far a new window is offset from the one it was opened from (logical pixels).
const CASCADE_OFFSET: f64 = 28.0;
/// macOS and Windows: the Welcome window a workspace window asks for (the connection list in a
/// window of its own, like TablePlus). Built at its size and hidden until its renderer has laid
/// the list out; the window-state plugin leaves it alone (it is always the same size, centred).
pub const WELCOME_WINDOW: &str = "welcome";
/// The Welcome window's size in logical pixels (`WELCOME_SIZE` in `src/stores/windowShape.ts`):
/// TablePlus for Windows's client area is 746×529, the macOS one 760×480.
#[cfg(windows)]
const WELCOME_SIZE: (f64, f64) = (746.0, 529.0);
#[cfg(not(windows))]
const WELCOME_SIZE: (f64, f64) = (760.0, 480.0);
/// The Settings window (TablePlus: "The Settings window holds the settings"): one at most, built
/// on demand at its own size and centred on the window it was opened from.
pub const SETTINGS_WINDOW: &str = "settings";
/// Its client area in logical pixels: TablePlus for Windows's Settings window is 888×654.
const SETTINGS_SIZE: (f64, f64) = (888.0, 654.0);
/// Its smallest size: the section list and a section's rows still fit side by side.
const SETTINGS_MIN_SIZE: (f64, f64) = (720.0, 480.0);
/// Sent to the Settings window: show this section (a `SettingsSection` id of the renderer).
pub const EVENT_SETTINGS_SECTION: &str = "settings:section";
/// How long an exit waits for the Settings window to write what it still has to save (its
/// preferences and themes are saved after a short debounce) and go.
const SETTINGS_FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);
/// How long a window built hidden may stay so: its renderer shows it once it is laid out (the
/// Welcome window; every window on Windows and macOS, `tauri.windows.conf.json` and
/// `tauri.macos.conf.json`), and a renderer that never gets there must not leave a window nobody
/// can see.
const HIDDEN_REVEAL: std::time::Duration = std::time::Duration::from_millis(2500);

/// What the window-state plugin keeps of a window. Not the decorations: the app sets those itself
/// (off on Windows, `own_title_bar`), and a saved "undecorated" would leave a build without its
/// own window buttons with none at all. Not the visibility: a window built hidden is shown by its
/// renderer once it has its shape (`applyShape`), so it never shows half laid out.
fn window_state_flags() -> tauri_plugin_window_state::StateFlags {
    use tauri_plugin_window_state::StateFlags;
    StateFlags::all() - StateFlags::DECORATIONS - StateFlags::VISIBLE
}

/// The window-state plugin. Windows: the main window's frame is restored by
/// [`restore_main_frame`] instead, without maximizing it.
pub fn window_state_plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    let builder = tauri_plugin_window_state::Builder::new()
        .with_state_flags(window_state_flags())
        // The Welcome and Settings windows always open at the same size, centred (Settings on the
        // window it was opened from), like TablePlus's.
        .with_denylist(&[WELCOME_WINDOW, SETTINGS_WINDOW]);
    #[cfg(windows)]
    let builder = builder.skip_initial_state(MAIN_WINDOW);
    builder.build()
}

/// Windows: the main window's saved size and position, not whether it was maximized. It is built
/// hidden and its renderer decides what it shows: the small Welcome window, or the workspace
/// window, maximized when it was (`rememberMaximized` in `src/stores/windowShape.ts`). Un-maximizing
/// a hidden window shows it, so it is never maximized before the renderer asks.
pub fn restore_main_frame<R: Runtime>(window: &tauri::WebviewWindow<R>) -> tauri::Result<()> {
    #[cfg(windows)]
    {
        use tauri_plugin_window_state::{StateFlags, WindowExt};
        window.restore_state(window_state_flags() - StateFlags::MAXIMIZED)
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        Ok(())
    }
}

/// Shows `window` when it is still hidden after [`HIDDEN_REVEAL`] (its renderer did not).
pub fn reveal_if_still_hidden<R: Runtime>(window: tauri::WebviewWindow<R>) {
    if window.is_visible().unwrap_or(true) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(HIDDEN_REVEAL).await;
        if !window.is_visible().unwrap_or(true) {
            log::warn!("window {} was still hidden: showing it", window.label());
            let _ = raise(&window);
        }
    });
}

/// Sent to a workspace window: open this profile here (picked in the Welcome window).
pub const EVENT_OPEN_PROFILE: &str = "window:open-profile";

/// Whether `label` is a window of its own kind beside the app's windows (the Settings window): it
/// runs no workspace, so it owns no connection or session, never holds unsaved work, does not keep
/// the app open and is not waited for by a quit.
pub fn is_aux_window(label: &str) -> bool {
    label == SETTINGS_WINDOW
}

/// How many of `labels` are app windows (workspace, Welcome): what the last-window rules count.
pub fn app_window_count(labels: &[String]) -> usize {
    labels.iter().filter(|l| !is_aux_window(l)).count()
}

/// Menu actions the Settings window answers itself when it has the focus: ⌘, (it is already
/// there), ⌘W (closes it), Lock Tabler and Check for Updates… (its own update dialog). Every
/// other menu item acts on a workspace window.
pub fn settings_handles(action: &str) -> bool {
    matches!(
        action,
        "settings.open" | "tab.close" | "app.lock" | "app.check-updates"
    )
}

/// Where a menu action goes.
#[derive(Debug, PartialEq, Eq)]
pub struct MenuRoute {
    pub label: String,
    /// It was chosen while the Settings window had the focus: the workspace window that runs it
    /// comes to the front, so what it opens is not hidden behind Settings.
    pub raise: bool,
}

/// The window a menu action goes to: the focused Settings window when it handles the action
/// ([`settings_handles`]), else the workspace window menu items act on (`workspace`: the focused
/// app window or the one focused last). `None`: no window to send it to.
pub fn menu_route(
    focused: Option<&str>,
    action: &str,
    workspace: impl FnOnce() -> Option<String>,
) -> Option<MenuRoute> {
    let settings_focused = focused.is_some_and(is_aux_window);
    if settings_focused && settings_handles(action) {
        return Some(MenuRoute {
            label: SETTINGS_WINDOW.to_string(),
            raise: false,
        });
    }
    workspace().map(|label| MenuRoute {
        label,
        raise: settings_focused,
    })
}

/// The top-left corner (logical pixels) that centres a window of `size` on the window `on`
/// (x, y, width, height).
pub fn centered_on(on: (f64, f64, f64, f64), size: (f64, f64)) -> (f64, f64) {
    let (x, y, width, height) = on;
    (x + (width - size.0) / 2.0, y + (height - size.1) / 2.0)
}

/// The lowest free `w-N` label (N ≥ 2: the main window is the first).
pub fn next_window_label<'a>(taken: impl IntoIterator<Item = &'a str>) -> String {
    let taken: HashSet<&str> = taken.into_iter().collect();
    (2..)
        .map(|n| format!("{WINDOW_PREFIX}{n}"))
        .find(|label| !taken.contains(label.as_str()))
        .unwrap_or_else(|| format!("{WINDOW_PREFIX}{}", Uuid::new_v4().simple()))
}

#[derive(Debug, Default)]
struct Inner {
    /// Parked for windows that have not taken them yet (their label is reserved meanwhile).
    handoffs: HashMap<String, WindowHandoff>,
    /// Labels handed out whose window may not be listed by Tauri yet.
    reserved: HashSet<String>,
    /// What each window last reported (`set_unsaved`).
    unsaved: HashMap<String, bool>,
    /// Connections each window opened (`window_set_connections`).
    connections: HashMap<String, HashSet<Uuid>>,
    /// A quit is under way: windows that answered it with nothing left to keep (`quit_app`).
    quit_ready: HashSet<String>,
    quitting: bool,
    /// The open windows that had the focus, the latest one last.
    focused: Vec<String>,
    /// Windows built after the main one, oldest first (labels are reused, so their numbers do not
    /// tell their age).
    born: Vec<String>,
    /// The window that keeps the session saved (`None`: the main window).
    session_owner: Option<String>,
    /// macOS and Windows: the last workspace window closed and the Welcome window is being built
    /// in its place([`reopens_welcome`]); the runtime's last-window exit is held meanwhile.
    welcome_pending: bool,
    /// The section the Settings window was last asked to show, for its renderer to take when it
    /// starts (the `settings:section` event may come before it listens).
    settings_section: Option<String>,
    /// The Settings window is being built (`open_settings_window`): a second Settings… meanwhile
    /// (⌘, pressed twice or held) only asks it for its section instead of building another.
    settings_building: bool,
}

impl Inner {
    fn session_owner(&self) -> &str {
        self.session_owner.as_deref().unwrap_or(MAIN_WINDOW)
    }

    /// The oldest of `candidates`: the main window, else the one built first. Never the Settings
    /// window.
    fn oldest(&self, candidates: &[String]) -> Option<String> {
        if candidates.iter().any(|l| l == MAIN_WINDOW) {
            return Some(MAIN_WINDOW.to_string());
        }
        self.born
            .iter()
            .find(|l| candidates.contains(l))
            .or_else(|| candidates.iter().filter(|l| !is_aux_window(l)).min())
            .filter(|l| !is_aux_window(l))
            .cloned()
    }
}

/// What closing a window leaves to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct WindowGone {
    /// Its connections, to disconnect.
    pub connections: Vec<Uuid>,
    /// Whether any window left still holds unsaved work.
    pub unsaved: bool,
    /// A quit was under way and every window left is ready for it: exit now.
    pub quit_now: bool,
    /// It kept the session saved: this window does from now on.
    pub promoted: Option<String>,
}

/// Managed state: see the module docs.
#[derive(Debug, Default)]
pub struct WindowRegistry {
    inner: Mutex<Inner>,
}

impl WindowRegistry {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Reserves the label of a new window (`open` lists the windows Tauri has now) and parks its
    /// handoff under it.
    pub fn reserve(&self, open: &[String], handoff: Option<WindowHandoff>) -> String {
        let mut inner = self.lock();
        let label = next_window_label(
            open.iter()
                .map(String::as_str)
                .chain(inner.reserved.iter().map(String::as_str)),
        );
        inner.reserved.insert(label.clone());
        if let Some(handoff) = handoff {
            inner.handoffs.insert(label.clone(), handoff);
        }
        label
    }

    /// The window was built (or failed to be): its label is Tauri's from now on.
    pub fn release(&self, label: &str) {
        self.lock().reserved.remove(label);
    }

    /// The window was built: it is the youngest one.
    pub fn born(&self, label: &str) {
        let mut inner = self.lock();
        inner.born.retain(|l| l != label);
        inner.born.push(label.to_string());
    }

    /// Whether `label` keeps the session saved (`open`: the windows Tauri lists now). The main
    /// window does until it closes; then the oldest window left (see [`WindowRegistry::forget`]).
    /// When the owner is gone without a successor (every window closed), the oldest open window
    /// takes over.
    pub fn is_session_owner(&self, label: &str, open: &[String]) -> bool {
        let mut inner = self.lock();
        if !open.iter().any(|l| l == inner.session_owner())
            && let Some(oldest) = inner.oldest(open)
        {
            inner.session_owner = Some(oldest);
        }
        inner.session_owner() == label
    }

    /// The handoff parked for `label`, once.
    pub fn take_handoff(&self, label: &str) -> Option<WindowHandoff> {
        self.lock().handoffs.remove(label)
    }

    /// Records what a window reported; returns whether any window holds unsaved work.
    pub fn set_unsaved(&self, label: &str, unsaved: bool) -> bool {
        let mut inner = self.lock();
        inner.unsaved.insert(label.to_string(), unsaved);
        inner.unsaved.values().any(|u| *u)
    }

    pub fn any_unsaved(&self) -> bool {
        self.lock().unsaved.values().any(|u| *u)
    }

    /// The connections a window owns now (replaces what it reported before). The Settings window
    /// owns none.
    pub fn set_connections(&self, label: &str, ids: impl IntoIterator<Item = Uuid>) {
        let ids: HashSet<Uuid> = ids.into_iter().collect();
        let mut inner = self.lock();
        if ids.is_empty() || is_aux_window(label) {
            inner.connections.remove(label);
        } else {
            inner.connections.insert(label.to_string(), ids);
        }
    }

    /// The window that owns a connection.
    pub fn owner_of(&self, connection_id: Uuid) -> Option<String> {
        let inner = self.lock();
        inner
            .connections
            .iter()
            .find(|(_, ids)| ids.contains(&connection_id))
            .map(|(label, _)| label.clone())
    }

    /// A window answered a quit with nothing left to keep (`quit_app`): the quit is under way.
    /// True when every app window in `open` (the windows Tauri still lists) is ready, so the app
    /// may exit; otherwise the others are still asking their user. The Settings window has nothing
    /// to keep, so the quit never waits for its answer (it answers too, which matters when it is the
    /// only window left); the exit closes it first so it writes its pending saves
    /// ([`exit_after_settings`]).
    pub fn ready_to_quit(&self, label: &str, open: &[String]) -> bool {
        let mut inner = self.lock();
        inner.quitting = true;
        inner.quit_ready.insert(label.to_string());
        open.iter()
            .filter(|l| !is_aux_window(l))
            .all(|l| inner.quit_ready.contains(l))
    }

    /// The user kept their work in one of the windows: the quit is off.
    pub fn cancel_quit(&self) {
        let mut inner = self.lock();
        inner.quitting = false;
        inner.quit_ready.clear();
    }

    pub fn is_quitting(&self) -> bool {
        self.lock().quitting
    }

    /// See `Inner::welcome_pending`.
    pub fn set_welcome_pending(&self, pending: bool) {
        self.lock().welcome_pending = pending;
    }

    pub fn welcome_pending(&self) -> bool {
        self.lock().welcome_pending
    }

    /// Parks the section the Settings window is asked to show (see `Inner::settings_section`).
    pub fn park_settings_section(&self, section: Option<String>) {
        self.lock().settings_section = section;
    }

    /// The section parked for the Settings window, once.
    pub fn take_settings_section(&self) -> Option<String> {
        self.lock().settings_section.take()
    }

    /// Starts building the Settings window. False when it is already being built: the caller then
    /// only asks it for its section.
    pub fn begin_settings_build(&self) -> bool {
        !std::mem::replace(&mut self.lock().settings_building, true)
    }

    /// The Settings window was built (or could not be).
    pub fn end_settings_build(&self) {
        self.lock().settings_building = false;
    }

    /// Windows whose label is reserved and that Tauri does not list yet (being built).
    pub fn building(&self) -> usize {
        self.lock().reserved.len()
    }

    /// A window went away (`remaining`: the windows left). Forgets it and says what is left to do.
    pub fn forget(&self, label: &str, remaining: &[String]) -> WindowGone {
        let mut inner = self.lock();
        inner.handoffs.remove(label);
        inner.reserved.remove(label);
        inner.unsaved.remove(label);
        inner.quit_ready.remove(label);
        inner.focused.retain(|l| l != label);
        let mut connections: Vec<Uuid> = inner
            .connections
            .remove(label)
            .map(|ids| ids.into_iter().collect())
            .unwrap_or_default();
        connections.sort();
        inner.born.retain(|l| l != label);
        if is_aux_window(label) {
            inner.settings_section = None;
        }
        // The app windows left: the Settings window neither keeps the session nor holds a quit up.
        let left: Vec<String> = remaining
            .iter()
            .filter(|l| *l != label && !is_aux_window(l))
            .cloned()
            .collect();
        let promoted = if inner.session_owner() == label {
            let next = inner.oldest(&left);
            inner.session_owner = next.clone();
            next
        } else {
            None
        };
        WindowGone {
            connections,
            unsaved: inner.unsaved.values().any(|u| *u),
            quit_now: inner.quitting && left.iter().all(|l| inner.quit_ready.contains(l)),
            promoted,
        }
    }

    /// An app window got the focus. The Settings window is not counted: menu items and picked
    /// connections go to the workspace window focused last.
    pub fn set_focused(&self, label: &str) {
        if is_aux_window(label) {
            return;
        }
        let mut inner = self.lock();
        inner.focused.retain(|l| l != label);
        inner.focused.push(label.to_string());
    }

    pub fn last_focused(&self) -> Option<String> {
        self.lock().focused.last().cloned()
    }

    /// Whether a window holds an open connection (it is a workspace window).
    pub fn has_connections(&self, label: &str) -> bool {
        self.lock().connections.contains_key(label)
    }

    /// The workspace window a connection picked in the window `from` opens in: of the windows in
    /// `open` that hold a connection, the one that had the focus last, else the oldest. `None`
    /// when no other window holds one.
    pub fn workspace_window(&self, from: &str, open: &[String]) -> Option<String> {
        let inner = self.lock();
        let candidates: Vec<String> = open
            .iter()
            .filter(|l| *l != from && inner.connections.contains_key(*l))
            .cloned()
            .collect();
        inner
            .focused
            .iter()
            .rev()
            .find(|l| candidates.contains(l))
            .cloned()
            .or_else(|| inner.oldest(&candidates))
    }
}

/// Whether the app ends now that a window went away, leaving `left` app windows Tauri lists (the
/// Settings window is not counted, [`app_window_count`]) and `building` ones on their way. On Windows and Linux an app without a window cannot be reached
/// (no Dock, no menu bar, no tray): it ends even when the runtime itself would keep running (a
/// window it holds that Tauri does not list, an exit request kept waiting), which left a Tabler
/// process without any window. macOS keeps its behaviour (the runtime's own last-window exit,
/// held while the Welcome window takes a closed workspace window's place, [`reopens_welcome`]).
pub fn exits_with_last_window(macos: bool, left: usize, building: usize) -> bool {
    !macos && left == 0 && building == 0
}

/// Whether the app ends now that a window went away ([`on_window_event`], after
/// [`reopens_welcome`] said no): no app window is left (`app_windows_left`, the Settings window
/// not counted), none is being built and no Welcome window is on its way (`welcome_pending`).
/// Windows and Linux: always then ([`exits_with_last_window`]). macOS: also while the Settings
/// window is still open (`aux_left`): the runtime's own last-window exit does not happen then,
/// and Settings alone would leave a menu bar whose items have no window to act on (closing the
/// Welcome window, or a last workspace window without connections, ended the app before Settings
/// was a window). Without it, the runtime's own exit ends the app as before.
pub fn exits_now(
    macos: bool,
    welcome_pending: bool,
    app_windows_left: usize,
    building: usize,
    aux_left: usize,
) -> bool {
    let none_left = app_windows_left == 0 && building == 0;
    !welcome_pending
        && (exits_with_last_window(macos, app_windows_left, building)
            || (macos && none_left && aux_left > 0))
}

/// macOS and Windows, like TablePlus: closing the last workspace window (one that held
/// connections) with its close button, ⌘W / Alt+F4 or the taskbar shows the Welcome window instead
/// of ending the app. A quit (File ▸ Quit, ⌘Q, every window asked) still ends it, and so does
/// closing the Welcome window itself. Linux has no Welcome window (`shapedWindows`).
pub fn reopens_welcome(
    shaped: bool,
    quitting: bool,
    held_connections: bool,
    left: usize,
    building: usize,
) -> bool {
    shaped && !quitting && held_connections && left == 0 && building == 0
}

/// Builds the Welcome window in place of the last workspace window, which just closed. Its label
/// may be the Welcome window's own, still held a moment while it goes away. Ends the app when the
/// Welcome window cannot be shown: a process without a window cannot be reached.
fn reopen_welcome<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        for _ in 0..40 {
            if app.get_webview_window(WELCOME_WINDOW).is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let shown = show_welcome_window(&app);
        if let Some(registry) = app.try_state::<WindowRegistry>() {
            registry.set_welcome_pending(false);
        }
        match shown {
            Ok(Some(_)) => {}
            Ok(None) => {
                log::warn!("the Welcome window could not be shown: exiting");
                app.exit(0);
            }
            Err(e) => {
                log::warn!("could not open the Welcome window: {e}; exiting");
                app.exit(0);
            }
        }
    });
}

// --- Tauri glue --------------------------------------------------------------------------------

/// Labels of the open webview windows.
pub fn open_labels<R: Runtime>(app: &AppHandle<R>) -> Vec<String> {
    let mut labels: Vec<String> = app.webview_windows().into_keys().collect();
    labels.sort();
    labels
}

/// The app window menu items act on: the focused one, else the one focused last, else the main
/// window (or any). Never the Settings window (see [`emit_menu_action`] for the items it handles).
pub fn target_label<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    let windows = app.webview_windows();
    if let Some((label, _)) = windows
        .iter()
        .find(|(l, w)| !is_aux_window(l) && w.is_focused().unwrap_or(false))
    {
        return Some(label.clone());
    }
    let last = app
        .try_state::<WindowRegistry>()
        .and_then(|r| r.last_focused())
        .filter(|l| windows.contains_key(l));
    last.or_else(|| {
        windows
            .contains_key(MAIN_WINDOW)
            .then(|| MAIN_WINDOW.to_string())
    })
    .or_else(|| windows.keys().filter(|l| !is_aux_window(l)).min().cloned())
}

/// The window that has the focus now, if any.
fn focused_label<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    app.webview_windows()
        .into_iter()
        .find(|(_, w)| w.is_focused().unwrap_or(false))
        .map(|(label, _)| label)
}

/// Sends a native menu item's action (`menu:action`) to the window it acts on ([`menu_route`]):
/// the focused Settings window for the few it handles, else the workspace window, which comes to
/// the front when Settings had the focus.
pub fn emit_menu_action<R: Runtime>(app: &AppHandle<R>, event: &str, action: &str) {
    let focused = focused_label(app);
    let route = menu_route(focused.as_deref(), action, || target_label(app));
    let result = match &route {
        Some(route) => app.emit_to(
            EventTarget::webview_window(route.label.as_str()),
            event,
            action,
        ),
        None => app.emit(event, action),
    };
    if let Err(e) = result {
        log::warn!("could not emit {event}: {e}");
    }
    if let Some(route) = route.filter(|r| r.raise)
        && let Some(window) = app.get_webview_window(&route.label)
        && let Err(e) = raise(&window)
    {
        log::warn!("could not bring window {} to the front: {e}", route.label);
    }
}

/// Emits to the window menu items act on (`target_label`): the renderer listens for it on its own
/// window (`listenWindowEvent`), so the other windows do not run the action too.
pub fn emit_to_target<R: Runtime, S: Serialize + Clone>(
    app: &AppHandle<R>,
    event: &str,
    payload: S,
) {
    emit_to_window(app, None, event, payload);
}

/// The window an event meant for `origin` goes to: `origin` while it is open, else `fallback`
/// (the window menu items act on).
pub fn route_label(
    origin: Option<&str>,
    open: &[String],
    fallback: impl FnOnce() -> Option<String>,
) -> Option<String> {
    origin
        .filter(|o| open.iter().any(|l| l == o))
        .map(str::to_string)
        .or_else(fallback)
}

/// Emits to the window `origin` (a window label, e.g. the window that started a connect) while it
/// is open, else to the window menu items act on (`target_label`). The renderer listens for it on
/// its own window (`listenWindowEvent`).
pub fn emit_to_window<R: Runtime, S: Serialize + Clone>(
    app: &AppHandle<R>,
    origin: Option<&str>,
    event: &str,
    payload: S,
) {
    let label = route_label(origin, &open_labels(app), || target_label(app));
    let result = match label {
        Some(label) => app.emit_to(EventTarget::webview_window(label), event, payload),
        None => app.emit(event, payload),
    };
    if let Err(e) = result {
        log::warn!("could not emit {event}: {e}");
    }
}

/// Sent to the window that keeps the session saved from now on (the one that did closed).
pub const EVENT_SESSION_OWNER: &str = "window:session-owner";

/// The main window's configuration (tauri.conf.json): what every other window starts from.
fn main_window_config<R: Runtime>(app: &AppHandle<R>) -> tauri::utils::config::WindowConfig {
    app.config()
        .app
        .windows
        .iter()
        .find(|w| w.label == MAIN_WINDOW)
        .cloned()
        .unwrap_or_default()
}

/// Brings a window to the front.
fn raise<R: Runtime>(window: &tauri::WebviewWindow<R>) -> tauri::Result<()> {
    if window.is_minimized()? {
        window.unminimize()?;
    }
    window.show()?;
    window.set_focus()
}

/// macOS and Windows: shows the Welcome window ([`WELCOME_WINDOW`]), building it when it is not
/// open. `None` when the window of that label holds a connection (the list in it was used to
/// connect with no workspace window open, so it became one): the caller opens a plain window
/// instead.
fn show_welcome_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Option<String>> {
    let registry = app.state::<WindowRegistry>();
    if let Some(open) = app.get_webview_window(WELCOME_WINDOW) {
        if registry.has_connections(WELCOME_WINDOW) {
            return Ok(None);
        }
        raise(&open)?;
        return Ok(Some(WELCOME_WINDOW.to_string()));
    }
    let mut config = main_window_config(app);
    config.label = WELCOME_WINDOW.to_string();
    (config.width, config.height) = WELCOME_SIZE;
    (config.min_width, config.min_height) = (None, None);
    (config.x, config.y) = (None, None);
    config.center = true;
    config.resizable = false;
    config.maximizable = false;
    // Shown by its renderer once the list is laid out (`applyShape`), so it never shows empty.
    config.visible = false;
    let window = WebviewWindowBuilder::from_config(app, &config).and_then(|b| b.build())?;
    #[cfg(windows)]
    if let Err(e) = own_title_bar(&window) {
        log::warn!("could not set up the Welcome window: {e}");
    }
    registry.born(WELCOME_WINDOW);
    log::info!("opened the Welcome window");
    reveal_if_still_hidden(window);
    Ok(Some(WELCOME_WINDOW.to_string()))
}

/// Settings… (⌘,): brings the one Settings window ([`SETTINGS_WINDOW`]) to the front, building it
/// when it is not open, and shows `section` in it (none: the section it shows, or showed last).
/// A new window is centred on the window it was opened from (`from`, else the window menu items
/// act on), at TablePlus's size; it is hidden until its renderer has laid the section out. Windows:
/// no native title bar (the renderer draws its close button and drag area, like TablePlus);
/// macOS: a standard title bar titled "Settings"; Linux: the window manager's.
pub fn open_settings_window<R: Runtime>(
    app: &AppHandle<R>,
    from: Option<&str>,
    section: Option<String>,
) -> tauri::Result<()> {
    let registry = app.state::<WindowRegistry>();
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW) {
        ask_settings_section(app, &registry, section);
        return raise(&window);
    }
    if !registry.begin_settings_build() {
        // An earlier Settings… is building it (⌘, twice, or held): it shows this section instead.
        ask_settings_section(app, &registry, section);
        return Ok(());
    }
    registry.park_settings_section(section);
    let built = build_settings_window(app, from);
    registry.end_settings_build();
    if built.is_err() && app.get_webview_window(SETTINGS_WINDOW).is_none() {
        registry.take_settings_section();
    }
    built
}

/// Asks the Settings window, open or being built, to show `section` (none: nothing to ask).
fn ask_settings_section<R: Runtime>(
    app: &AppHandle<R>,
    registry: &WindowRegistry,
    section: Option<String>,
) {
    let Some(section) = section else {
        return;
    };
    // Parked too: a renderer that is still starting takes it instead of the event.
    registry.park_settings_section(Some(section.clone()));
    let target = EventTarget::webview_window(SETTINGS_WINDOW);
    if let Err(e) = app.emit_to(target, EVENT_SETTINGS_SECTION, section) {
        log::warn!("could not emit {EVENT_SETTINGS_SECTION}: {e}");
    }
}

/// Builds the Settings window (see [`open_settings_window`]).
fn build_settings_window<R: Runtime>(app: &AppHandle<R>, from: Option<&str>) -> tauri::Result<()> {
    let mut config = main_window_config(app);
    config.label = SETTINGS_WINDOW.to_string();
    config.title = "Settings".to_string();
    (config.width, config.height) = SETTINGS_SIZE;
    (config.min_width, config.min_height) = (Some(SETTINGS_MIN_SIZE.0), Some(SETTINGS_MIN_SIZE.1));
    (config.max_width, config.max_height) = (None, None);
    config.resizable = true;
    config.maximizable = false;
    config.maximized = false;
    config.fullscreen = false;
    // TablePlus for Windows's Settings window has a close button only.
    config.minimizable = !cfg!(windows);
    #[cfg(target_os = "macos")]
    {
        config.title_bar_style = tauri::TitleBarStyle::Visible;
        config.hidden_title = false;
        config.traffic_light_position = None;
    }
    let from = from
        .filter(|l| !is_aux_window(l))
        .map(str::to_string)
        .or_else(|| target_label(app))
        .and_then(|l| app.get_webview_window(&l));
    (config.x, config.y) = (None, None);
    config.center = true;
    if let Some(from) = from
        && let (Ok(pos), Ok(size), Ok(scale)) = (
            from.outer_position(),
            from.outer_size(),
            from.scale_factor(),
        )
        && scale > 0.0
    {
        let (x, y) = centered_on(
            (
                f64::from(pos.x) / scale,
                f64::from(pos.y) / scale,
                f64::from(size.width) / scale,
                f64::from(size.height) / scale,
            ),
            SETTINGS_SIZE,
        );
        config.center = false;
        (config.x, config.y) = (Some(x), Some(y));
    }
    // Shown by its renderer once the section is laid out, so it never shows empty.
    config.visible = false;
    let window = WebviewWindowBuilder::from_config(app, &config)?.build()?;
    #[cfg(windows)]
    if let Err(e) = own_title_bar(&window).and_then(|()| fit_to_work_area(&window, false)) {
        log::warn!("could not set up the Settings window: {e}");
    }
    // Linux: the app menu is every window's; Settings has none (TablePlus's has no menu bar).
    #[cfg(target_os = "linux")]
    if let Err(e) = window.hide_menu() {
        log::warn!("could not hide the Settings window's menu: {e}");
    }
    log::info!("opened the Settings window");
    reveal_if_still_hidden(window);
    Ok(())
}

/// A connection picked in the Welcome window `from` opens in a workspace window when one is open
/// (TablePlus: a new workspace in the same window): that window is told to open the profile and
/// comes to the front. Returns its label; `None` when no window holds a connection (the Welcome
/// window opens it itself).
pub fn route_profile<R: Runtime>(
    app: &AppHandle<R>,
    from: &str,
    profile_id: Uuid,
) -> Option<String> {
    let registry = app.try_state::<WindowRegistry>()?;
    let label = registry.workspace_window(from, &open_labels(app))?;
    let window = app.get_webview_window(&label)?;
    let target = EventTarget::webview_window(label.as_str());
    if let Err(e) = app.emit_to(target, EVENT_OPEN_PROFILE, profile_id) {
        log::warn!("could not emit {EVENT_OPEN_PROFILE}: {e}");
        return None;
    }
    if let Err(e) = raise(&window) {
        log::warn!("could not bring window {label} to the front: {e}");
    }
    Some(label)
}

/// Opens a new window like the main one (cascaded from the focused window), with `handoff` parked
/// for its renderer. Returns its label. `welcome` (macOS and Windows, no handoff): the window is
/// asked for to show the connection list, so the one Welcome window comes up instead of another
/// plain window.
pub fn open_window<R: Runtime>(
    app: &AppHandle<R>,
    handoff: Option<WindowHandoff>,
    welcome: bool,
) -> tauri::Result<String> {
    if welcome
        && handoff.is_none()
        && cfg!(any(target_os = "macos", windows))
        && let Some(label) = show_welcome_window(app)?
    {
        return Ok(label);
    }
    let registry = app.state::<WindowRegistry>();
    let label = registry.reserve(&open_labels(app), handoff);
    let mut config = main_window_config(app);
    config.label = label.clone();
    // Cascaded from the window it was opened from (the window-state plugin moves it to where
    // this window number was last, if it was open before).
    let from = target_label(app).and_then(|l| app.get_webview_window(&l));
    if let Some(from) = from
        && let (Ok(pos), Ok(scale)) = (from.outer_position(), from.scale_factor())
    {
        config.center = false;
        config.x = Some(f64::from(pos.x) / scale + CASCADE_OFFSET);
        config.y = Some(f64::from(pos.y) / scale + CASCADE_OFFSET);
    }
    let built = WebviewWindowBuilder::from_config(app, &config).and_then(|b| b.build());
    registry.release(&label);
    let window = match built {
        Ok(window) => window,
        Err(e) => {
            registry.take_handoff(&label);
            return Err(e);
        }
    };
    #[cfg(windows)]
    if let Err(e) = own_title_bar(&window).and_then(|()| fit_to_work_area(&window, true)) {
        log::warn!("could not set up window {label}: {e}");
    }
    // Windows and macOS: built hidden like the main window; its renderer shows it.
    reveal_if_still_hidden(window);
    registry.born(&label);
    log::info!("opened window {label}");
    Ok(label)
}

/// A window's outer frame or a monitor's work area, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// How a window that does not sit inside the work area is brought back into it.
#[derive(Debug, PartialEq, Eq)]
pub enum Fit {
    /// It fits: moved, same size, so no edge is off screen or under the taskbar.
    Move { x: i32, y: i32 },
    /// Too big for the screen: resized to fit and centred (its size once un-maximized), then
    /// maximized.
    Maximize(Frame),
}

/// What it takes to get `window` inside `area`; `None` when it already is.
pub fn fit_frame(window: Frame, area: Frame) -> Option<Fit> {
    let offset = |free: u32| i32::try_from(free).unwrap_or(i32::MAX);
    if window.width > area.width || window.height > area.height {
        let width = window.width.min(area.width);
        let height = window.height.min(area.height);
        return Some(Fit::Maximize(Frame {
            x: area.x + offset((area.width - width) / 2),
            y: area.y + offset((area.height - height) / 2),
            width,
            height,
        }));
    }
    let x = window
        .x
        .clamp(area.x, area.x + offset(area.width - window.width));
    let y = window
        .y
        .clamp(area.y, area.y + offset(area.height - window.height));
    (x != window.x || y != window.y).then_some(Fit::Move { x, y })
}

/// Windows: like TablePlus for Windows, the toolbar is the title bar (it drags the window and holds
/// the minimize / maximize / close buttons) and there is no menu bar: the toolbar's ⋮ button pops
/// the menu up (`popup_app_menu`). The hidden menu keeps its accelerators. Call it before
/// `fit_to_work_area`, which measures the window's frame.
#[cfg(windows)]
pub fn own_title_bar<R: Runtime>(window: &tauri::WebviewWindow<R>) -> tauri::Result<()> {
    window.set_decorations(false)?;
    // The drop shadow and the resize border an undecorated window has none of otherwise.
    window.set_shadow(true)?;
    window.hide_menu()
}

/// Windows: the configured size (1280×820) is taller than a 768 px screen's work area, and a size
/// saved on a bigger monitor may not fit either, so a window that sticks out of its monitor's work
/// area (under the taskbar) is moved in, or resized to fit and (`maximize`) maximized when it is
/// too big. Call it after the window-state plugin has restored the window (that happens while it
/// is built); a restored size that fits is left alone. The main window on Windows is not maximized
/// here: it is hidden until its renderer has given it its shape, and maximizes it then if need be.
pub fn fit_to_work_area<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
    maximize: bool,
) -> tauri::Result<()> {
    if window.is_maximized()? || window.is_fullscreen()? || window.is_minimized()? {
        return Ok(());
    }
    let Some(monitor) = window.current_monitor()?.or(window.primary_monitor()?) else {
        return Ok(());
    };
    let area = monitor.work_area();
    let (outer, inner, pos) = (
        window.outer_size()?,
        window.inner_size()?,
        window.outer_position()?,
    );
    let frame = Frame {
        x: pos.x,
        y: pos.y,
        width: outer.width,
        height: outer.height,
    };
    let area = Frame {
        x: area.position.x,
        y: area.position.y,
        width: area.size.width,
        height: area.size.height,
    };
    match fit_frame(frame, area) {
        None => {}
        Some(Fit::Move { x, y }) => window.set_position(tauri::PhysicalPosition { x, y })?,
        Some(Fit::Maximize(to)) => {
            // `set_size` is the inner size: take off the borders, title and menu bars.
            let bars_width = outer.width.saturating_sub(inner.width);
            let bars_height = outer.height.saturating_sub(inner.height);
            window.set_size(tauri::PhysicalSize {
                width: to.width.saturating_sub(bars_width),
                height: to.height.saturating_sub(bars_height),
            })?;
            window.set_position(tauri::PhysicalPosition { x: to.x, y: to.y })?;
            if maximize {
                window.maximize()?;
            }
        }
    }
    Ok(())
}

/// Window events the registry follows: focus (menu items act on the focused window) and a window
/// going away (its connections are closed; a quit waiting on it may go on).
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    let app = window.app_handle();
    let Some(registry) = app.try_state::<WindowRegistry>() else {
        return;
    };
    match event {
        WindowEvent::Focused(true) => registry.set_focused(window.label()),
        WindowEvent::Destroyed => {
            let label = window.label().to_string();
            let remaining: Vec<String> = open_labels(app)
                .into_iter()
                .filter(|l| *l != label)
                .collect();
            let gone = registry.forget(&label, &remaining);
            let held_connections = !gone.connections.is_empty();
            if let Some(quit) = app.try_state::<crate::shell::QuitState>() {
                quit.set_unsaved(gone.unsaved);
            }
            if let Some(owner) = &gone.promoted {
                log::info!("window {label} closed: window {owner} keeps the session saved now");
                let target = EventTarget::webview_window(owner.as_str());
                if let Err(e) = app.emit_to(target, EVENT_SESSION_OWNER, ()) {
                    log::warn!("could not emit {EVENT_SESSION_OWNER}: {e}");
                }
            }
            if !gone.connections.is_empty() {
                log::info!(
                    "window {label} closed: closing its {} connection(s)",
                    gone.connections.len()
                );
                let app = app.clone();
                let ids = gone.connections;
                tauri::async_runtime::spawn(async move {
                    for id in ids {
                        disconnect(&app, id).await;
                    }
                });
            }
            // The Settings window does not keep the app open: closing the last workspace window
            // while it is open still shows Welcome (macOS, Windows) or ends the app (`exits_now`,
            // macOS too when Settings would be left alone).
            let app_windows_left = app_window_count(&remaining);
            let aux_left = remaining.len() - app_windows_left;
            if reopens_welcome(
                cfg!(any(target_os = "macos", windows)),
                registry.is_quitting(),
                held_connections,
                app_windows_left,
                registry.building(),
            ) {
                log::info!("window {label} closed: it was the last workspace, showing Welcome");
                registry.set_welcome_pending(true);
                reopen_welcome(app);
            } else if gone.quit_now && !remaining.is_empty() {
                log::info!("quit: the last window that held it up closed");
                if let Some(quit) = app.try_state::<crate::shell::QuitState>() {
                    quit.confirm();
                }
                exit_after_settings(app);
            } else if exits_now(
                cfg!(target_os = "macos"),
                registry.welcome_pending(),
                app_windows_left,
                registry.building(),
                aux_left,
            ) {
                // Its close guard already ran: nothing is left to ask.
                log::info!("window {label} closed: it was the last one, exiting");
                if let Some(quit) = app.try_state::<crate::shell::QuitState>() {
                    quit.confirm();
                }
                exit_after_settings(app);
            }
        }
        _ => {}
    }
}

/// Ends the app (a confirmed quit, the last app window gone). An open Settings window is closed
/// first and given [`SETTINGS_FLUSH_TIMEOUT`] to go: its close request writes the preferences and
/// themes still waiting for their debounced save (`useFlushOnClose` in
/// src/features/settings/SettingsWindow.tsx), which an immediate exit would drop.
pub fn exit_after_settings<R: Runtime>(app: &AppHandle<R>) {
    let Some(settings) = app.get_webview_window(SETTINGS_WINDOW) else {
        app.exit(0);
        return;
    };
    if let Err(e) = settings.close() {
        log::warn!("could not close the Settings window before exiting: {e}");
        app.exit(0);
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let started = std::time::Instant::now();
        while app.get_webview_window(SETTINGS_WINDOW).is_some()
            && started.elapsed() < SETTINGS_FLUSH_TIMEOUT
        {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        app.exit(0);
    });
}

/// Closes a connection whose window went away (same steps as the `disconnect` command).
async fn disconnect<R: Runtime>(app: &AppHandle<R>, connection_id: Uuid) {
    use crate::state::AppState;
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    close_connection(&state, connection_id).await;
}

/// The steps of [`disconnect`] that need no window: the connection leaves the state, its session
/// and tunnel close, and a SQLite over SSH copy of it gets its last upload and is cleaned up.
async fn close_connection(state: &crate::state::AppState, connection_id: Uuid) {
    use crate::types::profile::ConnStatus;
    let Some(conn) = state.remove_connection(connection_id).await else {
        return;
    };
    conn.any_session().await.close().await;
    conn.status.send_replace(ConnStatus::Disconnected);
    if let Some(t) = &conn.tunnel {
        t.close().await;
    }
    // SQLite over SSH: the last changes go back to the server, then the downloaded copy, its SSH
    // link and its upload watcher go (the window that held them is gone).
    crate::commands::sqlite_ssh::detach(connection_id).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn hands_out_the_lowest_free_label() {
        assert_eq!(next_window_label(["main"]), "w-2");
        assert_eq!(next_window_label(["main", "w-2", "w-4"]), "w-3");
        assert_eq!(next_window_label(Vec::<&str>::new()), "w-2");
        let registry = WindowRegistry::default();
        let open = labels(&["main"]);
        // A label is reserved until its window is built, so two quick ⌘N get two labels.
        let a = registry.reserve(&open, None);
        let b = registry.reserve(&open, None);
        assert_eq!((a.as_str(), b.as_str()), ("w-2", "w-3"));
        registry.release(&a);
        registry.release(&b);
        assert_eq!(registry.reserve(&open, None), "w-2");
    }

    #[test]
    fn parks_a_handoff_until_its_window_takes_it() {
        let registry = WindowRegistry::default();
        let handoff = WindowHandoff {
            profile_id: Some(Uuid::nil()),
            database: Some("shop".into()),
            schema: None,
            tab: Some(serde_json::json!({ "kind": "query", "sql": "select 1" })),
        };
        let label = registry.reserve(&labels(&["main"]), Some(handoff.clone()));
        assert_eq!(registry.take_handoff("main"), None);
        assert_eq!(registry.take_handoff(&label), Some(handoff));
        assert_eq!(registry.take_handoff(&label), None);
        // A window that goes away drops what it did not take.
        let other = registry.reserve(&labels(&["main"]), Some(WindowHandoff::default()));
        registry.forget(&other, &labels(&["main"]));
        assert_eq!(registry.take_handoff(&other), None);
    }

    #[test]
    fn aggregates_unsaved_work_per_window() {
        let registry = WindowRegistry::default();
        assert!(!registry.any_unsaved());
        assert!(registry.set_unsaved("main", true));
        assert!(registry.set_unsaved("w-2", false));
        assert!(!registry.set_unsaved("main", false));
        assert!(registry.set_unsaved("w-2", true));
        // Closing the window with the unsaved work leaves nothing unsaved.
        let gone = registry.forget("w-2", &labels(&["main"]));
        assert!(!gone.unsaved);
        assert!(!registry.any_unsaved());
    }

    #[test]
    fn closing_a_window_hands_back_its_connections() {
        let registry = WindowRegistry::default();
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        registry.set_connections("main", [a]);
        registry.set_connections("w-2", [b, c]);
        registry.set_connections("w-2", [c]);
        assert_eq!(registry.owner_of(a).as_deref(), Some("main"));
        assert_eq!(registry.owner_of(b), None);
        let gone = registry.forget("w-2", &labels(&["main"]));
        assert_eq!(gone.connections, vec![c]);
        assert_eq!(registry.owner_of(c), None);
        registry.set_connections("main", []);
        assert_eq!(registry.forget("main", &[]).connections, Vec::<Uuid>::new());
    }

    #[test]
    fn closing_the_last_workspace_window_shows_welcome() {
        // The last window, holding connections, closed by its user: the Welcome window comes up.
        assert!(reopens_welcome(true, false, true, 0, 0));
        // A quit ends the app; so does closing the Welcome window (no connections).
        assert!(!reopens_welcome(true, true, true, 0, 0));
        assert!(!reopens_welcome(true, false, false, 0, 0));
        // Other windows left (or on their way): nothing to replace.
        assert!(!reopens_welcome(true, false, true, 1, 0));
        assert!(!reopens_welcome(true, false, true, 0, 1));
        // Linux (no Welcome window) keeps its behaviour.
        assert!(!reopens_welcome(false, false, true, 0, 0));
        let reg = WindowRegistry::default();
        assert!(!reg.welcome_pending());
        reg.set_welcome_pending(true);
        assert!(reg.welcome_pending());
    }

    #[test]
    fn the_last_window_closing_ends_the_app_off_macos() {
        // Windows and Linux: no window left, none being built.
        assert!(exits_with_last_window(false, 0, 0));
        assert!(!exits_with_last_window(false, 1, 0));
        // A window on its way (Move Tab to New Window from the last one) keeps the app.
        assert!(!exits_with_last_window(false, 0, 1));
        // macOS is left to the runtime, as before.
        assert!(!exits_with_last_window(true, 0, 0));
        // The registry counts reserved windows until Tauri lists them.
        let reg = WindowRegistry::default();
        let label = reg.reserve(&labels(&["main"]), None);
        assert_eq!(reg.building(), 1);
        reg.release(&label);
        assert_eq!(reg.building(), 0);
        let label = reg.reserve(&labels(&["main"]), None);
        reg.forget(&label, &labels(&["main"]));
        assert_eq!(reg.building(), 0);
    }

    #[test]
    fn a_quit_waits_for_every_window() {
        let registry = WindowRegistry::default();
        let open = labels(&["main", "w-2"]);
        // The clean window answers first; the other one is still asking its user.
        assert!(!registry.ready_to_quit("main", &open));
        assert!(registry.is_quitting());
        // The other window discarded its work and closed: nothing holds the quit up any more.
        let gone = registry.forget("w-2", &labels(&["main"]));
        assert!(gone.quit_now);

        // Both answer: the second answer quits.
        let registry = WindowRegistry::default();
        assert!(!registry.ready_to_quit("w-2", &open));
        assert!(registry.ready_to_quit("main", &open));

        // Cancelled in one window: closing the other one later is not a quit.
        let registry = WindowRegistry::default();
        assert!(!registry.ready_to_quit("main", &open));
        registry.cancel_quit();
        assert!(!registry.is_quitting());
        assert!(!registry.forget("w-2", &labels(&["main"])).quit_now);

        // No quit under way: a window closing is just that.
        let registry = WindowRegistry::default();
        assert!(!registry.forget("w-2", &labels(&["main"])).quit_now);
    }

    #[test]
    fn the_oldest_window_left_keeps_the_session_when_its_owner_closes() {
        let registry = WindowRegistry::default();
        // w-3 was built before w-2 (w-2 was closed and its label handed out again).
        registry.born("w-3");
        registry.born("w-2");
        let open = labels(&["main", "w-2", "w-3"]);
        assert!(registry.is_session_owner("main", &open));
        assert!(!registry.is_session_owner("w-2", &open));
        // Another window closing changes nothing.
        let gone = registry.forget("w-2", &labels(&["main", "w-3"]));
        assert_eq!(gone.promoted, None);
        registry.born("w-2");
        // The main window closes: the oldest window left takes over.
        let gone = registry.forget("main", &labels(&["w-2", "w-3"]));
        assert_eq!(gone.promoted.as_deref(), Some("w-3"));
        assert!(registry.is_session_owner("w-3", &labels(&["w-2", "w-3"])));
        assert!(!registry.is_session_owner("w-2", &labels(&["w-2", "w-3"])));
        let gone = registry.forget("w-3", &labels(&["w-2"]));
        assert_eq!(gone.promoted.as_deref(), Some("w-2"));
        // The last window closes: nobody owns it until a window opens again.
        let gone = registry.forget("w-2", &[]);
        assert_eq!(gone.promoted, None);
        registry.born("w-4");
        assert!(registry.is_session_owner("w-4", &labels(&["w-4"])));
    }

    #[test]
    fn events_go_to_their_window_while_it_is_open() {
        let open = labels(&["main", "w-2"]);
        let focused = || Some("main".to_string());
        assert_eq!(
            route_label(Some("w-2"), &open, focused).as_deref(),
            Some("w-2")
        );
        // Closed meanwhile, or not known: the focused window.
        assert_eq!(
            route_label(Some("w-3"), &open, focused).as_deref(),
            Some("main")
        );
        assert_eq!(route_label(None, &open, focused).as_deref(), Some("main"));
        assert_eq!(route_label(None, &[], || None), None);
    }

    #[test]
    fn remembers_the_window_focused_last() {
        let registry = WindowRegistry::default();
        assert_eq!(registry.last_focused(), None);
        registry.set_focused("w-2");
        assert_eq!(registry.last_focused().as_deref(), Some("w-2"));
        registry.forget("w-2", &labels(&["main"]));
        assert_eq!(registry.last_focused(), None);
        // The window focused before the one that closed is the last one again.
        registry.set_focused("main");
        registry.set_focused("w-2");
        registry.forget("w-2", &labels(&["main"]));
        assert_eq!(registry.last_focused().as_deref(), Some("main"));
    }

    #[test]
    fn a_picked_connection_goes_to_the_workspace_window_focused_last() {
        let registry = WindowRegistry::default();
        let open = labels(&["main", "w-2", "w-3", WELCOME_WINDOW]);
        // No window holds a connection: the Welcome window opens it itself.
        assert_eq!(registry.workspace_window(WELCOME_WINDOW, &open), None);
        registry.set_connections("main", [Uuid::new_v4()]);
        registry.set_connections("w-3", [Uuid::new_v4()]);
        assert!(registry.has_connections("main"));
        assert!(!registry.has_connections("w-2"));
        // Never focused: the oldest one (the main window).
        assert_eq!(
            registry.workspace_window(WELCOME_WINDOW, &open).as_deref(),
            Some("main")
        );
        registry.set_focused("main");
        registry.set_focused("w-3");
        // A window without a connection (w-2) and the asking window do not count.
        registry.set_focused("w-2");
        registry.set_focused(WELCOME_WINDOW);
        assert_eq!(
            registry.workspace_window(WELCOME_WINDOW, &open).as_deref(),
            Some("w-3")
        );
        // Never the window that asks, and not one that closed.
        assert_eq!(
            registry.workspace_window("w-3", &open).as_deref(),
            Some("main")
        );
        registry.forget("w-3", &labels(&["main", "w-2", WELCOME_WINDOW]));
        assert_eq!(
            registry
                .workspace_window(WELCOME_WINDOW, &labels(&["main", "w-2", WELCOME_WINDOW]))
                .as_deref(),
            Some("main")
        );
        registry.set_connections("main", []);
        assert_eq!(registry.workspace_window(WELCOME_WINDOW, &open), None);
    }

    #[test]
    fn the_settings_window_is_not_an_app_window() {
        assert!(is_aux_window(SETTINGS_WINDOW));
        assert!(!is_aux_window(MAIN_WINDOW));
        assert!(!is_aux_window(WELCOME_WINDOW));
        assert!(!is_aux_window("w-2"));
        // Its label is never handed out to a new window, and does not take a number.
        assert_eq!(next_window_label(["main", SETTINGS_WINDOW]), "w-2");
        assert_eq!(
            app_window_count(&labels(&["main", "w-2", SETTINGS_WINDOW])),
            2
        );
        assert_eq!(app_window_count(&labels(&[SETTINGS_WINDOW])), 0);
    }

    #[test]
    fn closing_the_last_workspace_window_with_settings_open() {
        // The Settings window is all that is left: Welcome comes up (macOS, Windows)…
        let left = app_window_count(&labels(&[SETTINGS_WINDOW]));
        assert!(reopens_welcome(true, false, true, left, 0));
        // …and Windows / Linux end the app instead of keeping a Settings window alone.
        assert!(exits_with_last_window(false, left, 0));
        // With another app window open, nothing happens.
        let left = app_window_count(&labels(&["w-2", SETTINGS_WINDOW]));
        assert!(!reopens_welcome(true, false, true, left, 0));
        assert!(!exits_with_last_window(false, left, 0));
    }

    #[test]
    fn the_app_ends_rather_than_leave_the_settings_window_alone() {
        let settings_only = labels(&[SETTINGS_WINDOW]);
        let app_left = app_window_count(&settings_only);
        let aux_left = settings_only.len() - app_left;
        // Windows / Linux: the last app window went away (Welcome, or a workspace window without
        // connections), Settings is still open: the app ends.
        assert!(exits_now(false, false, app_left, 0, aux_left));
        // macOS: the same, although the runtime's own last-window exit does not happen while
        // Settings is open (its menu bar would act on no window).
        assert!(exits_now(true, false, app_left, 0, aux_left));
        // macOS without Settings: the runtime's own exit, as before.
        assert!(!exits_now(true, false, 0, 0, 0));
        assert!(exits_now(false, false, 0, 0, 0));
        // The Welcome window is on its way (the last workspace window closed with connections):
        // Settings closing meanwhile ends nothing, on any platform.
        for macos in [false, true] {
            assert!(!exits_now(macos, true, 0, 0, 0));
            assert!(!exits_now(macos, true, app_left, 0, aux_left));
        }
        // A window being built, or an app window left: nothing ends.
        assert!(!exits_now(false, false, 0, 1, aux_left));
        assert!(!exits_now(true, false, 0, 1, aux_left));
        assert!(!exits_now(false, false, 1, 0, aux_left));
        assert!(!exits_now(true, false, 1, 0, aux_left));
    }

    #[test]
    fn builds_one_settings_window_at_a_time() {
        let registry = WindowRegistry::default();
        assert!(registry.begin_settings_build());
        // ⌘, again while it is being built: no second window.
        assert!(!registry.begin_settings_build());
        registry.end_settings_build();
        assert!(registry.begin_settings_build());
    }

    #[test]
    fn the_settings_window_owns_no_connection_and_never_the_session() {
        let registry = WindowRegistry::default();
        registry.set_connections(SETTINGS_WINDOW, [Uuid::new_v4()]);
        assert!(!registry.has_connections(SETTINGS_WINDOW));
        // The main window closes with Settings open: Settings does not keep the session saved.
        let gone = registry.forget("main", &labels(&[SETTINGS_WINDOW]));
        assert_eq!(gone.promoted, None);
        assert!(!registry.is_session_owner(SETTINGS_WINDOW, &labels(&[SETTINGS_WINDOW])));
        registry.born("w-2");
        let open = labels(&["w-2", SETTINGS_WINDOW]);
        assert!(registry.is_session_owner("w-2", &open));
        // Never the window a picked connection goes to either.
        registry.set_connections("w-2", [Uuid::new_v4()]);
        assert_eq!(
            registry
                .workspace_window(
                    WELCOME_WINDOW,
                    &labels(&["w-2", SETTINGS_WINDOW, WELCOME_WINDOW])
                )
                .as_deref(),
            Some("w-2")
        );
    }

    #[test]
    fn a_quit_does_not_wait_for_the_settings_window() {
        let registry = WindowRegistry::default();
        let open = labels(&["main", SETTINGS_WINDOW]);
        // The workspace window answers: the app may quit, whether Settings answered or not.
        assert!(registry.ready_to_quit("main", &open));

        // Settings answers first: still waiting for the workspace window.
        let registry = WindowRegistry::default();
        assert!(!registry.ready_to_quit(SETTINGS_WINDOW, &open));
        assert!(registry.ready_to_quit("main", &open));

        // Settings alone (a quit chosen from it): its own answer quits.
        let registry = WindowRegistry::default();
        assert!(registry.ready_to_quit(SETTINGS_WINDOW, &labels(&[SETTINGS_WINDOW])));

        // The window holding the quit up discards its work and closes: Settings does not hold it.
        let registry = WindowRegistry::default();
        let open = labels(&["main", "w-2", SETTINGS_WINDOW]);
        assert!(!registry.ready_to_quit("main", &open));
        let gone = registry.forget("w-2", &labels(&["main", SETTINGS_WINDOW]));
        assert!(gone.quit_now);
    }

    #[test]
    fn the_settings_window_is_not_remembered_as_focused() {
        let registry = WindowRegistry::default();
        registry.set_focused("w-2");
        registry.set_focused(SETTINGS_WINDOW);
        assert_eq!(registry.last_focused().as_deref(), Some("w-2"));
    }

    #[test]
    fn parks_the_section_for_the_settings_window() {
        let registry = WindowRegistry::default();
        assert_eq!(registry.take_settings_section(), None);
        registry.park_settings_section(Some("shortcuts".into()));
        registry.park_settings_section(Some("security".into()));
        assert_eq!(
            registry.take_settings_section().as_deref(),
            Some("security")
        );
        assert_eq!(registry.take_settings_section(), None);
        // The window closes before its renderer took it: the next one starts without it.
        registry.park_settings_section(Some("mcp".into()));
        registry.forget(SETTINGS_WINDOW, &labels(&["main"]));
        assert_eq!(registry.take_settings_section(), None);
    }

    #[test]
    fn menu_items_reach_the_settings_window_only_when_it_handles_them() {
        let workspace = || Some("w-2".to_string());
        let to = |label: &str, raise: bool| {
            Some(MenuRoute {
                label: label.to_string(),
                raise,
            })
        };
        // Settings has the focus: ⌘, ⌘W, Lock Tabler and Check for Updates… are its own.
        for action in [
            "settings.open",
            "tab.close",
            "app.lock",
            "app.check-updates",
        ] {
            assert_eq!(
                menu_route(Some(SETTINGS_WINDOW), action, workspace),
                to(SETTINGS_WINDOW, false),
                "{action}"
            );
        }
        // Everything else goes to the workspace window, which comes to the front.
        for action in [
            "tab.new-query",
            "window.new",
            "palette.open",
            "recent.connection.1",
        ] {
            assert_eq!(
                menu_route(Some(SETTINGS_WINDOW), action, workspace),
                to("w-2", true),
                "{action}"
            );
        }
        // A workspace window has the focus (or none): as before, nothing is raised.
        assert_eq!(
            menu_route(Some("main"), "settings.open", || Some("main".into())),
            to("main", false)
        );
        assert_eq!(menu_route(None, "tab.close", workspace), to("w-2", false));
        // Settings alone and an item it does not handle: nowhere to send it.
        assert_eq!(
            menu_route(Some(SETTINGS_WINDOW), "tab.new-query", || None),
            None
        );
    }

    #[test]
    fn centres_the_settings_window_on_its_window() {
        // A 1280×820 window at (100, 50): the 888×654 Settings window sits in its middle.
        assert_eq!(
            centered_on((100.0, 50.0, 1280.0, 820.0), (888.0, 654.0)),
            (296.0, 133.0)
        );
        // A smaller window (the Welcome window): Settings sticks out evenly on every side.
        assert_eq!(
            centered_on((0.0, 0.0, 746.0, 529.0), (888.0, 654.0)),
            (-71.0, -62.5)
        );
    }

    #[test]
    fn fits_windows_into_the_work_area() {
        let frame = |x, y, width, height| Frame {
            x,
            y,
            width,
            height,
        };
        // 1920×1080 with a 48 px taskbar.
        let big = frame(0, 0, 1920, 1032);
        assert_eq!(fit_frame(frame(320, 100, 1296, 880), big), None);
        // Saved half under the taskbar: moved up, same size.
        assert_eq!(
            fit_frame(frame(320, 400, 1296, 880), big),
            Some(Fit::Move { x: 320, y: 152 })
        );
        // Left of a second monitor's work area.
        assert_eq!(
            fit_frame(frame(1900, 0, 1296, 880), frame(1920, 0, 1920, 1032)),
            Some(Fit::Move { x: 1920, y: 0 })
        );
        // The default size on a 1366×768 screen (728 px work area): centred at the screen's
        // height, then maximized.
        assert_eq!(
            fit_frame(frame(43, -56, 1296, 880), frame(0, 0, 1366, 728)),
            Some(Fit::Maximize(frame(35, 0, 1296, 728)))
        );
    }
    /// A SQLite over SSH copy that goes with a closed window's connection: the window cleanup
    /// must run the same final upload and clean-up as the `disconnect` command (it once left the
    /// copy, its SSH link and its watcher behind until the app quit).
    #[tokio::test]
    async fn a_closed_windows_sqlite_over_ssh_copy_is_detached() {
        use std::sync::Arc;

        use crate::drivers::{self, ConnectTarget};
        use crate::secrets::{Backend, Secrets};
        use crate::state::{AppState, Connection};
        use crate::tunnel::SshSession;
        use crate::tunnel::sqlite_remote::{self, RemoteHost, RemoteSqlite, SqliteSshEvent};
        use crate::types::profile::{
            ConnStatus, ConnectionInfo, ConnectionProfile, Driver, ProfileOptions, SslMode,
        };

        /// Never asked: the copy is read-only, so nothing goes over SSH.
        struct NoHost;
        #[async_trait::async_trait]
        impl RemoteHost for NoHost {
            async fn open_ssh(&self) -> Result<SshSession, crate::error::AppError> {
                unreachable!("a read-only copy has no SSH link")
            }
            async fn confirm(&self, _: &str, _: &str) -> Result<bool, crate::error::AppError> {
                unreachable!()
            }
            fn emit(&self, _: SqliteSshEvent) {}
        }

        let appdb = crate::appdb::AppDb::open_in_memory().unwrap();
        let state = AppState::new(appdb.clone(), Secrets::new(Backend::File, appdb));
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("remote.sqlite");
        rusqlite::Connection::open(&local)
            .unwrap()
            .execute_batch("CREATE TABLE t(x);")
            .unwrap();
        let profile = ConnectionProfile {
            id: Uuid::new_v4(),
            name: "remote lite".into(),
            color: None,
            group: None,
            driver: Driver::Sqlite,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: Some(local.to_string_lossy().into_owned()),
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        };
        let session = drivers::connect(ConnectTarget {
            profile: &profile,
            password: None,
            host_override: None,
        })
        .await
        .unwrap();
        let id = Uuid::new_v4();
        let info = ConnectionInfo {
            connection_id: id,
            profile_id: profile.id,
            name: profile.name.clone(),
            color: None,
            driver: profile.driver,
            server_version: String::new(),
            current_database: None,
            current_user: None,
            tunnel: None,
            read_only: false,
            capabilities: None,
        };
        let status = tokio::sync::watch::channel(ConnStatus::Connected);
        let conn = Arc::new(Connection {
            id,
            profile: profile.clone(),
            session: tokio::sync::RwLock::new(session.into()),
            info: tokio::sync::RwLock::new(info),
            schema_cache: tokio::sync::RwLock::new(Default::default()),
            status: status.0,
            tunnel: None,
        });
        state.insert_connection(conn, None).await;
        let copy: Arc<RemoteSqlite> =
            RemoteSqlite::for_test(dir.path().to_path_buf(), local.clone(), Arc::new(NoHost));
        sqlite_remote::register(id, copy);
        assert!(sqlite_remote::get(id).is_some());

        close_connection(&state, id).await;

        assert!(
            sqlite_remote::get(id).is_none(),
            "the copy is still registered"
        );
        assert!(!local.exists(), "the downloaded copy was not cleaned up");
        assert!(state.connections.read().await.get(&id).is_none());
        assert_eq!(*status.1.borrow(), ConnStatus::Disconnected);
    }
}
