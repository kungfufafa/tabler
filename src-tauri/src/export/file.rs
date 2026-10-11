//! The export's output file: written under a temporary name next to the target, moved into
//! place only when the export succeeded, deleted otherwise (error, cancel, dropped future).
//!
//! Replacing an existing file keeps its permission bits (and, where allowed, its owner and
//! group), refuses a file we may not write, and goes through symbolic links to the file they
//! point at. Temporary files of running exports are tracked so that `remove_partial_files` can
//! delete them when the app quits (in-flight futures are never dropped then); leftovers of a
//! crashed run are swept by the next export to the same file.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use crate::error::AppError;

/// Suffix of every temporary export file.
const TMP_SUFFIX: &str = ".tabler-export";
/// Random hex digits in a temporary name.
const TMP_RANDOM: usize = 12;
/// Longest file name (bytes) that every common file system accepts.
const NAME_MAX: usize = 255;
/// A temporary file of this name pattern that was not written for this long and does not
/// belong to an export of this process is a leftover (crash, forced quit) and may be removed.
const STALE_AFTER: Duration = Duration::from_secs(10 * 60);
/// Symbolic links followed at most when resolving the destination.
const MAX_LINKS: usize = 40;

/// Temporary files of the exports running in this process.
static LIVE: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn live() -> MutexGuard<'static, HashSet<PathBuf>> {
    LIVE.lock().unwrap_or_else(|p| p.into_inner())
}

/// Deletes the temporary files of every export still running in this process. Meant for app
/// exit (`RunEvent::Exit`): the runtime never drops in-flight command futures then, so their
/// `TempTarget`s cannot clean up. Returns how many files were removed.
pub fn remove_partial_files() -> usize {
    remove_live_files(|_| true)
}

fn remove_live_files(select: impl Fn(&Path) -> bool) -> usize {
    let mut live = live();
    let mut removed = 0;
    live.retain(|tmp| {
        if !select(tmp) {
            return true;
        }
        match std::fs::remove_file(tmp) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("export: cannot remove {}: {e}", tmp.display()),
        }
        false
    });
    removed
}

/// Moves file writes off the async worker thread: on Tokio's multi-thread runtime each write
/// (one per filled 256 KiB buffer) runs under `block_in_place`, so a slow or stalled disk does
/// not hold up the other tasks of that worker. Elsewhere it writes directly.
pub struct BlockingWrite<W: Write>(W);

impl<W: Write> BlockingWrite<W> {
    pub fn new(inner: W) -> BlockingWrite<W> {
        BlockingWrite(inner)
    }

    pub fn into_inner(self) -> W {
        self.0
    }

    fn blocking<R>(&mut self, op: impl FnOnce(&mut W) -> R) -> R {
        let multi_thread = tokio::runtime::Handle::try_current()
            .is_ok_and(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread);
        if multi_thread {
            tokio::task::block_in_place(|| op(&mut self.0))
        } else {
            op(&mut self.0)
        }
    }
}

impl<W: Write> Write for BlockingWrite<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.blocking(|w| w.write(buf))
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.blocking(|w| w.write_all(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.blocking(|w| w.flush())
    }
}

/// Counts the bytes written through it (shared with the progress reporter).
pub struct CountingWriter<W: Write> {
    inner: W,
    count: Arc<AtomicU64>,
}

impl<W: Write> CountingWriter<W> {
    pub fn new(inner: W, count: Arc<AtomicU64>) -> CountingWriter<W> {
        CountingWriter { inner, count }
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Checks the destination path: absolute, names a file, in an existing directory.
pub fn validate_target(path: &str) -> Result<PathBuf, AppError> {
    let dest = PathBuf::from(path);
    if path.trim().is_empty() || !dest.is_absolute() {
        return Err(AppError::InvalidInput(
            "the export path must be an absolute file path".into(),
        ));
    }
    if dest.file_name().is_none() || dest.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "\"{}\" is not a file path",
            dest.display()
        )));
    }
    match dest.parent() {
        Some(dir) if dir.is_dir() => Ok(dest),
        _ => Err(AppError::Io(format!(
            "the folder of \"{}\" does not exist",
            dest.display()
        ))),
    }
}

/// Follows symbolic links from `path` to the file they finally name (which may not exist yet).
fn resolve_links(path: &Path) -> Result<PathBuf, AppError> {
    let mut current = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = std::fs::read_link(&current).map_err(|e| {
                    AppError::Io(format!("cannot read link \"{}\": {e}", current.display()))
                })?;
                current = match current.parent() {
                    Some(dir) if target.is_relative() => dir.join(target),
                    _ => target,
                };
            }
            _ => return Ok(current),
        }
    }
    Err(AppError::Io(format!(
        "too many symbolic links in \"{}\"",
        path.display()
    )))
}

/// Cuts `name` to at most `max` bytes at a character boundary.
fn truncate_name(name: &str, max: usize) -> &str {
    if name.len() <= max {
        return name;
    }
    let mut end = max;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

/// `.<name>.` — the start of every temporary name for a destination called `name`, with `name`
/// shortened so the whole temporary name stays within `NAME_MAX` bytes.
fn tmp_prefix(name: &str) -> String {
    let budget = NAME_MAX - (1 + 1 + TMP_RANDOM + TMP_SUFFIX.len());
    format!(".{}.", truncate_name(name, budget))
}

/// Removes leftovers of earlier runs for the same destination (`prefix` from `tmp_prefix`):
/// files of the temporary name pattern that no export of this process owns and that were not
/// written for `STALE_AFTER`.
fn sweep_leftovers(dir: &Path, prefix: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let live = live();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(random) = name
            .to_str()
            .and_then(|n| n.strip_prefix(prefix))
            .and_then(|n| n.strip_suffix(TMP_SUFFIX))
        else {
            continue;
        };
        if random.len() != TMP_RANDOM || !random.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let path = entry.path();
        let stale = entry.file_type().is_ok_and(|t| t.is_file())
            && entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age >= STALE_AFTER);
        if stale && !live.contains(&path) {
            match std::fs::remove_file(&path) {
                Ok(()) => log::info!("export: removed leftover {}", path.display()),
                Err(e) => log::debug!("export: cannot remove leftover {}: {e}", path.display()),
            }
        }
    }
}

/// Gives the new file the permission bits of the file it replaces, and its owner and group
/// where the system allows it.
fn copy_access(file: &File, existing: &std::fs::Metadata) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if std::os::unix::fs::fchown(file, Some(existing.uid()), Some(existing.gid())).is_err() {
            // Not the owner's account: keep ours, but the group may still be ours to set.
            let _ = std::os::unix::fs::fchown(file, None, Some(existing.gid()));
        }
    }
    if let Err(e) = file.set_permissions(existing.permissions()) {
        log::warn!("export: cannot copy the permissions of the replaced file: {e}");
    }
}

/// A temporary file next to the destination. Dropping it without `commit` deletes it.
#[derive(Debug)]
pub struct TempTarget {
    tmp: PathBuf,
    /// The file the export replaces: the destination, or the file a symbolic link there names.
    dest: PathBuf,
    committed: bool,
}

impl TempTarget {
    /// Creates `.<name>.<random>.tabler-export` in the destination's folder (the folder of the
    /// linked file when the destination is a symbolic link). An existing destination must be
    /// writable; its permissions carry over to the new file.
    pub fn create(dest: &Path) -> Result<(TempTarget, File), AppError> {
        let dest = resolve_links(dest)?;
        let dir = dest
            .parent()
            .ok_or_else(|| AppError::InvalidInput("the export path has no folder".into()))?;
        let name = dest
            .file_name()
            .ok_or_else(|| AppError::InvalidInput("the export path has no file name".into()))?
            .to_string_lossy();
        let existing = match std::fs::metadata(&dest) {
            Ok(meta) if meta.is_dir() => {
                return Err(AppError::InvalidInput(format!(
                    "\"{}\" is a folder",
                    dest.display()
                )));
            }
            Ok(meta) => {
                // Opening for writing (no truncation) changes nothing, but tells whether the
                // user may modify the file: a read-only file is not silently replaced.
                if let Err(e) = OpenOptions::new().write(true).open(&dest) {
                    return Err(AppError::Io(format!(
                        "cannot replace \"{}\": {e}",
                        dest.display()
                    )));
                }
                Some(meta)
            }
            Err(_) => None,
        };
        let prefix = tmp_prefix(&name);
        sweep_leftovers(dir, &prefix);
        let random = uuid::Uuid::new_v4().simple().to_string();
        let tmp = dir.join(format!("{prefix}{}{TMP_SUFFIX}", &random[..TMP_RANDOM]));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| AppError::Io(format!("cannot create \"{}\": {e}", tmp.display())))?;
        live().insert(tmp.clone());
        let target = TempTarget {
            tmp,
            dest,
            committed: false,
        };
        if let Some(meta) = &existing {
            copy_access(&file, meta);
        }
        Ok((target, file))
    }

    pub fn tmp_path(&self) -> &Path {
        &self.tmp
    }

    /// The file the export replaces (symbolic links resolved).
    pub fn dest_path(&self) -> &Path {
        &self.dest
    }

    /// Moves the finished file over the destination (replacing an existing file).
    pub fn commit(mut self) -> Result<(), AppError> {
        std::fs::rename(&self.tmp, &self.dest)
            .map_err(|e| AppError::Io(format!("cannot write \"{}\": {e}", self.dest.display())))?;
        self.committed = true;
        live().remove(&self.tmp);
        Ok(())
    }
}

impl Drop for TempTarget {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Err(e) = std::fs::remove_file(&self.tmp)
            && e.kind() != io::ErrorKind::NotFound
        {
            log::warn!(
                "export: cannot remove temporary file {}: {e}",
                self.tmp.display()
            );
        }
        live().remove(&self.tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_bytes() {
        let count = Arc::new(AtomicU64::new(0));
        let mut w = CountingWriter::new(Vec::new(), count.clone());
        w.write_all(b"hello").unwrap();
        write!(w, " {}", 42).unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 8);
        assert_eq!(w.into_inner(), b"hello 42");
    }

    #[test]
    fn validates_paths() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validate_target("relative.csv").is_err());
        assert!(validate_target("").is_err());
        assert!(validate_target(&dir.path().to_string_lossy()).is_err());
        let missing = dir.path().join("nope").join("x.csv");
        assert!(validate_target(&missing.to_string_lossy()).is_err());
        let ok = dir.path().join("x.csv");
        assert_eq!(validate_target(&ok.to_string_lossy()).unwrap(), ok);
    }

    #[test]
    fn temp_file_is_removed_unless_committed() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.csv");
        let (target, mut file) = TempTarget::create(&dest).unwrap();
        let tmp = target.tmp_path().to_path_buf();
        assert_eq!(tmp.parent(), Some(dir.path()));
        file.write_all(b"partial").unwrap();
        drop(file);
        assert!(tmp.exists());
        drop(target);
        assert!(!tmp.exists());
        assert!(!dest.exists());

        std::fs::write(&dest, b"old").unwrap();
        let (target, mut file) = TempTarget::create(&dest).unwrap();
        let tmp = target.tmp_path().to_path_buf();
        file.write_all(b"new").unwrap();
        drop(file);
        target.commit().unwrap();
        assert!(!tmp.exists());
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    fn replace(dest: &Path, content: &[u8]) -> Result<(), AppError> {
        let (target, mut file) = TempTarget::create(dest)?;
        file.write_all(content).unwrap();
        drop(file);
        target.commit()
    }

    #[cfg(unix)]
    #[test]
    fn replacing_keeps_permissions_and_refuses_read_only_files() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;

        let private = dir.path().join("private.csv");
        std::fs::write(&private, b"old").unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600)).unwrap();
        replace(&private, b"new").unwrap();
        assert_eq!(std::fs::read(&private).unwrap(), b"new");
        assert_eq!(mode(&private), 0o600);

        let shared = dir.path().join("shared.csv");
        std::fs::write(&shared, b"old").unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o664)).unwrap();
        replace(&shared, b"new").unwrap();
        assert_eq!(mode(&shared), 0o664);

        let locked = dir.path().join("locked.csv");
        std::fs::write(&locked, b"old").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444)).unwrap();
        // (Root may write anything; then there is nothing to refuse.)
        if OpenOptions::new().write(true).open(&locked).is_err() {
            let err = replace(&locked, b"new").unwrap_err();
            assert!(matches!(err, AppError::Io(_)), "{err:?}");
            assert_eq!(std::fs::read(&locked).unwrap(), b"old");
            assert_eq!(mode(&locked), 0o444);
        }
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["locked.csv", "private.csv", "shared.csv"]);
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_symbolic_links() {
        let dir = tempfile::tempdir().unwrap();
        let real_dir = dir.path().join("real");
        std::fs::create_dir(&real_dir).unwrap();
        let real = real_dir.join("data.csv");
        std::fs::write(&real, b"old").unwrap();
        let link = dir.path().join("link.csv");
        std::os::unix::fs::symlink("real/data.csv", &link).unwrap();
        let chained = dir.path().join("chained.csv");
        std::os::unix::fs::symlink(&link, &chained).unwrap();

        let (target, mut file) = TempTarget::create(&chained).unwrap();
        assert_eq!(target.dest_path(), real.as_path());
        assert_eq!(target.tmp_path().parent(), Some(real_dir.as_path()));
        file.write_all(b"new").unwrap();
        drop(file);
        target.commit().unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::symlink_metadata(&chained)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&real).unwrap(), b"new");
        assert_eq!(std::fs::read(&link).unwrap(), b"new");

        // A dangling link: the file it names is created.
        let dangling = dir.path().join("dangling.csv");
        std::os::unix::fs::symlink("real/new.csv", &dangling).unwrap();
        replace(&dangling, b"fresh").unwrap();
        assert_eq!(std::fs::read(real_dir.join("new.csv")).unwrap(), b"fresh");
        assert!(
            std::fs::symlink_metadata(&dangling)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn long_file_names_get_a_short_enough_temporary_name() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            format!("{}.csv", "a".repeat(251)),
            format!("{}.csv", "a".repeat(224)),
            format!("{}.csv", "é".repeat(120)),
        ] {
            let dest = dir.path().join(&name);
            let (target, file) = TempTarget::create(&dest).unwrap();
            let tmp_name = target.tmp_path().file_name().unwrap().len();
            assert!(tmp_name <= NAME_MAX, "{tmp_name}");
            drop(file);
            target.commit().unwrap();
            assert!(dest.exists());
        }
        assert_eq!(truncate_name("aé", 2), "a");
        assert_eq!(truncate_name("abc", 5), "abc");
    }

    #[test]
    fn partial_files_are_removed_at_exit_and_leftovers_on_the_next_run() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("big.csv");
        let (running, mut file) = TempTarget::create(&dest).unwrap();
        file.write_all(b"partial").unwrap();
        let tmp = running.tmp_path().to_path_buf();
        // What the app's exit hook does (limited to this test's folder: other tests run
        // exports in parallel).
        assert_eq!(remove_live_files(|p| p.starts_with(dir.path())), 1);
        assert!(!tmp.exists());
        // The export never finishes (the process exits); its drop finds nothing to remove.
        std::mem::forget(running);

        // A leftover of a killed run, not touched for a while, is removed by the next export
        // to the same file; a recent one (maybe another process's) and other files are kept.
        let old = dir.path().join(".big.csv.0123456789ab.tabler-export");
        let recent = dir.path().join(".big.csv.ba9876543210.tabler-export");
        let other = dir.path().join(".other.csv.0123456789ab.tabler-export");
        let unrelated = dir.path().join(".big.csv.notes");
        for p in [&old, &recent, &other, &unrelated] {
            std::fs::write(p, b"x").unwrap();
        }
        let past = std::time::SystemTime::now() - Duration::from_secs(3600);
        for p in [&old, &other, &unrelated] {
            File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(past)
                .unwrap();
        }
        replace(&dest, b"done").unwrap();
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(other.exists());
        assert!(unrelated.exists());
        assert_eq!(std::fs::read(&dest).unwrap(), b"done");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_writes_work_on_both_runtimes() {
        let mut w = BlockingWrite::new(Vec::new());
        w.write_all(b"multi").unwrap();
        w.flush().unwrap();
        let joined = tokio::task::spawn_blocking(move || {
            w.write_all(b" blocking").unwrap();
            w
        })
        .await
        .unwrap();
        assert_eq!(joined.into_inner(), b"multi blocking");
        let mut w = BlockingWrite::new(Vec::new());
        w.write_all(b"plain").unwrap();
        assert_eq!(w.into_inner(), b"plain");
    }
}
