//! Saved queries (TablePlus's "Queries", formerly "Favorites") and the folders that hold them.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// A query kept in the sidebar's Queries pane. Typing its `keyword` in the SQL editor offers the
/// saved SQL as a completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SavedQuery {
    pub id: Uuid,
    pub name: String,
    /// Binding keyword (no whitespace); `None` when the query has none.
    pub keyword: Option<String>,
    pub sql: String,
    /// The folder it sits in; `None` = top level.
    pub folder_id: Option<Uuid>,
    /// The connection profile it belongs to; `None` = every connection.
    pub profile_id: Option<Uuid>,
    /// RFC 3339 timestamps.
    pub created_at: String,
    pub updated_at: String,
}

/// Creates (`id` = `None`) or replaces a saved query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SavedQueryInput {
    pub id: Option<Uuid>,
    pub name: String,
    pub keyword: Option<String>,
    pub sql: String,
    pub folder_id: Option<Uuid>,
    pub profile_id: Option<Uuid>,
    /// Rename / move / re-tag only: leave the file's SQL as it is on disk and ignore `sql`. The
    /// renderer's copy of the SQL may be stale (the file changed outside Tabler) or lossy (a file
    /// that is not UTF-8), so a metadata change must not write it back. Ignored when the query
    /// does not exist yet.
    #[serde(default)]
    #[ts(optional)]
    pub keep_sql: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueryFolder {
    pub id: Uuid,
    pub name: String,
    /// The enclosing folder; `None` = top level.
    pub parent_id: Option<Uuid>,
    pub created_at: String,
}

/// Creates (`id` = `None`) or renames / moves a folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueryFolderInput {
    pub id: Option<Uuid>,
    pub name: String,
    pub parent_id: Option<Uuid>,
}

/// Everything the Queries pane lists: folders and queries, each sorted by name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SavedQueryLibrary {
    pub folders: Vec<QueryFolder>,
    pub queries: Vec<SavedQuery>,
}
