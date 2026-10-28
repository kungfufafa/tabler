//! Vertica's frontend/backend protocol 3.x, the message layer: framing, the messages the client
//! sends, and the parsers of the ones the server sends.
//!
//! Vertica's protocol started as PostgreSQL's and has drifted (the server's engineers call it
//! undocumented); vertica-python's message layer is the reference this follows. What differs from
//! PostgreSQL: the startup message (version 3.5 on the wire, the version wanted as a binary
//! `protocol_version` parameter, client facts and `autocommit`), the password hashes of `auth`,
//! a RowDescription with a pool of type names, 8-byte table OIDs, schema and table names and
//! nullable and identity flags, a Bind whose parameter type OIDs all come before the values,
//! and the backend messages PostgreSQL lacks (CommandDescription, EndOfBatch, LoadBalance and
//! the COPY LOCAL file messages).
//!
//! Every parser here is total: a short or malformed message is an `Err`, never a panic, since
//! release builds abort on one.

use std::ops::Range;

use tokio::io::{AsyncRead, AsyncReadExt as _};

/// The version the startup message carries on the wire (3.5); servers from 3.7 negotiate the
/// real one from the `protocol_version` parameter and answer with a ParameterStatus.
pub const STARTUP_VERSION: u32 = 3 << 16 | 5;
/// The version Tabler asks for: vertica-python's (3.16), whose message layer this ports.
pub const PROTOCOL_VERSION: u32 = 3 << 16 | 16;
/// From this version a RowDescription field may name its parent column (complex types).
pub const COMPLEX_TYPES_VERSION: u32 = 3 << 16 | 12;
pub const SSL_REQUEST_CODE: u32 = 80_877_103;
pub const CANCEL_REQUEST_CODE: u32 = 80_877_102;
/// Longest backend message accepted. Vertica caps a LONG VARCHAR / LONG VARBINARY value at
/// 32 MB, so a row of a few of them stays far below this.
pub const MAX_MESSAGE: usize = 1 << 30;

/// A protocol violation: the connection's state is unknown afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError(pub String);

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Vertica protocol error: {}", self.0)
    }
}

impl std::error::Error for WireError {}

fn bad(what: impl Into<String>) -> WireError {
    WireError(what.into())
}

// --- Frontend messages ---------------------------------------------------------------------------

/// Appends `s` NUL-terminated; a NUL inside it would end the field early on the server, so it is
/// refused.
fn cstr(out: &mut Vec<u8>, s: &str) -> Result<(), WireError> {
    if s.contains('\0') {
        return Err(bad(
            "text holds a NUL character, which the protocol cannot carry",
        ));
    }
    out.extend_from_slice(s.as_bytes());
    out.push(0);
    Ok(())
}

/// A tagged message: the tag, the length (itself included) and the body.
fn tagged(tag: u8, body: &[u8]) -> Result<Vec<u8>, WireError> {
    let len = u32::try_from(body.len() + 4)
        .ok()
        .filter(|&n| n as usize <= MAX_MESSAGE)
        .ok_or_else(|| bad("message too long"))?;
    let mut out = Vec::with_capacity(body.len() + 5);
    out.push(tag);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(body);
    Ok(out)
}

/// An untagged message (startup, SSL and cancel requests): the length, then the body.
fn untagged(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}

pub fn ssl_request() -> Vec<u8> {
    untagged(&SSL_REQUEST_CODE.to_be_bytes())
}

/// Sent on a new connection; the server answers by closing it.
pub fn cancel_request(key: BackendKey) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&CANCEL_REQUEST_CODE.to_be_bytes());
    body.extend_from_slice(&key.pid.to_be_bytes());
    body.extend_from_slice(&key.key.to_be_bytes());
    untagged(&body)
}

/// The startup message: version 3.5, the wanted version as a binary parameter (as vertica-python
/// sends it: name, NUL, a big-endian u32, NUL), then the `params` and the closing NUL.
pub fn startup(params: &[(&str, &str)], protocol_version: u32) -> Result<Vec<u8>, WireError> {
    let mut body = Vec::with_capacity(256);
    body.extend_from_slice(&STARTUP_VERSION.to_be_bytes());
    body.extend_from_slice(b"protocol_version\0");
    body.extend_from_slice(&protocol_version.to_be_bytes());
    body.push(0);
    for (name, value) in params {
        cstr(&mut body, name)?;
        cstr(&mut body, value)?;
    }
    body.push(0);
    Ok(untagged(&body))
}

/// A password message with what `auth::response` computed. Every method sends it NUL-terminated
/// but GSS, which Tabler does not offer.
pub fn password(response: &[u8]) -> Result<Vec<u8>, WireError> {
    if response.contains(&0) {
        return Err(bad("the password holds a NUL character"));
    }
    let mut body = Vec::with_capacity(response.len() + 1);
    body.extend_from_slice(response);
    body.push(0);
    tagged(b'p', &body)
}

/// A simple query: one or more statements, run in turn.
pub fn query(sql: &str) -> Result<Vec<u8>, WireError> {
    let mut body = Vec::with_capacity(sql.len() + 1);
    cstr(&mut body, sql)?;
    tagged(b'Q', &body)
}

/// Prepares `sql` as the statement `name`; no parameter types, so the server infers them and says
/// which in the ParameterDescription.
pub fn parse(name: &str, sql: &str) -> Result<Vec<u8>, WireError> {
    let mut body = Vec::with_capacity(sql.len() + name.len() + 4);
    cstr(&mut body, name)?;
    cstr(&mut body, sql)?;
    body.extend_from_slice(&0u16.to_be_bytes());
    tagged(b'P', &body)
}

/// What Describe and Close address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Statement,
    Portal,
}

impl Target {
    fn byte(self) -> u8 {
        match self {
            Target::Statement => b'S',
            Target::Portal => b'P',
        }
    }
}

pub fn describe(target: Target, name: &str) -> Result<Vec<u8>, WireError> {
    let mut body = vec![target.byte()];
    cstr(&mut body, name)?;
    tagged(b'D', &body)
}

pub fn close(target: Target, name: &str) -> Result<Vec<u8>, WireError> {
    let mut body = vec![target.byte()];
    cstr(&mut body, name)?;
    tagged(b'C', &body)
}

/// Binds `values` (text format; None is NULL) to the statement, with the type OIDs its
/// ParameterDescription gave. Vertica's layout: the portal and statement names, no parameter
/// format codes, the count, every type OID, every value, then the result format (one code for
/// all columns: 1 binary, or none for text).
pub fn bind(
    portal: &str,
    statement: &str,
    values: &[Option<Vec<u8>>],
    oids: &[u32],
    binary_results: bool,
) -> Result<Vec<u8>, WireError> {
    if values.len() != oids.len() {
        return Err(bad(format!(
            "{} values for {} parameters",
            values.len(),
            oids.len()
        )));
    }
    let count = u16::try_from(values.len()).map_err(|_| bad("too many parameters"))?;
    let mut body = Vec::with_capacity(64);
    cstr(&mut body, portal)?;
    cstr(&mut body, statement)?;
    body.extend_from_slice(&0u16.to_be_bytes());
    body.extend_from_slice(&count.to_be_bytes());
    for oid in oids {
        body.extend_from_slice(&oid.to_be_bytes());
    }
    for v in values {
        match v {
            None => body.extend_from_slice(&(-1i32).to_be_bytes()),
            Some(bytes) => {
                let len = i32::try_from(bytes.len()).map_err(|_| bad("parameter too long"))?;
                body.extend_from_slice(&len.to_be_bytes());
                body.extend_from_slice(bytes);
            }
        }
    }
    if binary_results {
        body.extend_from_slice(&1u16.to_be_bytes());
        body.extend_from_slice(&1u16.to_be_bytes());
    } else {
        body.extend_from_slice(&0u16.to_be_bytes());
    }
    tagged(b'B', &body)
}

/// Runs the portal; `max_rows` 0 means all of them.
pub fn execute(portal: &str, max_rows: u32) -> Result<Vec<u8>, WireError> {
    let mut body = Vec::with_capacity(portal.len() + 5);
    cstr(&mut body, portal)?;
    body.extend_from_slice(&max_rows.to_be_bytes());
    tagged(b'E', &body)
}

pub fn sync() -> Vec<u8> {
    vec![b'S', 0, 0, 0, 4]
}

pub fn flush() -> Vec<u8> {
    vec![b'H', 0, 0, 0, 4]
}

pub fn terminate() -> Vec<u8> {
    vec![b'X', 0, 0, 0, 4]
}

/// Ends a COPY … FROM STDIN the server started (CopyInResponse) without sending data.
pub fn copy_fail(message: &str) -> Result<Vec<u8>, WireError> {
    let mut body = Vec::new();
    cstr(&mut body, &message.replace('\0', " "))?;
    tagged(b'f', &body)
}

/// Ends a COPY LOCAL the server started (VerifyFiles, LoadFile): no file name, line 0, no
/// function, then the message.
pub fn copy_error(message: &str) -> Result<Vec<u8>, WireError> {
    let mut body = vec![0];
    body.extend_from_slice(&0u32.to_be_bytes());
    body.push(0);
    cstr(&mut body, &message.replace('\0', " "))?;
    tagged(b'e', &body)
}

// --- Framing ----------------------------------------------------------------------------------

/// One backend message: its tag and body (the length is gone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub tag: u8,
    pub body: Vec<u8>,
}

/// Reads one backend message. The body is read as it arrives (never allocated up front from the
/// length the server claims), and a length below 4 or above `MAX_MESSAGE` is an error.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Frame> {
    let tag = r.read_u8().await?;
    let len = r.read_u32().await? as usize;
    if !(4..=MAX_MESSAGE).contains(&len) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            bad(format!("message {:?} of length {len}", tag as char)),
        ));
    }
    let want = len - 4;
    let mut body = Vec::with_capacity(want.min(64 * 1024));
    let got = r.take(want as u64).read_to_end(&mut body).await?;
    if got < want {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    Ok(Frame { tag, body })
}

// --- Backend messages -------------------------------------------------------------------------

/// A bounds-checked reader of a message body.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .ok_or_else(|| bad("message ends early"))?;
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    pub fn i16(&mut self) -> Result<i16, WireError> {
        Ok(i16::from_be_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    pub fn i32(&mut self) -> Result<i32, WireError> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    /// A NUL-terminated string; invalid UTF-8 is replaced rather than refused (a name or a
    /// message is still worth showing).
    pub fn cstr(&mut self) -> Result<String, WireError> {
        let rest = &self.buf[self.pos..];
        let n = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| bad("string without its terminating NUL"))?;
        let s = String::from_utf8_lossy(&rest[..n]).into_owned();
        self.pos += n + 1;
        Ok(s)
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let out = &self.buf[self.pos..];
        self.pos = self.buf.len();
        out
    }
}

/// What the server asks the client to prove (Authentication, `R`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthRequest {
    Ok,
    Cleartext,
    /// PostgreSQL's MD5 (5) or Vertica's HASH_MD5 (65541): `md5` + md5(md5(password + user) +
    /// salt).
    Md5 {
        salt: [u8; 4],
    },
    /// Vertica's HASH (65536) and HASH_SHA512 (66048): `sha512` + sha512(sha512(password +
    /// user salt) + salt).
    Sha512 {
        salt: [u8; 4],
        user_salt: [u8; 16],
    },
    /// The password field carries an OAuth access token.
    OAuth,
    Totp,
    /// The password has expired: the server wants it changed, which Tabler does not do.
    ChangePassword,
    /// The password expires soon; the login goes on.
    PasswordGrace,
    /// Kerberos, GSS, crypt, SCM credentials: not offered.
    Unsupported(u32),
}

pub const AUTH_OK: u32 = 0;
pub const AUTH_CLEARTEXT: u32 = 3;
pub const AUTH_CRYPT: u32 = 4;
pub const AUTH_MD5: u32 = 5;
pub const AUTH_CHANGE_PASSWORD: u32 = 9;
pub const AUTH_PASSWORD_GRACE: u32 = 11;
pub const AUTH_OAUTH: u32 = 12;
pub const AUTH_TOTP: u32 = 14;
pub const AUTH_HASH: u32 = 65_536;
pub const AUTH_HASH_MD5: u32 = 65_536 + 5;
pub const AUTH_HASH_SHA512: u32 = 65_536 + 512;

fn parse_auth(r: &mut Reader) -> Result<AuthRequest, WireError> {
    let code = r.u32()?;
    Ok(match code {
        AUTH_OK => AuthRequest::Ok,
        AUTH_CLEARTEXT => AuthRequest::Cleartext,
        AUTH_MD5 | AUTH_HASH_MD5 => AuthRequest::Md5 { salt: r.array()? },
        AUTH_HASH | AUTH_HASH_SHA512 => {
            let salt = r.array()?;
            let len = r.u32()?;
            if len != 16 {
                return Err(bad(format!("a user salt of {len} bytes (16 expected)")));
            }
            AuthRequest::Sha512 {
                salt,
                user_salt: r.array()?,
            }
        }
        AUTH_OAUTH => AuthRequest::OAuth,
        AUTH_TOTP => AuthRequest::Totp,
        AUTH_CHANGE_PASSWORD => AuthRequest::ChangePassword,
        AUTH_PASSWORD_GRACE => AuthRequest::PasswordGrace,
        other => AuthRequest::Unsupported(other),
    })
}

/// What a cancel request names: the backend's process id and secret key (BackendKeyData).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendKey {
    pub pid: u32,
    pub key: u32,
}

/// The transaction state of ReadyForQuery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxStatus {
    Idle,
    InTransaction,
    Failed,
}

/// The fields of an ErrorResponse or NoticeResponse (PostgreSQL's, plus Vertica's `V`, the
/// engine's own error number).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerMessage {
    pub severity: Option<String>,
    pub sqlstate: Option<String>,
    pub message: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
    /// 1-based character position in the statement.
    pub position: Option<u32>,
    pub internal_query: Option<String>,
    pub where_: Option<String>,
    pub routine: Option<String>,
    pub file: Option<String>,
    pub line: Option<String>,
    /// Vertica's error code (`V`), e.g. 4566 for a missing relation.
    pub error_code: Option<i64>,
}

fn parse_fields(r: &mut Reader) -> Result<ServerMessage, WireError> {
    let mut m = ServerMessage::default();
    loop {
        let kind = r.u8()?;
        if kind == 0 {
            break;
        }
        let value = r.cstr()?;
        match kind {
            b'S' => m.severity = Some(value),
            b'C' => m.sqlstate = Some(value),
            b'M' => m.message = value,
            b'D' => m.detail = Some(value),
            b'H' => m.hint = Some(value),
            b'P' => m.position = value.trim().parse().ok(),
            b'q' => m.internal_query = Some(value),
            b'W' => m.where_ = Some(value),
            b'R' => m.routine = Some(value),
            b'F' => m.file = Some(value),
            b'L' => m.line = Some(value),
            b'V' => m.error_code = value.trim().parse().ok(),
            // Fields this does not know are skipped, as the protocol allows.
            _ => {}
        }
    }
    Ok(m)
}

/// A type of the RowDescription's or ParameterDescription's pool: a user-defined type (GEOMETRY,
/// GEOGRAPHY…) by its base type's OID and its own name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolType {
    pub base_oid: u32,
    pub name: String,
}

fn parse_pool(r: &mut Reader) -> Result<Vec<PoolType>, WireError> {
    let n = r.u32()? as usize;
    // Each entry takes at least 5 bytes: a count beyond that is garbage, not an allocation.
    if n > r.remaining() / 5 {
        return Err(bad(format!("a type pool of {n} entries")));
    }
    let mut pool = Vec::with_capacity(n);
    for _ in 0..n {
        let base_oid = r.u32()?;
        let name = r.cstr()?;
        pool.push(PoolType { base_oid, name });
    }
    Ok(pool)
}

/// A column's type: a built-in type OID, or a pool type (then `oid` is its base type's and
/// `user_type` its name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef {
    pub oid: u32,
    pub user_type: Option<String>,
    pub modifier: i32,
}

fn type_ref(
    pool: &[PoolType],
    user: u8,
    oid_or_index: u32,
    modifier: i32,
) -> Result<TypeRef, WireError> {
    if user == 1 {
        let entry = pool
            .get(oid_or_index as usize)
            .ok_or_else(|| bad(format!("type pool index {oid_or_index} out of range")))?;
        Ok(TypeRef {
            oid: entry.base_oid,
            user_type: Some(entry.name.clone()),
            modifier,
        })
    } else {
        Ok(TypeRef {
            oid: oid_or_index,
            user_type: None,
            modifier,
        })
    }
}

/// A result column of a RowDescription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    /// 0 for a computed column.
    pub table_oid: u64,
    pub schema: Option<String>,
    pub table: Option<String>,
    pub attribute: u16,
    pub ty: TypeRef,
    /// The type's size in bytes, -1 when it varies.
    pub size: i16,
    pub nullable: bool,
    pub identity: bool,
    /// 1: binary, 0: text.
    pub format: u16,
    /// The fields of a complex type (ROW, ARRAY of ROW, MAP) below this one; their values come
    /// inside this column's (as JSON), never as columns of their own.
    pub children: Vec<Field>,
}

/// Parses a RowDescription. `complex`: the session negotiated complex types (protocol 3.12 and
/// `request_complex_types`), so each field carries its parent's attribute number; a field with a
/// parent goes into that field's `children`.
pub fn parse_row_description(body: &[u8], complex: bool) -> Result<Vec<Field>, WireError> {
    let mut r = Reader::new(body);
    let count = r.u16()?;
    if count == 0 {
        return Ok(Vec::new());
    }
    let pool = parse_pool(&mut r)?;
    let mut top: Vec<Field> = Vec::with_capacity(count as usize);
    // Where each described field sits: (table OID, attribute) → path of indexes from `top`.
    let mut paths: Vec<((u64, u16), Vec<usize>)> = Vec::new();
    for _ in 0..count {
        let name = r.cstr()?;
        let table_oid = r.u64()?;
        let (schema, table) = if table_oid != 0 {
            (Some(r.cstr()?), Some(r.cstr()?))
        } else {
            (None, None)
        };
        let attribute = r.u16()?;
        let parent = if complex { r.u16()? } else { 0 };
        let user = r.u8()?;
        let oid_or_index = r.u32()?;
        let size = r.i16()?;
        let nullable = r.u16()? == 1;
        let identity = r.u16()? == 1;
        let modifier = r.i32()?;
        let format = r.u16()?;
        let field = Field {
            name,
            table_oid,
            schema,
            table,
            attribute,
            ty: type_ref(&pool, user, oid_or_index, modifier)?,
            size,
            nullable,
            identity,
            format,
            children: Vec::new(),
        };
        let key = (table_oid, attribute);
        if parent == 0 {
            paths.push((key, vec![top.len()]));
            top.push(field);
        } else {
            let path = paths
                .iter()
                .rev()
                .find(|(k, _)| *k == (table_oid, parent))
                .map(|(_, p)| p.clone())
                .ok_or_else(|| {
                    bad(format!(
                        "complex type field {:?} without its parent",
                        field.name
                    ))
                })?;
            let holder =
                field_at(&mut top, &path).ok_or_else(|| bad("complex type parent lost"))?;
            let mut child_path = path;
            child_path.push(holder.children.len());
            holder.children.push(field);
            paths.push((key, child_path));
        }
    }
    // Bytes left over mean the fields were read with the wrong layout (the version asked for
    // decides it): fail rather than decode cells by misread types.
    if r.remaining() != 0 {
        return Err(bad("bytes after the last field of a RowDescription"));
    }
    Ok(top)
}

fn field_at<'f>(fields: &'f mut [Field], path: &[usize]) -> Option<&'f mut Field> {
    let (&first, rest) = path.split_first()?;
    let mut f = fields.get_mut(first)?;
    for &i in rest {
        f = f.children.get_mut(i)?;
    }
    Some(f)
}

/// A parameter of a ParameterDescription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamType {
    pub ty: TypeRef,
    pub not_null: bool,
}

pub fn parse_parameter_description(body: &[u8]) -> Result<Vec<ParamType>, WireError> {
    let mut r = Reader::new(body);
    let count = r.u16()?;
    if count == 0 {
        return Ok(Vec::new());
    }
    let pool = parse_pool(&mut r)?;
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let user = r.u8()?;
        let oid_or_index = r.u32()?;
        let modifier = r.i32()?;
        let not_null = r.u16()? == 1;
        out.push(ParamType {
            ty: type_ref(&pool, user, oid_or_index, modifier)?,
            not_null,
        });
    }
    if r.remaining() != 0 {
        return Err(bad(
            "bytes after the last parameter of a ParameterDescription",
        ));
    }
    Ok(out)
}

/// The cells of a DataRow, as ranges of its body (None: NULL).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataRow {
    body: Vec<u8>,
    cells: Vec<Option<Range<usize>>>,
}

impl DataRow {
    pub fn parse(body: Vec<u8>) -> Result<DataRow, WireError> {
        let mut r = Reader::new(&body);
        let count = r.u16()? as usize;
        if count > r.remaining() / 4 {
            return Err(bad(format!("a row of {count} cells")));
        }
        let mut cells = Vec::with_capacity(count);
        for _ in 0..count {
            let len = r.i32()?;
            if len == -1 {
                cells.push(None);
                continue;
            }
            let len = usize::try_from(len).map_err(|_| bad(format!("a cell of length {len}")))?;
            let start = r.pos();
            r.bytes(len)?;
            cells.push(Some(start..start + len));
        }
        if r.remaining() != 0 {
            return Err(bad("bytes after the last cell of a row"));
        }
        Ok(DataRow { body, cells })
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Cell `i`: None for NULL or a cell the row does not have.
    pub fn cell(&self, i: usize) -> Option<&[u8]> {
        let range = self.cells.get(i)?.clone()?;
        self.body.get(range)
    }

    pub fn is_null(&self, i: usize) -> bool {
        self.cells.get(i).is_none_or(Option::is_none)
    }
}

/// A backend message.
#[derive(Debug, Clone, PartialEq)]
pub enum Backend {
    Auth(AuthRequest),
    BackendKey(BackendKey),
    BindComplete,
    CloseComplete,
    /// The command tag of a finished statement (`SELECT 3`, `INSERT 0 1`, `COMMIT`).
    CommandComplete(String),
    /// What a prepared statement is (its command tag), and the COPY a COPY LOCAL was rewritten to.
    CommandDescription {
        tag: String,
        copy_rewrite: Option<String>,
    },
    CopyDone,
    /// COPY … FROM STDIN asks for data.
    CopyIn,
    DataRow(DataRow),
    EmptyQuery,
    EndOfBatch,
    Error(ServerMessage),
    LoadBalance {
        port: u32,
        host: String,
    },
    /// COPY LOCAL asks for a file.
    LoadFile(String),
    NoData,
    Notice(ServerMessage),
    ParameterDescription(Vec<ParamType>),
    ParameterStatus {
        name: String,
        value: String,
    },
    ParseComplete,
    PortalSuspended,
    Ready(TxStatus),
    RowDescription(Vec<Field>),
    /// COPY LOCAL names its input, rejection and exception files.
    VerifyFiles,
    /// COPY LOCAL sends back a rejected-rows or exceptions file.
    WriteFile,
    Unknown(u8),
}

impl Backend {
    /// Parses a frame. `complex`: see `parse_row_description`.
    pub fn parse(frame: Frame, complex: bool) -> Result<Backend, WireError> {
        let Frame { tag, body } = frame;
        let mut r = Reader::new(&body);
        let msg = match tag {
            b'R' => Backend::Auth(parse_auth(&mut r)?),
            b'K' => Backend::BackendKey(BackendKey {
                pid: r.u32()?,
                key: r.u32()?,
            }),
            b'2' => Backend::BindComplete,
            b'3' => Backend::CloseComplete,
            b'C' => Backend::CommandComplete(r.cstr()?),
            b'm' => {
                let tag = r.cstr()?;
                let rewrite = r.u16()? == 1;
                let text = r.cstr()?;
                Backend::CommandDescription {
                    tag,
                    copy_rewrite: rewrite.then_some(text),
                }
            }
            b'c' => Backend::CopyDone,
            b'G' => Backend::CopyIn,
            b'D' => Backend::DataRow(DataRow::parse(body)?),
            b'I' => Backend::EmptyQuery,
            b'J' => Backend::EndOfBatch,
            b'E' => Backend::Error(parse_fields(&mut r)?),
            b'N' => Backend::Notice(parse_fields(&mut r)?),
            b'Y' => Backend::LoadBalance {
                port: r.u32()?,
                host: r.cstr()?,
            },
            b'H' => Backend::LoadFile(r.cstr()?),
            b'n' => Backend::NoData,
            b't' => Backend::ParameterDescription(parse_parameter_description(&body)?),
            b'S' => Backend::ParameterStatus {
                name: r.cstr()?,
                value: r.cstr()?,
            },
            b'1' => Backend::ParseComplete,
            b's' => Backend::PortalSuspended,
            b'Z' => Backend::Ready(match r.u8()? {
                b'I' => TxStatus::Idle,
                b'T' => TxStatus::InTransaction,
                b'E' => TxStatus::Failed,
                other => return Err(bad(format!("transaction status {:?}", other as char))),
            }),
            b'T' => Backend::RowDescription(parse_row_description(&body, complex)?),
            b'F' => Backend::VerifyFiles,
            b'O' => Backend::WriteFile,
            other => Backend::Unknown(other),
        };
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::vertica::testing::msg;

    fn frame(bytes: &[u8]) -> Frame {
        Frame {
            tag: bytes[0],
            body: bytes[5..].to_vec(),
        }
    }

    #[test]
    fn startup_carries_the_wanted_version_in_binary() {
        let m = startup(
            &[("user", "dbadmin"), ("database", "VMart")],
            PROTOCOL_VERSION,
        )
        .unwrap();
        let len = u32::from_be_bytes(m[..4].try_into().unwrap()) as usize;
        assert_eq!(len, m.len());
        assert_eq!(&m[4..8], &STARTUP_VERSION.to_be_bytes());
        assert_eq!(&m[8..25], b"protocol_version\0");
        assert_eq!(&m[25..29], &(196_624u32).to_be_bytes());
        assert_eq!(m[29], 0);
        assert_eq!(&m[30..], b"user\0dbadmin\0database\0VMart\0\0");
        assert!(startup(&[("user", "a\0b")], PROTOCOL_VERSION).is_err());
    }

    #[test]
    fn fixed_messages() {
        assert_eq!(ssl_request(), [0, 0, 0, 8, 4, 210, 22, 47]);
        assert_eq!(
            cancel_request(BackendKey {
                pid: 7,
                key: 0xdead_beef
            }),
            [
                0, 0, 0, 16, 4, 210, 22, 46, 0, 0, 0, 7, 0xde, 0xad, 0xbe, 0xef
            ]
        );
        assert_eq!(query("SELECT 1").unwrap(), b"Q\0\0\0\x0dSELECT 1\0");
        assert!(query("SELECT '\0'").is_err());
        assert_eq!(password(b"md5abc").unwrap(), b"p\0\0\0\x0bmd5abc\0");
        assert_eq!(execute("", 0).unwrap(), b"E\0\0\0\x09\0\0\0\0\0");
        assert_eq!(
            describe(Target::Statement, "s0").unwrap(),
            b"D\0\0\0\x08Ss0\0"
        );
        assert_eq!(close(Target::Statement, "s0").unwrap(), b"C\0\0\0\x08Ss0\0");
        assert_eq!(
            parse("s0", "SELECT ?").unwrap(),
            b"P\0\0\0\x12s0\0SELECT ?\0\0\0"
        );
        assert_eq!(sync(), b"S\0\0\0\x04");
        assert_eq!(copy_fail("no").unwrap(), b"f\0\0\0\x07no\0");
        assert_eq!(copy_error("no").unwrap(), b"e\0\0\0\x0d\0\0\0\0\0\0no\0");
    }

    #[test]
    fn bind_puts_every_oid_before_the_values() {
        let m = bind("", "s0", &[Some(b"42".to_vec()), None], &[6, 9], false).unwrap();
        let mut want = b"B".to_vec();
        let body: Vec<u8> = [
            &b"\0s0\0"[..],
            &[0, 0, 0, 2],
            &[0, 0, 0, 6, 0, 0, 0, 9],
            &[0, 0, 0, 2, b'4', b'2'],
            &[0xff, 0xff, 0xff, 0xff],
            &[0, 0],
        ]
        .concat();
        want.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
        want.extend_from_slice(&body);
        assert_eq!(m, want);
        assert!(bind("", "s0", &[None], &[], false).is_err());
        let binary = bind("", "", &[], &[], true).unwrap();
        assert_eq!(&binary[binary.len() - 4..], &[0, 1, 0, 1]);
    }

    #[tokio::test]
    async fn frames_are_read_whole_and_checked() {
        let bytes = [msg::ready(b'I'), msg::command_complete("SELECT 1")].concat();
        let mut r = &bytes[..];
        assert_eq!(read_frame(&mut r).await.unwrap().tag, b'Z');
        let f = read_frame(&mut r).await.unwrap();
        assert_eq!((f.tag, f.body), (b'C', b"SELECT 1\0".to_vec()));
        assert!(read_frame(&mut r).await.is_err());
        // A length below 4, one past the limit, and a body cut short.
        for bad in [
            &b"Z\0\0\0\x03"[..],
            &[b'D', 0x40, 0, 0, 1],
            &b"C\0\0\0\x09ab"[..],
        ] {
            let mut r = bad;
            assert!(read_frame(&mut r).await.is_err(), "{bad:?}");
        }
    }

    #[test]
    fn auth_requests() {
        let p = |b: &[u8]| Backend::parse(frame(b), false);
        assert_eq!(p(&msg::auth_ok()).unwrap(), Backend::Auth(AuthRequest::Ok));
        assert_eq!(
            p(&msg::auth_md5([1, 2, 3, 4])).unwrap(),
            Backend::Auth(AuthRequest::Md5 { salt: [1, 2, 3, 4] })
        );
        let sha = msg::auth_sha512(AUTH_HASH_SHA512, [9, 8, 7, 6], [5; 16]);
        assert_eq!(
            p(&sha).unwrap(),
            Backend::Auth(AuthRequest::Sha512 {
                salt: [9, 8, 7, 6],
                user_salt: [5; 16]
            })
        );
        assert!(matches!(
            p(&msg::auth_sha512(AUTH_HASH, [0; 4], [0; 16])).unwrap(),
            Backend::Auth(AuthRequest::Sha512 { .. })
        ));
        assert_eq!(
            p(&msg::auth(AUTH_HASH_MD5, &[4, 3, 2, 1])).unwrap(),
            Backend::Auth(AuthRequest::Md5 { salt: [4, 3, 2, 1] })
        );
        assert_eq!(
            p(&msg::auth(7, &[])).unwrap(),
            Backend::Auth(AuthRequest::Unsupported(7))
        );
        // A user salt of another length, and a salt cut short.
        let mut wrong = msg::auth(AUTH_HASH_SHA512, &[1, 2, 3, 4, 0, 0, 0, 8]);
        wrong.extend_from_slice(&[0; 8]);
        assert!(p(&wrong).is_err());
        assert!(p(&msg::auth(AUTH_MD5, &[1, 2])).is_err());
    }

    #[test]
    fn row_description_with_a_type_pool_and_table_names() {
        let body = msg::row_description(
            &[(17, "geometry")],
            &[
                msg::field(
                    "id",
                    45_035_996_273_704_982,
                    Some(("public", "t")),
                    1,
                    0,
                    false,
                    6,
                    8,
                    -1,
                ),
                msg::field(
                    "name",
                    45_035_996_273_704_982,
                    Some(("public", "t")),
                    2,
                    0,
                    false,
                    9,
                    -1,
                    84,
                ),
                msg::field("shape", 0, None, 0, 0, true, 0, -1, -1),
            ],
            false,
        );
        let Backend::RowDescription(fields) = Backend::parse(frame(&body), false).unwrap() else {
            panic!("not a RowDescription")
        };
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0].name, "id");
        assert_eq!(fields[0].table_oid, 45_035_996_273_704_982);
        assert_eq!(fields[0].schema.as_deref(), Some("public"));
        assert_eq!(fields[0].table.as_deref(), Some("t"));
        assert_eq!((fields[0].ty.oid, fields[0].size), (6, 8));
        assert_eq!(fields[1].ty.modifier, 84);
        assert_eq!(
            fields[2].ty,
            TypeRef {
                oid: 17,
                user_type: Some("geometry".into()),
                modifier: -1
            }
        );
        assert!(fields[2].schema.is_none());
    }

    #[test]
    fn complex_children_go_under_their_parent() {
        let body = msg::row_description(
            &[],
            &[
                msg::field("r", 7, Some(("s", "t")), 1, 0, false, 300, -1, -1),
                msg::field("a", 7, Some(("s", "t")), 2, 1, false, 6, 8, -1),
                msg::field("b", 7, Some(("s", "t")), 3, 1, false, 1509, -1, -1),
                msg::field("x", 7, Some(("s", "t")), 4, 0, false, 9, -1, 14),
            ],
            true,
        );
        let fields = parse_row_description(&body[5..], true).unwrap();
        assert_eq!(
            fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["r", "x"]
        );
        assert_eq!(fields[0].children.len(), 2);
        // Without complex types negotiated the same bytes do not parse (the parent fields shift
        // everything), and a child whose parent is missing is an error.
        assert!(parse_row_description(&body[5..], false).is_err());
        let orphan = msg::row_description(
            &[],
            &[msg::field("a", 7, Some(("s", "t")), 2, 1, false, 6, 8, -1)],
            true,
        );
        assert!(parse_row_description(&orphan[5..], true).is_err());
    }

    #[test]
    fn data_rows_keep_nulls_and_refuse_bad_lengths() {
        let body = msg::data_row(&[Some(&b"1"[..]), None, Some(&b""[..])]);
        let Backend::DataRow(row) = Backend::parse(frame(&body), false).unwrap() else {
            panic!("not a DataRow")
        };
        assert_eq!(row.len(), 3);
        assert_eq!(row.cell(0), Some(&b"1"[..]));
        assert!(row.is_null(1) && row.cell(1).is_none());
        assert_eq!(row.cell(2), Some(&b""[..]));
        assert!(row.cell(3).is_none() && row.is_null(3));
        for bad in [
            vec![0, 1, 0, 0, 0, 5, b'a'],
            vec![0, 1, 0xff, 0xff, 0xff, 0xfe],
            vec![0, 2, 0xff, 0xff, 0xff, 0xff],
            vec![0, 0, 1],
            vec![0xff, 0xff],
        ] {
            assert!(DataRow::parse(bad.clone()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn error_fields_and_vertica_codes() {
        let body = msg::error(&[
            (b'S', "ERROR"),
            (b'C', "42V01"),
            (b'M', "Relation \"nope\" does not exist"),
            (b'P', "15"),
            (b'V', "4566"),
            (b'Z', "unknown field"),
        ]);
        let Backend::Error(e) = Backend::parse(frame(&body), false).unwrap() else {
            panic!("not an ErrorResponse")
        };
        assert_eq!(e.sqlstate.as_deref(), Some("42V01"));
        assert_eq!(e.error_code, Some(4566));
        assert_eq!(e.position, Some(15));
        assert_eq!(e.message, "Relation \"nope\" does not exist");
        assert!(Backend::parse(frame(b"E\0\0\0\x07Sab"), false).is_err());
    }

    #[test]
    fn other_backend_messages() {
        let p = |b: &[u8]| Backend::parse(frame(b), false).unwrap();
        assert_eq!(
            p(&msg::key_data(12, 34)),
            Backend::BackendKey(BackendKey { pid: 12, key: 34 })
        );
        assert_eq!(
            p(&msg::ready(b'T')),
            Backend::Ready(TxStatus::InTransaction)
        );
        assert!(Backend::parse(frame(b"Z\0\0\0\x05X"), false).is_err());
        assert_eq!(
            p(&msg::parameter_status("protocol_version", "196624")),
            Backend::ParameterStatus {
                name: "protocol_version".into(),
                value: "196624".into()
            }
        );
        assert_eq!(
            p(&msg::command_description("INSERT", None)),
            Backend::CommandDescription {
                tag: "INSERT".into(),
                copy_rewrite: None
            }
        );
        assert_eq!(
            p(&msg::command_description("COPY", Some("COPY t FROM STDIN"))),
            Backend::CommandDescription {
                tag: "COPY".into(),
                copy_rewrite: Some("COPY t FROM STDIN".into())
            }
        );
        assert_eq!(p(&msg::tag(b'J', &[])), Backend::EndOfBatch);
        assert_eq!(p(&msg::tag(b'x', &[1, 2])), Backend::Unknown(b'x'));
        let mut lb = 5434u32.to_be_bytes().to_vec();
        lb.extend_from_slice(b"node2\0");
        assert_eq!(
            p(&msg::tag(b'Y', &lb)),
            Backend::LoadBalance {
                port: 5434,
                host: "node2".into()
            }
        );
        let params =
            msg::parameter_description(&[(0, 6, -1, false), (1, 0, -1, true)], &[(17, "geometry")]);
        let Backend::ParameterDescription(params) = p(&params) else {
            panic!("not a ParameterDescription")
        };
        assert_eq!(params[0].ty.oid, 6);
        assert_eq!(params[1].ty.user_type.as_deref(), Some("geometry"));
        assert!(params[1].not_null);
        // A pool index past the pool.
        let bad = msg::parameter_description(&[(1, 3, -1, false)], &[]);
        assert!(Backend::parse(frame(&bad), false).is_err());
    }
}
