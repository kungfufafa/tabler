//! An in-process Redis for the lane's tests: a tokio TCP server speaking RESP2, and RESP3 after
//! `HELLO 3`, over a small keyspace of every key type. It has AUTH and HELLO … AUTH (ACL user
//! and password), SELECT, SCAN with MATCH / COUNT / TYPE (pages of uneven size, cursors as
//! positions), expiry, MULTI / EXEC / WATCH with Redis's semantics (no rollback, a nil EXEC when a
//! watched key changed), CLIENT ID / KILL and DEBUG SLEEP for cancel tests, and the INFO /
//! CONFIG / COMMAND replies introspection reads. It answers like Redis 7.2; `Options` pick the
//! version (older ones lack HELLO, KEEPTTL, SCAN TYPE, COPY, UNLINK), a product, a password and
//! a CONFIG refused like on managed services.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::info::Version;

/// A stream entry: its id and its field / value pairs.
pub type StreamEntry = (String, Vec<(Vec<u8>, Vec<u8>)>);

/// A value of the fake keyspace.
#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    Str(Vec<u8>),
    List(VecDeque<Vec<u8>>),
    /// Fields in insertion order.
    Hash(Vec<(Vec<u8>, Vec<u8>)>),
    /// Members in insertion order.
    Set(Vec<Vec<u8>>),
    /// Kept sorted by score, then member.
    ZSet(Vec<(Vec<u8>, f64)>),
    Stream(Vec<StreamEntry>),
    Json(String),
    /// A module type: TYPE names it, nothing reads it.
    Module(String),
}

impl Data {
    pub fn type_name(&self) -> &str {
        match self {
            Data::Str(_) => "string",
            Data::List(_) => "list",
            Data::Hash(_) => "hash",
            Data::Set(_) => "set",
            Data::ZSet(_) => "zset",
            Data::Stream(_) => "stream",
            Data::Json(_) => "ReJSON-RL",
            Data::Module(name) => name,
        }
    }

    fn weight(&self) -> usize {
        match self {
            Data::Str(v) => v.len(),
            Data::List(l) => l.iter().map(Vec::len).sum(),
            Data::Hash(h) => h.iter().map(|(f, v)| f.len() + v.len()).sum(),
            Data::Set(s) => s.iter().map(Vec::len).sum(),
            Data::ZSet(z) => z.iter().map(|(m, _)| m.len() + 8).sum(),
            Data::Stream(s) => s.len() * 32,
            Data::Json(j) => j.len(),
            Data::Module(_) => 64,
        }
    }

    pub fn str(s: &str) -> Data {
        Data::Str(s.as_bytes().to_vec())
    }

    pub fn hash(pairs: &[(&str, &str)]) -> Data {
        Data::Hash(
            pairs
                .iter()
                .map(|(f, v)| (f.as_bytes().to_vec(), v.as_bytes().to_vec()))
                .collect(),
        )
    }

    pub fn list(items: &[&str]) -> Data {
        Data::List(items.iter().map(|s| s.as_bytes().to_vec()).collect())
    }

    pub fn set(items: &[&str]) -> Data {
        Data::Set(items.iter().map(|s| s.as_bytes().to_vec()).collect())
    }

    pub fn zset(items: &[(&str, f64)]) -> Data {
        let mut z: Vec<(Vec<u8>, f64)> = items
            .iter()
            .map(|(m, s)| (m.as_bytes().to_vec(), *s))
            .collect();
        sort_zset(&mut z);
        Data::ZSet(z)
    }

    pub fn stream(entries: &[(&str, &[(&str, &str)])]) -> Data {
        Data::Stream(
            entries
                .iter()
                .map(|(id, fields)| {
                    (
                        id.to_string(),
                        fields
                            .iter()
                            .map(|(f, v)| (f.as_bytes().to_vec(), v.as_bytes().to_vec()))
                            .collect(),
                    )
                })
                .collect(),
        )
    }
}

fn sort_zset(z: &mut [(Vec<u8>, f64)]) {
    z.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
}

/// How the fake presents itself.
#[derive(Debug, Clone)]
pub struct Options {
    /// `redis_version` (decides which commands exist).
    pub version: String,
    /// Another product's `(name, version)`: `("valkey", "8.0.1")`.
    pub product: Option<(String, String)>,
    /// The ACL user AUTH accepts (`default` when `None`).
    pub user: Option<String>,
    pub password: Option<String>,
    /// CONFIG answers "unknown command" (ElastiCache, Upstash).
    pub refuse_config: bool,
    pub databases: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            version: "7.2.4".into(),
            product: None,
            user: None,
            password: None,
            refuse_config: false,
            databases: 16,
        }
    }
}

/// A reply, encoded per protocol.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Ok,
    Simple(String),
    Err(String),
    Int(i64),
    Bulk(Vec<u8>),
    Nil,
    /// RESP2's `*-1` (an aborted EXEC).
    NilArray,
    Array(Vec<Reply>),
    Map(Vec<(Reply, Reply)>),
    Set(Vec<Reply>),
    Double(f64),
    Bool(bool),
    Verbatim(String),
}

impl Reply {
    fn err(msg: &str) -> Reply {
        Reply::Err(msg.to_string())
    }

    fn bulk(b: &[u8]) -> Reply {
        Reply::Bulk(b.to_vec())
    }

    fn text(s: &str) -> Reply {
        Reply::Bulk(s.as_bytes().to_vec())
    }

    pub fn encode(&self, resp3: bool, out: &mut Vec<u8>) {
        let line = |out: &mut Vec<u8>, prefix: char, body: &str| {
            out.extend_from_slice(format!("{prefix}{body}\r\n").as_bytes());
        };
        match self {
            Reply::Ok => line(out, '+', "OK"),
            Reply::Simple(s) => line(out, '+', s),
            Reply::Err(e) => line(out, '-', e),
            Reply::Int(n) => line(out, ':', &n.to_string()),
            Reply::Bulk(b) => {
                line(out, '$', &b.len().to_string());
                out.extend_from_slice(b);
                out.extend_from_slice(b"\r\n");
            }
            Reply::Nil if resp3 => line(out, '_', ""),
            Reply::Nil => line(out, '$', "-1"),
            Reply::NilArray if resp3 => line(out, '_', ""),
            Reply::NilArray => line(out, '*', "-1"),
            Reply::Array(items) => {
                line(out, '*', &items.len().to_string());
                for i in items {
                    i.encode(resp3, out);
                }
            }
            Reply::Set(items) => {
                line(out, if resp3 { '~' } else { '*' }, &items.len().to_string());
                for i in items {
                    i.encode(resp3, out);
                }
            }
            Reply::Map(pairs) if resp3 => {
                line(out, '%', &pairs.len().to_string());
                for (k, v) in pairs {
                    k.encode(resp3, out);
                    v.encode(resp3, out);
                }
            }
            Reply::Map(pairs) => {
                line(out, '*', &(pairs.len() * 2).to_string());
                for (k, v) in pairs {
                    k.encode(resp3, out);
                    v.encode(resp3, out);
                }
            }
            Reply::Double(f) if resp3 => line(out, ',', &format_double(*f)),
            Reply::Double(f) => Reply::text(&format_double(*f)).encode(resp3, out),
            Reply::Bool(b) if resp3 => line(out, '#', if *b { "t" } else { "f" }),
            Reply::Bool(b) => line(out, ':', if *b { "1" } else { "0" }),
            Reply::Verbatim(text) if resp3 => {
                line(out, '=', &(text.len() + 4).to_string());
                out.extend_from_slice(b"txt:");
                out.extend_from_slice(text.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            Reply::Verbatim(text) => Reply::text(text).encode(resp3, out),
        }
    }
}

/// A score as Redis writes it: `1`, `1.5`, `inf`, `-inf`.
fn format_double(f: f64) -> String {
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.into();
    }
    if f.fract() == 0.0 && f.abs() < 1e15 {
        return format!("{}", f as i64);
    }
    format!("{f}")
}

/// A request's argv and the bytes it took.
pub type Request = (Vec<Vec<u8>>, usize);

/// One request from the buffer: a RESP array of bulk strings or an inline line. `Ok(None)` while
/// it is incomplete.
pub fn parse_request(buf: &[u8]) -> Result<Option<Request>, String> {
    let Some(first) = buf.first() else {
        return Ok(None);
    };
    let line_end = |from: usize| {
        buf[from..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .map(|p| from + p)
    };
    if *first != b'*' {
        let Some(end) = buf.iter().position(|&b| b == b'\n') else {
            return Ok(None);
        };
        let line = String::from_utf8_lossy(&buf[..end]);
        let argv = line
            .split_ascii_whitespace()
            .map(|s| s.as_bytes().to_vec())
            .collect();
        return Ok(Some((argv, end + 1)));
    }
    let Some(end) = line_end(0) else {
        return Ok(None);
    };
    let n: usize = std::str::from_utf8(&buf[1..end])
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or("Protocol error: invalid multibulk length")?;
    let mut pos = end + 2;
    let mut argv = Vec::with_capacity(n);
    for _ in 0..n {
        if pos >= buf.len() {
            return Ok(None);
        }
        if buf[pos] != b'$' {
            return Err("Protocol error: expected '$'".into());
        }
        let Some(end) = line_end(pos) else {
            return Ok(None);
        };
        let len: usize = std::str::from_utf8(&buf[pos + 1..end])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or("Protocol error: invalid bulk length")?;
        let start = end + 2;
        if buf.len() < start + len + 2 {
            return Ok(None);
        }
        argv.push(buf[start..start + len].to_vec());
        pos = start + len + 2;
    }
    Ok(Some((argv, pos)))
}

/// Redis's glob (`stringmatchlen`): `*`, `?`, `[a-z]`, `[^…]`, `\x`.
pub fn glob_match(p: &[u8], s: &[u8]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(b'*') => {
            let rest = &p[1..];
            (0..=s.len()).any(|i| glob_match(rest, &s[i..]))
        }
        Some(b'?') => !s.is_empty() && glob_match(&p[1..], &s[1..]),
        Some(b'[') => {
            let Some(&c) = s.first() else { return false };
            let mut i = 1;
            let not = p.get(1) == Some(&b'^');
            if not {
                i += 1;
            }
            let mut hit = false;
            while i < p.len() && p[i] != b']' {
                if p[i] == b'\\' && i + 1 < p.len() {
                    hit |= p[i + 1] == c;
                    i += 2;
                } else if i + 2 < p.len() && p[i + 1] == b'-' && p[i + 2] != b']' {
                    let (lo, hi) = (p[i].min(p[i + 2]), p[i].max(p[i + 2]));
                    hit |= (lo..=hi).contains(&c);
                    i += 3;
                } else {
                    hit |= p[i] == c;
                    i += 1;
                }
            }
            let rest = if i < p.len() {
                &p[i + 1..]
            } else {
                &p[p.len()..]
            };
            hit != not && glob_match(rest, &s[1..])
        }
        Some(b'\\') if p.len() >= 2 => s.first() == Some(&p[1]) && glob_match(&p[2..], &s[1..]),
        Some(c) => s.first() == Some(c) && glob_match(&p[1..], &s[1..]),
    }
}

struct Entry {
    data: Data,
    expires: Option<Instant>,
}

/// One client connection's state.
struct Conn {
    id: i64,
    resp3: bool,
    db: usize,
    authed: bool,
    name: Option<Vec<u8>>,
    multi: Option<Vec<Vec<Vec<u8>>>>,
    watched: Vec<(usize, Vec<u8>, u64)>,
    quit: bool,
}

struct Store {
    opts: Options,
    dbs: Vec<BTreeMap<Vec<u8>, Entry>>,
    /// Bumped on every change of a key (WATCH compares them).
    versions: HashMap<(usize, Vec<u8>), u64>,
    clock: u64,
    log: Vec<Vec<Vec<u8>>>,
    clients: HashMap<i64, Arc<Notify>>,
    next_id: i64,
}

const WRONGTYPE: &str = "WRONGTYPE Operation against a key holding the wrong kind of value";
const SYNTAX: &str = "ERR syntax error";
const NOT_INT: &str = "ERR value is not an integer or out of range";

fn upper(a: &[u8]) -> String {
    String::from_utf8_lossy(a).to_ascii_uppercase()
}

fn int(a: &[u8]) -> Option<i64> {
    std::str::from_utf8(a).ok()?.parse().ok()
}

fn float(a: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(a).ok()?.to_ascii_lowercase();
    match s.as_str() {
        "inf" | "+inf" => Some(f64::INFINITY),
        "-inf" => Some(f64::NEG_INFINITY),
        _ => s.parse().ok().filter(|f: &f64| f.is_finite()),
    }
}

/// `start..=stop` of a list of `len` with Redis's negative indexes; `None` when empty.
fn range(start: i64, stop: i64, len: usize) -> Option<(usize, usize)> {
    let len = len as i64;
    let norm = |i: i64| if i < 0 { len + i } else { i };
    let (s, e) = (norm(start).max(0), norm(stop).min(len - 1));
    (s <= e && s < len).then_some((s as usize, e as usize))
}

fn stream_id(id: &str) -> Option<(u64, u64)> {
    match id.split_once('-') {
        Some((ms, seq)) => Some((ms.parse().ok()?, seq.parse().ok()?)),
        None => Some((id.parse().ok()?, 0)),
    }
}

impl Store {
    fn has(&self, major: u32, minor: u32) -> bool {
        Version::parse(&self.opts.version).is_none_or(|v| v.at_least(major, minor))
    }

    fn bump(&mut self, db: usize, key: &[u8]) {
        self.clock += 1;
        self.versions.insert((db, key.to_vec()), self.clock);
    }

    fn version(&self, db: usize, key: &[u8]) -> u64 {
        self.versions.get(&(db, key.to_vec())).copied().unwrap_or(0)
    }

    /// The key's entry, after dropping it when it expired.
    fn live(&mut self, db: usize, key: &[u8]) -> Option<&mut Entry> {
        let expired = self.dbs[db]
            .get(key)
            .is_some_and(|e| e.expires.is_some_and(|t| t <= Instant::now()));
        if expired {
            self.dbs[db].remove(key);
            self.bump(db, key);
        }
        self.dbs[db].get_mut(key)
    }

    fn data(&mut self, db: usize, key: &[u8]) -> Option<&mut Data> {
        self.live(db, key).map(|e| &mut e.data)
    }

    fn put(&mut self, db: usize, key: &[u8], data: Data, expires: Option<Instant>) {
        self.dbs[db].insert(key.to_vec(), Entry { data, expires });
        self.bump(db, key);
    }

    fn remove(&mut self, db: usize, key: &[u8]) -> bool {
        let gone = self.live(db, key).is_some();
        if gone {
            self.dbs[db].remove(key);
            self.bump(db, key);
        }
        gone
    }

    fn live_keys(&mut self, db: usize) -> Vec<Vec<u8>> {
        let keys: Vec<Vec<u8>> = self.dbs[db].keys().cloned().collect();
        keys.into_iter()
            .filter(|k| self.live(db, k).is_some())
            .collect()
    }

    /// Changes a container, creating it with `make` when missing (`f` checks the type); drops it
    /// when it ends up empty (Redis has no empty lists, hashes, sets or sorted sets) or when a new
    /// one was not written.
    fn with<R>(
        &mut self,
        db: usize,
        key: &[u8],
        make: impl FnOnce() -> Data,
        f: impl FnOnce(&mut Data) -> Result<R, Reply>,
    ) -> Result<R, Reply> {
        let fresh = self.live(db, key).is_none();
        if fresh {
            self.dbs[db].insert(
                key.to_vec(),
                Entry {
                    data: make(),
                    expires: None,
                },
            );
        }
        let entry = self.dbs[db]
            .get_mut(key)
            .ok_or_else(|| Reply::err("ERR fake"))?;
        let out = f(&mut entry.data);
        let empty = match &entry.data {
            Data::List(l) => l.is_empty(),
            Data::Hash(h) => h.is_empty(),
            Data::Set(s) => s.is_empty(),
            Data::ZSet(z) => z.is_empty(),
            _ => false,
        };
        if empty || (fresh && out.is_err()) {
            self.dbs[db].remove(key);
        }
        if out.is_ok() {
            self.bump(db, key);
        }
        out
    }

    fn exec(&mut self, c: &mut Conn, argv: &[Vec<u8>]) -> Reply {
        let name = upper(&argv[0]);
        if !c.authed && !matches!(name.as_str(), "AUTH" | "HELLO" | "QUIT") {
            return Reply::err("NOAUTH Authentication required.");
        }
        if let Some(queue) = &mut c.multi {
            match name.as_str() {
                "EXEC" | "DISCARD" | "MULTI" | "WATCH" => {}
                _ => {
                    queue.push(argv.to_vec());
                    return Reply::Simple("QUEUED".into());
                }
            }
        }
        self.run(c, &name, argv)
    }

    fn unknown(name: &str) -> Reply {
        Reply::Err(format!("ERR unknown command '{name}'"))
    }

    fn run(&mut self, c: &mut Conn, name: &str, argv: &[Vec<u8>]) -> Reply {
        let a = |i: usize| argv.get(i).map(Vec::as_slice);
        let wrong = || {
            Reply::Err(format!(
                "ERR wrong number of arguments for '{}' command",
                name.to_ascii_lowercase()
            ))
        };
        let db = c.db;
        match name {
            "PING" => match a(1) {
                Some(m) => Reply::bulk(m),
                None => Reply::Simple("PONG".into()),
            },
            "ECHO" => a(1).map_or_else(wrong, Reply::bulk),
            "QUIT" => {
                c.quit = true;
                Reply::Ok
            }
            "HELLO" => self.hello(c, argv),
            "AUTH" => {
                let (user, pass) = match argv.len() {
                    2 => (None, &argv[1]),
                    3 => (Some(&argv[1]), &argv[2]),
                    _ => return wrong(),
                };
                match self.check_auth(user.map(Vec::as_slice), pass) {
                    Ok(()) => {
                        c.authed = true;
                        Reply::Ok
                    }
                    Err(e) => e,
                }
            }
            "SELECT" => match a(1).and_then(int) {
                Some(n) if n >= 0 && (n as u32) < self.opts.databases => {
                    c.db = n as usize;
                    Reply::Ok
                }
                Some(_) => Reply::err("ERR DB index is out of range"),
                None => Reply::err(NOT_INT),
            },
            "CLIENT" => match a(1).map(upper).as_deref() {
                Some("ID") => Reply::Int(c.id),
                Some("SETNAME") => {
                    c.name = a(2).map(<[u8]>::to_vec);
                    Reply::Ok
                }
                Some("GETNAME") => c.name.as_deref().map_or(Reply::Nil, Reply::bulk),
                Some("SETINFO") => Reply::Ok,
                Some("KILL") if a(2).map(upper).as_deref() == Some("ID") => {
                    match a(3).and_then(int).and_then(|id| self.clients.remove(&id)) {
                        Some(n) => {
                            n.notify_one();
                            Reply::Int(1)
                        }
                        None => Reply::Int(0),
                    }
                }
                _ => Reply::err(SYNTAX),
            },
            "INFO" => Reply::Verbatim(self.info(a(1).map(upper).as_deref())),
            "CONFIG" if self.opts.refuse_config => Self::unknown("CONFIG"),
            "CONFIG" => match (a(1).map(upper).as_deref(), a(2)) {
                (Some("GET"), Some(p)) if glob_match(p, b"databases") => Reply::Map(vec![(
                    Reply::text("databases"),
                    Reply::text(&self.opts.databases.to_string()),
                )]),
                (Some("GET"), Some(_)) => Reply::Map(Vec::new()),
                _ => Reply::err(SYNTAX),
            },
            "COMMAND" => command_table(),
            "DBSIZE" => Reply::Int(self.live_keys(db).len() as i64),
            "FLUSHDB" => {
                for k in self.live_keys(db) {
                    self.remove(db, &k);
                }
                Reply::Ok
            }
            "DEBUG" => Reply::err("ERR DEBUG SLEEP only (handled by the connection)"),
            "MULTI" => {
                if c.multi.is_some() {
                    return Reply::err("ERR MULTI calls can not be nested");
                }
                c.multi = Some(Vec::new());
                Reply::Ok
            }
            "DISCARD" => match c.multi.take() {
                Some(_) => {
                    c.watched.clear();
                    Reply::Ok
                }
                None => Reply::err("ERR DISCARD without MULTI"),
            },
            "WATCH" => {
                if c.multi.is_some() {
                    return Reply::err("ERR WATCH inside MULTI is not allowed");
                }
                for k in &argv[1..] {
                    self.live(db, k);
                    let v = self.version(db, k);
                    c.watched.push((db, k.clone(), v));
                }
                Reply::Ok
            }
            "UNWATCH" => {
                c.watched.clear();
                Reply::Ok
            }
            "EXEC" => {
                let Some(queue) = c.multi.take() else {
                    return Reply::err("ERR EXEC without MULTI");
                };
                let watched = std::mem::take(&mut c.watched);
                let changed = watched.iter().any(|(d, k, v)| {
                    self.live(*d, k);
                    self.version(*d, k) != *v
                });
                if changed {
                    return Reply::NilArray;
                }
                Reply::Array(
                    queue
                        .iter()
                        .map(|cmd| {
                            let n = upper(&cmd[0]);
                            self.run(c, &n, cmd)
                        })
                        .collect(),
                )
            }
            _ => match self.key_command(c, name, argv) {
                Ok(r) => r,
                Err(e) => e,
            },
        }
    }

    fn check_auth(&self, user: Option<&[u8]>, pass: &[u8]) -> Result<(), Reply> {
        let Some(expected) = &self.opts.password else {
            return Err(Reply::err(
                "ERR AUTH <password> called without any password configured for the default user. Are you sure your configuration is correct?",
            ));
        };
        let user_ok = match (user, &self.opts.user) {
            (None, None) => true,
            (Some(u), None) => u == b"default",
            (Some(u), Some(e)) => u == e.as_bytes(),
            (None, Some(_)) => false,
        };
        if user_ok && pass == expected.as_bytes() {
            Ok(())
        } else {
            Err(Reply::err(
                "WRONGPASS invalid username-password pair or user is disabled.",
            ))
        }
    }

    fn hello(&mut self, c: &mut Conn, argv: &[Vec<u8>]) -> Reply {
        if !self.has(6, 0) {
            return Self::unknown("HELLO");
        }
        let proto = match argv.get(1).map(|p| int(p)) {
            None => None,
            Some(Some(p @ (2 | 3))) => Some(p),
            Some(_) => return Reply::err("NOPROTO unsupported protocol version"),
        };
        let mut i = 2;
        while i < argv.len() {
            match upper(&argv[i]).as_str() {
                "AUTH" if i + 2 < argv.len() => {
                    if let Err(e) = self.check_auth(Some(&argv[i + 1]), &argv[i + 2]) {
                        return e;
                    }
                    c.authed = true;
                    i += 3;
                }
                "SETNAME" if i + 1 < argv.len() => {
                    c.name = Some(argv[i + 1].clone());
                    i += 2;
                }
                _ => return Reply::err(SYNTAX),
            }
        }
        if !c.authed {
            return Reply::err(
                "NOAUTH HELLO must be called with the client already authenticated, otherwise the HELLO <proto> AUTH <user> <pass> option can be used to authenticate the client and select the RESP protocol version at the same time",
            );
        }
        if let Some(p) = proto {
            c.resp3 = p == 3;
        }
        Reply::Map(vec![
            (Reply::text("server"), Reply::text("redis")),
            (Reply::text("version"), Reply::text(&self.opts.version)),
            (
                Reply::text("proto"),
                Reply::Int(if c.resp3 { 3 } else { 2 }),
            ),
            (Reply::text("id"), Reply::Int(c.id)),
            (Reply::text("mode"), Reply::text("standalone")),
            (Reply::text("role"), Reply::text("master")),
            (Reply::text("modules"), Reply::Array(Vec::new())),
        ])
    }

    fn info(&mut self, section: Option<&str>) -> String {
        let all = matches!(section, None | Some("ALL" | "EVERYTHING" | "DEFAULT"));
        let mut out = String::new();
        if all || section == Some("SERVER") {
            out.push_str("# Server\r\n");
            out.push_str(&format!("redis_version:{}\r\n", self.opts.version));
            if let Some((name, v)) = &self.opts.product {
                out.push_str(&format!("server_name:{name}\r\n{name}_version:{v}\r\n"));
            }
            out.push_str("redis_mode:standalone\r\nos:Fake 1.0\r\n\r\n");
        }
        if all || section == Some("KEYSPACE") {
            out.push_str("# Keyspace\r\n");
            for db in 0..self.dbs.len() {
                let keys = self.live_keys(db);
                if keys.is_empty() {
                    continue;
                }
                let expires = keys
                    .iter()
                    .filter(|k| self.dbs[db].get(*k).is_some_and(|e| e.expires.is_some()))
                    .count();
                out.push_str(&format!(
                    "db{db}:keys={},expires={expires},avg_ttl=0,subexpiry=0\r\n",
                    keys.len()
                ));
            }
        }
        out
    }

    fn key_command(&mut self, c: &mut Conn, name: &str, argv: &[Vec<u8>]) -> Result<Reply, Reply> {
        let db = c.db;
        let wrong = || {
            Reply::Err(format!(
                "ERR wrong number of arguments for '{}' command",
                name.to_ascii_lowercase()
            ))
        };
        let a = |i: usize| argv.get(i).map(Vec::as_slice).ok_or_else(wrong);
        let resp3 = c.resp3;
        Ok(match name {
            "SCAN" => {
                let cursor = int(a(1)?)
                    .filter(|n| *n >= 0)
                    .ok_or(Reply::err("ERR invalid cursor"))? as usize;
                let (mut pattern, mut count, mut ty) = (None, 10usize, None);
                let mut i = 2;
                while i < argv.len() {
                    match upper(&argv[i]).as_str() {
                        "MATCH" => pattern = Some(a(i + 1)?.to_vec()),
                        "COUNT" => {
                            count = int(a(i + 1)?)
                                .filter(|n| *n > 0)
                                .ok_or(Reply::err(SYNTAX))?
                                as usize
                        }
                        "TYPE" if self.has(6, 0) => ty = Some(a(i + 1)?.to_vec()),
                        _ => return Err(Reply::err(SYNTAX)),
                    }
                    i += 2;
                }
                let keys = self.live_keys(db);
                let end = (cursor + count).min(keys.len());
                let mut page = Vec::new();
                for k in keys.get(cursor..end).unwrap_or_default() {
                    let pattern_ok = pattern.as_deref().is_none_or(|p| glob_match(p, k));
                    let type_ok = ty.as_deref().is_none_or(|t| {
                        self.dbs[db]
                            .get(k)
                            .is_some_and(|e| e.data.type_name().as_bytes() == t)
                    });
                    if pattern_ok && type_ok {
                        page.push(Reply::bulk(k));
                    }
                }
                let next = if end >= keys.len() { 0 } else { end };
                Reply::Array(vec![Reply::text(&next.to_string()), Reply::Array(page)])
            }
            "TYPE" => Reply::Simple(
                self.data(db, a(1)?)
                    .map_or("none".to_string(), |d| d.type_name().to_string()),
            ),
            "EXISTS" => Reply::Int(
                argv[1..]
                    .iter()
                    .filter(|k| self.live(db, k).is_some())
                    .count() as i64,
            ),
            "PTTL" | "TTL" => {
                let ms = match self.live(db, a(1)?) {
                    None => -2,
                    Some(Entry { expires: None, .. }) => -1,
                    Some(Entry {
                        expires: Some(t), ..
                    }) => t.saturating_duration_since(Instant::now()).as_millis() as i64,
                };
                Reply::Int(if name == "TTL" && ms >= 0 {
                    (ms + 500) / 1000
                } else {
                    ms
                })
            }
            "DEL" | "UNLINK" => {
                if name == "UNLINK" && !self.has(4, 0) {
                    return Ok(Self::unknown("UNLINK"));
                }
                a(1)?;
                Reply::Int(argv[1..].iter().filter(|k| self.remove(db, k)).count() as i64)
            }
            "RENAMENX" => {
                let (from, to) = (a(1)?, a(2)?);
                if self.live(db, from).is_none() {
                    return Err(Reply::err("ERR no such key"));
                }
                if self.live(db, to).is_some() {
                    return Ok(Reply::Int(0));
                }
                let entry = self.dbs[db]
                    .remove(from)
                    .ok_or(Reply::err("ERR no such key"))?;
                self.bump(db, from);
                self.put(db, to, entry.data, entry.expires);
                Reply::Int(1)
            }
            "PEXPIRE" | "EXPIRE" => {
                let n = int(a(2)?).ok_or(Reply::err(NOT_INT))?;
                let ms = if name == "EXPIRE" {
                    n.saturating_mul(1000)
                } else {
                    n
                };
                let key = a(1)?;
                if self.live(db, key).is_none() {
                    return Ok(Reply::Int(0));
                }
                if ms <= 0 {
                    self.remove(db, key);
                } else if let Some(e) = self.live(db, key) {
                    e.expires = Some(Instant::now() + Duration::from_millis(ms as u64));
                    self.bump(db, key);
                }
                Reply::Int(1)
            }
            "PERSIST" => {
                let key = a(1)?;
                let had = match self.live(db, key) {
                    Some(e) => e.expires.take().is_some(),
                    None => false,
                };
                if had {
                    self.bump(db, key);
                }
                Reply::Int(had as i64)
            }
            "COPY" => {
                if !self.has(6, 2) {
                    return Ok(Self::unknown("COPY"));
                }
                let (from, to) = (a(1)?, a(2)?);
                if self.live(db, to).is_some() {
                    return Ok(Reply::Int(0));
                }
                match self.live(db, from).map(|e| (e.data.clone(), e.expires)) {
                    Some((data, expires)) => {
                        self.put(db, to, data, expires);
                        Reply::Int(1)
                    }
                    None => Reply::Int(0),
                }
            }
            "MEMORY" if a(1).map(upper).as_deref() == Ok("USAGE") => match self.data(db, a(2)?) {
                Some(d) => Reply::Int(50 + d.weight() as i64),
                None => Reply::Nil,
            },
            "GET" => match self.data(db, a(1)?) {
                None => Reply::Nil,
                Some(Data::Str(v)) => Reply::bulk(v),
                Some(_) => return Err(Reply::err(WRONGTYPE)),
            },
            "GETRANGE" => {
                let (s, e) = (
                    int(a(2)?).ok_or(Reply::err(NOT_INT))?,
                    int(a(3)?).ok_or(Reply::err(NOT_INT))?,
                );
                match self.data(db, a(1)?) {
                    None => Reply::text(""),
                    Some(Data::Str(v)) => match range(s, e, v.len()) {
                        Some((s, e)) => Reply::bulk(&v[s..=e]),
                        None => Reply::text(""),
                    },
                    Some(_) => return Err(Reply::err(WRONGTYPE)),
                }
            }
            "STRLEN" => match self.data(db, a(1)?) {
                None => Reply::Int(0),
                Some(Data::Str(v)) => Reply::Int(v.len() as i64),
                Some(_) => return Err(Reply::err(WRONGTYPE)),
            },
            "SET" => return self.set(db, argv),
            "HSET" | "HSETNX" => {
                let key = a(1)?;
                if argv.len() < 4
                    || !argv.len().is_multiple_of(2)
                    || (name == "HSETNX" && argv.len() != 4)
                {
                    return Err(wrong());
                }
                let nx = name == "HSETNX";
                let pairs: Vec<(Vec<u8>, Vec<u8>)> = argv[2..]
                    .chunks(2)
                    .map(|p| (p[0].clone(), p[1].clone()))
                    .collect();
                let added = self.with(
                    db,
                    key,
                    || Data::Hash(Vec::new()),
                    |d| {
                        let Data::Hash(h) = d else {
                            return Err(Reply::err(WRONGTYPE));
                        };
                        let mut added = 0;
                        for (f, v) in pairs {
                            match h.iter_mut().find(|(k, _)| *k == f) {
                                Some(_) if nx => {}
                                Some(slot) => slot.1 = v,
                                None => {
                                    h.push((f, v));
                                    added += 1;
                                }
                            }
                        }
                        Ok(added)
                    },
                )?;
                Reply::Int(added)
            }
            "HGET" => match self.data(db, a(1)?) {
                None => Reply::Nil,
                Some(Data::Hash(h)) => {
                    let f = a(2)?;
                    h.iter()
                        .find(|(k, _)| k == f)
                        .map_or(Reply::Nil, |(_, v)| Reply::bulk(v))
                }
                Some(_) => return Err(Reply::err(WRONGTYPE)),
            },
            "HEXISTS" => match self.data(db, a(1)?) {
                None => Reply::Int(0),
                Some(Data::Hash(h)) => {
                    let f = a(2)?;
                    Reply::Int(i64::from(h.iter().any(|(k, _)| k == f)))
                }
                Some(_) => return Err(Reply::err(WRONGTYPE)),
            },
            "HGETALL" => match self.data(db, a(1)?) {
                None => Reply::Map(Vec::new()),
                Some(Data::Hash(h)) => Reply::Map(
                    h.iter()
                        .map(|(f, v)| (Reply::bulk(f), Reply::bulk(v)))
                        .collect(),
                ),
                Some(_) => return Err(Reply::err(WRONGTYPE)),
            },
            "HLEN" | "LLEN" | "SCARD" | "ZCARD" | "XLEN" => {
                let n = match (name, self.data(db, a(1)?)) {
                    (_, None) => 0,
                    ("HLEN", Some(Data::Hash(h))) => h.len(),
                    ("LLEN", Some(Data::List(l))) => l.len(),
                    ("SCARD", Some(Data::Set(s))) => s.len(),
                    ("ZCARD", Some(Data::ZSet(z))) => z.len(),
                    ("XLEN", Some(Data::Stream(s))) => s.len(),
                    _ => return Err(Reply::err(WRONGTYPE)),
                };
                Reply::Int(n as i64)
            }
            "HDEL" | "SREM" | "ZREM" => {
                let key = a(1)?;
                a(2)?;
                if self.live(db, key).is_none() {
                    return Ok(Reply::Int(0));
                }
                let targets: Vec<Vec<u8>> = argv[2..].to_vec();
                let n = self.with(
                    db,
                    key,
                    || Data::Str(Vec::new()),
                    |d| match (name, d) {
                        ("HDEL", Data::Hash(h)) => {
                            let before = h.len();
                            h.retain(|(f, _)| !targets.contains(f));
                            Ok(before - h.len())
                        }
                        ("SREM", Data::Set(s)) => {
                            let before = s.len();
                            s.retain(|m| !targets.contains(m));
                            Ok(before - s.len())
                        }
                        ("ZREM", Data::ZSet(z)) => {
                            let before = z.len();
                            z.retain(|(m, _)| !targets.contains(m));
                            Ok(before - z.len())
                        }
                        _ => Err(Reply::err(WRONGTYPE)),
                    },
                )?;
                Reply::Int(n as i64)
            }
            "HSCAN" | "SSCAN" => {
                let key = a(1)?;
                let cursor = int(a(2)?)
                    .filter(|n| *n >= 0)
                    .ok_or(Reply::err("ERR invalid cursor"))? as usize;
                let mut count = 10usize;
                let mut novalues = false;
                let mut i = 3;
                while i < argv.len() {
                    match upper(&argv[i]).as_str() {
                        "COUNT" => {
                            count = int(a(i + 1)?)
                                .filter(|n| *n > 0)
                                .ok_or(Reply::err(SYNTAX))?
                                as usize;
                            i += 2;
                        }
                        "NOVALUES" if name == "HSCAN" => {
                            novalues = true;
                            i += 1;
                        }
                        _ => return Err(Reply::err(SYNTAX)),
                    }
                }
                let items: Vec<Vec<Reply>> = match (name, self.data(db, key)) {
                    (_, None) => Vec::new(),
                    ("HSCAN", Some(Data::Hash(h))) => h
                        .iter()
                        .map(|(f, v)| {
                            if novalues {
                                vec![Reply::bulk(f)]
                            } else {
                                vec![Reply::bulk(f), Reply::bulk(v)]
                            }
                        })
                        .collect(),
                    ("SSCAN", Some(Data::Set(s))) => {
                        s.iter().map(|m| vec![Reply::bulk(m)]).collect()
                    }
                    _ => return Err(Reply::err(WRONGTYPE)),
                };
                let end = (cursor + count).min(items.len());
                let page: Vec<Reply> = items
                    .get(cursor..end)
                    .unwrap_or_default()
                    .iter()
                    .flatten()
                    .cloned()
                    .collect();
                let next = if end >= items.len() { 0 } else { end };
                Reply::Array(vec![Reply::text(&next.to_string()), Reply::Array(page)])
            }
            "SMEMBERS" => match self.data(db, a(1)?) {
                None => Reply::Set(Vec::new()),
                Some(Data::Set(s)) => Reply::Set(s.iter().map(|m| Reply::bulk(m)).collect()),
                Some(_) => return Err(Reply::err(WRONGTYPE)),
            },
            "SISMEMBER" => match self.data(db, a(1)?) {
                None => Reply::Int(0),
                Some(Data::Set(s)) => {
                    let m = a(2)?;
                    Reply::Int(i64::from(s.iter().any(|x| x == m)))
                }
                Some(_) => return Err(Reply::err(WRONGTYPE)),
            },
            "SADD" => {
                let key = a(1)?;
                a(2)?;
                let members = argv[2..].to_vec();
                let n = self.with(
                    db,
                    key,
                    || Data::Set(Vec::new()),
                    |d| {
                        let Data::Set(s) = d else {
                            return Err(Reply::err(WRONGTYPE));
                        };
                        let mut added = 0;
                        for m in members {
                            if !s.contains(&m) {
                                s.push(m);
                                added += 1;
                            }
                        }
                        Ok(added)
                    },
                )?;
                Reply::Int(n)
            }
            "RPUSH" | "LPUSH" => {
                let key = a(1)?;
                a(2)?;
                let items = argv[2..].to_vec();
                let head = name == "LPUSH";
                let n = self.with(
                    db,
                    key,
                    || Data::List(VecDeque::new()),
                    |d| {
                        let Data::List(l) = d else {
                            return Err(Reply::err(WRONGTYPE));
                        };
                        for v in items {
                            if head {
                                l.push_front(v);
                            } else {
                                l.push_back(v);
                            }
                        }
                        Ok(l.len())
                    },
                )?;
                Reply::Int(n as i64)
            }
            "LRANGE" => {
                let (s, e) = (
                    int(a(2)?).ok_or(Reply::err(NOT_INT))?,
                    int(a(3)?).ok_or(Reply::err(NOT_INT))?,
                );
                match self.data(db, a(1)?) {
                    None => Reply::Array(Vec::new()),
                    Some(Data::List(l)) => match range(s, e, l.len()) {
                        Some((s, e)) => {
                            Reply::Array(l.range(s..=e).map(|v| Reply::bulk(v)).collect())
                        }
                        None => Reply::Array(Vec::new()),
                    },
                    Some(_) => return Err(Reply::err(WRONGTYPE)),
                }
            }
            "LINDEX" => {
                let idx = int(a(2)?).ok_or(Reply::err(NOT_INT))?;
                match self.data(db, a(1)?) {
                    None => Reply::Nil,
                    Some(Data::List(l)) => {
                        let i = if idx < 0 { l.len() as i64 + idx } else { idx };
                        usize::try_from(i)
                            .ok()
                            .and_then(|i| l.get(i))
                            .map_or(Reply::Nil, |v| Reply::bulk(v))
                    }
                    Some(_) => return Err(Reply::err(WRONGTYPE)),
                }
            }
            "LSET" => {
                let key = a(1)?;
                let idx = int(a(2)?).ok_or(Reply::err(NOT_INT))?;
                let v = a(3)?.to_vec();
                if self.live(db, key).is_none() {
                    return Err(Reply::err("ERR no such key"));
                }
                self.with(
                    db,
                    key,
                    || Data::Str(Vec::new()),
                    |d| {
                        let Data::List(l) = d else {
                            return Err(Reply::err(WRONGTYPE));
                        };
                        let i = if idx < 0 { l.len() as i64 + idx } else { idx };
                        match usize::try_from(i).ok().and_then(|i| l.get_mut(i)) {
                            Some(slot) => {
                                *slot = v;
                                Ok(Reply::Ok)
                            }
                            None => Err(Reply::err("ERR index out of range")),
                        }
                    },
                )?
            }
            "LREM" => {
                let key = a(1)?;
                let count = int(a(2)?).ok_or(Reply::err(NOT_INT))?;
                let v = a(3)?.to_vec();
                if self.live(db, key).is_none() {
                    return Ok(Reply::Int(0));
                }
                let n = self.with(
                    db,
                    key,
                    || Data::Str(Vec::new()),
                    |d| {
                        let Data::List(l) = d else {
                            return Err(Reply::err(WRONGTYPE));
                        };
                        let limit = if count == 0 {
                            usize::MAX
                        } else {
                            count.unsigned_abs() as usize
                        };
                        let mut removed = 0;
                        let mut items: Vec<Vec<u8>> = l.drain(..).collect();
                        if count < 0 {
                            items.reverse();
                        }
                        items.retain(|x| {
                            if removed < limit && *x == v {
                                removed += 1;
                                false
                            } else {
                                true
                            }
                        });
                        if count < 0 {
                            items.reverse();
                        }
                        l.extend(items);
                        Ok(removed)
                    },
                )?;
                Reply::Int(n as i64)
            }
            "ZADD" => {
                let key = a(1)?;
                let (mut nx, mut xx, mut ch) = (false, false, false);
                let mut i = 2;
                while i < argv.len() {
                    match upper(&argv[i]).as_str() {
                        "NX" => nx = true,
                        "XX" => xx = true,
                        "CH" => ch = true,
                        _ => break,
                    }
                    i += 1;
                }
                let rest = &argv[i..];
                if rest.is_empty() || !rest.len().is_multiple_of(2) {
                    return Err(Reply::err(SYNTAX));
                }
                let mut pairs = Vec::new();
                for p in rest.chunks(2) {
                    pairs.push((
                        float(&p[0]).ok_or(Reply::err("ERR value is not a valid float"))?,
                        p[1].clone(),
                    ));
                }
                if xx && self.live(db, key).is_none() {
                    return Ok(Reply::Int(0));
                }
                let n = self.with(
                    db,
                    key,
                    || Data::ZSet(Vec::new()),
                    |d| {
                        let Data::ZSet(z) = d else {
                            return Err(Reply::err(WRONGTYPE));
                        };
                        let (mut added, mut changed) = (0, 0);
                        for (score, m) in pairs {
                            match z.iter_mut().find(|(k, _)| *k == m) {
                                Some(slot) if !nx => {
                                    if slot.1 != score {
                                        slot.1 = score;
                                        changed += 1;
                                    }
                                }
                                Some(_) => {}
                                None if !xx => {
                                    z.push((m, score));
                                    added += 1;
                                }
                                None => {}
                            }
                        }
                        sort_zset(z);
                        Ok(if ch { added + changed } else { added })
                    },
                )?;
                Reply::Int(n)
            }
            "ZRANGE" => {
                let (s, e) = (
                    int(a(2)?).ok_or(Reply::err(NOT_INT))?,
                    int(a(3)?).ok_or(Reply::err(NOT_INT))?,
                );
                let scores = argv.get(4).is_some_and(|w| upper(w) == "WITHSCORES");
                match self.data(db, a(1)?) {
                    None => Reply::Array(Vec::new()),
                    Some(Data::ZSet(z)) => {
                        let slice = range(s, e, z.len()).map_or(&z[..0], |(s, e)| &z[s..=e]);
                        let mut out = Vec::new();
                        for (m, score) in slice {
                            match (scores, resp3) {
                                (false, _) => out.push(Reply::bulk(m)),
                                (true, true) => out.push(Reply::Array(vec![
                                    Reply::bulk(m),
                                    Reply::Double(*score),
                                ])),
                                (true, false) => {
                                    out.push(Reply::bulk(m));
                                    out.push(Reply::Double(*score));
                                }
                            }
                        }
                        Reply::Array(out)
                    }
                    Some(_) => return Err(Reply::err(WRONGTYPE)),
                }
            }
            "ZSCORE" => match self.data(db, a(1)?) {
                None => Reply::Nil,
                Some(Data::ZSet(z)) => {
                    let m = a(2)?;
                    z.iter()
                        .find(|(k, _)| k == m)
                        .map_or(Reply::Nil, |(_, s)| Reply::Double(*s))
                }
                Some(_) => return Err(Reply::err(WRONGTYPE)),
            },
            "XADD" => {
                let key = a(1)?;
                let id = String::from_utf8_lossy(a(2)?).into_owned();
                let fields = &argv[3..];
                if fields.is_empty() || !fields.len().is_multiple_of(2) {
                    return Err(wrong());
                }
                let fields: Vec<(Vec<u8>, Vec<u8>)> = fields
                    .chunks(2)
                    .map(|p| (p[0].clone(), p[1].clone()))
                    .collect();
                let new_id = self.with(db, key, || Data::Stream(Vec::new()), |d| {
                    let Data::Stream(s) = d else { return Err(Reply::err(WRONGTYPE)) };
                    let last = s.last().and_then(|(id, _)| stream_id(id)).unwrap_or((0, 0));
                    let next = if id == "*" {
                        (last.0 + 1, 0)
                    } else {
                        let given = stream_id(&id).ok_or(Reply::err("ERR Invalid stream ID specified as stream command argument"))?;
                        if given <= last {
                            return Err(Reply::err("ERR The ID specified in XADD is equal or smaller than the target stream top item"));
                        }
                        given
                    };
                    let text = format!("{}-{}", next.0, next.1);
                    s.push((text.clone(), fields));
                    Ok(text)
                })?;
                Reply::text(&new_id)
            }
            "XRANGE" | "XREVRANGE" => {
                let key = a(1)?;
                let (lo, hi) = if name == "XRANGE" {
                    (a(2)?, a(3)?)
                } else {
                    (a(3)?, a(2)?)
                };
                let bound = |b: &[u8], low: bool| -> Result<(u64, u64), Reply> {
                    match b {
                        b"-" => Ok((0, 0)),
                        b"+" => Ok((u64::MAX, u64::MAX)),
                        other => {
                            let s = String::from_utf8_lossy(other);
                            let id = match s.split_once('-') {
                                Some(_) => stream_id(&s),
                                None => s
                                    .parse::<u64>()
                                    .ok()
                                    .map(|ms| (ms, if low { 0 } else { u64::MAX })),
                            };
                            id.ok_or(Reply::err(
                                "ERR Invalid stream ID specified as stream command argument",
                            ))
                        }
                    }
                };
                let (lo, hi) = (bound(lo, true)?, bound(hi, false)?);
                let count = match argv.get(4).map(|w| upper(w)) {
                    Some(w) if w == "COUNT" => {
                        int(a(5)?).filter(|n| *n >= 0).ok_or(Reply::err(NOT_INT))? as usize
                    }
                    Some(_) => return Err(Reply::err(SYNTAX)),
                    None => usize::MAX,
                };
                let entries = match self.data(db, key) {
                    None => Vec::new(),
                    Some(Data::Stream(s)) => s.clone(),
                    Some(_) => return Err(Reply::err(WRONGTYPE)),
                };
                let mut picked: Vec<_> = entries
                    .into_iter()
                    .filter(|(id, _)| stream_id(id).is_some_and(|i| i >= lo && i <= hi))
                    .collect();
                if name == "XREVRANGE" {
                    picked.reverse();
                }
                Reply::Array(
                    picked
                        .into_iter()
                        .take(count)
                        .map(|(id, fields)| {
                            Reply::Array(vec![
                                Reply::text(&id),
                                Reply::Array(
                                    fields
                                        .iter()
                                        .flat_map(|(f, v)| [Reply::bulk(f), Reply::bulk(v)])
                                        .collect(),
                                ),
                            ])
                        })
                        .collect(),
                )
            }
            "XDEL" => {
                let key = a(1)?;
                a(2)?;
                let ids: Vec<String> = argv[2..]
                    .iter()
                    .map(|i| String::from_utf8_lossy(i).into_owned())
                    .collect();
                if self.live(db, key).is_none() {
                    return Ok(Reply::Int(0));
                }
                let n = self.with(
                    db,
                    key,
                    || Data::Stream(Vec::new()),
                    |d| {
                        let Data::Stream(s) = d else {
                            return Err(Reply::err(WRONGTYPE));
                        };
                        let before = s.len();
                        s.retain(|(id, _)| !ids.contains(id));
                        Ok(before - s.len())
                    },
                )?;
                Reply::Int(n as i64)
            }
            "JSON.SET" => {
                let (key, path, doc) = (a(1)?, a(2)?, a(3)?);
                if path != b"$" && path != b"." {
                    return Err(Reply::err("ERR the fake sets the root path only"));
                }
                let doc = std::str::from_utf8(doc)
                    .ok()
                    .filter(|d| serde_json::from_str::<serde_json::Value>(d).is_ok())
                    .ok_or(Reply::err("ERR expected value"))?
                    .to_string();
                let cond = argv.get(4).map(|c| upper(c));
                let exists = match self.data(db, key) {
                    None => false,
                    Some(Data::Json(_)) => true,
                    Some(_) => return Err(Reply::err(WRONGTYPE)),
                };
                match (cond.as_deref(), exists) {
                    (Some("NX"), true) | (Some("XX"), false) => Reply::Nil,
                    (Some("NX" | "XX") | None, _) => {
                        let expires = self.live(db, key).and_then(|e| e.expires);
                        self.put(db, key, Data::Json(doc), expires);
                        Reply::Ok
                    }
                    _ => return Err(Reply::err(SYNTAX)),
                }
            }
            "JSON.GET" => match self.data(db, a(1)?) {
                None => Reply::Nil,
                Some(Data::Json(doc)) => match argv.get(2).map(Vec::as_slice) {
                    None | Some(b".") => Reply::text(doc),
                    Some(b"$") => Reply::text(&format!("[{doc}]")),
                    Some(_) => return Err(Reply::err("ERR the fake reads the root path only")),
                },
                Some(_) => return Err(Reply::err(WRONGTYPE)),
            },
            _ => Reply::Err(format!(
                "ERR unknown command '{}', with args beginning with: ",
                String::from_utf8_lossy(&argv[0])
            )),
        })
    }

    fn set(&mut self, db: usize, argv: &[Vec<u8>]) -> Result<Reply, Reply> {
        if argv.len() < 3 {
            return Err(Reply::err(
                "ERR wrong number of arguments for 'set' command",
            ));
        }
        let (key, value) = (&argv[1], &argv[2]);
        let (mut nx, mut xx, mut keep, mut ttl) = (false, false, false, None);
        let mut i = 3;
        while i < argv.len() {
            match upper(&argv[i]).as_str() {
                "NX" => nx = true,
                "XX" => xx = true,
                "KEEPTTL" if self.has(6, 0) => keep = true,
                w @ ("PX" | "EX") => {
                    let n = argv
                        .get(i + 1)
                        .and_then(|n| int(n))
                        .filter(|n| *n > 0)
                        .ok_or(Reply::err("ERR invalid expire time in 'set' command"))?;
                    ttl = Some(Duration::from_millis(if w == "EX" {
                        n as u64 * 1000
                    } else {
                        n as u64
                    }));
                    i += 1;
                }
                _ => return Err(Reply::err(SYNTAX)),
            }
            i += 1;
        }
        if (nx && xx) || (keep && ttl.is_some()) {
            return Err(Reply::err(SYNTAX));
        }
        let old = self.live(db, key).map(|e| e.expires);
        if (nx && old.is_some()) || (xx && old.is_none()) {
            return Ok(Reply::Nil);
        }
        let expires = match (ttl, keep) {
            (Some(d), _) => Some(Instant::now() + d),
            (None, true) => old.flatten(),
            (None, false) => None,
        };
        self.put(db, key, Data::Str(value.clone()), expires);
        Ok(Reply::Ok)
    }
}

/// `COMMAND` as Redis 7 answers it, for a few commands (flags as status replies).
fn command_table() -> Reply {
    let entry = |name: &str, flags: &[&str], subs: Vec<Reply>| {
        Reply::Array(vec![
            Reply::text(name),
            Reply::Int(-2),
            Reply::Set(flags.iter().map(|f| Reply::Simple((*f).into())).collect()),
            Reply::Int(1),
            Reply::Int(1),
            Reply::Int(1),
            Reply::Set(Vec::new()),
            Reply::Array(Vec::new()),
            Reply::Array(Vec::new()),
            Reply::Array(subs),
        ])
    };
    Reply::Array(vec![
        entry("get", &["readonly", "fast"], Vec::new()),
        entry("set", &["write", "denyoom"], Vec::new()),
        entry("keys", &["readonly"], Vec::new()),
        entry("json.get", &["readonly"], Vec::new()),
        entry("ts.range", &["readonly"], Vec::new()),
        entry("pfcount", &["readonly", "may_replicate"], Vec::new()),
        entry(
            "object",
            &[],
            vec![
                entry("object|encoding", &["readonly"], Vec::new()),
                entry("object|freq", &["readonly"], Vec::new()),
            ],
        ),
    ])
}

fn lock(store: &Mutex<Store>) -> MutexGuard<'_, Store> {
    store.lock().unwrap_or_else(|e| e.into_inner())
}

/// The fake server; dropped, it stops listening.
pub struct FakeRedis {
    pub addr: SocketAddr,
    store: Arc<Mutex<Store>>,
    task: JoinHandle<()>,
}

impl Drop for FakeRedis {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeRedis {
    pub async fn start() -> FakeRedis {
        FakeRedis::with(Options::default()).await
    }

    pub async fn with(opts: Options) -> FakeRedis {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let addr = listener.local_addr().expect("the bound address");
        let dbs = (0..opts.databases).map(|_| BTreeMap::new()).collect();
        let store = Arc::new(Mutex::new(Store {
            opts,
            dbs,
            versions: HashMap::new(),
            clock: 0,
            log: Vec::new(),
            clients: HashMap::new(),
            next_id: 0,
        }));
        let shared = store.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, shared.clone()));
            }
        });
        FakeRedis { addr, store, task }
    }

    /// A `redis://` URL for the redis crate's client (`resp3` adds `?protocol=resp3`).
    pub fn url(&self, db: u32, resp3: bool) -> String {
        let auth = {
            let s = lock(&self.store);
            match (&s.opts.user, &s.opts.password) {
                (Some(u), Some(p)) => format!("{u}:{p}@"),
                (None, Some(p)) => format!(":{p}@"),
                _ => String::new(),
            }
        };
        let proto = if resp3 { "?protocol=resp3" } else { "" };
        format!("redis://{auth}{}/{db}{proto}", self.addr)
    }

    pub fn seed(&self, db: usize, key: &[u8], data: Data, ttl: Option<Duration>) {
        lock(&self.store).put(db, key, data, ttl.map(|d| Instant::now() + d));
    }

    pub fn data(&self, db: usize, key: &[u8]) -> Option<Data> {
        lock(&self.store).data(db, key).cloned()
    }

    pub fn ttl(&self, db: usize, key: &[u8]) -> Option<Duration> {
        let mut s = lock(&self.store);
        s.live(db, key)?
            .expires
            .map(|t| t.saturating_duration_since(Instant::now()))
    }

    /// Every command received, as names upper-case and arguments as text.
    pub fn log(&self) -> Vec<String> {
        lock(&self.store)
            .log
            .iter()
            .map(|argv| {
                argv.iter()
                    .enumerate()
                    .map(|(i, a)| {
                        let t = String::from_utf8_lossy(a);
                        if i == 0 {
                            t.to_ascii_uppercase()
                        } else {
                            t.into_owned()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    pub fn clear_log(&self) {
        lock(&self.store).log.clear();
    }

    /// Connections open now.
    pub fn clients(&self) -> usize {
        lock(&self.store).clients.len()
    }
}

/// `DEBUG SLEEP <seconds>`: the one command that waits.
fn sleep_of(argv: &[Vec<u8>]) -> Option<Duration> {
    if argv.len() != 3 || upper(&argv[0]) != "DEBUG" || upper(&argv[1]) != "SLEEP" {
        return None;
    }
    let secs: f64 = std::str::from_utf8(&argv[2]).ok()?.parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

async fn serve(mut stream: TcpStream, store: Arc<Mutex<Store>>) {
    let (id, kill, authed) = {
        let mut s = lock(&store);
        s.next_id += 1;
        let id = s.next_id;
        let kill = Arc::new(Notify::new());
        s.clients.insert(id, kill.clone());
        (id, kill, s.opts.password.is_none())
    };
    let mut conn = Conn {
        id,
        resp3: false,
        db: 0,
        authed,
        name: None,
        multi: None,
        watched: Vec::new(),
        quit: false,
    };
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    'outer: loop {
        loop {
            let (argv, used) = match parse_request(&buf) {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(e) => {
                    let mut out = Vec::new();
                    Reply::Err(format!("ERR {e}")).encode(conn.resp3, &mut out);
                    let _ = stream.write_all(&out).await;
                    break 'outer;
                }
            };
            buf.drain(..used);
            if argv.is_empty() {
                continue;
            }
            lock(&store).log.push(argv.clone());
            let reply = match sleep_of(&argv).filter(|_| conn.multi.is_none()) {
                Some(d) => {
                    tokio::select! {
                        _ = tokio::time::sleep(d) => Reply::Ok,
                        _ = kill.notified() => break 'outer,
                    }
                }
                None => lock(&store).exec(&mut conn, &argv),
            };
            let mut out = Vec::new();
            reply.encode(conn.resp3, &mut out);
            if stream.write_all(&out).await.is_err() || conn.quit {
                break 'outer;
            }
        }
        tokio::select! {
            n = stream.read(&mut chunk) => match n {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            },
            _ = kill.notified() => break,
        }
    }
    lock(&store).clients.remove(&id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::redis::parse_redis_value;

    #[test]
    fn requests() {
        let buf = b"*2\r\n$3\r\nGET\r\n$3\r\na b\r\n*1\r\n$4\r\nPI";
        let (argv, used) = parse_request(buf).unwrap().unwrap();
        assert_eq!(argv, [b"GET".to_vec(), b"a b".to_vec()]);
        assert_eq!(parse_request(&buf[used..]).unwrap(), None);
        assert_eq!(parse_request(b"").unwrap(), None);
        let (argv, used) = parse_request(b"PING  hi\r\nrest").unwrap().unwrap();
        assert_eq!((argv.len(), used), (2, 10));
        assert!(parse_request(b"*x\r\n").is_err());
        assert!(parse_request(b"*1\r\n:1\r\n").is_err());
        let bin = b"*1\r\n$2\r\n\xff\x00\r\n";
        assert_eq!(parse_request(bin).unwrap().unwrap().0, [vec![0xff, 0x00]]);
    }

    #[test]
    fn globs() {
        for (p, s) in [
            ("*", ""),
            ("user:*", "user:1"),
            ("h?llo", "hello"),
            ("h[ae]llo", "hallo"),
            ("h[^e]llo", "hallo"),
            ("h[a-b]llo", "hbllo"),
            ("[uU][sS]er", "USer"),
            ("a\\*b", "a*b"),
            ("*\\[x\\]", "key[x]"),
            ("*:*:?%", "user:1:a%"),
        ] {
            assert!(glob_match(p.as_bytes(), s.as_bytes()), "{p} {s}");
        }
        for (p, s) in [
            ("user:*", "users"),
            ("h[^e]llo", "hello"),
            ("a\\*b", "axb"),
            ("?", ""),
        ] {
            assert!(!glob_match(p.as_bytes(), s.as_bytes()), "{p} {s}");
        }
    }

    #[test]
    fn encodings_parse_back() {
        let reply = Reply::Array(vec![
            Reply::Ok,
            Reply::Nil,
            Reply::Double(1.5),
            Reply::Bool(true),
            Reply::Map(vec![(Reply::text("k"), Reply::Int(1))]),
            Reply::Set(vec![Reply::text("m")]),
            Reply::Verbatim("hi".into()),
            Reply::Err("ERR x".into()),
            Reply::NilArray,
        ]);
        for resp3 in [false, true] {
            let mut out = Vec::new();
            reply.encode(resp3, &mut out);
            let v = parse_redis_value(&out).unwrap();
            let ::redis::Value::Array(items) = v else {
                panic!()
            };
            assert_eq!(items.len(), 9);
            if resp3 {
                assert_eq!(items[2], ::redis::Value::Double(1.5));
                assert!(matches!(items[4], ::redis::Value::Map(_)));
            } else {
                assert_eq!(items[2], ::redis::Value::BulkString(b"1.5".to_vec()));
                assert!(matches!(&items[4], ::redis::Value::Array(a) if a.len() == 2));
            }
        }
        assert_eq!(format_double(f64::NEG_INFINITY), "-inf");
        assert_eq!(format_double(3.0), "3");
        assert_eq!(range(-2, -1, 5), Some((3, 4)));
        assert_eq!(range(3, 1, 5), None);
        assert_eq!(range(0, -1, 0), None);
    }
}
