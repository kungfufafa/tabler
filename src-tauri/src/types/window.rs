use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// What a new window opens with (File ▸ New Window opens an empty one). Parked in the backend under
/// the new window's label until its renderer takes it (`window_take_handoff`): "Open in New
/// Window" on a connection, Window ▸ Move Tab to New Window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WindowHandoff {
    /// The saved connection the new window connects to (each window owns its connections).
    pub profile_id: Option<Uuid>,
    /// The database to open instead of the profile's default.
    pub database: Option<String>,
    /// The schema the sidebar shows.
    pub schema: Option<String>,
    /// A moved tab, in the renderer's saved-session tab format (opaque here).
    #[ts(type = "unknown")]
    pub tab: Option<serde_json::Value>,
}
