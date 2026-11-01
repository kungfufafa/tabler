//! `CassandraParams`: the Cassandra settings of a profile beyond the common fields, read from
//! `options.driver_params`. TLS follows the profile's SSL mode, the keyspace is its database.

use std::time::Duration;

use ::scylla::frame::types::{Consistency, SerialConsistency};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::AppError;
use crate::types::profile::DriverParams;

/// The consistency level statements run at (cqlsh's `CONSISTENCY` changes it for the editor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(export)]
pub enum CassandraConsistency {
    Any,
    One,
    Two,
    Three,
    Quorum,
    All,
    LocalQuorum,
    EachQuorum,
    #[default]
    LocalOne,
}

impl CassandraConsistency {
    pub fn level(self) -> Consistency {
        match self {
            CassandraConsistency::Any => Consistency::Any,
            CassandraConsistency::One => Consistency::One,
            CassandraConsistency::Two => Consistency::Two,
            CassandraConsistency::Three => Consistency::Three,
            CassandraConsistency::Quorum => Consistency::Quorum,
            CassandraConsistency::All => Consistency::All,
            CassandraConsistency::LocalQuorum => Consistency::LocalQuorum,
            CassandraConsistency::EachQuorum => Consistency::EachQuorum,
            CassandraConsistency::LocalOne => Consistency::LocalOne,
        }
    }
}

/// The serial consistency of lightweight transactions (the grid's edits are ones).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(export)]
pub enum CassandraSerialConsistency {
    Serial,
    #[default]
    LocalSerial,
}

impl CassandraSerialConsistency {
    pub fn level(self) -> SerialConsistency {
        match self {
            CassandraSerialConsistency::Serial => SerialConsistency::Serial,
            CassandraSerialConsistency::LocalSerial => SerialConsistency::LocalSerial,
        }
    }
}

/// Compression of the frames the driver and the server exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CassandraCompression {
    #[default]
    None,
    Lz4,
    Snappy,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct CassandraParams {
    pub consistency: CassandraConsistency,
    pub serial_consistency: CassandraSerialConsistency,
    /// The datacenter whose nodes the driver prefers (a multi-datacenter cluster, AWS
    /// Keyspaces); None: no preference.
    #[ts(optional = nullable)]
    pub local_datacenter: Option<String>,
    /// How long one request may take before the driver gives up (1–3600 s); None: 30 s.
    #[ts(optional = nullable)]
    pub request_timeout_secs: Option<u32>,
    /// Rows per page of the editor's runs (1–10000); None: 100, as cqlsh.
    #[ts(optional = nullable)]
    pub page_size: Option<u32>,
    pub compression: CassandraCompression,
    /// Talk to the host only, never to the other nodes it names. Always on through an SSH
    /// tunnel, which reaches one node.
    pub single_node: bool,
}

pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_PAGE_SIZE: u32 = 100;
pub const MAX_PAGE_SIZE: u32 = 10_000;

impl CassandraParams {
    /// The params of a profile, or why they cannot be read.
    pub fn from_driver_params(params: Option<&DriverParams>) -> Result<CassandraParams, AppError> {
        let Some(DriverParams(map)) = params else {
            return Ok(CassandraParams::default());
        };
        let parsed: CassandraParams =
            serde_json::from_value(serde_json::Value::Object(map.clone()))
                .map_err(|e| AppError::InvalidInput(format!("Cassandra settings: {e}")))?;
        parsed.check()?;
        Ok(parsed)
    }

    fn check(&self) -> Result<(), AppError> {
        if self
            .request_timeout_secs
            .is_some_and(|s| !(1..=3600).contains(&s))
        {
            return Err(AppError::InvalidInput(
                "Cassandra settings: the request timeout is 1 to 3600 seconds".into(),
            ));
        }
        if self
            .page_size
            .is_some_and(|n| !(1..=MAX_PAGE_SIZE).contains(&n))
        {
            return Err(AppError::InvalidInput(format!(
                "Cassandra settings: the page size is 1 to {MAX_PAGE_SIZE} rows"
            )));
        }
        if self
            .local_datacenter
            .as_deref()
            .is_some_and(|dc| dc.trim().is_empty() && !dc.is_empty())
        {
            return Err(AppError::InvalidInput(
                "Cassandra settings: the local datacenter is blank".into(),
            ));
        }
        Ok(())
    }

    pub fn request_timeout(&self) -> Duration {
        self.request_timeout_secs
            .map_or(DEFAULT_REQUEST_TIMEOUT, |s| Duration::from_secs(s.into()))
    }

    pub fn page_size(&self) -> u32 {
        self.page_size.unwrap_or(DEFAULT_PAGE_SIZE)
    }

    pub fn local_datacenter(&self) -> Option<&str> {
        self.local_datacenter
            .as_deref()
            .map(str::trim)
            .filter(|dc| !dc.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: serde_json::Value) -> Result<CassandraParams, AppError> {
        let serde_json::Value::Object(map) = json else {
            panic!("an object");
        };
        CassandraParams::from_driver_params(Some(&DriverParams(map)))
    }

    #[test]
    fn reads_the_settings_and_their_defaults() {
        let none = CassandraParams::from_driver_params(None).unwrap();
        assert_eq!(none.consistency.level(), Consistency::LocalOne);
        assert_eq!(
            none.serial_consistency.level(),
            SerialConsistency::LocalSerial
        );
        assert_eq!(none.request_timeout(), DEFAULT_REQUEST_TIMEOUT);
        assert_eq!(none.page_size(), 100);
        assert!(!none.single_node);
        let p = parse(serde_json::json!({
            "consistency": "LOCAL_QUORUM", "serialConsistency": "SERIAL",
            "localDatacenter": " dc1 ", "requestTimeoutSecs": 5, "pageSize": 500,
            "compression": "lz4", "singleNode": true
        }))
        .unwrap();
        assert_eq!(p.consistency.level(), Consistency::LocalQuorum);
        assert_eq!(p.serial_consistency.level(), SerialConsistency::Serial);
        assert_eq!(p.local_datacenter(), Some("dc1"));
        assert_eq!(p.request_timeout(), Duration::from_secs(5));
        assert_eq!(p.page_size(), 500);
        assert_eq!(p.compression, CassandraCompression::Lz4);
        assert!(p.single_node);
    }

    #[test]
    fn refuses_what_the_driver_cannot_take() {
        for bad in [
            serde_json::json!({ "consistency": "SERIAL" }),
            serde_json::json!({ "requestTimeoutSecs": 0 }),
            serde_json::json!({ "pageSize": 10001 }),
            serde_json::json!({ "localDatacenter": "  " }),
            serde_json::json!({ "compression": "zstd" }),
        ] {
            assert!(
                matches!(parse(bad.clone()), Err(AppError::InvalidInput(_))),
                "{bad}"
            );
        }
    }
}
