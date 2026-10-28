//! `MssqlParams`: the SQL Server settings of a profile beyond the common fields, read from
//! `options.driver_params`. Encryption follows the profile's SSL mode (`connect::encryption`);
//! these add what only SQL Server has.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::AppError;
use crate::types::profile::DriverParams;

/// How the login proves who it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MssqlAuth {
    /// A SQL Server login: user and password.
    #[default]
    SqlServer,
    /// Windows (NTLMv2) with `DOMAIN\user` and its password. Windows builds only.
    Windows,
    /// The Windows user running Tabler (SSPI). Windows builds only.
    Integrated,
    /// A Microsoft Entra ID access token, which the password (usually its Command mode, `az
    /// account get-access-token …`) holds.
    EntraToken,
}

impl MssqlAuth {
    /// Whether the profile's user name is needed.
    pub fn needs_user(self) -> bool {
        matches!(self, MssqlAuth::SqlServer | MssqlAuth::Windows)
    }

    /// Whether this build can log in this way.
    pub fn available(self) -> bool {
        match self {
            MssqlAuth::Windows | MssqlAuth::Integrated => cfg!(windows),
            MssqlAuth::SqlServer | MssqlAuth::EntraToken => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct MssqlParams {
    pub auth: MssqlAuth,
    /// A named instance (`SQLEXPRESS`), found through SQL Browser on UDP 1434 when the profile
    /// sets no port. Not over SSH: the tunnel carries TCP only.
    #[ts(optional = nullable)]
    pub instance: Option<String>,
    /// TDS 8.0 strict encryption (SQL Server 2022+): TLS before anything else is sent.
    pub strict: bool,
    /// The name the server certificate must carry, when it is not the host's (an IP address, a
    /// load balancer).
    #[ts(optional = nullable)]
    pub hostname_in_certificate: Option<String>,
    /// ApplicationIntent=ReadOnly: routes an availability group connection to a readable
    /// secondary. Not Tabler's read-only mode, which the classifier enforces.
    pub read_only_intent: bool,
    /// Shown in `sys.dm_exec_sessions.program_name`; empty: Tabler.
    #[ts(optional = nullable)]
    pub application_name: Option<String>,
    pub multi_subnet_failover: bool,
    /// TDS packet size in bytes (512–32767); None: the server's.
    #[ts(optional = nullable)]
    pub packet_size: Option<u32>,
}

pub const DEFAULT_APPLICATION_NAME: &str = "Tabler";

impl MssqlParams {
    /// The params of a profile, or why they cannot be read.
    pub fn from_driver_params(params: Option<&DriverParams>) -> Result<MssqlParams, AppError> {
        let Some(DriverParams(map)) = params else {
            return Ok(MssqlParams::default());
        };
        let parsed: MssqlParams = serde_json::from_value(serde_json::Value::Object(map.clone()))
            .map_err(|e| AppError::InvalidInput(format!("SQL Server settings: {e}")))?;
        parsed.check()?;
        Ok(parsed)
    }

    /// The checks of `validate`: values the server would refuse, said before connecting.
    pub fn check(&self) -> Result<(), AppError> {
        if let Some(size) = self.packet_size
            && !(512..=32767).contains(&size)
        {
            return Err(AppError::InvalidInput(format!(
                "SQL Server packet size: {size} is not between 512 and 32767"
            )));
        }
        if let Some(instance) = self.instance()
            && (instance.len() > 16
                || !instance
                    .chars()
                    .all(|c| c.is_alphanumeric() || "_$#".contains(c)))
        {
            return Err(AppError::InvalidInput(format!(
                "SQL Server instance name \u{201c}{instance}\u{201d}: up to 16 letters, digits, _, $ or #"
            )));
        }
        if !self.auth.available() {
            return Err(AppError::Unsupported(
                "Windows and integrated authentication work in the Windows build of Tabler only"
                    .into(),
            ));
        }
        Ok(())
    }

    pub fn instance(&self) -> Option<&str> {
        self.instance
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    pub fn application_name(&self) -> &str {
        self.application_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_APPLICATION_NAME)
    }

    pub fn hostname_in_certificate(&self) -> Option<&str> {
        self.hostname_in_certificate
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(json: serde_json::Value) -> Result<MssqlParams, AppError> {
        let serde_json::Value::Object(map) = json else {
            panic!("an object");
        };
        MssqlParams::from_driver_params(Some(&DriverParams(map)))
    }

    #[test]
    fn reads_camel_case_with_defaults() {
        assert_eq!(
            MssqlParams::from_driver_params(None).unwrap(),
            MssqlParams::default()
        );
        let p = params(serde_json::json!({
            "auth": "entraToken", "instance": " SQLEXPRESS ", "strict": true,
            "readOnlyIntent": true, "packetSize": 8192, "unknownKey": 1
        }))
        .unwrap();
        assert_eq!(p.auth, MssqlAuth::EntraToken);
        assert_eq!(p.instance(), Some("SQLEXPRESS"));
        assert!(p.strict && p.read_only_intent && !p.multi_subnet_failover);
        assert_eq!(p.packet_size, Some(8192));
        assert_eq!(p.application_name(), "Tabler");
        assert!(!MssqlAuth::EntraToken.needs_user() && MssqlAuth::SqlServer.needs_user());
    }

    #[test]
    fn refuses_what_the_server_would() {
        assert!(params(serde_json::json!({ "packetSize": 100 })).is_err());
        assert!(params(serde_json::json!({ "instance": "a;b" })).is_err());
        assert!(params(serde_json::json!({ "auth": "kerberos" })).is_err());
        let integrated = params(serde_json::json!({ "auth": "integrated" }));
        assert_eq!(integrated.is_ok(), cfg!(windows));
    }
}
