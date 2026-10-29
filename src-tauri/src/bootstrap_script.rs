//! The bootstrap CLI script of a connection (§1.6, b546): a shell script run before every
//! connect and test, e.g. to start a proxy or to copy (rsync) a SQLite file into place. It runs
//! through the login shell with a 60 s limit (see [`secrets::run_bootstrap_script`]); a failure
//! aborts the connect with the script's output.

use crate::error::AppError;
use crate::secrets;
use crate::types::driver::Location;
use crate::types::profile::ConnectionProfile;

/// The variables a script sees about the connection it runs for.
pub fn script_env(profile: &ConnectionProfile) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("TABLER_CONNECTION", profile.name.clone()),
        (
            "TABLER_DRIVER",
            serde_json::to_value(profile.driver)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
        ),
    ];
    let optional = [
        ("TABLER_HOST", profile.host.clone()),
        ("TABLER_PORT", profile.port.map(|p| p.to_string())),
        ("TABLER_USER", profile.user.clone()),
        ("TABLER_DATABASE", profile.database.clone()),
        ("TABLER_SQLITE_PATH", profile.sqlite_path.clone()),
    ];
    env.extend(
        optional
            .into_iter()
            .filter_map(|(k, v)| v.filter(|v| !v.is_empty()).map(|v| (k, v))),
    );
    // The other drivers' file (DuckDB…) and settings; SQLite's file stays TABLER_SQLITE_PATH.
    // The params never hold a secret (`drivers::registry::validate`).
    if !profile.driver.is_core() {
        let more = [
            (
                "TABLER_FILE",
                profile
                    .sqlite_path
                    .clone()
                    .filter(|_| profile.driver.descriptor().location == Location::File),
            ),
            (
                "TABLER_DRIVER_PARAMS",
                profile
                    .options
                    .driver_params
                    .as_ref()
                    .filter(|p| !p.0.is_empty())
                    .and_then(|p| serde_json::to_string(p).ok()),
            ),
        ];
        env.extend(
            more.into_iter()
                .filter_map(|(k, v)| v.filter(|v| !v.is_empty()).map(|v| (k, v))),
        );
    }
    env
}

/// Runs the profile's bootstrap script, if it has one; call it before anything else of a connect
/// (the SQLite file check and the SSH tunnel included).
pub async fn run_for_profile(profile: &ConnectionProfile) -> Result<(), AppError> {
    let Some(script) = profile.options.bootstrap_script() else {
        return Ok(());
    };
    log::info!("running the bootstrap script of {}", profile.name);
    secrets::run_bootstrap_script(
        script,
        &script_env(profile),
        secrets::BOOTSTRAP_SCRIPT_TIMEOUT,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::profile::{Driver, ProfileOptions, SslMode};

    fn profile(script: Option<&str>) -> ConnectionProfile {
        ConnectionProfile {
            id: uuid::Uuid::nil(),
            name: "lite".into(),
            color: None,
            group: None,
            driver: Driver::Sqlite,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: Some("/tmp/x.sqlite".into()),
            ssh: None,
            options: ProfileOptions {
                bootstrap_script: script.map(str::to_string),
                ..ProfileOptions::default()
            },
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    #[test]
    fn env_names_the_connection() {
        let env = script_env(&profile(None));
        assert!(env.contains(&("TABLER_CONNECTION", "lite".into())));
        assert!(env.contains(&("TABLER_DRIVER", "sqlite".into())));
        assert!(env.contains(&("TABLER_SQLITE_PATH", "/tmp/x.sqlite".into())));
        assert!(!env.iter().any(|(k, _)| *k == "TABLER_HOST"));
        // SQLite's variables are as before.
        assert!(!env.iter().any(|(k, _)| k.starts_with("TABLER_FILE")));
        assert!(!env.iter().any(|(k, _)| *k == "TABLER_DRIVER_PARAMS"));
    }

    #[test]
    fn env_carries_the_file_and_the_driver_params() {
        use crate::types::profile::DriverParams;
        let mut duck = profile(None);
        duck.driver = Driver::Duckdb;
        duck.sqlite_path = Some("/data/w.duckdb".into());
        let env = script_env(&duck);
        assert!(env.contains(&("TABLER_DRIVER", "duckdb".into())));
        assert!(env.contains(&("TABLER_FILE", "/data/w.duckdb".into())));
        assert!(!env.iter().any(|(k, _)| *k == "TABLER_DRIVER_PARAMS"));

        let mut warehouse = profile(None);
        warehouse.driver = Driver::Snowflake;
        warehouse.sqlite_path = None;
        let mut params = serde_json::Map::new();
        params.insert("account".into(), "xy12345".into());
        params.insert("warehouse".into(), "COMPUTE_WH".into());
        warehouse.options.driver_params = Some(DriverParams(params));
        let env = script_env(&warehouse);
        let json = env
            .iter()
            .find(|(k, _)| *k == "TABLER_DRIVER_PARAMS")
            .map(|(_, v)| v.clone())
            .unwrap();
        let back: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(back["account"], "xy12345");
        assert_eq!(back["warehouse"], "COMPUTE_WH");
        assert!(!env.iter().any(|(k, _)| *k == "TABLER_FILE"));
        // A file path left from another driver is not a cloud driver's file.
        warehouse.sqlite_path = Some("/tmp/old.sqlite".into());
        assert!(
            !script_env(&warehouse)
                .iter()
                .any(|(k, _)| *k == "TABLER_FILE")
        );
    }

    /// The use case of the spec: copy the database into place before the file is checked.
    #[tokio::test]
    async fn a_script_can_put_the_sqlite_file_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("remote.sqlite");
        let dest = dir.path().join("local.sqlite");
        crate::drivers::sqlite::create_database_file(&src.to_string_lossy())
            .await
            .unwrap();
        let mut p = profile(Some(&format!(
            "cp \"{}\" \"$TABLER_SQLITE_PATH\"",
            src.display()
        )));
        p.sqlite_path = Some(dest.to_string_lossy().into_owned());
        assert!(crate::drivers::sqlite::check_database_file(&p).is_err());
        run_for_profile(&p).await.unwrap();
        crate::drivers::sqlite::check_database_file(&p).unwrap();

        p.options.bootstrap_script = Some("echo 'rsync: connection refused' >&2; exit 12".into());
        let err = run_for_profile(&p).await.unwrap_err();
        assert!(
            err.to_string().contains("rsync: connection refused"),
            "{err}"
        );
        // No script: nothing to do.
        run_for_profile(&profile(Some("   "))).await.unwrap();
    }
}
