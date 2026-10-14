//! The structure context of recent assistant requests, reused for [`CONTEXT_TTL`] per connection,
//! schema and table so a conversation does not introspect the database before every message. A
//! schema refresh (or any DDL the app runs) forgets the connection's entries
//! (`Connection::invalidate`), as does closing the connection.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::types::assistant::AssistantContextPreview;
use crate::types::schema::TableRef;

/// How long a gathered context is reused.
pub const CONTEXT_TTL: Duration = Duration::from_secs(60);
/// Entries kept at most (the oldest go first).
const MAX_ENTRIES: usize = 64;

/// What a context depends on: the connection, the schema and the active tab's table.
pub type ContextKey = (Uuid, Option<String>, Option<TableRef>);

/// Gathered contexts with the time they were read.
#[derive(Debug)]
pub struct ContextCache {
    ttl: Duration,
    entries: HashMap<ContextKey, (Instant, AssistantContextPreview)>,
}

impl ContextCache {
    pub fn new(ttl: Duration) -> ContextCache {
        ContextCache {
            ttl,
            entries: HashMap::new(),
        }
    }

    /// The context read for `key` less than the TTL before `now`.
    pub fn get(&mut self, key: &ContextKey, now: Instant) -> Option<AssistantContextPreview> {
        let (at, preview) = self.entries.get(key)?;
        if now.saturating_duration_since(*at) < self.ttl {
            return Some(preview.clone());
        }
        self.entries.remove(key);
        None
    }

    /// Keeps a context read at `now`. One that could not be read completely (a warning) is not
    /// kept: the next message tries again.
    pub fn put(&mut self, key: ContextKey, now: Instant, preview: &AssistantContextPreview) {
        if preview.warning.is_some() {
            self.entries.remove(&key);
            return;
        }
        let ttl = self.ttl;
        self.entries
            .retain(|_, (at, _)| now.saturating_duration_since(*at) < ttl);
        if self.entries.len() >= MAX_ENTRIES
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(key, (now, preview.clone()));
    }

    /// Forgets every context of a connection.
    pub fn forget(&mut self, connection_id: Uuid) {
        self.entries.retain(|(id, _, _), _| *id != connection_id);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

static CACHE: LazyLock<Mutex<ContextCache>> =
    LazyLock::new(|| Mutex::new(ContextCache::new(CONTEXT_TTL)));

/// The app's cache.
pub fn cache() -> std::sync::MutexGuard<'static, ContextCache> {
    CACHE.lock().unwrap_or_else(|p| p.into_inner())
}

/// The structure of a connection changed (schema refresh, DDL) or it closed.
pub fn forget(connection_id: Uuid) {
    cache().forget(connection_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(text: &str, warning: Option<&str>) -> AssistantContextPreview {
        AssistantContextPreview {
            text: text.into(),
            summary: String::new(),
            tables: 1,
            truncated: false,
            warning: warning.map(str::to_string),
        }
    }

    #[test]
    fn reuses_a_context_for_a_minute_per_connection_schema_and_table() {
        let mut cache = ContextCache::new(Duration::from_secs(60));
        let (conn, other) = (Uuid::new_v4(), Uuid::new_v4());
        let t0 = Instant::now();
        let public: ContextKey = (conn, Some("public".into()), None);
        let users: ContextKey = (
            conn,
            Some("public".into()),
            Some(TableRef {
                schema: Some("public".into()),
                name: "users".into(),
            }),
        );
        cache.put(public.clone(), t0, &preview("a", None));
        assert_eq!(
            cache
                .get(&public, t0 + Duration::from_secs(59))
                .unwrap()
                .text,
            "a"
        );
        // Another table (its DDL is in the context) or schema is read again.
        assert!(cache.get(&users, t0).is_none());
        assert!(cache.get(&(conn, None, None), t0).is_none());
        // After a minute it is read again.
        assert!(cache.get(&public, t0 + Duration::from_secs(60)).is_none());
        assert!(cache.is_empty());

        // A context with a warning is not kept.
        cache.put(
            public.clone(),
            t0,
            &preview("partial", Some("could not read")),
        );
        assert!(cache.get(&public, t0).is_none());

        // A schema refresh forgets the connection's contexts only.
        cache.put(public.clone(), t0, &preview("a", None));
        cache.put(users.clone(), t0, &preview("b", None));
        cache.put((other, None, None), t0, &preview("c", None));
        cache.forget(conn);
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&(other, None, None), t0).is_some());
    }
}
