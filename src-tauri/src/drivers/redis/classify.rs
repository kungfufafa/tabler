//! Read or write: the console's classifier (read-only connections, Safe Mode). An allow-list,
//! default-deny: a command is a read only when it is in the static list below, or when the
//! server's `COMMAND INFO` flags it `readonly` without `admin`, `may_replicate`, `noscript` or
//! `blocking` (modules' read commands). Scripts (`EVAL*`, `FCALL*`, `SCRIPT`, `FUNCTION`), server
//! administration (`CONFIG`, `CLIENT`, `DEBUG`, `SHUTDOWN`, …), transactions, connection state
//! (`HELLO`, `AUTH`) and `KEYS` (it blocks the server; use SCAN) are writes whatever the server
//! says. A line that cannot be read is a write.

use std::collections::HashSet;

use super::cli::{command_name, split_console, tokenize};
use crate::sqlgen::classify::StatementClass;

/// Commands that are never reads, even when a server flags them `readonly`. A module command
/// that runs the user's code (RedisGears, triggers and functions, RedisAI scripts, DAGs and
/// models) is here like `EVAL`: its `readonly` flag describes the keys it declares, not what
/// the code does.
const NEVER_READ: &[&str] = &[
    "ACL",
    "AI.DAGEXECUTE",
    "AI.DAGEXECUTE_RO",
    "AI.DAGRUN",
    "AI.DAGRUN_RO",
    "AI.MODELEXECUTE",
    "AI.MODELRUN",
    "AI.SCRIPTEXECUTE",
    "AI.SCRIPTRUN",
    "AUTH",
    "BGREWRITEAOF",
    "BGSAVE",
    "CLIENT",
    "CLUSTER",
    "CONFIG",
    "DEBUG",
    "DISCARD",
    "EVAL",
    "EVALSHA",
    "EVALSHA_RO",
    "EVAL_RO",
    "EXEC",
    "FAILOVER",
    "FCALL",
    "FCALL_RO",
    "FUNCTION",
    "HELLO",
    "KEYS",
    "MIGRATE",
    "MODULE",
    "MONITOR",
    "MULTI",
    "PSUBSCRIBE",
    "PSYNC",
    "QUIT",
    "READONLY",
    "READWRITE",
    "REPLCONF",
    "REPLICAOF",
    "RESET",
    "SAVE",
    "SCRIPT",
    "SHUTDOWN",
    "SLAVEOF",
    "SSUBSCRIBE",
    "SUBSCRIBE",
    "SYNC",
    "TFCALL",
    "TFCALLASYNC",
    "TFUNCTION",
    "UNWATCH",
    "WAIT",
    "WAITAOF",
    "WATCH",
];

/// Name prefixes that are never reads: every RedisGears 1 command (`RG.PYEXECUTE`, `RG.TRIGGER`,
/// …) runs or manages the user's code.
const NEVER_READ_PREFIXES: &[&str] = &["RG."];

/// How a command of the static list reads.
#[derive(Clone, Copy)]
enum Rule {
    Read,
    /// A read with one of these subcommands (`""`: also without one).
    Sub(&'static [&'static str]),
    /// A read unless one of these words is among its arguments (`XREAD … BLOCK`).
    Unless(&'static [&'static str]),
}

/// The static allow-list: Redis's read-only commands (core and RedisJSON / TimeSeries / Bloom /
/// Search reads), plus the harmless server commands that carry no `readonly` flag.
const READS: &[(&str, Rule)] = &[
    // Strings and bitmaps.
    ("GET", Rule::Read),
    ("GETRANGE", Rule::Read),
    ("SUBSTR", Rule::Read),
    ("MGET", Rule::Read),
    ("STRLEN", Rule::Read),
    ("LCS", Rule::Read),
    ("GETBIT", Rule::Read),
    ("BITCOUNT", Rule::Read),
    ("BITPOS", Rule::Read),
    ("BITFIELD_RO", Rule::Read),
    // Keys.
    ("EXISTS", Rule::Read),
    ("TYPE", Rule::Read),
    ("TTL", Rule::Read),
    ("PTTL", Rule::Read),
    ("EXPIRETIME", Rule::Read),
    ("PEXPIRETIME", Rule::Read),
    ("RANDOMKEY", Rule::Read),
    ("SCAN", Rule::Read),
    ("DBSIZE", Rule::Read),
    ("DUMP", Rule::Read),
    ("TOUCH", Rule::Read),
    ("SORT_RO", Rule::Read),
    // A read without `STORE` whatever a server's flags say, so that a server flagging it
    // `readonly` cannot make the `STORE` form one.
    ("SORT", Rule::Unless(&["STORE"])),
    (
        "OBJECT",
        Rule::Sub(&["ENCODING", "FREQ", "IDLETIME", "REFCOUNT", "HELP"]),
    ),
    ("MEMORY", Rule::Sub(&["USAGE", "HELP"])),
    // Lists.
    ("LRANGE", Rule::Read),
    ("LLEN", Rule::Read),
    ("LINDEX", Rule::Read),
    ("LPOS", Rule::Read),
    // Sets.
    ("SMEMBERS", Rule::Read),
    ("SISMEMBER", Rule::Read),
    ("SMISMEMBER", Rule::Read),
    ("SCARD", Rule::Read),
    ("SRANDMEMBER", Rule::Read),
    ("SSCAN", Rule::Read),
    ("SINTER", Rule::Read),
    ("SUNION", Rule::Read),
    ("SDIFF", Rule::Read),
    ("SINTERCARD", Rule::Read),
    // Sorted sets.
    ("ZRANGE", Rule::Read),
    ("ZRANGEBYSCORE", Rule::Read),
    ("ZRANGEBYLEX", Rule::Read),
    ("ZREVRANGE", Rule::Read),
    ("ZREVRANGEBYSCORE", Rule::Read),
    ("ZREVRANGEBYLEX", Rule::Read),
    ("ZCARD", Rule::Read),
    ("ZCOUNT", Rule::Read),
    ("ZLEXCOUNT", Rule::Read),
    ("ZSCORE", Rule::Read),
    ("ZMSCORE", Rule::Read),
    ("ZRANK", Rule::Read),
    ("ZREVRANK", Rule::Read),
    ("ZSCAN", Rule::Read),
    ("ZRANDMEMBER", Rule::Read),
    ("ZDIFF", Rule::Read),
    ("ZINTER", Rule::Read),
    ("ZUNION", Rule::Read),
    ("ZINTERCARD", Rule::Read),
    // Hashes.
    ("HGET", Rule::Read),
    ("HMGET", Rule::Read),
    ("HGETALL", Rule::Read),
    ("HKEYS", Rule::Read),
    ("HVALS", Rule::Read),
    ("HLEN", Rule::Read),
    ("HEXISTS", Rule::Read),
    ("HSTRLEN", Rule::Read),
    ("HSCAN", Rule::Read),
    ("HRANDFIELD", Rule::Read),
    ("HTTL", Rule::Read),
    ("HPTTL", Rule::Read),
    ("HEXPIRETIME", Rule::Read),
    ("HPEXPIRETIME", Rule::Read),
    // Streams.
    ("XRANGE", Rule::Read),
    ("XREVRANGE", Rule::Read),
    ("XLEN", Rule::Read),
    ("XPENDING", Rule::Read),
    (
        "XINFO",
        Rule::Sub(&["STREAM", "GROUPS", "CONSUMERS", "HELP"]),
    ),
    ("XREAD", Rule::Unless(&["BLOCK"])),
    // Geo.
    ("GEOPOS", Rule::Read),
    ("GEODIST", Rule::Read),
    ("GEOHASH", Rule::Read),
    ("GEORADIUS_RO", Rule::Read),
    ("GEORADIUSBYMEMBER_RO", Rule::Read),
    ("GEOSEARCH", Rule::Read),
    // Like `SORT`: reads unless they store.
    ("GEORADIUS", Rule::Unless(&["STORE", "STOREDIST"])),
    ("GEORADIUSBYMEMBER", Rule::Unless(&["STORE", "STOREDIST"])),
    // Server and connection.
    ("PING", Rule::Read),
    ("ECHO", Rule::Read),
    ("TIME", Rule::Read),
    ("INFO", Rule::Read),
    ("LASTSAVE", Rule::Read),
    ("ROLE", Rule::Read),
    // Changes only which database the console's own connection reads, like `USE`.
    ("SELECT", Rule::Read),
    (
        "COMMAND",
        Rule::Sub(&[
            "",
            "COUNT",
            "DOCS",
            "INFO",
            "LIST",
            "GETKEYS",
            "GETKEYSANDFLAGS",
            "HELP",
        ]),
    ),
    (
        "PUBSUB",
        Rule::Sub(&[
            "CHANNELS",
            "NUMSUB",
            "NUMPAT",
            "SHARDCHANNELS",
            "SHARDNUMSUB",
            "HELP",
        ]),
    ),
    // RedisJSON.
    ("JSON.GET", Rule::Read),
    ("JSON.MGET", Rule::Read),
    ("JSON.TYPE", Rule::Read),
    ("JSON.STRLEN", Rule::Read),
    ("JSON.OBJLEN", Rule::Read),
    ("JSON.OBJKEYS", Rule::Read),
    ("JSON.ARRLEN", Rule::Read),
    ("JSON.ARRINDEX", Rule::Read),
    ("JSON.RESP", Rule::Read),
    // RedisTimeSeries.
    ("TS.GET", Rule::Read),
    ("TS.MGET", Rule::Read),
    ("TS.RANGE", Rule::Read),
    ("TS.REVRANGE", Rule::Read),
    ("TS.MRANGE", Rule::Read),
    ("TS.MREVRANGE", Rule::Read),
    ("TS.INFO", Rule::Read),
    ("TS.QUERYINDEX", Rule::Read),
    // RedisBloom.
    ("BF.EXISTS", Rule::Read),
    ("BF.MEXISTS", Rule::Read),
    ("BF.INFO", Rule::Read),
    ("BF.CARD", Rule::Read),
    ("CF.EXISTS", Rule::Read),
    ("CF.MEXISTS", Rule::Read),
    ("CF.COUNT", Rule::Read),
    ("CF.INFO", Rule::Read),
    // RediSearch.
    ("FT.SEARCH", Rule::Read),
    ("FT.AGGREGATE", Rule::Read),
    ("FT.INFO", Rule::Read),
    ("FT.EXPLAIN", Rule::Read),
    ("FT._LIST", Rule::Read),
];

/// The flags that keep a `readonly` command out of the allow-list.
const EXCLUDING_FLAGS: &[&str] = &["admin", "may_replicate", "noscript", "blocking"];

/// The read commands a server declares in `COMMAND INFO` (names lower-case, subcommands as
/// `object|encoding`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerReads {
    names: HashSet<String>,
    /// Commands that list subcommands: only a listed read subcommand of theirs reads.
    containers: HashSet<String>,
}

impl ServerReads {
    /// Reads a `COMMAND` / `COMMAND INFO` reply (RESP2 or RESP3). Entries it cannot read are left
    /// out, which keeps their commands writes.
    pub fn from_reply(reply: &::redis::Value) -> ServerReads {
        let mut reads = ServerReads::default();
        if let Some(entries) = items(reply) {
            for entry in entries {
                collect(entry, &mut reads);
            }
        }
        reads
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// Whether `argv` (its name lower-case in `lower`) reads by the server's flags.
    fn reads(&self, lower: &str, argv: &[Vec<u8>]) -> bool {
        // Subcommands are kept as `container|sub`; a typed name with a `|` is no command (the
        // server refuses it), so it never matches one of those entries.
        if lower.contains('|') {
            return false;
        }
        // A module's configuration and debugging commands, and internal `_…` ones, change the
        // server whatever flags the module gave them.
        if lower.starts_with('_')
            || lower
                .rsplit_once('.')
                .is_some_and(|(_, c)| matches!(c, "config" | "debug"))
        {
            return false;
        }
        let sub = argv
            .get(1)
            .and_then(|a| std::str::from_utf8(a).ok())
            .map(str::to_ascii_lowercase);
        if self.containers.contains(lower) {
            return sub.is_some_and(|s| self.names.contains(&format!("{lower}|{s}")));
        }
        // Without subcommand entries (Redis 6, modules) the command's own flags decide.
        self.names.contains(lower)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

fn items(v: &::redis::Value) -> Option<&[::redis::Value]> {
    match v {
        ::redis::Value::Array(a) | ::redis::Value::Set(a) => Some(a),
        ::redis::Value::Attribute { data, .. } => items(data),
        _ => None,
    }
}

fn text(v: &::redis::Value) -> Option<String> {
    match v {
        ::redis::Value::BulkString(b) => std::str::from_utf8(b).ok().map(str::to_string),
        ::redis::Value::SimpleString(s) => Some(s.clone()),
        ::redis::Value::VerbatimString { text, .. } => Some(text.clone()),
        ::redis::Value::Okay => Some("OK".into()),
        _ => None,
    }
}

/// One entry: `[name, arity, flags, first, last, step, acl, tips, key specs, subcommands]`.
fn collect(entry: &::redis::Value, reads: &mut ServerReads) {
    let Some(fields) = items(entry) else { return };
    let Some(name) = fields.first().and_then(text) else {
        return;
    };
    let name = name.to_ascii_lowercase();
    let flags: Vec<String> = fields
        .get(2)
        .and_then(items)
        .map(|f| {
            f.iter()
                .filter_map(text)
                .map(|s| s.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let read = flags.iter().any(|f| f == "readonly")
        && !flags.iter().any(|f| EXCLUDING_FLAGS.contains(&f.as_str()));
    if let Some(subs) = fields.get(9).and_then(items).filter(|s| !s.is_empty()) {
        reads.containers.insert(name.clone());
        for sub in subs {
            collect(sub, reads);
        }
    }
    if read {
        reads.names.insert(name);
    }
}

/// The class of one command's argv.
pub fn classify_argv(argv: &[Vec<u8>], server: Option<&ServerReads>) -> StatementClass {
    let Some(name) = command_name(argv) else {
        return StatementClass::Write;
    };
    if NEVER_READ.contains(&name.as_str())
        || NEVER_READ_PREFIXES.iter().any(|p| name.starts_with(p))
    {
        return StatementClass::Write;
    }
    let arg_upper = |i: usize| {
        argv.get(i)
            .and_then(|a| std::str::from_utf8(a).ok())
            .map(str::to_ascii_uppercase)
    };
    if let Some((_, rule)) = READS.iter().find(|(n, _)| *n == name) {
        let read = match rule {
            Rule::Read => true,
            Rule::Sub(subs) => match arg_upper(1) {
                Some(sub) => subs.contains(&sub.as_str()),
                None => argv.len() == 1 && subs.contains(&""),
            },
            Rule::Unless(words) => !(1..argv.len())
                .filter_map(arg_upper)
                .any(|a| words.contains(&a.as_str())),
        };
        return if read {
            StatementClass::Read
        } else {
            StatementClass::Write
        };
    }
    match server {
        Some(server) if server.reads(&name.to_ascii_lowercase(), argv) => StatementClass::Read,
        _ => StatementClass::Write,
    }
}

/// The class of one command line; a line that cannot be read is a write.
pub fn classify_line(line: &str, server: Option<&ServerReads>) -> StatementClass {
    match tokenize(line) {
        Ok(argv) => classify_argv(&argv, server),
        Err(_) => StatementClass::Write,
    }
}

/// The class of console text: a read when it holds commands and all of them read.
pub fn classify_script(text: &str, server: Option<&ServerReads>) -> StatementClass {
    let commands = split_console(text);
    if commands.is_empty() {
        return StatementClass::Write;
    }
    if commands
        .iter()
        .all(|c| classify_line(&c.sql, server) == StatementClass::Read)
    {
        StatementClass::Read
    } else {
        StatementClass::Write
    }
}

/// Commands the console refuses to run: they block the connection or turn it into a stream that
/// never answers a command again (pub/sub, MONITOR), or change its protocol and identity.
pub fn refused_in_console(argv: &[Vec<u8>]) -> Option<&'static str> {
    let name = command_name(argv)?;
    let blocking = matches!(
        name.as_str(),
        "BLPOP" | "BRPOP" | "BLMOVE" | "BRPOPLPUSH" | "BLMPOP" | "BZPOPMIN" | "BZPOPMAX" | "BZMPOP"
    );
    let block_arg = matches!(name.as_str(), "XREAD" | "XREADGROUP")
        && argv[1..].iter().any(|a| a.eq_ignore_ascii_case(b"BLOCK"));
    if blocking || block_arg || matches!(name.as_str(), "WAIT" | "WAITAOF") {
        return Some(
            "blocking commands are not run in the console: they would hold the connection",
        );
    }
    match name.as_str() {
        "SUBSCRIBE" | "PSUBSCRIBE" | "SSUBSCRIBE" | "MONITOR" | "SYNC" | "PSYNC" => {
            Some("pub/sub, MONITOR and replication streams are not supported in the console")
        }
        "HELLO" | "AUTH" | "QUIT" | "RESET" => Some(CHANGES_CONNECTION),
        "CLIENT" if is_client_state_change(argv) => Some(CHANGES_CONNECTION),
        _ => None,
    }
}

const CHANGES_CONNECTION: &str =
    "this command changes the connection itself; edit the connection instead";

/// `CLIENT REPLY`, `CLIENT TRACKING`, `CLIENT CACHING` change what the connection answers.
fn is_client_state_change(argv: &[Vec<u8>]) -> bool {
    argv.get(1).is_some_and(|sub| {
        [b"REPLY".as_slice(), b"TRACKING", b"CACHING"]
            .iter()
            .any(|s| sub.eq_ignore_ascii_case(s))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::redis::parse_redis_value;
    use StatementClass::{Read, Write};

    fn c(line: &str) -> StatementClass {
        classify_line(line, None)
    }

    #[test]
    fn reads_of_every_type() {
        for line in [
            "GET k",
            "get k",
            "MGET a b",
            "GETRANGE k 0 10",
            "SCAN 0 MATCH user:* COUNT 100 TYPE hash",
            "TYPE k",
            "PTTL k",
            "HGETALL h",
            "HSCAN h 0 COUNT 10 NOVALUES",
            "LRANGE l 0 -1",
            "SMEMBERS s",
            "ZRANGE z 0 -1 WITHSCORES",
            "XRANGE s - + COUNT 10",
            "XREVRANGE s + - COUNT 10",
            "XINFO STREAM s",
            "XREAD COUNT 10 STREAMS s 0",
            "OBJECT ENCODING k",
            "MEMORY USAGE k SAMPLES 0",
            "INFO keyspace",
            "PING",
            "DBSIZE",
            "SELECT 3",
            "COMMAND",
            "COMMAND DOCS get",
            "PUBSUB CHANNELS",
            "JSON.GET doc $",
            "SORT l",
            "SORT l BY w_* GET # LIMIT 0 10 ALPHA",
            "GEORADIUS g 0 0 1 km WITHDIST",
            "GEORADIUSBYMEMBER g m 1 km ASC",
            "  \"GET\"   'k'",
        ] {
            assert_eq!(c(line), Read, "{line}");
        }
    }

    #[test]
    fn writes_and_unknowns() {
        for line in [
            "SET k v",
            "DEL k",
            "UNLINK k",
            "HSET h f v",
            "LPUSH l v",
            "ZADD z 1 m",
            "XADD s * f v",
            "EXPIRE k 10",
            "RENAME a b",
            "FLUSHALL",
            "FLUSHDB ASYNC",
            "KEYS *",
            "EVAL \"return 1\" 0",
            "EVAL_RO \"return 1\" 0",
            "EVALSHA abc 0",
            "FCALL f 0",
            "FCALL_RO f 0",
            "SCRIPT EXISTS abc",
            "FUNCTION LIST",
            "CONFIG GET maxmemory",
            "CONFIG SET maxmemory 1",
            "CLIENT LIST",
            "CLIENT KILL ID 1",
            "SHUTDOWN NOSAVE",
            "DEBUG SLEEP 1",
            "MULTI",
            "EXEC",
            "WATCH k",
            "HELLO 3",
            "AUTH pass",
            "XREAD BLOCK 0 STREAMS s $",
            "XREADGROUP GROUP g c STREAMS s >",
            "BLPOP l 0",
            "OBJECT",
            "OBJECT FREEZE k",
            "MEMORY PURGE",
            "XINFO",
            "SORT l STORE x",
            "GEORADIUS g 0 0 1 km STORE x",
            "sort l store x",
            "GEORADIUS g 0 0 1 km STOREDIST x",
            "GEORADIUSBYMEMBER g m 1 km STORE x",
            "TFCALL lib.f 0",
            "TFCALLASYNC lib.f 0",
            "TFUNCTION LIST",
            "RG.PYEXECUTE \"GB().run()\"",
            "rg.pystats",
            "AI.SCRIPTEXECUTE s f KEYS 1 k",
            "AI.DAGEXECUTE_RO KEYS 1 k |> AI.TENSORGET k",
            "OBJECT|ENCODING k",
            "PFCOUNT h",
            "GETEX k PERSIST",
            "GETDEL k",
            "SPOP s",
            "SUBSCRIBE c",
            "MONITOR",
            "NOSUCHCOMMAND x",
            "",
            "   ",
            "GET \"unbalanced",
            "\"\" k",
            "SET\u{a0}k v",
        ] {
            assert_eq!(c(line), Write, "{line:?}");
        }
        assert_eq!(classify_argv(&[b"\xffGET".to_vec()], None), Write);
        assert_eq!(classify_argv(&[], None), Write);
    }

    #[test]
    fn scripts_are_reads_only_when_every_command_reads() {
        assert_eq!(classify_script("GET a\nTYPE b; PTTL b", None), Read);
        assert_eq!(classify_script("GET a\nDEL b", None), Write);
        assert_eq!(classify_script("GET a; SET \"x;y\" 1", None), Write);
        // A `;` inside quotes does not hide a second command, and a write after it counts.
        assert_eq!(classify_script("GET \"a;b\"; FLUSHALL", None), Write);
        assert_eq!(classify_script("", None), Write);
        assert_eq!(classify_script(" ;\n", None), Write);
    }

    /// `COMMAND INFO` entries as Redis 7 sends them over RESP2.
    fn command_info() -> ::redis::Value {
        let entry = |name: &str, flags: &[&str], subs: &str| {
            let mut s = format!(
                "*10\r\n${}\r\n{name}\r\n:2\r\n*{}\r\n",
                name.len(),
                flags.len()
            );
            for f in flags {
                s.push_str(&format!("+{f}\r\n"));
            }
            s.push_str(":1\r\n:1\r\n:1\r\n*0\r\n*0\r\n*0\r\n");
            s.push_str(subs);
            s
        };
        let object_subs = format!(
            "*2\r\n{}{}",
            entry("object|encoding", &["readonly"], "*0\r\n"),
            entry("object|freeze", &["write"], "*0\r\n")
        );
        // A container that is itself flagged `readonly` but lists a write subcommand.
        let mod_subs = format!(
            "*2\r\n{}{}",
            entry("mod.cmd|get", &["readonly"], "*0\r\n"),
            entry("mod.cmd|set", &["write"], "*0\r\n")
        );
        let all = [
            entry("ts.range", &["readonly"], "*0\r\n"),
            entry("ts.add", &["write", "denyoom"], "*0\r\n"),
            entry("pfcount", &["readonly", "may_replicate"], "*0\r\n"),
            entry("blpop2", &["readonly", "blocking"], "*0\r\n"),
            entry("myadmin", &["readonly", "admin"], "*0\r\n"),
            entry("noscr", &["readonly", "noscript"], "*0\r\n"),
            entry("keys", &["readonly"], "*0\r\n"),
            entry("mod.read", &["READONLY", "fast"], "*0\r\n"),
            entry("object", &[], &object_subs),
            entry("mod.cmd", &["readonly"], &mod_subs),
            entry("ft.config", &["readonly"], "*0\r\n"),
            entry("json.debug", &["readonly"], "*0\r\n"),
            entry("_ft.search", &["readonly"], "*0\r\n"),
            // Forks or modules that flag these `readonly`.
            entry("sort", &["readonly"], "*0\r\n"),
            entry("georadius", &["readonly"], "*0\r\n"),
            entry("rg.pyexecute", &["readonly"], "*0\r\n"),
            entry("tfcall", &["readonly"], "*0\r\n"),
            entry("ai.dagexecute_ro", &["readonly"], "*0\r\n"),
        ];
        let text = format!("*{}\r\n{}", all.len(), all.concat());
        parse_redis_value(text.as_bytes()).unwrap()
    }

    #[test]
    fn server_flags_extend_the_allow_list() {
        let reads = ServerReads::from_reply(&command_info());
        assert!(reads.contains("ts.range"));
        assert!(reads.contains("mod.read"));
        assert!(reads.contains("object|encoding"));
        for name in [
            "ts.add",
            "pfcount",
            "blpop2",
            "myadmin",
            "noscr",
            "object",
            "object|freeze",
        ] {
            assert!(!reads.contains(name), "{name}");
        }
        let s = Some(&reads);
        assert_eq!(classify_line("MOD.READ x", s), Read);
        assert_eq!(classify_line("BLPOP2 x", s), Write);
        assert_eq!(classify_line("TS.ADD x * 1", s), Write);
        // The never-read list wins over the server's flags.
        assert_eq!(classify_line("KEYS *", s), Write);
        // Static subcommand rules still apply to listed commands.
        assert_eq!(classify_line("OBJECT FREEZE k", s), Write);
        assert_eq!(classify_line("UNKNOWN x", s), Write);
        assert!(ServerReads::from_reply(&::redis::Value::Nil).is_empty());
        assert!(
            ServerReads::from_reply(&::redis::Value::Array(vec![::redis::Value::Int(1)]))
                .is_empty()
        );
        // `keys` is flagged readonly; the never-read list keeps it a write.
        assert_eq!(reads.len(), 14);
    }

    #[test]
    fn containers_read_only_through_listed_read_subcommands() {
        let reads = ServerReads::from_reply(&command_info());
        let s = Some(&reads);
        assert_eq!(classify_line("MOD.CMD GET k", s), Read);
        assert_eq!(classify_line("mod.cmd get k", s), Read);
        // The container's own `readonly` flag does not cover its other subcommands.
        assert_eq!(classify_line("MOD.CMD SET k v", s), Write);
        assert_eq!(classify_line("MOD.CMD OTHER k", s), Write);
        assert_eq!(classify_line("MOD.CMD", s), Write);
        assert_eq!(
            classify_argv(&[b"MOD.CMD".to_vec(), b"\xff".to_vec()], s),
            Write
        );
        // A module's configuration, debugging and internal commands, whatever their flags.
        assert_eq!(classify_line("FT.CONFIG SET TIMEOUT 0", s), Write);
        assert_eq!(classify_line("FT.CONFIG GET *", s), Write);
        assert_eq!(classify_line("JSON.DEBUG MEMORY k", s), Write);
        assert_eq!(classify_line("_FT.SEARCH idx *", s), Write);
    }

    #[test]
    fn flags_never_promote_pipe_names_stores_or_user_code() {
        let reads = ServerReads::from_reply(&command_info());
        let s = Some(&reads);
        // Subcommands are kept as `container|sub`; typed as a name they never match.
        assert!(reads.contains("object|encoding"));
        assert_eq!(classify_line("OBJECT ENCODING k", s), Read);
        assert_eq!(classify_line("object|encoding k", s), Write);
        assert_eq!(classify_line("OBJECT|ENCODING k", s), Write);
        assert_eq!(classify_line("mod.cmd|get k", s), Write);
        // The static rules win over the flags: the STORE forms stay writes.
        assert_eq!(classify_line("SORT l", s), Read);
        assert_eq!(classify_line("SORT l STORE x", s), Write);
        assert_eq!(classify_line("GEORADIUS g 0 0 1 km STOREDIST x", s), Write);
        // Commands that run the user's code, whatever their flags.
        assert_eq!(classify_line("RG.PYEXECUTE x", s), Write);
        assert_eq!(classify_line("TFCALL lib.f 0", s), Write);
        assert_eq!(classify_line("AI.DAGEXECUTE_RO KEYS 1 k", s), Write);
    }

    #[test]
    fn server_flags_over_resp3_sets() {
        let text = "*1\r\n*10\r\n$7\r\nmod.get\r\n:2\r\n~1\r\n+readonly\r\n:1\r\n:1\r\n:1\r\n~0\r\n*0\r\n*0\r\n*0\r\n";
        let reads = ServerReads::from_reply(&parse_redis_value(text.as_bytes()).unwrap());
        assert!(reads.contains("mod.get"));
    }

    #[test]
    fn console_refusals() {
        let refused = |l: &str| refused_in_console(&tokenize(l).unwrap()).is_some();
        for l in [
            "BLPOP l 0",
            "BZPOPMIN z 1",
            "XREAD BLOCK 100 STREAMS s $",
            "xreadgroup group g c block 0 streams s >",
            "SUBSCRIBE ch",
            "MONITOR",
            "HELLO 2",
            "AUTH x",
            "CLIENT REPLY OFF",
            "WAIT 1 0",
        ] {
            assert!(refused(l), "{l}");
        }
        for l in [
            "GET k",
            "XREAD STREAMS s 0",
            "CLIENT LIST",
            "BITCOUNT k",
            "SET k v",
        ] {
            assert!(!refused(l), "{l}");
        }
        assert!(refused_in_console(&[]).is_none());
    }
}
