//! An in-process D1 REST API over rusqlite for the D1 tests: the account's database list
//! (paged, filtered by name), a database's details, and `/raw` with single statements and
//! batches, each run as one transaction that rolls back on any error.
//!
//! It refuses what D1 refuses: `BEGIN`, `COMMIT`, `SAVEPOINT` and friends, `ATTACH`, PRAGMAs
//! off D1's list (also as table-valued functions), and any statement naming `_cf_KV`, the
//! internal table D1 lists but will not read. D1 does this with an authorizer; without
//! rusqlite's `hooks` feature (a change request) the fake judges the statement text. It keeps
//! D1's limits (100 parameters, 100 KB statements) and answers as D1's JavaScript would: integers
//! beyond 2^53 rounded, whole REALs without a fraction, blobs as byte arrays, and `meta.changes`
//! as the `sqlite3_total_changes()` delta (rows triggers and cascades changed included).

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use rusqlite::Connection;
use rusqlite::types::Value as Sv;
use serde_json::{Value as J, json};

use crate::drivers::sqlite_http::codes::code_name;
use crate::drivers::sqlite_http::testing::{Executed, run};
use crate::drivers::sqlite_http::transport::{HttpRequest, HttpResponse, Method, Transport};
use crate::error::AppError;
use crate::sqlgen::Dialect;
use crate::sqlgen::split::{leading_keywords, split_statements};

use super::api::{MAX_PARAMS, MAX_STATEMENT_BYTES};

pub const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";
pub const MAIN_DB: &str = "11111111-2222-4333-8444-555555555555";
pub const API_BASE: &str = "http://d1.test/client/v4";

/// D1's PRAGMA allow-list.
const PRAGMAS: [&str; 17] = [
    "table_list",
    "table_info",
    "table_xinfo",
    "index_list",
    "index_info",
    "index_xinfo",
    "quick_check",
    "foreign_key_check",
    "foreign_key_list",
    "case_sensitive_like",
    "ignore_check_constraints",
    "legacy_alter_table",
    "recursive_triggers",
    "reverse_unordered_selects",
    "foreign_keys",
    "defer_foreign_keys",
    "optimize",
];

const NO_TRANSACTIONS: &str = "To execute a transaction, please use the state.storage.transaction() \
     or state.storage.transactionSync() APIs instead of the SQL BEGIN TRANSACTION or SAVEPOINT \
     statements: SQLITE_AUTH";

struct FakeDb {
    uuid: String,
    name: String,
    conn: Connection,
}

#[derive(Default)]
struct State {
    failures: VecDeque<HttpResponse>,
    status: Option<u16>,
    strict_params: bool,
}

pub struct FakeD1 {
    dbs: Mutex<Vec<FakeDb>>,
    state: Mutex<State>,
    log: Mutex<Vec<HttpRequest>>,
}

impl FakeD1 {
    /// One database, `main-db` (`MAIN_DB`), with D1's `_cf_KV`.
    pub fn new() -> FakeD1 {
        let fake = FakeD1 {
            dbs: Mutex::new(Vec::new()),
            state: Mutex::new(State::default()),
            log: Mutex::new(Vec::new()),
        };
        fake.add(MAIN_DB, "main-db");
        fake
    }

    fn add(&self, uuid: &str, name: &str) {
        let conn = Connection::open_in_memory().expect("an in-memory database");
        conn.execute_batch(
            "CREATE TABLE _cf_KV (key TEXT PRIMARY KEY, value BLOB) WITHOUT ROWID;
             PRAGMA foreign_keys = ON;",
        )
        .expect("_cf_KV");
        self.dbs.lock().unwrap().push(FakeDb {
            uuid: uuid.to_string(),
            name: name.to_string(),
            conn,
        });
    }

    /// Another database of the account; its UUID.
    pub fn add_database(&self, name: &str) -> String {
        let n = self.dbs.lock().unwrap().len() + 1;
        let uuid = format!("{n:08x}-0000-4000-8000-{n:012x}");
        self.add(&uuid, name);
        uuid
    }

    /// Runs `f` on the database `uuid` (seeding, checking).
    pub fn with_db<T>(&self, uuid: &str, f: impl FnOnce(&Connection) -> T) -> T {
        let dbs = self.dbs.lock().unwrap();
        let db = dbs
            .iter()
            .find(|d| d.uuid == uuid)
            .expect("a fake database");
        f(&db.conn)
    }

    pub fn seed(&self, sql: &str) {
        self.with_db(MAIN_DB, |c| c.execute_batch(sql).expect("seed"));
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.log.lock().unwrap().clone()
    }

    pub fn fail_next(&self, reply: HttpResponse) {
        self.state.lock().unwrap().failures.push_back(reply);
    }

    /// Every reply has this status, with Cloudflare's authentication error.
    pub fn answer_status(&self, status: Option<u16>) {
        self.state.lock().unwrap().status = status;
    }

    /// Parameters must be strings (or null), as the API documents.
    pub fn strict_params(&self, strict: bool) {
        self.state.lock().unwrap().strict_params = strict;
    }

    pub fn handle(&self, req: &HttpRequest) -> HttpResponse {
        self.log.lock().unwrap().push(req.clone());
        let strict = {
            let mut state = self.state.lock().unwrap();
            if let Some(reply) = state.failures.pop_front() {
                return reply;
            }
            if let Some(status) = state.status {
                return fail(status, 10000, "Authentication error");
            }
            state.strict_params
        };
        let segments: Vec<&str> = req.path().split('/').filter(|s| !s.is_empty()).collect();
        let Some(at) = segments.iter().position(|s| *s == "accounts") else {
            return fail(404, 7000, "No route for that URI");
        };
        let rest = &segments[at + 1..];
        match rest {
            [account, ..] if *account != ACCOUNT => fail(
                403,
                7403,
                "The given account is not valid or is not authorized to access this service",
            ),
            [_, "d1", "database"] if req.method == Method::Get => self.list(req),
            [_, "d1", "database", id] if req.method == Method::Get => self.info(id),
            [_, "d1", "database", id, "raw"] if req.method == Method::Post => {
                self.raw(id, req, strict)
            }
            _ => fail(404, 7000, "No route for that URI"),
        }
    }

    fn list(&self, req: &HttpRequest) -> HttpResponse {
        let url = reqwest::Url::parse(&req.url).expect("a request URL");
        let param = |k: &str| {
            url.query_pairs()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.into_owned())
        };
        let page: usize = param("page")
            .and_then(|p| p.parse().ok())
            .unwrap_or(1)
            .max(1);
        let per_page: usize = param("per_page")
            .and_then(|p| p.parse().ok())
            .unwrap_or(1000);
        let name = param("name");
        let dbs = self.dbs.lock().unwrap();
        let matching: Vec<&FakeDb> = dbs
            .iter()
            .filter(|d| name.as_deref().is_none_or(|n| d.name == n))
            .collect();
        let found: Vec<J> = matching
            .iter()
            .skip((page - 1) * per_page)
            .take(per_page)
            .map(|d| info_json(d))
            .collect();
        let mut reply = envelope(json!(found));
        reply["result_info"] = json!({
            "page": page, "per_page": per_page, "count": found.len(), "total_count": matching.len()
        });
        HttpResponse::json(200, &reply)
    }

    fn info(&self, id: &str) -> HttpResponse {
        let dbs = self.dbs.lock().unwrap();
        match dbs.iter().find(|d| d.uuid == id) {
            Some(d) => HttpResponse::json(200, &envelope(info_json(d))),
            None => not_found(id),
        }
    }

    fn raw(&self, id: &str, req: &HttpRequest, strict: bool) -> HttpResponse {
        let dbs = self.dbs.lock().unwrap();
        let Some(db) = dbs.iter().find(|d| d.uuid == id) else {
            return not_found(id);
        };
        let Ok(body) = req.json() else {
            return fail(400, 7400, "Malformed JSON in request body");
        };
        let queries: Vec<J> = match body.get("batch") {
            Some(J::Array(batch)) => batch.clone(),
            Some(_) => return fail(400, 7400, "batch must be an array"),
            None => vec![body.clone()],
        };
        let conn = &db.conn;
        if let Err(e) = conn.execute_batch("BEGIN") {
            return fail(500, 7500, &e.to_string());
        }
        let mut results = Vec::new();
        for q in &queries {
            match run_query(conn, q, strict) {
                Ok(mut r) => results.append(&mut r),
                Err((code, message)) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    return fail(400, code, &message);
                }
            }
        }
        if let Err(e) = conn.execute_batch("COMMIT") {
            let _ = conn.execute_batch("ROLLBACK");
            return fail(400, 7500, &format!("{e}: SQLITE_CONSTRAINT"));
        }
        HttpResponse::json(200, &envelope(json!(results)))
    }
}

impl Default for FakeD1 {
    fn default() -> FakeD1 {
        FakeD1::new()
    }
}

#[async_trait]
impl Transport for FakeD1 {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError> {
        Ok(self.handle(&req))
    }
}

fn envelope(result: J) -> J {
    json!({"success": true, "errors": [], "messages": [], "result": result})
}

fn fail(status: u16, code: i64, message: &str) -> HttpResponse {
    HttpResponse::json(
        status,
        &json!({"success": false, "errors": [{"code": code, "message": message}], "messages": [], "result": null}),
    )
}

fn not_found(id: &str) -> HttpResponse {
    fail(404, 7404, &format!("The database {id} could not be found"))
}

fn info_json(d: &FakeDb) -> J {
    json!({
        "uuid": d.uuid, "name": d.name, "version": "production",
        "created_at": "2026-10-01T00:00:00.000Z", "file_size": 12288, "num_tables": 1,
        "jurisdiction": null, "read_replication": {"mode": "disabled"}
    })
}

/// Why D1 would refuse a statement, as its message.
fn refusal(sql: &str) -> Option<String> {
    let kw = leading_keywords(sql, 2);
    let word = |i: usize| kw.get(i).map(String::as_str).unwrap_or("");
    let not_authorized = || Some("not authorized: SQLITE_AUTH".to_string());
    match word(0) {
        "BEGIN" | "COMMIT" | "END" | "ROLLBACK" | "SAVEPOINT" | "RELEASE" => {
            return Some(NO_TRANSACTIONS.to_string());
        }
        "ATTACH" | "DETACH" | "VACUUM" => return not_authorized(),
        "PRAGMA" if !PRAGMAS.contains(&word(1).to_ascii_lowercase().as_str()) => {
            return not_authorized();
        }
        _ => {}
    }
    let lower = sql.to_ascii_lowercase();
    for (at, _) in lower.match_indices("pragma_") {
        let name: String = lower[at + "pragma_".len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !PRAGMAS.contains(&name.as_str()) {
            return not_authorized();
        }
    }
    if lower.contains("_cf_kv") {
        return Some("access to _cf_KV.key is prohibited: SQLITE_AUTH".to_string());
    }
    None
}

fn param(v: &J, strict: bool) -> Result<Sv, (i64, String)> {
    let refused = || {
        (
            7400,
            format!("Invalid parameter {v}: params must be strings"),
        )
    };
    match v {
        J::Null => Ok(Sv::Null),
        J::String(s) => Ok(Sv::Text(s.clone())),
        _ if strict => Err(refused()),
        J::Bool(b) => Ok(Sv::Integer(i64::from(*b))),
        J::Number(n) => Ok(match n.as_i64() {
            Some(i) => Sv::Integer(i),
            None => Sv::Real(n.as_f64().unwrap_or(f64::NAN)),
        }),
        _ => Err(refused()),
    }
}

/// A cell as D1's JavaScript serializes it.
fn cell(v: &Sv) -> J {
    const SAFE: f64 = 9_007_199_254_740_992.0;
    match v {
        Sv::Null => J::Null,
        Sv::Integer(n) => {
            // A JavaScript number: beyond 2^53 the nearest double.
            let f = *n as f64;
            if n.unsigned_abs() <= 1 << 53 {
                json!(n)
            } else if f.abs() < 9.2e18 {
                json!(f as i64)
            } else {
                json!(f)
            }
        }
        Sv::Real(f) if f.fract() == 0.0 && f.abs() <= SAFE => json!(*f as i64),
        Sv::Real(f) => json!(f),
        Sv::Text(t) => json!(t),
        Sv::Blob(b) => json!(b),
    }
}

fn sqlite_message(e: &rusqlite::Error) -> String {
    match e {
        rusqlite::Error::SqliteFailure(ffi, msg) => format!(
            "{}: {}",
            msg.clone().unwrap_or_else(|| ffi.to_string()),
            code_name(ffi.extended_code)
        ),
        rusqlite::Error::SqlInputError {
            msg, offset, error, ..
        } => format!(
            "{msg} at offset {offset}: {}",
            code_name(error.extended_code)
        ),
        other => format!("{other}: SQLITE_ERROR"),
    }
}

/// One `{sql, params}` of a body: each of its statements' results.
fn run_query(conn: &Connection, q: &J, strict: bool) -> Result<Vec<J>, (i64, String)> {
    let sql = q["sql"].as_str().unwrap_or_default();
    let params: Vec<Sv> = match &q["params"] {
        J::Null => Vec::new(),
        J::Array(items) => items
            .iter()
            .map(|v| param(v, strict))
            .collect::<Result<_, _>>()?,
        _ => return Err((7400, "params must be an array".into())),
    };
    if params.len() > MAX_PARAMS {
        return Err((
            7500,
            "too many SQL variables at offset 0: SQLITE_ERROR".into(),
        ));
    }
    if sql.len() > MAX_STATEMENT_BYTES {
        return Err((7500, "statement too long: SQLITE_TOOBIG".into()));
    }
    let mut out = Vec::new();
    for piece in split_statements(sql, Dialect::Sqlite) {
        if let Some(message) = refusal(&piece.sql) {
            return Err((7500, message));
        }
        // D1's `meta.changes` is the `sqlite3_total_changes()` delta: what triggers and
        // cascades changed counts too.
        let Executed {
            cols,
            rows,
            total_changes: changes,
            last_insert_rowid,
            readonly,
            ..
        } = run(conn, &piece.sql, &params, &[]).map_err(|e| (7500, sqlite_message(&e)))?;
        out.push(json!({
            "results": {
                "columns": cols.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
                "rows": rows.iter().map(|r| r.iter().map(cell).collect::<Vec<_>>()).collect::<Vec<_>>(),
            },
            "meta": {
                "served_by": "v3-prod", "served_by_region": "WEUR", "served_by_primary": true,
                "duration": 0.1, "changes": changes, "last_row_id": last_insert_rowid,
                "changed_db": !readonly && changes > 0, "size_after": 16384,
                "rows_read": rows.len(), "rows_written": changes,
                "timings": {"sql_duration_ms": 0.1}
            },
            "success": true
        }));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_d1_refuses() {
        for refused in [
            "BEGIN",
            "begin transaction",
            "COMMIT",
            "SAVEPOINT a",
            "ATTACH 'x' AS y",
            "PRAGMA database_list",
            "PRAGMA main.table_info(t)",
            "PRAGMA query_only = 1",
            "SELECT name FROM pragma_database_list",
            "SELECT * FROM _cf_KV",
        ] {
            assert!(refusal(refused).is_some(), "{refused}");
        }
        for allowed in [
            "SELECT 1",
            "PRAGMA foreign_keys = ON",
            "PRAGMA table_info(t)",
            "SELECT * FROM pragma_table_xinfo(?1)",
            "SELECT name FROM sqlite_schema m WHERE m.name NOT LIKE '\\_cf\\_%' ESCAPE '\\'",
        ] {
            assert_eq!(refusal(allowed), None, "{allowed}");
        }
    }

    #[test]
    fn cells_as_javascript_writes_them() {
        assert_eq!(cell(&Sv::Integer(5)), json!(5));
        assert_eq!(
            cell(&Sv::Integer(9_007_199_254_740_993)),
            json!(9_007_199_254_740_992_i64)
        );
        assert_eq!(cell(&Sv::Real(2.0)), json!(2));
        assert_eq!(cell(&Sv::Real(2.5)), json!(2.5));
        assert_eq!(cell(&Sv::Blob(vec![0, 255])), json!([0, 255]));
        assert_eq!(cell(&Sv::Integer(i64::MAX)), json!(i64::MAX as f64));
    }
}
