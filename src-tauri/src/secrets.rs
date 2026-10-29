//! Secret storage: macOS Keychain through `keyring`, or a plaintext table in the app DB when
//! `TABLER_SECRETS_BACKEND=file` (development only — unsigned rebuilds trigger Keychain prompts).
//! Also the "Command Line (IAM authentication)" password mode: [`run_password_command`], and the
//! bootstrap CLI script run before connecting: [`run_bootstrap_script`].

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use uuid::Uuid;

use crate::appdb::AppDb;
use crate::error::AppError;
use crate::process::NoWindow as _;
use crate::types::driver::ExtraSecret;
use crate::types::profile::{ConnectionProfile, PasswordMode, SecretStatus};

const SERVICE: &str = "tabler";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    DbPassword,
    SshPassword,
    SshPassphrase,
    /// The passphrase of a driver's private key file (Snowflake key pair).
    KeyPassphrase,
    /// A temporary credential's session token (AWS).
    SessionToken,
}

impl SecretKind {
    fn suffix(self) -> &'static str {
        match self {
            SecretKind::DbPassword => "db",
            SecretKind::SshPassword => "ssh",
            SecretKind::SshPassphrase => "ssh-passphrase",
            SecretKind::KeyPassphrase => "key-passphrase",
            SecretKind::SessionToken => "session-token",
        }
    }

    /// Every kind: what a profile's secret status, copy and delete go through.
    pub fn all() -> &'static [SecretKind] {
        &[
            SecretKind::DbPassword,
            SecretKind::SshPassword,
            SecretKind::SshPassphrase,
            SecretKind::KeyPassphrase,
            SecretKind::SessionToken,
        ]
    }

    /// Kept in the Keychain as UTF-8 bytes rather than as a password: Windows Credential Manager
    /// stores a password as UTF-16, which halves its 2,560-byte limit to 1,280 characters, and an
    /// AWS session token can be longer. The older kinds keep their format, so what is stored
    /// still reads.
    fn as_bytes(self) -> bool {
        matches!(self, SecretKind::KeyPassphrase | SecretKind::SessionToken)
    }

    /// What the secret is called in messages.
    fn noun(self) -> &'static str {
        match self {
            SecretKind::DbPassword => "password",
            SecretKind::SshPassword => "SSH password",
            SecretKind::SshPassphrase => "SSH key passphrase",
            SecretKind::KeyPassphrase => "key passphrase",
            SecretKind::SessionToken => "session token",
        }
    }
}

impl From<ExtraSecret> for SecretKind {
    fn from(extra: ExtraSecret) -> SecretKind {
        match extra {
            ExtraSecret::KeyPassphrase => SecretKind::KeyPassphrase,
            ExtraSecret::SessionToken => SecretKind::SessionToken,
        }
    }
}

/// A Keychain failure as an error; a secret over the platform's size limit says so plainly.
fn keychain_error(kind: SecretKind, e: keyring::v1::Error) -> AppError {
    match e {
        keyring::v1::Error::TooLong(_, limit) => AppError::InvalidInput(format!(
            "The {} is too long for the system keychain (at most {limit} bytes)",
            kind.noun()
        )),
        e => AppError::Secrets(e.to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Keychain,
    File,
}

#[derive(Clone)]
pub struct Secrets {
    backend: Backend,
    appdb: AppDb,
}

fn account(profile_id: Uuid, kind: SecretKind) -> String {
    format!("{profile_id}/{}", kind.suffix())
}

impl Secrets {
    pub fn new(backend: Backend, appdb: AppDb) -> Secrets {
        Secrets { backend, appdb }
    }

    /// Picks the backend: `TABLER_SECRETS_BACKEND` env var wins, then the `secretsBackend` setting,
    /// then Keychain in release builds / file in debug builds.
    pub fn from_env(appdb: AppDb) -> Secrets {
        let from_env = std::env::var("TABLER_SECRETS_BACKEND").ok();
        let from_settings = appdb
            .get_setting("secretsBackend")
            .ok()
            .flatten()
            .and_then(|v| v.as_str().map(str::to_string));
        let chosen = from_env.or(from_settings).unwrap_or_else(|| {
            if cfg!(debug_assertions) {
                "file".into()
            } else {
                "keychain".into()
            }
        });
        let backend = match chosen.as_str() {
            "file" => Backend::File,
            _ => Backend::Keychain,
        };
        if backend == Backend::File {
            log::warn!(
                "secrets backend = file (plaintext in the app database); use only for development"
            );
        } else {
            log::info!("secrets backend = keychain");
        }
        Secrets::new(backend, appdb)
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub async fn get(
        &self,
        profile_id: Uuid,
        kind: SecretKind,
    ) -> Result<Option<String>, AppError> {
        let key = account(profile_id, kind);
        match self.backend {
            Backend::File => self.appdb.get_secret(&key),
            Backend::Keychain => tokio::task::spawn_blocking(move || {
                let entry = keyring::v1::Entry::new(SERVICE, &key)
                    .map_err(|e| AppError::Secrets(e.to_string()))?;
                let read = if kind.as_bytes() {
                    entry.get_secret().and_then(|bytes| {
                        String::from_utf8(bytes)
                            .map_err(|e| keyring::v1::Error::BadEncoding(e.into_bytes()))
                    })
                } else {
                    entry.get_password()
                };
                match read {
                    Ok(p) => Ok(Some(p)),
                    Err(keyring::v1::Error::NoEntry) => Ok(None),
                    Err(e) => Err(AppError::Secrets(e.to_string())),
                }
            })
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?,
        }
    }

    pub async fn set(
        &self,
        profile_id: Uuid,
        kind: SecretKind,
        value: &str,
    ) -> Result<(), AppError> {
        let key = account(profile_id, kind);
        let value = value.to_string();
        match self.backend {
            Backend::File => self.appdb.set_secret(&key, &value),
            Backend::Keychain => tokio::task::spawn_blocking(move || {
                let entry = keyring::v1::Entry::new(SERVICE, &key)
                    .map_err(|e| AppError::Secrets(e.to_string()))?;
                if kind.as_bytes() {
                    entry.set_secret(value.as_bytes())
                } else {
                    entry.set_password(&value)
                }
                .map_err(|e| keychain_error(kind, e))
            })
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?,
        }
    }

    pub async fn delete(&self, profile_id: Uuid, kind: SecretKind) -> Result<(), AppError> {
        let key = account(profile_id, kind);
        match self.backend {
            Backend::File => self.appdb.delete_secret(&key),
            Backend::Keychain => tokio::task::spawn_blocking(move || {
                let entry = keyring::v1::Entry::new(SERVICE, &key)
                    .map_err(|e| AppError::Secrets(e.to_string()))?;
                match entry.delete_credential() {
                    Ok(()) | Err(keyring::v1::Error::NoEntry) => Ok(()),
                    Err(e) => Err(AppError::Secrets(e.to_string())),
                }
            })
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?,
        }
    }

    pub async fn delete_all(&self, profile_id: Uuid) -> Result<(), AppError> {
        for kind in SecretKind::all() {
            self.delete(profile_id, *kind).await?;
        }
        Ok(())
    }

    pub async fn copy_all(&self, from: Uuid, to: Uuid) -> Result<(), AppError> {
        for kind in SecretKind::all() {
            if let Some(v) = self.get(from, *kind).await? {
                self.set(to, *kind, &v).await?;
            }
        }
        Ok(())
    }

    pub async fn status(&self, profile_id: Uuid) -> Result<SecretStatus, AppError> {
        let has =
            |kind| async move { Ok::<_, AppError>(self.get(profile_id, kind).await?.is_some()) };
        Ok(SecretStatus {
            has_password: has(SecretKind::DbPassword).await?,
            has_ssh_password: has(SecretKind::SshPassword).await?,
            has_ssh_passphrase: has(SecretKind::SshPassphrase).await?,
            has_key_passphrase: has(SecretKind::KeyPassphrase).await?,
            has_session_token: has(SecretKind::SessionToken).await?,
        })
    }

    /// The second secrets `profile`'s driver takes (none for most drivers, so nothing is read),
    /// as stored, for a connect.
    pub async fn extras(&self, profile: &ConnectionProfile) -> Result<ExtraSecrets, AppError> {
        let mut extras = ExtraSecrets::default();
        for extra in profile.driver.descriptor().extra_secrets {
            if let Some(value) = self.get(profile.id, (*extra).into()).await? {
                extras.set(*extra, value);
            }
        }
        Ok(extras)
    }
}

/// The second secrets of a connect (a key passphrase, a session token), handed to the driver
/// without a field in `ConnectTarget`: the connect runs its driver's `open` inside
/// [`ExtraSecrets::scope`], where [`extra_secret`] reads them. A driver that needs one later (a
/// re-login) keeps what it read at open.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ExtraSecrets {
    key_passphrase: Option<String>,
    session_token: Option<String>,
}

impl std::fmt::Debug for ExtraSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown = |v: &Option<String>| v.as_ref().map(|_| "<redacted>");
        f.debug_struct("ExtraSecrets")
            .field("key_passphrase", &shown(&self.key_passphrase))
            .field("session_token", &shown(&self.session_token))
            .finish()
    }
}

tokio::task_local! {
    static EXTRA_SECRETS: ExtraSecrets;
}

impl ExtraSecrets {
    pub fn get(&self, extra: ExtraSecret) -> Option<&str> {
        match extra {
            ExtraSecret::KeyPassphrase => self.key_passphrase.as_deref(),
            ExtraSecret::SessionToken => self.session_token.as_deref(),
        }
    }

    pub fn set(&mut self, extra: ExtraSecret, value: String) {
        let slot = match extra {
            ExtraSecret::KeyPassphrase => &mut self.key_passphrase,
            ExtraSecret::SessionToken => &mut self.session_token,
        };
        *slot = Some(value);
    }

    /// Runs `fut` (a driver's `open` or `util`) with these secrets readable through
    /// [`extra_secret`].
    pub async fn scope<F: std::future::Future>(self, fut: F) -> F::Output {
        EXTRA_SECRETS.scope(self, fut).await
    }
}

/// The second secret `extra` of the connect running now (see [`ExtraSecrets`]); `None` when it
/// is not set, or outside a connect.
pub fn extra_secret(extra: ExtraSecret) -> Option<String> {
    EXTRA_SECRETS
        .try_with(|extras| extras.get(extra).map(str::to_string))
        .ok()
        .flatten()
}

/// How long a password command may run ("Command Line (IAM authentication)").
pub const PASSWORD_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a session reuses what its password command printed for the new connections it opens
/// (worker pool, side connections) before running the command again: IAM tokens expire (after 15
/// minutes on RDS / Aurora), so a pool connection opened later needs a fresh one.
pub const PASSWORD_COMMAND_TTL: Duration = Duration::from_secs(10 * 60);
/// How much of a failed command's stderr goes into the error message.
const STDERR_LIMIT: usize = 600;
/// What is kept of a password command's stdout and of its stderr (each). More is read and
/// dropped; that much on stdout is refused (it is not a password).
const OUTPUT_LIMIT: usize = 64 * 1024;
/// Once the command's shell has ended (and its process group was killed), how long the rest of
/// its output may take to arrive.
const OUTPUT_DRAIN: Duration = Duration::from_secs(2);

/// The shell that runs password commands and bootstrap scripts (and the MCP clients' CLIs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoginShell {
    /// `<shell> -l -c <command>` (see [`shell_flags`]).
    Posix(String),
    /// Windows' command interpreter: `<cmd> /D /S /C "<command>"`. `/D` skips its AutoRun
    /// commands; `/S` makes it drop only the outer quotes and run the rest as typed.
    #[cfg(windows)]
    Cmd(String),
}

/// The user's login shell (`$SHELL`), else the platform's default one.
pub(crate) fn login_shell() -> LoginShell {
    login_shell_from(std::env::var("SHELL").ok().as_deref())
}

/// [`login_shell`] for the given `$SHELL`.
#[cfg(not(windows))]
fn login_shell_from(shell: Option<&str>) -> LoginShell {
    LoginShell::Posix(
        shell
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                if cfg!(target_os = "macos") {
                    "/bin/zsh".into()
                } else {
                    "/bin/sh".into()
                }
            }),
    )
}

/// [`login_shell`] for the given `$SHELL`, which counts only when it names a program that exists:
/// Git Bash exports an MSYS path (`/usr/bin/bash`) that Windows cannot start. Without one, the
/// command interpreter (`%ComSpec%`, else `cmd.exe`) runs the command, which is also what the
/// `.cmd` shims of `aws`, `gcloud` and `az` need.
#[cfg(windows)]
fn login_shell_from(shell: Option<&str>) -> LoginShell {
    match shell {
        Some(shell) if is_existing_program(shell) => {
            if is_cmd(shell) {
                LoginShell::Cmd(shell.to_string())
            } else {
                LoginShell::Posix(shell.to_string())
            }
        }
        _ => LoginShell::Cmd(
            std::env::var("ComSpec")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "cmd.exe".into()),
        ),
    }
}

/// A full Windows path (drive or UNC) to a file.
#[cfg(windows)]
fn is_existing_program(path: &str) -> bool {
    let path = std::path::Path::new(path);
    path.is_absolute() && path.is_file()
}

/// Whether `shell` is cmd itself (a `$SHELL` naming it), which takes `/C`, not `-c`.
#[cfg(windows)]
fn is_cmd(shell: &str) -> bool {
    std::path::Path::new(shell)
        .file_stem()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("cmd"))
}

impl LoginShell {
    /// The shell's program, as error messages name it.
    pub(crate) fn program(&self) -> &str {
        match self {
            LoginShell::Posix(shell) => shell,
            #[cfg(windows)]
            LoginShell::Cmd(cmd) => cmd,
        }
    }

    /// A process that runs `script` in this shell, without a console window. cmd runs only the
    /// first line of what `/C` gets, so a script of several lines is refused there rather than
    /// cut short.
    pub(crate) fn command(&self, script: &str) -> Result<tokio::process::Command, AppError> {
        let mut cmd = tokio::process::Command::new(self.program());
        match self {
            LoginShell::Posix(shell) => {
                cmd.args(shell_flags(shell)).arg(script);
            }
            #[cfg(windows)]
            LoginShell::Cmd(_) => {
                if script.contains(['\n', '\r']) {
                    return Err(AppError::InvalidInput(
                        "cmd.exe runs only one line: join the commands with &".into(),
                    ));
                }
                // As typed: Rust would quote it again, and cmd does not undo that.
                cmd.raw_arg(format!("/D /S /C \"{script}\""));
            }
        }
        cmd.no_window();
        Ok(cmd)
    }
}

/// The flags that make `shell` run a command: `-l -c` (a login shell, so the PATH and AWS profile
/// of the user's shell setup apply). csh and tcsh accept `-l` only as their sole flag, so they get
/// `-c` alone: no login scripts (`~/.login`), only their `~/.cshrc`.
fn shell_flags(shell: &str) -> &'static [&'static str] {
    let name = std::path::Path::new(shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(shell);
    match name.trim_start_matches('-') {
        "csh" | "tcsh" => &["-c"],
        _ => &["-l", "-c"],
    }
}

fn shorten(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(STDERR_LIMIT) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

/// The process group of a running password command: its shell and everything the shell started
/// (the other members of a pipeline, background jobs). Killed by [`ProcessGroup::kill`] once the
/// shell has ended or timed out, and when dropped (the connect waiting for it was given up), so
/// nothing of the command outlives it. On Windows, which has no process groups, it is a job
/// object: the processes the shell starts join it too.
struct ProcessGroup(Option<GroupId>);

/// The group id, which is the pid of the shell leading it.
#[cfg(unix)]
type GroupId = i32;

/// The job object.
#[cfg(windows)]
type GroupId = std::os::windows::io::OwnedHandle;

impl ProcessGroup {
    /// The group of `child`, a shell spawned in a process group of its own.
    #[cfg(unix)]
    fn of(child: &tokio::process::Child) -> ProcessGroup {
        ProcessGroup(child.id().and_then(|id| i32::try_from(id).ok()))
    }

    /// A new job object holding `child`, which joins right after its start (long before a shell
    /// gets to start anything); what it starts then joins too. Without a job (it could not be
    /// made) only the shell itself is killed.
    #[cfg(windows)]
    fn of(child: &tokio::process::Child) -> ProcessGroup {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        // SAFETY: creates an unnamed job with default security; NULL on failure.
        let job = unsafe { win32::CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return ProcessGroup(None);
        }
        // SAFETY: `job` is a new handle that nothing else owns.
        let job = unsafe { OwnedHandle::from_raw_handle(job) };
        let joined = child.raw_handle().is_some_and(|process| {
            // SAFETY: both handles are open (`child` has not been waited for yet).
            unsafe { win32::AssignProcessToJobObject(job.as_raw_handle(), process) != 0 }
        });
        ProcessGroup(joined.then_some(job))
    }

    fn kill(&mut self) {
        if let Some(group) = self.0.take() {
            kill_process_group(group);
        }
    }

    /// Lets the processes of the group run on.
    fn release(&mut self) {
        self.0 = None;
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(unix)]
fn kill_process_group(pgid: i32) {
    unsafe extern "C" {
        fn killpg(pgrp: i32, sig: i32) -> i32;
    }
    /// The same number on every Unix.
    const SIGKILL: i32 = 9;
    // SAFETY: `killpg` only sends a signal; a group that is gone already fails with ESRCH.
    let _ = unsafe { killpg(pgid, SIGKILL) };
}

#[cfg(windows)]
fn kill_process_group(job: std::os::windows::io::OwnedHandle) {
    use std::os::windows::io::AsRawHandle;
    // SAFETY: ends the processes of a job this module created; the handle is closed on drop.
    let _ = unsafe { win32::TerminateJobObject(job.as_raw_handle(), 1) };
}

/// The job object calls (kernel32, which every Windows program links).
#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;

    unsafe extern "system" {
        pub fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        pub fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        pub fn TerminateJobObject(job: *mut c_void, exit_code: u32) -> i32;
    }
}

/// Reads `pipe` to its end, keeping the first [`OUTPUT_LIMIT`] bytes; `true` when there was more.
async fn read_capped(mut pipe: impl tokio::io::AsyncRead + Unpin) -> (Vec<u8>, bool) {
    use tokio::io::AsyncReadExt as _;
    let mut kept = Vec::new();
    let mut over = false;
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = OUTPUT_LIMIT - kept.len();
                over |= n > room;
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
    (kept, over)
}

/// Runs the password command of a "Command Line (IAM authentication)" profile through the user's
/// login shell (`$SHELL -l -c <command>`, so the PATH and AWS profile of their shell setup apply;
/// see [`shell_flags`]; on Windows cmd unless `$SHELL` names a program, see [`login_shell_from`])
/// with the app's environment, no stdin and a `timeout`, in a process group of its own that is
/// killed when the shell ends or times out. The password is the last non-empty line of its
/// output, trimmed (a login script that prints a greeting cannot corrupt it); it is never stored.
/// A failure carries the command's stderr.
pub async fn run_password_command(command: &str, timeout: Duration) -> Result<String, AppError> {
    run_password_command_in(&login_shell(), command, timeout).await
}

/// [`run_password_command`] through `shell`.
async fn run_password_command_in(
    shell: &LoginShell,
    command: &str,
    timeout: Duration,
) -> Result<String, AppError> {
    let command = command.trim();
    if command.is_empty() {
        return Err(AppError::InvalidInput(
            "Enter the command that prints the password (Command Line mode)".into(),
        ));
    }
    let output = run_captured(shell, command, timeout, "password command").await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    match stdout.lines().map(str::trim).rfind(|l| !l.is_empty()) {
        Some(password) => Ok(password.to_string()),
        None => Err(with_stderr(
            &output.stderr,
            "the password command printed nothing".into(),
        )),
    }
}

/// Runs a command like [`run_password_command`] and returns everything it printed, for output
/// that spans lines (the JSON of an AWS `credential_process`). `what` names the command in errors
/// ("credential_process command").
#[cfg(feature = "common-aws")]
pub(crate) async fn run_command_output(
    command: &str,
    timeout: Duration,
    what: &str,
) -> Result<String, AppError> {
    let command = command.trim();
    if command.is_empty() {
        return Err(AppError::InvalidInput(format!("the {what} is empty")));
    }
    let output = run_captured(&login_shell(), command, timeout, what).await?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// What a command that succeeded printed: its stdout, and the start of its stderr.
struct Captured {
    stdout: Vec<u8>,
    stderr: String,
}

/// A `Connect` error: `msg`, then the command's stderr.
fn with_stderr(stderr: &str, msg: String) -> AppError {
    AppError::Connect(if stderr.is_empty() {
        msg
    } else {
        format!("{msg}: {stderr}")
    })
}

/// Runs `command` through `shell` the way [`run_password_command`] says and waits for its output.
/// A command that cannot start, fails, times out or prints too much is an error naming it as
/// `what`.
async fn run_captured(
    shell: &LoginShell,
    command: &str,
    timeout: Duration,
    what: &str,
) -> Result<Captured, AppError> {
    let mut cmd = shell.command(command)?;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| {
        AppError::Connect(format!("cannot run the {what} ({}): {e}", shell.program()))
    })?;
    let mut group = ProcessGroup::of(&child);
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(AppError::internal(format!(
            "the {what} has no output pipes"
        )));
    };
    let stdout = tokio::spawn(read_capped(stdout));
    let stderr = tokio::spawn(read_capped(stderr));
    let status = tokio::time::timeout(timeout, child.wait()).await;
    // Ends what the shell left running (the rest of a pipeline after a timeout, a background
    // job), which also closes the output pipes.
    group.kill();
    let status = status
        .map_err(|_| {
            AppError::Connect(format!(
                "the {what} did not finish within {}s",
                timeout.as_secs()
            ))
        })?
        .map_err(|e| AppError::Connect(format!("the {what} failed: {e}")))?;
    let drained = |reader: tokio::task::JoinHandle<(Vec<u8>, bool)>| async move {
        tokio::time::timeout(OUTPUT_DRAIN, reader)
            .await
            .ok()
            .and_then(Result::ok)
    };
    let stderr = drained(stderr).await.unwrap_or_default().0;
    let stderr = shorten(&String::from_utf8_lossy(&stderr));
    if !status.success() {
        let status = match status.code() {
            Some(code) => format!("exit status {code}"),
            None => "killed by a signal".into(),
        };
        return Err(with_stderr(
            &stderr,
            format!("the {what} failed ({status})"),
        ));
    }
    let Some((stdout, over)) = drained(stdout).await else {
        return Err(with_stderr(
            &stderr,
            format!("the {what} left a process running that keeps its output open"),
        ));
    };
    if over {
        return Err(with_stderr(
            &stderr,
            format!("the {what} printed more than {} KiB", OUTPUT_LIMIT / 1024),
        ));
    }
    Ok(Captured { stdout, stderr })
}

/// How long a bootstrap CLI script may run before the connect gives up on it.
pub const BOOTSTRAP_SCRIPT_TIMEOUT: Duration = Duration::from_secs(60);

/// A file in the temp directory that receives a script's output and is removed when dropped
/// (processes the script left running keep writing to the unlinked file).
struct OutputFile(std::path::PathBuf);

/// How much of the end of a script's output is read to build an error message, however much the
/// script wrote.
const TAIL_READ: u64 = 16 * 1024;

impl OutputFile {
    /// Creates the file readable by the user only: a script's output may hold tokens.
    fn create() -> Result<(OutputFile, std::fs::File), AppError> {
        let path =
            std::env::temp_dir().join(format!("tabler-bootstrap-{}.log", Uuid::new_v4().simple()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options
            .open(&path)
            .map_err(|e| AppError::Io(format!("bootstrap script output: {e}")))?;
        Ok((OutputFile(path), file))
    }

    /// The last [`STDERR_LIMIT`] characters of what was written. Only the last [`TAIL_READ`] bytes
    /// of the file are read, whatever its size.
    fn tail(&self) -> String {
        std::fs::File::open(&self.0)
            .map(tail_of)
            .unwrap_or_default()
    }
}

/// The last [`STDERR_LIMIT`] characters of `source`, from its last [`TAIL_READ`] bytes at most
/// (`…` in front when text was left out).
fn tail_of<R: std::io::Read + std::io::Seek>(mut source: R) -> String {
    use std::io::{Read as _, SeekFrom};
    let read = |source: &mut R| -> std::io::Result<(Vec<u8>, bool)> {
        let len = source.seek(SeekFrom::End(0))?;
        let start = len.saturating_sub(TAIL_READ);
        source.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        source.take(TAIL_READ).read_to_end(&mut bytes)?;
        Ok((bytes, start > 0))
    };
    let (bytes, cut_file) = read(&mut source).unwrap_or_default();
    // The read may start inside a character: drop what is left of it.
    let from = if cut_file {
        bytes
            .iter()
            .take(3)
            .take_while(|b| (**b & 0xC0) == 0x80)
            .count()
    } else {
        0
    };
    let text = String::from_utf8_lossy(&bytes[from..]);
    let text = text.trim();
    let skip = text.chars().count().saturating_sub(STDERR_LIMIT);
    let text = match text.char_indices().nth(skip) {
        Some((cut, _)) => &text[cut..],
        None => text,
    };
    if text.is_empty() {
        String::new()
    } else if cut_file || skip > 0 {
        format!("…{text}")
    } else {
        text.to_string()
    }
}

impl Drop for OutputFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Runs a connection's bootstrap CLI script (b546) before connecting: `$SHELL -l -c <script>`
/// (the login shell, as for password commands; see [`shell_flags`] and [`login_shell_from`])
/// with `env` added, no stdin and a `timeout`, in a process group of its own. Its output (stdout
/// and stderr) goes to a temporary file, so what it starts in the background (a proxy, a port
/// forward) can keep running and writing after the script ends. A script that fails (non-zero
/// exit) or times out has its whole process group killed, and the error carries the end of its
/// output.
pub async fn run_bootstrap_script(
    script: &str,
    env: &[(&str, String)],
    timeout: Duration,
) -> Result<(), AppError> {
    run_bootstrap_script_in(&login_shell(), script, env, timeout).await
}

/// [`run_bootstrap_script`] through `shell`.
async fn run_bootstrap_script_in(
    shell: &LoginShell,
    script: &str,
    env: &[(&str, String)],
    timeout: Duration,
) -> Result<(), AppError> {
    let script = script.trim();
    if script.is_empty() {
        return Ok(());
    }
    let mut cmd = shell.command(script)?;
    let (output, file) = OutputFile::create()?;
    let err_file = file
        .try_clone()
        .map_err(|e| AppError::Io(format!("bootstrap script output: {e}")))?;
    cmd.envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(err_file))
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| {
        AppError::Connect(format!(
            "cannot run the bootstrap script ({}): {e}",
            shell.program()
        ))
    })?;
    // Killed on timeout, failure or when the connect waiting for it is given up (dropped).
    let mut group = ProcessGroup::of(&child);
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => status,
        Err(_) => {
            group.kill();
            let out = output.tail();
            return Err(AppError::Connect(format!(
                "the bootstrap script did not finish within {}s{}",
                timeout.as_secs(),
                if out.is_empty() {
                    String::new()
                } else {
                    format!(": {out}")
                }
            )));
        }
    }
    .map_err(|e| AppError::Connect(format!("the bootstrap script failed: {e}")))?;
    if !status.success() {
        group.kill();
        let status = match status.code() {
            Some(code) => format!("exit status {code}"),
            None => "killed by a signal".into(),
        };
        let out = output.tail();
        return Err(AppError::Connect(if out.is_empty() {
            format!("the bootstrap script failed ({status})")
        } else {
            format!("the bootstrap script failed ({status}): {out}")
        }));
    }
    // Success: what it started in the background is the point of the script (a proxy…).
    group.release();
    Ok(())
}

type FetchPassword = Arc<dyn Fn() -> BoxFuture<'static, Result<String, AppError>> + Send + Sync>;

/// The password of a session whose profile gets it from a command ("Command Line (IAM
/// authentication)"): what the command printed, reused for the connections the session opens
/// while it is younger than the TTL. The first request after that runs the command again (once,
/// however many connections wait for it). Connections already open stay as they are.
pub struct CommandPassword {
    fetch: FetchPassword,
    ttl: Duration,
    /// The last output and when the command printed it.
    cached: tokio::sync::Mutex<Option<(String, Instant)>>,
    /// Why the last run failed; cleared by a successful run.
    failure: std::sync::Mutex<Option<AppError>>,
}

impl CommandPassword {
    /// Runs `command` (see [`run_password_command`]) when the password is older than `ttl`.
    pub fn new(command: &str, ttl: Duration) -> CommandPassword {
        let command = command.to_string();
        CommandPassword::with_fetch(ttl, move || {
            let command = command.clone();
            Box::pin(async move { run_password_command(&command, PASSWORD_COMMAND_TIMEOUT).await })
        })
    }

    /// Gets the password from `fetch` when it is older than `ttl` (tests).
    pub fn with_fetch(
        ttl: Duration,
        fetch: impl Fn() -> BoxFuture<'static, Result<String, AppError>> + Send + Sync + 'static,
    ) -> CommandPassword {
        CommandPassword {
            fetch: Arc::new(fetch),
            ttl,
            cached: tokio::sync::Mutex::new(None),
            failure: std::sync::Mutex::new(None),
        }
    }

    /// Starts with `current`, printed by the command just now (for the connect itself).
    pub fn seeded(mut self, current: Option<&str>) -> CommandPassword {
        *self.cached.get_mut() = current.map(|p| (p.to_string(), Instant::now()));
        self
    }

    /// The password source of a session of `profile` when the profile gets its password from a
    /// command (`None` otherwise); `current` is what the command printed for this connect.
    pub fn for_profile(
        profile: &ConnectionProfile,
        current: Option<&str>,
    ) -> Option<Arc<CommandPassword>> {
        CommandPassword::for_profile_ttl(profile, current, PASSWORD_COMMAND_TTL)
    }

    /// [`CommandPassword::for_profile`] reused for `ttl`: a driver's tokens live as long as its
    /// descriptor's `command_ttl_secs` says.
    pub fn for_profile_ttl(
        profile: &ConnectionProfile,
        current: Option<&str>,
        ttl: Duration,
    ) -> Option<Arc<CommandPassword>> {
        if profile.options.password_mode() != PasswordMode::Command {
            return None;
        }
        let command = profile.options.password_command()?;
        Some(Arc::new(CommandPassword::new(command, ttl).seeded(current)))
    }

    /// Forgets the password, so the next [`CommandPassword::password`] runs the command again
    /// (once, however many requests wait for it): the server refused the token before its TTL
    /// ended.
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }

    /// The password for a new connection: the cached one while younger than the TTL, else the
    /// command's output now.
    pub async fn password(&self) -> Result<String, AppError> {
        let mut cached = self.cached.lock().await;
        if let Some((value, at)) = cached.as_ref()
            && at.elapsed() < self.ttl
        {
            return Ok(value.clone());
        }
        let fetched = (self.fetch)().await;
        let mut failure = self.failure.lock().unwrap_or_else(|p| p.into_inner());
        match fetched {
            Ok(value) => {
                *failure = None;
                *cached = Some((value.clone(), Instant::now()));
                Ok(value)
            }
            Err(e) => {
                *failure = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Whether the next [`CommandPassword::password`] runs the command.
    pub async fn is_stale(&self) -> bool {
        self.cached
            .lock()
            .await
            .as_ref()
            .is_none_or(|(_, at)| at.elapsed() >= self.ttl)
    }

    /// Why the last run of the command failed (`None` once a run succeeded).
    pub fn failure(&self) -> Option<AppError> {
        self.failure
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn password_command_output_is_the_password() {
        let t = PASSWORD_COMMAND_TIMEOUT;
        assert_eq!(
            run_password_command("printf '  tok3n-abc \\n'", t)
                .await
                .unwrap(),
            "tok3n-abc"
        );
        // Login-script noise before the token is skipped; the last line wins.
        assert_eq!(
            run_password_command("printf 'Welcome\\n\\ns3cret\\n\\n'", t)
                .await
                .unwrap(),
            "s3cret"
        );
        // The shell's own syntax works (pipes, variables).
        assert_eq!(
            run_password_command("X=from-var; printf '%s' \"$X\" | tr a-z A-Z", t)
                .await
                .unwrap(),
            "FROM-VAR"
        );
    }

    #[tokio::test]
    async fn password_command_failures_carry_stderr() {
        let t = PASSWORD_COMMAND_TIMEOUT;
        let err = run_password_command("printf 'token expired' >&2; exit 3", t)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exit status 3"), "{msg}");
        assert!(msg.contains("token expired"), "{msg}");

        let err = run_password_command("printf 'no creds' >&2", t)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("printed nothing"), "{err}");
        assert!(err.to_string().contains("no creds"), "{err}");

        let err = run_password_command("   ", t).await.unwrap_err();
        assert_eq!(err.code(), "INVALID_INPUT");

        let started = std::time::Instant::now();
        let err = run_password_command("sleep 5; printf late", Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("did not finish"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    /// Regression: a timeout killed only the shell, so the rest of a pipeline (`sleep 30 | cat`)
    /// kept running; the whole process group goes now. (Unix pids and `ps`; the Windows job
    /// object has its own tests below.)
    #[cfg(unix)]
    #[tokio::test]
    async fn a_timed_out_pipeline_is_killed_entirely() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let command = format!(
            "sh -c 'echo $$ > \"{}\"; exec sleep 30' | cat",
            pid_file.display()
        );
        let started = std::time::Instant::now();
        let err = run_password_command(&command, Duration::from_millis(500))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("did not finish"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(4));
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let pid = pid.trim();
        let alive = || {
            std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", pid])
                .output()
                .map(|o| {
                    let stat = String::from_utf8_lossy(&o.stdout);
                    // A zombie waits for its new parent to reap it: it is not running.
                    o.status.success() && !stat.trim().is_empty() && !stat.contains('Z')
                })
                .unwrap_or(false)
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while alive() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!alive(), "the sleep of the pipeline ({pid}) still runs");
    }

    #[tokio::test]
    async fn a_background_job_neither_blocks_nor_outlives_the_command() {
        let started = std::time::Instant::now();
        let out = run_password_command("sleep 30 & printf tok", PASSWORD_COMMAND_TIMEOUT)
            .await
            .unwrap();
        assert_eq!(out, "tok");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn bootstrap_scripts_run_before_connecting() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        // Multi-line scripts with the connection's variables.
        let script = format!(
            "set -e\nprintf '%s' \"$TABLER_CONNECTION\" > \"{}\"\necho done",
            marker.display()
        );
        run_bootstrap_script(
            &script,
            &[("TABLER_CONNECTION", "prod".into())],
            BOOTSTRAP_SCRIPT_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "prod");
        // A failure aborts with the end of its output (stdout and stderr).
        let err = run_bootstrap_script(
            "echo starting; echo 'proxy: port in use' >&2; exit 3",
            &[],
            BOOTSTRAP_SCRIPT_TIMEOUT,
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("exit status 3") && msg.contains("proxy: port in use"),
            "{msg}"
        );
        assert!(msg.contains("starting"), "{msg}");
        // Empty: nothing runs.
        run_bootstrap_script("  ", &[], BOOTSTRAP_SCRIPT_TIMEOUT)
            .await
            .unwrap();
    }

    /// A proxy started in the background keeps running after the script ends; a script that
    /// times out is killed with everything it started. (Unix pids and `ps`, as above.)
    #[cfg(unix)]
    #[tokio::test]
    async fn bootstrap_background_jobs_survive_but_timeouts_kill_the_group() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let alive = |pid: &str| {
            std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", pid])
                .output()
                .map(|o| {
                    let stat = String::from_utf8_lossy(&o.stdout);
                    o.status.success() && !stat.trim().is_empty() && !stat.contains('Z')
                })
                .unwrap_or(false)
        };
        let script = format!(
            "sleep 30 & echo $! > \"{}\"; echo started",
            pid_file.display()
        );
        let started = std::time::Instant::now();
        run_bootstrap_script(&script, &[], BOOTSTRAP_SCRIPT_TIMEOUT)
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let pid = pid.trim().to_string();
        assert!(alive(&pid), "the background job was killed");
        let _ = std::process::Command::new("kill").arg(&pid).status();

        let err = run_bootstrap_script(
            &format!("sleep 30 & echo $! > \"{}\"; sleep 30", pid_file.display()),
            &[],
            Duration::from_millis(500),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("did not finish within"), "{err}");
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let pid = pid.trim().to_string();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while alive(&pid) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            !alive(&pid),
            "the background job of a timed-out script still runs"
        );
    }

    /// `cmd` (the shell) running `ping` (what the shell started), in a group, with `ping`'s
    /// output: the pipe stays open while `ping` runs.
    #[cfg(windows)]
    async fn ping_in_a_group(
        count: u32,
    ) -> (
        tokio::process::Child,
        ProcessGroup,
        tokio::process::ChildStdout,
    ) {
        use tokio::io::AsyncReadExt as _;
        let mut child = tokio::process::Command::new("cmd")
            .args(["/d", "/c", &format!("ping -n {count} 127.0.0.1")])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let group = ProcessGroup::of(&child);
        assert!(group.0.is_some(), "no job object");
        let mut stdout = child.stdout.take().unwrap();
        // `ping` is running once it has printed something.
        assert!(stdout.read(&mut [0u8; 64]).await.unwrap() > 0);
        (child, group, stdout)
    }

    /// Windows: killing the group (a timeout, a failed script) also ends what the shell started.
    #[cfg(windows)]
    #[tokio::test]
    async fn killing_the_group_ends_what_the_shell_started() {
        use tokio::io::AsyncReadExt as _;
        let (_child, mut group, mut stdout) = ping_in_a_group(30).await;
        group.kill();
        let closed =
            tokio::time::timeout(Duration::from_secs(5), stdout.read_to_end(&mut Vec::new())).await;
        assert!(closed.is_ok(), "ping still runs");
    }

    /// Windows: a released group (a bootstrap script that succeeded) leaves what the shell
    /// started running after the shell is gone.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_released_group_keeps_what_the_shell_started() {
        use tokio::io::AsyncReadExt as _;
        // About three more seconds of pings.
        let (mut child, mut group, mut stdout) = ping_in_a_group(4).await;
        group.release();
        drop(group);
        child.kill().await.unwrap();
        let mut out = Vec::new();
        let early =
            tokio::time::timeout(Duration::from_secs(1), stdout.read_to_end(&mut out)).await;
        assert!(early.is_err(), "ping was killed with the shell");
        // It then ends on its own.
        tokio::time::timeout(Duration::from_secs(10), stdout.read_to_end(&mut out))
            .await
            .unwrap()
            .unwrap();
    }

    /// Windows without a `$SHELL` it can start: cmd.
    #[cfg(windows)]
    fn cmd() -> LoginShell {
        let shell = login_shell_from(None);
        assert!(matches!(shell, LoginShell::Cmd(_)), "{shell:?}");
        shell
    }

    #[cfg(windows)]
    #[test]
    fn windows_takes_shell_only_when_it_names_a_program() {
        let cmd = cmd();
        assert!(is_cmd(cmd.program()), "{cmd:?}");
        // Empty, Git Bash's MSYS path, a bare name, a file that is not there: cmd.
        for shell in ["", "  ", "/usr/bin/bash", "bash", r"C:\nowhere\bash.exe"] {
            assert_eq!(login_shell_from(Some(shell)), cmd, "{shell}");
        }
        // A program that exists: that shell, with the POSIX flags.
        let dir = tempfile::tempdir().unwrap();
        let bash = dir.path().join("bash.exe");
        std::fs::write(&bash, b"").unwrap();
        let bash = bash.to_string_lossy().into_owned();
        assert_eq!(
            login_shell_from(Some(&bash)),
            LoginShell::Posix(bash.clone())
        );
        // `$SHELL` naming cmd itself gets cmd's flags.
        let own = dir.path().join("CMD.EXE");
        std::fs::write(&own, b"").unwrap();
        let own = own.to_string_lossy().into_owned();
        assert_eq!(login_shell_from(Some(&own)), LoginShell::Cmd(own.clone()));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cmd_runs_password_commands_as_typed() {
        let t = PASSWORD_COMMAND_TIMEOUT;
        assert_eq!(
            run_password_command_in(&cmd(), "echo tabler", t)
                .await
                .unwrap(),
            "tabler"
        );
        // `"` and `&` reach cmd unchanged: `&` separates commands, a quoted one is text.
        assert_eq!(
            run_password_command_in(&cmd(), r#"echo one& echo "two & three""#, t)
                .await
                .unwrap(),
            r#""two & three""#
        );
        // A quoted program in a folder with a space (a `.cmd` shim under Program Files).
        let dir = tempfile::tempdir().unwrap();
        let tools = dir.path().join("Cloud SDK");
        std::fs::create_dir(&tools).unwrap();
        let shim = tools.join("token.cmd");
        std::fs::write(&shim, "@echo off\r\necho tok-%1\r\n").unwrap();
        assert_eq!(
            run_password_command_in(&cmd(), &format!("\"{}\" abc", shim.display()), t)
                .await
                .unwrap(),
            "tok-abc"
        );
        let err = run_password_command_in(&cmd(), "echo token expired 1>&2& exit /b 3", t)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exit status 3"), "{msg}");
        assert!(msg.contains("token expired"), "{msg}");
        // cmd would run the first line only.
        let err = run_password_command_in(&cmd(), "echo a\necho b", t)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_INPUT");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cmd_runs_bootstrap_scripts() {
        let t = BOOTSTRAP_SCRIPT_TIMEOUT;
        run_bootstrap_script_in(&cmd(), "echo ok", &[], t)
            .await
            .unwrap();
        // The connection's variables.
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let script = format!("echo %TABLER_CONNECTION%>\"{}\"", marker.display());
        run_bootstrap_script_in(&cmd(), &script, &[("TABLER_CONNECTION", "prod".into())], t)
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "prod");
        // A failure aborts with the end of its output (stdout and stderr).
        let err = run_bootstrap_script_in(
            &cmd(),
            "echo starting& echo proxy: port in use 1>&2& exit /b 3",
            &[],
            t,
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("exit status 3") && msg.contains("proxy: port in use"),
            "{msg}"
        );
        assert!(msg.contains("starting"), "{msg}");
        let err = run_bootstrap_script_in(&cmd(), "echo a\necho b", &[], t)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_INPUT");
    }

    /// A source of `len` bytes that is never in memory, counting what is read from it: the end
    /// of a script's output must be found without reading the rest.
    struct Endless {
        len: u64,
        pos: u64,
        read: u64,
    }

    impl Endless {
        const END: &'static [u8] = b"proxy: port in use\n";

        fn byte(&self, at: u64) -> u8 {
            let end_start = self.len - Self::END.len() as u64;
            if at >= end_start {
                Self::END[(at - end_start) as usize]
            } else if at % 20 == 19 {
                b'\n'
            } else {
                b'a' + (at % 20) as u8
            }
        }
    }

    impl std::io::Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = (buf.len() as u64).min(self.len.saturating_sub(self.pos)) as usize;
            for (i, b) in buf[..n].iter_mut().enumerate() {
                *b = self.byte(self.pos + i as u64);
            }
            self.pos += n as u64;
            self.read += n as u64;
            Ok(n)
        }
    }

    impl std::io::Seek for Endless {
        fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
            self.pos = match to {
                std::io::SeekFrom::Start(p) => p,
                std::io::SeekFrom::End(d) => self.len.checked_add_signed(d).unwrap(),
                std::io::SeekFrom::Current(d) => self.pos.checked_add_signed(d).unwrap(),
            };
            Ok(self.pos)
        }
    }

    #[test]
    fn the_tail_of_a_huge_output_reads_only_its_end() {
        // 40 GiB: reading it all (the old `fs::read`) would not finish.
        let mut source = Endless {
            len: 40 << 30,
            pos: 0,
            read: 0,
        };
        let tail = tail_of(&mut source);
        assert!(source.read <= TAIL_READ, "{} bytes read", source.read);
        assert!(tail.starts_with('…'), "{tail}");
        assert!(tail.ends_with("proxy: port in use"), "{tail}");
        assert!(tail.chars().count() <= STDERR_LIMIT + 1, "{}", tail.len());

        // Short output is returned whole, trimmed, without the ellipsis.
        assert_eq!(
            tail_of(std::io::Cursor::new(b"  one\ntwo \n\n")),
            "one\ntwo"
        );
        assert_eq!(tail_of(std::io::Cursor::new(b"")), "");
        assert_eq!(tail_of(std::io::Cursor::new(b" \n ")), "");
        // More than STDERR_LIMIT characters: the last ones, with the ellipsis.
        let long = format!("{}END", "x".repeat(2 * STDERR_LIMIT));
        let tail = tail_of(std::io::Cursor::new(long.as_bytes()));
        assert_eq!(tail.chars().count(), STDERR_LIMIT + 1);
        assert!(tail.starts_with("…x") && tail.ends_with("xEND"), "{tail}");
    }

    #[test]
    fn a_tail_that_starts_inside_a_character_drops_it() {
        // 20000 two-byte characters and a byte: the read starts on the second byte of one.
        let text = format!("{}a", "é".repeat(20_000));
        assert_eq!(
            (text.len() as u64 - TAIL_READ) % 2,
            1,
            "the read must cut a character"
        );
        let tail = tail_of(std::io::Cursor::new(text.as_bytes()));
        assert!(!tail.contains('\u{FFFD}'), "{tail}");
        assert!(tail.starts_with("…é") && tail.ends_with("éa"), "{tail}");
        // Four-byte characters, any alignment.
        for pad in 0..4 {
            let text = format!("{}{}", "z".repeat(pad), "\u{1F600}".repeat(10_000));
            let tail = tail_of(std::io::Cursor::new(text.as_bytes()));
            assert!(!tail.contains('\u{FFFD}'), "pad {pad}: {tail}");
            assert_eq!(tail.chars().count(), STDERR_LIMIT + 1, "pad {pad}");
        }
    }

    #[test]
    fn a_big_output_file_is_tailed_quickly() {
        use std::io::Write as _;
        let (output, file) = OutputFile::create().unwrap();
        let mut out = std::io::BufWriter::new(file);
        let mut written = 0usize;
        let mut n = 0u64;
        while written < 20 * 1024 * 1024 {
            let line = format!("line {n}: the proxy is chatty\n");
            out.write_all(line.as_bytes()).unwrap();
            written += line.len();
            n += 1;
        }
        out.write_all(b"fatal: port in use\n").unwrap();
        out.flush().unwrap();
        let started = std::time::Instant::now();
        let tail = output.tail();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert!(tail.chars().count() <= STDERR_LIMIT + 1, "{}", tail.len());
        assert!(tail.ends_with("fatal: port in use"), "{tail}");
    }

    #[cfg(unix)]
    #[test]
    fn the_bootstrap_output_file_is_private_and_removed() {
        use std::os::unix::fs::PermissionsExt as _;
        let (output, file) = OutputFile::create().unwrap();
        let mode = file.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{mode:o}");
        let path = output.0.clone();
        assert!(path.exists());
        drop(output);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn output_is_capped() {
        let t = PASSWORD_COMMAND_TIMEOUT;
        let err = run_password_command("head -c 70000 /dev/zero | tr '\\0' a", t)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("more than 64 KiB"), "{err}");
        // A flood on stderr is read (the command does not block on it) and shortened.
        let err = run_password_command("head -c 200000 /dev/zero | tr '\\0' e >&2; exit 1", t)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exit status 1"), "{msg}");
        assert!(msg.len() < 2 * STDERR_LIMIT, "{} bytes", msg.len());
    }

    #[test]
    fn csh_and_tcsh_get_no_login_flag() {
        assert_eq!(shell_flags("/bin/zsh"), ["-l", "-c"]);
        assert_eq!(shell_flags("/opt/homebrew/bin/fish"), ["-l", "-c"]);
        assert_eq!(shell_flags("bash"), ["-l", "-c"]);
        assert_eq!(shell_flags("/bin/csh"), ["-c"]);
        assert_eq!(shell_flags("/bin/tcsh"), ["-c"]);
        assert_eq!(shell_flags("-tcsh"), ["-c"]);
    }

    #[cfg(not(windows))]
    #[test]
    fn the_login_shell_is_shell_else_the_platform_default() {
        assert_eq!(
            login_shell_from(Some("/opt/homebrew/bin/fish")),
            LoginShell::Posix("/opt/homebrew/bin/fish".into())
        );
        let default = if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/sh"
        };
        for shell in [None, Some(""), Some("  ")] {
            assert_eq!(login_shell_from(shell), LoginShell::Posix(default.into()));
        }
    }

    fn counting(
        ttl: Duration,
        results: Vec<Result<&'static str, &'static str>>,
    ) -> (CommandPassword, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = runs.clone();
        let source = CommandPassword::with_fetch(ttl, move || {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let result = results.get(n).copied().unwrap_or(Ok("last"));
            Box::pin(async move {
                // Slow enough for concurrent callers to pile up behind one run.
                tokio::time::sleep(Duration::from_millis(30)).await;
                result
                    .map(str::to_string)
                    .map_err(|e| AppError::Connect(e.to_string()))
            })
        });
        (source, runs)
    }

    #[tokio::test]
    async fn the_command_output_is_reused_until_the_ttl_ends() {
        use std::sync::atomic::Ordering;
        let (source, runs) = counting(
            Duration::from_millis(300),
            vec![Ok("tok-1"), Err("aws: token expired"), Ok("tok-2")],
        );
        let source = Arc::new(source.seeded(Some("tok-0")));
        // What the connect itself printed serves new connections for the TTL.
        assert!(!source.is_stale().await);
        assert_eq!(source.password().await.unwrap(), "tok-0");
        assert_eq!(source.password().await.unwrap(), "tok-0");
        assert_eq!(runs.load(Ordering::SeqCst), 0);

        // After it, one run serves everyone waiting.
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(source.is_stale().await);
        let waiting: Vec<_> = (0..4)
            .map(|_| {
                let source = source.clone();
                tokio::spawn(async move { source.password().await.unwrap() })
            })
            .collect();
        for w in waiting {
            assert_eq!(w.await.unwrap(), "tok-1");
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(source.failure().is_none());

        // A failed run is reported (and remembered), and the next request runs it again.
        tokio::time::sleep(Duration::from_millis(350)).await;
        let err = source.password().await.unwrap_err();
        assert!(err.to_string().contains("token expired"), "{err}");
        assert!(source.failure().is_some());
        assert!(source.is_stale().await);
        assert_eq!(source.password().await.unwrap(), "tok-2");
        assert!(source.failure().is_none());
        assert_eq!(runs.load(Ordering::SeqCst), 3);

        // Unseeded: the first request runs the command.
        let (fresh, runs) = counting(Duration::from_secs(60), vec![Ok("x")]);
        assert!(fresh.is_stale().await);
        assert_eq!(fresh.password().await.unwrap(), "x");
        assert_eq!(fresh.password().await.unwrap(), "x");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn only_command_profiles_get_a_password_source() {
        use crate::types::profile::{Driver, ProfileOptions, SslMode};
        let mut profile = ConnectionProfile {
            id: Uuid::nil(),
            name: "iam".into(),
            color: None,
            group: None,
            driver: Driver::Postgres,
            host: Some("db".into()),
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Require,
            ssl_ca_path: None,
            sqlite_path: None,
            ssh: None,
            options: ProfileOptions {
                password_command: Some("printf tok".into()),
                ..ProfileOptions::default()
            },
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        };
        assert!(CommandPassword::for_profile(&profile, Some("pw")).is_none());
        profile.options.password_mode = Some(PasswordMode::Command);
        assert!(CommandPassword::for_profile(&profile, Some("tok")).is_some());
        profile.options.password_command = Some("  ".into());
        assert!(CommandPassword::for_profile(&profile, None).is_none());
    }

    #[tokio::test]
    async fn file_backend_round_trip() {
        let secrets = Secrets::new(Backend::File, AppDb::open_in_memory().unwrap());
        let id = Uuid::new_v4();
        assert_eq!(secrets.get(id, SecretKind::DbPassword).await.unwrap(), None);
        secrets.set(id, SecretKind::DbPassword, "pw").await.unwrap();
        assert_eq!(
            secrets
                .get(id, SecretKind::DbPassword)
                .await
                .unwrap()
                .as_deref(),
            Some("pw")
        );
        let status = secrets.status(id).await.unwrap();
        assert!(status.has_password && !status.has_ssh_password);
        let other = Uuid::new_v4();
        secrets.copy_all(id, other).await.unwrap();
        assert!(secrets.status(other).await.unwrap().has_password);
        secrets.delete_all(id).await.unwrap();
        assert!(!secrets.status(id).await.unwrap().has_password);
    }

    #[test]
    fn every_kind_has_its_own_account() {
        let suffixes: std::collections::HashSet<_> =
            SecretKind::all().iter().map(|k| k.suffix()).collect();
        assert_eq!(suffixes.len(), SecretKind::all().len());
        assert_eq!(
            account(Uuid::nil(), SecretKind::SessionToken),
            "00000000-0000-0000-0000-000000000000/session-token"
        );
        // The older kinds keep their Keychain format; the new ones are stored as bytes.
        let bytes: Vec<_> = SecretKind::all()
            .iter()
            .filter(|k| k.as_bytes())
            .copied()
            .collect();
        assert_eq!(bytes, [SecretKind::KeyPassphrase, SecretKind::SessionToken]);
        assert_eq!(
            SecretKind::from(ExtraSecret::KeyPassphrase),
            SecretKind::KeyPassphrase
        );
    }

    #[tokio::test]
    async fn the_second_secrets_store_copy_and_delete_with_the_profile() {
        let secrets = Secrets::new(Backend::File, AppDb::open_in_memory().unwrap());
        let id = Uuid::new_v4();
        // An AWS session token is longer than Windows Credential Manager keeps as a password.
        let token = "FwoGZXIvYXdzE".repeat(150);
        secrets
            .set(id, SecretKind::KeyPassphrase, "pass phrase")
            .await
            .unwrap();
        secrets
            .set(id, SecretKind::SessionToken, &token)
            .await
            .unwrap();
        let status = secrets.status(id).await.unwrap();
        assert!(status.has_key_passphrase && status.has_session_token);
        assert!(!status.has_password);
        let copy = Uuid::new_v4();
        secrets.copy_all(id, copy).await.unwrap();
        assert_eq!(
            secrets
                .get(copy, SecretKind::SessionToken)
                .await
                .unwrap()
                .as_deref(),
            Some(token.as_str())
        );
        assert_eq!(
            secrets
                .get(copy, SecretKind::KeyPassphrase)
                .await
                .unwrap()
                .as_deref(),
            Some("pass phrase")
        );
        secrets.delete_all(id).await.unwrap();
        let status = secrets.status(id).await.unwrap();
        assert!(!status.has_key_passphrase && !status.has_session_token);
        assert!(secrets.status(copy).await.unwrap().has_session_token);
    }

    #[test]
    fn a_secret_over_the_keychain_limit_is_refused_plainly() {
        let err = keychain_error(
            SecretKind::SessionToken,
            keyring::v1::Error::TooLong("secret".into(), 2560),
        );
        assert_eq!(err.code(), "INVALID_INPUT");
        assert_eq!(
            err.to_string(),
            "The session token is too long for the system keychain (at most 2560 bytes)"
        );
        let err = keychain_error(SecretKind::DbPassword, keyring::v1::Error::NoEntry);
        assert_eq!(err.code(), "SECRETS");
    }

    fn driver_profile(driver: crate::types::profile::Driver) -> ConnectionProfile {
        ConnectionProfile {
            id: Uuid::new_v4(),
            name: "p".into(),
            color: None,
            group: None,
            driver,
            host: Some("db".into()),
            port: None,
            user: None,
            database: None,
            ssl: crate::types::profile::SslMode::Require,
            ssl_ca_path: None,
            sqlite_path: None,
            ssh: None,
            options: crate::types::profile::ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    #[tokio::test]
    async fn a_connect_reads_only_the_second_secrets_of_its_driver() {
        use crate::types::profile::Driver;
        let secrets = Secrets::new(Backend::File, AppDb::open_in_memory().unwrap());
        let snowflake = driver_profile(Driver::Snowflake);
        let pg = driver_profile(Driver::Postgres);
        for id in [snowflake.id, pg.id] {
            for kind in [SecretKind::KeyPassphrase, SecretKind::SessionToken] {
                secrets.set(id, kind, "stored").await.unwrap();
            }
        }
        let extras = secrets.extras(&snowflake).await.unwrap();
        assert_eq!(extras.get(ExtraSecret::KeyPassphrase), Some("stored"));
        assert_eq!(extras.get(ExtraSecret::SessionToken), None);
        assert_eq!(secrets.extras(&pg).await.unwrap(), ExtraSecrets::default());
        // Never in a log line.
        let shown = format!("{extras:?}");
        assert!(
            !shown.contains("stored") && shown.contains("<redacted>"),
            "{shown}"
        );
    }

    #[tokio::test]
    async fn a_driver_reads_the_second_secrets_inside_the_connect_only() {
        let mut extras = ExtraSecrets::default();
        extras.set(ExtraSecret::SessionToken, "tok".into());
        assert_eq!(extra_secret(ExtraSecret::SessionToken), None);
        let read = extras
            .scope(async {
                // Across an await, as a driver's open has them.
                tokio::task::yield_now().await;
                (
                    extra_secret(ExtraSecret::SessionToken),
                    extra_secret(ExtraSecret::KeyPassphrase),
                )
            })
            .await;
        assert_eq!(read, (Some("tok".into()), None));
        assert_eq!(extra_secret(ExtraSecret::SessionToken), None);
    }

    #[tokio::test]
    async fn a_token_ttl_of_one_second_runs_the_command_again() {
        use std::sync::atomic::Ordering;
        let (source, runs) = counting(Duration::from_secs(1), vec![Ok("tok-1"), Ok("tok-2")]);
        assert_eq!(source.password().await.unwrap(), "tok-1");
        assert_eq!(source.password().await.unwrap(), "tok-1");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert_eq!(source.password().await.unwrap(), "tok-2");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn invalidate_runs_the_command_exactly_once_more() {
        use std::sync::atomic::Ordering;
        let (source, runs) = counting(Duration::from_secs(600), vec![Ok("tok-2")]);
        let source = Arc::new(source.seeded(Some("tok-1")));
        assert_eq!(source.password().await.unwrap(), "tok-1");
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        // The server refused tok-1: everyone waiting gets the one new token.
        source.invalidate().await;
        assert!(source.is_stale().await);
        let waiting: Vec<_> = (0..4)
            .map(|_| {
                let source = source.clone();
                tokio::spawn(async move { source.password().await.unwrap() })
            })
            .collect();
        for w in waiting {
            assert_eq!(w.await.unwrap(), "tok-2");
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(source.password().await.unwrap(), "tok-2");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_driver_ttl_applies_to_command_profiles_only() {
        use crate::types::profile::Driver;
        let mut profile = driver_profile(Driver::Snowflake);
        let ttl = Duration::from_secs(u64::from(Driver::Snowflake.descriptor().command_ttl_secs));
        assert!(CommandPassword::for_profile_ttl(&profile, None, ttl).is_none());
        profile.options.password_mode = Some(PasswordMode::Command);
        profile.options.password_command = Some("printf tok".into());
        let source = CommandPassword::for_profile_ttl(&profile, None, ttl).unwrap();
        assert_eq!(source.ttl, ttl);
        // The core drivers keep the ten minutes of RDS IAM tokens.
        let core = CommandPassword::for_profile(&profile, None).unwrap();
        assert_eq!(core.ttl, PASSWORD_COMMAND_TTL);
    }
}
