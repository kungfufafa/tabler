//! One Vertica connection over a byte stream: the startup message and the login, simple queries
//! and prepared statements with their rows decoded to `Value`, COPY LOCAL refused, and the
//! CancelRequest a second connection sends.
//!
//! The stream is the caller's: a socket, the rustls stream after `negotiate_tls` said yes, or an
//! SSH tunnel's port (the session layer of `entry.rs` picks it, Phase B). Nothing here keeps a
//! time limit either: the caller wraps a call in its timeout and cancels the statement on the
//! server through `send_cancel`.
//!
//! A connection whose stream failed or whose server broke the protocol is broken: every later
//! call fails with `NotConnected` without touching the stream.

use std::collections::BTreeMap;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, BufReader};

use super::auth::{self, Credential};
use super::decode;
use super::types;
use super::wire::{
    self, Backend, BackendKey, COMPLEX_TYPES_VERSION, Field, ParamType, ServerMessage, Target,
    TxStatus,
};
use crate::error::{AppError, ErrorPos};
use crate::types::value::Value;

/// What the client tells the server about itself at startup (vertica-python's `Startup`).
#[derive(Debug, Clone)]
pub struct StartupOptions {
    pub user: String,
    pub database: String,
    /// The session's label (`v_monitor.sessions.client_label`).
    pub client_label: String,
    /// The workload a routing rule sends the session by; None: the server's default.
    pub workload: Option<String>,
    /// Every statement commits on its own (pool connections); off, the editor's statements
    /// wait for a COMMIT.
    pub autocommit: bool,
    /// Ask for ROW / ARRAY / MAP columns to be described field by field (protocol 3.12+).
    pub request_complex_types: bool,
    pub client_version: String,
    pub client_os: String,
    pub client_os_user: String,
    pub client_hostname: String,
    pub client_pid: u32,
}

impl StartupOptions {
    pub fn new(user: &str, database: &str) -> StartupOptions {
        let env = |names: &[&str]| {
            names
                .iter()
                .find_map(|n| std::env::var(n).ok())
                .unwrap_or_default()
        };
        StartupOptions {
            user: user.into(),
            database: database.into(),
            client_label: "Tabler".into(),
            workload: None,
            autocommit: true,
            request_complex_types: true,
            client_version: env!("CARGO_PKG_VERSION").into(),
            client_os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            client_os_user: env(&["USER", "USERNAME"]),
            client_hostname: env(&["HOSTNAME", "COMPUTERNAME"]),
            client_pid: std::process::id(),
        }
    }

    /// The startup message's parameters. An OAuth token goes in them too (servers of protocol
    /// 3.11 read it there rather than from the password message).
    pub fn params(&self, credential: &Credential) -> Vec<(&'static str, String)> {
        let on = |b: bool| if b { "on" } else { "off" }.to_string();
        let mut out = vec![
            ("user", self.user.clone()),
            ("database", self.database.clone()),
            ("client_label", self.client_label.clone()),
            ("client_type", "Tabler".to_string()),
            ("client_version", self.client_version.clone()),
            ("client_os", self.client_os.clone()),
            ("client_os_user_name", self.client_os_user.clone()),
            ("client_os_hostname", self.client_hostname.clone()),
            ("client_pid", self.client_pid.to_string()),
            ("autocommit", on(self.autocommit)),
            // Text results: the decoders read both, but text is what every server sends alike.
            ("binary_data_protocol", "0".to_string()),
            (
                "protocol_features",
                format!(
                    "{{\"request_complex_types\":{}}}",
                    self.request_complex_types
                ),
            ),
            ("protocol_compat", "VER".to_string()),
        ];
        if let Some(w) = self.workload.as_ref().filter(|w| !w.is_empty()) {
            out.push(("workload", w.clone()));
        }
        if let Credential::OAuthToken(token) = credential {
            out.push(("oauth_access_token", token.clone()));
            out.push(("auth_category", "OAuth".to_string()));
        }
        out
    }
}

/// Asks the server for TLS on a fresh stream: true when it agreed (`S`; the caller then runs the
/// TLS handshake over the same stream), false when it refused (`N`).
pub async fn negotiate_tls<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
) -> Result<bool, AppError> {
    let io = |e: std::io::Error| AppError::Connect(format!("Vertica: {e}"));
    stream.write_all(&wire::ssl_request()).await.map_err(io)?;
    stream.flush().await.map_err(io)?;
    match stream.read_u8().await.map_err(io)? {
        b'S' => Ok(true),
        b'N' => Ok(false),
        other => Err(AppError::Connect(format!(
            "Vertica answered the TLS request with {:?}: is this a Vertica server?",
            other as char
        ))),
    }
}

/// Sends a CancelRequest for the session `key` over `stream`, a new connection to the same server,
/// and waits until the server hangs up, which it does once it has the request. The server answers
/// the statement it stopped with `57014` on the session's own connection.
pub async fn send_cancel<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    key: BackendKey,
) -> Result<(), AppError> {
    let io = |e: std::io::Error| AppError::Connect(format!("Vertica cancel request: {e}"));
    stream
        .write_all(&wire::cancel_request(key))
        .await
        .map_err(io)?;
    stream.flush().await.map_err(io)?;
    let mut rest = Vec::new();
    // Whatever the server sends before closing is ignored; a reset is a close too.
    let _ = stream.read_to_end(&mut rest).await;
    Ok(())
}

/// What a run hands its sink, in order: per statement its columns (when it returns rows), its
/// rows, then `Done` with its command tag.
#[derive(Debug, PartialEq)]
pub enum Item<'a> {
    Columns(&'a [Field]),
    Row(Vec<Value>),
    /// The command tag (`SELECT`, `INSERT`, `COMMIT`; empty for an empty statement).
    Done(&'a str),
}

/// Whether the sink wants more rows. After `Stop` the rest of the run is read and dropped (the
/// server cannot be stopped without a cancel), so the connection stays usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop,
}

/// How a run ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// The command tag of each statement that finished.
    pub tags: Vec<String>,
    /// The sink stopped it.
    pub stopped: bool,
}

pub type Sink<'s> = dyn FnMut(Item<'_>) -> Flow + Send + 's;

/// Rows of one statement, collected.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rows {
    pub fields: Vec<Field>,
    pub rows: Vec<Vec<Value>>,
    pub tag: String,
}

/// The rows affected by a DML statement: Vertica answers INSERT, UPDATE, DELETE and MERGE with
/// one row of one column `OUTPUT` holding the count (and a bare tag); a tag with a count (`COPY
/// 3`) is read too.
pub fn affected(result: &Rows) -> Option<u64> {
    if let Some(n) = result
        .tag
        .rsplit(' ')
        .next()
        .filter(|_| result.tag.contains(' '))
        .and_then(|n| n.parse().ok())
    {
        return Some(n);
    }
    match (result.fields.as_slice(), result.rows.as_slice()) {
        ([f], [row]) if f.name.eq_ignore_ascii_case("OUTPUT") => match row.first()? {
            Value::Int(n) => u64::try_from(*n).ok(),
            Value::BigInt(s) | Value::Decimal(s) => s.parse().ok(),
            _ => None,
        },
        _ => None,
    }
}

/// The text a bind parameter of type `oid` carries (None: NULL). A binary parameter reads its
/// text like a VARBINARY literal, so its backslashes get octal escapes (`decode::escape_binary`),
/// as vertica-python does; any other type takes the bytes as they are (a VARCHAR cell that was
/// not UTF-8 comes back as bytes and goes back unchanged).
pub fn param_text(value: &Value, oid: u32) -> Result<Option<Vec<u8>>, AppError> {
    Ok(Some(match value {
        Value::Null => return Ok(None),
        Value::Default => {
            return Err(AppError::InvalidInput(
                "DEFAULT cannot be a bind parameter".into(),
            ));
        }
        Value::Bool(b) => if *b { "true" } else { "false" }.into(),
        Value::Bytes(b) => {
            if b.truncated {
                return Err(AppError::InvalidInput(
                    "a shortened preview of a binary value cannot be written back; load the \
                     whole value first"
                        .into(),
                ));
            }
            use base64::Engine as _;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(&b.base64)
                .map_err(|e| AppError::InvalidInput(format!("binary value: {e}")))?;
            if matches!(oid, types::BINARY | types::VARBINARY | types::LONGVARBINARY) {
                decode::escape_binary(&raw)
            } else {
                raw
            }
        }
        Value::Array(_) => {
            return Err(AppError::Unsupported(
                "Vertica: array values cannot be bind parameters".into(),
            ));
        }
        other => other.as_text().unwrap_or_default().into_bytes(),
    }))
}

/// A server error as the app reports it: a cancelled statement is `Cancelled`, a refused login
/// `Auth`, anything else `Db` with Vertica's own code as `vendor_code` and the position mapped
/// into `sql`.
pub fn server_error(
    m: ServerMessage,
    sql: Option<&str>,
    statement_index: Option<usize>,
) -> AppError {
    let state = m.sqlstate.as_deref().unwrap_or_default();
    if state == "57014" {
        return AppError::Cancelled;
    }
    if state.starts_with("28") {
        return AppError::Auth(m.message);
    }
    let position = match (m.position, sql) {
        (Some(p), Some(sql)) if p >= 1 => Some(ErrorPos::from_offset(sql, p as usize - 1)),
        _ => None,
    };
    AppError::Db {
        message: m.message,
        detail: m.detail,
        hint: m.hint,
        sqlstate: m.sqlstate,
        vendor_code: m.error_code,
        position,
        statement_index,
    }
}

/// What the server reported at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hello {
    pub parameters: BTreeMap<String, String>,
    pub key: Option<BackendKey>,
    /// The protocol version in effect (3.5 when the server did not say).
    pub protocol: u32,
}

/// A statement prepared by `prepare`.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub params: Vec<ParamType>,
    /// None: it returns no rows.
    pub fields: Option<Vec<Field>>,
    /// The command it is (`SELECT`, `INSERT`…).
    pub command: String,
}

pub struct Connection<S> {
    io: BufReader<S>,
    hello: Hello,
    complex: bool,
    tx: TxStatus,
    notices: Vec<ServerMessage>,
    broken: bool,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> Connection<S> {
    /// Logs in over `stream`. `tls`: the stream is encrypted, which a cleartext password needs.
    pub async fn startup(
        stream: S,
        options: &StartupOptions,
        credential: &Credential,
        tls: bool,
    ) -> Result<Connection<S>, AppError> {
        let mut conn = Connection {
            io: BufReader::new(stream),
            hello: Hello {
                protocol: wire::STARTUP_VERSION,
                ..Hello::default()
            },
            complex: false,
            tx: TxStatus::Idle,
            notices: Vec::new(),
            broken: false,
        };
        let params = options.params(credential);
        let params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let startup = wire::startup(&params, wire::PROTOCOL_VERSION)
            .map_err(|e| AppError::InvalidInput(e.0))?;
        conn.send(&startup).await.map_err(during_startup)?;
        loop {
            match conn.next().await.map_err(during_startup)? {
                Backend::Auth(request) => {
                    if let Some(answer) = auth::response(&request, &options.user, credential, tls)?
                    {
                        let message = wire::password(&answer).map_err(|e| AppError::Auth(e.0))?;
                        conn.send(&message).await.map_err(during_startup)?;
                    }
                }
                Backend::ParameterStatus { name, value } => conn.parameter(name, value),
                Backend::BackendKey(key) => conn.hello.key = Some(key),
                Backend::Notice(n) => conn.notices.push(n),
                Backend::Error(e) => {
                    conn.broken = true;
                    return Err(match server_error(e, None, None) {
                        AppError::Db { message, .. } => AppError::Connect(message),
                        other => other,
                    });
                }
                Backend::Ready(tx) => {
                    conn.tx = tx;
                    break;
                }
                other => {
                    conn.broken = true;
                    return Err(AppError::Connect(format!(
                        "Vertica sent {other:?} during the login"
                    )));
                }
            }
        }
        conn.complex =
            options.request_complex_types && conn.hello.protocol >= COMPLEX_TYPES_VERSION;
        Ok(conn)
    }

    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    pub fn key(&self) -> Option<BackendKey> {
        self.hello.key
    }

    /// The transaction state of the last ReadyForQuery.
    pub fn tx_status(&self) -> TxStatus {
        self.tx
    }

    pub fn in_transaction(&self) -> bool {
        self.tx != TxStatus::Idle
    }

    pub fn is_broken(&self) -> bool {
        self.broken
    }

    /// The notices the server sent since the last call.
    pub fn take_notices(&mut self) -> Vec<ServerMessage> {
        std::mem::take(&mut self.notices)
    }

    fn parameter(&mut self, name: String, value: String) {
        if name == "protocol_version"
            && let Ok(v) = value.trim().parse()
        {
            self.hello.protocol = v;
        }
        self.hello.parameters.insert(name, value);
    }

    fn lost(&mut self, e: &std::io::Error) -> AppError {
        self.broken = true;
        log::debug!("Vertica connection lost: {e}");
        AppError::NotConnected
    }

    async fn send(&mut self, bytes: &[u8]) -> Result<(), AppError> {
        if self.broken {
            return Err(AppError::NotConnected);
        }
        let mut res = self.io.get_mut().write_all(bytes).await;
        if res.is_ok() {
            res = self.io.get_mut().flush().await;
        }
        res.map_err(|e| self.lost(&e))
    }

    async fn next(&mut self) -> Result<Backend, AppError> {
        if self.broken {
            return Err(AppError::NotConnected);
        }
        let frame = match wire::read_frame(&mut self.io).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                self.broken = true;
                return Err(AppError::Internal(e.to_string()));
            }
            Err(e) => return Err(self.lost(&e)),
        };
        Backend::parse(frame, self.complex).map_err(|e| {
            self.broken = true;
            AppError::Internal(e.to_string())
        })
    }

    /// Runs `sql` (one statement or several) as a simple query, handing what it returns to
    /// `sink`. The first error stops the run (the server skips the statements after it) and is
    /// returned with the index of its statement, once the connection is ready again.
    pub async fn simple(&mut self, sql: &str, sink: &mut Sink<'_>) -> Result<Summary, AppError> {
        let message = wire::query(sql).map_err(|e| AppError::InvalidInput(e.0))?;
        self.send(&message).await?;
        self.results(sql, None, sink).await
    }

    /// Prepares `sql` as the unnamed statement: its parameters and result columns.
    pub async fn prepare(&mut self, sql: &str) -> Result<Prepared, AppError> {
        let mut out = wire::parse("", sql).map_err(|e| AppError::InvalidInput(e.0))?;
        out.extend(wire::describe(Target::Statement, "").map_err(|e| AppError::InvalidInput(e.0))?);
        out.extend(wire::flush());
        self.send(&out).await?;
        let mut params = Vec::new();
        let mut fields = None;
        loop {
            match self.next().await? {
                Backend::ParseComplete => {}
                Backend::ParameterDescription(p) => params = p,
                Backend::RowDescription(f) => fields = Some(f),
                Backend::NoData => fields = None,
                Backend::CommandDescription { tag, .. } => {
                    if tag.is_empty() {
                        self.resync().await?;
                        return Err(AppError::InvalidInput("the statement is empty".into()));
                    }
                    return Ok(Prepared {
                        params,
                        fields,
                        command: tag,
                    });
                }
                Backend::Notice(n) => self.notices.push(n),
                Backend::ParameterStatus { name, value } => self.parameter(name, value),
                Backend::Error(e) => {
                    let err = server_error(e, Some(sql), None);
                    self.resync().await?;
                    return Err(err);
                }
                other => {
                    self.broken = true;
                    return Err(AppError::Internal(format!(
                        "Vertica sent {other:?} while preparing a statement"
                    )));
                }
            }
        }
    }

    /// After a failed prepare (the server skips everything up to a Sync): Sync, then read to
    /// ReadyForQuery.
    async fn resync(&mut self) -> Result<(), AppError> {
        self.send(&wire::sync()).await?;
        loop {
            match self.next().await? {
                Backend::Ready(tx) => {
                    self.tx = tx;
                    return Ok(());
                }
                Backend::Notice(n) => self.notices.push(n),
                _ => {}
            }
        }
    }

    /// Runs one statement with bind parameters (`?`): prepare, bind as text with the types the
    /// server inferred, execute, sync.
    pub async fn execute(
        &mut self,
        sql: &str,
        params: &[Value],
        sink: &mut Sink<'_>,
    ) -> Result<Summary, AppError> {
        let prepared = self.prepare(sql).await?;
        if prepared.params.len() != params.len() {
            self.resync().await?;
            return Err(AppError::InvalidInput(format!(
                "the statement takes {} parameters, {} were given",
                prepared.params.len(),
                params.len()
            )));
        }
        let oids: Vec<u32> = prepared.params.iter().map(|p| p.ty.oid).collect();
        let values = match params
            .iter()
            .zip(&oids)
            .map(|(v, &oid)| param_text(v, oid))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(v) => v,
            Err(e) => {
                self.resync().await?;
                return Err(e);
            }
        };
        let mut out = match wire::bind("", "", &values, &oids, false) {
            Ok(b) => b,
            Err(e) => {
                self.resync().await?;
                return Err(AppError::InvalidInput(e.0));
            }
        };
        out.extend(wire::execute("", 0).map_err(|e| AppError::InvalidInput(e.0))?);
        out.extend(wire::sync());
        self.send(&out).await?;
        self.results(sql, prepared.fields, sink).await
    }

    /// `simple` without parameters, `execute` with them.
    pub async fn run(
        &mut self,
        sql: &str,
        params: &[Value],
        sink: &mut Sink<'_>,
    ) -> Result<Summary, AppError> {
        if params.is_empty() {
            self.simple(sql, sink).await
        } else {
            self.execute(sql, params, sink).await
        }
    }

    /// Every statement's rows, collected (catalog queries and other small results).
    pub async fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Rows>, AppError> {
        let mut out: Vec<Rows> = Vec::new();
        let mut current = Rows::default();
        let mut sink = |item: Item<'_>| {
            match item {
                Item::Columns(f) => current.fields = f.to_vec(),
                Item::Row(r) => current.rows.push(r),
                Item::Done(tag) => {
                    let mut done = std::mem::take(&mut current);
                    done.tag = tag.to_string();
                    out.push(done);
                }
            }
            Flow::Continue
        };
        self.run(sql, params, &mut sink).await?;
        Ok(out)
    }

    /// The rows of the last statement of `sql`.
    pub async fn rows(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Vec<Value>>, AppError> {
        Ok(self
            .query(sql, params)
            .await?
            .pop()
            .map(|r| r.rows)
            .unwrap_or_default())
    }

    /// Reads a run's messages up to ReadyForQuery. `fields`: the columns a prepare described
    /// (the extended protocol sends no RowDescription with the rows).
    async fn results(
        &mut self,
        sql: &str,
        mut fields: Option<Vec<Field>>,
        sink: &mut Sink<'_>,
    ) -> Result<Summary, AppError> {
        let mut summary = Summary::default();
        let mut error: Option<AppError> = None;
        let mut announced = false;
        loop {
            let message = self.next().await?;
            let live = error.is_none() && !summary.stopped;
            match message {
                Backend::RowDescription(f) => {
                    fields = Some(f);
                    announced = false;
                }
                Backend::DataRow(row) => {
                    if !live {
                        continue;
                    }
                    let described = fields.as_deref().unwrap_or_default();
                    if !announced {
                        announced = true;
                        if sink(Item::Columns(described)) == Flow::Stop {
                            summary.stopped = true;
                            continue;
                        }
                    }
                    let values = decode::decode_row(described, &row);
                    if sink(Item::Row(values)) == Flow::Stop {
                        summary.stopped = true;
                    }
                }
                Backend::CommandComplete(tag) => {
                    if live {
                        if !announced && let Some(f) = fields.as_deref() {
                            // A query that returned no rows still has its columns.
                            let _ = sink(Item::Columns(f));
                        }
                        let _ = sink(Item::Done(&tag));
                    }
                    summary.tags.push(tag);
                    fields = None;
                    announced = false;
                }
                Backend::EmptyQuery => {
                    if live {
                        let _ = sink(Item::Done(""));
                    }
                    summary.tags.push(String::new());
                    fields = None;
                    announced = false;
                }
                Backend::Error(e) => {
                    if error.is_none() {
                        error = Some(server_error(e, Some(sql), Some(summary.tags.len())));
                    }
                }
                Backend::Notice(n) => self.notices.push(n),
                Backend::ParameterStatus { name, value } => self.parameter(name, value),
                // COPY … FROM STDIN and COPY LOCAL: Tabler sends no files (v1). The server
                // answers the refusal with an error, then ReadyForQuery.
                Backend::CopyIn => {
                    let m = wire::copy_fail("Tabler does not send data to COPY FROM STDIN")
                        .map_err(|e| AppError::Internal(e.0))?;
                    self.send(&m).await?;
                }
                Backend::LoadFile(_) | Backend::VerifyFiles => {
                    let m = wire::copy_error("Tabler does not send local files to COPY LOCAL")
                        .map_err(|e| AppError::Internal(e.0))?;
                    self.send(&m).await?;
                }
                Backend::Ready(tx) => {
                    self.tx = tx;
                    return match error {
                        Some(e) => Err(e),
                        None => Ok(summary),
                    };
                }
                // Steps of the extended protocol and messages a run may carry that change
                // nothing here.
                Backend::BindComplete
                | Backend::ParseComplete
                | Backend::CloseComplete
                | Backend::NoData
                | Backend::PortalSuspended
                | Backend::ParameterDescription(_)
                | Backend::CommandDescription { .. }
                | Backend::CopyDone
                | Backend::EndOfBatch
                | Backend::WriteFile
                | Backend::LoadBalance { .. }
                | Backend::Unknown(_) => {}
                other @ (Backend::Auth(_) | Backend::BackendKey(_)) => {
                    self.broken = true;
                    return Err(AppError::Internal(format!(
                        "Vertica sent {other:?} during a query"
                    )));
                }
            }
        }
    }

    /// Ends the session politely (Terminate) and shuts the stream down.
    pub async fn terminate(&mut self) {
        if !self.broken {
            let _ = self.send(&wire::terminate()).await;
            let _ = self.io.get_mut().shutdown().await;
        }
        self.broken = true;
    }
}

/// A transport failure while logging in is a failed connection, not a lost one.
fn during_startup(e: AppError) -> AppError {
    match e {
        AppError::NotConnected => {
            AppError::Connect("Vertica closed the connection during the login".into())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::drivers::vertica::testing::{self, AuthPlan, Reply, Request, Script, msg};

    /// The fake reports protocol 3.16, so a RowDescription's fields carry their parent's
    /// attribute number.
    const COMPLEX: bool = true;
    const SALT: [u8; 4] = [1, 2, 3, 4];
    const USER_SALT: [u8; 16] = [7; 16];

    fn pw(p: &str) -> Credential {
        Credential::Password(p.into())
    }

    async fn login(
        script: Script,
        credential: &Credential,
    ) -> (
        Result<Connection<tokio::net::TcpStream>, AppError>,
        testing::Seen,
        u16,
    ) {
        let (port, seen) = testing::serve(script).await;
        let stream = testing::connect(port).await;
        let options = StartupOptions::new("dbadmin", "VMart");
        let conn = Connection::startup(stream, &options, credential, false).await;
        (conn, seen, port)
    }

    fn collect() -> (Arc<Mutex<Vec<String>>>, impl FnMut(Item<'_>) -> Flow + Send) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = Arc::clone(&log);
        let sink = move |item: Item<'_>| {
            let line = match item {
                Item::Columns(f) => format!(
                    "columns {}",
                    f.iter()
                        .map(|f| f.name.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                Item::Row(r) => format!("row {r:?}"),
                Item::Done(t) => format!("done {t}"),
            };
            l.lock().unwrap().push(line);
            Flow::Continue
        };
        (log, sink)
    }

    #[tokio::test]
    async fn logs_in_with_the_sha512_hash() {
        let script = Script::new(
            AuthPlan::Sha512 {
                salt: SALT,
                user_salt: USER_SALT,
            },
            "s3cret",
            |r| match r {
                Request::Query(_) => Reply::Bytes(msg::select(&["v"], &[&["a"]], COMPLEX)),
                _ => Reply::Close,
            },
        );
        let (conn, seen, _) = login(script, &pw("s3cret")).await;
        let mut conn = conn.unwrap();
        assert_eq!(
            seen.passwords(),
            [auth::sha512_response(b"s3cret", &USER_SALT, &SALT).into_bytes()]
        );
        let start = &seen.startups()[0];
        assert_eq!(start["user"], "dbadmin");
        assert_eq!(start["database"], "VMart");
        assert_eq!(start["autocommit"], "on");
        assert_eq!(start["binary_data_protocol"], "0");
        assert_eq!(
            start["protocol_features"],
            "{\"request_complex_types\":true}"
        );
        assert_eq!(start["client_type"], "Tabler");
        assert!(!start.contains_key("oauth_access_token"));
        assert_eq!(seen.versions.lock().unwrap()[0], wire::PROTOCOL_VERSION);
        assert_eq!(conn.hello().protocol, wire::PROTOCOL_VERSION);
        assert_eq!(
            conn.hello()
                .parameters
                .get("server_version")
                .map(String::as_str),
            Some("v24.1.0-0")
        );
        let key = conn.key().unwrap();
        assert_eq!(key.key, testing::key_of(key.pid));
        assert!(conn.complex && !conn.in_transaction());
        // At 3.16 a RowDescription's fields carry their parent's attribute number.
        let rows = conn.rows("SELECT v FROM t", &[]).await.unwrap();
        assert_eq!(rows, [vec![Value::Text("a".into())]]);
    }

    #[tokio::test]
    async fn md5_cleartext_oauth_and_refusals() {
        let md5 = Script::new(AuthPlan::Md5 { salt: SALT }, "pw", |_| Reply::Close);
        let (conn, seen, _) = login(md5.clone(), &pw("pw")).await;
        assert!(conn.is_ok());
        assert_eq!(
            seen.passwords()[0],
            auth::md5_response(b"pw", b"dbadmin", &SALT).into_bytes()
        );
        // A wrong password: the server's 28000 is an authentication failure.
        let (conn, _, _) = login(md5, &pw("nope")).await;
        assert!(
            matches!(conn, Err(AppError::Auth(ref m)) if m.contains("Invalid")),
            "{:?}",
            conn.err()
        );

        // Cleartext only over TLS: without it the client hangs up and sends nothing.
        let clear = Script::new(AuthPlan::Cleartext, "pw", |_| Reply::Close);
        let (conn, seen, port) = login(clear, &pw("pw")).await;
        assert!(matches!(conn, Err(AppError::Auth(ref m)) if m.contains("TLS")));
        assert!(seen.passwords().is_empty() || seen.passwords()[0].is_empty());
        let stream = testing::connect(port).await;
        let options = StartupOptions::new("dbadmin", "VMart");
        assert!(
            Connection::startup(stream, &options, &pw("pw"), true)
                .await
                .is_ok()
        );

        // A token answers the OAuth request and rides in the startup message.
        let oauth = Script::new(AuthPlan::OAuth, "tok.en", |_| Reply::Close);
        let (conn, seen, _) = login(oauth, &Credential::OAuthToken("tok.en".into())).await;
        assert!(conn.is_ok());
        assert_eq!(seen.startups()[0]["oauth_access_token"], "tok.en");
        assert_eq!(seen.passwords()[0], b"tok.en");

        // TOTP and Kerberos are refused with a clear message.
        for code in [14, 2] {
            let s = Script::new(AuthPlan::Code(code), "pw", |_| Reply::Close);
            let (conn, _, _) = login(s, &pw("pw")).await;
            assert!(matches!(conn, Err(AppError::Auth(_))), "{code}");
        }
    }

    #[tokio::test]
    async fn an_old_server_keeps_protocol_3_5_without_complex_types() {
        let mut script = Script::new(AuthPlan::Trust, "", |r| match r {
            Request::Query(_) => Reply::Bytes(msg::select(&["v"], &[&["a"], &["NULL"]], false)),
            _ => Reply::Close,
        });
        script.protocol_version = None;
        let (conn, seen, _) = login(script, &pw("")).await;
        let mut conn = conn.unwrap();
        assert_eq!(conn.hello().protocol, wire::STARTUP_VERSION);
        assert!(!conn.complex);
        assert!(seen.passwords().is_empty());
        // Without complex types the fields have no parent's attribute number.
        let rows = conn.rows("SELECT v FROM t", &[]).await.unwrap();
        assert_eq!(rows, [vec![Value::Text("a".into())], vec![Value::Null]]);
    }

    #[tokio::test]
    async fn negotiates_tls_and_cancels() {
        let script = Script::new(AuthPlan::Trust, "", |r| match r {
            Request::Query(_) => Reply::Hang(msg::row_description(
                &[],
                &[msg::column("n", types::INT8, -1)],
                COMPLEX,
            )),
            _ => Reply::Close,
        });
        let (port, seen) = testing::serve(script).await;
        let mut stream = testing::connect(port).await;
        assert!(!negotiate_tls(&mut stream).await.unwrap());
        assert_eq!(*seen.ssl_requests.lock().unwrap(), 1);
        let options = StartupOptions::new("dbadmin", "VMart");
        let mut conn = Connection::startup(stream, &options, &pw(""), false)
            .await
            .unwrap();
        let key = conn.key().unwrap();
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            send_cancel(testing::connect(port).await, key).await
        });
        let (_, mut sink) = collect();
        let err = conn
            .simple("SELECT sleep(100)", &mut sink)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Cancelled), "{err:?}");
        canceller.await.unwrap().unwrap();
        assert_eq!(seen.cancels(), [(key.pid, key.key)]);
        assert!(!conn.is_broken());
    }

    fn table_field(name: &str, attnum: u16, oid: u32, modifier: i32) -> msg::FieldSpec {
        msg::field(
            name,
            45_035_996_273_704_982,
            Some(("public", "t")),
            attnum,
            0,
            false,
            oid,
            -1,
            modifier,
        )
    }

    #[tokio::test]
    async fn simple_queries_stream_typed_rows_and_keep_going_after_an_error() {
        let script = Script::new(AuthPlan::Trust, "", |r| {
            let Request::Query(sql) = r else {
                return Reply::Close;
            };
            let mut out = Vec::new();
            match sql.as_str() {
                "SELECT * FROM t; SELECT 1 WHERE false; ; COMMIT" => {
                    out.extend(msg::row_description(
                        &[],
                        &[
                            table_field("id", 1, types::INT8, -1),
                            table_field("name", 2, types::VARCHAR, 84),
                            table_field("price", 3, types::NUMERIC, (10 << 16 | 2) + 4),
                            table_field("at", 4, types::TIMESTAMPTZ, -1),
                        ],
                        COMPLEX,
                    ));
                    out.extend(msg::text_row(&["1", "a", "1.50", "2024-01-01 00:00:00+07"]));
                    out.extend(msg::text_row(&["9007199254740993", "NULL", "NULL", "NULL"]));
                    out.extend(msg::command_complete("SELECT"));
                    out.extend(msg::row_description(
                        &[],
                        &[msg::column("?column?", types::INT8, -1)],
                        COMPLEX,
                    ));
                    out.extend(msg::command_complete("SELECT"));
                    out.extend(msg::empty_query());
                    out.extend(msg::command_complete("COMMIT"));
                    out.extend(msg::ready(b'I'));
                }
                "SELECT 1; SELECT nope; SELECT 3" => {
                    out.extend(msg::row_description(
                        &[],
                        &[msg::column("x", types::INT8, -1)],
                        COMPLEX,
                    ));
                    out.extend(msg::text_row(&["1"]));
                    out.extend(msg::command_complete("SELECT"));
                    out.extend(msg::notice(&[(b'S', "NOTICE"), (b'M', "careful")]));
                    out.extend(msg::error(&[
                        (b'S', "ERROR"),
                        (b'C', "42703"),
                        (b'M', "Column \"nope\" does not exist"),
                        (b'P', "18"),
                        (b'V', "2624"),
                    ]));
                    out.extend(msg::ready(b'I'));
                }
                "BEGIN" => {
                    out.extend(msg::command_complete("BEGIN"));
                    out.extend(msg::ready(b'T'));
                }
                _ => return Reply::Close,
            }
            Reply::Bytes(out)
        });
        let (conn, _, _) = login(script, &pw("")).await;
        let mut conn = conn.unwrap();
        let (log, mut sink) = collect();
        let summary = conn
            .simple("SELECT * FROM t; SELECT 1 WHERE false; ; COMMIT", &mut sink)
            .await
            .unwrap();
        assert_eq!(summary.tags, ["SELECT", "SELECT", "", "COMMIT"]);
        assert_eq!(
            *log.lock().unwrap(),
            [
                "columns id,name,price,at".to_string(),
                format!(
                    "row {:?}",
                    vec![
                        Value::Int(1),
                        Value::Text("a".into()),
                        Value::Decimal("1.50".into()),
                        Value::TimestampTz("2024-01-01 00:00:00+07".into())
                    ]
                ),
                format!(
                    "row {:?}",
                    vec![
                        Value::BigInt("9007199254740993".into()),
                        Value::Null,
                        Value::Null,
                        Value::Null
                    ]
                ),
                "done SELECT".into(),
                "columns ?column?".into(),
                "done SELECT".into(),
                "done ".into(),
                "done COMMIT".into(),
            ]
        );

        let (_, mut sink) = collect();
        let err = conn
            .simple("SELECT 1; SELECT nope; SELECT 3", &mut sink)
            .await
            .unwrap_err();
        let AppError::Db {
            sqlstate,
            vendor_code,
            position,
            statement_index,
            message,
            ..
        } = err
        else {
            panic!("not a server error: {err:?}")
        };
        assert_eq!(sqlstate.as_deref(), Some("42703"));
        assert_eq!(vendor_code, Some(2624));
        assert_eq!(statement_index, Some(1));
        assert!(message.contains("nope"));
        let position = position.unwrap();
        assert_eq!(
            (position.offset, position.line, position.column),
            (17, 1, 18)
        );
        assert_eq!(conn.take_notices().len(), 2, "the welcome and the warning");

        // The connection is still in step: a transaction shows in the status.
        let (_, mut sink) = collect();
        conn.simple("BEGIN", &mut sink).await.unwrap();
        assert!(conn.in_transaction());
    }

    #[tokio::test]
    async fn a_stopped_sink_drops_the_rest_and_stays_in_step() {
        let script = Script::new(AuthPlan::Trust, "", |r| match r {
            Request::Query(_) => {
                let mut out =
                    msg::row_description(&[], &[msg::column("n", types::INT8, -1)], COMPLEX);
                for i in 0..5 {
                    out.extend(msg::text_row(&[&i.to_string()]));
                }
                out.extend(msg::command_complete("SELECT"));
                out.extend(msg::ready(b'I'));
                Reply::Bytes(out)
            }
            _ => Reply::Close,
        });
        let (conn, _, _) = login(script, &pw("")).await;
        let mut conn = conn.unwrap();
        let mut rows = 0;
        let mut sink = |item: Item<'_>| match item {
            Item::Row(_) => {
                rows += 1;
                if rows == 2 {
                    Flow::Stop
                } else {
                    Flow::Continue
                }
            }
            _ => Flow::Continue,
        };
        let summary = conn.simple("SELECT n FROM t", &mut sink).await.unwrap();
        assert!(summary.stopped);
        assert_eq!(rows, 2);
        let again = conn.rows("SELECT n FROM t", &[]).await.unwrap();
        assert_eq!(again.len(), 5);
    }

    #[tokio::test]
    async fn prepared_statements_bind_text_with_the_inferred_types() {
        let script = Script::new(AuthPlan::Trust, "", |r| {
            let mut out = Vec::new();
            match r {
                Request::Prepare(sql) if sql.starts_with("UPDATE") => {
                    out.extend(msg::parse_complete());
                    out.extend(msg::parameter_description(
                        &[(0, types::VARBINARY, -1, false), (0, types::INT8, -1, true)],
                        &[],
                    ));
                    out.extend(msg::no_data());
                    out.extend(msg::command_description("UPDATE", None));
                }
                Request::Prepare(sql) if sql.starts_with("SELECT") => {
                    out.extend(msg::parse_complete());
                    out.extend(msg::parameter_description(
                        &[(0, types::VARCHAR, -1, false)],
                        &[],
                    ));
                    out.extend(msg::row_description(
                        &[],
                        &[msg::column("table_name", types::VARCHAR, -1)],
                        COMPLEX,
                    ));
                    out.extend(msg::command_description("SELECT", None));
                }
                Request::Prepare(_) => {
                    out.extend(msg::simple_error(
                        "42601",
                        "Syntax error at or near \"x\"",
                        "4856",
                    ));
                }
                Request::Execute { sql, .. } if sql.starts_with("UPDATE") => {
                    out.extend(msg::bind_complete());
                    out.extend(msg::row_description(
                        &[],
                        &[msg::column("OUTPUT", types::INT8, -1)],
                        COMPLEX,
                    ));
                    out.extend(msg::text_row(&["3"]));
                    out.extend(msg::command_complete("UPDATE"));
                    out.extend(msg::ready(b'I'));
                }
                Request::Execute { .. } => {
                    out.extend(msg::bind_complete());
                    out.extend(msg::text_row(&["t1"]));
                    out.extend(msg::text_row(&["t2"]));
                    out.extend(msg::command_complete("SELECT"));
                    out.extend(msg::ready(b'I'));
                }
                Request::Sync => out.extend(msg::ready(b'I')),
                _ => return Reply::Close,
            }
            Reply::Bytes(out)
        });
        let (conn, seen, _) = login(script, &pw("")).await;
        let mut conn = conn.unwrap();

        let results = conn
            .query(
                "UPDATE t SET b = ? WHERE id = ?",
                &[Value::from_bytes(b"a\\b"), Value::Int(7)],
            )
            .await
            .unwrap();
        assert_eq!(affected(&results[0]), Some(3));
        let Request::Execute {
            oids,
            values,
            binary,
            ..
        } = seen.requests()[1].clone()
        else {
            panic!("no execution: {:?}", seen.requests())
        };
        assert_eq!(oids, [types::VARBINARY, types::INT8]);
        assert_eq!(values, [Some(b"a\\134b".to_vec()), Some(b"7".to_vec())]);
        assert!(!binary);

        let rows = conn
            .rows(
                "SELECT table_name FROM v_catalog.tables WHERE table_schema = ?",
                &[Value::Text("public".into())],
            )
            .await
            .unwrap();
        assert_eq!(
            rows,
            [
                vec![Value::Text("t1".into())],
                vec![Value::Text("t2".into())]
            ]
        );

        // A wrong parameter count and a syntax error leave the connection in step.
        let err = conn
            .rows(
                "SELECT table_name FROM v_catalog.tables WHERE table_schema = ?",
                &[Value::Null, Value::Null],
            )
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)), "{err:?}");
        let err = conn.rows("x ?", &[Value::Int(1)]).await.unwrap_err();
        assert!(
            matches!(err, AppError::Db { ref sqlstate, .. } if sqlstate.as_deref() == Some("42601"))
        );
        let err = conn
            .rows(
                "SELECT table_name FROM v_catalog.tables WHERE table_schema = ?",
                &[Value::Default],
            )
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)), "{err:?}");
        let rows = conn
            .rows(
                "SELECT table_name FROM v_catalog.tables WHERE table_schema = ?",
                &[Value::Null],
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(!conn.is_broken());
    }

    #[tokio::test]
    async fn copy_local_is_refused_and_a_lost_server_breaks_the_connection() {
        let script = Script::new(AuthPlan::Trust, "", |r| match r {
            Request::Query(sql) if sql.starts_with("COPY") => {
                Reply::Bytes(msg::load_file("/tmp/data.csv"))
            }
            Request::CopyRefused(m) => {
                let mut out = msg::simple_error("08000", &format!("COPY: {m}"), "2147");
                out.extend(msg::ready(b'I'));
                Reply::Bytes(out)
            }
            _ => Reply::Close,
        });
        let (conn, seen, _) = login(script, &pw("")).await;
        let mut conn = conn.unwrap();
        let err = conn
            .rows("COPY t FROM LOCAL '/tmp/data.csv'", &[])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("COPY LOCAL"), "{err}");
        assert!(matches!(seen.requests()[1], Request::CopyRefused(_)));
        assert!(!conn.is_broken());

        let err = conn.rows("SELECT 1", &[]).await.unwrap_err();
        assert!(matches!(err, AppError::NotConnected), "{err:?}");
        assert!(conn.is_broken());
        assert!(matches!(
            conn.rows("SELECT 1", &[]).await,
            Err(AppError::NotConnected)
        ));
        conn.terminate().await;
    }

    #[tokio::test]
    async fn terminate_says_goodbye() {
        let script = Script::new(AuthPlan::Trust, "", |_| Reply::Close);
        let (conn, seen, _) = login(script, &pw("")).await;
        let mut conn = conn.unwrap();
        conn.terminate().await;
        for _ in 0..200 {
            if *seen.terminated.lock().unwrap() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(*seen.terminated.lock().unwrap(), 1);
        assert!(conn.is_broken());
    }

    #[test]
    fn parameters_affected_counts_and_errors() {
        assert_eq!(
            param_text(&Value::Bool(true), types::BOOL).unwrap(),
            Some(b"true".to_vec())
        );
        assert_eq!(param_text(&Value::Null, types::VARCHAR).unwrap(), None);
        assert_eq!(
            param_text(&Value::Decimal("1.50".into()), types::NUMERIC).unwrap(),
            Some(b"1.50".to_vec())
        );
        assert!(param_text(&Value::Array(vec![]), types::VARCHAR).is_err());
        // Backslashes are escaped for the binary types only: a VARCHAR cell that came back as
        // bytes goes back as it was.
        let slash = Value::from_bytes(b"a\\b\xff");
        for oid in [types::BINARY, types::VARBINARY, types::LONGVARBINARY] {
            assert_eq!(
                param_text(&slash, oid).unwrap(),
                Some(b"a\\134b\xff".to_vec())
            );
        }
        for oid in [
            types::VARCHAR,
            types::LONGVARCHAR,
            types::CHAR,
            types::UNKNOWN,
        ] {
            assert_eq!(param_text(&slash, oid).unwrap(), Some(b"a\\b\xff".to_vec()));
        }
        let mut big = Value::from_bytes(&vec![0u8; crate::types::value::BYTES_PREVIEW_CAP + 1]);
        assert!(param_text(&big, types::VARBINARY).is_err());
        if let Value::Bytes(b) = &mut big {
            b.base64 = "!!".into();
            b.truncated = false;
        }
        assert!(param_text(&big, types::VARBINARY).is_err());

        let rows = |tag: &str| Rows {
            tag: tag.into(),
            ..Rows::default()
        };
        assert_eq!(affected(&rows("COPY 12")), Some(12));
        assert_eq!(affected(&rows("INSERT 0 1")), Some(1));
        assert_eq!(affected(&rows("COMMIT")), None);

        let m = |state: &str| ServerMessage {
            sqlstate: Some(state.into()),
            message: "m".into(),
            position: Some(3),
            error_code: Some(1),
            ..ServerMessage::default()
        };
        assert!(matches!(
            server_error(m("57014"), None, None),
            AppError::Cancelled
        ));
        assert!(matches!(
            server_error(m("28000"), None, None),
            AppError::Auth(_)
        ));
        let AppError::Db { position, .. } = server_error(m("42601"), Some("ab\ncd"), Some(0))
        else {
            panic!()
        };
        assert_eq!(position.map(|p| (p.line, p.column)), Some((1, 3)));
        let options = StartupOptions {
            workload: Some("reporting".into()),
            autocommit: false,
            request_complex_types: false,
            ..StartupOptions::new("u", "d")
        };
        let params = options.params(&pw("x"));
        let get = |k: &str| {
            params
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("workload"), Some("reporting"));
        assert_eq!(get("autocommit"), Some("off"));
        assert_eq!(
            get("protocol_features"),
            Some("{\"request_complex_types\":false}")
        );
        assert_eq!(get("auth_category"), None);
    }
}
