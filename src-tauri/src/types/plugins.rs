//! Plugins ▸ (TablePlus's JavaScript plugins): the manifest of an installed plugin, the list the
//! Manage Plugins… window shows and the entries of the native Plugins menu.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where a plugin command is offered: the data grid's cell menu, the sidebar item menu, the SQL
/// editor, or only the Plugins menu and the command palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PluginContextKind {
    Grid,
    Sidebar,
    Editor,
    Global,
}

/// One command of a plugin: `script` (a `.js` file inside the plugin folder) runs in a sandboxed
/// worker. With `handler`, the script defines that function and it is called with `context`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginCommand {
    pub name: String,
    /// Path of the script, relative to the plugin folder (`main.js`, `lib/format.js`).
    pub script: String,
    /// Keyboard shortcut in the keymap's spelling (`Ctrl+I`, `Mod+Shift+U`).
    #[serde(default)]
    #[ts(optional = nullable)]
    pub key: Option<String>,
    pub contexts: Vec<PluginContextKind>,
    /// A function the script defines (TablePlus's `Handler`); called with `context`.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub handler: Option<String>,
}

/// `manifest.json` of a plugin folder, validated (see `plugins::parse_manifest`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginManifest {
    pub name: String,
    /// Reverse-DNS style id (`com.example.row-tools`); the folder it is installed into.
    pub identifier: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub commands: Vec<PluginCommand>,
}

/// A plugin found in the Plugins folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstalledPlugin {
    pub manifest: PluginManifest,
    /// Absolute path of its folder.
    pub folder: String,
}

/// A folder in the Plugins folder whose manifest could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InvalidPlugin {
    pub folder: String,
    pub error: String,
}

/// What Manage Plugins… lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PluginList {
    /// The Plugins folder (`<app data>/Plugins`).
    pub dir: String,
    pub plugins: Vec<InstalledPlugin>,
    pub invalid: Vec<InvalidPlugin>,
}

/// An entry of the native Plugins menu, between Manage Plugins… and Open Plugins Folder. A chosen
/// command reaches the focused window as `plugins:run` with its `run` id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename_all_fields = "camelCase")]
pub enum PluginMenuEntry {
    Command {
        /// `<identifier>#<command index>`.
        run: String,
        label: String,
        /// Tauri accelerator (`Ctrl+I`, `CmdOrCtrl+Shift+U`).
        accelerator: Option<String>,
    },
    /// A plugin with several commands.
    Submenu {
        label: String,
        items: Vec<PluginMenuEntry>,
    },
    Separator,
    /// A disabled line ("No Plugins Enabled").
    Note {
        label: String,
    },
}
