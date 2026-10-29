//! Test support: the throwaway keys of `tests/fixtures/snowflake`, and a scripted Snowflake that
//! answers the session API snowflake-connector-rs speaks (reached through `custom_base_url`):
//! `POST /session/v1/login-request` (key-pair JWTs checked like Snowflake checks them: the RS256
//! signature with the registered public key, `iss` with its fingerprint, `sub`, the lifetime),
//! `POST /queries/v1/query-request` (canned answers per statement text, inline rows and gzipped
//! remote partitions, errors, asynchronous answers to poll, statements that run until aborted,
//! an expired session), `POST /queries/v1/abort-request`, and the partition downloads.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine as _;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::params::jwt_account;

pub const KEY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/snowflake/rsa_key.p8"
));
pub const ENCRYPTED_KEY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/snowflake/rsa_key_encrypted.p8"
));
pub const PKCS1_KEY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/snowflake/rsa_key_pkcs1.pem"
));
pub const PUBLIC_KEY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/snowflake/rsa_key.pub"
));
pub const KEY_PASSPHRASE: &str = "tabler-test";
/// From Snowflake's documented openssl pipeline over `rsa_key.p8` (see the fixtures' README).
pub const KEY_FINGERPRINT: &str = "SHA256:vJ5HJhsgorWJFKft6LsG4A6Tator6dqNsrtQ1Za0BTE=";
/// The token the fake hands out at sign-in and expects with every statement.
pub const SESSION_TOKEN: &str = "fake-session-token";

pub fn fixture_path(name: &str) -> String {
    format!(
        "{}/tests/fixtures/snowflake/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn b64(s: &str) -> Option<Vec<u8>> {
    let body: String = s
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .concat();
    base64::engine::general_purpose::STANDARD.decode(body).ok()
}

/// The SubjectPublicKeyInfo DER of the fixtures' key pair.
pub fn public_key_der() -> Vec<u8> {
    b64(PUBLIC_KEY).unwrap_or_default()
}

/// How the fake's user signs in.
#[derive(Debug, Clone)]
pub enum Login {
    /// RSA_PUBLIC_KEY_FP and the public key registered for the user.
    KeyPair {
        fingerprint: String,
        public_key: Vec<u8>,
    },
    Password {
        password: String,
        passcode: Option<String>,
    },
    OAuth(String),
}

impl Login {
    /// The fixtures' key, with the fingerprint Snowflake would compute for it.
    pub fn key_pair() -> Login {
        let der = public_key_der();
        let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &der);
        Login::KeyPair {
            fingerprint: format!(
                "SHA256:{}",
                base64::engine::general_purpose::STANDARD.encode(digest.as_ref())
            ),
            public_key: der,
        }
    }
}

/// A result column as the row type describes it.
#[derive(Debug, Clone)]
pub struct FakeColumn {
    name: String,
    ty: String,
    precision: Option<u8>,
    scale: Option<u8>,
    length: Option<u32>,
}

impl FakeColumn {
    pub fn new(name: &str, ty: &str) -> FakeColumn {
        FakeColumn {
            name: name.into(),
            ty: ty.into(),
            precision: None,
            scale: None,
            length: None,
        }
    }

    pub fn precision(mut self, precision: u8, scale: u8) -> FakeColumn {
        self.precision = Some(precision);
        self.scale = Some(scale);
        self
    }

    pub fn scale(mut self, scale: u8) -> FakeColumn {
        self.scale = Some(scale);
        self
    }

    pub fn length(mut self, length: u32) -> FakeColumn {
        self.length = Some(length);
        self
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "name": self.name, "type": self.ty, "nullable": true, "precision": self.precision,
            "scale": self.scale, "length": self.length, "byteLength": self.length,
            "database": "DB", "schema": "PUBLIC", "table": "T",
        })
    }
}

type FakeRows = Vec<Vec<Option<String>>>;

/// How the fake answers a statement.
#[derive(Debug, Clone)]
pub enum Answer {
    /// Rows inline, then one remote partition per chunk (served gzipped).
    Rows {
        columns: Vec<FakeColumn>,
        rows: FakeRows,
        chunks: Vec<FakeRows>,
    },
    Error {
        code: String,
        message: String,
    },
    /// Runs until an abort request naming its requestId arrives, then answers "SQL execution
    /// canceled"; without one, after 30 s, an error that no cancel maps to.
    Hang,
    /// Waits, then answers.
    Slow(Duration, Box<Answer>),
    /// `333334` with a result URL, which then answers.
    Async(Box<Answer>),
}

impl Answer {
    pub fn rows(columns: Vec<FakeColumn>, rows: FakeRows) -> Answer {
        Answer::Rows {
            columns,
            rows,
            chunks: Vec::new(),
        }
    }

    pub fn chunk(mut self, rows: FakeRows) -> Answer {
        if let Answer::Rows { chunks, .. } = &mut self {
            chunks.push(rows);
        }
        self
    }

    pub fn error(code: &str, message: &str) -> Answer {
        Answer::Error {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// A request the fake received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: serde_json::Value,
}

impl Recorded {
    /// A parameter of the query string (`requestId`).
    pub fn param(&self, name: &str) -> Option<String> {
        query_param(&self.query, name)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A sign-in the fake received, with the JWT's claims when it checked one.
#[derive(Debug, Clone)]
pub struct LoginRecord {
    pub query: String,
    pub body: serde_json::Value,
    pub claims: serde_json::Value,
}

struct State {
    account: String,
    user: String,
    login: Login,
    answers: HashMap<String, Answer>,
    pending: HashMap<String, Answer>,
    chunks: HashMap<String, Vec<u8>>,
    queries: Vec<Recorded>,
    logins: Vec<LoginRecord>,
    aborted: HashSet<String>,
    expire: bool,
    next_id: u64,
}

struct Shared {
    state: Mutex<State>,
    aborts: Notify,
    base: String,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The scripted server; stops when dropped.
pub struct FakeSnowflake {
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Drop for FakeSnowflake {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeSnowflake {
    pub async fn start(account: &str, user: &str, login: Login) -> FakeSnowflake {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the fake Snowflake");
        let addr: SocketAddr = listener.local_addr().expect("the fake's address");
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                account: account.into(),
                user: user.into(),
                login,
                answers: HashMap::new(),
                pending: HashMap::new(),
                chunks: HashMap::new(),
                queries: Vec::new(),
                logins: Vec::new(),
                aborted: HashSet::new(),
                expire: false,
                next_id: 0,
            }),
            aborts: Notify::new(),
            base: format!("http://{addr}"),
        });
        let serving = Arc::clone(&shared);
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let shared = Arc::clone(&serving);
                tokio::spawn(async move {
                    let _ = serve(socket, shared).await;
                });
            }
        });
        FakeSnowflake { shared, task }
    }

    /// The base URL for `EndpointConfig::custom_base_url`.
    pub fn url(&self) -> String {
        format!("{}/", self.shared.base)
    }

    /// How to answer a statement (its text, trimmed).
    pub fn answer(&self, sql: &str, answer: Answer) {
        self.shared
            .state()
            .answers
            .insert(sql.trim().to_string(), answer);
    }

    /// The next statement is told its session expired.
    pub fn expire_session(&self) {
        self.shared.state().expire = true;
    }

    pub fn logins(&self) -> Vec<LoginRecord> {
        self.shared.state().logins.clone()
    }

    pub fn queries(&self) -> Vec<Recorded> {
        self.shared.state().queries.clone()
    }

    /// The request ids abort requests named.
    pub fn aborts(&self) -> Vec<String> {
        self.shared.state().aborted.iter().cloned().collect()
    }

    /// Waits (up to 5 s) until `n` statements have arrived.
    pub async fn wait_for_queries(&self, n: usize) {
        for _ in 0..500 {
            if self.shared.state().queries.len() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

struct Request {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

async fn read_request(socket: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        if buf.len() > 1 << 20 {
            return None;
        }
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    Some(Request {
        method,
        path,
        query,
        headers,
        body,
    })
}

async fn respond(socket: &mut TcpStream, status: u16, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n",
        if status == 200 { "OK" } else { "Error" },
        body.len()
    );
    socket.write_all(head.as_bytes()).await?;
    socket.write_all(body).await?;
    socket.flush().await
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

async fn serve(mut socket: TcpStream, shared: Arc<Shared>) -> std::io::Result<()> {
    let Some(req) = read_request(&mut socket).await else {
        return Ok(());
    };
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
    let (status, reply): (u16, Vec<u8>) = match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/session/v1/login-request") => (200, login(&shared, &req, body).into_bytes()),
        ("POST", "/queries/v1/query-request") => (200, query(&shared, &req, body).await),
        ("POST", "/queries/v1/abort-request") => {
            if let Some(id) = body.get("requestId").and_then(|v| v.as_str()) {
                shared.state().aborted.insert(id.to_string());
            }
            shared.aborts.notify_waiters();
            (
                200,
                json!({"success": true, "data": null})
                    .to_string()
                    .into_bytes(),
            )
        }
        ("GET", path) if path.starts_with("/chunks/") => {
            match shared.state().chunks.get(path).cloned() {
                Some(bytes) => (200, bytes),
                None => (404, b"no such chunk".to_vec()),
            }
        }
        ("GET", path) if path.starts_with("/queries/") && path.ends_with("/result") => {
            let pending = shared.state().pending.remove(path);
            match pending {
                Some(answer) => (200, reply(&shared, &answer, "").await),
                None => (404, b"no such result".to_vec()),
            }
        }
        _ => (404, b"not found".to_vec()),
    };
    respond(&mut socket, status, &reply).await
}

fn rejected(code: &str, message: &str) -> String {
    json!({"success": false, "code": code, "message": message, "data": null}).to_string()
}

fn login(shared: &Shared, req: &Request, body: serde_json::Value) -> String {
    let data = &body["data"];
    let (account, user, expected) = {
        let s = shared.state();
        (s.account.clone(), s.user.clone(), s.login.clone())
    };
    let mut record = LoginRecord {
        query: req.query.clone(),
        body: body.clone(),
        claims: serde_json::Value::Null,
    };
    let text = |k: &str| data.get(k).and_then(|v| v.as_str()).unwrap_or_default();
    let verdict = if text("ACCOUNT_NAME") != account || text("LOGIN_NAME") != user {
        Err(("390100", "Incorrect username or password was specified."))
    } else {
        match &expected {
            Login::KeyPair {
                fingerprint,
                public_key,
            } => {
                if text("AUTHENTICATOR") != "SNOWFLAKE_JWT" {
                    Err(("390100", "Incorrect username or password was specified."))
                } else {
                    match verify_jwt(text("TOKEN"), public_key) {
                        Some(claims) => {
                            let issuer = format!(
                                "{}.{}.{fingerprint}",
                                jwt_account(&account),
                                user.to_ascii_uppercase()
                            );
                            let subject =
                                format!("{}.{}", jwt_account(&account), user.to_ascii_uppercase());
                            let iat = claims["iat"].as_i64().unwrap_or(0);
                            let exp = claims["exp"].as_i64().unwrap_or(0);
                            let fresh = (iat - chrono::Utc::now().timestamp()).abs() < 600
                                && exp > iat
                                && exp - iat <= 3600;
                            let ok = claims["iss"] == issuer.as_str()
                                && claims["sub"] == subject.as_str()
                                && fresh;
                            record.claims = claims;
                            if ok {
                                Ok(())
                            } else {
                                Err(("390144", "JWT token is invalid."))
                            }
                        }
                        None => Err(("390144", "JWT token is invalid.")),
                    }
                }
            }
            Login::Password { password, passcode } => {
                let code_ok = match passcode {
                    Some(code) => {
                        text("PASSCODE") == code && text("EXT_AUTHN_DUO_METHOD") == "passcode"
                    }
                    None => true,
                };
                if text("PASSWORD") == password && code_ok {
                    Ok(())
                } else {
                    Err(("390100", "Incorrect username or password was specified."))
                }
            }
            Login::OAuth(token) => {
                if text("AUTHENTICATOR") == "OAUTH" && text("TOKEN") == token {
                    Ok(())
                } else {
                    Err(("390303", "Invalid OAuth access token."))
                }
            }
        }
    };
    shared.state().logins.push(record);
    match verdict {
        Ok(()) => json!({
            "success": true,
            "data": {
                "token": SESSION_TOKEN,
                "masterToken": "fake-master-token",
                "validityInSeconds": 3600,
                "sessionInfo": {"databaseName": "DB", "schemaName": "PUBLIC"}
            }
        })
        .to_string(),
        Err((code, message)) => rejected(code, message),
    }
}

/// The claims of an RS256 JWT whose signature `public_key` (SPKI DER) verifies.
fn verify_jwt(token: &str, public_key: &[u8]) -> Option<serde_json::Value> {
    let url = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut parts = token.split('.');
    let (header, payload, signature) = (parts.next()?, parts.next()?, parts.next()?);
    let header: serde_json::Value = serde_json::from_slice(&url.decode(header).ok()?).ok()?;
    if header["alg"] != "RS256" {
        return None;
    }
    let signed = format!("{}.{payload}", token.split('.').next()?);
    aws_lc_rs::signature::UnparsedPublicKey::new(
        &aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256,
        public_key,
    )
    .verify(signed.as_bytes(), &url.decode(signature).ok()?)
    .ok()?;
    serde_json::from_slice(&url.decode(payload).ok()?).ok()
}

async fn query(shared: &Shared, req: &Request, body: serde_json::Value) -> Vec<u8> {
    let request_id = query_param(&req.query, "requestId").unwrap_or_default();
    let sql = body["sqlText"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    let answer = {
        let mut s = shared.state();
        s.queries.push(Recorded {
            method: req.method.clone(),
            path: req.path.clone(),
            query: req.query.clone(),
            headers: req.headers.clone(),
            body: body.clone(),
        });
        let token = format!("Snowflake Token=\"{SESSION_TOKEN}\"");
        let authorized = req
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && *v == token);
        if !authorized {
            return rejected("390104", "Authentication token is invalid.").into_bytes();
        }
        if std::mem::take(&mut s.expire) {
            return rejected("390112", "Your session has expired. Please login again.")
                .into_bytes();
        }
        s.answers.get(&sql).cloned()
    };
    match answer {
        Some(a) => reply(shared, &a, &request_id).await,
        None => json!({
            "success": false,
            "code": "002003",
            "message": format!("SQL compilation error: the fake knows no answer to {sql}"),
            "data": {"queryId": "q-unknown"}
        })
        .to_string()
        .into_bytes(),
    }
}

fn gzip(text: &str) -> Vec<u8> {
    use std::io::Write as _;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = enc.write_all(text.as_bytes());
    enc.finish().unwrap_or_default()
}

fn rows_json(rows: &FakeRows) -> Vec<serde_json::Value> {
    rows.iter().map(|r| json!(r)).collect()
}

/// The reply to `answer`, waiting where it says to.
fn reply<'a>(
    shared: &'a Shared,
    answer: &'a Answer,
    request_id: &'a str,
) -> futures::future::BoxFuture<'a, Vec<u8>> {
    Box::pin(async move {
        let query_id = {
            let mut s = shared.state();
            s.next_id += 1;
            format!("01b{:05}-fake", s.next_id)
        };
        match answer {
            Answer::Rows {
                columns,
                rows,
                chunks,
            } => {
                let mut chunk_list = Vec::new();
                for (i, chunk) in chunks.iter().enumerate() {
                    let path = format!("/chunks/{query_id}/{i}");
                    let text = rows_json(chunk)
                        .iter()
                        .map(|r| r.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    let bytes = gzip(&text);
                    chunk_list.push(json!({
                        "url": format!("{}{path}", shared.base),
                        "rowCount": chunk.len(),
                        "uncompressedSize": text.len(),
                        "compressedSize": bytes.len(),
                    }));
                    shared.state().chunks.insert(path, bytes);
                }
                let total = rows.len() + chunks.iter().map(Vec::len).sum::<usize>();
                json!({
                    "success": true,
                    "code": null,
                    "message": null,
                    "data": {
                        "queryId": query_id,
                        "rowtype": columns.iter().map(FakeColumn::json).collect::<Vec<_>>(),
                        "rowset": rows_json(rows),
                        "total": total,
                        "returned": rows.len(),
                        "queryResultFormat": "json",
                        "chunks": chunk_list,
                        "chunkHeaders": {},
                        "parameters": [{"name": "TIMEZONE", "value": "America/Los_Angeles"}],
                    }
                })
                .to_string()
                .into_bytes()
            }
            Answer::Error { code, message } => json!({
                "success": false,
                "code": code,
                "message": message,
                "data": {"queryId": query_id, "sqlState": "42000"}
            })
            .to_string()
            .into_bytes(),
            Answer::Hang => {
                let mut canceled = false;
                for _ in 0..300 {
                    let aborted = shared.aborts.notified();
                    if shared.state().aborted.contains(request_id) {
                        canceled = true;
                        break;
                    }
                    let _ = tokio::time::timeout(Duration::from_millis(100), aborted).await;
                }
                if !canceled {
                    return rejected("999999", "the fake hung statement saw no abort of its own")
                        .into_bytes();
                }
                json!({
                    "success": false,
                    "code": "000604",
                    "sqlState": "57014",
                    "message": "SQL execution canceled",
                    "data": {"queryId": query_id}
                })
                .to_string()
                .into_bytes()
            }
            Answer::Slow(wait, inner) => {
                tokio::time::sleep(*wait).await;
                reply(shared, inner, request_id).await
            }
            Answer::Async(inner) => {
                let url = format!("/queries/{query_id}/result");
                shared
                    .state()
                    .pending
                    .insert(url.clone(), (**inner).clone());
                json!({
                    "success": true,
                    "code": "333334",
                    "message": "Asynchronous execution in progress. Use provided query id to perform query monitoring and management.",
                    "data": {"queryId": query_id, "getResultUrl": url}
                })
                .to_string()
                .into_bytes()
            }
        }
    })
}
