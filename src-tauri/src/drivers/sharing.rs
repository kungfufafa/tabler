//! Engines that open a database file once per process (`Sharing::SingleProcess`: DuckDB). Their
//! driver keeps one instance per file and refuses a second one with other settings, so the app
//! does not open a second session of a file it has open unless the driver says it can share
//! (`ensure_second_session`), and finds the connections that hold a file (`file_key`).

use std::path::PathBuf;

use crate::error::AppError;
use crate::types::caps::{Feature, SessionCaps};
use crate::types::driver::Sharing;
use crate::types::profile::{ConnectionProfile, Driver};

pub fn single_process(driver: Driver) -> bool {
    driver.descriptor().sharing == Sharing::SingleProcess
}

/// The file a profile of a SingleProcess engine opens, the same for every spelling of its path;
/// None for every other profile.
pub fn file_key(profile: &ConnectionProfile) -> Option<PathBuf> {
    if !single_process(profile.driver) {
        return None;
    }
    let path = profile.sqlite_path.as_deref().map(str::trim)?;
    if path.is_empty() {
        return None;
    }
    Some(std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path)))
}

/// Refuses a path that opens a second session next to a SingleProcess session unless the
/// session's caps grant the path's feature `f`: the driver grants it only when its second
/// session shares the first one's instance. The paths of an open connection (switching the
/// database, another encoding, the session of an import or export) need `f` on every engine and
/// check it with `require`; this is for a path that holds no connection of its own (a backup's
/// inspection of a file another connection has open).
pub fn ensure_second_session(caps: &SessionCaps, f: Feature) -> Result<(), AppError> {
    if single_process(caps.driver) && !caps.has(f) {
        return Err(AppError::Unsupported(format!(
            "{}: {} would open the database file a second time",
            caps.engine(),
            f.label()
        )));
    }
    Ok(())
}

/// Whether a new session of `wanted` may share the instance of the open `holder` of the same
/// file: the same profile with the same settings (a second workspace of it). Anything else
/// would need another configuration of the one instance.
pub fn may_share(holder: &ConnectionProfile, wanted: &ConnectionProfile) -> bool {
    let settings = |p: &ConnectionProfile| {
        let mut p = p.clone();
        p.last_connected_at = None;
        p.updated_at = String::new();
        p
    };
    settings(holder) == settings(wanted)
}

/// Why a session of `wanted` is not opened while the connection named `name` has its file open.
pub fn file_in_use(wanted: &ConnectionProfile, name: &str) -> AppError {
    AppError::InvalidInput(format!(
        "{} opens a database file once per app: \u{201c}{}\u{201d} is open in this Tabler \
         (connection \u{201c}{name}\u{201d}). Disconnect it first.",
        wanted.driver.label(),
        wanted.sqlite_path.as_deref().unwrap_or_default()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::profile::{ProfileOptions, SslMode};

    fn profile(driver: Driver, path: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: uuid::Uuid::new_v4(),
            name: "f".into(),
            color: None,
            group: None,
            driver,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: Some(path.into()),
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    #[test]
    fn only_single_process_engines_have_a_file_key() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.duckdb");
        std::fs::write(&file, b"").unwrap();
        let path = file.to_string_lossy().into_owned();
        // Every spelling of the path is one key.
        let other = dir
            .path()
            .join(".")
            .join("a.duckdb")
            .to_string_lossy()
            .into_owned();
        let duck = file_key(&profile(Driver::Duckdb, &path)).unwrap();
        assert_eq!(file_key(&profile(Driver::Duckdb, &other)), Some(duck));
        assert_eq!(file_key(&profile(Driver::Duckdb, "  ")), None);
        // SQLite opens a file as often as it likes.
        assert_eq!(file_key(&profile(Driver::Sqlite, &path)), None);
        for d in Driver::ALL {
            assert_eq!(single_process(d), d == Driver::Duckdb, "{d:?}");
        }
    }

    #[test]
    fn a_second_session_needs_the_feature_on_single_process_engines() {
        let duck = SessionCaps::minimal(Driver::Duckdb);
        let err = ensure_second_session(&duck, Feature::SwitchDatabase).unwrap_err();
        assert_eq!(err.code(), "UNSUPPORTED");
        assert!(err.to_string().contains("a second time"), "{err}");
        ensure_second_session(
            &duck.with(&[Feature::SwitchDatabase]),
            Feature::SwitchDatabase,
        )
        .unwrap();
        // Other engines open as many sessions as they like (their commands check the feature).
        ensure_second_session(&SessionCaps::minimal(Driver::Clickhouse), Feature::Encoding)
            .unwrap();
    }

    #[test]
    fn only_the_same_settings_share_a_file() {
        let a = profile(Driver::Duckdb, "/x.duckdb");
        let mut later = a.clone();
        later.last_connected_at = Some("now".into());
        later.updated_at = "now".into();
        assert!(may_share(&a, &later));
        let mut ro = a.clone();
        ro.options.read_only = true;
        assert!(!may_share(&a, &ro));
        let other = profile(Driver::Duckdb, "/x.duckdb");
        assert!(!may_share(&a, &other), "another profile");
        assert!(
            file_in_use(&ro, "main")
                .to_string()
                .contains("Disconnect it first")
        );
    }
}
