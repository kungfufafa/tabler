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
        }
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
