//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md).

use std::sync::Arc;

use super::params::MssqlParams;
use super::session::MssqlSession;
use crate::drivers::native::{CallKind, unknown_call};
use crate::drivers::registry::UtilInput;
use crate::drivers::{ConnectTarget, Session};
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, Driver};

const DRIVER: Driver = Driver::Sqlserver;

/// Opens a SQL Server session: TLS by the profile's SSL mode, SSH through the target's tunnel, a
/// Command password (an Entra ID token) run again when it expires.
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    let session = MssqlSession::open(&target).await?;
    Ok(Session::Sql(Arc::new(session)))
}

/// The SQL Server settings of a profile (`MssqlParams`), the user its login needs, and both
/// files of a client certificate.
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    let params = MssqlParams::from_driver_params(profile.options.driver_params.as_ref())?;
    let blank = |s: Option<&str>| s.is_none_or(|s| s.trim().is_empty());
    if params.auth.needs_user() && blank(profile.user.as_deref()) {
        return Err(AppError::InvalidInput(
            "User is required for this authentication".into(),
        ));
    }
    if blank(profile.options.ssl_cert_path.as_deref())
        != blank(profile.options.ssl_key_path.as_deref())
    {
        return Err(AppError::InvalidInput(
            "SQL Server: a client certificate needs both its certificate and its key file".into(),
        ));
    }
    Ok(())
}

/// SQL Server has no calls of the connection form.
pub async fn util(
    _input: UtilInput<'_>,
    method: &str,
    _args: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    Err(unknown_call(DRIVER, method))
}

/// Whether a `util` call only reads; anything unknown changes something.
pub fn util_kind(_method: &str) -> CallKind {
    CallKind::Write
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::registry;

    fn profile(json: serde_json::Value) -> ConnectionProfile {
        let mut base = serde_json::json!({
            "id": "6f9619ff-8b86-d011-b42d-00c04fc964ff", "name": "ms", "color": null,
            "group": null, "driver": "sqlserver", "host": "db.example.com", "port": null,
            "user": "sa", "database": null, "sslCaPath": null, "sqlitePath": null, "ssh": null,
            "lastConnectedAt": null
        });
        for (k, v) in json.as_object().expect("fields") {
            base[k] = v.clone();
        }
        serde_json::from_value(base).expect("a profile")
    }

    #[test]
    fn validate_needs_a_user_unless_the_login_has_none() {
        assert!(validate(&profile(serde_json::json!({}))).is_ok());
        let no_user = profile(serde_json::json!({ "user": " " }));
        assert!(matches!(
            validate(&no_user),
            Err(AppError::InvalidInput(m)) if m.contains("User")
        ));
        let entra = profile(serde_json::json!({
            "user": null,
            "options": { "driverParams": { "auth": "entraToken" } }
        }));
        assert!(validate(&entra).is_ok());
        // Through the registry, after its common checks (host required).
        assert!(registry::validate(&entra).is_ok());
        let no_host = profile(serde_json::json!({ "host": null }));
        assert!(registry::validate(&no_host).is_err());
        let bad = profile(serde_json::json!({
            "options": { "driverParams": { "packetSize": 3 } }
        }));
        assert!(validate(&bad).is_err());
        let half_cert = profile(serde_json::json!({
            "options": { "sslCertPath": "client.pem" }
        }));
        assert!(validate(&half_cert).is_err());
    }

    #[tokio::test]
    async fn util_knows_no_call_and_counts_it_a_write() {
        let p = profile(serde_json::json!({}));
        let input = UtilInput {
            profile: &p,
            secret: None,
        };
        assert!(matches!(
            util(input, "listDatabases", serde_json::Value::Null).await,
            Err(AppError::Unsupported(_))
        ));
        assert_eq!(util_kind("listDatabases"), CallKind::Write);
        assert_eq!(registry::util_kind(DRIVER, "anything"), CallKind::Write);
    }
}
