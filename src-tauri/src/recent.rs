//! File ▸ Open Recent (TablePlus): the saved connections and `.sql` files opened last, at most
//! `MAX_RECENT` of each, newest first. The list is kept in the app database's settings
//! (`SETTINGS_KEY`), shared by every window, and the native submenu (`SUBMENU_ID`) is rebuilt from
//! it whenever it changes. Choosing an entry sends its menu item id to the focused window's
//! renderer (`menu:action`: `recent.connection:<profile id>` / `recent.file:<path>`), which opens
//! it; Clear Menu is handled here.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::menu::{MenuItemBuilder, MenuItemKind, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, Runtime};
use ts_rs::TS;
use uuid::Uuid;

use crate::appdb::AppDb;
use crate::error::AppError;
use crate::types::profile::ConnectionProfile;

/// Entries kept per list (TablePlus / macOS keep ten).
pub const MAX_RECENT: usize = 10;
/// The settings key holding `RecentItems`.
pub const SETTINGS_KEY: &str = "recentItems";
/// File ▸ Open Recent's submenu id (its items are replaced on every change).
pub const SUBMENU_ID: &str = "file.open-recent";
/// Open Recent ▸ Clear Menu.
pub const CLEAR_ID: &str = "recent.clear";
/// Menu item ids of the entries: the prefix, then the profile id or the file's path.
pub const CONNECTION_PREFIX: &str = "recent.connection:";
pub const FILE_PREFIX: &str = "recent.file:";
/// Longest file path remembered (anything longer is refused).
const MAX_PATH_LEN: usize = 4096;

/// What File ▸ Open Recent lists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct RecentItems {
    /// Saved connections (profile ids), newest first.
    pub connections: Vec<Uuid>,
    /// `.sql` (`.sql.gz`) files, absolute paths, newest first.
    pub files: Vec<String>,
}

/// Something that was just opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum RecentEntry {
    /// A saved connection was opened (connected).
    Connection { profile_id: Uuid },
    /// An `.sql` file was opened or saved.
    File { path: String },
}

impl RecentItems {
    /// Moves `entry` to the front of its list, keeping at most `MAX_RECENT`.
    pub fn push(&mut self, entry: &RecentEntry) -> Result<(), AppError> {
        match entry {
            RecentEntry::Connection { profile_id } => {
                front(&mut self.connections, *profile_id);
            }
            RecentEntry::File { path } => {
                let path = path.trim();
                if path.is_empty() || path.len() > MAX_PATH_LEN || !Path::new(path).is_absolute() {
                    return Err(AppError::InvalidInput(format!(
                        "not an absolute file path: {path:?}"
                    )));
                }
                front(&mut self.files, path.to_string());
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.connections.is_empty() && self.files.is_empty()
    }
}

fn front<T: PartialEq>(list: &mut Vec<T>, value: T) {
    list.retain(|v| *v != value);
    list.insert(0, value);
    list.truncate(MAX_RECENT);
}

/// One Open Recent row: (menu item id, label).
pub type MenuRow = (String, String);

/// Last path segment ("/a/b/report.sql" → "report.sql").
fn file_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

/// The rows of the submenu: connections that still exist (labelled with their current name), then
/// files (their name, plus the folder when two share a name, as macOS does).
pub fn menu_rows(
    items: &RecentItems,
    profiles: &[ConnectionProfile],
) -> (Vec<MenuRow>, Vec<MenuRow>) {
    let connections = items
        .connections
        .iter()
        .filter_map(|id| profiles.iter().find(|p| p.id == *id))
        .map(|p| {
            let name = p.name.trim();
            let label = if name.is_empty() {
                "Untitled connection"
            } else {
                name
            };
            (format!("{CONNECTION_PREFIX}{}", p.id), label.to_string())
        })
        .collect();
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for path in &items.files {
        *seen.entry(file_name(path)).or_default() += 1;
    }
    let files = items
        .files
        .iter()
        .map(|path| {
            let name = file_name(path);
            let label = if seen.get(name).copied().unwrap_or(0) > 1 {
                let folder = Path::new(path)
                    .parent()
                    .and_then(|d| d.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("/");
                format!("{name} — {folder}")
            } else {
                name.to_string()
            };
            (format!("{FILE_PREFIX}{path}"), label)
        })
        .collect();
    (connections, files)
}

/// Serializes the read-modify-write of the stored list (two windows may add at once).
static STORE_LOCK: Mutex<()> = Mutex::new(());

/// The stored list (empty when missing or unreadable).
pub fn load(db: &AppDb) -> RecentItems {
    db.get_setting(SETTINGS_KEY)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// Changes the stored list with `change` and saves it. Returns the new list.
pub fn update(
    db: &AppDb,
    change: impl FnOnce(&mut RecentItems) -> Result<(), AppError>,
) -> Result<RecentItems, AppError> {
    let _guard = STORE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut items = load(db);
    change(&mut items)?;
    let value = serde_json::to_value(&items).map_err(|e| AppError::internal(e.to_string()))?;
    db.set_setting(SETTINGS_KEY, &value)?;
    Ok(items)
}

/// File ▸ Open Recent, empty (filled by `refresh_menu` once the app database is open).
pub fn submenu<R: Runtime, M: Manager<R>>(manager: &M) -> tauri::Result<Submenu<R>> {
    let menu = Submenu::with_id(manager, SUBMENU_ID, "Open Recent", true)?;
    fill(manager, &menu, &[], &[])?;
    Ok(menu)
}

/// Replaces the submenu's items: connections, files, then Clear Menu (disabled when empty).
fn fill<R: Runtime, M: Manager<R>>(
    manager: &M,
    menu: &Submenu<R>,
    connections: &[MenuRow],
    files: &[MenuRow],
) -> tauri::Result<()> {
    for _ in 0..menu.items()?.len() {
        menu.remove_at(0)?;
    }
    for (id, label) in connections.iter().chain(files) {
        if files.first().is_some_and(|(first, _)| first == id) && !connections.is_empty() {
            menu.append(&PredefinedMenuItem::separator(manager)?)?;
        }
        menu.append(&MenuItemBuilder::with_id(id.as_str(), label).build(manager)?)?;
    }
    if !connections.is_empty() || !files.is_empty() {
        menu.append(&PredefinedMenuItem::separator(manager)?)?;
    }
    let clear = MenuItemBuilder::with_id(CLEAR_ID, "Clear Menu")
        .enabled(!connections.is_empty() || !files.is_empty())
        .build(manager)?;
    menu.append(&clear)
}

/// The submenu with this id among `items`, nested submenus included.
fn find_submenu<R: Runtime>(items: Vec<MenuItemKind<R>>, id: &str) -> Option<Submenu<R>> {
    for entry in items {
        if let MenuItemKind::Submenu(sub) = entry {
            if sub.id() == id {
                return Some(sub);
            }
            if let Some(found) = find_submenu(sub.items().unwrap_or_default(), id) {
                return Some(found);
            }
        }
    }
    None
}

/// Rebuilds File ▸ Open Recent from the stored list and the saved connections' current names.
pub fn refresh_menu<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<crate::state::AppState>() else {
        return;
    };
    let items = load(&state.appdb);
    let profiles = state.appdb.list_profiles().unwrap_or_default();
    let (connections, files) = menu_rows(&items, &profiles);
    let Some(menu) = app.menu() else {
        return;
    };
    let Some(submenu) = find_submenu(menu.items().unwrap_or_default(), SUBMENU_ID) else {
        return;
    };
    if let Err(e) = fill(app, &submenu, &connections, &files) {
        log::warn!("could not rebuild File ▸ Open Recent: {e}");
    }
}

/// Handles Open Recent's items; false for any other id. An entry goes to the focused window,
/// which opens it; Clear Menu empties the list here.
pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) -> bool {
    if id == CLEAR_ID {
        if let Some(state) = app.try_state::<crate::state::AppState>()
            && let Err(e) = update(&state.appdb, |items| {
                *items = RecentItems::default();
                Ok(())
            })
        {
            log::warn!("could not clear the recent items: {e}");
        }
        refresh_menu(app);
        return true;
    }
    if id.starts_with(CONNECTION_PREFIX) || id.starts_with(FILE_PREFIX) {
        crate::windows::emit_to_target(app, crate::shell::EVENT_MENU_ACTION, id.to_string());
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: Uuid, name: &str) -> ConnectionProfile {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": name,
            "driver": "postgres",
        }))
        .expect("a minimal profile")
    }

    fn file(path: &str) -> RecentEntry {
        RecentEntry::File { path: path.into() }
    }

    /// `path` as an absolute path of this platform: unchanged, or on drive C: on Windows.
    fn abs(path: &str) -> String {
        if cfg!(windows) {
            format!("C:{}", path.replace('/', "\\"))
        } else {
            path.to_string()
        }
    }

    #[test]
    fn keeps_the_newest_ten_of_each_without_duplicates() {
        let mut items = RecentItems::default();
        let ids: Vec<Uuid> = (0..12).map(|_| Uuid::new_v4()).collect();
        for id in &ids {
            items
                .push(&RecentEntry::Connection { profile_id: *id })
                .unwrap();
        }
        assert_eq!(items.connections.len(), MAX_RECENT);
        assert_eq!(items.connections[0], ids[11]);
        // Opening one again moves it to the front instead of listing it twice.
        items
            .push(&RecentEntry::Connection { profile_id: ids[5] })
            .unwrap();
        assert_eq!(items.connections[0], ids[5]);
        assert_eq!(items.connections.len(), MAX_RECENT);
        assert_eq!(
            items.connections.iter().filter(|id| **id == ids[5]).count(),
            1
        );
        items.push(&file(&abs("/a/one.sql"))).unwrap();
        items.push(&file(&abs("/a/two.sql"))).unwrap();
        items
            .push(&file(&format!(" {} ", abs("/a/one.sql"))))
            .unwrap();
        assert_eq!(items.files, vec![abs("/a/one.sql"), abs("/a/two.sql")]);
    }

    #[test]
    fn refuses_relative_or_empty_paths() {
        let mut items = RecentItems::default();
        assert!(items.push(&file("")).is_err());
        assert!(items.push(&file("queries/x.sql")).is_err());
        assert!(
            items
                .push(&file(&format!("/{}", "a".repeat(5000))))
                .is_err()
        );
        assert!(items.is_empty());
    }

    #[test]
    fn labels_rows_with_current_names_and_drops_deleted_connections() {
        let (a, gone) = (Uuid::new_v4(), Uuid::new_v4());
        let items = RecentItems {
            connections: vec![gone, a],
            files: vec![
                "/x/report.sql".into(),
                "/y/report.sql".into(),
                "/z/users.sql.gz".into(),
            ],
        };
        let (connections, files) = menu_rows(&items, &[profile(a, "Production")]);
        assert_eq!(
            connections,
            vec![(format!("recent.connection:{a}"), "Production".to_string())]
        );
        assert_eq!(
            files,
            vec![
                (
                    "recent.file:/x/report.sql".to_string(),
                    "report.sql — x".to_string()
                ),
                (
                    "recent.file:/y/report.sql".to_string(),
                    "report.sql — y".to_string()
                ),
                (
                    "recent.file:/z/users.sql.gz".to_string(),
                    "users.sql.gz".to_string()
                ),
            ]
        );
    }

    #[test]
    fn stores_the_list_in_the_settings() {
        let db = AppDb::open_in_memory().unwrap();
        assert!(load(&db).is_empty());
        let id = Uuid::new_v4();
        update(&db, |items| {
            items.push(&RecentEntry::Connection { profile_id: id })
        })
        .unwrap();
        let items = update(&db, |items| items.push(&file(&abs("/q.sql")))).unwrap();
        assert_eq!(load(&db), items);
        assert_eq!(items.connections, vec![id]);
        // A bad entry leaves the stored list alone.
        assert!(update(&db, |items| items.push(&file("rel.sql"))).is_err());
        assert_eq!(load(&db), items);
        // Garbage in the setting reads as an empty list.
        db.set_setting(SETTINGS_KEY, &serde_json::json!("nope"))
            .unwrap();
        assert!(load(&db).is_empty());
    }

    #[test]
    fn entries_use_the_renderers_shape() {
        let id = Uuid::nil();
        assert_eq!(
            serde_json::to_value(RecentEntry::Connection { profile_id: id }).unwrap(),
            serde_json::json!({ "kind": "connection", "profileId": id })
        );
        assert_eq!(
            serde_json::from_value::<RecentEntry>(
                serde_json::json!({ "kind": "file", "path": "/a.sql" })
            )
            .unwrap(),
            file("/a.sql")
        );
    }
}
