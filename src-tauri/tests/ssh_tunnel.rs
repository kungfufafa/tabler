//! SSH tunnel tests against an in-process russh server (no external SSH server needed): host-key
//! trust (unknown / accepted / changed, `known_hosts` quirks), password and key authentication,
//! byte forwarding, shutdown, status changes, the reserved local port of a dead tunnel, and the
//! temporary tunnel of "Test connection". When `TABLER_TEST_PG_URL` / `TABLER_TEST_MYSQL_URL` are
//! set, a real database connection also goes through the tunnel.

// Same allowance as the library: `AppError` carries full server diagnostics.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::{Cipher, Kdf, LineEnding};
use russh::keys::{HashAlg, PrivateKey, PublicKey, PublicKeyBase64};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelOpenFailure, MethodKind, MethodSet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{RwLock, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use tabler_lib::appdb::AppDb;
use tabler_lib::commands::connections::{open_profile_tunnel, release_pinned_port};
use tabler_lib::commands::profiles::{resolve_tunnel_auth, run_connection_test, store_secrets};
use tabler_lib::drivers::{ConnectTarget, connect};
use tabler_lib::error::AppError;
use tabler_lib::secrets::{Backend, SecretKind, Secrets};
use tabler_lib::state::{AppState, Connection};
use tabler_lib::tunnel::{
    AppHostKeys, MemoryHostKeys, PendingHostKey, TunnelAuth, TunnelHandle, TunnelStatus,
    accept_pending_host_key, open_tunnel, trust_host_key,
};
#[cfg(unix)]
use tabler_lib::tunnel::{ForwardTarget, open_tunnel_to};
use tabler_lib::types::profile::{
    ConnStatus, ConnectionInfo, ConnectionProfile, Driver, ProfileInput, ProfileOptions, SshAuth,
    SshConfig, SslMode, TunnelInfo,
};
use tabler_lib::types::value::Value;

const USER: &str = "tabler";
const PASSWORD: &str = "secret";
const PASSPHRASE: &str = "correct horse";
const LOCALHOST: &str = "127.0.0.1";

// ----- fixtures ---------------------------------------------------------------------------------

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    for chunk in out.chunks_mut(16) {
        let id = Uuid::new_v4();
        chunk.copy_from_slice(&id.as_bytes()[..chunk.len()]);
    }
    out
}

fn random_key() -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&random_bytes::<32>()))
}

/// Writes `key` as an OpenSSH private key file, encrypted when `passphrase` is given.
fn write_key(dir: &Path, name: &str, key: &PrivateKey, passphrase: Option<&str>) -> PathBuf {
    let path = dir.join(name);
    let key = match passphrase {
        Some(p) => key
            .encrypt_with(
                Cipher::Aes256Ctr,
                Kdf::Bcrypt {
                    salt: random_bytes::<16>().to_vec(),
                    rounds: 16,
                },
                u32::from_le_bytes(random_bytes::<4>()),
                p,
            )
            .expect("encrypt key"),
        None => key.clone(),
    };
    key.write_openssh_file(&path, LineEnding::LF)
        .expect("write key");
    path
}

/// Writes `key` as a PKCS#8 PEM file (`BEGIN PRIVATE KEY`, or `BEGIN ENCRYPTED PRIVATE KEY` with
/// a passphrase), the format `openssl genpkey` produces.
fn write_pkcs8_key(dir: &Path, name: &str, key: &PrivateKey, passphrase: Option<&str>) -> PathBuf {
    let path = dir.join(name);
    let mut file = std::fs::File::create(&path).unwrap();
    match passphrase {
        Some(p) => russh::keys::encode_pkcs8_pem_encrypted(key, p.as_bytes(), 16, &mut file),
        None => russh::keys::encode_pkcs8_pem(key, &mut file),
    }
    .expect("write PKCS#8 key");
    path
}

/// How the in-process SSH server authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthMode {
    /// Password (`tabler` / `secret`) or public key.
    Standard,
    /// `AuthenticationMethods password,keyboard-interactive`: the password is only the first
    /// factor (partial success), then a keyboard-interactive prompt asks for a one-time code.
    PasswordThenOtp,
    /// Keyboard-interactive only, prompting for the password.
    KeyboardInteractiveOnly,
    /// Drops the connection as soon as the client tries to authenticate.
    DropOnAuth,
}

impl AuthMode {
    fn methods(self) -> MethodSet {
        let kinds: &[MethodKind] = match self {
            AuthMode::Standard | AuthMode::DropOnAuth => {
                &[MethodKind::Password, MethodKind::PublicKey]
            }
            AuthMode::PasswordThenOtp => &[MethodKind::Password, MethodKind::KeyboardInteractive],
            AuthMode::KeyboardInteractiveOnly => &[MethodKind::KeyboardInteractive],
        };
        MethodSet::from(kinds)
    }
}

/// The one-time code `AuthMode::PasswordThenOtp` accepts.
const OTP: &str = "123456";

/// In-process SSH server: password (`tabler` / `secret`) and public-key auth for one client
/// key (or another `AuthMode`), `direct-tcpip` channels forwarded to the requested host:port.
/// The host key can be rotated and live sessions kicked.
struct SshServer {
    addr: SocketAddr,
    host_key: Arc<Mutex<PrivateKey>>,
    sessions: Arc<Mutex<Vec<server::Handle>>>,
    /// `direct-tcpip` channels requested so far.
    forwards: Arc<AtomicUsize>,
    /// Answers received to keyboard-interactive prompts.
    answers: Arc<Mutex<Vec<String>>>,
    stop: CancellationToken,
}

impl SshServer {
    async fn start(client_key: PublicKey) -> SshServer {
        SshServer::start_with(client_key, AuthMode::Standard).await
    }

    async fn start_with(client_key: PublicKey, mode: AuthMode) -> SshServer {
        let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let host_key = Arc::new(Mutex::new(random_key()));
        let sessions = Arc::new(Mutex::new(Vec::new()));
        let forwards = Arc::new(AtomicUsize::new(0));
        let answers = Arc::new(Mutex::new(Vec::new()));
        let stop = CancellationToken::new();
        tokio::spawn({
            let host_key = host_key.clone();
            let sessions = sessions.clone();
            let forwards = forwards.clone();
            let answers = answers.clone();
            let stop = stop.clone();
            async move {
                loop {
                    let socket = tokio::select! {
                        _ = stop.cancelled() => break,
                        accepted = listener.accept() => match accepted {
                            Ok((socket, _)) => socket,
                            Err(_) => continue,
                        },
                    };
                    let config = Arc::new(server::Config {
                        keys: vec![host_key.lock().unwrap().clone()],
                        methods: mode.methods(),
                        auth_rejection_time: Duration::from_millis(10),
                        auth_rejection_time_initial: Some(Duration::ZERO),
                        inactivity_timeout: None,
                        ..Default::default()
                    });
                    let handler = ServerHandler {
                        client_key: client_key.clone(),
                        forwards: forwards.clone(),
                        mode,
                        answers: answers.clone(),
                    };
                    let sessions = sessions.clone();
                    tokio::spawn(async move {
                        if let Ok(running) = server::run_stream(config, socket, handler).await {
                            sessions.lock().unwrap().push(running.handle());
                            let _ = running.await;
                        }
                    });
                }
            }
        });
        SshServer {
            addr,
            host_key,
            sessions,
            forwards,
            answers,
            stop,
        }
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }

    fn forwards(&self) -> usize {
        self.forwards.load(Ordering::SeqCst)
    }

    fn answers(&self) -> Vec<String> {
        self.answers.lock().unwrap().clone()
    }

    fn host_public_key(&self) -> PublicKey {
        self.host_key.lock().unwrap().public_key().clone()
    }

    fn rotate_host_key(&self) {
        *self.host_key.lock().unwrap() = random_key();
    }

    /// Disconnects every live session from the server side.
    async fn kick_all(&self) {
        let sessions: Vec<server::Handle> = self.sessions.lock().unwrap().drain(..).collect();
        for s in sessions {
            let _ = s
                .disconnect(
                    russh::Disconnect::ByApplication,
                    "server going away".into(),
                    "en".into(),
                )
                .await;
        }
    }
}

impl Drop for SshServer {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

struct ServerHandler {
    client_key: PublicKey,
    forwards: Arc<AtomicUsize>,
    mode: AuthMode,
    answers: Arc<Mutex<Vec<String>>>,
}

impl server::Handler for ServerHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let ok = user == USER && password == PASSWORD;
        match self.mode {
            AuthMode::DropOnAuth => Err(russh::Error::Disconnect),
            AuthMode::Standard if ok => Ok(Auth::Accept),
            AuthMode::PasswordThenOtp if ok => Ok(Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(&[MethodKind::KeyboardInteractive][..])),
                partial_success: true,
            }),
            // Like OpenSSH: until the first factor succeeds, only `password` may continue.
            AuthMode::PasswordThenOtp => Ok(Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(&[MethodKind::Password][..])),
                partial_success: false,
            }),
            _ => Ok(Auth::reject()),
        }
    }

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        _key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        if self.mode == AuthMode::DropOnAuth {
            return Err(russh::Error::Disconnect);
        }
        Ok(Auth::Accept)
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        response: Option<server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        let (prompt, expected) = match self.mode {
            AuthMode::PasswordThenOtp => ("Verification code: ", OTP),
            AuthMode::KeyboardInteractiveOnly => ("Password: ", PASSWORD),
            _ => return Ok(Auth::reject()),
        };
        let Some(response) = response else {
            return Ok(Auth::Partial {
                name: "".into(),
                instructions: "".into(),
                prompts: vec![(prompt.into(), false)].into(),
            });
        };
        let answers: Vec<String> = response
            .map(|answer| String::from_utf8_lossy(&answer).into_owned())
            .collect();
        self.answers.lock().unwrap().extend(answers.iter().cloned());
        Ok(if user == USER && answers == [expected] {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        Ok(
            if user == USER && key.key_data() == self.client_key.key_data() {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.forwards.fetch_add(1, Ordering::SeqCst);
        let target = format!("{host_to_connect}:{port_to_connect}");
        tokio::spawn(async move {
            match TcpStream::connect(&target).await {
                Ok(mut tcp) => {
                    reply.accept().await;
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
                }
                Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
            }
        });
        Ok(())
    }

    /// `direct-streamlocal@openssh.com`: a Unix socket on the server (MySQL socket over SSH).
    async fn channel_open_direct_streamlocal(
        &mut self,
        channel: Channel<Msg>,
        socket_path: &str,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.forwards.fetch_add(1, Ordering::SeqCst);
        let path = socket_path.to_string();
        // Unix sockets only exist on Unix test hosts; elsewhere the channel is refused.
        #[cfg(not(unix))]
        {
            let _ = (channel, path);
            reply.reject(ChannelOpenFailure::ConnectFailed).await;
        }
        #[cfg(unix)]
        tokio::spawn(async move {
            match tokio::net::UnixStream::connect(&path).await {
                Ok(mut unix) => {
                    reply.accept().await;
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut unix).await;
                }
                Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
            }
        });
        Ok(())
    }
}

/// Unix socket echo server at `path`.
#[cfg(unix)]
fn start_unix_echo(path: &Path) {
    let listener = tokio::net::UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = socket.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
}

/// TCP echo server; returns its port.
async fn start_echo() -> u16 {
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = socket.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    port
}

fn ssh_cfg(port: u16, auth: SshAuth, key_path: Option<&Path>) -> SshConfig {
    SshConfig {
        host: LOCALHOST.into(),
        port,
        user: USER.into(),
        auth,
        key_path: key_path.map(|p| p.display().to_string()),
        local_port: None,
        keepalive_secs: Some(5),
        password_mode: None,
    }
}

fn password(p: &str) -> TunnelAuth {
    TunnelAuth {
        password: Some(p.into()),
        ..TunnelAuth::default()
    }
}

fn passphrase(p: Option<&str>) -> TunnelAuth {
    TunnelAuth {
        passphrase: p.map(str::to_string),
        ..TunnelAuth::default()
    }
}

/// A memory trust store that already trusts the server's current key.
fn trusting(server: &SshServer) -> MemoryHostKeys {
    let trust = MemoryHostKeys::new();
    trust.trust(PendingHostKey::from_key(
        LOCALHOST,
        server.port(),
        &server.host_public_key(),
    ));
    trust
}

/// Sends `len` bytes through the tunnel and checks the echo comes back unchanged.
async fn round_trip(local_port: u16, len: usize) {
    let stream = TcpStream::connect((LOCALHOST, local_port))
        .await
        .expect("connect to tunnel");
    let (mut r, mut w) = stream.into_split();
    let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    let expected = payload.clone();
    let writer = tokio::spawn(async move {
        w.write_all(&payload).await.unwrap();
        w.flush().await.unwrap();
        w
    });
    let mut got = vec![0u8; len];
    tokio::time::timeout(Duration::from_secs(20), r.read_exact(&mut got))
        .await
        .expect("echo timed out")
        .expect("read echo");
    assert!(got == expected, "echoed bytes differ ({len} bytes)");
    drop(writer.await.unwrap());
}

/// A free local port (bound, then released).
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind((LOCALHOST, 0)).unwrap();
    l.local_addr().unwrap().port()
}

/// The port of a tunnel whose SSH session is gone: still bound (nobody else can take it), but
/// every connection is closed straight away without reaching anything.
async fn assert_reserved_and_closed(port: u16) {
    assert!(
        std::net::TcpListener::bind((LOCALHOST, port)).is_err(),
        "port {port} was released while the tunnel handle is alive"
    );
    let mut stream = TcpStream::connect((LOCALHOST, port))
        .await
        .expect("the reserved port must still accept");
    let _ = stream.write_all(b"ping").await;
    let mut buf = [0u8; 8];
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .expect("connection to a dead tunnel was not closed")
        .unwrap_or(0);
    assert_eq!(n, 0, "a dead tunnel must not forward bytes");
}

async fn wait_down(tunnel: &TunnelHandle) {
    let mut status = tunnel.subscribe();
    tokio::time::timeout(Duration::from_secs(10), status.wait_for(|s| !s.is_up()))
        .await
        .expect("tunnel status did not change")
        .expect("status channel closed");
}

async fn assert_refused(port: u16) {
    // The listener is dropped by a task; give it a moment after shutdown.
    for _ in 0..50 {
        if TcpStream::connect((LOCALHOST, port)).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("port {port} still accepts connections after the tunnel closed");
}

async fn open(
    cfg: &SshConfig,
    auth: TunnelAuth,
    target_port: u16,
    trust: &dyn tabler_lib::tunnel::HostKeyTrust,
) -> Result<TunnelHandle, AppError> {
    tokio::time::timeout(
        Duration::from_secs(20),
        open_tunnel(cfg, auth, LOCALHOST.into(), target_port, trust),
    )
    .await
    .expect("open_tunnel timed out")
}

// ----- tests ------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_host_key_then_trusted_tunnel_forwards_bytes() {
    let echo = start_echo().await;
    let server = SshServer::start(random_key().public_key().clone()).await;
    let appdb = AppDb::open_in_memory().unwrap();
    let pending = Mutex::new(HashMap::new());
    let profile_id = Uuid::new_v4();
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    let trust =
        AppHostKeys::new(&appdb, &pending, profile_id).with_known_hosts(Some(known_hosts.clone()));
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);

    // Unknown key: HostKey error with the server's fingerprint, key parked as pending.
    let err = open(&cfg, password(PASSWORD), echo, &trust)
        .await
        .unwrap_err();
    let payload = serde_json::to_value(&err).unwrap();
    assert_eq!(payload["code"], "SSH_HOST_KEY_UNKNOWN");
    let AppError::HostKey {
        host,
        port,
        key_type,
        fingerprint,
        changed,
    } = err
    else {
        panic!("expected HostKey, got {err:?}");
    };
    let expected_fp = server
        .host_public_key()
        .fingerprint(HashAlg::Sha256)
        .to_string();
    assert!(!changed);
    assert_eq!((host.as_str(), port), (LOCALHOST, server.port()));
    assert_eq!(key_type, "ssh-ed25519");
    assert_eq!(fingerprint, expected_fp);
    assert_eq!(payload["data"]["fingerprint"], expected_fp.as_str());
    assert_eq!(
        pending.lock().unwrap()[&profile_id].fingerprint,
        expected_fp
    );

    // A wrong fingerprint is refused; the right one trusts the key (and writes known_hosts).
    assert!(matches!(
        accept_pending_host_key(&appdb, &pending, profile_id, "SHA256:bogus", None),
        Err(AppError::InvalidInput(_))
    ));
    accept_pending_host_key(
        &appdb,
        &pending,
        profile_id,
        &fingerprint,
        Some(&known_hosts),
    )
    .unwrap();
    assert!(pending.lock().unwrap().is_empty());
    let kh = std::fs::read_to_string(&known_hosts).unwrap();
    assert!(
        kh.starts_with(&format!("[{LOCALHOST}]:{} ssh-ed25519 ", server.port())),
        "{kh}"
    );

    // Trusted: the tunnel comes up and forwards bytes, also on concurrent connections.
    let tunnel = open(&cfg, password(PASSWORD), echo, &trust).await.unwrap();
    assert!(tunnel.is_up());
    let local_port = tunnel.local_port();
    assert_ne!(local_port, 0);
    assert_eq!(tunnel.connect_addr(), (LOCALHOST.to_string(), local_port));
    round_trip(local_port, 11).await;
    round_trip(local_port, 1024 * 1024).await;
    let parallel: Vec<_> = (0..4)
        .map(|i| tokio::spawn(round_trip(local_port, 64 * 1024 + i)))
        .collect();
    for p in parallel {
        p.await.unwrap();
    }

    // The known_hosts entry alone is enough to trust the server (fresh app database).
    let other_db = AppDb::open_in_memory().unwrap();
    let kh_trust = AppHostKeys::new(&other_db, &pending, profile_id)
        .with_known_hosts(Some(known_hosts.clone()));
    let second = open(&cfg, password(PASSWORD), echo, &kh_trust)
        .await
        .unwrap();
    round_trip(second.local_port(), 100).await;
    drop(second);

    // Closing stops the listener and reports down.
    let mut status = tunnel.subscribe();
    tunnel.close().await;
    assert!(!tunnel.is_up());
    assert_eq!(status.borrow_and_update().status, TunnelStatus::Down);
    assert_refused(local_port).await;
    tunnel.close().await; // idempotent
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_password_is_an_auth_error() {
    let echo = start_echo().await;
    let server = SshServer::start(random_key().public_key().clone()).await;
    let trust = trusting(&server);
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);

    let err = open(&cfg, password("wrong"), echo, &trust)
        .await
        .unwrap_err();
    let AppError::Auth(msg) = &err else {
        panic!("expected Auth, got {err:?}");
    };
    assert!(
        msg.starts_with("ssh: password authentication failed"),
        "{msg}"
    );
    assert!(
        msg.contains("password") && msg.contains("publickey"),
        "{msg}"
    );
    assert_eq!(serde_json::to_value(&err).unwrap()["code"], "AUTH_FAILED");

    // No saved password at all: auth error before any network traffic.
    let err = open(&cfg, TunnelAuth::default(), echo, &trust)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Auth(_)), "{err:?}");

    // The right password still works afterwards.
    let tunnel = open(&cfg, password(PASSWORD), echo, &trust).await.unwrap();
    round_trip(tunnel.local_port(), 32).await;
    tunnel.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_authentication_plain_and_encrypted() {
    let echo = start_echo().await;
    let client_key = random_key();
    let server = SshServer::start(client_key.public_key().clone()).await;
    let trust = trusting(&server);
    let dir = tempfile::tempdir().unwrap();

    // Unencrypted key.
    let plain = write_key(dir.path(), "id_plain", &client_key, None);
    let cfg = ssh_cfg(server.port(), SshAuth::Key, Some(&plain));
    let tunnel = open(&cfg, passphrase(None), echo, &trust).await.unwrap();
    round_trip(tunnel.local_port(), 4096).await;
    tunnel.close().await;

    // Encrypted copy: needs the passphrase.
    let encrypted = write_key(dir.path(), "id_encrypted", &client_key, Some(PASSPHRASE));
    let cfg = ssh_cfg(server.port(), SshAuth::Key, Some(&encrypted));
    let err = open(&cfg, passphrase(None), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.contains("encrypted")),
        "{err:?}"
    );
    let err = open(&cfg, passphrase(Some("nope")), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.contains("passphrase")),
        "{err:?}"
    );
    let tunnel = open(&cfg, passphrase(Some(PASSPHRASE)), echo, &trust)
        .await
        .unwrap();
    round_trip(tunnel.local_port(), 4096).await;
    tunnel.close().await;

    // A key the server does not know is rejected with the accepted methods listed.
    let stranger = write_key(dir.path(), "id_stranger", &random_key(), None);
    let cfg = ssh_cfg(server.port(), SshAuth::Key, Some(&stranger));
    let err = open(&cfg, passphrase(None), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.contains("public key authentication failed")),
        "{err:?}"
    );

    // A missing key file is reported as such.
    let missing = dir.path().join("does_not_exist");
    let cfg = ssh_cfg(server.port(), SshAuth::Key, Some(&missing));
    let err = open(&cfg, passphrase(None), echo, &trust)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Tunnel(_)), "{err:?}");
}

/// In-process ssh-agent on a temp socket (russh's agent server), so the user's agent is untouched.
#[cfg(unix)]
async fn start_agent(dir: &Path) -> PathBuf {
    start_agent_with(dir, "agent.sock", ()).await
}

#[cfg(unix)]
async fn start_agent_with<A>(dir: &Path, name: &str, agent: A) -> PathBuf
where
    A: russh::keys::agent::server::Agent + Send + Sync + 'static,
{
    let socket = dir.join(name);
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let incoming = Box::pin(futures::stream::unfold(listener, |l| async move {
        let next = l.accept().await.map(|(stream, _)| stream);
        Some((next, l))
    }));
    tokio::spawn(russh::keys::agent::server::serve(incoming, agent));
    socket
}

/// An ssh-agent that refuses every signature, like a confirmation prompt (`ssh-add -c`,
/// 1Password, Secretive) the user dismissed.
#[cfg(unix)]
#[derive(Clone)]
struct RefusingAgent;

#[cfg(unix)]
impl russh::keys::agent::server::Agent for RefusingAgent {
    fn confirm_request(
        &self,
        msg: russh::keys::agent::server::MessageType,
    ) -> impl std::future::Future<Output = bool> + Send {
        let allowed = !matches!(msg, russh::keys::agent::server::MessageType::Sign);
        async move { allowed }
    }
}

#[cfg(unix)]
fn via_agent(socket: &Path) -> TunnelAuth {
    TunnelAuth {
        agent_socket: Some(socket.to_path_buf()),
        ..TunnelAuth::default()
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_authentication() {
    use russh::keys::agent::client::AgentClient;

    let echo = start_echo().await;
    let client_key = random_key();
    let server = SshServer::start(client_key.public_key().clone()).await;
    let trust = trusting(&server);
    let dir = tempfile::tempdir().unwrap();
    let socket = start_agent(dir.path()).await;
    let cfg = ssh_cfg(server.port(), SshAuth::Agent, None);

    // Empty agent.
    let err = open(&cfg, via_agent(&socket), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.contains("no keys")),
        "{err:?}"
    );

    // An unrelated key, then the right one: the agent tries each identity.
    let mut agent = AgentClient::connect_uds(&socket).await.unwrap();
    agent.add_identity(&random_key(), &[]).await.unwrap();
    agent.add_identity(&client_key, &[]).await.unwrap();
    let tunnel = open(&cfg, via_agent(&socket), echo, &trust).await.unwrap();
    round_trip(tunnel.local_port(), 2048).await;
    tunnel.close().await;

    // No agent listening at the socket.
    let err = open(
        &cfg,
        via_agent(&dir.path().join("missing.sock")),
        echo,
        &trust,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.contains("cannot reach the SSH agent")),
        "{err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_host_key_is_rejected() {
    let echo = start_echo().await;
    let server = SshServer::start(random_key().public_key().clone()).await;
    let appdb = AppDb::open_in_memory().unwrap();
    let pending = Mutex::new(HashMap::new());
    let profile_id = Uuid::new_v4();
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);

    // Trust the original key in the app table (no known_hosts) and in a known_hosts file.
    let db_trust = AppHostKeys::new(&appdb, &pending, profile_id).with_known_hosts(None);
    let err = open(&cfg, password(PASSWORD), echo, &db_trust)
        .await
        .unwrap_err();
    let AppError::HostKey { fingerprint, .. } = err else {
        panic!("expected HostKey, got {err:?}");
    };
    accept_pending_host_key(
        &appdb,
        &pending,
        profile_id,
        &fingerprint,
        Some(&known_hosts),
    )
    .unwrap();
    let tunnel = open(&cfg, password(PASSWORD), echo, &db_trust)
        .await
        .unwrap();
    tunnel.close().await;

    server.rotate_host_key();
    let new_fp = server
        .host_public_key()
        .fingerprint(HashAlg::Sha256)
        .to_string();

    // App table says "changed".
    let err = open(&cfg, password(PASSWORD), echo, &db_trust)
        .await
        .unwrap_err();
    assert_eq!(
        serde_json::to_value(&err).unwrap()["code"],
        "SSH_HOST_KEY_CHANGED"
    );
    let AppError::HostKey {
        changed,
        fingerprint,
        ..
    } = err
    else {
        panic!("expected HostKey, got {err:?}");
    };
    assert!(changed);
    assert_eq!(fingerprint, new_fp);
    // A changed key is never parked for acceptance.
    assert!(pending.lock().unwrap().is_empty());
    assert!(matches!(
        accept_pending_host_key(&appdb, &pending, profile_id, &new_fp, None),
        Err(AppError::NotFound(_))
    ));

    // known_hosts says "changed" too, even with an empty app table.
    let fresh_db = AppDb::open_in_memory().unwrap();
    let kh_trust =
        AppHostKeys::new(&fresh_db, &pending, profile_id).with_known_hosts(Some(known_hosts));
    let err = open(&cfg, password(PASSWORD), echo, &kh_trust)
        .await
        .unwrap_err();
    assert!(
        matches!(err, AppError::HostKey { changed: true, .. }),
        "{err:?}"
    );
}

/// Regression: one unparsable `known_hosts` line, or a host name in another case, used to turn a
/// changed server key into an "unknown" key the user could accept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn known_hosts_quirks_cannot_downgrade_a_changed_key() {
    let echo = start_echo().await;
    let server = SshServer::start(random_key().public_key().clone()).await;
    let pinned_key = server.host_public_key();
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    let entry = |host: &str| {
        format!(
            "[{host}]:{} ssh-ed25519 {}",
            server.port(),
            pinned_key.public_key_base64()
        )
    };
    // A valid pin, the same pin with two spaces (OpenSSH accepts that) and a truncated RSA key.
    std::fs::write(
        &known_hosts,
        format!(
            "{}\n{}\n[{LOCALHOST}]:{} ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQC\n",
            entry(LOCALHOST),
            entry(LOCALHOST).replacen(' ', "  ", 1),
            server.port()
        ),
    )
    .unwrap();
    let appdb = AppDb::open_in_memory().unwrap();
    let pending = Mutex::new(HashMap::new());
    let profile_id = Uuid::new_v4();
    let trust =
        AppHostKeys::new(&appdb, &pending, profile_id).with_known_hosts(Some(known_hosts.clone()));
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);

    // The pinned key is trusted despite the odd lines.
    let tunnel = open(&cfg, password(PASSWORD), echo, &trust).await.unwrap();
    round_trip(tunnel.local_port(), 32).await;
    tunnel.close().await;

    server.rotate_host_key();
    let err = open(&cfg, password(PASSWORD), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(err, AppError::HostKey { changed: true, .. }),
        "{err:?}"
    );
    assert!(pending.lock().unwrap().is_empty());

    // Host names are case-insensitive: a pin for `localhost` applies to `LocalHost`.
    std::fs::write(&known_hosts, format!("{}\n", entry("localhost"))).unwrap();
    let mut mixed = cfg.clone();
    mixed.host = "LocalHost".into();
    let err = open(&mixed, password(PASSWORD), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::HostKey { changed: true, host, .. } if host == "localhost"),
        "{err:?}"
    );
    assert!(pending.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_goes_down_when_the_server_drops_the_session() {
    let echo = start_echo().await;
    let server = SshServer::start(random_key().public_key().clone()).await;
    let trust = trusting(&server);
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);
    let tunnel = open(&cfg, password(PASSWORD), echo, &trust).await.unwrap();
    let local_port = tunnel.local_port();
    round_trip(local_port, 16).await;

    let mut status = tunnel.subscribe();
    server.kick_all().await;
    let state = tokio::time::timeout(Duration::from_secs(10), status.wait_for(|s| !s.is_up()))
        .await
        .expect("tunnel status did not change")
        .expect("status channel closed")
        .clone();
    assert_ne!(state.status, TunnelStatus::Up);
    assert!(state.message.is_some());
    assert!(!tunnel.is_up());
    // The port stays reserved (connections closed at once) until the handle is closed.
    assert_reserved_and_closed(local_port).await;
    let explained = tunnel.explain_error(AppError::NotConnected, Instant::now());
    assert!(matches!(explained, AppError::Tunnel(_)), "{explained:?}");
    tunnel.close().await;
    assert_refused(local_port).await;
}

/// Regression: a dead tunnel used to free its local port while the connection's pools kept
/// dialing it, so another profile's tunnel (or any local process) binding the same port received
/// that connection's traffic and login.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dead_tunnel_keeps_its_pinned_port_reserved() {
    let echo = start_echo().await;
    let server_a = SshServer::start(random_key().public_key().clone()).await;
    let server_b = SshServer::start(random_key().public_key().clone()).await;
    let trust = MemoryHostKeys::new();
    for s in [&server_a, &server_b] {
        trust.trust(PendingHostKey::from_key(
            LOCALHOST,
            s.port(),
            &s.host_public_key(),
        ));
    }
    let pinned = free_port();
    let mut cfg_a = ssh_cfg(server_a.port(), SshAuth::Password, None);
    cfg_a.local_port = Some(pinned);
    let mut cfg_b = ssh_cfg(server_b.port(), SshAuth::Password, None);
    cfg_b.local_port = Some(pinned);

    let a = open(&cfg_a, password(PASSWORD), echo, &trust)
        .await
        .unwrap();
    assert_eq!(a.local_port(), pinned);
    round_trip(pinned, 64).await;
    server_a.kick_all().await;
    wait_down(&a).await;

    // Another tunnel cannot take the port while A's handle (its connection) is alive.
    let err = open(&cfg_b, password(PASSWORD), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Tunnel(m) if m.contains("cannot listen")),
        "{err:?}"
    );
    assert_reserved_and_closed(pinned).await;
    assert_eq!(server_b.forwards(), 0);

    // Closing A frees the port for B.
    a.close().await;
    let b = open(&cfg_b, password(PASSWORD), echo, &trust)
        .await
        .unwrap();
    assert_eq!(b.local_port(), pinned);
    round_trip(pinned, 64).await;
    assert_eq!(server_b.forwards(), 1);
    b.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreachable_target_reports_a_forwarding_error() {
    let server = SshServer::start(random_key().public_key().clone()).await;
    let trust = trusting(&server);
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);
    // A port nobody listens on.
    let closed = free_port();
    let tunnel = open(&cfg, password(PASSWORD), closed, &trust)
        .await
        .unwrap();
    let started = Instant::now();
    let mut stream = TcpStream::connect((LOCALHOST, tunnel.local_port()))
        .await
        .unwrap();
    let mut buf = [0u8; 8];
    let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
        .await
        .expect("forwarded stream was not closed")
        .unwrap_or(0);
    assert_eq!(n, 0);
    assert!(
        tunnel.is_up(),
        "a failed forward must not take the tunnel down"
    );
    let err = tunnel.explain_error(AppError::Connect("connection closed".into()), started);
    assert!(
        matches!(&err, AppError::Tunnel(m) if m.contains("could not open a connection")),
        "{err:?}"
    );

    // Regression: a forwarding failure from an earlier attempt must not explain a later,
    // unrelated error (e.g. a timeout of a switch_database long after a pool reconnect failed).
    let mut stream = TcpStream::connect((LOCALHOST, tunnel.local_port()))
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf)).await;
    let later = Instant::now();
    let err = tunnel.explain_error(AppError::Timeout, later);
    assert!(matches!(err, AppError::Timeout), "{err:?}");
    // ... and it was discarded, not kept for the next attempt either.
    let err = tunnel.explain_error(AppError::Timeout, started);
    assert!(matches!(err, AppError::Timeout), "{err:?}");
    tunnel.close().await;
}

/// MySQL "Use socket" over SSH: the tunnel forwards its local port to a Unix socket on the SSH
/// server, trying the usual places in turn when no path was typed.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socket_target_forwards_to_the_servers_unix_socket() {
    let server = SshServer::start(random_key().public_key().clone()).await;
    let trust = trusting(&server);
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("mysqld.sock");
    start_unix_echo(&sock);
    let missing = dir.path().join("none.sock").to_string_lossy().into_owned();
    let target = ForwardTarget::Socket(vec![missing.clone(), sock.to_string_lossy().into_owned()]);
    let tunnel = tokio::time::timeout(
        Duration::from_secs(20),
        open_tunnel_to(&cfg, password(PASSWORD), target, &trust),
    )
    .await
    .expect("open timed out")
    .unwrap();
    round_trip(tunnel.local_port(), 64 * 1024).await;
    // The path that opened is tried first from then on.
    let before = server.forwards();
    round_trip(tunnel.local_port(), 1024).await;
    assert_eq!(server.forwards(), before + 1);
    tunnel.close().await;

    // No socket there: the forward fails and says so.
    let target = ForwardTarget::Socket(vec![missing]);
    let tunnel = open_tunnel_to(&cfg, password(PASSWORD), target, &trust)
        .await
        .unwrap();
    let started = Instant::now();
    let mut stream = TcpStream::connect((LOCALHOST, tunnel.local_port()))
        .await
        .unwrap();
    let mut buf = [0u8; 8];
    let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
        .await
        .expect("forwarded stream was not closed")
        .unwrap_or(0);
    assert_eq!(n, 0);
    let err = tunnel.explain_error(AppError::Connect("connection closed".into()), started);
    assert!(
        matches!(&err, AppError::Tunnel(m) if m.contains("could not open the socket")),
        "{err:?}"
    );
    tunnel.close().await;
}

/// Another workspace of a profile leases the same tunnel: closing one lease leaves it up for the
/// other; the last one closes it and frees the port.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shared_tunnel_stays_up_until_its_last_lease_closes() {
    let echo = start_echo().await;
    let server = SshServer::start(random_key().public_key().clone()).await;
    let trust = trusting(&server);
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);
    let first = open(&cfg, password(PASSWORD), echo, &trust).await.unwrap();
    let second = first.share().expect("an up tunnel is shared");
    assert!(second.same_tunnel(&first));
    assert_eq!(
        (first.leases(), second.local_port()),
        (2, first.local_port())
    );
    let port = first.local_port();
    first.close().await;
    // Closing twice gives the lease back once.
    first.close().await;
    assert!(second.is_up());
    assert_eq!(second.leases(), 1);
    round_trip(port, 4096).await;
    // Dropping a lease (a connection that failed to open) keeps the others.
    let third = second.share().expect("still up");
    assert_eq!(second.leases(), 2);
    drop(third);
    assert_eq!(second.leases(), 1);
    assert!(second.is_up(), "dropping a lease keeps the others");
    second.close().await;
    assert!(!second.is_up());
    assert!(second.share().is_none());
    assert_refused(port).await;
}

/// Regression: with `AuthenticationMethods password,keyboard-interactive` the password was also
/// sent as the answer to the one-time-code prompt (handing it to the second-factor backend and
/// burning an attempt there), and the user was told that the correct password had failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_factor_prompt_is_not_answered_with_the_password() {
    let echo = start_echo().await;
    let server =
        SshServer::start_with(random_key().public_key().clone(), AuthMode::PasswordThenOtp).await;
    let trust = trusting(&server);
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);

    let err = open(&cfg, password(PASSWORD), echo, &trust)
        .await
        .unwrap_err();
    let AppError::Auth(msg) = &err else {
        panic!("expected Auth, got {err:?}");
    };
    assert!(msg.contains("second factor"), "{msg}");
    assert!(msg.contains("keyboard-interactive"), "{msg}");
    assert!(!msg.contains("failed"), "the password was accepted: {msg}");
    assert!(
        server.answers().is_empty(),
        "the password was sent to the second-factor prompt: {:?}",
        server.answers()
    );

    // A wrong password is still reported as such.
    let err = open(&cfg, password("wrong"), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.starts_with("ssh: password authentication failed")),
        "{err:?}"
    );
    assert!(server.answers().is_empty());
}

/// Servers that offer keyboard-interactive but not `password` still get the password as the
/// answer to their prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyboard_interactive_only_server_gets_the_password() {
    let echo = start_echo().await;
    let server = SshServer::start_with(
        random_key().public_key().clone(),
        AuthMode::KeyboardInteractiveOnly,
    )
    .await;
    let trust = trusting(&server);
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);

    let tunnel = open(&cfg, password(PASSWORD), echo, &trust).await.unwrap();
    round_trip(tunnel.local_port(), 32).await;
    tunnel.close().await;
    assert_eq!(server.answers(), [PASSWORD]);

    let err = open(&cfg, password("wrong"), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.contains("password authentication failed")),
        "{err:?}"
    );
}

/// Regression: a saved passphrase (left over from an earlier key, or typed by mistake) made an
/// unencrypted PEM / PKCS#8 key fail to load ("cannot decrypt the private key"), so connect and
/// Test connection failed for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_passphrase_does_not_break_an_unencrypted_key() {
    let echo = start_echo().await;
    let client_key = random_key();
    let server = SshServer::start(client_key.public_key().clone()).await;
    let trust = trusting(&server);
    let dir = tempfile::tempdir().unwrap();

    for key_file in [
        write_pkcs8_key(dir.path(), "id_pkcs8", &client_key, None),
        write_key(dir.path(), "id_openssh", &client_key, None),
    ] {
        let cfg = ssh_cfg(server.port(), SshAuth::Key, Some(&key_file));
        let tunnel = open(&cfg, passphrase(Some("old passphrase")), echo, &trust)
            .await
            .unwrap_or_else(|e| panic!("{}: {e:?}", key_file.display()));
        round_trip(tunnel.local_port(), 256).await;
        tunnel.close().await;
    }

    // An encrypted PKCS#8 key still needs its passphrase, and the right one.
    let encrypted = write_pkcs8_key(dir.path(), "id_pkcs8_enc", &client_key, Some(PASSPHRASE));
    let cfg = ssh_cfg(server.port(), SshAuth::Key, Some(&encrypted));
    let err = open(&cfg, passphrase(None), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.contains("is encrypted")),
        "{err:?}"
    );
    let err = open(&cfg, passphrase(Some("nope")), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.contains("wrong passphrase")),
        "{err:?}"
    );
    let tunnel = open(&cfg, passphrase(Some(PASSPHRASE)), echo, &trust)
        .await
        .unwrap();
    round_trip(tunnel.local_port(), 256).await;
    tunnel.close().await;

    // Garbage is reported as an unreadable key, whatever the passphrase.
    let garbage = dir.path().join("id_garbage");
    std::fs::write(&garbage, "not a key\n").unwrap();
    let cfg = ssh_cfg(server.port(), SshAuth::Key, Some(&garbage));
    let err = open(&cfg, passphrase(Some("x")), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Tunnel(m) if m.contains("cannot load")),
        "{err:?}"
    );
}

/// Regression: a signature the agent refused (a dismissed confirmation prompt) was reported as
/// "the server offers no other method", and with two keys in the agent the login hung (russh
/// keeps waiting for the refused signature).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_signing_refusal_is_reported() {
    use russh::keys::agent::client::AgentClient;

    let echo = start_echo().await;
    let client_key = random_key();
    let server = SshServer::start(client_key.public_key().clone()).await;
    let trust = trusting(&server);
    let dir = tempfile::tempdir().unwrap();
    let socket = start_agent_with(dir.path(), "refusing.sock", RefusingAgent).await;
    let mut agent = AgentClient::connect_uds(&socket).await.unwrap();
    agent.add_identity(&random_key(), &[]).await.unwrap();
    agent.add_identity(&client_key, &[]).await.unwrap();
    let cfg = ssh_cfg(server.port(), SshAuth::Agent, None);

    let started = Instant::now();
    let err = open(&cfg, via_agent(&socket), echo, &trust)
        .await
        .unwrap_err();
    let AppError::Auth(msg) = &err else {
        panic!("expected Auth, got {err:?}");
    };
    assert!(msg.contains("did not sign"), "{msg}");
    assert!(!msg.contains("no other method"), "{msg}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );
}

/// Regression: a connection that dropped during authentication was reported as an
/// authentication failure ("the server offers no other method") instead of a tunnel error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_dropped_during_authentication_is_a_tunnel_error() {
    let echo = start_echo().await;
    let client_key = random_key();
    let server = SshServer::start_with(client_key.public_key().clone(), AuthMode::DropOnAuth).await;
    let trust = trusting(&server);
    let dir = tempfile::tempdir().unwrap();

    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);
    let err = open(&cfg, password(PASSWORD), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Tunnel(m) if m.contains("ended during password authentication")),
        "{err:?}"
    );

    let key_file = write_key(dir.path(), "id_plain", &client_key, None);
    let cfg = ssh_cfg(server.port(), SshAuth::Key, Some(&key_file));
    let err = open(&cfg, passphrase(None), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Tunnel(m) if m.contains("ended during public key authentication")),
        "{err:?}"
    );

    #[cfg(unix)]
    {
        use russh::keys::agent::client::AgentClient;
        let socket = start_agent(dir.path()).await;
        let mut agent = AgentClient::connect_uds(&socket).await.unwrap();
        agent.add_identity(&client_key, &[]).await.unwrap();
        let cfg = ssh_cfg(server.port(), SshAuth::Agent, None);
        let err = open(&cfg, via_agent(&socket), echo, &trust)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, AppError::Tunnel(m) if m.contains("ended during ssh-agent authentication")),
            "{err:?}"
        );
    }
}

/// A local service listening on the wildcard address of a port, like Docker Desktop publishing a
/// database on `0.0.0.0:5432`. On macOS it runs in the system Python (Apple-signed): a test
/// binary that listens on 0.0.0.0 itself would make the application firewall ask for permission.
struct WildcardService {
    port: u16,
    _listener: Option<std::net::TcpListener>,
    child: Option<std::process::Child>,
}

impl Drop for WildcardService {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Listens on 0.0.0.0 with SO_REUSEADDR (as Docker, Go and most servers do), prints the port and
/// closes every connection it accepts.
const WILDCARD_SERVICE_PY: &str = "import socket
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('0.0.0.0', 0))
s.listen(64)
print(s.getsockname()[1], flush=True)
while True:
    s.accept()[0].close()
";

fn wildcard_service() -> Option<WildcardService> {
    use std::io::BufRead;

    if !cfg!(target_os = "macos") {
        let listener = std::net::TcpListener::bind(("0.0.0.0", 0)).ok()?;
        return Some(WildcardService {
            port: listener.local_addr().ok()?.port(),
            _listener: Some(listener),
            child: None,
        });
    }
    let mut child = std::process::Command::new("/usr/bin/python3")
        .args(["-I", "-c", WILDCARD_SERVICE_PY])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let mut service = WildcardService {
        port: 0,
        _listener: None,
        child: Some(child),
    };
    let mut line = String::new();
    std::io::BufReader::new(stdout).read_line(&mut line).ok()?;
    service.port = line.trim().parse().ok()?;
    Some(service)
}

/// Regression: on macOS a pinned local port was bound with SO_REUSEADDR, which let the tunnel
/// bind 127.0.0.1:P while another program listened on *:P and then take over all loopback
/// traffic to that port (psql and other profiles silently reached the tunnel's remote database).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pinned_port_is_not_taken_from_a_wildcard_listener() {
    let Some(service) = wildcard_service() else {
        println!("skipped: could not start a listener on 0.0.0.0");
        return;
    };
    let echo = start_echo().await;
    let server = SshServer::start(random_key().public_key().clone()).await;
    let trust = trusting(&server);
    let mut cfg = ssh_cfg(server.port(), SshAuth::Password, None);
    cfg.local_port = Some(service.port);

    let err = open(&cfg, password(PASSWORD), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Tunnel(m) if m.contains("cannot listen") && m.contains("in use")),
        "{err:?}"
    );
    // Loopback clients still reach the local service, never the SSH server.
    let mut stream = TcpStream::connect((LOCALHOST, service.port))
        .await
        .expect("the local service is unreachable");
    let _ = stream.write_all(b"ping").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(server.forwards(), 0, "a loopback client reached the tunnel");
}

/// The pinned port is bound without SO_REUSEADDR now; connections of the previous tunnel that
/// are still closing down (TIME_WAIT on the tunnel's side) must not keep the next tunnel (an
/// immediate reconnect) from binding it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pinned_port_is_reusable_right_after_its_tunnel_closed() {
    let echo = start_echo().await;
    let server = SshServer::start(random_key().public_key().clone()).await;
    let trust = trusting(&server);
    let pinned = free_port();
    let mut cfg = ssh_cfg(server.port(), SshAuth::Password, None);
    cfg.local_port = Some(pinned);

    let a = open(&cfg, password(PASSWORD), echo, &trust).await.unwrap();
    assert_eq!(a.local_port(), pinned);
    // A forwarded connection open while the tunnel closes: the tunnel closes its end first.
    let mut stream = TcpStream::connect((LOCALHOST, pinned)).await.unwrap();
    stream.write_all(b"x").await.unwrap();
    let mut buf = [0u8; 8];
    let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
        .await
        .expect("no echo")
        .unwrap();
    assert_eq!(n, 1);
    a.close().await;
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .expect("the closed tunnel kept the connection open")
        .unwrap_or(0);
    assert_eq!(n, 0);
    drop(stream);

    let b = open(&cfg, password(PASSWORD), echo, &trust).await.unwrap();
    assert_eq!(b.local_port(), pinned);
    round_trip(pinned, 64).await;
    b.close().await;
}

// ----- commands-level helpers -------------------------------------------------------------------

/// App state with an in-memory app database and file-backed secrets (no Keychain).
fn test_state() -> AppState {
    let appdb = AppDb::open_in_memory().unwrap();
    AppState::new(appdb.clone(), Secrets::new(Backend::File, appdb))
}

fn trust_in_appdb(state: &AppState, server: &SshServer) {
    trust_host_key(
        &state.appdb,
        &PendingHostKey::from_key(LOCALHOST, server.port(), &server.host_public_key()),
    )
    .unwrap();
}

/// `profile` reached through `server`'s tunnel on the pinned local port `pinned`.
fn tunneled_profile(
    mut profile: ConnectionProfile,
    server: &SshServer,
    pinned: u16,
) -> ConnectionProfile {
    let mut ssh = ssh_cfg(server.port(), SshAuth::Password, None);
    ssh.local_port = Some(pinned);
    profile.ssh = Some(ssh);
    profile
}

/// Regression: "Test connection" bound the profile's pinned local port, so it always failed
/// ("address already in use") while the same profile was connected, or when two tests overlapped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_connection_does_not_need_the_pinned_local_port() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let server = SshServer::start(random_key().public_key().clone()).await;
    let state = test_state();
    trust_in_appdb(&state, &server);
    let pinned = free_port();
    // The real database when configured; otherwise a closed port, so the probe fails *through*
    // the tunnel (which proves the temporary tunnel itself came up).
    let real_db = std::env::var("TABLER_TEST_PG_URL").ok();
    let (base, db_password) = match &real_db {
        Some(url) => profile_from_url(Driver::Postgres, url),
        None => {
            println!("note: TABLER_TEST_PG_URL unset; probing a closed port through the tunnel");
            let p = profile(
                Driver::Postgres,
                LOCALHOST.into(),
                Some(free_port()),
                Some("tabler".into()),
                Some("tabler".into()),
            );
            (p, None)
        }
    };
    let profile = tunneled_profile(base, &server, pinned);

    // The profile is connected: its tunnel holds the pinned port.
    let connected = open_profile_tunnel(&state, &profile, password(PASSWORD))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(connected.local_port(), pinned);

    let input = || ProfileInput {
        profile: profile.clone(),
        password: db_password.clone(),
        ssh_password: Some(PASSWORD.into()),
        ssh_passphrase: None,
        clear_password: false,
    };
    let (first, second) = tokio::join!(
        run_connection_test(&state, input()),
        run_connection_test(&state, input())
    );
    for result in [first, second] {
        let result = result.expect("test_connection");
        assert!(result.tunnel_used);
        assert!(
            !result.message.contains("cannot listen"),
            "{}",
            result.message
        );
        if real_db.is_some() {
            assert!(result.ok, "{}", result.message);
        } else {
            assert!(!result.ok);
            assert!(
                result.message.contains("could not open a connection"),
                "{}",
                result.message
            );
        }
    }
    // The connected tunnel is untouched and still owns its pinned port.
    assert!(connected.is_up());
    assert!(std::net::TcpListener::bind((LOCALHOST, pinned)).is_err());
    connected.close().await;
}

/// Regression (commands level): once a connection's tunnel dies, its PostgreSQL pool keeps
/// dialing the tunnel's local port. That port must never lead to another profile's tunnel.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_tunnel_session_never_reaches_another_tunnel() {
    let Ok(url) = std::env::var("TABLER_TEST_PG_URL") else {
        println!("skipped: TABLER_TEST_PG_URL unset");
        return;
    };
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let (base, db_password) = profile_from_url(Driver::Postgres, &url);
    let server_a = SshServer::start(random_key().public_key().clone()).await;
    let server_b = SshServer::start(random_key().public_key().clone()).await;
    let state = test_state();
    trust_in_appdb(&state, &server_a);
    trust_in_appdb(&state, &server_b);
    let pinned = free_port();
    let profile_a = tunneled_profile(base.clone(), &server_a, pinned);
    let mut profile_b = tunneled_profile(base, &server_b, pinned);
    profile_b.id = Uuid::new_v4();
    const SQL: &str = "SELECT 1::text AS one";
    let query = |s: Arc<dyn tabler_lib::drivers::DbSession>| async move {
        tokio::time::timeout(Duration::from_secs(20), s.query(SQL, &[]))
            .await
            .expect("query timed out")
    };

    // "connect(A)": tunnel + session, registered like the command does.
    let tunnel_a = open_profile_tunnel(&state, &profile_a, password(PASSWORD))
        .await
        .unwrap()
        .unwrap();
    let session = connect(ConnectTarget {
        profile: &profile_a,
        password: db_password.as_deref(),
        host_override: Some(tunnel_a.connect_addr()),
    })
    .await
    .expect("connect through tunnel A");
    query(session.clone()).await.expect("query through A");
    let info = connection_info(&profile_a, pinned);
    let conn = Arc::new(Connection {
        id: info.connection_id,
        profile: profile_a.clone(),
        session: RwLock::new(session.clone()),
        info: RwLock::new(info),
        schema_cache: RwLock::new(HashMap::new()),
        status: watch::channel(ConnStatus::Connected).0,
        tunnel: Some(tunnel_a),
    });
    state
        .connections
        .write()
        .await
        .insert(conn.id, conn.clone());
    let forwards_a = server_a.forwards();

    server_a.kick_all().await;
    wait_down(conn.tunnel.as_ref().unwrap()).await;

    // B cannot take the port while A's connection holds it, and A's pool reaches nothing.
    let err = open_profile_tunnel(&state, &profile_b, password(PASSWORD))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Tunnel(m) if m.contains("cannot listen")),
        "{err:?}"
    );
    assert!(query(session.clone()).await.is_err());
    assert_eq!(server_a.forwards(), forwards_a);

    // connect() for a profile pinning that port releases it: A's session closes first.
    release_pinned_port(&state, profile_b.id, pinned).await;
    let tunnel_b = open_profile_tunnel(&state, &profile_b, password(PASSWORD))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tunnel_b.local_port(), pinned);
    let err = query(session.clone()).await.unwrap_err();
    assert!(matches!(err, AppError::NotConnected), "{err:?}");
    assert_eq!(server_b.forwards(), 0, "A's session reached B's tunnel");

    // B works on its own.
    let session_b = connect(ConnectTarget {
        profile: &profile_b,
        password: db_password.as_deref(),
        host_override: Some(tunnel_b.connect_addr()),
    })
    .await
    .expect("connect through tunnel B");
    query(session_b.clone()).await.expect("query through B");
    assert!(server_b.forwards() >= 1);
    session_b.close().await;
    tunnel_b.close().await;
    state.connections.write().await.remove(&conn.id);
}

/// `ConnectionInfo` of a test connection through a tunnel on `local_port`.
fn connection_info(profile: &ConnectionProfile, local_port: u16) -> ConnectionInfo {
    ConnectionInfo {
        connection_id: Uuid::new_v4(),
        profile_id: profile.id,
        name: profile.name.clone(),
        color: None,
        driver: profile.driver,
        server_version: String::new(),
        current_database: None,
        current_user: None,
        tunnel: Some(TunnelInfo { local_port }),
        read_only: false,
    }
}

/// Registers an open connection of `profile` like `connect` does: its tunnel, and a database
/// session (a SQLite file in `dir` stands in for the remote database).
async fn register_connection(
    state: &AppState,
    profile: &ConnectionProfile,
    dir: &Path,
) -> Arc<Connection> {
    let tunnel = open_profile_tunnel(state, profile, password(PASSWORD))
        .await
        .unwrap()
        .unwrap();
    let mut lite = profile.clone();
    lite.driver = Driver::Sqlite;
    lite.ssh = None;
    lite.sqlite_path = Some(
        dir.join(format!("tabler_{}.sqlite", Uuid::new_v4().simple()))
            .display()
            .to_string(),
    );
    let session = connect(ConnectTarget {
        profile: &lite,
        password: None,
        host_override: None,
    })
    .await
    .expect("open the stand-in session");
    let info = connection_info(profile, tunnel.local_port());
    let conn = Arc::new(Connection {
        id: info.connection_id,
        profile: profile.clone(),
        session: RwLock::new(session),
        info: RwLock::new(info),
        schema_cache: RwLock::new(HashMap::new()),
        status: watch::channel(ConnStatus::Connected).0,
        tunnel: Some(tunnel),
    });
    state
        .connections
        .write()
        .await
        .insert(conn.id, conn.clone());
    conn
}

/// A second workspace of a profile that pins its local port (another database of the server)
/// leases the open connection's tunnel instead of binding the port again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_workspace_shares_the_profiles_pinned_tunnel() {
    let server = SshServer::start(random_key().public_key().clone()).await;
    let state = test_state();
    trust_in_appdb(&state, &server);
    let dir = tempfile::tempdir().unwrap();
    let pinned = free_port();
    let base = profile(
        Driver::Postgres,
        LOCALHOST.into(),
        Some(free_port()),
        Some("tabler".into()),
        Some("tabler".into()),
    );
    let profile_a = tunneled_profile(base, &server, pinned);
    let conn = register_connection(&state, &profile_a, dir.path()).await;
    let mut other_db = profile_a.clone();
    other_db.database = Some("analytics".into());
    let shared = state.shared_tunnel(&other_db).await.expect("shared");
    assert_eq!(shared.local_port(), pinned);
    assert!(shared.same_tunnel(conn.tunnel.as_ref().unwrap()));
    // Another profile (or edited SSH settings) gets no lease of it.
    let mut profile_b = profile_a.clone();
    profile_b.id = Uuid::new_v4();
    assert!(state.shared_tunnel(&profile_b).await.is_none());
    // The first workspace closes: the second keeps the tunnel.
    conn.tunnel.as_ref().unwrap().close().await;
    assert!(shared.is_up());
    shared.close().await;
    assert!(!conn.tunnel.as_ref().unwrap().is_up());
    assert!(state.shared_tunnel(&other_db).await.is_none());
    conn.session().await.close().await;
}

/// Regression: Reconnect (connect the profile again, then disconnect the old connection) always
/// failed with "cannot listen" for a profile with a pinned local port when the database session
/// was lost while the SSH tunnel stayed up: the old connection's live tunnel kept the port.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnect_releases_the_pinned_port_of_a_lost_session() {
    let server = SshServer::start(random_key().public_key().clone()).await;
    let state = test_state();
    trust_in_appdb(&state, &server);
    let dir = tempfile::tempdir().unwrap();
    let pinned = free_port();
    let base = profile(
        Driver::Postgres,
        LOCALHOST.into(),
        Some(free_port()),
        Some("tabler".into()),
        Some("tabler".into()),
    );
    let profile_a = tunneled_profile(base, &server, pinned);
    let still_held = |err: AppError| {
        assert!(
            matches!(&err, AppError::Tunnel(m) if m.contains("cannot listen")),
            "{err:?}"
        );
    };

    // A healthy connection keeps its port.
    let conn = register_connection(&state, &profile_a, dir.path()).await;
    release_pinned_port(&state, profile_a.id, pinned).await;
    assert!(conn.tunnel.as_ref().unwrap().is_up());
    assert_eq!(*conn.status.borrow(), ConnStatus::Connected);
    still_held(
        open_profile_tunnel(&state, &profile_a, password(PASSWORD))
            .await
            .unwrap_err(),
    );

    // Lost (database side), tunnel up: another profile pinning the port does not take it over.
    conn.status.send_replace(ConnStatus::Lost);
    let mut profile_b = profile_a.clone();
    profile_b.id = Uuid::new_v4();
    release_pinned_port(&state, profile_b.id, pinned).await;
    assert!(conn.tunnel.as_ref().unwrap().is_up());
    still_held(
        open_profile_tunnel(&state, &profile_b, password(PASSWORD))
            .await
            .unwrap_err(),
    );

    // The same profile reconnecting replaces it: session and tunnel closed, port free.
    release_pinned_port(&state, profile_a.id, pinned).await;
    assert!(!conn.tunnel.as_ref().unwrap().is_up());
    assert_eq!(*conn.status.borrow(), ConnStatus::Disconnected);
    assert!(conn.session().await.ping().await.is_err());
    let fresh = open_profile_tunnel(&state, &profile_a, password(PASSWORD))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fresh.local_port(), pinned);
    fresh.close().await;
    state.connections.write().await.remove(&conn.id);

    // A session that died while the connection still counts as connected (a ping that hung or
    // has not run yet): the health check finds out.
    let conn = register_connection(&state, &profile_a, dir.path()).await;
    conn.session().await.close().await;
    release_pinned_port(&state, profile_a.id, pinned).await;
    assert!(!conn.tunnel.as_ref().unwrap().is_up());
    let fresh = open_profile_tunnel(&state, &profile_a, password(PASSWORD))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fresh.local_port(), pinned);
    fresh.close().await;
    state.connections.write().await.remove(&conn.id);
}

/// Regression: when the connect timeout fired during the key exchange, the session task russh
/// spawned for the handshake kept the SSH server's socket open forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_timeout_closes_the_ssh_socket() {
    // A server that sends its banner, then never answers the key exchange.
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket.write_all(b"SSH-2.0-OpenSSH_9.6\r\n").await.unwrap();
        let mut buf = [0u8; 4096];
        while let Ok(n) = socket.read(&mut buf).await {
            if n == 0 {
                break;
            }
        }
        let _ = closed_tx.send(());
    });
    let state = test_state();
    let mut stalled = profile(
        Driver::Postgres,
        LOCALHOST.into(),
        Some(5432),
        Some("tabler".into()),
        None,
    );
    stalled.options.connect_timeout_secs = Some(1);
    stalled.ssh = Some(ssh_cfg(port, SshAuth::Password, None));

    let err = open_profile_tunnel(&state, &stalled, password(PASSWORD))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Tunnel(m) if m.contains("timed out")),
        "{err:?}"
    );
    tokio::time::timeout(Duration::from_secs(5), closed_rx)
        .await
        .expect("the SSH socket stayed open after the connect timeout")
        .unwrap();
}

/// Regression: a saved SSH key passphrase could not be removed (an empty field meant
/// "unchanged"), and with it saved, an unencrypted PEM / PKCS#8 key failed in Test connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_ssh_secrets_can_be_cleared() {
    let client_key = random_key();
    let server = SshServer::start(client_key.public_key().clone()).await;
    let state = test_state();
    trust_in_appdb(&state, &server);
    let dir = tempfile::tempdir().unwrap();
    let key_file = write_pkcs8_key(dir.path(), "id_pkcs8", &client_key, None);
    let mut keyed = profile(
        Driver::Postgres,
        LOCALHOST.into(),
        Some(free_port()),
        Some("tabler".into()),
        Some("tabler".into()),
    );
    keyed.ssh = Some(ssh_cfg(server.port(), SshAuth::Key, Some(&key_file)));
    let id = keyed.id;
    state
        .secrets
        .set(id, SecretKind::SshPassphrase, "old passphrase")
        .await
        .unwrap();
    state
        .secrets
        .set(id, SecretKind::SshPassword, "old password")
        .await
        .unwrap();
    let input = |ssh_password: Option<&str>, ssh_passphrase: Option<&str>| ProfileInput {
        profile: keyed.clone(),
        password: None,
        ssh_password: ssh_password.map(str::to_string),
        ssh_passphrase: ssh_passphrase.map(str::to_string),
        clear_password: false,
    };

    // The stale passphrase does not stop the unencrypted key: the tunnel comes up and only the
    // (closed) database port fails.
    let result = run_connection_test(&state, input(None, None))
        .await
        .unwrap();
    assert!(result.tunnel_used);
    assert!(
        result.message.contains("could not open a connection"),
        "{}",
        result.message
    );

    // In the dialog, an empty field (`""`) means "no passphrase", null means "the saved one".
    let typed = resolve_tunnel_auth(&state, &input(None, Some("")))
        .await
        .unwrap();
    assert_eq!(typed.passphrase, None);
    let saved = resolve_tunnel_auth(&state, &input(None, None))
        .await
        .unwrap();
    assert_eq!(saved.passphrase.as_deref(), Some("old passphrase"));

    // Saving with null keeps the secrets, with "" removes them.
    store_secrets(&state, &input(None, None)).await.unwrap();
    assert!(
        state
            .secrets
            .get(id, SecretKind::SshPassphrase)
            .await
            .unwrap()
            .is_some()
    );
    store_secrets(&state, &input(Some(""), Some("")))
        .await
        .unwrap();
    assert_eq!(
        state
            .secrets
            .get(id, SecretKind::SshPassphrase)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        state
            .secrets
            .get(id, SecretKind::SshPassword)
            .await
            .unwrap(),
        None
    );
    store_secrets(&state, &input(None, Some("new passphrase")))
        .await
        .unwrap();
    assert_eq!(
        state
            .secrets
            .get(id, SecretKind::SshPassphrase)
            .await
            .unwrap()
            .as_deref(),
        Some("new passphrase")
    );
}

// ----- real databases through the tunnel (optional) --------------------------------------------

fn profile(
    driver: Driver,
    host: String,
    port: Option<u16>,
    user: Option<String>,
    db: Option<String>,
) -> ConnectionProfile {
    ConnectionProfile {
        id: Uuid::new_v4(),
        name: "ssh-tunnel-test".into(),
        color: None,
        group: None,
        driver,
        host: Some(host),
        port,
        user,
        database: db,
        ssl: if driver == Driver::Postgres {
            SslMode::Prefer
        } else {
            SslMode::Disable
        },
        ssl_ca_path: None,
        sqlite_path: None,
        ssh: None,
        options: ProfileOptions {
            connect_timeout_secs: Some(10),
            ..ProfileOptions::default()
        },
        sort_order: 0,
        created_at: String::new(),
        updated_at: String::new(),
        last_connected_at: None,
    }
}

/// Parses `scheme://user[:pass]@host[:port]/db`.
fn profile_from_url(driver: Driver, url: &str) -> (ConnectionProfile, Option<String>) {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let (auth, hostdb) = match rest.rsplit_once('@') {
        Some((a, h)) => (Some(a), h),
        None => (None, rest),
    };
    let (user, pass) = match auth {
        Some(a) => match a.split_once(':') {
            Some((u, p)) => (Some(u.to_string()), Some(p.to_string())),
            None => (Some(a.to_string()), None),
        },
        None => (None, None),
    };
    let (hostport, db) = match hostdb.split_once('/') {
        Some((h, d)) => (h, Some(d.split('?').next().unwrap_or("").to_string())),
        None => (hostdb, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (hostport.to_string(), None),
    };
    (
        profile(driver, host, port, user, db.filter(|d| !d.is_empty())),
        pass,
    )
}

async fn select_one_through_tunnel(driver: Driver, env: &str, sql: &str) {
    let Ok(url) = std::env::var(env) else {
        println!("skipped: {env} unset");
        return;
    };
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let (profile, db_password) = profile_from_url(driver, &url);
    let server = SshServer::start(random_key().public_key().clone()).await;
    let trust = trusting(&server);
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);
    let target_host = profile.host.clone().unwrap();
    let target_port = profile.port.or_else(|| driver.default_port()).unwrap();
    let tunnel = tokio::time::timeout(
        Duration::from_secs(20),
        open_tunnel(&cfg, password(PASSWORD), target_host, target_port, &trust),
    )
    .await
    .unwrap()
    .unwrap();
    let session = connect(ConnectTarget {
        profile: &profile,
        password: db_password.as_deref(),
        host_override: Some(tunnel.connect_addr()),
    })
    .await
    .expect("connect through tunnel");
    let rs = session.query(sql, &[]).await.expect("SELECT 1");
    assert_eq!(rs.rows.len(), 1);
    let one = &rs.rows[0][0];
    assert!(
        matches!(one, Value::Int(1)) || matches!(one, Value::Text(s) if s == "1"),
        "{one:?}"
    );
    let info = session.server_info().await.unwrap();
    assert!(!info.version.is_empty());
    session.close().await;
    tunnel.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_through_tunnel() {
    select_one_through_tunnel(
        Driver::Postgres,
        "TABLER_TEST_PG_URL",
        "SELECT 1::text AS one",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_through_tunnel() {
    select_one_through_tunnel(Driver::Mysql, "TABLER_TEST_MYSQL_URL", "SELECT 1 AS one").await;
}

// ----- keyboard-interactive (2FA) questions -----------------------------------------------------

/// The user at the 2FA dialog: answers every question with `code` (`None`: cancels), and
/// remembers what was asked.
struct TypingUser {
    code: Option<&'static str>,
    asked: Mutex<Vec<tabler_lib::tunnel::PromptRound>>,
}

impl TypingUser {
    fn new(code: Option<&'static str>) -> Arc<TypingUser> {
        Arc::new(TypingUser {
            code,
            asked: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait::async_trait]
impl tabler_lib::tunnel::KeyboardInteractive for TypingUser {
    async fn answer(
        &self,
        round: tabler_lib::tunnel::PromptRound,
    ) -> Result<Vec<String>, AppError> {
        let n = round.prompts.len();
        self.asked.lock().unwrap().push(round);
        match self.code {
            Some(code) => Ok(vec![code.to_string(); n]),
            None => Err(AppError::Cancelled),
        }
    }
}

fn with_user(auth: TunnelAuth, user: &Arc<TypingUser>) -> TunnelAuth {
    TunnelAuth {
        prompts: Some(user.clone()),
        ..auth
    }
}

/// `AuthenticationMethods password,keyboard-interactive`: the saved password is the first factor,
/// the one-time code is asked from the user (never answered with the password).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_factor_code_is_asked_from_the_user() {
    let echo = start_echo().await;
    let server =
        SshServer::start_with(random_key().public_key().clone(), AuthMode::PasswordThenOtp).await;
    let trust = trusting(&server);
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);

    let user = TypingUser::new(Some(OTP));
    let tunnel = open(&cfg, with_user(password(PASSWORD), &user), echo, &trust)
        .await
        .unwrap();
    round_trip(tunnel.local_port(), 32).await;
    tunnel.close().await;
    assert_eq!(server.answers(), [OTP]);
    {
        let asked = user.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        let round = &asked[0];
        assert_eq!(
            (round.user.as_str(), round.host.as_str(), round.port),
            (USER, LOCALHOST, server.port())
        );
        assert_eq!(round.prompts.len(), 1);
        assert_eq!(round.prompts[0].prompt, "Verification code: ");
        assert!(!round.prompts[0].echo);
    }

    // A wrong code is a keyboard-interactive failure.
    let wrong = TypingUser::new(Some("000000"));
    let err = open(&cfg, with_user(password(PASSWORD), &wrong), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.contains("keyboard-interactive authentication failed")),
        "{err:?}"
    );

    // Cancelling the question gives the connect up.
    let cancel = TypingUser::new(None);
    let err = open(&cfg, with_user(password(PASSWORD), &cancel), echo, &trust)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");

    // A wrong password never reaches the second factor.
    let never = TypingUser::new(Some(OTP));
    let err = open(&cfg, with_user(password("wrong"), &never), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.starts_with("ssh: password authentication failed")),
        "{err:?}"
    );
    assert!(never.asked.lock().unwrap().is_empty());
}

/// A keyboard-interactive-only server still gets the saved password for its password question
/// without bothering the user.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn password_question_is_answered_without_asking_the_user() {
    let echo = start_echo().await;
    let server = SshServer::start_with(
        random_key().public_key().clone(),
        AuthMode::KeyboardInteractiveOnly,
    )
    .await;
    let trust = trusting(&server);
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);
    let user = TypingUser::new(Some("not-asked"));
    let tunnel = open(&cfg, with_user(password(PASSWORD), &user), echo, &trust)
        .await
        .unwrap();
    tunnel.close().await;
    assert_eq!(server.answers(), [PASSWORD]);
    assert!(user.asked.lock().unwrap().is_empty());
}

/// Regression: Password authentication without a saved password failed up front ("no SSH
/// password is saved") although a keyboard-interactive server asks for the password itself: its
/// question now goes to the user.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_saved_password_the_user_answers_the_password_question() {
    let echo = start_echo().await;
    let server = SshServer::start_with(
        random_key().public_key().clone(),
        AuthMode::KeyboardInteractiveOnly,
    )
    .await;
    let trust = trusting(&server);
    let cfg = ssh_cfg(server.port(), SshAuth::Password, None);
    let user = TypingUser::new(Some(PASSWORD));
    let tunnel = open(&cfg, with_user(TunnelAuth::default(), &user), echo, &trust)
        .await
        .unwrap();
    round_trip(tunnel.local_port(), 32).await;
    tunnel.close().await;
    assert_eq!(server.answers(), [PASSWORD]);
    assert_eq!(user.asked.lock().unwrap().len(), 1);

    // A wrong answer is reported as such.
    let wrong = TypingUser::new(Some("nope"));
    let err = open(&cfg, with_user(TunnelAuth::default(), &wrong), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.contains("keyboard-interactive authentication failed")),
        "{err:?}"
    );

    // Nobody to ask (a backup's tunnel): refused up front, as before.
    let err = open(&cfg, TunnelAuth::default(), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.contains("no SSH password is saved")),
        "{err:?}"
    );

    // A server that takes the password only as such needs it saved; nobody is asked.
    let plain = SshServer::start(random_key().public_key().clone()).await;
    let trust = trusting(&plain);
    let cfg = ssh_cfg(plain.port(), SshAuth::Password, None);
    let never = TypingUser::new(Some(PASSWORD));
    let err = open(&cfg, with_user(TunnelAuth::default(), &never), echo, &trust)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Auth(m) if m.contains("no SSH password is saved") && m.contains("accepts: password")),
        "{err:?}"
    );
    assert!(never.asked.lock().unwrap().is_empty());
}

/// The app path: the question goes out as an event, the answer comes back through the state's
/// registry (`answer_ssh_prompt`), and the time spent answering does not count against the
/// profile's connect timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answering_through_the_registry_pauses_the_connect_timeout() {
    use tabler_lib::tunnel::{ForwardedPrompts, PromptEvent, SshPromptPurpose};

    let server =
        SshServer::start_with(random_key().public_key().clone(), AuthMode::PasswordThenOtp).await;
    let state = test_state();
    trust_in_appdb(&state, &server);
    let mut profile = tunneled_profile(
        profile(
            Driver::Postgres,
            LOCALHOST.into(),
            Some(5432),
            Some("tabler".into()),
            None,
        ),
        &server,
        0,
    );
    if let Some(ssh) = profile.ssh.as_mut() {
        ssh.local_port = None;
    }
    profile.options.connect_timeout_secs = Some(1);

    let events = Arc::new(Mutex::new(Vec::new()));
    let pending = state.ssh_prompts.clone();
    let log = events.clone();
    let prompts = ForwardedPrompts::new(
        state.ssh_prompts.clone(),
        move |event| match event {
            PromptEvent::Ask(req) => {
                log.lock()
                    .unwrap()
                    .push(format!("ask {}", req.prompts[0].prompt));
                let pending = pending.clone();
                // The user takes longer than the whole connect timeout to type the code.
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    assert!(pending.answer(&req.request_id, Some(vec![OTP.into()])));
                });
            }
            PromptEvent::Done(_) => log.lock().unwrap().push("done".into()),
        },
        profile.id,
        profile.name.clone(),
        SshPromptPurpose::Connect,
    );
    let auth = TunnelAuth {
        prompts: Some(Arc::new(prompts)),
        ..password(PASSWORD)
    };
    let tunnel = open_profile_tunnel(&state, &profile, auth)
        .await
        .unwrap()
        .unwrap();
    tunnel.close().await;
    assert_eq!(server.answers(), [OTP]);
    assert_eq!(*events.lock().unwrap(), ["ask Verification code: ", "done"]);
    assert!(state.ssh_prompts.is_empty());
}
