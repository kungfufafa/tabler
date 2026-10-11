//! The app's own SQLite database: connection profiles, settings, query history, trusted SSH hosts
//! and (dev-only) secrets. Lives in the Tauri app data dir.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::error::AppError;
use crate::types::history::{HistoryEntry, HistoryFilter, NewHistoryEntry};
use crate::types::profile::ConnectionProfile;

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
];

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

    pub fn list_profiles(&self) -> Result<Vec<ConnectionProfile>, AppError> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT data FROM profiles ORDER BY sort_order, lower(name)")
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            let data = row.map_err(|e| AppError::AppDb(e.to_string()))?;
            out.push(serde_json::from_str(&data)?);
        }
        Ok(out)
    }

    pub fn get_profile(&self, id: Uuid) -> Result<Option<ConnectionProfile>, AppError> {
        let conn = self.lock();
        let data: Option<String> = conn
            .query_row(
                "SELECT data FROM profiles WHERE id = ?1",
                params![id.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AppError::AppDb(e.to_string()))?;
        match data {
            Some(d) => Ok(Some(serde_json::from_str(&d)?)),
            None => Ok(None),
        }
    }

    /// Inserts or updates a profile, filling timestamps. Returns the stored profile.
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
        let conn = self.lock();
        let n = conn
            .execute(
                "DELETE FROM profiles WHERE id = ?1",
                params![id.to_string()],
            )
            .map_err(|e| AppError::AppDb(e.to_string()))?;
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

    /// History rows, newest first. `search` is a case-insensitive substring match on the SQL.
    pub fn list_history(&self, filter: &HistoryFilter) -> Result<Vec<HistoryEntry>, AppError> {
        let limit = filter
            .limit
            .unwrap_or(HISTORY_DEFAULT_LIMIT)
            .clamp(1, HISTORY_MAX_LIMIT);
        let pattern = filter.search.as_deref().filter(|s| !s.is_empty()).map(|s| {
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
                 ORDER BY id DESC
                 LIMIT ?4",
            )
            .map_err(db)?;
        let rows = stmt
            .query_map(
                params![
                    filter.profile_id.map(|id| id.to_string()),
                    pattern,
                    filter.before_id,
                    limit
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
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
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

    /// Runs an arbitrary closure with the raw connection (history, trusted hosts).
    pub fn with_conn<R>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<R, rusqlite::Error>,
    ) -> Result<R, AppError> {
        let mut conn = self.lock();
        f(&mut conn).map_err(|e| AppError::AppDb(e.to_string()))
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
