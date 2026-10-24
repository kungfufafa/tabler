//! Runs one dump / restore binary: streams a restore file into its stdin (decompressing gzip),
//! writes a backup from its stdout to a partial file (gzip-compressing when asked) that is renamed
//! into place only when the binary succeeds, and forwards its output lines and byte counts.
//! Cancelling kills the process and removes the partial file.

use std::collections::{HashSet, VecDeque};
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::types::backup::{BackupEvent, BackupLogLine, BackupLogStream};

use super::args::{CLEARED_ENV, Invocation, StdinSource, StdoutSink};

/// Receives the job's events (the command's IPC channel).
pub type Emit = Arc<dyn Fn(BackupEvent) + Send + Sync>;

/// How often output lines and byte counts are sent.
const FLUSH_EVERY: Duration = Duration::from_millis(200);
/// Lines forwarded per stream; later ones are counted, not sent (a psql restore prints a line
/// per statement).
const MAX_FORWARDED_LINES: usize = 5_000;
const MAX_LINE_CHARS: usize = 2_000;
/// Last stderr lines kept for the error message.
const TAIL_LINES: usize = 30;
const CHUNK: usize = 64 * 1024;
/// Bound on collecting the output of a binary that was killed.
const DRAIN_AFTER_KILL: Duration = Duration::from_secs(3);

pub struct JobOutcome {
    /// The file written (backup) or read (restore).
    pub path: PathBuf,
    pub bytes: u64,
    pub duration: Duration,
    /// Output lines that look like errors (psql keeps going after an error by default).
    pub errors: u32,
}

// ---- process-wide bookkeeping (app exit) -------------------------------------------------------

static PARTIAL_FILES: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Default::default);
static RUNNING_PIDS: LazyLock<Mutex<HashSet<u32>>> = LazyLock::new(Default::default);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

/// Kills the binaries still running and deletes their partial backup files. Meant for app exit
/// (`RunEvent::Exit`), where in-flight jobs never get to clean up. Returns how many files were
/// removed.
pub fn shutdown() -> usize {
    for pid in lock(&RUNNING_PIDS).drain() {
        #[cfg(unix)]
        if let Ok(pid) = i32::try_from(pid) {
            // SAFETY: plain libc kill(2) with SIGKILL on a pid this process spawned.
            unsafe {
                kill(pid, 9);
            }
        }
        #[cfg(not(unix))]
        let _ = pid;
    }
    let mut removed = 0;
    for path in lock(&PARTIAL_FILES).drain() {
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

struct PidGuard(u32);

impl PidGuard {
    fn register(pid: u32) -> PidGuard {
        lock(&RUNNING_PIDS).insert(pid);
        PidGuard(pid)
    }
}

impl Drop for PidGuard {
    fn drop(&mut self) {
        lock(&RUNNING_PIDS).remove(&self.0);
    }
}

/// The hidden file a backup is written to; deleted unless [`PartialFile::commit`] renames it.
struct PartialFile {
    path: PathBuf,
    kept: bool,
}

impl PartialFile {
    fn create(target: &Path) -> Result<(PartialFile, File), AppError> {
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "backup".into());
        let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
        let path = target.with_file_name(format!(".{name}.{tag}.partial"));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| AppError::Io(format!("cannot create {}: {e}", path.display())))?;
        lock(&PARTIAL_FILES).insert(path.clone());
        Ok((PartialFile { path, kept: false }, file))
    }

    /// Moves the finished file to `target` (or a free name next to it, if one appeared since).
    fn commit(mut self, target: &Path) -> Result<PathBuf, AppError> {
        let target = free_path(target);
        std::fs::rename(&self.path, &target)
            .map_err(|e| AppError::Io(format!("cannot save {}: {e}", target.display())))?;
        self.kept = true;
        Ok(target)
    }
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        if !self.kept {
            let _ = std::fs::remove_file(&self.path);
        }
        lock(&PARTIAL_FILES).remove(&self.path);
    }
}

/// Double extensions kept together when a name needs a number (`db (2).sql.gz`).
const COMPOUND_EXTENSIONS: &[&str] = &[".sql.gz", ".tar.gz", ".dump.gz"];

/// `("db_2026", ".sql.gz")`; a name without a dot has no extension.
pub fn split_extension(name: &str) -> (&str, &str) {
    let lower = name.to_ascii_lowercase();
    if let Some(ext) = COMPOUND_EXTENSIONS.iter().find(|e| lower.ends_with(*e))
        && name.len() > ext.len()
    {
        return name.split_at(name.len() - ext.len());
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// `path`, or `name (2).ext`, `name (3).ext`… when it exists.
pub fn free_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = split_extension(&name);
    for n in 2u32.. {
        let candidate = path.with_file_name(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("u32 names exhausted")
}

// ---- output lines -------------------------------------------------------------------------------

/// Whether a line of output reports an error (psql / pg_* / mysql / mysqldump wording).
pub fn is_error_line(line: &str) -> bool {
    let l = line.trim_start();
    l.contains("ERROR:")
        || l.contains(": error:")
        || l.starts_with("error:")
        || l.contains("Got error")
        || (l.starts_with("ERROR ") && l[6..].starts_with(|c: char| c.is_ascii_digit()))
        || l.starts_with("FATAL:")
}

#[derive(Default)]
struct LogState {
    pending: Vec<BackupLogLine>,
    forwarded: [usize; 2],
    dropped: [usize; 2],
    tail: VecDeque<String>,
    errors: u32,
}

impl LogState {
    fn push(&mut self, stream: BackupLogStream, text: String) {
        let text = if text.chars().count() > MAX_LINE_CHARS {
            let mut t: String = text.chars().take(MAX_LINE_CHARS).collect();
            t.push('…');
            t
        } else {
            text
        };
        if stream != BackupLogStream::Info && is_error_line(&text) {
            self.errors = self.errors.saturating_add(1);
        }
        let slot = match stream {
            BackupLogStream::Stdout => Some(0),
            BackupLogStream::Stderr => Some(1),
            BackupLogStream::Info => None,
        };
        if stream == BackupLogStream::Stderr {
            if self.tail.len() == TAIL_LINES {
                self.tail.pop_front();
            }
            self.tail.push_back(text.clone());
        }
        if let Some(i) = slot {
            if self.forwarded[i] >= MAX_FORWARDED_LINES {
                self.dropped[i] += 1;
                return;
            }
            self.forwarded[i] += 1;
        }
        self.pending.push(BackupLogLine { stream, text });
    }

    /// Reports the lines that were counted but not sent.
    fn finish(&mut self) {
        let dropped = self.dropped[0] + self.dropped[1];
        if dropped > 0 {
            self.pending.push(BackupLogLine {
                stream: BackupLogStream::Info,
                text: format!("… {dropped} more line(s) of output not shown"),
            });
            self.dropped = [0, 0];
        }
    }
}

struct Shared {
    log: Mutex<LogState>,
    /// Bytes written to the backup file / read from the restore file.
    bytes: AtomicU64,
    total: Option<u64>,
    /// First internal failure (reading the restore file, writing the backup file).
    failure: Mutex<Option<String>>,
    stop: CancellationToken,
}

impl Shared {
    fn fail(&self, message: String) {
        lock(&self.failure).get_or_insert(message);
        self.stop.cancel();
    }

    fn line(&self, stream: BackupLogStream, text: String) {
        lock(&self.log).push(stream, text);
    }
}

fn flush(shared: &Shared, emit: &Emit, last_bytes: &mut Option<u64>) {
    let lines = std::mem::take(&mut lock(&shared.log).pending);
    if !lines.is_empty() {
        emit(BackupEvent::Log { lines });
    }
    let bytes = shared.bytes.load(Ordering::Relaxed);
    if *last_bytes != Some(bytes) {
        *last_bytes = Some(bytes);
        emit(BackupEvent::Progress {
            bytes,
            total: shared.total,
        });
    }
}

async fn read_lines(stream: impl AsyncRead + Unpin, kind: BackupLogStream, shared: Arc<Shared>) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf);
                let text = text.trim_end_matches(['\n', '\r']);
                shared.line(kind, text.to_string());
            }
        }
    }
}

// ---- byte counting ----------------------------------------------------------------------------

struct CountingWriter<W> {
    inner: W,
    shared: Arc<Shared>,
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.shared.bytes.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct CountingReader<R> {
    inner: R,
    shared: Arc<Shared>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.shared.bytes.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// Copies the binary's stdout to the backup file (through gzip when asked) on a blocking thread.
async fn write_output(mut stdout: ChildStdout, file: File, gzip: bool, shared: Arc<Shared>) {
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);
    let counted = Arc::clone(&shared);
    let writer = tokio::task::spawn_blocking(move || -> io::Result<()> {
        let out = BufWriter::with_capacity(
            256 * 1024,
            CountingWriter {
                inner: file,
                shared: counted,
            },
        );
        let out = if gzip {
            let mut enc = GzEncoder::new(out, Compression::default());
            while let Some(chunk) = rx.blocking_recv() {
                enc.write_all(&chunk)?;
            }
            enc.finish()?
        } else {
            let mut out = out;
            while let Some(chunk) = rx.blocking_recv() {
                out.write_all(&chunk)?;
            }
            out
        };
        let counting = out.into_inner().map_err(|e| e.into_error())?;
        counting.inner.sync_all()
    });
    let mut buf = vec![0u8; CHUNK];
    loop {
        match stdout.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).await.is_err() {
                    break; // The writer failed; its error is reported below.
                }
            }
            Err(e) => {
                shared.fail(format!("cannot read the dump output: {e}"));
                break;
            }
        }
    }
    drop(tx);
    match writer.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => shared.fail(format!("cannot write the backup file: {e}")),
        Err(e) => shared.fail(format!("the backup file writer stopped: {e}")),
    }
}

/// Streams the restore file (decompressed when `gzip`) into the binary's stdin.
async fn feed_input(path: PathBuf, gzip: bool, mut stdin: ChildStdin, shared: Arc<Shared>) {
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(8);
    let counted = Arc::clone(&shared);
    let reader = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let file = File::open(&path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        let counting = CountingReader {
            inner: file,
            shared: counted,
        };
        let mut source: Box<dyn Read> = if gzip {
            Box::new(MultiGzDecoder::new(io::BufReader::new(counting)))
        } else {
            Box::new(counting)
        };
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = source
                .read(&mut buf)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            if n == 0 || tx.blocking_send(buf[..n].to_vec()).is_err() {
                // End of file, or the binary stopped reading.
                return Ok(());
            }
        }
    });
    while let Some(chunk) = rx.recv().await {
        // A binary that stops reading (it exited, e.g. psql ON_ERROR_STOP) is not our failure:
        // its exit status tells what happened.
        if stdin.write_all(&chunk).await.is_err() {
            break;
        }
    }
    drop(rx);
    let _ = stdin.shutdown().await;
    drop(stdin);
    match reader.await {
        Ok(Ok(())) => {}
        Ok(Err(message)) => shared.fail(message),
        Err(e) => shared.fail(format!("the restore file reader stopped: {e}")),
    }
}

fn spawn_error(inv: &Invocation, e: io::Error) -> AppError {
    let path = inv.program.display();
    match e.kind() {
        io::ErrorKind::NotFound => {
            AppError::NotFound(format!("{path} no longer exists; choose another version"))
        }
        _ => AppError::Io(format!("cannot run {path}: {e}")),
    }
}

/// A remedy for well-known failures, from the binary's last stderr lines.
fn known_fix(tail: &[String]) -> Option<&'static str> {
    let text = tail.join("\n");
    if text.contains("GTID_PURGED") {
        Some(
            "The dump sets GTID_PURGED: back it up with --set-gtid-purged=OFF to restore it into a server with GTIDs.",
        )
    } else if text.contains("server version mismatch") {
        Some("Choose a pg_dump at least as new as the server.")
    } else if text.contains("column_statistics") || text.contains("COLUMN_STATISTICS") {
        Some("Add --column-statistics=0 (mysqldump 8 with an older server).")
    } else if text.contains("role") && text.contains("does not exist") {
        Some("Back up with --no-owner --no-privileges, or restore with --no-owner.")
    } else {
        None
    }
}

/// The error of a binary that exited unsuccessfully, from its last stderr lines.
pub fn exit_error(label: &str, status: Option<i32>, tail: &[String]) -> AppError {
    let headline = tail
        .iter()
        .rev()
        .find(|l| is_error_line(l))
        .or_else(|| tail.iter().rev().find(|l| !l.trim().is_empty()))
        .map(|l| {
            let l = l.trim();
            let l = l
                .strip_prefix(label)
                .and_then(|r| r.strip_prefix(':'))
                .map(str::trim_start)
                .unwrap_or(l);
            l.strip_prefix("error:").map(str::trim_start).unwrap_or(l)
        });
    let message = match (headline, status) {
        (Some(line), _) => format!("{label} failed: {line}"),
        (None, Some(code)) => format!("{label} exited with code {code}"),
        (None, None) => format!("{label} was stopped by a signal"),
    };
    let exited = match status {
        Some(code) => format!("{label} exited with code {code}."),
        None => format!("{label} was stopped by a signal."),
    };
    let hint = match known_fix(tail) {
        Some(fix) => format!("{exited} {fix}"),
        None => exited,
    };
    let detail = (!tail.is_empty()).then(|| tail.join("\n"));
    AppError::Db {
        message,
        detail,
        hint: Some(hint),
        sqlstate: None,
        vendor_code: None,
        position: None,
        statement_index: None,
    }
}

enum End {
    Exited(ExitStatus),
    Cancelled,
    Failed,
}

/// Runs `inv`. A backup (`StdoutSink::File`) writes to `output`; a restore reads its
/// `StdinSource`. Events go to `emit`; `cancel` kills the binary.
pub async fn run(
    inv: Invocation,
    output: Option<PathBuf>,
    emit: Emit,
    cancel: CancellationToken,
) -> Result<JobOutcome, AppError> {
    let started = Instant::now();
    let (partial, out_file, gzip_out) = match (inv.stdout, &output) {
        (StdoutSink::File { gzip }, Some(target)) => {
            let (p, f) = PartialFile::create(target)?;
            (Some(p), Some(f), gzip)
        }
        (StdoutSink::File { .. }, None) => {
            return Err(AppError::internal("a backup needs an output file"));
        }
        (StdoutSink::Log, _) => (None, None, false),
    };
    let input = match &inv.stdin {
        StdinSource::File { path, gzip } => {
            let size = std::fs::metadata(path)
                .map_err(|e| AppError::Io(format!("cannot read {}: {e}", path.display())))?
                .len();
            Some((path.clone(), *gzip, size))
        }
        StdinSource::Null => None,
    };
    let shown_path = output
        .clone()
        .or_else(|| input.as_ref().map(|(p, _, _)| p.clone()))
        .unwrap_or_default();
    emit(BackupEvent::Started {
        command: inv.command_line(),
        path: shown_path.to_string_lossy().into_owned(),
    });
    let shared = Arc::new(Shared {
        log: Mutex::new(LogState::default()),
        bytes: AtomicU64::new(0),
        total: input.as_ref().map(|(_, _, size)| *size),
        failure: Mutex::new(None),
        stop: CancellationToken::new(),
    });
    for note in &inv.notes {
        shared.line(BackupLogStream::Info, note.clone());
    }

    let mut cmd = Command::new(&inv.program);
    cmd.args(&inv.args);
    for key in CLEARED_ENV {
        cmd.env_remove(key);
    }
    cmd.envs(inv.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| spawn_error(&inv, e))?;
    let _pid = child.id().map(PidGuard::register);
    log::info!("backup: started {} ({:?})", inv.label, child.id());

    let mut tasks = Vec::new();
    if let (Some((path, gzip, _)), Some(stdin)) = (input, child.stdin.take()) {
        tasks.push(tokio::spawn(feed_input(
            path,
            gzip,
            stdin,
            Arc::clone(&shared),
        )));
    }
    if let Some(stdout) = child.stdout.take() {
        tasks.push(match out_file {
            Some(file) => tokio::spawn(write_output(stdout, file, gzip_out, Arc::clone(&shared))),
            None => tokio::spawn(read_lines(
                stdout,
                BackupLogStream::Stdout,
                Arc::clone(&shared),
            )),
        });
    }
    if let Some(stderr) = child.stderr.take() {
        tasks.push(tokio::spawn(read_lines(
            stderr,
            BackupLogStream::Stderr,
            Arc::clone(&shared),
        )));
    }
    let ticker = {
        let shared = Arc::clone(&shared);
        let emit = Arc::clone(&emit);
        tokio::spawn(async move {
            let mut last = None;
            let mut every = tokio::time::interval(FLUSH_EVERY);
            loop {
                every.tick().await;
                flush(&shared, &emit, &mut last);
            }
        })
    };

    let end = tokio::select! {
        status = child.wait() => match status {
            Ok(s) => End::Exited(s),
            Err(e) => {
                shared.fail(format!("waiting for {} failed: {e}", inv.label));
                End::Failed
            }
        },
        _ = cancel.cancelled() => End::Cancelled,
        _ = shared.stop.cancelled() => End::Failed,
    };
    let killed = !matches!(end, End::Exited(_));
    if killed {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    for task in tasks {
        if killed {
            let _ = tokio::time::timeout(DRAIN_AFTER_KILL, task).await;
        } else {
            let _ = task.await;
        }
    }
    ticker.abort();
    lock(&shared.log).finish();
    let mut last = None;
    flush(&shared, &emit, &mut last);

    let (tail, errors) = {
        let log = lock(&shared.log);
        (log.tail.iter().cloned().collect::<Vec<_>>(), log.errors)
    };
    let failure = lock(&shared.failure).take();
    let bytes = shared.bytes.load(Ordering::Relaxed);
    let outcome = |path: PathBuf| JobOutcome {
        path,
        bytes,
        duration: started.elapsed(),
        errors,
    };
    match end {
        End::Cancelled => {
            log::info!("backup: {} cancelled", inv.label);
            Err(AppError::Cancelled)
        }
        _ if failure.is_some() => Err(AppError::Io(failure.unwrap_or_default())),
        End::Failed => Err(AppError::internal(format!("{} stopped", inv.label))),
        End::Exited(status) if status.success() => {
            let path = match (partial, output) {
                (Some(p), Some(target)) => p.commit(&target)?,
                _ => shown_path,
            };
            log::info!(
                "backup: {} finished in {} ms ({bytes} bytes)",
                inv.label,
                started.elapsed().as_millis()
            );
            Ok(outcome(path))
        }
        End::Exited(status) => Err(exit_error(&inv.label, status.code(), &tail)),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::sync::Mutex as StdMutex;

    #[cfg(unix)]
    use flate2::read::GzDecoder;

    use super::*;
    #[cfg(unix)]
    use crate::types::backup::DumpTool;

    // The tests that run a fake tool use /bin/sh, so they and these helpers are Unix-only.
    #[cfg(unix)]
    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake_tool.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[cfg(unix)]
    fn inv(program: PathBuf, stdin: StdinSource, stdout: StdoutSink) -> Invocation {
        Invocation {
            program,
            tool: DumpTool::PgDump,
            label: "pg_dump".into(),
            args: vec![],
            env: vec![("PGPASSWORD".into(), "hunter2".into())],
            stdin,
            stdout,
            notes: vec!["a note".into()],
        }
    }

    #[cfg(unix)]
    fn collector() -> (Emit, Arc<StdMutex<Vec<BackupEvent>>>) {
        let events = Arc::new(StdMutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        (Arc::new(move |e| sink.lock().unwrap().push(e)), events)
    }

    #[cfg(unix)]
    fn log_text(events: &[BackupEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                BackupEvent::Log { lines } => Some(lines.iter().map(|l| l.text.clone())),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn names_free_paths() {
        assert_eq!(split_extension("db_2026.sql.gz"), ("db_2026", ".sql.gz"));
        assert_eq!(split_extension("db.v2.dump"), ("db.v2", ".dump"));
        assert_eq!(split_extension("noext"), ("noext", ""));
        assert_eq!(split_extension(".sql.gz"), (".sql", ".gz"));
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("app.sql.gz");
        assert_eq!(free_path(&p), p);
        std::fs::write(&p, "").unwrap();
        assert_eq!(free_path(&p), dir.path().join("app (2).sql.gz"));
        std::fs::write(dir.path().join("app (2).sql.gz"), "").unwrap();
        assert_eq!(free_path(&p), dir.path().join("app (3).sql.gz"));
    }

    #[test]
    fn recognises_error_lines() {
        assert!(is_error_line(
            "psql:<stdin>:12: ERROR:  relation \"t\" already exists"
        ));
        assert!(is_error_line("pg_dump: error: connection to server failed"));
        assert!(is_error_line(
            "ERROR 1064 (42000) at line 5: You have an error"
        ));
        assert!(is_error_line("mysqldump: Got error: 1045: Access denied"));
        assert!(!is_error_line(
            "pg_dump: dumping contents of table \"public.error_log\""
        ));
        assert!(!is_error_line("ERRORS are fine in names"));
        assert!(!is_error_line("SET"));
    }

    #[test]
    fn builds_exit_errors_from_stderr() {
        let tail = vec![
            "pg_dump: warning: something".to_string(),
            "pg_dump: error: connection to server at \"127.0.0.1\", port 5432 failed".to_string(),
        ];
        let e = exit_error("pg_dump", Some(1), &tail);
        assert_eq!(
            e.to_string(),
            "pg_dump failed: connection to server at \"127.0.0.1\", port 5432 failed"
        );
        let AppError::Db { detail, hint, .. } = e else {
            panic!("db error expected")
        };
        assert_eq!(detail.as_deref(), Some(tail.join("\n").as_str()));
        assert_eq!(hint.as_deref(), Some("pg_dump exited with code 1."));
        assert_eq!(
            exit_error("mysql", Some(2), &[]).to_string(),
            "mysql exited with code 2"
        );
        let gtid = vec![
            "ERROR 3546 (HY000) at line 24: @@GLOBAL.GTID_PURGED cannot be changed".to_string(),
        ];
        let AppError::Db { hint, .. } = exit_error("mysql", Some(1), &gtid) else {
            panic!("db error expected")
        };
        assert!(hint.unwrap().contains("--set-gtid-purged=OFF"));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn writes_a_gzipped_backup_and_never_logs_the_password() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(
            dir.path(),
            "echo 'pg_dump: dumping' >&2\necho \"pw=$PGPASSWORD\" >&2\nprintf 'CREATE TABLE t (id int);\\n'",
        );
        let target = dir.path().join("out.sql.gz");
        let (emit, events) = collector();
        let out = run(
            inv(program, StdinSource::Null, StdoutSink::File { gzip: true }),
            Some(target.clone()),
            emit,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(out.path, target);
        assert_eq!(out.bytes, std::fs::metadata(&target).unwrap().len());
        let mut text = String::new();
        GzDecoder::new(File::open(&target).unwrap())
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "CREATE TABLE t (id int);\n");
        let events = events.lock().unwrap();
        let BackupEvent::Started { command, path } = &events[0] else {
            panic!("started first")
        };
        assert!(!command.contains("hunter2"));
        assert_eq!(path, &target.to_string_lossy());
        let lines = log_text(&events);
        assert_eq!(lines[0], "a note");
        assert!(lines.contains(&"pg_dump: dumping".to_string()));
        // The environment reaches the binary (here it echoes it on purpose).
        assert!(lines.contains(&"pw=hunter2".to_string()));
        // No partial file is left behind.
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!names.iter().any(|n| n.ends_with(".partial")), "{names:?}");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failing_binary_removes_the_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(
            dir.path(),
            "printf 'partial'\necho 'pg_dump: error: permission denied for table t' >&2\nexit 1",
        );
        let target = dir.path().join("out.sql");
        let (emit, _events) = collector();
        let err = run(
            inv(program, StdinSource::Null, StdoutSink::File { gzip: false }),
            Some(target.clone()),
            emit,
            CancellationToken::new(),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            err.to_string(),
            "pg_dump failed: permission denied for table t"
        );
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1); // the script only
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_kills_the_binary() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "printf 'x'\nexec sleep 30");
        let target = dir.path().join("out.sql");
        let (emit, _events) = collector();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            token.cancel();
        });
        let t0 = Instant::now();
        let err = run(
            inv(program, StdinSource::Null, StdoutSink::File { gzip: false }),
            Some(target.clone()),
            emit,
            cancel,
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(err, AppError::Cancelled));
        assert!(t0.elapsed() < Duration::from_secs(10));
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn feeds_a_gzipped_restore_file_into_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "cat\necho 'psql:<stdin>:2: ERROR:  oops' >&2");
        let file = dir.path().join("in.sql.gz");
        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(b"SET a = 1;\nSELECT 1;\n").unwrap();
        std::fs::write(&file, enc.finish().unwrap()).unwrap();
        let size = std::fs::metadata(&file).unwrap().len();
        let (emit, events) = collector();
        let out = run(
            inv(
                program,
                StdinSource::File {
                    path: file.clone(),
                    gzip: true,
                },
                StdoutSink::Log,
            ),
            None,
            emit,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(out.path, file);
        assert_eq!(out.bytes, size);
        assert_eq!(out.errors, 1);
        let events = events.lock().unwrap();
        let lines = log_text(&events);
        assert!(lines.contains(&"SET a = 1;".to_string()));
        assert!(lines.contains(&"SELECT 1;".to_string()));
        let last_progress = events.iter().rev().find_map(|e| match e {
            BackupEvent::Progress { bytes, total } => Some((*bytes, *total)),
            _ => None,
        });
        assert_eq!(last_progress, Some((size, Some(size))));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_binary_that_stops_reading_is_reported_by_its_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        // Exits without reading stdin (like psql -v ON_ERROR_STOP=1 after an error).
        let program = script(
            dir.path(),
            "echo 'psql:<stdin>:1: ERROR:  boom' >&2\nexit 3",
        );
        let file = dir.path().join("big.sql");
        std::fs::write(&file, "SELECT 1;\n".repeat(200_000)).unwrap();
        let (emit, _events) = collector();
        let err = run(
            inv(
                program,
                StdinSource::File {
                    path: file,
                    gzip: false,
                },
                StdoutSink::Log,
            ),
            None,
            emit,
            CancellationToken::new(),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            err.to_string(),
            "pg_dump failed: psql:<stdin>:1: ERROR:  boom"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_missing_binary_is_not_found() {
        let (emit, _events) = collector();
        let err = run(
            inv(
                PathBuf::from("/nonexistent/pg_dump"),
                StdinSource::Null,
                StdoutSink::Log,
            ),
            None,
            emit,
            CancellationToken::new(),
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(err, AppError::NotFound(_)));
    }

    #[test]
    fn caps_forwarded_lines() {
        let mut log = LogState::default();
        for i in 0..(MAX_FORWARDED_LINES + 10) {
            log.push(BackupLogStream::Stdout, format!("line {i}"));
        }
        log.push(BackupLogStream::Stderr, "x".repeat(MAX_LINE_CHARS + 5));
        log.finish();
        assert_eq!(log.pending.len(), MAX_FORWARDED_LINES + 2);
        assert_eq!(
            log.pending.last().unwrap().text,
            "… 10 more line(s) of output not shown"
        );
        assert_eq!(
            log.pending[MAX_FORWARDED_LINES].text.chars().count(),
            MAX_LINE_CHARS + 1
        );
    }
}
