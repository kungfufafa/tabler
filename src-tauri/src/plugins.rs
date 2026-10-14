//! Plugins (TablePlus's Plugins ▸, ⌘L): folders with a `manifest.json` and JavaScript files, kept
//! in `<app data>/Plugins`. This module validates manifests and installs (copies), lists, removes
//! and reads plugins; the renderer runs their scripts in a sandboxed Web Worker. `menu` keeps the
//! native Plugins menu in step with the enabled plugins.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::error::AppError;
use crate::types::plugins::{
    InstalledPlugin, InvalidPlugin, PluginCommand, PluginContextKind, PluginList, PluginManifest,
};

/// Name of the folder under the app data folder.
pub const PLUGINS_DIR: &str = "Plugins";
pub const MANIFEST_FILE: &str = "manifest.json";
/// Identifiers starting with this are the built-in plugins'.
pub const RESERVED_PREFIX: &str = "tabler.";
/// Largest script a command may run.
pub const MAX_SCRIPT_BYTES: u64 = 2 * 1024 * 1024;
/// Largest manifest read.
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// Limits of a folder copied by Install from Folder….
pub const MAX_PLUGIN_FILES: usize = 1_000;
pub const MAX_PLUGIN_BYTES: u64 = 50 * 1024 * 1024;
const MAX_COMMANDS: usize = 50;
/// Never copied: version control and Finder metadata.
const SKIPPED_NAMES: &[&str] = &[".git", ".DS_Store", "node_modules"];

/// `<app data>/Plugins`.
pub fn plugins_dir(app_data: &Path) -> PathBuf {
    app_data.join(PLUGINS_DIR)
}

// --- manifest ------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawManifest {
    name: Option<String>,
    identifier: Option<String>,
    /// TablePlus manifests use numbers (`"Version": 1`); strings are fine too.
    version: Option<serde_json::Value>,
    author: Option<String>,
    description: Option<String>,
    commands: Option<Vec<RawCommand>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCommand {
    name: Option<String>,
    script: Option<String>,
    key: Option<String>,
    contexts: Option<Vec<String>>,
    handler: Option<String>,
}

fn text(field: &str, value: Option<String>, max: usize) -> Result<String, String> {
    let value = value.map(|v| v.trim().to_string()).unwrap_or_default();
    if value.is_empty() {
        return Err(format!("\"{field}\" is missing"));
    }
    if value.chars().count() > max {
        return Err(format!("\"{field}\" is longer than {max} characters"));
    }
    Ok(value)
}

fn optional_text(field: &str, value: Option<String>, max: usize) -> Result<String, String> {
    let value = value.map(|v| v.trim().to_string()).unwrap_or_default();
    if value.chars().count() > max {
        return Err(format!("\"{field}\" is longer than {max} characters"));
    }
    Ok(value)
}

/// `com.example.row-tools`: letters, digits, `.`, `_`, `-`, starting with a letter or digit.
pub fn identifier_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !id.contains("..")
}

/// A script path inside the plugin folder: relative, `/`-separated, no `..`, ending in `.js`.
pub fn script_path_ok(script: &str) -> bool {
    if script.is_empty() || script.len() > 255 || !script.ends_with(".js") || script.contains('\\')
    {
        return false;
    }
    let path = Path::new(script);
    path.components().all(|c| match c {
        Component::Normal(part) => !part.to_string_lossy().starts_with('.'),
        _ => false,
    })
}

/// A JavaScript function name (`format`, `onRun`, `$run`).
fn handler_ok(handler: &str) -> bool {
    let mut chars = handler.chars();
    handler.len() <= 64
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

const COMBO_MODIFIERS: &[&str] = &["Mod", "Ctrl", "Alt", "Shift"];
const NAMED_KEYS: &[&str] = &[
    "Enter",
    "Escape",
    "Backspace",
    "Delete",
    "Tab",
    "Space",
    "Up",
    "Down",
    "Left",
    "Right",
    "Home",
    "End",
    "PageUp",
    "PageDown",
];

fn function_key(key: &str) -> bool {
    key.strip_prefix('F')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n))
}

/// A shortcut in the keymap's spelling (`Ctrl+I`, `Mod+Shift+U`): modifiers in the order Mod, Ctrl,
/// Alt, Shift, then one key. A plain key needs a modifier (it would steal typing), F-keys do not.
pub fn combo_ok(combo: &str) -> bool {
    let mut parts: Vec<&str> = combo.split('+').collect();
    let Some(key) = parts.pop() else {
        return false;
    };
    let mut last = None;
    for m in &parts {
        let Some(i) = COMBO_MODIFIERS.iter().position(|x| x == m) else {
            return false;
        };
        if last.is_some_and(|l| i <= l) {
            return false;
        }
        last = Some(i);
    }
    let single = key.len() == 1
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || ",./\\[]-=`;'".contains(c));
    let key_ok = single || NAMED_KEYS.contains(&key) || function_key(key);
    key_ok && (!parts.is_empty() || function_key(key))
}

fn context(name: &str) -> Option<PluginContextKind> {
    match name {
        "grid" => Some(PluginContextKind::Grid),
        "sidebar" => Some(PluginContextKind::Sidebar),
        "editor" => Some(PluginContextKind::Editor),
        "global" => Some(PluginContextKind::Global),
        _ => None,
    }
}

fn command(index: usize, raw: RawCommand) -> Result<PluginCommand, String> {
    let at = |e: String| format!("command {}: {e}", index + 1);
    let name = text("name", raw.name, 100).map_err(at)?;
    let script = raw.script.map(|s| s.trim().to_string()).unwrap_or_default();
    if !script_path_ok(&script) {
        return Err(at(format!(
            "\"script\" must be a .js file inside the plugin folder (got {script:?})"
        )));
    }
    let key = match raw.key.map(|k| k.trim().to_string()) {
        Some(k) if k.is_empty() => None,
        Some(k) if combo_ok(&k) => Some(k),
        Some(k) => {
            return Err(at(format!(
                "\"key\" {k:?} is not a shortcut like \"Ctrl+I\" or \"Mod+Shift+U\""
            )));
        }
        None => None,
    };
    let mut contexts = Vec::new();
    for c in raw.contexts.unwrap_or_default() {
        let kind = context(c.trim()).ok_or_else(|| {
            at(format!(
                "unknown context {c:?} (grid, sidebar, editor or global)"
            ))
        })?;
        if !contexts.contains(&kind) {
            contexts.push(kind);
        }
    }
    if contexts.is_empty() {
        return Err(at("\"contexts\" is empty".into()));
    }
    let handler = match raw.handler.map(|h| h.trim().to_string()) {
        Some(h) if h.is_empty() => None,
        Some(h) if handler_ok(&h) => Some(h),
        Some(h) => return Err(at(format!("\"handler\" {h:?} is not a function name"))),
        None => None,
    };
    Ok(PluginCommand {
        name,
        script,
        key,
        contexts,
        handler,
    })
}

/// Parses and validates a `manifest.json`. Unknown fields are ignored.
pub fn parse_manifest(json: &str) -> Result<PluginManifest, String> {
    let raw: RawManifest =
        serde_json::from_str(json).map_err(|e| format!("manifest.json is not valid: {e}"))?;
    let name = text("name", raw.name, 100)?;
    let identifier = raw
        .identifier
        .map(|i| i.trim().to_string())
        .unwrap_or_default();
    if !identifier_ok(&identifier) {
        return Err(format!(
            "\"identifier\" {identifier:?} must use letters, digits, '.', '_' or '-' (like com.example.my-plugin)"
        ));
    }
    if identifier.to_ascii_lowercase().starts_with(RESERVED_PREFIX) {
        return Err(format!(
            "\"identifier\" {identifier:?} is reserved for Tabler's built-in plugins"
        ));
    }
    let version = match raw.version {
        None | Some(serde_json::Value::Null) => "1".to_string(),
        Some(serde_json::Value::String(s)) => text("version", Some(s), 32)?,
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(_) => return Err("\"version\" must be a string or a number".into()),
    };
    let author = optional_text("author", raw.author, 100)?;
    let description = optional_text("description", raw.description, 1_000)?;
    let raw_commands = raw.commands.unwrap_or_default();
    if raw_commands.is_empty() {
        return Err("\"commands\" is empty: a plugin needs at least one command".into());
    }
    if raw_commands.len() > MAX_COMMANDS {
        return Err(format!("more than {MAX_COMMANDS} commands"));
    }
    let commands = raw_commands
        .into_iter()
        .enumerate()
        .map(|(i, c)| command(i, c))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PluginManifest {
        name,
        identifier,
        version,
        author,
        description,
        commands,
    })
}

/// The text of `manifest.json`: a regular file (not a symbolic link, a FIFO or a device such as
/// `/dev/zero`, whose reported length is 0 although it never ends), read through a bound instead of
/// trusting the length seen before the read.
fn read_manifest_text(path: &Path) -> Result<String, String> {
    let meta =
        fs::symlink_metadata(path).map_err(|_| format!("no {MANIFEST_FILE} in this folder"))?;
    if !meta.file_type().is_file() {
        return Err(format!("{MANIFEST_FILE} must be a regular file"));
    }
    if meta.len() > MAX_MANIFEST_BYTES {
        return Err(format!("{MANIFEST_FILE} is too large"));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // The file may be swapped for a link or a FIFO after the check above: never follow a link
        // and never block opening a FIFO.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("{MANIFEST_FILE}: {e}"))?;
    let opened = file
        .metadata()
        .map_err(|e| format!("{MANIFEST_FILE}: {e}"))?;
    if !opened.file_type().is_file() {
        return Err(format!("{MANIFEST_FILE} must be a regular file"));
    }
    read_limited(file, MAX_MANIFEST_BYTES)
}

/// All of `reader` as text, an error when more than `max` bytes arrive (the reader is never read
/// past `max + 1` bytes, whatever length it reported beforehand).
fn read_limited(reader: impl std::io::Read, max: u64) -> Result<String, String> {
    use std::io::Read;

    let mut text = String::new();
    reader
        .take(max + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("{MANIFEST_FILE}: {e}"))?;
    if text.len() as u64 > max {
        return Err(format!("{MANIFEST_FILE} is too large"));
    }
    Ok(text)
}

/// Reads and validates `folder/manifest.json`, and checks that every command's script is a file
/// in the folder.
pub fn read_manifest(folder: &Path) -> Result<PluginManifest, String> {
    let json = read_manifest_text(&folder.join(MANIFEST_FILE))?;
    let manifest = parse_manifest(&json)?;
    for c in &manifest.commands {
        let script = folder.join(&c.script);
        match fs::symlink_metadata(&script) {
            Ok(m) if m.is_file() => {}
            _ => {
                return Err(format!(
                    "the script {} of \"{}\" is missing",
                    c.script, c.name
                ));
            }
        }
    }
    Ok(manifest)
}

// --- the Plugins folder ----------------------------------------------------------------------------

fn hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// Every plugin in `dir` (a missing folder lists nothing), sorted by name. Folders whose manifest
/// is invalid, and second folders with an identifier already listed, go to `invalid`.
pub fn list(dir: &Path) -> PluginList {
    let mut plugins = Vec::new();
    let mut invalid = Vec::new();
    let mut folders: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter(|e| !hidden(&e.file_name().to_string_lossy()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    folders.sort();
    let mut seen = HashSet::new();
    for folder in folders {
        let shown = folder.display().to_string();
        match read_manifest(&folder) {
            Ok(manifest) if !seen.insert(manifest.identifier.to_ascii_lowercase()) => {
                invalid.push(InvalidPlugin {
                    folder: shown,
                    error: format!(
                        "another folder already holds the plugin {}",
                        manifest.identifier
                    ),
                });
            }
            Ok(manifest) => plugins.push(InstalledPlugin {
                manifest,
                folder: shown,
            }),
            Err(error) => invalid.push(InvalidPlugin {
                folder: shown,
                error,
            }),
        }
    }
    plugins.sort_by_key(|p| p.manifest.name.to_lowercase());
    PluginList {
        dir: dir.display().to_string(),
        plugins,
        invalid,
    }
}

/// The installed plugin with this identifier.
pub fn find(dir: &Path, identifier: &str) -> Result<InstalledPlugin, AppError> {
    list(dir)
        .plugins
        .into_iter()
        .find(|p| p.manifest.identifier.eq_ignore_ascii_case(identifier))
        .ok_or_else(|| AppError::NotFound(format!("plugin {identifier}")))
}

/// The files of `src` to copy (relative paths), refusing symbolic links and oversized folders.
fn collect_files(src: &Path) -> Result<Vec<PathBuf>, AppError> {
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for entry in fs::read_dir(src.join(&rel))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if SKIPPED_NAMES.contains(&name.as_str()) {
                continue;
            }
            let path = rel.join(&name);
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.file_type().is_symlink() {
                return Err(AppError::InvalidInput(format!(
                    "the plugin folder contains a symbolic link ({}); copy the real files instead",
                    path.display()
                )));
            }
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                total += meta.len();
                files.push(path);
                if files.len() > MAX_PLUGIN_FILES {
                    return Err(AppError::InvalidInput(format!(
                        "the plugin folder holds more than {MAX_PLUGIN_FILES} files"
                    )));
                }
                if total > MAX_PLUGIN_BYTES {
                    return Err(AppError::InvalidInput(
                        "the plugin folder is larger than 50 MB".into(),
                    ));
                }
            }
        }
    }
    Ok(files)
}

fn copy_files(src: &Path, dst: &Path, files: &[PathBuf]) -> Result<(), AppError> {
    fs::create_dir_all(dst)?;
    for rel in files {
        let to = dst.join(rel);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src.join(rel), &to)?;
    }
    Ok(())
}

/// Install from Folder…: validates `src` and copies it into `dir/<identifier>`, replacing an
/// installed plugin with the same identifier (an update).
pub fn install_from_folder(dir: &Path, src: &Path) -> Result<InstalledPlugin, AppError> {
    if !src.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "{} is not a folder",
            src.display()
        )));
    }
    let manifest = read_manifest(src).map_err(AppError::InvalidInput)?;
    fs::create_dir_all(dir)?;
    let canonical_dir = dir.canonicalize()?;
    let canonical_src = src.canonicalize()?;
    if canonical_src.starts_with(&canonical_dir) {
        return Err(AppError::InvalidInput(
            "this folder is already in the Plugins folder; use Reload".into(),
        ));
    }
    let files = collect_files(src)?;
    let staging = dir.join(format!(".install-{}", uuid::Uuid::new_v4().simple()));
    if let Err(e) = copy_files(src, &staging, &files) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    // An update: the folder of the installed copy goes (whatever its name), then the new one
    // takes the identifier's name.
    let result = (|| {
        if let Ok(old) = find(dir, &manifest.identifier) {
            remove_folder(&canonical_dir, Path::new(&old.folder))?;
        }
        let target = dir.join(&manifest.identifier);
        if target.exists() {
            remove_folder(&canonical_dir, &target)?;
        }
        fs::rename(&staging, &target)?;
        Ok::<_, AppError>(target)
    })();
    match result {
        Ok(target) => Ok(InstalledPlugin {
            manifest,
            folder: target.display().to_string(),
        }),
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            Err(e)
        }
    }
}

/// Deletes `folder`, which must sit inside the Plugins folder.
fn remove_folder(canonical_dir: &Path, folder: &Path) -> Result<(), AppError> {
    let canonical = folder.canonicalize()?;
    if canonical == canonical_dir || !canonical.starts_with(canonical_dir) {
        return Err(AppError::InvalidInput(format!(
            "{} is not inside the Plugins folder",
            folder.display()
        )));
    }
    fs::remove_dir_all(&canonical)?;
    Ok(())
}

/// Remove: deletes the plugin's folder.
pub fn remove(dir: &Path, identifier: &str) -> Result<(), AppError> {
    let plugin = find(dir, identifier)?;
    remove_folder(&dir.canonicalize()?, Path::new(&plugin.folder))
}

/// The text of one of the plugin's command scripts (only scripts its manifest names).
pub fn read_script(dir: &Path, identifier: &str, script: &str) -> Result<String, AppError> {
    let plugin = find(dir, identifier)?;
    if !plugin.manifest.commands.iter().any(|c| c.script == script) || !script_path_ok(script) {
        return Err(AppError::InvalidInput(format!(
            "{script} is not a script of {identifier}"
        )));
    }
    let folder = Path::new(&plugin.folder).canonicalize()?;
    let path = folder.join(script).canonicalize()?;
    if !path.starts_with(&folder) {
        return Err(AppError::InvalidInput(format!(
            "{script} is outside the plugin folder"
        )));
    }
    if fs::metadata(&path)?.len() > MAX_SCRIPT_BYTES {
        return Err(AppError::InvalidInput(format!(
            "{script} is larger than 2 MB"
        )));
    }
    fs::read_to_string(&path).map_err(|e| AppError::Io(format!("{script}: {e}")))
}

// --- the native Plugins menu -----------------------------------------------------------------------

/// The Plugins menu between Connection and Navigate: Manage Plugins… (⌘L), the enabled plugins'
/// commands (sent by the renderer, `set_entries`), Open Plugins Folder.
pub mod menu {
    use tauri::menu::{
        IsMenuItem, MenuItem, MenuItemBuilder, MenuItemKind, PredefinedMenuItem, Submenu,
        SubmenuBuilder,
    };
    use tauri::{AppHandle, Manager, Runtime};

    use crate::error::AppError;
    use crate::types::plugins::PluginMenuEntry;

    pub const SUBMENU_ID: &str = "plugins.menu";
    /// Menu item ids of plugin commands: this prefix + the command's run id.
    pub const RUN_PREFIX: &str = "plugin:";
    pub const NOTE_ID: &str = "plugins.note";
    pub const REVEAL_ID: &str = "plugins.reveal-folder";
    /// Payload: the run id (`<identifier>#<index>`) of the chosen command; to the focused window.
    pub const EVENT_RUN: &str = "plugins:run";
    /// Items before the commands (Manage Plugins…, a separator) and after them (a separator, Open
    /// Plugins Folder).
    const LEADING: usize = 2;
    const TRAILING: usize = 2;
    const MAX_ENTRIES: usize = 500;

    /// The menu as built at startup: no commands yet (the renderer sends them once it loaded).
    pub fn build<R: Runtime>(
        app: &AppHandle<R>,
        manage: &MenuItem<R>,
    ) -> tauri::Result<Submenu<R>> {
        let note = MenuItemBuilder::with_id(NOTE_ID, "No Plugins Enabled")
            .enabled(false)
            .build(app)?;
        let reveal = MenuItemBuilder::with_id(REVEAL_ID, "Open Plugins Folder").build(app)?;
        SubmenuBuilder::with_id(app, SUBMENU_ID, "Plugins")
            .item(manage)
            .separator()
            .item(&note)
            .separator()
            .item(&reveal)
            .build()
    }

    fn check_label(label: &str) -> Result<(), AppError> {
        if label.trim().is_empty() || label.chars().count() > 200 {
            return Err(AppError::InvalidInput(format!(
                "invalid menu label {label:?}"
            )));
        }
        Ok(())
    }

    /// Checks entries before anything in the menu changes: labels, run ids, accelerators (Tauri
    /// drops what it cannot parse), one level of submenus.
    pub fn validate(entries: &[PluginMenuEntry], nested: bool) -> Result<(), AppError> {
        if entries.len() > MAX_ENTRIES {
            return Err(AppError::InvalidInput(
                "too many plugin menu entries".into(),
            ));
        }
        for entry in entries {
            match entry {
                PluginMenuEntry::Command {
                    run,
                    label,
                    accelerator,
                } => {
                    check_label(label)?;
                    if run.is_empty() || run.len() > 200 {
                        return Err(AppError::InvalidInput(format!("invalid run id {run:?}")));
                    }
                    if let Some(a) = accelerator
                        && !crate::commands::menu::accelerator_ok(a)
                    {
                        return Err(AppError::InvalidInput(format!(
                            "invalid accelerator {a:?} for {label}"
                        )));
                    }
                }
                PluginMenuEntry::Submenu { label, items } => {
                    check_label(label)?;
                    if nested {
                        return Err(AppError::InvalidInput("plugin submenus cannot nest".into()));
                    }
                    validate(items, true)?;
                }
                PluginMenuEntry::Note { label } => check_label(label)?,
                PluginMenuEntry::Separator => {}
            }
        }
        Ok(())
    }

    fn command_item<R: Runtime>(
        app: &AppHandle<R>,
        run: &str,
        label: &str,
        accelerator: Option<&str>,
    ) -> tauri::Result<MenuItem<R>> {
        let mut builder = MenuItemBuilder::with_id(format!("{RUN_PREFIX}{run}"), label);
        if let Some(a) = accelerator {
            builder = builder.accelerator(a);
        }
        builder.build(app)
    }

    fn make<R: Runtime>(
        app: &AppHandle<R>,
        entry: &PluginMenuEntry,
    ) -> tauri::Result<MenuItemKind<R>> {
        Ok(match entry {
            PluginMenuEntry::Command {
                run,
                label,
                accelerator,
            } => MenuItemKind::MenuItem(command_item(app, run, label, accelerator.as_deref())?),
            PluginMenuEntry::Submenu { label, items } => {
                let sub = SubmenuBuilder::new(app, label).build()?;
                for item in items {
                    sub.append(&make(app, item)?)?;
                }
                MenuItemKind::Submenu(sub)
            }
            PluginMenuEntry::Note { label } => MenuItemKind::MenuItem(
                MenuItemBuilder::with_id(NOTE_ID, label)
                    .enabled(false)
                    .build(app)?,
            ),
            PluginMenuEntry::Separator => {
                MenuItemKind::Predefined(PredefinedMenuItem::separator(app)?)
            }
        })
    }

    fn find_submenu<R: Runtime>(app: &AppHandle<R>) -> Option<Submenu<R>> {
        app.menu()?
            .items()
            .ok()?
            .into_iter()
            .find_map(|item| match item {
                MenuItemKind::Submenu(sub) if sub.id() == SUBMENU_ID => Some(sub),
                _ => None,
            })
    }

    /// Replaces the commands between Manage Plugins… and Open Plugins Folder.
    pub fn set_entries<R: Runtime>(
        app: &AppHandle<R>,
        entries: &[PluginMenuEntry],
    ) -> Result<(), AppError> {
        validate(entries, false)?;
        let Some(sub) = find_submenu(app) else {
            return Ok(());
        };
        let internal = |e: tauri::Error| AppError::Internal(e.to_string());
        let count = sub.items().map_err(internal)?.len();
        for _ in 0..count.saturating_sub(LEADING + TRAILING) {
            sub.remove_at(LEADING).map_err(internal)?;
        }
        let placeholder = [PluginMenuEntry::Note {
            label: "No Plugins Enabled".into(),
        }];
        let shown = if entries.is_empty() {
            &placeholder[..]
        } else {
            entries
        };
        for (i, entry) in shown.iter().enumerate() {
            let item = make(app, entry).map_err(internal)?;
            sub.insert(&item as &dyn IsMenuItem<R>, LEADING + i)
                .map_err(internal)?;
        }
        Ok(())
    }

    /// The run id of a plugin command's menu item id.
    pub fn run_id(menu_id: &str) -> Option<&str> {
        menu_id.strip_prefix(RUN_PREFIX).filter(|r| !r.is_empty())
    }

    /// Opens the Plugins folder in Finder (creating it first).
    pub fn reveal_folder<R: Runtime>(app: &AppHandle<R>) -> Result<(), AppError> {
        let data = app
            .path()
            .app_data_dir()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let dir = super::plugins_dir(&data);
        std::fs::create_dir_all(&dir)?;
        tauri_plugin_opener::open_path(&dir, None::<&str>)
            .map_err(|e| AppError::Io(format!("{}: {e}", dir.display())))
    }

    /// Handles the Plugins menu's own items; false for every other id.
    pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) -> bool {
        if id == REVEAL_ID {
            if let Err(e) = reveal_folder(app) {
                log::warn!("could not open the Plugins folder: {e}");
            }
            return true;
        }
        if let Some(run) = run_id(id) {
            crate::windows::emit_to_target(app, EVENT_RUN, run.to_string());
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::plugins::PluginMenuEntry;

    const GOOD: &str = r#"{
        "name": "Row Tools",
        "identifier": "com.example.row-tools",
        "version": 2,
        "author": "Example",
        "description": "Does things with rows",
        "commands": [
            { "name": "Count", "script": "count.js", "contexts": ["grid", "sidebar"], "key": "Mod+Shift+U" },
            { "name": "Format", "script": "lib/format.js", "contexts": ["editor"], "handler": "onRun" }
        ],
        "homepage": "ignored"
    }"#;

    fn manifest_with(patch: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut v: serde_json::Value = serde_json::from_str(GOOD).unwrap();
        patch(&mut v);
        v.to_string()
    }

    #[test]
    fn parses_a_valid_manifest() {
        let m = parse_manifest(GOOD).unwrap();
        assert_eq!(m.name, "Row Tools");
        assert_eq!(m.identifier, "com.example.row-tools");
        assert_eq!(m.version, "2");
        assert_eq!(m.commands.len(), 2);
        assert_eq!(
            m.commands[0].contexts,
            vec![PluginContextKind::Grid, PluginContextKind::Sidebar]
        );
        assert_eq!(m.commands[0].key.as_deref(), Some("Mod+Shift+U"));
        assert_eq!(m.commands[1].handler.as_deref(), Some("onRun"));
        // Defaults: version "1", empty author / description.
        let bare = r#"{"name":"A","identifier":"a","commands":[{"name":"Go","script":"a.js","contexts":["global"]}]}"#;
        let m = parse_manifest(bare).unwrap();
        assert_eq!((m.version.as_str(), m.author.as_str()), ("1", ""));
    }

    #[test]
    fn refuses_invalid_manifests() {
        let cases: Vec<(String, &str)> = vec![
            ("not json".into(), "not valid"),
            (manifest_with(|v| v["name"] = "  ".into()), "\"name\""),
            (
                manifest_with(|v| v["identifier"] = "../evil".into()),
                "identifier",
            ),
            (
                manifest_with(|v| v["identifier"] = "tabler.sql-formatter".into()),
                "reserved",
            ),
            (
                manifest_with(|v| v["commands"] = serde_json::json!([])),
                "at least one command",
            ),
            (
                manifest_with(|v| v["commands"][0]["script"] = "../x.js".into()),
                "script",
            ),
            (
                manifest_with(|v| v["commands"][0]["script"] = "/etc/x.js".into()),
                "script",
            ),
            (
                manifest_with(|v| v["commands"][0]["script"] = "x.txt".into()),
                "script",
            ),
            (
                manifest_with(|v| v["commands"][0]["contexts"] = serde_json::json!(["window"])),
                "unknown context",
            ),
            (
                manifest_with(|v| v["commands"][0]["contexts"] = serde_json::json!([])),
                "contexts",
            ),
            (
                manifest_with(|v| v["commands"][0]["key"] = "U".into()),
                "\"key\"",
            ),
            (
                manifest_with(|v| v["commands"][0]["key"] = "Shift+Mod+U".into()),
                "\"key\"",
            ),
            (
                manifest_with(|v| v["commands"][1]["handler"] = "on run()".into()),
                "handler",
            ),
            (
                manifest_with(|v| v["version"] = serde_json::json!([1])),
                "version",
            ),
        ];
        for (json, needle) in cases {
            let err = parse_manifest(&json).expect_err(&json);
            assert!(err.contains(needle), "{err:?} should mention {needle:?}");
        }
    }

    #[test]
    fn validates_shortcuts_and_paths() {
        for ok in [
            "Ctrl+I",
            "Mod+Shift+U",
            "Mod+Ctrl+Alt+Shift+9",
            "F5",
            "Alt+F2",
            "Mod+[",
        ] {
            assert!(combo_ok(ok), "{ok}");
        }
        for bad in [
            "I",
            "Ctrl+",
            "Ctrl+i",
            "Hyper+I",
            "Shift+Ctrl+I",
            "Ctrl+Ctrl+I",
            "",
        ] {
            assert!(!combo_ok(bad), "{bad}");
        }
        for ok in ["main.js", "lib/format.js"] {
            assert!(script_path_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "main.ts",
            "../main.js",
            "/main.js",
            "./main.js",
            ".hidden.js",
            "a\\b.js",
        ] {
            assert!(!script_path_ok(bad), "{bad}");
        }
    }

    fn write_plugin(folder: &Path, identifier: &str, script_body: &str) {
        fs::create_dir_all(folder.join("lib")).unwrap();
        let manifest = manifest_with(|v| v["identifier"] = identifier.into());
        fs::write(folder.join(MANIFEST_FILE), manifest).unwrap();
        fs::write(folder.join("count.js"), script_body).unwrap();
        fs::write(folder.join("lib/format.js"), "function onRun(context) {}").unwrap();
    }

    #[test]
    fn installs_lists_reads_and_removes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = plugins_dir(tmp.path());
        // A missing folder lists nothing.
        assert!(list(&dir).plugins.is_empty());

        let src = tmp.path().join("src-plugin");
        write_plugin(&src, "com.example.row-tools", "context.alert('v1', '')");
        fs::write(src.join(".DS_Store"), "x").unwrap();
        let installed = install_from_folder(&dir, &src).unwrap();
        assert_eq!(
            installed.folder,
            dir.join("com.example.row-tools").display().to_string()
        );
        assert!(!Path::new(&installed.folder).join(".DS_Store").exists());
        let listed = list(&dir);
        assert_eq!(listed.plugins.len(), 1);
        assert!(listed.invalid.is_empty());
        assert_eq!(
            read_script(&dir, "com.example.row-tools", "count.js").unwrap(),
            "context.alert('v1', '')"
        );
        // Only the scripts the manifest names.
        assert!(read_script(&dir, "com.example.row-tools", "manifest.json").is_err());
        assert!(read_script(&dir, "com.example.row-tools", "../x.js").is_err());

        // Installing again updates the copy in place.
        write_plugin(&src, "com.example.row-tools", "context.alert('v2', '')");
        install_from_folder(&dir, &src).unwrap();
        assert_eq!(list(&dir).plugins.len(), 1);
        assert_eq!(
            read_script(&dir, "com.example.row-tools", "count.js").unwrap(),
            "context.alert('v2', '')"
        );
        // No staging folder is left behind.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty());

        // The Plugins folder itself cannot be installed from.
        assert!(install_from_folder(&dir, Path::new(&installed.folder)).is_err());

        remove(&dir, "com.example.row-tools").unwrap();
        assert!(list(&dir).plugins.is_empty());
        assert!(matches!(
            remove(&dir, "com.example.row-tools"),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn lists_broken_folders_as_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = plugins_dir(tmp.path());
        write_plugin(&dir.join("good"), "com.example.good", "");
        // Same identifier in a second folder.
        write_plugin(&dir.join("zz-copy"), "com.example.good", "");
        // A script the manifest names is missing.
        write_plugin(&dir.join("missing"), "com.example.missing", "");
        fs::remove_file(dir.join("missing/count.js")).unwrap();
        // No manifest at all.
        fs::create_dir_all(dir.join("empty")).unwrap();
        let listed = list(&dir);
        assert_eq!(listed.plugins.len(), 1);
        assert_eq!(listed.invalid.len(), 3);
        let errors: Vec<&str> = listed.invalid.iter().map(|i| i.error.as_str()).collect();
        assert!(errors.iter().any(|e| e.contains("already holds")));
        assert!(errors.iter().any(|e| e.contains("is missing")));
        assert!(errors.iter().any(|e| e.contains("no manifest.json")));
    }

    #[test]
    fn refuses_invalid_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = plugins_dir(tmp.path());
        // Not a folder.
        assert!(install_from_folder(&dir, &tmp.path().join("nope")).is_err());
        // An invalid manifest installs nothing.
        let bad = tmp.path().join("bad");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join(MANIFEST_FILE), "{}").unwrap();
        assert!(matches!(
            install_from_folder(&dir, &bad),
            Err(AppError::InvalidInput(_))
        ));
        #[cfg(unix)]
        {
            let linked = tmp.path().join("linked");
            write_plugin(&linked, "com.example.linked", "");
            std::os::unix::fs::symlink("/etc/hosts", linked.join("hosts")).unwrap();
            let err = install_from_folder(&dir, &linked).unwrap_err();
            assert!(err.to_string().contains("symbolic link"), "{err}");
            assert!(list(&dir).plugins.is_empty());
        }
    }

    /// Runs `f` on another thread: a manifest read that never returns must fail the test, not hang
    /// the suite.
    fn within_seconds<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(std::time::Duration::from_secs(secs))
            .expect("the manifest read did not return")
    }

    #[test]
    fn a_valid_manifest_still_loads_from_a_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("ok");
        write_plugin(&folder, "com.example.ok", "");
        let m = read_manifest(&folder).unwrap();
        assert_eq!(m.identifier, "com.example.ok");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_manifest_that_is_not_a_regular_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = plugins_dir(tmp.path());

        // manifest.json -> /dev/zero: its length reads 0 although it never ends.
        let zero = dir.join("zero");
        write_plugin(&zero, "com.example.zero", "");
        fs::remove_file(zero.join(MANIFEST_FILE)).unwrap();
        std::os::unix::fs::symlink("/dev/zero", zero.join(MANIFEST_FILE)).unwrap();
        let z = zero.clone();
        let err = within_seconds(5, move || read_manifest(&z)).unwrap_err();
        assert!(err.contains("regular file"), "{err}");

        // A symbolic link to a real manifest is refused as well.
        let linked = dir.join("linked");
        write_plugin(&linked, "com.example.linked", "");
        let real = linked.join("real.json");
        fs::rename(linked.join(MANIFEST_FILE), &real).unwrap();
        std::os::unix::fs::symlink(&real, linked.join(MANIFEST_FILE)).unwrap();
        let l = linked.clone();
        let err = within_seconds(5, move || read_manifest(&l)).unwrap_err();
        assert!(err.contains("regular file"), "{err}");

        // A FIFO would block the reader until somebody writes to it.
        let fifo = dir.join("fifo");
        write_plugin(&fifo, "com.example.fifo", "");
        fs::remove_file(fifo.join(MANIFEST_FILE)).unwrap();
        let c_path =
            std::ffi::CString::new(fifo.join(MANIFEST_FILE).to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let f = fifo.clone();
        let err = within_seconds(5, move || read_manifest(&f)).unwrap_err();
        assert!(err.contains("regular file"), "{err}");

        // Listing the Plugins folder survives all of them and reports each as invalid.
        let d = dir.clone();
        let listed = within_seconds(5, move || list(&d));
        assert!(listed.plugins.is_empty());
        assert_eq!(listed.invalid.len(), 3);

        // Installing from such a folder fails the same way.
        let d = dir.clone();
        let z = zero.clone();
        let err =
            within_seconds(5, move || install_from_folder(&d.join("target"), &z)).unwrap_err();
        assert!(err.to_string().contains("regular file"), "{err}");
    }

    #[test]
    fn refuses_an_oversized_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("big");
        write_plugin(&folder, "com.example.big", "");
        let padding = " ".repeat(MAX_MANIFEST_BYTES as usize);
        fs::write(
            folder.join(MANIFEST_FILE),
            format!("{}{padding}", manifest_with(|_| {})),
        )
        .unwrap();
        let err = read_manifest(&folder).unwrap_err();
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn never_reads_more_than_the_limit_from_a_stream_that_does_not_end() {
        // What a file reporting length 0 (a device, a file growing while it is read) looks like.
        let endless = std::io::repeat(b' ');
        let err = within_seconds(5, move || read_limited(endless, 1_000)).unwrap_err();
        assert!(err.contains("too large"), "{err}");
        assert_eq!(read_limited(&b"{}"[..], 1_000).unwrap(), "{}");
        let exact = vec![b'a'; 1_000];
        assert_eq!(read_limited(&exact[..], 1_000).unwrap().len(), 1_000);
        let over = vec![b'a'; 1_001];
        assert!(read_limited(&over[..], 1_000).is_err());
    }

    #[test]
    fn validates_menu_entries() {
        let cmd = |run: &str, acc: Option<&str>| PluginMenuEntry::Command {
            run: run.into(),
            label: "Go".into(),
            accelerator: acc.map(String::from),
        };
        assert!(
            menu::validate(
                &[cmd("a#0", Some("Ctrl+I")), PluginMenuEntry::Separator],
                false
            )
            .is_ok()
        );
        assert!(menu::validate(&[cmd("", None)], false).is_err());
        assert!(menu::validate(&[cmd("a#0", Some("Ctrl+"))], false).is_err());
        let nested = PluginMenuEntry::Submenu {
            label: "P".into(),
            items: vec![PluginMenuEntry::Submenu {
                label: "Q".into(),
                items: vec![],
            }],
        };
        assert!(menu::validate(&[nested], false).is_err());
        assert_eq!(menu::run_id("plugin:a#0"), Some("a#0"));
        assert_eq!(menu::run_id("plugin:"), None);
        assert_eq!(menu::run_id("tab.close"), None);
    }
}
