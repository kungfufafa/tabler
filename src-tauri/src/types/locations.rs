//! Settings ▸ Locations: the folders holding the app database (connections, settings, history)
//! and the saved queries (`.sql` files). Passwords never move: they stay in the Keychain.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Which folder a Locations call is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LocationKind {
    /// "Connections & settings": the folder of `tabler.sqlite` (profiles, preferences, history).
    Data,
    /// "Queries": the folder of the saved queries, one `.sql` file each, folders = sub-folders.
    Queries,
}

/// What happens to the data when a location changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LocationChange {
    /// Bring the current data along (queries are merged into the new folder; the app database is
    /// copied there at the next start, the old copy stays where it was).
    Move,
    /// Start with nothing (the new folder holds no Tabler data yet).
    Empty,
    /// The new folder already holds Tabler data: use it as it is.
    UseExisting,
    /// The new folder already holds an app database: keep it as a backup file next to it and
    /// move the current data in.
    Replace,
}

/// The folders in use, as Settings ▸ Locations shows them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocationsInfo {
    /// Folder of the app database this session.
    pub data_dir: String,
    pub default_data_dir: String,
    /// The folder chosen in Settings (`None`: the default). Differs from `data_dir` when it was
    /// not available at startup, or until the next start after a change.
    pub configured_data_dir: Option<String>,
    /// The app database file (connections, settings and the query history).
    pub data_file: String,
    /// The chosen data folder was missing at startup: this session uses the default one.
    pub data_fallback: bool,
    /// A data folder change waits for the next start (the folder it will use).
    pub pending_data_dir: Option<String>,
    /// Folder of the saved queries this session.
    pub queries_dir: String,
    pub default_queries_dir: String,
    pub configured_queries_dir: Option<String>,
    /// The chosen queries folder was missing at startup: this session uses the default one.
    pub queries_fallback: bool,
    /// Problems found at startup (a folder missing, a move that failed…).
    pub warnings: Vec<String>,
}

/// A folder the user picked, checked before the change is confirmed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocationInspection {
    pub path: String,
    /// The folder is the one already in use (or already chosen).
    pub is_current: bool,
    /// Data: the folder already holds a `tabler.sqlite`. Queries: it holds `.sql` files.
    pub has_data: bool,
    /// Number of `.sql` files in it (queries only, counted up to 10,000).
    pub query_files: u32,
    /// Why the folder cannot be used (not a folder, not writable, inside the other one…).
    pub problem: Option<String>,
}
