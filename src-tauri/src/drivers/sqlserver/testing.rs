//! A scripted TDS server for the driver's tests, over an in-memory pipe.
//!
//! It answers the prelogin (no encryption) and the login, then hands each request to the test's
//! handler as a `Request` (a SQL batch's text, an `sp_executesql` call's statement and parameters,
//! a transaction-manager request) and writes back the tokens the handler returns, built with the
//! helpers below. `Reply::Hang` writes its tokens and then waits for the client's Attention, like
//! a statement still running; an Attention is acknowledged with `DONE_ATTN` (`Reply::Stuck` never
//! acknowledges it, like a server that stopped answering). Only what the tests need of TDS is
//! here: a few column types, DONE, ERROR.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ::tiberius::{AuthMethod, Client, Config, EncryptionLevel};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

use crate::error::AppError;

const SQL_BATCH: u8 = 0x01;
const RPC: u8 = 0x03;
const TABULAR: u8 = 0x04;
const ATTENTION: u8 = 0x06;
const TRANSACTION_MANAGER: u8 = 0x0E;
const LOGIN7: u8 = 0x10;
const PRELOGIN: u8 = 0x12;

pub const DONE_FINAL: u16 = 0x00;
pub const DONE_MORE: u16 = 0x01;
pub const DONE_ERROR: u16 = 0x02;
pub const DONE_COUNT: u16 = 0x10;
pub const DONE_ATTN: u16 = 0x20;

/// What the client asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Login {
        user: String,
        database: String,
        application: String,
    },
    /// A SQL batch's text.
    Batch(String),
    /// `sp_executesql`: the statement, then each parameter value as text (`NULL`, digits, the
    /// string, `0x…` for bytes).
    Rpc { sql: String, params: Vec<String> },
    /// A transaction-manager request: 5 begin, 7 commit, 8 rollback.
    Transaction(u16),
}

/// How the server answers a request.
pub enum Reply {
    /// These tokens, as the whole answer.
    Tokens(Vec<u8>),
    /// These tokens, then nothing until the client sends an Attention.
    Hang(Vec<u8>),
    /// These tokens, then no answer to anything, the Attention included, until the client hangs
    /// up.
    Stuck(Vec<u8>),
    /// Hang up.
    Close,
}

/// What the server saw.
#[derive(Clone, Default)]
pub struct Seen {
    pub requests: Arc<Mutex<Vec<Request>>>,
    pub attentions: Arc<AtomicUsize>,
    pub ended: Arc<AtomicBool>,
}

impl Seen {
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().map(|r| r.clone()).unwrap_or_default()
    }

    pub fn attentions(&self) -> usize {
        self.attentions.load(Ordering::SeqCst)
    }

    /// The session is over: the client hung up (or the handler did).
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }
}

pub type Handler = Box<dyn FnMut(&Request) -> Reply + Send>;

/// A configuration that reaches the fake server: no TLS, a SQL login, no timeouts, and the
/// session options of the real one.
pub fn config() -> Config {
    let mut config = Config::new();
    config.host("fake.local");
    config.port(1433);
    config.database("shop");
    config.application_name("Tabler");
    config.encryption(EncryptionLevel::NotSupported);
    config.authentication(AuthMethod::sql_server("sa", "pw"));
    config.handshake_timeout(None);
    super::connect::session_options(&mut config);
    config
}

/// Starts a fake server on one end of a pipe; the other end is for the client.
pub fn serve(
    handler: impl FnMut(&Request) -> Reply + Send + 'static,
) -> (Compat<DuplexStream>, Seen) {
    let seen = Seen::default();
    (serve_into(Box::new(handler), seen.clone()), seen)
}

/// `serve`, recording into `seen`.
fn serve_into(handler: Handler, seen: Seen) -> Compat<DuplexStream> {
    let (client, server) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        run(server, handler, seen.clone()).await;
        seen.ended.store(true, Ordering::SeqCst);
    });
    client.compat()
}

/// One fake server for every connection a session opens: one handler answers them all, one log
/// records what they asked.
#[derive(Clone)]
pub struct Server {
    handler: Arc<Mutex<Handler>>,
    pub seen: Seen,
    logins: Arc<AtomicUsize>,
}

impl Server {
    pub fn new(handler: impl FnMut(&Request) -> Reply + Send + 'static) -> Server {
        Server {
            handler: Arc::new(Mutex::new(Box::new(handler))),
            seen: Seen::default(),
            logins: Arc::default(),
        }
    }

    /// Connections opened so far.
    pub fn logins(&self) -> usize {
        self.logins.load(Ordering::SeqCst)
    }

    /// A new connection, logged in (no setup yet).
    pub async fn client(&self) -> Result<Client<Compat<DuplexStream>>, AppError> {
        self.client_with(config()).await
    }

    /// A new connection logged in with `config`, which must not ask for encryption: the fake
    /// speaks no TLS.
    pub async fn client_with(
        &self,
        config: Config,
    ) -> Result<Client<Compat<DuplexStream>>, AppError> {
        self.logins.fetch_add(1, Ordering::SeqCst);
        let shared = Arc::clone(&self.handler);
        let handler: Handler = Box::new(move |r: &Request| match shared.lock() {
            Ok(mut h) => h(r),
            Err(_) => Reply::Close,
        });
        let io = serve_into(handler, self.seen.clone());
        Client::connect(config, io)
            .await
            .map_err(|e| super::errors::map_error(e, super::errors::Phase::Connect, None))
    }
}

/// A client logged in to a fake server (the login answered by the handler too).
pub async fn client(
    handler: impl FnMut(&Request) -> Reply + Send + 'static,
) -> Result<(Client<Compat<DuplexStream>>, Seen), AppError> {
    let (io, seen) = serve(handler);
    let client = Client::connect(config(), io)
        .await
        .map_err(|e| super::errors::map_error(e, super::errors::Phase::Connect, None))?;
    Ok((client, seen))
}

/// The answer most requests get: the login accepted, the session setup run, a bare DONE.
pub fn ok(_: &Request) -> Reply {
    Reply::Tokens(done(DONE_FINAL, 0))
}

async fn run(mut io: DuplexStream, mut handler: Handler, seen: Seen) {
    while let Some((ty, payload)) = read_message(&mut io).await {
        let request = match ty {
            PRELOGIN => {
                if write(&mut io, &prelogin_reply(), true).await.is_err() {
                    return;
                }
                continue;
            }
            ATTENTION => {
                // An Attention with nothing running is acknowledged all the same.
                seen.attentions.fetch_add(1, Ordering::SeqCst);
                if write(&mut io, &done(DONE_ATTN, 0), true).await.is_err() {
                    return;
                }
                continue;
            }
            LOGIN7 => login(&payload),
            SQL_BATCH => batch(&payload),
            RPC => rpc(&payload),
            TRANSACTION_MANAGER => transaction(&payload),
            _ => None,
        };
        let Some(request) = request else {
            return;
        };
        if let Ok(mut log) = seen.requests.lock() {
            log.push(request.clone());
        }
        match handler(&request) {
            Reply::Tokens(tokens) => {
                if write(&mut io, &tokens, true).await.is_err() {
                    return;
                }
            }
            Reply::Hang(tokens) => {
                if write(&mut io, &tokens, false).await.is_err() {
                    return;
                }
                // Whatever comes next must be the Attention.
                match read_message(&mut io).await {
                    Some((ATTENTION, _)) => {
                        seen.attentions.fetch_add(1, Ordering::SeqCst);
                        if write(&mut io, &done(DONE_ATTN, 0), true).await.is_err() {
                            return;
                        }
                    }
                    _ => return,
                }
            }
            Reply::Stuck(tokens) => {
                if write(&mut io, &tokens, false).await.is_err() {
                    return;
                }
                while let Some((ty, _)) = read_message(&mut io).await {
                    if ty == ATTENTION {
                        seen.attentions.fetch_add(1, Ordering::SeqCst);
                    }
                }
                return;
            }
            Reply::Close => return,
        }
    }
}

/// One message: its packets' payloads up to the end-of-message one.
async fn read_message(io: &mut DuplexStream) -> Option<(u8, Vec<u8>)> {
    let mut payload = Vec::new();
    loop {
        let mut header = [0u8; 8];
        io.read_exact(&mut header).await.ok()?;
        let len = usize::from(u16::from_be_bytes([header[2], header[3]]));
        let mut body = vec![0u8; len.checked_sub(8)?];
        io.read_exact(&mut body).await.ok()?;
        payload.extend_from_slice(&body);
        if header[1] & 0x01 != 0 {
            return Some((header[0], payload));
        }
    }
}

/// Writes `payload` as a tabular result, in packets of at most 4 KiB; the last one ends the
/// message unless `end` is false (a statement still running).
async fn write(io: &mut DuplexStream, payload: &[u8], end: bool) -> std::io::Result<()> {
    const BODY: usize = 4096 - 8;
    let chunks: Vec<&[u8]> = if payload.is_empty() {
        vec![&[]]
    } else {
        payload.chunks(BODY).collect()
    };
    let count = chunks.len();
    for (i, chunk) in chunks.into_iter().enumerate() {
        let last = i + 1 == count;
        let len = (chunk.len() + 8) as u16;
        let mut packet = vec![TABULAR, u8::from(last && end), 0, 0, 0, 0, 1, 0];
        packet[2..4].copy_from_slice(&len.to_be_bytes());
        packet.extend_from_slice(chunk);
        io.write_all(&packet).await?;
    }
    io.flush().await
}

/// VERSION and ENCRYPTION (not supported), then the terminator.
fn prelogin_reply() -> Vec<u8> {
    let mut out = vec![0x00, 0, 11, 0, 6, 0x01, 0, 17, 0, 1, 0xFF];
    out.extend_from_slice(&[16, 0, 0x10, 0x27, 0, 0]);
    out.push(0x02);
    out
}

/// Reads the little-endian fields and UTF-16 strings of a request.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let out = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(out)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Option<u64> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Some(u64::from_le_bytes(a))
    }

    fn utf16(&mut self, bytes: usize) -> Option<String> {
        utf16(self.take(bytes)?)
    }

    fn is_done(&self) -> bool {
        self.at >= self.bytes.len()
    }

    /// Past ALL_HEADERS (its total length comes first).
    fn skip_headers(&mut self) -> Option<()> {
        let total = self.u32()? as usize;
        self.take(total.checked_sub(4)?)?;
        Some(())
    }
}

fn utf16(bytes: &[u8]) -> Option<String> {
    let (pairs, _) = bytes.as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().map(|&c| u16::from_le_bytes(c)).collect();
    String::from_utf16(&units).ok()
}

/// LOGIN7: the user, database and application names from its offset table.
fn login(payload: &[u8]) -> Option<Request> {
    let field = |at: usize| -> Option<String> {
        let mut r = Reader::new(payload);
        r.take(at)?;
        let offset = usize::from(r.u16()?);
        let chars = usize::from(r.u16()?);
        utf16(payload.get(offset..offset + chars * 2)?)
    };
    Some(Request::Login {
        user: field(40)?,
        application: field(48)?,
        database: field(68)?,
    })
}

fn batch(payload: &[u8]) -> Option<Request> {
    let mut r = Reader::new(payload);
    r.skip_headers()?;
    let rest = payload.len() - r.at;
    Some(Request::Batch(r.utf16(rest)?))
}

fn transaction(payload: &[u8]) -> Option<Request> {
    let mut r = Reader::new(payload);
    r.skip_headers()?;
    Some(Request::Transaction(r.u16()?))
}

fn rpc(payload: &[u8]) -> Option<Request> {
    let mut r = Reader::new(payload);
    r.skip_headers()?;
    let proc = r.u16()?;
    if proc == 0xFFFF {
        r.u16()?;
    } else {
        r.take(usize::from(proc) * 2)?;
    }
    r.u16()?;
    let mut values = Vec::new();
    while !r.is_done() {
        values.push(param(&mut r)?);
    }
    let mut values = values.into_iter();
    let sql = values.next()?;
    // The second parameter declares the others' types.
    values.next();
    Some(Request::Rpc {
        sql,
        params: values.collect(),
    })
}

/// A PLP value: its chunks up to the empty one.
fn plp(r: &mut Reader<'_>) -> Option<Option<Vec<u8>>> {
    if r.u64()? == u64::MAX {
        return Some(None);
    }
    let mut out = Vec::new();
    loop {
        let len = r.u32()? as usize;
        if len == 0 {
            return Some(Some(out));
        }
        out.extend_from_slice(r.take(len)?);
    }
}

/// One parameter's value as text.
fn param(r: &mut Reader<'_>) -> Option<String> {
    let name = usize::from(r.u8()?);
    r.take(name * 2)?;
    r.u8()?;
    let null = || Some("NULL".to_string());
    match r.u8()? {
        0x1F => null(),
        0xE7 => {
            let max = r.u16()?;
            r.take(5)?;
            if max == 0xFFFF {
                plp(r)?.map_or_else(null, |b| utf16(&b))
            } else {
                match r.u16()? {
                    0xFFFF => null(),
                    len => r.utf16(usize::from(len)),
                }
            }
        }
        0xA5 => {
            let bytes = if r.u16()? == 0xFFFF {
                plp(r)?
            } else {
                match r.u16()? {
                    0xFFFF => None,
                    len => Some(r.take(usize::from(len))?.to_vec()),
                }
            };
            bytes.map_or_else(null, |b| {
                Some(format!(
                    "0x{}",
                    b.iter().map(|x| format!("{x:02x}")).collect::<String>()
                ))
            })
        }
        ty @ (0x26 | 0x68 | 0x6D | 0x24) => {
            r.u8()?;
            let len = usize::from(r.u8()?);
            if len == 0 {
                return null();
            }
            let b = r.take(len)?;
            Some(match (ty, len) {
                (0x26, 4) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]).to_string(),
                (0x26, 8) => {
                    let mut a = [0u8; 8];
                    a.copy_from_slice(b);
                    i64::from_le_bytes(a).to_string()
                }
                (0x6D, 8) => {
                    let mut a = [0u8; 8];
                    a.copy_from_slice(b);
                    f64::from_le_bytes(a).to_string()
                }
                (0x68, _) => b[0].to_string(),
                _ => b.iter().map(|x| format!("{x:02x}")).collect(),
            })
        }
        0x6A | 0x6C => {
            r.u8()?;
            r.u8()?;
            let scale = r.u8()?;
            let len = usize::from(r.u8()?);
            if len == 0 {
                return null();
            }
            let sign = r.u8()?;
            let mut magnitude = [0u8; 16];
            magnitude[..len - 1].copy_from_slice(r.take(len - 1)?);
            let value = i128::from_le_bytes(magnitude);
            let value = if sign == 0 { -value } else { value };
            Some(format!("{value}e-{scale}"))
        }
        _ => None,
    }
}

// --- Tokens ----------------------------------------------------------------------------------------

/// Column types the fake server can send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Int,
    BigInt,
    NVarChar,
    Bit,
    Float,
    Money,
    /// varbinary(max), sent in PLP chunks.
    VarBinaryMax,
}

/// A value of a row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cell<'a> {
    Null,
    Int(i64),
    Text(&'a str),
    /// nvarchar as raw UTF-16 code units, which need not be valid UTF-16.
    Units(&'a [u16]),
    Bit(bool),
    Float(f64),
    /// Money as its value times 10^4.
    Money(i64),
    Bytes(&'a [u8]),
}

fn b_varchar(out: &mut Vec<u8>, s: &str) {
    let units: Vec<u16> = s.encode_utf16().collect();
    out.push(units.len() as u8);
    units
        .iter()
        .for_each(|u| out.extend_from_slice(&u.to_le_bytes()));
}

fn us_varchar(out: &mut Vec<u8>, s: &str) {
    let units: Vec<u16> = s.encode_utf16().collect();
    out.extend_from_slice(&(units.len() as u16).to_le_bytes());
    units
        .iter()
        .for_each(|u| out.extend_from_slice(&u.to_le_bytes()));
}

/// COLMETADATA of nullable columns.
pub fn columns(cols: &[(&str, Ty)]) -> Vec<u8> {
    let mut out = vec![0x81];
    out.extend_from_slice(&(cols.len() as u16).to_le_bytes());
    for (name, ty) in cols {
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        match ty {
            Ty::Int => out.extend_from_slice(&[0x26, 4]),
            Ty::BigInt => out.extend_from_slice(&[0x26, 8]),
            Ty::Bit => out.extend_from_slice(&[0x68, 1]),
            Ty::Float => out.extend_from_slice(&[0x6D, 8]),
            Ty::Money => out.extend_from_slice(&[0x6E, 8]),
            Ty::VarBinaryMax => {
                out.push(0xA5);
                out.extend_from_slice(&0xFFFFu16.to_le_bytes());
            }
            Ty::NVarChar => {
                out.push(0xE7);
                out.extend_from_slice(&8000u16.to_le_bytes());
                out.extend_from_slice(&[0x09, 0x04, 0xD0, 0x00, 0x34]);
            }
        }
        b_varchar(&mut out, name);
    }
    out
}

fn nvarchar(out: &mut Vec<u8>, units: &[u16]) {
    out.extend_from_slice(&((units.len() * 2) as u16).to_le_bytes());
    units
        .iter()
        .for_each(|u| out.extend_from_slice(&u.to_le_bytes()));
}

/// A ROW of `cells`, typed like the columns `types`.
pub fn row(types: &[Ty], cells: &[Cell<'_>]) -> Vec<u8> {
    let mut out = vec![0xD1];
    for (ty, cell) in types.iter().zip(cells) {
        match (ty, cell) {
            (Ty::NVarChar, Cell::Null) => out.extend_from_slice(&0xFFFFu16.to_le_bytes()),
            (Ty::VarBinaryMax, Cell::Null) => out.extend_from_slice(&u64::MAX.to_le_bytes()),
            (_, Cell::Null) => out.push(0),
            (Ty::Int, Cell::Int(n)) => {
                out.push(4);
                out.extend_from_slice(&(*n as i32).to_le_bytes());
            }
            (Ty::BigInt, Cell::Int(n)) => {
                out.push(8);
                out.extend_from_slice(&n.to_le_bytes());
            }
            (Ty::Bit, Cell::Bit(b)) => out.extend_from_slice(&[1, u8::from(*b)]),
            (Ty::Float, Cell::Float(f)) => {
                out.push(8);
                out.extend_from_slice(&f.to_le_bytes());
            }
            (Ty::Money, Cell::Money(m)) => {
                out.push(8);
                out.extend_from_slice(&((*m >> 32) as i32).to_le_bytes());
                out.extend_from_slice(&(*m as u32).to_le_bytes());
            }
            (Ty::NVarChar, Cell::Text(s)) => {
                let units: Vec<u16> = s.encode_utf16().collect();
                nvarchar(&mut out, &units);
            }
            (Ty::NVarChar, Cell::Units(units)) => nvarchar(&mut out, units),
            (Ty::VarBinaryMax, Cell::Bytes(bytes)) => {
                out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                for chunk in bytes.chunks(8000) {
                    out.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
                    out.extend_from_slice(chunk);
                }
                out.extend_from_slice(&0u32.to_le_bytes());
            }
            (ty, cell) => panic!("a test row puts {cell:?} in a {ty:?} column"),
        }
    }
    out
}

/// A result set: its metadata, rows and the DONE that counts them (more results follow when
/// `more`).
pub fn result(cols: &[(&str, Ty)], rows: &[Vec<Cell<'_>>], more: bool) -> Vec<u8> {
    let types: Vec<Ty> = cols.iter().map(|c| c.1).collect();
    let mut out = columns(cols);
    for r in rows {
        out.extend(row(&types, r));
    }
    let status = DONE_COUNT | if more { DONE_MORE } else { DONE_FINAL };
    out.extend(done(status, rows.len() as u64));
    out
}

pub fn done(status: u16, rows: u64) -> Vec<u8> {
    let mut out = vec![0xFD];
    out.extend_from_slice(&status.to_le_bytes());
    out.extend_from_slice(&0xC1u16.to_le_bytes());
    out.extend_from_slice(&rows.to_le_bytes());
    out
}

/// An ERROR token (with no procedure), as the server raises it.
pub fn error(code: u32, class: u8, line: u32, message: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&code.to_le_bytes());
    body.push(1);
    body.push(class);
    us_varchar(&mut body, message);
    b_varchar(&mut body, "fake");
    b_varchar(&mut body, "");
    body.extend_from_slice(&line.to_le_bytes());
    let mut out = vec![0xAA];
    out.extend_from_slice(&(body.len() as u16).to_le_bytes());
    out.extend(body);
    out
}

/// An error and the DONE that ends its statement.
pub fn failed(code: u32, line: u32, message: &str) -> Vec<u8> {
    let mut out = error(code, 16, line, message);
    out.extend(done(DONE_ERROR, 0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::sqlserver::connect::setup;
    use crate::sqlgen::dialect::tsql::SESSION_SETUP;

    #[tokio::test]
    async fn logs_in_and_sets_the_session_up() {
        let (mut client, seen) = client(ok).await.unwrap();
        setup(&mut client).await.unwrap();
        assert_eq!(
            seen.requests(),
            [
                Request::Login {
                    user: "sa".into(),
                    database: "shop".into(),
                    application: "Tabler".into(),
                },
                Request::Batch(SESSION_SETUP.into()),
            ]
        );
    }

    #[tokio::test]
    async fn a_refused_login_is_an_auth_error() {
        let refused = client(|r: &Request| match r {
            Request::Login { .. } => Reply::Tokens(failed(18456, 1, "Login failed for user 'sa'.")),
            _ => Reply::Close,
        })
        .await;
        assert!(matches!(refused, Err(AppError::Auth(m)) if m.contains("Login failed")));
    }

    #[test]
    fn rpc_parameters_read_back() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&22u32.to_le_bytes());
        payload.extend_from_slice(&[0u8; 18]);
        payload.extend_from_slice(&0xFFFFu16.to_le_bytes());
        payload.extend_from_slice(&10u16.to_le_bytes());
        payload.extend_from_slice(&0u16.to_le_bytes());
        let text = |p: &mut Vec<u8>, s: &str| {
            p.push(0);
            p.push(0);
            p.push(0xE7);
            p.extend_from_slice(&8000u16.to_le_bytes());
            p.extend_from_slice(&[0; 5]);
            let units: Vec<u16> = s.encode_utf16().collect();
            p.extend_from_slice(&((units.len() * 2) as u16).to_le_bytes());
            units
                .iter()
                .for_each(|u| p.extend_from_slice(&u.to_le_bytes()));
        };
        text(&mut payload, "SELECT @P1");
        text(&mut payload, "@P1 int");
        payload.extend_from_slice(&[0, 0, 0x26, 4, 4]);
        payload.extend_from_slice(&7i32.to_le_bytes());
        payload.extend_from_slice(&[0, 0, 0x1F]);
        assert_eq!(
            rpc(&payload),
            Some(Request::Rpc {
                sql: "SELECT @P1".into(),
                params: vec!["7".into(), "NULL".into()],
            })
        );
    }
}
