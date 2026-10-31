//! Test helpers both drivers' fakes share: statements run on rusqlite with bound values the way a
//! server runs them, and a fake served over a real socket (`testkit::fake_http`), so a session
//! opened by its `entry::open` talks to it through the shared HTTP client.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, watch};

use rusqlite::types::{Value as Sv, ValueRef};
use rusqlite::{Connection, Statement};

use super::catalog::{CatalogQuery, Rows};
use super::transport::{HttpRequest, HttpResponse, HttpTransport, Method};
use crate::appdb::AppDb;
use crate::drivers::common::http::{Credential, HttpClient, HttpEndpoint, HttpOptions};
use crate::drivers::common::token::TokenSource;
use crate::drivers::{RowSink, Session};
use crate::secrets::{Backend, Secrets};
use crate::state::{AppState, Connection as AppConnection};
use crate::testkit::{FakeServer, Reply, Route};
use crate::types::profile::{ConnStatus, ConnectionInfo, ConnectionProfile, Driver};
use crate::types::query::ResultColumn;
use crate::types::value::Value;

/// What one statement did.
#[derive(Debug, Clone, Default)]
pub struct Executed {
    /// Name and declared type.
    pub cols: Vec<(String, Option<String>)>,
    pub rows: Vec<Vec<Sv>>,
    /// Rows the statement changed (0 for a statement that changed none, unlike `changes()`).
    pub changes: u64,
    /// The `total_changes()` delta: also the rows its triggers and foreign-key cascades changed
    /// (D1's `meta.changes`).
    pub total_changes: u64,
    pub last_insert_rowid: i64,
    pub readonly: bool,
}

/// Prepares and runs one statement (a text of several is refused, as `sqld` refuses it).
pub fn run(conn: &Connection, sql: &str, params: &[Sv]) -> rusqlite::Result<Executed> {
    let mut stmt = conn.prepare(sql)?;
    run_prepared(conn, &mut stmt, params)
}

/// Runs a prepared statement (one of a `rusqlite::Batch`, say) with its parameters by position;
/// those beyond `params` stay unbound (NULL).
pub fn run_prepared(
    conn: &Connection,
    stmt: &mut Statement<'_>,
    params: &[Sv],
) -> rusqlite::Result<Executed> {
    for (i, v) in params.iter().enumerate().take(stmt.parameter_count()) {
        stmt.raw_bind_parameter(i + 1, v)?;
    }
    let cols: Vec<(String, Option<String>)> = stmt
        .columns()
        .iter()
        .map(|c| (c.name().to_string(), c.decl_type().map(str::to_string)))
        .collect();
    let readonly = stmt.readonly();
    let before = conn.total_changes();
    let mut rows = Vec::new();
    if cols.is_empty() {
        stmt.raw_execute()?;
    } else {
        let mut found = stmt.raw_query();
        while let Some(row) = found.next()? {
            let mut out = Vec::with_capacity(cols.len());
            for i in 0..cols.len() {
                out.push(owned(row.get_ref(i)?));
            }
            rows.push(out);
        }
    }
    let total_changes = conn.total_changes().saturating_sub(before);
    let changes = if total_changes != 0 {
        conn.changes()
    } else {
        0
    };
    Ok(Executed {
        cols,
        rows,
        changes,
        total_changes,
        last_insert_rowid: conn.last_insert_rowid(),
        readonly,
    })
}

fn owned(v: ValueRef<'_>) -> Sv {
    match v {
        ValueRef::Null => Sv::Null,
        ValueRef::Integer(n) => Sv::Integer(n),
        ValueRef::Real(f) => Sv::Real(f),
        ValueRef::Text(t) => Sv::Text(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Sv::Blob(b.to_vec()),
    }
}

/// A parameter as the local driver would bind it.
pub fn to_sqlite(v: &Value) -> Sv {
    match v {
        Value::Null | Value::Default => Sv::Null,
        Value::Bool(b) => Sv::Integer(i64::from(*b)),
        Value::Int(n) => Sv::Integer(*n),
        Value::Float(f) => Sv::Real(*f),
        Value::BigInt(s) => s.parse().map_or_else(|_| Sv::Text(s.clone()), Sv::Integer),
        Value::Bytes(b) => Sv::Blob(super::value::param_bytes(b).unwrap_or_default()),
        other => Sv::Text(other.as_text().unwrap_or_default()),
    }
}

/// A cell by its storage class alone (what the catalog parsers get from either engine).
pub fn from_sqlite(v: &Sv) -> Value {
    match v {
        Sv::Null => Value::Null,
        Sv::Integer(n) => Value::from_i64(*n),
        Sv::Real(f) => Value::from_f64("real", *f),
        Sv::Text(t) => Value::Text(t.clone()),
        Sv::Blob(b) => Value::from_bytes(b),
    }
}

/// The rows of a catalog statement run on a local database.
pub fn sqlite_rows(conn: &Connection, q: &CatalogQuery) -> Rows {
    let params: Vec<Sv> = q.params.iter().map(to_sqlite).collect();
    let done = run(conn, q.sql, &params).unwrap_or_else(|e| panic!("{}: {e}", q.sql));
    done.rows
        .iter()
        .map(|r| r.iter().map(from_sqlite).collect())
        .collect()
}

pub fn catalog_rows(conn: &Connection, queries: &[CatalogQuery]) -> Vec<Rows> {
    queries.iter().map(|q| sqlite_rows(conn, q)).collect()
}

/// A fake server: answers the lane's requests, after an optional delay.
pub trait Answers: Send + Sync + 'static {
    fn answer(&self, req: &HttpRequest) -> HttpResponse;

    /// How long the reply waits once the request has run (a client may give up meanwhile).
    fn delay(&self) -> Option<Duration> {
        None
    }
}

/// `fake` behind a socket on 127.0.0.1: every path, any method; with `auth` a request without
/// that `authorization` header gets a 401.
pub async fn serve(fake: Arc<dyn Answers>, auth: Option<&str>) -> FakeServer {
    crate::testkit::install_crypto();
    let mut route = Route::new("*", "/*").handler(move |req| {
        let url = match &req.query {
            Some(q) => format!("http://fake{}?{q}", req.path),
            None => format!("http://fake{}", req.path),
        };
        let method = if req.method.eq_ignore_ascii_case("GET") {
            Method::Get
        } else {
            Method::Post
        };
        let body = (!req.body.is_empty()).then(|| req.body.clone());
        let reply = fake.answer(&HttpRequest { method, url, body });
        let mut out = Reply::new(reply.status).body(reply.body);
        if let Some(content_type) = &reply.content_type {
            out = out.header("content-type", content_type);
        }
        if let Some(secs) = reply.retry_after {
            out = out.header("retry-after", &secs.to_string());
        }
        if let Some(delay) = fake.delay() {
            out = out.delay(delay);
        }
        out
    });
    if let Some(auth) = auth {
        route = route.auth(auth);
    }
    FakeServer::start(vec![route]).await
}

/// The sessions' transport to `base`, with `token` as the bearer (none when empty).
pub fn http_transport(base: &str, token: Option<&str>) -> HttpTransport {
    crate::testkit::install_crypto();
    let client = HttpClient::new(
        HttpEndpoint::new(base, None).expect("an endpoint"),
        HttpOptions::default(),
        Credential::bearer(TokenSource::from_secret(token)),
    )
    .expect("an HTTP client");
    HttpTransport::new(client)
}

/// A profile of `driver` with `fields` set over the defaults (profile JSON, camelCase).
pub fn profile(driver: Driver, fields: serde_json::Value) -> ConnectionProfile {
    let mut json = serde_json::json!({
        "id": uuid::Uuid::new_v4(), "name": "remote", "driver": driver,
        "ssl": "disable", "options": {}, "sortOrder": 0, "createdAt": "", "updatedAt": "",
    });
    if let (Some(base), Some(extra)) = (json.as_object_mut(), fields.as_object()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    serde_json::from_value(json).expect("a profile")
}

/// A new app state with one open connection of `profile` holding `session`, for the command
/// bodies (`browse_rows`, `apply_changes_on`…).
pub async fn app_connection(
    profile: ConnectionProfile,
    session: Session,
) -> (Arc<AppState>, Arc<AppConnection>) {
    let appdb = AppDb::open_in_memory().expect("an app database");
    let profile = appdb.upsert_profile(&profile).expect("a saved profile");
    let state = Arc::new(AppState::new(
        appdb.clone(),
        Secrets::new(Backend::File, appdb),
    ));
    let id = uuid::Uuid::new_v4();
    let conn = Arc::new(AppConnection {
        id,
        profile: profile.clone(),
        info: RwLock::new(ConnectionInfo {
            connection_id: id,
            profile_id: profile.id,
            name: profile.name.clone(),
            color: None,
            driver: profile.driver,
            server_version: String::new(),
            current_database: None,
            current_user: None,
            tunnel: None,
            read_only: profile.options.read_only,
            capabilities: Some(session.capabilities()),
        }),
        session: RwLock::new(session),
        schema_cache: RwLock::new(HashMap::new()),
        status: watch::channel(ConnStatus::Connected).0,
        tunnel: None,
    });
    state.insert_connection(conn.clone(), None).await;
    (state, conn)
}

/// Collects what a statement streams.
#[derive(Debug, Default)]
pub struct Sink {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

impl RowSink for Sink {
    fn begin_set(&mut self, columns: Vec<ResultColumn>) {
        self.columns = columns.into_iter().map(|c| c.name).collect();
    }

    fn rows(&mut self, rows: Vec<Vec<Value>>) -> ControlFlow<()> {
        self.rows.extend(rows);
        ControlFlow::Continue(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_bind_by_position() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (a INTEGER, b TEXT)")
            .unwrap();
        let ins = run(
            &conn,
            "INSERT INTO t VALUES (?1, ?2) RETURNING a, b",
            &[Sv::Integer(7), Sv::Text("x".into())],
        )
        .unwrap();
        assert_eq!((ins.changes, ins.total_changes), (1, 1));
        assert_eq!(ins.rows, [[Sv::Integer(7), Sv::Text("x".into())]]);
        assert!(!ins.readonly);
        let sel = run(&conn, "SELECT a, b FROM t", &[]).unwrap();
        assert_eq!(
            (sel.changes, sel.total_changes),
            (0, 0),
            "a read changes nothing"
        );
        // What a trigger changes counts in the total only.
        conn.execute_batch(
            "CREATE TABLE log (a INTEGER);
             CREATE TRIGGER t_log AFTER DELETE ON t BEGIN INSERT INTO log VALUES (old.a); END;",
        )
        .unwrap();
        let del = run(&conn, "DELETE FROM t", &[]).unwrap();
        assert_eq!((del.changes, del.total_changes), (1, 2));
        assert!(sel.readonly);
        assert_eq!(
            sel.cols,
            [
                ("a".to_string(), Some("INTEGER".to_string())),
                ("b".to_string(), Some("TEXT".to_string()))
            ]
        );
    }
}
