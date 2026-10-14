//! Secret storage: macOS Keychain through `keyring`, or a plaintext table in the app DB when
//! `TABLER_SECRETS_BACKEND=file` (development only — unsigned rebuilds trigger Keychain prompts).
//! Also the "Command Line (IAM authentication)" password mode: [`run_password_command`].

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use uuid::Uuid;

use crate::appdb::AppDb;
use crate::error::AppError;
use crate::types::profile::{ConnectionProfile, PasswordMode, SecretStatus};

const SERVICE: &str = "tabler";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    DbPassword,
    SshPassword,
    SshPassphrase,
}

impl SecretKind {
    fn suffix(self) -> &'static str {
        match self {
            SecretKind::DbPassword => "db",
            SecretKind::SshPassword => "ssh",
            SecretKind::SshPassphrase => "ssh-passphrase",
        }
    }

    pub fn all() -> [SecretKind; 3] {
        [
            SecretKind::DbPassword,
            SecretKind::SshPassword,
            SecretKind::SshPassphrase,
        ]
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
                match entry.get_password() {
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
                entry
                    .set_password(&value)
                    .map_err(|e| AppError::Secrets(e.to_string()))
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
            self.delete(profile_id, kind).await?;
        }
        Ok(())
    }

    pub async fn copy_all(&self, from: Uuid, to: Uuid) -> Result<(), AppError> {
        for kind in SecretKind::all() {
            if let Some(v) = self.get(from, kind).await? {
                self.set(to, kind, &v).await?;
            }
        }
        Ok(())
    }

    pub async fn status(&self, profile_id: Uuid) -> Result<SecretStatus, AppError> {
        Ok(SecretStatus {
            has_password: self
                .get(profile_id, SecretKind::DbPassword)
                .await?
                .is_some(),
            has_ssh_password: self
                .get(profile_id, SecretKind::SshPassword)
                .await?
                .is_some(),
            has_ssh_passphrase: self
                .get(profile_id, SecretKind::SshPassphrase)
                .await?
                .is_some(),
        })
    }
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

/// The user's login shell (`$SHELL`), else the platform's default one.
fn login_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/sh".into()
            }
        })
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
/// nothing of the command outlives it.
struct ProcessGroup(Option<i32>);

impl ProcessGroup {
    fn kill(&mut self) {
        if let Some(pgid) = self.0.take() {
            kill_process_group(pgid);
        }
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

#[cfg(not(unix))]
fn kill_process_group(_pgid: i32) {}

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
/// see [`shell_flags`]) with the app's environment, no stdin and a `timeout`, in a process group
/// of its own that is killed when the shell ends or times out. The password is the last non-empty
/// line of its output, trimmed (a login script that prints a greeting cannot corrupt it); it is
/// never stored. A failure carries the command's stderr.
pub async fn run_password_command(command: &str, timeout: Duration) -> Result<String, AppError> {
    let command = command.trim();
    if command.is_empty() {
        return Err(AppError::InvalidInput(
            "Enter the command that prints the password (Command Line mode)".into(),
        ));
    }
    let shell = login_shell();
    let mut cmd = tokio::process::Command::new(&shell);
    cmd.args(shell_flags(&shell))
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| {
        AppError::Connect(format!("cannot run the password command ({shell}): {e}"))
    })?;
    // The shell leads its own group: the group id is its pid.
    let mut group = ProcessGroup(child.id().and_then(|id| i32::try_from(id).ok()));
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(AppError::internal(
            "the password command has no output pipes",
        ));
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
                "the password command did not finish within {}s",
                timeout.as_secs()
            ))
        })?
        .map_err(|e| AppError::Connect(format!("the password command failed: {e}")))?;
    let drained = |reader: tokio::task::JoinHandle<(Vec<u8>, bool)>| async move {
        tokio::time::timeout(OUTPUT_DRAIN, reader)
            .await
            .ok()
            .and_then(Result::ok)
    };
    let stderr = drained(stderr).await.unwrap_or_default().0;
    let stderr = shorten(&String::from_utf8_lossy(&stderr));
    let with_stderr = |msg: String| {
        AppError::Connect(if stderr.is_empty() {
            msg
        } else {
            format!("{msg}: {stderr}")
        })
    };
    if !status.success() {
        let status = match status.code() {
            Some(code) => format!("exit status {code}"),
            None => "killed by a signal".into(),
        };
        return Err(with_stderr(format!(
            "the password command failed ({status})"
        )));
    }
    let Some((stdout, over)) = drained(stdout).await else {
        return Err(with_stderr(
            "the password command left a process running that keeps its output open".into(),
        ));
    };
    if over {
        return Err(with_stderr(format!(
            "the password command printed more than {} KiB",
            OUTPUT_LIMIT / 1024
        )));
    }
    let stdout = String::from_utf8_lossy(&stdout);
    match stdout.lines().map(str::trim).rfind(|l| !l.is_empty()) {
        Some(password) => Ok(password.to_string()),
        None => Err(with_stderr("the password command printed nothing".into())),
    }
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
        if profile.options.password_mode() != PasswordMode::Command {
            return None;
        }
        let command = profile.options.password_command()?;
        Some(Arc::new(
            CommandPassword::new(command, PASSWORD_COMMAND_TTL).seeded(current),
        ))
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
    /// kept running; the whole process group goes now.
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
}
