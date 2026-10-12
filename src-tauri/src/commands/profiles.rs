use std::time::Instant;

use tauri::State;
use uuid::Uuid;

use crate::commands::connections::{connect_timeout, open_profile_tunnel, tunnel_config};
use crate::drivers::{self, ConnectTarget, ServerInfo};
use crate::error::AppError;
use crate::secrets::SecretKind;
use crate::state::AppState;
use crate::tunnel::{TunnelAuth, TunnelHandle};
use crate::types::profile::{
    ConnectionProfile, Driver, ProfileInput, SecretStatus, SshAuth, TestResult,
};

#[tauri::command]
pub async fn list_profiles(state: State<'_, AppState>) -> Result<Vec<ConnectionProfile>, AppError> {
    log::debug!("list_profiles");
    state.appdb.list_profiles()
}

#[tauri::command]
pub async fn get_profile(
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<ConnectionProfile, AppError> {
    state
        .appdb
        .get_profile(id)?
        .ok_or_else(|| AppError::NotFound(format!("profile {id}")))
}

fn validate(profile: &ConnectionProfile) -> Result<(), AppError> {
    if profile.name.trim().is_empty() {
        return Err(AppError::InvalidInput("Connection name is required".into()));
    }
    match profile.driver {
        crate::types::profile::Driver::Sqlite => {
            if profile
                .sqlite_path
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
            {
                return Err(AppError::InvalidInput(
                    "SQLite file path is required".into(),
                ));
            }
        }
        _ => {
            if profile.host.as_deref().unwrap_or("").trim().is_empty() {
                return Err(AppError::InvalidInput("Host is required".into()));
            }
            if profile.user.as_deref().unwrap_or("").trim().is_empty() {
                return Err(AppError::InvalidInput("User is required".into()));
            }
        }
    }
    Ok(())
}

/// Saves the secrets of a profile being saved. SSH password / passphrase: `None` (what the
/// renderer sends for an untouched field) keeps the stored value, `""` removes it (an empty SSH
/// secret is never valid, so it can only mean "forget it"), anything else replaces it.
pub async fn store_secrets(state: &AppState, input: &ProfileInput) -> Result<(), AppError> {
    let id = input.profile.id;
    if input.clear_password {
        state.secrets.delete(id, SecretKind::DbPassword).await?;
    } else if let Some(p) = &input.password {
        state.secrets.set(id, SecretKind::DbPassword, p).await?;
    }
    for (value, kind) in [
        (&input.ssh_password, SecretKind::SshPassword),
        (&input.ssh_passphrase, SecretKind::SshPassphrase),
    ] {
        match value.as_deref() {
            None => {}
            Some("") => state.secrets.delete(id, kind).await?,
            Some(secret) => state.secrets.set(id, kind, secret).await?,
        }
    }
    Ok(())
}

/// An SSH secret for a test: the one typed in the dialog, none when it was cleared (`""`), else
/// the stored one.
async fn typed_or_stored(
    state: &AppState,
    id: Uuid,
    typed: &Option<String>,
    kind: SecretKind,
) -> Result<Option<String>, AppError> {
    match typed.as_deref() {
        Some("") => Ok(None),
        Some(secret) => Ok(Some(secret.to_string())),
        None => state.secrets.get(id, kind).await,
    }
}

#[tauri::command]
pub async fn save_profile(
    state: State<'_, AppState>,
    input: ProfileInput,
) -> Result<ConnectionProfile, AppError> {
    validate(&input.profile)?;
    let stored = state.appdb.upsert_profile(&input.profile)?;
    store_secrets(&state, &input).await?;
    Ok(stored)
}

#[tauri::command]
pub async fn delete_profile(state: State<'_, AppState>, id: Uuid) -> Result<(), AppError> {
    state.appdb.delete_profile(id)?;
    state.secrets.delete_all(id).await?;
    Ok(())
}

#[tauri::command]
pub async fn duplicate_profile(
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<ConnectionProfile, AppError> {
    let mut p = state
        .appdb
        .get_profile(id)?
        .ok_or_else(|| AppError::NotFound(format!("profile {id}")))?;
    p.id = Uuid::new_v4();
    p.name = format!("{} copy", p.name);
    p.sort_order = 0;
    p.last_connected_at = None;
    let stored = state.appdb.upsert_profile(&p)?;
    state.secrets.copy_all(id, p.id).await?;
    Ok(stored)
}

#[tauri::command]
pub async fn reorder_profiles(state: State<'_, AppState>, ids: Vec<Uuid>) -> Result<(), AppError> {
    state.appdb.reorder_profiles(&ids)
}

#[tauri::command]
pub async fn get_secret_status(
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<SecretStatus, AppError> {
    state.secrets.status(id).await
}

/// Resolves the password to use: the one typed in the dialog, else the stored one.
pub async fn resolve_password(
    state: &AppState,
    input: &ProfileInput,
) -> Result<Option<String>, AppError> {
    if input.clear_password {
        return Ok(None);
    }
    if let Some(p) = &input.password {
        return Ok(Some(p.clone()));
    }
    state
        .secrets
        .get(input.profile.id, SecretKind::DbPassword)
        .await
}

/// Resolves the SSH secret the tunnel needs: the one typed in the dialog, none when the dialog
/// clears it (`""`), else the stored one.
pub async fn resolve_tunnel_auth(
    state: &AppState,
    input: &ProfileInput,
) -> Result<TunnelAuth, AppError> {
    let Some(ssh) = tunnel_config(&input.profile) else {
        return Ok(TunnelAuth::default());
    };
    let id = input.profile.id;
    let mut auth = TunnelAuth::default();
    match ssh.auth {
        SshAuth::Password => {
            auth.password =
                typed_or_stored(state, id, &input.ssh_password, SecretKind::SshPassword).await?;
        }
        SshAuth::Key => {
            auth.passphrase =
                typed_or_stored(state, id, &input.ssh_passphrase, SecretKind::SshPassphrase)
                    .await?;
        }
        SshAuth::Agent => {}
    }
    Ok(auth)
}

/// Connects once, reads the server version and disconnects.
async fn probe(
    profile: &ConnectionProfile,
    password: Option<&str>,
    host_override: Option<(String, u16)>,
) -> Result<ServerInfo, AppError> {
    let session = drivers::connect(ConnectTarget {
        profile,
        password,
        host_override,
    })
    .await?;
    let info = session.server_info().await;
    session.close().await;
    info
}

/// Opens the temporary tunnel `test_connection` probes through. It always listens on an
/// ephemeral local port: only the probe uses it, and the profile's pinned `local_port` may be
/// held by the same profile's live connection (or by an overlapping test).
pub async fn open_test_tunnel(
    state: &AppState,
    profile: &ConnectionProfile,
    auth: TunnelAuth,
) -> Result<Option<TunnelHandle>, AppError> {
    let mut profile = profile.clone();
    if let Some(ssh) = profile.ssh.as_mut() {
        ssh.local_port = None;
    }
    open_profile_tunnel(state, &profile, auth).await
}

/// Tries the dialog's settings (through a temporary SSH tunnel when configured). Failures come
/// back as `ok: false`, except an unknown or changed SSH host key: that is returned as the
/// `HostKey` error so the renderer can show the fingerprint prompt, then call `accept_host_key`
/// with `input.profile.id` and test again.
#[tauri::command]
pub async fn test_connection(
    state: State<'_, AppState>,
    input: ProfileInput,
) -> Result<TestResult, AppError> {
    run_connection_test(&state, input).await
}

/// The body of [`test_connection`], callable without a Tauri runtime.
pub async fn run_connection_test(
    state: &AppState,
    input: ProfileInput,
) -> Result<TestResult, AppError> {
    validate(&input.profile)?;
    log::info!(
        "test_connection {} ({:?})",
        input.profile.name,
        input.profile.driver
    );
    let password = resolve_password(state, &input).await?;
    let ssh_auth = resolve_tunnel_auth(state, &input).await?;
    let tunnel_used = tunnel_config(&input.profile).is_some();
    let timeout = connect_timeout(&input.profile);
    let started = Instant::now();
    let result = async {
        if input.profile.driver == Driver::Sqlite {
            // Probing would create a missing file and report success.
            drivers::sqlite::check_database_file(&input.profile)?;
        }
        let tunnel = open_test_tunnel(state, &input.profile, ssh_auth).await?;
        let host_override = tunnel.as_ref().map(TunnelHandle::connect_addr);
        let attempt = Instant::now();
        let outcome = tokio::time::timeout(
            timeout,
            probe(&input.profile, password.as_deref(), host_override),
        )
        .await
        .unwrap_or(Err(AppError::Timeout));
        match tunnel {
            Some(t) => {
                let outcome = outcome.map_err(|e| t.explain_error(e, attempt));
                t.close().await;
                outcome
            }
            None => outcome,
        }
    }
    .await;
    let latency_ms = started.elapsed().as_millis() as u64;
    let failed = |message: String| TestResult {
        ok: false,
        server_version: None,
        latency_ms,
        message,
        tunnel_used,
    };
    match result {
        Ok(info) => Ok(TestResult {
            ok: true,
            server_version: Some(info.version.clone()),
            latency_ms,
            message: if tunnel_used {
                format!("Connected to {} through the SSH tunnel", info.version)
            } else {
                format!("Connected to {}", info.version)
            },
            tunnel_used,
        }),
        Err(e @ AppError::HostKey { .. }) => Err(e),
        Err(AppError::Timeout) => Ok(failed(format!("Timed out after {}s", timeout.as_secs()))),
        Err(AppError::NotFound(message)) => Ok(failed(message)),
        Err(e) => Ok(failed(e.to_string())),
    }
}

/// "New…" in the connection dialog: creates an empty SQLite database at `path` (an existing file
/// is left untouched). Connecting never creates files (see `check_database_file`).
#[tauri::command]
pub async fn create_sqlite_database(path: String) -> Result<(), AppError> {
    drivers::sqlite::create_database_file(&path).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appdb::AppDb;
    use crate::secrets::{Backend, Secrets};
    use crate::types::profile::{ProfileOptions, SslMode};

    fn sqlite_input(path: &str) -> ProfileInput {
        ProfileInput {
            profile: ConnectionProfile {
                id: Uuid::new_v4(),
                name: "lite".into(),
                color: None,
                group: None,
                driver: Driver::Sqlite,
                host: None,
                port: None,
                user: None,
                database: None,
                ssl: SslMode::default(),
                ssl_ca_path: None,
                sqlite_path: Some(path.into()),
                ssh: None,
                options: ProfileOptions {
                    sqlite_foreign_keys: true,
                    ..ProfileOptions::default()
                },
                sort_order: 0,
                created_at: String::new(),
                updated_at: String::new(),
                last_connected_at: None,
            },
            password: None,
            ssh_password: None,
            ssh_passphrase: None,
            clear_password: false,
        }
    }

    #[tokio::test]
    async fn test_connection_never_creates_a_missing_sqlite_file() {
        let dir = tempfile::tempdir().unwrap();
        let appdb = AppDb::open_in_memory().unwrap();
        // Never the Keychain in unit tests.
        let state = AppState::new(appdb.clone(), Secrets::new(Backend::File, appdb));
        let path = dir.path().join("moved.sqlite");
        let path_str = path.to_string_lossy().into_owned();

        let result = run_connection_test(&state, sqlite_input(&path_str))
            .await
            .unwrap();
        assert!(!result.ok);
        assert!(
            result.message.starts_with("SQLite database file not found"),
            "{}",
            result.message
        );
        assert!(!path.exists());

        drivers::sqlite::create_database_file(&path_str)
            .await
            .unwrap();
        let result = run_connection_test(&state, sqlite_input(&path_str))
            .await
            .unwrap();
        assert!(result.ok, "{}", result.message);
        assert!(result.server_version.unwrap().starts_with("SQLite"));
    }
}
