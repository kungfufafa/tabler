//! Native menu accelerators the renderer remaps (Settings ▸ Shortcuts).

use std::collections::HashMap;

use tauri::AppHandle;
use tauri::menu::{Menu, MenuItem, MenuItemKind};

use crate::error::AppError;

const MODIFIERS: &[&str] = &[
    "CmdOrCtrl",
    "CommandOrControl",
    "Cmd",
    "Command",
    "Super",
    "Ctrl",
    "Control",
    "Alt",
    "Option",
    "Shift",
];

/// A Tauri accelerator the renderer may send: known modifiers, then one key (`CmdOrCtrl+Shift+T`,
/// `CmdOrCtrl+Ctrl+Right`, `CmdOrCtrl+\`). Anything else is refused instead of silently clearing
/// the item's accelerator (Tauri drops what it cannot parse).
pub fn accelerator_ok(accelerator: &str) -> bool {
    let mut parts: Vec<&str> = accelerator.split('+').collect();
    let Some(key) = parts.pop() else {
        return false;
    };
    let key_ok = !key.is_empty() && !MODIFIERS.contains(&key) && !key.contains(char::is_whitespace);
    key_ok && parts.iter().all(|m| MODIFIERS.contains(m))
}

/// The menu item with this id among `items`, nested submenus included (Edit ▸ Set Value ▸ NULL,
/// Navigate ▸ Go to Tab ▸ Tab 1).
fn find_in(items: Vec<MenuItemKind<tauri::Wry>>, id: &str) -> Option<MenuItem<tauri::Wry>> {
    for entry in items {
        match entry {
            MenuItemKind::MenuItem(item) if item.id() == id => return Some(item),
            MenuItemKind::Submenu(sub) => {
                if let Some(item) = find_in(sub.items().unwrap_or_default(), id) {
                    return Some(item);
                }
            }
            _ => {}
        }
    }
    None
}

/// The menu item with this id, in any submenu.
fn find_item(menu: &Menu<tauri::Wry>, id: &str) -> Option<MenuItem<tauri::Wry>> {
    find_in(menu.items().ok()?, id)
}

/// Rebinds the accelerators of the menu items that run renderer actions: action id (= menu item
/// id) → accelerator, `None` for none. Other ids are ignored.
#[tauri::command]
pub fn set_menu_accelerators(
    app: AppHandle,
    accelerators: HashMap<String, Option<String>>,
) -> Result<(), AppError> {
    for (id, accelerator) in &accelerators {
        if let Some(a) = accelerator
            && !accelerator_ok(a)
        {
            return Err(AppError::InvalidInput(format!(
                "invalid accelerator {a:?} for {id}"
            )));
        }
    }
    let Some(menu) = app.menu() else {
        return Ok(());
    };
    for (id, accelerator) in &accelerators {
        if crate::shell::menu_action(id).is_none() {
            continue;
        }
        if let Some(item) = find_item(&menu, id) {
            item.set_accelerator(accelerator.as_deref())
                .map_err(|e| AppError::Internal(e.to_string()))?;
        }
    }
    Ok(())
}

/// The app menu (File, Edit, View…) as a popup at `x`, `y` (logical pixels in the window): on
/// Windows the menu bar is hidden and the toolbar's ⋮ button opens it, like TablePlus for Windows.
/// Its items reach `on_menu_event` like the menu bar's.
#[tauri::command]
pub fn popup_app_menu(
    app: AppHandle,
    window: tauri::Window,
    x: f64,
    y: f64,
) -> Result<(), AppError> {
    let Some(menu) = app.menu() else {
        return Ok(());
    };
    window
        .popup_menu_at(&menu, tauri::LogicalPosition::new(x, y))
        .map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::accelerator_ok;

    #[test]
    fn accepts_the_renderers_accelerators() {
        assert!(accelerator_ok("CmdOrCtrl+T"));
        assert!(accelerator_ok("CmdOrCtrl+Shift+T"));
        assert!(accelerator_ok("CmdOrCtrl+Ctrl+Right"));
        assert!(accelerator_ok("CmdOrCtrl+Ctrl+["));
        assert!(accelerator_ok("CmdOrCtrl+Shift+\\"));
        assert!(accelerator_ok("Ctrl+-"));
        assert!(accelerator_ok("F5"));
    }

    #[test]
    fn refuses_malformed_ones() {
        assert!(!accelerator_ok(""));
        assert!(!accelerator_ok("CmdOrCtrl+"));
        assert!(!accelerator_ok("CmdOrCtrl+Shift"));
        assert!(!accelerator_ok("Hyper+T"));
        assert!(!accelerator_ok("CmdOrCtrl+Page Up"));
    }
}
