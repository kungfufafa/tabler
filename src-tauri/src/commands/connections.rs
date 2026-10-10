use std::collections::HashMap;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};
use tokio::sync::{RwLock, watch};
use uuid::Uuid;

use crate::drivers::{self, ConnectTarget};
use crate::error::AppError;
use crate::secrets::SecretKind;
use crate::state::{AppState, Connection};
use crate::types::profile::{ConnStatus, ConnectionInfo, ConnectionProfile, ConnectionStatusEvent};

pub const EVENT_CONNECTION_STATUS: &str = "connection:status";

pub fn emit_status(
    app: &AppHandle,
    connection_id: Uuid,
    status: ConnStatus,
    message: Option<String>,
) {
    let _ = app.emit(
        EVENT_CONNECTION_STATUS,
        ConnectionStatusEvent {
            connection_id,
            status,
            message,
        },
    );
}

async fn open_session(
    state: &AppState,
    profile: &ConnectionProfile,
) -> Result<(Arc<dyn drivers::DbSession>, crate::drivers::ServerInfo), AppError> {
    let password = state
        .secrets
        .get(profile.id, SecretKind::DbPassword)
        .await?;
    let timeout =
        std::time::Duration::from_secs(profile.options.connect_timeout_secs.unwrap_or(15) as u64);
    let session = tokio::time::timeout(
        timeout,
        drivers::connect(ConnectTarget {
            profile,
            password: password.as_deref(),
            host_override: None,
        }),
    )
    .await
    .map_err(|_| AppError::Timeout)??;
    let info = session.server_info().await?;
    Ok((session, info))
}

#[tauri::command]
pub async fn connect(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: Uuid,
) -> Result<ConnectionInfo, AppError> {
    let profile = state
        .appdb
        .get_profile(profile_id)?
        .ok_or_else(|| AppError::NotFound(format!("profile {profile_id}")))?;
    let (session, server) = open_session(&state, &profile).await?;
    let connection_id = Uuid::new_v4();
    let info = ConnectionInfo {
        connection_id,
        profile_id,
        name: profile.name.clone(),
        color: profile.color.clone(),
        driver: profile.driver,
        server_version: server.version,
        current_database: server.current_database,
        current_user: server.current_user,
        tunnel: None,
    };
    let (status_tx, _) = watch::channel(ConnStatus::Connected);
    let conn = Arc::new(Connection {
        id: connection_id,
        profile: profile.clone(),
        session: RwLock::new(session),
        info: RwLock::new(info.clone()),
        schema_cache: RwLock::new(HashMap::new()),
        status: status_tx,
    });
    state.connections.write().await.insert(connection_id, conn);
    let _ = state.appdb.touch_last_connected(profile_id);
    emit_status(&app, connection_id, ConnStatus::Connected, None);
    log::info!(
        "connected {} ({:?}) as {}",
        profile.name,
        profile.driver,
        connection_id
    );
    Ok(info)
}

#[tauri::command]
pub async fn disconnect(
    app: AppHandle,
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<(), AppError> {
    let conn = state.connections.write().await.remove(&connection_id);
    if let Some(conn) = conn {
        conn.session().await.close().await;
        let _ = conn.status.send(ConnStatus::Disconnected);
        emit_status(&app, connection_id, ConnStatus::Disconnected, None);
    }
    Ok(())
}

#[tauri::command]
pub async fn list_connections(state: State<'_, AppState>) -> Result<Vec<ConnectionInfo>, AppError> {
    let conns: Vec<Arc<Connection>> = state.connections.read().await.values().cloned().collect();
    let mut out = Vec::with_capacity(conns.len());
    for c in conns {
        out.push(c.info().await);
    }
    Ok(out)
}

#[tauri::command]
pub async fn ping(
    app: AppHandle,
    state: State<'_, AppState>,
    connection_id: Uuid,
) -> Result<ConnStatus, AppError> {
    let conn = state.connection(connection_id).await?;
    match conn.session().await.ping().await {
        Ok(()) => {
            if *conn.status.borrow() != ConnStatus::Connected {
                let _ = conn.status.send(ConnStatus::Connected);
                emit_status(&app, connection_id, ConnStatus::Connected, None);
            }
            Ok(ConnStatus::Connected)
        }
        Err(e) => {
            let _ = conn.status.send(ConnStatus::Lost);
            emit_status(&app, connection_id, ConnStatus::Lost, Some(e.to_string()));
            Ok(ConnStatus::Lost)
        }
    }
}

/// Reconnects the same connection id against another database (PG needs a new session; MySQL
/// pools would otherwise drift, so both reconnect).
#[tauri::command]
pub async fn switch_database(
    state: State<'_, AppState>,
    connection_id: Uuid,
    database: String,
) -> Result<ConnectionInfo, AppError> {
    let conn = state.connection(connection_id).await?;
    let mut profile = conn.profile.clone();
    profile.database = Some(database);
    let (session, server) = open_session(&state, &profile).await?;
    let old = std::mem::replace(&mut *conn.session.write().await, session);
    old.close().await;
    conn.invalidate(None).await;
    let mut info = conn.info.write().await;
    info.server_version = server.version;
    info.current_database = server.current_database;
    info.current_user = server.current_user;
    Ok(info.clone())
}
