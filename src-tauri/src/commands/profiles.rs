use std::time::Instant;

use tauri::State;
use uuid::Uuid;

use crate::drivers::{self, ConnectTarget};
use crate::error::AppError;
use crate::secrets::SecretKind;
use crate::state::AppState;
use crate::types::profile::{ConnectionProfile, ProfileInput, SecretStatus, TestResult};

#[tauri::command]
pub async fn list_profiles(state: State<'_, AppState>) -> Result<Vec<ConnectionProfile>, AppError> {
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

async fn store_secrets(state: &AppState, input: &ProfileInput) -> Result<(), AppError> {
    let id = input.profile.id;
    if input.clear_password {
        state.secrets.delete(id, SecretKind::DbPassword).await?;
    } else if let Some(p) = &input.password {
        state.secrets.set(id, SecretKind::DbPassword, p).await?;
    }
    if let Some(p) = &input.ssh_password {
        state.secrets.set(id, SecretKind::SshPassword, p).await?;
    }
    if let Some(p) = &input.ssh_passphrase {
        state.secrets.set(id, SecretKind::SshPassphrase, p).await?;
    }
    Ok(())
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

#[tauri::command]
pub async fn test_connection(
    state: State<'_, AppState>,
    input: ProfileInput,
) -> Result<TestResult, AppError> {
    validate(&input.profile)?;
    let password = resolve_password(&state, &input).await?;
    let started = Instant::now();
    let attempt = async {
        let session = drivers::connect(ConnectTarget {
            profile: &input.profile,
            password: password.as_deref(),
            host_override: None,
        })
        .await?;
        let info = session.server_info().await;
        session.close().await;
        info
    };
    let timeout = std::time::Duration::from_secs(
        input.profile.options.connect_timeout_secs.unwrap_or(15) as u64,
    );
    let result = tokio::time::timeout(timeout, attempt).await;
    let latency_ms = started.elapsed().as_millis() as u64;
    Ok(match result {
        Ok(Ok(info)) => TestResult {
            ok: true,
            server_version: Some(info.version.clone()),
            latency_ms,
            message: format!("Connected to {}", info.version),
            tunnel_used: false,
        },
        Ok(Err(e)) => TestResult {
            ok: false,
            server_version: None,
            latency_ms,
            message: e.to_string(),
            tunnel_used: false,
        },
        Err(_) => TestResult {
            ok: false,
            server_version: None,
            latency_ms,
            message: format!("Timed out after {}s", timeout.as_secs()),
            tunnel_used: false,
        },
    })
}
