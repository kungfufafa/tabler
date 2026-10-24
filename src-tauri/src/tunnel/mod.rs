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
//! so on macOS/BSD (and, through a probe of the wildcard address, on Windows) a pinned port
//! cannot take loopback traffic away from another program that listens on the wildcard address
//! of that port. And it only serves this app: a connection from any other program on the Mac is
//! refused ([`peer`]).
//!
//! `~/.ssh/config` decides how the SSH server is reached ([`transport`]): directly, through
//! `ProxyJump` hosts (one SSH session per hop, each running over a `direct-tcpip` channel of the
//! previous one) or through a `ProxyCommand`'s stdio; its `IdentityAgent` picks the agent socket
//! (1Password, Secretive…). [`SshSession`] is the same signed-in session without a listener,
//! for SFTP (SQLite over SSH).

pub mod debug_log;
mod hostkeys;
pub mod peer;
pub mod prompts;
pub mod sqlite_remote;
pub mod ssh_config;
pub mod transport;

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
pub use prompts::{
    ClockedPrompts, ForwardedPrompts, KeyboardInteractive, PendingPrompts, PromptClock,
    PromptEvent, PromptRound, SshPromptDone, SshPromptField, SshPromptKind, SshPromptPurpose,
    SshPromptRequest, timeout_excluding_prompts,
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

/// SSH secrets for one tunnel: the password (password auth) or the key passphrase (key auth),
/// and who answers the server's keyboard-interactive (2FA) questions.
#[derive(Clone, Default)]
pub struct TunnelAuth {
    pub password: Option<String>,
    pub passphrase: Option<String>,
    /// SSH agent socket for agent auth; `None` uses `IdentityAgent` of `~/.ssh/config`, else
    /// `SSH_AUTH_SOCK`.
    pub agent_socket: Option<PathBuf>,
    /// Answers keyboard-interactive questions (the user, through the renderer). `None`: only a
    /// password question can be answered (with `password`).
    pub prompts: Option<Arc<dyn KeyboardInteractive>>,
    /// The SSH password is asked on every connect ("Ask every time"): without `password`, the
    /// user types it in the SSH prompt dialog (through `prompts`).
    pub ask_password: bool,
}

impl std::fmt::Debug for TunnelAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelAuth")
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("passphrase", &self.passphrase.as_ref().map(|_| "***"))
            .field("agent_socket", &self.agent_socket)
            .field("prompts", &self.prompts.is_some())
            .field("ask_password", &self.ask_password)
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

/// Where a tunnel's connections go, as seen from the SSH server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardTarget {
    /// A host and port (`direct-tcpip` channels).
    Tcp { host: String, port: u16 },
    /// A Unix socket on the SSH server (`direct-streamlocal@openssh.com` channels): MySQL "Use
    /// socket" over SSH. The first path that opens is used from then on (several: the usual places
    /// when no path was typed).
    Socket(Vec<String>),
}

impl std::fmt::Display for ForwardTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForwardTarget::Tcp { host, port } => write!(f, "{host}:{port}"),
            ForwardTarget::Socket(paths) => write!(f, "unix:{}", paths.join("|")),
        }
    }
}

/// The listener and SSH session of a tunnel, shared by its leases ([`TunnelHandle`]).
struct TunnelShared {
    target: String,
    /// Cancelled when the last lease goes: stops the listener, forwarded connections and the
    /// session.
    cancel: CancellationToken,
    state: Arc<watch::Sender<TunnelState>>,
    session: Arc<client::Handle<Client>>,
    /// The `ProxyJump` sessions the tunnel's session runs through (first hop first).
    jumps: Jumps,
    listener_task: Mutex<Option<JoinHandle<()>>>,
    forward_error: LastForwardFailure,
    /// Leases not closed or dropped yet.
    leases: Mutex<usize>,
}

/// One lease of a tunnel: [`TunnelHandle::share`] hands out more (another connection of the same
/// profile reuses the tunnel, e.g. a second workspace while the profile pins its local port). The
/// tunnel stays up while any lease does; closing (or dropping) the last one shuts the listener and
/// SSH session down.
pub struct TunnelHandle {
    pub local_port: u16,
    shared: Arc<TunnelShared>,
    /// This lease was given back (closed or dropped).
    released: std::sync::atomic::AtomicBool,
}

/// The jump-host sessions of a connection, first hop first.
type Jumps = Arc<Vec<client::Handle<Client>>>;

/// Disconnects a session and then its jump hosts, last hop first.
async fn disconnect_all(session: &client::Handle<Client>, jumps: &[client::Handle<Client>]) {
    let _ = session
        .disconnect(Disconnect::ByApplication, "tunnel closed", "en")
        .await;
    for jump in jumps.iter().rev() {
        let _ = jump
            .disconnect(Disconnect::ByApplication, "tunnel closed", "en")
            .await;
    }
}

impl std::fmt::Debug for TunnelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelHandle")
            .field("local_port", &self.local_port)
            .field("target", &self.shared.target)
            .field("state", &*self.shared.state.borrow())
            .field("leases", &self.leases())
            .finish()
    }
}

/// Takes one lease of `count` back; true when it was the last one.
fn release_lease(released: &std::sync::atomic::AtomicBool, count: &Mutex<usize>) -> bool {
    if released.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    let mut n = lock(count);
    *n = n.saturating_sub(1);
    *n == 0
}

/// Adds a lease to `count` unless every lease was given back already.
fn add_lease(count: &Mutex<usize>) -> bool {
    let mut n = lock(count);
    if *n == 0 {
        return false;
    }
    *n += 1;
    true
}

impl TunnelHandle {
    /// The tunnel's first lease (`shared.leases` starts at 1).
    fn first_lease(local_port: u16, shared: TunnelShared) -> TunnelHandle {
        TunnelHandle {
            local_port,
            shared: Arc::new(shared),
            released: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Another lease of this tunnel while it is up (`None` once it is down or closed).
    pub fn share(&self) -> Option<TunnelHandle> {
        if !self.is_up() || !add_lease(&self.shared.leases) {
            return None;
        }
        Some(TunnelHandle {
            local_port: self.local_port,
            shared: self.shared.clone(),
            released: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Leases of the tunnel not given back yet (this one included while it is held).
    pub fn leases(&self) -> usize {
        *lock(&self.shared.leases)
    }

    /// Whether both handles lease the same tunnel.
    pub fn same_tunnel(&self, other: &TunnelHandle) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// `host_override` for `drivers::ConnectTarget`.
    pub fn connect_addr(&self) -> (String, u16) {
        (LOCAL_HOST.to_string(), self.local_port)
    }

    pub fn state(&self) -> TunnelState {
        self.shared.state.borrow().clone()
    }

    pub fn is_up(&self) -> bool {
        self.shared.state.borrow().is_up()
    }

    /// Watch that leaves `Up` once when the tunnel goes down (closed, server gone, keepalive
    /// timeout). It never comes back up: a new tunnel is a new handle.
    pub fn subscribe(&self) -> watch::Receiver<TunnelState> {
        self.shared.state.subscribe()
    }

    /// The last reason a forwarded connection could not be opened, if that happened at or after
    /// `since` (taken, so it is reported once; an older failure is discarded).
    pub fn take_forward_error(&self, since: Instant) -> Option<String> {
        let failure = lock(&self.shared.forward_error).take()?;
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

    /// Gives this lease back. The last one stops the listener (new connections are refused and
    /// the port is free once this returns), closes forwarded connections and disconnects the SSH
    /// session; with other leases left the tunnel stays up for them. Idempotent.
    pub async fn close(&self) {
        if !release_lease(&self.released, &self.shared.leases) {
            return;
        }
        let shared = &self.shared;
        mark_down(&shared.state, TunnelStatus::Down, "tunnel closed");
        shared.cancel.cancel();
        let task = lock(&shared.listener_task).take();
        if let Some(task) = task {
            let _ = task.await;
        }
        disconnect_all(&shared.session, &shared.jumps).await;
    }
}

impl Drop for TunnelHandle {
    fn drop(&mut self) {
        if release_lease(&self.released, &self.shared.leases) {
            mark_down(&self.shared.state, TunnelStatus::Down, "tunnel closed");
            // The listener loop and the supervisor see the token and finish the shutdown.
            self.shared.cancel.cancel();
        }
    }
}

/// Where and as whom the tunnel connects, once `~/.ssh/config` filled the form's gaps.
#[derive(Debug, PartialEq, Eq)]
struct ResolvedSsh {
    host: String,
    port: u16,
    user: String,
    /// The private key for key authentication (None when none could be found).
    key_path: Option<String>,
}

/// Merges the form's SSH settings with the `~/.ssh/config` entry of its host: a typed user, port
/// (non-zero) or key wins; then the config's `User` / `Port` / `IdentityFile`; then OpenSSH's
/// defaults (the local user, port 22, the first existing `~/.ssh/id_*` key). The config's
/// `HostName` replaces an alias.
fn resolve_ssh(
    cfg: &SshConfig,
    alias: &str,
    config: &ssh_config::HostConfig,
) -> Result<ResolvedSsh, AppError> {
    let host = config
        .host_name
        .clone()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| alias.to_string());
    let port = match cfg.port {
        0 => config.port.unwrap_or(22),
        p => p,
    };
    let user = Some(cfg.user.trim().to_string())
        .filter(|u| !u.is_empty())
        .or_else(|| config.user.clone())
        .or_else(ssh_config::default_user)
        .ok_or_else(|| {
            AppError::InvalidInput(
                "SSH user is required (none typed, none in ~/.ssh/config for this host)".into(),
            )
        })?;
    let key_path = match cfg.auth {
        SshAuth::Key => cfg
            .key_path
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .or_else(|| {
                config
                    .identity_files
                    .iter()
                    .map(String::as_str)
                    .chain(ssh_config::DEFAULT_IDENTITIES)
                    .find(|p| expand_tilde(p).is_file())
                    .map(str::to_string)
            }),
        _ => None,
    };
    Ok(ResolvedSsh {
        host,
        port,
        user,
        key_path,
    })
}

/// What a connect needs once the form and `~/.ssh/config` are merged, before any network I/O.
struct Plan {
    alias: String,
    resolved: ResolvedSsh,
    credential: Credential,
    /// `ProxyJump` / `ProxyCommand` of the alias.
    proxy: Option<ssh_config::Proxy>,
    /// For jump hosts: the agent and the passphrase the profile's key auth uses.
    agent_socket: Option<PathBuf>,
    passphrase: Option<String>,
    prompts: Option<Arc<dyn KeyboardInteractive>>,
    client: Arc<client::Config>,
}

/// The agent socket of `IdentityAgent`, `~` expanded (`None`: not set, `none` or an unset
/// variable).
fn configured_agent(config: &ssh_config::HostConfig) -> Option<PathBuf> {
    config
        .identity_agent
        .as_ref()
        .and_then(ssh_config::IdentityAgent::socket)
        .map(|p| expand_tilde(&p))
}

/// The public key in a key file (an `IdentityFile` that names an agent's key by its `.pub`).
fn read_public_key(path: &str) -> Option<PublicKey> {
    let text = std::fs::read_to_string(expand_tilde(path)).ok()?;
    russh::keys::ssh_key::PublicKey::from_openssh(text.trim()).ok()
}

/// Merges the form with `~/.ssh/config` and loads the credential: local problems (missing key,
/// wrong passphrase) fail here, before any network I/O.
fn plan(cfg: &SshConfig, auth: TunnelAuth) -> Result<Plan, AppError> {
    let alias = cfg.host.trim().to_string();
    if alias.is_empty() {
        return Err(AppError::InvalidInput("SSH host is required".into()));
    }
    // What the form leaves empty comes from ~/.ssh/config (the form wins, as in TablePlus); the
    // form's user is what `Match user` blocks of the config are tested with.
    let config = ssh_config::for_host_as(&alias, Some(cfg.user.trim()));
    let resolved = resolve_ssh(cfg, &alias, &config)?;
    let identity_agent = configured_agent(&config);
    if cfg.auth == SshAuth::Agent
        && auth.agent_socket.is_none()
        && config.identity_agent == Some(ssh_config::IdentityAgent::Disabled)
    {
        return Err(AppError::Auth(
            "ssh: ~/.ssh/config sets \u{201c}IdentityAgent none\u{201d} for this host; choose \
             another authentication method"
                .into(),
        ));
    }
    let agent_socket = auth.agent_socket.clone().or(identity_agent.clone());
    let credential = match cfg.auth {
        SshAuth::Password => match auth.password.clone() {
            Some(password) => Credential::Password(password),
            // "Ask every time": the user types the password in the SSH prompt dialog.
            None if auth.ask_password && auth.prompts.is_some() => Credential::Ask,
            // Nothing saved: the server's own questions (its password prompt) go to the user.
            None if auth.prompts.is_some() => Credential::Prompted,
            None => return Err(no_saved_password()),
        },
        SshAuth::Key => {
            let path = resolved.key_path.as_deref();
            // `IdentityFile ~/.ssh/key.pub` + `IdentityAgent` (1Password, Secretive): the key
            // lives in the agent, the file only says which one.
            let public = path.and_then(read_public_key);
            let agent = identity_agent
                .clone()
                .or_else(|| std::env::var_os("SSH_AUTH_SOCK").map(PathBuf::from))
                .filter(|_| config.identity_agent != Some(ssh_config::IdentityAgent::Disabled));
            match (path, public, agent) {
                (Some(_), Some(key), Some(socket)) => Credential::Agent {
                    socket: Some(socket),
                    prefer: Some(key),
                },
                // No key file anywhere, but the config names an agent.
                (None, _, Some(socket)) if identity_agent.is_some() => Credential::Agent {
                    socket: Some(socket),
                    prefer: None,
                },
                _ => Credential::Key(load_private_key(path, auth.passphrase.as_deref())?),
            }
        }
        SshAuth::Agent => Credential::Agent {
            socket: agent_socket.clone(),
            prefer: None,
        },
    };
    let keepalive = cfg
        .keepalive_secs
        .filter(|s| *s > 0)
        .map(u64::from)
        .unwrap_or(DEFAULT_KEEPALIVE_SECS);
    let client = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(keepalive)),
        keepalive_max: KEEPALIVE_MAX,
        inactivity_timeout: None,
        nodelay: true,
        ..Default::default()
    });
    Ok(Plan {
        alias,
        resolved,
        credential,
        proxy: config.proxy,
        agent_socket,
        passphrase: auth.passphrase,
        prompts: auth.prompts,
        client,
    })
}

/// A signed-in SSH session, with the jump hosts it runs through.
struct Connected {
    session: client::Handle<Client>,
    jumps: Vec<client::Handle<Client>>,
    state: Arc<watch::Sender<TunnelState>>,
    /// Shuts the transport down if the connect is given up before the caller disarms it.
    abort: transport::AbortOnDrop,
}

/// `host:port` for messages.
fn endpoint(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Connects (directly, through the jump hosts or the proxy command), checks the host key(s)
/// against `trust` and signs in.
async fn connect(plan: Plan, trust: &dyn HostKeyTrust) -> Result<Connected, AppError> {
    let ResolvedSsh {
        host, port, user, ..
    } = &plan.resolved;
    let (host, port, user) = (host.clone(), *port, user.clone());
    let state = Arc::new(watch::channel(TunnelState::up()).0);
    let mut abort = transport::AbortOnDrop::default();
    let mut jumps = Vec::new();
    let mut proxy_stderr = None;
    let stream = match &plan.proxy {
        None => {
            let (stream, guard) = transport::tcp(&host, port, plan.client.nodelay)
                .await
                .map_err(|e| {
                    AppError::Tunnel(format!(
                        "cannot connect to the SSH server {}: {e}",
                        endpoint(&host, port)
                    ))
                })?;
            abort.extend(guard);
            stream
        }
        Some(ssh_config::Proxy::Command(raw)) => {
            let tokens = ssh_config::ProxyTokens {
                host: &host,
                port,
                user: &user,
                alias: &plan.alias,
            };
            // The command runs under `/bin/sh` before the host key and the sign-in are checked:
            // a host, user or alias from an imported profile or a link must not reach the shell
            // as anything but a plain name.
            ssh_config::check_proxy_command(raw, &tokens).map_err(AppError::Tunnel)?;
            let command = ssh_config::expand_proxy_command(raw, &tokens);
            log::debug!("ssh: ProxyCommand {command}");
            let (stream, guard, tail) = transport::command(&command).map_err(|e| {
                AppError::Tunnel(format!(
                    "cannot run the ProxyCommand \u{201c}{command}\u{201d}: {e}"
                ))
            })?;
            abort.extend(guard);
            proxy_stderr = Some(tail);
            stream
        }
        Some(ssh_config::Proxy::Jump(hops)) => {
            let (stream, guard, sessions) = jump_chain(&plan, hops, &host, port, trust).await?;
            abort.extend(guard);
            jumps = sessions;
            stream
        }
    };
    let what = match &plan.proxy {
        None => format!("the SSH server {}", endpoint(&host, port)),
        Some(ssh_config::Proxy::Command(_)) => format!(
            "the SSH server {} through its ProxyCommand",
            endpoint(&host, port)
        ),
        Some(ssh_config::Proxy::Jump(_)) => format!(
            "the SSH server {} through the jump host",
            endpoint(&host, port)
        ),
    };
    let (check_tx, check_rx) = oneshot::channel();
    let handler = Client {
        host_key_check: Some(check_tx),
        accepted_key: None,
        state: state.clone(),
    };
    let handshake = handshake(
        plan.client.clone(),
        stream,
        &what,
        (&host, port),
        handler,
        check_rx,
        trust,
    )
    .await;
    let mut session = match handshake {
        Ok(session) => session,
        Err(AppError::Tunnel(msg)) => {
            // A proxy command that failed usually said why on stderr.
            let said = proxy_stderr.map(|t| t.text()).unwrap_or_default();
            return Err(AppError::Tunnel(if said.is_empty() {
                msg
            } else {
                format!("{msg} (ProxyCommand: {said})")
            }));
        }
        Err(e) => return Err(e),
    };
    let sign_in = SignIn {
        user: &user,
        host: &host,
        port,
        prompts: plan.prompts.as_deref(),
    };
    if let Err(e) = authenticate(&mut session, &sign_in, plan.credential, &state).await {
        disconnect_all(&session, &jumps).await;
        return Err(e);
    }
    Ok(Connected {
        session,
        jumps,
        state,
        abort,
    })
}

/// The settings of one `ProxyJump` hop, its own `~/.ssh/config` entry applied.
fn resolve_jump(hop: &ssh_config::JumpHost) -> (String, u16, String, ssh_config::HostConfig) {
    let config = ssh_config::for_host_as(&hop.host, hop.user.as_deref());
    let host = config
        .host_name
        .clone()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| hop.host.clone());
    let port = hop.port.or(config.port).unwrap_or(22);
    let user = hop
        .user
        .clone()
        .or_else(|| config.user.clone())
        .or_else(ssh_config::default_user)
        .unwrap_or_default();
    (host, port, user, config)
}

/// Opens the `ProxyJump` chain one hop at a time (the first over TCP, each next one through a
/// `direct-tcpip` channel of the previous session) and returns a channel through the last hop
/// to `target_host:target_port`, the transports' abort guard and the hop sessions.
async fn jump_chain(
    plan: &Plan,
    hops: &[ssh_config::JumpHost],
    target_host: &str,
    target_port: u16,
    trust: &dyn HostKeyTrust,
) -> Result<
    (
        transport::BoxedStream,
        transport::AbortOnDrop,
        Vec<client::Handle<Client>>,
    ),
    AppError,
> {
    let mut abort = transport::AbortOnDrop::default();
    let mut sessions: Vec<client::Handle<Client>> = Vec::new();
    for hop in hops {
        let (host, port, user, config) = resolve_jump(hop);
        let what = format!("the jump host {}", endpoint(&host, port));
        let stream = match sessions.last() {
            None => {
                let (stream, guard) = transport::tcp(&host, port, plan.client.nodelay)
                    .await
                    .map_err(|e| AppError::Tunnel(format!("cannot connect to {what}: {e}")))?;
                abort.extend(guard);
                stream
            }
            Some(previous) => open_channel(previous, &host, port, &what).await?,
        };
        let (check_tx, check_rx) = oneshot::channel();
        let handler = Client {
            host_key_check: Some(check_tx),
            accepted_key: None,
            // A hop's own state is not watched: when it ends, the sessions over it end too.
            state: Arc::new(watch::channel(TunnelState::up()).0),
        };
        let mut session = handshake(
            plan.client.clone(),
            stream,
            &what,
            (&host, port),
            handler,
            check_rx,
            trust,
        )
        .await?;
        log::debug!("ssh: signing in to {what} as {user}");
        if let Err(e) = authenticate_jump(&mut session, plan, &user, &host, port, &config).await {
            disconnect_all(&session, &sessions).await;
            return Err(e);
        }
        sessions.push(session);
    }
    let last = sessions
        .last()
        .ok_or_else(|| AppError::internal("ProxyJump without hosts"))?;
    let what = format!("the SSH server {}", endpoint(target_host, target_port));
    let stream = open_channel(last, target_host, target_port, &what).await?;
    Ok((stream, abort, sessions))
}

/// A `direct-tcpip` channel through `session` to `host:port`, as a stream.
async fn open_channel(
    session: &client::Handle<Client>,
    host: &str,
    port: u16,
    what: &str,
) -> Result<transport::BoxedStream, AppError> {
    let channel = session
        .channel_open_direct_tcpip(host.to_string(), u32::from(port), LOCAL_HOST, 0)
        .await
        .map_err(|e| {
            AppError::Tunnel(format!(
                "the jump host could not open a connection to {what}: {}",
                describe_channel_error(&e)
            ))
        })?;
    Ok(Box::new(channel.into_stream()))
}

/// Signs in to a jump host the way OpenSSH would without asking: its `IdentityFile` keys (and the
/// profile's key and the default `~/.ssh/id_*`), then the agent (its `IdentityAgent`, the
/// profile's agent, `SSH_AUTH_SOCK`), then keyboard-interactive answered by the user.
async fn authenticate_jump(
    session: &mut client::Handle<Client>,
    plan: &Plan,
    user: &str,
    host: &str,
    port: u16,
    config: &ssh_config::HostConfig,
) -> Result<(), AppError> {
    let mut tried = Vec::new();
    let profile_key = match &plan.credential {
        Credential::Key(key) => Some(key.clone()),
        _ => None,
    };
    let mut keys: Vec<Arc<PrivateKey>> = profile_key.into_iter().collect();
    for path in config
        .identity_files
        .iter()
        .map(String::as_str)
        .chain(ssh_config::DEFAULT_IDENTITIES)
    {
        if !expand_tilde(path).is_file() {
            continue;
        }
        // Keys that need a passphrase other than the profile's are skipped.
        if let Ok(key) = load_private_key(Some(path), plan.passphrase.as_deref())
            && !keys.iter().any(|k| k.public_key() == key.public_key())
        {
            keys.push(key);
        }
    }
    let mut last = None;
    for key in keys {
        let hash = rsa_hash(session, &key.public_key().clone()).await?;
        let result = session
            .authenticate_publickey(user, PrivateKeyWithHashAlg::new(key, hash))
            .await
            .map_err(ssh_err)?;
        if result.success() {
            return Ok(());
        }
        last = Some(result);
        tried.push("public key");
    }
    let agent = configured_agent(config)
        .or_else(|| plan.agent_socket.clone())
        .filter(|_| config.identity_agent != Some(ssh_config::IdentityAgent::Disabled));
    let has_env_agent = std::env::var_os("SSH_AUTH_SOCK").is_some();
    if agent.is_some() || has_env_agent {
        let state = watch::channel(TunnelState::up()).0;
        match agent_auth(session, user, agent, None, &state).await {
            Ok(AuthResult::Success) => return Ok(()),
            Ok(other) => last = Some(other),
            // An agent that is not running is no reason to give up on this hop.
            Err(e) => log::debug!("ssh: agent sign-in to the jump host failed: {e}"),
        }
        tried.push("ssh-agent");
    }
    let offers_keyboard = match &last {
        Some(AuthResult::Failure {
            remaining_methods, ..
        }) => remaining_methods.contains(&MethodKind::KeyboardInteractive),
        _ => true,
    };
    if offers_keyboard && plan.prompts.is_some() {
        let sign_in = SignIn {
            user,
            host,
            port,
            prompts: plan.prompts.as_deref(),
        };
        if let Some((_, result)) = keyboard_interactive(session, &sign_in, None).await? {
            if result.success() {
                return Ok(());
            }
            last = Some(result);
        }
        tried.push("keyboard-interactive");
    }
    let accepted = match &last {
        Some(AuthResult::Failure {
            remaining_methods, ..
        }) => method_names(remaining_methods),
        _ => String::new(),
    };
    let mut msg = format!(
        "ssh: cannot sign in to the jump host {} as \u{201c}{user}\u{201d}",
        endpoint(host, port)
    );
    if tried.is_empty() {
        msg.push_str(" (no key, agent or prompt to try)");
    } else {
        msg.push_str(&format!(" (tried {})", tried.join(", ")));
    }
    if !accepted.is_empty() {
        msg.push_str(&format!("; it accepts: {accepted}"));
    }
    msg.push_str("; add its key to ~/.ssh/config (IdentityFile) or to the SSH agent");
    Err(AppError::Auth(msg))
}

/// An SSH session signed in like a tunnel's, without a listener: the SFTP transfers of
/// SQLite over SSH run on it.
pub struct SshSession {
    session: client::Handle<Client>,
    jumps: Vec<client::Handle<Client>>,
    state: Arc<watch::Sender<TunnelState>>,
}

impl std::fmt::Debug for SshSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshSession")
            .field("jumps", &self.jumps.len())
            .field("state", &*self.state.borrow())
            .finish()
    }
}

impl SshSession {
    /// Connects and signs in as `cfg` says (host key checked against `trust`).
    pub async fn open(
        cfg: &SshConfig,
        auth: TunnelAuth,
        trust: &dyn HostKeyTrust,
    ) -> Result<SshSession, AppError> {
        let plan = plan(cfg, auth)?;
        let Connected {
            session,
            jumps,
            state,
            abort,
        } = connect(plan, trust).await?;
        abort.disarm();
        Ok(SshSession {
            session,
            jumps,
            state,
        })
    }

    pub fn is_up(&self) -> bool {
        self.state.borrow().is_up() && !self.session.is_closed()
    }

    /// Opens an SFTP subsystem channel.
    pub async fn sftp(&self) -> Result<russh_sftp::client::SftpSession, AppError> {
        let channel = self.session.channel_open_session().await.map_err(ssh_err)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(ssh_err)?;
        russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| AppError::Tunnel(format!("the SSH server did not start SFTP: {e}")))
    }

    pub async fn close(&self) {
        disconnect_all(&self.session, &self.jumps).await;
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
    let target = ForwardTarget::Tcp {
        host: target_host,
        port: target_port,
    };
    open_tunnel_to(cfg, auth, target, trust).await
}

/// [`open_tunnel`] towards a TCP host and port or a Unix socket on the SSH server.
pub async fn open_tunnel_to(
    cfg: &SshConfig,
    auth: TunnelAuth,
    target: ForwardTarget,
    trust: &dyn HostKeyTrust,
) -> Result<TunnelHandle, AppError> {
    if cfg.host.trim().is_empty() {
        return Err(AppError::InvalidInput("SSH host is required".into()));
    }
    match &target {
        ForwardTarget::Tcp { port: 0, .. } => {
            return Err(AppError::InvalidInput(
                "database port is required for an SSH tunnel".into(),
            ));
        }
        ForwardTarget::Socket(paths) if paths.iter().all(|p| p.trim().is_empty()) => {
            return Err(AppError::InvalidInput(
                "the socket path on the SSH server is required".into(),
            ));
        }
        _ => {}
    }
    // Local problems (missing key, wrong passphrase, port in use) fail before any network I/O.
    let plan = plan(cfg, auth)?;
    let listener = bind_local(cfg.local_port.unwrap_or(0)).await?;
    let local_port = listener.local_addr()?.port();
    let (user, ssh_host, ssh_port) = (
        plan.resolved.user.clone(),
        plan.resolved.host.clone(),
        plan.resolved.port,
    );
    let via = match &plan.proxy {
        None => String::new(),
        Some(ssh_config::Proxy::Command(_)) => " (ProxyCommand)".into(),
        Some(ssh_config::Proxy::Jump(hops)) => format!(
            " (via {})",
            hops.iter()
                .map(|h| h.host.as_str())
                .collect::<Vec<_>>()
                .join(" -> ")
        ),
    };
    // Until the tunnel is up, the transport is shut down if this future is dropped (the caller's
    // connect timeout) or returns an error.
    let Connected {
        session,
        jumps,
        state,
        abort,
    } = connect(plan, trust).await?;
    abort.disarm();
    log::info!(
        "ssh tunnel {LOCAL_HOST}:{local_port} -> {user}@{ssh_host}:{ssh_port}{via} -> {target}"
    );

    let session = Arc::new(session);
    let jumps: Jumps = Arc::new(jumps);
    let cancel = CancellationToken::new();
    // Cancelled when the SSH session ends (or with `cancel`): forwarding stops, the port does not.
    let forwarding = cancel.child_token();
    let forward_error = Arc::new(Mutex::new(None));
    let label = target.to_string();
    let listener_task = tokio::spawn(accept_loop(
        listener,
        Forwarder {
            session: session.clone(),
            target: Arc::new(target),
            socket_pick: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            forward_error: forward_error.clone(),
        },
        cancel.clone(),
        forwarding.clone(),
    ));
    tokio::spawn(supervise(
        session.clone(),
        jumps.clone(),
        state.subscribe(),
        cancel.clone(),
        forwarding,
    ));

    Ok(TunnelHandle::first_lease(
        local_port,
        TunnelShared {
            target: label,
            cancel,
            state,
            session,
            jumps,
            listener_task: Mutex::new(Some(listener_task)),
            forward_error,
            leases: Mutex::new(1),
        },
    ))
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
///
/// Windows binds `127.0.0.1:P` next to a wildcard listener even without the flag, so there the
/// wildcard address is checked first ([`wildcard_taken`]).
async fn bind_local(port: u16) -> Result<TcpListener, AppError> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let cannot_listen = |e: &dyn Display| AppError::Tunnel(format!("cannot listen on {addr}: {e}"));
    #[cfg(windows)]
    if port != 0 && wildcard_taken(port) {
        return Err(cannot_listen(
            &"the port is already in use (another program listens on it)",
        ));
    }
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

/// Whether another socket holds `port` on a wildcard address that also takes IPv4 loopback
/// connections: `0.0.0.0`, or a dual-stack `[::]`. Windows lets the tunnel bind `127.0.0.1:P`
/// next to it and then hands every loopback connection to the tunnel. A second bind of the same
/// wildcard address is refused, though, so that is the probe (bound, never listening: no firewall
/// prompt). Connecting would be slow: Windows reports a refused loopback connect after ~2 s.
#[cfg(windows)]
fn wildcard_taken(port: u16) -> bool {
    let taken =
        |bound: io::Result<()>| matches!(bound, Err(e) if e.kind() == io::ErrorKind::AddrInUse);
    let v4 = || TcpSocket::new_v4()?.bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)));
    let dual_stack = || {
        let socket = TcpSocket::new_v6()?;
        socket2::SockRef::from(&socket).set_only_v6(false)?;
        socket.bind(SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)))
    };
    taken(v4()) || taken(dual_stack())
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

/// Runs the SSH handshake over `stream` while answering the handler's host-key question with
/// `trust` for `key_for` (host, port). `what` names the server in errors ("the jump host …").
async fn handshake(
    config: Arc<client::Config>,
    stream: transport::BoxedStream,
    what: &str,
    key_for: (&str, u16),
    handler: Client,
    mut check_rx: oneshot::Receiver<HostKeyCheck>,
    trust: &dyn HostKeyTrust,
) -> Result<client::Handle<Client>, AppError> {
    let connect = client::connect_stream(config, stream, handler);
    tokio::pin!(connect);
    let mut checked = false;
    let result = loop {
        tokio::select! {
            res = &mut connect => break res,
            check = &mut check_rx, if !checked => {
                checked = true;
                if let Ok(HostKeyCheck { key, reply }) = check {
                    let _ = reply.send(verify_host_key(trust, key_for.0, key_for.1, &key));
                }
            }
        }
    };
    match result {
        Ok(session) => Ok(session),
        Err(ClientError::HostKey(e)) => Err(e),
        Err(ClientError::Ssh(e)) => Err(AppError::Tunnel(format!(
            "cannot connect to {what}: {}",
            describe_ssh_error(&e)
        ))),
    }
}

// ----- authentication ---------------------------------------------------------------------------

enum Credential {
    Password(String),
    /// Password authentication without a saved password: keyboard-interactive only, the user
    /// answering the server's questions (see `prompted_password`).
    Prompted,
    Key(Arc<PrivateKey>),
    /// Agent socket override (`None`: `SSH_AUTH_SOCK`); `prefer`: the key to offer first (the
    /// `.pub` an `IdentityFile` names).
    Agent {
        socket: Option<PathBuf>,
        prefer: Option<PublicKey>,
    },
    /// "Ask every time": the user types the SSH password when the server takes one.
    Ask,
}

const NO_SAVED_PASSWORD: &str = "ssh: no SSH password is saved for this connection";

fn no_saved_password() -> AppError {
    AppError::Auth(NO_SAVED_PASSWORD.into())
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

/// Who signs in where, and who answers the server's keyboard-interactive questions.
struct SignIn<'a> {
    user: &'a str,
    host: &'a str,
    port: u16,
    prompts: Option<&'a dyn KeyboardInteractive>,
}

/// Rounds of keyboard-interactive questions answered before giving up.
const MAX_PROMPT_ROUNDS: usize = 10;

/// Signs in with the profile's credential (key, agent or password), then, when the server asks
/// for keyboard-interactive (a 2FA code after a partial success, or instead of a method it does
/// not accept), answers its questions: TablePlus's order key → password → keyboard-interactive.
async fn authenticate(
    session: &mut client::Handle<Client>,
    sign_in: &SignIn<'_>,
    credential: Credential,
    state: &watch::Sender<TunnelState>,
) -> Result<(), AppError> {
    let user = sign_in.user;
    let mut password = None;
    let credential = match credential {
        Credential::Ask => match ask_password(session, sign_in, state).await? {
            Asked::Done => return Ok(()),
            Asked::Password(pw) => Credential::Password(pw),
            Asked::KeyboardInteractive => return prompted_password(session, sign_in, state).await,
        },
        other => other,
    };
    let (method, result) = match credential {
        Credential::Password(pw) => {
            let result = session
                .authenticate_password(user, pw.as_str())
                .await
                .map_err(ssh_err)?;
            password = Some(pw);
            ("password", result)
        }
        Credential::Key(key) => {
            let hash = rsa_hash(session, &key.public_key().clone()).await?;
            let result = session
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(key, hash))
                .await
                .map_err(ssh_err)?;
            ("public key", result)
        }
        Credential::Agent { socket, prefer } => (
            "ssh-agent",
            agent_auth(session, user, socket, prefer.as_ref(), state).await?,
        ),
        Credential::Prompted | Credential::Ask => {
            return prompted_password(session, sign_in, state).await;
        }
    };
    let AuthResult::Failure {
        remaining_methods,
        partial_success,
    } = result
    else {
        return Ok(());
    };
    if !partial_success
        && method_names(&remaining_methods).is_empty()
        && let Some(reason) = session_end(session, state).await
    {
        return Err(ended_during_auth(method, &reason));
    }
    // The password answers the server's password question only when the server never took it as
    // `password` (it does password authentication through keyboard-interactive / PAM). After a
    // partial success the password was *accepted* and the next question wants a second factor:
    // sending the password there would hand it to the 2FA backend and burn an attempt.
    let password =
        password.filter(|_| !partial_success && !remaining_methods.contains(&MethodKind::Password));
    if remaining_methods.contains(&MethodKind::KeyboardInteractive)
        && (sign_in.prompts.is_some() || password.is_some())
        && let Some((asked, result)) =
            keyboard_interactive(session, sign_in, password.as_deref()).await?
    {
        let method = if asked {
            "keyboard-interactive"
        } else {
            method
        };
        return match result {
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
        };
    }
    Err(auth_failure(
        method,
        user,
        &remaining_methods,
        partial_success,
    ))
}

/// What "Ask every time" found out before asking.
enum Asked {
    /// The server let the user in without a password (`none` authentication).
    Done,
    /// The user typed this password for the `password` method.
    Password(String),
    /// The server takes no `password`: its keyboard-interactive questions go to the user.
    KeyboardInteractive,
}

/// The SSH password of an "Ask every time" connection: asks which methods the server takes
/// (`none` authentication) and, when it takes a password, asks the user for it in the SSH prompt
/// dialog (an `SshPromptKind::Password` round). Cancelling gives the connect up.
async fn ask_password(
    session: &mut client::Handle<Client>,
    sign_in: &SignIn<'_>,
    state: &watch::Sender<TunnelState>,
) -> Result<Asked, AppError> {
    let Some(prompts) = sign_in.prompts else {
        return Err(no_saved_password());
    };
    let methods = match session
        .authenticate_none(sign_in.user)
        .await
        .map_err(ssh_err)?
    {
        AuthResult::Success => return Ok(Asked::Done),
        AuthResult::Failure {
            remaining_methods, ..
        } => remaining_methods,
    };
    if method_names(&methods).is_empty()
        && let Some(reason) = session_end(session, state).await
    {
        return Err(ended_during_auth("password", &reason));
    }
    if !methods.contains(&MethodKind::Password) {
        return Ok(Asked::KeyboardInteractive);
    }
    let answers = prompts
        .answer(PromptRound {
            user: sign_in.user.to_string(),
            host: sign_in.host.to_string(),
            port: sign_in.port,
            name: "SSH password".into(),
            instructions: String::new(),
            prompts: vec![SshPromptField {
                prompt: "Password:".into(),
                echo: false,
            }],
            kind: Some(SshPromptKind::Password),
        })
        .await?;
    Ok(Asked::Password(
        answers.into_iter().next().unwrap_or_default(),
    ))
}

/// Password authentication without a saved password: straight to keyboard-interactive, whose
/// questions (the server's password prompt, then any second factor in the same exchange) the user
/// answers. A server that takes the password only as such (the `password` method, no
/// keyboard-interactive) needs it saved.
async fn prompted_password(
    session: &mut client::Handle<Client>,
    sign_in: &SignIn<'_>,
    state: &watch::Sender<TunnelState>,
) -> Result<(), AppError> {
    let Some((asked, result)) = keyboard_interactive(session, sign_in, None).await? else {
        return Err(no_saved_password());
    };
    let AuthResult::Failure {
        remaining_methods,
        partial_success,
    } = result
    else {
        return Ok(());
    };
    if !partial_success
        && method_names(&remaining_methods).is_empty()
        && let Some(reason) = session_end(session, state).await
    {
        return Err(ended_during_auth("keyboard-interactive", &reason));
    }
    if !asked && !partial_success {
        // No question came: the server does not do keyboard-interactive.
        let accepted = method_names(&remaining_methods);
        return Err(AppError::Auth(if accepted.is_empty() {
            NO_SAVED_PASSWORD.to_string()
        } else {
            format!("{NO_SAVED_PASSWORD}; the server accepts: {accepted}")
        }));
    }
    Err(auth_failure(
        "keyboard-interactive",
        sign_in.user,
        &remaining_methods,
        partial_success,
    ))
}

/// A round that is the server's password question: one hidden prompt that mentions the password
/// (any hidden prompt when nobody else could answer it).
fn is_password_question(prompts: &[client::Prompt], only_answerer: bool) -> bool {
    matches!(prompts, [p] if !p.echo
        && (only_answerer || p.prompt.to_ascii_lowercase().contains("password")))
}

/// Keyboard-interactive authentication. Each round is answered with `password` when it is the
/// server's (first) password question, else by the user through `sign_in.prompts`; a round
/// without questions (instructions only) gets an empty answer. Returns whether the user answered
/// any round, with the server's verdict, or `None` when a round could not be answered (nobody to
/// ask). A cancelled question is `AppError::Cancelled`.
async fn keyboard_interactive(
    session: &mut client::Handle<Client>,
    sign_in: &SignIn<'_>,
    password: Option<&str>,
) -> Result<Option<(bool, AuthResult)>, AppError> {
    let mut password = password;
    let mut asked = false;
    let mut reply = session
        .authenticate_keyboard_interactive_start(sign_in.user, None)
        .await
        .map_err(ssh_err)?;
    for _ in 0..MAX_PROMPT_ROUNDS {
        let (name, instructions, prompts) = match reply {
            client::KeyboardInteractiveAuthResponse::Success => {
                return Ok(Some((asked, AuthResult::Success)));
            }
            client::KeyboardInteractiveAuthResponse::Failure {
                remaining_methods,
                partial_success,
            } => {
                return Ok(Some((
                    asked,
                    AuthResult::Failure {
                        remaining_methods,
                        partial_success,
                    },
                )));
            }
            client::KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => (name, instructions, prompts),
        };
        let answers = if prompts.is_empty() {
            Vec::new()
        } else if let Some(pw) =
            password.filter(|_| is_password_question(&prompts, sign_in.prompts.is_none()))
        {
            password = None;
            vec![pw.to_string()]
        } else if let Some(user) = sign_in.prompts {
            asked = true;
            user.answer(PromptRound {
                user: sign_in.user.to_string(),
                host: sign_in.host.to_string(),
                port: sign_in.port,
                name,
                instructions,
                prompts: prompts
                    .into_iter()
                    .map(|p| SshPromptField {
                        prompt: p.prompt,
                        echo: p.echo,
                    })
                    .collect(),
                kind: None,
            })
            .await?
        } else {
            return Ok(None);
        };
        reply = session
            .authenticate_keyboard_interactive_respond(answers)
            .await
            .map_err(ssh_err)?;
    }
    Err(AppError::Auth(format!(
        "ssh: the SSH server kept asking questions ({MAX_PROMPT_ROUNDS} rounds); giving up"
    )))
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
    prefer: Option<&PublicKey>,
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
    let mut identities = agent
        .request_identities()
        .await
        .map_err(|e| AppError::Auth(format!("ssh: the SSH agent did not list its keys: {e}")))?;
    if let Some(prefer) = prefer {
        // The key the config names goes first (agents like 1Password hold many keys, and servers
        // stop after a few tries).
        identities.sort_by_key(|i| i.public_key().key_data() != prefer.key_data());
    }
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
    _prefer: Option<&PublicKey>,
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
                "an SSH private key file is required for key authentication (none chosen, none \
                 in ~/.ssh/config, no ~/.ssh/id_ed25519, id_ecdsa or id_rsa)"
                    .into(),
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
    target: Arc<ForwardTarget>,
    /// The socket path that opened last (tried first next time).
    socket_pick: Arc<std::sync::atomic::AtomicUsize>,
    forward_error: LastForwardFailure,
}

/// Why the SSH server did not open a channel to a Unix socket.
fn describe_socket_error(e: &russh::Error) -> String {
    match e {
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed) => {
            "no server listens on that socket (check the socket path: SELECT @@socket shows it)"
                .into()
        }
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited) => {
            "socket forwarding is not allowed by the SSH server (AllowStreamLocalForwarding)".into()
        }
        other => describe_channel_error(other),
    }
}

/// The order the socket paths are tried in: the one that opened last, then the others.
fn socket_order(count: usize, last: usize) -> Vec<usize> {
    let first = if last < count { last } else { 0 };
    std::iter::once(first)
        .chain((0..count).filter(|i| *i != first))
        .filter(|i| *i < count)
        .collect()
}

/// Opens the channel one accepted connection is forwarded over; the error is the message the
/// tunnel reports.
async fn open_forward_channel(
    forwarder: &Forwarder,
    peer: SocketAddr,
) -> Result<russh::Channel<client::Msg>, String> {
    match forwarder.target.as_ref() {
        ForwardTarget::Tcp { host, port } => forwarder
            .session
            .channel_open_direct_tcpip(
                host.clone(),
                u32::from(*port),
                peer.ip().to_string(),
                u32::from(peer.port()),
            )
            .await
            .map_err(|e| {
                format!(
                    "the SSH server could not open a connection to {host}:{port}: {}",
                    describe_channel_error(&e)
                )
            }),
        ForwardTarget::Socket(paths) => {
            use std::sync::atomic::Ordering;
            let last = forwarder.socket_pick.load(Ordering::Relaxed);
            let mut failure = String::new();
            for i in socket_order(paths.len(), last) {
                let path = paths[i].trim();
                match forwarder
                    .session
                    .channel_open_direct_streamlocal(path)
                    .await
                {
                    Ok(channel) => {
                        forwarder.socket_pick.store(i, Ordering::Relaxed);
                        return Ok(channel);
                    }
                    Err(e) => {
                        failure = format!(
                            "the SSH server could not open the socket {path}: {}",
                            describe_socket_error(&e)
                        );
                        // Not a missing socket: the next path would fail the same way.
                        if !matches!(
                            e,
                            russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed)
                        ) {
                            break;
                        }
                    }
                }
            }
            if paths.len() > 1 {
                failure = format!(
                    "the SSH server has no MySQL socket at {}; enter the socket path (SELECT \
                     @@socket shows it)",
                    paths.join(", ")
                );
            }
            Err(failure)
        }
    }
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
    // Only this app (and its dump binaries) may use the tunnel (TablePlus: "the local port only
    // accepts connections from TablePlus").
    if peer_check_enabled()
        && let Ok(local_addr) = local.local_addr()
    {
        let owner = tokio::task::spawn_blocking(move || peer::owner(peer, local_addr))
            .await
            .unwrap_or(peer::PeerOwner::Unknown);
        if !owner.allowed() {
            log::warn!(
                "ssh tunnel on port {}: refused a connection from {peer}, which does not come \
                 from Tabler",
                local_addr.port()
            );
            return;
        }
        log::debug!("ssh tunnel: local connection from {peer} ({owner:?})");
    }
    let _ = local.set_nodelay(true);
    let opened = tokio::select! {
        _ = cancel.cancelled() => return,
        ch = open_forward_channel(&forwarder, peer) => ch,
    };
    let channel = match opened {
        Ok(ch) => ch,
        Err(msg) => {
            log::warn!("{msg}");
            *lock(&forwarder.forward_error) = Some(ForwardFailure {
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

/// Whether accepted connections are checked to come from this app ([`peer`]). Setting
/// `TABLER_TUNNEL_ANY_PEER=1` turns the check off (troubleshooting only).
fn peer_check_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        let off = std::env::var("TABLER_TUNNEL_ANY_PEER").is_ok_and(|v| v == "1");
        if off {
            log::warn!("TABLER_TUNNEL_ANY_PEER=1: SSH tunnels accept connections from any program");
        }
        !off
    })
}

/// Disconnects the SSH session (and its jump hosts) when the handle is closed/dropped, and stops
/// forwarding (but keeps the port reserved) when the session dies on its own.
async fn supervise(
    session: Arc<client::Handle<Client>>,
    jumps: Jumps,
    mut state: watch::Receiver<TunnelState>,
    cancel: CancellationToken,
    forwarding: CancellationToken,
) {
    tokio::select! {
        _ = cancel.cancelled() => {}
        _ = state.wait_for(|s| !s.is_up()) => forwarding.cancel(),
    }
    disconnect_all(&session, &jumps).await;
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leases_are_counted_once_each_and_the_last_one_closes() {
        let count = Mutex::new(1);
        let (first, second) = (
            std::sync::atomic::AtomicBool::new(false),
            std::sync::atomic::AtomicBool::new(false),
        );
        assert!(add_lease(&count));
        assert_eq!(*lock(&count), 2);
        // Closing a lease twice gives it back once; the other lease keeps the tunnel.
        assert!(!release_lease(&first, &count));
        assert!(!release_lease(&first, &count));
        assert_eq!(*lock(&count), 1);
        assert!(release_lease(&second, &count));
        assert_eq!(*lock(&count), 0);
        // A tunnel whose leases are all given back is not handed out again.
        assert!(!add_lease(&count));
    }

    #[test]
    fn socket_paths_start_with_the_one_that_opened_last() {
        assert_eq!(socket_order(3, 0), [0, 1, 2]);
        assert_eq!(socket_order(3, 2), [2, 0, 1]);
        assert_eq!(socket_order(1, 5), [0]);
        assert!(socket_order(0, 0).is_empty());
        let target = ForwardTarget::Socket(vec!["/tmp/mysql.sock".into()]);
        assert_eq!(target.to_string(), "unix:/tmp/mysql.sock");
        let tcp = ForwardTarget::Tcp {
            host: "db".into(),
            port: 3306,
        };
        assert_eq!(tcp.to_string(), "db:3306");
    }

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

    /// Windows binds 127.0.0.1:P next to a socket on the wildcard address of P and then hands the
    /// loopback connections to the tunnel, so such a port is refused. An IPv6-only socket takes
    /// no IPv4 connections and is left alone.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_pinned_port_held_on_a_wildcard_address_is_refused() {
        use socket2::{Domain, Socket, Type};
        let hold = |any: SocketAddr, only_v6: bool| {
            let socket = Socket::new(Domain::for_address(any), Type::STREAM, None).unwrap();
            if any.is_ipv6() {
                socket.set_only_v6(only_v6).unwrap();
            }
            // Bound only: listening on a wildcard address would ask for a firewall rule.
            socket.bind(&any.into()).unwrap();
            let port = socket.local_addr().unwrap().as_socket().unwrap().port();
            (socket, port)
        };
        let v4_any = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0));
        let v6_any = SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0));
        for any in [v4_any, v6_any] {
            let (socket, port) = hold(any, false);
            let err = bind_local(port).await.unwrap_err();
            assert!(
                matches!(&err, AppError::Tunnel(m) if m.contains("cannot listen") && m.contains("in use")),
                "{any}: {err:?}"
            );
            drop(socket);
            let free = bind_local(port).await.unwrap();
            assert_eq!(free.local_addr().unwrap().port(), port);
        }
        let (_v6_only, port) = hold(v6_any, true);
        let pinned = bind_local(port).await.unwrap();
        assert_eq!(pinned.local_addr().unwrap().port(), port);
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
            password_mode: None,
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

    #[test]
    fn form_values_win_over_ssh_config() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("deploy_key");
        std::fs::write(&key, "key").unwrap();
        let key = key.to_string_lossy().into_owned();
        let config = ssh_config::HostConfig {
            host_name: Some("bastion.example.com".into()),
            user: Some("deploy".into()),
            port: Some(2222),
            identity_files: vec!["/nonexistent/first".into(), key.clone()],
            ..Default::default()
        };
        let mut cfg = SshConfig {
            host: "bastion".into(),
            port: 0,
            user: " ".into(),
            auth: SshAuth::Key,
            key_path: None,
            local_port: None,
            keepalive_secs: None,
            password_mode: None,
        };
        // Empty form fields: the config fills them (the first existing IdentityFile).
        assert_eq!(
            resolve_ssh(&cfg, "bastion", &config).unwrap(),
            ResolvedSsh {
                host: "bastion.example.com".into(),
                port: 2222,
                user: "deploy".into(),
                key_path: Some(key.clone()),
            }
        );
        // Typed values win.
        cfg.port = 22;
        cfg.user = "me".into();
        cfg.key_path = Some("/keys/mine".into());
        let r = resolve_ssh(&cfg, "bastion", &config).unwrap();
        assert_eq!((r.port, r.user.as_str()), (22, "me"));
        assert_eq!(r.key_path.as_deref(), Some("/keys/mine"));
        // No config entry: the alias is the host, port 22; password auth needs no key.
        cfg.port = 0;
        cfg.auth = SshAuth::Password;
        let r = resolve_ssh(&cfg, "db.example.com", &ssh_config::HostConfig::default()).unwrap();
        assert_eq!((r.host.as_str(), r.port), ("db.example.com", 22));
        assert_eq!(r.key_path, None);
    }
}
