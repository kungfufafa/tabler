use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, State};
use tokio::sync::{RwLock, watch};
use uuid::Uuid;

use crate::drivers::{self, ConnectTarget};
use crate::error::AppError;
use crate::secrets::SecretKind;
use crate::state::{AppState, Connection};
use crate::tunnel::{
    self, AppHostKeys, TunnelAuth, TunnelHandle, TunnelState, TunnelStatus, TunnelStatusEvent,
};
use crate::types::profile::{
    ConnStatus, ConnectionInfo, ConnectionProfile, ConnectionStatusEvent, Driver, SshAuth,
    SshConfig, TunnelInfo,
};

pub const EVENT_CONNECTION_STATUS: &str = "connection:status";
pub const EVENT_TUNNEL_STATUS: &str = "tunnel:status";

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

pub fn emit_tunnel_status(
    app: &AppHandle,
    connection_id: Uuid,
    local_port: u16,
    status: TunnelStatus,
    message: Option<String>,
) {
    let _ = app.emit(
        EVENT_TUNNEL_STATUS,
        TunnelStatusEvent {
            connection_id,
            status,
            local_port: Some(local_port),
            message,
        },
    );
}

pub fn connect_timeout(profile: &ConnectionProfile) -> Duration {
    Duration::from_secs(u64::from(
        profile.options.connect_timeout_secs.unwrap_or(15).max(1),
    ))
}

/// The SSH settings of a profile that connects through a tunnel (SQLite files never do).
pub fn tunnel_config(profile: &ConnectionProfile) -> Option<&SshConfig> {
    profile
        .ssh
        .as_ref()
        .filter(|_| profile.driver != Driver::Sqlite)
}

/// Reads the stored SSH secret the profile's auth method needs (none for the agent), so other
/// Keychain entries are not touched.
pub async fn stored_tunnel_auth(
    state: &AppState,
    profile_id: Uuid,
    ssh: &SshConfig,
) -> Result<TunnelAuth, AppError> {
    let mut auth = TunnelAuth::default();
    match ssh.auth {
        SshAuth::Password => {
            auth.password = state
                .secrets
                .get(profile_id, SecretKind::SshPassword)
                .await?;
        }
        SshAuth::Key => {
            auth.passphrase = state
                .secrets
                .get(profile_id, SecretKind::SshPassphrase)
                .await?;
        }
        SshAuth::Agent => {}
    }
    Ok(auth)
}

/// Opens the profile's SSH tunnel towards its database host and port, bounded by the profile's
/// connect timeout. `Ok(None)` when the profile does not use SSH. Unknown host keys are parked in
/// `state.pending_host_keys` under the profile id and returned as `AppError::HostKey`.
pub async fn open_profile_tunnel(
    state: &AppState,
    profile: &ConnectionProfile,
    auth: TunnelAuth,
) -> Result<Option<TunnelHandle>, AppError> {
    let Some(ssh) = tunnel_config(profile) else {
        return Ok(None);
    };
    let target_host = profile
        .host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or(tunnel::LOCAL_HOST)
        .to_string();
    let target_port = profile
        .port
        .or_else(|| profile.driver.default_port())
        .unwrap_or(0);
    let trust = AppHostKeys::for_state(state, profile.id);
    let timeout = connect_timeout(profile);
    let opened = tokio::time::timeout(
        timeout,
        tunnel::open_tunnel(ssh, auth, target_host, target_port, &trust),
    )
    .await
    .map_err(|_| {
        AppError::Tunnel(format!(
            "timed out after {}s connecting to the SSH server {}:{}",
            timeout.as_secs(),
            ssh.host,
            ssh.port
        ))
    })??;
    Ok(Some(opened))
}

async fn open_session(
    state: &AppState,
    profile: &ConnectionProfile,
    host_override: Option<(String, u16)>,
) -> Result<(Arc<dyn drivers::DbSession>, crate::drivers::ServerInfo), AppError> {
    let password = state
        .secrets
        .get(profile.id, SecretKind::DbPassword)
        .await?;
    let session = tokio::time::timeout(
        connect_timeout(profile),
        drivers::connect(ConnectTarget {
            profile,
            password: password.as_deref(),
            host_override,
        }),
    )
    .await
    .map_err(|_| AppError::Timeout)??;
    let info = session.server_info().await?;
    Ok((session, info))
}

/// Bound on closing a dead connection's driver session (MySQL waits for pooled connections).
const SESSION_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Closes a connection's driver session so its pools stop opening new connections.
async fn close_session(conn: &Connection) {
    let session = conn.session().await;
    if tokio::time::timeout(SESSION_CLOSE_TIMEOUT, session.close())
        .await
        .is_err()
    {
        log::warn!("closing the session of connection {} timed out", conn.id);
    }
}

/// Bound on the health check of a connection of the same profile that holds a pinned port.
const HOLDER_PING_TIMEOUT: Duration = Duration::from_secs(3);

/// Whether a connection's database session still answers.
async fn session_alive(conn: &Connection) -> bool {
    let session = conn.session().await;
    matches!(
        tokio::time::timeout(HOLDER_PING_TIMEOUT, session.ping()).await,
        Ok(Ok(()))
    )
}

/// Frees the pinned local `port` before `profile_id` opens a tunnel on it. The renderer's
/// Reconnect opens the new connection before it disconnects the lost one, so the port may still
/// be held by
/// - an open connection whose tunnel died (it never comes back), or
/// - a connection of the same profile whose database session is lost while its tunnel is still up
///   (MySQL `wait_timeout`, a server restart, a terminated backend): the new connection replaces
///   it. A holder still marked connected is pinged first, since a hung ping leaves it so.
///
/// Such a connection is closed (session first, so its pools can never dial the port once another
/// tunnel owns it; then its tunnel) and marked disconnected, so its tunnel watcher stays quiet.
/// Healthy connections and other profiles' live tunnels are left alone: the port is in use.
pub async fn release_pinned_port(state: &AppState, profile_id: Uuid, port: u16) {
    if port == 0 {
        return;
    }
    let holders: Vec<Arc<Connection>> = state
        .connections
        .read()
        .await
        .values()
        .filter(|c| c.tunnel.as_ref().is_some_and(|t| t.local_port() == port))
        .cloned()
        .collect();
    for conn in holders {
        let Some(tunnel) = &conn.tunnel else {
            continue;
        };
        let reason = if !tunnel.is_up() {
            "the lost ssh tunnel"
        } else if conn.profile.id != profile_id {
            continue;
        } else {
            let lost = *conn.status.borrow() == ConnStatus::Lost;
            if !lost && session_alive(&conn).await {
                continue;
            }
            "the lost database session"
        };
        conn.status.send_replace(ConnStatus::Disconnected);
        close_session(&conn).await;
        tunnel.close().await;
        log::info!(
            "released local port {port} held by {reason} of connection {}",
            conn.id
        );
    }
}

/// Flags the connection as lost (and reports the tunnel state) once its tunnel goes down,
/// unless it was closed on purpose (disconnected, or replaced by a reconnect). The driver session
/// is closed too: the tunnel never comes back, and its port stays reserved (refusing connections)
/// until the connection is closed.
fn watch_tunnel(
    app: AppHandle,
    conn: Weak<Connection>,
    mut status: watch::Receiver<TunnelState>,
    local_port: u16,
) {
    tauri::async_runtime::spawn(async move {
        let down = match status.wait_for(|s| !s.is_up()).await {
            Ok(s) => s.clone(),
            Err(_) => TunnelState {
                status: TunnelStatus::Down,
                message: Some("the SSH session ended".into()),
            },
        };
        let Some(conn) = conn.upgrade() else {
            return;
        };
        if *conn.status.borrow() == ConnStatus::Disconnected {
            return;
        }
        emit_tunnel_status(&app, conn.id, local_port, down.status, down.message.clone());
        let reason = down
            .message
            .unwrap_or_else(|| "the SSH session ended".into());
        log::warn!("ssh tunnel of connection {} went down: {reason}", conn.id);
        conn.status.send_replace(ConnStatus::Lost);
        emit_status(
            &app,
            conn.id,
            ConnStatus::Lost,
            Some(format!("SSH tunnel down: {reason}")),
        );
        close_session(&conn).await;
    });
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
    let tunnel = match tunnel_config(&profile) {
        Some(ssh) => {
            let auth = stored_tunnel_auth(&state, profile.id, ssh).await?;
            if let Some(port) = ssh.local_port {
                release_pinned_port(&state, profile.id, port).await;
            }
            open_profile_tunnel(&state, &profile, auth).await?
        }
        None => None,
    };
    let host_override = tunnel.as_ref().map(TunnelHandle::connect_addr);
    let attempt = Instant::now();
    let (session, server) = match open_session(&state, &profile, host_override).await {
        Ok(opened) => opened,
        Err(e) => {
            if let Some(t) = &tunnel {
                let e = t.explain_error(e, attempt);
                t.close().await;
                return Err(e);
            }
            return Err(e);
        }
    };
    let connection_id = Uuid::new_v4();
    let tunnel_port = tunnel.as_ref().map(TunnelHandle::local_port);
    let info = ConnectionInfo {
        connection_id,
        profile_id,
        name: profile.name.clone(),
        color: profile.color.clone(),
        driver: profile.driver,
        server_version: server.version,
        current_database: server.current_database,
        current_user: server.current_user,
        tunnel: tunnel_port.map(|local_port| TunnelInfo { local_port }),
        read_only: profile.options.read_only,
    };
    let (status_tx, _) = watch::channel(ConnStatus::Connected);
    let conn = Arc::new(Connection {
        id: connection_id,
        profile: profile.clone(),
        session: RwLock::new(session),
        info: RwLock::new(info.clone()),
        schema_cache: RwLock::new(HashMap::new()),
        status: status_tx,
        tunnel,
    });
    if let (Some(t), Some(port)) = (&conn.tunnel, tunnel_port) {
        emit_tunnel_status(&app, connection_id, port, TunnelStatus::Up, None);
        watch_tunnel(app.clone(), Arc::downgrade(&conn), t.subscribe(), port);
    }
    state.connections.write().await.insert(connection_id, conn);
    let _ = state.appdb.touch_last_connected(profile_id);
    emit_status(&app, connection_id, ConnStatus::Connected, None);
    log::info!(
        "connected {} ({:?}) as {}{}",
        profile.name,
        profile.driver,
        connection_id,
        tunnel_port
            .map(|p| format!(" via ssh tunnel on port {p}"))
            .unwrap_or_default()
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
        // Marked before the tunnel closes so the tunnel watcher does not report a lost connection.
        conn.status.send_replace(ConnStatus::Disconnected);
        if let Some(t) = &conn.tunnel {
            t.close().await;
        }
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
    let result = match conn.tunnel.as_ref().map(TunnelHandle::state) {
        Some(t) if !t.is_up() => Err(AppError::Tunnel(
            t.message.unwrap_or_else(|| "the SSH session ended".into()),
        )),
        _ => conn.session().await.ping().await,
    };
    match result {
        Ok(()) => {
            if *conn.status.borrow() != ConnStatus::Connected {
                conn.status.send_replace(ConnStatus::Connected);
                emit_status(&app, connection_id, ConnStatus::Connected, None);
            }
            Ok(ConnStatus::Connected)
        }
        Err(e) => {
            conn.status.send_replace(ConnStatus::Lost);
            emit_status(&app, connection_id, ConnStatus::Lost, Some(e.to_string()));
            Ok(ConnStatus::Lost)
        }
    }
}

/// Reconnects the same connection id against another database (PG needs a new session; MySQL
/// pools would otherwise drift, so both reconnect). Reuses the connection's SSH tunnel.
#[tauri::command]
pub async fn switch_database(
    state: State<'_, AppState>,
    connection_id: Uuid,
    database: String,
) -> Result<ConnectionInfo, AppError> {
    let conn = state.connection(connection_id).await?;
    let mut profile = conn.profile.clone();
    profile.database = Some(database);
    let host_override = match &conn.tunnel {
        Some(t) if !t.is_up() => {
            return Err(AppError::Tunnel(format!(
                "{}; reconnect to open a new tunnel",
                t.state()
                    .message
                    .unwrap_or_else(|| "the SSH session ended".into())
            )));
        }
        Some(t) => Some(t.connect_addr()),
        None => None,
    };
    let attempt = Instant::now();
    let (session, server) = open_session(&state, &profile, host_override)
        .await
        .map_err(|e| match &conn.tunnel {
            Some(t) => t.explain_error(e, attempt),
            None => e,
        })?;
    let old = std::mem::replace(&mut *conn.session.write().await, session);
    old.close().await;
    conn.invalidate(None).await;
    let mut info = conn.info.write().await;
    info.server_version = server.version;
    info.current_database = server.current_database;
    info.current_user = server.current_user;
    Ok(info.clone())
}

/// Trusts the pending host key of a profile (after the user confirmed its fingerprint). With
/// `persist_to_known_hosts == Some(true)` the key is also appended to `~/.ssh/known_hosts`.
/// The renderer then retries `connect` / `test_connection`.
#[tauri::command]
pub async fn accept_host_key(
    state: State<'_, AppState>,
    profile_id: Uuid,
    fingerprint: String,
    persist_to_known_hosts: Option<bool>,
) -> Result<(), AppError> {
    let known_hosts = if persist_to_known_hosts == Some(true) {
        Some(tunnel::default_known_hosts_path().ok_or_else(|| {
            AppError::Io("no home directory, cannot update ~/.ssh/known_hosts".into())
        })?)
    } else {
        None
    };
    tunnel::accept_pending_host_key(
        &state.appdb,
        &state.pending_host_keys,
        profile_id,
        &fingerprint,
        known_hosts.as_deref(),
    )?;
    Ok(())
}
