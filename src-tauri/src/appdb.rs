//! The app's own SQLite database: connection profiles, settings, query history, trusted SSH hosts
//! and (dev-only) secrets. Lives in the Tauri app data dir.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::error::AppError;
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
