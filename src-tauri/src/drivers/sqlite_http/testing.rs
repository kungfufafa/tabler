//! Test helpers both drivers' fakes share: statements run on rusqlite with bound values the way a
//! server runs them, and a plain reqwest transport for the wire-level tests against
//! `testkit::fake_http` (the shared `HttpClient` takes its place in Phase B).

use async_trait::async_trait;
use futures::StreamExt as _;
use rusqlite::types::{Value as Sv, ValueRef};
use rusqlite::{Connection, Statement};

use super::catalog::{CatalogQuery, Rows};
use super::transport::{HttpRequest, HttpResponse, Method, StreamingResponse, Transport};
use crate::error::AppError;
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

/// Prepares and runs one statement with positional and named parameters.
pub fn run(
    conn: &Connection,
    sql: &str,
    positional: &[Sv],
    named: &[(String, Sv)],
) -> rusqlite::Result<Executed> {
    let mut stmt = conn.prepare(sql)?;
    run_prepared(conn, &mut stmt, positional, named)
}

/// Runs a prepared statement (one of a `rusqlite::Batch`, say).
pub fn run_prepared(
    conn: &Connection,
    stmt: &mut Statement<'_>,
    positional: &[Sv],
    named: &[(String, Sv)],
) -> rusqlite::Result<Executed> {
    for (i, v) in positional.iter().enumerate() {
        if i < stmt.parameter_count() {
            stmt.raw_bind_parameter(i + 1, v)?;
        }
    }
    for (name, v) in named {
        let index = [String::new(), ":".into(), "@".into(), "$".into()]
            .iter()
            .find_map(|p| stmt.parameter_index(&format!("{p}{name}")).ok().flatten());
        if let Some(index) = index {
            stmt.raw_bind_parameter(index, v)?;
        }
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
    let done = run(conn, q.sql, &params, &[]).unwrap_or_else(|e| panic!("{}: {e}", q.sql));
    done.rows
        .iter()
        .map(|r| r.iter().map(from_sqlite).collect())
        .collect()
}

pub fn catalog_rows(conn: &Connection, queries: &[CatalogQuery]) -> Vec<Rows> {
    queries.iter().map(|q| sqlite_rows(conn, q)).collect()
}

/// reqwest without proxies (the requests must reach 127.0.0.1), sending the token as a bearer
/// only when there is one: a `sqld` without auth answers 400 to an empty one.
pub struct ReqwestTransport {
    http: reqwest::Client,
    token: Option<String>,
}

impl ReqwestTransport {
    pub fn new(token: Option<&str>) -> ReqwestTransport {
        crate::testkit::install_crypto();
        ReqwestTransport {
            http: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("a reqwest client"),
            token: token.filter(|t| !t.is_empty()).map(str::to_string),
        }
    }

    async fn start(&self, req: HttpRequest) -> Result<reqwest::Response, AppError> {
        let mut builder = match req.method {
            Method::Get => self.http.get(&req.url),
            Method::Post => self.http.post(&req.url),
        };
        if let Some(token) = &self.token {
            builder = builder.bearer_auth(token);
        }
        if let Some(body) = req.body {
            builder = builder
                .header("content-type", "application/json")
                .body(body);
        }
        builder.send().await.map_err(map_reqwest)
    }
}

fn map_reqwest(e: reqwest::Error) -> AppError {
    if e.is_timeout() {
        AppError::Timeout
    } else if e.is_connect() {
        AppError::Connect(e.to_string())
    } else {
        AppError::NotConnected
    }
}

fn head(resp: &reqwest::Response) -> (u16, Option<u64>, Option<String>) {
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    (
        resp.status().as_u16(),
        header("retry-after").and_then(|v| v.trim().parse().ok()),
        header("content-type"),
    )
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError> {
        let resp = self.start(req).await?;
        let (status, retry_after, content_type) = head(&resp);
        let body = resp.bytes().await.map_err(map_reqwest)?.to_vec();
        Ok(HttpResponse {
            status,
            retry_after,
            content_type,
            body,
        })
    }

    async fn send_streaming(&self, req: HttpRequest) -> Result<StreamingResponse, AppError> {
        let resp = self.start(req).await?;
        let (status, retry_after, content_type) = head(&resp);
        let body = resp
            .bytes_stream()
            .map(|piece| piece.map(|b| b.to_vec()).map_err(map_reqwest));
        Ok(StreamingResponse {
            status,
            retry_after,
            content_type,
            body: Box::pin(body),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_bind_by_position_and_name() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (a INTEGER, b TEXT)")
            .unwrap();
        let ins = run(
            &conn,
            "INSERT INTO t VALUES (?1, :b) RETURNING a, b",
            &[Sv::Integer(7)],
            &[("b".into(), Sv::Text("x".into()))],
        )
        .unwrap();
        assert_eq!((ins.changes, ins.total_changes), (1, 1));
        assert_eq!(ins.rows, [[Sv::Integer(7), Sv::Text("x".into())]]);
        assert!(!ins.readonly);
        let sel = run(&conn, "SELECT a, b FROM t", &[], &[]).unwrap();
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
        let del = run(&conn, "DELETE FROM t", &[], &[]).unwrap();
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
