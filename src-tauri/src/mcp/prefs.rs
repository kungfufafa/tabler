//! The preferences the MCP server follows. They live in the renderer's preferences object (the
//! `prefs` key of the app DB settings table, see `src/stores/prefs.ts`); the `--mcp` process reads
//! them at startup and again on every tool call, so a change in Settings applies at once.

use serde_json::Value as Json;

use crate::appdb::AppDb;
use crate::error::AppError;

/// App DB settings key holding the renderer's preferences object.
pub const PREFS_KEY: &str = "prefs";
/// Settings ▸ Connections ▸ MCP Server ▸ "Enable MCP server".
pub const ENABLED_KEY: &str = "mcpEnabled";
/// "Allow MCP clients to change data".
pub const ALLOW_WRITES_KEY: &str = "mcpAllowWrites";
/// "Expose production connections".
pub const EXPOSE_PRODUCTION_KEY: &str = "mcpExposeProduction";
/// Settings ▸ Connections ▸ Query time out (seconds, 0 = no limit), shared with the SQL editor.
pub const QUERY_TIMEOUT_KEY: &str = "queryTimeout";
/// The renderer's default and bounds of `queryTimeout`.
pub const DEFAULT_QUERY_TIMEOUT_SECS: u32 = 300;
const QUERY_TIMEOUT_MAX: u64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpPrefs {
    pub enabled: bool,
    pub allow_writes: bool,
    pub expose_production: bool,
    pub query_timeout_secs: u32,
}

impl Default for McpPrefs {
    /// Everything off: the server must be turned on, and stays read-only and away from production
    /// connections until the user says otherwise.
    fn default() -> Self {
        McpPrefs {
            enabled: false,
            allow_writes: false,
            expose_production: false,
            query_timeout_secs: DEFAULT_QUERY_TIMEOUT_SECS,
        }
    }
}

impl McpPrefs {
    /// Reads the preferences object; a missing or invalid value keeps its default, as the
    /// renderer's `sanitizePrefs` does (so hand-edited data never turns a safety switch on).
    pub fn from_json(prefs: Option<&Json>) -> McpPrefs {
        let mut out = McpPrefs::default();
        let Some(obj) = prefs.and_then(Json::as_object) else {
            return out;
        };
        let flag =
            |key: &str, default: bool| obj.get(key).and_then(Json::as_bool).unwrap_or(default);
        out.enabled = flag(ENABLED_KEY, out.enabled);
        out.allow_writes = flag(ALLOW_WRITES_KEY, out.allow_writes);
        out.expose_production = flag(EXPOSE_PRODUCTION_KEY, out.expose_production);
        if let Some(secs) = obj
            .get(QUERY_TIMEOUT_KEY)
            .and_then(Json::as_u64)
            .filter(|s| *s <= QUERY_TIMEOUT_MAX)
        {
            out.query_timeout_secs = secs as u32;
        }
        out
    }

    pub fn load(appdb: &AppDb) -> Result<McpPrefs, AppError> {
        Ok(McpPrefs::from_json(appdb.get_setting(PREFS_KEY)?.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_are_off_and_invalid_values_are_ignored() {
        assert_eq!(McpPrefs::from_json(None), McpPrefs::default());
        assert_eq!(McpPrefs::from_json(Some(&json!("x"))), McpPrefs::default());
        let p = McpPrefs::from_json(Some(&json!({
            "mcpEnabled": "yes",
            "mcpAllowWrites": 1,
            "mcpExposeProduction": null,
            "queryTimeout": -5,
        })));
        assert_eq!(p, McpPrefs::default());
        let p = McpPrefs::from_json(Some(&json!({ "queryTimeout": 100_000 })));
        assert_eq!(p.query_timeout_secs, DEFAULT_QUERY_TIMEOUT_SECS);
    }

    #[test]
    fn reads_the_renderer_prefs_object() {
        let p = McpPrefs::from_json(Some(&json!({
            "pageSize": 300,
            "mcpEnabled": true,
            "mcpAllowWrites": true,
            "mcpExposeProduction": true,
            "queryTimeout": 0,
        })));
        assert_eq!(
            p,
            McpPrefs {
                enabled: true,
                allow_writes: true,
                expose_production: true,
                query_timeout_secs: 0,
            }
        );
    }

    #[test]
    fn loads_from_the_app_db() {
        let db = AppDb::open_in_memory().unwrap();
        assert_eq!(McpPrefs::load(&db).unwrap(), McpPrefs::default());
        db.set_setting(PREFS_KEY, &json!({ "mcpEnabled": true }))
            .unwrap();
        assert!(McpPrefs::load(&db).unwrap().enabled);
    }
}
