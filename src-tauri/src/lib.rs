//! Tabler backend: Tauri commands are thin wrappers; logic lives in the modules below so it can be
//! unit-tested without a running app.

// `AppError::Db` carries the full server diagnostic; boxing it everywhere is not worth the noise.
#![allow(clippy::result_large_err)]

pub mod admin;
pub mod appdb;
pub mod assistant;
pub mod backup;
pub mod biometrics;
pub mod bootstrap_script;
pub mod commands;
pub mod conn_options;
pub mod deeplink;
pub mod diagram;
pub mod drivers;
pub mod error;
pub mod export;
pub mod geometry;
pub mod import;
pub mod local_infile;
pub mod locations;
pub mod mcp;
pub mod metrics;
pub mod objects;
pub mod passcode;
pub mod plugins;
pub mod process;
pub mod profiles_io;
pub mod query_files;
pub mod recent;
pub mod secrets;
pub mod sqlgen;
pub mod state;
#[cfg(test)]
pub mod testkit;
#[cfg(feature = "common-tls")]
pub mod tls_client;
pub mod tls_files;
pub mod tunnel;
pub mod types;
pub mod windows;

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

    /// `mac` on macOS, `other` elsewhere: off macOS the Ctrl key is CmdOrCtrl, so a ⌃ accelerator
    /// takes the renderer keymap's `nonMacKeys` instead (⌃⌘] → Ctrl+Alt+], ⌃- → Alt+Left).
    const fn mac_or(mac: &'static str, other: &'static str) -> &'static str {
        if cfg!(target_os = "macos") {
            mac
        } else {
            other
        }
    }

    /// Menu items that run a renderer action: (action id = menu item id, label, accelerator).
    const MENU_ACTIONS: &[(&str, &str, &str)] = &[
        ("settings.open", "Settings…", "CmdOrCtrl+,"),
        // File ▸ New Window (TablePlus ⌘N; New Connection… has no key).
        ("window.new", "New Window", "CmdOrCtrl+N"),
        ("tab.new-query", "New Query Tab", "CmdOrCtrl+T"),
        ("query.open", "Open SQL File…", "CmdOrCtrl+O"),
        ("tab.close", "Close Tab / Window", "CmdOrCtrl+W"),
        ("tab.close-all", "Close All Tabs", "CmdOrCtrl+Shift+W"),
        ("tab.reopen", "Reopen Closed Tab", "CmdOrCtrl+Shift+T"),
        ("export.open", "Export…", "CmdOrCtrl+Shift+E"),
        // File ▸ New SQL Viewer (TablePlus ⇧⌘O); the connection manager has no key.
        ("sqlviewer.new", "New SQL Viewer", "CmdOrCtrl+Shift+O"),
        ("palette.open", "Open Anything…", "CmdOrCtrl+P"),
        ("palette.databases", "Switch Database…", "CmdOrCtrl+K"),
        (
            "palette.connections",
            "Switch Connection…",
            "CmdOrCtrl+Shift+K",
        ),
        (
            "view.data",
            "Data",
            mac_or("CmdOrCtrl+Ctrl+[", "CmdOrCtrl+Alt+["),
        ),
        (
            "view.structure",
            "Structure",
            mac_or("CmdOrCtrl+Ctrl+]", "CmdOrCtrl+Alt+]"),
        ),
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
        ("workspace.reload", "Reload Workspace", "CmdOrCtrl+R"),
        ("view.refresh", "Refresh", "CmdOrCtrl+Alt+R"),
        ("connection.reconnect", "Reconnect", "CmdOrCtrl+Shift+R"),
        ("admin.process-list", "Process List…", "CmdOrCtrl+."),
        ("workspace.show", "Show Workspaces", "CmdOrCtrl+Shift+\\"),
        (
            "workspace.next",
            "Select Next Workspace",
            mac_or("CmdOrCtrl+Ctrl+Right", "CmdOrCtrl+Alt+Right"),
        ),
        (
            "workspace.prev",
            "Select Previous Workspace",
            mac_or("CmdOrCtrl+Ctrl+Left", "CmdOrCtrl+Alt+Left"),
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
        ("tab.back", "Back", mac_or("Ctrl+-", "Alt+Left")),
        (
            "tab.forward",
            "Forward",
            mac_or("Ctrl+Shift+-", "Alt+Right"),
        ),
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
        // Window: the app lock (Settings ▸ Security ▸ Passcode).
        (
            "app.lock",
            "Lock Tabler",
            mac_or("CmdOrCtrl+Ctrl+L", "CmdOrCtrl+Alt+L"),
        ),
        // Plugins ▸ Manage Plugins… (TablePlus ⌘L).
        ("plugins.manage", "Manage Plugins…", "CmdOrCtrl+L"),
    ];

    /// Menu items without a keyboard shortcut (TablePlus gives them none): (action id, label).
    /// Also the items whose keys are context-dependent and belong to another menu item here
    /// (⌘I Beautify / Insert Row, ⇧⌘D Split Pane / Split Right, ⇧⌘C Copy Cell Value / the console
    /// log, ⌘D the editor's next occurrence): the page resolves those keys by focus
    /// (`MENU_UNBOUND_ACTIONS` in useNativeMenu.ts).
    const PLAIN_MENU_ACTIONS: &[(&str, &str)] = &[
        ("connection.new", "New Connection…"),
        // File ▸ New Workspace (the connection picker); File ▸ Connection Manager.
        ("workspace.new", "New Workspace"),
        ("ui.launcher", "Connection Manager"),
        // Edit ▸ Copy Cell Value: ⇧⌘C in a grid (the page decides by focus; the console log has
        // the menu's ⇧⌘C everywhere else).
        ("grid.copy-cell", "Copy Cell Value"),
        ("window.move-tab", "Move Tab to New Window"),
        // ⇧⌘D outside the SQL editor (inside it the same key is Split Pane); no accelerator here
        // because Split Pane has it.
        ("workspace.split-right", "Split Right"),
        ("backup.open", "Backup…"),
        ("restore.open", "Restore…"),
        ("admin.users", "User Management…"),
        ("metrics.open", "Metrics Board"),
        ("connection.disconnect", "Disconnect"),
        ("connection.encoding", "View Using Encoding…"),
        ("import.csv", "From CSV…"),
        ("import.json", "From JSON…"),
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
    /// Elsewhere there is no app menu: Settings and Exit end File, About ends Help.
    pub fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
        let name = app.package_info().name.clone();
        let quit_label = if cfg!(target_os = "macos") {
            format!("Quit {name}")
        } else {
            "Exit".to_owned()
        };
        let quit = MenuItemBuilder::with_id(QUIT_ITEM_ID, quit_label)
            .accelerator("CmdOrCtrl+Q")
            .build(app)?;
        #[cfg(target_os = "macos")]
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
            .item(&action_item(app, "window.new")?)
            .item(&action_item(app, "connection.new")?)
            .item(&action_item(app, "workspace.new")?)
            .item(&action_item(app, "sqlviewer.new")?)
            .item(&action_item(app, "workspace.reload")?)
            .item(&action_item(app, "tab.new-query")?)
            .item(&action_item(app, "query.open")?)
            .item(&crate::recent::submenu(app)?)
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
                    .item(&action_item(app, "import.json")?)
                    .item(&action_item(app, "import.sql")?)
                    .build()?,
            )
            .item(&action_item(app, "ui.launcher")?);
        #[cfg(not(target_os = "macos"))]
        let file = file
            .separator()
            .item(&action_item(app, "settings.open")?)
            .separator()
            .item(&quit);
        let file = file.build()?;
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
            .item(&action_item(app, "grid.copy-cell")?)
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
            .item(&action_item(app, "workspace.split-right")?)
            .item(&action_item(app, "metrics.open")?)
            .separator()
            .item(&action_item(app, "view.refresh")?)
            .separator()
            .item(&PredefinedMenuItem::fullscreen(app, None)?)
            .build()?;
        // TablePlus's Connection menu.
        let connection = SubmenuBuilder::new(app, "Connection")
            .item(&action_item(app, "connection.encoding")?)
            .separator()
            .item(&action_item(app, "admin.process-list")?)
            .item(&action_item(app, "admin.users")?)
            .separator()
            .item(&action_item(app, "connection.reconnect")?)
            .item(&action_item(app, "connection.disconnect")?)
            .build()?;
        // Plugins ▸ Manage Plugins…, the enabled plugins' commands, Open Plugins Folder.
        let plugins = crate::plugins::menu::build(app, &action_item(app, "plugins.manage")?)?;
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
            .separator()
            .item(&action_item(app, "window.move-tab")?)
            .separator()
            .item(&action_item(app, "app.lock")?)
            .build()?;
        // Help ▸ Enable SSH Debug Log / Show Log File (handled natively, see `on_menu_event`).
        let help = crate::tunnel::debug_log::help_menu(app)?;
        #[cfg(not(target_os = "macos"))]
        {
            // Without metadata the predefined About item does nothing outside macOS.
            let about = tauri::menu::AboutMetadataBuilder::new()
                .name(Some(name.clone()))
                .version(Some(app.package_info().version.to_string()))
                .build();
            help.append(&PredefinedMenuItem::separator(app)?)?;
            help.append(&PredefinedMenuItem::about(
                app,
                Some(&format!("About {name}")),
                Some(about),
            )?)?;
        }
        MenuBuilder::new(app)
            .items(&[
                #[cfg(target_os = "macos")]
                &app_menu,
                &file,
                &edit,
                &view,
                &connection,
                &plugins,
                &navigate,
                &window,
                &help,
            ])
            .build()
    }

    /// Forwards a chosen menu item to the renderer.
    pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
        if crate::tunnel::debug_log::on_menu_event(app, id) {
            return;
        }
        if crate::plugins::menu::on_menu_event(app, id) {
            return;
        }
        // File ▸ Open Recent's entries (and Clear Menu).
        if crate::recent::on_menu_event(app, id) {
            return;
        }
        if id == QUIT_ITEM_ID {
            let _ = app.emit(EVENT_QUIT_REQUESTED, ());
        } else if let Some(action) = menu_action(id) {
            // Only the window the menu acts on (the focused one) runs it.
            crate::windows::emit_to_target(app, EVENT_MENU_ACTION, action);
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

    /// Runs `fut` on Tauri's runtime from a thread outside it (the main thread in `RunEvent::Exit`)
    /// and gives up after `limit` (`None`). The timer is created inside `block_on`:
    /// `tokio::time::timeout` needs a runtime when it is *created*, so
    /// `block_on(tokio::time::timeout(..))` panics on the main thread, and the release profile
    /// (`panic = "abort"`) turns that into a crash on every quit.
    pub fn block_on_for<F: std::future::Future>(limit: Duration, fut: F) -> Option<F::Output> {
        tauri::async_runtime::block_on(async move { tokio::time::timeout(limit, fut).await.ok() })
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
        fn bounded_block_on_runs_from_a_thread_outside_the_runtime() {
            // A plain #[test] thread is outside any runtime, like the main thread at exit.
            assert_eq!(block_on_for(Duration::from_secs(1), async { 7 }), Some(7));
            let never = std::future::pending::<()>();
            assert_eq!(block_on_for(Duration::from_millis(20), never), None);
        }

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
            // Off macOS Ctrl is CmdOrCtrl: an accelerator with Ctrl would be a second CmdOrCtrl
            // (Show Structure's ⌃⌘] read as Next Tab's Ctrl+]).
            if !cfg!(target_os = "macos") {
                for (id, _, accel) in MENU_ACTIONS {
                    assert!(!accel.split('+').any(|p| p == "Ctrl"), "{id}: {accel}");
                }
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
        .manage(windows::WindowRegistry::default())
        .on_window_event(windows::on_window_event)
        .menu(shell::build_menu)
        .on_menu_event(|app, event| shell::on_menu_event(app, event.id().as_ref()))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            // Not the decorations: the app sets those itself (off on Windows, `own_title_bar`),
            // and a saved "undecorated" would leave a build without its own window buttons with
            // none at all.
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        - tauri_plugin_window_state::StateFlags::DECORATIONS,
                )
                .build(),
        )
        .plugin(
            // The SSH client's debug records go to the log while Help ▸ Enable SSH Debug Log is on.
            tunnel::debug_log::configure(tauri_plugin_log::Builder::new().level(log_level)).build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            // Settings ▸ Locations may put the app DB elsewhere (a pointer file in `dir`).
            let (appdb, locations) = locations::open_app_db(dir)?;
            log::info!("app database at {}", locations.data_file().display());
            let secrets = secrets::Secrets::from_env(appdb.clone());
            app.manage(state::AppState::new(appdb, secrets));
            // MySQL LOAD DATA LOCAL INFILE: the user allows each local file the server asks for.
            local_infile::install_dialog_approver(app.handle().clone());
            app.manage(locations);
            // Help ▸ Enable SSH Debug Log as last chosen; URLs / files from the command line.
            tunnel::debug_log::load(app.handle(), &app.state::<state::AppState>().appdb);
            deeplink::startup(app.handle());
            // File ▸ Open Recent from the saved list.
            recent::refresh_menu(app.handle());
            #[cfg(target_os = "macos")]
            shell::terminate_guard::install(app.handle());
            // The main window (built from tauri.conf.json and restored by the window-state
            // plugin before `setup`) back inside the screen.
            #[cfg(windows)]
            if let Some(main) = app.get_webview_window(windows::MAIN_WINDOW)
                && let Err(e) =
                    windows::own_title_bar(&main).and_then(|()| windows::fit_to_work_area(&main))
            {
                log::warn!("could not set up the main window: {e}");
            }
            // The app lock: sleep and screen lock lock every window.
            biometrics::watch_system_lock(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::app_info,
            commands::deeplink::take_pending_opens,
            commands::deeplink::mark_temporary_profile,
            commands::deeplink::list_temporary_profiles,
            commands::sqlite_ssh::sqlite_ssh_upload,
            commands::app::quit_app,
            commands::app::set_unsaved,
            commands::app::cancel_quit,
            commands::app::window_open,
            commands::app::window_take_handoff,
            commands::app::window_set_connections,
            commands::app::window_label,
            commands::app::window_is_session_owner,
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
            commands::conn_extras::get_connection_encoding,
            commands::conn_extras::set_connection_encoding,
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
            commands::browse::drain_sql_trace,
            commands::mutate::preview_changes,
            commands::mutate::apply_changes,
            commands::workspace_commit::preview_workspace_changes,
            commands::workspace_commit::apply_workspace_changes,
            commands::ddl::preview_ddl,
            commands::ddl::apply_ddl,
            commands::ddl::get_table_ddl,
            commands::objects::list_routines,
            commands::objects::list_sequences,
            commands::objects::list_system_schemas,
            commands::objects::list_database_items,
            commands::objects::get_routine_definition,
            commands::objects::get_sequence_definition,
            commands::objects::get_item_overview,
            commands::objects::preview_object_op,
            commands::objects::apply_object_op,
            commands::query::run_sql,
            commands::query::cancel_query,
            commands::query::explain,
            commands::native::session_call,
            commands::native::driver_util,
            commands::safety::verify_connection_password,
            commands::history::list_history,
            commands::history::delete_history,
            commands::history::clear_history,
            commands::history::clear_history_before,
            commands::saved_queries::list_saved_queries,
            commands::saved_queries::save_saved_query,
            commands::saved_queries::delete_saved_query,
            commands::saved_queries::save_query_folder,
            commands::saved_queries::delete_query_folder,
            commands::saved_queries::import_saved_queries,
            commands::saved_queries::add_query_folder_from_disk,
            commands::saved_queries::reveal_saved_query,
            commands::locations::get_locations,
            commands::locations::inspect_location,
            commands::locations::set_data_location,
            commands::locations::set_queries_location,
            commands::locations::reveal_location,
            commands::locations::restart_app,
            commands::mcp::mcp_info,
            commands::mcp::mcp_install_cli,
            commands::mcp::mcp_preview_config,
            commands::mcp::mcp_write_config,
            commands::sql_files::read_sql_file,
            commands::sql_files::sql_file_info,
            commands::sql_files::read_sql_file_head,
            commands::sql_files::write_sql_file,
            commands::sql_files::saved_query_path,
            commands::files::read_text_file,
            commands::files::write_text_file,
            commands::files::read_binary_file,
            commands::diagram::schema_relations,
            commands::diagram::write_binary_file,
            commands::settings::get_settings,
            commands::settings::set_setting,
            commands::decoder::set_geometry_wkt,
            commands::menu::set_menu_accelerators,
            commands::menu::popup_app_menu,
            commands::recent::recent_list,
            commands::recent::recent_add,
            commands::recent::recent_clear,
            commands::recent::recent_refresh_menu,
            commands::export::export_data,
            commands::export::cancel_export,
            commands::export::export_tables,
            commands::export::export_results,
            commands::export::export_database_tables,
            commands::import::preview_csv_import,
            commands::import::import_csv,
            commands::import::preview_sql_dump,
            commands::import::import_sql_dump,
            commands::import::preview_json_import,
            commands::import::import_json,
            commands::import::cancel_import,
            commands::backup::backup_list_binaries,
            commands::backup::backup_inspect_target,
            commands::backup::backup_inspect_file,
            commands::backup::backup_run,
            commands::backup::backup_cancel,
            commands::backup::backup_copy_sqlite,
            commands::backup::backup_install_with_brew,
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
            commands::lock::authenticate_biometric,
            commands::lock::passcode_status,
            commands::lock::set_passcode,
            commands::lock::verify_passcode,
            commands::lock::lock_app,
            commands::lock::reveal_connection_password,
            commands::lock::session_details,
            commands::metrics::metrics_list_boards,
            commands::metrics::metrics_save_board,
            commands::metrics::metrics_delete_board,
            commands::metrics::metrics_export_boards,
            commands::metrics::metrics_import_boards,
            commands::metrics::metrics_run,
            commands::plugins::plugins_list,
            commands::plugins::plugins_install,
            commands::plugins::plugins_remove,
            commands::plugins::plugins_read_script,
            commands::plugins::plugins_reveal_folder,
            commands::plugins::plugins_set_menu,
            commands::assistant::assistant_chat,
            commands::assistant::assistant_cancel,
            commands::assistant::assistant_context,
            commands::assistant::assistant_key_status,
            commands::assistant::assistant_set_key,
            commands::assistant::assistant_test,
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
            // Every open session is closed (`app.exit()` runs no window handlers): a SQLite file
            // in WAL mode is checkpointed and loses its -wal / -shm files. Before the SQLite over
            // SSH uploads below, which send the main file of the local copy.
            if let Some(state) = app.try_state::<state::AppState>()
                && shell::block_on_for(
                    std::time::Duration::from_secs(3),
                    state.close_all_sessions(),
                )
                .is_none()
            {
                log::warn!("closing the open sessions at exit timed out");
            }
            // SQLite over SSH: the last changes go back to the server.
            commands::sqlite_ssh::shutdown();
        }
        // Finder (double-clicked files), `open -a Tabler "<url>"` and links to Tabler's schemes.
        #[cfg(target_os = "macos")]
        RunEvent::Opened { urls } => deeplink::on_opened(app, &urls),
        _ => {}
    });
}
