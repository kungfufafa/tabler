//! Redis command builders (the redis lane): the commands an edit of the key browser sends, and
//! their redis-cli text for Preview. A command is an argv of byte strings, sent as is (binary
//! safe, never formatted into a line); `Command::text` quotes it the way redis-cli reads it back,
//! so the preview is exactly what runs.
//!
//! The grid edits keys of one database (`db3`): the row identity is the key plus its type
//! (`RowKey::Pk { key, type }`), so a plan knows how to write the value and the apply can prove,
//! with WATCH and TYPE, that the key is still what was loaded (`Guard`).

use std::collections::BTreeMap;

use base64::Engine as _;

use crate::error::AppError;
use crate::types::browse::RowKey;
use crate::types::mutate::StagedChange;
use crate::types::value::Value;

/// The columns of a database's key table (`drivers::redis::keys::columns`).
pub mod col {
    pub const KEY: &str = "key";
    pub const TYPE: &str = "type";
    pub const TTL: &str = "ttl";
    pub const SIZE: &str = "size";
    pub const MEMORY: &str = "memory";
    pub const VALUE: &str = "value";
}

/// The `TYPE` names of the key types the grid edits.
pub const TYPE_STRING: &str = "string";
pub const TYPE_JSON: &str = "ReJSON-RL";

/// What the server supports, from its version (`drivers::redis::info::ServerVersion::gates`).
/// The defaults are a current server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gates {
    /// `SET … KEEPTTL` (6.0).
    pub keepttl: bool,
    /// `UNLINK` and `FLUSHDB ASYNC` (4.0).
    pub unlink: bool,
    /// `COPY` (6.2).
    pub copy: bool,
    /// `SCAN … TYPE` (6.0).
    pub scan_type: bool,
}

impl Default for Gates {
    fn default() -> Self {
        Gates {
            keepttl: true,
            unlink: true,
            copy: true,
            scan_type: true,
        }
    }
}

/// The reply a write must get for its change to count as applied. Redis answers a write whose
/// condition failed (`SET … XX` on a missing key, `RENAMENX` onto an existing one) with nil or 0
/// rather than an error; the apply turns those into the change's error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// Any reply that is not an error.
    Any,
    /// `OK`; nil (or anything else) fails with the message.
    Ok(&'static str),
    /// Exactly this integer; another one fails with the message.
    Int(i64, &'static str),
    /// Exactly these bytes, as a bulk or status reply (a guard's `TYPE`, `HGET` or `LINDEX`);
    /// anything else, nil included, fails with the message.
    Bytes(Vec<u8>, &'static str),
}

/// One command: its argv and the reply it expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub argv: Vec<Vec<u8>>,
    pub expect: Expect,
}

impl Command {
    pub fn new(name: &str) -> Command {
        Command {
            argv: vec![name.as_bytes().to_vec()],
            expect: Expect::Any,
        }
    }

    pub fn arg(mut self, arg: impl AsRef<[u8]>) -> Command {
        self.argv.push(arg.as_ref().to_vec());
        self
    }

    pub fn expect(mut self, expect: Expect) -> Command {
        self.expect = expect;
        self
    }

    /// The command name, upper-case.
    pub fn name(&self) -> String {
        self.argv
            .first()
            .map(|n| String::from_utf8_lossy(n).to_ascii_uppercase())
            .unwrap_or_default()
    }

    /// The redis-cli line of the command (`SET "my key" "a\nb" XX KEEPTTL`).
    pub fn text(&self) -> String {
        argv_text(&self.argv)
    }
}

/// An argv as a redis-cli line.
pub fn argv_text(argv: &[Vec<u8>]) -> String {
    let mut out = String::new();
    for (i, arg) in argv.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&quote_arg(arg));
    }
    out
}

/// One argument the way redis-cli reads it back as the same bytes: bare when it is plain
/// printable text, otherwise in double quotes with `\"`, `\\`, `\n`, `\r`, `\t`, `\a`, `\b` and
/// `\xHH` for other control characters and for bytes that are not UTF-8. `;` is quoted too,
/// because the console splits commands at it.
pub fn quote_arg(arg: &[u8]) -> String {
    let bare = !arg.is_empty()
        && std::str::from_utf8(arg).is_ok_and(|s| {
            s.chars()
                .all(|c| !c.is_control() && !matches!(c, ' ' | '"' | '\'' | ';' | '\u{a0}'))
                && !s.chars().any(char::is_whitespace)
        });
    if bare {
        // Checked above.
        return String::from_utf8_lossy(arg).into_owned();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for chunk in arg.utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{7}' => out.push_str("\\a"),
                '\u{8}' => out.push_str("\\b"),
                c if c.is_control() => {
                    let mut buf = [0u8; 4];
                    for b in c.encode_utf8(&mut buf).bytes() {
                        out.push_str(&format!("\\x{b:02x}"));
                    }
                }
                c => out.push(c),
            }
        }
        for b in chunk.invalid() {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------------------------
// Values as arguments
// ---------------------------------------------------------------------------------------------

/// The bytes a cell value is sent as. Redis stores byte strings: text as UTF-8, numbers in their
/// textual form, a blob as its bytes. There is no NULL (TablePlus deleted the key for one), and a
/// blob only previewed cannot be written back.
pub fn value_arg(value: &Value, what: &str) -> Result<Vec<u8>, AppError> {
    match value {
        Value::Null => Err(AppError::InvalidInput(format!(
            "Redis has no NULL {what}: set an empty string, or delete the key"
        ))),
        Value::Default => Err(AppError::InvalidInput(format!(
            "Redis has no DEFAULT {what}"
        ))),
        Value::Bytes(b) if b.truncated => Err(AppError::InvalidInput(format!(
            "only a preview of the {what} is loaded: load the whole value before editing it"
        ))),
        Value::Bytes(b) => base64::engine::general_purpose::STANDARD
            .decode(&b.base64)
            .map_err(|_| AppError::InvalidInput(format!("the {what} is not valid base64"))),
        Value::Array(_) => Err(AppError::InvalidInput(format!(
            "an array is not a Redis {what}: edit it as text"
        ))),
        other => Ok(other.as_text().unwrap_or_default().into_bytes()),
    }
}

/// A key name from a cell: text or (whole) bytes, never empty-of-meaning values.
pub fn key_arg(value: &Value) -> Result<Vec<u8>, AppError> {
    match value {
        Value::Text(_) | Value::Bytes(_) => value_arg(value, "key"),
        Value::Null => Err(AppError::InvalidInput("a key name is required".into())),
        other => value_arg(other, "key"),
    }
}

/// A TTL cell in seconds as milliseconds: `None` (no expiry) for NULL and -1, an error for zero
/// and other negative numbers (`PEXPIRE` with them deletes the key; delete the row instead).
pub fn ttl_ms(value: &Value) -> Result<Option<i64>, AppError> {
    let bad = || {
        AppError::InvalidInput(
            "a TTL is a number of seconds (empty or -1 for no expiry)".to_string(),
        )
    };
    let secs: f64 = match value {
        Value::Null => return Ok(None),
        Value::Int(n) => *n as f64,
        Value::Float(f) => *f,
        Value::BigInt(s) | Value::Decimal(s) | Value::Text(s) => {
            let s = s.trim();
            if s.is_empty() {
                return Ok(None);
            }
            s.parse::<f64>().map_err(|_| bad())?
        }
        _ => return Err(bad()),
    };
    if !secs.is_finite() {
        return Err(bad());
    }
    if secs == -1.0 {
        return Ok(None);
    }
    let ms = (secs * 1000.0).round();
    if ms < 1.0 {
        return Err(AppError::InvalidInput(
            "a TTL must be positive (empty or -1 for no expiry); delete the key to remove it"
                .into(),
        ));
    }
    // 2^62 ms is far beyond any expiry Redis accepts; it answers out-of-range itself.
    if ms > (1u64 << 62) as f64 {
        return Err(bad());
    }
    Ok(Some(ms as i64))
}

// ---------------------------------------------------------------------------------------------
// Key commands
// ---------------------------------------------------------------------------------------------

const GONE: &str = "the key no longer exists (changed by someone else)";
const EXISTS: &str = "a key with this name already exists";
/// A guarded key is not what was loaded any more, or it changed between `WATCH` and `EXEC`.
pub const CHANGED: &str = "the key was changed by someone else";

/// Replaces a string's value and keeps its TTL. Before 6.0 (no `KEEPTTL`) a script reads the TTL
/// and sets it again in the same atomic step. Fails when the key is gone.
pub fn set_string(key: &[u8], value: &[u8], gates: &Gates) -> Command {
    if gates.keepttl {
        return Command::new("SET")
            .arg(key)
            .arg(value)
            .arg("XX")
            .arg("KEEPTTL")
            .expect(Expect::Ok(GONE));
    }
    Command::new("EVAL")
        .arg(SET_KEEP_TTL_SCRIPT)
        .arg("1")
        .arg(key)
        .arg(value)
        .expect(Expect::Int(1, GONE))
}

/// `SET` that keeps the TTL on servers before 6.0; 0 when the key is gone.
const SET_KEEP_TTL_SCRIPT: &str = "local t = redis.call('PTTL', KEYS[1]) \
if t == -2 then return 0 end \
redis.call('SET', KEYS[1], ARGV[1]) \
if t > 0 then redis.call('PEXPIRE', KEYS[1], t) end \
return 1";

/// A new string key; fails when the name is taken.
pub fn insert_string(key: &[u8], value: &[u8], ttl_ms: Option<i64>) -> Command {
    let cmd = Command::new("SET").arg(key).arg(value).arg("NX");
    let cmd = match ttl_ms {
        Some(ms) => cmd.arg("PX").arg(ms.to_string()),
        None => cmd,
    };
    cmd.expect(Expect::Ok(EXISTS))
}

/// The TTL: `PEXPIRE key ms`, or `PERSIST key` for no expiry.
pub fn set_ttl(key: &[u8], ms: Option<i64>) -> Command {
    match ms {
        Some(ms) => Command::new("PEXPIRE")
            .arg(key)
            .arg(ms.to_string())
            .expect(Expect::Int(1, GONE)),
        // 0 both when the key had no TTL and when it is gone; the guard covers the latter.
        None => Command::new("PERSIST").arg(key),
    }
}

/// Renames a key without overwriting another one.
pub fn rename_key(from: &[u8], to: &[u8]) -> Command {
    Command::new("RENAMENX")
        .arg(from)
        .arg(to)
        .expect(Expect::Int(1, EXISTS))
}

/// Deletes a key: `UNLINK` (frees the memory in the background), `DEL` before 4.0.
pub fn delete_key(key: &[u8], gates: &Gates) -> Command {
    Command::new(if gates.unlink { "UNLINK" } else { "DEL" })
        .arg(key)
        .expect(Expect::Int(1, GONE))
}

/// Copies a key (Duplicate row) without overwriting another one; 6.2 and later.
pub fn copy_key(from: &[u8], to: &[u8], gates: &Gates) -> Result<Command, AppError> {
    if !gates.copy {
        return Err(AppError::Unsupported(
            "copying a key needs Redis 6.2 or later (COPY)".into(),
        ));
    }
    Ok(Command::new("COPY")
        .arg(from)
        .arg(to)
        .expect(Expect::Int(1, EXISTS)))
}

/// Empties the current database (Truncate on a `dbN` item).
pub fn flush_db(gates: &Gates) -> Command {
    let cmd = Command::new("FLUSHDB").expect(Expect::Ok("FLUSHDB failed"));
    if gates.unlink { cmd.arg("ASYNC") } else { cmd }
}

/// `SELECT n`, for the editor connection.
pub fn select_db(index: u32) -> Command {
    Command::new("SELECT")
        .arg(index.to_string())
        .expect(Expect::Ok("SELECT failed"))
}

/// The commands of an optimistic transaction around a plan.
pub fn watch(keys: &[&[u8]]) -> Command {
    keys.iter().fold(Command::new("WATCH"), |c, k| c.arg(k))
}

pub fn multi() -> Command {
    Command::new("MULTI")
}

pub fn exec() -> Command {
    Command::new("EXEC")
}

pub fn discard() -> Command {
    Command::new("DISCARD")
}

pub fn unwatch() -> Command {
    Command::new("UNWATCH")
}

/// `TYPE key`, the guard's check.
pub fn key_type(key: &[u8]) -> Command {
    Command::new("TYPE").arg(key)
}

/// `EXISTS key`, the guard of a new key.
pub fn exists(key: &[u8]) -> Command {
    Command::new("EXISTS").arg(key)
}

// ---------------------------------------------------------------------------------------------
// Reads: the key browser, the value inspector and introspection
// ---------------------------------------------------------------------------------------------

/// `SCAN cursor [MATCH pattern] [COUNT n] [TYPE type]` (the cursor is checked by the caller,
/// `drivers::redis::scan::cursor`).
pub fn scan(cursor: &str, pattern: Option<&[u8]>, count: u32, ty: Option<&str>) -> Command {
    let mut c = Command::new("SCAN").arg(cursor);
    if let Some(p) = pattern {
        c = c.arg("MATCH").arg(p);
    }
    if count > 0 {
        c = c.arg("COUNT").arg(count.to_string());
    }
    if let Some(t) = ty {
        c = c.arg("TYPE").arg(t);
    }
    c
}

pub fn pttl(key: &[u8]) -> Command {
    Command::new("PTTL").arg(key)
}

/// `MEMORY USAGE key SAMPLES 0` (every element counted).
pub fn memory_usage(key: &[u8]) -> Command {
    Command::new("MEMORY")
        .arg("USAGE")
        .arg(key)
        .arg("SAMPLES")
        .arg("0")
}

/// The size of a key of type `ty`: its length or number of elements; `None` for types without
/// a standard command (RedisJSON, modules).
pub fn size_of(ty: &str, key: &[u8]) -> Option<Command> {
    let name = match ty {
        TYPE_STRING => "STRLEN",
        "list" => "LLEN",
        "set" => "SCARD",
        "zset" => "ZCARD",
        "hash" => "HLEN",
        "stream" => "XLEN",
        _ => return None,
    };
    Some(Command::new(name).arg(key))
}

/// The value preview of a key of type `ty`: the first `bytes` of a string, the first `elements`
/// of a container (a stream's newest entries). A RedisJSON document is the exception and is read
/// whole, because RedisJSON has no ranged read of a document (a limit of the brief).
pub fn preview_of(ty: &str, key: &[u8], bytes: u64, elements: u32) -> Option<Command> {
    let n = elements.max(1);
    Some(match ty {
        TYPE_STRING => get_range(key, 0, bytes.max(1) - 1),
        "list" => list_range(key, 0, i64::from(n) - 1),
        "set" => set_scan(key, "0", n),
        "zset" => zset_range(key, 0, i64::from(n) - 1),
        "hash" => hash_scan(key, "0", n, false),
        "stream" => stream_range_rev(key, "+", "-", n),
        TYPE_JSON => json_get(key, None),
        _ => return None,
    })
}

/// `GETRANGE key start end` (inclusive).
pub fn get_range(key: &[u8], start: u64, end: u64) -> Command {
    Command::new("GETRANGE")
        .arg(key)
        .arg(start.to_string())
        .arg(end.to_string())
}

pub fn get(key: &[u8]) -> Command {
    Command::new("GET").arg(key)
}

pub fn list_range(key: &[u8], start: i64, stop: i64) -> Command {
    Command::new("LRANGE")
        .arg(key)
        .arg(start.to_string())
        .arg(stop.to_string())
}

pub fn set_scan(key: &[u8], cursor: &str, count: u32) -> Command {
    Command::new("SSCAN")
        .arg(key)
        .arg(cursor)
        .arg("COUNT")
        .arg(count.to_string())
}

/// `HSCAN`, with `NOVALUES` (7.4) for a field list.
pub fn hash_scan(key: &[u8], cursor: &str, count: u32, novalues: bool) -> Command {
    let c = Command::new("HSCAN")
        .arg(key)
        .arg(cursor)
        .arg("COUNT")
        .arg(count.to_string());
    if novalues { c.arg("NOVALUES") } else { c }
}

pub fn zset_scan(key: &[u8], cursor: &str, count: u32) -> Command {
    Command::new("ZSCAN")
        .arg(key)
        .arg(cursor)
        .arg("COUNT")
        .arg(count.to_string())
}

/// `ZRANGE key start stop WITHSCORES` (by rank).
pub fn zset_range(key: &[u8], start: i64, stop: i64) -> Command {
    Command::new("ZRANGE")
        .arg(key)
        .arg(start.to_string())
        .arg(stop.to_string())
        .arg("WITHSCORES")
}

/// `XRANGE key start end COUNT n`.
pub fn stream_range(key: &[u8], start: &str, end: &str, count: u32) -> Command {
    Command::new("XRANGE")
        .arg(key)
        .arg(start)
        .arg(end)
        .arg("COUNT")
        .arg(count.to_string())
}

/// `XREVRANGE key end start COUNT n` (newest first).
pub fn stream_range_rev(key: &[u8], end: &str, start: &str, count: u32) -> Command {
    Command::new("XREVRANGE")
        .arg(key)
        .arg(end)
        .arg(start)
        .arg("COUNT")
        .arg(count.to_string())
}

/// `JSON.GET key [path]`; without a path the whole document as JSON (not wrapped in an array as
/// a `$` path would be).
pub fn json_get(key: &[u8], path: Option<&str>) -> Command {
    let c = Command::new("JSON.GET").arg(key);
    match path {
        Some(p) => c.arg(p),
        None => c,
    }
}

pub fn info(section: Option<&str>) -> Command {
    let c = Command::new("INFO");
    match section {
        Some(s) => c.arg(s),
        None => c,
    }
}

pub fn config_get(parameter: &str) -> Command {
    Command::new("CONFIG").arg("GET").arg(parameter)
}

pub fn dbsize() -> Command {
    Command::new("DBSIZE")
}

pub fn ping() -> Command {
    Command::new("PING")
}

/// Every command with its flags (`COMMAND`; `COMMAND INFO` without names needs 7.0).
pub fn command_table() -> Command {
    Command::new("COMMAND")
}

/// The connection's id, for cancel.
pub fn client_id() -> Command {
    Command::new("CLIENT").arg("ID")
}

/// Ends a connection's running command (cancel), sent on another connection.
pub fn client_kill(id: i64) -> Command {
    Command::new("CLIENT")
        .arg("KILL")
        .arg("ID")
        .arg(id.to_string())
}

// ---------------------------------------------------------------------------------------------
// New keys and container edits (the per-type value editor and the New key dialog)
// ---------------------------------------------------------------------------------------------

/// A key the New key dialog creates.
#[derive(Debug, Clone, PartialEq)]
pub enum NewKey {
    String(Vec<u8>),
    Hash(Vec<(Vec<u8>, Vec<u8>)>),
    List(Vec<Vec<u8>>),
    Set(Vec<Vec<u8>>),
    /// `(score, member)`.
    ZSet(Vec<(String, Vec<u8>)>),
    /// The first entry: its id (`None` = `*`) and fields.
    Stream {
        id: Option<String>,
        fields: Vec<(Vec<u8>, Vec<u8>)>,
    },
    /// A RedisJSON document.
    Json(String),
}

impl NewKey {
    pub fn type_name(&self) -> &'static str {
        match self {
            NewKey::String(_) => TYPE_STRING,
            NewKey::Hash(_) => "hash",
            NewKey::List(_) => "list",
            NewKey::Set(_) => "set",
            NewKey::ZSet(_) => "zset",
            NewKey::Stream { .. } => "stream",
            NewKey::Json(_) => TYPE_JSON,
        }
    }
}

/// The commands that create `key`, behind `Guard::Absent` unless they are a single command that
/// refuses an existing key itself (`SET … NX [PX]`, `JSON.SET … NX` without a TTL). `HSET`,
/// `RPUSH`, `SADD`, `ZADD` and `XADD` would add to a key that exists, and inside `MULTI` the
/// `PEXPIRE` after a refused `JSON.SET … NX` would still set the TTL of the key that was there.
pub fn create_key(key: &[u8], new: &NewKey, ttl_ms: Option<i64>) -> Result<Guarded, AppError> {
    let empty = |what: &str| {
        Err(AppError::InvalidInput(format!(
            "a new {what} needs at least one element (Redis deletes empty ones)"
        )))
    };
    let mut cmds = Vec::new();
    match new {
        NewKey::String(v) => {
            return Ok(Guarded {
                guard: Guard::None,
                commands: vec![insert_string(key, v, ttl_ms)],
            });
        }
        NewKey::Hash(pairs) => {
            if pairs.is_empty() {
                return empty("hash");
            }
            let c = pairs
                .iter()
                .fold(Command::new("HSET").arg(key), |c, (f, v)| c.arg(f).arg(v));
            cmds.push(c);
        }
        NewKey::List(items) => {
            if items.is_empty() {
                return empty("list");
            }
            cmds.push(
                items
                    .iter()
                    .fold(Command::new("RPUSH").arg(key), |c, v| c.arg(v)),
            );
        }
        NewKey::Set(items) => {
            if items.is_empty() {
                return empty("set");
            }
            cmds.push(
                items
                    .iter()
                    .fold(Command::new("SADD").arg(key), |c, v| c.arg(v)),
            );
        }
        NewKey::ZSet(items) => {
            if items.is_empty() {
                return empty("sorted set");
            }
            let mut c = Command::new("ZADD").arg(key);
            for (score, member) in items {
                c = c.arg(score_arg(score)?).arg(member);
            }
            cmds.push(c);
        }
        NewKey::Stream { id, fields } => cmds.push(stream_add(key, id.as_deref(), fields)?),
        NewKey::Json(doc) => {
            cmds.push(
                json_set(key, "$", doc)?
                    .arg("NX")
                    .expect(Expect::Ok(EXISTS)),
            );
        }
    }
    if let Some(ms) = ttl_ms {
        cmds.push(set_ttl(key, Some(ms)));
    }
    let refuses_itself = matches!(new, NewKey::Json(_)) && cmds.len() == 1;
    let guard = if refuses_itself {
        Guard::None
    } else {
        Guard::Absent { key: key.to_vec() }
    };
    Ok(Guarded {
        guard,
        commands: cmds,
    })
}

/// A score as `ZADD` takes it: a float, `inf`, `+inf` or `-inf`.
pub fn score_arg(score: &str) -> Result<String, AppError> {
    let s = score.trim();
    let ok = matches!(s.to_ascii_lowercase().as_str(), "inf" | "+inf" | "-inf")
        || s.parse::<f64>().is_ok_and(f64::is_finite);
    if ok {
        Ok(s.to_string())
    } else {
        Err(AppError::InvalidInput(format!(
            "“{score}” is not a score (a number, inf or -inf)"
        )))
    }
}

/// A stream entry id: `ms` or `ms-seq` (or `*` for the server's next one).
pub fn stream_id_arg(id: &str) -> Result<String, AppError> {
    let id = id.trim();
    if id == "*" {
        return Ok(id.to_string());
    }
    let digits = |s: &str| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit());
    let ok = match id.split_once('-') {
        Some((ms, seq)) => digits(ms) && (digits(seq) || seq == "*"),
        None => digits(id),
    };
    if ok {
        Ok(id.to_string())
    } else {
        Err(AppError::InvalidInput(format!(
            "“{id}” is not a stream entry id (milliseconds-sequence)"
        )))
    }
}

pub fn hash_set(key: &[u8], field: &[u8], value: &[u8]) -> Command {
    Command::new("HSET").arg(key).arg(field).arg(value)
}

/// A new field; fails when it exists.
pub fn hash_add(key: &[u8], field: &[u8], value: &[u8]) -> Command {
    Command::new("HSETNX")
        .arg(key)
        .arg(field)
        .arg(value)
        .expect(Expect::Int(1, "the field already exists"))
}

pub fn hash_delete(key: &[u8], field: &[u8]) -> Command {
    Command::new("HDEL")
        .arg(key)
        .arg(field)
        .expect(Expect::Int(1, "the field no longer exists"))
}

/// Renames a field that holds `value` (as loaded): the new one first, then the old one goes.
/// Inside `MULTI` Redis still runs the `HDEL` after a refused `HSETNX`, which would lose the
/// field, so the guard proves under `WATCH`, before `MULTI`, that the old field still holds
/// `value` and the new name is free.
pub fn hash_rename(key: &[u8], from: &[u8], to: &[u8], value: &[u8]) -> Result<Guarded, AppError> {
    if from == to {
        return Err(AppError::InvalidInput(
            "the field already has this name".into(),
        ));
    }
    let checks = vec![
        Command::new("HGET")
            .arg(key)
            .arg(from)
            .expect(Expect::Bytes(value.to_vec(), FIELD_CHANGED)),
        Command::new("HEXISTS")
            .arg(key)
            .arg(to)
            .expect(Expect::Int(0, "the field already exists")),
    ];
    Ok(Guarded {
        guard: Guard::Checks {
            key: key.to_vec(),
            checks,
        },
        commands: vec![hash_add(key, to, value), hash_delete(key, from)],
    })
}

const FIELD_CHANGED: &str = "the field changed or no longer exists (changed by someone else)";

/// `LSET`, unguarded: `list_replace` and `list_delete_at` prove the position first.
pub fn list_set(key: &[u8], index: i64, value: &[u8]) -> Command {
    Command::new("LSET")
        .arg(key)
        .arg(index.to_string())
        .arg(value)
        .expect(Expect::Ok("the element no longer exists"))
}

pub fn list_push(key: &[u8], values: &[Vec<u8>], head: bool) -> Command {
    values.iter().fold(
        Command::new(if head { "LPUSH" } else { "RPUSH" }).arg(key),
        |c, v| c.arg(v),
    )
}

/// Inserts next to the first element equal to `pivot`; -1 (no pivot) fails.
pub fn list_insert(key: &[u8], before: bool, pivot: &[u8], value: &[u8]) -> Command {
    Command::new("LINSERT")
        .arg(key)
        .arg(if before { "BEFORE" } else { "AFTER" })
        .arg(pivot)
        .arg(value)
}

/// The guard of a positional list edit: a push or pop elsewhere shifts the positions, so the
/// element at `index` must still be `old` (as loaded).
fn list_holds(key: &[u8], index: i64, old: &[u8]) -> Guard {
    Guard::Checks {
        key: key.to_vec(),
        checks: vec![
            Command::new("LINDEX")
                .arg(key)
                .arg(index.to_string())
                .expect(Expect::Bytes(
                    old.to_vec(),
                    "the element changed or moved (changed by someone else)",
                )),
        ],
    }
}

/// Replaces the element at `index`, which holds `old` (as loaded), by `value`.
pub fn list_replace(key: &[u8], index: i64, old: &[u8], value: &[u8]) -> Guarded {
    Guarded {
        guard: list_holds(key, index, old),
        commands: vec![list_set(key, index, value)],
    }
}

/// Deletes the element at `index`, which holds `old` (as loaded): Redis deletes list elements by
/// value only, so the element is first replaced by a marker no other element has (the caller
/// makes it unique), then removed.
pub fn list_delete_at(key: &[u8], index: i64, old: &[u8], marker: &[u8]) -> Guarded {
    Guarded {
        guard: list_holds(key, index, old),
        commands: vec![
            list_set(key, index, marker),
            Command::new("LREM")
                .arg(key)
                .arg("1")
                .arg(marker)
                .expect(Expect::Int(1, "the element no longer exists")),
        ],
    }
}

pub fn set_add(key: &[u8], member: &[u8]) -> Command {
    Command::new("SADD")
        .arg(key)
        .arg(member)
        .expect(Expect::Int(1, "the member already exists"))
}

pub fn set_remove(key: &[u8], member: &[u8]) -> Command {
    Command::new("SREM")
        .arg(key)
        .arg(member)
        .expect(Expect::Int(1, "the member no longer exists"))
}

/// Renames a set member: the new one first, then the old one goes. Inside `MULTI` a refused
/// command does not stop the other one, so the guard proves under `WATCH`, before `MULTI`, that
/// the old member is there and the new one is not.
pub fn set_rename(key: &[u8], from: &[u8], to: &[u8]) -> Result<Guarded, AppError> {
    if from == to {
        return Err(AppError::InvalidInput(
            "the member already has this name".into(),
        ));
    }
    let is_member = |m: &[u8], want: i64, msg: &'static str| {
        Command::new("SISMEMBER")
            .arg(key)
            .arg(m)
            .expect(Expect::Int(want, msg))
    };
    Ok(Guarded {
        guard: Guard::Checks {
            key: key.to_vec(),
            checks: vec![
                is_member(from, 1, "the member no longer exists"),
                is_member(to, 0, "the member already exists"),
            ],
        },
        commands: vec![set_add(key, to), set_remove(key, from)],
    })
}

/// A new member of a sorted set; fails when it exists.
pub fn zset_add(key: &[u8], score: &str, member: &[u8]) -> Result<Command, AppError> {
    Ok(Command::new("ZADD")
        .arg(key)
        .arg("NX")
        .arg(score_arg(score)?)
        .arg(member)
        .expect(Expect::Int(1, "the member already exists")))
}

/// A member's new score. `XX` never adds back a member that was removed meanwhile; it answers 0
/// both then and for any update without `CH`, so the reply proves nothing either way.
pub fn zset_score(key: &[u8], score: &str, member: &[u8]) -> Result<Command, AppError> {
    Ok(Command::new("ZADD")
        .arg(key)
        .arg("XX")
        .arg(score_arg(score)?)
        .arg(member))
}

pub fn zset_remove(key: &[u8], member: &[u8]) -> Command {
    Command::new("ZREM")
        .arg(key)
        .arg(member)
        .expect(Expect::Int(1, "the member no longer exists"))
}

/// `XADD key id field value …`.
pub fn stream_add(
    key: &[u8],
    id: Option<&str>,
    fields: &[(Vec<u8>, Vec<u8>)],
) -> Result<Command, AppError> {
    if fields.is_empty() {
        return Err(AppError::InvalidInput(
            "a stream entry needs at least one field".into(),
        ));
    }
    let id = stream_id_arg(id.unwrap_or("*"))?;
    Ok(fields
        .iter()
        .fold(Command::new("XADD").arg(key).arg(id), |c, (f, v)| {
            c.arg(f).arg(v)
        }))
}

pub fn stream_delete(key: &[u8], id: &str) -> Result<Command, AppError> {
    Ok(Command::new("XDEL")
        .arg(key)
        .arg(stream_id_arg(id)?)
        .expect(Expect::Int(1, "the entry no longer exists")))
}

/// `JSON.SET key path doc`, with the document checked to be JSON first.
pub fn json_set(key: &[u8], path: &str, doc: &str) -> Result<Command, AppError> {
    if serde_json::from_str::<serde::de::IgnoredAny>(doc).is_err() {
        return Err(AppError::InvalidInput("the value is not valid JSON".into()));
    }
    Ok(Command::new("JSON.SET")
        .arg(key)
        .arg(path)
        .arg(doc)
        .expect(Expect::Ok(GONE)))
}

pub fn json_delete(key: &[u8], path: &str) -> Command {
    Command::new("JSON.DEL").arg(key).arg(path)
}

// ---------------------------------------------------------------------------------------------
// Grid changes
// ---------------------------------------------------------------------------------------------

/// What the apply proves before it runs a change's commands: it `WATCH`es the key, sends the
/// guard's `checks`, and only when each answers as it expects sends the commands in one
/// `MULTI` … `EXEC` (a nil EXEC reply means someone changed the key in between). Redis runs the
/// rest of a transaction after one of its commands fails, so a change of several commands needs
/// a guard whenever a failed command would let the next one do harm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Guard {
    None,
    /// The key exists with this `TYPE`.
    Type {
        key: Vec<u8>,
        ty: String,
    },
    /// The key does not exist.
    Absent {
        key: Vec<u8>,
    },
    /// Reads of the key that must answer as they expect: a hash field or set member there or
    /// not, a list position still holding what was loaded.
    Checks {
        key: Vec<u8>,
        checks: Vec<Command>,
    },
}

impl Guard {
    pub fn key(&self) -> Option<&[u8]> {
        match self {
            Guard::None => None,
            Guard::Type { key, .. } | Guard::Absent { key } | Guard::Checks { key, .. } => {
                Some(key)
            }
        }
    }

    /// The reads the apply sends after `WATCH key` and before `MULTI`, each with the reply it
    /// must get; the first one that gets another is the change's error, and nothing is sent.
    pub fn checks(&self) -> Vec<Command> {
        match self {
            Guard::None => Vec::new(),
            Guard::Type { key, ty } => {
                vec![key_type(key).expect(Expect::Bytes(ty.as_bytes().to_vec(), CHANGED))]
            }
            Guard::Absent { key } => vec![exists(key).expect(Expect::Int(0, EXISTS))],
            Guard::Checks { checks, .. } => checks.clone(),
        }
    }
}

/// Commands that apply together behind their guard, in one `MULTI` … `EXEC`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guarded {
    pub guard: Guard,
    pub commands: Vec<Command>,
}

impl Guarded {
    /// As change `change` of a plan.
    pub fn edit(self, change: usize) -> KeyEdit {
        KeyEdit {
            change,
            guard: self.guard,
            commands: self.commands,
        }
    }
}

/// The commands of one staged change.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyEdit {
    /// The change's index in the request.
    pub change: usize,
    pub guard: Guard,
    pub commands: Vec<Command>,
}

/// The key and type of a row (`RowKey::Pk { key, type }`); the type is absent in rows of older
/// pages and then only TTL edits, renames and deletes are planned.
pub fn row_key(key: &RowKey) -> Result<(Vec<u8>, Option<String>), AppError> {
    let RowKey::Pk(cols) = key else {
        return Err(AppError::InvalidInput(
            "Redis rows are addressed by their key".into(),
        ));
    };
    let k = cols
        .get(col::KEY)
        .ok_or_else(|| AppError::InvalidInput("the row has no key".into()))?;
    let ty = match cols.get(col::TYPE) {
        Some(Value::Text(t) | Value::Enum(t)) => Some(t.clone()),
        _ => None,
    };
    Ok((key_arg(k)?, ty))
}

/// Plans one staged change of a key table. Updates write the value, then the TTL, then rename
/// (last, so a retry after a refused rename only repeats idempotent writes).
pub fn plan_change(
    index: usize,
    change: &StagedChange,
    gates: &Gates,
) -> Result<KeyEdit, AppError> {
    match change {
        StagedChange::Update {
            key,
            changes,
            exprs,
            ..
        } => {
            refuse_exprs(exprs.as_ref())?;
            let (k, ty) = row_key(key)?;
            plan_update(index, k, ty, changes, gates)
        }
        StagedChange::Insert { values, exprs, .. } => {
            refuse_exprs(exprs.as_ref())?;
            plan_insert(index, values)
        }
        StagedChange::Delete { key, .. } => {
            let (k, ty) = row_key(key)?;
            let guard = match ty {
                Some(ty) => Guard::Type { key: k.clone(), ty },
                None => Guard::None,
            };
            Ok(KeyEdit {
                change: index,
                commands: vec![delete_key(&k, gates)],
                guard,
            })
        }
    }
}

fn refuse_exprs<T>(exprs: Option<&BTreeMap<String, T>>) -> Result<(), AppError> {
    match exprs {
        Some(e) if !e.is_empty() => Err(AppError::InvalidInput(
            "Redis has no server expressions (NOW(), DEFAULT)".into(),
        )),
        _ => Ok(()),
    }
}

fn read_only_column(column: &str) -> AppError {
    AppError::InvalidInput(format!("the {column} column is read-only"))
}

fn plan_update(
    index: usize,
    key: Vec<u8>,
    ty: Option<String>,
    changes: &BTreeMap<String, Value>,
    gates: &Gates,
) -> Result<KeyEdit, AppError> {
    let mut value = None;
    let mut ttl = None;
    let mut rename = None;
    for (column, v) in changes {
        match column.as_str() {
            col::VALUE => value = Some(v),
            col::TTL => ttl = Some(ttl_ms(v)?),
            col::KEY => rename = Some(key_arg(v)?),
            col::TYPE | col::SIZE | col::MEMORY => return Err(read_only_column(column)),
            other => {
                return Err(AppError::InvalidInput(format!(
                    "a Redis key has no column “{other}”"
                )));
            }
        }
    }
    let mut commands = Vec::new();
    if let Some(v) = value {
        match ty.as_deref() {
            Some(TYPE_STRING) => commands.push(set_string(&key, &value_arg(v, "value")?, gates)),
            Some(TYPE_JSON) => {
                let doc = String::from_utf8(value_arg(v, "value")?)
                    .map_err(|_| AppError::InvalidInput("the value is not valid JSON".into()))?;
                commands.push(json_set(&key, "$", &doc)?.arg("XX"));
            }
            Some(other) => {
                return Err(AppError::InvalidInput(format!(
                    "a {other} is edited element by element in the value editor"
                )));
            }
            None => {
                return Err(AppError::InvalidInput(
                    "the key's type is not known: reload the page before editing its value".into(),
                ));
            }
        }
    }
    if let Some(ms) = ttl {
        commands.push(set_ttl(&key, ms));
    }
    if let Some(to) = rename.filter(|to| *to != key) {
        commands.push(rename_key(&key, &to));
    }
    let guard = match ty {
        Some(ty) => Guard::Type {
            key: key.clone(),
            ty,
        },
        None => Guard::None,
    };
    Ok(KeyEdit {
        change: index,
        guard,
        commands,
    })
}

fn plan_insert(index: usize, values: &BTreeMap<String, Value>) -> Result<KeyEdit, AppError> {
    let mut key = None;
    let mut ty = None;
    let mut value = None;
    let mut ttl = None;
    for (column, v) in values {
        match column.as_str() {
            col::KEY => key = Some(key_arg(v)?),
            col::TYPE => match v {
                Value::Null => {}
                Value::Text(t) | Value::Enum(t) => ty = Some(t.as_str()),
                _ => return Err(AppError::InvalidInput("the type is a name".into())),
            },
            col::VALUE => value = Some(v),
            col::TTL => ttl = ttl_ms(v)?,
            // Computed by the server: an insert may carry the renderer's empty cells.
            col::SIZE | col::MEMORY if v.is_null() => {}
            col::SIZE | col::MEMORY => return Err(read_only_column(column)),
            other => {
                return Err(AppError::InvalidInput(format!(
                    "a Redis key has no column “{other}”"
                )));
            }
        }
    }
    let key = key.ok_or_else(|| AppError::InvalidInput("a new key needs a name".into()))?;
    let new = match ty.unwrap_or(TYPE_STRING) {
        TYPE_STRING => NewKey::String(match value {
            None | Some(Value::Null) => Vec::new(),
            Some(v) => value_arg(v, "value")?,
        }),
        TYPE_JSON => NewKey::Json(match value {
            None | Some(Value::Null) => "{}".to_string(),
            Some(v) => String::from_utf8(value_arg(v, "value")?)
                .map_err(|_| AppError::InvalidInput("the value is not valid JSON".into()))?,
        }),
        other => {
            return Err(AppError::InvalidInput(format!(
                "a new {other} is created with New key, which asks for its first elements"
            )));
        }
    };
    Ok(create_key(&key, &new, ttl)?.edit(index))
}

/// Plans every change of a request; the first one that cannot be planned fails the whole plan
/// (nothing is sent), like the SQL builders.
pub fn plan_changes(changes: &[StagedChange], gates: &Gates) -> Result<Vec<KeyEdit>, AppError> {
    changes
        .iter()
        .enumerate()
        .map(|(i, c)| plan_change(i, c, gates))
        .collect()
}

/// Preview lines of a plan, per change, the way the apply sends them: each change in its own
/// `MULTI` … `EXEC` (Redis has no rollback, so a change is the unit that applies or not), after a
/// `WATCH` of its guarded key.
pub fn preview(plan: &[KeyEdit]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for edit in plan {
        if let Some(key) = edit.guard.key() {
            out.push((edit.change, watch(&[key]).text()));
        }
        let wrap = edit.commands.len() > 1 || edit.guard != Guard::None;
        if wrap {
            out.push((edit.change, multi().text()));
        }
        for c in &edit.commands {
            out.push((edit.change, c.text()));
        }
        if wrap {
            out.push((edit.change, exec().text()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::redis::cli::tokenize;
    use crate::types::value::BytesValue;

    fn pk(key: &str, ty: Option<&str>) -> RowKey {
        let mut m = BTreeMap::new();
        m.insert(col::KEY.to_string(), Value::Text(key.into()));
        if let Some(t) = ty {
            m.insert(col::TYPE.to_string(), Value::Text(t.into()));
        }
        RowKey::Pk(m)
    }

    fn update(key: &str, ty: Option<&str>, changes: &[(&str, Value)]) -> StagedChange {
        StagedChange::Update {
            row_id: "r".into(),
            key: pk(key, ty),
            changes: changes
                .iter()
                .map(|(c, v)| (c.to_string(), v.clone()))
                .collect(),
            exprs: None,
        }
    }

    fn texts(edit: &KeyEdit) -> Vec<String> {
        edit.commands.iter().map(Command::text).collect()
    }

    #[test]
    fn quoting_keeps_plain_words_bare() {
        assert_eq!(quote_arg(b"user:1"), "user:1");
        assert_eq!(quote_arg(b"caf\xc3\xa9"), "café");
        assert_eq!(quote_arg(b""), "\"\"");
        assert_eq!(quote_arg(b"a b"), "\"a b\"");
        assert_eq!(quote_arg(b"a;b"), "\"a;b\"");
        assert_eq!(quote_arg(b"it's"), "\"it's\"");
        assert_eq!(quote_arg(b"say \"hi\"\n"), "\"say \\\"hi\\\"\\n\"");
        assert_eq!(quote_arg(b"\x00\xff\\"), "\"\\x00\\xff\\\\\"");
        assert_eq!(quote_arg(b"\x07\x08\t\r"), "\"\\a\\b\\t\\r\"");
        assert_eq!(quote_arg(b"\x0b"), "\"\\x0b\"");
        assert_eq!(quote_arg("a\u{2003}b".as_bytes()), "\"a\u{2003}b\"");
    }

    #[test]
    fn quoting_round_trips_through_the_tokenizer() {
        // A fixed-seed generator over bytes that matter to the tokenizer.
        let alphabet: &[u8] = b"ab \t\n\r\"'\\x;\x00\x0b\x0c\xff\xc3\xa9\x07\x08$*[]";
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..3000 {
            let mut args = Vec::new();
            for _ in 0..(seed % 4 + 1) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let len = (seed % 9) as usize;
                let arg: Vec<u8> = (0..len)
                    .map(|i| alphabet[((seed >> (i * 5)) as usize) % alphabet.len()])
                    .collect();
                args.push(arg);
            }
            let line = argv_text(&args);
            assert_eq!(tokenize(&line).unwrap(), args, "line {line:?}");
        }
    }

    #[test]
    fn values_as_arguments() {
        assert_eq!(value_arg(&Value::Text("x".into()), "value").unwrap(), b"x");
        assert_eq!(value_arg(&Value::Int(-3), "value").unwrap(), b"-3");
        assert_eq!(
            value_arg(&Value::BigInt("9007199254740993".into()), "value").unwrap(),
            b"9007199254740993"
        );
        assert_eq!(
            value_arg(&Value::from_bytes(&[0, 255]), "value").unwrap(),
            vec![0, 255]
        );
        let truncated = Value::Bytes(BytesValue {
            base64: "AA==".into(),
            len: 10,
            truncated: true,
        });
        assert!(value_arg(&truncated, "value").is_err());
        assert!(value_arg(&Value::Null, "value").is_err());
        assert!(value_arg(&Value::Default, "value").is_err());
        assert!(key_arg(&Value::Null).is_err());
    }

    #[test]
    fn ttl_cells() {
        assert_eq!(ttl_ms(&Value::Null).unwrap(), None);
        assert_eq!(ttl_ms(&Value::Int(-1)).unwrap(), None);
        assert_eq!(ttl_ms(&Value::Text(" ".into())).unwrap(), None);
        assert_eq!(ttl_ms(&Value::Int(30)).unwrap(), Some(30_000));
        assert_eq!(ttl_ms(&Value::Float(1.5)).unwrap(), Some(1500));
        assert_eq!(ttl_ms(&Value::Text("2".into())).unwrap(), Some(2000));
        assert!(ttl_ms(&Value::Int(0)).is_err());
        assert!(ttl_ms(&Value::Int(-2)).is_err());
        assert!(ttl_ms(&Value::Text("soon".into())).is_err());
        assert!(ttl_ms(&Value::Float(f64::INFINITY)).is_err());
        assert!(ttl_ms(&Value::BigInt("99999999999999999999".into())).is_err());
    }

    #[test]
    fn string_edit_keeps_the_ttl_and_requires_the_key() {
        let gates = Gates::default();
        let edit = plan_change(
            0,
            &update(
                "user:1",
                Some("string"),
                &[("value", Value::Text("new v".into()))],
            ),
            &gates,
        )
        .unwrap();
        assert_eq!(texts(&edit), ["SET user:1 \"new v\" XX KEEPTTL"]);
        assert_eq!(edit.commands[0].expect, Expect::Ok(GONE));
        assert_eq!(
            edit.guard,
            Guard::Type {
                key: b"user:1".to_vec(),
                ty: "string".into()
            }
        );
        assert_eq!(
            edit.guard.checks(),
            [Command::new("TYPE")
                .arg("user:1")
                .expect(Expect::Bytes(b"string".to_vec(), CHANGED))]
        );
        // Before 6.0 a script keeps the TTL.
        let old = Gates {
            keepttl: false,
            ..gates
        };
        let edit = plan_change(
            0,
            &update("k", Some("string"), &[("value", Value::Text("v".into()))]),
            &old,
        )
        .unwrap();
        assert_eq!(edit.commands[0].name(), "EVAL");
        assert_eq!(
            edit.commands[0].argv[2..],
            [b"1".to_vec(), b"k".to_vec(), b"v".to_vec()]
        );
    }

    #[test]
    fn value_ttl_and_rename_in_that_order() {
        let edit = plan_change(
            2,
            &update(
                "old",
                Some("string"),
                &[
                    ("key", Value::Text("new name".into())),
                    ("ttl", Value::Int(60)),
                    ("value", Value::Text("".into())),
                ],
            ),
            &Gates::default(),
        )
        .unwrap();
        assert_eq!(edit.change, 2);
        assert_eq!(
            texts(&edit),
            [
                "SET old \"\" XX KEEPTTL",
                "PEXPIRE old 60000",
                "RENAMENX old \"new name\""
            ]
        );
        assert_eq!(edit.commands[2].expect, Expect::Int(1, EXISTS));
        // Clearing the TTL persists the key; renaming to the same name is nothing.
        let edit = plan_change(
            0,
            &update(
                "k",
                None,
                &[("ttl", Value::Null), ("key", Value::Text("k".into()))],
            ),
            &Gates::default(),
        )
        .unwrap();
        assert_eq!(texts(&edit), ["PERSIST k"]);
        assert_eq!(edit.guard, Guard::None);
    }

    #[test]
    fn refused_updates() {
        let g = Gates::default();
        let refused = |c: StagedChange| plan_change(0, &c, &g).unwrap_err().to_string();
        assert!(refused(update("k", Some("string"), &[("value", Value::Null)])).contains("NULL"));
        assert!(
            refused(update(
                "k",
                Some("hash"),
                &[("value", Value::Text("x".into()))]
            ))
            .contains("value editor")
        );
        assert!(
            refused(update("k", None, &[("value", Value::Text("x".into()))])).contains("reload")
        );
        assert!(
            refused(update(
                "k",
                Some("string"),
                &[("type", Value::Text("hash".into()))]
            ))
            .contains("read-only")
        );
        assert!(
            refused(update("k", Some("string"), &[("size", Value::Int(1))])).contains("read-only")
        );
        assert!(
            refused(update("k", Some("string"), &[("nope", Value::Int(1))])).contains("no column")
        );
        assert!(
            refused(update("k", Some("string"), &[("ttl", Value::Int(0))])).contains("positive")
        );
        let mut exprs = BTreeMap::new();
        exprs.insert("value".to_string(), crate::types::mutate::ServerExpr::Now);
        let with_expr = StagedChange::Update {
            row_id: "r".into(),
            key: pk("k", Some("string")),
            changes: BTreeMap::new(),
            exprs: Some(exprs),
        };
        assert!(refused(with_expr).contains("server expressions"));
        let rowid = StagedChange::Delete {
            row_id: "r".into(),
            key: RowKey::Rowid(1),
        };
        assert!(refused(rowid).contains("addressed by their key"));
    }

    #[test]
    fn json_values_are_checked_and_set_whole() {
        let g = Gates::default();
        let edit = plan_change(
            0,
            &update(
                "doc",
                Some(TYPE_JSON),
                &[("value", Value::Json("{\"a\": [1, 2]}".into()))],
            ),
            &g,
        )
        .unwrap();
        assert_eq!(texts(&edit), ["JSON.SET doc $ \"{\\\"a\\\": [1, 2]}\" XX"]);
        assert!(
            plan_change(
                0,
                &update(
                    "doc",
                    Some(TYPE_JSON),
                    &[("value", Value::Text("{oops".into()))]
                ),
                &g
            )
            .is_err()
        );
    }

    #[test]
    fn inserts_and_deletes() {
        let g = Gates::default();
        let mut values = BTreeMap::new();
        values.insert("key".to_string(), Value::Text("new:1".into()));
        values.insert("value".to_string(), Value::Text("hello".into()));
        values.insert("ttl".to_string(), Value::Int(10));
        values.insert("size".to_string(), Value::Null);
        let insert = StagedChange::Insert {
            row_id: "n".into(),
            values: values.clone(),
            exprs: None,
        };
        let edit = plan_change(0, &insert, &g).unwrap();
        assert_eq!(texts(&edit), ["SET new:1 hello NX PX 10000"]);
        assert_eq!(edit.commands[0].expect, Expect::Ok(EXISTS));
        // One command that refuses an existing key itself needs no guard.
        assert_eq!(edit.guard, Guard::None);

        values.insert("type".to_string(), Value::Text("hash".into()));
        let hash = StagedChange::Insert {
            row_id: "n".into(),
            values: values.clone(),
            exprs: None,
        };
        assert!(
            plan_change(0, &hash, &g)
                .unwrap_err()
                .to_string()
                .contains("New key")
        );

        values.insert("type".to_string(), Value::Text(TYPE_JSON.into()));
        values.insert("value".to_string(), Value::Json("[1]".into()));
        let json = StagedChange::Insert {
            row_id: "n".into(),
            values,
            exprs: None,
        };
        // Two commands: inside MULTI the PEXPIRE would also run after a refused JSON.SET … NX,
        // on the key that was there, so the key must be proved absent first.
        let edit = plan_change(0, &json, &g).unwrap();
        assert_eq!(
            texts(&edit),
            ["JSON.SET new:1 $ [1] NX", "PEXPIRE new:1 10000"]
        );
        assert_eq!(
            edit.guard,
            Guard::Absent {
                key: b"new:1".to_vec()
            }
        );
        assert_eq!(
            edit.guard.checks(),
            [Command::new("EXISTS")
                .arg("new:1")
                .expect(Expect::Int(0, EXISTS))]
        );
        assert_eq!(
            preview(&[edit])
                .into_iter()
                .map(|(_, l)| l)
                .collect::<Vec<_>>(),
            [
                "WATCH new:1",
                "MULTI",
                "JSON.SET new:1 $ [1] NX",
                "PEXPIRE new:1 10000",
                "EXEC"
            ]
        );
        let mut without_ttl = json.clone();
        if let StagedChange::Insert { values, .. } = &mut without_ttl {
            values.remove("ttl");
        }
        assert_eq!(plan_change(0, &without_ttl, &g).unwrap().guard, Guard::None);

        let no_key = StagedChange::Insert {
            row_id: "n".into(),
            values: BTreeMap::new(),
            exprs: None,
        };
        assert!(plan_change(0, &no_key, &g).is_err());

        let delete = StagedChange::Delete {
            row_id: "d".into(),
            key: pk("gone soon", Some("list")),
        };
        let edit = plan_change(1, &delete, &g).unwrap();
        assert_eq!(texts(&edit), ["UNLINK \"gone soon\""]);
        let old = Gates { unlink: false, ..g };
        assert_eq!(
            texts(&plan_change(1, &delete, &old).unwrap()),
            ["DEL \"gone soon\""]
        );
    }

    #[test]
    fn binary_keys_round_trip() {
        let mut m = BTreeMap::new();
        m.insert(
            col::KEY.to_string(),
            Value::from_bytes_uncapped(&[0x00, 0xff, b' ']),
        );
        m.insert(col::TYPE.to_string(), Value::Text("string".into()));
        let mut changes = BTreeMap::new();
        changes.insert("ttl".to_string(), Value::Int(5));
        let edit = plan_change(
            0,
            &StagedChange::Update {
                row_id: "r".into(),
                key: RowKey::Pk(m),
                changes,
                exprs: None,
            },
            &Gates::default(),
        )
        .unwrap();
        assert_eq!(edit.commands[0].argv[1], vec![0x00, 0xff, b' ']);
        assert_eq!(texts(&edit), ["PEXPIRE \"\\x00\\xff \" 5000"]);
        assert_eq!(
            tokenize(&edit.commands[0].text()).unwrap(),
            edit.commands[0].argv
        );
    }

    #[test]
    fn previews_wrap_guarded_changes() {
        let g = Gates::default();
        let plan = plan_changes(
            &[
                update("a", Some("string"), &[("value", Value::Text("1".into()))]),
                StagedChange::Insert {
                    row_id: "n".into(),
                    values: [("key".to_string(), Value::Text("b".into()))].into(),
                    exprs: None,
                },
            ],
            &g,
        )
        .unwrap();
        let lines: Vec<String> = preview(&plan)
            .into_iter()
            .map(|(i, l)| format!("{i} {l}"))
            .collect();
        assert_eq!(
            lines,
            [
                "0 WATCH a",
                "0 MULTI",
                "0 SET a 1 XX KEEPTTL",
                "0 EXEC",
                "1 SET b \"\" NX"
            ]
        );
        // The first change that cannot be planned fails the plan.
        assert!(
            plan_changes(
                &[
                    update("a", Some("string"), &[("value", Value::Text("1".into()))]),
                    update("a", Some("string"), &[("value", Value::Null)]),
                ],
                &g
            )
            .is_err()
        );
    }

    #[test]
    fn new_keys_of_every_type() {
        let k = b"k".as_slice();
        let absent = Guard::Absent { key: k.to_vec() };
        let made = |new: NewKey, ttl: Option<i64>| {
            let g = create_key(k, &new, ttl).unwrap();
            let texts: Vec<String> = g.commands.iter().map(Command::text).collect();
            (texts, g.guard)
        };
        // HSET, RPUSH, SADD, ZADD and XADD would add to a key that exists: they need the guard.
        assert_eq!(
            made(NewKey::Hash(vec![(b"f".to_vec(), b"v 1".to_vec())]), None),
            (vec!["HSET k f \"v 1\"".to_string()], absent.clone())
        );
        assert_eq!(
            made(NewKey::List(vec![b"a".to_vec(), b"b".to_vec()]), Some(5)),
            (
                vec!["RPUSH k a b".to_string(), "PEXPIRE k 5".to_string()],
                absent.clone()
            )
        );
        assert_eq!(
            made(NewKey::Set(vec![b"m".to_vec()]), None),
            (vec!["SADD k m".to_string()], absent.clone())
        );
        assert_eq!(
            made(
                NewKey::ZSet(vec![
                    ("1.5".into(), b"m".to_vec()),
                    ("-inf".into(), b"n".to_vec())
                ]),
                None
            ),
            (vec!["ZADD k 1.5 m -inf n".to_string()], absent.clone())
        );
        assert_eq!(
            made(
                NewKey::Stream {
                    id: None,
                    fields: vec![(b"f".to_vec(), b"v".to_vec())]
                },
                None
            ),
            (vec!["XADD k * f v".to_string()], absent.clone())
        );
        // One command that refuses an existing key itself does not.
        assert_eq!(
            made(NewKey::String(b"v".to_vec()), Some(7)),
            (vec!["SET k v NX PX 7".to_string()], Guard::None)
        );
        assert_eq!(
            made(NewKey::Json("{}".into()), None),
            (vec!["JSON.SET k $ {} NX".to_string()], Guard::None)
        );
        assert_eq!(
            made(NewKey::Json("{}".into()), Some(7)),
            (
                vec!["JSON.SET k $ {} NX".to_string(), "PEXPIRE k 7".to_string()],
                absent
            )
        );
        assert!(create_key(k, &NewKey::Hash(vec![]), None).is_err());
        assert!(create_key(k, &NewKey::ZSet(vec![("abc".into(), b"m".to_vec())]), None).is_err());
        assert!(create_key(k, &NewKey::Json("{".into()), None).is_err());
        assert_eq!(NewKey::Json("{}".into()).type_name(), TYPE_JSON);
        let edit = create_key(k, &NewKey::Set(vec![b"m".to_vec()]), None)
            .unwrap()
            .edit(4);
        assert_eq!(edit.change, 4);
    }

    #[test]
    fn container_renames_and_positional_edits_are_guarded() {
        let k = b"k".as_slice();
        let texts = |g: &Guarded| g.commands.iter().map(Command::text).collect::<Vec<_>>();
        let checks = |g: &Guarded| {
            g.guard
                .checks()
                .iter()
                .map(|c| (c.text(), c.expect.clone()))
                .collect::<Vec<_>>()
        };
        let rename = hash_rename(k, b"a", b"b", b"v").unwrap();
        assert_eq!(texts(&rename), ["HSETNX k b v", "HDEL k a"]);
        assert_eq!(rename.guard.key(), Some(k));
        assert_eq!(
            checks(&rename),
            [
                (
                    "HGET k a".to_string(),
                    Expect::Bytes(b"v".to_vec(), FIELD_CHANGED)
                ),
                (
                    "HEXISTS k b".to_string(),
                    Expect::Int(0, "the field already exists")
                )
            ]
        );
        assert!(hash_rename(k, b"a", b"a", b"v").is_err());
        let rename = set_rename(k, b"a", b"b").unwrap();
        assert_eq!(texts(&rename), ["SADD k b", "SREM k a"]);
        assert_eq!(
            checks(&rename),
            [
                (
                    "SISMEMBER k a".to_string(),
                    Expect::Int(1, "the member no longer exists")
                ),
                (
                    "SISMEMBER k b".to_string(),
                    Expect::Int(0, "the member already exists")
                )
            ]
        );
        assert!(set_rename(k, b"a", b"a").is_err());
        let delete = list_delete_at(k, 3, b"old", b"__tabler:1");
        assert_eq!(
            texts(&delete),
            ["LSET k 3 __tabler:1", "LREM k 1 __tabler:1"]
        );
        let replace = list_replace(k, -1, b"old", b"new");
        assert_eq!(texts(&replace), ["LSET k -1 new"]);
        for (g, at) in [(&delete, "3"), (&replace, "-1")] {
            assert_eq!(checks(g).len(), 1);
            assert_eq!(checks(g)[0].0, format!("LINDEX k {at}"));
            assert!(matches!(&checks(g)[0].1, Expect::Bytes(old, _) if old == b"old"));
        }
        // Each in one MULTI … EXEC after its WATCH.
        let lines: Vec<String> = preview(&[rename.edit(0)])
            .into_iter()
            .map(|(_, l)| l)
            .collect();
        assert_eq!(lines, ["WATCH k", "MULTI", "SADD k b", "SREM k a", "EXEC"]);
    }

    #[test]
    fn container_edits() {
        let k = b"k".as_slice();
        assert_eq!(hash_set(k, b"f", b"v").text(), "HSET k f v");
        assert_eq!(list_set(k, -1, b"v").text(), "LSET k -1 v");
        assert_eq!(list_push(k, &[b"a".to_vec()], true).text(), "LPUSH k a");
        assert_eq!(
            list_insert(k, true, b"p", b"v").text(),
            "LINSERT k BEFORE p v"
        );
        assert_eq!(zset_add(k, "2", b"m").unwrap().text(), "ZADD k NX 2 m");
        assert_eq!(
            zset_score(k, "+inf", b"m").unwrap().text(),
            "ZADD k XX +inf m"
        );
        assert_eq!(zset_remove(k, b"m").text(), "ZREM k m");
        assert!(zset_score(k, "NaN", b"m").is_err());
        assert_eq!(
            stream_add(k, Some("1-1"), &[(b"f".to_vec(), b"v".to_vec())])
                .unwrap()
                .text(),
            "XADD k 1-1 f v"
        );
        assert!(stream_add(k, Some("1-x"), &[(b"f".to_vec(), b"v".to_vec())]).is_err());
        assert!(stream_add(k, None, &[]).is_err());
        assert_eq!(
            stream_delete(k, "1700000000000-0").unwrap().text(),
            "XDEL k 1700000000000-0"
        );
        assert_eq!(json_delete(k, "$.a").text(), "JSON.DEL k $.a");
        assert_eq!(
            copy_key(k, b"k2", &Gates::default()).unwrap().text(),
            "COPY k k2"
        );
        assert!(
            copy_key(
                k,
                b"k2",
                &Gates {
                    copy: false,
                    ..Gates::default()
                }
            )
            .is_err()
        );
        assert_eq!(flush_db(&Gates::default()).text(), "FLUSHDB ASYNC");
        assert_eq!(select_db(3).text(), "SELECT 3");
        assert_eq!(
            watch(&[b"a".as_slice(), b"b c".as_slice()]).text(),
            "WATCH a \"b c\""
        );
        assert_eq!(key_type(k).text(), "TYPE k");
        assert_eq!(exists(k).text(), "EXISTS k");
        assert_eq!(
            [multi(), exec(), discard(), unwatch()]
                .iter()
                .map(Command::text)
                .collect::<Vec<_>>(),
            ["MULTI", "EXEC", "DISCARD", "UNWATCH"]
        );
    }

    #[test]
    fn read_commands() {
        let k = b"user:1".as_slice();
        assert_eq!(scan("0", None, 0, None).text(), "SCAN 0");
        assert_eq!(
            scan("17", Some(b"user:*"), 200, Some("hash")).text(),
            "SCAN 17 MATCH user:* COUNT 200 TYPE hash"
        );
        assert_eq!(
            scan("0", Some(b"a b*"), 10, None).text(),
            "SCAN 0 MATCH \"a b*\" COUNT 10"
        );
        assert_eq!(pttl(k).text(), "PTTL user:1");
        assert_eq!(memory_usage(k).text(), "MEMORY USAGE user:1 SAMPLES 0");
        let sizes: Vec<String> = ["string", "list", "set", "zset", "hash", "stream"]
            .iter()
            .map(|t| size_of(t, k).unwrap().text())
            .collect();
        assert_eq!(
            sizes,
            [
                "STRLEN user:1",
                "LLEN user:1",
                "SCARD user:1",
                "ZCARD user:1",
                "HLEN user:1",
                "XLEN user:1"
            ]
        );
        assert!(size_of(TYPE_JSON, k).is_none());
        assert!(size_of("TSDB-TYPE", k).is_none());
        let previews: Vec<String> = ["string", "list", "set", "zset", "hash", "stream", TYPE_JSON]
            .iter()
            .map(|t| preview_of(t, k, 1024, 50).unwrap().text())
            .collect();
        assert_eq!(
            previews,
            [
                "GETRANGE user:1 0 1023",
                "LRANGE user:1 0 49",
                "SSCAN user:1 0 COUNT 50",
                "ZRANGE user:1 0 49 WITHSCORES",
                "HSCAN user:1 0 COUNT 50",
                "XREVRANGE user:1 + - COUNT 50",
                "JSON.GET user:1"
            ]
        );
        assert_eq!(
            preview_of("string", k, 0, 0).unwrap().text(),
            "GETRANGE user:1 0 0"
        );
        assert!(preview_of("MBbloom--", k, 1, 1).is_none());
        assert_eq!(
            hash_scan(k, "5", 10, true).text(),
            "HSCAN user:1 5 COUNT 10 NOVALUES"
        );
        assert_eq!(zset_scan(k, "0", 10).text(), "ZSCAN user:1 0 COUNT 10");
        assert_eq!(
            stream_range(k, "-", "+", 2).text(),
            "XRANGE user:1 - + COUNT 2"
        );
        assert_eq!(json_get(k, Some("$.a")).text(), "JSON.GET user:1 $.a");
        assert_eq!(get(k).text(), "GET user:1");
        assert_eq!(info(Some("keyspace")).text(), "INFO keyspace");
        assert_eq!(info(None).text(), "INFO");
        assert_eq!(config_get("databases").text(), "CONFIG GET databases");
        assert_eq!(dbsize().text(), "DBSIZE");
        assert_eq!(ping().text(), "PING");
        assert_eq!(command_table().text(), "COMMAND");
        assert_eq!(client_id().text(), "CLIENT ID");
        assert_eq!(client_kill(42).text(), "CLIENT KILL ID 42");
        assert_eq!(pttl(k).name(), "PTTL");
    }
}
