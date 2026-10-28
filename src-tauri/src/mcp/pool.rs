//! The MCP server's own database connections: opened on demand from a saved profile, kept for
//! the next calls and closed after five idle minutes.
//!
//! A connection is opened the way the app opens it (bootstrap script, password mode, SSH tunnel,
//! TLS…), except where that needs the user:
//! - "Ask every time" DB or SSH passwords, keyboard-interactive (2FA) SSH questions and host keys
//!   that are not trusted yet fail with a message to open the connection in Tabler first;
//! - "Command Line (IAM authentication)" passwords run their command as usual;
//! - a pinned local tunnel port is not used (the app may hold it), and SQLite over SSH is
//!   refused (its copy lives in the app).
//!
//! A connection MCP clients may not write to is opened read-only on the server
//! (`gate::session_writable`), so read-only and writable sessions of one profile are kept apart,
//! except for an engine that opens a file once per process (DuckDB): there one entry per file
//! holds whichever session the last call needed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::commands::connections::{
    check_socket, connect_password, connect_timeout, open_profile_tunnel, stored_tunnel_auth,
    tunnel_config,
};
use crate::drivers::{self, ConnectTarget, DbSession, ServerInfo};
use crate::error::AppError;
use crate::state::AppState;
use crate::tunnel::{TunnelHandle, sqlite_remote};
use crate::types::driver::Location;
use crate::types::profile::{ConnectionProfile, Driver, PasswordMode, SshAuth, SshPasswordMode};

/// Connections unused this long are closed.
pub const IDLE_CLOSE: Duration = Duration::from_secs(5 * 60);
/// Bound on closing one connection.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Profile {
        profile_id: Uuid,
        database: Option<String>,
        writable: bool,
    },
    /// An engine that opens a file once per process (`drivers::sharing`): every profile, and
    /// the read-only and writable sessions, of one file share one entry, which a call with other
    /// settings replaces.
    File(PathBuf),
}

impl Key {
    fn of(profile: &ConnectionProfile, database: Option<String>, writable: bool) -> Key {
        match drivers::sharing::file_key(profile) {
            Some(file) => Key::File(file),
            None => Key::Profile {
                profile_id: profile.id,
                database,
                writable,
            },
        }
    }
}

/// One open connection.
pub struct Entry {
    key: Key,
    /// The profile as connected (database override, read-only flag applied).
    pub profile: ConnectionProfile,
    pub session: Arc<dyn DbSession>,
    pub server: ServerInfo,
    tunnel: Option<TunnelHandle>,
    in_use: AtomicUsize,
    last_used: Mutex<Instant>,
}

impl Entry {
    fn touch(&self) {
        *self.last_used.lock().unwrap_or_else(|p| p.into_inner()) = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_used
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .elapsed()
    }

    fn alive(&self) -> bool {
        self.tunnel.as_ref().is_none_or(TunnelHandle::is_up)
    }

    async fn close(&self) {
        if tokio::time::timeout(CLOSE_TIMEOUT, self.session.close())
            .await
            .is_err()
        {
            log::warn!(
                "mcp: closing the session of {} timed out",
                self.profile.name
            );
        }
        if let Some(t) = &self.tunnel {
            t.close().await;
        }
    }
}

/// A connection in use by one call; it counts as idle again once dropped.
pub struct Lease {
    entry: Arc<Entry>,
}

impl std::ops::Deref for Lease {
    type Target = Entry;
    fn deref(&self) -> &Entry {
        &self.entry
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.entry.touch();
        self.entry.in_use.fetch_sub(1, Ordering::SeqCst);
    }
}

type Slot = Arc<tokio::sync::Mutex<Option<Arc<Entry>>>>;

pub struct Pool {
    state: Arc<AppState>,
    slots: Mutex<HashMap<Key, Slot>>,
    idle: Duration,
}

impl Pool {
    pub fn new(state: Arc<AppState>) -> Pool {
        Pool::with_idle(state, IDLE_CLOSE)
    }

    pub fn with_idle(state: Arc<AppState>, idle: Duration) -> Pool {
        Pool {
            state,
            slots: Mutex::new(HashMap::new()),
            idle,
        }
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    fn slot(&self, key: &Key) -> Slot {
        self.slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(key.clone())
            .or_default()
            .clone()
    }

    /// The open connection for the profile (and database), opening it when needed.
    pub async fn get(
        &self,
        profile: &ConnectionProfile,
        database: Option<&str>,
        writable: bool,
    ) -> Result<Lease, AppError> {
        let database = database
            .map(str::trim)
            .filter(|d| !d.is_empty() && profile.driver.descriptor().location != Location::File)
            .map(str::to_string);
        let key = Key::of(profile, database.clone(), writable);
        let slot = self.slot(&key);
        let mut guard = slot.lock().await;
        // The profile may have been edited since: a connection opened from older settings goes.
        let stale = guard.as_ref().is_some_and(|e| {
            !e.alive() || !same_settings(&e.profile, profile, database.as_deref(), writable)
        });
        if stale && let Some(old) = guard.take() {
            if old.in_use.load(Ordering::SeqCst) == 0 {
                old.close().await;
            } else {
                // Still running a call: it closes when its last session reference goes.
                log::info!("mcp: replacing the connection of {}", old.profile.name);
            }
        }
        let entry = match guard.as_ref() {
            Some(e) => e.clone(),
            None => {
                let opened = Arc::new(
                    open(&self.state, &key, profile, database.as_deref(), writable).await?,
                );
                *guard = Some(opened.clone());
                opened
            }
        };
        entry.in_use.fetch_add(1, Ordering::SeqCst);
        entry.touch();
        Ok(Lease { entry })
    }

    /// Puts `session` in the pool as the connection of `profile` (with `database` and
    /// `writable`), as `get` would open it, for tests that need a session no server gives.
    #[cfg(test)]
    pub(crate) async fn insert_for_test(
        &self,
        profile: &ConnectionProfile,
        database: Option<&str>,
        writable: bool,
        session: Arc<dyn DbSession>,
    ) {
        let key = Key::of(profile, database.map(str::to_string), writable);
        let entry = Entry {
            key: key.clone(),
            profile: effective_profile(profile, database, writable).unwrap(),
            session,
            server: ServerInfo::default(),
            tunnel: None,
            in_use: AtomicUsize::new(0),
            last_used: Mutex::new(Instant::now()),
        };
        *self.slot(&key).lock().await = Some(Arc::new(entry));
    }

    /// Drops the connection (after a connection-level failure), so the next call reconnects.
    pub async fn discard(&self, lease: Lease) {
        let entry = lease.entry.clone();
        drop(lease);
        let slot = self
            .slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&entry.key)
            .cloned();
        let Some(slot) = slot else {
            return;
        };
        let mut guard = slot.lock().await;
        if guard.as_ref().is_some_and(|e| Arc::ptr_eq(e, &entry)) {
            guard.take();
            if entry.in_use.load(Ordering::SeqCst) == 0 {
                entry.close().await;
            }
        }
    }

    /// Closes connections idle for longer than the idle limit (or whose tunnel went down).
    /// Returns how many were closed.
    pub async fn reap(&self) -> usize {
        let slots: Vec<Slot> = self
            .slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect();
        let mut closed = 0;
        for slot in slots {
            let Ok(mut guard) = slot.try_lock() else {
                continue; // being opened or replaced right now
            };
            let expired = guard.as_ref().is_some_and(|e| {
                e.in_use.load(Ordering::SeqCst) == 0 && (e.idle_for() >= self.idle || !e.alive())
            });
            if expired && let Some(entry) = guard.take() {
                log::info!("mcp: closing the idle connection to {}", entry.profile.name);
                entry.close().await;
                closed += 1;
            }
        }
        self.slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|_, slot| slot.try_lock().map_or(true, |g| g.is_some()));
        closed
    }

    /// Number of open connections.
    pub fn open_count(&self) -> usize {
        self.slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .filter(|s| s.try_lock().map_or(true, |g| g.is_some()))
            .count()
    }

    /// Closes everything (the client went away).
    pub async fn close_all(&self) {
        let slots: Vec<Slot> = self
            .slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain()
            .map(|(_, s)| s)
            .collect();
        for slot in slots {
            if let Some(entry) = slot.lock().await.take() {
                entry.close().await;
            }
        }
    }
}

/// Whether an open connection still matches the profile's current settings.
fn same_settings(
    opened: &ConnectionProfile,
    current: &ConnectionProfile,
    database: Option<&str>,
    writable: bool,
) -> bool {
    match effective_profile(current, database, writable) {
        Ok(p) => p == *opened,
        Err(_) => false,
    }
}

/// The profile as the MCP server connects it.
fn effective_profile(
    profile: &ConnectionProfile,
    database: Option<&str>,
    writable: bool,
) -> Result<ConnectionProfile, AppError> {
    let mut p = profile.clone();
    // Timestamps change on every connect in the app; they do not matter here.
    p.last_connected_at = None;
    p.updated_at = String::new();
    if let Some(db) = database {
        p.database = Some(db.to_string());
    }
    if !writable {
        p.options.read_only = true;
    }
    if let Some(ssh) = p.ssh.as_mut() {
        ssh.local_port = None;
    }
    Ok(p)
}

/// Refuses what needs the user in front of the app.
pub fn check_unattended(profile: &ConnectionProfile) -> Result<(), AppError> {
    let open_in_app = |what: &str| {
        AppError::Auth(format!(
            "\u{201c}{}\u{201d} needs {what}: open it in Tabler",
            profile.name
        ))
    };
    if profile.options.password_mode() == PasswordMode::Ask {
        return Err(open_in_app("a password"));
    }
    if let Some(ssh) = tunnel_config(profile)
        && ssh.auth == SshAuth::Password
        && ssh.password_mode() == SshPasswordMode::Ask
    {
        return Err(open_in_app("an SSH password"));
    }
    if sqlite_remote::applies(profile) {
        return Err(AppError::Unsupported(format!(
            "\u{201c}{}\u{201d} is a SQLite file over SSH: MCP clients cannot open it",
            profile.name
        )));
    }
    Ok(())
}

async fn open(
    state: &AppState,
    key: &Key,
    profile: &ConnectionProfile,
    database: Option<&str>,
    writable: bool,
) -> Result<Entry, AppError> {
    // A driver this build lacks, or a store that speaks no SQL (MCP clients get SQL sessions
    // only): refused before its script, password command or tunnel run.
    drivers::registry::ensure_built(profile)?;
    drivers::registry::ensure_sql(profile)?;
    let profile = effective_profile(profile, database, writable)?;
    check_unattended(&profile)?;
    crate::bootstrap_script::run_for_profile(&profile).await?;
    if profile.driver == Driver::Sqlite {
        drivers::sqlite::check_database_file(&profile)?;
    }
    check_socket(&profile)?;
    let password = connect_password(state, &profile, None).await?;
    let tunnel = match tunnel_config(&profile) {
        Some(ssh) => {
            // No `prompts`: keyboard-interactive questions cannot be answered here.
            let auth = stored_tunnel_auth(state, profile.id, ssh).await?;
            open_profile_tunnel(state, &profile, auth)
                .await
                .map_err(|e| match e {
                    AppError::HostKey {
                        host,
                        port,
                        changed: false,
                        ..
                    } => AppError::Tunnel(format!(
                        "the SSH host key of {host}:{port} is not trusted yet: connect to \
                         \u{201c}{}\u{201d} in Tabler once and trust it",
                        profile.name
                    )),
                    other => other,
                })?
        }
        None => None,
    };
    // Unknown host keys are parked for an app dialog that never comes.
    state
        .pending_host_keys
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&profile.id);
    let host_override = tunnel.as_ref().map(TunnelHandle::connect_addr);
    let attempt = Instant::now();
    let opened = async {
        let session = tokio::time::timeout(
            connect_timeout(&profile),
            drivers::connect(ConnectTarget {
                profile: &profile,
                password: password.as_deref(),
                host_override,
            }),
        )
        .await
        .map_err(|_| AppError::Timeout)??;
        match session.server_info().await {
            Ok(info) => Ok((session, info)),
            Err(e) => {
                session.close().await;
                Err(e)
            }
        }
    }
    .await;
    match opened {
        Ok((session, server)) => Ok(Entry {
            key: key.clone(),
            profile,
            session,
            server,
            tunnel,
            in_use: AtomicUsize::new(0),
            last_used: Mutex::new(Instant::now()),
        }),
        Err(e) => {
            if let Some(t) = &tunnel {
                let e = t.explain_error(e, attempt);
                t.close().await;
                return Err(e);
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appdb::AppDb;
    use crate::secrets::{Backend, Secrets};
    use crate::types::profile::{ProfileOptions, SslMode};

    pub fn sqlite_profile(path: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: Uuid::new_v4(),
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
            sqlite_path: Some(path.to_string()),
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    fn state() -> Arc<AppState> {
        let db = AppDb::open_in_memory().unwrap();
        Arc::new(AppState::new(db.clone(), Secrets::new(Backend::File, db)))
    }

    fn sqlite_file() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.sqlite");
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE t (a INTEGER);")
            .unwrap();
        let path = path.to_string_lossy().into_owned();
        (dir, path)
    }

    #[tokio::test]
    async fn reuses_connections_and_closes_idle_ones() {
        let (_dir, path) = sqlite_file();
        let p = sqlite_profile(&path);
        let pool = Pool::with_idle(state(), Duration::from_millis(50));
        let a = pool.get(&p, None, false).await.unwrap();
        let b = pool.get(&p, None, false).await.unwrap();
        assert!(Arc::ptr_eq(&a.entry, &b.entry));
        assert!(a.profile.options.read_only, "read-only on the server");
        // A writable session is a different connection.
        let w = pool.get(&p, None, true).await.unwrap();
        assert!(!Arc::ptr_eq(&a.entry, &w.entry));
        assert!(!w.profile.options.read_only);
        assert_eq!(pool.open_count(), 2);
        drop((a, b, w));
        // In use: never reaped.
        let held = pool.get(&p, None, false).await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(pool.reap().await, 1);
        assert_eq!(pool.open_count(), 1);
        drop(held);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(pool.reap().await, 1);
        assert_eq!(pool.open_count(), 0);
    }

    #[tokio::test]
    async fn an_edited_profile_reconnects() {
        let (_dir, path) = sqlite_file();
        let mut p = sqlite_profile(&path);
        let pool = Pool::new(state());
        let a = pool.get(&p, None, false).await.unwrap();
        let first = a.entry.clone();
        drop(a);
        p.updated_at = "later".into();
        let same = pool.get(&p, None, false).await.unwrap();
        assert!(
            Arc::ptr_eq(&same.entry, &first),
            "timestamps alone do not reconnect"
        );
        drop(same);
        p.options.startup_sql = Some("SELECT 1".into());
        let b = pool.get(&p, None, false).await.unwrap();
        assert!(!Arc::ptr_eq(&b.entry, &first));
        // A lost connection is dropped; the next call opens a new one.
        pool.discard(b).await;
        assert_eq!(pool.open_count(), 0);
        let c = pool.get(&p, None, false).await.unwrap();
        assert_eq!(pool.open_count(), 1);
        drop(c);
        pool.close_all().await;
        assert_eq!(pool.open_count(), 0);
    }

    #[tokio::test]
    async fn refuses_what_needs_the_user() {
        let pool = Pool::new(state());
        let (_dir, path) = sqlite_file();
        let mut ask = sqlite_profile(&path);
        ask.options.password_mode = Some(PasswordMode::Ask);
        let err = pool.get(&ask, None, false).await.err().unwrap();
        assert!(
            err.to_string()
                .contains("needs a password: open it in Tabler"),
            "{err}"
        );

        let mut ssh_ask = sqlite_profile(&path);
        ssh_ask.driver = Driver::Postgres;
        ssh_ask.sqlite_path = None;
        ssh_ask.ssh = Some(
            serde_json::from_value(serde_json::json!({
                "host": "example.invalid", "port": 22, "user": "me", "auth": "password",
                "localPort": 4000, "passwordMode": "ask"
            }))
            .unwrap(),
        );
        let err = pool.get(&ssh_ask, None, false).await.err().unwrap();
        assert!(err.to_string().contains("needs an SSH password"), "{err}");

        let missing = sqlite_profile("/nonexistent/dir/x.sqlite");
        assert!(pool.get(&missing, None, false).await.is_err());
        assert_eq!(pool.open_count(), 0);
    }

    /// A profile of a store that speaks no SQL is refused before its bootstrap script runs: as a
    /// driver this build lacks, or once built as no SQL session.
    #[tokio::test]
    async fn refuses_a_native_store_before_its_script() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let mut redis = sqlite_profile("/x");
        redis.driver = Driver::Redis;
        redis.sqlite_path = None;
        redis.host = Some("127.0.0.1".into());
        redis.options.bootstrap_script = Some(format!("echo ran > \"{}\"", marker.display()));
        let pool = Pool::new(state());
        let err = pool.get(&redis, None, false).await.err().unwrap();
        let expected = if drivers::registry::built(Driver::Redis) {
            "unsupported: Redis is not a SQL database: it has no SQL session"
        } else {
            "unsupported: Redis is not built into this Tabler"
        };
        assert_eq!(err.to_string(), expected);
        assert!(!marker.exists());
        assert_eq!(pool.open_count(), 0);
        // The script does run here: the refusal is what kept it from running.
        crate::bootstrap_script::run_for_profile(&redis)
            .await
            .unwrap();
        assert!(marker.exists());
    }

    /// An engine that opens a file once per process gets one entry per file, whatever the profile,
    /// database or mode; every other profile one per profile, database and mode.
    #[test]
    fn single_process_files_share_one_entry() {
        let (_dir, path) = sqlite_file();
        let mut duck = sqlite_profile(&path);
        duck.driver = Driver::Duckdb;
        let mut other = duck.clone();
        other.id = Uuid::new_v4();
        let key = Key::of(&duck, None, false);
        assert!(matches!(key, Key::File(_)));
        assert_eq!(Key::of(&other, Some("x".into()), true), key);
        let lite = sqlite_profile(&path);
        assert_ne!(Key::of(&lite, None, false), Key::of(&lite, None, true));
        assert_ne!(
            Key::of(&lite, None, false),
            Key::of(&sqlite_profile(&path), None, false)
        );
    }

    #[test]
    fn effective_profile_drops_the_pinned_port_and_forces_read_only() {
        let mut p = sqlite_profile("/x");
        p.driver = Driver::Postgres;
        p.database = Some("a".into());
        p.ssh = Some(
            serde_json::from_value(serde_json::json!({
                "host": "h", "port": 22, "user": "u", "auth": "agent", "localPort": 5555
            }))
            .unwrap(),
        );
        let e = effective_profile(&p, Some("b"), false).unwrap();
        assert_eq!(e.database.as_deref(), Some("b"));
        assert!(e.options.read_only);
        assert_eq!(e.ssh.unwrap().local_port, None);
        let e = effective_profile(&p, None, true).unwrap();
        assert_eq!(e.database.as_deref(), Some("a"));
        assert!(!e.options.read_only);
    }
}
