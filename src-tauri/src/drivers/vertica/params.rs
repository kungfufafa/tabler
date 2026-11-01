//! `VerticaParams`: the Vertica settings of a profile beyond the common fields, read from
//! `options.driver_params`. TLS follows the profile's SSL mode (`connect::Dial`); these add what
//! only Vertica has.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::AppError;
use crate::types::profile::DriverParams;

/// What the password field (or its Command) holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum VerticaAuth {
    /// A password, answered with the hash the server asks for (SHA-512 or MD5), or as it is
    /// over TLS (LDAP users).
    #[default]
    Password,
    /// An OAuth access token (usually from the Command mode), which the server checks with its
    /// identity provider.
    #[serde(rename = "oauth")]
    OAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct VerticaParams {
    pub auth: VerticaAuth,
    /// Shown in `v_monitor.sessions.client_label`; empty: Tabler.
    #[ts(optional = nullable)]
    pub client_label: Option<String>,
    /// The workload a routing rule sends the sessions by; empty: the server's default.
    #[ts(optional = nullable)]
    pub workload: Option<String>,
    /// The editor's statements wait for a COMMIT (autocommit off). The app's own connections
    /// always commit each statement.
    pub manual_commit: bool,
}

pub const DEFAULT_CLIENT_LABEL: &str = "Tabler";

/// The longest label or workload name Tabler sends (the server keeps 128 characters).
const MAX_NAME: usize = 128;

impl VerticaParams {
    /// The params of a profile, or why they cannot be read.
    pub fn from_driver_params(params: Option<&DriverParams>) -> Result<VerticaParams, AppError> {
        let Some(DriverParams(map)) = params else {
            return Ok(VerticaParams::default());
        };
        let parsed: VerticaParams = serde_json::from_value(serde_json::Value::Object(map.clone()))
            .map_err(|e| AppError::InvalidInput(format!("Vertica settings: {e}")))?;
        parsed.check()?;
        Ok(parsed)
    }

    /// The checks of `validate`: the label and workload go into the startup message, which
    /// takes no control characters.
    pub fn check(&self) -> Result<(), AppError> {
        for (what, value) in [
            ("client label", self.client_label()),
            ("workload", self.workload()),
        ] {
            if let Some(v) = value
                && (v.chars().count() > MAX_NAME || v.chars().any(char::is_control))
            {
                return Err(AppError::InvalidInput(format!(
                    "Vertica {what} \u{201c}{v}\u{201d}: up to {MAX_NAME} characters, no line breaks"
                )));
            }
        }
        Ok(())
    }

    pub fn client_label(&self) -> Option<&str> {
        trimmed(self.client_label.as_deref())
    }

    pub fn workload(&self) -> Option<&str> {
        trimmed(self.workload.as_deref())
    }
}

fn trimmed(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(json: serde_json::Value) -> Result<VerticaParams, AppError> {
        let serde_json::Value::Object(map) = json else {
            panic!("an object");
        };
        VerticaParams::from_driver_params(Some(&DriverParams(map)))
    }

    #[test]
    fn reads_camel_case_with_defaults() {
        assert_eq!(
            VerticaParams::from_driver_params(None).unwrap(),
            VerticaParams::default()
        );
        let p = params(serde_json::json!({
            "auth": "oauth", "clientLabel": " reports ", "workload": "",
            "manualCommit": true, "unknownKey": 1
        }))
        .unwrap();
        assert_eq!(p.auth, VerticaAuth::OAuth);
        assert_eq!(p.client_label(), Some("reports"));
        assert_eq!(p.workload(), None);
        assert!(p.manual_commit);
    }

    #[test]
    fn refuses_what_the_startup_message_cannot_carry() {
        assert!(params(serde_json::json!({ "clientLabel": "a\nb" })).is_err());
        assert!(params(serde_json::json!({ "workload": "w".repeat(129) })).is_err());
        assert!(params(serde_json::json!({ "auth": "kerberos" })).is_err());
        assert!(params(serde_json::json!({ "workload": "w".repeat(128) })).is_ok());
    }
}
