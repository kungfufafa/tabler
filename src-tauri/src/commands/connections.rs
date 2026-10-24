use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, State};
use tokio::sync::{RwLock, watch};
use uuid::Uuid;

use super::conn_extras::{live_encoding, with_live_encoding};
use crate::drivers::{self, ConnectTarget};
use crate::error::AppError;
use crate::secrets::{self, SecretKind};
use crate::state::{AppState, Connection};
use crate::tunnel::{
    self, AppHostKeys, ClockedPrompts, ForwardTarget, ForwardedPrompts, KeyboardInteractive,
    PromptClock, PromptEvent, SshPromptPurpose, TunnelAuth, TunnelHandle, TunnelState,
    TunnelStatus, TunnelStatusEvent,
};
use crate::types::driver::{Location, SshSupport};
use crate::types::profile::{
    ConnStatus, ConnectionInfo, ConnectionProfile, ConnectionStatusEvent,
    DEFAULT_CONNECT_TIMEOUT_SECS, Driver, PasswordMode, SshAuth, SshConfig, SshPasswordMode,
    TunnelInfo,
};

pub const EVENT_CONNECTION_STATUS: &str = "connection:status";
pub const EVENT_TUNNEL_STATUS: &str = "tunnel:status";
/// An SSH server asks a keyboard-interactive (2FA) question (`SshPromptRequest`).
pub const EVENT_SSH_PROMPT: &str = "ssh:prompt";
/// The backend stopped waiting for the answer to a question (`SshPromptDone`).
pub const EVENT_SSH_PROMPT_DONE: &str = "ssh:prompt-done";

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

/// How long a connect (the SSH tunnel and, separately, the database session) may take: the
/// profile's Advanced ▸ Connect timeout, else [`DEFAULT_CONNECT_TIMEOUT_SECS`] (60 s).
pub fn connect_timeout(profile: &ConnectionProfile) -> Duration {
    Duration::from_secs(
        profile
            .options
            .connect_timeout_secs
            .map(u64::from)
            .filter(|s| *s > 0)
            .unwrap_or(DEFAULT_CONNECT_TIMEOUT_SECS),
    )
}

/// The SSH settings of a profile that connects through a tunnel (SQLite files never do: they are
/// copied over SFTP; cloud services have no SSH).
pub fn tunnel_config(profile: &ConnectionProfile) -> Option<&SshConfig> {
    profile
        .ssh
        .as_ref()
        .filter(|_| profile.driver.descriptor().ssh == SshSupport::Tunnel)
}

/// Reads the stored SSH secret the profile's auth method needs (none for the agent), so other
/// Keychain entries are not touched. The SSH password follows its storage mode: read from the
/// Keychain, asked in the SSH prompt dialog ("Ask every time"), or none at all.
pub async fn stored_tunnel_auth(
    state: &AppState,
    profile_id: Uuid,
    ssh: &SshConfig,
) -> Result<TunnelAuth, AppError> {
    let mut auth = TunnelAuth::default();
    match ssh.auth {
        SshAuth::Password => match ssh.password_mode() {
            SshPasswordMode::Keychain => {
                auth.password = state
                    .secrets
                    .get(profile_id, SecretKind::SshPassword)
                    .await?;
            }
            SshPasswordMode::Ask => auth.ask_password = true,
            SshPasswordMode::None => {}
        },
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

/// Where MySQL "Use socket" over SSH looks on the SSH server when no socket path is typed, in
/// order (the first that opens is used): Debian / Ubuntu packages', the official packages' and
/// RHEL's places, systemd's `/run`, XAMPP's on Linux, then the macOS installs (Apple-silicon and
/// Intel Homebrew, MAMP, XAMPP, MacPorts) for an SSH server that is a Mac.
pub const REMOTE_SOCKET_CANDIDATES: [&str; 11] = [
    "/var/run/mysqld/mysqld.sock",
    "/tmp/mysql.sock",
    "/var/lib/mysql/mysql.sock",
    "/run/mysqld/mysqld.sock",
    "/opt/lampp/var/mysql/mysql.sock",
    "/opt/homebrew/var/mysql/mysql.sock",
    "/usr/local/var/mysql/mysql.sock",
    "/Applications/MAMP/tmp/mysql/mysql.sock",
    "/Applications/XAMPP/xamppfiles/var/mysql/mysql.sock",
    "/opt/local/var/run/mysql8/mysqld.sock",
    "/opt/local/var/run/mysql57/mysqld.sock",
];

/// The typed socket path of a "Use socket" profile, if any.
fn typed_socket_path(profile: &ConnectionProfile) -> Option<&str> {
    profile
        .options
        .socket_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
}

/// The socket paths on the SSH server a "Use socket" over SSH profile forwards to: the typed one,
/// else the usual places (the first that opens is used).
pub fn remote_socket_paths(profile: &ConnectionProfile) -> Vec<String> {
    match typed_socket_path(profile) {
        Some(path) => vec![path.to_string()],
        None => REMOTE_SOCKET_CANDIDATES
            .iter()
            .map(|p| (*p).to_string())
            .collect(),
    }
}

/// MySQL "Use socket" over SSH forwards the socket on the SSH server (TablePlus b228): a typed
/// path is the one there and must be absolute (`~` means nothing to the forwarding).
pub fn check_socket(profile: &ConnectionProfile) -> Result<(), AppError> {
    if profile.uses_socket()
        && tunnel_config(profile).is_some()
        && let Some(path) = typed_socket_path(profile)
        && !path.starts_with('/')
    {
        return Err(AppError::InvalidInput(format!(
            "Over SSH the socket path is the one on the SSH server: enter its full path (e.g. \
             {}), not \u{201c}{path}\u{201d}",
            REMOTE_SOCKET_CANDIDATES[0]
        )));
    }
    Ok(())
}

/// Whether two profiles reach the database through the same SSH tunnel: the same profile with the
/// same SSH settings and the same database host, port or socket (a database switch shares it).
pub fn same_tunnel_settings(a: &ConnectionProfile, b: &ConnectionProfile) -> bool {
    a.id == b.id
        && a.driver == b.driver
        && tunnel_config(a).is_some()
        && a.ssh == b.ssh
        && a.host == b.host
        && a.port == b.port
        && a.uses_socket() == b.uses_socket()
        && (!a.uses_socket() || typed_socket_path(a) == typed_socket_path(b))
}

/// Where the profile's SSH tunnel forwards to: its database host and port, or its socket on the
/// SSH server ("Use socket" over SSH).
pub fn forward_target(profile: &ConnectionProfile) -> ForwardTarget {
    if profile.uses_socket() {
        return ForwardTarget::Socket(remote_socket_paths(profile));
    }
    let host = profile
        .host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or(tunnel::LOCAL_HOST)
        .to_string();
    let port = profile.port.or_else(|| profile.default_port()).unwrap_or(0);
    ForwardTarget::Tcp { host, port }
}

/// Forwards the keyboard-interactive (2FA) questions of `profile`'s SSH server to the renderer
/// (`ssh:prompt`); its dialog answers with `answer_ssh_prompt`. The questions go to `window` (the
/// label of the window that started the connect or test) while it is open, else to the focused
/// window, so a 2FA dialog shows in one window only. `ssh:prompt-done` goes to every window: the
/// one showing the question drops it.
pub fn renderer_prompts(
    app: &AppHandle,
    state: &AppState,
    profile: &ConnectionProfile,
    purpose: SshPromptPurpose,
    window: Option<&str>,
) -> Arc<dyn KeyboardInteractive> {
    let app = app.clone();
    let window = window.map(str::to_string);
    Arc::new(ForwardedPrompts::new(
        state.ssh_prompts.clone(),
        move |event| match event {
            PromptEvent::Ask(request) => {
                crate::windows::emit_to_window(&app, window.as_deref(), EVENT_SSH_PROMPT, request);
            }
            PromptEvent::Done(done) => {
                let _ = app.emit(EVENT_SSH_PROMPT_DONE, done);
            }
        },
        profile.id,
        profile.name.clone(),
        purpose,
    ))
}

/// Opens the profile's SSH tunnel towards its database host and port, bounded by the profile's
/// connect timeout (the time the user spends answering the server's questions left out).
/// `Ok(None)` when the profile does not use SSH. Unknown host keys are parked in
/// `state.pending_host_keys` under the profile id and returned as `AppError::HostKey`.
pub async fn open_profile_tunnel(
    state: &AppState,
    profile: &ConnectionProfile,
    mut auth: TunnelAuth,
) -> Result<Option<TunnelHandle>, AppError> {
    let Some(ssh) = tunnel_config(profile) else {
        return Ok(None);
    };
    let trust = AppHostKeys::for_state(state, profile.id);
    let timeout = connect_timeout(profile);
    let clock = Arc::new(PromptClock::default());
    if let Some(prompts) = auth.prompts.take() {
        auth.prompts = Some(Arc::new(ClockedPrompts::new(prompts, clock.clone())));
    }
    let opened = tunnel::timeout_excluding_prompts(
        timeout,
        &clock,
        tunnel::open_tunnel_to(ssh, auth, forward_target(profile), &trust),
    )
    .await
    .ok_or_else(|| {
        AppError::Tunnel(format!(
            "timed out after {}s connecting to the SSH server {}:{}",
            timeout.as_secs(),
            ssh.host,
            ssh.port
        ))
    })??;
    Ok(Some(opened))
}

/// The DB password a connect uses, following the profile's password mode:
/// - Keychain: the one passed in (typed in a prompt), else the stored one.
/// - Ask every time: the one passed in, else the one typed earlier this app session (reconnects,
///   database switches); without either the connect is refused so the renderer asks.
/// - No password: none, whatever is passed in or stored.
/// - Command Line (IAM authentication): the output of the profile's command, run now (tokens
///   expire, so every connect, test and reconnect runs it again).
pub async fn connect_password(
    state: &AppState,
    profile: &ConnectionProfile,
    typed: Option<String>,
) -> Result<Option<String>, AppError> {
    match profile.options.password_mode() {
        PasswordMode::None => Ok(None),
        PasswordMode::Command => command_password(profile).await.map(Some),
        PasswordMode::Ask => match typed {
            Some(p) => Ok(Some(p)),
            None => state.session_password(profile.id).map(Some).ok_or_else(|| {
                AppError::Auth(format!(
                    "the password of \u{201c}{}\u{201d} is asked on every connect; enter it to connect",
                    profile.name
                ))
            }),
        },
        PasswordMode::Keychain => match typed {
            Some(p) => Ok(Some(p)),
            None => state.secrets.get(profile.id, SecretKind::DbPassword).await,
        },
    }
}

/// Runs the password command of a "Command Line (IAM authentication)" profile.
pub async fn command_password(profile: &ConnectionProfile) -> Result<String, AppError> {
    let command = profile.options.password_command().ok_or_else(|| {
        AppError::InvalidInput(format!(
            "\u{201c}{}\u{201d} gets its password from a command: enter it in the connection's \
             settings",
            profile.name
        ))
    })?;
    secrets::run_password_command(command, secrets::PASSWORD_COMMAND_TIMEOUT).await
}

async fn open_session(
    profile: &ConnectionProfile,
    password: Option<&str>,
    host_override: Option<(String, u16)>,
) -> Result<(Arc<dyn drivers::DbSession>, crate::drivers::ServerInfo), AppError> {
    let session = tokio::time::timeout(
        connect_timeout(profile),
        drivers::connect(ConnectTarget {
            profile,
            password,
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

/// Opens a saved profile. `password` is the DB password typed in the renderer's prompt (profiles
/// whose password is asked on every connect); `None` uses the stored or remembered one.
/// `database_override`: open this database instead of the profile's (another workspace on the same
/// server, TablePlus ⌘K → open a database; PostgreSQL / MySQL only). The SSH server's questions
/// go to the window that asked for the connect.
#[tauri::command]
pub async fn connect(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, AppState>,
    profile_id: Uuid,
    password: Option<String>,
    database_override: Option<String>,
) -> Result<ConnectionInfo, AppError> {
    let mut profile = state
        .appdb
        .get_profile(profile_id)?
        .ok_or_else(|| AppError::NotFound(format!("profile {profile_id}")))?;
    // A driver this build lacks: refused before its script, password command or tunnel run.
    drivers::registry::ensure_built(&profile)?;
    if let Some(database) = database_override.filter(|d| !d.trim().is_empty())
        && profile.driver.descriptor().location != Location::File
    {
        profile.database = Some(database);
    }
    // Before anything else: the "Bootstrap CLI script" may put the file in place or start a proxy.
    crate::bootstrap_script::run_for_profile(&profile).await?;
    // SQLite over SSH: the remote file is downloaded first; the driver opens the local copy.
    let origin = window.label().to_string();
    let (profile, sqlite_copy) = super::sqlite_ssh::localize(&app, profile, Some(&origin)).await?;
    if profile.driver == Driver::Sqlite {
        // Opening would create a missing file: a moved or mistyped path must not "connect".
        drivers::sqlite::check_database_file(&profile)?;
    }
    check_socket(&profile)?;
    let typed = password.clone();
    let password = connect_password(&state, &profile, password).await?;
    // Another workspace of the profile shares its open tunnel (a pinned local port could not be
    // bound twice, and the SSH server's questions are not asked again).
    let shared = state.shared_tunnel(&profile).await;
    let tunnel = match tunnel_config(&profile) {
        Some(_) if shared.is_some() => shared,
        Some(ssh) => {
            let mut auth = stored_tunnel_auth(&state, profile.id, ssh).await?;
            auth.prompts = Some(renderer_prompts(
                &app,
                &state,
                &profile,
                SshPromptPurpose::Connect,
                Some(&origin),
            ));
            if let Some(port) = ssh.local_port {
                release_pinned_port(&state, profile.id, port).await;
            }
            open_profile_tunnel(&state, &profile, auth).await?
        }
        None => None,
    };
    let host_override = tunnel.as_ref().map(TunnelHandle::connect_addr);
    let attempt = Instant::now();
    let (session, server) = match open_session(&profile, password.as_deref(), host_override).await {
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
    // Never persisted: reconnects reuse it while a connection of the profile is open.
    let session_password = typed.filter(|_| profile.options.password_mode() == PasswordMode::Ask);
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
        // A profile with an encoding other than UTF-8 is read-only (see `conn_options`).
        read_only: crate::conn_options::effective_read_only(&profile),
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
    state.insert_connection(conn, session_password).await;
    sqlite_copy.attach(connection_id);
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
    // The profile's last connection also forgets its "Ask every time" password.
    let conn = state.remove_connection(connection_id).await;
    if let Some(conn) = conn {
        conn.session().await.close().await;
        // Marked before the tunnel closes so the tunnel watcher does not report a lost connection.
        conn.status.send_replace(ConnStatus::Disconnected);
        if let Some(t) = &conn.tunnel {
            t.close().await;
        }
        // SQLite over SSH: the last changes go back to the server once the file is closed.
        super::sqlite_ssh::detach(connection_id).await;
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
    // View Using Encoding… changed the open session only: the new one keeps its encoding (a
    // session that cannot be asked keeps the profile's own).
    let current = conn.session().await;
    let profile = with_live_encoding(profile, live_encoding(&*current).await);
    drop(current);
    let password = connect_password(&state, &profile, None).await?;
    let attempt = Instant::now();
    let (session, server) = open_session(&profile, password.as_deref(), host_override)
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
    info.read_only = crate::conn_options::effective_read_only(&profile);
    Ok(info.clone())
}

/// Answers an SSH server's keyboard-interactive question (`ssh:prompt`): one answer per prompt,
/// or `None` to cancel (the connect or test waiting for it is given up). Answers that arrive after
/// the backend stopped waiting are ignored.
#[tauri::command]
pub async fn answer_ssh_prompt(
    state: State<'_, AppState>,
    request_id: String,
    answers: Option<Vec<String>>,
) -> Result<(), AppError> {
    if !state.ssh_prompts.answer(&request_id, answers) {
        log::debug!("ssh prompt {request_id} is no longer waiting for an answer");
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::profile::{ProfileOptions, SslMode};

    fn mysql_over_ssh() -> ConnectionProfile {
        ConnectionProfile {
            id: Uuid::new_v4(),
            name: "prod".into(),
            color: None,
            group: None,
            driver: Driver::Mysql,
            host: Some("10.0.0.5".into()),
            port: Some(3306),
            user: Some("app".into()),
            database: Some("shop".into()),
            ssl: SslMode::Disable,
            ssl_ca_path: None,
            sqlite_path: None,
            ssh: Some(SshConfig {
                host: "bastion".into(),
                port: 22,
                user: "deploy".into(),
                auth: SshAuth::Agent,
                key_path: None,
                local_port: Some(33_306),
                keepalive_secs: None,
                password_mode: None,
            }),
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    #[test]
    fn workspaces_of_a_profile_share_its_tunnel_while_the_settings_match() {
        let a = mysql_over_ssh();
        // Another database of the same server: same tunnel.
        let mut b = a.clone();
        b.database = Some("analytics".into());
        assert!(same_tunnel_settings(&a, &b));
        // Edited SSH settings, another host or another profile: a tunnel of its own.
        let mut edited = a.clone();
        edited.ssh.as_mut().unwrap().local_port = Some(33_307);
        assert!(!same_tunnel_settings(&a, &edited));
        let mut moved = a.clone();
        moved.host = Some("10.0.0.6".into());
        assert!(!same_tunnel_settings(&a, &moved));
        let mut other = a.clone();
        other.id = Uuid::new_v4();
        assert!(!same_tunnel_settings(&a, &other));
        // Without SSH there is no tunnel to share.
        let mut direct = a.clone();
        direct.ssh = None;
        assert!(!same_tunnel_settings(&direct, &direct.clone()));
        // A socket profile shares only with the same socket.
        let mut sock = a.clone();
        sock.options.use_socket = Some(true);
        assert!(!same_tunnel_settings(&a, &sock));
        let mut sock2 = sock.clone();
        sock2.options.socket_path = Some("/tmp/mysql.sock".into());
        assert!(!same_tunnel_settings(&sock, &sock2));
        assert!(same_tunnel_settings(&sock2, &sock2.clone()));
    }

    #[test]
    fn a_socket_over_ssh_forwards_the_servers_socket() {
        let mut p = mysql_over_ssh();
        assert_eq!(
            forward_target(&p),
            ForwardTarget::Tcp {
                host: "10.0.0.5".into(),
                port: 3306
            }
        );
        p.options.use_socket = Some(true);
        assert!(check_socket(&p).is_ok());
        // No path typed: the usual places on the server.
        assert_eq!(
            forward_target(&p),
            ForwardTarget::Socket(REMOTE_SOCKET_CANDIDATES.map(String::from).to_vec())
        );
        p.options.socket_path = Some(" /run/mysqld/mysqld.sock ".into());
        assert_eq!(
            forward_target(&p),
            ForwardTarget::Socket(vec!["/run/mysqld/mysqld.sock".into()])
        );
        // The path is the server's: it must be absolute there.
        p.options.socket_path = Some("~/mysql.sock".into());
        assert_eq!(check_socket(&p).unwrap_err().code(), "INVALID_INPUT");
        // Locally any path goes (the driver checks it exists).
        p.ssh = None;
        assert!(check_socket(&p).is_ok());
    }

    /// TablePlus waits 60 s for a connect (the SSH tunnel and, separately, the database).
    #[test]
    fn the_connect_timeout_defaults_to_60_seconds() {
        let mut p = mysql_over_ssh();
        assert_eq!(connect_timeout(&p), Duration::from_secs(60));
        assert_eq!(DEFAULT_CONNECT_TIMEOUT_SECS, 60);
        // 0 means unset; a typed value wins, up to the form's 300 s.
        p.options.connect_timeout_secs = Some(0);
        assert_eq!(connect_timeout(&p), Duration::from_secs(60));
        p.options.connect_timeout_secs = Some(5);
        assert_eq!(connect_timeout(&p), Duration::from_secs(5));
        p.options.connect_timeout_secs = Some(300);
        assert_eq!(connect_timeout(&p), Duration::from_secs(300));
    }

    #[test]
    fn the_remote_socket_candidates_keep_their_order_and_add_the_usual_installs() {
        assert_eq!(
            REMOTE_SOCKET_CANDIDATES[..3],
            [
                "/var/run/mysqld/mysqld.sock",
                "/tmp/mysql.sock",
                "/var/lib/mysql/mysql.sock"
            ]
        );
        for usual in [
            "/run/mysqld/mysqld.sock",
            "/usr/local/var/mysql/mysql.sock",
            "/Applications/MAMP/tmp/mysql/mysql.sock",
            "/Applications/XAMPP/xamppfiles/var/mysql/mysql.sock",
            "/opt/local/var/run/mysql8/mysqld.sock",
        ] {
            assert!(REMOTE_SOCKET_CANDIDATES.contains(&usual), "{usual}");
        }
        let mut seen = std::collections::HashSet::new();
        for c in REMOTE_SOCKET_CANDIDATES {
            assert!(c.starts_with('/') && seen.insert(c), "{c}");
        }
        // All of them are tried over SSH when no path is typed; a typed one is the only one.
        let mut p = mysql_over_ssh();
        p.options.use_socket = Some(true);
        assert_eq!(
            remote_socket_paths(&p).len(),
            REMOTE_SOCKET_CANDIDATES.len()
        );
        p.options.socket_path = Some("/srv/mysql.sock".into());
        assert_eq!(remote_socket_paths(&p), ["/srv/mysql.sock"]);
    }
}
