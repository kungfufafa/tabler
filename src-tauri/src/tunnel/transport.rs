//! How the SSH client reaches its server: a TCP socket, the stdio of a `ProxyCommand`, or a
//! `direct-tcpip` channel through the previous jump host (`ProxyJump`). Every transport is a
//! boxed byte stream handed to `russh::client::connect_stream`.
//!
//! [`AbortOnDrop`] shuts a transport down when the connect is given up (an error, or the caller's
//! connect timeout dropping the future): russh's session task otherwise keeps waiting on the
//! stream. Once the tunnel is up it is disarmed and the session owns the transport.

#[cfg(unix)]
use std::ffi::OsString;
use std::io;
#[cfg(unix)]
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader, ReadBuf};
use tokio::net::TcpStream;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// A byte stream an SSH session can run over.
pub trait SshStream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> SshStream for T {}

pub type BoxedStream = Box<dyn SshStream>;

/// How to tear a transport down before the session took it over.
enum Abort {
    /// A duplicate of the TCP socket: shutting it down ends reads on the session's copy.
    Tcp(std::net::TcpStream),
    /// The proxy command: killed.
    Command(Arc<Mutex<Option<Child>>>),
}

/// Shuts the transports it holds down when dropped, unless disarmed.
#[derive(Default)]
pub struct AbortOnDrop(Vec<Abort>);

impl AbortOnDrop {
    /// Adds the guards of `other` (the hops of a jump chain).
    pub fn extend(&mut self, mut other: AbortOnDrop) {
        self.0.append(&mut other.0);
    }

    /// Keeps the transports open; the sessions own them from now on.
    pub fn disarm(mut self) {
        self.0.clear();
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        for abort in self.0.drain(..) {
            match abort {
                Abort::Tcp(socket) => {
                    let _ = socket.shutdown(std::net::Shutdown::Both);
                }
                Abort::Command(child) => {
                    if let Some(child) = lock(&child).as_mut() {
                        let _ = child.start_kill();
                    }
                }
            }
        }
    }
}

/// A TCP connection to `host:port`, with its abort guard.
pub async fn tcp(host: &str, port: u16, nodelay: bool) -> io::Result<(BoxedStream, AbortOnDrop)> {
    let tcp = TcpStream::connect((host, port)).await?;
    if nodelay && let Err(e) = tcp.set_nodelay(true) {
        log::debug!("ssh: set_nodelay failed: {e}");
    }
    let tcp = tcp.into_std()?;
    let guard = AbortOnDrop(vec![Abort::Tcp(tcp.try_clone()?)]);
    Ok((Box::new(TcpStream::from_std(tcp)?), guard))
}

/// The last lines a proxy command wrote to stderr, for error messages.
#[derive(Clone, Default)]
pub struct StderrTail(Arc<Mutex<Vec<String>>>);

impl StderrTail {
    const KEEP: usize = 5;

    fn push(&self, line: String) {
        let mut lines = lock(&self.0);
        lines.push(line);
        let len = lines.len();
        if len > Self::KEEP {
            lines.drain(..len - Self::KEEP);
        }
    }

    /// The collected lines, joined (empty when the command said nothing).
    pub fn text(&self) -> String {
        lock(&self.0).join("; ")
    }
}

/// What `PATH` is when the app was started without one (what a Finder / Dock launch has).
#[cfg(unix)]
const DEFAULT_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// The `PATH` of a `ProxyCommand`: `base` (the app's own, the system default when it has none)
/// followed by the folders that hold the usual proxy helpers (`aws`, `cloudflared`, `gcloud`,
/// `tailscale`) and are missing from the minimal `PATH` of an app started from Finder or the
/// Dock: Homebrew's, `/usr/local/bin` and the user's `~/.local/bin` and `~/bin`. Each folder is
/// added only when it is not there already; the existing order is kept.
#[cfg(unix)]
fn proxy_path(base: Option<OsString>, home: Option<PathBuf>) -> OsString {
    let mut dirs: Vec<PathBuf> = base
        .iter()
        .flat_map(std::env::split_paths)
        .filter(|dir| !dir.as_os_str().is_empty())
        .collect();
    if dirs.is_empty() {
        dirs = std::env::split_paths(DEFAULT_PATH).collect();
    }
    let mut extra = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/sbin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = home.filter(|h| h.is_absolute()) {
        extra.push(home.join(".local/bin"));
        extra.push(home.join("bin"));
    }
    for dir in extra {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    // A folder with a `:` in its name cannot be part of a `PATH`: keep the app's own then.
    std::env::join_paths(&dirs)
        .or_else(|_| std::env::join_paths(base.iter().flat_map(std::env::split_paths)))
        .unwrap_or_else(|_| DEFAULT_PATH.into())
}

/// The process of a `ProxyCommand`: `/bin/sh -c "exec …"` like OpenSSH. The shell is not a login
/// shell (its profile output would corrupt the SSH byte stream); the helpers that live outside
/// the GUI `PATH` are found through [`proxy_path`] instead.
#[cfg(unix)]
fn proxy_process(command: &str) -> Command {
    let mut process = Command::new("/bin/sh");
    process
        .env(
            "PATH",
            proxy_path(
                std::env::var_os("PATH"),
                std::env::var_os("HOME").map(PathBuf::from),
            ),
        )
        .arg("-c")
        .arg(format!("exec {command}"));
    process
}

/// Windows has no `/bin/sh`: the command is started directly, as Win32-OpenSSH does (the
/// process killed on abort is then the proxy itself, as after `exec`). Its first word is the
/// program, the rest its command line as written. An app started from the Start menu already
/// has the user's whole `PATH`.
#[cfg(windows)]
fn proxy_process(command: &str) -> Command {
    let (program, args) = split_program(command);
    let mut process = Command::new(program);
    if !args.is_empty() {
        process.raw_arg(args);
    }
    process
}

/// A command line split the way Windows reads it: the program is its first word, or what is
/// between the quotes it starts with (`"C:\Program Files\…\x.exe" -a`); then its arguments.
#[cfg(windows)]
fn split_program(command: &str) -> (&str, &str) {
    let command = command.trim();
    let (program, args) = match command.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"').unwrap_or((quoted, "")),
        None => command.split_once([' ', '\t']).unwrap_or((command, "")),
    };
    (program, args.trim_start())
}

/// Runs `command` (a `ProxyCommand`, tokens expanded; see [`proxy_process`]) and returns its
/// stdin / stdout as the transport. Its stderr goes to the log (and `StderrTail`).
pub fn command(command: &str) -> io::Result<(BoxedStream, AbortOnDrop, StderrTail)> {
    let mut child = proxy_process(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("the proxy command has no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("the proxy command has no stdout"))?;
    let tail = StderrTail::default();
    if let Some(stderr) = child.stderr.take() {
        let tail = tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                log::debug!("ssh ProxyCommand: {line}");
                if !line.trim().is_empty() {
                    tail.push(line.trim().to_string());
                }
            }
        });
    }
    let child = Arc::new(Mutex::new(Some(child)));
    let guard = AbortOnDrop(vec![Abort::Command(child.clone())]);
    Ok((
        Box::new(CommandStream {
            stdin,
            stdout,
            _child: ChildGuard(child),
        }),
        guard,
        tail,
    ))
}

/// Kills the proxy command once the session is done with its stream.
struct ChildGuard(Arc<Mutex<Option<Child>>>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = lock(&self.0).take() {
            let _ = child.start_kill();
        }
    }
}

/// A proxy command's stdout (read) and stdin (write) as one stream.
struct CommandStream {
    stdin: ChildStdin,
    stdout: ChildStdout,
    _child: ChildGuard,
}

impl AsyncRead for CommandStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for CommandStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stdin).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stdin).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Echoes its stdin to its stdout as it comes.
    #[cfg(unix)]
    const ECHO: &str = "cat";
    #[cfg(windows)]
    const ECHO: &str = "powershell -NoProfile -Command \
                        \"[Console]::OpenStandardInput().CopyTo([Console]::OpenStandardOutput())\"";

    #[tokio::test]
    async fn proxy_command_stdio_is_the_stream() {
        // `ECHO` echoes what the SSH client would send: the stream is the command's stdio.
        let (mut stream, guard, _tail) = command(ECHO).unwrap();
        stream.write_all(b"SSH-2.0-test\r\n").await.unwrap();
        stream.flush().await.unwrap();
        let mut buf = [0u8; 14];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"SSH-2.0-test\r\n");
        guard.disarm();
    }

    #[cfg(unix)]
    #[test]
    fn proxy_path_adds_the_usual_tool_folders_after_the_existing_ones() {
        let path = proxy_path(Some("/usr/bin:/bin".into()), Some("/Users/me".into()));
        assert_eq!(
            path,
            "/usr/bin:/bin:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:\
             /Users/me/.local/bin:/Users/me/bin"
        );
        // Without a home folder only the system-wide ones are added.
        let path = proxy_path(Some("/usr/bin".into()), None);
        assert_eq!(
            path,
            "/usr/bin:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin"
        );
    }

    #[cfg(unix)]
    #[test]
    fn proxy_path_keeps_the_order_and_adds_no_duplicates() {
        let path = proxy_path(
            Some("/usr/local/bin:/Users/me/bin:/usr/bin:/opt/homebrew/bin/".into()),
            Some("/Users/me".into()),
        );
        // `/opt/homebrew/bin/` is `/opt/homebrew/bin`; nothing is moved or repeated.
        assert_eq!(
            path,
            "/usr/local/bin:/Users/me/bin:/usr/bin:/opt/homebrew/bin/:/opt/homebrew/sbin:\
             /Users/me/.local/bin"
        );
        let again = proxy_path(Some(path.clone()), Some("/Users/me".into()));
        assert_eq!(again, path);
    }

    #[cfg(unix)]
    #[test]
    fn proxy_path_works_without_a_path() {
        let expected = format!(
            "{DEFAULT_PATH}:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:\
             /Users/me/.local/bin:/Users/me/bin"
        );
        assert_eq!(
            proxy_path(None, Some("/Users/me".into())),
            expected.as_str()
        );
        // An empty PATH would mean "the current folder": the default is used instead.
        assert_eq!(
            proxy_path(Some("".into()), Some("/Users/me".into())),
            expected.as_str()
        );
        // A relative home would put a relative folder on the PATH.
        assert_eq!(
            proxy_path(None, Some("me".into())),
            format!("{DEFAULT_PATH}:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin").as_str()
        );
    }

    /// What the proxy command wrote to stderr, once it wrote something (at most five seconds).
    async fn stderr_said(tail: &StderrTail) -> String {
        let mut said = String::new();
        for _ in 0..100 {
            said = tail.text();
            if !said.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        said
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_proxy_command_runs_with_the_extended_path() {
        let (_stream, guard, tail) = command("printf '%s\\n' \"$PATH\" >&2").unwrap();
        let said = stderr_said(&tail).await;
        guard.disarm();
        let expected = proxy_path(
            std::env::var_os("PATH"),
            std::env::var_os("HOME").map(PathBuf::from),
        );
        assert_eq!(said, expected.to_string_lossy());
        for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
            assert!(said.split(':').any(|d| d == dir), "{said}");
        }
    }

    /// Windows: the command runs without a shell, with the app's `PATH` as it is.
    #[cfg(windows)]
    #[tokio::test]
    async fn the_proxy_command_runs_with_the_apps_path() {
        let (_stream, guard, tail) =
            command("powershell -NoProfile -Command \"[Console]::Error.WriteLine($env:PATH)\"")
                .unwrap();
        let said = stderr_said(&tail).await;
        guard.disarm();
        let path = std::env::var_os("PATH").unwrap_or_default();
        assert_eq!(said, path.to_string_lossy().trim());
    }

    #[cfg(windows)]
    #[test]
    fn windows_command_lines_split_into_program_and_arguments() {
        assert_eq!(
            split_program("  ssh.exe -W %h:%p bastion "),
            ("ssh.exe", "-W %h:%p bastion")
        );
        assert_eq!(
            split_program(r#""C:\Program Files\proxy\p.exe"  -a "b c""#),
            (r"C:\Program Files\proxy\p.exe", r#"-a "b c""#)
        );
        assert_eq!(split_program("cloudflared"), ("cloudflared", ""));
        assert_eq!(split_program("\"unclosed quote"), ("unclosed quote", ""));
    }

    #[tokio::test]
    async fn aborting_kills_the_proxy_command_and_keeps_its_stderr() {
        #[cfg(unix)]
        let (mut stream, guard, tail) =
            command("echo 'no route to bastion' >&2; sleep 30").unwrap();
        #[cfg(unix)]
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        // PowerShell takes a while to start: the abort comes once it has said its line.
        #[cfg(windows)]
        let (mut stream, guard, tail) = command(
            "powershell -NoProfile -Command \
             \"[Console]::Error.WriteLine('no route to bastion'); Start-Sleep 30\"",
        )
        .unwrap();
        #[cfg(windows)]
        stderr_said(&tail).await;
        drop(guard);
        let mut buf = Vec::new();
        // The killed command's stdout reaches EOF instead of waiting 30 s.
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stream.read_to_end(&mut buf),
        )
        .await;
        assert!(read.is_ok(), "the proxy command was not killed");
        assert!(
            tail.text().contains("no route to bastion"),
            "{}",
            tail.text()
        );
    }
}
