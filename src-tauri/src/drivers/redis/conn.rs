//! Connections to the server. The driver opens its own socket (TCP, through the SSH tunnel's
//! local port when there is one) and wraps it in TLS itself (tokio-rustls with the profile's
//! modes, CA and client certificate, the profile's host for SNI and the certificate check), then
//! hands it to redis-rs's multiplexed connection, whose task it spawns and aborts when the link is
//! dropped. Every command is an argv sent as is; replies come back raw (`req_packed_command`), so a
//! server error stays a value and an EXEC keeps the result of each command.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ::redis::aio::{ConnectionLike, MultiplexedConnection};
use ::redis::{AsyncConnectionConfig, ProtocolVersion, RedisConnectionInfo};
use futures::future::BoxFuture;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::Notify;

use crate::drivers::{CancelAction, ConnectTarget, SqlTrace, TRACE_SQL_MAX_CHARS};
use crate::error::AppError;
use crate::secrets::CommandPassword;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::native::redis::{self as cmd, Command};
use crate::types::browse::{TraceCategory, TracedSql};
use crate::types::profile::{
    ConnectionProfile, DEFAULT_CONNECT_TIMEOUT_SECS, PasswordMode, SslMode,
};

use super::classify::{classify_argv, refused_in_console};
use super::cli::{command_name, split_console, tokenize};
use super::errors::{redis_error, server_error};
use super::params::RedisProtocol;
use super::values::as_int;

type Resp = ::redis::Value;

/// How long the app's own reads and writes wait for a reply (redis-rs gives up after 500 ms by
/// default, which a large SCAN page or a busy server exceeds). The console waits without a limit
/// of its own: the query time out cancels it.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a cancel waits for its `CLIENT KILL` to be answered (a server busy with the command
/// answers once the command is done).
const KILL_WAIT: Duration = Duration::from_secs(2);

/// Where the password of each new connection comes from.
enum Password {
    None,
    Fixed(String),
    /// Command Line mode: the command's output, run again once it is older than the TTL.
    Command(Arc<CommandPassword>),
}

struct Tls {
    config: Arc<rustls::ClientConfig>,
    name: ServerName<'static>,
}

/// What every connection of a session opens with.
pub struct Dial {
    host: String,
    port: u16,
    tls: Option<Tls>,
    user: Option<String>,
    password: Password,
    protocol: RedisProtocol,
    /// Set once a server refused `HELLO`: later connections speak RESP2 at once.
    resp2_only: AtomicBool,
    connect_timeout: Duration,
    /// The profile's bootstrap commands, run on every connection after it logs in.
    startup: Vec<Vec<Vec<u8>>>,
}

/// One connection. Clones of `con` share it; the link aborts its task when dropped.
pub struct Link {
    con: MultiplexedConnection,
    /// `CLIENT ID`, which a cancel kills from another connection; `None` when refused (ACL).
    pub id: Option<i64>,
    /// The database it was opened on.
    pub db: u32,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Link {
    /// Whether the connection is still up (its task ends when the socket closes).
    pub fn alive(&self) -> bool {
        !self.task.is_finished()
    }

    /// Closes the socket now, also under a command still waiting for its reply (which fails as
    /// not connected).
    pub fn kill(&self) {
        self.task.abort();
    }

    /// A handle to send on.
    pub fn con(&self) -> MultiplexedConnection {
        self.con.clone()
    }

    /// One command; a server error stays a value.
    pub async fn send(&self, c: &Command) -> Result<Resp, AppError> {
        send(&mut self.con(), c).await
    }

    /// Commands pipelined in one round trip, a reply each.
    pub async fn pipeline(&self, cmds: &[Command]) -> Result<Vec<Resp>, AppError> {
        pipeline(&mut self.con(), cmds).await
    }
}

/// An argv as the client's command (binary safe: the name too is bytes).
fn packed(argv: &[Vec<u8>]) -> ::redis::Cmd {
    let mut out = ::redis::Cmd::new();
    for a in argv {
        out.arg(a.as_slice());
    }
    out
}

/// One command on `con`, its reply raw.
pub async fn send(con: &mut MultiplexedConnection, c: &Command) -> Result<Resp, AppError> {
    send_argv(con, &c.argv).await
}

pub async fn send_argv(
    con: &mut MultiplexedConnection,
    argv: &[Vec<u8>],
) -> Result<Resp, AppError> {
    con.req_packed_command(&packed(argv))
        .await
        .map_err(|e| redis_error(&e))
}

/// Commands pipelined on `con`, a raw reply each (none sent for none).
pub async fn pipeline(
    con: &mut MultiplexedConnection,
    cmds: &[Command],
) -> Result<Vec<Resp>, AppError> {
    if cmds.is_empty() {
        return Ok(Vec::new());
    }
    let mut p = ::redis::pipe();
    for c in cmds {
        p.add_command(packed(&c.argv));
    }
    let replies = con
        .req_packed_commands(&p, 0, cmds.len())
        .await
        .map_err(|e| redis_error(&e))?;
    if replies.len() != cmds.len() {
        return Err(AppError::Internal(format!(
            "Redis: {} replies for {} commands",
            replies.len(),
            cmds.len()
        )));
    }
    Ok(replies)
}

/// A reply that is an error, as the command's error.
pub fn ok_reply(reply: Resp) -> Result<Resp, AppError> {
    match reply {
        Resp::ServerError(e) => Err(server_error(&e)),
        other => Ok(other),
    }
}

/// Why opening a connection failed: a server without `HELLO` (retried with RESP2), or anything
/// else.
enum Attempt {
    NoHello,
    Failed(AppError),
}

impl From<AppError> for Attempt {
    fn from(e: AppError) -> Attempt {
        Attempt::Failed(e)
    }
}

/// The server has no RESP3: `HELLO` unknown (before 6.0) or the protocol refused (`NOPROTO`).
fn no_hello(e: &::redis::RedisError) -> bool {
    if e.kind() == ::redis::ErrorKind::RESP3NotSupported || e.code() == Some("NOPROTO") {
        return true;
    }
    let detail = e.detail().unwrap_or_default().to_ascii_lowercase();
    e.code() == Some("ERR") && detail.starts_with("unknown command") && detail.contains("hello")
}

impl Dial {
    /// The dial of `target`: its address (the tunnel's when there is one), TLS, credentials and
    /// bootstrap commands, checked. `read_only` refuses bootstrap commands that write.
    pub fn new(
        target: &ConnectTarget<'_>,
        protocol: RedisProtocol,
        read_only: bool,
    ) -> Result<Dial, AppError> {
        let profile = target.profile;
        let tls = match profile.ssl {
            // Redis has no TLS negotiation: a server takes TLS from the first byte or not at all.
            SslMode::Disable | SslMode::Prefer => None,
            mode => {
                let identity = crate::tls_files::load_client_certificate(
                    profile.options.ssl_key_path.as_deref(),
                    profile.options.ssl_cert_path.as_deref(),
                )?;
                let ca = profile
                    .ssl_ca_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty());
                let config = crate::tls_client::rustls_config(mode, ca, identity, &[], None)?;
                Some(Tls {
                    config: Arc::new(config),
                    name: crate::tls_client::server_name(&target.tls_host())?,
                })
            }
        };
        let ttl = Duration::from_secs(profile.driver.descriptor().command_ttl_secs.into());
        let password = match CommandPassword::for_profile_ttl(profile, target.password, ttl) {
            Some(command) => Password::Command(command),
            None => match (profile.options.password_mode(), target.password) {
                (PasswordMode::None, _) | (_, None) => Password::None,
                (_, Some("")) => Password::None,
                (_, Some(p)) => Password::Fixed(p.to_string()),
            },
        };
        Ok(Dial {
            host: target.host(),
            port: target.port(),
            tls,
            user: profile
                .user
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string),
            password,
            protocol,
            resp2_only: AtomicBool::new(false),
            connect_timeout: connect_timeout(profile),
            startup: startup_commands(profile, read_only)?,
        })
    }

    /// Whether connections speak RESP3.
    pub fn resp3(&self) -> bool {
        match self.protocol {
            RedisProtocol::Resp2 => false,
            RedisProtocol::Resp3 => true,
            RedisProtocol::Auto => !self.resp2_only.load(Ordering::SeqCst),
        }
    }

    /// A new connection on database `db`; `reply_timeout` bounds each reply (`None`: no limit).
    pub async fn open(&self, db: u32, reply_timeout: Option<Duration>) -> Result<Link, AppError> {
        let resp3 = self.resp3();
        match self.open_with(db, reply_timeout, resp3).await {
            Ok(link) => Ok(link),
            Err(Attempt::NoHello) if self.protocol == RedisProtocol::Auto => {
                self.resp2_only.store(true, Ordering::SeqCst);
                match self.open_with(db, reply_timeout, false).await {
                    Ok(link) => Ok(link),
                    Err(Attempt::Failed(e)) => Err(e),
                    Err(Attempt::NoHello) => Err(AppError::Connect(
                        "the server refused the connection's handshake".into(),
                    )),
                }
            }
            Err(Attempt::NoHello) => Err(AppError::Connect(
                "the server does not speak RESP3 (HELLO 3): choose RESP2 or Auto".into(),
            )),
            Err(Attempt::Failed(e)) => Err(e),
        }
    }

    async fn open_with(
        &self,
        db: u32,
        reply_timeout: Option<Duration>,
        resp3: bool,
    ) -> Result<Link, Attempt> {
        let limit = self.connect_timeout;
        let timed_out = || {
            AppError::Connect(format!(
                "{}:{}: no answer within {} s",
                self.host,
                self.port,
                limit.as_secs()
            ))
        };
        let started = Instant::now();
        let stream = tokio::time::timeout(limit, self.stream())
            .await
            .map_err(|_| timed_out())??;
        let mut info = RedisConnectionInfo::default()
            .set_db(i64::from(db))
            .set_protocol(if resp3 {
                ProtocolVersion::RESP3
            } else {
                ProtocolVersion::RESP2
            })
            .set_lib_name("tabler", env!("CARGO_PKG_VERSION"));
        if let Some(user) = &self.user {
            info = info.set_username(user);
        }
        match &self.password {
            Password::None => {}
            Password::Fixed(p) => info = info.set_password(p),
            Password::Command(command) => info = info.set_password(command.password().await?),
        }
        let config = AsyncConnectionConfig::new()
            .set_response_timeout(reply_timeout)
            .set_connection_timeout(None);
        let remaining = limit.saturating_sub(started.elapsed());
        let (con, driver) = tokio::time::timeout(
            remaining,
            MultiplexedConnection::new_with_config(&info, stream, config),
        )
        .await
        .map_err(|_| timed_out())?
        .map_err(|e| {
            if resp3 && no_hello(&e) {
                Attempt::NoHello
            } else {
                Attempt::Failed(redis_error(&e))
            }
        })?;
        let mut link = Link {
            con,
            id: None,
            db,
            task: tokio::spawn(driver),
        };
        // The connection's id and the bootstrap commands, in one round trip.
        let mut first = vec![cmd::client_id()];
        first.extend(self.startup.iter().map(|argv| Command {
            argv: argv.clone(),
            expect: cmd::Expect::Any,
        }));
        let replies = tokio::time::timeout(
            limit.saturating_sub(started.elapsed()),
            link.pipeline(&first),
        )
        .await
        .map_err(|_| timed_out())??;
        link.id = replies.first().and_then(as_int);
        for (argv, reply) in self.startup.iter().zip(replies.iter().skip(1)) {
            if let Resp::ServerError(e) = reply {
                return Err(Attempt::Failed(AppError::Connect(format!(
                    "the bootstrap command \u{201c}{}\u{201d} failed: {}",
                    cmd::argv_text(argv),
                    server_error(e)
                ))));
            }
        }
        Ok(link)
    }

    /// The socket, in TLS when the profile asks for it.
    async fn stream(&self) -> Result<Box<dyn Io>, AppError> {
        let tcp = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|e| AppError::Connect(format!("{}:{}: {e}", self.host, self.port)))?;
        // Commands are small and answered one by one: no Nagle delay.
        let _ = tcp.set_nodelay(true);
        let Some(tls) = &self.tls else {
            return Ok(Box::new(tcp));
        };
        let stream = tokio_rustls::TlsConnector::from(tls.config.clone())
            .connect(tls.name.clone(), tcp)
            .await
            .map_err(|e| AppError::Connect(format!("tls: {e}")))?;
        Ok(Box::new(stream))
    }
}

/// A socket, plain or in TLS.
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// The profile's Advanced ▸ Connect timeout, else the app's default.
fn connect_timeout(profile: &ConnectionProfile) -> Duration {
    Duration::from_secs(
        profile
            .options
            .connect_timeout_secs
            .map(u64::from)
            .filter(|s| *s > 0)
            .unwrap_or(DEFAULT_CONNECT_TIMEOUT_SECS),
    )
}

/// The profile's bootstrap commands (one per line or `;`), checked: none may change what the
/// session relies on of a connection (its database, a transaction, its protocol), and on a
/// read-only profile each must read.
pub fn startup_commands(
    profile: &ConnectionProfile,
    read_only: bool,
) -> Result<Vec<Vec<Vec<u8>>>, AppError> {
    let Some(text) = profile.options.startup_sql.as_deref() else {
        return Ok(Vec::new());
    };
    let refused = |line: &str, why: &str| {
        Err(AppError::InvalidInput(format!(
            "Bootstrap command \u{201c}{line}\u{201d}: {why}"
        )))
    };
    let mut out = Vec::new();
    for st in split_console(text) {
        let argv = match tokenize(&st.sql) {
            Ok(argv) => argv,
            Err(e) => return refused(&st.sql, &e.to_string()),
        };
        if let Some(why) = refused_in_console(&argv) {
            return refused(&st.sql, why);
        }
        let name = command_name(&argv).unwrap_or_default();
        if matches!(
            name.as_str(),
            "SELECT" | "SWAPDB" | "MULTI" | "EXEC" | "DISCARD" | "WATCH" | "UNWATCH"
        ) {
            return refused(
                &st.sql,
                "the DB index field picks the database, and a transaction cannot span connections",
            );
        }
        if read_only && classify_argv(&argv, None) == StatementClass::Write {
            return refused(&st.sql, "it writes, and the connection is read-only");
        }
        out.push(argv);
    }
    Ok(out)
}

// --- cancel ------------------------------------------------------------------------------------

/// What a cancel fires to stop waiting for the server, so the run ends at once.
#[derive(Default)]
pub struct Abort {
    fired: AtomicBool,
    notify: Notify,
}

impl Abort {
    pub fn fire(&self) {
        self.fired.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }

    /// Returns once fired.
    pub async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.fired() {
                return;
            }
            notified.await;
        }
    }

    /// `work`, or `Cancelled` as soon as the abort fires.
    pub async fn run<T>(
        &self,
        work: impl Future<Output = Result<T, AppError>>,
    ) -> Result<T, AppError> {
        tokio::select! {
            biased;
            () = self.wait() => Err(AppError::Cancelled),
            out = work => if self.fired() { Err(AppError::Cancelled) } else { out },
        }
    }
}

/// The cancel of a run (`CancelHandle::Custom`): stops waiting at once and, for a connection of
/// the run's own, kills it from another one (`CLIENT KILL ID`) so the server drops what it has
/// queued; the command already running finishes on the server (Redis runs one command at a time
/// and cannot stop it, except a script).
pub struct KillAction {
    pub abort: Arc<Abort>,
    /// The connection to send the kill on and the id to kill.
    pub kill: Option<(Arc<Link>, i64)>,
}

impl CancelAction for KillAction {
    fn fire(&self) -> BoxFuture<'static, Result<(), AppError>> {
        self.abort.fire();
        let kill = self.kill.clone();
        Box::pin(async move {
            if let Some((link, id)) = kill {
                // Best effort: the run has already stopped waiting, and its connection is dropped.
                match tokio::time::timeout(KILL_WAIT, link.send(&cmd::client_kill(id))).await {
                    Ok(Err(e)) => log::debug!("redis: CLIENT KILL ID {id}: {e}"),
                    Err(_) => log::debug!("redis: CLIENT KILL ID {id}: no answer yet"),
                    Ok(Ok(_)) => {}
                }
            }
            Ok(())
        })
    }

    fn label(&self) -> &'static str {
        if self.kill.is_some() {
            "client kill"
        } else {
            "stop waiting"
        }
    }
}

// --- trace -------------------------------------------------------------------------------------

/// Records what one round trip sent for the app (one line per command), with its outcome.
pub fn trace<T>(
    trace: &SqlTrace,
    category: TraceCategory,
    cmds: &[Command],
    started: Instant,
    result: &Result<T, AppError>,
    rows: impl FnOnce(&T) -> Option<u64>,
) {
    if cmds.is_empty() {
        return;
    }
    // Capped like every trace entry: a pipeline of a page's reads or a large value is long.
    let mut text = String::new();
    let mut chars = 0;
    for (i, c) in cmds.iter().enumerate() {
        let line = c.text();
        let room = TRACE_SQL_MAX_CHARS.saturating_sub(chars + usize::from(i > 0));
        if i > 0 {
            text.push('\n');
        }
        if line.chars().count() > room {
            text.extend(line.chars().take(room));
            text.push('\u{2026}');
            break;
        }
        chars += usize::from(i > 0) + line.chars().count();
        text.push_str(&line);
    }
    let (rows, error) = match result {
        Ok(v) => (rows(v), None),
        Err(e) => (None, Some(e.to_string())),
    };
    trace.record(TracedSql {
        sql: text,
        category,
        duration_ms: started.elapsed().as_millis() as u64,
        rows,
        affected: None,
        error,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(startup: &str) -> ConnectionProfile {
        let mut p: ConnectionProfile = serde_json::from_value(json!({
            "id": uuid::Uuid::nil(), "name": "r", "driver": "redis", "host": "h",
            "port": null, "user": null, "database": null, "sslCaPath": null,
            "sqlitePath": null, "ssh": null, "lastConnectedAt": null,
        }))
        .unwrap();
        p.options.startup_sql = Some(startup.into());
        p
    }

    #[test]
    fn bootstrap_commands_are_checked() {
        let ok =
            startup_commands(&profile("CLIENT SETNAME app\nCONFIG GET maxmemory"), false).unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(
            ok[0],
            [b"CLIENT".to_vec(), b"SETNAME".to_vec(), b"app".to_vec()]
        );
        // Read-only: only reads.
        assert!(startup_commands(&profile("PING; GET a"), true).is_ok());
        let err = startup_commands(&profile("PING\nSET a 1"), true).unwrap_err();
        assert!(err.to_string().contains("read-only"), "{err}");
        for bad in [
            "SELECT 2",
            "MULTI",
            "SUBSCRIBE ch",
            "HELLO 3",
            "GET \"a",
            "\"SELECT\\x00a\" 5",
            "\"multi\\x00b\"",
        ] {
            assert!(
                startup_commands(&profile(bad), false).is_err(),
                "{bad} is refused"
            );
        }
        assert!(
            startup_commands(&profile("  \n ; "), true)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_pipeline_is_traced_as_one_capped_entry() {
        let t = SqlTrace::default();
        let cmds: Vec<Command> = (0..3)
            .map(|i| cmd::pttl(format!("k{i}").as_bytes()))
            .collect();
        let ok: Result<Vec<u8>, AppError> = Ok(vec![1, 2]);
        trace(&t, TraceCategory::Data, &cmds, Instant::now(), &ok, |v| {
            Some(v.len() as u64)
        });
        let big = [cmd::get_range(&[b'k'; 3 * TRACE_SQL_MAX_CHARS], 0, 9)];
        let failed: Result<(), AppError> = Err(AppError::NotConnected);
        trace(
            &t,
            TraceCategory::Data,
            &big,
            Instant::now(),
            &failed,
            |_| None,
        );
        let entries = t.drain();
        assert_eq!(entries[0].sql, "PTTL k0\nPTTL k1\nPTTL k2");
        assert_eq!(entries[0].rows, Some(2));
        assert_eq!(entries[1].sql.chars().count(), TRACE_SQL_MAX_CHARS + 1);
        assert!(entries[1].sql.ends_with('\u{2026}'));
        assert_eq!(entries[1].error.as_deref(), Some("not connected"));
    }

    #[tokio::test]
    async fn an_abort_stops_the_wait_at_once() {
        let abort = Arc::new(Abort::default());
        let fire = abort.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            fire.fire();
        });
        let out: Result<(), AppError> = abort
            .run(async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(())
            })
            .await;
        assert!(matches!(out, Err(AppError::Cancelled)));
        // Fired before the run starts: it never waits.
        let out: Result<u8, AppError> = abort.run(async { Ok(1) }).await;
        assert!(matches!(out, Err(AppError::Cancelled)));
    }
}
