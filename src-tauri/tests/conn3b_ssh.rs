//! SSH extras against in-process russh servers: `~/.ssh/config` ProxyCommand and ProxyJump (one
//! session per hop), the "Ask every time" SSH password, and SQLite over SSH (SFTP download,
//! upload with the remote-change check). `HOME` points at a temporary folder for every test, so
//! the user's own `~/.ssh/config` never interferes.

// Same allowance as the library: `AppError` carries full server diagnostics.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use russh::keys::ssh_key::LineEnding;
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, ChannelOpenFailure, MethodKind, MethodSet};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use tabler_lib::drivers::sqlite::cipher;
use tabler_lib::drivers::sqlite::{Cipher, HeaderKind, header_kind};
use tabler_lib::error::AppError;
use tabler_lib::tunnel::sqlite_remote::{RemoteHost, RemoteSqlite, SqliteSshEvent};
use tabler_lib::tunnel::{
    KeyboardInteractive, MemoryHostKeys, PendingHostKey, PromptRound, SshPromptKind, SshSession,
    TunnelAuth, open_tunnel,
};
use tabler_lib::types::profile::{ConnectionProfile, SshConfig};

const USER: &str = "tabler";
const PASSWORD: &str = "secret";
const LOCALHOST: &str = "127.0.0.1";

// ----- fixtures ---------------------------------------------------------------------------------

fn random_key() -> PrivateKey {
    let mut seed = [0u8; 32];
    seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    PrivateKey::from(Ed25519Keypair::from_seed(&seed))
}

/// Tests change `HOME` (where `~/.ssh/config` is read): one at a time.
static HOME_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Points `HOME` at a fresh folder holding `ssh_config` as `~/.ssh/config`.
fn fake_home(ssh_config: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
    std::fs::write(home.path().join(".ssh/config"), ssh_config).unwrap();
    // SAFETY: tests that read the environment hold HOME_LOCK.
    unsafe { std::env::set_var("HOME", home.path()) };
    home
}

/// In-process SSH server: password (`tabler` / `secret`, unless `passwords` is off) and one
/// client key, `direct-tcpip` forwarding, and an SFTP subsystem serving `root`.
struct SshServer {
    addr: SocketAddr,
    host_key: PublicKey,
    forwards: Arc<AtomicUsize>,
    stop: CancellationToken,
}

impl SshServer {
    async fn start(client_key: Option<PublicKey>, passwords: bool, root: Option<PathBuf>) -> Self {
        let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let host_key = random_key();
        let public = host_key.public_key().clone();
        let forwards = Arc::new(AtomicUsize::new(0));
        let stop = CancellationToken::new();
        let methods: &[MethodKind] = if passwords {
            &[MethodKind::Password, MethodKind::PublicKey]
        } else {
            &[MethodKind::PublicKey]
        };
        let config = Arc::new(server::Config {
            keys: vec![host_key],
            methods: MethodSet::from(methods),
            auth_rejection_time: Duration::from_millis(10),
            auth_rejection_time_initial: Some(Duration::ZERO),
            inactivity_timeout: None,
            ..Default::default()
        });
        tokio::spawn({
            let forwards = forwards.clone();
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
                    let handler = ServerHandler {
                        client_key: client_key.clone(),
                        passwords,
                        root: root.clone(),
                        forwards: forwards.clone(),
                        channels: HashMap::new(),
                    };
                    let config = config.clone();
                    tokio::spawn(async move {
                        if let Ok(running) = server::run_stream(config, socket, handler).await {
                            let _ = running.await;
                        }
                    });
                }
            }
        });
        SshServer {
            addr,
            host_key: public,
            forwards,
            stop,
        }
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }

    fn forwards(&self) -> usize {
        self.forwards.load(Ordering::SeqCst)
    }

    fn trust_in(&self, trust: &MemoryHostKeys) {
        trust.trust(PendingHostKey::from_key(
            LOCALHOST,
            self.port(),
            &self.host_key,
        ));
    }
}

impl Drop for SshServer {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

struct ServerHandler {
    client_key: Option<PublicKey>,
    passwords: bool,
    root: Option<PathBuf>,
    forwards: Arc<AtomicUsize>,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl server::Handler for ServerHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if self.passwords && user == USER && password == PASSWORD {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        _key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        let ok = user == USER
            && self
                .client_key
                .as_ref()
                .is_some_and(|k| k.key_data() == key.key_data());
        Ok(if ok { Auth::Accept } else { Auth::reject() })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match (name, self.root.clone(), self.channels.remove(&channel)) {
            ("sftp", Some(root), Some(ch)) => {
                session.channel_success(channel)?;
                russh_sftp::server::run(ch.into_stream(), FsSftp::new(root)).await;
            }
            _ => session.channel_failure(channel)?,
        }
        Ok(())
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
}

/// An SFTP server over a folder (paths are relative to it; renames refuse an existing target,
/// like OpenSSH's SFTP v3).
struct FsSftp {
    root: PathBuf,
    files: HashMap<String, std::fs::File>,
    next: AtomicU64,
}

impl FsSftp {
    fn new(root: PathBuf) -> FsSftp {
        FsSftp {
            root,
            files: HashMap::new(),
            next: AtomicU64::new(1),
        }
    }

    fn path(&self, p: &str) -> PathBuf {
        self.root.join(p.trim_start_matches('/'))
    }

    fn ok(id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".into(),
            language_tag: "en-US".into(),
        }
    }
}

fn io_status(e: std::io::Error) -> StatusCode {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

impl russh_sftp::server::Handler for FsSftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let file = std::fs::OpenOptions::new()
            .read(pflags.contains(OpenFlags::READ))
            .write(pflags.contains(OpenFlags::WRITE) || pflags.contains(OpenFlags::APPEND))
            .create(pflags.contains(OpenFlags::CREATE))
            .truncate(pflags.contains(OpenFlags::TRUNCATE))
            .open(self.path(&filename))
            .map_err(io_status)?;
        let handle = self.next.fetch_add(1, Ordering::SeqCst).to_string();
        self.files.insert(handle.clone(), file);
        Ok(Handle { id, handle })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.files.remove(&handle);
        Ok(FsSftp::ok(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let file = self.files.get_mut(&handle).ok_or(StatusCode::Failure)?;
        file.seek(SeekFrom::Start(offset)).map_err(io_status)?;
        let mut data = vec![0u8; len as usize];
        let n = file.read(&mut data).map_err(io_status)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        data.truncate(n);
        Ok(Data { id, data })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let file = self.files.get_mut(&handle).ok_or(StatusCode::Failure)?;
        file.seek(SeekFrom::Start(offset)).map_err(io_status)?;
        file.write_all(&data).map_err(io_status)?;
        Ok(FsSftp::ok(id))
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let meta = std::fs::metadata(self.path(&path)).map_err(io_status)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&meta),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.stat(id, path).await
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let file = self.files.get(&handle).ok_or(StatusCode::Failure)?;
        let meta = file.metadata().map_err(io_status)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&meta),
        })
    }

    async fn setstat(
        &mut self,
        id: u32,
        _path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        Ok(FsSftp::ok(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let to = self.path(&newpath);
        if to.exists() {
            return Err(StatusCode::Failure);
        }
        std::fs::rename(self.path(&oldpath), to).map_err(io_status)?;
        Ok(FsSftp::ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        std::fs::remove_file(self.path(&filename)).map_err(io_status)?;
        Ok(FsSftp::ok(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![File::dummy(format!("/{}", path.trim_start_matches('/')))],
        })
    }
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

/// Sends bytes through the tunnel's local port and expects them back (this also passes the
/// tunnel's check that the connection comes from this process).
async fn round_trip(local_port: u16) {
    let mut stream = TcpStream::connect((LOCALHOST, local_port)).await.unwrap();
    stream.write_all(b"ping through the tunnel").await.unwrap();
    let mut got = [0u8; 23];
    tokio::time::timeout(Duration::from_secs(20), stream.read_exact(&mut got))
        .await
        .expect("echo timed out")
        .unwrap();
    assert_eq!(&got, b"ping through the tunnel");
}

fn ssh_cfg(host: &str, port: u16) -> SshConfig {
    ssh_cfg_as(host, port, USER)
}

fn ssh_cfg_as(host: &str, port: u16, user: &str) -> SshConfig {
    serde_json::from_value(serde_json::json!({
        "host": host,
        "port": port,
        "user": user,
        "auth": "password",
        "keyPath": null,
        "localPort": null,
        "keepaliveSecs": 5,
    }))
    .unwrap()
}

fn password_auth() -> TunnelAuth {
    TunnelAuth {
        password: Some(PASSWORD.into()),
        ..TunnelAuth::default()
    }
}

/// The user at the SSH prompt dialog: answers with `answer` (`None`: cancels).
struct TypingUser {
    answer: Option<&'static str>,
    asked: Mutex<Vec<PromptRound>>,
}

#[async_trait]
impl KeyboardInteractive for TypingUser {
    async fn answer(&self, round: PromptRound) -> Result<Vec<String>, AppError> {
        let n = round.prompts.len();
        self.asked.lock().unwrap().push(round);
        match self.answer {
            Some(a) => Ok(vec![a.to_string(); n]),
            None => Err(AppError::Cancelled),
        }
    }
}

// ----- ProxyCommand / ProxyJump -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_command_and_proxy_jump_from_ssh_config() {
    let _home_lock = HOME_LOCK.lock().await;
    let echo = start_echo().await;
    let target = SshServer::start(None, true, None).await;
    // The bastion only takes the key named by its IdentityFile: the jump never sends passwords.
    let bastion_key = random_key();
    let bastion = SshServer::start(Some(bastion_key.public_key().clone()), false, None).await;
    let key_dir = tempfile::tempdir().unwrap();
    let key_path = key_dir.path().join("bastion_key");
    std::fs::write(
        &key_path,
        bastion_key.to_openssh(LineEnding::LF).unwrap().as_bytes(),
    )
    .unwrap();
    let _home = fake_home(&format!(
        "Host viacmd\n  HostName {LOCALHOST}\n  Port {target}\n  ProxyCommand /usr/bin/nc %h %p\n\
         Host viajump\n  HostName {LOCALHOST}\n  Port {target}\n  ProxyJump {USER}@bastion-alias\n\
         Host bastion-alias\n  HostName {LOCALHOST}\n  Port {bastion}\n  IdentityFile {key}\n",
        target = target.port(),
        bastion = bastion.port(),
        key = key_path.display(),
    ));
    let trust = MemoryHostKeys::new();
    target.trust_in(&trust);
    bastion.trust_in(&trust);

    // ProxyCommand: the session runs over nc's stdio (a Unix nc).
    #[cfg(unix)]
    {
        let tunnel = open_tunnel(
            &ssh_cfg("viacmd", 0),
            password_auth(),
            LOCALHOST.into(),
            echo,
            &trust,
        )
        .await
        .expect("tunnel through ProxyCommand");
        round_trip(tunnel.local_port()).await;
        assert_eq!(target.forwards(), 1);
        assert_eq!(bastion.forwards(), 0);
        tunnel.close().await;
    }
    let before_jump = target.forwards();

    // ProxyJump: the bastion forwards to the target, which forwards to the database.
    let tunnel = open_tunnel(
        &ssh_cfg("viajump", 0),
        password_auth(),
        LOCALHOST.into(),
        echo,
        &trust,
    )
    .await
    .expect("tunnel through the jump host");
    round_trip(tunnel.local_port()).await;
    assert_eq!(bastion.forwards(), 1);
    assert_eq!(target.forwards(), before_jump + 1);
    tunnel.close().await;

    // An unknown jump-host key stops the connect before the target is reached.
    let untrusted = MemoryHostKeys::new();
    target.trust_in(&untrusted);
    let err = open_tunnel(
        &ssh_cfg("viajump", 0),
        password_auth(),
        LOCALHOST.into(),
        echo,
        &untrusted,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::HostKey { .. }), "{err:?}");

    // A proxy command that fails says why.
    let _home = fake_home(
        "Host broken\n  HostName 127.0.0.1\n  ProxyCommand sh -c 'echo bastion unreachable >&2; exit 1'\n",
    );
    let err = open_tunnel(
        &ssh_cfg("broken", 0),
        password_auth(),
        LOCALHOST.into(),
        echo,
        &trust,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, AppError::Tunnel(m) if m.contains("ProxyCommand")),
        "{err:?}"
    );
}

/// The SSH host, user and alias of a profile reach `/bin/sh -c "exec <ProxyCommand>"` through the
/// `%h`, `%r` and `%n` tokens, before any host key or password check. A profile from an imported
/// file or a link can name `$(cmd).corp`, which a `Host *.corp` block of `~/.ssh/config` matches:
/// the connect must be refused, and `cmd` must never run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_command_tokens_never_reach_the_shell() {
    let _home_lock = HOME_LOCK.lock().await;
    let echo = start_echo().await;
    let trust = MemoryHostKeys::new();
    let marks = tempfile::tempdir().unwrap();
    let mark = |name: &str| marks.path().join(name);
    // `true` ignores its arguments: without the check the shell still runs the substitution.
    let _home = fake_home(
        "Host *.corp\n  ProxyCommand /usr/bin/true -W %h:%p bastion\n\
         Host viauser plainuser\n  HostName 127.0.0.1\n\
         Host viauser\n  ProxyCommand /usr/bin/true -l %r %h %p\n\
         Host plainuser\n  ProxyCommand /usr/bin/true %h %p\n\
         Host *.alias\n  HostName 127.0.0.1\n  ProxyCommand /usr/bin/true %n\n",
    );
    let connect = |host: &str, user: &str| {
        let cfg = ssh_cfg_as(host, 0, user);
        let trust = &trust;
        async move {
            open_tunnel(&cfg, password_auth(), LOCALHOST.into(), echo, trust)
                .await
                .unwrap_err()
        }
    };
    let refused = |err: &AppError, what: &str| {
        assert!(
            matches!(err, AppError::Tunnel(m)
                if m.contains("cannot be used with the ProxyCommand") && m.contains(what)),
            "{err:?}"
        );
    };

    // %h: the host a `Host *.corp` block matches.
    let evil_host = format!("$(touch {}).corp", mark("pwned-host").display());
    let err = connect(&evil_host, USER).await;
    assert!(!mark("pwned-host").exists(), "%h reached the shell");
    refused(&err, "SSH host");

    // %r: the user, for whatever host the block matches.
    let evil_user = format!("$(touch {})", mark("pwned-user").display());
    let err = connect("viauser", &evil_user).await;
    assert!(!mark("pwned-user").exists(), "%r reached the shell");
    refused(&err, "SSH user");

    // %n: the host as typed, even where HostName replaced it.
    let evil_alias = format!("`touch {}`.alias", mark("pwned-alias").display());
    let err = connect(&evil_alias, USER).await;
    assert!(!mark("pwned-alias").exists(), "%n reached the shell");
    refused(&err, "SSH host alias");

    // A token the command does not use is not looked at (the user is not in this command): the
    // connect goes on, and fails later, in the handshake with the command that exits at once
    // (Unix `true`).
    #[cfg(unix)]
    {
        let err = connect("plainuser", &evil_user).await;
        assert!(
            !matches!(&err, AppError::Tunnel(m) if m.contains("cannot be used")),
            "{err:?}"
        );
        assert!(
            matches!(&err, AppError::Tunnel(m) if m.contains("ProxyCommand")),
            "{err:?}"
        );
        assert!(!mark("pwned-user").exists());
    }
}

// ----- "Ask every time" -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ask_every_time_asks_for_the_ssh_password() {
    let _home_lock = HOME_LOCK.lock().await;
    let _home = fake_home("");
    let echo = start_echo().await;
    let server = SshServer::start(None, true, None).await;
    let trust = MemoryHostKeys::new();
    server.trust_in(&trust);

    let user = Arc::new(TypingUser {
        answer: Some(PASSWORD),
        asked: Mutex::new(Vec::new()),
    });
    let auth = TunnelAuth {
        ask_password: true,
        prompts: Some(user.clone()),
        ..TunnelAuth::default()
    };
    let tunnel = open_tunnel(
        &ssh_cfg(LOCALHOST, server.port()),
        auth,
        LOCALHOST.into(),
        echo,
        &trust,
    )
    .await
    .expect("signed in with the typed password");
    round_trip(tunnel.local_port()).await;
    tunnel.close().await;
    let asked = user.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].kind, Some(SshPromptKind::Password));
    assert_eq!(asked[0].prompts.len(), 1);
    assert!(!asked[0].prompts[0].echo);

    // Cancelling the dialog gives the connect up.
    let cancelling = Arc::new(TypingUser {
        answer: None,
        asked: Mutex::new(Vec::new()),
    });
    let err = open_tunnel(
        &ssh_cfg(LOCALHOST, server.port()),
        TunnelAuth {
            ask_password: true,
            prompts: Some(cancelling),
            ..TunnelAuth::default()
        },
        LOCALHOST.into(),
        echo,
        &trust,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppError::Cancelled), "{err:?}");
}

// ----- SQLite over SSH --------------------------------------------------------------------------

/// The app side of a remote copy in tests: password sign-in, a fixed answer to confirmations,
/// recorded events.
struct TestHost {
    cfg: SshConfig,
    trust: MemoryHostKeys,
    events: Mutex<Vec<SqliteSshEvent>>,
    /// The key of an encrypted database (what the app reads from the profile's password).
    cipher: Option<Cipher>,
}

#[async_trait]
impl RemoteHost for TestHost {
    async fn open_ssh(&self) -> Result<SshSession, AppError> {
        SshSession::open(&self.cfg, password_auth(), &self.trust).await
    }

    async fn confirm(&self, _title: &str, _message: &str) -> Result<bool, AppError> {
        Ok(true)
    }

    fn emit(&self, event: SqliteSshEvent) {
        self.events.lock().unwrap().push(event);
    }

    async fn cipher(&self) -> Option<Cipher> {
        self.cipher.clone()
    }
}

fn sqlite_profile(cfg: &SshConfig, path: &str, upload: bool) -> ConnectionProfile {
    serde_json::from_value(serde_json::json!({
        "id": Uuid::new_v4(),
        "name": "Remote SQLite",
        "driver": "sqlite",
        "sqlitePath": path,
        "ssh": cfg,
        "options": { "sqliteSshUpload": upload },
    }))
    .unwrap()
}

fn count_rows(db: &Path) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_over_ssh_downloads_uploads_and_checks_for_remote_changes() {
    let _home_lock = HOME_LOCK.lock().await;
    let _home = fake_home("");
    let root = tempfile::tempdir().unwrap();
    let remote_db = root.path().join("app.db");
    {
        let conn = rusqlite::Connection::open(&remote_db).unwrap();
        conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);")
            .unwrap();
    }
    let server = SshServer::start(None, true, Some(root.path().to_path_buf())).await;
    let trust = MemoryHostKeys::new();
    server.trust_in(&trust);
    let cfg = ssh_cfg(LOCALHOST, server.port());
    let host = Arc::new(TestHost {
        cfg: cfg.clone(),
        trust,
        events: Mutex::new(Vec::new()),
        cipher: None,
    });

    // Read-only by default: the copy opens read-only and nothing goes back.
    let (copy, local) = RemoteSqlite::open(&sqlite_profile(&cfg, "~/app.db", false), host.clone())
        .await
        .expect("downloaded");
    assert!(local.options.read_only);
    let local_path = PathBuf::from(local.sqlite_path.clone().unwrap());
    assert_ne!(local_path, remote_db);
    assert_eq!(count_rows(&local_path), 1);
    assert!(matches!(
        copy.sync(false).await,
        Err(AppError::InvalidInput(_))
    ));
    copy.finish().await;
    assert!(!local_path.exists(), "the copy is removed on disconnect");

    // With uploads: a local change goes back.
    let (copy, local) = RemoteSqlite::open(&sqlite_profile(&cfg, "app.db", true), host.clone())
        .await
        .expect("downloaded");
    assert!(!local.options.read_only);
    let local_path = PathBuf::from(local.sqlite_path.clone().unwrap());
    rusqlite::Connection::open(&local_path)
        .unwrap()
        .execute("INSERT INTO t VALUES (2)", [])
        .unwrap();
    let done = copy.sync(false).await.unwrap();
    assert!(done.uploaded && !done.conflict, "{done:?}");
    assert_eq!(count_rows(&remote_db), 2);
    assert!(!root.path().join("app.db.tabler-upload").exists());
    assert!(!root.path().join("app.db.tabler-backup").exists());
    // Nothing new: nothing to upload.
    assert!(!copy.sync(false).await.unwrap().uploaded);

    // Someone else changes the remote file: the next upload does not overwrite it...
    rusqlite::Connection::open(&remote_db)
        .unwrap()
        .execute_batch("CREATE TABLE other(y); INSERT INTO other VALUES (zeroblob(50000));")
        .unwrap();
    rusqlite::Connection::open(&local_path)
        .unwrap()
        .execute("INSERT INTO t VALUES (3)", [])
        .unwrap();
    let done = copy.sync(false).await.unwrap();
    assert!(done.conflict && !done.uploaded, "{done:?}");
    assert_eq!(count_rows(&remote_db), 2);
    // ...unless the user says so.
    let done = copy.sync(true).await.unwrap();
    assert!(done.uploaded, "{done:?}");
    assert_eq!(count_rows(&remote_db), 3);

    // Disconnect after a change: the last upload, then the copy is gone.
    rusqlite::Connection::open(&local_path)
        .unwrap()
        .execute("INSERT INTO t VALUES (4)", [])
        .unwrap();
    copy.finish().await;
    assert_eq!(count_rows(&remote_db), 4);
    assert!(!local_path.exists());
    let kinds: Vec<_> = host.events.lock().unwrap().iter().map(|e| e.kind).collect();
    assert!(!kinds.is_empty(), "the final upload is reported");

    // A missing remote file is a clear error.
    let err = RemoteSqlite::open(&sqlite_profile(&cfg, "nope.db", false), host.clone())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::NotFound(m) if m.contains("nope.db")),
        "{err:?}"
    );
}

fn keyed_connection(db: &Path, cipher: &Cipher) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(db).unwrap();
    cipher::apply(&conn, cipher).unwrap();
    conn
}

/// "Upload changes back" of an encrypted database: the copy goes back encrypted with the same key
/// (an unkeyed snapshot of it failed at every sync, and the edits never reached the server).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_over_ssh_uploads_an_encrypted_database_with_its_key() {
    let _home_lock = HOME_LOCK.lock().await;
    let _home = fake_home("");
    let root = tempfile::tempdir().unwrap();
    let remote_db = root.path().join("vault.db");
    let cipher = Cipher {
        key: "s3cret".into(),
        compatibility: Some(3),
    };
    cipher::create_encrypted(&remote_db.to_string_lossy(), &cipher).unwrap();
    keyed_connection(&remote_db, &cipher)
        .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);")
        .unwrap();
    let server = SshServer::start(None, true, Some(root.path().to_path_buf())).await;
    let cfg = ssh_cfg(LOCALHOST, server.port());
    let host_with = |cipher: Option<Cipher>| {
        let trust = MemoryHostKeys::new();
        server.trust_in(&trust);
        Arc::new(TestHost {
            cfg: cfg.clone(),
            trust,
            events: Mutex::new(Vec::new()),
            cipher,
        })
    };

    // The app could not read the key (a failed Keychain read, an "Ask every time" password that
    // is gone): the upload fails, says why, and the server keeps the file it has.
    let (copy, local) =
        RemoteSqlite::open(&sqlite_profile(&cfg, "vault.db", true), host_with(None))
            .await
            .expect("downloaded");
    let local_path = PathBuf::from(local.sqlite_path.clone().unwrap());
    assert_eq!(
        header_kind(&local_path.to_string_lossy()),
        HeaderKind::Encrypted
    );
    keyed_connection(&local_path, &cipher)
        .execute("INSERT INTO t VALUES (2)", [])
        .unwrap();
    let before = std::fs::read(&remote_db).unwrap();
    let err = copy.sync(false).await.unwrap_err();
    assert!(err.to_string().contains("key"), "{err}");
    assert_eq!(std::fs::read(&remote_db).unwrap(), before);
    copy.finish().await;

    // With the key the change goes back, encrypted with the same key and compatibility.
    let host = host_with(Some(cipher.clone()));
    let (copy, local) = RemoteSqlite::open(&sqlite_profile(&cfg, "vault.db", true), host.clone())
        .await
        .expect("downloaded");
    let local_path = PathBuf::from(local.sqlite_path.clone().unwrap());
    keyed_connection(&local_path, &cipher)
        .execute("INSERT INTO t VALUES (2)", [])
        .unwrap();
    let done = copy.sync(false).await.unwrap();
    assert!(done.uploaded && !done.conflict, "{done:?}");
    assert_ne!(std::fs::read(&remote_db).unwrap(), before);
    assert_eq!(
        header_kind(&remote_db.to_string_lossy()),
        HeaderKind::Encrypted
    );
    let rows: i64 = keyed_connection(&remote_db, &cipher)
        .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 2);
    // Without the key the server's file stays unreadable.
    assert!(
        rusqlite::Connection::open(&remote_db)
            .unwrap()
            .query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0))
            .is_err()
    );
    assert!(!root.path().join("vault.db.tabler-upload").exists());
    // Nothing new: nothing to upload.
    assert!(!copy.sync(false).await.unwrap().uploaded);

    // Disconnect after a change: the last upload is encrypted too, and the copy is gone.
    keyed_connection(&local_path, &cipher)
        .execute("INSERT INTO t VALUES (3)", [])
        .unwrap();
    copy.finish().await;
    let rows: i64 = keyed_connection(&remote_db, &cipher)
        .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 3);
    assert!(!local_path.exists());
    let kinds: Vec<_> = host.events.lock().unwrap().iter().map(|e| e.kind).collect();
    assert!(
        !kinds.contains(&tabler_lib::tunnel::sqlite_remote::SqliteSshEventKind::Error),
        "{kinds:?}"
    );
}
