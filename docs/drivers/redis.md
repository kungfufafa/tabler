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

| Engine | When read-only                                                                                                                                                                                                                   | When unattended (MCP) |
| ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------- |
| Redis  | Allow-list: `COMMAND INFO` `readonly` flag minus `admin`, `may_replicate`, `noscript`, `blocking`, plus a static list (GET, MGET, HGETALL, SCAN, TYPE, TTL, …). EVAL*, FCALL, SCRIPT, CONFIG, CLIENT, SHUTDOWN, DEBUG are writes | MCP refused (native)  |

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
- Read-only is an allow-list: the `readonly` flag of `COMMAND INFO` minus `admin`, `may_replicate`, `noscript` and `blocking`, plus a static list; `EVAL*`, `FCALL`, `SCRIPT`, `CONFIG`, `CLIENT`, `SHUTDOWN` and `DEBUG` are writes (plan §5.3).
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

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._
