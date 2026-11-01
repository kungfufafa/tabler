//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md).

use std::sync::Arc;

use super::params::{VerticaAuth, VerticaParams};
use super::session::VerticaSession;
use crate::drivers::native::{CallKind, unknown_call};
use crate::drivers::registry::UtilInput;
use crate::drivers::{ConnectTarget, Session};
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, Driver, PasswordMode, SslMode};

const DRIVER: Driver = Driver::Vertica;

/// Opens a Vertica session: TLS by the profile's SSL mode, SSH through the target's tunnel, a
/// Command password (an OAuth token) run again when it expires or is refused.
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    validate(target.profile)?;
    let session = VerticaSession::open(&target).await?;
    Ok(Session::Sql(Arc::new(session)))
}

/// The Vertica settings of a profile (`VerticaParams`), both files of a client certificate, and
/// an OAuth token that has somewhere to come from.
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    let params = VerticaParams::from_driver_params(profile.options.driver_params.as_ref())?;
    let blank = |s: Option<&str>| s.is_none_or(|s| s.trim().is_empty());
    if blank(profile.options.ssl_cert_path.as_deref())
        != blank(profile.options.ssl_key_path.as_deref())
    {
        return Err(AppError::InvalidInput(
            "Vertica: a client certificate needs both its certificate and its key file".into(),
        ));
    }
    if params.auth == VerticaAuth::OAuth {
        if profile.options.password_mode() == PasswordMode::None {
            return Err(AppError::InvalidInput(
                "Vertica: an OAuth token comes from the password field or its Command".into(),
            ));
        }
        // `prefer` goes on without TLS when the server (or anyone between) says no.
        if matches!(profile.ssl, SslMode::Disable | SslMode::Prefer) {
            return Err(AppError::InvalidInput(
                "Vertica: an OAuth token is sent only over TLS (set SSL mode to Require or stricter)"
                    .into(),
            ));
        }
    }
    Ok(())
}

/// Vertica has no calls of the connection form.
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
            "id": "6f9619ff-8b86-d011-b42d-00c04fc964ff", "name": "ve", "color": null,
            "group": null, "driver": "vertica", "host": "db.example.com", "port": null,
            "user": "dbadmin", "database": "VMart", "sslCaPath": null, "sqlitePath": null,
            "ssh": null, "lastConnectedAt": null
        });
        for (k, v) in json.as_object().expect("fields") {
            base[k] = v.clone();
        }
        serde_json::from_value(base).expect("a profile")
    }

    #[test]
    fn validate_checks_the_params_the_certificate_and_tokens() {
        assert!(validate(&profile(serde_json::json!({}))).is_ok());
        assert!(registry::validate(&profile(serde_json::json!({}))).is_ok());
        // The registry's common checks: host, user and database are required.
        for field in ["host", "user", "database"] {
            let p = profile(serde_json::json!({ field: null }));
            assert!(registry::validate(&p).is_err(), "{field}");
        }
        let bad = profile(serde_json::json!({
            "options": { "driverParams": { "clientLabel": "a\u{7}" } }
        }));
        assert!(validate(&bad).is_err());
        let half_cert = profile(serde_json::json!({
            "options": { "sslKeyPath": "client.key" }
        }));
        assert!(validate(&half_cert).is_err());
        let plain_token = profile(serde_json::json!({
            "ssl": "disable",
            "options": { "driverParams": { "auth": "oauth" } }
        }));
        assert!(validate(&plain_token).is_err());
        let preferred_token = profile(serde_json::json!({
            "ssl": "prefer",
            "options": { "driverParams": { "auth": "oauth" } }
        }));
        assert!(validate(&preferred_token).is_err());
        let no_token = profile(serde_json::json!({
            "ssl": "require",
            "options": { "passwordMode": "none", "driverParams": { "auth": "oauth" } }
        }));
        assert!(validate(&no_token).is_err());
        let token = profile(serde_json::json!({
            "ssl": "require",
            "options": { "driverParams": { "auth": "oauth" } }
        }));
        assert!(validate(&token).is_ok());
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
