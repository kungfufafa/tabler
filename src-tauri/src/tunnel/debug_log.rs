//! The Help menu: Help ▸ Changelog and Report a Bug / Send Feedback… (open the project's releases
//! and issue tracker in the browser), Help ▸ Enable SSH Debug Log and Help ▸ Show Log File
//! (TablePlus's troubleshooting items).
//!
//! The app log (tauri-plugin-log, `~/Library/Logs/<identifier>/Tabler.log`) normally keeps the
//! SSH client (russh) and the tunnel at Info. With the debug log on, their Debug records are
//! written too: key exchange, authentication methods tried, channel opens and forwarding. The
//! switch is remembered in the app settings (`sshDebugLog`) and applies at once.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{CheckMenuItemBuilder, MenuItemBuilder, MenuItemKind, Submenu, SubmenuBuilder};
use tauri::{AppHandle, Manager, Runtime};

use crate::appdb::AppDb;

/// Menu item ids (handled here, not by the renderer's action registry).
pub const MENU_SSH_DEBUG: &str = "help.ssh-debug-log";
pub const MENU_SHOW_LOG: &str = "help.show-log-file";
pub const MENU_CHANGELOG: &str = "help.changelog";
pub const MENU_REPORT_BUG: &str = "help.report-bug";

/// Help items that open a web page: (menu id, label, URL). The releases page is the changelog.
pub const HELP_LINKS: [(&str, &str, &str); 2] = [
    (
        MENU_CHANGELOG,
        "Changelog",
        "https://github.com/kungfufafa/tabler/releases",
    ),
    (
        MENU_REPORT_BUG,
        "Report a Bug / Send Feedback\u{2026}",
        "https://github.com/kungfufafa/tabler/issues/new",
    ),
];

/// The page a Help menu item opens (`None`: not a link item).
pub fn help_link_url(id: &str) -> Option<&'static str> {
    HELP_LINKS
        .iter()
        .find(|(link, _, _)| *link == id)
        .map(|(_, _, url)| *url)
}
/// App settings key of the switch.
pub const SETTING_KEY: &str = "sshDebugLog";

/// Log targets of the SSH client and the tunnel.
const SSH_TARGETS: [&str; 3] = ["russh", "russh_sftp", "tabler_lib::tunnel"];

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    let was = ENABLED.swap(on, Ordering::Relaxed);
    if was != on {
        log::info!("SSH debug log {}", if on { "enabled" } else { "disabled" });
    }
}

fn is_ssh_target(target: &str) -> bool {
    SSH_TARGETS.iter().any(|t| {
        target == *t
            || target
                .strip_prefix(t)
                .is_some_and(|rest| rest.starts_with("::"))
    })
}

/// Whether a log record passes: SSH Debug / Trace records only while the debug log is on.
pub fn allows(metadata: &log::Metadata<'_>) -> bool {
    metadata.level() <= log::Level::Info || !is_ssh_target(metadata.target()) || enabled()
}

/// Adds the SSH targets at Debug level (filtered by [`allows`]) to the app's log builder.
pub fn configure(builder: tauri_plugin_log::Builder) -> tauri_plugin_log::Builder {
    SSH_TARGETS
        .iter()
        .fold(builder, |b, t| b.level_for(*t, log::LevelFilter::Debug))
        .filter(allows)
}

/// Reads the saved switch (call once at startup) and ticks the menu item.
pub fn load<R: Runtime>(app: &AppHandle<R>, appdb: &AppDb) {
    let on = appdb
        .get_setting(SETTING_KEY)
        .ok()
        .flatten()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    set_enabled(on);
    sync_menu(app);
}

/// The app's log file (`<app log dir>/<product name>.log`).
pub fn log_file_path<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    let dir = app.path().app_log_dir().ok()?;
    Some(dir.join(format!("{}.log", app.package_info().name)))
}

/// Help ▸ Changelog, Enable SSH Debug Log, Show Log File, Report a Bug / Send Feedback…
/// (macOS adds its search field on top).
pub fn help_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Submenu<R>> {
    let debug = CheckMenuItemBuilder::with_id(MENU_SSH_DEBUG, "Enable SSH Debug Log")
        .checked(enabled())
        .build(app)?;
    let show = MenuItemBuilder::with_id(MENU_SHOW_LOG, "Show Log File").build(app)?;
    let [
        (changelog_id, changelog_label, _),
        (report_id, report_label, _),
    ] = HELP_LINKS;
    let changelog = MenuItemBuilder::with_id(changelog_id, changelog_label).build(app)?;
    let report = MenuItemBuilder::with_id(report_id, report_label).build(app)?;
    let help = SubmenuBuilder::new(app, "Help")
        .item(&changelog)
        .separator()
        .item(&debug)
        .item(&show)
        .separator()
        .item(&report)
        .build()?;
    #[cfg(target_os = "macos")]
    let _ = help.set_as_help_menu_for_nsapp();
    Ok(help)
}

fn find_check<R: Runtime>(
    items: Vec<MenuItemKind<R>>,
    id: &str,
) -> Option<tauri::menu::CheckMenuItem<R>> {
    for item in items {
        if item.id() == id
            && let Some(check) = item.as_check_menuitem()
        {
            return Some(check.clone());
        }
        if let Some(sub) = item.as_submenu()
            && let Some(found) = sub.items().ok().and_then(|i| find_check(i, id))
        {
            return Some(found);
        }
    }
    None
}

fn sync_menu<R: Runtime>(app: &AppHandle<R>) {
    let item = app
        .menu()
        .and_then(|m| m.items().ok())
        .and_then(|items| find_check(items, MENU_SSH_DEBUG));
    if let Some(item) = item {
        let _ = item.set_checked(enabled());
    }
}

/// Turns the debug log on or off, saves the choice and ticks the menu item.
pub fn set_and_save<R: Runtime>(app: &AppHandle<R>, on: bool) {
    set_enabled(on);
    if let Some(state) = app.try_state::<crate::state::AppState>()
        && let Err(e) = state
            .appdb
            .set_setting(SETTING_KEY, &serde_json::Value::Bool(on))
    {
        log::warn!("could not save the SSH debug log switch: {e}");
    }
    sync_menu(app);
}

/// Reveals the log file in Finder (its folder when the file does not exist yet).
pub fn reveal_log_file<R: Runtime>(app: &AppHandle<R>) {
    use tauri_plugin_opener::OpenerExt;
    let Some(path) = log_file_path(app) else {
        log::warn!("no log directory");
        return;
    };
    let result = if path.exists() {
        app.opener().reveal_item_in_dir(&path)
    } else {
        let dir = path.parent().map(PathBuf::from).unwrap_or_default();
        app.opener().open_path(dir.to_string_lossy(), None::<&str>)
    };
    if let Err(e) = result {
        log::warn!("could not show the log file {}: {e}", path.display());
    }
}

/// Opens a Help link in the default browser.
fn open_help_link<R: Runtime>(app: &AppHandle<R>, url: &str) {
    use tauri_plugin_opener::OpenerExt;
    if let Err(e) = app.opener().open_url(url, None::<&str>) {
        log::warn!("could not open {url}: {e}");
    }
}

/// Handles the Help menu's items; false for any other id.
pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) -> bool {
    if let Some(url) = help_link_url(id) {
        open_help_link(app, url);
        return true;
    }
    match id {
        MENU_SSH_DEBUG => {
            // The check item already flipped its own tick; the switch follows it.
            set_and_save(app, !enabled());
            true
        }
        MENU_SHOW_LOG => {
            reveal_log_file(app);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(level: log::Level, target: &str) -> log::Metadata<'_> {
        log::Metadata::builder().level(level).target(target).build()
    }

    #[test]
    fn ssh_debug_records_follow_the_switch() {
        set_enabled(false);
        assert!(!allows(&meta(log::Level::Debug, "russh::client")));
        assert!(!allows(&meta(log::Level::Trace, "russh")));
        assert!(!allows(&meta(
            log::Level::Debug,
            "tabler_lib::tunnel::peer"
        )));
        // Info and above always pass; other targets are not affected.
        assert!(allows(&meta(log::Level::Info, "russh::client")));
        assert!(allows(&meta(log::Level::Warn, "tabler_lib::tunnel")));
        assert!(allows(&meta(log::Level::Debug, "tabler_lib::drivers")));
        assert!(allows(&meta(log::Level::Debug, "russhx")));
        set_enabled(true);
        assert!(allows(&meta(log::Level::Debug, "russh::client")));
        assert!(allows(&meta(log::Level::Debug, "russh_sftp::client")));
        set_enabled(false);
    }

    #[test]
    fn the_help_menu_has_a_changelog_and_a_bug_report_link() {
        let ids: Vec<&str> = HELP_LINKS.iter().map(|(id, _, _)| *id).collect();
        assert_eq!(ids, ["help.changelog", "help.report-bug"]);
        for (id, label, url) in HELP_LINKS {
            assert!(
                url.starts_with("https://github.com/kungfufafa/tabler"),
                "{id}: {url}"
            );
            assert!(!label.is_empty());
            assert_eq!(help_link_url(id), Some(url));
        }
        assert_eq!(
            help_link_url(MENU_CHANGELOG),
            Some("https://github.com/kungfufafa/tabler/releases")
        );
        assert_eq!(
            help_link_url(MENU_REPORT_BUG),
            Some("https://github.com/kungfufafa/tabler/issues/new")
        );
        // The other Help items are not links (they have their own handlers).
        assert_eq!(help_link_url(MENU_SSH_DEBUG), None);
        assert_eq!(help_link_url(MENU_SHOW_LOG), None);
        assert_eq!(help_link_url("help.unknown"), None);
        // Ids are unique across the menu.
        let all = [
            MENU_SSH_DEBUG,
            MENU_SHOW_LOG,
            MENU_CHANGELOG,
            MENU_REPORT_BUG,
        ];
        let unique: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(unique.len(), all.len());
    }
}
