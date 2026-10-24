//! The app's own SQLite database: connection profiles, settings, query history, trusted SSH hosts
//! and (dev-only) secrets. Lives in the Tauri app data dir.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::error::AppError;
use crate::types::history::{HistoryEntry, HistoryFilter, NewHistoryEntry};
use crate::types::profile::ConnectionProfile;
use crate::types::saved_query::{
    QueryFolder, QueryFolderInput, SavedQuery, SavedQueryInput, SavedQueryLibrary,
};

#[derive(Clone)]
pub struct AppDb {
    conn: Arc<Mutex<Connection>>,
}

const MIGRATIONS: &[&str] = &[
    // v1
    "CREATE TABLE profiles (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        driver TEXT NOT NULL,
        data TEXT NOT NULL,
        sort_order INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        last_connected_at TEXT
    );
    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE secrets (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE history (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        profile_id TEXT,
        connection_name TEXT NOT NULL,
        database TEXT,
        sql TEXT NOT NULL,
        started_at TEXT NOT NULL,
        duration_ms INTEGER,
        row_count INTEGER,
        affected INTEGER,
        success INTEGER NOT NULL,
        error TEXT,
        source TEXT NOT NULL DEFAULT 'editor'
    );
    CREATE INDEX history_started_at ON history(started_at DESC);
    CREATE TABLE ssh_trusted_hosts (
        host TEXT NOT NULL,
        port INTEGER NOT NULL,
        key_type TEXT NOT NULL,
        public_key TEXT NOT NULL,
        added_at TEXT NOT NULL,
        PRIMARY KEY (host, port, key_type)
    );",
    // v2: saved queries (the sidebar's Queries pane) and their folders.
    "CREATE TABLE query_folders (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        parent_id TEXT REFERENCES query_folders(id) ON DELETE CASCADE,
        created_at TEXT NOT NULL
    );
    CREATE TABLE saved_queries (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        keyword TEXT,
        sql TEXT NOT NULL,
        folder_id TEXT REFERENCES query_folders(id) ON DELETE CASCADE,
        profile_id TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    CREATE INDEX saved_queries_folder ON saved_queries(folder_id);",
    // v3: Metrics Board boards (`crate::metrics`); `profile_id` NULL = every connection.
    "CREATE TABLE IF NOT EXISTS metrics_boards (
        id TEXT PRIMARY KEY,
        profile_id TEXT,
        name TEXT NOT NULL,
        objects TEXT NOT NULL,
        sort_order INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS metrics_boards_profile ON metrics_boards(profile_id);",
];

/// Settings key set once the app DB's saved queries were written out as `.sql` files.
const QUERIES_MIGRATED_KEY: &str = "queries.migratedToFiles";

/// The Match Case / Whole Word / Regex matcher of a history search.
fn history_matcher(term: &str, filter: &HistoryFilter) -> Result<regex::Regex, AppError> {
    let body = if filter.regex {
        term.to_string()
    } else {
        regex::escape(term)
    };
    let body = if filter.whole_word {
        format!(r"\b(?:{body})\b")
    } else {
        body
    };
    regex::RegexBuilder::new(&body)
        .case_insensitive(!filter.match_case)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| AppError::InvalidInput(format!("invalid regular expression: {e}")))
}

/// History rows kept; older ones are pruned.
const HISTORY_KEEP: i64 = 10_000;
const HISTORY_DEFAULT_LIMIT: u32 = 200;
const HISTORY_MAX_LIMIT: u32 = 1_000;

fn clamp_i64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Longest saved query / folder name accepted.
const SAVED_NAME_MAX: usize = 200;
/// Longest binding keyword accepted.
const KEYWORD_MAX: usize = 64;

/// A trimmed, non-empty name of at most `SAVED_NAME_MAX` characters.
fn saved_name(name: &str, what: &str) -> Result<String, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput(format!("{what} name is required")));
    }
    if name.chars().count() > SAVED_NAME_MAX {
        return Err(AppError::InvalidInput(format!(
            "{what} name is longer than {SAVED_NAME_MAX} characters"
        )));
    }
    Ok(name.to_string())
}

/// The binding keyword: trimmed, `None` when blank, one word (no whitespace).
fn saved_keyword(keyword: Option<&str>) -> Result<Option<String>, AppError> {
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

/// A stored profile. A row that does not decode was saved by a newer Tabler (a driver or an
/// option value this build does not know): `Unsupported`, and the row stays as it is.
fn decode_profile(name: &str, data: &str) -> Result<ConnectionProfile, AppError> {
    serde_json::from_str(data).map_err(|e| {
        AppError::Unsupported(format!(
            "the connection {name:?} was saved by a newer Tabler ({e})"
        ))
    })
}

fn parse_uuid(s: Option<String>) -> Option<Uuid> {
    s.and_then(|s| Uuid::parse_str(&s).ok())
}

fn saved_query_row(r: &rusqlite::Row<'_>) -> Result<SavedQuery, rusqlite::Error> {
    let id: String = r.get(0)?;
    Ok(SavedQuery {
        id: Uuid::parse_str(&id).unwrap_or_default(),
        name: r.get(1)?,
        keyword: r.get(2)?,
        sql: r.get(3)?,
        folder_id: parse_uuid(r.get(4)?),
        profile_id: parse_uuid(r.get(5)?),
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

fn query_folder_row(r: &rusqlite::Row<'_>) -> Result<QueryFolder, rusqlite::Error> {
    let id: String = r.get(0)?;
    Ok(QueryFolder {
        id: Uuid::parse_str(&id).unwrap_or_default(),
        name: r.get(1)?,
        parent_id: parse_uuid(r.get(2)?),
        created_at: r.get(3)?,
    })
}

const SAVED_QUERY_COLUMNS: &str =
    "id, name, keyword, sql, folder_id, profile_id, created_at, updated_at";

/// The folder subtree rooted at `?1` (the folder and every folder inside it).
const FOLDER_SUBTREE: &str = "WITH RECURSIVE sub(id) AS (
        SELECT ?1
        UNION SELECT f.id FROM query_folders f JOIN sub ON f.parent_id = sub.id
    )";

fn folder_exists(conn: &Connection, id: Uuid) -> Result<bool, rusqlite::Error> {
    conn.query_row(
        "SELECT 1 FROM query_folders WHERE id = ?1",
        params![id.to_string()],
        |_| Ok(()),
    )
    .optional()
    .map(|r| r.is_some())
}

impl AppDb {
    pub fn open(path: &Path) -> Result<AppDb, AppError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).map_err(|e| AppError::AppDb(e.to_string()))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;",
        )
        .map_err(|e| AppError::AppDb(e.to_string()))?;
        let db = AppDb {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<AppDb, AppError> {
        let conn = Connection::open_in_memory().map_err(|e| AppError::AppDb(e.to_string()))?;
        let db = AppDb {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<(), AppError> {
        let conn = self.lock();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let target = (i + 1) as i64;
            if version < target {
                conn.execute_batch(&format!(
                    "BEGIN; {sql} PRAGMA user_version = {target}; COMMIT;"
                ))
                .map_err(|e| AppError::AppDb(format!("migration {target}: {e}")))?;
            }
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ----- profiles ---------------------------------------------------------

    /// The saved profiles, in list order. A row this build cannot read (see `decode_profile`)
    /// is logged and left out, never changed: one such row must not break the launcher, Open
    /// Recent, Export Connections or the MCP tools.
    pub fn list_profiles(&self) -> Result<Vec<ConnectionProfile>, AppError> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT id, name, data FROM profiles ORDER BY sort_order, lower(name)")
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, data) = row.map_err(|e| AppError::AppDb(e.to_string()))?;
            match decode_profile(&name, &data) {
                Ok(profile) => out.push(profile),
                Err(e) => log::warn!("profile {id} left out of the list: {e}"),
            }
        }
        Ok(out)
    }

    /// One saved profile; `Unsupported` for a row this build cannot read.
    pub fn get_profile(&self, id: Uuid) -> Result<Option<ConnectionProfile>, AppError> {
        let conn = self.lock();
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT name, data FROM profiles WHERE id = ?1",
                params![id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        row.map(|(name, data)| decode_profile(&name, &data))
            .transpose()
    }

    /// Inserts or updates a profile, filling timestamps. Returns the stored profile. Only the
    /// profile's own row changes; a row this build cannot read is never overwritten.
    pub fn upsert_profile(
        &self,
        profile: &ConnectionProfile,
    ) -> Result<ConnectionProfile, AppError> {
        let mut p = profile.clone();
        let now = now_rfc3339();
        let existing = self.get_profile(p.id)?;
        match &existing {
            Some(old) => {
                p.created_at = old.created_at.clone();
                p.last_connected_at = old.last_connected_at.clone();
                if p.sort_order == 0 {
                    p.sort_order = old.sort_order;
                }
                // Option keys of a newer build: the dialog does not know them and saves without
                // them, so the stored ones are kept.
                for (key, value) in &old.options.unknown {
                    if !p.options.unknown.contains_key(key) {
                        p.options.unknown.insert(key.clone(), value.clone());
                    }
                }
            }
            None => {
                p.created_at = now.clone();
                let conn = self.lock();
                let max: Option<i64> = conn
                    .query_row("SELECT max(sort_order) FROM profiles", [], |r| r.get(0))
                    .map_err(|e| AppError::AppDb(e.to_string()))?;
                p.sort_order = (max.unwrap_or(0) + 1) as i32;
            }
        }
        p.updated_at = now;
        let data = serde_json::to_string(&p)?;
        let conn = self.lock();
        conn.execute(
            "INSERT INTO profiles (id, name, driver, data, sort_order, created_at, updated_at, last_connected_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, driver = excluded.driver, data = excluded.data,
               sort_order = excluded.sort_order, updated_at = excluded.updated_at",
            params![
                p.id.to_string(),
                p.name,
                serde_json::to_string(&p.driver)?.trim_matches('"'),
                data,
                p.sort_order,
                p.created_at,
                p.updated_at,
                p.last_connected_at
            ],
        )
        .map_err(|e| AppError::AppDb(e.to_string()))?;
        Ok(p)
    }

    pub fn delete_profile(&self, id: Uuid) -> Result<bool, AppError> {
        let mut conn = self.lock();
        let db = |e: rusqlite::Error| AppError::AppDb(e.to_string());
        let tx = conn.transaction().map_err(db)?;
        let n = tx
            .execute(
                "DELETE FROM profiles WHERE id = ?1",
                params![id.to_string()],
            )
            .map_err(db)?;
        // Its saved queries are kept, for every connection.
        tx.execute(
            "UPDATE saved_queries SET profile_id = NULL WHERE profile_id = ?1",
            params![id.to_string()],
        )
        .map_err(db)?;
        // Its Metrics boards go with it: a board holds SQL written for that connection's schema
        // (never shown elsewhere), and without the profile nothing could list or delete it
        // (also the case for a URL-opened temporary connection that is purged).
        tx.execute(
            "DELETE FROM metrics_boards WHERE profile_id = ?1",
            params![id.to_string()],
        )
        .map_err(db)?;
        tx.commit().map_err(db)?;
        Ok(n > 0)
    }

    pub fn reorder_profiles(&self, ids: &[Uuid]) -> Result<(), AppError> {
        let mut conn = self.lock();
        let tx = conn
            .transaction()
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        for (i, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE profiles SET sort_order = ?1, data = json_set(data, '$.sortOrder', ?1) WHERE id = ?2",
                params![i as i64 + 1, id.to_string()],
            )
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        }
        tx.commit().map_err(|e| AppError::AppDb(e.to_string()))
    }

    pub fn touch_last_connected(&self, id: Uuid) -> Result<(), AppError> {
        let now = now_rfc3339();
        let conn = self.lock();
        conn.execute(
            "UPDATE profiles SET last_connected_at = ?1, data = json_set(data, '$.lastConnectedAt', ?1) WHERE id = ?2",
            params![now, id.to_string()],
        )
        .map_err(|e| AppError::AppDb(e.to_string()))?;
        Ok(())
    }

    // ----- settings ---------------------------------------------------------

    pub fn get_settings(&self) -> Result<serde_json::Map<String, serde_json::Value>, AppError> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT key, value FROM settings")
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        let mut map = serde_json::Map::new();
        for row in rows {
            let (k, v) = row.map_err(|e| AppError::AppDb(e.to_string()))?;
            map.insert(
                k,
                serde_json::from_str(&v).unwrap_or(serde_json::Value::Null),
            );
        }
        Ok(map)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<serde_json::Value>, AppError> {
        let conn = self.lock();
        let v: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        Ok(v.and_then(|s| serde_json::from_str(&s).ok()))
    }

    pub fn set_setting(&self, key: &str, value: &serde_json::Value) -> Result<(), AppError> {
        let conn = self.lock();
        if value.is_null() {
            conn.execute("DELETE FROM settings WHERE key = ?1", params![key])
                .map_err(|e| AppError::AppDb(e.to_string()))?;
        } else {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, serde_json::to_string(value)?],
            )
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        }
        Ok(())
    }

    // ----- dev-only plaintext secrets ---------------------------------------

    pub fn get_secret(&self, key: &str) -> Result<Option<String>, AppError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT value FROM secrets WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| AppError::AppDb(e.to_string()))
    }

    pub fn set_secret(&self, key: &str, value: &str) -> Result<(), AppError> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO secrets (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(|e| AppError::AppDb(e.to_string()))?;
        Ok(())
    }

    pub fn delete_secret(&self, key: &str) -> Result<(), AppError> {
        let conn = self.lock();
        conn.execute("DELETE FROM secrets WHERE key = ?1", params![key])
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        Ok(())
    }

    // ----- query history ----------------------------------------------------

    /// Stores history rows (one per executed statement) and returns their ids. Old rows beyond
    /// `HISTORY_KEEP` are pruned now and then.
    pub fn insert_history(&self, entries: &[NewHistoryEntry]) -> Result<Vec<i64>, AppError> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.lock();
        let db = |e: rusqlite::Error| AppError::AppDb(e.to_string());
        let tx = conn.transaction().map_err(db)?;
        let mut ids = Vec::with_capacity(entries.len());
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO history (profile_id, connection_name, database, sql, started_at,
                        duration_ms, row_count, affected, success, error, source)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                )
                .map_err(db)?;
            for e in entries {
                stmt.execute(params![
                    e.profile_id.map(|id| id.to_string()),
                    e.connection_name,
                    e.database,
                    e.sql,
                    e.started_at,
                    e.duration_ms.map(clamp_i64),
                    e.row_count.map(clamp_i64),
                    e.affected.map(clamp_i64),
                    e.success,
                    e.error,
                    e.source,
                ])
                .map_err(db)?;
                ids.push(tx.last_insert_rowid());
            }
        }
        if ids.iter().any(|id| id % 100 == 0) {
            tx.execute(
                "DELETE FROM history WHERE id <=
                   (SELECT id FROM history ORDER BY id DESC LIMIT 1 OFFSET ?1)",
                params![HISTORY_KEEP],
            )
            .map_err(db)?;
        }
        tx.commit().map_err(db)?;
        Ok(ids)
    }

    /// History rows, newest first. `search` is a case-insensitive substring match on the SQL;
    /// Match Case / Whole Word / Regex refine it (checked in Rust after a LIKE pre-filter, which
    /// is exact for a plain search and a superset otherwise; a regex scans every row).
    pub fn list_history(&self, filter: &HistoryFilter) -> Result<Vec<HistoryEntry>, AppError> {
        let limit = filter
            .limit
            .unwrap_or(HISTORY_DEFAULT_LIMIT)
            .clamp(1, HISTORY_MAX_LIMIT);
        let term = filter.search.as_deref().filter(|s| !s.is_empty());
        let matcher = match term {
            Some(t) if filter.match_case || filter.whole_word || filter.regex => {
                Some(history_matcher(t, filter)?)
            }
            _ => None,
        };
        let pattern = term.filter(|_| !filter.regex).map(|s| {
            let escaped = s
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            format!("%{escaped}%")
        });
        let conn = self.lock();
        let db = |e: rusqlite::Error| AppError::AppDb(e.to_string());
        let mut stmt = conn
            .prepare(
                "SELECT id, profile_id, connection_name, database, sql, started_at, duration_ms,
                        row_count, affected, success, error, source
                 FROM history
                 WHERE (?1 IS NULL OR profile_id = ?1)
                   AND (?2 IS NULL OR sql LIKE ?2 ESCAPE '\\')
                   AND (?3 IS NULL OR id < ?3)
                 ORDER BY id DESC",
            )
            .map_err(db)?;
        let rows = stmt
            .query_map(
                params![
                    filter.profile_id.map(|id| id.to_string()),
                    pattern,
                    filter.before_id
                ],
                |r| {
                    let profile_id: Option<String> = r.get(1)?;
                    let opt_u64 = |v: Option<i64>| v.map(|n| n.max(0) as u64);
                    Ok(HistoryEntry {
                        id: r.get(0)?,
                        profile_id: profile_id.and_then(|p| Uuid::parse_str(&p).ok()),
                        connection_name: r.get(2)?,
                        database: r.get(3)?,
                        sql: r.get(4)?,
                        started_at: r.get(5)?,
                        duration_ms: opt_u64(r.get(6)?),
                        row_count: opt_u64(r.get(7)?),
                        affected: opt_u64(r.get(8)?),
                        success: r.get(9)?,
                        error: r.get(10)?,
                        source: r.get(11)?,
                    })
                },
            )
            .map_err(db)?;
        let mut out = Vec::new();
        for row in rows {
            let entry = row.map_err(db)?;
            if matcher.as_ref().is_none_or(|m| m.is_match(&entry.sql)) {
                out.push(entry);
                if out.len() >= limit as usize {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Deletes the history older than `before` (RFC 3339), of one profile or of all: "Clear
    /// History Older Than…" and the "Keep history for" pruning. Returns how many rows went.
    pub fn clear_history_before(
        &self,
        before: &str,
        profile_id: Option<Uuid>,
    ) -> Result<u64, AppError> {
        let cutoff = chrono::DateTime::parse_from_rfc3339(before)
            .map_err(|e| AppError::InvalidInput(format!("invalid date {before:?}: {e}")))?
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let conn = self.lock();
        let n = conn
            .execute(
                "DELETE FROM history WHERE started_at < ?1 AND (?2 IS NULL OR profile_id = ?2)",
                params![cutoff, profile_id.map(|id| id.to_string())],
            )
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        Ok(n as u64)
    }

    /// The saved queries of the app DB were written to the queries folder (they are files now).
    pub fn saved_queries_migrated(&self) -> Result<bool, AppError> {
        Ok(self.get_setting(QUERIES_MIGRATED_KEY)?.is_some())
    }

    pub fn mark_saved_queries_migrated(&self) -> Result<(), AppError> {
        self.set_setting(QUERIES_MIGRATED_KEY, &serde_json::Value::Bool(true))
    }

    /// Deletes the given history rows; returns how many existed.
    pub fn delete_history(&self, ids: &[i64]) -> Result<u64, AppError> {
        let mut conn = self.lock();
        let db = |e: rusqlite::Error| AppError::AppDb(e.to_string());
        let tx = conn.transaction().map_err(db)?;
        let mut n = 0u64;
        {
            let mut stmt = tx
                .prepare("DELETE FROM history WHERE id = ?1")
                .map_err(db)?;
            for id in ids {
                n += stmt.execute(params![id]).map_err(db)? as u64;
            }
        }
        tx.commit().map_err(db)?;
        Ok(n)
    }

    /// Deletes all history, or only one profile's.
    pub fn clear_history(&self, profile_id: Option<Uuid>) -> Result<u64, AppError> {
        let conn = self.lock();
        let n = match profile_id {
            Some(id) => conn.execute(
                "DELETE FROM history WHERE profile_id = ?1",
                params![id.to_string()],
            ),
            None => conn.execute("DELETE FROM history", []),
        }
        .map_err(|e| AppError::AppDb(e.to_string()))?;
        Ok(n as u64)
    }

    // ----- saved queries ----------------------------------------------------

    /// Every folder and saved query, each sorted by name (case-insensitive).
    pub fn list_saved_queries(&self) -> Result<SavedQueryLibrary, AppError> {
        let conn = self.lock();
        let db = |e: rusqlite::Error| AppError::AppDb(e.to_string());
        let folders = conn
            .prepare(
                "SELECT id, name, parent_id, created_at FROM query_folders
                 ORDER BY lower(name), created_at",
            )
            .map_err(db)?
            .query_map([], query_folder_row)
            .map_err(db)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db)?;
        let queries = conn
            .prepare(&format!(
                "SELECT {SAVED_QUERY_COLUMNS} FROM saved_queries ORDER BY lower(name), created_at"
            ))
            .map_err(db)?
            .query_map([], saved_query_row)
            .map_err(db)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db)?;
        Ok(SavedQueryLibrary { folders, queries })
    }

    /// Creates (`input.id` = `None`, or an id not stored yet) or replaces a saved query. The name
    /// is required, the keyword is one word (blank = none) and the folder must exist.
    pub fn save_saved_query(&self, input: &SavedQueryInput) -> Result<SavedQuery, AppError> {
        let name = saved_name(&input.name, "Query")?;
        let keyword = saved_keyword(input.keyword.as_deref())?;
        let id = input.id.unwrap_or_else(Uuid::new_v4);
        let now = now_rfc3339();
        let conn = self.lock();
        let db = |e: rusqlite::Error| AppError::AppDb(e.to_string());
        if let Some(folder) = input.folder_id
            && !folder_exists(&conn, folder).map_err(db)?
        {
            return Err(AppError::NotFound("the folder no longer exists".into()));
        }
        conn.execute(
            "INSERT INTO saved_queries (id, name, keyword, sql, folder_id, profile_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, keyword = excluded.keyword,
               sql = excluded.sql, folder_id = excluded.folder_id, profile_id = excluded.profile_id,
               updated_at = excluded.updated_at",
            params![
                id.to_string(),
                name,
                keyword,
                input.sql,
                input.folder_id.map(|f| f.to_string()),
                input.profile_id.map(|p| p.to_string()),
                now,
            ],
        )
        .map_err(db)?;
        conn.query_row(
            &format!("SELECT {SAVED_QUERY_COLUMNS} FROM saved_queries WHERE id = ?1"),
            params![id.to_string()],
            saved_query_row,
        )
        .map_err(db)
    }

    /// Deletes a saved query; false when it did not exist.
    pub fn delete_saved_query(&self, id: Uuid) -> Result<bool, AppError> {
        let conn = self.lock();
        let n = conn
            .execute(
                "DELETE FROM saved_queries WHERE id = ?1",
                params![id.to_string()],
            )
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        Ok(n > 0)
    }

    /// Creates or renames / moves a folder. The parent must exist and must not be the folder
    /// itself or one inside it.
    pub fn save_query_folder(&self, input: &QueryFolderInput) -> Result<QueryFolder, AppError> {
        let name = saved_name(&input.name, "Folder")?;
        let id = input.id.unwrap_or_else(Uuid::new_v4);
        let conn = self.lock();
        let db = |e: rusqlite::Error| AppError::AppDb(e.to_string());
        if let Some(parent) = input.parent_id {
            if !folder_exists(&conn, parent).map_err(db)? {
                return Err(AppError::NotFound(
                    "the parent folder no longer exists".into(),
                ));
            }
            let inside: bool = conn
                .query_row(
                    &format!("{FOLDER_SUBTREE} SELECT EXISTS (SELECT 1 FROM sub WHERE id = ?2)"),
                    params![id.to_string(), parent.to_string()],
                    |r| r.get(0),
                )
                .map_err(db)?;
            if inside {
                return Err(AppError::InvalidInput(
                    "a folder cannot be moved into itself".into(),
                ));
            }
        }
        conn.execute(
            "INSERT INTO query_folders (id, name, parent_id, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, parent_id = excluded.parent_id",
            params![
                id.to_string(),
                name,
                input.parent_id.map(|p| p.to_string()),
                now_rfc3339()
            ],
        )
        .map_err(db)?;
        conn.query_row(
            "SELECT id, name, parent_id, created_at FROM query_folders WHERE id = ?1",
            params![id.to_string()],
            query_folder_row,
        )
        .map_err(db)
    }

    /// Deletes a folder with the folders and queries inside it; returns how many queries went.
    pub fn delete_query_folder(&self, id: Uuid) -> Result<u64, AppError> {
        let mut conn = self.lock();
        let db = |e: rusqlite::Error| AppError::AppDb(e.to_string());
        let tx = conn.transaction().map_err(db)?;
        let queries = tx
            .execute(
                &format!(
                    "{FOLDER_SUBTREE} DELETE FROM saved_queries WHERE folder_id IN (SELECT id FROM sub)"
                ),
                params![id.to_string()],
            )
            .map_err(db)?;
        tx.execute(
            &format!("{FOLDER_SUBTREE} DELETE FROM query_folders WHERE id IN (SELECT id FROM sub)"),
            params![id.to_string()],
        )
        .map_err(db)?;
        tx.commit().map_err(db)?;
        Ok(queries as u64)
    }

    /// Runs an arbitrary closure with the raw connection (history, trusted hosts).
    pub fn with_conn<R>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<R, rusqlite::Error>,
    ) -> Result<R, AppError> {
        let mut conn = self.lock();
        f(&mut conn).map_err(|e| AppError::AppDb(e.to_string()))
    }
}

/// Profile rows as a newer Tabler writes them, for the tests of everything that lists profiles.
#[cfg(test)]
pub(crate) mod newer {
    use super::*;

    /// Stores a profile with a driver and an option this build does not know, `data` verbatim
    /// (sort order 0: first in the list). Returns its id and `data`.
    pub fn insert_profile(db: &AppDb, name: &str) -> (Uuid, String) {
        let id = Uuid::new_v4();
        let data = serde_json::json!({
            "id": id,
            "name": name,
            "driver": "futuredb",
            "host": "db.example.com",
            "options": { "tag": "staging", "futureOption": { "nested": [1, 2] } },
            "sortOrder": 0,
            "createdAt": "2030-01-01T00:00:00.000Z",
            "updatedAt": "2030-01-01T00:00:00.000Z",
        })
        .to_string();
        db.with_conn(|c| {
            c.execute(
                "INSERT INTO profiles (id, name, driver, data, sort_order, created_at, updated_at)
                 VALUES (?1, ?2, 'futuredb', ?3, 0, '2030', '2030')",
                params![id.to_string(), name, data],
            )
        })
        .unwrap();
        (id, data)
    }

    /// The `data` of a profile row, as stored.
    pub fn raw_data(db: &AppDb, id: Uuid) -> String {
        db.with_conn(|c| {
            c.query_row(
                "SELECT data FROM profiles WHERE id = ?1",
                params![id.to_string()],
                |r| r.get(0),
            )
        })
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::profile::{Driver, ProfileOptions, SslMode};

    fn profile(name: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: Uuid::new_v4(),
            name: name.into(),
            color: Some("blue".into()),
            group: None,
            driver: Driver::Sqlite,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl: SslMode::Prefer,
            ssl_ca_path: None,
            sqlite_path: Some("/tmp/x.sqlite".into()),
            ssh: None,
            options: ProfileOptions::default(),
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
            last_connected_at: None,
        }
    }

    #[test]
    fn migrates_and_round_trips_profiles() {
        let db = AppDb::open_in_memory().unwrap();
        let a = db.upsert_profile(&profile("b-second")).unwrap();
        let b = db.upsert_profile(&profile("a-first")).unwrap();
        assert!(!a.created_at.is_empty());
        assert_eq!(a.sort_order, 1);
        assert_eq!(b.sort_order, 2);
        let list = db.list_profiles().unwrap();
        assert_eq!(
            list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["b-second", "a-first"]
        );
        db.reorder_profiles(&[b.id, a.id]).unwrap();
        let list = db.list_profiles().unwrap();
        assert_eq!(list[0].id, b.id);
        assert_eq!(list[0].sort_order, 1);
        db.touch_last_connected(a.id).unwrap();
        assert!(
            db.get_profile(a.id)
                .unwrap()
                .unwrap()
                .last_connected_at
                .is_some()
        );
        assert!(db.delete_profile(a.id).unwrap());
        assert!(db.get_profile(a.id).unwrap().is_none());
    }

    /// A row a newer Tabler saved (a driver this build lacks): left out of the list, refused by
    /// id, and never rewritten by what this build does to the other profiles.
    #[test]
    fn a_profile_saved_by_a_newer_build_is_skipped_and_left_alone() {
        let db = AppDb::open_in_memory().unwrap();
        let a = db.upsert_profile(&profile("a")).unwrap();
        let (future, raw) = newer::insert_profile(&db, "Analytics");
        let b = db.upsert_profile(&profile("b")).unwrap();
        let names = |db: &AppDb| {
            db.list_profiles()
                .unwrap()
                .into_iter()
                .map(|p| p.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&db), ["a", "b"]);
        let err = db.get_profile(future).unwrap_err();
        assert_eq!(err.code(), "UNSUPPORTED");
        assert!(err.to_string().contains("\"Analytics\""), "{err}");
        assert!(err.to_string().contains("newer Tabler"), "{err}");

        // Whatever happens to the other profiles leaves the row byte for byte.
        let mut edited = a.clone();
        edited.name = "a2".into();
        db.upsert_profile(&edited).unwrap();
        db.upsert_profile(&profile("c")).unwrap();
        db.reorder_profiles(&[b.id, a.id]).unwrap();
        db.touch_last_connected(b.id).unwrap();
        assert!(db.delete_profile(b.id).unwrap());
        assert_eq!(names(&db), ["a2", "c"]);
        assert_eq!(newer::raw_data(&db, future), raw);

        // A save over it from this build is refused instead of dropping what it cannot read.
        let mut clash = profile("clash");
        clash.id = future;
        assert_eq!(db.upsert_profile(&clash).unwrap_err().code(), "UNSUPPORTED");
        assert_eq!(newer::raw_data(&db, future), raw);
    }

    /// Option keys of a newer build come along: they decode into `unknown`, are written back as
    /// they were, and survive a save from the dialog, which leaves them out.
    #[test]
    fn unknown_option_keys_survive_a_save_without_them() {
        let options: ProfileOptions =
            serde_json::from_value(serde_json::json!({ "readOnly": true, "futureOption": 1 }))
                .unwrap();
        assert!(options.read_only);
        assert_eq!(
            options.unknown.get("futureOption"),
            Some(&serde_json::json!(1))
        );
        let written = serde_json::to_value(&options).unwrap();
        assert_eq!(written["futureOption"], 1);
        assert_eq!(written["readOnly"], true);
        assert_eq!(
            serde_json::from_value::<ProfileOptions>(written).unwrap(),
            options
        );
        // Known keys never land in `unknown`.
        assert!(ProfileOptions::default().unknown.is_empty());
        let known: ProfileOptions =
            serde_json::from_value(serde_json::to_value(ProfileOptions::default()).unwrap())
                .unwrap();
        assert!(known.unknown.is_empty());

        let db = AppDb::open_in_memory().unwrap();
        let mut p = profile("lite");
        p.options = options;
        let stored = db.upsert_profile(&p).unwrap();
        // The dialog's save: the profile as the renderer has it, minus the key it does not know.
        let mut json = serde_json::to_value(&stored).unwrap();
        json["options"]
            .as_object_mut()
            .unwrap()
            .remove("futureOption");
        let mut from_dialog: ConnectionProfile = serde_json::from_value(json).unwrap();
        assert!(from_dialog.options.unknown.is_empty());
        from_dialog.options.read_only = false;
        let saved = db.upsert_profile(&from_dialog).unwrap();
        assert!(!saved.options.read_only);
        assert_eq!(
            saved.options.unknown.get("futureOption"),
            Some(&serde_json::json!(1))
        );
        assert_eq!(db.get_profile(p.id).unwrap().unwrap(), saved);
        assert!(newer_key_in(&db, p.id, 1));

        // A value that is sent wins over the stored one.
        let mut newer_value = saved.clone();
        newer_value
            .options
            .unknown
            .insert("futureOption".into(), serde_json::json!(2));
        db.upsert_profile(&newer_value).unwrap();
        assert!(newer_key_in(&db, p.id, 2));
    }

    /// The stored row's `options.futureOption` is `value`.
    fn newer_key_in(db: &AppDb, id: Uuid, value: i64) -> bool {
        let data: serde_json::Value = serde_json::from_str(&newer::raw_data(db, id)).unwrap();
        data["options"]["futureOption"] == value
    }

    fn entry(profile: Option<Uuid>, sql: &str, success: bool) -> NewHistoryEntry {
        NewHistoryEntry {
            profile_id: profile,
            connection_name: "local".into(),
            database: Some("app".into()),
            sql: sql.into(),
            started_at: now_rfc3339(),
            duration_ms: Some(3),
            row_count: Some(1),
            affected: None,
            success,
            error: (!success).then(|| "boom".to_string()),
            source: "editor".into(),
        }
    }

    #[test]
    fn history_search_options_and_pruning() {
        let db = AppDb::open_in_memory().unwrap();
        let p1 = Uuid::new_v4();
        db.insert_history(&[
            entry(Some(p1), "SELECT * FROM users", true),
            entry(Some(p1), "select * from users_archive", true),
            entry(None, "update Users set x = 1", true),
        ])
        .unwrap();
        let search = |term: &str, match_case: bool, whole_word: bool, regex: bool| {
            db.list_history(&HistoryFilter {
                search: Some(term.into()),
                match_case,
                whole_word,
                regex,
                ..HistoryFilter::default()
            })
            .map(|rows| rows.into_iter().map(|r| r.sql).collect::<Vec<_>>())
        };
        assert_eq!(search("users", false, false, false).unwrap().len(), 3);
        assert_eq!(
            search("Users", true, false, false).unwrap(),
            ["update Users set x = 1"]
        );
        assert_eq!(search("users", false, true, false).unwrap().len(), 2);
        assert_eq!(
            search("users", true, true, false).unwrap(),
            ["SELECT * FROM users"]
        );
        assert_eq!(
            search(r"^select .* users_\w+$", false, false, true).unwrap(),
            ["select * from users_archive"]
        );
        assert!(matches!(
            search("(", false, false, true),
            Err(AppError::InvalidInput(_))
        ));
        // A limit counts matches, not scanned rows.
        let one = db
            .list_history(&HistoryFilter {
                search: Some("users".into()),
                whole_word: true,
                limit: Some(1),
                ..HistoryFilter::default()
            })
            .unwrap();
        assert_eq!(one.len(), 1);

        // Clear older than a date (all, or one profile's).
        let old = NewHistoryEntry {
            started_at: "2020-01-01T00:00:00.000Z".into(),
            ..entry(Some(p1), "select 'old'", true)
        };
        let older = NewHistoryEntry {
            started_at: "2019-06-01T00:00:00.000Z".into(),
            ..entry(None, "select 'older'", true)
        };
        db.insert_history(&[old, older]).unwrap();
        assert_eq!(
            db.clear_history_before("2020-06-01T00:00:00+02:00", Some(p1))
                .unwrap(),
            1
        );
        assert_eq!(
            db.clear_history_before("2020-06-01T00:00:00Z", None)
                .unwrap(),
            1
        );
        assert_eq!(db.list_history(&HistoryFilter::default()).unwrap().len(), 3);
        assert!(db.clear_history_before("yesterday", None).is_err());

        assert!(!db.saved_queries_migrated().unwrap());
        db.mark_saved_queries_migrated().unwrap();
        assert!(db.saved_queries_migrated().unwrap());
    }

    #[test]
    fn history_crud() {
        let db = AppDb::open_in_memory().unwrap();
        let p1 = Uuid::new_v4();
        let p2 = Uuid::new_v4();
        let ids = db
            .insert_history(&[
                entry(Some(p1), "select 1", true),
                entry(Some(p1), "select 100% _x", true),
                entry(Some(p2), "update users set a = 1", false),
                entry(None, "SELECT name FROM users", true),
            ])
            .unwrap();
        assert_eq!(ids.len(), 4);
        assert!(ids.windows(2).all(|w| w[0] < w[1]));

        // Newest first, default limit.
        let all = db.list_history(&HistoryFilter::default()).unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].sql, "SELECT name FROM users");
        assert_eq!(all[3].sql, "select 1");
        assert_eq!(all[1].profile_id, Some(p2));
        assert!(!all[1].success);
        assert_eq!(all[1].error.as_deref(), Some("boom"));
        assert_eq!(all[3].duration_ms, Some(3));
        assert_eq!(all[3].database.as_deref(), Some("app"));

        // Profile filter.
        let p1_rows = db
            .list_history(&HistoryFilter {
                profile_id: Some(p1),
                ..HistoryFilter::default()
            })
            .unwrap();
        assert_eq!(p1_rows.len(), 2);

        // Case-insensitive search; LIKE wildcards in the search text are literal.
        let found = db
            .list_history(&HistoryFilter {
                search: Some("users".into()),
                ..HistoryFilter::default()
            })
            .unwrap();
        assert_eq!(found.len(), 2);
        let found = db
            .list_history(&HistoryFilter {
                search: Some("NAME FROM".into()),
                ..HistoryFilter::default()
            })
            .unwrap();
        assert_eq!(found.len(), 1);
        let found = db
            .list_history(&HistoryFilter {
                search: Some("100%".into()),
                ..HistoryFilter::default()
            })
            .unwrap();
        assert_eq!(found.len(), 1);
        let found = db
            .list_history(&HistoryFilter {
                search: Some("%".into()),
                ..HistoryFilter::default()
            })
            .unwrap();
        assert_eq!(found.len(), 1);

        // Paging with before_id + limit.
        let page1 = db
            .list_history(&HistoryFilter {
                limit: Some(2),
                ..HistoryFilter::default()
            })
            .unwrap();
        assert_eq!(page1.len(), 2);
        let page2 = db
            .list_history(&HistoryFilter {
                limit: Some(2),
                before_id: Some(page1[1].id),
                ..HistoryFilter::default()
            })
            .unwrap();
        assert_eq!(page2.len(), 2);
        assert!(page2[0].id < page1[1].id);

        // Delete and clear.
        assert_eq!(db.delete_history(&[ids[0], 999_999]).unwrap(), 1);
        assert_eq!(db.list_history(&HistoryFilter::default()).unwrap().len(), 3);
        assert_eq!(db.clear_history(Some(p2)).unwrap(), 1);
        assert_eq!(db.list_history(&HistoryFilter::default()).unwrap().len(), 2);
        assert_eq!(db.clear_history(None).unwrap(), 2);
        assert!(
            db.list_history(&HistoryFilter::default())
                .unwrap()
                .is_empty()
        );
        assert!(db.insert_history(&[]).unwrap().is_empty());
    }

    fn saved(name: &str, folder: Option<Uuid>, profile: Option<Uuid>) -> SavedQueryInput {
        SavedQueryInput {
            id: None,
            name: name.into(),
            keyword: None,
            sql: format!("select '{name}'"),
            folder_id: folder,
            profile_id: profile,
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
    fn saved_queries_crud() {
        let db = AppDb::open_in_memory().unwrap();
        assert_eq!(
            db.list_saved_queries().unwrap(),
            SavedQueryLibrary::default()
        );

        let reports = db.save_query_folder(&folder("Reports", None)).unwrap();
        let p = Uuid::new_v4();
        let mut input = saved("  users by day ", Some(reports.id), Some(p));
        input.keyword = Some(" ubd ".into());
        let q = db.save_saved_query(&input).unwrap();
        assert_eq!(q.name, "users by day");
        assert_eq!(q.keyword.as_deref(), Some("ubd"));
        assert_eq!(q.folder_id, Some(reports.id));
        assert_eq!(q.profile_id, Some(p));
        assert_eq!(q.created_at, q.updated_at);
        let b = db
            .save_saved_query(&saved("Active sessions", None, None))
            .unwrap();
        assert_eq!(b.keyword, None);

        // Sorted by name, case-insensitively.
        let lib = db.list_saved_queries().unwrap();
        assert_eq!(lib.folders, vec![reports.clone()]);
        assert_eq!(
            lib.queries
                .iter()
                .map(|q| q.name.as_str())
                .collect::<Vec<_>>(),
            ["Active sessions", "users by day"]
        );

        // Update in place: rename, re-key, move to the top level, all connections.
        let updated = db
            .save_saved_query(&SavedQueryInput {
                id: Some(q.id),
                name: "Signups".into(),
                keyword: Some("".into()),
                sql: "select 2".into(),
                folder_id: None,
                profile_id: None,
                keep_sql: None,
            })
            .unwrap();
        assert_eq!(updated.id, q.id);
        assert_eq!(updated.created_at, q.created_at);
        assert_eq!(updated.keyword, None);
        assert_eq!(updated.folder_id, None);
        assert_eq!(updated.profile_id, None);
        assert_eq!(db.list_saved_queries().unwrap().queries.len(), 2);

        // A client-chosen id that is not stored yet creates the query.
        let fresh = Uuid::new_v4();
        let c = db
            .save_saved_query(&SavedQueryInput {
                id: Some(fresh),
                ..saved("c", None, None)
            })
            .unwrap();
        assert_eq!(c.id, fresh);

        assert!(db.delete_saved_query(c.id).unwrap());
        assert!(!db.delete_saved_query(c.id).unwrap());
        assert_eq!(db.list_saved_queries().unwrap().queries.len(), 2);
    }

    #[test]
    fn saved_queries_validate_input() {
        let db = AppDb::open_in_memory().unwrap();
        let invalid = |r: Result<SavedQuery, AppError>| {
            assert!(matches!(r, Err(AppError::InvalidInput(_))), "{r:?}")
        };
        invalid(db.save_saved_query(&saved("   ", None, None)));
        invalid(db.save_saved_query(&saved(&"x".repeat(201), None, None)));
        let mut spaced = saved("a", None, None);
        spaced.keyword = Some("two words".into());
        invalid(db.save_saved_query(&spaced));
        let mut long = saved("a", None, None);
        long.keyword = Some("k".repeat(65));
        invalid(db.save_saved_query(&long));
        assert!(matches!(
            db.save_saved_query(&saved("a", Some(Uuid::new_v4()), None)),
            Err(AppError::NotFound(_))
        ));
        assert!(matches!(
            db.save_query_folder(&folder(" ", None)),
            Err(AppError::InvalidInput(_))
        ));
        assert!(matches!(
            db.save_query_folder(&folder("x", Some(Uuid::new_v4()))),
            Err(AppError::NotFound(_))
        ));
        assert!(db.list_saved_queries().unwrap().queries.is_empty());
    }

    #[test]
    fn query_folders_nest_move_and_cascade() {
        let db = AppDb::open_in_memory().unwrap();
        let a = db.save_query_folder(&folder("A", None)).unwrap();
        let b = db.save_query_folder(&folder("B", Some(a.id))).unwrap();
        let c = db.save_query_folder(&folder("C", Some(b.id))).unwrap();
        let other = db.save_query_folder(&folder("Other", None)).unwrap();
        assert_eq!(b.parent_id, Some(a.id));

        // Not into itself, nor into a folder inside it.
        for parent in [a.id, b.id, c.id] {
            let r = db.save_query_folder(&QueryFolderInput {
                id: Some(a.id),
                name: "A".into(),
                parent_id: Some(parent),
            });
            assert!(matches!(r, Err(AppError::InvalidInput(_))), "{parent}");
        }
        // Rename and move elsewhere.
        let moved = db
            .save_query_folder(&QueryFolderInput {
                id: Some(c.id),
                name: "C2".into(),
                parent_id: Some(other.id),
            })
            .unwrap();
        assert_eq!(moved.name, "C2");
        assert_eq!(moved.parent_id, Some(other.id));
        assert_eq!(moved.created_at, c.created_at);
        let back = db
            .save_query_folder(&QueryFolderInput {
                id: Some(c.id),
                name: "C".into(),
                parent_id: Some(b.id),
            })
            .unwrap();
        assert_eq!(back.parent_id, Some(b.id));

        db.save_saved_query(&saved("in a", Some(a.id), None))
            .unwrap();
        db.save_saved_query(&saved("in c", Some(c.id), None))
            .unwrap();
        db.save_saved_query(&saved("in other", Some(other.id), None))
            .unwrap();
        db.save_saved_query(&saved("top", None, None)).unwrap();

        // Deleting A takes B, C and the queries inside them.
        assert_eq!(db.delete_query_folder(a.id).unwrap(), 2);
        let lib = db.list_saved_queries().unwrap();
        assert_eq!(lib.folders, vec![other.clone()]);
        assert_eq!(
            lib.queries
                .iter()
                .map(|q| q.name.as_str())
                .collect::<Vec<_>>(),
            ["in other", "top"]
        );
        assert_eq!(db.delete_query_folder(a.id).unwrap(), 0);
    }

    #[test]
    fn deleting_a_profile_keeps_its_saved_queries() {
        let db = AppDb::open_in_memory().unwrap();
        let p = db.upsert_profile(&profile("local")).unwrap();
        let q = db
            .save_saved_query(&saved("mine", None, Some(p.id)))
            .unwrap();
        assert_eq!(q.profile_id, Some(p.id));
        assert!(db.delete_profile(p.id).unwrap());
        let lib = db.list_saved_queries().unwrap();
        assert_eq!(lib.queries.len(), 1);
        assert_eq!(lib.queries[0].profile_id, None);
    }

    #[test]
    fn deleting_a_profile_deletes_its_metrics_boards() {
        use crate::metrics::{get_board, list_boards, save_board};
        use crate::types::metrics::MetricsBoardInput;
        let db = AppDb::open_in_memory().unwrap();
        let gone = db.upsert_profile(&profile("temporary")).unwrap();
        let kept = db.upsert_profile(&profile("other")).unwrap();
        let board = |profile_id: Option<Uuid>, name: &str| {
            save_board(
                &db,
                &MetricsBoardInput {
                    id: None,
                    profile_id,
                    name: name.into(),
                    objects: serde_json::json!([{ "id": "o1", "sql": "SELECT 1" }]),
                },
            )
            .unwrap()
        };
        let orphan = board(Some(gone.id), "only on temporary");
        let global = board(None, "everywhere");
        let theirs = board(Some(kept.id), "only on other");

        assert!(db.delete_profile(gone.id).unwrap());

        // The deleted profile's board is gone for good (it cannot be listed any more, so it
        // would never be cleaned up) ...
        assert!(get_board(&db, orphan.id).unwrap().is_none());
        // ... while boards of every connection and of other profiles stay.
        assert!(get_board(&db, global.id).unwrap().is_some());
        assert!(get_board(&db, theirs.id).unwrap().is_some());
        let names: Vec<_> = list_boards(&db, None)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert_eq!(names, ["everywhere", "only on other"]);
    }

    #[test]
    fn settings_and_secrets() {
        let db = AppDb::open_in_memory().unwrap();
        db.set_setting("theme", &serde_json::json!("dark")).unwrap();
        assert_eq!(
            db.get_setting("theme").unwrap(),
            Some(serde_json::json!("dark"))
        );
        db.set_setting("theme", &serde_json::Value::Null).unwrap();
        assert_eq!(db.get_setting("theme").unwrap(), None);
        db.set_secret("k", "v").unwrap();
        assert_eq!(db.get_secret("k").unwrap().as_deref(), Some("v"));
        db.delete_secret("k").unwrap();
        assert_eq!(db.get_secret("k").unwrap(), None);
    }
}
