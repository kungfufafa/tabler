//! SSH tunnels (russh): a listener on 127.0.0.1 whose connections are forwarded to the database
//! host through an SSH server (`direct-tcpip` channels). The database driver then connects to the
//! listener through `ConnectTarget::host_override`.
//!
//! Lifecycle: [`open_tunnel`] connects, checks the host key ([`HostKeyTrust`]), authenticates and
//! starts the listener. [`TunnelHandle::close`] (or dropping the handle) stops the listener and
//! disconnects. The status watch ([`TunnelHandle::subscribe`]) flips to down/error when the SSH
//! session ends on its own (server gone, keepalive timeout). The local port stays reserved until
//! the handle is closed: after the session ends the listener keeps accepting and immediately
//! closes every connection, so a database pool that still dials the port can never reach another
//! tunnel (or another process) that would otherwise be able to bind the freed port.
//!
//! The listener never shares its port: it is bound without `SO_REUSEADDR` (see [`bind_local`]),
//! so on macOS/BSD a pinned port cannot take loopback traffic away from another program that
//! listens on the wildcard address of that port.

mod hostkeys;

use std::fmt::Display;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh::client::{self, AuthResult};
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelOpenFailure, Disconnect, MethodKind, MethodSet};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;
use uuid::Uuid;

pub use hostkeys::{
    AppHostKeys, HostKeyMatch, HostKeyTrust, MemoryHostKeys, PendingHostKey,
    accept_pending_host_key, append_known_host, check_known_hosts_file, default_known_hosts_path,
    fingerprint, key_type, lookup_trusted_host, normalize_host, trust_host_key, verify_host_key,
};

use crate::error::AppError;
use crate::types::profile::{SshAuth, SshConfig};

const DEFAULT_KEEPALIVE_SECS: u64 = 30;
const KEEPALIVE_MAX: usize = 3;
/// Address the local listener binds to and drivers connect to.
pub const LOCAL_HOST: &str = "127.0.0.1";
/// How long a connect to a busy pinned port may take before it counts as "something listens".
const PORT_PROBE_TIMEOUT: Duration = Duration::from_secs(1);
/// How long to wait for the session to report its end after an authentication reply that may
/// only mean "the connection closed" (see [`session_end`]).
const SESSION_END_GRACE: Duration = Duration::from_millis(500);

/// SSH secrets for one tunnel: the password (password auth) or the key passphrase (key auth).
#[derive(Clone, Default)]
pub struct TunnelAuth {
    pub password: Option<String>,
    pub passphrase: Option<String>,
    /// SSH agent socket for agent auth; `None` uses `SSH_AUTH_SOCK`.
    pub agent_socket: Option<PathBuf>,
}

impl std::fmt::Debug for TunnelAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelAuth")
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("passphrase", &self.passphrase.as_ref().map(|_| "***"))
            .field("agent_socket", &self.agent_socket)
            .finish()
    }
}

/// Tunnel state as the renderer sees it in `tunnel:status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum TunnelStatus {
    Up,
    /// Closed by the app or by the server.
    Down,
    /// The SSH session failed (network error, keepalive timeout, …).
    Error,
}

/// Payload of the global `tunnel:status` event.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TunnelStatusEvent {
    pub connection_id: Uuid,
    pub status: TunnelStatus,
    pub local_port: Option<u16>,
    pub message: Option<String>,
}

/// Value of a tunnel's status watch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelState {
    pub status: TunnelStatus,
    pub message: Option<String>,
}

impl TunnelState {
    fn up() -> TunnelState {
        TunnelState {
            status: TunnelStatus::Up,
            message: None,
        }
    }

    pub fn is_up(&self) -> bool {
        self.status == TunnelStatus::Up
    }
}

/// Moves an up tunnel to `status`; later reports are ignored so the first cause wins.
fn mark_down(state: &watch::Sender<TunnelState>, status: TunnelStatus, message: &str) {
    state.send_if_modified(|s| {
        if s.is_up() {
            *s = TunnelState {
                status,
                message: Some(message.to_string()),
            };
            true
        } else {
            false
        }
    });
}

/// Why the SSH server could not open a forwarded connection, and when.
#[derive(Debug, Clone)]
struct ForwardFailure {
    at: Instant,
    message: String,
}

type LastForwardFailure = Arc<Mutex<Option<ForwardFailure>>>;

/// Keeps a tunnel alive; dropping it (or calling `close`) shuts the listener and SSH session down.
pub struct TunnelHandle {
    pub local_port: u16,
    target: String,
    /// Cancelled by `close` / `Drop`: stops the listener, forwarded connections and the session.
    cancel: CancellationToken,
    state: Arc<watch::Sender<TunnelState>>,
    session: Arc<client::Handle<Client>>,
    listener_task: Mutex<Option<JoinHandle<()>>>,
    forward_error: LastForwardFailure,
}

impl std::fmt::Debug for TunnelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelHandle")
            .field("local_port", &self.local_port)
            .field("target", &self.target)
            .field("state", &*self.state.borrow())
            .finish()
    }
}

impl TunnelHandle {
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// `host_override` for `drivers::ConnectTarget`.
    pub fn connect_addr(&self) -> (String, u16) {
        (LOCAL_HOST.to_string(), self.local_port)
    }

    pub fn state(&self) -> TunnelState {
        self.state.borrow().clone()
    }

    pub fn is_up(&self) -> bool {
        self.state.borrow().is_up()
    }

    /// Watch that leaves `Up` once when the tunnel goes down (closed, server gone, keepalive
    /// timeout). It never comes back up: a new tunnel is a new handle.
    pub fn subscribe(&self) -> watch::Receiver<TunnelState> {
        self.state.subscribe()
    }

    /// The last reason a forwarded connection could not be opened, if that happened at or after
    /// `since` (taken, so it is reported once; an older failure is discarded).
    pub fn take_forward_error(&self, since: Instant) -> Option<String> {
        let failure = lock(&self.forward_error).take()?;
        (failure.at >= since).then_some(failure.message)
    }

    /// Replaces a vague driver error ("connection closed") with the tunnel's own diagnosis when
    /// the tunnel is the reason the database connection failed. `since` is when the failed attempt
    /// started: forwarding failures from before it (e.g. a pool reconnect long ago) are unrelated.
    pub fn explain_error(&self, err: AppError, since: Instant) -> AppError {
        if matches!(err, AppError::Auth(_) | AppError::Db { .. }) {
            return err;
        }
        if let Some(msg) = self.take_forward_error(since) {
            return AppError::Tunnel(msg);
        }
        let state = self.state();
        if !state.is_up() {
            return AppError::Tunnel(
                state
                    .message
                    .unwrap_or_else(|| "the SSH session ended".to_string()),
            );
        }
        err
    }

    /// Stops the listener (new connections are refused and the port is free once this returns),
    /// closes forwarded connections and disconnects the SSH session. Idempotent.
    pub async fn close(&self) {
        mark_down(&self.state, TunnelStatus::Down, "tunnel closed");
        self.cancel.cancel();
        let task = lock(&self.listener_task).take();
        if let Some(task) = task {
            let _ = task.await;
        }
        let _ = self
            .session
            .disconnect(Disconnect::ByApplication, "tunnel closed", "en")
            .await;
    }
}

impl Drop for TunnelHandle {
    fn drop(&mut self) {
        mark_down(&self.state, TunnelStatus::Down, "tunnel closed");
        // The listener loop and the supervisor see the token and finish the shutdown.
        self.cancel.cancel();
    }
}

/// Opens an SSH session described by `cfg`, verifies the server key against `trust`,
/// authenticates and listens on `127.0.0.1:<cfg.local_port or ephemeral>`; every accepted
/// connection is forwarded to `target_host:target_port` as seen from the SSH server.
pub async fn open_tunnel(
    cfg: &SshConfig,
    auth: TunnelAuth,
    target_host: String,
    target_port: u16,
    trust: &dyn HostKeyTrust,
) -> Result<TunnelHandle, AppError> {
    let ssh_host = cfg.host.trim().to_string();
    if ssh_host.is_empty() {
        return Err(AppError::InvalidInput("SSH host is required".into()));
    }
    let user = cfg.user.trim().to_string();
    if user.is_empty() {
        return Err(AppError::InvalidInput("SSH user is required".into()));
    }
    if target_port == 0 {
        return Err(AppError::InvalidInput(
            "database port is required for an SSH tunnel".into(),
        ));
    }
    let ssh_port = if cfg.port == 0 { 22 } else { cfg.port };

    // Local problems (missing key, wrong passphrase, port in use) fail before any network I/O.
    let credential = match cfg.auth {
        SshAuth::Password => Credential::Password(auth.password.clone().ok_or_else(|| {
            AppError::Auth("ssh: no SSH password is saved for this connection".into())
        })?),
        SshAuth::Key => Credential::Key(load_private_key(
            cfg.key_path.as_deref(),
            auth.passphrase.as_deref(),
        )?),
        SshAuth::Agent => Credential::Agent(auth.agent_socket.clone()),
    };
    let listener = bind_local(cfg.local_port.unwrap_or(0)).await?;
    let local_port = listener.local_addr()?.port();

    let keepalive = cfg
        .keepalive_secs
        .filter(|s| *s > 0)
        .map(u64::from)
        .unwrap_or(DEFAULT_KEEPALIVE_SECS);
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(keepalive)),
        keepalive_max: KEEPALIVE_MAX,
        inactivity_timeout: None,
        nodelay: true,
        ..Default::default()
    });
    let state = Arc::new(watch::channel(TunnelState::up()).0);
    let (check_tx, check_rx) = oneshot::channel();
    let handler = Client {
        host_key_check: Some(check_tx),
        accepted_key: None,
        state: state.clone(),
    };

    // From here until the tunnel is up, `abort` shuts the SSH socket down if this future is
    // dropped (the caller's connect timeout) or returns an error.
    let (mut session, abort) =
        handshake(config, &ssh_host, ssh_port, handler, check_rx, trust).await?;
    if let Err(e) = authenticate(&mut session, &user, credential, &state).await {
        let _ = session
            .disconnect(Disconnect::ByApplication, "authentication failed", "en")
            .await;
        return Err(e);
    }
    abort.disarm();
    log::info!(
        "ssh tunnel {LOCAL_HOST}:{local_port} -> {user}@{ssh_host}:{ssh_port} -> {target_host}:{target_port}"
    );

    let session = Arc::new(session);
    let cancel = CancellationToken::new();
    // Cancelled when the SSH session ends (or with `cancel`): forwarding stops, the port does not.
    let forwarding = cancel.child_token();
    let forward_error = Arc::new(Mutex::new(None));
    let target = format!("{target_host}:{target_port}");
    let listener_task = tokio::spawn(accept_loop(
        listener,
        Forwarder {
            session: session.clone(),
            target_host: Arc::from(target_host),
            target_port,
            forward_error: forward_error.clone(),
        },
        cancel.clone(),
        forwarding.clone(),
    ));
    tokio::spawn(supervise(
        session.clone(),
        state.subscribe(),
        cancel.clone(),
        forwarding,
    ));

    Ok(TunnelHandle {
        local_port,
        target,
        cancel,
        state,
        session,
        listener_task: Mutex::new(Some(listener_task)),
        forward_error,
    })
}

// ----- local listener ---------------------------------------------------------------------------

/// Binds the tunnel's listener on `127.0.0.1:<port>` (`0`: any free port).
///
/// The socket is bound without `SO_REUSEADDR` (tokio's `TcpListener::bind` always sets it on
/// Unix). With that flag, macOS/BSD let a socket bind `127.0.0.1:P` while another program listens
/// on the wildcard address `*:P` (Docker Desktop publishing a local database, for one) and then
/// route every loopback connection to `127.0.0.1:P` into the tunnel: psql, other tools and other
/// profiles would silently reach the tunnel's remote database instead of the local one.
///
/// Without the flag, though, a pinned port is also refused while connections of an earlier tunnel
/// on it are still closing down (TIME_WAIT), which would break an immediate reconnect. So when a
/// pinned port is busy, it is reused only if nothing answers on it, i.e. no program listens there.
async fn bind_local(port: u16) -> Result<TcpListener, AppError> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let cannot_listen = |e: &dyn Display| AppError::Tunnel(format!("cannot listen on {addr}: {e}"));
    let in_use = match listen_local(addr, false) {
        Ok(listener) => return Ok(listener),
        Err(e) if port != 0 && e.kind() == io::ErrorKind::AddrInUse => e,
        Err(e) => return Err(cannot_listen(&e)),
    };
    if port_answers(addr).await {
        return Err(cannot_listen(&format!(
            "{in_use}: the port is already in use (another program, or another connection's \
             tunnel, listens on it)"
        )));
    }
    // Only sockets that are winding down hold the port.
    #[cfg(not(windows))]
    {
        listen_local(addr, true).map_err(|e| cannot_listen(&e))
    }
    // Windows does not keep a port from being bound for TIME_WAIT, and its SO_REUSEADDR would
    // allow stealing a port that is genuinely in use.
    #[cfg(windows)]
    {
        Err(cannot_listen(&in_use))
    }
}

fn listen_local(addr: SocketAddr, reuse_addr: bool) -> io::Result<TcpListener> {
    let socket = TcpSocket::new_v4()?;
    if reuse_addr {
        socket.set_reuseaddr(true)?;
    }
    socket.bind(addr)?;
    socket.listen(1024)
}

/// Whether something accepts connections on `addr`. A probe that neither connects nor is refused
/// (timeout, odd errors) counts as "yes": the port is then left alone.
async fn port_answers(addr: SocketAddr) -> bool {
    match tokio::time::timeout(PORT_PROBE_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => e.kind() != io::ErrorKind::ConnectionRefused,
        Err(_) => true,
    }
}

// ----- russh client handler ---------------------------------------------------------------------

/// Sent by the handler when the server presents its key; the reply decides whether to go on.
struct HostKeyCheck {
    key: PublicKey,
    reply: oneshot::Sender<Result<(), AppError>>,
}

#[derive(Debug)]
enum ClientError {
    Ssh(russh::Error),
    HostKey(AppError),
}

impl From<russh::Error> for ClientError {
    fn from(e: russh::Error) -> Self {
        ClientError::Ssh(e)
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Ssh(e) => f.write_str(&describe_ssh_error(e)),
            ClientError::HostKey(e) => write!(f, "{e}"),
        }
    }
}

struct Client {
    host_key_check: Option<oneshot::Sender<HostKeyCheck>>,
    /// Key accepted at the first key exchange; re-keying must present the same key.
    accepted_key: Option<PublicKey>,
    state: Arc<watch::Sender<TunnelState>>,
}

impl client::Handler for Client {
    type Error = ClientError;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = server_public_key.public_key();
        if let Some(accepted) = &self.accepted_key {
            return Ok(accepted.key_data() == key.key_data());
        }
        let Some(tx) = self.host_key_check.take() else {
            return Ok(false);
        };
        let (reply, verdict) = oneshot::channel();
        if tx
            .send(HostKeyCheck {
                key: key.clone(),
                reply,
            })
            .is_err()
        {
            return Ok(false);
        }
        match verdict.await {
            Ok(Ok(())) => {
                self.accepted_key = Some(key);
                Ok(true)
            }
            Ok(Err(e)) => Err(ClientError::HostKey(e)),
            Err(_) => Ok(false),
        }
    }

    async fn disconnected(
        &mut self,
        reason: client::DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        match reason {
            client::DisconnectReason::ReceivedDisconnect(info) => {
                let msg = if info.message.trim().is_empty() {
                    "the SSH server closed the connection".to_string()
                } else {
                    format!(
                        "the SSH server closed the connection: {}",
                        info.message.trim()
                    )
                };
                mark_down(&self.state, TunnelStatus::Down, &msg);
                Ok(())
            }
            client::DisconnectReason::Error(e) => {
                mark_down(&self.state, TunnelStatus::Error, &e.to_string());
                Err(e)
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // The session task owns the handler, so this runs whenever the SSH session ends.
        mark_down(&self.state, TunnelStatus::Down, "the SSH session ended");
    }
}

fn describe_ssh_error(e: &russh::Error) -> String {
    match e {
        russh::Error::IO(io) => io.to_string(),
        russh::Error::KeepaliveTimeout => "the SSH server stopped answering keepalives".into(),
        russh::Error::HUP | russh::Error::Disconnect => {
            "the SSH server closed the connection".into()
        }
        russh::Error::ConnectionTimeout => "timed out connecting to the SSH server".into(),
        other => other.to_string(),
    }
}

/// Shuts the SSH server socket down when dropped, unless disarmed once the tunnel is up.
///
/// russh spawns the session task during the handshake and dropping its handle does not stop it:
/// while the key exchange runs, that task only waits on the socket. Without this guard, a caller
/// that gives up on [`open_tunnel`] (connect timeout) against a server that stalls after its
/// banner would leave the task and the socket open until the server hangs up.
struct AbortOnDrop(Option<std::net::TcpStream>);

impl AbortOnDrop {
    /// Hands `tcp` back together with a guard that holds a duplicate of its socket.
    fn wrap(tcp: TcpStream) -> io::Result<(TcpStream, AbortOnDrop)> {
        let tcp = tcp.into_std()?;
        let guard = AbortOnDrop(Some(tcp.try_clone()?));
        Ok((TcpStream::from_std(tcp)?, guard))
    }

    /// Keeps the socket open; the session owns it from now on.
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(socket) = self.0.take() {
            // Pending reads in the session task see EOF, so it ends and drops its copy.
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// Connects to the SSH server and runs the handshake while answering the handler's host-key
/// question with `trust`.
async fn handshake(
    config: Arc<client::Config>,
    host: &str,
    port: u16,
    handler: Client,
    mut check_rx: oneshot::Receiver<HostKeyCheck>,
    trust: &dyn HostKeyTrust,
) -> Result<(client::Handle<Client>, AbortOnDrop), AppError> {
    let unreachable = |e: &dyn Display| {
        AppError::Tunnel(format!(
            "cannot connect to the SSH server {host}:{port}: {e}"
        ))
    };
    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|e| unreachable(&e))?;
    if config.nodelay
        && let Err(e) = tcp.set_nodelay(true)
    {
        log::debug!("ssh: set_nodelay failed: {e}");
    }
    let (tcp, abort) = AbortOnDrop::wrap(tcp).map_err(|e| unreachable(&e))?;
    let connect = client::connect_stream(config, tcp, handler);
    tokio::pin!(connect);
    let mut checked = false;
    let result = loop {
        tokio::select! {
            res = &mut connect => break res,
            check = &mut check_rx, if !checked => {
                checked = true;
                if let Ok(HostKeyCheck { key, reply }) = check {
                    let _ = reply.send(verify_host_key(trust, host, port, &key));
                }
            }
        }
    };
    match result {
        Ok(session) => Ok((session, abort)),
        Err(ClientError::HostKey(e)) => Err(e),
        Err(ClientError::Ssh(e)) => Err(unreachable(&describe_ssh_error(&e))),
    }
}

// ----- authentication ---------------------------------------------------------------------------

enum Credential {
    Password(String),
    Key(Arc<PrivateKey>),
    /// Agent socket override (`None`: `SSH_AUTH_SOCK`).
    Agent(Option<PathBuf>),
}

fn ssh_err(e: russh::Error) -> AppError {
    AppError::Tunnel(describe_ssh_error(&e))
}

fn method_names(methods: &MethodSet) -> String {
    methods
        .iter()
        .filter(|m| **m != MethodKind::None)
        .map(<&'static str>::from)
        .collect::<Vec<_>>()
        .join(", ")
}

fn auth_failure(method: &str, user: &str, remaining: &MethodSet, partial: bool) -> AppError {
    let accepted = method_names(remaining);
    let mut msg = if partial {
        format!(
            "ssh: {method} authentication for user \"{user}\" succeeded, but the server also \
             requires a second factor, which is not supported"
        )
    } else {
        format!("ssh: {method} authentication failed for user \"{user}\"")
    };
    if accepted.is_empty() {
        msg.push_str("; the server offers no other method");
    } else if partial {
        msg.push_str(&format!("; it asks for: {accepted}"));
    } else {
        msg.push_str(&format!("; the server accepts: {accepted}"));
    }
    AppError::Auth(msg)
}

fn ended_during_auth(method: &str, reason: &str) -> AppError {
    AppError::Tunnel(format!(
        "the SSH connection ended during {method} authentication: {reason}"
    ))
}

/// Why the SSH session ended, if it did. russh answers an authentication request on a session
/// that ended meanwhile (server gone, connection reset) with an ordinary failure listing no
/// methods, so such a failure is only blamed on the credentials when the session is still alive.
async fn session_end(
    session: &client::Handle<Client>,
    state: &watch::Sender<TunnelState>,
) -> Option<String> {
    let mut status = state.subscribe();
    let ended = tokio::time::timeout(SESSION_END_GRACE, status.wait_for(|s| !s.is_up())).await;
    match ended {
        Ok(Ok(s)) => Some(
            s.message
                .clone()
                .unwrap_or_else(|| "the SSH session ended".into()),
        ),
        _ if session.is_closed() => Some("the SSH session ended".into()),
        _ => None,
    }
}

async fn authenticate(
    session: &mut client::Handle<Client>,
    user: &str,
    credential: Credential,
    state: &watch::Sender<TunnelState>,
) -> Result<(), AppError> {
    let (method, result) = match credential {
        Credential::Password(password) => {
            ("password", password_auth(session, user, &password).await?)
        }
        Credential::Key(key) => {
            let hash = rsa_hash(session, &key.public_key().clone()).await?;
            let result = session
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(key, hash))
                .await
                .map_err(ssh_err)?;
            ("public key", result)
        }
        Credential::Agent(socket) => ("ssh-agent", agent_auth(session, user, socket, state).await?),
    };
    match result {
        AuthResult::Success => Ok(()),
        AuthResult::Failure {
            remaining_methods,
            partial_success,
        } => {
            if !partial_success
                && method_names(&remaining_methods).is_empty()
                && let Some(reason) = session_end(session, state).await
            {
                return Err(ended_during_auth(method, &reason));
            }
            Err(auth_failure(
                method,
                user,
                &remaining_methods,
                partial_success,
            ))
        }
    }
}

/// RSA keys need the signature hash the server supports; other key types ignore it.
async fn rsa_hash(
    session: &client::Handle<Client>,
    key: &PublicKey,
) -> Result<Option<russh::keys::HashAlg>, AppError> {
    if !key.algorithm().is_rsa() {
        return Ok(None);
    }
    Ok(session
        .best_supported_rsa_hash()
        .await
        .map_err(ssh_err)?
        .flatten())
}

/// Password auth; servers that only offer keyboard-interactive get the password as the answer
/// to their prompts.
async fn password_auth(
    session: &mut client::Handle<Client>,
    user: &str,
    password: &str,
) -> Result<AuthResult, AppError> {
    let result = session
        .authenticate_password(user, password)
        .await
        .map_err(ssh_err)?;
    let AuthResult::Failure {
        remaining_methods,
        partial_success,
    } = &result
    else {
        return Ok(result);
    };
    // After a partial success the password was *accepted* and keyboard-interactive is a second
    // factor (`AuthenticationMethods password,keyboard-interactive`): its prompt wants a one-time
    // code, and answering with the password would hand it to the second-factor backend and burn
    // an attempt there. Only a server that never offered `password` gets the fallback.
    if *partial_success
        || remaining_methods.contains(&MethodKind::Password)
        || !remaining_methods.contains(&MethodKind::KeyboardInteractive)
    {
        return Ok(result);
    }
    let mut reply = session
        .authenticate_keyboard_interactive_start(user, None)
        .await
        .map_err(ssh_err)?;
    for _ in 0..5 {
        match reply {
            client::KeyboardInteractiveAuthResponse::Success => return Ok(AuthResult::Success),
            client::KeyboardInteractiveAuthResponse::Failure {
                remaining_methods,
                partial_success,
            } => {
                return Ok(AuthResult::Failure {
                    remaining_methods,
                    partial_success,
                });
            }
            client::KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts
                    .iter()
                    .map(|p| {
                        if p.echo {
                            String::new()
                        } else {
                            password.to_string()
                        }
                    })
                    .collect();
                reply = session
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(ssh_err)?;
            }
        }
    }
    Ok(result)
}

/// The agent did not produce a signature (refused, e.g. a dismissed confirmation prompt with
/// `ssh-add -c`, 1Password or Secretive, or failed). `last` is the server's last answer, if any.
#[cfg(unix)]
fn agent_sign_failure(
    user: &str,
    comment: &str,
    err: &dyn Display,
    last: Option<&AuthResult>,
) -> AppError {
    let key = if comment.trim().is_empty() {
        "its key".to_string()
    } else {
        format!("the key \"{}\"", comment.trim())
    };
    let mut msg = format!(
        "ssh: the SSH agent did not sign with {key} for user \"{user}\" ({err}); if the agent \
         asks for confirmation, approve the request"
    );
    if let Some(AuthResult::Failure {
        remaining_methods, ..
    }) = last
    {
        let accepted = method_names(remaining_methods);
        if !accepted.is_empty() {
            msg.push_str(&format!("; the server accepts: {accepted}"));
        }
    }
    AppError::Auth(msg)
}

#[cfg(unix)]
async fn agent_auth(
    session: &mut client::Handle<Client>,
    user: &str,
    socket: Option<PathBuf>,
    state: &watch::Sender<TunnelState>,
) -> Result<AuthResult, AppError> {
    use russh::AgentAuthError;
    use russh::keys::agent::client::AgentClient;

    let connected = match &socket {
        Some(path) => AgentClient::connect_uds(path).await,
        None => AgentClient::connect_env().await,
    };
    let mut agent = connected.map_err(|e| {
        AppError::Auth(format!(
            "ssh: cannot reach the SSH agent ({e}); start ssh-agent and add your key with ssh-add"
        ))
    })?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| AppError::Auth(format!("ssh: the SSH agent did not list its keys: {e}")))?;
    if identities.is_empty() {
        return Err(AppError::Auth(
            "ssh: the SSH agent has no keys; add one with ssh-add".into(),
        ));
    }
    // The server's last real answer (a session that ends meanwhile also looks like a failure
    // without methods; `authenticate` tells the two apart).
    let mut last: Option<AuthResult> = None;
    for identity in identities {
        let key = identity.public_key().into_owned();
        let hash = rsa_hash(session, &key).await?;
        match session
            .authenticate_publickey_with(user, key, hash, &mut agent)
            .await
        {
            Ok(AuthResult::Success) => return Ok(AuthResult::Success),
            Ok(failure) => {
                let more_keys_welcome = matches!(
                    &failure,
                    AuthResult::Failure { remaining_methods, .. }
                        if remaining_methods.contains(&MethodKind::PublicKey)
                );
                last = Some(failure);
                if !more_keys_welcome {
                    break;
                }
            }
            // The session could not be reached: it ended.
            Err(AgentAuthError::Send(_)) => {
                let reason = session_end(session, state)
                    .await
                    .unwrap_or_else(|| "the SSH session ended".into());
                return Err(ended_during_auth("ssh-agent", &reason));
            }
            // No signature. russh's session now waits for exactly this signature and drops every
            // other request, so no further key can be tried on this session: stop and say why.
            Err(AgentAuthError::Key(e)) => {
                log::warn!("ssh-agent could not sign with {}: {e}", identity.comment());
                return Err(agent_sign_failure(
                    user,
                    identity.comment(),
                    &e,
                    last.as_ref(),
                ));
            }
        }
    }
    Ok(last.unwrap_or(AuthResult::Failure {
        remaining_methods: MethodSet::empty(),
        partial_success: false,
    }))
}

#[cfg(not(unix))]
async fn agent_auth(
    _session: &mut client::Handle<Client>,
    _user: &str,
    _socket: Option<PathBuf>,
    _state: &watch::Sender<TunnelState>,
) -> Result<AuthResult, AppError> {
    Err(AppError::Unsupported(
        "SSH agent authentication is only supported on macOS and Linux; use a key file".into(),
    ))
}

/// Expands a leading `~` / `~/` to the home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    let home = || std::env::var_os("HOME").filter(|h| !h.is_empty());
    if path == "~" {
        if let Some(h) = home() {
            return PathBuf::from(h);
        }
    } else if let Some(rest) = path.strip_prefix("~/")
        && let Some(h) = home()
    {
        return PathBuf::from(h).join(rest);
    }
    PathBuf::from(path)
}

/// Loads an OpenSSH / PEM / PuTTY private key, decrypting it with `passphrase` when needed.
pub fn load_private_key(
    path: Option<&str>,
    passphrase: Option<&str>,
) -> Result<Arc<PrivateKey>, AppError> {
    let raw = path
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| {
            AppError::InvalidInput(
                "an SSH private key file is required for key authentication".into(),
            )
        })?;
    let path = expand_tilde(raw);
    let passphrase = passphrase.filter(|p| !p.is_empty());
    let text = std::fs::read_to_string(&path).map_err(|e| {
        AppError::Tunnel(format!(
            "cannot read the SSH private key {}: {e}",
            path.display()
        ))
    })?;
    // Without the passphrase first: an unencrypted key must load even when a passphrase is saved
    // (left over from an earlier key, or typed by mistake). Given one, russh tries to decrypt
    // PEM / PKCS#8 keys with it, which fails for an unencrypted key.
    let plain_error = match russh::keys::decode_secret_key(&text, None) {
        Ok(key) => return Ok(Arc::new(key)),
        Err(e) => e,
    };
    let encrypted =
        matches!(plain_error, russh::keys::Error::KeyIsEncrypted) || looks_encrypted(&text);
    let cannot_load = || {
        AppError::Tunnel(format!(
            "cannot load the SSH private key {}: {plain_error}",
            path.display()
        ))
    };
    match passphrase {
        None if encrypted => Err(AppError::Auth(format!(
            "ssh: the private key {} is encrypted; enter its passphrase",
            path.display()
        ))),
        None => Err(cannot_load()),
        Some(passphrase) => match russh::keys::decode_secret_key(&text, Some(passphrase)) {
            Ok(key) => Ok(Arc::new(key)),
            Err(e) if encrypted => Err(AppError::Auth(format!(
                "ssh: cannot decrypt the private key {} (wrong passphrase?): {e}",
                path.display()
            ))),
            Err(_) => Err(cannot_load()),
        },
    }
}

/// Whether a key file is passphrase-protected in a format where russh does not report it as such
/// without a passphrase (PKCS#8, legacy PEM, PuTTY).
fn looks_encrypted(text: &str) -> bool {
    text.lines().map(str::trim).any(|line| {
        line == "-----BEGIN ENCRYPTED PRIVATE KEY-----"
            || (line.starts_with("Proc-Type:") && line.contains("ENCRYPTED"))
            || line
                .strip_prefix("Encryption:")
                .is_some_and(|v| v.trim() != "none")
    })
}

// ----- forwarding -------------------------------------------------------------------------------

/// What every accepted connection is forwarded to.
#[derive(Clone)]
struct Forwarder {
    session: Arc<client::Handle<Client>>,
    target_host: Arc<str>,
    target_port: u16,
    forward_error: LastForwardFailure,
}

/// Accepts local connections until `cancel` (close / drop). While `forwarding` is live each one
/// is forwarded; once the SSH session has ended they are closed at once, but the listener stays
/// bound so the port cannot be taken over while the connection's drivers may still dial it.
async fn accept_loop(
    listener: TcpListener,
    forwarder: Forwarder,
    cancel: CancellationToken,
    forwarding: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    if forwarding.is_cancelled() {
                        log::debug!("ssh tunnel is down; refusing the local connection from {peer}");
                        drop(stream);
                        continue;
                    }
                    tokio::spawn(forward(
                        stream,
                        peer,
                        forwarder.clone(),
                        forwarding.child_token(),
                    ));
                }
                Err(e) => {
                    // e.g. too many open files: back off instead of spinning.
                    log::warn!("ssh tunnel accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
    // Dropping the listener here closes the port.
}

fn describe_channel_error(e: &russh::Error) -> String {
    match e {
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed) => {
            "the database host refused the connection or is unreachable from the SSH server".into()
        }
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited) => {
            "port forwarding is not allowed by the SSH server (AllowTcpForwarding)".into()
        }
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::Other { reason, .. }) => {
            reason.clone()
        }
        other => describe_ssh_error(other),
    }
}

async fn forward(
    mut local: TcpStream,
    peer: SocketAddr,
    forwarder: Forwarder,
    cancel: CancellationToken,
) {
    let Forwarder {
        session,
        target_host,
        target_port,
        forward_error,
    } = forwarder;
    let _ = local.set_nodelay(true);
    let opened = tokio::select! {
        _ = cancel.cancelled() => return,
        ch = session.channel_open_direct_tcpip(
            target_host.to_string(),
            u32::from(target_port),
            peer.ip().to_string(),
            u32::from(peer.port()),
        ) => ch,
    };
    let channel = match opened {
        Ok(ch) => ch,
        Err(e) => {
            let msg = format!(
                "the SSH server could not open a connection to {target_host}:{target_port}: {}",
                describe_channel_error(&e)
            );
            log::warn!("{msg}");
            *lock(&forward_error) = Some(ForwardFailure {
                at: Instant::now(),
                message: msg,
            });
            return;
        }
    };
    let mut remote = channel.into_stream();
    tokio::select! {
        _ = cancel.cancelled() => {}
        res = tokio::io::copy_bidirectional(&mut local, &mut remote) => {
            if let Err(e) = res {
                log::debug!("ssh tunnel stream from {peer} ended: {e}");
            }
        }
    }
}

/// Disconnects the SSH session when the handle is closed/dropped, and stops forwarding (but
/// keeps the port reserved) when the session dies on its own.
async fn supervise(
    session: Arc<client::Handle<Client>>,
    mut state: watch::Receiver<TunnelState>,
    cancel: CancellationToken,
    forwarding: CancellationToken,
) {
    tokio::select! {
        _ = cancel.cancelled() => {}
        _ = state.wait_for(|s| !s.is_up()) => forwarding.cancel(),
    }
    let _ = session
        .disconnect(Disconnect::ByApplication, "tunnel closed", "en")
        .await;
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_home() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        assert_eq!(
            expand_tilde("~/.ssh/id_ed25519"),
            PathBuf::from(&home).join(".ssh/id_ed25519")
        );
        assert_eq!(expand_tilde("~"), PathBuf::from(&home));
        assert_eq!(expand_tilde("/abs/key"), PathBuf::from("/abs/key"));
        assert_eq!(expand_tilde("~other/key"), PathBuf::from("~other/key"));
    }

    #[test]
    fn missing_key_path_is_invalid_input() {
        assert!(matches!(
            load_private_key(None, None),
            Err(AppError::InvalidInput(_))
        ));
        assert!(matches!(
            load_private_key(Some("/nonexistent/tabler_key"), None),
            Err(AppError::Tunnel(_))
        ));
    }

    #[test]
    fn auth_failure_lists_accepted_methods() {
        let methods = MethodSet::from(&[MethodKind::PublicKey, MethodKind::Password][..]);
        let err = auth_failure("password", "deploy", &methods, false);
        let AppError::Auth(msg) = err else {
            panic!("expected Auth");
        };
        assert!(
            msg.starts_with("ssh: password authentication failed"),
            "{msg}"
        );
        assert!(msg.contains("publickey, password"), "{msg}");
    }

    #[test]
    fn partial_success_is_reported_as_a_second_factor() {
        let methods = MethodSet::from(&[MethodKind::KeyboardInteractive][..]);
        let AppError::Auth(msg) = auth_failure("password", "deploy", &methods, true) else {
            panic!("expected Auth");
        };
        assert!(msg.contains("succeeded"), "{msg}");
        assert!(msg.contains("second factor"), "{msg}");
        assert!(msg.contains("keyboard-interactive"), "{msg}");
        assert!(!msg.contains("failed"), "{msg}");
    }

    #[test]
    fn detects_encrypted_key_formats() {
        assert!(looks_encrypted(
            "-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIF\n-----END ENCRYPTED PRIVATE KEY-----\n"
        ));
        assert!(looks_encrypted(
            "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n"
        ));
        assert!(looks_encrypted(
            "PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: aes256-cbc\n"
        ));
        assert!(!looks_encrypted(
            "PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: none\n"
        ));
        assert!(!looks_encrypted(
            "-----BEGIN PRIVATE KEY-----\nMC4C\n-----END PRIVATE KEY-----\n"
        ));
        assert!(!looks_encrypted(
            "-----BEGIN EC PRIVATE KEY-----\nMHcC\n-----END EC PRIVATE KEY-----\n"
        ));
    }

    #[tokio::test]
    async fn ephemeral_and_pinned_local_ports() {
        let first = bind_local(0).await.unwrap();
        let port = first.local_addr().unwrap().port();
        assert_ne!(port, 0);
        // A pinned port that another listener holds on the same address is refused...
        let err = bind_local(port).await.unwrap_err();
        assert!(
            matches!(&err, AppError::Tunnel(m) if m.contains("cannot listen") && m.contains("in use")),
            "{err:?}"
        );
        // ...and free again once that listener is gone.
        drop(first);
        let again = bind_local(port).await.unwrap();
        assert_eq!(again.local_addr().unwrap().port(), port);
    }

    #[test]
    fn status_event_is_camel_case() {
        let ev = TunnelStatusEvent {
            connection_id: Uuid::nil(),
            status: TunnelStatus::Error,
            local_port: Some(5555),
            message: Some("x".into()),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["status"], "error");
        assert_eq!(json["localPort"], 5555);
        assert!(json.get("connectionId").is_some());
    }

    #[tokio::test]
    async fn unreachable_ssh_server_is_a_tunnel_error() {
        // Grab a free port, then release it so nothing listens there.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let cfg = SshConfig {
            host: "127.0.0.1".into(),
            port,
            user: "tabler".into(),
            auth: SshAuth::Password,
            key_path: None,
            local_port: None,
            keepalive_secs: None,
        };
        let auth = TunnelAuth {
            password: Some("x".into()),
            ..TunnelAuth::default()
        };
        let err = open_tunnel(&cfg, auth, "127.0.0.1".into(), 5432, &MemoryHostKeys::new())
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Tunnel(_)), "{err:?}");
    }
}
