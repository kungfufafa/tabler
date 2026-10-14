//! Where the `--mcp` process finds the app database without a running Tauri app: the default app
//! data folder Tauri uses for the bundle identifier, then the Settings ▸ Locations pointer file
//! in it (read only: a pending move of the database is left to the app's next start).

use std::path::{Path, PathBuf};

use crate::locations::{DB_FILE, read_pointer};

/// `identifier` of `tauri.conf.json` (checked by a test): the name of the app data folder.
pub const APP_IDENTIFIER: &str = "com.mekayastudio.tabler";

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn home() -> Option<PathBuf> {
    env_dir("HOME").or_else(|| env_dir("USERPROFILE"))
}

/// The folder Tauri's `app_data_dir()` returns: the platform's data folder + the identifier.
pub fn default_app_data_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        home()?.join("Library").join("Application Support")
    } else if cfg!(windows) {
        env_dir("APPDATA")?
    } else {
        env_dir("XDG_DATA_HOME").or_else(|| Some(home()?.join(".local").join("share")))?
    };
    Some(base.join(APP_IDENTIFIER))
}

/// The app database file the app uses: in the folder chosen in Settings ▸ Locations when it is
/// there, else in the default folder (the app falls back the same way).
pub fn app_db_file(default_dir: &Path) -> PathBuf {
    let chosen = read_pointer(default_dir)
        .ok()
        .and_then(|p| p.data_dir)
        .filter(|d| d.is_dir());
    chosen
        .unwrap_or_else(|| default_dir.to_path_buf())
        .join(DB_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locations::{Pointer, write_pointer};

    #[test]
    fn identifier_matches_tauri_conf() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"].as_str(), Some(APP_IDENTIFIER));
    }

    #[test]
    fn default_dir_ends_with_the_identifier() {
        if let Some(dir) = default_app_data_dir() {
            assert!(dir.ends_with(APP_IDENTIFIER));
            assert!(dir.is_absolute());
            #[cfg(target_os = "macos")]
            assert!(
                dir.to_string_lossy()
                    .contains("/Library/Application Support/")
            );
        }
    }

    #[test]
    fn follows_the_locations_pointer() {
        let default = tempfile::tempdir().unwrap();
        let chosen = tempfile::tempdir().unwrap();
        assert_eq!(app_db_file(default.path()), default.path().join(DB_FILE));
        let pointer = Pointer {
            data_dir: Some(chosen.path().to_path_buf()),
            ..Pointer::default()
        };
        write_pointer(default.path(), &pointer).unwrap();
        assert_eq!(app_db_file(default.path()), chosen.path().join(DB_FILE));
        // A chosen folder that is not there (unmounted drive): the default one, as the app does.
        let gone = Pointer {
            data_dir: Some(chosen.path().join("missing")),
            ..Pointer::default()
        };
        write_pointer(default.path(), &gone).unwrap();
        assert_eq!(app_db_file(default.path()), default.path().join(DB_FILE));
    }
}
