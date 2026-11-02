import type { StatementClass } from "@/features/safety/classify";
import { asciiUpper, commandName, splitConsole, tokenize, utf8Text, type Argv } from "./cli";

/**
 * Read or write, for Safe Mode: the backend's static classifier (`drivers/redis/classify.rs`)
 * rule for rule, default-deny. The backend also reads the server's `COMMAND` flags for core reads
 * newer than its list; the renderer does not know them, so such a command asks as a write here
 * and still runs (the backend calls it a read). Both run `tests/fixtures/redis/console.cases.json`.
 */

/** Never reads, whatever a server flags them (scripts, administration, transactions, …). */
const NEVER_READ = new Set([
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
]);

const NEVER_READ_PREFIXES = ["RG."];

/** How a listed command reads: always, with one of its subcommands, or unless an option word. */
type Rule =
  | { read: true }
  /** `""`: also without a subcommand. */
  | { sub: readonly string[] }
  | { unless: readonly string[] };

const READ: Rule = { read: true };
const sub = (...s: string[]): Rule => ({ sub: s });
const unless = (...w: string[]): Rule => ({ unless: w });

const READS = new Map<string, Rule>([
  // Strings and bitmaps.
  ...["GET", "GETRANGE", "SUBSTR", "MGET", "STRLEN", "LCS"].map((n) => [n, READ] as const),
  ["STRALGO", sub("LCS")],
  ...["GETBIT", "BITCOUNT", "BITPOS", "BITFIELD_RO"].map((n) => [n, READ] as const),
  // Keys.
  ...[
    "EXISTS",
    "TYPE",
    "TTL",
    "PTTL",
    "EXPIRETIME",
    "PEXPIRETIME",
    "RANDOMKEY",
    "SCAN",
    "DBSIZE",
    "DUMP",
    "TOUCH",
    "SORT_RO",
  ].map((n) => [n, READ] as const),
  ["SORT", unless("STORE")],
  ["OBJECT", sub("ENCODING", "FREQ", "IDLETIME", "REFCOUNT", "HELP")],
  ["MEMORY", sub("USAGE", "HELP")],
  // Lists, sets, sorted sets, hashes.
  ...[
    "LRANGE",
    "LLEN",
    "LINDEX",
    "LPOS",
    "SMEMBERS",
    "SISMEMBER",
    "SMISMEMBER",
    "SCARD",
    "SRANDMEMBER",
    "SSCAN",
    "SINTER",
    "SUNION",
    "SDIFF",
    "SINTERCARD",
    "ZRANGE",
    "ZRANGEBYSCORE",
    "ZRANGEBYLEX",
    "ZREVRANGE",
    "ZREVRANGEBYSCORE",
    "ZREVRANGEBYLEX",
    "ZCARD",
    "ZCOUNT",
    "ZLEXCOUNT",
    "ZSCORE",
    "ZMSCORE",
    "ZRANK",
    "ZREVRANK",
    "ZSCAN",
    "ZRANDMEMBER",
    "ZDIFF",
    "ZINTER",
    "ZUNION",
    "ZINTERCARD",
    "HGET",
    "HMGET",
    "HGETALL",
    "HKEYS",
    "HVALS",
    "HLEN",
    "HEXISTS",
    "HSTRLEN",
    "HSCAN",
    "HRANDFIELD",
    "HTTL",
    "HPTTL",
    "HEXPIRETIME",
    "HPEXPIRETIME",
    // Streams.
    "XRANGE",
    "XREVRANGE",
    "XLEN",
    "XPENDING",
  ].map((n) => [n, READ] as const),
  ["XINFO", sub("STREAM", "GROUPS", "CONSUMERS", "HELP")],
  ["XREAD", unless("BLOCK")],
  // Geo.
  ...["GEOPOS", "GEODIST", "GEOHASH", "GEORADIUS_RO", "GEORADIUSBYMEMBER_RO", "GEOSEARCH"].map(
    (n) => [n, READ] as const,
  ),
  ["GEORADIUS", unless("STORE", "STOREDIST")],
  ["GEORADIUSBYMEMBER", unless("STORE", "STOREDIST")],
  // Server and connection; `SELECT` changes only the console's own connection.
  ...["PING", "ECHO", "TIME", "INFO", "LASTSAVE", "ROLE", "LOLWUT", "SELECT"].map(
    (n) => [n, READ] as const,
  ),
  ["COMMAND", sub("", "COUNT", "DOCS", "INFO", "LIST", "GETKEYS", "GETKEYSANDFLAGS", "HELP")],
  ["PUBSUB", sub("CHANNELS", "NUMSUB", "NUMPAT", "SHARDCHANNELS", "SHARDNUMSUB", "HELP")],
  // RedisJSON, TimeSeries, Bloom, Search.
  ...[
    "JSON.GET",
    "JSON.MGET",
    "JSON.TYPE",
    "JSON.STRLEN",
    "JSON.OBJLEN",
    "JSON.OBJKEYS",
    "JSON.ARRLEN",
    "JSON.ARRINDEX",
    "JSON.RESP",
    "TS.GET",
    "TS.MGET",
    "TS.RANGE",
    "TS.REVRANGE",
    "TS.MRANGE",
    "TS.MREVRANGE",
    "TS.INFO",
    "TS.QUERYINDEX",
    "BF.EXISTS",
    "BF.MEXISTS",
    "BF.INFO",
    "BF.CARD",
    "BF.SCANDUMP",
    "CF.EXISTS",
    "CF.MEXISTS",
    "CF.COUNT",
    "CF.INFO",
    "CF.SCANDUMP",
    "CMS.QUERY",
    "CMS.INFO",
    "TOPK.QUERY",
    "TOPK.COUNT",
    "TOPK.LIST",
    "TOPK.INFO",
    "TDIGEST.MIN",
    "TDIGEST.MAX",
    "TDIGEST.QUANTILE",
    "TDIGEST.CDF",
    "TDIGEST.RANK",
    "TDIGEST.REVRANK",
    "TDIGEST.BYRANK",
    "TDIGEST.BYREVRANK",
    "TDIGEST.TRIMMED_MEAN",
    "TDIGEST.INFO",
    "FT.SEARCH",
    "FT.AGGREGATE",
    "FT.PROFILE",
    "FT.INFO",
    "FT.EXPLAIN",
    "FT.EXPLAINCLI",
    "FT.SPELLCHECK",
    "FT.TAGVALS",
    "FT.SUGGET",
    "FT.SUGLEN",
    "FT.SYNDUMP",
    "FT.DICTDUMP",
    "FT._LIST",
  ].map((n) => [n, READ] as const),
  ["FT.CURSOR", sub("READ")],
  ["GRAPH.SLOWLOG", unless("RESET")],
]);

const EQ_FOLD = (a: Uint8Array, word: string): boolean => {
  if (a.length !== word.length) return false;
  for (let i = 0; i < a.length; i++) {
    const x = a[i]!;
    const lower = x >= 0x41 && x <= 0x5a ? x + 0x20 : x;
    if (lower !== word.toLowerCase().charCodeAt(i)) return false;
  }
  return true;
};

/**
 * Whether an argument selects an option word as the server reads it: `strcasecmp` stops at the
 * first NUL, so `"STORE\x00junk"` is STORE too.
 */
export function isOption(arg: Uint8Array, word: string): boolean {
  const end = arg.indexOf(0);
  return EQ_FOLD(end < 0 ? arg : arg.subarray(0, end), word);
}

/** The class of one command's argv (`classify_argv` without the server's flags). */
export function classifyArgv(argv: Argv): StatementClass {
  const name = commandName(argv);
  if (name === null) return "write";
  if (NEVER_READ.has(name) || NEVER_READ_PREFIXES.some((p) => name.startsWith(p))) return "write";
  const rule = READS.get(name);
  if (!rule) return "write";
  if ("read" in rule) return "read";
  if ("sub" in rule) {
    const second = argv[1];
    const s = second === undefined ? null : utf8Text(second);
    if (second !== undefined)
      return s !== null && rule.sub.includes(asciiUpper(s)) ? "read" : "write";
    return argv.length === 1 && rule.sub.includes("") ? "read" : "write";
  }
  return argv.slice(1).some((a) => rule.unless.some((w) => isOption(a, w))) ? "write" : "read";
}

/** The class of one command line; a line that cannot be read is a write (`classify_line`). */
export function classifyCommand(line: string): StatementClass {
  const t = tokenize(line);
  return t.ok ? classifyArgv(t.argv) : "write";
}

/** Every command of console text, as Safe Mode reads it: a write when any one writes. */
export function classifyText(text: string): StatementClass {
  return splitConsole(text).some((c) => classifyCommand(c.sql) === "write") ? "write" : "read";
}

const CHANGES_CONNECTION =
  "this command changes the connection itself; edit the connection instead";

const BLOCKING = new Set([
  "BLPOP",
  "BRPOP",
  "BLMOVE",
  "BRPOPLPUSH",
  "BLMPOP",
  "BZPOPMIN",
  "BZPOPMAX",
  "BZMPOP",
]);

/** Why the console refuses to run `argv` (`refused_in_console`); null when it runs it. */
export function refusedInConsole(argv: Argv): string | null {
  if (argv[0]?.includes(0)) return "a command name cannot contain a NUL byte";
  const name = commandName(argv);
  if (name === null) return null;
  const blockArg =
    (name === "XREAD" || name === "XREADGROUP") && argv.slice(1).some((a) => isOption(a, "BLOCK"));
  if (BLOCKING.has(name) || blockArg || name === "WAIT" || name === "WAITAOF")
    return "blocking commands are not run in the console: they would hold the connection";
  switch (name) {
    case "SUBSCRIBE":
    case "PSUBSCRIBE":
    case "SSUBSCRIBE":
    case "MONITOR":
    case "SYNC":
    case "PSYNC":
      return "pub/sub, MONITOR and replication streams are not supported in the console";
    case "HELLO":
    case "AUTH":
    case "QUIT":
    case "RESET":
      return CHANGES_CONNECTION;
    case "CLIENT": {
      const s = argv[1];
      return s && ["REPLY", "TRACKING", "CACHING"].some((w) => isOption(s, w))
        ? CHANGES_CONNECTION
        : null;
    }
    default:
      return null;
  }
}
