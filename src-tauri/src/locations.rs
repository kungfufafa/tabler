//! Settings ▸ Locations (TablePlus's Preferences ▸ Locations): the folder of the app database
//! (connections, settings and history) and the folder of the saved queries can be moved, e.g. to
//! Dropbox or iCloud Drive. Passwords stay in the Keychain.
//!
//! A small pointer file in the default app data folder (`locations.json`) names the chosen
//! folders. It is read once at startup, before the app database opens:
//! - a chosen folder that is missing (a drive or cloud folder not mounted) is never created: this
//!   session falls back to the default folder and reports a warning; nothing is written to, or
//!   deleted from, the chosen folder, and the next start tries it again;
//! - moving the app database is done at the next start, while nothing has it open: it is copied
//!   with `VACUUM INTO` (a consistent snapshot, WAL included) and checked; the old copy stays.
//!   If the copy fails, the old folder stays in use.
//!
//! The queries folder changes at once: its `.sql` files are copied over, then removed from the old
//! folder once the new location is saved.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::query_files;
use crate::types::locations::{LocationChange, LocationInspection, LocationKind, LocationsInfo};

/// The pointer file, in the default app data folder.
pub const POINTER_FILE: &str = "locations.json";
/// The app database file, in the data folder.
pub const DB_FILE: &str = "tabler.sqlite";
/// The default queries folder, inside the default app data folder.
pub const QUERIES_DIR: &str = "Queries";
/// `.sql` files counted when inspecting a folder.
const COUNT_MAX: u32 = 10_000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Pointer {
    /// The data folder; `None` = the default app data folder.
    pub data_dir: Option<PathBuf>,
    /// The queries folder; `None` = `<default app data>/Queries`.
    pub queries_dir: Option<PathBuf>,
    /// An app database move done at the next start.
    pub pending_data_move: Option<PendingMove>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingMove {
    pub from: PathBuf,
    pub to: PathBuf,
    /// `to` already holds an app database: keep it as a backup file, then copy.
    #[serde(default)]
    pub replace: bool,
}

pub fn read_pointer(default_dir: &Path) -> Result<Pointer, AppError> {
    let path = default_dir.join(POINTER_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(serde_json::from_str(&text)?),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Pointer::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn write_pointer(default_dir: &Path, pointer: &Pointer) -> Result<(), AppError> {
    std::fs::create_dir_all(default_dir)?;
    let tmp = default_dir.join(".locations.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(pointer)?)?;
    std::fs::rename(&tmp, default_dir.join(POINTER_FILE))?;
    Ok(())
}

/// Whether two folders are the same (resolved, so `/var` and `/private/var` match).
pub fn same_dir(a: &Path, b: &Path) -> bool {
    query_files::resolve(a) == query_files::resolve(b)
}

fn custom_or_none(dir: &Path, default: &Path) -> Option<PathBuf> {
    (!same_dir(dir, default)).then(|| dir.to_path_buf())
}

fn display(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Renames `db` (and its `-wal` / `-shm` files) to `tabler-backup-<time>.sqlite…` next to it.
pub fn backup_db(db: &Path) -> Result<PathBuf, AppError> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup = db.with_file_name(format!("tabler-backup-{stamp}.sqlite"));
    std::fs::rename(db, &backup)?;
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", db.display()));
        if side.exists() {
            let _ = std::fs::rename(&side, format!("{}{suffix}", backup.display()));
        }
    }
    Ok(backup)
}

/// Undoes [`backup_db`]: the backup (and its `-wal` / `-shm` files) takes `db`'s name again.
fn restore_backup(db: &Path, backup: &Path) -> Result<(), AppError> {
    std::fs::rename(backup, db)?;
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", backup.display()));
        if side.exists() {
            let _ = std::fs::rename(&side, format!("{}{suffix}", db.display()));
        }
    }
    Ok(())
}

/// The name an app database copy has until it is checked (next to its destination).
fn moving_file(dst: &Path) -> PathBuf {
    dst.with_file_name(".tabler.sqlite.moving")
}

/// Copies an app database with `VACUUM INTO` into a temporary file next to `dst` and checks it.
/// Returns the temporary file; nothing at `dst` is touched, and a failure leaves no file behind.
fn copy_db_checked(src: &Path, dst: &Path) -> Result<PathBuf, AppError> {
    let db = |e: rusqlite::Error| AppError::AppDb(e.to_string());
    let tmp = moving_file(dst);
    let _ = std::fs::remove_file(&tmp);
    let copied = (|| {
        {
            let conn = rusqlite::Connection::open(src).map_err(db)?;
            conn.busy_timeout(std::time::Duration::from_secs(5))
                .map_err(db)?;
            conn.execute("VACUUM INTO ?1", [display(&tmp)])
                .map_err(db)?;
        }
        let check = (|| {
            let conn = rusqlite::Connection::open(&tmp)?;
            conn.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
        })();
        match check {
            Ok(ok) if ok == "ok" => Ok(()),
            Ok(problem) => Err(AppError::AppDb(format!(
                "the copy failed its check: {problem}"
            ))),
            Err(e) => Err(db(e)),
        }
    })();
    match copied {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Carries out a pending app database move (at startup, before the database opens).
///
/// A copy is made and checked first; only then does an existing database at the target (`replace`)
/// move aside as a backup, and the copy takes its name. A copy that fails (the source is locked,
/// the check fails, the disk is full) therefore leaves the target folder as it was: another
/// computer using that folder (Dropbox, iCloud Drive) never finds it without its `tabler.sqlite`.
pub fn move_app_db(mv: &PendingMove) -> Result<(), AppError> {
    if !mv.to.is_dir() {
        return Err(AppError::Io(format!(
            "{} is not available",
            mv.to.display()
        )));
    }
    let src = mv.from.join(DB_FILE);
    let dst = mv.to.join(DB_FILE);
    let occupied = dst.exists();
    if occupied && !mv.replace {
        return Err(AppError::InvalidInput(
            "the folder already holds Tabler data".into(),
        ));
    }
    if !src.exists() {
        // Nothing to copy: a replace still sets the target's database aside.
        if occupied {
            backup_db(&dst)?;
        }
        return Ok(());
    }
    let tmp = copy_db_checked(&src, &dst)?;
    let backup = if occupied {
        match backup_db(&dst) {
            Ok(backup) => Some(backup),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    } else {
        None
    };
    if let Err(e) = std::fs::rename(&tmp, &dst) {
        let _ = std::fs::remove_file(&tmp);
        if let Some(backup) = backup
            && let Err(restore) = restore_backup(&dst, &backup)
        {
            log::error!(
                "could not put {} back after a failed move: {restore}; it is {}",
                dst.display(),
                backup.display()
            );
        }
        return Err(e.into());
    }
    Ok(())
}

/// What startup resolved: the folders this session uses and what went wrong.
#[derive(Debug, Clone)]
pub struct Startup {
    pub data_dir: PathBuf,
    pub data_fallback: bool,
    pub queries_dir: PathBuf,
    pub queries_fallback: bool,
    pub pointer: Pointer,
    pub warnings: Vec<String>,
}

/// Reads the pointer file, carries out a pending database move and picks the folders, falling
/// back to the defaults (with a warning) for a chosen folder that is not there.
pub fn resolve_startup(default_dir: &Path) -> Startup {
    let mut warnings = Vec::new();
    let mut pointer = read_pointer(default_dir).unwrap_or_else(|e| {
        warnings.push(format!(
            "Could not read the Locations settings ({e}); the default folders are used."
        ));
        Pointer::default()
    });
    if let Some(mv) = pointer.pending_data_move.take() {
        match move_app_db(&mv) {
            Ok(()) => pointer.data_dir = custom_or_none(&mv.to, default_dir),
            Err(e) => {
                warnings.push(format!(
                    "Could not move the connections & settings to {}: {e}. Tabler still uses {}.",
                    mv.to.display(),
                    mv.from.display()
                ));
                pointer.data_dir = custom_or_none(&mv.from, default_dir);
            }
        }
        if let Err(e) = write_pointer(default_dir, &pointer) {
            warnings.push(format!("Could not update the Locations settings: {e}"));
        }
    }
    let (data_dir, data_fallback) = match &pointer.data_dir {
        Some(dir) if dir.is_dir() => (dir.clone(), false),
        Some(dir) => {
            warnings.push(format!(
                "The connections & settings folder {} is not available (is its drive or cloud \
                 folder mounted?). This session uses the default folder; nothing was changed in \
                 the chosen one.",
                dir.display()
            ));
            (default_dir.to_path_buf(), true)
        }
        None => (default_dir.to_path_buf(), false),
    };
    let default_queries = default_dir.join(QUERIES_DIR);
    let (queries_dir, queries_fallback) = match &pointer.queries_dir {
        Some(dir) if dir.is_dir() => (dir.clone(), false),
        Some(dir) => {
            warnings.push(format!(
                "The queries folder {} is not available (is its drive or cloud folder mounted?). \
                 This session reads and saves queries in the default folder.",
                dir.display()
            ));
            (default_queries, true)
        }
        None => (default_queries, false),
    };
    Startup {
        data_dir,
        data_fallback,
        queries_dir,
        queries_fallback,
        pointer,
        warnings,
    }
}

/// Startup: resolves the folders and opens the app database, falling back to the default folder
/// when the chosen one cannot be opened. Returns it with the Locations state to manage.
pub fn open_app_db(default_dir: PathBuf) -> Result<(crate::appdb::AppDb, Locations), AppError> {
    let mut startup = resolve_startup(&default_dir);
    let appdb = match crate::appdb::AppDb::open(&startup.data_dir.join(DB_FILE)) {
        Ok(db) => db,
        Err(e) if !same_dir(&startup.data_dir, &default_dir) => {
            startup.warnings.push(format!(
                "Could not open the app database in {} ({e}). This session uses the default folder.",
                startup.data_dir.display()
            ));
            startup.data_dir = default_dir.clone();
            startup.data_fallback = true;
            crate::appdb::AppDb::open(&default_dir.join(DB_FILE))?
        }
        Err(e) => return Err(e),
    };
    for w in &startup.warnings {
        log::warn!("{w}");
    }
    Ok((appdb, Locations::new(default_dir, startup)))
}

/// Number of `.sql` files under `dir` (hidden entries skipped), up to `COUNT_MAX`.
fn count_sql_files(dir: &Path, depth: usize) -> u32 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0u32;
    for e in entries.filter_map(Result::ok) {
        if n >= COUNT_MAX {
            break;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() && depth < 24 => n += count_sql_files(&e.path(), depth + 1),
            Ok(_)
                if Path::new(&name)
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("sql")) =>
            {
                n += 1
            }
            _ => {}
        }
    }
    n.min(COUNT_MAX)
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".tabler-write-test-{}", uuid::Uuid::new_v4()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

struct Inner {
    pointer: Pointer,
    queries_dir: PathBuf,
    queries_fallback: bool,
    /// The data folder was changed this session (it applies at the next start).
    data_changed: bool,
}

/// The folders in use, managed as Tauri state.
pub struct Locations {
    default_dir: PathBuf,
    data_dir: PathBuf,
    data_fallback: bool,
    warnings: Vec<String>,
    inner: Mutex<Inner>,
}

impl Locations {
    pub fn new(default_dir: PathBuf, startup: Startup) -> Locations {
        Locations {
            default_dir,
            data_dir: startup.data_dir,
            data_fallback: startup.data_fallback,
            warnings: startup.warnings,
            inner: Mutex::new(Inner {
                pointer: startup.pointer,
                queries_dir: startup.queries_dir,
                queries_fallback: startup.queries_fallback,
                data_changed: false,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn default_queries_dir(&self) -> PathBuf {
        self.default_dir.join(QUERIES_DIR)
    }

    /// The app database this session.
    pub fn data_file(&self) -> PathBuf {
        self.data_dir.join(DB_FILE)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The saved queries' folder this session.
    pub fn queries_dir(&self) -> PathBuf {
        self.lock().queries_dir.clone()
    }

    pub fn info(&self) -> LocationsInfo {
        let inner = self.lock();
        let configured_data = inner
            .pointer
            .pending_data_move
            .as_ref()
            .map(|m| m.to.clone())
            .or_else(|| inner.pointer.data_dir.clone());
        let effective = configured_data
            .clone()
            .unwrap_or_else(|| self.default_dir.clone());
        let pending = (inner.data_changed && !same_dir(&effective, &self.data_dir))
            .then(|| display(&effective));
        LocationsInfo {
            data_dir: display(&self.data_dir),
            default_data_dir: display(&self.default_dir),
            configured_data_dir: configured_data.as_deref().map(display),
            data_file: display(&self.data_file()),
            data_fallback: self.data_fallback,
            pending_data_dir: pending,
            queries_dir: display(&inner.queries_dir),
            default_queries_dir: display(&self.default_queries_dir()),
            configured_queries_dir: inner.pointer.queries_dir.as_deref().map(display),
            queries_fallback: inner.queries_fallback,
            warnings: self.warnings.clone(),
        }
    }

    /// Checks a folder picked for `kind` before the change is confirmed.
    pub fn inspect(&self, kind: LocationKind, path: &Path) -> LocationInspection {
        let mut out = LocationInspection {
            path: display(path),
            is_current: false,
            has_data: false,
            query_files: 0,
            problem: None,
        };
        if !path.is_absolute() {
            out.problem = Some("Choose a folder (an absolute path).".into());
            return out;
        }
        if !path.is_dir() {
            out.problem = Some(if path.exists() {
                "That is not a folder.".into()
            } else {
                "The folder does not exist.".into()
            });
            return out;
        }
        if !writable(path) {
            out.problem = Some("Tabler cannot write to this folder.".into());
            return out;
        }
        let inner = self.lock();
        match kind {
            LocationKind::Data => {
                let configured = inner
                    .pointer
                    .pending_data_move
                    .as_ref()
                    .map(|m| m.to.clone())
                    .or_else(|| inner.pointer.data_dir.clone())
                    .unwrap_or_else(|| self.default_dir.clone());
                out.is_current = same_dir(path, &configured);
                out.has_data = path.join(DB_FILE).exists();
            }
            LocationKind::Queries => {
                out.is_current = same_dir(path, &inner.queries_dir);
                out.query_files = count_sql_files(path, 0);
                out.has_data = out.query_files > 0;
                if !out.is_current
                    && (query_files::is_within(path, &inner.queries_dir)
                        || query_files::is_within(&inner.queries_dir, path))
                {
                    out.problem = Some(
                        "The folder is inside the current queries folder, or holds it.".into(),
                    );
                }
            }
        }
        out
    }

    /// Chooses the data folder (`None`: the default). It applies at the next start; `Move` /
    /// `Replace` copy the current app database there first.
    pub fn set_data(
        &self,
        path: Option<&Path>,
        change: LocationChange,
    ) -> Result<LocationsInfo, AppError> {
        let target = path
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.default_dir.clone());
        let inspection = self.inspect(LocationKind::Data, &target);
        if let Some(problem) = inspection.problem {
            return Err(AppError::InvalidInput(problem));
        }
        {
            let mut inner = self.lock();
            let mut pointer = inner.pointer.clone();
            if same_dir(&target, &self.data_dir) {
                // Back to the folder in use: whatever was pending is dropped.
                pointer.pending_data_move = None;
                pointer.data_dir = custom_or_none(&self.data_dir, &self.default_dir);
            } else {
                match change {
                    LocationChange::Move | LocationChange::Replace => {
                        if inspection.has_data && change == LocationChange::Move {
                            return Err(AppError::InvalidInput(
                                "The folder already holds Tabler data: use it, or replace it."
                                    .into(),
                            ));
                        }
                        pointer.pending_data_move = Some(PendingMove {
                            from: self.data_dir.clone(),
                            to: target.clone(),
                            replace: change == LocationChange::Replace,
                        });
                        pointer.data_dir = custom_or_none(&self.data_dir, &self.default_dir);
                    }
                    LocationChange::Empty => {
                        if inspection.has_data {
                            return Err(AppError::InvalidInput(
                                "The folder already holds Tabler data.".into(),
                            ));
                        }
                        pointer.pending_data_move = None;
                        pointer.data_dir = custom_or_none(&target, &self.default_dir);
                    }
                    LocationChange::UseExisting => {
                        if !inspection.has_data {
                            return Err(AppError::InvalidInput(
                                "The folder holds no Tabler data.".into(),
                            ));
                        }
                        pointer.pending_data_move = None;
                        pointer.data_dir = custom_or_none(&target, &self.default_dir);
                    }
                }
            }
            write_pointer(&self.default_dir, &pointer)?;
            inner.pointer = pointer;
            inner.data_changed = true;
        }
        Ok(self.info())
    }

    /// Chooses the queries folder (`None`: the default) at once. `Move` / `Replace` bring the
    /// current queries along (merged into what the folder holds); the others just switch.
    pub fn set_queries(
        &self,
        path: Option<&Path>,
        change: LocationChange,
    ) -> Result<LocationsInfo, AppError> {
        let default = self.default_queries_dir();
        let target = path
            .map(Path::to_path_buf)
            .unwrap_or_else(|| default.clone());
        if same_dir(&target, &default) {
            std::fs::create_dir_all(&target)?;
        }
        let inspection = self.inspect(LocationKind::Queries, &target);
        if let Some(problem) = inspection.problem {
            return Err(AppError::InvalidInput(problem));
        }
        {
            let mut inner = self.lock();
            let current = inner.queries_dir.clone();
            let moving = !inspection.is_current
                && matches!(change, LocationChange::Move | LocationChange::Replace);
            let copied = if moving {
                query_files::copy_sql_tree(&current, &target)?
            } else {
                Vec::new()
            };
            let mut pointer = inner.pointer.clone();
            pointer.queries_dir = custom_or_none(&target, &default);
            write_pointer(&self.default_dir, &pointer)?;
            // The new location is saved: only now do the originals go.
            query_files::remove_copied(&current, &copied);
            inner.pointer = pointer;
            inner.queries_dir = target;
            inner.queries_fallback = false;
        }
        Ok(self.info())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db(dir: &Path, marker: &str) {
        let conn = rusqlite::Connection::open(dir.join(DB_FILE)).unwrap();
        conn.execute_batch(&format!(
            "PRAGMA journal_mode = WAL; CREATE TABLE t (v TEXT); INSERT INTO t VALUES ('{marker}');"
        ))
        .unwrap();
    }

    fn marker(dir: &Path) -> String {
        let conn = rusqlite::Connection::open(dir.join(DB_FILE)).unwrap();
        conn.query_row("SELECT v FROM t", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn defaults_without_a_pointer_file() {
        let tmp = tempfile::tempdir().unwrap();
        let s = resolve_startup(tmp.path());
        assert_eq!(s.data_dir, tmp.path());
        assert_eq!(s.queries_dir, tmp.path().join(QUERIES_DIR));
        assert!(s.warnings.is_empty());
        assert!(!s.data_fallback && !s.queries_fallback);
    }

    #[test]
    fn missing_folders_fall_back_without_being_created() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("Volumes/Dropbox/Tabler");
        write_pointer(
            tmp.path(),
            &Pointer {
                data_dir: Some(gone.clone()),
                queries_dir: Some(gone.join("Queries")),
                pending_data_move: None,
            },
        )
        .unwrap();
        let s = resolve_startup(tmp.path());
        assert_eq!(s.data_dir, tmp.path());
        assert!(s.data_fallback && s.queries_fallback);
        assert_eq!(s.warnings.len(), 2);
        assert!(!gone.exists());
        // The pointer is kept: the next start tries the folder again.
        assert_eq!(read_pointer(tmp.path()).unwrap().data_dir, Some(gone));
        // A corrupt pointer file: defaults and a warning.
        std::fs::write(tmp.path().join(POINTER_FILE), "{not json").unwrap();
        let s = resolve_startup(tmp.path());
        assert_eq!(s.data_dir, tmp.path());
        assert_eq!(s.warnings.len(), 1);
    }

    #[test]
    fn pending_moves_copy_the_database_at_startup() {
        let tmp = tempfile::tempdir().unwrap();
        let default = tmp.path().join("default");
        let cloud = tmp.path().join("cloud");
        std::fs::create_dir_all(&default).unwrap();
        std::fs::create_dir_all(&cloud).unwrap();
        make_db(&default, "current");
        let locations = Locations::new(default.clone(), resolve_startup(&default));
        let info = locations
            .set_data(Some(&cloud), LocationChange::Move)
            .unwrap();
        assert_eq!(
            info.pending_data_dir.as_deref(),
            Some(display(&cloud).as_str())
        );
        // Nothing moves before the restart.
        assert!(!cloud.join(DB_FILE).exists());
        let s = resolve_startup(&default);
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        assert!(same_dir(&s.data_dir, &cloud));
        assert_eq!(marker(&cloud), "current");
        // The old copy stays.
        assert_eq!(marker(&default), "current");
        assert_eq!(read_pointer(&default).unwrap().pending_data_move, None);

        // Moving back into a folder that holds data needs Replace, which keeps a backup.
        let locations = Locations::new(default.clone(), resolve_startup(&default));
        assert!(locations.set_data(None, LocationChange::Move).is_err());
        locations.set_data(None, LocationChange::Replace).unwrap();
        let s = resolve_startup(&default);
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        assert_eq!(s.data_dir, default);
        let backups = std::fs::read_dir(&default)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("tabler-backup-")
            })
            .count();
        assert!(backups >= 1);
        assert_eq!(read_pointer(&default).unwrap().data_dir, None);
    }

    fn backups_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("tabler-backup-"))
            .collect();
        names.sort();
        names
    }

    /// Replacing the data of a folder that is shared with another computer: when the copy fails
    /// that computer must still find its `tabler.sqlite` there.
    #[test]
    fn a_failed_replace_leaves_the_target_database_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let default = tmp.path().join("default");
        let cloud = tmp.path().join("cloud");
        std::fs::create_dir_all(&default).unwrap();
        std::fs::create_dir_all(&cloud).unwrap();
        make_db(&cloud, "theirs");
        // The source is not a database: the copy fails.
        std::fs::write(default.join(DB_FILE), "this is not a database, just text").unwrap();
        let mv = PendingMove {
            from: default.clone(),
            to: cloud.clone(),
            replace: true,
        };
        let err = move_app_db(&mv).unwrap_err();
        assert!(matches!(err, AppError::AppDb(_)), "{err:?}");
        assert_eq!(marker(&cloud), "theirs");
        assert!(backups_in(&cloud).is_empty(), "{:?}", backups_in(&cloud));
        // No half-made copy next to it either.
        assert!(!cloud.join(".tabler.sqlite.moving").exists());

        // The same through startup: the old folder stays in use, the target is untouched.
        write_pointer(
            &default,
            &Pointer {
                pending_data_move: Some(mv),
                ..Pointer::default()
            },
        )
        .unwrap();
        let s = resolve_startup(&default);
        assert_eq!(s.warnings.len(), 1, "{:?}", s.warnings);
        assert_eq!(s.data_dir, default);
        assert_eq!(marker(&cloud), "theirs");
        assert!(backups_in(&cloud).is_empty());
        assert!(!cloud.join(".tabler.sqlite.moving").exists());
    }

    #[test]
    fn a_replace_sets_the_old_database_aside_after_the_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let default = tmp.path().join("default");
        let cloud = tmp.path().join("cloud");
        std::fs::create_dir_all(&default).unwrap();
        std::fs::create_dir_all(&cloud).unwrap();
        make_db(&default, "mine");
        make_db(&cloud, "theirs");
        // The other computer keeps the database open: its -wal and -shm files exist.
        let live = rusqlite::Connection::open(cloud.join(DB_FILE)).unwrap();
        live.execute_batch("PRAGMA journal_mode = WAL; INSERT INTO t VALUES ('more');")
            .unwrap();
        assert!(cloud.join("tabler.sqlite-wal").exists());

        move_app_db(&PendingMove {
            from: default.clone(),
            to: cloud.clone(),
            replace: true,
        })
        .unwrap();
        assert_eq!(marker(&cloud), "mine");
        assert!(!cloud.join(".tabler.sqlite.moving").exists());
        let backups = backups_in(&cloud);
        assert!(
            backups.iter().any(|n| n.ends_with(".sqlite-wal")),
            "the -wal file went with the backup: {backups:?}"
        );
        let backup = backups.iter().find(|n| n.ends_with(".sqlite")).unwrap();
        let old = rusqlite::Connection::open(cloud.join(backup)).unwrap();
        let rows: i64 = old
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2);
        drop(live);
    }

    #[test]
    fn a_backup_can_be_put_back() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join(DB_FILE);
        std::fs::write(&db, "main").unwrap();
        std::fs::write(tmp.path().join("tabler.sqlite-wal"), "wal").unwrap();
        std::fs::write(tmp.path().join("tabler.sqlite-shm"), "shm").unwrap();
        let backup = backup_db(&db).unwrap();
        assert!(!db.exists() && !tmp.path().join("tabler.sqlite-wal").exists());
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "main");

        restore_backup(&db, &backup).unwrap();
        assert_eq!(std::fs::read_to_string(&db).unwrap(), "main");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("tabler.sqlite-wal")).unwrap(),
            "wal"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("tabler.sqlite-shm")).unwrap(),
            "shm"
        );
        assert!(backups_in(tmp.path()).is_empty());
    }

    #[test]
    fn a_failed_move_keeps_the_old_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let default = tmp.path().to_path_buf();
        make_db(&default, "x");
        write_pointer(
            &default,
            &Pointer {
                pending_data_move: Some(PendingMove {
                    from: default.clone(),
                    to: default.join("missing"),
                    replace: false,
                }),
                ..Pointer::default()
            },
        )
        .unwrap();
        let s = resolve_startup(&default);
        assert_eq!(s.data_dir, default);
        assert_eq!(s.warnings.len(), 1);
        assert_eq!(read_pointer(&default).unwrap(), Pointer::default());
    }

    #[test]
    fn data_changes_validate_the_target() {
        let tmp = tempfile::tempdir().unwrap();
        let default = tmp.path().join("d");
        let other = tmp.path().join("o");
        std::fs::create_dir_all(&default).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let locations = Locations::new(default.clone(), resolve_startup(&default));
        assert!(
            locations
                .set_data(Some(&other), LocationChange::UseExisting)
                .is_err()
        );
        let info = locations
            .set_data(Some(&other), LocationChange::Empty)
            .unwrap();
        assert!(info.pending_data_dir.is_some());
        assert!(
            locations
                .set_data(Some(&tmp.path().join("nope")), LocationChange::Empty)
                .is_err()
        );
        // Choosing the folder in use again cancels the change.
        let info = locations.set_data(None, LocationChange::Move).unwrap();
        assert_eq!(info.pending_data_dir, None);
        assert_eq!(read_pointer(&default).unwrap(), Pointer::default());
    }

    #[test]
    fn queries_move_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let default = tmp.path().join("d");
        std::fs::create_dir_all(default.join("Queries/Reports")).unwrap();
        std::fs::write(default.join("Queries/Reports/a.sql"), "select 1").unwrap();
        let cloud = tmp.path().join("cloud");
        std::fs::create_dir_all(&cloud).unwrap();
        std::fs::write(cloud.join("a.sql"), "select 2").unwrap();
        let locations = Locations::new(default.clone(), resolve_startup(&default));
        let seen = locations.inspect(LocationKind::Queries, &cloud);
        assert_eq!(
            (seen.has_data, seen.query_files, seen.problem),
            (true, 1, None)
        );
        let inside = locations.inspect(LocationKind::Queries, &default.join("Queries/Reports"));
        assert!(inside.problem.is_some());
        let info = locations
            .set_queries(Some(&cloud), LocationChange::Move)
            .unwrap();
        assert!(same_dir(Path::new(&info.queries_dir), &cloud));
        assert!(cloud.join("Reports/a.sql").is_file());
        assert!(cloud.join("a.sql").is_file());
        assert!(!default.join("Queries/Reports/a.sql").exists());
        assert_eq!(
            read_pointer(&default).unwrap().queries_dir,
            Some(cloud.clone())
        );
        // Back to the default, starting empty there.
        let info = locations.set_queries(None, LocationChange::Empty).unwrap();
        assert_eq!(info.configured_queries_dir, None);
        assert!(cloud.join("a.sql").is_file());
    }
}
