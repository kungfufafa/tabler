//! The entry points `drivers::registry` calls (docs/drivers/FOUNDATION.md).

use std::sync::Arc;

use super::auth::{public_key_fingerprint, read_private_key};
use super::params::{SnowflakeAuth, SnowflakeParams};
use super::session::SnowflakeSession;
use super::signin::SignIn;
use crate::drivers::native::{CallKind, unknown_call};
use crate::drivers::registry::UtilInput;
use crate::drivers::{ConnectTarget, Session};
use crate::error::AppError;
use crate::sqlgen::dialect::snowflake as sf;
use crate::types::profile::{ConnectionProfile, Driver};

const DRIVER: Driver = Driver::Snowflake;

/// The form's calls: the private key's fingerprint (read from the file, nothing is sent), and
/// the warehouses, roles and databases the user may pick (one sign-in, then `SHOW`, which needs
/// no warehouse).
const KEY_FINGERPRINT: &str = "keyFingerprint";
const LIST_WAREHOUSES: &str = "listWarehouses";
const LIST_ROLES: &str = "listRoles";
const LIST_DATABASES: &str = "listDatabases";

/// Opens a Snowflake session: the account's endpoint over HTTPS (or the profile's own), signed
/// in with the key pair, a PAT, an OAuth token (a Command one is run again when it expires) or
/// the password.
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError> {
    // Boxed: the connector's sign-in futures are deep, and the MCP server's request future
    // that opens a session would otherwise pass the compiler's layout depth.
    let session = Box::pin(SnowflakeSession::open(&target)).await?;
    Ok(Session::Sql(Arc::new(session)))
}

/// The Snowflake settings of a profile (`SnowflakeParams`: the account identifier, the key file
/// of key-pair sign-in, the endpoint and session parameters).
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    SnowflakeParams::from_driver_params(profile.options.driver_params.as_ref()).map(|_| ())
}

/// A call of the connection form.
pub async fn util(
    input: UtilInput<'_>,
    method: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    match method {
        KEY_FINGERPRINT => {
            // The file the form names now, or the profile's.
            let path = match args.get("privateKeyPath").and_then(|v| v.as_str()) {
                Some(path) => path.to_string(),
                None => {
                    let params = SnowflakeParams::from_driver_params(
                        input.profile.options.driver_params.as_ref(),
                    )?;
                    if params.auth != SnowflakeAuth::KeyPair {
                        return Err(AppError::InvalidInput(
                            "Snowflake: the profile does not sign in with a key pair".into(),
                        ));
                    }
                    params.private_key_path.unwrap_or_default()
                }
            };
            let key = read_private_key(&path)?;
            let fingerprint = public_key_fingerprint(&key.pem)?;
            Ok(serde_json::json!({ "fingerprint": fingerprint }))
        }
        LIST_WAREHOUSES | LIST_ROLES | LIST_DATABASES => {
            let sql = match method {
                LIST_WAREHOUSES => sf::SHOW_WAREHOUSES,
                LIST_ROLES => sf::SHOW_ROLES,
                _ => sf::SHOW_DATABASES,
            };
            let signin = SignIn::for_util(&input)?;
            let conn = Box::pin(signin.connect()).await?;
            let rows = conn.query(sql, &[], None).await?.into_rows();
            let names: Vec<String> = rows.iter().filter_map(|r| r.text("name")).collect();
            Ok(serde_json::json!(names))
        }
        _ => Err(unknown_call(DRIVER, method)),
    }
}

/// Whether a `util` call only reads; anything unknown changes something.
pub fn util_kind(method: &str) -> CallKind {
    match method {
        KEY_FINGERPRINT | LIST_WAREHOUSES | LIST_ROLES | LIST_DATABASES => CallKind::Read,
        _ => CallKind::Write,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::registry;
    use crate::drivers::snowflake::testing::{
        Answer, FakeColumn, FakeSnowflake, KEY_FINGERPRINT as FP, Login, fixture_path,
    };

    pub(crate) fn profile(json: serde_json::Value) -> ConnectionProfile {
        let mut base = serde_json::json!({
            "id": "6f9619ff-8b86-d011-b42d-00c04fc964ff", "name": "sf", "color": null,
            "group": null, "driver": "snowflake", "host": null, "port": null,
            "user": "tabler_user", "database": null, "sslCaPath": null, "sqlitePath": null,
            "ssh": null, "lastConnectedAt": null
        });
        for (k, v) in json.as_object().expect("fields") {
            base[k] = v.clone();
        }
        serde_json::from_value(base).expect("a profile")
    }

    #[test]
    fn validate_reads_the_params() {
        let key = profile(serde_json::json!({ "options": { "driverParams": {
            "account": "myorg-acct", "privateKeyPath": "C:/keys/rsa_key.p8" } } }));
        assert!(validate(&key).is_ok());
        assert!(registry::validate(&key).is_ok());
        // No account, no key file for key pair, an unknown method.
        for params in [
            serde_json::json!({}),
            serde_json::json!({ "account": "a" }),
            serde_json::json!({ "account": "a", "auth": "sso" }),
            serde_json::json!({ "account": "x;DROP", "auth": "pat" }),
        ] {
            let p = profile(serde_json::json!({ "options": { "driverParams": params } }));
            assert!(validate(&p).is_err(), "{p:?}");
        }
        let no_params = profile(serde_json::json!({}));
        assert!(validate(&no_params).is_err());
        // The registry's common checks: the user is required.
        let no_user = profile(
            serde_json::json!({ "user": null, "options": { "driverParams": {
            "account": "a", "auth": "pat" } } }),
        );
        assert!(registry::validate(&no_user).is_err());
    }

    #[tokio::test]
    async fn the_form_reads_the_fingerprint_and_lists_what_to_pick() {
        let path = fixture_path("rsa_key.p8");
        let p = profile(serde_json::json!({ "options": { "driverParams": {
            "account": "myorg-acct", "privateKeyPath": path } } }));
        let input = UtilInput {
            profile: &p,
            secret: None,
        };
        let out = util(input, KEY_FINGERPRINT, serde_json::Value::Null)
            .await
            .unwrap();
        assert_eq!(out["fingerprint"], FP);
        // The form's file wins over the profile's.
        let input = UtilInput {
            profile: &p,
            secret: None,
        };
        let enc = fixture_path("rsa_key_encrypted.p8");
        let err = util(
            input,
            KEY_FINGERPRINT,
            serde_json::json!({ "privateKeyPath": enc }),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("encrypted"), "{err}");

        crate::testkit::install_crypto();
        let fake = FakeSnowflake::start(
            "myorg-acct",
            "tabler_user",
            Login::Password {
                password: "pat-1".into(),
                passcode: None,
            },
        )
        .await;
        fake.answer(
            sf::SHOW_WAREHOUSES,
            Answer::rows(
                vec![
                    FakeColumn::new("name", "text"),
                    FakeColumn::new("state", "text"),
                ],
                vec![
                    vec![Some("COMPUTE_WH".into()), Some("SUSPENDED".into())],
                    vec![Some("ETL".into()), Some("STARTED".into())],
                ],
            ),
        );
        let p = profile(serde_json::json!({ "options": { "driverParams": {
            "account": "myorg-acct", "auth": "pat", "baseUrl": fake.url() } } }));
        let input = UtilInput {
            profile: &p,
            secret: Some("pat-1"),
        };
        let out = util(input, LIST_WAREHOUSES, serde_json::Value::Null)
            .await
            .unwrap();
        assert_eq!(out, serde_json::json!(["COMPUTE_WH", "ETL"]));
        let input = UtilInput {
            profile: &p,
            secret: Some("wrong"),
        };
        assert!(matches!(
            util(input, LIST_ROLES, serde_json::Value::Null).await,
            Err(AppError::Auth(_))
        ));
    }

    #[tokio::test]
    async fn unknown_calls_are_writes() {
        let p = profile(serde_json::json!({}));
        let input = UtilInput {
            profile: &p,
            secret: None,
        };
        assert!(matches!(
            util(input, "dropEverything", serde_json::Value::Null).await,
            Err(AppError::Unsupported(_))
        ));
        assert_eq!(util_kind("dropEverything"), CallKind::Write);
        for read in [KEY_FINGERPRINT, LIST_WAREHOUSES, LIST_ROLES, LIST_DATABASES] {
            assert_eq!(util_kind(read), CallKind::Read);
            assert_eq!(registry::util_kind(DRIVER, read), CallKind::Read);
        }
    }
}
