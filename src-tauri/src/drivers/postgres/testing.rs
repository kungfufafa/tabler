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
//!     .start()
//!     .await;
//! // … connect to 127.0.0.1:pg.port() (password: anything), then:
//! assert_eq!(pg.queries()[0], "SELECT version()");
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

/// Type OIDs columns are described with: tokio-postgres knows these without a catalog lookup.
const TEXT: u32 = 25;
const BOOL: u32 = 16;
const INT8: u32 = 20;
const INT4: u32 = 23;

/// A result column of a canned answer.
#[derive(Debug, Clone)]
pub(crate) struct Col {
    name: String,
    oid: u32,
}

impl Col {
    pub(crate) fn text(name: &str) -> Col {
        Col::of(name, TEXT)
    }

    pub(crate) fn bool(name: &str) -> Col {
        Col::of(name, BOOL)
    }

    pub(crate) fn int8(name: &str) -> Col {
        Col::of(name, INT8)
    }

    pub(crate) fn int4(name: &str) -> Col {
        Col::of(name, INT4)
    }

    fn of(name: &str, oid: u32) -> Col {
        Col {
            name: name.to_string(),
            oid,
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

    /// The value in the text (`binary: false`) or binary format of a column of type `oid`.
    fn encode(&self, oid: u32, binary: bool) -> Option<Vec<u8>> {
        Some(match self {
            Cell::Null => return None,
            Cell::Text(s) => s.as_bytes().to_vec(),
            Cell::Bool(b) if binary => vec![u8::from(*b)],
            Cell::Bool(b) => if *b { b"t" } else { b"f" }.to_vec(),
            Cell::Int(n) if binary && oid == INT4 => (*n as i32).to_be_bytes().to_vec(),
            Cell::Int(n) if binary => n.to_be_bytes().to_vec(),
            Cell::Int(n) => n.to_string().into_bytes(),
        })
    }
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
    /// Sent as the `server_version` parameter at startup.
    server_version: Option<String>,
}

impl Script {
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
        body.extend(c.oid.to_be_bytes());
        body.extend((-1i16).to_be_bytes()); // type size
        body.extend((-1i32).to_be_bytes()); // type modifier
        body.extend(0i16.to_be_bytes()); // format
    }
    message(b'T', &body)
}

fn data_row(columns: &[Col], row: &[Cell], binary: &dyn Fn(usize) -> bool) -> Vec<u8> {
    let mut body = (row.len() as i16).to_be_bytes().to_vec();
    for (i, cell) in row.iter().enumerate() {
        let oid = columns.get(i).map_or(TEXT, |c| c.oid);
        match cell.encode(oid, binary(i)) {
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
                params.resize(params.len().max(parameter_count(&sql)), 0);
                for p in &mut params {
                    if *p == 0 {
                        *p = TEXT;
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
