# Redis and Valkey: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                               |
| ----------------- | --------------------------------------------- |
| Lane              | `redis`                                       |
| Databases         | Redis, Valkey                                 |
| Drivers           | `redis`                                       |
| Picker kinds      | `redis`                                       |
| SQL dialect       | none (native store)                           |
| Native builders   | `sqlgen/native/redis.rs`                      |
| Cargo features    | `drv-redis`                                   |
| Home slot         | `s3`                                          |
| Bindings prefix   | `Redis*`                                      |
| Lib test filters  | `drivers::redis`, `sqlgen::native::redis`     |
| Real-server suite | `tests/redis.rs` with `TABLER_TEST_REDIS_URL` |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/redis/**`
- `src-tauri/src/sqlgen/native/redis.rs`
- `src-tauri/tests/redis.rs`
- `src-tauri/tests/fixtures/redis/**`
- `src/features/drivers/redis/**`
- `src/lib/ipc/mock/drivers/redis/**`
- `src/lib/ipc/bindings/Redis*.ts`
- `docs/drivers/redis.md`

Anything else (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead. `node dev/scripts/lane-commit.mjs redis -m "redis: …"` refuses other paths.

## Scope of v1

### Redis

- **Identity:** `redis`, no dialect (RedisCli). Crates: redis `=1.7.1` (no default features, tokio-comp), tokio-rustls.
- **UI:** key browser (`TableView`) plus a per-type `Inspector`.
- **Auth v1:** none, password, ACL user plus password, Command.
- **Required fields:** host (6379); database = DB index.
- **Out of scope v1:** cluster, sentinel, modules beyond raw commands, MONITOR, pub/sub.
- **Test:** in-process RESP server (RESP2 and RESP3), tokenizer goldens. Opt-in: Memurai or Garnet (approval).

## Descriptor values

Seeded from the plan (§5.2). The values in force are the descriptors in `src-tauri/src/types/driver.rs` (keepalive belongs to the session's caps); a change is a request to the lead.

| Driver | Location | Port (TLS) | SSH    | TLS style | Secret label | Command TTL |
| ------ | -------- | ---------- | ------ | --------- | ------------ | ----------- |
| redis  | Host     | 6379       | Tunnel | OnOff     | Password     | 600         |

| Driver | Required fields / params | Label overrides    | Keepalive | Read-only      | Sharing | Token secret |
| ------ | ------------------------ | ------------------ | --------- | -------------- | ------- | ------------ |
| redis  | host                     | database: DB index | 30        | ClassifierOnly | Indep   | no           |

## Read-only and hardening

| Engine | When read-only                                                                                                                                                                                                                                                                                                                 | When unattended (MCP) |
| ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------- |
| Redis  | Allow-list: a static list (GET, MGET, HGETALL, SCAN, TYPE, TTL, …, modules' reads), plus core commands whose `COMMAND INFO` (Redis 7+) flags them `readonly` without `admin`, `may_replicate`, `noscript`, `blocking`; never a module's command by its flags. EVAL*, FCALL, SCRIPT, CONFIG, CLIENT, SHUTDOWN, DEBUG are writes | MCP refused (native)  |

Every classifier is default-deny: a statement or command it does not know is a write. The read-only mode in the caps (`Server`, `PerRequest` or `ClassifierOnly`) must say what the session really enforces.

## What to implement

- `entry.rs` (Phase B): `open`, `validate`, `util`, `util_kind` (plan §2.5). `<Prefix>Params` (`#[serde(rename_all = "camelCase", default)]`, ts-exported) parsed from `options.driver_params`.
- `NativeSession` plus `TableSource` (plan §2.5): browse with cursors, count, fetch cell, `plan_changes` / `apply_changes` (preview and operations from one plan, text through `sqlgen/native/<x>.rs`), the console (`split_console`, `classify_console` default-deny, `run_console`), `definition()`, `call` / `call_kind` (unknown calls are writes).
- `ping` is free, `close` returns within about 2 s.
- Cancel through `CancelHandle::Custom(Arc<dyn CancelAction>)`.
- Renderer: a `KindUi` per tile, `DriverUi` with `ConsoleUi`, `TableView` and `Inspector` where the grid does not fit, and a `MockDriver`. Write-capable `sessionCall` / `driverUtil` helpers await `guardExecution` first.

## Decisions and corrections for this lane

- A native store (key browser and per-type inspector); MCP refuses it in v1 (plan A30). Phase A starts after F6 or is limited to codecs and the RESP fake (critique optional 16).
- The module `drivers::redis` shadows the `redis` crate in parent scopes: refer to the crate as `::redis::…` (critique optional 2).
- Read-only is an allow-list: the `readonly` flag of `COMMAND INFO` minus `admin`, `may_replicate`, `noscript` and `blocking`, plus a static list; `EVAL*`, `FCALL`, `SCRIPT`, `CONFIG`, `CLIENT`, `SHUTDOWN` and `DEBUG` are writes (plan §5.3). B1 trusts the flag for core commands only (modules flag writes `readonly`), see Phase B status.
- MULTI/EXEC (TxMode Batch) applies in part when a command fails, and non-idempotent writes (RPUSH, HINCRBY) would apply twice on a retry: return a result per change, and the renderer unstages the applied ones (critique D2). `WATCH` proves the expected key state (plan A13).
- redis is built without default features and without native-tls; TLS goes through tokio-rustls (`tests/manifest_features.rs` guards it).

## Research notes

> Background from the driver research (October 2026), with file references and no line numbers. Verify against the code; where it differs from the scope above or from README.md, those win.

### Redis

**Approach.**

Use the `redis` crate 1.7.1 (MSRV 1.88, edition 2024 like the app). 1.8.0 was published on 2026-10-23; stay on 1.7.x until it has had a few weeks. Configure it as `redis = { version = "1.7.1", default-features = false, features = ["tokio-comp"] }`. Add `"cluster-async"` and `"sentinel"` in a later phase. Bring your own stream: open the TCP socket yourself, either to the profile host or to the SSH tunnel's local port (`tunnel/mod.rs connect_addr`). Wrap it in tokio-rustls 0.26, which is already in Cargo.lock, using the same aws-lc-rs provider and verifier code as `drivers/postgres/connect.rs provider()`. Then call `redis::aio::MultiplexedConnection::new_with_config(&RedisConnectionInfo::default().set_db(n).set_username(u).set_password(p).set_protocol(..), stream, AsyncConnectionConfig)` and spawn the driver future it returns. This approach: (a) keeps the real host name for SNI and certificate checks even through a tunnel; (b) reuses the existing SslMode, CA and client-cert handling (`types/profile.rs`); (c) needs none of redis's own TLS features. Send every command as raw `redis::cmd(argv0).arg(..)` or a `redis::Pipeline`, read the reply as untyped `redis::Value` (15 variants, non_exhaustive: Nil, Int, BulkString, Array, SimpleString, Okay, Map, Attribute, Set, Double, Boolean, VerbatimString, BigNumber, Push, ServerError) and map it to the tagged `Value`. Connections: one multiplexed connection per db index for grid reads, because SELECT is per connection and must never be sent on a shared one. Plus one dedicated editor connection where SELECT, MULTI and WATCH persist, like the PG/MySQL editor session. Why redis-rs: it is the de-facto standard, pure Rust, supports RESP2 and RESP3 and pipelines, and has cluster and sentinel support for later. TablePlus is hiredis-based and still cannot do cluster mode (issue #1280, its most-upvoted Redis request).

**Windows build.**

Pure Rust with no C dependencies. With `default-features = false` plus `tokio-comp` it pulls only small crates (combine, bytes, tokio-util, futures-util, url, socket2, arc-swap, itoa/ryu, percent-encoding). Expect about 15-25 s for a clean debug build and under 1 MB added to the release binary. Do not enable `tls-native-tls` or `tokio-native-tls-comp` (native-tls/SChannel). If redis-managed TLS is ever needed (cluster-async opens its own node connections), enable `tokio-rustls-comp`. It declares `rustls = { default-features = false }` plus `std`, so it uses the process default provider. That resolves to aws-lc-rs because rustls 0.23.45 in Cargo.lock depends only on aws-lc-rs. It also adds rustls-native-certs 0.8, which is already a dependency. Leaving default features off also drops `num-bigint`; map RESP3 BigNumber from its text instead. Dev-only: `redis-test` 1.0.4 (MockRedisConnection).

**Authentication.**

TablePlus offers password AUTH (b111), an ACL username (Redis 6+; a 'default' user hangs, #3693), SSL/TLS including ElastiCache TLS (b200, b233), SSH, and redis:// URL import (b121). Tabler should offer: (1) no auth; (2) password only; (3) ACL user + password, via `HELLO 3 AUTH user pass` or `AUTH user pass` on RESP2 servers, using the existing PasswordMode (keychain/ask/none/command, `types/profile.rs`). The 'command' mode also covers short-lived IAM tokens for ElastiCache/MemoryDB and Entra ID tokens for Azure Cache, printed by a CLI. TLS reuses SslMode (disable/prefer/require/verify-ca/verify-full) with a CA file and client cert/key for managed mTLS, plus the existing SSH tunnel. Phase 2: Sentinel (sentinel's own password plus the master name) and Cluster (seed nodes, shared credentials).

**Data model.**

How TablePlus shows Redis (from its issues and changelog): the left sidebar lists databases as items db0, db1… It derives them from INFO/CONFIG, so nothing shows on ElastiCache, which blocks those commands (#3304); there is also a db-switch bug (#2154). Clicking an item opens a spreadsheet grid of keys (key / value, plus TTL), paged with `SCAN <cursor> MATCH <pattern> COUNT <page size>` (#1094). The filter bar is a glob 'Pattern'; `key = abc` wrongly runs a SCAN (#1224). The right sidebar edits values, but Set NULL deletes the key (#2923) and editing resets the TTL (#3332). Streams show only their first entry (#1803), and clicking to sort keys does nothing (#2097). There is no namespace tree (requested in #1033, #1552 and Windows #654) and no JSON pretty view (#3215, Windows #655). A Console tab runs raw commands.

How Redis maps onto DbSession (drivers/mod.rs):

- list_databases → db0..db{N-1}. N comes from `CONFIG GET databases`, falling back to 16 or to the highest dbN in INFO keyspace.
- list_schemas → a single schema named after the current db (the MySQL pattern, drivers/mysql/introspect.rs).
- list_tables → one TableSummary per db (name 'db3', kind Table, row_estimate = keys from INFO keyspace, comment 'expires=…'). This gives TablePlus parity: the sidebar shows the dbs without a reconnect.
- table_meta('dbN') → synthetic columns:
  - `key`: Text, or Bytes when not UTF-8; it is the PK, so RowIdentity::Pk / RowKey::Pk{key}.
  - `type`: Enum with enum_values string/list/set/zset/hash/stream/ReJSON-RL/TSDB-TYPE/….
  - `ttl`: Int seconds from PTTL; NULL means no expiry.
  - `size`: Int from STRLEN/HLEN/LLEN/SCARD/ZCARD/XLEN.
  - `memory`: Int from MEMORY USAGE, loaded lazily.
  - `value`: Text/Json/Bytes preview, capped.
- view_definition → None. completion_metadata → command names plus key prefixes seen.
- query/execute/run_statement → one command line. The RESP reply becomes a ResultSet: a scalar is 1×1 `value`; an array becomes (index, value) rows; a RESP3 map or the flat pairs from HGETALL/CONFIG GET become (field, value); `ZRANGE … WITHSCORES` becomes (member, score); a SCAN reply becomes the cursor plus keys; nested replies become Json cells.
- transaction() → MULTI/EXEC. in_transaction() tracks MULTI on the editor connection. stream_query (export) → a SCAN loop with pipelined TYPE/PTTL/value reads.

Shared plumbing needed (also used by DynamoDB, Cassandra and Elasticsearch):

- commands/browse.rs `browse_page` and `count_table_rows` always call sqlgen `build_select`/`build_count` + `query_limited`. commands/mutate.rs `plan_rows` always calls `build_change` → TxStatement. Add default DbSession methods that NoSQL drivers override: `browse_native`, `count_native`, `plan_native`/`apply_native`, `split_script`.
- BrowseRequest (types/browse.rs, `offset: u64`) needs `cursor: Option<String>`, and BrowsePage needs `next_cursor`. For Redis these hold the SCAN cursor; the renderer keeps a stack of cursors for Previous.
- Driver (types/profile.rs) gains `Redis` with default_port 6379. CancelHandle (drivers/mod.rs) gains a drop/abort variant.

New UI beyond the grid:

- Key browser with a namespace tree split by the separator, built incrementally from SCAN pages; never KEYS.
- Per-type value editor in the right panel or its own tab:
  - string: text / JSON pretty / hex, read with a capped GETRANGE;
  - hash: field/value grid via HSCAN, plus field TTL on 7.4+;
  - list: windowed LRANGE;
  - set: SSCAN;
  - zset: member/score grid;
  - stream: XRANGE pages with id + fields, and consumer groups;
  - ReJSON: JSON tree.

  Reuse src/features/datagrid/editors/JsonEditorDialog.tsx and quicklook.

- TTL editor and a 'New key' dialog per type.
- Server info (INFO sections), plus CLIENT LIST/KILL and SLOWLOG as the Process List.
- The SQL editor in Redis-command mode as the console.

**Query language.**

Redis commands in redis-cli syntax, one per line or separated by `;`. Arguments split on whitespace; double-quoted strings take C-style escapes (backslash-n, backslash-t, backslash-xHH, escaped quote), and single-quoted strings are literal. Each statement is sent as argv through `redis::cmd(argv[0]).arg(..)`, so it is binary-safe and never string-concatenated. TablePlus had bugs with keys containing spaces (#460) and with empty strings (#2923), and its Beautify broke globs (#631), so disable SQL formatting for Redis.

Client-side handling:

- `SELECT n` changes only the editor connection.
- MULTI/EXEC/DISCARD/WATCH drive in_transaction.
- Blocking or streaming commands (BLPOP, XREAD BLOCK, SUBSCRIBE, MONITOR) are refused or time-limited.

Splitting: a new line-based splitter beside sqlgen/split.rs.

Safe mode: guardExecution (src/features/safety/guard.ts, classify.ts) classifies commands as read or write from a bundled table or from `COMMAND INFO` flags (write/admin/dangerous). It flags FLUSHALL, FLUSHDB, KEYS, DEBUG, SHUTDOWN, CONFIG SET and SCRIPT FLUSH.

Editor: a small CodeMirror StreamLanguage for Redis instead of @codemirror/lang-sql (src/features/editor/cmExtensions.ts CM_DIALECTS). Completion comes from `COMMAND DOCS` (7.0+) or a bundled list.

**Editing.**

Grid edits stay StagedChange. A Redis command builder plans them; put it in a new module such as src-tauri/src/sqlgen/redis.rs, keeping the CLAUDE.md rule that all generated text goes through sqlgen.

- String value edit → `SET key value KEEPTTL` (6.0+). On older servers, SET and then PEXPIRE the remaining TTL. This avoids TablePlus's TTL reset (#3332).
- Key rename → `RENAMENX old new`, which refuses to overwrite.
- TTL cell → `PEXPIRE key ms`; clearing it or entering -1 → `PERSIST key`.
- Type column → read-only.
- Delete row → `UNLINK key` (4.0+, else DEL).
- Insert row → `SET key value NX`, or HSET/RPUSH/SADD/ZADD/XADD from the New-key dialog.
- Set NULL → disabled, because TablePlus deletes the key (#2923). Allow an empty string.
- Container edits happen in the per-type editor: HSET/HDEL, LSET/LINSERT/LREM, SADD/SREM, ZADD XX/ZREM, XADD/XDEL, JSON.SET path.

Commit of several changes: `MULTI … EXEC` on one connection. This is atomic on a single node but has no rollback, and per-command errors in the EXEC reply map to RowResult.error. Optionally WATCH the loaded keys for optimistic concurrency: a nil EXEC reply becomes 'changed by someone else', mirroring check_affected (drivers/mod.rs). DbSession::transaction(Vec\<TxStatement>) maps onto this with one command line per TxStatement.

Other operations: Duplicate row → `COPY src dst` (6.2+) or DUMP/RESTORE. Truncate a db item → `FLUSHDB` (ASYNC), guarded by safe mode. There is no column DDL.

**Introspection.**

Connect:

- `HELLO 3 [AUTH user pass] [SETNAME tabler]`, falling back to `AUTH` and RESP2 on servers before 6.0.
- `PING`.
- `CLIENT ID`, for cancel: `CLIENT KILL ID <id>` from a second connection.
- `CLIENT SETINFO LIB-NAME tabler`, skipped on old servers.

Server:

- `INFO server` → redis_version, redis_mode (standalone/cluster/sentinel), os.
- `INFO keyspace` → lines like `db0:keys=12,expires=3,avg_ttl=0`.
- `CONFIG GET databases` → db count. Managed services such as ElastiCache, MemoryDB and Upstash may refuse CONFIG and parts of INFO (#3304); catch the error and fall back to 16.
- `DBSIZE` per db connection.
- `MODULE LIST` → ReJSON, timeseries, bf, search.
- `ACL WHOAMI`.

Key listing:

- `SCAN <cursor> MATCH <glob> COUNT <n> [TYPE <type>]`; TYPE needs 6.0+.
- An exact-key filter uses `EXISTS key` + `TYPE key`, never SCAN (#1224).

Per page, pipelined:

- `TYPE k`, `PTTL k`.
- Size by type: `STRLEN`, `HLEN`, `LLEN`, `SCARD`, `ZCARD`, `XLEN`, `JSON.STRLEN`/`JSON.OBJLEN`.
- Optional `MEMORY USAGE k SAMPLES 0` and `OBJECT ENCODING k`.

Values, always ranged:

- `GETRANGE k 0 <cap-1>`, `HSCAN k <c> COUNT n` (`NOVALUES` on 7.4+), `LRANGE k a b`, `SSCAN k <c> COUNT n`, `ZRANGE k a b WITHSCORES` / `ZSCAN`.
- Streams: `XREVRANGE k + - COUNT n`, `XINFO STREAM k`, `XINFO GROUPS k`.
- JSON: `JSON.GET k $`, `JSON.TYPE k`.

Admin:

- `CLIENT LIST`, `CLIENT KILL ID`, `SLOWLOG GET 128`.
- `INFO stats|memory|replication|clients` for a Metrics Board.
- `COMMAND DOCS` (7.0+) for completion.

Cluster (phase 2): `CLUSTER SHARDS` / `CLUSTER NODES`, then SCAN on each primary.

**Testing without a server.**

Unit tests:

- RESP `redis::Value` → ResultSet/Value mapping, covering RESP2 vs RESP3 shapes (HGETALL flat pairs vs Map), binary keys, Nil, BigNumber and VerbatimString.
- The redis-cli tokenizer: quotes, hex escapes, empty string, unbalanced quotes.
- Command-builder golden tests: SET … KEEPTTL, RENAMENX, PEXPIRE/PERSIST, MULTI batches.
- SCAN page assembly and the Previous cursor stack.
- INFO keyspace parsing.

Without any server:

- The `redis-test` 1.0.4 crate provides MockRedisConnection/MockCmd, which implement redis's async ConnectionLike, so the key-browser logic can run against scripted replies.
- A tiny in-process RESP server (tokio TcpListener with hand-encoded replies) covers the handshake, our tokio-rustls TLS stream and tunnel plumbing.

Integration:

- Add a `redis` profile to dev/docker-compose.yml with redis:8 or valkey/valkey:8, plus a TLS variant using dev/certs.
- Windows has no native Redis. Use Docker, WSL, Memurai (a Redis-compatible Windows service) or Microsoft Garnet (.NET), keeping their command gaps in mind.

**What it shares with an existing driver.**

Not wire-compatible with postgres, mysql or sqlite; none of sqlgen's SQL builders apply. The same driver serves the RESP-compatible servers, each with gaps:

- Valkey 7-9.
- KeyDB.
- Dragonfly: partial MEMORY/OBJECT support.
- Microsoft Garnet: limited INFO/CONFIG, no MODULE.
- Upstash: TLS only, CONFIG blocked.
- AWS ElastiCache and MemoryDB: CONFIG and INFO restricted, often cluster mode.
- Azure Cache for Redis: port 6380 with TLS.

Version gates: SCAN TYPE and SET KEEPTTL need 6.0+, COPY 6.2+, COMMAND DOCS 7.0+, HSCAN NOVALUES and hash-field TTL 7.4+, MEMORY USAGE and UNLINK 4.0+, HELLO/RESP3 6.0+.

**Connection fields.**

- Name / color / tag / group (existing profile fields)
- Host (default 127.0.0.1)
- Port (default 6379; Azure TLS 6380)
- Username (optional ACL user, Redis 6+)
- Password + storage mode (keychain/ask/none/command)
- Database index (default 0; maps to profile.database)
- TLS mode (reuse SslMode) + CA certificate file
- TLS client certificate + key (mTLS)
- Key separator for the namespace tree (default ':')
- Keys per page / SCAN COUNT hint (TablePlus asks users to lower it for slow servers, Windows #439)
- Value preview cap (bytes / elements loaded per key)
- Connection mode: Standalone (phase 1) / Sentinel (master name + sentinel nodes) / Cluster (seed nodes) — phase 2
- SSH tunnel (existing SshConfig)
- Connect timeout (existing)
- Read only / safe mode (existing ProfileOptions)
- Startup commands (existing startup_sql, run as Redis command lines, e.g. CLIENT SETNAME)
- URL import: redis://user:pass@host:6379/3 and rediss:// (TLS), added to SCHEMES in src/features/connections/url.ts

**Effort:** L.

**Risks.**

- SCAN cost: production keyspaces can hold millions of keys. The app must never send KEYS (TablePlus Windows #439 took 10 minutes). Paging must be cursor-based: SCAN can return duplicates and uneven page sizes, and its order is arbitrary, so server-side sorting is impossible (TablePlus #2097). Make clear that sorting only reorders the current page.
- Managed services refuse CONFIG, INFO or MODULE (ElastiCache #3304, Upstash, Garnet). Every introspection call needs a fallback.
- Binary keys and values: keys must round-trip byte-exact through RowKey::Pk (Value::Bytes). Text keys with spaces or quotes must be sent as argv, never formatted (TablePlus #460).
- Huge values (multi-MB strings, million-element lists or hashes) must always be read with ranges (GETRANGE/LRANGE/HSCAN) and loaded lazily (TablePlus #975).
- TTL semantics: SET without KEEPTTL drops the TTL (TablePlus #3332). KEEPTTL needs 6.0+, so older servers need the remaining PTTL re-applied, which is racy.
- No server-side cancel of a running command, because Redis is single-threaded. Only Lua scripts can be stopped (SCRIPT KILL / FUNCTION KILL); otherwise the client drops the connection. A slow command blocks the whole server, so safe-mode defaults matter.
- Cluster mode (MOVED/ASK redirects, per-node SCAN, db0 only, CROSSSLOT errors in MULTI) is the most requested TablePlus Redis feature (#1280). It needs `cluster-async` and its own connection management (TLS then via redis's tokio-rustls-comp).
- RESP2 and RESP3 reply shapes differ (maps vs flat arrays, Double vs bulk string). Pick one protocol per connection and normalize.
- Module types (ReJSON-RL, TSDB-TYPE, MBbloom--, vector sets) have no standard read commands. Show unknown types read-only.
- redis 1.8.0 was released on 2026-10-23 (the day of this research). Pin 1.7.x and expect fast minor-version churn (seven minors in four months).
- MULTI/EXEC has no rollback: a failing command inside EXEC leaves the earlier ones applied. The commit UI must report per-command errors instead of claiming the batch was atomic.

## Phase A status

Phase A was limited to codecs, builders and the fake, because the native session arrives with F6. Nothing here is wired into `entry.rs` yet.

**Done** (`src-tauri/src/drivers/redis/`, `src-tauri/src/sqlgen/native/redis.rs`):

- `cli.rs`: the redis-cli tokenizer. It is `sdssplitargs` exactly: `"…"` takes escapes and `\xHH`, `'…'` is literal except `\'`, a quote can open mid-word, and a closing quote must be followed by a space. Arguments come out as bytes. The console splitter breaks at line ends and `;` outside quotes, so a quoted value may hold both; offsets are in characters.
- `classify.rs`: the read/write classifier, default-deny.
  - The static allow-list covers the core read commands (with subcommand rules for `OBJECT`, `MEMORY`, `XINFO`, `COMMAND`, `PUBSUB`) and the read commands of RedisJSON, TimeSeries, Bloom and Search.
  - `ServerReads` parses `COMMAND` / `COMMAND INFO` in RESP2 and RESP3, including subcommand entries. It adds a command flagged `readonly` unless it is also `admin`, `may_replicate`, `noscript` or `blocking`. A command that lists subcommands reads only through a listed read subcommand, whatever its own flags; a module's `*.CONFIG` and `*.DEBUG` commands and internal `_…` commands stay writes.
  - A never-read list wins over the server's flags: `EVAL*`, `FCALL*`, `SCRIPT`, `FUNCTION`, `CONFIG`, `CLIENT`, `DEBUG`, `SHUTDOWN`, `MULTI`/`EXEC`/`WATCH`, `HELLO`, `AUTH`, `KEYS`, pub/sub, replication. Module commands that run the user's code are on it too, because their `readonly` flag describes only the keys they declare: RedisGears (`RG.*`), triggers and functions (`TFCALL`, `TFCALLASYNC`, `TFUNCTION`) and RedisAI's script, DAG and model runs (`AI.SCRIPTEXECUTE`, `AI.DAGEXECUTE[_RO]`, …).
  - Static rules also win over the flags: `SORT`, `GEORADIUS` and `GEORADIUSBYMEMBER` read unless they `STORE` (or `STOREDIST`), so a server that flags them `readonly` cannot make the storing forms reads. Option words (`STORE`, `STOREDIST`, `RESET`, `BLOCK`) are matched up to an argument's first NUL, since the server compares them with `strcasecmp`: `SORT l "STORE\x00" dst` is a write. A typed name with a `|` (`object|encoding`, the form subcommand entries are kept in) never matches the server's list.
  - `SELECT` is a read, since it changes only the console's own connection, like `USE`. `XREAD … BLOCK` is a write. A line that cannot be tokenized is a write.
  - `refused_in_console` names the commands the console must not run: blocking pops, `XREAD… BLOCK`, `WAIT`, pub/sub, `MONITOR`, `HELLO`/`AUTH`/`QUIT`/`RESET`, `CLIENT REPLY|TRACKING|CACHING`, and any command name with a NUL byte (Redis looks names up with `strcasecmp`, so `"CLIENT\x00a"` can run as CLIENT; the bootstrap check calls it first, so `"SELECT\x00a" 5` is refused there too).
- `values.rs`: total over every `redis::Value` variant in RESP2 and RESP3.
  - nil becomes NULL; integers beyond 2^53 become `BigInt`; a big number keeps its digits.
  - UTF-8 strings are text, anything else is bytes. Keys are never truncated.
  - Doubles are floats; `inf` and `nan` become `Other`.
  - A server error inside a reply becomes `Other{ty:"error"}`.
  - Nested replies become JSON: binary as `{"$base64":…}`, a map with non-text keys as pairs, exact integers.
  - A string preview that may be cut becomes `Bytes{truncated, len}`, which the grid does not edit until the whole value is loaded: saving a prefix with `SET … XX KEEPTTL` would drop the rest. The `GETRANGE` read counts as the whole value only when its length equals `STRLEN`'s, or, when `STRLEN` failed (an ACL that allows only `GETRANGE`), when it stopped short of the cap. A `STRLEN` that disagrees with the read (a write between the two) keeps it a preview.
  - TTL is in seconds from `PTTL`, rounded the way Redis's `TTL` rounds.
- `keys.rs`: key types, plus the columns of a `dbN` table: `key`, `type`, `ttl`, `size`, `memory`, `value`. **The primary key is `(key, type)`**, so `RowKey::Pk` carries the type. A plan then knows how to write the value, and the apply can prove the key was not replaced (see below). Previews of strings and containers are capped and ranged (`GETRANGE`, `LRANGE`, `SSCAN`, `HSCAN`, `ZRANGE … WITHSCORES`, `XREVRANGE`); a RedisJSON document is read whole with `JSON.GET` (see Limits). Module types show without a preview.
- `scan.rs`:
  - Cursors are validated decimal `String`s; `next_token` is `None` at 0.
  - `parse_scan` reads the replies of `SCAN`, `HSCAN`, `SSCAN` and `ZSCAN`. Keys are deduplicated within a page.
  - The filter bar maps onto `MATCH` globs. Equals is an exact key, read with `TYPE` + `PTTL` and never a SCAN. Contains, prefix and suffix are escaped literals; the `…Case` variants match either ASCII case with `[aA]`. `Like`/`Ilike` map `%`→`*` and `_`→`?`. The raw filter is the glob as typed. A type equality becomes `SCAN … TYPE`, or a filter after the `TYPE` round before 6.0.
  - A page is built from two pipelined rounds (`TYPE`+`PTTL`, then size, preview and optional `MEMORY USAGE`). Keys that are gone are dropped; failed replies become NULL cells.
- `info.rs`:
  - parses `INFO` and the keyspace lines;
  - identifies the product (Valkey, Dragonfly, Garnet, KeyDB, Memurai or Redis) and its compatible version;
  - derives the gates `KEEPTTL`, `UNLINK`, `COPY` and `SCAN TYPE` from that version;
  - reads `CONFIG GET databases` in both reply shapes, falling back to 16 and to the highest `dbN` in the keyspace, capped at 4096;
  - builds the sidebar's databases, its single schema, and one table per database with key counts.
- `reply.rs`: console replies as grids.
  - A scalar is one `value` cell; an array is `(#, value)` rows numbered from 1.
  - A RESP3 map, or RESP2 pairs (`HGETALL`, `CONFIG GET`, `XINFO STREAM`, `MEMORY STATS`, `HRANDFIELD … WITHVALUES`), is `(field, value)`.
  - `… WITHSCORES` and `ZPOPMIN`/`ZPOPMAX` are `(member, score)` in both protocols.
  - The SCAN family gives its items, with the next cursor as a notice. `INFO` gives `(section, field, value)`.
  - The row cap is applied.
- `errors.rs`: a server error keeps redis-cli's text. Bad credentials (`WRONGPASS`, `NOAUTH`, the pre-6 `ERR invalid password` forms) map to `Auth`; timeouts to `Timeout`; a dropped connection to `NotConnected`; a refused connection to `Connect`. `NOPERM`, `READONLY`, `BUSY`, `LOADING`, `OOM` and the cluster redirects get hints.
- `sqlgen/native/redis.rs`: every command as an argv plus the reply it expects (`Expect::Ok` / `Int(n)` / `Bytes(…)` with the failure message).
  - `Command::text` quotes each argument so that the redis-cli tokenizer reads it back byte for byte. A fuzz test checks the round trip.
  - Read builders: SCAN, sizes, previews, `INFO`, `CONFIG GET`, `COMMAND`, `CLIENT ID`/`KILL`.
  - Edit builders: `SET … XX KEEPTTL` (before 6.0, an atomic `EVAL` that keeps the TTL), `SET … NX [PX]`, `PEXPIRE`/`PERSIST`, `RENAMENX`, `UNLINK`/`DEL`, `COPY`, `FLUSHDB ASYNC`, and `JSON.SET`.
  - New keys of every type, with checked scores and stream ids, and the container edits: hash, list (replace or delete at an index, a delete through a unique marker), set, zset and stream.
  - A `Guard` is what the apply proves under `WATCH`, before `MULTI`: `Guard::checks` gives the reads to send and the reply each must get (`TYPE` for `Type`, `EXISTS` = 0 for `Absent`, or its own `Checks`). Redis runs the rest of a transaction after a command fails, so every change of several commands is guarded where a failed command would let the next one do harm:
    - a new key is `Absent` unless it is one command that refuses an existing key itself (`SET … NX [PX]`, `JSON.SET … NX` without a TTL). `HSET`, `RPUSH`, `SADD`, `ZADD` and `XADD` would add to the key that is there, and the `PEXPIRE` after a refused `JSON.SET … NX` would set its TTL;
    - a hash field rename (`HSETNX` + `HDEL`) checks that the old field still holds the loaded value (`HGET`) and the new name is free (`HEXISTS`), a set member rename (`SADD` + `SREM`) the same with `SISMEMBER`, and both refuse a rename to the same name;
    - a list edit by position (`LSET`, or `LSET` + `LREM` of a marker) checks that the position still holds the loaded element (`LINDEX`), since a push or pop elsewhere shifts it.
  - The builders that need a guard return `Guarded { guard, commands }`; `Guarded::edit(change)` makes it a plan's `KeyEdit`.
  - `plan_change` / `plan_changes` turn `StagedChange`s into a `KeyEdit` per change, each with a `Guard`: the type for updates and deletes, the rule above for inserts. An update writes the value, then the TTL, then renames last, so a retry repeats only idempotent writes.
  - NULL, `DEFAULT`, server expressions, read-only columns, TTL ≤ 0 and value edits of containers are refused.
  - `preview` prints `WATCH`, then `MULTI … EXEC` per guarded change.
- `testing.rs`: an in-process RESP2/RESP3 server (tokio). It implements:
  - `HELLO`, ACL `AUTH`, `SELECT`, `SCAN` (MATCH/COUNT/TYPE, uneven pages), expiry and every key type;
  - `MULTI`/`EXEC`/`WATCH` with Redis semantics (no rollback, a nil EXEC);
  - `CLIENT ID`/`KILL` and `DEBUG SLEEP` for cancel tests;
  - `INFO`/`CONFIG`/`COMMAND`;
  - version gates (5.0 lacks `HELLO`, `KEEPTTL`, `SCAN TYPE`, `COPY`), a product name, and a refused `CONFIG`.
- `tests.rs`: the codecs against the fake through redis-rs's own client.
  - The handshake in both protocols with an ACL user; bad credentials.
  - Introspection, with the fallbacks for managed services and old servers.
  - Browsing every type page by page, with RESP2 and RESP3 giving identical rows.
  - Every filter kind, including an exact key that sends no SCAN.
  - A commit with failing changes: a refused rename after its TTL edit applied, a type guard, an insert onto an existing key. Also a WATCH conflict (nil EXEC), container edits, the console in both protocols, a cancel through `CLIENT KILL ID`, and a response timeout.
  - Through the `MULTI` path of the apply: a RedisJSON insert with a TTL onto an existing RedisJSON key and onto a hash leaves both keys and their (absent) TTLs as they were and sends none of its writes; hash and set renames onto existing names, from stale or missing fields and members, and list edits at shifted positions are refused before `MULTI` and change nothing.
- Checks: `lane.mjs redis clippy --snapshot` is clean; `test-all --snapshot` passes 1140 lib tests, 77 of them the lane's.

**Left for Phase B** (after F6 and F10; part B1 did all of it but the renderer, the mock and the URL schemes, see Phase B status):

- `entry.rs`: `open` builds its own TCP or TLS stream (tokio-rustls through `tls_client::rustls_config`, SNI from `ConnectTarget::tls_host`) and hands it to `MultiplexedConnection::new_with_config`.
  - Use one connection per database index for grid reads, plus a dedicated console connection where `SELECT`, `MULTI` and `WATCH` persist.
  - Set `AsyncConnectionConfig`'s response timeout. redis-rs 1.x gives up after **500 ms** by default (connect: 1 s), which breaks long SCANs and console commands.
  - Read replies with `ConnectionLike::req_packed_command(s)`. `query_async` turns the errors nested in an EXEC reply into one `Err`, which loses the per-change results.
- `NativeSession` and `TableSource`:
  - `browse` uses `scan::*`, `count` uses `DBSIZE` (or 1/0 for an exact key), and `fetch_cell` reads the whole value with `GET` or whole containers.
  - `plan_changes` uses `sqlgen::native::redis::plan_changes` and `preview`.
  - `apply_changes` follows `tests.rs::apply`: `WATCH` the guard's key, send `Guard::checks` and compare each reply with its `Expect` (the first mismatch is the change's error and nothing else is sent), then `MULTI … EXEC`, one `RowResult` per change. Its TxMode is Batch: "no rollback on command errors". The per-type editor applies its container edits the same way.
  - Decide whether to cap the RedisJSON preview (for example a third round that reads `JSON.DEBUG MEMORY` or `MEMORY USAGE` first and skips `JSON.GET` above a threshold), or keep it under Limits.
  - The console uses `split_console`, `classify_line` (with `ServerReads` from `COMMAND` at connect), `refused_in_console`, `reply::shape` and the row cap.
  - `definition()` and `call`/`call_kind` serve the per-type inspector and the server info. Cancel is `CancelHandle::Custom`, sending `CLIENT KILL ID` from another connection.
  - Caps: ReadOnlyMode `ClassifierOnly`, `hardened` for MCP refusal.
  - Sorting can only reorder the current page; decide whether to set `caps.data.sort`.
- The renderer (key browser, per-type inspector, console mode, New key dialog), the mock, the URL schemes (`redis`, `rediss`), bindings, and the real-server suite `tests/redis.rs`.

## Phase B status

Part B1, the backend driver, is done: `entry.rs` opens a real session and the module is the driver. Part B2, the renderer, the mock and the URL schemes, is done too (see Renderer and mock below).

**What works** (`src-tauri/src/drivers/redis/`):

- `entry.rs`: `open` returns `Session::Native`; `validate` checks `RedisParams` (`driver_params`: `scanCount`, `previewBytes`, `previewElements`, `memoryColumn`, `keySeparator`, `protocol` auto / RESP2 / RESP3; ts-exported), the DB index (the Database field, `3` or `db3`) and the bootstrap commands; the form has no `util` call (`util_kind` is `Write`).
- `conn.rs`: the driver opens its own socket to `ConnectTarget::host()` / `port()` (the SSH tunnel's address when there is one) and wraps it in TLS itself (tokio-rustls over `tls_client::rustls_config`: the profile's mode, CA file and client certificate, the profile's host for SNI and the certificate check, also through a tunnel), then hands it to redis-rs's multiplexed connection, whose task it spawns and aborts when the link is dropped. `Disable` and `Prefer` are plain (Redis has no TLS negotiation); `Require`, `VerifyCa` and `VerifyFull` are TLS. A connection tries RESP3 (`HELLO 3 AUTH user pass`) and falls back to RESP2 (`AUTH`) once a server has no `HELLO` (unknown command, `NOPROTO`); later connections skip the try. Command Line passwords go through `CommandPassword::for_profile_ttl` (the descriptor's 600 s): every new connection asks it, so a token older than the TTL is printed again. The connect is bounded by the profile's Connect timeout; the app's own reads and writes wait 60 s for a reply (redis-rs's default is 500 ms), the console has no reply limit of its own (the query time out cancels it). `CLIENT SETINFO LIB-NAME tabler` names the connections.
- Bootstrap commands (`startup_sql`, one per line or `;`) run on every connection after login, in the round trip of its `CLIENT ID`; a failing one fails the connect with its text. They may not `SELECT`, `SWAPDB`, open a transaction or run what the console refuses, and on a read-only profile each must read (by the static allow-list: the server's flags are not known yet).
- `session.rs`: connections per role. One per database for the grid's reads (pipelined and shared, opened when a database is first read, at most 8, the least recently used closing; the profile's database's also answers the introspection and sends cancels), one for edits (`WATCH` holds per connection, so one apply at a time uses it and it `SELECT`s the database it writes), and one for the console, where `SELECT`, `MULTI` and `WATCH` persist. A grid read that finds its connection lost runs once more on a new one; `ping` too (a server's idle `timeout`). `close` aborts every connection's task at once.
- Connect reads `INFO`, `CONFIG GET databases` and `COMMAND` in one round trip: the product and version for the status pill (`Valkey 8.0.1`), the version gates, the number of databases (CONFIG refused: 16, or past the highest `dbN` in the keyspace; a cluster node: 1) and the server's read commands for the classifier. A Sentinel is refused; a DB index the server lacks fails the connect.
- Sidebar: `SchemaMode::Single` with the labels Database / Databases: every database is one table of keys (`db0`, `db1`, …) with its key count from `INFO keyspace` and how many keys have a TTL; `definition()` gives the database's keys, TTLs, server, mode and protocol as JSON (the structure view).
- `grid.rs`, the key browser (`TableSource`): `browse` sends SCAN steps from the page's cursor (`MATCH` from the filter bar, `TYPE` from 6.0, `COUNT` the `scanCount` param or the page size) until it holds a page of keys, the keyspace ends or 0.5 s passed, deduplicates them, then reads `TYPE` + `PTTL`, and the size, preview and `MEMORY USAGE` (the server's sampled estimate), in two pipelined rounds; `next_cursor` is the server's cursor, `None` at 0. An exact key (`key = x`) sends no SCAN. `count`: `DBSIZE` without a filter, `EXISTS` (and `TYPE`) for an exact key; a pattern or a type scans the whole keyspace when asked for an exact count, otherwise for 0.3 s and extrapolates from the share it saw (`exact: false`), or reports exact when the scan ended in time. `fetch_cell` checks the key still has the row's type, then reads the whole value (`GET`; a container in the preview's form, when it has at most 100,000 elements), the TTL, the size, the exact memory (`SAMPLES 0`) or the type. A cancel stops the read waiting (the grid's connection is shared, so it is not killed).
- `apply.rs`: `plan_changes` is `sqlgen::native::redis::plan_changes` and `preview`; `apply_changes` takes the edit connection and, per change, `WATCH`es its guarded key and sends the guard's checks (the first that answers otherwise is the change's error, nothing of it is sent, then `UNWATCH`), then `MULTI` on its own and, once it is answered OK, its commands and `EXEC` (a nil EXEC is "changed by someone else", `EXECABORT` the server's error); a single unguarded command goes as it is. What is sent is what the preview shows. A `WATCH` or `MULTI` the server refuses (an ACL user without `@transaction`, a proxy) refuses the change with none of its commands sent: pipelined, they would have run at once, unguarded and outside the transaction, and a retry would repeat a push. A refused `EXEC` (not `EXECABORT`) leaves the connection inside `MULTI`, so it is not used again. One `RowResult` per change; a connection lost once its commands were sent reports the change as "may or may not have been applied" and the later ones as not applied; lost before (its `WATCH`, its `MULTI`), the change is "not applied". The edit connection `SELECT`s the database of each apply; a `SELECT` that is not answered in time (the server may still run it) or answered oddly drops the connection, so a later edit never runs in a database it did not choose, and the edits are "not applied". After anything that leaves a connection's database or transaction unknown, the next change opens a new one.
- Grid inserts take the rows the renderer stages: a new row's cells it was not given (`Other{missing}`) and `DEFAULT` are no value, the server's `size` and `memory` that a duplicated or pasted row copies are ignored, a type left out is a string, and a row without a key name (missing, NULL or empty) is refused with "a new key needs a name".
- `console.rs`: one command per line or `;` (`cli::split_console`), tokenized like redis-cli, classified with the server's read commands (`classify_line`), refused when `refused_in_console` says so, sent as argv on the console's connection and shaped by `reply::shape` with the row cap (`QUEUED` inside MULTI, an EXEC's replies as rows; no cell past the cap is built, so a huge `LRANGE` costs its reply once, not again in cells). `SELECT` changes the console's database once the server answers it; one queued inside `MULTI` once its `EXEC` ran it (the `SELECT`'s slot in the `EXEC` reply is OK), not after a `DISCARD` or a nil `EXEC`. `MULTI` / `EXEC` / `DISCARD` drive `in_transaction`. Whenever the console's connection is opened again (after a cancel, a kill or the server's idle time out), the next command's notice names the database it is on, and says when a MULTI was lost with the old one. `console_lock` keeps a run's commands together.
- Cancel: `CancelHandle::Custom` (`conn::KillAction`, started with `cancel_portable`): the run stops waiting at once and, for the console, `CLIENT KILL ID` goes out on another connection (best effort, 2 s) and the console's connection is dropped; the next command opens a new one on the database the console had selected, with a notice naming it. The query time out reaches it through the same cancel.
- `calls.rs`, the session's calls (ts-exported argument and reply types, `Redis*`): reads `serverInfo` (product, version, mode, protocol, databases, every `INFO` section), `keyPage` (a page of a key's elements: `HSCAN` / `SSCAN` by cursor, a list or sorted set by position, a stream from its newest entry back by id; a string's preview or a RedisJSON document; refused when the key's type changed), `slowLog` (`SLOWLOG GET`) and `previewEdits` (sends nothing); writes `editKey` (the value editor's edits of hashes, lists, sets, sorted sets, streams, RedisJSON and strings, each behind the key's type or its own checks, through the grid's apply, a result each), `newKey` (every type, with a TTL, refused onto an existing key), `copyKey` (`COPY`, 6.2) and `flushDb` (`FLUSHDB ASYNC`). Unknown calls are writes.
- `clients.rs`: `list_processes` is `CLIENT LIST` (id, user, database, client name or library, address, flags, last command, idle time; the session's own connections marked), `kill_processes` is `CLIENT KILL ID` per id.
- Read-only (`conn_options::effective_read_only`) is enforced by the session itself, whatever its caller checked: `plan_changes` and `apply_changes`, write commands in the console (by the classifier, default-deny), write calls, kills and bootstrap commands that write are refused with "This connection is read-only". `SafetyCaps.read_only` is `ClassifierOnly`. Unattended (`registry::unattended()`, the MCP process) the driver refuses to open, as MCP refuses native stores in v1; `hardened` stays false, since no session runs unattended.
- Classifier hardening in B1: a module's internal commands stay writes whatever their flags also when the `_` follows the module's dot (`FT._ALTERIFNX`, `FT._DROPIFX`), and `PFCOUNT` is on the never-read list (it stores the cardinality it computes in the key; Redis before 6.2 flags it plain `readonly`).
- The server's flags never make a module's command a read (this replaces Phase A's rule above). Modules flag commands `readonly` that change data: RediSearch 2.6 to 2.10 (Redis Stack 6.2.6 to 7.4) registers `FT.ALIASADD`, `FT.ALIASUPDATE`, `FT.ALIASDEL`, `FT.DICTADD` and `FT.DICTDEL` so, RedisBloom registers `CF.COMPACT` (which rewrites the filter and replicates) `readonly fast`, and `FT.CURSOR` (`DEL`, `GC`) and `GRAPH.SLOWLOG` (`RESET`) are flagged `readonly` as a whole. So `ServerReads` keeps only core commands: an entry that is dotted, internal (`_…`), flagged `module` (Redis 7 sends that flag) or a subcommand of one of those adds nothing. Entries older than Redis 7 (fewer than ten fields) add nothing either, since nothing in them marks an undotted module command; the static list covers every core read of Redis 6 (it gained `STRALGO LCS` and `LOLWUT` for that). The server's flags therefore add only core reads newer than the list. A module's read must be on the static list, which now also has RediSearch's `FT.PROFILE`, `FT.EXPLAINCLI`, `FT.SPELLCHECK`, `FT.TAGVALS`, `FT.SUGGET`, `FT.SUGLEN`, `FT.SYNDUMP`, `FT.DICTDUMP` and `FT.CURSOR READ`, RedisBloom's `BF.SCANDUMP`, `CF.SCANDUMP` and the `CMS`, `TOPK` and `TDIGEST` reads, and `GRAPH.SLOWLOG` without `RESET`. The six writes above are also on the never-read list. `classify.rs` tests each of them with the flags those modules send, with and without `module`, and `a_read_only_profile_refuses_every_write_path` runs them on a read-only console against a fake whose `COMMAND` flags them `readonly`: each is refused and none reaches the server.
- `mod.rs` had no `#![allow(dead_code)]`; the new modules are private where only the session uses them.

**Caps granted** (`SessionCaps::minimal(Redis)` plus), each with the test that proves it (`src-tauri/src/drivers/redis/session_tests.rs` unless named):

| Cap                                                                                                  | Why                                                                                                                                              | Test                                                                                                                                                                                                                                                                                                             |
| ---------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `StructureView`                                                                                      | the sidebar, `get_table`, the key browser                                                                                                        | `connects_in_both_protocols_and_lists_the_databases`, `browses_page_by_page_counts_and_fetches_cells`                                                                                                                                                                                                            |
| `Editor`                                                                                             | the console                                                                                                                                      | `the_console_keeps_its_database_and_transaction`, `the_console_says_where_it_reopens_after_a_select_in_exec`                                                                                                                                                                                                     |
| `EditRows`, `InsertRows`, `DeleteRows`                                                               | grid edits by `(key, type)` behind guards, a result per change                                                                                   | `a_commit_reports_each_change_and_one_fails`, `grid_inserts_take_the_rows_the_renderer_stages`, `a_server_refusing_transactions_gets_no_unguarded_writes`, `an_edit_connection_whose_select_timed_out_is_not_used_again`, `injection_stays_inside_the_arguments`, Phase A's `edits_apply_per_change_with_guards` |
| `ExactCount`                                                                                         | `DBSIZE`, `EXISTS`, a full SCAN when asked                                                                                                       | `browses_page_by_page_counts_and_fetches_cells`                                                                                                                                                                                                                                                                  |
| `ViewDefinition`                                                                                     | `definition()` in the structure view                                                                                                             | `connects_in_both_protocols_and_lists_the_databases`                                                                                                                                                                                                                                                             |
| `ProcessList`, `KillProcess`                                                                         | `CLIENT LIST`, `CLIENT KILL ID`                                                                                                                  | `the_process_list_shows_and_kills_clients`                                                                                                                                                                                                                                                                       |
| `StartupCommands`                                                                                    | bootstrap commands on every connection                                                                                                           | `command_passwords_and_bootstrap_commands_on_every_connection`, `conn::tests::bootstrap_commands_are_checked`                                                                                                                                                                                                    |
| `Paging::Cursor`, `CountMode::Estimate`, `TxMode::Batch`, `Affected::Exact`                          | SCAN cursors; DBSIZE is free, a filtered count is sampled; each change in its own MULTI … EXEC; a change applies to the one key its guard proved | `caps_grant_what_the_session_implements` and the tests above                                                                                                                                                                                                                                                     |
| filter ops `=`, `LIKE`, `ILIKE`, contains, prefix, suffix and their case-insensitive forms; `type =` | the filter bar as `MATCH` globs, an exact key, `SCAN … TYPE`                                                                                     | `browses_page_by_page_counts_and_fetches_cells`, Phase A's `filters_scan_by_pattern_type_or_exact_key`                                                                                                                                                                                                           |
| keep-alive 30 s, ping not billed                                                                     | `PING` on the grid connection                                                                                                                    | `connects_in_both_protocols_and_lists_the_databases`                                                                                                                                                                                                                                                             |

Not granted: `CancelOnServer` (a running command finishes on the server), sort (SCAN has no order), a raw filter (no `RawFilter` kind takes a glob: a change request), `Explain`, `Format`, `EditWithoutKey`, `ServerExpressions`, `Returning`, `WorkspaceCommit`, `SwitchDatabase` (every database is listed at once), `TableDdl`, `ExportTables`, the imports, `Backup` / `Restore`, `Users` (ACL), `SessionDetails`, `Metrics` and `Mcp`.

**Tests**: `node dev/scripts/lane.mjs redis test --snapshot` runs the lane's lib tests: the codecs, builders, classifier and tokenizer of Phase A; params, bootstrap commands, the abort, the calls and `CLIENT LIST` parsing; and the session on the fake (connect in RESP3 and RESP2 with ACL auth and wrong credentials, CONFIG refused, the sidebar, cursor pages, counts, whole cells, a commit with one failing change, inserts shaped like the grid's (a new row, a duplicated row), an ACL that refuses `WATCH`, `MULTI` or `EXEC` (no write is sent unguarded), an edit connection whose `SELECT` timed out (not used again), the console with SELECT and MULTI (a `SELECT` run by `EXEC`, the notice when its connection is opened again), the value editor's calls, the process list, a cancel, the console's query time out and a grid read's, TLS with a CA, a client certificate and a tunnel's address, Command Line passwords, bootstrap commands, every write path refused read-only, injection through keys and values). The fake (`testing.rs`) gained TLS, `CLIENT LIST`, `SLOWLOG GET`, slow commands and commands an ACL refuses (`FakeRedis::deny`); its `COMMAND` table flags module writes `readonly` the way Redis Stack does. `src-tauri/tests/redis.rs` runs the same story against `TABLER_TEST_REDIS_URL` (keys under a prefix of its own, deleted at the end; cancel and time out only where `DEBUG` is enabled), including the grid's insert payloads, verified TLS with `rediss://…?verify=full[&ca=<path>]` (and, with a CA, the refusal without it), and a read-only profile's refusals on every write path (console, including the module writes Redis Stack flags `readonly`, `plan_changes`, `apply_changes` of a writable session's plan, the write calls, a kill, a writing bootstrap command), each naming a key under its prefix, then checks nothing was written; it skips without the variable. Totals: `lane.mjs redis test --snapshot` passes the lane's 109 lib tests, `clippy --snapshot` is clean and `bindings --snapshot` writes the 20 `Redis*.ts`; `test-all --snapshot` passes all 1306 lib tests, none failing.

**Renderer and mock** (part B2; `src/features/drivers/redis/`, `src/lib/ipc/mock/drivers/redis/`):

- `index.ts` gives the `redis` kind's form and the `redis` driver's console, `Inspector` and `objectMenu`. The key browser is the app's grid (the `(key, type)` rows, SCAN cursor pages and the read-only `type`, `size` and `memory` columns fit it), so there is no `TableView`; the Structure tab shows `definition()`, so no `StructureView`; no SQL dialect.
- The form (`form.ts`, `params.ts`, `RedisFields.tsx`): host and port 6379, the ACL user, the password (all of `PasswordMode`, Command Line for IAM or Entra tokens), the DB index, TLS as Off / On / On verifying the CA / On verifying the certificate and host (no Prefer: the backend reads it as plain), the CA and client certificate, and the `RedisParams` rows (protocol, SCAN count, preview bytes and elements, key separator, memory column); only what differs from the defaults is saved. `validate` says what `entry::validate` refuses, in its words: the params' bounds, the DB index (`3` or `db3`, 0 to 4095) and the bootstrap commands (unreadable, refused in the console, `SELECT` / `SWAPDB` / a transaction, a write on a read-only profile).
- URLs (`url.ts`): `redis://[[user]:password@]host[:port][/db]` and `rediss://` for TLS (`verify=full|ca|none` or redis-py's `ssl_cert_reqs`), `db=`, `protocol=2|3`, `socket_connect_timeout` and the lane's params; the password goes to the secret, and so does userinfo without a `:` (redis-cli's legacy `redis://password@host`), never the user field; `format` writes the same keys and the common query (`profileUrlQuery`), a user as `user:@` so it reads back as a user, never a password, and every profile reads back to itself (Prefer is written as plain `redis://`).
- The console (`cli.ts`, `classify.ts`, `console.ts`): the tokenizer, the splitter and the static classifier of `cli.rs` / `classify.rs` rule for rule (bytes, `\xHH`, the NUL rule of option words, Rust's whitespace), `refusedInConsole`, a StreamLanguage highlighter, `.redis` files, no formatter. Both sides run `src-tauri/tests/fixtures/redis/console.cases.json` (311 classify and 13 split cases): `drivers::redis::classify::tests::shared_console_cases` and `console.test.ts`. The renderer does not know the server's `COMMAND` flags, so a core read newer than the static list asks Safe Mode as a write (the backend then runs it as a read).
- The inspector (`RedisInspector.tsx`): a string or RedisJSON document as text, staged with `onStage` like a grid edit (a preview-only value says so instead; a document that is not JSON cannot be staged); the TTL, staged (empty for no expiry); a hash, list, set, sorted set or stream a page of 100 at a time through `keyPage` (a read, with Load more), each element deleted and new ones set, pushed, added, scored or appended one edit at a time through `editKey`, after `guardExecution` with the commands `previewEdits` returns; Copy to… (`copyKey`, Safe Mode as an operation). Without `onStage` (read-only) it only reads. A module's type shows without a value.
- The database menu (`menu.ts`): Server Info and Slow Log through `ctx.read`; New Key… (a dialog for every type with its first value and TTL, `NewKeyDialog.tsx`, `newKey.ts`) and Flush Database… (confirmed with the command it sends: `FLUSHDB ASYNC` from Redis 4, plain `FLUSHDB` before, read from `serverInfo`'s `redis_version` as `Server::gates` decides) through `ctx.call`, disabled on a read-only connection.
- The mock (`?mockDrivers=redis`): three profiles (db0, db1, read-only) on one shared in-memory server (`data.ts`: 16 databases, every key type, TTLs that do not run down, clients, a slow log). `session.ts` has the backend's caps exactly, the databases as tables with key counts and TTL comments, `(key, type)` rows with the backend's columns and capped previews (a 40 KB string arrives truncated), SCAN cursor pages and the filter bar as `scan::key_filter` maps it (globs, an exact key without SCAN, `TYPE`, unsupported filters refused), exact counts, whole cells refused when the type changed, and `definition()`. `plan.ts` plans and applies like `sqlgen::native::redis` and `apply.rs`: the same preview lines (`WATCH`, `MULTI` … `EXEC`), guards proved first, every command of a change run (no rollback) and its first failure reported, one result per change. `engine.ts` answers about 90 commands with Redis's replies (`WRONGTYPE`, unknown command); `console.ts` refuses writes on a read-only connection and what the console does not run, keeps `SELECT` and `MULTI` … `EXEC` on its own state, and shapes replies as `reply.rs`. The calls (`serverInfo`, `keyPage`, `slowLog`, `previewEdits`, `editKey`, `newKey`, `copyKey`, `flushDb`, unknown ones refused; default-deny like `call_kind`, so on a read-only session every call but the four reads, an unknown one too, gets the read-only refusal first) and the clients as the process list mirror `calls.rs` and `clients.rs`; a read-only session refuses every write call and plan with "This connection is read-only".
- Tests: `lane.mjs redis ts` is clean (tsc, eslint, prettier, 447 vitest tests with the smoke files): `console.test.ts` (the shared cases, the tokenizer and quoting round trip, offsets, refusals, a fuzz), `form.test.ts` (round trip, defaults, validate messages, summary), `url.test.ts` (both schemes, every key, the legacy `password@` kept out of the user, refusals, format round trip), `fields.test.tsx`, `views.test.tsx` (the module's slots, the inspector's staging, paging and Safe Mode, the menu and its FLUSHDB by server version) and the mock's `index.test.ts` (connect and caps, the sidebar, cursor pages without a lost or repeated key, filters, counts, cells, a commit with one failing change, the console with SELECT and MULTI, the calls, a leading BOM kept as a byte by the console, `GETRANGE` and a bytes cell edit, the clients, every read-only refusal including an unknown call).

**What only a real server can confirm**: `HELLO` / `AUTH` on Redis 5, 6, 7 and 8, Valkey, KeyDB, Dragonfly, Garnet and Memurai; ACL users with restricted commands (`NOPERM` on `CLIENT ID`, `INFO`, `COMMAND`, `CONFIG`, `MEMORY`, `CLIENT LIST`); ACL users without `@transaction` (the refusals of `WATCH`, `MULTI` and `EXEC` and what a proxy answers); managed services that refuse CONFIG or INFO (ElastiCache, Upstash) and TLS on Azure's 6380; verified TLS against a managed service's certificate; the real `COMMAND` table in the classifier (Redis 7 and 8 with modules loaded: their entries flagged `module`, the core reads the flags add); `CLIENT KILL` while the server is busy (it is single-threaded, so the kill waits for the running command); `MEMORY USAGE` and `SLOWLOG` replies; RedisJSON's `JSON.SET … NX` / `XX` and `JSON.GET`; SCAN on large keyspaces (how full pages get within their time, the count estimate).

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._

- Standalone servers only: no Cluster (MOVED/ASK are errors with a hint) and no Sentinel.
- The console does not run blocking commands, pub/sub, `MONITOR`, or commands that change the connection itself.
- Grid edits change the value of strings and RedisJSON documents only. Container elements are edited in the per-type editor, and module types are read-only.
- The value preview of a RedisJSON key reads the whole document (`JSON.GET`), because RedisJSON has no ranged read of a document: a page of large documents is slow and loads every document whole, unlike strings and containers, which are always read ranged.
- No sort (`caps.data.sort` is off): SCAN has no order, and it may return a key twice or return empty pages.
- A page holds the page size unless the scan ended or its 0.5 s passed, and may hold a few keys more: a SCAN step's matches are taken whole (its cursor cannot resume in the middle). A key found twice shows once per page; it may still show again on a later page.
- A filtered count is an estimate from 0.3 s of the keyspace unless an exact count is asked for, which scans all of it.
- No cancel on the server: a cancel stops waiting and drops the console's connection (its MULTI and WATCH go with it), and `CLIENT KILL` ends it on the server, but a command already running (a script, `KEYS`, a large `SMEMBERS`) finishes there and a busy server answers the next command late. Scripts are not killed: `SCRIPT KILL` would stop another client's script too. Kill Query and Kill Connection both end the connection. A console connection opened again is on the database the last `SELECT` that ran chose (also one inside `MULTI` … `EXEC`), which its notice names.
- `fetch_cell` loads a container whole only up to 100,000 elements; a larger one is paged in the value editor (`keyPage`). The elements `keyPage` returns are not cut (a 50 MB hash field arrives whole).
- TLS mode `Prefer` is plain: Redis has no TLS negotiation.
- No raw filter: the filter bar's LIKE, contains, prefix and suffix cover globs (`*` in a contains filter matches literally).
- Bootstrap commands cannot `SELECT` (the DB index field does), open a transaction or run what the console refuses; on a read-only profile they must read.
- MCP refuses Redis (native stores, v1), and the driver refuses to open unattended.
- `KEYS` is classified as a write (it blocks the server). Read-only connections use SCAN through the key browser. `PFCOUNT` is a write too (it stores the cardinality it computes in the key).
- A module's command reads only when the static list names it (RedisJSON, TimeSeries, Bloom, Search and Graph reads): the server's flags are not trusted for modules, which flag some writes `readonly`. Any other module command is a write, so Safe Mode asks before it and a read-only connection refuses it. Before Redis 7 the server's flags add nothing.
- Edits need `WATCH`, `MULTI` and `EXEC` (`@transaction`): for an ACL user without them, every guarded change is refused with nothing sent, rather than run unguarded. Single commands that need no guard (a new string key, `copyKey`, `flushDb`) still run.
- Redis error codes are words, so they stay in the message text; `vendor_code` stays empty.
- No namespace tree yet: the grid lists a database's keys flat, in SCAN order. The key separator is saved with the profile (`keySeparator`) for it, but nothing uses it yet.
- The inspector edits a container one element at a time and offers delete, set (hash), push to the tail (list), add (set), add or score (sorted set) and append (stream, one field). The backend's other edits (`hashRename`, `setRename`, `listSet`, `listInsert`, `jsonSet` / `jsonDelete` of a path, a stream entry with its own id) are reachable through `editKey` but have no control in the inspector; a RedisJSON document is replaced whole.
- Safe Mode in the renderer classifies with the static list only (it does not know the server's `COMMAND` flags), so a core read newer than the list asks as a write.
- The mock answers RESP2-shaped replies for about 90 commands, its TTLs do not run down, and a read-only connection refuses a console write command by command (the backend refuses the whole run before any of it).
