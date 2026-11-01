//! DuckDB databases: one engine instance per file per process, the settings it is opened with,
//! and the hardening of read-only and unattended sessions.
//!
//! DuckDB opens a file read-write in one process at a time, and a second `Connection::open` of
//! a file this process has open fails on the file lock, so every session of a file shares one
//! instance (`Sharing::SingleProcess`): the registry keeps it per canonical path, hands it out
//! again to a session with the same settings and refuses one with other settings. A session's
//! connections are clones of the instance's (`try_clone`); the file closes when the last one
//! goes. An in-memory database is its own instance each time.
//!
//! Most settings are the instance's, not a connection's: access mode, external access, the
//! configuration lock, extension loading, ATTACHed databases. So the startup SQL runs once per
//! instance, except `USE` and `SET schema` / `search_path`, which every new connection repeats.
//!
//! **Hardening.** The classifier cannot see a file named where a table goes (`FROM 'x.csv'`,
//! `FROM data.csv`), so read-only and MCP sessions are opened with the engine refusing them
//! (plugin sessions too, once the driver can tell them: the brief's change requests). The open
//! configuration turns extension autoload and autoinstall, community extensions and persistent
//! secrets off, and sets no temp directory (`temp_directory = ''`: no spilling to
//! disk), because DuckDB lets SQL read and write its temp directory once external access is off
//! (it adds it to `allowed_directories`; for an in-memory database that is `.tmp` under the
//! process's working directory). The profile's startup SQL runs first (a core extension it
//! INSTALLs or LOADs stays loaded, its choice; community extensions are refused), then `SET
//! enable_external_access = false`: no file but those DuckDB allows itself (the database files
//! it attached and the WAL paths beside them, `<db>.wal`, `.wal.checkpoint`, `.wal.recovery`), no
//! ATTACH of another file, no extension loading, no HTTP. When no attached database file takes
//! writes and the startup SQL allowed no folder, file or temp directory of its own (always for a
//! read-only profile without them), `SET disabled_filesystems = 'LocalFileSystem'` turns the
//! local file system off as well, so SQL reaches no file at all (not even those WAL paths, which
//! COPY could otherwise create beside a read-only file) and the databases are read through the
//! handles DuckDB opened before. Last `SET lock_configuration = true`, after which `SET` /
//! `RESET` (and `PRAGMA x = …`) of a setting fail, so all of it stays. A read-only file is opened
//! with `access_mode=READ_ONLY` besides.
//!
//! A hardened instance that takes writes keeps the paths DuckDB allows itself: `COPY … TO
//! '<db>.wal'` creates or overwrites the file there (and, with `USE_TMP_FILE false` on macOS and
//! Linux, could overwrite the database file), so such a session can damage the database it may
//! write anyway; every other path is refused.
//!
//! The lock covers only SET and RESET. The table and pragma functions that switch the logger and
//! the profiler (`enable_logging`, `enable_profiling`, `PRAGMA enable_profiling`, …) and the
//! pragmas of the optimizer, verification, the progress bar and checkpoint-on-shutdown still
//! change the instance or the connection. None of them reaches a file outside the paths above.
//! `enable_logging` with file storage is refused where the local file system is off (always on
//! a read-only profile); where it is on, any path is accepted and only fails when the logger
//! flushes. DuckDB writes log entries inside destructors (a file handle opened while
//! file-system logging is on logs its close from `~FileHandle`), so a flush that fails there
//! throws from a destructor and aborts the process, in the middle of an ordinary read or write
//! (a CSV read, `CREATE TABLE`), not only on close. `enable_profiling` with a save location
//! fails the connection's statements until `disable_profiling`. The classifier calls them
//! writes; a session that lets writes through has to refuse them itself (see the brief's
//! Limits).
//!
//! **A switched logger.** No check of the SQL text finds every call of `enable_logging` (a view
//! or a table macro stored in the file, dynamic SQL built from pieces), and no setting stops it
//! (the configuration lock does not cover it). So every job of a connection runs through
//! [`Instance::guarded`], which asks the logger's storage before the job and after it: anything
//! but `memory` (or a check that fails, which a failing flush makes it do) breaks the instance,
//! and from then on none of its connections runs a statement or opens again. A connection whose
//! transaction failed cannot answer (the statement that switched the logger may have failed it
//! on purpose), so the root connection answers for it: the storage is the instance's. Connections close through
//! [`Instance::release`]: on a broken instance, or one whose logger is switched, they are
//! leaked, never closed, and so is the root connection; otherwise it first ends the
//! connection's transaction with a `ROLLBACK` of its own, since DuckDB's destructor would roll
//! it back where nothing catches. The file stays held until the app quits, and the file stays
//! refused for as long, after its last session closed too ([`refuse_broken_file`]): DuckDB's
//! file lock (`fcntl` on macOS and Linux) belongs to the process, so it would not stop a second
//! instance of the file. The guard's own queries call the builtins as `system.main.<fn>`, so a
//! macro the file stores under a builtin's name cannot answer for them.
//!
//! What this cannot stop: the statement that switches the logger runs to its end, and so does
//! a statement already running on another connection of the instance. If that same statement
//! (or the other one) then opens and closes a file, or writes, the failing flush still aborts
//! the process. Closing that needs DuckDB to refuse `enable_logging` once the configuration is
//! locked (the brief's Limits).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Weak};

use ::duckdb::{AccessMode, Config, Connection};

use super::errors;
use crate::error::AppError;
use crate::sqlgen::dialect::duckdb::{LEX, attach_encrypted, use_database};
use crate::types::value::Value;

/// Where the database lives.
#[derive(Clone, PartialEq, Eq)]
pub enum Location {
    File(PathBuf),
    Memory,
}

impl std::fmt::Debug for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Location::File(p) => write!(f, "File({})", p.display()),
            Location::Memory => f.write_str("Memory"),
        }
    }
}

impl Location {
    /// `:memory:` (or nothing) is an in-memory database; anything else a file.
    pub fn parse(path: &str) -> Location {
        let path = path.trim();
        if path.is_empty() || path.eq_ignore_ascii_case(":memory:") {
            Location::Memory
        } else {
            Location::File(PathBuf::from(path))
        }
    }
}

/// How an instance is opened. Two sessions of one file share an instance only when these are
/// equal.
#[derive(Clone, PartialEq, Eq)]
pub struct EngineConfig {
    pub location: Location,
    /// `access_mode=READ_ONLY`: the engine refuses every write to the file.
    pub read_only: bool,
    /// The engine refuses files, other databases, extensions and setting changes (module doc).
    pub hardened: bool,
    /// Opens an encrypted file (`ATTACH … (ENCRYPTION_KEY …)`), always read-only (`open_root`).
    pub encryption_key: Option<String>,
    /// Lets DuckDB load and install the extensions a statement needs (from the network). Never
    /// for a hardened instance.
    pub autoload_extensions: bool,
    /// The profile's startup SQL (`ATTACH`, `SET threads = 4`, `USE other`…).
    pub startup_sql: Option<String>,
}

impl std::fmt::Debug for EngineConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineConfig")
            .field("location", &self.location)
            .field("read_only", &self.read_only)
            .field("hardened", &self.hardened)
            .field("encrypted", &self.encryption_key.is_some())
            .field("autoload_extensions", &self.autoload_extensions)
            .finish_non_exhaustive()
    }
}

impl EngineConfig {
    pub fn file(path: impl Into<PathBuf>) -> EngineConfig {
        EngineConfig {
            location: Location::File(path.into()),
            read_only: false,
            hardened: false,
            encryption_key: None,
            autoload_extensions: false,
            startup_sql: None,
        }
    }

    pub fn memory() -> EngineConfig {
        EngineConfig {
            location: Location::Memory,
            ..EngineConfig::file("")
        }
    }

    /// Read-only and hardened, as a read-only profile is opened.
    pub fn read_only(mut self) -> EngineConfig {
        self.read_only = true;
        self.hardened = true;
        self
    }

    pub fn hardened(mut self) -> EngineConfig {
        self.hardened = true;
        self
    }

    /// Everything but the spelling of the path (the registry key already matched the file).
    pub fn same_settings(&self, other: &EngineConfig) -> bool {
        self.read_only == other.read_only
            && self.hardened == other.hardened
            && self.encryption_key == other.encryption_key
            && self.autoload_extensions == other.autoload_extensions
            && self.startup_sql == other.startup_sql
    }
}

/// The settings that harden an instance after its startup SQL, in order; then
/// `LOCAL_FILES_OFF` when the instance may, and `LOCK` last.
pub const HARDENING: &[&str] = &[
    "SET autoinstall_known_extensions = false",
    "SET autoload_known_extensions = false",
    "SET enable_external_access = false",
];

/// Turns DuckDB's local file system off: SQL reaches no file, and DuckDB reads its databases
/// through the handles it opened before. It cannot come back (DuckDB refuses to enable a
/// disabled file system again). Only after external access is off: that setting needs the file
/// system to note the attached databases' paths.
pub const LOCAL_FILES_OFF: &str = "SET disabled_filesystems = 'LocalFileSystem'";

/// After it, no setting changes.
pub const LOCK: &str = "SET lock_configuration = true";

/// Whether the instance may turn its local file system off (before the hardening): no attached
/// database file takes writes (its WAL needs the file system), and the startup SQL set no temp
/// directory and allowed no folder or file of its own (the profile asked for that access).
/// Every function is named `system.main.<fn>`: DuckDB resolves an unqualified name in the
/// current database first, so a macro the file stores as `current_setting` or `len` would
/// answer in the builtin's place and keep the file system on.
pub const MAY_TURN_LOCAL_FILES_OFF: &str = "SELECT \
     system.main.current_setting('temp_directory') = '' \
     AND system.main.len(system.main.current_setting('allowed_directories')) = 0 \
     AND system.main.len(system.main.current_setting('allowed_paths')) = 0 \
     AND NOT EXISTS (SELECT 1 FROM system.main.duckdb_databases() \
     WHERE path IS NOT NULL AND NOT readonly)";

/// One open database: its settings and the connection the session connections are cloned from.
pub struct Instance {
    config: EngineConfig,
    /// The registry key (the canonical path of a file).
    key: Option<PathBuf>,
    /// The instance's own connection, the one the others are cloned from; None once dropped.
    root: Mutex<Option<Connection>>,
    /// What every new connection runs first (`USE …`, `SET schema …`).
    per_connection: Vec<String>,
    /// Hardened with the local file system off (`LOCAL_FILES_OFF`).
    local_files_off: bool,
    /// Its logger was switched, or a connection stopped answering (module doc: a switched
    /// logger): no connection runs or closes any more.
    broken: AtomicBool,
}

/// DuckDB's refusal of a `ROLLBACK` outside a transaction.
const NO_TRANSACTION: &str = "no transaction is active";

/// DuckDB's refusal of any statement but `ROLLBACK` in a failed transaction.
const ABORTED_TRANSACTION: &str = "Current transaction is aborted";

/// Where the instance's logger keeps its entries. Only `memory` is safe: file storage that
/// cannot be written throws from the destructors that log (module doc: a switched logger).
/// The builtin by its full name: a `current_setting` macro of the file would answer `memory`.
const LOGGER_STORAGE: &str = "SELECT system.main.current_setting('logging_storage')";

/// Why nothing more runs on an instance whose logger was switched (module doc).
pub const LOGGER_SWITCHED: &str = "This DuckDB database switched its logger away from memory \
     (a view or macro of the file calls enable_logging), and a log file DuckDB cannot write \
     would close the app. Its connections run nothing more and stay open until Tabler quits; \
     restart Tabler to connect again, and do not call that view or macro.";

/// What [`logger_state`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Logger {
    /// Entries stay in memory.
    Memory,
    /// Another storage, or the check itself failed (a failing flush fails it).
    Switched,
    /// The connection's transaction failed, so it runs nothing but `ROLLBACK`: ask again later.
    Unknown,
}

/// The logger's storage as `conn` sees it. A failing check is asked once more (a late
/// interrupt of a closing session fails one statement) before it counts as switched.
pub fn logger_state(conn: &Connection) -> Logger {
    let ask = || conn.query_row(LOGGER_STORAGE, [], |r| r.get::<_, String>(0));
    match ask().or_else(|_| ask()) {
        Ok(storage) if storage == "memory" => Logger::Memory,
        Ok(_) => Logger::Switched,
        Err(e) if e.to_string().contains(ABORTED_TRANSACTION) => Logger::Unknown,
        Err(_) => Logger::Switched,
    }
}

/// Whether `conn` can be closed: its logger keeps to memory, its transaction ends here, as a
/// statement, and then it still answers. Closing it with a transaction open would roll it back
/// inside DuckDB's destructor, where a logger's failing flush throws and aborts the process;
/// here that is an error.
fn answers(conn: &Connection) -> bool {
    if logger_state(conn) == Logger::Switched {
        return false;
    }
    let ended = match conn.execute_batch("ROLLBACK") {
        Ok(()) => true,
        Err(e) => e.to_string().contains(NO_TRANSACTION),
    };
    ended && logger_state(conn) == Logger::Memory
}

impl Drop for Instance {
    fn drop(&mut self) {
        let root = self
            .root
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(root) = root {
            self.close_or_leak(root);
        }
    }
}

impl std::fmt::Debug for Instance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Instance")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

static REGISTRY: LazyLock<Mutex<HashMap<PathBuf, Weak<Instance>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn registry() -> MutexGuard<'static, HashMap<PathBuf, Weak<Instance>>> {
    REGISTRY.lock().unwrap_or_else(|p| p.into_inner())
}

/// The files whose instance broke (module doc: a switched logger), for the rest of the process.
static BROKEN_FILES: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn broken_files() -> MutexGuard<'static, HashSet<PathBuf>> {
    BROKEN_FILES.lock().unwrap_or_else(|p| p.into_inner())
}

/// Refuses a file whose instance broke in this process: its connections are leaked and still
/// hold it until the app quits.
pub fn refuse_broken_file(path: &Path) -> Result<(), AppError> {
    if broken_files().contains(&file_key(path)) {
        return Err(AppError::InvalidInput(LOGGER_SWITCHED.into()));
    }
    Ok(())
}

/// The registry key of a file: the same for every spelling of its path.
pub fn file_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The open instance of a file, if any.
pub fn open_instance(path: &Path) -> Option<Arc<Instance>> {
    registry().get(&file_key(path)).and_then(Weak::upgrade)
}

impl Instance {
    /// The instance of `config`'s database: the open one when its settings are the same, a new
    /// one when the file is not open in this process. A file open with other settings is
    /// refused: one instance cannot be two configurations.
    pub fn open(config: EngineConfig) -> Result<Arc<Instance>, AppError> {
        let path = match &config.location {
            Location::Memory => return Instance::create(config, None).map(Arc::new),
            Location::File(p) => p.clone(),
        };
        let key = file_key(&path);
        // Held across the open, so two sessions of one file cannot both create an instance.
        let mut reg = registry();
        refuse_broken_file(&key)?;
        if let Some(live) = reg.get(&key).and_then(Weak::upgrade) {
            live.usable()?;
            if live.config.same_settings(&config) {
                return Ok(live);
            }
            return Err(AppError::InvalidInput(format!(
                "DuckDB opens a database file once per app, and \u{201c}{}\u{201d} is open in \
                 this Tabler with other settings ({}). Disconnect it first.",
                path.display(),
                live.describe()
            )));
        }
        let instance = Arc::new(Instance::create(config, Some(key.clone()))?);
        reg.retain(|_, w| w.strong_count() > 0);
        reg.insert(key, Arc::downgrade(&instance));
        Ok(instance)
    }

    fn create(config: EngineConfig, key: Option<PathBuf>) -> Result<Instance, AppError> {
        let shown = match &config.location {
            Location::File(p) => p.display().to_string(),
            Location::Memory => ":memory:".into(),
        };
        if config.read_only && config.location == Location::Memory {
            return Err(AppError::InvalidInput(
                "An in-memory DuckDB database cannot be opened read-only.".into(),
            ));
        }
        let root = open_root(&config).map_err(|e| errors::open_error(&shown, e))?;
        // Built first, so a failed startup closes the root through `Drop` (`close_or_leak`).
        let mut instance = Instance {
            config,
            key,
            root: Mutex::new(None),
            per_connection: Vec::new(),
            local_files_off: false,
            broken: AtomicBool::new(false),
        };
        let root = &*instance
            .root
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .insert(root);
        let config = &instance.config;
        let per_connection = &mut instance.per_connection;
        // The clones of the root start in its default database: an encrypted file's
        // connections switch to it first.
        if let (Location::File(path), Some(_)) = (&config.location, &config.encryption_key) {
            per_connection.push(use_database(&encrypted_name(path)));
        }
        if let Some(sql) = config.startup_sql.as_deref() {
            for stmt in crate::sqlgen::portable::split::split(&LEX, sql) {
                let text = stmt.sql.trim();
                if text.is_empty() {
                    continue;
                }
                let ran = root.execute_batch(text);
                // A view or macro the startup SQL calls can switch the logger as well.
                if logger_state(root) == Logger::Switched {
                    instance.mark_broken();
                    return Err(AppError::InvalidInput(LOGGER_SWITCHED.into()));
                }
                ran.map_err(|e| startup_error(text, e))?;
                if per_connection_statement(text) {
                    per_connection.push(text.to_string());
                }
            }
        }
        if config.hardened {
            let local_files_off = may_turn_local_files_off(root)?;
            for stmt in HARDENING {
                root.execute_batch(stmt).map_err(errors::map)?;
            }
            if local_files_off {
                root.execute_batch(LOCAL_FILES_OFF).map_err(errors::map)?;
            }
            root.execute_batch(LOCK).map_err(errors::map)?;
            instance.local_files_off = local_files_off;
        }
        Ok(instance)
    }

    /// A new connection to the database, with the per-connection startup statements run.
    pub fn connect(&self) -> Result<Connection, AppError> {
        self.usable()?;
        let conn = self
            .root
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .ok_or(AppError::NotConnected)?
            .try_clone()
            .map_err(errors::map)?;
        for stmt in &self.per_connection {
            if let Err(e) = conn.execute_batch(stmt) {
                self.release(conn);
                return Err(startup_error(stmt, e));
            }
        }
        Ok(conn)
    }

    /// Closes a connection of this instance, or leaks it and marks the instance broken when it
    /// no longer answers (module doc: a broken logger).
    pub fn release(&self, conn: Connection) {
        self.close_or_leak(conn);
    }

    /// Closes `conn` unless it or the instance is broken (then it is leaked: closing it would
    /// throw from DuckDB's destructor and abort the process). Marks the instance broken when
    /// `conn` no longer answers.
    fn close_or_leak(&self, conn: Connection) {
        if self.broken.load(Ordering::SeqCst) || !answers(&conn) {
            self.mark_broken();
            log::warn!(
                "duckdb: a switched logger or a connection that no longer answers: left open"
            );
            std::mem::forget(conn);
        } else {
            drop(conn);
        }
    }

    /// Stops the instance, and refuses its file for the rest of the process at once: the
    /// leaked connections hold it, and a second instance beside them (POSIX file locks are the
    /// process's, so DuckDB's would not stop it) would be two instances of one file. Not left
    /// to `Drop`, where a dead registry entry and no refusal yet would let one open.
    fn mark_broken(&self) {
        self.broken.store(true, Ordering::SeqCst);
        if let Some(key) = &self.key {
            broken_files().insert(key.clone());
        }
    }

    /// Runs `f` on `conn` unless the instance is broken, looking at the logger before and
    /// after: switched, it marks the instance broken, so nothing more runs on any of its
    /// connections and none closes (module doc: a switched logger). Before, too, since a job of
    /// another connection may have switched it while its own check could not tell (a failed
    /// transaction). `f`'s own result stands.
    pub fn guarded<R>(
        &self,
        conn: &mut Connection,
        f: impl FnOnce(&mut Connection) -> Result<R, AppError>,
    ) -> Result<R, AppError> {
        self.usable()?;
        if self.logger_switched(conn) {
            self.mark_broken();
            log::warn!("duckdb: the logger was switched away from memory: the instance stops");
            return Err(AppError::InvalidInput(LOGGER_SWITCHED.into()));
        }
        let result = f(conn);
        if self.logger_switched(conn) {
            self.mark_broken();
            log::warn!("duckdb: the logger was switched away from memory: the instance stops");
        }
        result
    }

    /// Whether the instance's logger left memory. The storage is the instance's, so when
    /// `conn` cannot answer (its transaction failed, perhaps in the very statement that
    /// switched it) the root connection answers for it, which never holds a user's
    /// transaction; when neither can, it counts as switched.
    fn logger_switched(&self, conn: &Connection) -> bool {
        match logger_state(conn) {
            Logger::Memory => false,
            Logger::Switched => true,
            Logger::Unknown => {
                let root = self.root.lock().unwrap_or_else(|p| p.into_inner());
                root.as_ref()
                    .is_none_or(|root| logger_state(root) != Logger::Memory)
            }
        }
    }

    /// Refuses everything once the instance is broken.
    fn usable(&self) -> Result<(), AppError> {
        if self.broken.load(Ordering::SeqCst) {
            return Err(AppError::InvalidInput(LOGGER_SWITCHED.into()));
        }
        Ok(())
    }

    /// Its logger was switched or a connection stopped answering, so none of them runs or
    /// closes any more.
    #[cfg(test)]
    pub fn is_broken(&self) -> bool {
        self.broken.load(Ordering::SeqCst)
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    pub fn is_hardened(&self) -> bool {
        self.config.hardened
    }

    /// The engine refuses every write that would outlive the instance (what
    /// `ReadOnlyMode::Server` may claim): a read-only profile, opened with
    /// `access_mode=READ_ONLY` (an encrypted file attached `READ_ONLY`), hardened and with the
    /// local file system off, so no file is read but through the databases' handles, and none
    /// is written, attached or exported. Not when the startup SQL allowed folders, files or a
    /// temp directory (the file system stays on for them, and COPY could write there and beside
    /// the database). In-memory scratch still takes writes: TEMP tables, and for an encrypted
    /// file the in-memory database it is attached into (`memory`, `ATTACH ':memory:'`); both go
    /// with the instance.
    pub fn is_read_only(&self) -> bool {
        self.config.read_only && self.config.hardened && self.local_files_off
    }

    /// Hardened with the local file system off: SQL reaches no file (module doc).
    pub fn local_files_off(&self) -> bool {
        self.local_files_off
    }

    /// The database file takes no writes: a read-only profile, or an encrypted file (attached
    /// read-only here). Unlike `is_read_only`, other databases may still be written: an
    /// encrypted file of a read-write profile ATTACHes and writes other files and `memory`.
    pub fn is_file_read_only(&self) -> bool {
        self.config.read_only || self.config.encryption_key.is_some()
    }

    /// The canonical path of the file, None in memory.
    pub fn file(&self) -> Option<&Path> {
        self.key.as_deref()
    }

    fn describe(&self) -> String {
        let mut parts = vec![if self.is_file_read_only() {
            "read-only"
        } else {
            "read-write"
        }];
        if self.config.hardened {
            parts.push("without file access");
        }
        if self.config.encryption_key.is_some() {
            parts.push("encrypted");
        }
        parts.join(", ")
    }
}

/// The instance's first connection: the file opened with the configuration, or for an
/// encrypted file an in-memory instance that ATTACHes it with its key and makes it current.
fn open_root(config: &EngineConfig) -> Result<Connection, ::duckdb::Error> {
    let autoload = config.autoload_extensions && !config.hardened;
    let mut flags = Config::default()
        .enable_autoload_extension(autoload)?
        .with("autoinstall_known_extensions", bool_text(autoload))?;
    if config.hardened {
        // No temp directory: DuckDB would let SQL read and write it once external access is
        // off (module doc).
        flags = flags
            .with("allow_community_extensions", "false")?
            .with("allow_persistent_secrets", "false")?
            .with("temp_directory", "")?;
    }
    match (&config.location, &config.encryption_key) {
        (Location::Memory, _) => Connection::open_in_memory_with_flags(flags),
        (Location::File(path), None) => {
            let mode = if config.read_only {
                AccessMode::ReadOnly
            } else {
                AccessMode::ReadWrite
            };
            Connection::open_with_flags(path, flags.access_mode(mode)?)
        }
        (Location::File(path), Some(key)) => {
            let conn = Connection::open_in_memory_with_flags(flags)?;
            // Always read-only: DuckDB 1.5's built-in crypto (mbedtls) only decrypts; writing
            // an encrypted file needs the httpfs extension's OpenSSL, which this build lacks.
            let name = encrypted_name(path);
            conn.execute_batch(&attach_encrypted(&path.to_string_lossy(), &name, key))?;
            conn.execute_batch(&use_database(&name))?;
            Ok(conn)
        }
    }
}

/// The catalog name an encrypted file is attached as: its file name without the extension,
/// as DuckDB names a file it opens, unless that is a name DuckDB keeps for itself.
pub fn encrypted_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| {
            !s.is_empty() && !["memory", "system", "temp"].contains(&s.to_lowercase().as_str())
        })
        .unwrap_or_else(|| "encrypted".into())
}

fn bool_text(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// `MAY_TURN_LOCAL_FILES_OFF` on the instance's first connection.
fn may_turn_local_files_off(root: &Connection) -> Result<bool, AppError> {
    let (_, rows) = super::conn::query_all(root, MAY_TURN_LOCAL_FILES_OFF, &[])?;
    Ok(matches!(
        rows.first().and_then(|r| r.first()),
        Some(Value::Bool(true))
    ))
}

/// Whether a startup statement sets something of the connection rather than the instance:
/// `USE …`, `SET [SESSION | LOCAL] schema | search_path …`.
fn per_connection_statement(sql: &str) -> bool {
    let words = crate::sqlgen::split::leading_keywords(sql, 3);
    let word = |i: usize| words.get(i).map(String::as_str);
    match word(0) {
        Some("USE") => true,
        Some("SET") => {
            let name = match word(1) {
                Some("SESSION" | "LOCAL") => word(2),
                other => other,
            };
            matches!(name, Some("SCHEMA" | "SEARCH_PATH"))
        }
        _ => false,
    }
}

fn startup_error(sql: &str, e: ::duckdb::Error) -> AppError {
    let short: String = sql.chars().take(80).collect();
    AppError::Connect(format!("startup SQL failed ({short}): {}", errors::map(e)))
}

/// What a DuckDB database file starts with: an 8-byte checksum, then `DUCK`.
pub const MAGIC: &[u8; 4] = b"DUCK";

/// Refuses a path that is no DuckDB database: missing, a directory, or a file without DuckDB's
/// magic bytes. An empty file is taken (DuckDB writes a new database into it).
pub fn check_database_file(path: &str) -> Result<(), AppError> {
    // Not readable beside the leaked instance on Windows: say why instead.
    refuse_broken_file(Path::new(path))?;
    let meta = std::fs::metadata(path).map_err(|e| {
        AppError::InvalidInput(format!(
            "The database file \u{201c}{path}\u{201d} cannot be opened: {e}"
        ))
    })?;
    if meta.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "\u{201c}{path}\u{201d} is a folder, not a DuckDB database file."
        )));
    }
    if meta.len() == 0 {
        return Ok(());
    }
    let mut head = [0u8; 12];
    let read = {
        use std::io::Read as _;
        let mut f = std::fs::File::open(path)?;
        let mut n = 0;
        while n < head.len() {
            match f.read(&mut head[n..])? {
                0 => break,
                k => n += k,
            }
        }
        n
    };
    if read < 12 || &head[8..12] != MAGIC {
        return Err(AppError::InvalidInput(format!(
            "\u{201c}{path}\u{201d} is not a DuckDB database file."
        )));
    }
    Ok(())
}

/// Creates a new, empty database file ("New…" of the connection form): refuses an existing
/// path, and closes the file again before it returns.
pub fn create_database_file(path: &str) -> Result<(), AppError> {
    let p = Path::new(path);
    if p.exists() {
        return Err(AppError::InvalidInput(format!(
            "\u{201c}{path}\u{201d} already exists."
        )));
    }
    let conn = Connection::open(p).map_err(|e| errors::open_error(path, e))?;
    // A checkpoint writes the header, so the file is a database before anything is stored.
    conn.execute_batch("CHECKPOINT").map_err(errors::map)?;
    conn.close().map_err(|(_, e)| errors::map(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations() {
        assert_eq!(Location::parse(" :memory: "), Location::Memory);
        assert_eq!(Location::parse(""), Location::Memory);
        assert_eq!(
            Location::parse("C:/x.duckdb"),
            Location::File(PathBuf::from("C:/x.duckdb"))
        );
    }

    #[test]
    fn startup_statements_of_a_connection() {
        for sql in [
            "USE other",
            "use other.main",
            "SET schema = 'x'",
            "SET search_path = 'a,b'",
            "SET SESSION search_path = 'a'",
        ] {
            assert!(per_connection_statement(sql), "{sql}");
        }
        for sql in [
            "ATTACH 'x.duckdb' AS x",
            "SET threads = 4",
            "SET GLOBAL memory_limit = '1GB'",
            "INSTALL icu",
        ] {
            assert!(!per_connection_statement(sql), "{sql}");
        }
    }

    #[test]
    fn the_key_stays_out_of_debug_output() {
        let mut c = EngineConfig::file("x.duckdb");
        c.encryption_key = Some("s3cret".into());
        assert!(!format!("{c:?}").contains("s3cret"));
        assert_eq!(encrypted_name(Path::new("C:/d/sales.duckdb")), "sales");
        assert_eq!(encrypted_name(Path::new("Memory.duckdb")), "encrypted");
        assert_eq!(encrypted_name(Path::new("a\"b.db")), "a\"b");
    }

    /// A connection with an open transaction, released while a logger buffers until its buffer
    /// is full and cannot flush (module doc): the rollback runs as a statement, not in DuckDB's
    /// destructor, so the process lives on whichever entry fills the buffer.
    #[test]
    fn releasing_an_open_transaction_under_a_buffered_logger_does_not_abort() {
        for size in 1..=6 {
            let dir = tempfile::tempdir().unwrap();
            let blocker = dir.path().join("blocker");
            std::fs::write(&blocker, "x").unwrap();
            let logs = super::super::testing::sql_path(&blocker.join("logs"));
            let inst = Instance::open(EngineConfig::file(dir.path().join("x.duckdb"))).unwrap();
            let a = inst.connect().unwrap();
            let b = inst.connect().unwrap();
            // A table read, so the transaction reaches the file (`SELECT 1` alone does not).
            a.execute_batch("CREATE TABLE rt AS SELECT 1 AS x").unwrap();
            b.execute_batch("BEGIN; SELECT * FROM rt").unwrap();
            let _ = a.execute_batch(&format!(
                "CALL enable_logging(level := 'debug', storage := 'file', \
                 storage_path := {logs}, storage_buffer_size := {size}, \
                 storage_config := {{'only_flush_on_full_buffer': true}})"
            ));
            inst.release(b);
            inst.release(a);
            drop(inst);
            drop(dir);
        }
    }
}
