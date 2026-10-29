//! A scripted PostgreSQL server on 127.0.0.1 for the unit tests of the PostgreSQL driver and of
//! the engines it serves (the pg-flavors lane). It speaks as much of the wire protocol as
//! tokio-postgres needs (cleartext password, simple and extended queries, cancel requests),
//! answers the statements it is told about with canned rows or errors, every other one with an
//! empty result, and records what it was sent.
//!
//! ```ignore
//! let pg = FakePg::new()
//!     .server_version("13.0.0")
//!     .version("CockroachDB CCL v24.2.4 (x86_64-pc-linux-gnu, built 2024/10/14, go1.22.5)")
//!     .rows("FROM pg_namespace", vec![Col::text("nspname")], vec![vec![Cell::text("public")]])
//!     .error("SET IntervalStyle", "42704", "unrecognized configuration parameter")
//!     .params("WHERE c.oid = $1", &[PgType::Int4])
//!     .start()
//!     .await;
//! // … connect to 127.0.0.1:pg.port() (password: anything), then:
//! assert_eq!(pg.queries()[0], "SELECT version()");
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

/// The types columns and parameters are described with: tokio-postgres knows these without a
/// catalog lookup. Recorded rows keep the server's types, so a decode that the server's answer
/// would fail fails here too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PgType {
    Text,
    Bool,
    Int8,
    Int4,
    /// `"char"`, the one-byte type of the catalog's codes (`relkind`, `contype`).
    Char,
    Numeric,
    Timestamp,
}

impl PgType {
    fn oid(self) -> u32 {
        match self {
            PgType::Text => 25,
            PgType::Bool => 16,
            PgType::Int8 => 20,
            PgType::Int4 => 23,
            PgType::Char => 18,
            PgType::Numeric => 1700,
            PgType::Timestamp => 1114,
        }
    }
}

/// A result column of a canned answer.
#[derive(Debug, Clone)]
pub(crate) struct Col {
    name: String,
    ty: PgType,
}

impl Col {
    pub(crate) fn text(name: &str) -> Col {
        Col::of(name, PgType::Text)
    }

    pub(crate) fn bool(name: &str) -> Col {
        Col::of(name, PgType::Bool)
    }

    pub(crate) fn int8(name: &str) -> Col {
        Col::of(name, PgType::Int8)
    }

    pub(crate) fn int4(name: &str) -> Col {
        Col::of(name, PgType::Int4)
    }

    /// A `"char"` column: its cells are one ASCII character (`Cell::text("r")`).
    pub(crate) fn char(name: &str) -> Col {
        Col::of(name, PgType::Char)
    }

    /// A numeric column: its cells are the server's text (`Cell::text("-1.50")`) or integers.
    pub(crate) fn numeric(name: &str) -> Col {
        Col::of(name, PgType::Numeric)
    }

    /// A timestamp column: its cells read `YYYY-MM-DD HH:MM:SS[.ffffff]` or `infinity`.
    pub(crate) fn timestamp(name: &str) -> Col {
        Col::of(name, PgType::Timestamp)
    }

    fn of(name: &str, ty: PgType) -> Col {
        Col {
            name: name.to_string(),
            ty,
        }
    }
}

/// A value of a canned row, encoded for its column's type.
#[derive(Debug, Clone)]
pub(crate) enum Cell {
    Null,
    Text(String),
    Bool(bool),
    Int(i64),
}

impl Cell {
    pub(crate) fn text(s: impl Into<String>) -> Cell {
        Cell::Text(s.into())
    }

    /// The value in the text (`binary: false`) or binary format of a column of type `ty`.
    fn encode(&self, ty: PgType, binary: bool) -> Option<Vec<u8>> {
        Some(match self {
            Cell::Null => return None,
            Cell::Text(s) if binary && ty == PgType::Numeric => numeric_binary(s),
            Cell::Text(s) if binary && ty == PgType::Timestamp => timestamp_binary(s),
            Cell::Text(s) => s.as_bytes().to_vec(),
            Cell::Bool(b) if binary => vec![u8::from(*b)],
            Cell::Bool(b) => if *b { b"t" } else { b"f" }.to_vec(),
            Cell::Int(n) if binary && ty == PgType::Int4 => (*n as i32).to_be_bytes().to_vec(),
            Cell::Int(n) if binary && ty == PgType::Numeric => numeric_binary(&n.to_string()),
            Cell::Int(n) if binary => n.to_be_bytes().to_vec(),
            Cell::Int(n) => n.to_string().into_bytes(),
        })
    }
}

/// A numeric's binary format: the count of base-10000 digits, the weight of the first, the sign
/// (`0x4000` negative, `0xC000` NaN), the scale, then the digits. `text` is the server's
/// text of it (`-12.50`, `NaN`).
fn numeric_binary(text: &str) -> Vec<u8> {
    let text = text.trim();
    let mut out = Vec::new();
    let mut put = |n: i32| out.extend((n as i16).to_be_bytes());
    if text.eq_ignore_ascii_case("nan") {
        for n in [0, 0, 0xC000, 0] {
            put(n);
        }
        return out;
    }
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    assert!(
        (int.chars().chain(frac.chars())).all(|c| c.is_ascii_digit()),
        "a numeric cell holds the server's text of a number, not {text:?}"
    );
    let int = int.trim_start_matches('0');
    // Groups of four digits from the decimal point outwards.
    let int_pad = (4 - int.len() % 4) % 4;
    let int_digits = format!("{}{int}", "0".repeat(int_pad));
    let frac_digits = format!("{frac}{}", "0".repeat((4 - frac.len() % 4) % 4));
    let group = |s: &str| -> Vec<i32> {
        (0..s.len() / 4)
            .map(|i| s[4 * i..4 * i + 4].parse().unwrap_or(0))
            .collect()
    };
    let mut groups = group(&int_digits);
    let mut weight = groups.len() as i32 - 1;
    groups.extend(group(&frac_digits));
    while groups.first() == Some(&0) {
        groups.remove(0);
        weight -= 1;
    }
    while groups.last() == Some(&0) {
        groups.pop();
    }
    if groups.is_empty() {
        weight = 0;
    }
    let sign = if negative && !groups.is_empty() {
        0x4000
    } else {
        0
    };
    for n in [groups.len() as i32, weight, sign, frac.len() as i32] {
        put(n);
    }
    for g in groups {
        put(g);
    }
    out
}

/// A timestamp's binary format: microseconds since 2000-01-01 00:00:00.
fn timestamp_binary(text: &str) -> Vec<u8> {
    let micros = match text.trim() {
        "infinity" => i64::MAX,
        "-infinity" => i64::MIN,
        t => {
            let at = chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S%.f")
                .unwrap_or_else(|e| {
                    panic!("a timestamp cell reads YYYY-MM-DD HH:MM:SS: {t:?}: {e}")
                });
            let epoch = chrono::NaiveDate::from_ymd_opt(2000, 1, 1)
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .expect("the PostgreSQL epoch");
            (at - epoch)
                .num_microseconds()
                .expect("a timestamp in range")
        }
    };
    micros.to_be_bytes().to_vec()
}

#[derive(Debug, Clone)]
enum Reply {
    Rows {
        columns: Vec<Col>,
        rows: Vec<Vec<Cell>>,
        tag: String,
    },
    Error {
        code: String,
        message: String,
    },
}

/// A canned answer to the statements whose text contains `contains`, `uses` times (or always).
#[derive(Debug, Clone)]
struct Rule {
    contains: String,
    reply: Reply,
    uses: Option<usize>,
}

#[derive(Default)]
struct Script {
    rules: Vec<Rule>,
    /// The parameter types of the statements whose text contains the first (`FakePg::params`).
    params: Vec<(String, Vec<PgType>)>,
    /// Sent as the `server_version` parameter at startup.
    server_version: Option<String>,
}

impl Script {
    /// The declared parameter types of `sql`, if a declaration matches it.
    fn params(&self, sql: &str) -> Vec<PgType> {
        self.params
            .iter()
            .find(|(contains, _)| sql.contains(contains.as_str()))
            .map(|(_, types)| types.clone())
            .unwrap_or_default()
    }

    /// The first rule matching `sql` that has uses left (taking one).
    fn answer(&mut self, sql: &str) -> Option<Reply> {
        let rule = self
            .rules
            .iter_mut()
            .find(|r| sql.contains(&r.contains) && r.uses != Some(0))?;
        if let Some(n) = &mut rule.uses {
            *n -= 1;
        }
        Some(rule.reply.clone())
    }
}

#[derive(Default)]
struct Log {
    /// The process id of the connection, and the text of every simple query and every parsed
    /// statement, in arrival order.
    queries: Vec<(i32, String)>,
    passwords: Vec<String>,
    /// Cancel requests: `(process id, secret key)`.
    cancels: Vec<(i32, i32)>,
    /// The startup parameters (`user`, `database`, `application_name`…) of each connection.
    startups: Vec<Vec<(String, String)>>,
}

/// The fake server: built with the answers it gives, then `start`ed.
#[derive(Clone, Default)]
pub(crate) struct FakePg {
    script: Arc<Mutex<Script>>,
    log: Arc<Mutex<Log>>,
    port: u16,
}

/// The secret key of every connection's `BackendKeyData`.
const SECRET_KEY: i32 = 42;

impl FakePg {
    pub(crate) fn new() -> FakePg {
        FakePg::default()
    }

    /// Reports `server_version` at startup (nothing by default).
    pub(crate) fn server_version(self, version: &str) -> FakePg {
        self.script.lock().unwrap().server_version = Some(version.to_string());
        self
    }

    /// Answers `SELECT version()` with `version`, and the driver's `server_info` query with it,
    /// database `tabler` and user `tabler`.
    pub(crate) fn version(self, version: &str) -> FakePg {
        self.rows(
            "version()::text, current_database()",
            vec![
                Col::text("version"),
                Col::text("current_database"),
                Col::text("current_user"),
            ],
            vec![vec![
                Cell::text(version),
                Cell::text("tabler"),
                Cell::text("tabler"),
            ]],
        )
        .rows(
            "version()",
            vec![Col::text("version")],
            vec![vec![Cell::text(version)]],
        )
    }

    /// Answers the statements containing `contains` with `rows`.
    pub(crate) fn rows(self, contains: &str, columns: Vec<Col>, rows: Vec<Vec<Cell>>) -> FakePg {
        let tag = format!("SELECT {}", rows.len());
        self.rule(contains, Reply::Rows { columns, rows, tag }, None)
    }

    /// Answers the statements containing `contains` with a command tag (`UPDATE 1`…).
    pub(crate) fn command(self, contains: &str, tag: &str) -> FakePg {
        let reply = Reply::Rows {
            columns: Vec::new(),
            rows: Vec::new(),
            tag: tag.to_string(),
        };
        self.rule(contains, reply, None)
    }

    /// Fails the statements containing `contains` with SQLSTATE `code`.
    pub(crate) fn error(self, contains: &str, code: &str, message: &str) -> FakePg {
        self.error_times(contains, code, message, None)
    }

    /// `error`, for the first `times` such statements only (`None`: all of them).
    pub(crate) fn error_times(
        self,
        contains: &str,
        code: &str,
        message: &str,
        times: Option<usize>,
    ) -> FakePg {
        let reply = Reply::Error {
            code: code.to_string(),
            message: message.to_string(),
        };
        self.rule(contains, reply, times)
    }

    /// Describes the parameters of the statements containing `contains` with `types` (by
    /// position) where the client leaves their type to the server, as the server infers them;
    /// any other parameter is text.
    pub(crate) fn params(self, contains: &str, types: &[PgType]) -> FakePg {
        self.script
            .lock()
            .unwrap()
            .params
            .push((contains.to_string(), types.to_vec()));
        self
    }

    fn rule(self, contains: &str, reply: Reply, uses: Option<usize>) -> FakePg {
        self.script.lock().unwrap().rules.push(Rule {
            contains: contains.to_string(),
            reply,
            uses,
        });
        self
    }

    /// Listens on 127.0.0.1 (a free port) and serves every connection.
    pub(crate) async fn start(mut self) -> FakePg {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.port = listener.local_addr().unwrap().port();
        let fake = self.clone();
        tokio::spawn(async move {
            let mut pid = 100;
            while let Ok((socket, _)) = listener.accept().await {
                pid += 1;
                tokio::spawn(serve(socket, fake.clone(), pid));
            }
        });
        self
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// The text of every simple query and parsed statement, in arrival order.
    pub(crate) fn queries(&self) -> Vec<String> {
        self.statements().into_iter().map(|(_, sql)| sql).collect()
    }

    /// `queries` with the process id of the connection each one came on.
    pub(crate) fn statements(&self) -> Vec<(i32, String)> {
        self.log.lock().unwrap().queries.clone()
    }

    /// The passwords of the connections, in the order they logged in.
    pub(crate) fn passwords(&self) -> Vec<String> {
        self.log.lock().unwrap().passwords.clone()
    }

    /// The cancel requests received: `(process id, secret key)`.
    pub(crate) fn cancels(&self) -> Vec<(i32, i32)> {
        self.log.lock().unwrap().cancels.clone()
    }

    /// The startup parameters of each connection, in the order they connected.
    pub(crate) fn startups(&self) -> Vec<Vec<(String, String)>> {
        self.log.lock().unwrap().startups.clone()
    }

    fn answer(&self, pid: i32, sql: &str) -> Option<Reply> {
        self.log
            .lock()
            .unwrap()
            .queries
            .push((pid, sql.to_string()));
        self.script.lock().unwrap().answer(sql)
    }
}

/// A backend message: its tag, the length and `body`.
fn message(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend((body.len() as i32 + 4).to_be_bytes());
    out.extend_from_slice(body);
    out
}

fn cstr(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

/// Reads NUL-terminated strings and big-endian integers from a frontend message body.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn cstr(&mut self) -> String {
        let end = self.0.iter().position(|b| *b == 0).unwrap_or(self.0.len());
        let s = String::from_utf8_lossy(&self.0[..end]).into_owned();
        self.0 = self.0.get(end + 1..).unwrap_or_default();
        s
    }

    fn i16(&mut self) -> i16 {
        let v = i16::from_be_bytes(self.0[..2].try_into().unwrap());
        self.0 = &self.0[2..];
        v
    }

    fn i32(&mut self) -> i32 {
        let v = i32::from_be_bytes(self.0[..4].try_into().unwrap());
        self.0 = &self.0[4..];
        v
    }

    fn skip(&mut self, n: usize) {
        self.0 = &self.0[n..];
    }
}

fn row_description(columns: &[Col]) -> Vec<u8> {
    let mut body = (columns.len() as i16).to_be_bytes().to_vec();
    for c in columns {
        cstr(&mut body, &c.name);
        body.extend(0i32.to_be_bytes()); // table oid
        body.extend(0i16.to_be_bytes()); // column number
        body.extend(c.ty.oid().to_be_bytes());
        body.extend((-1i16).to_be_bytes()); // type size
        body.extend((-1i32).to_be_bytes()); // type modifier
        body.extend(0i16.to_be_bytes()); // format
    }
    message(b'T', &body)
}

fn data_row(columns: &[Col], row: &[Cell], binary: &dyn Fn(usize) -> bool) -> Vec<u8> {
    let mut body = (row.len() as i16).to_be_bytes().to_vec();
    for (i, cell) in row.iter().enumerate() {
        let ty = columns.get(i).map_or(PgType::Text, |c| c.ty);
        match cell.encode(ty, binary(i)) {
            Some(bytes) => {
                body.extend((bytes.len() as i32).to_be_bytes());
                body.extend(bytes);
            }
            None => body.extend((-1i32).to_be_bytes()),
        }
    }
    message(b'D', &body)
}

fn error_response(code: &str, text: &str) -> Vec<u8> {
    let mut body = Vec::new();
    for (field, value) in [(b'S', "ERROR"), (b'V', "ERROR"), (b'C', code), (b'M', text)] {
        body.push(field);
        cstr(&mut body, value);
    }
    body.push(0);
    message(b'E', &body)
}

fn command_complete(tag: &str) -> Vec<u8> {
    let mut body = Vec::new();
    cstr(&mut body, tag);
    message(b'C', &body)
}

const READY: [u8; 6] = [b'Z', 0, 0, 0, 5, b'I'];

/// The number of parameters a statement uses: its highest `$n`.
fn parameter_count(sql: &str) -> usize {
    let bytes = sql.as_bytes();
    let mut max = 0;
    for (i, b) in bytes.iter().enumerate() {
        if *b != b'$' {
            continue;
        }
        let digits: String = bytes[i + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .map(|c| *c as char)
            .collect();
        max = max.max(digits.parse().unwrap_or(0));
    }
    max
}

/// A parsed statement: its parameter types and its answer.
struct Prepared {
    params: Vec<u32>,
    reply: Option<Reply>,
}

async fn body(socket: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let len = socket.read_i32().await?;
    let mut body = vec![0u8; usize::try_from(len - 4).unwrap_or(0)];
    socket.read_exact(&mut body).await?;
    Ok(body)
}

async fn serve(mut socket: TcpStream, fake: FakePg, pid: i32) -> std::io::Result<()> {
    let startup = loop {
        let startup = body(&mut socket).await?;
        let mut r = Reader(&startup);
        match r.i32() {
            // CancelRequest: record it; closing the socket tells the client it was handled.
            80_877_102 => {
                let cancel = (r.i32(), r.i32());
                fake.log.lock().unwrap().cancels.push(cancel);
                return Ok(());
            }
            // SSLRequest / GSSENCRequest: not supported.
            80_877_103 | 80_877_104 => socket.write_all(b"N").await?,
            _ => break startup,
        }
    };
    let mut r = Reader(&startup[4..]);
    let mut params = Vec::new();
    loop {
        let name = r.cstr();
        if name.is_empty() {
            break;
        }
        params.push((name, r.cstr()));
    }
    fake.log.lock().unwrap().startups.push(params);
    // AuthenticationCleartextPassword, then the PasswordMessage.
    socket.write_all(&[b'R', 0, 0, 0, 8, 0, 0, 0, 3]).await?;
    let _tag = socket.read_u8().await?;
    let password = body(&mut socket).await?;
    let password = password.strip_suffix(&[0]).unwrap_or(&password);
    fake.log
        .lock()
        .unwrap()
        .passwords
        .push(String::from_utf8_lossy(password).into_owned());
    // AuthenticationOk, ParameterStatus, BackendKeyData, ReadyForQuery.
    let mut out = message(b'R', &0i32.to_be_bytes());
    let server_version = fake.script.lock().unwrap().server_version.clone();
    if let Some(version) = server_version {
        let mut status = Vec::new();
        cstr(&mut status, "server_version");
        cstr(&mut status, &version);
        out.extend(message(b'S', &status));
    }
    let mut key = pid.to_be_bytes().to_vec();
    key.extend(SECRET_KEY.to_be_bytes());
    out.extend(message(b'K', &key));
    out.extend(READY);
    socket.write_all(&out).await?;

    let mut statements: HashMap<String, Prepared> = HashMap::new();
    // Portal name → (statement name, result formats).
    let mut portals: HashMap<String, (String, Vec<i16>)> = HashMap::new();
    // After an error the extended protocol skips everything up to the next Sync.
    let mut failed = false;
    loop {
        let tag = socket.read_u8().await?;
        let msg = body(&mut socket).await?;
        let mut r = Reader(&msg);
        let mut out = Vec::new();
        match tag {
            b'X' => return Ok(()),
            b'Q' => {
                let sql = r.cstr();
                match fake.answer(pid, &sql) {
                    Some(Reply::Rows { columns, rows, tag }) => {
                        if !columns.is_empty() {
                            out.extend(row_description(&columns));
                        }
                        for row in &rows {
                            out.extend(data_row(&columns, row, &|_| false));
                        }
                        out.extend(command_complete(&tag));
                    }
                    Some(Reply::Error { code, message }) => {
                        out.extend(error_response(&code, &message))
                    }
                    // EmptyQueryResponse.
                    None => out.extend(message(b'I', &[])),
                }
                out.extend(READY);
            }
            b'S' => {
                failed = false;
                out.extend(READY);
            }
            _ if failed => {}
            b'P' => {
                let name = r.cstr();
                let sql = r.cstr();
                let mut params: Vec<u32> = (0..r.i16()).map(|_| r.i32() as u32).collect();
                let declared = fake.script.lock().unwrap().params(&sql);
                params.resize(
                    params.len().max(parameter_count(&sql)).max(declared.len()),
                    0,
                );
                for (i, p) in params.iter_mut().enumerate() {
                    if *p == 0 {
                        *p = declared.get(i).copied().unwrap_or(PgType::Text).oid();
                    }
                }
                let reply = fake.answer(pid, &sql);
                statements.insert(name, Prepared { params, reply });
                out.extend(message(b'1', &[]));
            }
            b'D' => {
                let kind = r.0[0];
                r.skip(1);
                let name = r.cstr();
                let statement = match kind {
                    b'S' => Some(name),
                    _ => portals.get(&name).map(|(s, _)| s.clone()),
                };
                let prepared = statement.and_then(|s| statements.get(&s));
                if kind == b'S' {
                    let params = prepared.map(|p| p.params.clone()).unwrap_or_default();
                    let mut body = (params.len() as i16).to_be_bytes().to_vec();
                    for p in params {
                        body.extend(p.to_be_bytes());
                    }
                    out.extend(message(b't', &body));
                }
                match prepared.and_then(|p| p.reply.as_ref()) {
                    Some(Reply::Rows { columns, .. }) if !columns.is_empty() => {
                        out.extend(row_description(columns))
                    }
                    // NoData.
                    _ => out.extend(message(b'n', &[])),
                }
            }
            b'B' => {
                let portal = r.cstr();
                let statement = r.cstr();
                let formats = r.i16();
                r.skip(2 * formats as usize);
                for _ in 0..r.i16() {
                    let len = r.i32();
                    r.skip(len.max(0) as usize);
                }
                let results: Vec<i16> = (0..r.i16()).map(|_| r.i16()).collect();
                portals.insert(portal, (statement, results));
                out.extend(message(b'2', &[]));
            }
            b'E' => {
                let portal = r.cstr();
                let (statement, results) = portals.get(&portal).cloned().unwrap_or_default();
                let binary = |i: usize| match results.as_slice() {
                    [] => false,
                    [one] => *one == 1,
                    many => many.get(i) == Some(&1),
                };
                match statements.get(&statement).and_then(|p| p.reply.as_ref()) {
                    Some(Reply::Rows { columns, rows, tag }) => {
                        for row in rows {
                            out.extend(data_row(columns, row, &binary));
                        }
                        out.extend(command_complete(tag));
                    }
                    Some(Reply::Error { code, message }) => {
                        out.extend(error_response(code, message));
                        failed = true;
                    }
                    None => out.extend(command_complete("SELECT 0")),
                }
            }
            // Close: CloseComplete.
            b'C' => {
                let kind = r.0[0];
                r.skip(1);
                let name = r.cstr();
                if kind == b'S' {
                    statements.remove(&name);
                } else {
                    portals.remove(&name);
                }
                out.extend(message(b'3', &[]));
            }
            _ => {}
        }
        if !out.is_empty() {
            socket.write_all(&out).await?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_the_parameters_a_statement_uses() {
        assert_eq!(parameter_count("SELECT 1"), 0);
        assert_eq!(parameter_count("SELECT $1, $2 WHERE x = $1"), 2);
        assert_eq!(parameter_count("SELECT $$x$$, $12"), 12);
    }

    #[test]
    fn encodes_numerics_and_timestamps_as_the_server_does() {
        let words = |n: &[i16]| -> Vec<u8> { n.iter().flat_map(|w| w.to_be_bytes()).collect() };
        // ndigits, weight, sign, dscale, digits (base 10000).
        assert_eq!(
            numeric_binary("-12345.678"),
            words(&[3, 1, 0x4000, 3, 1, 2345, 6780])
        );
        assert_eq!(numeric_binary("0.001"), words(&[1, -1, 0, 3, 10]));
        assert_eq!(numeric_binary("0.00001"), words(&[1, -2, 0, 5, 1000]));
        assert_eq!(numeric_binary("10000"), words(&[1, 1, 0, 0, 1]));
        assert_eq!(numeric_binary("0.50"), words(&[1, -1, 0, 2, 5000]));
        assert_eq!(numeric_binary("0"), words(&[0, 0, 0, 0]));
        assert_eq!(numeric_binary("-0.0"), words(&[0, 0, 0, 1]));
        assert_eq!(numeric_binary("NaN"), words(&[0, 0, 0xC000u16 as i16, 0]));
        assert_eq!(timestamp_binary("2000-01-01 00:00:00"), 0i64.to_be_bytes());
        assert_eq!(
            timestamp_binary("2000-01-02 00:00:01.5"),
            (86_401_500_000i64).to_be_bytes()
        );
        assert_eq!(timestamp_binary("infinity"), i64::MAX.to_be_bytes());
    }

    /// Recorded rows keep the server's types: "char", numeric and timestamp columns in both
    /// formats, and parameters of the types the server infers.
    #[tokio::test]
    async fn answers_with_the_servers_types() {
        use tokio_postgres::types::{FromSql, Type};

        /// A value's bytes as the server sent them.
        struct Raw(Vec<u8>);
        impl<'a> FromSql<'a> for Raw {
            fn from_sql(
                _: &Type,
                raw: &'a [u8],
            ) -> Result<Raw, Box<dyn std::error::Error + Sync + Send>> {
                Ok(Raw(raw.to_vec()))
            }
            fn accepts(_: &Type) -> bool {
                true
            }
        }

        crate::testkit::install_crypto();
        let row = vec![
            Cell::text("r"),
            Cell::text("-12345.678"),
            Cell::Int(42),
            Cell::text("2026-10-29 10:00:00.5"),
        ];
        let pg = FakePg::new()
            .rows(
                "FROM pg_class",
                vec![
                    Col::char("relkind"),
                    Col::numeric("size"),
                    Col::numeric("rows"),
                    Col::timestamp("created"),
                ],
                vec![row],
            )
            .params("WHERE c.oid = $1", &[PgType::Int4])
            .start()
            .await;
        let mut config = tokio_postgres::Config::new();
        config
            .host("127.0.0.1")
            .port(pg.port())
            .user("u")
            .password("pw")
            .dbname("d");
        let (client, connection) = config.connect(tokio_postgres::NoTls).await.unwrap();
        tokio::spawn(connection);

        let sql = "SELECT relkind, size, rows, created FROM pg_class c WHERE c.oid = $1";
        let rows = client.query(sql, &[&7i32]).await.unwrap();
        let stmt = client.prepare(sql).await.unwrap();
        assert_eq!(stmt.params(), [Type::INT4]);
        let types: Vec<Type> = stmt.columns().iter().map(|c| c.type_().clone()).collect();
        assert_eq!(
            types,
            [Type::CHAR, Type::NUMERIC, Type::NUMERIC, Type::TIMESTAMP]
        );
        assert_eq!(rows[0].get::<_, i8>(0), b'r' as i8);
        assert_eq!(rows[0].get::<_, Raw>(1).0, numeric_binary("-12345.678"));
        assert_eq!(rows[0].get::<_, Raw>(2).0, numeric_binary("42"));
        let decoded = crate::drivers::postgres::decode::decode_row(&rows[0]).unwrap();
        assert_eq!(decoded[0], crate::types::value::Value::Text("r".into()));
        assert_eq!(
            decoded[3],
            crate::types::value::Value::Timestamp("2026-10-29 10:00:00.5".into())
        );
        // A text parameter where none is declared: the client cannot bind an int there.
        let undeclared = "SELECT relkind, size, rows, created FROM pg_class c WHERE c.relname = $1";
        assert!(client.query(undeclared, &[&7i32]).await.is_err());

        // The simple protocol carries the server's text.
        let simple = client.simple_query(sql).await.unwrap();
        let text: Vec<Option<String>> = simple
            .iter()
            .find_map(|m| match m {
                tokio_postgres::SimpleQueryMessage::Row(row) => {
                    Some((0..4).map(|i| row.get(i).map(str::to_string)).collect())
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(
            text,
            ["r", "-12345.678", "42", "2026-10-29 10:00:00.5"].map(|s| Some(s.to_string()))
        );
    }

    #[tokio::test]
    async fn answers_simple_and_extended_queries_as_scripted() {
        crate::testkit::install_crypto();
        let pg = FakePg::new()
            .server_version("17.2")
            .version("PostgreSQL 17.2")
            .rows(
                "FROM t",
                vec![
                    Col::text("a"),
                    Col::bool("b"),
                    Col::int8("c"),
                    Col::int4("d"),
                ],
                vec![
                    vec![
                        Cell::text("x"),
                        Cell::Bool(true),
                        Cell::Int(1 << 40),
                        Cell::Int(7),
                    ],
                    vec![Cell::Null, Cell::Bool(false), Cell::Null, Cell::Int(-1)],
                ],
            )
            .error_times("UPDATE t", "40001", "restart transaction", Some(1))
            .command("UPDATE t", "UPDATE 3")
            .start()
            .await;
        let mut config = tokio_postgres::Config::new();
        config
            .host("127.0.0.1")
            .port(pg.port())
            .user("u")
            .password("pw")
            .dbname("d")
            .ssl_mode(tokio_postgres::config::SslMode::Disable);
        let (client, connection) = config.connect(tokio_postgres::NoTls).await.unwrap();
        assert_eq!(connection.parameter("server_version"), Some("17.2"));
        tokio::spawn(connection);

        let simple = client.simple_query("SELECT version()").await.unwrap();
        let version = simple.iter().find_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(row) => row.get(0).map(str::to_string),
            _ => None,
        });
        assert_eq!(version.as_deref(), Some("PostgreSQL 17.2"));

        let rows = client
            .query("SELECT a, b, c, d FROM t WHERE a = $1", &[&"x"])
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get::<_, Option<String>>(0).as_deref(), Some("x"));
        assert!(rows[0].get::<_, bool>(1));
        assert_eq!(rows[0].get::<_, Option<i64>>(2), Some(1 << 40));
        assert_eq!(rows[1].get::<_, i32>(3), -1);
        assert_eq!(rows[1].get::<_, Option<String>>(0), None);
        // Anything else: an empty result.
        assert!(client.query("SELECT 1", &[]).await.unwrap().is_empty());
        client.batch_execute("SET x = 1; SET y = 2").await.unwrap();

        let failed = client.execute("UPDATE t SET a = 'y'", &[]).await;
        assert_eq!(
            failed.unwrap_err().code(),
            Some(&tokio_postgres::error::SqlState::T_R_SERIALIZATION_FAILURE)
        );
        assert_eq!(
            client.execute("UPDATE t SET a = 'y'", &[]).await.unwrap(),
            3
        );

        assert_eq!(pg.passwords(), ["pw"]);
        let startup = &pg.startups()[0];
        assert!(
            startup.contains(&("database".into(), "d".into())),
            "{startup:?}"
        );
        assert_eq!(
            pg.queries(),
            [
                "SELECT version()",
                "SELECT a, b, c, d FROM t WHERE a = $1",
                "SELECT 1",
                "SET x = 1; SET y = 2",
                "UPDATE t SET a = 'y'",
                "UPDATE t SET a = 'y'",
            ]
        );
    }
}
