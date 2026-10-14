//! Saved queries as `.sql` files (TablePlus stores its Queries / Favorites on disk, in the folder
//! chosen in Preferences ▸ Locations). One file per query, named after it; folders are
//! sub-directories. A short front-matter comment at the top of a file keeps what a file name
//! cannot hold:
//!
//! ```sql
//! -- @id: 6f0c…            (stable identity, survives renames and moves)
//! -- @keyword: recent      (the binding keyword, optional)
//! -- @connection: 1b2c…    (the connection profile; absent = every connection)
//! select …
//! ```
//!
//! Files without it (added in Finder) list fine: their id comes from their path. Folder ids come
//! from their path too, so they change when a folder is renamed or moved. Hidden entries (names
//! starting with `.`) and symlinked folders are skipped. Deleting a folder removes only the `.sql`
//! files inside it (and the folders left empty), never other files.
//!
//! The library only lists `.sql` files up to [`QUERY_FILE_MAX_BYTES`] (a dump in the folder is
//! not a query and would cost its size in memory on every listing) and at most
//! [`MAX_LISTED_QUERIES`] of them.
//!
//! Also here: reading / writing the SQL editor's files, gzip-aware (`.sql.gz`).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::error::AppError;
use crate::import::text::{Utf8Reader, detect_encoding, encoding_label};
use crate::types::import::TextEncoding;
use crate::types::saved_query::{
    QueryFolder, QueryFolderInput, SavedQuery, SavedQueryInput, SavedQueryLibrary,
};
use crate::types::sql_file::{SqlFileHead, SqlFileInfo};

/// Longest query / folder name accepted (as in the app DB).
const NAME_MAX: usize = 200;
/// Longest binding keyword accepted.
const KEYWORD_MAX: usize = 64;
/// Folders deeper than this are not listed.
const MAX_DEPTH: usize = 24;
/// Largest uncompressed text the editor opens from a `.gz` file.
pub const GZ_TEXT_MAX: u64 = 512 * 1024 * 1024;
/// Largest plain file the editor reads whole (one string over IPC, one CodeMirror document). A
/// bigger one opens as its head (`read_sql_head`) or goes through File ▸ Import ▸ SQL dump.
pub const EDITOR_FILE_MAX_BYTES: u64 = 512 * 1024 * 1024;
/// Largest `.sql` file the library lists, imports or copies: a saved query is a few KB, and every
/// listing reads every file.
pub const QUERY_FILE_MAX_BYTES: u64 = 5 * 1024 * 1024;
/// A listing stops after this many queries (a folder of thousands of generated files).
const MAX_LISTED_QUERIES: usize = 20_000;
/// A listing looks at no more directory entries than this, files that are not queries included.
const MAX_SCANNED_ENTRIES: usize = 200_000;

fn io_at(path: &Path) -> impl Fn(std::io::Error) -> AppError + '_ {
    move |e| AppError::Io(format!("{}: {e}", path.display()))
}

/// A trimmed, non-empty name of at most `NAME_MAX` characters.
pub fn valid_name(name: &str, what: &str) -> Result<String, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput(format!("{what} name is required")));
    }
    if name.chars().count() > NAME_MAX {
        return Err(AppError::InvalidInput(format!(
            "{what} name is longer than {NAME_MAX} characters"
        )));
    }
    Ok(name.to_string())
}

/// The binding keyword: trimmed, `None` when blank, one word (no whitespace).
pub fn valid_keyword(keyword: Option<&str>) -> Result<Option<String>, AppError> {
    let Some(k) = keyword.map(str::trim).filter(|k| !k.is_empty()) else {
        return Ok(None);
    };
    if k.chars().any(char::is_whitespace) {
        return Err(AppError::InvalidInput(
            "the keyword must be a single word".into(),
        ));
    }
    if k.chars().count() > KEYWORD_MAX {
        return Err(AppError::InvalidInput(format!(
            "the keyword is longer than {KEYWORD_MAX} characters"
        )));
    }
    Ok(Some(k.to_string()))
}

/// The file / folder name for a query or folder name: path separators, `:` and control
/// characters become `-`, leading dots and spaces go (a hidden file would not be listed).
pub fn file_stem(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches(['.', ' ']).trim_end();
    if trimmed.is_empty() {
        "Untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// A stable id for a path inside the queries folder (`kind` keeps folder and file ids apart).
pub fn path_id(kind: &str, rel: &str) -> Uuid {
    let input = format!("tabler-queries\0{kind}\0{rel}");
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, input.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.as_ref()[..16]);
    // RFC 9562 version 8 (custom), variant 10.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn is_sql_file_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("sql"))
}

fn stem_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[..i].to_string(),
        _ => name.to_string(),
    }
}

fn rfc3339(t: Option<std::time::SystemTime>) -> String {
    let t = t.unwrap_or(std::time::UNIX_EPOCH);
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn created_and_modified(meta: &std::fs::Metadata) -> (String, String) {
    let modified = meta.modified().ok();
    let created = meta.created().ok().or(modified);
    (rfc3339(created), rfc3339(modified))
}

// --- front matter --------------------------------------------------------------------------

/// What the front-matter comment of a query file holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Header {
    pub id: Option<Uuid>,
    pub keyword: Option<String>,
    pub connection: Option<Uuid>,
}

/// The front matter at the start of `bytes` (a leading byte order mark is skipped) and the offset
/// where the SQL starts. Only the leading `-- @id:`, `-- @keyword:` and `-- @connection:` lines
/// count; the first other line starts the SQL. Works on bytes so that a file that is not UTF-8
/// keeps its SQL exactly (`save_query` with `keep_sql`).
fn parse_header(bytes: &[u8]) -> (Header, usize) {
    let mut header = Header::default();
    let mut offset = if bytes.starts_with(BOM) { BOM.len() } else { 0 };
    while offset < bytes.len() {
        let rest = &bytes[offset..];
        let end = rest
            .iter()
            .position(|&b| b == b'\n')
            .map_or(rest.len(), |i| i + 1);
        let line = String::from_utf8_lossy(&rest[..end]);
        let line = line.trim_end_matches(['\n', '\r']);
        let Some((key, value)) = line
            .strip_prefix("-- @")
            .and_then(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim(), v.trim()))
        else {
            break;
        };
        match key {
            "id" => header.id = Uuid::parse_str(value).ok(),
            "keyword" => header.keyword = Some(value.to_string()).filter(|v| !v.is_empty()),
            "connection" => header.connection = Uuid::parse_str(value).ok(),
            _ => break,
        }
        offset += end;
    }
    (header, offset)
}

/// UTF-8 byte order mark.
const BOM: &[u8] = b"\xef\xbb\xbf";

/// Splits a query file into its front matter and its SQL. Only the leading `-- @id:`,
/// `-- @keyword:` and `-- @connection:` lines count; the first other line starts the SQL.
pub fn parse_query_file(text: &str) -> (Header, &str) {
    let (header, offset) = parse_header(text.as_bytes());
    (header, &text[offset..])
}

/// The file text of a query: its front matter, then the SQL as it is.
pub fn render_query_file(
    id: Uuid,
    keyword: Option<&str>,
    connection: Option<Uuid>,
    sql: &str,
) -> String {
    let mut out = format!("-- @id: {id}\n");
    if let Some(k) = keyword {
        out.push_str(&format!("-- @keyword: {k}\n"));
    }
    if let Some(c) = connection {
        out.push_str(&format!("-- @connection: {c}\n"));
    }
    out.push_str(sql);
    out
}

// --- scanning ------------------------------------------------------------------------------

/// Everything in a queries folder, with the paths behind the ids.
#[derive(Debug, Default)]
pub struct Scan {
    pub folders: Vec<(QueryFolder, PathBuf)>,
    pub queries: Vec<(SavedQuery, PathBuf)>,
    /// The walk stopped at a limit ([`MAX_LISTED_QUERIES`], [`MAX_SCANNED_ENTRIES`]).
    pub truncated: bool,
}

impl Scan {
    pub fn folder_path(&self, id: Uuid) -> Option<&PathBuf> {
        self.folders
            .iter()
            .find(|(f, _)| f.id == id)
            .map(|(_, p)| p)
    }

    pub fn query(&self, id: Uuid) -> Option<&(SavedQuery, PathBuf)> {
        self.queries.iter().find(|(q, _)| q.id == id)
    }

    pub fn into_library(self) -> SavedQueryLibrary {
        let key = |name: &str, created: &str| (name.to_lowercase(), created.to_string());
        let mut folders: Vec<QueryFolder> = self.folders.into_iter().map(|(f, _)| f).collect();
        folders.sort_by_key(|f| key(&f.name, &f.created_at));
        let mut queries: Vec<SavedQuery> = self.queries.into_iter().map(|(q, _)| q).collect();
        queries.sort_by_key(|q| key(&q.name, &q.created_at));
        SavedQueryLibrary { folders, queries }
    }
}

fn read_text_lossy(path: &Path) -> Result<String, AppError> {
    let bytes = std::fs::read(path).map_err(io_at(path))?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

fn read_query(
    root: &Path,
    path: &Path,
    folder: Option<Uuid>,
    seen: &mut HashSet<Uuid>,
) -> Result<SavedQuery, AppError> {
    let text = read_text_lossy(path)?;
    let (header, sql) = parse_query_file(&text);
    let meta = std::fs::metadata(path).map_err(io_at(path))?;
    let (created_at, updated_at) = created_and_modified(&meta);
    let name = path
        .file_name()
        .map(|n| stem_of(&n.to_string_lossy()))
        .unwrap_or_default();
    // A copy of a file (made in Finder) carries the same id: the second one gets its path's.
    let id = header
        .id
        .filter(|id| !seen.contains(id))
        .unwrap_or_else(|| path_id("query", &rel_path(root, path)));
    seen.insert(id);
    Ok(SavedQuery {
        id,
        name,
        keyword: header.keyword,
        sql: sql.to_string(),
        folder_id: folder,
        profile_id: header.connection,
        created_at,
        updated_at,
    })
}

fn sorted_entries(dir: &Path) -> Result<Vec<std::fs::DirEntry>, AppError> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(io_at(dir))?
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|e| e.file_name());
    Ok(entries)
}

/// How much of a folder a listing reads.
#[derive(Clone, Copy)]
struct Limits {
    queries: usize,
    entries: usize,
}

const LIMITS: Limits = Limits {
    queries: MAX_LISTED_QUERIES,
    entries: MAX_SCANNED_ENTRIES,
};

/// A listing in progress.
struct Walker<'a> {
    root: &'a Path,
    scan: Scan,
    seen: HashSet<Uuid>,
    limits: Limits,
    /// Directory entries still allowed to be looked at.
    budget: usize,
}

impl Walker<'_> {
    fn walk(&mut self, dir: &Path, parent: Option<Uuid>, depth: usize) -> Result<(), AppError> {
        let root = self.root;
        for entry in sorted_entries(dir)? {
            if self.scan.truncated {
                return Ok(());
            }
            if self.budget == 0 {
                log::warn!(
                    "queries: {} holds more than {} entries: the rest is not listed",
                    root.display(),
                    self.limits.entries
                );
                self.scan.truncated = true;
                return Ok(());
            }
            self.budget -= 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth >= MAX_DEPTH {
                    continue;
                }
                let id = path_id("folder", &rel_path(root, &path));
                let created_at = entry
                    .metadata()
                    .map(|m| created_and_modified(&m).0)
                    .unwrap_or_else(|_| rfc3339(None));
                self.scan.folders.push((
                    QueryFolder {
                        id,
                        name: name.clone(),
                        parent_id: parent,
                        created_at,
                    },
                    path.clone(),
                ));
                self.walk(&path, Some(id), depth + 1)?;
            } else if is_sql_file_name(&name) && path.is_file() {
                if self.scan.queries.len() >= self.limits.queries {
                    log::warn!(
                        "queries: {} holds more than {} queries: the rest is not listed",
                        root.display(),
                        self.limits.queries
                    );
                    self.scan.truncated = true;
                    return Ok(());
                }
                // A dump is not a query: its size is checked before a byte of it is read.
                match std::fs::metadata(&path) {
                    Ok(meta) if meta.len() > QUERY_FILE_MAX_BYTES => {
                        log::debug!(
                            "queries: {} is {} bytes (over {QUERY_FILE_MAX_BYTES}): not listed",
                            path.display(),
                            meta.len()
                        );
                        continue;
                    }
                    Ok(_) => {}
                    Err(_) => continue,
                }
                // An unreadable file is skipped rather than failing the whole listing.
                if let Ok(q) = read_query(root, &path, parent, &mut self.seen) {
                    self.scan.queries.push((q, path));
                }
            }
        }
        Ok(())
    }
}

/// Everything in `root` (an absent folder holds nothing yet).
pub fn scan(root: &Path) -> Result<Scan, AppError> {
    scan_limited(root, LIMITS)
}

fn scan_limited(root: &Path, limits: Limits) -> Result<Scan, AppError> {
    let mut walker = Walker {
        root,
        scan: Scan::default(),
        seen: HashSet::new(),
        limits,
        budget: limits.entries,
    };
    if root.is_dir() {
        walker.walk(root, None, 0)?;
    }
    Ok(walker.scan)
}

pub fn list(root: &Path) -> Result<SavedQueryLibrary, AppError> {
    Ok(scan(root)?.into_library())
}

// --- writing -------------------------------------------------------------------------------

/// Lowercased names in `dir` (APFS and HFS+ compare names case-insensitively), but `except`.
fn taken_names(dir: &Path, except: Option<&Path>) -> HashSet<String> {
    let except = except
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_lowercase());
    std::fs::read_dir(dir)
        .map(|it| {
            it.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().to_lowercase())
                .filter(|n| Some(n) != except.as_ref())
                .collect()
        })
        .unwrap_or_default()
}

/// `dir/<stem><ext>`, or `dir/<stem> 2<ext>`… when the name is taken. Returns the path and the
/// stem used.
fn unique_path(dir: &Path, stem: &str, ext: &str, except: Option<&Path>) -> (PathBuf, String) {
    let taken = taken_names(dir, except);
    let mut candidate = stem.to_string();
    for n in 2.. {
        if !taken.contains(&format!("{candidate}{ext}").to_lowercase()) {
            break;
        }
        candidate = format!("{stem} {n}");
    }
    (dir.join(format!("{candidate}{ext}")), candidate)
}

/// Serialises the atomic writes of this process: every write to one file uses the same temporary
/// file name.
static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Writes `bytes` to `path` through a temporary file next to it (`.<name>.tabler-tmp`) that is
/// renamed over `path` once it is complete and on disk: a crash, a full disk or a sync client
/// never sees a truncated or half-written file, only the old content or the new one. Writing
/// through a symlink replaces the file it points to (the link stays). The file keeps the
/// permissions of the one it replaces, a read-only file is refused as a plain write would
/// refuse it, and the temporary file is removed when anything fails.
pub fn write_atomic_bytes(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let existing = std::fs::metadata(&target).ok();
    if let Some(meta) = &existing {
        if meta.is_dir() {
            return Err(AppError::Io(format!("{}: is a directory", path.display())));
        }
        if meta.permissions().readonly() {
            return Err(io_at(path)(std::io::ErrorKind::PermissionDenied.into()));
        }
    }
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // The temporary name adds 12 bytes to the name: a file name near the 255 byte limit of the
    // file system (it takes that, `.sql` included) must still be writable.
    let mut keep = name.len().min(200);
    while !name.is_char_boundary(keep) {
        keep -= 1;
    }
    name.truncate(keep);
    let tmp = dir.join(format!(".{name}.tabler-tmp"));
    let write_tmp = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        if let Some(meta) = &existing {
            file.set_permissions(meta.permissions())?;
        }
        file.sync_all()
    };
    if let Err(e) = write_tmp() {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_at(path)(e));
    }
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io_at(path)(e)
    })
}

/// [`write_atomic_bytes`] for text.
fn write_atomic(path: &Path, text: &str) -> Result<(), AppError> {
    write_atomic_bytes(path, text.as_bytes())
}

fn same_name(a: &Path, b: &Path) -> bool {
    a.file_name() == b.file_name()
}

/// Creates (`input.id` = `None`, or an id not found) or replaces a saved query: renames / moves
/// its file when the name or the folder changed. A name taken in the folder gets ` 2`, ` 3`….
///
/// With `keep_sql` the SQL of the file is never rewritten from `input.sql` (the renderer's copy
/// may be stale, or lossy for a file that is not UTF-8): the file is only renamed / moved, and
/// when its front matter has to change (keyword, connection, or an id it did not carry yet) the
/// header is rewritten around the body as the file holds it right now.
pub fn save_query(root: &Path, input: &SavedQueryInput) -> Result<SavedQuery, AppError> {
    let name = valid_name(&input.name, "Query")?;
    let keyword = valid_keyword(input.keyword.as_deref())?;
    let scanned = scan(root)?;
    let dir = match input.folder_id {
        Some(f) => scanned
            .folder_path(f)
            .cloned()
            .ok_or_else(|| AppError::NotFound("the folder no longer exists".into()))?,
        None => root.to_path_buf(),
    };
    std::fs::create_dir_all(&dir).map_err(io_at(&dir))?;
    let existing = input
        .id
        .and_then(|id| scanned.query(id))
        .map(|(_, p)| p.clone());
    let id = input.id.unwrap_or_else(Uuid::new_v4);
    let stem = file_stem(&name);
    let target = match &existing {
        Some(p)
            if p.parent() == Some(dir.as_path())
                && same_name(p, &dir.join(format!("{stem}.sql"))) =>
        {
            p.clone()
        }
        _ => {
            unique_path(
                &dir,
                &stem,
                ".sql",
                existing
                    .as_deref()
                    .filter(|p| p.parent() == Some(dir.as_path())),
            )
            .0
        }
    };
    // The file as it is on disk now, for a save that keeps its SQL.
    let kept = match (&existing, input.keep_sql) {
        (Some(old), Some(true)) => Some(std::fs::read(old).map_err(io_at(old))?),
        _ => None,
    };
    if let Some(old) = &existing
        && old != &target
    {
        std::fs::rename(old, &target).map_err(io_at(old))?;
    }
    match kept {
        Some(bytes) => {
            let (header, body_at) = parse_header(&bytes);
            let unchanged = header.id == Some(id)
                && header.keyword == keyword
                && header.connection == input.profile_id;
            if !unchanged {
                let bom = if bytes.starts_with(BOM) { BOM } else { b"" };
                let mut rewritten = bom.to_vec();
                rewritten.extend_from_slice(
                    render_query_file(id, keyword.as_deref(), input.profile_id, "").as_bytes(),
                );
                rewritten.extend_from_slice(&bytes[body_at..]);
                write_atomic_bytes(&target, &rewritten)?;
            }
        }
        None => {
            let text = render_query_file(id, keyword.as_deref(), input.profile_id, &input.sql);
            write_atomic(&target, &text)?;
        }
    }
    let mut seen = HashSet::new();
    let folder_id = input.folder_id.filter(|_| dir != root);
    let mut saved = read_query(root, &target, folder_id, &mut seen)?;
    saved.id = id;
    Ok(saved)
}

/// Deletes a saved query's file; false when there was none.
pub fn delete_query(root: &Path, id: Uuid) -> Result<bool, AppError> {
    let scanned = scan(root)?;
    let Some((_, path)) = scanned.query(id) else {
        return Ok(false);
    };
    std::fs::remove_file(path).map_err(io_at(path))?;
    Ok(true)
}

fn folder_at(root: &Path, path: &Path, parent: Option<Uuid>) -> Result<QueryFolder, AppError> {
    let meta = std::fs::metadata(path).map_err(io_at(path))?;
    Ok(QueryFolder {
        id: path_id("folder", &rel_path(root, path)),
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        parent_id: parent,
        created_at: created_and_modified(&meta).0,
    })
}

/// Creates a folder, or renames / moves one (its id changes with its path). The parent must exist
/// and must not be the folder itself or one inside it.
pub fn save_folder(root: &Path, input: &QueryFolderInput) -> Result<QueryFolder, AppError> {
    let name = valid_name(&input.name, "Folder")?;
    let stem = file_stem(&name);
    let scanned = scan(root)?;
    let parent_dir = match input.parent_id {
        Some(p) => scanned
            .folder_path(p)
            .cloned()
            .ok_or_else(|| AppError::NotFound("the parent folder no longer exists".into()))?,
        None => root.to_path_buf(),
    };
    std::fs::create_dir_all(&parent_dir).map_err(io_at(&parent_dir))?;
    let existing = input.id.and_then(|id| scanned.folder_path(id)).cloned();
    let target = match &existing {
        Some(old) => {
            if parent_dir.starts_with(old) {
                return Err(AppError::InvalidInput(
                    "a folder cannot be moved into itself".into(),
                ));
            }
            if old.parent() == Some(parent_dir.as_path()) && same_name(old, &parent_dir.join(&stem))
            {
                old.clone()
            } else {
                let except =
                    Some(old.as_path()).filter(|p| p.parent() == Some(parent_dir.as_path()));
                let (target, _) = unique_path(&parent_dir, &stem, "", except);
                std::fs::rename(old, &target).map_err(io_at(old))?;
                target
            }
        }
        None => {
            let (target, _) = unique_path(&parent_dir, &stem, "", None);
            std::fs::create_dir(&target).map_err(io_at(&target))?;
            target
        }
    };
    let parent = input.parent_id.filter(|_| parent_dir != root);
    folder_at(root, &target, parent)
}

/// Removes the listed `.sql` files under `dir` (not hidden ones, nor inside hidden folders, nor
/// ones over [`QUERY_FILE_MAX_BYTES`], which the library does not list), then the folders left
/// empty (bottom-up; `dir` too when it ends up empty). Returns how many files went.
fn remove_sql_tree(dir: &Path) -> Result<u64, AppError> {
    let mut removed = 0;
    for entry in sorted_entries(dir)? {
        // Hidden entries (`.draft.sql`, `.stversions/`, `.git/`) are not in the library: the
        // listing skips them, so Delete Folder must leave them alone too.
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            removed += remove_sql_tree(&path)?;
        } else if is_sql_file_name(&entry.file_name().to_string_lossy()) {
            // A dump over the size limit is not in the library either (the listing skips it).
            if entry
                .metadata()
                .is_ok_and(|m| m.len() > QUERY_FILE_MAX_BYTES)
            {
                continue;
            }
            std::fs::remove_file(&path).map_err(io_at(&path))?;
            removed += 1;
        }
    }
    // Not empty (other files, hidden ones, dumps): it stays.
    let _ = std::fs::remove_dir(dir);
    Ok(removed)
}

/// Deletes a folder's queries (and the folders inside it); returns how many queries went. Files
/// that are not queries stay, with the folders holding them.
pub fn delete_folder(root: &Path, id: Uuid) -> Result<u64, AppError> {
    let scanned = scan(root)?;
    let Some(dir) = scanned.folder_path(id) else {
        return Ok(0);
    };
    remove_sql_tree(dir)
}

/// The file of a saved query, or the folder of a query folder (Show in Finder).
pub fn path_of(root: &Path, id: Uuid, folder: bool) -> Result<PathBuf, AppError> {
    let scanned = scan(root)?;
    let found = if folder {
        scanned.folder_path(id).cloned()
    } else {
        scanned.query(id).map(|(_, p)| p.clone())
    };
    found.ok_or_else(|| AppError::NotFound("the query no longer exists".into()))
}

/// The query name an imported file gets: its name without `.sql` / `.sql.gz`.
fn import_name(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = strip_suffix_ci(&name, ".sql.gz")
        .or_else(|| strip_suffix_ci(&name, ".gz"))
        .unwrap_or(&name);
    stem_of(base)
}

fn strip_suffix_ci<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    let n = s.len().checked_sub(suffix.len())?;
    (s.is_char_boundary(n) && s[n..].eq_ignore_ascii_case(suffix)).then(|| &s[..n])
}

/// "Import…": copies `.sql` (or `.sql.gz`) files into the folder `folder_id` as new queries.
/// Their front matter (keyword, connection) is kept; ids already in the library are replaced.
/// A file over [`QUERY_FILE_MAX_BYTES`] (decompressed, for a `.gz`) is refused, naming it, before
/// it is read.
pub fn import_files(
    root: &Path,
    paths: &[PathBuf],
    folder_id: Option<Uuid>,
) -> Result<Vec<SavedQuery>, AppError> {
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let text = read_query_source(path)?;
        let (header, sql) = parse_query_file(&text);
        let taken: HashSet<Uuid> = scan(root)?.queries.iter().map(|(q, _)| q.id).collect();
        let id = header.id.filter(|id| !taken.contains(id));
        let name = import_name(path);
        out.push(save_query(
            root,
            &SavedQueryInput {
                id: Some(id.unwrap_or_else(Uuid::new_v4)),
                name: if name.trim().is_empty() {
                    "Imported".into()
                } else {
                    name
                },
                keyword: header.keyword.clone(),
                sql: sql.to_string(),
                folder_id,
                profile_id: header.connection,
                keep_sql: None,
            },
        )?);
    }
    Ok(out)
}

/// `path` with its longest existing ancestor resolved (symlinks, `/var` → `/private/var`), so
/// paths that do not exist yet compare with ones that do.
pub fn resolve(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut cur = path.to_path_buf();
    loop {
        if let Ok(canon) = std::fs::canonicalize(&cur) {
            let mut out = canon;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (cur.file_name().map(|n| n.to_os_string()), cur.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                cur = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Whether `inner` is `outer` or inside it.
pub fn is_within(inner: &Path, outer: &Path) -> bool {
    resolve(inner).starts_with(resolve(outer))
}

/// Copies the folders and `.sql` files of `from` into `to` (merging: a taken name gets ` 2`…).
/// Returns the source files copied. On failure, the copies made so far are removed again. A
/// `.sql` file over [`QUERY_FILE_MAX_BYTES`] fails the copy, naming it: the library would not
/// list it, and it must not be left behind unnoticed either.
pub fn copy_sql_tree(from: &Path, to: &Path) -> Result<Vec<PathBuf>, AppError> {
    fn go(
        from: &Path,
        to: &Path,
        depth: usize,
        copied: &mut Vec<PathBuf>,
        made: &mut Vec<PathBuf>,
    ) -> Result<(), AppError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        std::fs::create_dir_all(to).map_err(io_at(to))?;
        for entry in sorted_entries(from)? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                go(&path, &to.join(&name), depth + 1, copied, made)?;
            } else if is_sql_file_name(&name) && path.is_file() {
                if std::fs::metadata(&path).map_err(io_at(&path))?.len() > QUERY_FILE_MAX_BYTES {
                    return Err(oversized(&path));
                }
                let (target, _) = unique_path(to, &stem_of(&name), ".sql", None);
                std::fs::copy(&path, &target).map_err(io_at(&path))?;
                made.push(target);
                copied.push(path);
            }
        }
        Ok(())
    }
    if is_within(to, from) || is_within(from, to) {
        return Err(AppError::InvalidInput(
            "one folder is inside the other".into(),
        ));
    }
    let mut copied = Vec::new();
    let mut made = Vec::new();
    if from.is_dir()
        && let Err(e) = go(from, to, 0, &mut copied, &mut made)
    {
        for p in &made {
            let _ = std::fs::remove_file(p);
        }
        return Err(e);
    }
    Ok(copied)
}

/// Removes the source files `copy_sql_tree` copied, then the folders of `from` left empty.
pub fn remove_copied(from: &Path, copied: &[PathBuf]) {
    let mut dirs: HashSet<PathBuf> = HashSet::new();
    for p in copied {
        if std::fs::remove_file(p).is_ok()
            && let Some(parent) = p.parent()
        {
            let mut d = parent.to_path_buf();
            while d.starts_with(from) && d != from {
                dirs.insert(d.clone());
                if !d.pop() {
                    break;
                }
            }
        }
    }
    let mut dirs: Vec<PathBuf> = dirs.into_iter().collect();
    // Deepest first.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        let _ = std::fs::remove_dir(d);
    }
}

/// "Add Folder to Favorites…": copies a folder from disk (its `.sql` files and sub-folders) into
/// the queries folder, under `parent_id`.
pub fn add_folder(
    root: &Path,
    source: &Path,
    parent_id: Option<Uuid>,
) -> Result<QueryFolder, AppError> {
    if !source.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "{} is not a folder",
            source.display()
        )));
    }
    if is_within(source, root) || (root.exists() && is_within(root, source)) {
        return Err(AppError::InvalidInput(
            "that folder is (or holds) the queries folder".into(),
        ));
    }
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported".into());
    let folder = save_folder(
        root,
        &QueryFolderInput {
            id: None,
            name,
            parent_id,
        },
    )?;
    let dir = path_of(root, folder.id, true)?;
    if let Err(e) = copy_sql_tree(source, &dir) {
        let _ = remove_sql_tree(&dir);
        return Err(e);
    }
    Ok(folder)
}

/// A file stem cut to what every file system takes: at most 100 characters and 200 bytes (a name
/// of emoji is four bytes a character, and the temporary file of a write adds `.`, `.sql` and
/// `.tabler-tmp` to it).
fn short_stem(stem: &str) -> String {
    let mut out = String::new();
    for (i, c) in stem.chars().enumerate() {
        if i >= 100 || out.len() + c.len_utf8() > 200 {
            break;
        }
        out.push(c);
    }
    let out = out.trim_end();
    if out.is_empty() {
        "Untitled".to_string()
    } else {
        out.to_string()
    }
}

/// The folder in `parent` that already has this name (ignoring case: APFS does), if any.
fn existing_subdir(parent: &Path, stem: &str) -> Option<PathBuf> {
    let wanted = stem.to_lowercase();
    std::fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .find(|e| {
            e.file_type().is_ok_and(|t| t.is_dir())
                && e.file_name().to_string_lossy().to_lowercase() == wanted
        })
        .map(|e| e.path())
}

/// The folder a migrated folder `name` goes to under `parent`: the one of that name that is
/// already there (an earlier run made it), else a new one; a name the file system refuses is
/// shortened, and when even that fails the queries go to `parent`.
fn migration_dir(parent: &Path, name: &str) -> PathBuf {
    let stem = file_stem(name);
    let mut candidates = vec![stem.clone()];
    let short = short_stem(&stem);
    if short != stem {
        candidates.push(short);
    }
    if let Some(dir) = candidates.iter().find_map(|c| existing_subdir(parent, c)) {
        return dir;
    }
    for candidate in &candidates {
        let (dir, _) = unique_path(parent, candidate, "", None);
        match std::fs::create_dir_all(&dir) {
            Ok(()) => return dir,
            Err(e) => log::warn!("queries: cannot create the folder {}: {e}", dir.display()),
        }
    }
    parent.to_path_buf()
}

/// Writes a library kept in the app DB (before queries became files) into `root`: folders become
/// sub-folders, queries keep their ids. Returns how many queries were written.
///
/// Safe to run again over the same folder, also after a run that stopped halfway: a query whose
/// id is already in `root` (its file carries the `@id`) is not written a second time, and a
/// folder that is already there is reused rather than made again as `Name 2`. A query that cannot
/// be written (a name the file system refuses, a full disk, no permission) is logged and skipped
/// after one more try with a shortened file name: the others still migrate, and its row stays in
/// the app DB.
pub fn migrate_library(root: &Path, lib: &SavedQueryLibrary) -> Result<usize, AppError> {
    std::fs::create_dir_all(root).map_err(io_at(root))?;
    let present: HashSet<Uuid> = scan(root)?.queries.iter().map(|(q, _)| q.id).collect();
    // Parents first: a folder is placed once its parent has been.
    let mut dirs: HashMap<Uuid, PathBuf> = HashMap::new();
    let mut pending: Vec<&QueryFolder> = lib.folders.iter().collect();
    while !pending.is_empty() {
        let before = pending.len();
        let mut next = Vec::new();
        for f in pending {
            let parent_dir = match f.parent_id {
                None => Some(root.to_path_buf()),
                Some(p) if p == f.id => Some(root.to_path_buf()),
                Some(p) => dirs.get(&p).cloned(),
            };
            match parent_dir {
                Some(parent) => {
                    dirs.insert(f.id, migration_dir(&parent, &f.name));
                }
                None => next.push(f),
            }
        }
        if next.len() == before {
            // Orphans (a parent that no longer exists, a cycle): to the top level.
            for f in &next {
                dirs.insert(f.id, migration_dir(root, &f.name));
            }
            break;
        }
        pending = next;
    }
    let mut written = 0;
    for q in &lib.queries {
        if present.contains(&q.id) {
            continue;
        }
        let dir = q
            .folder_id
            .and_then(|f| dirs.get(&f).cloned())
            .unwrap_or_else(|| root.to_path_buf());
        let keyword = q.keyword.as_deref().filter(|k| !k.trim().is_empty());
        let text = render_query_file(q.id, keyword, q.profile_id, &q.sql);
        let stem = file_stem(&q.name);
        let mut stems = vec![stem.clone()];
        let short = short_stem(&stem);
        if short != stem {
            stems.push(short);
        }
        let mut failure = None;
        for candidate in &stems {
            let (path, _) = unique_path(&dir, candidate, ".sql", None);
            match write_atomic(&path, &text) {
                Ok(()) => {
                    failure = None;
                    written += 1;
                    break;
                }
                Err(e) => failure = Some(e),
            }
        }
        if let Some(e) = failure {
            log::warn!(
                "queries: \"{}\" was not moved out of the app database: {e}",
                q.name
            );
        }
    }
    Ok(written)
}

/// Serializes the one-time migration (two listings at startup must not both write the files).
static MIGRATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Once per app database: writes the saved queries it still holds (Tabler kept them there before
/// they became files) into `root`. The rows stay in the app DB as a backup. Queries that could
/// not be written are skipped (see [`migrate_library`]) and the migration still counts as done; a
/// run that cannot even start (the folder cannot be created) is retried on the next listing, and
/// running it again never duplicates what an earlier run wrote.
pub fn migrate_from_appdb(appdb: &crate::appdb::AppDb, root: &Path) -> Result<(), AppError> {
    let _guard = MIGRATION.lock().unwrap_or_else(|p| p.into_inner());
    if appdb.saved_queries_migrated()? {
        return Ok(());
    }
    let lib = appdb.list_saved_queries()?;
    if !lib.queries.is_empty() || !lib.folders.is_empty() {
        let n = migrate_library(root, &lib)?;
        log::info!(
            "moved {n} of {} saved queries from the app database to {}",
            lib.queries.len(),
            root.display()
        );
    }
    appdb.mark_saved_queries_migrated()
}

// --- the SQL editor's files ------------------------------------------------------------------

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// The refusal of a file the library does not take.
fn oversized(path: &Path) -> AppError {
    AppError::InvalidInput(format!(
        "{} is larger than {} MB, too big for a saved query",
        path.display(),
        QUERY_FILE_MAX_BYTES / (1024 * 1024)
    ))
}

/// The refusal of a file the editor will not read whole.
fn too_big_for_the_editor(path: &Path, limit: u64) -> AppError {
    AppError::InvalidInput(format!(
        "{} is larger than {} MB: open its first part instead, or import it as an SQL dump",
        path.display(),
        limit / (1024 * 1024)
    ))
}

/// The text of an SQL file and, when the file was not UTF-8, how it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlText {
    pub text: String,
    /// `None` = UTF-8; otherwise the label of the encoding the bytes were decoded as.
    pub encoding: Option<&'static str>,
}

/// How much of a file's start tells UTF-16 from Windows-1252.
const SNIFF_BYTES: usize = 64 * 1024;

/// Decodes the bytes of an SQL file: strict UTF-8 first, and when they are not UTF-8, UTF-16 (a
/// byte order mark, or NUL in every other byte) or else Windows-1252, which maps every byte, so
/// any file opens. The encoding is reported for the user: saving writes UTF-8.
pub fn decode_sql_bytes(bytes: Vec<u8>) -> SqlText {
    let bytes = match String::from_utf8(bytes) {
        Ok(text) => {
            return SqlText {
                text,
                encoding: None,
            };
        }
        Err(e) => e.into_bytes(),
    };
    let encoding = match detect_encoding(&bytes[..bytes.len().min(SNIFF_BYTES)]) {
        e @ (TextEncoding::Utf16Le | TextEncoding::Utf16Be) => e,
        _ => TextEncoding::Windows1252,
    };
    let mut decoded = Vec::with_capacity(bytes.len() + bytes.len() / 4);
    // Both decoders yield valid UTF-8 and reading a slice cannot fail; lossy only to be total.
    let _ = Utf8Reader::new(&bytes[..], encoding).read_to_end(&mut decoded);
    SqlText {
        text: String::from_utf8_lossy(&decoded).into_owned(),
        encoding: Some(encoding_label(encoding)),
    }
}

/// Text of an SQL file; a gzip file (`.sql.gz`, recognised by its content) is decompressed, and a
/// file that is not UTF-8 is read as UTF-16 / Windows-1252 ([`decode_sql_bytes`]).
pub fn read_sql_text(path: &Path) -> Result<String, AppError> {
    Ok(read_sql_decoded(path)?.text)
}

/// [`read_sql_text`] with the encoding it had to assume.
pub fn read_sql_decoded(path: &Path) -> Result<SqlText, AppError> {
    read_sql_text_limited(path, Some(EDITOR_FILE_MAX_BYTES), GZ_TEXT_MAX, || {
        too_big_for_the_editor(path, EDITOR_FILE_MAX_BYTES)
    })
}

/// The text of a file to import as a query: [`read_sql_text`] with the saved-query size limit,
/// checked before the file is read (and while a gzip file is decompressed).
fn read_query_source(path: &Path) -> Result<String, AppError> {
    Ok(read_sql_text_limited(
        path,
        Some(QUERY_FILE_MAX_BYTES),
        QUERY_FILE_MAX_BYTES,
        || oversized(path),
    )?
    .text)
}

/// [`read_sql_decoded`] that refuses a file over `file_limit` bytes (when given) without reading
/// it, and a gzip file that decompresses to more than `text_limit` bytes (`refusal` builds the
/// error of either).
fn read_sql_text_limited(
    path: &Path,
    file_limit: Option<u64>,
    text_limit: u64,
    refusal: impl Fn() -> AppError,
) -> Result<SqlText, AppError> {
    if let Some(limit) = file_limit
        && std::fs::metadata(path).map_err(io_at(path))?.len() > limit
    {
        return Err(refusal());
    }
    let bytes = std::fs::read(path).map_err(io_at(path))?;
    let bytes = if bytes.starts_with(&GZIP_MAGIC) {
        let mut out = Vec::new();
        flate2::read::MultiGzDecoder::new(&bytes[..])
            .take(text_limit + 1)
            .read_to_end(&mut out)
            .map_err(io_at(path))?;
        if out.len() as u64 > text_limit {
            return Err(AppError::InvalidInput(format!(
                "{} decompresses to more than {} MB",
                path.display(),
                text_limit / (1024 * 1024)
            )));
        }
        out
    } else {
        bytes
    };
    Ok(decode_sql_bytes(bytes))
}

/// Whether the file starts with the gzip magic. Reads two bytes, never the file.
fn is_gzip_file(path: &Path) -> Result<bool, AppError> {
    let mut magic = [0u8; 2];
    let mut file = std::fs::File::open(path).map_err(io_at(path))?;
    let mut got = 0;
    while got < magic.len() {
        match file.read(&mut magic[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(io_at(path)(e)),
        }
    }
    Ok(got == magic.len() && magic == GZIP_MAGIC)
}

/// The size of an SQL file and whether it is gzip-compressed, from its metadata and its first two
/// bytes: the editor asks before it reads a file that may be huge.
pub fn sql_file_info(path: &Path) -> Result<SqlFileInfo, AppError> {
    let meta = std::fs::metadata(path).map_err(io_at(path))?;
    if !meta.is_file() {
        return Err(AppError::InvalidInput(format!(
            "{} is not a file",
            path.display()
        )));
    }
    Ok(SqlFileInfo {
        bytes: meta.len(),
        compressed: is_gzip_file(path)?,
    })
}

/// The first `max_bytes` of an SQL file as text (`None`: all of it, as [`read_sql_decoded`]).
///
/// Only the head is read; a gzip file is streamed, so only the head is decompressed. A truncated
/// head ends after its last complete line (a single line longer than `max_bytes` is cut at a
/// character), `truncated` says the file goes on, and a file that is not UTF-8 is decoded like
/// [`decode_sql_bytes`].
pub fn read_sql_head(path: &Path, max_bytes: Option<u64>) -> Result<SqlFileHead, AppError> {
    let info = sql_file_info(path)?;
    let Some(max) = max_bytes else {
        let whole = read_sql_decoded(path)?;
        return Ok(SqlFileHead {
            text: whole.text,
            total_bytes: info.bytes,
            truncated: false,
            compressed: info.compressed,
            encoding: whole.encoding.map(str::to_string),
        });
    };
    let file = std::fs::File::open(path).map_err(io_at(path))?;
    let source: Box<dyn Read> = if info.compressed {
        Box::new(flate2::read::MultiGzDecoder::new(std::io::BufReader::new(
            file,
        )))
    } else {
        Box::new(file)
    };
    // One byte more than asked for tells whether the file goes on.
    let mut bytes = Vec::new();
    source
        .take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io_at(path))?;
    let truncated = bytes.len() as u64 > max;
    if truncated {
        bytes.truncate(usize::try_from(max).unwrap_or(usize::MAX));
        // The cut may fall inside a UTF-8 character: drop the incomplete end, it is not an error.
        if let Err(e) = std::str::from_utf8(&bytes)
            && e.error_len().is_none()
        {
            bytes.truncate(e.valid_up_to());
        }
    }
    let SqlText { mut text, encoding } = decode_sql_bytes(bytes);
    if truncated && let Some(end) = text.rfind('\n') {
        text.truncate(end + 1);
    }
    Ok(SqlFileHead {
        text,
        total_bytes: info.bytes,
        truncated,
        compressed: info.compressed,
        encoding: encoding.map(str::to_string),
    })
}

/// Writes an SQL file; a path ending in `.gz` is written gzip-compressed. The file is replaced
/// atomically ([`write_atomic_bytes`]): a failure partway (full disk, crash) leaves the old
/// content, never a truncated file.
pub fn write_sql_text(path: &Path, content: &str) -> Result<(), AppError> {
    let gz = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gz"));
    if !gz {
        return write_atomic_bytes(path, content.as_bytes());
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(content.as_bytes()).map_err(io_at(path))?;
    let bytes = encoder.finish().map_err(io_at(path))?;
    write_atomic_bytes(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, sql: &str, folder: Option<Uuid>) -> SavedQueryInput {
        SavedQueryInput {
            id: None,
            name: name.into(),
            keyword: None,
            sql: sql.into(),
            folder_id: folder,
            profile_id: None,
            keep_sql: None,
        }
    }

    fn folder(name: &str, parent: Option<Uuid>) -> QueryFolderInput {
        QueryFolderInput {
            id: None,
            name: name.into(),
            parent_id: parent,
        }
    }

    #[test]
    fn front_matter_round_trips() {
        let id = Uuid::new_v4();
        let conn = Uuid::new_v4();
        let text = render_query_file(id, Some("recent"), Some(conn), "select 1;\n");
        let (h, sql) = parse_query_file(&text);
        assert_eq!(h.id, Some(id));
        assert_eq!(h.keyword.as_deref(), Some("recent"));
        assert_eq!(h.connection, Some(conn));
        assert_eq!(sql, "select 1;\n");
        // Plain SQL, a comment that is not front matter, CRLF line ends.
        let (h, sql) = parse_query_file("-- just a comment\nselect 2");
        assert_eq!(h, Header::default());
        assert_eq!(sql, "-- just a comment\nselect 2");
        let (h, sql) = parse_query_file("-- @keyword: k\r\nselect 3");
        assert_eq!(h.keyword.as_deref(), Some("k"));
        assert_eq!(sql, "select 3");
        let (_, sql) = parse_query_file("-- @other: x\nselect 4");
        assert_eq!(sql, "-- @other: x\nselect 4");
    }

    #[test]
    fn file_stems_are_safe() {
        assert_eq!(file_stem("Posts per user"), "Posts per user");
        assert_eq!(file_stem("a/b:c\\d"), "a-b-c-d");
        assert_eq!(file_stem("..hidden"), "hidden");
        assert_eq!(file_stem("  "), "Untitled");
        assert_eq!(path_id("folder", "a/b"), path_id("folder", "a/b"));
        assert_ne!(path_id("folder", "a/b"), path_id("query", "a/b"));
    }

    #[test]
    fn queries_and_folders_crud_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Queries");
        assert!(list(&root).unwrap().queries.is_empty());

        let reports = save_folder(&root, &folder("Reports", None)).unwrap();
        assert!(root.join("Reports").is_dir());
        let q = save_query(
            &root,
            &SavedQueryInput {
                keyword: Some("ppu".into()),
                profile_id: Some(Uuid::nil()),
                ..input("Posts per user", "select 1;", Some(reports.id))
            },
        )
        .unwrap();
        assert_eq!(q.folder_id, Some(reports.id));
        assert_eq!(q.keyword.as_deref(), Some("ppu"));
        assert_eq!(q.profile_id, Some(Uuid::nil()));
        assert!(root.join("Reports/Posts per user.sql").is_file());

        // Same name in the same folder: " 2".
        let q2 = save_query(
            &root,
            &input("posts per user", "select 2;", Some(reports.id)),
        )
        .unwrap();
        assert_eq!(q2.name, "posts per user 2");

        // Rename + move to the top level keeps the id.
        let moved = save_query(
            &root,
            &SavedQueryInput {
                id: Some(q.id),
                ..input("Renamed", "select 11;", None)
            },
        )
        .unwrap();
        assert_eq!(moved.id, q.id);
        assert_eq!(moved.folder_id, None);
        assert!(root.join("Renamed.sql").is_file());
        assert!(!root.join("Reports/Posts per user.sql").exists());
        let lib = list(&root).unwrap();
        assert_eq!(lib.queries.len(), 2);
        let found = lib.queries.iter().find(|x| x.id == q.id).unwrap();
        assert_eq!(found.sql, "select 11;");
        assert_eq!(found.keyword, None);

        // Case-only rename.
        let renamed = save_query(
            &root,
            &SavedQueryInput {
                id: Some(q.id),
                ..input("RENAMED", "select 12;", None)
            },
        )
        .unwrap();
        assert_eq!(renamed.name, "RENAMED");
        assert_eq!(list(&root).unwrap().queries.len(), 2);

        // Folders nest, rename and refuse to move into themselves.
        let inner = save_folder(&root, &folder("Inner", Some(reports.id))).unwrap();
        assert_eq!(inner.parent_id, Some(reports.id));
        let err = save_folder(
            &root,
            &QueryFolderInput {
                id: Some(reports.id),
                name: "Reports".into(),
                parent_id: Some(inner.id),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("into itself"), "{err}");
        let renamed_folder = save_folder(
            &root,
            &QueryFolderInput {
                id: Some(reports.id),
                name: "Monthly".into(),
                parent_id: None,
            },
        )
        .unwrap();
        assert!(root.join("Monthly/Inner").is_dir());
        assert_ne!(renamed_folder.id, reports.id);

        // Deleting a folder removes its queries but keeps files that are not queries.
        std::fs::write(root.join("Monthly/notes.txt"), "keep me").unwrap();
        save_query(
            &root,
            &input(
                "In inner",
                "select 3;",
                Some(path_id("folder", "Monthly/Inner")),
            ),
        )
        .unwrap();
        let n = delete_folder(&root, renamed_folder.id).unwrap();
        assert_eq!(n, 2);
        assert!(root.join("Monthly/notes.txt").is_file());
        assert!(!root.join("Monthly/Inner").exists());

        assert!(delete_query(&root, q.id).unwrap());
        assert!(!delete_query(&root, q.id).unwrap());
        let missing = save_query(&root, &input("x", "", Some(Uuid::new_v4()))).unwrap_err();
        assert!(matches!(missing, AppError::NotFound(_)));
        assert!(save_query(&root, &input(" ", "", None)).is_err());
    }

    #[test]
    fn plain_files_list_with_path_ids_and_hidden_ones_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Ops/.git")).unwrap();
        std::fs::write(root.join("Ops/vacuum.SQL"), "vacuum;").unwrap();
        std::fs::write(root.join(".secret.sql"), "x").unwrap();
        std::fs::write(root.join("readme.md"), "x").unwrap();
        let lib = list(root).unwrap();
        assert_eq!(lib.folders.len(), 1);
        assert_eq!(lib.queries.len(), 1);
        let q = &lib.queries[0];
        assert_eq!(q.name, "vacuum");
        assert_eq!(q.id, path_id("query", "Ops/vacuum.SQL"));
        assert_eq!(q.folder_id, Some(lib.folders[0].id));
        // Two files with the same id (a copy made in Finder) get distinct ids.
        let id = Uuid::new_v4();
        std::fs::write(root.join("a.sql"), render_query_file(id, None, None, "a")).unwrap();
        std::fs::write(root.join("b.sql"), render_query_file(id, None, None, "b")).unwrap();
        let ids: HashSet<Uuid> = list(root).unwrap().queries.iter().map(|q| q.id).collect();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn migrates_imports_and_copies_trees() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Q");
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();
        let at = "2026-01-01T00:00:00.000Z".to_string();
        let lib = SavedQueryLibrary {
            folders: vec![
                QueryFolder {
                    id: child,
                    name: "Child".into(),
                    parent_id: Some(parent),
                    created_at: at.clone(),
                },
                QueryFolder {
                    id: parent,
                    name: "Parent".into(),
                    parent_id: None,
                    created_at: at.clone(),
                },
            ],
            queries: vec![SavedQuery {
                id: Uuid::new_v4(),
                name: "Deep".into(),
                keyword: Some("deep".into()),
                sql: "select 1".into(),
                folder_id: Some(child),
                profile_id: None,
                created_at: at.clone(),
                updated_at: at,
            }],
        };
        assert_eq!(migrate_library(&root, &lib).unwrap(), 1);
        let listed = list(&root).unwrap();
        assert_eq!(listed.queries[0].id, lib.queries[0].id);
        assert_eq!(listed.queries[0].keyword.as_deref(), Some("deep"));
        assert!(root.join("Parent/Child/Deep.sql").is_file());

        // Import… a plain and a gzipped file.
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("one.sql"), "select 'one';").unwrap();
        write_sql_text(&src.join("two.sql.gz"), "select 'two';").unwrap();
        let imported =
            import_files(&root, &[src.join("one.sql"), src.join("two.sql.gz")], None).unwrap();
        assert_eq!(
            imported.iter().map(|q| q.name.as_str()).collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(imported[1].sql, "select 'two';");

        // Add Folder to Favorites…
        std::fs::write(src.join("sub/three.sql"), "select 3;").unwrap();
        let added = add_folder(&root, &src, None).unwrap();
        assert_eq!(added.name, "src");
        assert!(root.join("src/sub/three.sql").is_file());
        assert!(add_folder(&root, &root.join("Parent"), None).is_err());

        // Moving the whole tree elsewhere.
        let to = dir.path().join("elsewhere");
        let copied = copy_sql_tree(&root, &to).unwrap();
        assert_eq!(copied.len(), 5);
        remove_copied(&root, &copied);
        assert!(list(&root).unwrap().queries.is_empty());
        assert_eq!(list(&to).unwrap().queries.len(), 5);
        assert!(copy_sql_tree(&to, &to.join("x")).is_err());
    }

    #[test]
    fn migrates_the_app_db_library_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Queries");
        let db = crate::appdb::AppDb::open_in_memory().unwrap();
        let saved = db
            .save_saved_query(&input("Legacy", "select 'legacy';", None))
            .unwrap();
        migrate_from_appdb(&db, &root).unwrap();
        let lib = list(&root).unwrap();
        assert_eq!(lib.queries.len(), 1);
        assert_eq!(lib.queries[0].id, saved.id);
        // Not again: a query deleted from the folder does not come back.
        assert!(delete_query(&root, saved.id).unwrap());
        migrate_from_appdb(&db, &root).unwrap();
        assert!(list(&root).unwrap().queries.is_empty());
    }

    #[test]
    fn sql_files_read_gzip_and_write_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let gz = dir.path().join("dump.sql.gz");
        write_sql_text(&gz, "select 1;\n").unwrap();
        assert_eq!(std::fs::read(&gz).unwrap()[..2], GZIP_MAGIC);
        assert_eq!(read_sql_text(&gz).unwrap(), "select 1;\n");
        let plain = dir.path().join("a.sql");
        write_sql_text(&plain, "select 2;").unwrap();
        assert_eq!(std::fs::read_to_string(&plain).unwrap(), "select 2;");
        assert_eq!(read_sql_text(&plain).unwrap(), "select 2;");
        // Not UTF-8: it opens anyway (here as UTF-16 LE with a stray byte), it is no error.
        std::fs::write(&plain, [0xff, 0xfe, 0x00]).unwrap();
        let odd = read_sql_decoded(&plain).unwrap();
        assert_eq!(odd.encoding, Some("UTF-16 LE"));
        assert_eq!(odd.text, "\u{FFFD}");
    }

    // ---- any file opens; a big one opens as its head -------------------------------------------

    #[test]
    fn a_latin_1_file_is_read_as_windows_1252_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.sql");
        // é (E9), € (80), “ ” (93 94) and the undefined 81.
        std::fs::write(
            &path,
            b"insert into t values ('caf\xe9 \x80 \x93q\x94 \x81');\n",
        )
        .unwrap();
        let read = read_sql_decoded(&path).unwrap();
        assert_eq!(read.encoding, Some("Windows-1252"));
        assert_eq!(
            read.text,
            "insert into t values ('café € \u{201C}q\u{201D} \u{81}');\n"
        );
        assert_eq!(read_sql_text(&path).unwrap(), read.text);

        // UTF-8 (with or without accents) reports nothing and is unchanged.
        let utf8 = dir.path().join("utf8.sql");
        std::fs::write(&utf8, "select 'café €';").unwrap();
        assert_eq!(
            read_sql_decoded(&utf8).unwrap(),
            SqlText {
                text: "select 'café €';".into(),
                encoding: None
            }
        );

        // UTF-16 with a byte order mark is decoded and the mark dropped.
        let utf16 = dir.path().join("utf16.sql");
        let mut bytes = vec![0xff, 0xfe];
        for unit in "select 'é';\n".encode_utf16() {
            bytes.extend(unit.to_le_bytes());
        }
        std::fs::write(&utf16, &bytes).unwrap();
        let read = read_sql_decoded(&utf16).unwrap();
        assert_eq!(
            (read.text.as_str(), read.encoding),
            ("select 'é';\n", Some("UTF-16 LE"))
        );

        // The same through a gzip file.
        let gz = dir.path().join("legacy.sql.gz");
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(b"select 'caf\xe9';").unwrap();
        std::fs::write(&gz, enc.finish().unwrap()).unwrap();
        let read = read_sql_decoded(&gz).unwrap();
        assert_eq!(
            (read.text.as_str(), read.encoding),
            ("select 'café';", Some("Windows-1252"))
        );
    }

    /// `n` numbered lines of the same length.
    fn numbered_lines(n: usize) -> String {
        (0..n)
            .map(|i| format!("insert into t values ({i:06}, 'row');\n"))
            .collect()
    }

    #[test]
    fn a_head_ends_at_a_line_boundary_and_says_the_file_goes_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.sql");
        let all = numbered_lines(1000);
        std::fs::write(&path, &all).unwrap();

        // Cut in the middle of a line: the head stops after the last complete one.
        let head = read_sql_head(&path, Some(1000)).unwrap();
        assert!(head.truncated);
        assert_eq!(head.total_bytes, all.len() as u64);
        assert!(!head.compressed && head.encoding.is_none());
        assert!(
            head.text.len() <= 1000 && head.text.len() > 900,
            "{}",
            head.text.len()
        );
        assert!(head.text.ends_with(";\n"));
        assert!(all.starts_with(&head.text));

        // Exactly the whole file, or more than it: everything, not truncated.
        for max in [all.len() as u64, all.len() as u64 + 1, u64::MAX] {
            let head = read_sql_head(&path, Some(max)).unwrap();
            assert!(!head.truncated, "{max}");
            assert_eq!(head.text, all);
        }
        // One byte short is truncated (and ends at the previous line).
        let head = read_sql_head(&path, Some(all.len() as u64 - 1)).unwrap();
        assert!(head.truncated);
        assert_eq!(head.text, all[..all.len() - numbered_lines(1).len()]);
        // No limit: all of it.
        assert_eq!(read_sql_head(&path, None).unwrap().text, all);
        // Nothing asked for, nothing returned (and the file is still said to go on).
        let none = read_sql_head(&path, Some(0)).unwrap();
        assert!(none.truncated && none.text.is_empty());
    }

    #[test]
    fn a_head_never_splits_a_character_or_fails_on_one_long_line() {
        let dir = tempfile::tempdir().unwrap();
        // One line of 3-byte characters: every cut point but multiples of 3 is inside one.
        let path = dir.path().join("one-line.sql");
        let line = "€".repeat(500);
        std::fs::write(&path, &line).unwrap();
        for max in [10u64, 11, 12, 100, 101, 1000] {
            let head = read_sql_head(&path, Some(max)).unwrap();
            assert!(head.truncated, "{max}");
            assert!(
                head.encoding.is_none(),
                "cut at {max}: read as {:?}",
                head.encoding
            );
            assert_eq!(head.text, "€".repeat(max as usize / 3), "{max}");
        }
        // A cut inside a Windows-1252 file is still Windows-1252 (not a failure).
        let latin = dir.path().join("latin.sql");
        std::fs::write(&latin, b"select 'caf\xe9';\nselect 'caf\xe9';\n").unwrap();
        let head = read_sql_head(&latin, Some(20)).unwrap();
        assert_eq!(head.encoding.as_deref(), Some("Windows-1252"));
        assert_eq!(head.text, "select 'café';\n");
    }

    #[test]
    fn a_gzip_head_decompresses_only_the_head() {
        let dir = tempfile::tempdir().unwrap();
        let all = numbered_lines(40_000);
        let gz = dir.path().join("dump.sql.gz");
        write_sql_text(&gz, &all).unwrap();
        let info = sql_file_info(&gz).unwrap();
        assert!(info.compressed);
        assert!(
            info.bytes < all.len() as u64 / 3,
            "the file is the compressed size"
        );

        let head = read_sql_head(&gz, Some(4096)).unwrap();
        assert!(head.truncated && head.compressed);
        assert_eq!(head.total_bytes, info.bytes);
        assert!(all.starts_with(&head.text) && head.text.len() > 4000);

        // Proof that the rest is never decompressed: with the end of the archive missing, the
        // head still reads (a whole read hits the unexpected end and fails).
        let bytes = std::fs::read(&gz).unwrap();
        let broken = dir.path().join("broken.sql.gz");
        std::fs::write(&broken, &bytes[..bytes.len() / 2]).unwrap();
        assert!(read_sql_text(&broken).is_err());
        let head = read_sql_head(&broken, Some(4096)).unwrap();
        assert!(head.truncated);
        assert!(all.starts_with(&head.text) && head.text.len() > 4000);
        // A gzip file is recognised by its content, whatever its name.
        let renamed = dir.path().join("dump.sql");
        std::fs::copy(&gz, &renamed).unwrap();
        assert!(sql_file_info(&renamed).unwrap().compressed);
        assert!(read_sql_head(&renamed, None).unwrap().text == all);
    }

    #[test]
    fn info_and_a_head_never_read_a_huge_file_whole() {
        let dir = tempfile::tempdir().unwrap();
        // 3 GiB, sparse: reading it whole would take seconds and gigabytes of memory.
        let huge = dir.path().join("dump.sql");
        sparse_file(&huge, 3 << 30);
        let started = std::time::Instant::now();
        let info = sql_file_info(&huge).unwrap();
        assert_eq!(
            info,
            SqlFileInfo {
                bytes: 3 << 30,
                compressed: false
            }
        );
        let head = read_sql_head(&huge, Some(1 << 20)).unwrap();
        assert!(head.truncated);
        assert_eq!(head.text.len(), 1 << 20);
        assert_eq!(head.total_bytes, 3 << 30);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        // A whole read of it is refused up front (the editor reads at most EDITOR_FILE_MAX_BYTES).
        let err = read_sql_decoded(&huge).unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)), "{err:?}");
        assert!(err.to_string().contains("first part"), "{err}");
        // A 40 MB file (a "large file" for the editor's prompt) is still readable whole when the
        // user insists; `sql_file_info` is what lets the editor ask first.
        let forty = dir.path().join("forty.sql");
        sparse_file(&forty, 40 * 1024 * 1024);
        assert_eq!(sql_file_info(&forty).unwrap().bytes, 40 * 1024 * 1024);
        let whole = read_sql_head(&forty, None).unwrap();
        assert!(!whole.truncated && whole.text.len() == 40 * 1024 * 1024);

        assert!(sql_file_info(&dir.path().join("missing.sql")).is_err());
        let err = sql_file_info(dir.path()).unwrap_err();
        assert!(err.to_string().contains("not a file"), "{err}");
    }

    // ---- rename / move never rewrite the SQL of a file (keep_sql) ------------------------------

    fn as_input(q: &SavedQuery) -> SavedQueryInput {
        SavedQueryInput {
            id: Some(q.id),
            name: q.name.clone(),
            keyword: q.keyword.clone(),
            sql: q.sql.clone(),
            folder_id: q.folder_id,
            profile_id: q.profile_id,
            keep_sql: None,
        }
    }

    /// A query file with front matter and a raw body (which may not be UTF-8).
    fn write_query_bytes(root: &Path, rel: &str, id: Uuid, body: &[u8]) -> PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = render_query_file(id, None, None, "").into_bytes();
        bytes.extend_from_slice(body);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    const LATIN_1: &[u8] = b"select 'caf\xe9';\n";

    #[test]
    fn renaming_with_keep_sql_leaves_a_latin_1_file_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let id = Uuid::new_v4();
        let old = write_query_bytes(root, "report.sql", id, LATIN_1);
        let original = std::fs::read(&old).unwrap();
        // What the pane holds: the lossy text.
        let listed = list(root).unwrap().queries.remove(0);
        assert!(listed.sql.contains('\u{fffd}'));

        let renamed = save_query(
            root,
            &SavedQueryInput {
                name: "Renamed".into(),
                keep_sql: Some(true),
                ..as_input(&listed)
            },
        )
        .unwrap();
        assert_eq!(renamed.id, id);
        assert_eq!(renamed.name, "Renamed");
        assert!(!old.exists());
        assert_eq!(std::fs::read(root.join("Renamed.sql")).unwrap(), original);
        assert!(
            std::fs::read(root.join("Renamed.sql"))
                .unwrap()
                .contains(&0xe9)
        );
    }

    #[test]
    fn without_keep_sql_the_given_sql_is_still_written() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let id = Uuid::new_v4();
        write_query_bytes(root, "report.sql", id, LATIN_1);
        let listed = list(root).unwrap().queries.remove(0);
        for keep_sql in [None, Some(false)] {
            let saved = save_query(
                root,
                &SavedQueryInput {
                    keep_sql,
                    sql: "select 'from the dialog';".into(),
                    ..as_input(&listed)
                },
            )
            .unwrap();
            assert_eq!(saved.sql, "select 'from the dialog';");
            assert_eq!(
                std::fs::read_to_string(root.join("report.sql")).unwrap(),
                render_query_file(id, None, None, "select 'from the dialog';")
            );
        }
        // The pane's lossy copy, sent without the flag, is what ends up in the file.
        let lossy = save_query(root, &as_input(&listed)).unwrap();
        assert!(lossy.sql.contains('\u{fffd}'));
    }

    #[test]
    fn a_stale_copy_does_not_overwrite_an_external_edit_on_move_or_rename() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cached = save_query(root, &input("Daily", "select 1;", None)).unwrap();
        let reports = save_folder(root, &folder("Reports", None)).unwrap();
        // Edited in another editor (or by a sync client) after the pane listed it.
        std::fs::write(
            root.join("Daily.sql"),
            render_query_file(cached.id, None, None, "select 'edited elsewhere';"),
        )
        .unwrap();

        let moved = save_query(
            root,
            &SavedQueryInput {
                folder_id: Some(reports.id),
                keep_sql: Some(true),
                ..as_input(&cached)
            },
        )
        .unwrap();
        assert_eq!(moved.id, cached.id);
        assert_eq!(moved.sql, "select 'edited elsewhere';");
        assert!(!root.join("Daily.sql").exists());
        let text = std::fs::read_to_string(root.join("Reports/Daily.sql")).unwrap();
        assert!(text.ends_with("select 'edited elsewhere';"), "{text}");

        // Still the stale copy, renamed in place.
        let renamed = save_query(
            root,
            &SavedQueryInput {
                name: "Weekly".into(),
                folder_id: Some(reports.id),
                keep_sql: Some(true),
                ..as_input(&cached)
            },
        )
        .unwrap();
        assert_eq!(renamed.sql, "select 'edited elsewhere';");
        assert!(root.join("Reports/Weekly.sql").is_file());
        assert_eq!(list(root).unwrap().queries.len(), 1);
    }

    #[test]
    fn keep_sql_rewrites_only_the_header_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cached = save_query(root, &input("Tagged", "select 1;", None)).unwrap();
        let path = root.join("Tagged.sql");
        let mut external = render_query_file(cached.id, None, None, "").into_bytes();
        external.extend_from_slice(LATIN_1);
        std::fs::write(&path, &external).unwrap();

        let connection = Uuid::new_v4();
        let tagged = save_query(
            root,
            &SavedQueryInput {
                keyword: Some("recent".into()),
                profile_id: Some(connection),
                sql: "STALE".into(),
                keep_sql: Some(true),
                ..as_input(&cached)
            },
        )
        .unwrap();
        assert_eq!(tagged.keyword.as_deref(), Some("recent"));
        assert_eq!(tagged.profile_id, Some(connection));
        let mut expected =
            render_query_file(cached.id, Some("recent"), Some(connection), "").into_bytes();
        expected.extend_from_slice(LATIN_1);
        assert_eq!(std::fs::read(&path).unwrap(), expected);
        assert!(!String::from_utf8_lossy(&std::fs::read(&path).unwrap()).contains("STALE"));
        // Nothing left behind.
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
    }

    #[test]
    fn keep_sql_gives_a_file_without_front_matter_its_id_and_keeps_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut plain = BOM.to_vec();
        plain.extend_from_slice(LATIN_1);
        std::fs::write(root.join("plain.sql"), &plain).unwrap();
        let listed = list(root).unwrap().queries.remove(0);
        assert_eq!(listed.id, path_id("query", "plain.sql"));

        let renamed = save_query(
            root,
            &SavedQueryInput {
                name: "Better".into(),
                keep_sql: Some(true),
                ..as_input(&listed)
            },
        )
        .unwrap();
        // The id is written so that it survives the rename; the SQL bytes are the file's own.
        assert_eq!(renamed.id, listed.id);
        let bytes = std::fs::read(root.join("Better.sql")).unwrap();
        let mut expected = BOM.to_vec();
        expected.extend_from_slice(render_query_file(listed.id, None, None, "").as_bytes());
        expected.extend_from_slice(LATIN_1);
        assert_eq!(bytes, expected);
        let again = list(root).unwrap().queries;
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].id, listed.id);
        assert_eq!(again[0].name, "Better");
    }

    #[test]
    fn keep_sql_on_a_query_that_does_not_exist_creates_it_from_the_sql() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let created = save_query(
            root,
            &SavedQueryInput {
                id: Some(Uuid::new_v4()),
                keep_sql: Some(true),
                ..input("New", "select 5;", None)
            },
        )
        .unwrap();
        assert_eq!(created.sql, "select 5;");
    }

    // ---- size and count limits ------------------------------------------------------------------

    fn sparse_file(path: &Path, len: u64) {
        std::fs::File::create(path).unwrap().set_len(len).unwrap();
    }

    #[test]
    fn oversized_files_are_neither_listed_nor_imported() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Queries");
        save_query(&root, &input("Normal", "select 1;", None)).unwrap();
        let dump = root.join("dump.sql");
        sparse_file(&dump, QUERY_FILE_MAX_BYTES + 1);
        sparse_file(&root.join("edge.sql"), QUERY_FILE_MAX_BYTES);

        let started = std::time::Instant::now();
        let names: Vec<_> = list(&root)
            .unwrap()
            .queries
            .into_iter()
            .map(|q| q.name)
            .collect();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        // Exactly at the limit still lists; one byte over does not.
        assert_eq!(names, ["edge", "Normal"]);

        let err = import_files(&root, std::slice::from_ref(&dump), None).unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)), "{err:?}");
        assert!(err.to_string().contains("dump.sql"), "{err}");
        assert!(err.to_string().contains("5 MB"), "{err}");

        // A gzip file that unpacks past the limit.
        let gz = dir.path().join("huge.sql.gz");
        write_sql_text(&gz, &"a".repeat(QUERY_FILE_MAX_BYTES as usize + 1)).unwrap();
        let err = import_files(&root, std::slice::from_ref(&gz), None).unwrap_err();
        assert!(err.to_string().contains("huge.sql.gz"), "{err}");
        assert_eq!(list(&root).unwrap().queries.len(), 2);
    }

    #[test]
    fn add_folder_refuses_an_oversized_file_and_copies_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Queries");
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.sql"), "select 1;").unwrap();
        sparse_file(&src.join("sub/dump.sql"), QUERY_FILE_MAX_BYTES + 1);

        let err = add_folder(&root, &src, None).unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)), "{err:?}");
        assert!(err.to_string().contains("dump.sql"), "{err}");
        assert!(list(&root).unwrap().queries.is_empty());
        assert!(!root.join("src").exists());
        // The source is untouched.
        assert!(src.join("a.sql").is_file() && src.join("sub/dump.sql").is_file());

        let to = dir.path().join("elsewhere");
        let err = copy_sql_tree(&src, &to).unwrap_err();
        assert!(err.to_string().contains("dump.sql"), "{err}");
        assert!(
            !to.join("a.sql").exists(),
            "the copies made before the refusal are removed again"
        );
    }

    #[test]
    fn a_listing_stops_at_its_limits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["a", "b", "c", "d", "e"] {
            std::fs::write(root.join(format!("{name}.sql")), "select 1;").unwrap();
        }
        let scan = scan_limited(
            root,
            Limits {
                queries: 3,
                entries: 100,
            },
        )
        .unwrap();
        assert_eq!(scan.queries.len(), 3);
        assert!(scan.truncated);
        let scan = scan_limited(
            root,
            Limits {
                queries: 100,
                entries: 4,
            },
        )
        .unwrap();
        assert_eq!(scan.queries.len(), 4);
        assert!(scan.truncated);
        // Exactly at the limits is not a truncation.
        let scan = scan_limited(
            root,
            Limits {
                queries: 5,
                entries: 5,
            },
        )
        .unwrap();
        assert_eq!(scan.queries.len(), 5);
        assert!(!scan.truncated);
        assert!(!list_is_truncated(root));
    }

    fn list_is_truncated(root: &Path) -> bool {
        scan(root).unwrap().truncated
    }

    // ---- Delete Folder only removes what the library lists -------------------------------------

    #[test]
    fn delete_folder_leaves_hidden_files_and_folders_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let reports = save_folder(root, &folder("Reports", None)).unwrap();
        save_query(root, &input("Visible", "select 1;", Some(reports.id))).unwrap();
        let inner = save_folder(root, &folder("Inner", Some(reports.id))).unwrap();
        save_query(root, &input("Deeper", "select 2;", Some(inner.id))).unwrap();
        let base = root.join("Reports");
        std::fs::write(base.join(".draft.sql"), "draft").unwrap();
        std::fs::create_dir_all(base.join(".stversions")).unwrap();
        std::fs::write(base.join(".stversions/q (v1).sql"), "v1").unwrap();
        std::fs::create_dir_all(base.join(".backup")).unwrap();
        std::fs::write(base.join(".backup/old.sql"), "old").unwrap();
        std::fs::write(base.join("Inner/.hidden.sql"), "hidden").unwrap();
        // A dump over the size limit is not listed either.
        sparse_file(&base.join("dump.sql"), QUERY_FILE_MAX_BYTES + 1);

        // The pane lists two queries, and the confirmation counts them.
        assert_eq!(list(root).unwrap().queries.len(), 2);
        let removed = delete_folder(root, reports.id).unwrap();
        assert_eq!(removed, 2);

        assert!(!base.join("Visible.sql").exists());
        assert!(!base.join("Inner/Deeper.sql").exists());
        for kept in [
            ".draft.sql",
            ".stversions/q (v1).sql",
            ".backup/old.sql",
            "Inner/.hidden.sql",
            "dump.sql",
        ] {
            assert!(base.join(kept).is_file(), "{kept} was deleted");
        }
        // The folders that still hold something stay.
        assert!(base.is_dir() && base.join("Inner").is_dir());
    }

    // ---- the editor's files are written atomically ----------------------------------------------

    fn entries_of(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[cfg(unix)]
    #[test]
    fn overwriting_an_sql_file_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.sql");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        write_sql_text(&path, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
        assert_eq!(entries_of(dir.path()), ["a.sql"]);

        // A read-only file is refused, as a plain write refuses it.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(write_sql_text(&path, "newer").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(entries_of(dir.path()), ["a.sql"]);
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_folder_leaves_the_original_untouched_and_no_temp_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let path = locked.join("a.sql");
        std::fs::write(&path, "keep me").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let restore = || {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        // Running as root ignores the mode: nothing to prove then.
        if std::fs::File::create(locked.join("probe")).is_ok() {
            restore();
            return;
        }
        let result = write_sql_text(&path, "replacement");
        let names = entries_of(&locked);
        restore();
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep me");
        assert_eq!(names, ["a.sql"]);
    }

    #[test]
    fn a_write_that_fails_halfway_does_not_truncate_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.sql");
        std::fs::write(&path, "original").unwrap();
        // A folder where the temporary file goes: creating it fails.
        std::fs::create_dir(dir.path().join(".a.sql.tabler-tmp")).unwrap();
        assert!(write_sql_text(&path, "replacement").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        // The same for a gzip file.
        let gz = dir.path().join("b.sql.gz");
        write_sql_text(&gz, "zipped").unwrap();
        std::fs::create_dir(dir.path().join(".b.sql.gz.tabler-tmp")).unwrap();
        assert!(write_sql_text(&gz, "replacement").is_err());
        assert_eq!(read_sql_text(&gz).unwrap(), "zipped");
    }

    #[test]
    fn a_file_name_at_the_file_system_limit_can_still_be_replaced() {
        // 255 bytes with `.sql`: the temporary file's name must not outgrow the limit.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{}.sql", "a".repeat(251)));
        write_sql_text(&path, "first").unwrap();
        write_sql_text(&path, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(entries_of(dir.path()).len(), 1);
        // A multi-byte name is cut on a character boundary.
        let path = dir.path().join(format!("{}.sql", "é".repeat(125)));
        write_sql_text(&path, "x").unwrap();
        write_sql_text(&path, "y").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "y");
        assert_eq!(entries_of(dir.path()).len(), 2);
    }

    #[test]
    fn a_directory_is_not_overwritten_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("folder.sql");
        std::fs::create_dir(&target).unwrap();
        assert!(write_sql_text(&target, "x").is_err());
        assert_eq!(entries_of(dir.path()), ["folder.sql"]);
    }

    #[cfg(unix)]
    #[test]
    fn writing_through_a_symlink_replaces_its_target_and_keeps_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.sql");
        let link = dir.path().join("link.sql");
        std::fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_sql_text(&link, "through the link").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "through the link");
        assert_eq!(entries_of(dir.path()), ["link.sql", "real.sql"]);
    }

    #[test]
    fn gzip_files_round_trip_through_the_atomic_write() {
        let dir = tempfile::tempdir().unwrap();
        let gz = dir.path().join("dump.sql.gz");
        write_sql_text(&gz, "select 1;\n").unwrap();
        write_sql_text(&gz, "select 2;\n").unwrap();
        assert_eq!(std::fs::read(&gz).unwrap()[..2], GZIP_MAGIC);
        assert_eq!(read_sql_text(&gz).unwrap(), "select 2;\n");
        assert_eq!(entries_of(dir.path()), ["dump.sql.gz"]);
    }

    // ---- the one-time migration is idempotent and survives a bad query -----------------------

    fn legacy(name: &str, folder: Option<Uuid>) -> SavedQuery {
        SavedQuery {
            id: Uuid::new_v4(),
            name: name.into(),
            keyword: None,
            sql: format!("select '{name}';"),
            folder_id: folder,
            profile_id: None,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
        }
    }

    fn legacy_folder(name: &str, parent: Option<Uuid>) -> QueryFolder {
        QueryFolder {
            id: Uuid::new_v4(),
            name: name.into(),
            parent_id: parent,
            created_at: "2026-01-01T00:00:00.000Z".into(),
        }
    }

    fn library() -> SavedQueryLibrary {
        let reports = legacy_folder("Reports", None);
        let inner = legacy_folder("Inner", Some(reports.id));
        SavedQueryLibrary {
            queries: vec![
                legacy("Top", None),
                legacy("Mid", Some(reports.id)),
                legacy("Deep", Some(inner.id)),
            ],
            folders: vec![reports, inner],
        }
    }

    fn names_in(root: &Path) -> (Vec<String>, Vec<String>) {
        let lib = list(root).unwrap();
        let mut folders: Vec<_> = lib.folders.into_iter().map(|f| f.name).collect();
        let mut queries: Vec<_> = lib.queries.into_iter().map(|q| q.name).collect();
        folders.sort();
        queries.sort();
        (folders, queries)
    }

    #[test]
    fn migrating_twice_does_not_duplicate_files_or_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Queries");
        let lib = library();
        assert_eq!(migrate_library(&root, &lib).unwrap(), 3);
        let first = names_in(&root);
        assert_eq!(first.0, ["Inner", "Reports"]);
        assert_eq!(first.1, ["Deep", "Mid", "Top"]);

        // Again over the same folder (a retry after a failed first run): nothing new.
        assert_eq!(migrate_library(&root, &lib).unwrap(), 0);
        assert_eq!(names_in(&root), first);
        assert_eq!(entries_of(&root), ["Reports", "Top.sql"]);
        assert_eq!(entries_of(&root.join("Reports")), ["Inner", "Mid.sql"]);
    }

    #[test]
    fn a_migration_that_stopped_halfway_is_completed_not_repeated() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Queries");
        let lib = library();
        // The first run got as far as the first two queries.
        let partial = SavedQueryLibrary {
            folders: lib.folders.clone(),
            queries: lib.queries[..2].to_vec(),
        };
        assert_eq!(migrate_library(&root, &partial).unwrap(), 2);
        assert_eq!(migrate_library(&root, &lib).unwrap(), 1);
        let (folders, queries) = names_in(&root);
        assert_eq!(folders, ["Inner", "Reports"]);
        assert_eq!(queries, ["Deep", "Mid", "Top"]);
    }

    #[test]
    fn an_over_long_name_is_shortened_instead_of_failing_the_migration() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Queries");
        // 150 emoji: 600 bytes, more than a file name can be.
        let long = "\u{1F980}".repeat(150);
        let lib = SavedQueryLibrary {
            folders: Vec::new(),
            queries: vec![legacy("Short", None), legacy(&long, None)],
        };
        assert_eq!(migrate_library(&root, &lib).unwrap(), 2);
        let listed = list(&root).unwrap().queries;
        assert_eq!(listed.len(), 2);
        let long_id = lib.queries[1].id;
        let found = listed.iter().find(|q| q.id == long_id).unwrap();
        assert!(found.name.chars().count() <= 100, "{}", found.name);
        assert!(found.sql.contains(&long));
        // And again: not twice.
        assert_eq!(migrate_library(&root, &lib).unwrap(), 0);
        assert_eq!(list(&root).unwrap().queries.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn a_query_that_cannot_be_written_is_skipped_and_the_migration_completes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Queries");
        let locked = root.join("Locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let restore = || {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        // Running as root ignores the mode: nothing to prove then.
        if std::fs::File::create(locked.join("probe")).is_ok() {
            restore();
            return;
        }
        let db = crate::appdb::AppDb::open_in_memory().unwrap();
        let folder_in_db = db.save_query_folder(&folder("Locked", None)).unwrap();
        for (name, folder) in [
            ("Alpha", None),
            ("Stuck", Some(folder_in_db.id)),
            ("Omega", None),
        ] {
            db.save_saved_query(&input(name, &format!("select '{name}';"), folder))
                .unwrap();
        }

        let result = migrate_from_appdb(&db, &root);
        restore();
        result.unwrap();
        // The others are there, the one that failed is not, and the migration counts as done: its
        // row stays in the app database as the backup.
        assert_eq!(names_in(&root).1, ["Alpha", "Omega"]);
        assert!(db.saved_queries_migrated().unwrap());
        assert_eq!(db.list_saved_queries().unwrap().queries.len(), 3);
        // Nothing more happens on the next listing: no duplicates.
        migrate_from_appdb(&db, &root).unwrap();
        assert_eq!(names_in(&root).1, ["Alpha", "Omega"]);
    }
}
