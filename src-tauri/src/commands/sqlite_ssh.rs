//! SQLite over SSH glue for `connect` / `disconnect` (see `tunnel::sqlite_remote`): the SSH
//! session uses the profile's stored secrets, the renderer's SSH prompt dialog and the app's host
//! keys; upload results go to the renderer as `sqlite-ssh:status`.

use std::sync::Arc;

use async_trait::async_trait;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use super::connections::{connect_password, connect_timeout, renderer_prompts, stored_tunnel_auth};
use crate::drivers::sqlite::Cipher;
use crate::error::AppError;
use crate::state::AppState;
use crate::tunnel::sqlite_remote::{self, RemoteHost, RemoteSqlite, SqliteSshEvent, SqliteSshSync};
use crate::tunnel::{
    self, AppHostKeys, ClockedPrompts, HostKeyTrust, KeyboardInteractive, PromptClock, PromptRound,
    SshPromptKind, SshPromptPurpose, SshSession, TunnelAuth,
};
use crate::types::profile::ConnectionProfile;

/// Upload results of SQLite over SSH connections (`SqliteSshEvent`).
pub const EVENT_SQLITE_SSH: &str = "sqlite-ssh:status";

/// Opens a signed-in SSH session for `profile`, bounded by its connect timeout (time spent
/// answering questions left out). Host keys are checked like the tunnel's.
pub async fn open_ssh_session(
    state: &AppState,
    profile: &ConnectionProfile,
    mut auth: TunnelAuth,
) -> Result<SshSession, AppError> {
    let ssh = profile
        .ssh
        .as_ref()
        .ok_or_else(|| AppError::internal("the profile has no SSH settings"))?;
    let trust = AppHostKeys::for_state(state, profile.id);
    let timeout = connect_timeout(profile);
    let clock = Arc::new(PromptClock::default());
    if let Some(prompts) = auth.prompts.take() {
        auth.prompts = Some(Arc::new(ClockedPrompts::new(prompts, clock.clone())));
    }
    tunnel::timeout_excluding_prompts(
        timeout,
        &clock,
        SshSession::open(ssh, auth, &trust as &dyn HostKeyTrust),
    )
    .await
    .ok_or_else(|| {
        AppError::Tunnel(format!(
            "timed out after {}s connecting to the SSH server {}",
            timeout.as_secs(),
            ssh.host
        ))
    })?
}

/// What the copy of a connecting profile asks the app for.
struct AppRemoteHost {
    app: AppHandle,
    profile: ConnectionProfile,
    purpose: SshPromptPurpose,
    /// The window that opened the connection: its questions and upload results go there.
    window: Option<String>,
    /// The database's key, read once (see [`RemoteHost::cipher`]): a "Command Line" password is
    /// not run again for every upload.
    cipher: tokio::sync::OnceCell<Option<Cipher>>,
}

impl AppRemoteHost {
    fn new(
        app: &AppHandle,
        profile: ConnectionProfile,
        purpose: SshPromptPurpose,
        window: Option<&str>,
    ) -> AppRemoteHost {
        AppRemoteHost {
            app: app.clone(),
            profile,
            purpose,
            window: window.map(str::to_string),
            cipher: tokio::sync::OnceCell::new(),
        }
    }

    fn prompts(&self, state: &AppState) -> Arc<dyn KeyboardInteractive> {
        renderer_prompts(
            &self.app,
            state,
            &self.profile,
            self.purpose,
            self.window.as_deref(),
        )
    }
}

#[async_trait]
impl RemoteHost for AppRemoteHost {
    async fn open_ssh(&self) -> Result<SshSession, AppError> {
        let state = self.app.state::<AppState>();
        let ssh = self
            .profile
            .ssh
            .as_ref()
            .ok_or_else(|| AppError::internal("the profile has no SSH settings"))?;
        let mut auth = stored_tunnel_auth(&state, self.profile.id, ssh).await?;
        auth.prompts = Some(self.prompts(&state));
        open_ssh_session(&state, &self.profile, auth).await
    }

    async fn confirm(&self, title: &str, message: &str) -> Result<bool, AppError> {
        let state = self.app.state::<AppState>();
        let ssh = self.profile.ssh.as_ref();
        let round = PromptRound {
            user: ssh.map(|s| s.user.clone()).unwrap_or_default(),
            host: ssh.map(|s| s.host.clone()).unwrap_or_default(),
            port: ssh.map(|s| s.port).filter(|p| *p != 0).unwrap_or(22),
            name: title.to_string(),
            instructions: message.to_string(),
            prompts: Vec::new(),
            kind: Some(SshPromptKind::Confirm),
        };
        match self.prompts(&state).answer(round).await {
            Ok(_) => Ok(true),
            Err(AppError::Cancelled) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The profile's password is the key of an encrypted database: the one the connect used
    /// (stored, remembered for the session or from the profile's command). An error means no key
    /// (and is tried again at the next upload, not remembered).
    async fn cipher(&self) -> Option<Cipher> {
        let state = self.app.state::<AppState>();
        let resolved = self
            .cipher
            .get_or_try_init(|| async {
                let password = connect_password(&state, &self.profile, None).await?;
                Ok::<_, AppError>(Cipher::from_key(
                    password.as_deref(),
                    self.profile.options.sqlite_cipher_compatibility,
                ))
            })
            .await;
        match resolved {
            Ok(cipher) => cipher.clone(),
            Err(e) => {
                log::warn!(
                    "sqlite over ssh: cannot get the encryption key of {}: {e}",
                    self.profile.name
                );
                None
            }
        }
    }

    fn emit(&self, event: SqliteSshEvent) {
        // The window that owns the connection now (a moved tab's window may differ).
        let owner = self
            .app
            .try_state::<crate::windows::WindowRegistry>()
            .and_then(|r| r.owner_of(event.connection_id));
        let origin = owner.as_deref().or(self.window.as_deref());
        crate::windows::emit_to_window(&self.app, origin, EVENT_SQLITE_SSH, event);
    }
}

/// The copy of a connect in progress: dropped before `attach` (the connect failed), it is
/// cleaned up.
#[derive(Default)]
pub struct PendingCopy(Option<Arc<RemoteSqlite>>);

impl PendingCopy {
    /// Hands the copy to the opened connection (uploads start).
    pub fn attach(mut self, connection_id: Uuid) {
        if let Some(remote) = self.0.take() {
            sqlite_remote::register(connection_id, remote);
        }
    }
}

impl Drop for PendingCopy {
    fn drop(&mut self) {
        if let Some(remote) = self.0.take() {
            tauri::async_runtime::spawn(async move { remote.finish().await });
        }
    }
}

/// For a SQLite over SSH profile: downloads the remote file and returns the profile the driver
/// opens (the local copy). Other profiles come back unchanged.
pub async fn localize(
    app: &AppHandle,
    profile: ConnectionProfile,
    window: Option<&str>,
) -> Result<(ConnectionProfile, PendingCopy), AppError> {
    if !sqlite_remote::applies(&profile) {
        return Ok((profile, PendingCopy::default()));
    }
    let host = Arc::new(AppRemoteHost::new(
        app,
        profile.clone(),
        SshPromptPurpose::Connect,
        window,
    ));
    let (remote, local) = RemoteSqlite::open(&profile, host).await?;
    Ok((local, PendingCopy(Some(remote))))
}

/// The local copy an open connection of the profile downloaded at connect, if one is open.
pub async fn cached_copy(state: &AppState, profile_id: Uuid) -> Option<std::path::PathBuf> {
    let open: Vec<Uuid> = state
        .connections
        .read()
        .await
        .values()
        .filter(|c| c.profile.id == profile_id)
        .map(|c| c.id)
        .collect();
    open.into_iter()
        .find_map(sqlite_remote::get)
        .map(|remote| remote.local_path().to_path_buf())
}

/// The file a backup of a SQLite over SSH profile copies: the local copy an open connection of
/// the profile downloaded at connect (unless `fresh`), else the remote file downloaded now for the
/// backup (read-only; removed when the returned copy is dropped). `window` is asked the SSH
/// server's questions.
pub async fn backup_source(
    app: &AppHandle,
    state: &AppState,
    profile: &ConnectionProfile,
    fresh: bool,
    window: Option<&str>,
) -> Result<(std::path::PathBuf, PendingCopy), AppError> {
    if !fresh && let Some(path) = cached_copy(state, profile.id).await {
        return Ok((path, PendingCopy::default()));
    }
    let mut download = profile.clone();
    download.options.sqlite_ssh_upload = Some(false);
    let host = Arc::new(AppRemoteHost::new(
        app,
        download.clone(),
        SshPromptPurpose::Connect,
        window,
    ));
    let (remote, local) = RemoteSqlite::open(&download, host).await?;
    let path = std::path::PathBuf::from(local.sqlite_path.unwrap_or_default());
    Ok((path, PendingCopy(Some(remote))))
}

/// A closing connection's copy: last upload, then clean-up.
pub async fn detach(connection_id: Uuid) {
    if let Some(remote) = sqlite_remote::take(connection_id) {
        remote.finish().await;
    }
}

/// App exit: the last uploads of every open copy, bounded so quitting is not held up long.
pub fn shutdown() {
    let remotes = sqlite_remote::take_all();
    if remotes.is_empty() {
        return;
    }
    tauri::async_runtime::block_on(async move {
        let all = futures::future::join_all(remotes.iter().map(|r| r.finish()));
        if tokio::time::timeout(std::time::Duration::from_secs(10), all)
            .await
            .is_err()
        {
            log::warn!("sqlite over ssh: uploads at exit timed out");
        }
    });
}

/// "Test" of a SQLite over SSH profile: signs in and checks the remote file over SFTP.
pub async fn test_remote(
    state: &AppState,
    profile: &ConnectionProfile,
    auth: TunnelAuth,
) -> Result<String, AppError> {
    let path = sqlite_remote::sftp_path(profile.sqlite_path.as_deref().unwrap_or(""))?;
    let ssh = open_ssh_session(state, profile, auth).await?;
    let checked = async {
        let fs = sqlite_remote::SftpFs(ssh.sftp().await?);
        sqlite_remote::RemoteFs::stat(&fs, &path).await
    }
    .await;
    ssh.close().await;
    let stamp = checked?;
    Ok(format!(
        "Found {path} on the SSH server ({:.1} MB); it is downloaded over SFTP on connect",
        stamp.size as f64 / (1024.0 * 1024.0)
    ))
}

/// Uploads the local changes of a SQLite over SSH connection now (`force`: even though the
/// remote file changed since it was downloaded).
#[tauri::command]
pub async fn sqlite_ssh_upload(
    _state: State<'_, AppState>,
    connection_id: Uuid,
    force: bool,
) -> Result<SqliteSshSync, AppError> {
    let remote = sqlite_remote::get(connection_id).ok_or_else(|| {
        AppError::InvalidInput("this connection is not a SQLite file over SSH".into())
    })?;
    remote.sync(force).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appdb::AppDb;
    use crate::secrets::{Backend, Secrets};

    #[tokio::test]
    async fn no_open_connection_means_no_cached_copy() {
        let appdb = AppDb::open_in_memory().unwrap();
        let state = AppState::new(appdb.clone(), Secrets::new(Backend::File, appdb));
        // Nothing is open: the backup downloads the file itself.
        assert_eq!(cached_copy(&state, Uuid::new_v4()).await, None);
    }
}
