//! Process-wide state managed by Tauri.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, watch};
use uuid::Uuid;

use crate::appdb::AppDb;
use crate::drivers::{CancelSlot, DbSession};
use crate::error::AppError;
use crate::secrets::Secrets;
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
    pub session: RwLock<Arc<dyn DbSession>>,
    pub info: RwLock<ConnectionInfo>,
    pub schema_cache: RwLock<HashMap<TableRef, Arc<TableMeta>>>,
    pub status: watch::Sender<ConnStatus>,
    /// SSH tunnel the session connects through, if the profile uses one.
    pub tunnel: Option<crate::tunnel::TunnelHandle>,
}

impl Connection {
    pub async fn session(&self) -> Arc<dyn DbSession> {
        self.session.read().await.clone()
    }

    pub async fn info(&self) -> ConnectionInfo {
        self.info.read().await.clone()
    }

    pub async fn invalidate(&self, table: Option<&TableRef>) {
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
        if !conns.values().any(|c| c.profile.id == conn.profile.id) {
            self.set_session_password(conn.profile.id, None);
        }
        Some(conn)
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
        Ok(self.connection(id).await?.session().await)
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
        };
        Arc::new(Connection {
            id,
            profile: profile.clone(),
            session: RwLock::new(session),
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
            conn.session().await.close().await;
        }
    }
}
