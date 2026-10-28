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
//! `FROM data.csv`), so read-only, MCP and plugin sessions are opened with the engine refusing
//! them. The open configuration turns extension autoload and autoinstall, community extensions
//! and persistent secrets off, and sets no temp directory (`temp_directory = ''`: no spilling to
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
//! `enable_logging` with file storage is refused where the local file system is off; where it
//! is on, a path outside them leaves a logger that fails every later statement of the instance,
//! and closing a connection of it then aborts the process (DuckDB throws from a destructor).
//! `enable_profiling` with a save location fails the connection's statements until
//! `disable_profiling`. The classifier calls them writes; a session that lets writes through
//! has to refuse them itself (see the brief's Limits).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
pub const MAY_TURN_LOCAL_FILES_OFF: &str = "SELECT current_setting('temp_directory') = '' \
     AND len(current_setting('allowed_directories')) = 0 \
     AND len(current_setting('allowed_paths')) = 0 \
     AND NOT EXISTS (SELECT 1 FROM duckdb_databases() WHERE path IS NOT NULL AND NOT readonly)";

/// One open database: its settings and the connection the session connections are cloned from.
pub struct Instance {
    config: EngineConfig,
    /// The registry key (the canonical path of a file).
    key: Option<PathBuf>,
    root: Mutex<Connection>,
    /// What every new connection runs first (`USE …`, `SET schema …`).
    per_connection: Vec<String>,
    /// Hardened with the local file system off (`LOCAL_FILES_OFF`).
    local_files_off: bool,
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
        if let Some(live) = reg.get(&key).and_then(Weak::upgrade) {
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
        let mut per_connection = Vec::new();
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
                root.execute_batch(text)
                    .map_err(|e| startup_error(text, e))?;
                if per_connection_statement(text) {
                    per_connection.push(text.to_string());
                }
            }
        }
        let mut local_files_off = false;
        if config.hardened {
            local_files_off = may_turn_local_files_off(&root)?;
            for stmt in HARDENING {
                root.execute_batch(stmt).map_err(errors::map)?;
            }
            if local_files_off {
                root.execute_batch(LOCAL_FILES_OFF).map_err(errors::map)?;
            }
            root.execute_batch(LOCK).map_err(errors::map)?;
        }
        Ok(Instance {
            config,
            key,
            root: Mutex::new(root),
            per_connection,
            local_files_off,
        })
    }

    /// A new connection to the database, with the per-connection startup statements run.
    pub fn connect(&self) -> Result<Connection, AppError> {
        let conn = self
            .root
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .try_clone()
            .map_err(errors::map)?;
        for stmt in &self.per_connection {
            conn.execute_batch(stmt)
                .map_err(|e| startup_error(stmt, e))?;
        }
        Ok(conn)
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
}
