//! Read or write: the console's classifier (read-only connections, Safe Mode). An allow-list,
//! default-deny: a command is a read only when it is in the static list below, or when it is a
//! core command the server's `COMMAND INFO` flags `readonly` without `admin`, `may_replicate`,
//! `noscript` or `blocking` (core reads newer than the list). A module's command reads only
//! through the static list: modules flag commands `readonly` that change data. Scripts (`EVAL*`,
//! `FCALL*`, `SCRIPT`, `FUNCTION`), server administration (`CONFIG`, `CLIENT`, `DEBUG`,
//! `SHUTDOWN`, …), transactions, connection state (`HELLO`, `AUTH`) and `KEYS` (it blocks the
//! server; use SCAN) are writes whatever the server says. A line that cannot be read is a write.

use std::collections::HashSet;

use super::cli::{command_name, tokenize};
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
    // RedisBloom flags it `readonly`, yet it rewrites the filter and replicates.
    "CF.COMPACT",
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
    // RediSearch before 8 (Redis Stack up to 7.4) flags its alias and dictionary edits
    // `readonly`.
    "FT.ALIASADD",
    "FT.ALIASDEL",
    "FT.ALIASUPDATE",
    "FT.DICTADD",
    "FT.DICTDEL",
    "FUNCTION",
    "HELLO",
    "KEYS",
    "MIGRATE",
    "MODULE",
    "MONITOR",
    "MULTI",
    // Stores the cardinality it computes in the key (before 6.2 it is flagged plain readonly).
    "PFCOUNT",
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
/// Search / Graph reads), plus the harmless server commands that carry no `readonly` flag. A
/// module's read must be listed here: the server's flags never make a module's command a read.
const READS: &[(&str, Rule)] = &[
    // Strings and bitmaps.
    ("GET", Rule::Read),
    ("GETRANGE", Rule::Read),
    ("SUBSTR", Rule::Read),
    ("MGET", Rule::Read),
    ("STRLEN", Rule::Read),
    ("LCS", Rule::Read),
    // `LCS` before 7.0.
    ("STRALGO", Rule::Sub(&["LCS"])),
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
    ("LOLWUT", Rule::Read),
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
    ("BF.SCANDUMP", Rule::Read),
    ("CF.EXISTS", Rule::Read),
    ("CF.MEXISTS", Rule::Read),
    ("CF.COUNT", Rule::Read),
    ("CF.INFO", Rule::Read),
    ("CF.SCANDUMP", Rule::Read),
    ("CMS.QUERY", Rule::Read),
    ("CMS.INFO", Rule::Read),
    ("TOPK.QUERY", Rule::Read),
    ("TOPK.COUNT", Rule::Read),
    ("TOPK.LIST", Rule::Read),
    ("TOPK.INFO", Rule::Read),
    ("TDIGEST.MIN", Rule::Read),
    ("TDIGEST.MAX", Rule::Read),
    ("TDIGEST.QUANTILE", Rule::Read),
    ("TDIGEST.CDF", Rule::Read),
    ("TDIGEST.RANK", Rule::Read),
    ("TDIGEST.REVRANK", Rule::Read),
    ("TDIGEST.BYRANK", Rule::Read),
    ("TDIGEST.BYREVRANK", Rule::Read),
    ("TDIGEST.TRIMMED_MEAN", Rule::Read),
    ("TDIGEST.INFO", Rule::Read),
    // RediSearch.
    ("FT.SEARCH", Rule::Read),
    ("FT.AGGREGATE", Rule::Read),
    ("FT.PROFILE", Rule::Read),
    ("FT.INFO", Rule::Read),
    ("FT.EXPLAIN", Rule::Read),
    ("FT.EXPLAINCLI", Rule::Read),
    ("FT.SPELLCHECK", Rule::Read),
    ("FT.TAGVALS", Rule::Read),
    ("FT.SUGGET", Rule::Read),
    ("FT.SUGLEN", Rule::Read),
    ("FT.SYNDUMP", Rule::Read),
    ("FT.DICTDUMP", Rule::Read),
    ("FT._LIST", Rule::Read),
    // Reads an `FT.AGGREGATE … WITHCURSOR` cursor; `DEL` and `GC` drop cursors.
    ("FT.CURSOR", Rule::Sub(&["READ"])),
    // RedisGraph / FalkorDB: the slow log, unless it is reset.
    ("GRAPH.SLOWLOG", Rule::Unless(&["RESET"])),
];

/// The flags that keep a `readonly` command out of the allow-list.
const EXCLUDING_FLAGS: &[&str] = &["admin", "may_replicate", "noscript", "blocking"];

/// The core read commands a server declares in `COMMAND INFO` (names lower-case, subcommands as
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
                collect(entry, false, &mut reads);
            }
        }
        reads
    }

    /// Whether `argv` (its name lower-case in `lower`) reads by the server's flags.
    fn reads(&self, lower: &str, argv: &[Vec<u8>]) -> bool {
        // Subcommands are kept as `container|sub`; a typed name with a `|` is no command (the
        // server refuses it), so it never matches one of those entries.
        if lower.contains('|') {
            return false;
        }
        if self.containers.contains(lower) {
            let sub = argv
                .get(1)
                .and_then(|a| std::str::from_utf8(a).ok())
                .map(str::to_ascii_lowercase);
            return sub.is_some_and(|s| self.names.contains(&format!("{lower}|{s}")));
        }
        self.names.contains(lower)
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

/// One entry: `[name, arity, flags, first, last, step, acl, tips, key specs, subcommands]`;
/// `in_module`: a subcommand of a module's command.
fn collect(entry: &::redis::Value, in_module: bool, reads: &mut ServerReads) {
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
    let flag = |f: &str| flags.iter().any(|g| g == f);
    // Modules flag commands `readonly` that change data (RediSearch before 8 its alias and
    // dictionary edits, RedisBloom `CF.COMPACT`), so the flags only add core commands. A module's
    // command is dotted (`FT.ALIASUPDATE`), internal (`_FT.X`) or, from Redis 7 on, flagged
    // `module`. Before 7 (entries of fewer than 10 fields) nothing marks an undotted module
    // command, so those entries add nothing: the static list covers every core read of Redis 6.
    let untrusted = in_module
        || fields.len() < 10
        || flag("module")
        || name.contains('.')
        || name.starts_with('_');
    let read = !untrusted && flag("readonly") && !EXCLUDING_FLAGS.iter().any(|f| flag(f));
    if let Some(subs) = fields.get(9).and_then(items).filter(|s| !s.is_empty()) {
        reads.containers.insert(name.clone());
        for sub in subs {
            collect(sub, untrusted, reads);
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
            Rule::Unless(words) => !argv[1..]
                .iter()
                .any(|a| words.iter().any(|w| is_option(a, w))),
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

/// Whether an argument selects an option word the way the server reads it: Redis compares
/// options with `strcasecmp`, which stops at the first NUL, so `"STORE\x00junk"` is STORE too.
fn is_option(arg: &[u8], word: &str) -> bool {
    let end = arg.iter().position(|&b| b == 0).unwrap_or(arg.len());
    arg.get(..end)
        .is_some_and(|a| a.eq_ignore_ascii_case(word.as_bytes()))
}

/// The class of one command line; a line that cannot be read is a write.
pub fn classify_line(line: &str, server: Option<&ServerReads>) -> StatementClass {
    match tokenize(line) {
        Ok(argv) => classify_argv(&argv, server),
        Err(_) => StatementClass::Write,
    }
}

/// Commands the console refuses to run: they block the connection or turn it into a stream that
/// never answers a command again (pub/sub, MONITOR), or change its protocol and identity.
pub fn refused_in_console(argv: &[Vec<u8>]) -> Option<&'static str> {
    // Redis looks names up with `strcasecmp`, which stops at a NUL, so `"CLIENT\x00a"` can run
    // as CLIENT and would slip past every exact-name check here and in the bootstrap check.
    if argv.first().is_some_and(|a| a.contains(&0)) {
        return Some("a command name cannot contain a NUL byte");
    }
    let name = command_name(argv)?;
    let blocking = matches!(
        name.as_str(),
        "BLPOP" | "BRPOP" | "BLMOVE" | "BRPOPLPUSH" | "BLMPOP" | "BZPOPMIN" | "BZPOPMAX" | "BZMPOP"
    );
    let block_arg = matches!(name.as_str(), "XREAD" | "XREADGROUP")
        && argv[1..].iter().any(|a| is_option(a, "BLOCK"));
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
/// Redis 6 matches CLIENT subcommands with `strcasecmp` too, so `"REPLY\x00"` is REPLY.
fn is_client_state_change(argv: &[Vec<u8>]) -> bool {
    argv.get(1).is_some_and(|sub| {
        ["REPLY", "TRACKING", "CACHING"]
            .iter()
            .any(|s| is_option(sub, s))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::redis::cli::split_console;
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
            "STRALGO LCS KEYS a b",
            "LOLWUT VERSION 5",
            "FT.SPELLCHECK idx helo",
            "FT.CURSOR READ idx 1 COUNT 10",
            "ft.cursor read idx 1",
            "CMS.QUERY c a b",
            "TDIGEST.QUANTILE t 0.5",
            "GRAPH.SLOWLOG g",
            // Only the part before a NUL is the option word.
            "SORT l GET \"#\\x00STORE\"",
            "SORT l BY \"STORES\"",
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
            "STRALGO",
            "FT.CURSOR DEL idx 1",
            "FT.CURSOR GC idx 0",
            "FT.CURSOR",
            "GRAPH.SLOWLOG g RESET",
            "graph.slowlog g reset",
            // strcasecmp stops at a NUL: these are STORE, STOREDIST, RESET and BLOCK.
            "SORT list:1 \"STORE\\x00\" dst",
            "SORT src \"store\\x00junk\" dst",
            "GEORADIUS g 0 0 1 km \"STORE\\x00\" dst",
            "GEORADIUSBYMEMBER g m 1 km \"STOREDIST\\x00\" dst",
            "GRAPH.SLOWLOG g \"RESET\\x00\"",
            "XREAD \"BLOCK\\x00\" 0 STREAMS s $",
            "GRAPH.QUERY g \"MATCH (n) DELETE n\"",
            "CF.COMPACT cf",
            "FT.ALIASUPDATE alias idx",
            "FT.DICTADD dict word",
            "FT.SUGADD sug hello 1",
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

    /// Console text the way Safe Mode reads it: split like the console, then each command.
    fn classes(text: &str) -> Vec<StatementClass> {
        split_console(text)
            .iter()
            .map(|c| classify_line(&c.sql, None))
            .collect()
    }

    #[test]
    fn console_text_is_classified_command_by_command() {
        assert_eq!(classes("GET a\nTYPE b; PTTL b"), [Read, Read, Read]);
        assert_eq!(classes("GET a\nDEL b"), [Read, Write]);
        assert_eq!(classes("GET a; SET \"x;y\" 1"), [Read, Write]);
        // A `;` inside quotes does not hide a second command, and a write after it counts.
        assert_eq!(classes("GET \"a;b\"; FLUSHALL"), [Read, Write]);
        assert!(classes("").is_empty());
        assert!(classes(" ;\n").is_empty());
    }

    const NO_SUBS: &str = "*0\r\n";

    /// One `COMMAND INFO` entry as Redis 7 sends it over RESP2 (ten fields).
    fn entry(name: &str, flags: &[&str], subs: &str) -> String {
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
    }

    fn reads_of(entries: &[String]) -> ServerReads {
        let text = format!("*{}\r\n{}", entries.len(), entries.concat());
        ServerReads::from_reply(&parse_redis_value(text.as_bytes()).unwrap())
    }

    /// A Redis 7 `COMMAND` reply: core commands, newer core ones the static list lacks, and
    /// modules' commands.
    fn command_info() -> ServerReads {
        let object_subs = format!(
            "*2\r\n{}{}",
            entry("object|encoding", &["readonly"], NO_SUBS),
            entry("object|freeze", &["write"], NO_SUBS)
        );
        // A newer core container, itself flagged `readonly`, that lists a write subcommand.
        let newcmd_subs = format!(
            "*2\r\n{}{}",
            entry("newcmd|get", &["readonly"], NO_SUBS),
            entry("newcmd|set", &["write"], NO_SUBS)
        );
        // A module's containers: their subcommands are the module's too, whatever their flags.
        let modcmd_subs = format!("*1\r\n{}", entry("modcmd|get", &["readonly"], NO_SUBS));
        let dotted_subs = format!("*1\r\n{}", entry("mod.cmd|get", &["readonly"], NO_SUBS));
        reads_of(&[
            entry("newread", &["READONLY", "fast"], NO_SUBS),
            entry("newwrite", &["write", "denyoom"], NO_SUBS),
            entry("pfcount", &["readonly", "may_replicate"], NO_SUBS),
            entry("blpop2", &["readonly", "blocking"], NO_SUBS),
            entry("myadmin", &["readonly", "admin"], NO_SUBS),
            entry("noscr", &["readonly", "noscript"], NO_SUBS),
            entry("keys", &["readonly"], NO_SUBS),
            entry("object", &[], &object_subs),
            entry("newcmd", &["readonly"], &newcmd_subs),
            entry("modcmd", &["readonly", "module"], &modcmd_subs),
            entry("modread", &["readonly", "fast", "module"], NO_SUBS),
            entry("mod.read", &["readonly", "fast"], NO_SUBS),
            entry("mod.cmd", &["readonly"], &dotted_subs),
            entry("ts.range", &["readonly"], NO_SUBS),
            entry("ft.config", &["readonly"], NO_SUBS),
            entry("_ft.search", &["readonly"], NO_SUBS),
            entry("_internal", &["readonly"], NO_SUBS),
            // Forks or modules that flag these `readonly`.
            entry("sort", &["readonly"], NO_SUBS),
            entry("georadius", &["readonly"], NO_SUBS),
            entry("tfcall", &["readonly"], NO_SUBS),
        ])
    }

    #[test]
    fn server_flags_add_newer_core_reads() {
        let reads = command_info();
        // `keys`, `sort`, `georadius` and `tfcall` are kept; the static lists decide them.
        let mut names: Vec<&str> = reads.names.iter().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "georadius",
                "keys",
                "newcmd",
                "newcmd|get",
                "newread",
                "object|encoding",
                "sort",
                "tfcall"
            ]
        );
        let s = Some(&reads);
        assert_eq!(classify_line("NEWREAD x", s), Read);
        assert_eq!(classify_line("newread x", s), Read);
        for line in [
            "NEWWRITE x",
            "PFCOUNT h",
            "BLPOP2 x",
            "MYADMIN",
            "NOSCR",
            "UNKNOWN x",
        ] {
            assert_eq!(classify_line(line, s), Write, "{line}");
        }
        // The never-read list wins over the server's flags.
        assert_eq!(classify_line("KEYS *", s), Write);
        // Static subcommand rules still apply to listed commands.
        assert_eq!(classify_line("OBJECT FREEZE k", s), Write);
        assert!(ServerReads::from_reply(&::redis::Value::Nil).is_empty());
        assert!(
            ServerReads::from_reply(&::redis::Value::Array(vec![::redis::Value::Int(1)]))
                .is_empty()
        );
    }

    #[test]
    fn containers_read_only_through_listed_read_subcommands() {
        let reads = command_info();
        let s = Some(&reads);
        assert_eq!(classify_line("NEWCMD GET k", s), Read);
        assert_eq!(classify_line("newcmd get k", s), Read);
        // The container's own `readonly` flag does not cover its other subcommands.
        assert_eq!(classify_line("NEWCMD SET k v", s), Write);
        assert_eq!(classify_line("NEWCMD OTHER k", s), Write);
        assert_eq!(classify_line("NEWCMD", s), Write);
        assert_eq!(
            classify_argv(&[b"NEWCMD".to_vec(), b"\xff".to_vec()], s),
            Write
        );
    }

    #[test]
    fn module_commands_never_read_by_their_flags() {
        let reads = command_info();
        let s = Some(&reads);
        for line in [
            // Flagged `module` (Redis 7), and a subcommand of such a command.
            "MODREAD x",
            "MODCMD GET k",
            // Dotted, with or without the `module` flag; a subcommand of one too.
            "MOD.READ x",
            "MOD.CMD GET k",
            "mod.cmd|get k",
            "FT.CONFIG GET *",
            // Internal commands.
            "_FT.SEARCH idx *",
            "_INTERNAL x",
        ] {
            assert_eq!(classify_line(line, s), Write, "{line}");
        }
        // A module's read on the static list reads by the list.
        assert_eq!(classify_line("TS.RANGE t - +", s), Read);
    }

    /// Commands that change data, with the flags RediSearch 2.6 to 2.10 (Redis Stack 6.2.6 to
    /// 7.4), RedisBloom 2.6 and RedisGraph register them with: with Redis 7's `module` flag, and
    /// without it.
    #[test]
    fn writes_that_modules_flag_readonly_stay_writes() {
        let flagged = |module: &[&str]| {
            let e = |name: &str, flags: &[&str]| entry(name, &[flags, module].concat(), NO_SUBS);
            reads_of(&[
                e("ft.dictadd", &["readonly"]),
                e("ft.dictdel", &["readonly"]),
                e("ft.aliasadd", &["readonly"]),
                e("ft.aliasupdate", &["readonly"]),
                e("ft.aliasdel", &["readonly"]),
                e("ft.cursor", &["readonly"]),
                e("cf.compact", &["readonly", "fast"]),
                e("graph.slowlog", &["readonly"]),
            ])
        };
        for reads in [flagged(&["module"]), flagged(&[])] {
            assert!(reads.is_empty());
            let s = Some(&reads);
            for line in [
                "FT.DICTADD dict word",
                "FT.DICTDEL dict word",
                "FT.ALIASADD alias idx",
                "FT.ALIASUPDATE alias idx2",
                "FT.ALIASDEL alias",
                "FT.CURSOR DEL idx 1",
                "FT.CURSOR GC idx 0",
                "CF.COMPACT cf",
                "GRAPH.SLOWLOG g RESET",
            ] {
                assert_eq!(classify_line(line, s), Write, "{line}");
            }
            // Their read forms read by the static list.
            assert_eq!(classify_line("FT.CURSOR READ idx 1", s), Read);
            assert_eq!(classify_line("GRAPH.SLOWLOG g", s), Read);
        }
    }

    #[test]
    fn entries_older_than_redis_7_add_nothing() {
        // Redis 6 sends seven fields (Redis 5 six) and no `module` flag: a module's undotted
        // command looks like a core one.
        let text = "*2\r\n*7\r\n$7\r\nmodread\r\n:2\r\n*1\r\n+readonly\r\n:1\r\n:1\r\n:1\r\n*0\r\n*6\r\n$7\r\nnewread\r\n:2\r\n*2\r\n+readonly\r\n+fast\r\n:1\r\n:1\r\n:1\r\n";
        let reads = ServerReads::from_reply(&parse_redis_value(text.as_bytes()).unwrap());
        assert!(reads.is_empty());
        assert_eq!(classify_line("MODREAD x", Some(&reads)), Write);
        assert_eq!(classify_line("NEWREAD x", Some(&reads)), Write);
        assert_eq!(classify_line("GET k", Some(&reads)), Read);
    }

    #[test]
    fn pfcount_and_internals_stay_writes_whatever_their_flags() {
        // Flagged plain `readonly`, as Redis before 6.2 flags PFCOUNT.
        let reads = reads_of(&[
            entry("pfcount", &["readonly"], NO_SUBS),
            entry("ft._alterifnx", &["readonly"], NO_SUBS),
        ]);
        assert!(reads.names.contains("pfcount"));
        let s = Some(&reads);
        assert_eq!(classify_line("PFCOUNT h", s), Write);
        assert_eq!(
            classify_line("FT._ALTERIFNX idx SCHEMA ADD f TEXT", s),
            Write
        );
        assert_eq!(classify_line("ft._alterifnx idx", s), Write);
    }

    #[test]
    fn flags_never_promote_pipe_names_stores_or_user_code() {
        let reads = command_info();
        let s = Some(&reads);
        // Subcommands are kept as `container|sub`; typed as a name they never match.
        assert!(reads.names.contains("object|encoding"));
        assert_eq!(classify_line("OBJECT ENCODING k", s), Read);
        assert_eq!(classify_line("object|encoding k", s), Write);
        assert_eq!(classify_line("OBJECT|ENCODING k", s), Write);
        assert_eq!(classify_line("newcmd|get k", s), Write);
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
        let text = "*1\r\n*10\r\n$7\r\nnewread\r\n:2\r\n~1\r\n+readonly\r\n:1\r\n:1\r\n:1\r\n~0\r\n*0\r\n*0\r\n*0\r\n";
        let reads = ServerReads::from_reply(&parse_redis_value(text.as_bytes()).unwrap());
        assert!(reads.names.contains("newread"));
    }

    #[test]
    fn console_refusals() {
        let refused = |l: &str| refused_in_console(&tokenize(l).unwrap()).is_some();
        for l in [
            "BLPOP l 0",
            "BZPOPMIN z 1",
            "XREAD BLOCK 100 STREAMS s $",
            "xreadgroup group g c block 0 streams s >",
            "XREAD \"BLOCK\\x00\" 0 STREAMS s $",
            "XREADGROUP GROUP g c \"block\\x00x\" 0 STREAMS s >",
            "SUBSCRIBE ch",
            "MONITOR",
            "HELLO 2",
            "AUTH x",
            "CLIENT REPLY OFF",
            "CLIENT \"REPLY\\x00\" OFF",
            "client \"tracking\\x00x\" on",
            "CLIENT \"CACHING\\x00\" yes",
            "WAIT 1 0",
            "\"CLIENT\\x00a\" REPLY OFF",
            "\"MONITOR\\x00a\"",
            "\"SUBSCRIBE\\x00a\" ch",
            "\"HELLO\\x00a\" 3",
            "\"BLPOP\\x00a\" k 0",
            "\"SELECT\\x00a\" 5",
            "\"get\\x00\" k",
            "\"client\\x00\\xff\" reply off",
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
