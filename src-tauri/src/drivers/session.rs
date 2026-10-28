//! An open connection's session: a SQL session, or the session of a store that speaks no SQL.
//! Commands that build SQL take the SQL view (`sql`), which refuses a native store, so every
//! SQL-only command turns those away without a check of its own; what both kinds can do
//! (introspection, ping, close, cancel, the driver's own calls) goes through `Session` itself.

use std::sync::Arc;

use crate::drivers::native::{CallKind, NativeSession, TableSource};
use crate::drivers::{CancelHandle, DbSession, ServerInfo};
use crate::error::AppError;
use crate::types::browse::TracedSql;
use crate::types::caps::SessionCaps;
use crate::types::profile::Driver;
use crate::types::schema::{
    CompletionMetadata, DatabaseInfo, SchemaInfo, TableMeta, TableRef, TableSummary,
};

#[derive(Clone)]
pub enum Session {
    Sql(Arc<dyn DbSession>),
    Native(Arc<dyn NativeSession>),
}

impl From<Arc<dyn DbSession>> for Session {
    fn from(session: Arc<dyn DbSession>) -> Session {
        Session::Sql(session)
    }
}

impl From<Arc<dyn NativeSession>> for Session {
    fn from(session: Arc<dyn NativeSession>) -> Session {
        Session::Native(session)
    }
}

/// The refusal of a SQL-only command, or of a SQL session, for a store that speaks no SQL.
pub fn not_sql(driver: Driver) -> AppError {
    AppError::Unsupported(format!(
        "{} is not a SQL database: it has no SQL session",
        driver.label()
    ))
}

/// `$body` with `$s` bound to the session of either kind (both traits name the call alike).
macro_rules! either {
    ($session:expr, $s:ident => $body:expr) => {
        match $session {
            Session::Sql($s) => $body,
            Session::Native($s) => $body,
        }
    };
}

impl Session {
    pub fn driver(&self) -> Driver {
        either!(self, s => s.driver())
    }

    pub fn capabilities(&self) -> SessionCaps {
        either!(self, s => s.capabilities())
    }

    /// The SQL session; a native store has none (`Unsupported`).
    pub fn sql(&self) -> Result<Arc<dyn DbSession>, AppError> {
        match self {
            Session::Sql(s) => Ok(s.clone()),
            Session::Native(n) => Err(not_sql(n.driver())),
        }
    }

    pub fn native(&self) -> Option<&Arc<dyn NativeSession>> {
        match self {
            Session::Sql(_) => None,
            Session::Native(n) => Some(n),
        }
    }

    /// What reads a table's rows without SQL: a native store always, a SQL session when its
    /// engine pages rows through its own API.
    pub fn table_source(&self) -> Option<&dyn TableSource> {
        match self {
            Session::Sql(s) => s.table_source(),
            Session::Native(n) => Some(n.as_ref()),
        }
    }

    pub async fn server_info(&self) -> Result<ServerInfo, AppError> {
        either!(self, s => s.server_info().await)
    }

    pub async fn ping(&self) -> Result<(), AppError> {
        either!(self, s => s.ping().await)
    }

    pub async fn close(&self) {
        either!(self, s => s.close().await)
    }

    pub async fn cancel(&self, handle: CancelHandle) -> Result<(), AppError> {
        either!(self, s => s.cancel(handle).await)
    }

    pub fn in_transaction(&self) -> Option<bool> {
        either!(self, s => s.in_transaction())
    }

    pub fn drain_trace(&self) -> Vec<TracedSql> {
        either!(self, s => s.drain_trace())
    }

    pub async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, AppError> {
        either!(self, s => s.list_databases().await)
    }

    pub async fn list_schemas(&self) -> Result<Vec<SchemaInfo>, AppError> {
        either!(self, s => s.list_schemas().await)
    }

    pub async fn list_tables(&self, schema: &str) -> Result<Vec<TableSummary>, AppError> {
        either!(self, s => s.list_tables(schema).await)
    }

    pub async fn table_meta(&self, table: &TableRef) -> Result<TableMeta, AppError> {
        either!(self, s => s.table_meta(table).await)
    }

    /// A view's SQL; for a native store what defines the collection (`definition`).
    pub async fn view_definition(&self, table: &TableRef) -> Result<Option<String>, AppError> {
        match self {
            Session::Sql(s) => s.view_definition(table).await,
            Session::Native(n) => n.definition(table).await,
        }
    }

    pub async fn completion_metadata(&self) -> Result<CompletionMetadata, AppError> {
        either!(self, s => s.completion_metadata().await)
    }

    /// A call of the driver's own (`session_call`).
    pub async fn call(
        &self,
        method: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        either!(self, s => s.call(method, args).await)
    }

    /// Whether `call(method)` only reads; anything unknown changes something.
    pub fn call_kind(&self, method: &str) -> CallKind {
        either!(self, s => s.call_kind(method))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::drivers::CancelSlot;
    use crate::sqlgen::Dialect;
    use crate::testkit::{FakeSession, FakeStore};
    use crate::types::browse::{BrowseRequest, RowKey};
    use crate::types::caps::Feature;
    use crate::types::value::Value;

    fn users() -> TableRef {
        TableRef::new(None, "users")
    }

    #[tokio::test]
    async fn a_sql_session_is_its_own_sql_view() {
        let fake = Arc::new(FakeSession::new(SessionCaps::core(
            Driver::Postgres,
            Dialect::Postgres,
            None,
        )));
        let session = Session::from(fake.clone() as Arc<dyn DbSession>);
        assert_eq!(session.driver(), Driver::Postgres);
        assert!(session.capabilities().has(Feature::EditRows));
        let sql = session.sql().unwrap();
        assert!(std::ptr::addr_eq(Arc::as_ptr(&sql), Arc::as_ptr(&fake)));
        assert!(session.native().is_none());
        assert!(session.table_source().is_none());
        assert_eq!(session.in_transaction(), Some(false));
        session.ping().await.unwrap();
        session.list_tables("s").await.unwrap();
        session.view_definition(&users()).await.unwrap();
        assert_eq!(
            fake.calls(),
            ["ping", "list_tables s", "view_definition users"]
        );
        // A SQL driver offers no calls of its own unless it says so.
        let err = session.call("stats", json!({})).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "unsupported: PostgreSQL: unknown call \u{201c}stats\u{201d}"
        );
        assert_eq!(session.call_kind("stats"), CallKind::Write);
    }

    #[tokio::test]
    async fn a_native_store_has_no_sql_view_and_answers_the_rest_itself() {
        let store = Arc::new(FakeStore::new(SessionCaps::minimal(Driver::Redis)));
        let session = Session::from(store.clone() as Arc<dyn NativeSession>);
        let err = session.sql().err().expect("no SQL session");
        assert_eq!(err.code(), "UNSUPPORTED");
        assert_eq!(
            err.to_string(),
            "unsupported: Redis is not a SQL database: it has no SQL session"
        );
        assert!(session.native().is_some());
        assert_eq!(session.driver(), Driver::Redis);
        assert_eq!(session.capabilities(), SessionCaps::minimal(Driver::Redis));
        assert_eq!(session.in_transaction(), None);

        let names: Vec<String> = session
            .list_tables("")
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, ["orders", "users"]);
        let meta = session.table_meta(&users()).await.unwrap();
        assert_eq!(meta.primary_key, ["_id"]);
        // A store's definition stands in for a view's SQL.
        assert_eq!(
            session.view_definition(&users()).await.unwrap().as_deref(),
            Some("{ \"collection\": \"users\" }")
        );

        let source = session.table_source().expect("a store reads its own rows");
        let req: BrowseRequest = serde_json::from_value(json!({
            "table": users(), "columns": null, "filter": null, "limit": 1, "offset": 1,
        }))
        .unwrap();
        let page = source
            .browse(&meta, &req, CancelSlot::default())
            .await
            .unwrap();
        assert_eq!(page.rows, [[Value::Int(2), Value::Text("bob".into())]]);
        let count = source
            .count(&meta, None, true, CancelSlot::default())
            .await
            .unwrap();
        assert_eq!((count.count, count.exact), (2, true));
        let key = RowKey::Pk([("_id".to_string(), Value::Int(1))].into());
        assert_eq!(
            source.fetch_cell(&meta, &key, "name").await.unwrap(),
            Value::Text("ada".into())
        );

        assert_eq!(session.call_kind("stats"), CallKind::Read);
        assert_eq!(session.call_kind("flush"), CallKind::Write);
        assert_eq!(session.call_kind("anything else"), CallKind::Write);
        assert_eq!(
            session.call("stats", json!({})).await.unwrap(),
            json!({ "orders": 3, "users": 2 })
        );
        let err = session.call("nope", json!({})).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "unsupported: Redis: unknown call \u{201c}nope\u{201d}"
        );

        // What the store was sent drains through the session, once.
        let drained: Vec<String> = session.drain_trace().into_iter().map(|t| t.sql).collect();
        assert!(drained.contains(&"browse users".to_string()), "{drained:?}");
        assert!(session.drain_trace().is_empty());

        session.cancel(CancelHandle::Requested).await.unwrap();
        session.ping().await.unwrap();
        session.server_info().await.unwrap();
        session.close().await;
        assert!(store.is_closed());
        assert!(session.ping().await.is_err());
    }
}
