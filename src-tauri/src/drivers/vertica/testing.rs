//! A scripted Vertica server for the driver's tests, on a loopback port (a cancel request comes on
//! a second connection, as from the real client).
//!
//! It reads the startup message (answering an SSLRequest with `N`), asks for the password the
//! script names and checks the answer with `auth`, then reports the effective protocol version,
//! the backend key and ReadyForQuery. After that each request goes to the test's handler as a
//! `Request` (a simple query, a statement to prepare, a bound execution, a COPY refusal) and the
//! bytes the handler returns, built with `msg`, are written back. `Reply::Hang` writes its bytes
//! and then waits for a CancelRequest naming this connection, which it answers like Vertica
//! (`57014`, then ReadyForQuery); `Reply::Close` hangs up.
//!
//! Only what the tests need of the protocol is here; `msg` builds the backend messages byte by
//! byte, independently of `wire`'s parsers.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use super::auth;
use super::wire::{CANCEL_REQUEST_CODE, SSL_REQUEST_CODE};

/// Backend messages, whole (tag, length, body).
pub mod msg {
    /// A message with its tag and length.
    pub fn tag(tag: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        out.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
        out.extend_from_slice(body);
        out
    }

    fn cstr(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    }

    pub fn auth(code: u32, extra: &[u8]) -> Vec<u8> {
        let mut body = code.to_be_bytes().to_vec();
        body.extend_from_slice(extra);
        tag(b'R', &body)
    }

    pub fn auth_ok() -> Vec<u8> {
        auth(0, &[])
    }

    pub fn auth_md5(salt: [u8; 4]) -> Vec<u8> {
        auth(5, &salt)
    }

    /// HASH (65536) or HASH_SHA512 (66048): the salt, the user salt's length (16), the user salt.
    pub fn auth_sha512(code: u32, salt: [u8; 4], user_salt: [u8; 16]) -> Vec<u8> {
        let mut extra = salt.to_vec();
        extra.extend_from_slice(&16u32.to_be_bytes());
        extra.extend_from_slice(&user_salt);
        auth(code, &extra)
    }

    pub fn key_data(pid: u32, key: u32) -> Vec<u8> {
        let mut body = pid.to_be_bytes().to_vec();
        body.extend_from_slice(&key.to_be_bytes());
        tag(b'K', &body)
    }

    pub fn ready(status: u8) -> Vec<u8> {
        tag(b'Z', &[status])
    }

    pub fn parameter_status(name: &str, value: &str) -> Vec<u8> {
        let mut body = Vec::new();
        cstr(&mut body, name);
        cstr(&mut body, value);
        tag(b'S', &body)
    }

    pub fn command_complete(command: &str) -> Vec<u8> {
        let mut body = Vec::new();
        cstr(&mut body, command);
        tag(b'C', &body)
    }

    pub fn command_description(command: &str, copy_rewrite: Option<&str>) -> Vec<u8> {
        let mut body = Vec::new();
        cstr(&mut body, command);
        body.extend_from_slice(&u16::from(copy_rewrite.is_some()).to_be_bytes());
        cstr(&mut body, copy_rewrite.unwrap_or(""));
        tag(b'm', &body)
    }

    pub fn empty_query() -> Vec<u8> {
        tag(b'I', &[])
    }

    pub fn parse_complete() -> Vec<u8> {
        tag(b'1', &[])
    }

    pub fn bind_complete() -> Vec<u8> {
        tag(b'2', &[])
    }

    pub fn no_data() -> Vec<u8> {
        tag(b'n', &[])
    }

    pub fn load_file(name: &str) -> Vec<u8> {
        let mut body = Vec::new();
        cstr(&mut body, name);
        tag(b'H', &body)
    }

    fn fields(fields: &[(u8, &str)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (kind, value) in fields {
            body.push(*kind);
            cstr(&mut body, value);
        }
        body.push(0);
        body
    }

    pub fn error(fields_: &[(u8, &str)]) -> Vec<u8> {
        tag(b'E', &fields(fields_))
    }

    pub fn notice(fields_: &[(u8, &str)]) -> Vec<u8> {
        tag(b'N', &fields(fields_))
    }

    /// An ERROR with its SQLSTATE, message and Vertica code.
    pub fn simple_error(sqlstate: &str, message: &str, code: &str) -> Vec<u8> {
        error(&[
            (b'S', "ERROR"),
            (b'C', sqlstate),
            (b'M', message),
            (b'V', code),
        ])
    }

    /// A RowDescription field as the server describes it.
    #[derive(Debug, Clone)]
    pub struct FieldSpec {
        pub name: String,
        pub table_oid: u64,
        pub table: Option<(String, String)>,
        pub attribute: u16,
        pub parent: u16,
        /// `oid_or_index` is an index into the type pool.
        pub user: bool,
        pub oid_or_index: u32,
        pub size: i16,
        pub nullable: bool,
        pub identity: bool,
        pub modifier: i32,
        pub format: u16,
    }

    impl FieldSpec {
        pub fn not_null(mut self) -> FieldSpec {
            self.nullable = false;
            self
        }

        pub fn identity(mut self) -> FieldSpec {
            self.identity = true;
            self
        }

        pub fn binary(mut self) -> FieldSpec {
            self.format = 1;
            self
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn field(
        name: &str,
        table_oid: u64,
        table: Option<(&str, &str)>,
        attribute: u16,
        parent: u16,
        user: bool,
        oid_or_index: u32,
        size: i16,
        modifier: i32,
    ) -> FieldSpec {
        FieldSpec {
            name: name.into(),
            table_oid,
            table: table.map(|(s, t)| (s.into(), t.into())),
            attribute,
            parent,
            user,
            oid_or_index,
            size,
            nullable: true,
            identity: false,
            modifier,
            format: 0,
        }
    }

    /// A computed column of a built-in type.
    pub fn column(name: &str, oid: u32, modifier: i32) -> FieldSpec {
        field(name, 0, None, 0, 0, false, oid, -1, modifier)
    }

    fn pool(body: &mut Vec<u8>, pool: &[(u32, &str)]) {
        body.extend_from_slice(&(pool.len() as u32).to_be_bytes());
        for (oid, name) in pool {
            body.extend_from_slice(&oid.to_be_bytes());
            cstr(body, name);
        }
    }

    /// A RowDescription: the count, the type pool, the fields; `complex`: each field carries its
    /// parent's attribute number.
    pub fn row_description(pool_: &[(u32, &str)], fields: &[FieldSpec], complex: bool) -> Vec<u8> {
        let mut body = (fields.len() as u16).to_be_bytes().to_vec();
        if fields.is_empty() {
            return tag(b'T', &body);
        }
        pool(&mut body, pool_);
        for f in fields {
            cstr(&mut body, &f.name);
            body.extend_from_slice(&f.table_oid.to_be_bytes());
            if f.table_oid != 0 {
                let (s, t) = f.table.clone().unwrap_or_default();
                cstr(&mut body, &s);
                cstr(&mut body, &t);
            }
            body.extend_from_slice(&f.attribute.to_be_bytes());
            if complex {
                body.extend_from_slice(&f.parent.to_be_bytes());
            }
            body.push(u8::from(f.user));
            body.extend_from_slice(&f.oid_or_index.to_be_bytes());
            body.extend_from_slice(&f.size.to_be_bytes());
            body.extend_from_slice(&u16::from(f.nullable).to_be_bytes());
            body.extend_from_slice(&u16::from(f.identity).to_be_bytes());
            body.extend_from_slice(&f.modifier.to_be_bytes());
            body.extend_from_slice(&f.format.to_be_bytes());
        }
        tag(b'T', &body)
    }

    /// A ParameterDescription: per parameter (user type flag, OID or pool index, modifier, not
    /// null), then the pool.
    pub fn parameter_description(
        params: &[(u8, u32, i32, bool)],
        pool_: &[(u32, &str)],
    ) -> Vec<u8> {
        let mut body = (params.len() as u16).to_be_bytes().to_vec();
        if params.is_empty() {
            return tag(b't', &body);
        }
        pool(&mut body, pool_);
        for (user, oid, modifier, not_null) in params {
            body.push(*user);
            body.extend_from_slice(&oid.to_be_bytes());
            body.extend_from_slice(&modifier.to_be_bytes());
            body.extend_from_slice(&u16::from(*not_null).to_be_bytes());
        }
        tag(b't', &body)
    }

    /// A DataRow; None is NULL.
    pub fn data_row(cells: &[Option<&[u8]>]) -> Vec<u8> {
        let mut body = (cells.len() as u16).to_be_bytes().to_vec();
        for c in cells {
            match c {
                None => body.extend_from_slice(&(-1i32).to_be_bytes()),
                Some(bytes) => {
                    body.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                    body.extend_from_slice(bytes);
                }
            }
        }
        tag(b'D', &body)
    }

    /// A DataRow of text cells; "NULL" stands for NULL.
    pub fn text_row(cells: &[&str]) -> Vec<u8> {
        let cells: Vec<Option<&[u8]>> = cells
            .iter()
            .map(|c| (*c != "NULL").then_some(c.as_bytes()))
            .collect();
        data_row(&cells)
    }

    /// A whole SELECT result of text columns (`varchar`), its rows and the command tag, then
    /// ReadyForQuery; `complex` as in `row_description` (true at the fake's default protocol,
    /// 3.16; false where the script reports none).
    pub fn select(columns: &[&str], rows: &[&[&str]], complex: bool) -> Vec<u8> {
        let specs: Vec<FieldSpec> = columns.iter().map(|c| column(c, 9, -1)).collect();
        let mut out = row_description(&[], &specs, complex);
        for r in rows {
            out.extend(text_row(r));
        }
        out.extend(command_complete("SELECT"));
        out.extend(ready(b'I'));
        out
    }
}

/// What the server asks the client to prove.
#[derive(Debug, Clone)]
pub enum AuthPlan {
    /// No password (trust).
    Trust,
    Md5 {
        salt: [u8; 4],
    },
    Sha512 {
        salt: [u8; 4],
        user_salt: [u8; 16],
    },
    Cleartext,
    /// An OAuth access token.
    OAuth,
    /// A request of this code, which the client cannot answer.
    Code(u32),
}

/// What the client asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// A simple query's text.
    Query(String),
    /// Parse, Describe and Flush of a statement.
    Prepare(String),
    /// Bind, Execute and Sync of the prepared statement: its text, the parameter type OIDs and
    /// values (None: NULL), whether binary results were asked for.
    Execute {
        sql: String,
        oids: Vec<u32>,
        values: Vec<Option<Vec<u8>>>,
        binary: bool,
    },
    /// A Sync with nothing bound (after a failed prepare).
    Sync,
    /// CopyFail or CopyError, with the client's message.
    CopyRefused(String),
}

/// How the server answers a request.
pub enum Reply {
    Bytes(Vec<u8>),
    /// These bytes, then nothing until a cancel request for the connection arrives.
    Hang(Vec<u8>),
    /// Hang up.
    Close,
}

pub type Handler = Arc<dyn Fn(Request) -> Reply + Send + Sync>;

/// The server's script.
#[derive(Clone)]
pub struct Script {
    pub auth: AuthPlan,
    /// The password (or token) the client must prove.
    pub password: String,
    /// The protocol version the server reports (None: an old server, no ParameterStatus).
    pub protocol_version: Option<u32>,
    pub handler: Handler,
}

impl Script {
    pub fn new(
        auth: AuthPlan,
        password: &str,
        handler: impl Fn(Request) -> Reply + Send + Sync + 'static,
    ) -> Script {
        Script {
            auth,
            password: password.into(),
            protocol_version: Some(super::wire::PROTOCOL_VERSION),
            handler: Arc::new(handler),
        }
    }
}

/// What the server saw.
#[derive(Clone, Default)]
pub struct Seen {
    pub startups: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
    /// The protocol version each startup asked for.
    pub versions: Arc<Mutex<Vec<u32>>>,
    pub passwords: Arc<Mutex<Vec<Vec<u8>>>>,
    pub requests: Arc<Mutex<Vec<Request>>>,
    pub cancels: Arc<Mutex<Vec<(u32, u32)>>>,
    pub ssl_requests: Arc<Mutex<usize>>,
    pub terminated: Arc<Mutex<usize>>,
}

impl Seen {
    pub fn startups(&self) -> Vec<BTreeMap<String, String>> {
        self.startups.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn passwords(&self) -> Vec<Vec<u8>> {
        self.passwords.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn cancels(&self) -> Vec<(u32, u32)> {
        self.cancels.lock().map(|v| v.clone()).unwrap_or_default()
    }

    fn push<T>(list: &Mutex<Vec<T>>, item: T) {
        if let Ok(mut l) = list.lock() {
            l.push(item);
        }
    }
}

/// The secret key the server gives connection `pid`.
pub fn key_of(pid: u32) -> u32 {
    pid.wrapping_mul(7919) ^ 0x5eed
}

/// Starts the server; returns its port and what it saw.
pub async fn serve(script: Script) -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let port = listener.local_addr().expect("a local address").port();
    let seen = Seen::default();
    let state = seen.clone();
    tokio::spawn(async move {
        let mut pid = 1000;
        while let Ok((socket, _)) = listener.accept().await {
            pid += 1;
            tokio::spawn(connection(socket, script.clone(), state.clone(), pid));
        }
    });
    (port, seen)
}

/// Connects to the fake.
pub async fn connect(port: u16) -> TcpStream {
    TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the fake server")
}

async fn untagged(socket: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let len = socket.read_u32().await? as usize;
    let mut body = vec![0u8; len.saturating_sub(4)];
    socket.read_exact(&mut body).await?;
    Ok(body)
}

async fn tagged(socket: &mut TcpStream) -> std::io::Result<(u8, Vec<u8>)> {
    let tag = socket.read_u8().await?;
    Ok((tag, untagged(socket).await?))
}

/// A body's NUL-terminated strings, from `at`.
fn cstrs(body: &[u8], at: usize) -> Vec<String> {
    body.get(at..)
        .unwrap_or_default()
        .split(|&b| b == 0)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

fn be32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4)
        .and_then(|s| s.try_into().ok())
        .map_or(0, u32::from_be_bytes)
}

fn be16(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2)
        .and_then(|s| s.try_into().ok())
        .map_or(0, u16::from_be_bytes)
}

/// The startup message's version and parameters (`protocol_version` is binary).
fn startup_params(body: &[u8]) -> (u32, BTreeMap<String, String>) {
    let mut params = BTreeMap::new();
    let mut version = 0;
    let mut at = 4;
    while let Some(n) = body.get(at..).and_then(|r| r.iter().position(|&b| b == 0)) {
        if n == 0 {
            break;
        }
        let name = String::from_utf8_lossy(&body[at..at + n]).into_owned();
        at += n + 1;
        if name == "protocol_version" {
            version = be32(body, at);
            at += 5;
            continue;
        }
        let Some(m) = body.get(at..).and_then(|r| r.iter().position(|&b| b == 0)) else {
            break;
        };
        params.insert(
            name,
            String::from_utf8_lossy(&body[at..at + m]).into_owned(),
        );
        at += m + 1;
    }
    (version, params)
}

/// The execution a Bind of the prepared `sql` asks for: the parameter OIDs and values, binary
/// results.
fn bind_request(sql: &str, body: &[u8]) -> Request {
    let names = cstrs(body, 0);
    let mut at = names.first().map_or(0, String::len) + 1;
    at += names.get(1).map_or(0, String::len) + 1;
    at += 2; // no parameter format codes
    let count = be16(body, at) as usize;
    at += 2;
    let oids: Vec<u32> = (0..count).map(|i| be32(body, at + 4 * i)).collect();
    at += 4 * count;
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        let len = be32(body, at) as i32;
        at += 4;
        if len < 0 {
            values.push(None);
        } else {
            let len = len as usize;
            values.push(body.get(at..at + len).map(<[u8]>::to_vec));
            at += len;
        }
    }
    Request::Execute {
        sql: sql.to_string(),
        oids,
        values,
        binary: be16(body, at) == 1,
    }
}

async fn connection(
    mut socket: TcpStream,
    script: Script,
    seen: Seen,
    pid: u32,
) -> std::io::Result<()> {
    let startup = loop {
        let body = untagged(&mut socket).await?;
        match be32(&body, 0) {
            CANCEL_REQUEST_CODE => {
                Seen::push(&seen.cancels, (be32(&body, 4), be32(&body, 8)));
                return Ok(());
            }
            SSL_REQUEST_CODE => {
                if let Ok(mut n) = seen.ssl_requests.lock() {
                    *n += 1;
                }
                socket.write_all(b"N").await?;
            }
            _ => break body,
        }
    };
    let (version, params) = startup_params(&startup);
    let user = params.get("user").cloned().unwrap_or_default();
    Seen::push(&seen.startups, params);
    Seen::push(&seen.versions, version);

    let request = match &script.auth {
        AuthPlan::Trust => None,
        AuthPlan::Md5 { salt } => Some(msg::auth_md5(*salt)),
        AuthPlan::Sha512 { salt, user_salt } => Some(msg::auth_sha512(66_048, *salt, *user_salt)),
        AuthPlan::Cleartext => Some(msg::auth(3, &[])),
        AuthPlan::OAuth => Some(msg::auth(12, &[])),
        AuthPlan::Code(code) => Some(msg::auth(*code, &[])),
    };
    if let Some(request) = request {
        socket.write_all(&request).await?;
        let (tag, body) = tagged(&mut socket).await?;
        let answer = body.strip_suffix(&[0]).unwrap_or(&body).to_vec();
        Seen::push(&seen.passwords, answer.clone());
        let pw = script.password.as_bytes();
        let want = match &script.auth {
            AuthPlan::Md5 { salt } => auth::md5_response(pw, user.as_bytes(), salt).into_bytes(),
            AuthPlan::Sha512 { salt, user_salt } => {
                auth::sha512_response(pw, user_salt, salt).into_bytes()
            }
            _ => pw.to_vec(),
        };
        if tag != b'p' || answer != want {
            let e = msg::error(&[
                (b'S', "FATAL"),
                (b'C', "28000"),
                (b'M', &format!("Invalid username or password for {user}")),
                (b'V', "3781"),
            ]);
            socket.write_all(&e).await?;
            return Ok(());
        }
    }
    let mut hello = msg::auth_ok();
    if let Some(v) = script.protocol_version {
        hello.extend(msg::parameter_status("protocol_version", &v.to_string()));
    }
    hello.extend(msg::parameter_status("server_version", "v24.1.0-0"));
    hello.extend(msg::notice(&[(b'S', "NOTICE"), (b'M', "welcome")]));
    hello.extend(msg::key_data(pid, key_of(pid)));
    hello.extend(msg::ready(b'I'));
    socket.write_all(&hello).await?;

    let mut prepared = String::new();
    let mut bound: Option<Request> = None;
    loop {
        let (tag, body) = match tagged(&mut socket).await {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };
        let request = match tag {
            b'X' => {
                if let Ok(mut n) = seen.terminated.lock() {
                    *n += 1;
                }
                return Ok(());
            }
            b'Q' => Request::Query(cstrs(&body, 0).remove(0)),
            b'P' => {
                prepared = cstrs(&body, 0).get(1).cloned().unwrap_or_default();
                continue;
            }
            b'D' | b'E' => continue,
            b'H' => Request::Prepare(prepared.clone()),
            b'B' => {
                bound = Some(bind_request(&prepared, &body));
                continue;
            }
            b'S' => bound.take().unwrap_or(Request::Sync),
            b'f' => Request::CopyRefused(cstrs(&body, 0).remove(0)),
            // CopyError: file name, line, function, then the message.
            b'e' => Request::CopyRefused(cstrs(&body, 6).remove(0)),
            _ => continue,
        };
        Seen::push(&seen.requests, request.clone());
        match (script.handler)(request) {
            Reply::Bytes(b) => socket.write_all(&b).await?,
            Reply::Close => return Ok(()),
            Reply::Hang(b) => {
                socket.write_all(&b).await?;
                let me = (pid, key_of(pid));
                let mut waited = Duration::ZERO;
                while !seen.cancels().contains(&me) {
                    if waited > Duration::from_secs(10) {
                        return Ok(());
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    waited += Duration::from_millis(5);
                }
                let mut out = msg::error(&[
                    (b'S', "ERROR"),
                    (b'C', "57014"),
                    (b'M', "Execution canceled by operator"),
                    (b'V', "5196"),
                ]);
                out.extend(msg::ready(b'I'));
                socket.write_all(&out).await?;
            }
        }
    }
}
