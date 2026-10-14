//! SQLite over SSH (TablePlus b233): a SQLite profile with "Over SSH" names a database file on
//! the SSH server. On connect the file is downloaded over SFTP into a private cache folder and
//! the driver opens that copy:
//! - read-only by default (the copy is a snapshot; nothing goes back), or
//! - with "Upload changes back on Commit / Disconnect" (`options.sqlite_ssh_upload`), writable:
//!   whenever the copy changes (a commit, an editor statement) and again on disconnect, a
//!   consistent snapshot (`VACUUM INTO`; an encrypted copy is exported with its key,
//!   `sqlcipher_export`, and stays encrypted) is uploaded through a temporary file that is then
//!   swapped in. Before each upload the remote file's size and modification time are compared with what
//!   was downloaded (or last uploaded): if someone else changed it meanwhile, nothing is
//!   overwritten and the renderer is told (`sqlite-ssh:status`, kind `conflict`); the user may
//!   then upload anyway (`sqlite_ssh_upload` with `force`).
//!
//! The whole file is transferred, so this suits small databases (TablePlus says the same): above
//! [`LARGE_FILE_BYTES`] the user confirms the download first. The SSH session for uploads stays
//! open while the connection does, and is reopened (asking any questions again) if it dropped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;
use uuid::Uuid;

use super::SshSession;
use crate::drivers::sqlite::Cipher;
use crate::drivers::sqlite::cipher;
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, Driver};

/// Downloads above this size are confirmed first.
pub const LARGE_FILE_BYTES: u64 = 100 * 1024 * 1024;
/// How often the local copy is checked for changes to upload.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);
/// The statement that writes a consistent copy of the open database (a bound file name).
const VACUUM_INTO: &str = "VACUUM INTO ?1";

/// Whether `profile` is a SQLite file on an SSH server.
pub fn applies(profile: &ConnectionProfile) -> bool {
    profile.driver == Driver::Sqlite && profile.ssh.is_some()
}

/// Whether changes go back to the server.
pub fn uploads(profile: &ConnectionProfile) -> bool {
    profile.options.sqlite_ssh_upload == Some(true)
}

/// The remote path as SFTP takes it: `~/x` is relative to the login folder (where SFTP starts).
pub fn sftp_path(path: &str) -> Result<String, AppError> {
    let path = path.trim();
    let out = match path {
        "" | "~" | "~/" => "",
        p => p.strip_prefix("~/").unwrap_or(p),
    };
    if out.is_empty() || out.ends_with('/') {
        return Err(AppError::InvalidInput(
            "enter the path of the SQLite file on the SSH server".into(),
        ));
    }
    Ok(out.to_string())
}

/// What is known about the remote file: its size and modification time (seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteStamp {
    pub size: u64,
    pub mtime: Option<u32>,
}

/// The remote file system (SFTP in the app, a fake in tests).
#[async_trait]
pub trait RemoteFs: Send + Sync {
    async fn stat(&self, path: &str) -> Result<RemoteStamp, AppError>;
    /// Copies the remote file into `local` (created or truncated).
    async fn download(&self, path: &str, local: &Path) -> Result<(), AppError>;
    /// Replaces the remote file with `local`: written to a temporary file next to it first.
    async fn upload(&self, local: &Path, path: &str) -> Result<(), AppError>;
}

/// Downloads `remote` into `local` after a `confirm_large(size)` for big files (false gives
/// the connect up). Returns the remote stamp the copy corresponds to.
pub async fn fetch<F, Fut>(
    fs: &dyn RemoteFs,
    remote: &str,
    local: &Path,
    confirm_large: F,
) -> Result<RemoteStamp, AppError>
where
    F: FnOnce(u64) -> Fut,
    Fut: std::future::Future<Output = Result<bool, AppError>>,
{
    let stamp = fs.stat(remote).await?;
    if stamp.size > LARGE_FILE_BYTES && !confirm_large(stamp.size).await? {
        return Err(AppError::Cancelled);
    }
    fs.download(remote, local).await?;
    Ok(stamp)
}

/// What an upload did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// Uploaded; the remote file now has this stamp.
    Uploaded(RemoteStamp),
    /// The remote file is not the one downloaded (`found`; `None`: it is gone): not overwritten.
    Conflict { found: Option<RemoteStamp> },
}

/// Uploads `snapshot` over `remote` unless the remote file changed since `expected` (`force`
/// uploads anyway).
pub async fn push(
    fs: &dyn RemoteFs,
    snapshot: &Path,
    remote: &str,
    expected: RemoteStamp,
    force: bool,
) -> Result<PushOutcome, AppError> {
    let found = match fs.stat(remote).await {
        Ok(stamp) => Some(stamp),
        Err(AppError::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    if !force && found != Some(expected) {
        return Ok(PushOutcome::Conflict { found });
    }
    fs.upload(snapshot, remote).await?;
    Ok(PushOutcome::Uploaded(fs.stat(remote).await?))
}

/// Writes a consistent copy of the SQLite database `db` to `out` (replacing it), while the
/// driver may keep the database open. An encrypted database (`key`: its cipher) is copied with
/// `sqlcipher_export`, so the copy opens with the same key and compatibility setting; `VACUUM
/// INTO` cannot read it without the key.
pub fn snapshot(db: &Path, out: &Path, key: Option<&Cipher>) -> Result<(), AppError> {
    let _ = std::fs::remove_file(out);
    if let Some(key) = key {
        return cipher::copy_encrypted(db, out, key).inspect_err(|_| {
            let _ = std::fs::remove_file(out);
        });
    }
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| AppError::Io(format!("cannot read {}: {e}", db.display())))?;
    // The bundled SQLCipher keeps temporary data in memory by default (see `Actor::spawn`).
    let _ = conn.execute_batch("PRAGMA temp_store = FILE;");
    conn.execute(VACUUM_INTO, [out.to_string_lossy()])
        .map_err(|e| {
            let hint = match e.sqlite_error_code() {
                Some(rusqlite::ErrorCode::NotADatabase) => {
                    " (an encrypted database needs its key: it was not available)"
                }
                _ => "",
            };
            AppError::Io(format!("cannot copy {}: {e}{hint}", db.display()))
        })?;
    Ok(())
}

// ----- SFTP -------------------------------------------------------------------------------------

fn sftp_error(what: &str, path: &str, e: russh_sftp::client::error::Error) -> AppError {
    use russh_sftp::protocol::StatusCode;
    match &e {
        russh_sftp::client::error::Error::Status(s) if s.status_code == StatusCode::NoSuchFile => {
            AppError::NotFound(format!("{path} does not exist on the SSH server"))
        }
        russh_sftp::client::error::Error::Status(s)
            if s.status_code == StatusCode::PermissionDenied =>
        {
            AppError::Tunnel(format!(
                "permission denied to {what} {path} on the SSH server"
            ))
        }
        _ => AppError::Tunnel(format!("cannot {what} {path} over SFTP: {e}")),
    }
}

/// [`RemoteFs`] over an SFTP session.
pub struct SftpFs(pub russh_sftp::client::SftpSession);

#[async_trait]
impl RemoteFs for SftpFs {
    async fn stat(&self, path: &str) -> Result<RemoteStamp, AppError> {
        let meta = self
            .0
            .metadata(path)
            .await
            .map_err(|e| sftp_error("read", path, e))?;
        if meta.is_dir() {
            return Err(AppError::InvalidInput(format!(
                "{path} is a folder on the SSH server, not a SQLite file"
            )));
        }
        Ok(RemoteStamp {
            size: meta.size.unwrap_or(0),
            mtime: meta.mtime,
        })
    }

    async fn download(&self, path: &str, local: &Path) -> Result<(), AppError> {
        let mut remote = self
            .0
            .open(path)
            .await
            .map_err(|e| sftp_error("open", path, e))?;
        let mut file = tokio::fs::File::create(local)
            .await
            .map_err(|e| AppError::Io(format!("cannot write {}: {e}", local.display())))?;
        tokio::io::copy(&mut remote, &mut file)
            .await
            .map_err(|e| AppError::Tunnel(format!("downloading {path} failed: {e}")))?;
        file.flush().await?;
        file.sync_all().await?;
        Ok(())
    }

    async fn upload(&self, local: &Path, path: &str) -> Result<(), AppError> {
        let temp = format!("{path}.tabler-upload");
        let backup = format!("{path}.tabler-backup");
        {
            let mut remote = self
                .0
                .create(temp.clone())
                .await
                .map_err(|e| sftp_error("create", &temp, e))?;
            let mut file = tokio::fs::File::open(local).await?;
            tokio::io::copy(&mut file, &mut remote)
                .await
                .map_err(|e| AppError::Tunnel(format!("uploading {path} failed: {e}")))?;
            remote
                .shutdown()
                .await
                .map_err(|e| AppError::Tunnel(format!("uploading {path} failed: {e}")))?;
        }
        // Keep the original's permissions on the new file.
        if let Ok(meta) = self.0.metadata(path).await
            && let Some(permissions) = meta.permissions
        {
            let attrs = russh_sftp::protocol::FileAttributes {
                permissions: Some(permissions & 0o7777),
                ..Default::default()
            };
            let _ = self.0.set_metadata(temp.clone(), attrs).await;
        }
        // SFTP v3 renames refuse an existing target: move the original aside first.
        if self.0.rename(temp.clone(), path).await.is_ok() {
            return Ok(());
        }
        let _ = self.0.remove_file(backup.clone()).await;
        self.0
            .rename(path, backup.clone())
            .await
            .map_err(|e| sftp_error("replace", path, e))?;
        if let Err(e) = self.0.rename(temp.clone(), path).await {
            let _ = self.0.rename(backup.clone(), path).await;
            return Err(sftp_error("replace", path, e));
        }
        let _ = self.0.remove_file(backup).await;
        Ok(())
    }
}

// ----- the connection's copy --------------------------------------------------------------------

/// Payload of the global `sqlite-ssh:status` event.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SqliteSshEvent {
    pub connection_id: Uuid,
    /// The connection's name.
    pub name: String,
    pub kind: SqliteSshEventKind,
    pub message: String,
    /// `user@host:path` of the remote file.
    pub remote: String,
    /// The local copy, when it was kept (unsaved changes after a conflict).
    pub local_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SqliteSshEventKind {
    /// The changes were uploaded.
    Uploaded,
    /// The remote file changed meanwhile: nothing was uploaded (upload anyway with `force`).
    Conflict,
    /// An upload failed.
    Error,
    /// Disconnected with changes that could not be uploaded: the local copy was kept.
    KeptLocal,
}

/// Result of `sqlite_ssh_upload`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SqliteSshSync {
    /// Something was uploaded.
    pub uploaded: bool,
    /// Not uploaded: the remote file changed since it was downloaded.
    pub conflict: bool,
}

/// What the copy needs from the app: SSH sessions (with the user's answers), questions and
/// events.
#[async_trait]
pub trait RemoteHost: Send + Sync {
    /// Opens a signed-in SSH session to the profile's server.
    async fn open_ssh(&self) -> Result<SshSession, AppError>;
    /// Asks a yes / no question (no answer: no).
    async fn confirm(&self, title: &str, message: &str) -> Result<bool, AppError>;
    fn emit(&self, event: SqliteSshEvent);
    /// The key of the database when it is encrypted (SQLCipher), so that the copy that goes back
    /// to the server is exported with it; `None`: a plain database. Called on every upload: the
    /// implementation keeps the answer.
    async fn cipher(&self) -> Option<Cipher> {
        None
    }
}

/// The size and modification time of the database file and its `-wal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct LocalStamp {
    db: Option<(SystemTime, u64)>,
    wal: Option<(SystemTime, u64)>,
}

fn file_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn local_stamp(db: &Path) -> LocalStamp {
    let mut wal = db.as_os_str().to_owned();
    wal.push("-wal");
    LocalStamp {
        db: file_stamp(db),
        wal: file_stamp(Path::new(&wal)),
    }
}

struct Link {
    ssh: SshSession,
    fs: SftpFs,
}

struct SyncState {
    link: Option<Link>,
    /// The remote file as last downloaded or uploaded.
    remote: RemoteStamp,
    /// The local copy as last synced (or as last found in conflict).
    synced: LocalStamp,
    /// Local changes not uploaded because of a conflict.
    conflicted: bool,
}

/// The local copy of one connection's remote SQLite file.
pub struct RemoteSqlite {
    name: String,
    remote_path: String,
    remote_label: String,
    dir: PathBuf,
    local_path: PathBuf,
    upload: bool,
    host: Arc<dyn RemoteHost>,
    sync: tokio::sync::Mutex<SyncState>,
    connection_id: Mutex<Option<Uuid>>,
    stop: CancellationToken,
}

impl std::fmt::Debug for RemoteSqlite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteSqlite")
            .field("remote", &self.remote_label)
            .field("local", &self.local_path)
            .field("upload", &self.upload)
            .finish()
    }
}

/// Where the copies live: a private folder under the temporary directory.
pub fn cache_root() -> PathBuf {
    std::env::temp_dir().join("tabler-sqlite-ssh")
}

/// Removes copies left behind by an earlier run that ended without disconnecting (none of them
/// holds unsaved changes the user was told about: those are in `kept-*` folders).
pub fn purge_stale_copies() {
    let Ok(entries) = std::fs::read_dir(cache_root()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("kept-") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn private_dir(path: &Path) -> Result<(), AppError> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn file_name_of(remote: &str) -> String {
    remote
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("database.sqlite")
        .to_string()
}

fn megabytes(bytes: u64) -> String {
    format!("{:.0} MB", bytes as f64 / (1024.0 * 1024.0))
}

impl RemoteSqlite {
    /// Downloads the profile's remote file and returns the copy together with the profile to
    /// connect with: its `sqlite_path` is the copy, read-only unless changes are uploaded.
    pub async fn open(
        profile: &ConnectionProfile,
        host: Arc<dyn RemoteHost>,
    ) -> Result<(Arc<RemoteSqlite>, ConnectionProfile), AppError> {
        let ssh_cfg = profile
            .ssh
            .as_ref()
            .ok_or_else(|| AppError::internal("SQLite over SSH without SSH settings"))?;
        let remote_path = sftp_path(profile.sqlite_path.as_deref().unwrap_or(""))?;
        let remote_label = format!(
            "{}{}:{}",
            Some(ssh_cfg.user.trim())
                .filter(|u| !u.is_empty())
                .map(|u| format!("{u}@"))
                .unwrap_or_default(),
            ssh_cfg.host.trim(),
            profile.sqlite_path.as_deref().unwrap_or("").trim()
        );
        let dir = cache_root().join(Uuid::new_v4().to_string());
        private_dir(&dir)?;
        let local_path = dir.join(file_name_of(&remote_path));
        let result = async {
            let ssh = host.open_ssh().await?;
            let fs = SftpFs(ssh.sftp().await?);
            let file = file_name_of(&remote_path);
            let stamp = fetch(&fs, &remote_path, &local_path, |size| {
                let host = host.clone();
                let file = file.clone();
                async move {
                    host.confirm(
                        "Large database file",
                        &format!(
                            "\u{201c}{file}\u{201d} is {} on the SSH server. SQLite over SSH \
                             downloads the whole file on every connect, which may take a while. \
                             Download it?",
                            megabytes(size)
                        ),
                    )
                    .await
                }
            })
            .await?;
            Ok::<_, AppError>((ssh, fs, stamp))
        }
        .await;
        let (ssh, fs, stamp) = match result {
            Ok(r) => r,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
        };
        log::info!(
            "sqlite over ssh: downloaded {remote_label} ({} bytes) to {}",
            stamp.size,
            local_path.display()
        );
        let upload = uploads(profile);
        let link = if upload {
            Some(Link { ssh, fs })
        } else {
            ssh.close().await;
            None
        };
        let remote = Arc::new(RemoteSqlite {
            name: profile.name.clone(),
            remote_path,
            remote_label,
            dir,
            local_path: local_path.clone(),
            upload,
            host,
            sync: tokio::sync::Mutex::new(SyncState {
                link,
                remote: stamp,
                synced: local_stamp(&local_path),
                conflicted: false,
            }),
            connection_id: Mutex::new(None),
            stop: CancellationToken::new(),
        });
        let mut local = profile.clone();
        local.sqlite_path = Some(local_path.to_string_lossy().into_owned());
        local.options.read_only = profile.options.read_only || !upload;
        Ok((remote, local))
    }

    pub fn local_path(&self) -> &Path {
        &self.local_path
    }

    /// A read-only copy in `dir` that needs no SSH server (tests of the code that attaches and
    /// detaches copies).
    #[cfg(test)]
    pub(crate) fn for_test(
        dir: PathBuf,
        local_path: PathBuf,
        host: Arc<dyn RemoteHost>,
    ) -> Arc<Self> {
        Arc::new(RemoteSqlite {
            name: "test copy".into(),
            remote_path: "/remote/test.sqlite".into(),
            remote_label: "test@host:/remote/test.sqlite".into(),
            dir,
            local_path: local_path.clone(),
            upload: false,
            host,
            sync: tokio::sync::Mutex::new(SyncState {
                link: None,
                remote: RemoteStamp {
                    size: 0,
                    mtime: None,
                },
                synced: local_stamp(&local_path),
                conflicted: false,
            }),
            connection_id: Mutex::new(None),
            stop: CancellationToken::new(),
        })
    }

    fn event(&self, kind: SqliteSshEventKind, message: String, kept: bool) -> SqliteSshEvent {
        SqliteSshEvent {
            connection_id: lock(&self.connection_id).unwrap_or_default(),
            name: self.name.clone(),
            kind,
            message,
            remote: self.remote_label.clone(),
            local_path: kept.then(|| self.local_path.to_string_lossy().into_owned()),
        }
    }

    /// Uploads the local changes (all of them when `force`, even over a changed remote file).
    pub async fn sync(&self, force: bool) -> Result<SqliteSshSync, AppError> {
        if !self.upload {
            return Err(AppError::InvalidInput(
                "this SQLite over SSH connection is read-only: turn on \u{201c}Upload changes \
                 back\u{201d} in its settings"
                    .into(),
            ));
        }
        let mut sync = self.sync.lock().await;
        let local = local_stamp(&self.local_path);
        if !force && local == sync.synced && !sync.conflicted {
            return Ok(SqliteSshSync {
                uploaded: false,
                conflict: false,
            });
        }
        let snapshot_path = self.dir.join(".upload.sqlite");
        let (db, out) = (self.local_path.clone(), snapshot_path.clone());
        let key = self.host.cipher().await;
        tokio::task::spawn_blocking(move || snapshot(&db, &out, key.as_ref()))
            .await
            .map_err(|e| AppError::internal(e.to_string()))??;
        if sync.link.as_ref().is_none_or(|l| !l.ssh.is_up()) {
            if let Some(old) = sync.link.take() {
                old.ssh.close().await;
            }
            let ssh = self.host.open_ssh().await?;
            let fs = SftpFs(ssh.sftp().await?);
            sync.link = Some(Link { ssh, fs });
        }
        let link = sync
            .link
            .as_ref()
            .ok_or_else(|| AppError::internal("no SFTP session"))?;
        let outcome = push(
            &link.fs,
            &snapshot_path,
            &self.remote_path,
            sync.remote,
            force,
        )
        .await;
        let _ = std::fs::remove_file(&snapshot_path);
        match outcome? {
            PushOutcome::Uploaded(stamp) => {
                sync.remote = stamp;
                sync.synced = local;
                sync.conflicted = false;
                log::info!("sqlite over ssh: uploaded changes to {}", self.remote_label);
                Ok(SqliteSshSync {
                    uploaded: true,
                    conflict: false,
                })
            }
            PushOutcome::Conflict { found } => {
                // Not retried until the copy changes again (or the user forces it).
                sync.synced = local;
                sync.conflicted = true;
                log::warn!(
                    "sqlite over ssh: {} changed on the server ({found:?}, downloaded {:?}); \
                     not uploaded",
                    self.remote_label,
                    sync.remote
                );
                Ok(SqliteSshSync {
                    uploaded: false,
                    conflict: true,
                })
            }
        }
    }

    /// Syncs and reports the outcome as an event (the watcher and disconnect).
    async fn sync_and_report(&self) -> SqliteSshSync {
        match self.sync(false).await {
            Ok(done) => {
                if done.uploaded {
                    self.host.emit(self.event(
                        SqliteSshEventKind::Uploaded,
                        "Changes uploaded".into(),
                        false,
                    ));
                } else if done.conflict {
                    self.host.emit(
                        self.event(
                            SqliteSshEventKind::Conflict,
                            "The file on the SSH server changed since it was downloaded; your \
                         changes were not uploaded"
                                .into(),
                            false,
                        ),
                    );
                }
                done
            }
            Err(e) => {
                log::warn!(
                    "sqlite over ssh: upload to {} failed: {e}",
                    self.remote_label
                );
                self.host
                    .emit(self.event(SqliteSshEventKind::Error, e.to_string(), false));
                SqliteSshSync {
                    uploaded: false,
                    conflict: false,
                }
            }
        }
    }

    /// Starts uploading changes as they happen (only when changes go back).
    fn watch(self: &Arc<Self>) {
        if !self.upload {
            return;
        }
        let me = Arc::downgrade(self);
        let stop = self.stop.clone();
        tokio::spawn(async move {
            // The key of an encrypted copy is read while the connection is open: an "Ask every
            // time" password is forgotten when it closes, before the last upload.
            if let Some(me) = me.upgrade() {
                me.host.cipher().await;
            }
            let mut seen: Option<LocalStamp> = None;
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = tokio::time::sleep(WATCH_INTERVAL) => {}
                }
                let Some(me) = me.upgrade() else { break };
                let now = local_stamp(&me.local_path);
                let synced = me.sync.lock().await.synced;
                // Uploaded once the copy has stopped changing for one interval.
                if now != synced && seen == Some(now) {
                    me.sync_and_report().await;
                    seen = None;
                } else {
                    seen = Some(now);
                }
            }
        });
    }

    /// Final upload and clean-up when the connection closes (or fails to open). A copy with
    /// changes that could not be uploaded is kept and reported.
    pub async fn finish(&self) {
        self.stop.cancel();
        let mut keep = false;
        if self.upload {
            let pending = {
                let sync = self.sync.lock().await;
                sync.conflicted || local_stamp(&self.local_path) != sync.synced
            };
            if pending {
                let done = self.sync_and_report().await;
                keep = !done.uploaded;
            }
        }
        if let Some(link) = self.sync.lock().await.link.take() {
            link.ssh.close().await;
        }
        if keep {
            let kept = cache_root().join(format!(
                "kept-{}",
                self.dir.file_name().unwrap_or_default().to_string_lossy()
            ));
            let path = match std::fs::rename(&self.dir, &kept) {
                Ok(()) => kept.join(self.local_path.file_name().unwrap_or_default()),
                Err(_) => self.local_path.clone(),
            };
            let _ = std::fs::remove_file(path.with_file_name(".upload.sqlite"));
            self.host.emit(SqliteSshEvent {
                local_path: Some(path.to_string_lossy().into_owned()),
                ..self.event(
                    SqliteSshEventKind::KeptLocal,
                    "Your changes could not be uploaded; the local copy was kept".into(),
                    true,
                )
            });
        } else {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

// ----- the open copies, by connection -----------------------------------------------------------

fn registry() -> &'static Mutex<HashMap<Uuid, Arc<RemoteSqlite>>> {
    static REGISTRY: std::sync::OnceLock<Mutex<HashMap<Uuid, Arc<RemoteSqlite>>>> =
        std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Attaches the copy to its connection and starts uploading changes.
pub fn register(connection_id: Uuid, remote: Arc<RemoteSqlite>) {
    *lock(&remote.connection_id) = Some(connection_id);
    remote.watch();
    lock(registry()).insert(connection_id, remote);
}

/// The copy of an open connection.
pub fn get(connection_id: Uuid) -> Option<Arc<RemoteSqlite>> {
    lock(registry()).get(&connection_id).cloned()
}

/// Detaches the copy of a closing connection (then `finish` it).
pub fn take(connection_id: Uuid) -> Option<Arc<RemoteSqlite>> {
    lock(registry()).remove(&connection_id)
}

/// Every open copy (app exit).
pub fn take_all() -> Vec<Arc<RemoteSqlite>> {
    lock(registry()).drain().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An in-memory remote file system.
    #[derive(Default)]
    struct FakeFs {
        files: Mutex<HashMap<String, (Vec<u8>, u32)>>,
        clock: std::sync::atomic::AtomicU32,
    }

    impl FakeFs {
        fn put(&self, path: &str, data: &[u8]) {
            let t = self.clock.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            lock(&self.files).insert(path.into(), (data.to_vec(), t));
        }

        fn data(&self, path: &str) -> Option<Vec<u8>> {
            lock(&self.files).get(path).map(|(d, _)| d.clone())
        }
    }

    #[async_trait]
    impl RemoteFs for FakeFs {
        async fn stat(&self, path: &str) -> Result<RemoteStamp, AppError> {
            lock(&self.files)
                .get(path)
                .map(|(d, t)| RemoteStamp {
                    size: d.len() as u64,
                    mtime: Some(*t),
                })
                .ok_or_else(|| AppError::NotFound(path.into()))
        }

        async fn download(&self, path: &str, local: &Path) -> Result<(), AppError> {
            let data = self
                .data(path)
                .ok_or_else(|| AppError::NotFound(path.into()))?;
            std::fs::write(local, data)?;
            Ok(())
        }

        async fn upload(&self, local: &Path, path: &str) -> Result<(), AppError> {
            let data = std::fs::read(local)?;
            self.put(path, &data);
            Ok(())
        }
    }

    #[test]
    fn remote_paths() {
        assert_eq!(sftp_path("~/data/app.db").unwrap(), "data/app.db");
        assert_eq!(sftp_path(" /srv/app.sqlite ").unwrap(), "/srv/app.sqlite");
        assert_eq!(sftp_path("app.db").unwrap(), "app.db");
        assert!(sftp_path("").is_err());
        assert!(sftp_path("~/").is_err());
        assert!(sftp_path("/srv/").is_err());
        assert_eq!(file_name_of("data/app.db"), "app.db");
        assert_eq!(file_name_of("/x/"), "x");
    }

    #[tokio::test]
    async fn fetch_downloads_and_confirms_large_files() {
        let dir = tempfile::tempdir().unwrap();
        let fs = FakeFs::default();
        fs.put("app.db", b"SQLite format 3\0small");
        let local = dir.path().join("app.db");
        let stamp = fetch(&fs, "app.db", &local, |_| async { panic!("not large") })
            .await
            .unwrap();
        assert_eq!(std::fs::read(&local).unwrap(), b"SQLite format 3\0small");
        assert_eq!(stamp.size, 21);

        // Above the limit the user decides; "no" gives the connect up.
        let big = vec![0u8; (LARGE_FILE_BYTES + 1) as usize];
        fs.put("big.db", &big);
        let asked = Arc::new(Mutex::new(None));
        let a = asked.clone();
        let err = fetch(&fs, "big.db", &dir.path().join("big.db"), move |size| {
            *lock(&a) = Some(size);
            async { Ok(false) }
        })
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Cancelled));
        assert_eq!(*lock(&asked), Some(LARGE_FILE_BYTES + 1));
        assert!(!dir.path().join("big.db").exists());
        assert!(
            fetch(&fs, "big.db", &dir.path().join("big.db"), |_| async {
                Ok(true)
            })
            .await
            .is_ok()
        );
        assert!(matches!(
            fetch(&fs, "missing.db", &dir.path().join("m.db"), |_| async {
                Ok(true)
            })
            .await,
            Err(AppError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn push_checks_for_remote_changes_first() {
        let dir = tempfile::tempdir().unwrap();
        let fs = FakeFs::default();
        fs.put("app.db", b"v1");
        let downloaded = fs.stat("app.db").await.unwrap();
        let snapshot = dir.path().join("snap");
        std::fs::write(&snapshot, b"v2 local").unwrap();

        // Unchanged remote: uploaded, and the new stamp is returned.
        let PushOutcome::Uploaded(after) = push(&fs, &snapshot, "app.db", downloaded, false)
            .await
            .unwrap()
        else {
            panic!("expected an upload");
        };
        assert_eq!(fs.data("app.db").unwrap(), b"v2 local");
        assert_ne!(after, downloaded);

        // Someone else wrote meanwhile: not overwritten...
        fs.put("app.db", b"v3 theirs");
        std::fs::write(&snapshot, b"v4 local").unwrap();
        let outcome = push(&fs, &snapshot, "app.db", after, false).await.unwrap();
        assert!(
            matches!(outcome, PushOutcome::Conflict { found: Some(f) } if f.size == 9),
            "{outcome:?}"
        );
        assert_eq!(fs.data("app.db").unwrap(), b"v3 theirs");
        // ...unless forced.
        assert!(matches!(
            push(&fs, &snapshot, "app.db", after, true).await.unwrap(),
            PushOutcome::Uploaded(_)
        ));
        assert_eq!(fs.data("app.db").unwrap(), b"v4 local");

        // A remote file that is gone is a conflict too.
        let outcome = push(&fs, &snapshot, "gone.db", after, false).await.unwrap();
        assert_eq!(outcome, PushOutcome::Conflict { found: None });
    }

    #[test]
    fn snapshots_are_consistent_copies() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("app.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; CREATE TABLE t(x); INSERT INTO t VALUES (1), (2);",
        )
        .unwrap();
        // The rows are still in the -wal file; the snapshot has them anyway.
        let out = dir.path().join("snap.db");
        std::fs::write(&out, b"old").unwrap();
        snapshot(&db, &out, None).unwrap();
        drop(conn);
        let copy = rusqlite::Connection::open(&out).unwrap();
        let n: i64 = copy
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    /// An encrypted database with a table `t`, kept open (as the driver does) while it is copied.
    fn encrypted_db(path: &Path, cipher: &Cipher) -> rusqlite::Connection {
        cipher::create_encrypted(&path.to_string_lossy(), cipher).unwrap();
        let conn = rusqlite::Connection::open(path).unwrap();
        cipher::apply(&conn, cipher).unwrap();
        conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1), (2);")
            .unwrap();
        conn
    }

    fn keyed_rows(path: &Path, cipher: &Cipher) -> i64 {
        let conn = rusqlite::Connection::open(path).unwrap();
        cipher::apply(&conn, cipher).unwrap();
        conn.query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn snapshots_of_an_encrypted_database_stay_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        for compatibility in [None, Some(3)] {
            let cipher = Cipher {
                key: "s3cret".into(),
                compatibility,
            };
            let db = dir.path().join(format!("vault{compatibility:?}.db"));
            let live = encrypted_db(&db, &cipher);
            let out = dir.path().join(format!("snap{compatibility:?}.db"));
            std::fs::write(&out, b"old").unwrap();

            // Without the key the copy cannot be read: that was the upload failing every 2 s.
            let err = snapshot(&db, &out, None).unwrap_err();
            assert!(
                err.to_string().contains("not a database") && err.to_string().contains("key"),
                "{err}"
            );
            assert!(!out.exists() || std::fs::read(&out).unwrap() != b"old");

            // With it the copy has the rows, while the driver's connection is still open.
            live.execute("INSERT INTO t VALUES (3)", []).unwrap();
            snapshot(&db, &out, Some(&cipher)).unwrap();
            assert_eq!(keyed_rows(&out, &cipher), 3);
            drop(live);

            // It is encrypted: only the key (and its format) opens it.
            assert_eq!(
                crate::drivers::sqlite::header_kind(&out.to_string_lossy()),
                crate::drivers::sqlite::HeaderKind::Encrypted
            );
            let plain = rusqlite::Connection::open(&out).unwrap();
            assert!(
                plain
                    .query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0))
                    .is_err()
            );
            let wrong = rusqlite::Connection::open(&out).unwrap();
            let wrong_key = Cipher {
                key: "other".into(),
                ..cipher.clone()
            };
            assert!(cipher::apply(&wrong, &wrong_key).is_err());
        }
    }

    #[test]
    fn encrypted_snapshots_include_what_is_still_in_the_wal() {
        let dir = tempfile::tempdir().unwrap();
        let cipher = Cipher {
            key: "s3cret".into(),
            compatibility: None,
        };
        let db = dir.path().join("vault.db");
        let live = encrypted_db(&db, &cipher);
        live.execute_batch("PRAGMA journal_mode=WAL; INSERT INTO t VALUES (3), (4);")
            .unwrap();
        let out = dir.path().join("snap.db");
        snapshot(&db, &out, Some(&cipher)).unwrap();
        assert_eq!(keyed_rows(&out, &cipher), 4);
        drop(live);
    }

    #[test]
    fn a_failed_encrypted_snapshot_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let cipher = Cipher {
            key: "s3cret".into(),
            compatibility: None,
        };
        let db = dir.path().join("vault.db");
        let _live = encrypted_db(&db, &cipher);
        let wrong = Cipher {
            key: "wrong".into(),
            compatibility: None,
        };
        let out = dir.path().join("snap.db");
        std::fs::write(&out, b"old").unwrap();
        assert!(snapshot(&db, &out, Some(&wrong)).is_err());
        assert!(!out.exists());
    }

    #[test]
    fn local_changes_are_seen_in_the_file_or_its_wal() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("a.db");
        std::fs::write(&db, b"1").unwrap();
        let before = local_stamp(&db);
        assert!(before.db.is_some() && before.wal.is_none());
        std::fs::write(dir.path().join("a.db-wal"), b"wal").unwrap();
        assert_ne!(local_stamp(&db), before);
    }
}
