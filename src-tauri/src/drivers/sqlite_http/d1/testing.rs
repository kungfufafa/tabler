//! An in-process D1 REST API over rusqlite for the D1 tests: the account's database list
//! (paged, filtered by name), a database's details, and `/raw` with single statements and
//! batches, each run as one transaction that rolls back on any error.
//!
//! It refuses what D1 refuses, the way workerd does: an authorizer while a statement is prepared
//! (`BEGIN`, `COMMIT`, `SAVEPOINT` and friends, `ATTACH`, PRAGMAs off D1's list, also read as
//! table-valued functions, and `_cf_KV`, the internal table D1 lists but will not read), and
//! SQLite's own statement loop over a text of several, where only the last statement may have
//! parameters and the last must be given exactly as many as it has. It keeps D1's limits (100
//! parameters, 100 KB statements) and answers as D1's JavaScript would: integers beyond 2^53
//! rounded, whole REALs without a fraction, blobs as byte arrays, and `meta.changes` as the
//! `sqlite3_total_changes()` delta (rows triggers and cascades changed included).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::Connection;
use rusqlite::fallible_iterator::FallibleIterator as _;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::types::Value as Sv;
use serde_json::{Value as J, json};

use crate::drivers::sqlite_http::codes::code_name;
use crate::drivers::sqlite_http::testing::{Answers, Executed, run_prepared};
use crate::drivers::sqlite_http::transport::{HttpRequest, HttpResponse, Method, Transport};
use crate::error::AppError;

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
     statements";
const NOT_AUTHORIZED: &str = "not authorized";
const PARAMS_IN_LAST: &str = "When executing multiple SQL statements in a single call, only the \
     last statement can have parameters.";
const WRONG_BINDINGS: &str = "Wrong number of parameter bindings for SQL query.";

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
    reply_delay: Option<Duration>,
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

    /// Requests run, then the reply waits this long (a client that gives up never reads it).
    pub fn delay_replies(&self, delay: Option<Duration>) {
        self.state.lock().unwrap().reply_delay = delay;
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
        let reply = self.handle(&req);
        if let Some(delay) = Answers::delay(self) {
            tokio::time::sleep(delay).await;
        }
        Ok(reply)
    }
}

impl Answers for FakeD1 {
    fn answer(&self, req: &HttpRequest) -> HttpResponse {
        self.handle(req)
    }

    fn delay(&self) -> Option<Duration> {
        self.state.lock().unwrap().reply_delay
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

/// Why D1's authorizer refuses an action while a statement is prepared, as its message.
fn refusal(action: &AuthAction<'_>) -> Option<String> {
    let protected = |table: &str| {
        table
            .get(..4)
            .is_some_and(|p| p.eq_ignore_ascii_case("_cf_"))
    };
    let listed = |pragma: &str| PRAGMAS.contains(&pragma.to_ascii_lowercase().as_str());
    let not_authorized = || Some(NOT_AUTHORIZED.to_string());
    match *action {
        AuthAction::Transaction { .. } | AuthAction::Savepoint { .. } => {
            Some(NO_TRANSACTIONS.to_string())
        }
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => not_authorized(),
        AuthAction::Pragma {
            pragma_name,
            pragma_value,
        } if !listed(pragma_name) || pragma_value.is_some_and(protected) => not_authorized(),
        AuthAction::Read {
            table_name,
            column_name,
        } if protected(table_name) => Some(format!(
            "access to {table_name}.{column_name} is prohibited"
        )),
        // A PRAGMA read as a table-valued function (`pragma_table_info(?1)`).
        AuthAction::Read { table_name, .. }
            if table_name
                .get(..7)
                .is_some_and(|p| p.eq_ignore_ascii_case("pragma_"))
                && !table_name.get(7..).is_some_and(listed) =>
        {
            not_authorized()
        }
        AuthAction::Insert { table_name }
        | AuthAction::Delete { table_name }
        | AuthAction::Update { table_name, .. }
            if protected(table_name) =>
        {
            not_authorized()
        }
        _ => None,
    }
}

/// D1's authorizer on `conn` while a request's statements are prepared and run; the reason of
/// its last refusal goes to `refused`. Dropping it removes it, so the fake's own `BEGIN` and
/// `COMMIT` around the request pass.
struct Authorizer<'c>(&'c Connection);

impl<'c> Authorizer<'c> {
    fn install(conn: &'c Connection, refused: Arc<Mutex<Option<String>>>) -> Authorizer<'c> {
        conn.authorizer(Some(move |ctx: AuthContext<'_>| {
            match refusal(&ctx.action) {
                Some(why) => {
                    *refused.lock().unwrap() = Some(why);
                    Authorization::Deny
                }
                None => Authorization::Allow,
            }
        }))
        .expect("an authorizer");
        Authorizer(conn)
    }
}

impl Drop for Authorizer<'_> {
    fn drop(&mut self) {
        let _ = self
            .0
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    }
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

/// A failed statement as D1 words it: an authorizer's refusal by its reason.
fn d1_message(e: &rusqlite::Error, refused: &Mutex<Option<String>>) -> String {
    match (e.sqlite_error_code(), refused.lock().unwrap().take()) {
        (Some(rusqlite::ErrorCode::AuthorizationForStatementDenied), Some(why)) => {
            format!("{why}: SQLITE_AUTH")
        }
        _ => sqlite_message(e),
    }
}

/// One `{sql, params}` of a body: each of its statements' results. A text of several runs as D1
/// runs it, through SQLite's own statement loop: only the last statement may have parameters,
/// and the last gets exactly as many values as it has (none are left unbound). D1 refuses a
/// statement with parameters that is not the last before running it; the loop here only learns
/// that once the next statement is prepared, so the request rolls back instead.
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
    let refused = Arc::new(Mutex::new(None));
    let _authorizer = Authorizer::install(conn, refused.clone());
    let failed = |e: rusqlite::Error| (7500, d1_message(&e, &refused));
    let mut statements = rusqlite::Batch::new(conn, sql);
    let mut out = Vec::new();
    let mut last_params = 0;
    while let Some(mut stmt) = statements.next().map_err(failed)? {
        if last_params > 0 {
            return Err((7500, format!("{PARAMS_IN_LAST}: SQLITE_ERROR")));
        }
        last_params = stmt.parameter_count();
        if last_params > 0 && last_params != params.len() {
            return Err((7500, format!("{WRONG_BINDINGS}: SQLITE_ERROR")));
        }
        let bound = if last_params > 0 { &params[..] } else { &[] };
        // D1's `meta.changes` is the `sqlite3_total_changes()` delta: what triggers and
        // cascades changed counts too.
        let Executed {
            cols,
            rows,
            total_changes: changes,
            last_insert_rowid,
            readonly,
            ..
        } = run_prepared(conn, &mut stmt, bound).map_err(failed)?;
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
    if last_params != params.len() {
        return Err((7500, format!("{WRONG_BINDINGS}: SQLITE_ERROR")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> FakeD1 {
        let fake = FakeD1::new();
        fake.seed(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);
             INSERT INTO t (v) VALUES ('a'), ('b'), ('c');",
        );
        fake
    }

    /// `/raw` with one `{sql, params}`: its results, or D1's error message.
    fn raw(fake: &FakeD1, sql: &str, params: J) -> Result<J, String> {
        let url = format!("{API_BASE}/accounts/{ACCOUNT}/d1/database/{MAIN_DB}/raw");
        let req = HttpRequest::post_json(url, &json!({"sql": sql, "params": params})).unwrap();
        let reply: J = serde_json::from_slice(&fake.handle(&req).body).unwrap();
        match reply["success"].as_bool() {
            Some(true) => Ok(reply["result"].clone()),
            _ => Err(reply["errors"][0]["message"].as_str().unwrap().to_string()),
        }
    }

    fn rows(fake: &FakeD1) -> i64 {
        fake.with_db(MAIN_DB, |c| {
            c.query_row("SELECT count(*) FROM t", [], |r| r.get(0))
                .unwrap()
        })
    }

    #[test]
    fn what_d1_refuses() {
        let fake = seeded();
        for refused in [
            "BEGIN",
            "begin transaction",
            "COMMIT",
            "SAVEPOINT a",
            "RELEASE a",
            "ATTACH 'x' AS y",
            "PRAGMA database_list",
            "PRAGMA query_only = 1",
            "PRAGMA table_info(_cf_KV)",
            "SELECT name FROM pragma_database_list",
            "SELECT * FROM _cf_KV",
            "DELETE FROM _cf_KV",
            "SELECT 1; DELETE FROM t; BEGIN",
        ] {
            let message = raw(&fake, refused, json!([])).expect_err(refused);
            assert!(message.ends_with(": SQLITE_AUTH"), "{refused}: {message}");
        }
        assert!(
            raw(&fake, "BEGIN", json!([]))
                .unwrap_err()
                .starts_with("To execute a transaction")
        );
        assert_eq!(rows(&fake), 3, "a refused request changes nothing");
        for allowed in [
            "SELECT 1",
            "PRAGMA foreign_keys = ON",
            "PRAGMA table_info(t)",
            "SELECT * FROM pragma_table_xinfo('t')",
            "SELECT name FROM sqlite_schema m WHERE m.name NOT LIKE '\\_cf\\_%' ESCAPE '\\'",
            "SELECT '_cf_KV', 'pragma_database_list', 'BEGIN'",
        ] {
            raw(&fake, allowed, json!([])).unwrap_or_else(|e| panic!("{allowed}: {e}"));
        }
    }

    #[test]
    fn a_text_runs_as_sqlites_statement_loop_reads_it() {
        let fake = seeded();
        let two = raw(&fake, "SELECT 1; SELECT ?1", json!(["x"])).unwrap();
        assert_eq!(two.as_array().unwrap().len(), 2);
        // Parameters only in the last statement, exactly as many as it has.
        for (sql, params) in [
            ("SELECT ?1; DELETE FROM t", json!(["1"])),
            ("DELETE FROM t WHERE id > ?1", json!([])),
            ("DELETE FROM t", json!(["1"])),
            ("DELETE FROM t WHERE id IN (?1, ?2)", json!(["1"])),
        ] {
            let message = raw(&fake, sql, params).expect_err(sql);
            assert!(message.contains("parameter"), "{sql}: {message}");
        }
        assert_eq!(rows(&fake), 3);
        // SQLite reads `$a(--)` as one variable, so these texts hide a DELETE from a lexer that
        // sees a comment there; unbound (the editor binds nothing) D1 runs none of them.
        for sql in [
            "SELECT $a(--) ; DELETE FROM t",
            "WITH x AS (SELECT $a(--)) DELETE FROM t",
            "SELECT :a::b(') ; DELETE FROM t ; SELECT $b(')",
        ] {
            let message = raw(&fake, sql, json!([])).expect_err(sql);
            assert!(message.contains("parameter"), "{sql}: {message}");
        }
        assert_eq!(rows(&fake), 3);
        // Given its value, the one statement runs: what SQLite reads is a DELETE.
        raw(
            &fake,
            "WITH x AS (SELECT $a(--)) DELETE FROM t",
            json!(["v"]),
        )
        .unwrap();
        assert_eq!(rows(&fake), 0);
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
