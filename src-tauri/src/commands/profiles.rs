use std::time::Instant;

use tauri::State;
use uuid::Uuid;

use crate::commands::connections::{connect_timeout, open_profile_tunnel, tunnel_config};
use crate::drivers::{self, ConnectTarget, ServerInfo};
use crate::error::AppError;
use crate::profiles_io::{self, ExportedConnection, ExportedSecrets, Payload, ReadOutcome};
use crate::secrets::SecretKind;
use crate::state::AppState;
use crate::tunnel::{TunnelAuth, TunnelHandle};
use crate::types::profile::{
    ConnectionProfile, ConnectionsFilePreview, Driver, ExportConnectionsRequest, PasswordMode,
    ProfileInput, SecretStatus, SshAuth, TestResult,
};

/// Settings key of the connection group names (so empty groups exist): a JSON array of strings.
pub const GROUPS_SETTING: &str = "connectionGroups";

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

/// Saves the secrets of a profile being saved. DB password: only "Store in keychain" profiles
/// keep one (the other modes remove a stored one and ignore what is typed). SSH password /
/// passphrase: `None` (what the renderer sends for an untouched field) keeps the stored value,
/// `""` removes it (an empty SSH secret is never valid, so it can only mean "forget it"),
/// anything else replaces it.
pub async fn store_secrets(state: &AppState, input: &ProfileInput) -> Result<(), AppError> {
    let id = input.profile.id;
    let mode = input.profile.options.password_mode();
    if mode != PasswordMode::Ask {
        // A password typed for an earlier "Ask every time" setting is no longer the one to use.
        state.set_session_password(id, None);
    }
    if input.clear_password || mode != PasswordMode::Keychain {
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
    state.set_session_password(id, None);
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

/// Resolves the password a test uses, by the dialog's password mode: Keychain — the one typed
/// in the dialog, else the stored one; Ask every time — the one typed in the prompt, else the
/// one remembered this session; No password — none.
pub async fn resolve_password(
    state: &AppState,
    input: &ProfileInput,
) -> Result<Option<String>, AppError> {
    match input.profile.options.password_mode() {
        PasswordMode::None => return Ok(None),
        PasswordMode::Ask => {
            return Ok(input
                .password
                .clone()
                .or_else(|| state.session_password(input.profile.id)));
        }
        PasswordMode::Keychain => {}
    }
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

// ----- groups -----------------------------------------------------------------------------------

/// The group names saved in the settings (`GROUPS_SETTING`).
pub fn stored_groups(state: &AppState) -> Result<Vec<String>, AppError> {
    Ok(state
        .appdb
        .get_setting(GROUPS_SETTING)?
        .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        .unwrap_or_default())
}

/// "Move to Group ▸ …", dragging rows onto a group, renaming or deleting a group: puts the
/// profiles in `group` (`None` or blank: no group). Unknown ids are skipped.
#[tauri::command]
pub async fn move_profiles_to_group(
    state: State<'_, AppState>,
    ids: Vec<Uuid>,
    group: Option<String>,
) -> Result<Vec<ConnectionProfile>, AppError> {
    move_to_group(&state, &ids, group)
}

pub fn move_to_group(
    state: &AppState,
    ids: &[Uuid],
    group: Option<String>,
) -> Result<Vec<ConnectionProfile>, AppError> {
    let group = group
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty());
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(mut p) = state.appdb.get_profile(*id)? else {
            continue;
        };
        if p.group != group {
            p.group = group.clone();
            p = state.appdb.upsert_profile(&p)?;
        }
        out.push(p);
    }
    Ok(out)
}

// ----- export / import ----------------------------------------------------------------------------

fn io_error(path: &str, e: std::io::Error) -> AppError {
    AppError::Io(format!("{path}: {e}"))
}

/// "Export Connections ▸ …": writes the chosen profiles (in list order) to a
/// `.tablerconnections` file. Returns how many were written.
#[tauri::command]
pub async fn export_connections(
    state: State<'_, AppState>,
    request: ExportConnectionsRequest,
) -> Result<u32, AppError> {
    run_export(&state, request).await
}

pub async fn run_export(state: &AppState, req: ExportConnectionsRequest) -> Result<u32, AppError> {
    if req.path.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "Choose where to save the file".into(),
        ));
    }
    let password = req.file_password.as_deref().filter(|p| !p.is_empty());
    if (req.include_passwords || req.include_ssh_secrets) && password.is_none() {
        return Err(AppError::InvalidInput(
            "Set a file password to export passwords and passphrases".into(),
        ));
    }
    let mut connections = Vec::new();
    for profile in state.appdb.list_profiles()? {
        if !req.profile_ids.contains(&profile.id) {
            continue;
        }
        let id = profile.id;
        let mut secrets = ExportedSecrets::default();
        if req.include_passwords && profile.options.password_mode() == PasswordMode::Keychain {
            secrets.password = state.secrets.get(id, SecretKind::DbPassword).await?;
        }
        if req.include_ssh_secrets && profile.ssh.is_some() {
            secrets.ssh_password = state.secrets.get(id, SecretKind::SshPassword).await?;
            secrets.ssh_passphrase = state.secrets.get(id, SecretKind::SshPassphrase).await?;
        }
        connections.push(ExportedConnection { profile, secrets });
    }
    if connections.is_empty() {
        return Err(AppError::InvalidInput("No connections to export".into()));
    }
    let used: Vec<String> = connections
        .iter()
        .filter_map(|c| c.profile.group.clone())
        .collect();
    let payload = Payload {
        groups: profiles_io::merge_groups(&req.groups, &used),
        connections,
    };
    let count = payload.connections.len();
    // The key derivation takes a moment: off the async workers.
    let password = password.map(str::to_owned);
    let exported_at = chrono::Utc::now().to_rfc3339();
    let bytes = tokio::task::spawn_blocking(move || {
        profiles_io::write(&payload, password.as_deref(), &exported_at)
    })
    .await
    .map_err(|e| AppError::internal(e.to_string()))??;
    tokio::fs::write(&req.path, bytes)
        .await
        .map_err(|e| io_error(&req.path, e))?;
    log::info!("exported {count} connection(s) to {}", req.path);
    Ok(count as u32)
}

async fn read_connections_file(
    path: &str,
    password: Option<&str>,
) -> Result<ReadOutcome, AppError> {
    let bytes = tokio::fs::read(path).await.map_err(|e| io_error(path, e))?;
    // The key derivation of an encrypted file takes a moment: off the async workers.
    let password = password.map(str::to_owned);
    tokio::task::spawn_blocking(move || profiles_io::read(&bytes, password.as_deref()))
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
}

/// "Import Connections…", step 1: what the file holds (asks for the password of an encrypted
/// file with `needs_password`), with the names that are already taken.
#[tauri::command]
pub async fn inspect_connections_file(
    state: State<'_, AppState>,
    path: String,
    password: Option<String>,
) -> Result<ConnectionsFilePreview, AppError> {
    inspect_file(&state, &path, password.as_deref()).await
}

pub async fn inspect_file(
    state: &AppState,
    path: &str,
    password: Option<&str>,
) -> Result<ConnectionsFilePreview, AppError> {
    Ok(match read_connections_file(path, password).await? {
        ReadOutcome::NeedsPassword => ConnectionsFilePreview {
            encrypted: true,
            needs_password: true,
            groups: Vec::new(),
            connections: Vec::new(),
        },
        ReadOutcome::Opened { payload, encrypted } => ConnectionsFilePreview {
            encrypted,
            needs_password: false,
            connections: profiles_io::preview(&payload, &state.appdb.list_profiles()?),
            groups: payload.groups,
        },
    })
}

/// "Import Connections…", step 2: adds the chosen connections (`ids` as in the file) as new
/// profiles, their secrets to the Keychain and their groups to the group list.
#[tauri::command]
pub async fn import_connections(
    state: State<'_, AppState>,
    path: String,
    password: Option<String>,
    ids: Vec<Uuid>,
) -> Result<Vec<ConnectionProfile>, AppError> {
    run_import(&state, &path, password.as_deref(), &ids).await
}

pub async fn run_import(
    state: &AppState,
    path: &str,
    password: Option<&str>,
    ids: &[Uuid],
) -> Result<Vec<ConnectionProfile>, AppError> {
    let ReadOutcome::Opened { payload, .. } = read_connections_file(path, password).await? else {
        return Err(AppError::InvalidInput(
            "Enter the file password to import these connections".into(),
        ));
    };
    let mut imported = Vec::new();
    for (profile, secrets) in profiles_io::plan_import(&payload, ids) {
        let stored = state.appdb.upsert_profile(&profile)?;
        let id = stored.id;
        if stored.options.password_mode() == PasswordMode::Keychain
            && let Some(p) = &secrets.password
        {
            state.secrets.set(id, SecretKind::DbPassword, p).await?;
        }
        for (value, kind) in [
            (&secrets.ssh_password, SecretKind::SshPassword),
            (&secrets.ssh_passphrase, SecretKind::SshPassphrase),
        ] {
            if let Some(secret) = value.as_deref().filter(|s| !s.is_empty()) {
                state.secrets.set(id, kind, secret).await?;
            }
        }
        imported.push(stored);
    }
    // The imported connections' groups, plus the file's empty groups when importing everything.
    let mut extra: Vec<String> = imported.iter().filter_map(|p| p.group.clone()).collect();
    if imported.len() == payload.connections.len() {
        extra.extend(payload.groups.iter().cloned());
    }
    let known = stored_groups(state)?;
    let merged = profiles_io::merge_groups(&known, &extra);
    if merged != known {
        state
            .appdb
            .set_setting(GROUPS_SETTING, &serde_json::json!(merged))?;
    }
    log::info!("imported {} connection(s) from {path}", imported.len());
    Ok(imported)
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

    fn state() -> AppState {
        let appdb = AppDb::open_in_memory().unwrap();
        // Never the Keychain in unit tests.
        AppState::new(appdb.clone(), Secrets::new(Backend::File, appdb))
    }

    fn pg_profile(name: &str, group: Option<&str>) -> ConnectionProfile {
        let mut p = sqlite_input("/x.db").profile;
        p.id = Uuid::new_v4();
        p.name = name.into();
        p.group = group.map(str::to_string);
        p.driver = Driver::Postgres;
        p.sqlite_path = None;
        p.host = Some("127.0.0.1".into());
        p.user = Some("app".into());
        p
    }

    #[tokio::test]
    async fn password_modes_decide_what_is_stored_and_used() {
        let state = state();
        let mut input = sqlite_input("/x.db");
        input.profile = pg_profile("pg", None);
        input.password = Some("kept".into());
        store_secrets(&state, &input).await.unwrap();
        assert_eq!(
            resolve_password(&state, &input).await.unwrap().as_deref(),
            Some("kept")
        );

        // Ask every time: the stored password goes, typed ones are never stored.
        input.profile.options.password_mode = Some(PasswordMode::Ask);
        input.password = Some("typed".into());
        store_secrets(&state, &input).await.unwrap();
        let id = input.profile.id;
        assert_eq!(
            state.secrets.get(id, SecretKind::DbPassword).await.unwrap(),
            None
        );
        assert_eq!(
            resolve_password(&state, &input).await.unwrap().as_deref(),
            Some("typed")
        );
        input.password = None;
        assert_eq!(resolve_password(&state, &input).await.unwrap(), None);
        state.set_session_password(id, Some("remembered".into()));
        assert_eq!(
            resolve_password(&state, &input).await.unwrap().as_deref(),
            Some("remembered")
        );
        let profile = input.profile.clone();
        assert_eq!(
            crate::commands::connections::connect_password(&state, &profile, None)
                .await
                .unwrap()
                .as_deref(),
            Some("remembered")
        );

        // No password: never one, and the remembered one is forgotten on save.
        input.profile.options.password_mode = Some(PasswordMode::None);
        input.password = Some("ignored".into());
        store_secrets(&state, &input).await.unwrap();
        assert_eq!(state.session_password(id), None);
        assert_eq!(resolve_password(&state, &input).await.unwrap(), None);

        // An "Ask every time" connect without a typed or remembered password is refused.
        let mut ask = profile;
        ask.id = Uuid::new_v4();
        let err = crate::commands::connections::connect_password(&state, &ask, None)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "AUTH_FAILED");
    }

    #[tokio::test]
    async fn export_then_import_round_trips_profiles_secrets_and_groups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("team.tablerconnections");
        let path = path.to_string_lossy().into_owned();
        let source = state();
        let a = source
            .appdb
            .upsert_profile(&pg_profile("Alpha", Some("Team")))
            .unwrap();
        let b = source
            .appdb
            .upsert_profile(&pg_profile("Beta", None))
            .unwrap();
        source
            .secrets
            .set(a.id, SecretKind::DbPassword, "alpha-pw")
            .await
            .unwrap();
        let request = |include: bool, password: Option<&str>| ExportConnectionsRequest {
            profile_ids: vec![a.id, b.id],
            groups: vec!["Empty".into()],
            include_passwords: include,
            include_ssh_secrets: false,
            file_password: password.map(str::to_string),
            path: path.clone(),
        };
        // Secrets need a file password.
        assert!(run_export(&source, request(true, None)).await.is_err());
        assert_eq!(
            run_export(&source, request(true, Some("pw")))
                .await
                .unwrap(),
            2
        );
        assert!(!std::fs::read_to_string(&path).unwrap().contains("alpha-pw"));

        let target = state();
        target
            .appdb
            .upsert_profile(&pg_profile("alpha", None))
            .unwrap();
        let locked = inspect_file(&target, &path, None).await.unwrap();
        assert!(locked.needs_password && locked.connections.is_empty());
        assert!(inspect_file(&target, &path, Some("nope")).await.is_err());
        let preview = inspect_file(&target, &path, Some("pw")).await.unwrap();
        assert!(preview.encrypted && !preview.needs_password);
        assert_eq!(preview.groups, ["Empty", "Team"]);
        assert_eq!(preview.connections.len(), 2);
        assert!(preview.connections[0].duplicate_name);
        assert!(preview.connections[0].has_password);

        let imported = run_import(&target, &path, Some("pw"), &[a.id, b.id])
            .await
            .unwrap();
        assert_eq!(imported.len(), 2);
        assert!(imported.iter().all(|p| p.id != a.id && p.id != b.id));
        assert_eq!(imported[0].name, "Alpha");
        assert_eq!(imported[0].group.as_deref(), Some("Team"));
        assert_eq!(
            target
                .secrets
                .get(imported[0].id, SecretKind::DbPassword)
                .await
                .unwrap()
                .as_deref(),
            Some("alpha-pw")
        );
        assert_eq!(target.appdb.list_profiles().unwrap().len(), 3);
        // Everything was imported: the file's empty group comes along.
        assert_eq!(stored_groups(&target).unwrap(), ["Team", "Empty"]);

        // Without a file password nothing is encrypted (and no secret is allowed).
        run_export(&source, request(false, None)).await.unwrap();
        let plain = inspect_file(&target, &path, None).await.unwrap();
        assert!(!plain.encrypted && !plain.connections[0].has_password);
        let only_beta = run_import(&target, &path, None, &[b.id]).await.unwrap();
        assert_eq!(only_beta.len(), 1);
    }

    #[tokio::test]
    async fn moving_profiles_between_groups() {
        let state = state();
        let a = state.appdb.upsert_profile(&pg_profile("a", None)).unwrap();
        let b = state
            .appdb
            .upsert_profile(&pg_profile("b", Some("Old")))
            .unwrap();
        let moved =
            move_to_group(&state, &[a.id, b.id, Uuid::new_v4()], Some(" New ".into())).unwrap();
        assert_eq!(moved.len(), 2);
        assert!(moved.iter().all(|p| p.group.as_deref() == Some("New")));
        let out = move_to_group(&state, &[b.id], Some("  ".into())).unwrap();
        assert_eq!(out[0].group, None);
        assert_eq!(state.appdb.get_profile(b.id).unwrap().unwrap().group, None);
        // The list order is kept.
        assert_eq!(
            state.appdb.get_profile(a.id).unwrap().unwrap().sort_order,
            a.sort_order
        );
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
