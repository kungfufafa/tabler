//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md).

use std::sync::Arc;
use std::time::Duration;

use super::client::ScyllaConnector;
use super::params::CassandraParams;
use super::session::CassandraSession;
use super::wire::{Credentials, Dial, Password, RunOptions, Tls};
use crate::conn_options::effective_read_only;
use crate::drivers::native::{CallKind, unknown_call};
use crate::drivers::registry::UtilInput;
use crate::drivers::{ConnectTarget, Session};
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::types::caps::SafeConfirm;
use crate::types::profile::{ConnectionProfile, Driver, PasswordMode, SslMode};

const DRIVER: Driver = Driver::Cassandra;

/// How often the driver checks an idle connection (the descriptor's keep-alive).
const KEEPALIVE: Duration = Duration::from_secs(30);

fn trimmed(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

/// What a target opens: its contact point (the tunnel's local end through SSH, which pins the
/// session to that one node), TLS checked for the profile's host, PasswordAuthenticator when the
/// profile has a user (a Command password runs again once it expires), and the keyspace.
pub fn dial(target: &ConnectTarget<'_>) -> Result<(Dial, CassandraParams), AppError> {
    let profile = target.profile;
    let params = CassandraParams::from_driver_params(profile.options.driver_params.as_ref())?;
    let tls = match profile.ssl {
        // CQL has no TLS negotiation: a node takes TLS on its port or not at all.
        SslMode::Disable | SslMode::Prefer => None,
        mode => Some(Tls {
            mode,
            ca_path: trimmed(profile.ssl_ca_path.as_deref()).map(str::to_string),
            cert_path: trimmed(profile.options.ssl_cert_path.as_deref()).map(str::to_string),
            key_path: trimmed(profile.options.ssl_key_path.as_deref()).map(str::to_string),
            server_name: target.tls_host(),
        }),
    };
    let ttl = Duration::from_secs(u64::from(DRIVER.descriptor().command_ttl_secs));
    let credentials = trimmed(profile.user.as_deref()).map(|user| Credentials {
        user: user.to_string(),
        password: match CommandPassword::for_profile_ttl(profile, target.password, ttl) {
            Some(command) => Password::Command(command),
            None if profile.options.password_mode() == PasswordMode::None => {
                Password::Fixed(String::new())
            }
            None => Password::Fixed(target.password.unwrap_or_default().to_string()),
        },
    });
    let dial = Dial {
        host: target.host(),
        port: target.port(),
        single_node: params.single_node || target.host_override.is_some(),
        tls,
        credentials,
        keyspace: trimmed(profile.database.as_deref()).map(str::to_string),
        connect_timeout: crate::commands::connections::connect_timeout(profile),
        options: RunOptions {
            consistency: params.consistency.level(),
            serial: params.serial_consistency.level(),
            timeout: params.request_timeout(),
            tracing: false,
        },
        local_datacenter: params.local_datacenter().map(str::to_string),
        compression: params.compression,
        keepalive: KEEPALIVE,
    };
    Ok((dial, params))
}

/// How Safe Mode confirms a write: nobody types a Command password.
pub fn confirm_of(profile: &ConnectionProfile) -> SafeConfirm {
    if profile.options.password_mode() == PasswordMode::Command {
        SafeConfirm::Passcode
    } else {
        SafeConfirm::Password
    }
}

/// Opens a Cassandra or ScyllaDB session.
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    let profile = target.profile;
    let (dial, params) = dial(&target)?;
    let session = CassandraSession::open(
        &ScyllaConnector,
        &dial,
        params,
        effective_read_only(profile),
        confirm_of(profile),
        trimmed(profile.options.startup_sql.as_deref()),
    )
    .await?;
    Ok(Session::Sql(Arc::new(session)))
}

/// The Cassandra settings of a profile (`CassandraParams`) and both files of a client
/// certificate.
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    CassandraParams::from_driver_params(profile.options.driver_params.as_ref())?;
    if trimmed(profile.options.ssl_cert_path.as_deref()).is_some()
        != trimmed(profile.options.ssl_key_path.as_deref()).is_some()
    {
        return Err(AppError::InvalidInput(
            "Cassandra: a client certificate needs both its certificate and its key file".into(),
        ));
    }
    Ok(())
}

/// Cassandra has no calls of the connection form.
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

    pub(crate) fn profile(json: serde_json::Value) -> ConnectionProfile {
        let mut base = serde_json::json!({
            "id": "6f9619ff-8b86-d011-b42d-00c04fc964ff", "name": "cass", "color": null,
            "group": null, "driver": "cassandra", "host": "db.example.com", "port": null,
            "user": null, "database": null, "sslCaPath": null, "sqlitePath": null, "ssh": null,
            "lastConnectedAt": null
        });
        for (k, v) in json.as_object().expect("fields") {
            base[k] = v.clone();
        }
        serde_json::from_value(base).expect("a profile")
    }

    fn target<'a>(p: &'a ConnectionProfile, password: Option<&'static str>) -> ConnectTarget<'a> {
        ConnectTarget {
            profile: p,
            password,
            host_override: None,
        }
    }

    #[test]
    fn dial_reads_the_profile() {
        let p = profile(serde_json::json!({
            "user": "cassandra", "database": " shop ", "ssl": "verify-full",
            "sslCaPath": "ca.pem",
            "options": { "driverParams": { "consistency": "QUORUM", "localDatacenter": "dc1" } }
        }));
        let (dial, _) = dial(&target(&p, Some("pw"))).unwrap();
        assert_eq!((dial.host.as_str(), dial.port), ("db.example.com", 9042));
        assert!(!dial.single_node);
        assert_eq!(dial.keyspace.as_deref(), Some("shop"));
        let tls = dial.tls.expect("TLS");
        assert_eq!(tls.mode, SslMode::VerifyFull);
        assert_eq!(tls.ca_path.as_deref(), Some("ca.pem"));
        assert_eq!(tls.server_name, "db.example.com");
        let creds = dial.credentials.expect("credentials");
        assert_eq!(creds.user, "cassandra");
        assert!(matches!(creds.password, Password::Fixed(ref p) if p == "pw"));
        assert_eq!(
            dial.options.consistency,
            ::scylla::frame::types::Consistency::Quorum
        );
        assert_eq!(dial.local_datacenter.as_deref(), Some("dc1"));
        assert_eq!(confirm_of(&p), SafeConfirm::Password);
    }

    #[test]
    fn a_tunnel_pins_one_node_and_tls_checks_the_real_host() {
        let p = profile(serde_json::json!({ "ssl": "require" }));
        let t = ConnectTarget {
            profile: &p,
            password: None,
            host_override: Some(("127.0.0.1".into(), 40000)),
        };
        let (dial, _) = dial(&t).unwrap();
        assert_eq!((dial.host.as_str(), dial.port), ("127.0.0.1", 40000));
        assert!(dial.single_node);
        assert_eq!(dial.tls.expect("TLS").server_name, "db.example.com");
        assert!(dial.credentials.is_none(), "no user: no authenticator");
        let plain = profile(serde_json::json!({ "ssl": "disable" }));
        assert!(dial_of(&plain).tls.is_none());
    }

    fn dial_of(p: &ConnectionProfile) -> Dial {
        dial(&target(p, None)).unwrap().0
    }

    #[test]
    fn a_command_password_runs_for_each_connection() {
        let p = profile(serde_json::json!({
            "user": "u",
            "options": { "passwordMode": "command", "passwordCommand": "echo pw" }
        }));
        assert!(matches!(
            dial_of(&p).credentials.unwrap().password,
            Password::Command(_)
        ));
        assert_eq!(confirm_of(&p), SafeConfirm::Passcode);
        let none = profile(serde_json::json!({
            "user": "u", "options": { "passwordMode": "none" }
        }));
        assert!(matches!(
            dial_of(&none).credentials.unwrap().password,
            Password::Fixed(ref p) if p.is_empty()
        ));
    }

    #[test]
    fn validate_checks_the_params_and_the_certificate_pair() {
        assert!(validate(&profile(serde_json::json!({}))).is_ok());
        assert!(registry::validate(&profile(serde_json::json!({}))).is_ok());
        assert!(registry::validate(&profile(serde_json::json!({ "host": null }))).is_err());
        let bad = profile(serde_json::json!({
            "options": { "driverParams": { "pageSize": 0 } }
        }));
        assert!(validate(&bad).is_err());
        let half = profile(serde_json::json!({ "options": { "sslCertPath": "c.pem" } }));
        assert!(validate(&half).is_err());
    }

    #[tokio::test]
    async fn util_knows_no_call_and_counts_it_a_write() {
        let p = profile(serde_json::json!({}));
        let input = UtilInput {
            profile: &p,
            secret: None,
        };
        assert!(matches!(
            util(input, "listKeyspaces", serde_json::Value::Null).await,
            Err(AppError::Unsupported(_))
        ));
        assert_eq!(util_kind("listKeyspaces"), CallKind::Write);
        assert_eq!(registry::util_kind(DRIVER, "anything"), CallKind::Write);
    }
}
