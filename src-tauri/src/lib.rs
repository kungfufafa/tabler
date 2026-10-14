//! Tabler backend: Tauri commands are thin wrappers; logic lives in the modules below so it can be
//! unit-tested without a running app.

// `AppError::Db` carries the full server diagnostic; boxing it everywhere is not worth the noise.
#![allow(clippy::result_large_err)]

pub mod admin;
pub mod appdb;
pub mod backup;
pub mod commands;
pub mod diagram;
pub mod drivers;
pub mod error;
pub mod export;
pub mod import;
pub mod objects;
pub mod profiles_io;
pub mod secrets;
pub mod sqlgen;
pub mod state;
pub mod tls_files;
pub mod tunnel;
pub mod types;

use tauri::{Emitter, Manager, RunEvent};

/// The native menu bar and quitting: menu items route into the renderer's action registry, and
/// quitting goes through the renderer's unsaved-changes guard first.
pub mod shell {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use tauri::menu::{
        Menu, MenuBuilder, MenuItem, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder,
    };
    use tauri::{AppHandle, Emitter, Manager, Runtime};

    /// Payload: the action id (`src/features/palette/actions.ts`) of the chosen menu item.
    pub const EVENT_MENU_ACTION: &str = "menu:action";
    /// Asks the renderer to quit once nothing unsaved is left (it then calls `quit_app`).
    pub const EVENT_QUIT_REQUESTED: &str = "app:quit-requested";
    /// The app menu's custom Quit item (the predefined one would exit unguarded).
    pub const QUIT_ITEM_ID: &str = "app.quit";
    /// An exit request the renderer has not answered after this long is let through, so a hung
    /// renderer cannot keep the app from quitting.
    pub const RENDERER_ANSWER_TIMEOUT: Duration = Duration::from_secs(5);

    /// Menu items that run a renderer action: (action id = menu item id, label, accelerator).
    const MENU_ACTIONS: &[(&str, &str, &str)] = &[
        ("settings.open", "Settings…", "CmdOrCtrl+,"),
        ("connection.new", "New Connection…", "CmdOrCtrl+N"),
        ("tab.new-query", "New Query Tab", "CmdOrCtrl+T"),
        ("query.open", "Open SQL File…", "CmdOrCtrl+O"),
        ("tab.close", "Close Tab", "CmdOrCtrl+W"),
        ("tab.close-all", "Close All Tabs", "CmdOrCtrl+Shift+W"),
        ("tab.reopen", "Reopen Closed Tab", "CmdOrCtrl+Shift+T"),
        ("export.open", "Export…", "CmdOrCtrl+Shift+E"),
        ("ui.launcher", "Connection Manager", "CmdOrCtrl+Shift+O"),
        ("palette.open", "Open Anything…", "CmdOrCtrl+P"),
        ("palette.databases", "Switch Database…", "CmdOrCtrl+K"),
        (
            "palette.connections",
            "Switch Connection…",
            "CmdOrCtrl+Shift+K",
        ),
        ("view.data", "Data", "CmdOrCtrl+Ctrl+["),
        ("view.structure", "Structure", "CmdOrCtrl+Ctrl+]"),
        ("ui.toggle-sidebar", "Toggle Sidebar", "CmdOrCtrl+0"),
        (
            "ui.toggle-inspector",
            "Toggle Row Detail",
            "CmdOrCtrl+Shift+I",
        ),
        (
            "ui.toggle-console",
            "Toggle Console Log",
            "CmdOrCtrl+Shift+C",
        ),
        ("view.refresh", "Refresh", "CmdOrCtrl+R"),
        ("connection.reconnect", "Reconnect", "CmdOrCtrl+Shift+R"),
        ("admin.process-list", "Process List…", "CmdOrCtrl+."),
        ("workspace.show", "Show Workspaces", "CmdOrCtrl+Shift+\\"),
        (
            "workspace.next",
            "Select Next Workspace",
            "CmdOrCtrl+Ctrl+Right",
        ),
        (
            "workspace.prev",
            "Select Previous Workspace",
            "CmdOrCtrl+Ctrl+Left",
        ),
        // File: the SQL editor's save items.
        (
            "query.save-selection",
            "Save Selected SQL Statements As…",
            "CmdOrCtrl+Shift+S",
        ),
        ("query.add-to-queries", "Add to Queries…", "CmdOrCtrl+Alt+S"),
        // Edit: the SQL editor's items.
        ("query.format", "Beautify", "CmdOrCtrl+I"),
        ("query.minify", "Uglify", "CmdOrCtrl+Shift+I"),
        ("query.comment-line", "Toggle Line Comment", "CmdOrCtrl+/"),
        (
            "query.comment-block",
            "Toggle Block Comment",
            "CmdOrCtrl+Alt+/",
        ),
        ("query.split", "Split Pane", "CmdOrCtrl+Shift+D"),
        // Edit: the data grid's items with keys of their own.
        ("grid.delete-rows", "Delete Rows", "CmdOrCtrl+Backspace"),
        ("grid.set-null", "NULL", "CmdOrCtrl+Shift+N"),
        ("grid.copy-as", "Copy Rows As…", "CmdOrCtrl+Alt+C"),
        ("grid.quick-look", "Quick Look", "CmdOrCtrl+Enter"),
        // View.
        ("grid.columns", "Columns", "CmdOrCtrl+Alt+F"),
        ("grid.filter", "Filters", "CmdOrCtrl+F"),
        // Navigate.
        ("tab.back", "Back", "Ctrl+-"),
        ("tab.forward", "Forward", "Ctrl+Shift+-"),
        ("tab.next", "Select Next Tab", "CmdOrCtrl+]"),
        ("tab.prev", "Select Previous Tab", "CmdOrCtrl+["),
        ("tab.goto-1", "Tab 1", "CmdOrCtrl+1"),
        ("tab.goto-2", "Tab 2", "CmdOrCtrl+2"),
        ("tab.goto-3", "Tab 3", "CmdOrCtrl+3"),
        ("tab.goto-4", "Tab 4", "CmdOrCtrl+4"),
        ("tab.goto-5", "Tab 5", "CmdOrCtrl+5"),
        ("tab.goto-6", "Tab 6", "CmdOrCtrl+6"),
        ("tab.goto-7", "Tab 7", "CmdOrCtrl+7"),
        ("tab.goto-8", "Tab 8", "CmdOrCtrl+8"),
        ("tab.goto-9", "Last Tab", "CmdOrCtrl+9"),
        ("ui.next-pane", "Select Next Pane", "CmdOrCtrl+Shift+]"),
        ("ui.prev-pane", "Select Previous Pane", "CmdOrCtrl+Shift+["),
    ];

    /// Menu items without a keyboard shortcut (TablePlus gives them none): (action id, label).
    /// Also the grid items whose keys are context-dependent and belong to another menu item here
    /// (⌘I Beautify / Insert Row, ⇧⌘D Split Pane / Set DEFAULT, ⌘D the editor's next occurrence):
    /// the page resolves those keys by focus (`MENU_UNBOUND_ACTIONS` in useNativeMenu.ts).
    const PLAIN_MENU_ACTIONS: &[(&str, &str)] = &[
        ("backup.open", "Backup…"),
        ("restore.open", "Restore…"),
        ("admin.users", "User Management…"),
        ("connection.disconnect", "Disconnect"),
        ("import.csv", "From CSV…"),
        ("import.sql", "From SQL Dump…"),
        ("query.save-as", "Save SQL As…"),
        ("grid.new-row", "Insert Row"),
        ("grid.duplicate-row", "Duplicate Row"),
        ("grid.set-empty", "EMPTY"),
        ("grid.set-default", "DEFAULT"),
        ("grid.set-now", "Now (local time)"),
    ];

    /// The renderer action a menu item id stands for; `None` for predefined items and Quit.
    pub fn menu_action(id: &str) -> Option<&'static str> {
        MENU_ACTIONS
            .iter()
            .find(|(action, _, _)| *action == id)
            .map(|(action, _, _)| *action)
            .or_else(|| {
                PLAIN_MENU_ACTIONS
                    .iter()
                    .find(|(action, _)| *action == id)
                    .map(|(action, _)| *action)
            })
    }

    fn action_item<R: Runtime>(app: &AppHandle<R>, id: &str) -> tauri::Result<MenuItem<R>> {
        if let Some((action, label)) = PLAIN_MENU_ACTIONS.iter().find(|(action, _)| *action == id) {
            return MenuItemBuilder::with_id(*action, *label).build(app);
        }
        let (action, label, accelerator) = MENU_ACTIONS
            .iter()
            .find(|(action, _, _)| *action == id)
            .ok_or_else(|| {
                tauri::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("no menu action {id}"),
                ))
            })?;
        MenuItemBuilder::with_id(*action, *label)
            .accelerator(*accelerator)
            .build(app)
    }

    /// App / File / Edit / View / Connection / Navigate / Window, TablePlus's menu bar. Edit keeps
    /// the predefined items: they are what makes ⌘C / ⌘V / ⌘Z work in text fields on macOS.
    pub fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
        let name = app.package_info().name.clone();
        let quit = MenuItemBuilder::with_id(QUIT_ITEM_ID, format!("Quit {name}"))
            .accelerator("CmdOrCtrl+Q")
            .build(app)?;
        let app_menu = SubmenuBuilder::new(app, &name)
            .about(None)
            .separator()
            .item(&action_item(app, "settings.open")?)
            .separator()
            .services()
            .separator()
            .hide()
            .hide_others()
            .show_all()
            .separator()
            .item(&quit)
            .build()?;
        let file = SubmenuBuilder::new(app, "File")
            .item(&action_item(app, "connection.new")?)
            .item(&action_item(app, "tab.new-query")?)
            .item(&action_item(app, "query.open")?)
            .separator()
            .item(&action_item(app, "query.save-as")?)
            .item(&action_item(app, "query.save-selection")?)
            .item(&action_item(app, "query.add-to-queries")?)
            .separator()
            .item(&action_item(app, "tab.close")?)
            .item(&action_item(app, "tab.close-all")?)
            .item(&action_item(app, "tab.reopen")?)
            .separator()
            .item(&action_item(app, "backup.open")?)
            .item(&action_item(app, "restore.open")?)
            .item(&action_item(app, "export.open")?)
            .item(
                &SubmenuBuilder::new(app, "Import")
                    .item(&action_item(app, "import.csv")?)
                    .item(&action_item(app, "import.sql")?)
                    .build()?,
            )
            .item(&action_item(app, "ui.launcher")?)
            .build()?;
        // The SQL editor's items, then the data grid's (TablePlus resolves the shared keys by
        // focus; so does the page, see `PLAIN_MENU_ACTIONS`).
        let edit = SubmenuBuilder::new(app, "Edit")
            .undo()
            .redo()
            .separator()
            .cut()
            .copy()
            .paste()
            .select_all()
            .separator()
            .item(&action_item(app, "query.format")?)
            .item(&action_item(app, "query.minify")?)
            .item(&action_item(app, "query.comment-line")?)
            .item(&action_item(app, "query.comment-block")?)
            .item(&action_item(app, "query.split")?)
            .separator()
            .item(&action_item(app, "grid.new-row")?)
            .item(&action_item(app, "grid.duplicate-row")?)
            .item(&action_item(app, "grid.delete-rows")?)
            .item(
                &SubmenuBuilder::new(app, "Set Value")
                    .item(&action_item(app, "grid.set-null")?)
                    .item(&action_item(app, "grid.set-empty")?)
                    .item(&action_item(app, "grid.set-default")?)
                    .item(&action_item(app, "grid.set-now")?)
                    .build()?,
            )
            .item(&action_item(app, "grid.copy-as")?)
            .item(&action_item(app, "grid.quick-look")?)
            .build()?;
        let view = SubmenuBuilder::new(app, "View")
            .item(&action_item(app, "palette.open")?)
            .item(&action_item(app, "palette.databases")?)
            .item(&action_item(app, "palette.connections")?)
            .separator()
            .item(&action_item(app, "view.data")?)
            .item(&action_item(app, "view.structure")?)
            .separator()
            .item(&action_item(app, "grid.columns")?)
            .item(&action_item(app, "grid.filter")?)
            .separator()
            .item(&action_item(app, "ui.toggle-sidebar")?)
            .item(&action_item(app, "ui.toggle-inspector")?)
            .item(&action_item(app, "ui.toggle-console")?)
            .item(&action_item(app, "workspace.show")?)
            .separator()
            .item(&action_item(app, "view.refresh")?)
            .separator()
            .item(&PredefinedMenuItem::fullscreen(app, None)?)
            .build()?;
        // TablePlus's Connection menu.
        let connection = SubmenuBuilder::new(app, "Connection")
            .item(&action_item(app, "admin.process-list")?)
            .item(&action_item(app, "admin.users")?)
            .separator()
            .item(&action_item(app, "connection.reconnect")?)
            .item(&action_item(app, "connection.disconnect")?)
            .build()?;
        let mut go_to_tab = SubmenuBuilder::new(app, "Go to Tab");
        for n in 1..=9 {
            go_to_tab = go_to_tab.item(&action_item(app, &format!("tab.goto-{n}"))?);
        }
        // TablePlus's Navigate menu (and Keymap ▸ Navigate).
        let navigate = SubmenuBuilder::new(app, "Navigate")
            .item(&action_item(app, "tab.back")?)
            .item(&action_item(app, "tab.forward")?)
            .separator()
            .item(&action_item(app, "tab.next")?)
            .item(&action_item(app, "tab.prev")?)
            .item(&go_to_tab.build()?)
            .separator()
            .item(&action_item(app, "workspace.next")?)
            .item(&action_item(app, "workspace.prev")?)
            .separator()
            .item(&action_item(app, "ui.next-pane")?)
            .item(&action_item(app, "ui.prev-pane")?)
            .build()?;
        let window = SubmenuBuilder::new(app, "Window")
            .minimize()
            .item(&PredefinedMenuItem::maximize(app, None)?)
            .build()?;
        MenuBuilder::new(app)
            .items(&[
                &app_menu,
                &file,
                &edit,
                &view,
                &connection,
                &navigate,
                &window,
            ])
            .build()
    }

    /// Forwards a chosen menu item to the renderer.
    pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
        if id == QUIT_ITEM_ID {
            let _ = app.emit(EVENT_QUIT_REQUESTED, ());
        } else if let Some(action) = menu_action(id) {
            let _ = app.emit(EVENT_MENU_ACTION, action);
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ExitDecision {
        /// Let the app exit.
        Allow,
        /// Keep running and ask the renderer, which guards unsaved work.
        AskRenderer,
    }

    /// What to do with an exit request. `confirmed`: the renderer already asked (`quit_app`).
    /// `windows_open == 0`: the window's own close guard already ran. `pending_for`: how long an
    /// earlier request has gone unanswered.
    pub fn exit_decision(
        confirmed: bool,
        windows_open: usize,
        pending_for: Option<Duration>,
    ) -> ExitDecision {
        if confirmed
            || windows_open == 0
            || pending_for.is_some_and(|d| d >= RENDERER_ANSWER_TIMEOUT)
        {
            ExitDecision::Allow
        } else {
            ExitDecision::AskRenderer
        }
    }

    /// What to do when macOS asks the app to terminate (Dock → Quit, logout, shutdown). Same as
    /// `exit_decision`, except that nothing waits for the renderer unless it reported unsaved
    /// work: the answer is synchronous, and "cancel" would also cancel a logout or shutdown.
    pub fn terminate_decision(
        confirmed: bool,
        windows_open: usize,
        unsaved: bool,
        pending_for: Option<Duration>,
    ) -> ExitDecision {
        if unsaved {
            exit_decision(confirmed, windows_open, pending_for)
        } else {
            ExitDecision::Allow
        }
    }

    /// Where an exit request came from.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ExitSource {
        /// `RunEvent::ExitRequested` (an `exit()` call, or the last window went away).
        App,
        /// NSApp `terminate:` on macOS (see `terminate_guard`).
        System,
    }

    /// Decides an exit request with the current quit state. When the renderer must be asked, the
    /// request is recorded; the caller then keeps the app running and emits
    /// `EVENT_QUIT_REQUESTED`.
    pub fn decide_exit<R: Runtime>(app: &AppHandle<R>, source: ExitSource) -> ExitDecision {
        let Some(quit) = app.try_state::<QuitState>() else {
            return ExitDecision::Allow;
        };
        let now = Instant::now();
        let windows_open = app.webview_windows().len();
        let pending_for = quit.pending_for(now);
        let decision = match source {
            ExitSource::App => exit_decision(quit.is_confirmed(), windows_open, pending_for),
            ExitSource::System => terminate_decision(
                quit.is_confirmed(),
                windows_open,
                quit.has_unsaved(),
                pending_for,
            ),
        };
        if decision == ExitDecision::AskRenderer {
            quit.mark_requested(now);
        }
        decision
    }

    /// Managed state of the quit handshake with the renderer.
    #[derive(Debug, Default)]
    pub struct QuitState {
        confirmed: AtomicBool,
        unsaved: AtomicBool,
        requested_at: Mutex<Option<Instant>>,
    }

    impl QuitState {
        /// The renderer cleared the quit (nothing unsaved, or the user discarded it).
        pub fn confirm(&self) {
            self.confirmed.store(true, Ordering::SeqCst);
        }

        pub fn is_confirmed(&self) -> bool {
            self.confirmed.load(Ordering::SeqCst)
        }

        /// The renderer reports whether any tab holds unsaved work (whenever that changes, and in
        /// answer to `EVENT_QUIT_REQUESTED`). Hearing from it also answers the pending exit
        /// request: the renderer is alive, so a later request is asked again instead of counting
        /// as unanswered (after the user cancelled the unsaved-changes dialog, say).
        pub fn set_unsaved(&self, unsaved: bool) {
            self.unsaved.store(unsaved, Ordering::SeqCst);
            *self.requested_at.lock().unwrap_or_else(|p| p.into_inner()) = None;
        }

        pub fn has_unsaved(&self) -> bool {
            self.unsaved.load(Ordering::SeqCst)
        }

        /// Records an exit request sent to the renderer (the first unanswered one counts).
        pub fn mark_requested(&self, now: Instant) {
            let mut at = self.requested_at.lock().unwrap_or_else(|p| p.into_inner());
            at.get_or_insert(now);
        }

        /// How long the oldest unanswered exit request has been waiting.
        pub fn pending_for(&self, now: Instant) -> Option<Duration> {
            let at = self.requested_at.lock().unwrap_or_else(|p| p.into_inner());
            at.map(|t| now.saturating_duration_since(t))
        }
    }

    /// macOS: NSApp `terminate:` (Dock → Quit, Quit from the Dock menu, logout and shutdown, which
    /// all arrive as the quit Apple event) asks the app delegate's `applicationShouldTerminate:`.
    /// tao 0.37 does not implement it, so Cocoa would go straight to `applicationWillTerminate:` →
    /// `RunEvent::Exit` and never reach `RunEvent::ExitRequested`, dropping unsaved work.
    ///
    /// `install` adds that method to tao's delegate class at runtime (the Objective-C runtime
    /// allows adding methods to a registered class, and NSApp looks the method up when
    /// `terminate:` runs) and answers through `decide_exit(.., ExitSource::System)`:
    /// `NSTerminateCancel` plus `EVENT_QUIT_REQUESTED` while unsaved work must be confirmed,
    /// `NSTerminateNow` otherwise. The renderer then quits through `quit_app`, which ends tao's
    /// loop with `stop:` (not `terminate:`), or closes the window after the user discarded.
    /// Plain libobjc calls: no extra dependency.
    #[cfg(target_os = "macos")]
    pub mod terminate_guard {
        use std::ffi::{c_char, c_int, c_void};
        use std::sync::OnceLock;

        use tauri::{AppHandle, Emitter};

        use super::{EVENT_QUIT_REQUESTED, ExitDecision, ExitSource, decide_exit};

        type Id = *mut c_void;
        type Sel = *const c_void;
        type Class = *mut c_void;

        /// `NSApplicationTerminateReply` (an `NSUInteger`).
        const NS_TERMINATE_CANCEL: usize = 0;
        const NS_TERMINATE_NOW: usize = 1;

        #[link(name = "objc", kind = "dylib")]
        unsafe extern "C" {
            fn objc_getClass(name: *const c_char) -> Class;
            fn sel_registerName(name: *const c_char) -> Sel;
            fn object_getClass(obj: Id) -> Class;
            fn class_addMethod(
                cls: Class,
                name: Sel,
                imp: *const c_void,
                types: *const c_char,
            ) -> i8;
            fn objc_msgSend();
        }

        unsafe extern "C" {
            fn pthread_main_np() -> c_int;
        }

        static APP: OnceLock<AppHandle> = OnceLock::new();

        /// `-[TaoAppDelegateParent applicationShouldTerminate:]`. Must not panic (it runs inside
        /// AppKit, and release builds abort on panic).
        extern "C" fn application_should_terminate(_this: Id, _cmd: Sel, _sender: Id) -> usize {
            let Some(app) = APP.get() else {
                return NS_TERMINATE_NOW;
            };
            match decide_exit(app, ExitSource::System) {
                ExitDecision::Allow => NS_TERMINATE_NOW,
                ExitDecision::AskRenderer => {
                    log::info!("system quit with unsaved work: asking the renderer");
                    let app = app.clone();
                    // Emitted off this AppKit callback, so it cannot re-enter the event loop.
                    tauri::async_runtime::spawn(async move {
                        let _ = app.emit(EVENT_QUIT_REQUESTED, ());
                    });
                    NS_TERMINATE_CANCEL
                }
            }
        }

        /// Adds `applicationShouldTerminate:` to the class of NSApp's delegate. Main thread only.
        fn add_should_terminate() -> Result<(), &'static str> {
            // SAFETY: plain Objective-C runtime calls on the main thread (checked first). Each
            // `objc_msgSend` is called through a pointer cast to the exact signature of the method
            // it sends (`+sharedApplication` and `-delegate` both take no argument and return an
            // object). The added IMP matches the type encoding "Q@:@": it returns an NSUInteger
            // and takes self, _cmd and the sender.
            unsafe {
                if pthread_main_np() == 0 {
                    return Err("not on the main thread");
                }
                let send = std::mem::transmute::<
                    unsafe extern "C" fn(),
                    unsafe extern "C" fn(Id, Sel) -> Id,
                >(objc_msgSend);
                let ns_application = objc_getClass(c"NSApplication".as_ptr());
                if ns_application.is_null() {
                    return Err("no NSApplication class");
                }
                let ns_app = send(
                    ns_application,
                    sel_registerName(c"sharedApplication".as_ptr()),
                );
                if ns_app.is_null() {
                    return Err("no NSApp");
                }
                let delegate = send(ns_app, sel_registerName(c"delegate".as_ptr()));
                if delegate.is_null() {
                    return Err("NSApp has no delegate");
                }
                let imp: extern "C" fn(Id, Sel, Id) -> usize = application_should_terminate;
                let added = class_addMethod(
                    object_getClass(delegate),
                    sel_registerName(c"applicationShouldTerminate:".as_ptr()),
                    imp as *const c_void,
                    c"Q@:@".as_ptr(),
                );
                if added == 0 {
                    return Err("the app delegate already implements applicationShouldTerminate:");
                }
            }
            Ok(())
        }

        /// Routes NSApp `terminate:` through the quit guard. Call once, from `setup`.
        pub fn install(app: &AppHandle) {
            if APP.set(app.clone()).is_err() {
                return;
            }
            match add_should_terminate() {
                Ok(()) => log::debug!("Dock / system quit goes through the unsaved-changes guard"),
                Err(why) => log::warn!("Dock / system quit is not guarded: {why}"),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn maps_menu_items_to_renderer_actions() {
            assert_eq!(menu_action("tab.new-query"), Some("tab.new-query"));
            assert_eq!(menu_action("settings.open"), Some("settings.open"));
            assert_eq!(menu_action("view.refresh"), Some("view.refresh"));
            assert_eq!(menu_action("backup.open"), Some("backup.open"));
            assert_eq!(menu_action("restore.open"), Some("restore.open"));
            assert_eq!(
                menu_action("admin.process-list"),
                Some("admin.process-list")
            );
            assert_eq!(menu_action("admin.users"), Some("admin.users"));
            assert_eq!(
                menu_action("connection.disconnect"),
                Some("connection.disconnect")
            );
            assert_eq!(menu_action(QUIT_ITEM_ID), None);
            assert_eq!(menu_action("grid.commit"), None);
            assert_eq!(menu_action(""), None);
            // Every entry has a label and an accelerator, and ids are unique.
            for (i, (id, label, accel)) in MENU_ACTIONS.iter().enumerate() {
                assert!(
                    !label.is_empty() && crate::commands::menu::accelerator_ok(accel),
                    "{id}"
                );
                assert!(
                    MENU_ACTIONS[i + 1..]
                        .iter()
                        .all(|(other, _, _)| other != id),
                    "duplicate {id}"
                );
                assert!(
                    PLAIN_MENU_ACTIONS.iter().all(|(other, _)| other != id),
                    "{id} is listed twice"
                );
            }
            for (i, (id, label)) in PLAIN_MENU_ACTIONS.iter().enumerate() {
                assert!(!label.is_empty(), "{id}");
                assert!(
                    PLAIN_MENU_ACTIONS[i + 1..]
                        .iter()
                        .all(|(other, _)| other != id),
                    "duplicate {id}"
                );
            }
        }

        /// The renderer's whitelist (`MENU_ACTIONS` in src/hooks/useNativeMenu.ts) lists exactly
        /// the actions this menu can send, and the items it shows without keys are the plain ones.
        #[test]
        fn menu_actions_match_the_renderer_whitelist() {
            let ts = include_str!("../../src/hooks/useNativeMenu.ts");
            let list = |name: &str| -> Vec<String> {
                let start = ts.find(name).expect("the list exists");
                let body = &ts[start..];
                let body = &body[body.find("= [").expect("list start") + 3..];
                let body = &body[..body.find(']').expect("list end")];
                body.lines()
                    .map(str::trim)
                    .filter(|l| l.starts_with('"'))
                    .map(|l| l.trim_end_matches(',').trim_matches('"').to_string())
                    .collect()
            };
            let mut renderer = list("export const MENU_ACTIONS");
            let mut rust: Vec<String> = MENU_ACTIONS
                .iter()
                .map(|(id, _, _)| *id)
                .chain(PLAIN_MENU_ACTIONS.iter().map(|(id, _)| *id))
                .map(String::from)
                .collect();
            renderer.sort();
            rust.sort();
            assert_eq!(renderer, rust);
            for id in list("export const MENU_UNBOUND_ACTIONS") {
                assert!(
                    PLAIN_MENU_ACTIONS.iter().any(|(plain, _)| *plain == id),
                    "{id} must have no accelerator"
                );
            }
        }

        #[test]
        fn decides_exit_requests() {
            use ExitDecision::*;
            assert_eq!(exit_decision(false, 1, None), AskRenderer);
            assert_eq!(exit_decision(true, 1, None), Allow);
            assert_eq!(exit_decision(false, 0, None), Allow);
            assert_eq!(
                exit_decision(false, 1, Some(Duration::from_secs(1))),
                AskRenderer
            );
            assert_eq!(
                exit_decision(false, 1, Some(RENDERER_ANSWER_TIMEOUT)),
                Allow
            );
            assert_eq!(
                exit_decision(false, 2, Some(Duration::from_secs(60))),
                Allow
            );
        }

        #[test]
        fn tracks_the_oldest_unanswered_request() {
            let state = QuitState::default();
            let t0 = Instant::now();
            assert_eq!(state.pending_for(t0), None);
            assert!(!state.is_confirmed());
            state.mark_requested(t0);
            state.mark_requested(t0 + Duration::from_secs(3));
            assert_eq!(
                state.pending_for(t0 + Duration::from_secs(4)),
                Some(Duration::from_secs(4))
            );
            state.confirm();
            assert!(state.is_confirmed());
        }

        #[test]
        fn decides_system_terminate_requests() {
            use ExitDecision::*;
            // Nothing unsaved: quit right away (a logout or shutdown is not held up).
            assert_eq!(terminate_decision(false, 1, false, None), Allow);
            // Unsaved work: ask, unless confirmed, windowless or the renderer is hung.
            assert_eq!(terminate_decision(false, 1, true, None), AskRenderer);
            assert_eq!(
                terminate_decision(false, 1, true, Some(Duration::from_secs(1))),
                AskRenderer
            );
            assert_eq!(terminate_decision(true, 1, true, None), Allow);
            assert_eq!(terminate_decision(false, 0, true, None), Allow);
            assert_eq!(
                terminate_decision(false, 1, true, Some(RENDERER_ANSWER_TIMEOUT)),
                Allow
            );
        }

        #[test]
        fn a_renderer_answer_clears_the_pending_request() {
            // Dock Quit → the renderer shows the dialog (and answers) → the user cancels. A Dock
            // Quit minutes later must ask again, not slip through the hung-renderer timeout.
            let state = QuitState::default();
            let t0 = Instant::now();
            state.set_unsaved(true);
            assert!(state.has_unsaved());
            state.mark_requested(t0);
            state.set_unsaved(true);
            let later = t0 + Duration::from_secs(600);
            assert_eq!(state.pending_for(later), None);
            assert_eq!(
                terminate_decision(false, 1, state.has_unsaved(), state.pending_for(later)),
                ExitDecision::AskRenderer
            );
            // A renderer that never answers is let through after the timeout.
            state.mark_requested(later);
            assert_eq!(
                terminate_decision(
                    false,
                    1,
                    state.has_unsaved(),
                    state.pending_for(later + RENDERER_ANSWER_TIMEOUT)
                ),
                ExitDecision::Allow
            );
            state.set_unsaved(false);
            assert!(!state.has_unsaved());
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // One process-wide TLS crypto provider for russh, mysql_async and tokio-postgres-rustls.
    // Mixing providers panics at runtime, so install it before any connection is made.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    let log_level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };

    let app = tauri::Builder::default()
        .manage(shell::QuitState::default())
        .menu(shell::build_menu)
        .on_menu_event(|app, event| shell::on_menu_event(app, event.id().as_ref()))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_log::Builder::new().level(log_level).build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let db_path = dir.join("tabler.sqlite");
            let appdb = appdb::AppDb::open(&db_path)?;
            log::info!("app database at {}", db_path.display());
            let secrets = secrets::Secrets::from_env(appdb.clone());
            app.manage(state::AppState::new(appdb, secrets));
            #[cfg(target_os = "macos")]
            shell::terminate_guard::install(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::app_info,
            commands::app::quit_app,
            commands::app::set_unsaved,
            commands::profiles::list_profiles,
            commands::profiles::get_profile,
            commands::profiles::save_profile,
            commands::profiles::delete_profile,
            commands::profiles::duplicate_profile,
            commands::profiles::reorder_profiles,
            commands::profiles::get_secret_status,
            commands::profiles::test_connection,
            commands::profiles::create_sqlite_database,
            commands::profiles::move_profiles_to_group,
            commands::profiles::export_connections,
            commands::profiles::inspect_connections_file,
            commands::profiles::import_connections,
            commands::connections::connect,
            commands::connections::disconnect,
            commands::connections::list_connections,
            commands::connections::ping,
            commands::connections::switch_database,
            commands::schema::list_databases,
            commands::schema::list_schemas,
            commands::schema::list_tables,
            commands::schema::get_table,
            commands::schema::get_view_definition,
            commands::schema::get_completion_metadata,
            commands::schema::invalidate_schema_cache,
            commands::schema::get_structure_choices,
            commands::browse::browse_table,
            commands::browse::count_table_rows,
            commands::browse::fetch_cell,
            commands::mutate::preview_changes,
            commands::mutate::apply_changes,
            commands::ddl::preview_ddl,
            commands::ddl::apply_ddl,
            commands::ddl::get_table_ddl,
            commands::objects::list_routines,
            commands::objects::list_sequences,
            commands::objects::list_system_schemas,
            commands::objects::get_routine_definition,
            commands::objects::get_sequence_definition,
            commands::objects::get_item_overview,
            commands::objects::preview_object_op,
            commands::objects::apply_object_op,
            commands::query::run_sql,
            commands::query::cancel_query,
            commands::query::explain,
            commands::safety::verify_connection_password,
            commands::history::list_history,
            commands::history::delete_history,
            commands::history::clear_history,
            commands::saved_queries::list_saved_queries,
            commands::saved_queries::save_saved_query,
            commands::saved_queries::delete_saved_query,
            commands::saved_queries::save_query_folder,
            commands::saved_queries::delete_query_folder,
            commands::files::read_text_file,
            commands::files::write_text_file,
            commands::files::read_binary_file,
            commands::diagram::schema_relations,
            commands::diagram::write_binary_file,
            commands::settings::get_settings,
            commands::settings::set_setting,
            commands::menu::set_menu_accelerators,
            commands::export::export_data,
            commands::export::cancel_export,
            commands::export::export_tables,
            commands::import::preview_csv_import,
            commands::import::import_csv,
            commands::import::preview_sql_dump,
            commands::import::import_sql_dump,
            commands::import::cancel_import,
            commands::backup::backup_list_binaries,
            commands::backup::backup_inspect_target,
            commands::backup::backup_inspect_file,
            commands::backup::backup_run,
            commands::backup::backup_cancel,
            commands::backup::backup_copy_sqlite,
            commands::connections::accept_host_key,
            commands::connections::answer_ssh_prompt,
            commands::admin::admin_database_options,
            commands::admin::admin_preview_ddl,
            commands::admin::admin_apply_ddl,
            commands::admin::admin_list_processes,
            commands::admin::admin_kill_processes,
            commands::admin::admin_list_users,
            commands::admin::admin_get_user,
            commands::admin::admin_preview_user,
            commands::admin::admin_apply_user,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| match event {
        // Quit from the menu bar arrives as `app.quit` (a menu event), the red close button as the
        // window's close guard, Dock Quit / logout / shutdown as NSApp `terminate:`
        // (`shell::terminate_guard`); this catches exits requested any other way while a window
        // is open.
        RunEvent::ExitRequested { api, .. } => {
            if shell::decide_exit(app, shell::ExitSource::App) == shell::ExitDecision::AskRenderer {
                api.prevent_exit();
                let _ = app.emit(shell::EVENT_QUIT_REQUESTED, ());
            }
        }
        // Exports still running never get to drop their temporary files.
        RunEvent::Exit => {
            let removed = export::remove_partial_files();
            if removed > 0 {
                log::info!("removed {removed} partial export file(s) on exit");
            }
            // Dump / restore binaries still running are killed; partial backups removed.
            let removed = backup::shutdown();
            if removed > 0 {
                log::info!("removed {removed} partial backup file(s) on exit");
            }
        }
        _ => {}
    });
}
