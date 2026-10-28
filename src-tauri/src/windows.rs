//! Multi-window bookkeeping (TablePlus: File ▸ New Window ⌘N, "Open in New Window", Window ▸ Move
//! Tab to New Window). Every window is a webview with its own renderer stores; the backend state
//! (open connections) is shared, but each window owns the connections its renderer opened: they are
//! closed when the window goes away.
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
    last_focused: Option<String>,
    /// Windows built after the main one, oldest first (labels are reused, so their numbers do not
    /// tell their age).
    born: Vec<String>,
    /// The window that keeps the session saved (`None`: the main window).
    session_owner: Option<String>,
}

impl Inner {
    fn session_owner(&self) -> &str {
        self.session_owner.as_deref().unwrap_or(MAIN_WINDOW)
    }

    /// The oldest of `candidates`: the main window, else the one built first.
    fn oldest(&self, candidates: &[String]) -> Option<String> {
        if candidates.iter().any(|l| l == MAIN_WINDOW) {
            return Some(MAIN_WINDOW.to_string());
        }
        self.born
            .iter()
            .find(|l| candidates.contains(l))
            .or_else(|| candidates.iter().min())
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

    /// The connections a window owns now (replaces what it reported before).
    pub fn set_connections(&self, label: &str, ids: impl IntoIterator<Item = Uuid>) {
        let ids: HashSet<Uuid> = ids.into_iter().collect();
        let mut inner = self.lock();
        if ids.is_empty() {
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
    /// True when every window in `open` (the windows Tauri still lists) is ready, so the app may
    /// exit; otherwise the others are still asking their user.
    pub fn ready_to_quit(&self, label: &str, open: &[String]) -> bool {
        let mut inner = self.lock();
        inner.quitting = true;
        inner.quit_ready.insert(label.to_string());
        open.iter().all(|l| inner.quit_ready.contains(l))
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

    /// A window went away (`remaining`: the windows left). Forgets it and says what is left to do.
    pub fn forget(&self, label: &str, remaining: &[String]) -> WindowGone {
        let mut inner = self.lock();
        inner.handoffs.remove(label);
        inner.reserved.remove(label);
        inner.unsaved.remove(label);
        inner.quit_ready.remove(label);
        if inner.last_focused.as_deref() == Some(label) {
            inner.last_focused = None;
        }
        let mut connections: Vec<Uuid> = inner
            .connections
            .remove(label)
            .map(|ids| ids.into_iter().collect())
            .unwrap_or_default();
        connections.sort();
        inner.born.retain(|l| l != label);
        let left: Vec<String> = remaining.iter().filter(|l| *l != label).cloned().collect();
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

    pub fn set_focused(&self, label: &str) {
        self.lock().last_focused = Some(label.to_string());
    }

    pub fn last_focused(&self) -> Option<String> {
        self.lock().last_focused.clone()
    }
}

// --- Tauri glue --------------------------------------------------------------------------------

/// Labels of the open webview windows.
pub fn open_labels<R: Runtime>(app: &AppHandle<R>) -> Vec<String> {
    let mut labels: Vec<String> = app.webview_windows().into_keys().collect();
    labels.sort();
    labels
}

/// The window menu items act on: the focused one, else the one focused last, else the main window
/// (or any).
pub fn target_label<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    let windows = app.webview_windows();
    if let Some((label, _)) = windows
        .iter()
        .find(|(_, w)| w.is_focused().unwrap_or(false))
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
    .or_else(|| windows.keys().min().cloned())
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

/// Opens a new window like the main one (cascaded from the focused window), with `handoff` parked
/// for its renderer. Returns its label.
pub fn open_window<R: Runtime>(
    app: &AppHandle<R>,
    handoff: Option<WindowHandoff>,
) -> tauri::Result<String> {
    let registry = app.state::<WindowRegistry>();
    let label = registry.reserve(&open_labels(app), handoff);
    let mut config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == MAIN_WINDOW)
        .cloned()
        .unwrap_or_default();
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
    if let Err(e) = own_title_bar(&window).and_then(|()| fit_to_work_area(&window)) {
        log::warn!("could not set up window {label}: {e}");
    }
    #[cfg(not(windows))]
    let _ = window;
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
/// area (under the taskbar) is moved in, or maximized when it is too big. Call it after the
/// window-state plugin has restored the window (that happens while it is built); a restored size
/// that fits is left alone.
pub fn fit_to_work_area<R: Runtime>(window: &tauri::WebviewWindow<R>) -> tauri::Result<()> {
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
            window.maximize()?;
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
            if gone.quit_now && !remaining.is_empty() {
                log::info!("quit: the last window that held it up closed");
                if let Some(quit) = app.try_state::<crate::shell::QuitState>() {
                    quit.confirm();
                }
                app.exit(0);
            }
        }
        _ => {}
    }
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
