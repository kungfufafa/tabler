//! `DuckdbParams`: the DuckDB settings of a profile beyond the common fields (read from
//! `options.driver_params`), and the engine configuration a profile opens with.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::engine::{EngineConfig, Location};
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, DriverParams};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct DuckdbParams {
    /// Lets DuckDB download and load the extensions a statement needs (from
    /// extensions.duckdb.org, into `~/.duckdb`). Off: only the built-in JSON and Parquet, and
    /// whatever the startup SQL loads. Never in a read-only or unattended session.
    pub autoload_extensions: bool,
}

impl DuckdbParams {
    /// The params of a profile, or why they cannot be read.
    pub fn from_driver_params(params: Option<&DriverParams>) -> Result<DuckdbParams, AppError> {
        let Some(DriverParams(map)) = params else {
            return Ok(DuckdbParams::default());
        };
        serde_json::from_value(serde_json::Value::Object(map.clone()))
            .map_err(|e| AppError::InvalidInput(format!("DuckDB settings: {e}")))
    }
}

/// How `profile` opens: its file (`:memory:` or nothing for an in-memory database), read-only
/// when the profile is, its encryption key (the profile's password), its startup SQL and
/// params. `hardened` asks for the hardened settings besides the read-only ones' (an unattended
/// or plugin session); a read-only profile is always hardened.
pub fn engine_config(
    profile: &ConnectionProfile,
    key: Option<&str>,
    hardened: bool,
) -> Result<EngineConfig, AppError> {
    let params = DuckdbParams::from_driver_params(profile.options.driver_params.as_ref())?;
    let read_only = crate::conn_options::effective_read_only(profile);
    let startup_sql = profile
        .options
        .startup_sql
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(EngineConfig {
        location: Location::parse(profile.sqlite_path.as_deref().unwrap_or_default()),
        read_only,
        hardened: read_only || hardened,
        encryption_key: key.filter(|k| !k.is_empty()).map(str::to_string),
        autoload_extensions: params.autoload_extensions,
        startup_sql,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(json: serde_json::Value) -> ConnectionProfile {
        let mut base = serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001", "name": "d", "driver": "duckdb",
            "ssl": "disable", "options": {}, "sortOrder": 0, "createdAt": "", "updatedAt": "",
        });
        if let (Some(b), Some(extra)) = (base.as_object_mut(), json.as_object()) {
            for (k, v) in extra {
                b.insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn profiles_become_engine_configs() {
        let p = profile(serde_json::json!({
            "sqlitePath": "C:/data/x.duckdb",
            "options": { "startupSql": " SET threads = 2 ", "driverParams": { "autoloadExtensions": true } }
        }));
        let c = engine_config(&p, Some("k"), false).unwrap();
        assert_eq!(c.location, Location::File("C:/data/x.duckdb".into()));
        assert!(!c.read_only && !c.hardened && c.autoload_extensions);
        assert_eq!(c.encryption_key.as_deref(), Some("k"));
        assert_eq!(c.startup_sql.as_deref(), Some("SET threads = 2"));
        // Unattended: hardened, not read-only.
        let c = engine_config(&p, None, true).unwrap();
        assert!(c.hardened && !c.read_only && c.encryption_key.is_none());

        let ro = profile(serde_json::json!({
            "sqlitePath": ":memory:", "options": { "readOnly": true }
        }));
        let c = engine_config(&ro, Some(""), false).unwrap();
        assert_eq!(c.location, Location::Memory);
        assert!(c.read_only && c.hardened && c.encryption_key.is_none());

        let bad = profile(serde_json::json!({
            "options": { "driverParams": { "autoloadExtensions": "yes" } }
        }));
        assert!(engine_config(&bad, None, false).is_err());
    }
}
