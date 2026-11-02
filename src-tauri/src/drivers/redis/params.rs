//! The Redis settings of a profile beyond the common fields (`options.driver_params`), and the
//! database index the profile's Database field names.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, DriverParams};

use super::info::MAX_DATABASES;
use super::keys::Limits;

/// Which RESP version a connection speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RedisProtocol {
    /// RESP3 (`HELLO 3`), or RESP2 when the server has no `HELLO` (before 6.0).
    #[default]
    Auto,
    Resp2,
    Resp3,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct RedisParams {
    /// The `COUNT` hint of each `SCAN` the key browser sends; none: the page size.
    pub scan_count: Option<u32>,
    /// Bytes of a string value the grid previews; none: 16 KiB.
    pub preview_bytes: Option<u32>,
    /// Elements of a container the grid previews; none: 50.
    pub preview_elements: Option<u32>,
    /// Whether the grid reads each key's `MEMORY USAGE`; none: yes.
    pub memory_column: Option<bool>,
    /// What separates the levels of the key tree (`user:1:name`); none: `:`. The renderer's.
    pub key_separator: Option<String>,
    pub protocol: RedisProtocol,
}

/// The largest `SCAN … COUNT` hint (a bigger one holds the server longer per call).
pub const MAX_SCAN_COUNT: u32 = 100_000;
/// The largest string preview of a cell.
pub const MAX_PREVIEW_BYTES: u32 = 16 * 1024 * 1024;
/// The most container elements a cell previews.
pub const MAX_PREVIEW_ELEMENTS: u32 = 10_000;

impl RedisParams {
    /// The params of a profile, checked.
    pub fn from_driver_params(params: Option<&DriverParams>) -> Result<RedisParams, AppError> {
        let Some(DriverParams(map)) = params else {
            return Ok(RedisParams::default());
        };
        let params: RedisParams = serde_json::from_value(serde_json::Value::Object(map.clone()))
            .map_err(|e| AppError::InvalidInput(format!("Redis settings: {e}")))?;
        params.check()?;
        Ok(params)
    }

    fn check(&self) -> Result<(), AppError> {
        let within = |v: Option<u32>, max: u32, what: &str| match v {
            Some(n) if n == 0 || n > max => Err(AppError::InvalidInput(format!(
                "Redis settings: {what} is between 1 and {max}"
            ))),
            _ => Ok(()),
        };
        within(self.scan_count, MAX_SCAN_COUNT, "the SCAN count")?;
        within(
            self.preview_bytes,
            MAX_PREVIEW_BYTES,
            "the value preview (bytes)",
        )?;
        within(
            self.preview_elements,
            MAX_PREVIEW_ELEMENTS,
            "the value preview (elements)",
        )?;
        if let Some(sep) = &self.key_separator
            && (sep.is_empty() || sep.chars().count() > 8)
        {
            return Err(AppError::InvalidInput(
                "Redis settings: the key separator is 1 to 8 characters".into(),
            ));
        }
        Ok(())
    }

    /// How much of a value the grid previews.
    pub fn limits(&self) -> Limits {
        let d = Limits::default();
        Limits {
            bytes: self.preview_bytes.map_or(d.bytes, u64::from),
            elements: self.preview_elements.unwrap_or(d.elements),
        }
    }
}

/// The profile's database index (its Database field, "DB index"): 0 when empty.
pub fn database_index(profile: &ConnectionProfile) -> Result<u32, AppError> {
    let Some(text) = profile
        .database
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(0);
    };
    // The sidebar's own names (`db3`) are taken too.
    let digits = text.strip_prefix("db").unwrap_or(text);
    match digits.parse::<u32>() {
        Ok(n) if n < MAX_DATABASES => Ok(n),
        _ => Err(AppError::InvalidInput(format!(
            "the DB index is a number from 0 to {}: \u{201c}{text}\u{201d}",
            MAX_DATABASES - 1
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(v: serde_json::Value) -> Result<RedisParams, AppError> {
        let serde_json::Value::Object(map) = v else {
            panic!()
        };
        RedisParams::from_driver_params(Some(&DriverParams(map)))
    }

    #[test]
    fn params_parse_and_are_checked() {
        assert_eq!(
            RedisParams::from_driver_params(None).unwrap(),
            RedisParams::default()
        );
        let p =
            params(json!({ "scanCount": 500, "previewBytes": 10, "protocol": "resp2" })).unwrap();
        assert_eq!(p.scan_count, Some(500));
        assert_eq!(p.protocol, RedisProtocol::Resp2);
        assert_eq!(
            p.limits(),
            Limits {
                bytes: 10,
                elements: Limits::default().elements
            }
        );
        // Unknown keys of a newer Tabler are kept out of the way, not refused.
        assert!(params(json!({ "somethingNew": 1 })).is_ok());
        for bad in [
            json!({ "scanCount": 0 }),
            json!({ "previewElements": 1_000_000 }),
            json!({ "keySeparator": "" }),
            json!({ "protocol": "resp4" }),
            json!({ "scanCount": "many" }),
        ] {
            assert!(
                matches!(params(bad.clone()), Err(AppError::InvalidInput(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_database_field_is_an_index() {
        let mut p: ConnectionProfile = serde_json::from_value(json!({
            "id": uuid::Uuid::nil(), "name": "r", "driver": "redis",
            "host": "h", "port": null, "user": null, "database": null,
            "sslCaPath": null, "sqlitePath": null, "ssh": null, "lastConnectedAt": null,
        }))
        .unwrap();
        assert_eq!(database_index(&p).unwrap(), 0);
        for (text, n) in [("3", 3), (" 15 ", 15), ("db2", 2)] {
            p.database = Some(text.into());
            assert_eq!(database_index(&p).unwrap(), n, "{text}");
        }
        for bad in ["-1", "x", "4096", "db"] {
            p.database = Some(bad.into());
            assert!(database_index(&p).is_err(), "{bad}");
        }
    }
}
