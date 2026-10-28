//! Process-wide state managed by Tauri.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, watch};
use uuid::Uuid;

use crate::appdb::AppDb;
use crate::drivers::{CancelSlot, DbSession, Session};
use crate::error::AppError;
use crate::secrets::Secrets;
use crate::types::caps::{Feature, SessionCaps};
use crate::types::profile::{ConnStatus, ConnectionInfo, ConnectionProfile};
use crate::types::schema::{TableMeta, TableRef};

pub struct AppState {
    pub appdb: AppDb,
    pub secrets: Secrets,
    pub connections: RwLock<HashMap<Uuid, Arc<Connection>>>,
    /// Running editor queries, keyed by (connection id, query id from the renderer).
    pub active_queries: std::sync::Mutex<HashMap<(Uuid, String), CancelSlot>>,
    /// Running exports, keyed by export id.
    pub exports: std::sync::Mutex<HashMap<String, tokio_util::sync::CancellationToken>>,
    /// Host keys awaiting the user's decision, keyed by profile id.
    pub pending_host_keys: std::sync::Mutex<HashMap<Uuid, crate::tunnel::PendingHostKey>>,
    /// DB passwords of "Ask every time" profiles, typed at connect: kept in memory only, for the
    /// reconnects (and database switches) while a connection of the profile is open; forgotten
    /// when its last one disconnects. Keyed by profile id.
    pub session_passwords: std::sync::Mutex<HashMap<Uuid, String>>,
    /// SSH keyboard-interactive (2FA) questions waiting for the user's answer
    /// (`answer_ssh_prompt`), by request id.
    pub ssh_prompts: Arc<crate::tunnel::PendingPrompts>,
}

/// One open connection: the driver session plus caches and status.
pub struct Connection {
    pub id: Uuid,
    pub profile: ConnectionProfile,
    /// SQL or native; replaced in place by a database switch or an encoding change.
    pub session: RwLock<Session>,
    pub info: RwLock<ConnectionInfo>,
    pub schema_cache: RwLock<HashMap<TableRef, Arc<TableMeta>>>,
    pub status: watch::Sender<ConnStatus>,
    /// SSH tunnel the session connects through, if the profile uses one.
    pub tunnel: Option<crate::tunnel::TunnelHandle>,
}

impl Connection {
    /// The SQL session: a command that builds SQL asks for it this way, so a store that speaks
    /// no SQL is refused (`Unsupported`) before anything is sent.
    pub async fn session(&self) -> Result<Arc<dyn DbSession>, AppError> {
        self.session.read().await.sql()
    }

    /// The session whatever its kind, for what every session does (ping, close, introspection).
    pub async fn any_session(&self) -> Session {
        self.session.read().await.clone()
    }

    /// What the connection's session can do now.
    pub async fn caps(&self) -> SessionCaps {
        self.any_session().await.capabilities()
    }

    /// The SQL session, once its caps grant `f`: a command that needs `f` asks for its session
    /// this way, so it is refused before it sends anything (for a missing feature first, then a
    /// store that speaks no SQL for having no SQL session).
    pub async fn session_for(&self, f: Feature) -> Result<Arc<dyn DbSession>, AppError> {
        let session = self.any_session().await;
        session.capabilities().require(f)?;
        session.sql()
    }

    pub async fn info(&self) -> ConnectionInfo {
        self.info.read().await.clone()
    }

    pub async fn invalidate(&self, table: Option<&TableRef>) {
        // The assistant's structure context of this connection is read again too.
        crate::assistant::context_cache::forget(self.id);
        let mut cache = self.schema_cache.write().await;
        match table {
            Some(t) => {
                cache.remove(t);
            }
            None => cache.clear(),
        }
    }
}

impl AppState {
    pub fn new(appdb: AppDb, secrets: Secrets) -> AppState {
        AppState {
            appdb,
            secrets,
            connections: RwLock::new(HashMap::new()),
            active_queries: std::sync::Mutex::new(HashMap::new()),
            exports: std::sync::Mutex::new(HashMap::new()),
            pending_host_keys: std::sync::Mutex::new(HashMap::new()),
            session_passwords: std::sync::Mutex::new(HashMap::new()),
            ssh_prompts: Arc::new(crate::tunnel::PendingPrompts::default()),
        }
    }

    /// The in-memory password of an "Ask every time" profile, if one was typed this session.
    pub fn session_password(&self, profile_id: Uuid) -> Option<String> {
        self.session_passwords
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&profile_id)
            .cloned()
    }

    /// Remembers (`Some`) or forgets (`None`) a profile's session password.
    pub fn set_session_password(&self, profile_id: Uuid, password: Option<String>) {
        let mut map = self
            .session_passwords
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        match password {
            Some(p) => {
                map.insert(profile_id, p);
            }
            None => {
                map.remove(&profile_id);
            }
        }
    }

    /// Adds an open connection; `session_password` (typed for an "Ask every time" profile) is
    /// remembered under the same lock, so a concurrent last disconnect cannot forget it after.
    pub async fn insert_connection(&self, conn: Arc<Connection>, session_password: Option<String>) {
        let mut conns = self.connections.write().await;
        if session_password.is_some() {
            self.set_session_password(conn.profile.id, session_password);
        }
        conns.insert(conn.id, conn);
    }

    /// Removes an open connection. When it was the last one of its profile, the profile's
    /// session password is forgotten: an "Ask every time" connection asks again on its next
    /// connect (and backups no longer reuse it).
    pub async fn remove_connection(&self, id: Uuid) -> Option<Arc<Connection>> {
        let mut conns = self.connections.write().await;
        let conn = conns.remove(&id)?;
        crate::assistant::context_cache::forget(id);
        if !conns.values().any(|c| c.profile.id == conn.profile.id) {
            self.set_session_password(conn.profile.id, None);
        }
        Some(conn)
    }

    /// App exit: takes every open connection and closes its driver session, so the databases end
    /// cleanly. `app.exit()` runs no window handlers, so nothing else closes them: a SQLite file
    /// in WAL mode would keep its `-wal` / `-shm` files, with the pages that are not in the main
    /// file yet, and a SQLite-over-SSH copy would be uploaded without them. (The caller bounds
    /// the time this takes, and runs the SQLite-over-SSH uploads after it.)
    pub async fn close_all_sessions(&self) {
        let conns: Vec<Arc<Connection>> = {
            let mut map = self.connections.write().await;
            map.drain().map(|(_, c)| c).collect()
        };
        for conn in &conns {
            crate::assistant::context_cache::forget(conn.id);
        }
        futures::future::join_all(conns.iter().map(|c| async move {
            c.any_session().await.close().await;
        }))
        .await;
        self.session_passwords
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    pub async fn connection(&self, id: Uuid) -> Result<Arc<Connection>, AppError> {
        self.connections
            .read()
            .await
            .get(&id)
            .cloned()
            .ok_or(AppError::NotConnected)
    }

    pub async fn session(&self, id: Uuid) -> Result<Arc<dyn DbSession>, AppError> {
        self.connection(id).await?.session().await
    }

    /// `Connection::session_for` of an open connection.
    pub async fn session_for(&self, id: Uuid, f: Feature) -> Result<Arc<dyn DbSession>, AppError> {
        self.connection(id).await?.session_for(f).await
    }

    /// An open connection that has the file of `profile` open, for an engine that opens a file
    /// once per process (`drivers::sharing`); None for every other engine.
    pub async fn file_holder(&self, profile: &ConnectionProfile) -> Option<Arc<Connection>> {
        let key = crate::drivers::sharing::file_key(profile)?;
        let conns = self.connections.read().await;
        conns
            .values()
            .find(|c| crate::drivers::sharing::file_key(&c.profile).as_ref() == Some(&key))
            .cloned()
    }

    /// A new lease of the SSH tunnel an open connection of `profile` uses (same SSH settings and
    /// database host), while that tunnel is up: a second workspace of the profile goes through
    /// it. The tunnel closes with its last connection.
    pub async fn shared_tunnel(
        &self,
        profile: &ConnectionProfile,
    ) -> Option<crate::tunnel::TunnelHandle> {
        let conns = self.connections.read().await;
        conns
            .values()
            .filter(|c| crate::commands::connections::same_tunnel_settings(&c.profile, profile))
            .filter_map(|c| c.tunnel.as_ref())
            .find_map(crate::tunnel::TunnelHandle::share)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::{self, ConnectTarget};
    use crate::secrets::Backend;
    use crate::types::profile::{Driver, ProfileOptions, SslMode};

    fn sqlite_profile(id: Uuid, path: &str) -> ConnectionProfile {
        ConnectionProfile {
            id,
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

    async fn open(profile: &ConnectionProfile) -> Arc<Connection> {
        let session = drivers::connect(ConnectTarget {
            profile,
            password: None,
            host_override: None,
        })
        .await
        .unwrap();
        let id = Uuid::new_v4();
        let info = ConnectionInfo {
            connection_id: id,
            profile_id: profile.id,
            name: profile.name.clone(),
            color: None,
            driver: profile.driver,
            server_version: String::new(),
            current_database: None,
            current_user: None,
            tunnel: None,
            read_only: false,
            capabilities: None,
        };
        Arc::new(Connection {
            id,
            profile: profile.clone(),
            session: RwLock::new(session.into()),
            info: RwLock::new(info),
            schema_cache: RwLock::new(HashMap::new()),
            status: watch::channel(ConnStatus::Connected).0,
            tunnel: None,
        })
    }

    #[tokio::test]
    async fn the_last_disconnect_of_a_profile_forgets_its_session_password() {
        let appdb = AppDb::open_in_memory().unwrap();
        let state = AppState::new(appdb.clone(), Secrets::new(Backend::File, appdb));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.sqlite").to_string_lossy().into_owned();
        let (pid, other) = (Uuid::new_v4(), Uuid::new_v4());
        let (p, q) = (sqlite_profile(pid, &path), sqlite_profile(other, &path));
        let (a, b, c) = (open(&p).await, open(&p).await, open(&q).await);
        state
            .insert_connection(a.clone(), Some("typed".into()))
            .await;
        // A reconnect opens the new connection before the old one goes.
        state.insert_connection(b.clone(), None).await;
        state
            .insert_connection(c.clone(), Some("other".into()))
            .await;
        assert_eq!(state.session_password(pid).as_deref(), Some("typed"));

        assert!(state.remove_connection(a.id).await.is_some());
        assert_eq!(state.session_password(pid).as_deref(), Some("typed"));
        assert!(state.remove_connection(b.id).await.is_some());
        assert_eq!(state.session_password(pid), None);
        // Other profiles keep theirs; unknown ids change nothing.
        assert_eq!(state.session_password(other).as_deref(), Some("other"));
        assert!(state.remove_connection(Uuid::new_v4()).await.is_none());
        assert_eq!(state.session_password(other).as_deref(), Some("other"));
        for conn in [a, b, c] {
            conn.any_session().await.close().await;
        }
    }

    /// Quitting closes the local SQLite sessions: a WAL database leaves no `-wal` / `-shm` file,
    /// and what was written is in the main file (it used to stay in the WAL until the process
    /// died, or was lost with the files).
    #[tokio::test]
    async fn closing_all_sessions_checkpoints_a_wal_database() {
        let appdb = AppDb::open_in_memory().unwrap();
        let state = AppState::new(appdb.clone(), Secrets::new(Backend::File, appdb));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wal.sqlite");
        let path_str = path.to_string_lossy().into_owned();
        let (pid, other) = (Uuid::new_v4(), Uuid::new_v4());
        let (a, b) = (
            open(&sqlite_profile(pid, &path_str)).await,
            open(&sqlite_profile(other, &path_str)).await,
        );
        let session = a.session().await.unwrap();
        session
            .query("PRAGMA journal_mode = WAL", &[])
            .await
            .unwrap();
        session
            .execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)", &[])
            .await
            .unwrap();
        session
            .execute("INSERT INTO t (v) VALUES ('kept')", &[])
            .await
            .unwrap();
        let wal = dir.path().join("wal.sqlite-wal");
        let shm = dir.path().join("wal.sqlite-shm");
        assert!(
            wal.exists() && shm.exists(),
            "an open WAL database has both files"
        );
        state.insert_connection(a.clone(), Some("pw".into())).await;
        state.insert_connection(b.clone(), None).await;
        drop(session);

        state.close_all_sessions().await;

        assert!(!wal.exists(), "the -wal file is left behind");
        assert!(!shm.exists(), "the -shm file is left behind");
        assert!(state.connections.read().await.is_empty());
        assert_eq!(state.session_password(pid), None);
        // The sessions are closed: further queries fail.
        let closed = a.session().await.unwrap();
        assert!(closed.execute("SELECT 1", &[]).await.is_err());
        // The row survived: it is in the main file.
        let conn = rusqlite::Connection::open(&path).unwrap();
        let v: String = conn.query_row("SELECT v FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "kept");
        // Nothing open: harmless.
        state.close_all_sessions().await;
    }
}
