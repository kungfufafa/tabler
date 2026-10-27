# Remote SQLite (LibSQL and Cloudflare D1): lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                                                      |
| ----------------- | ------------------------------------------------------------------------------------ |
| Lane              | `sqlite-remote`                                                                      |
| Databases         | LibSQL, Cloudflare D1                                                                |
| Drivers           | `libsql`, `d1`                                                                       |
| Picker kinds      | `libsql`, `d1`                                                                       |
| SQL dialect       | `sqlite` (core builders)                                                             |
| Cargo features    | `drv-libsql`, `drv-d1`                                                               |
| Home slot         | `s1`                                                                                 |
| Bindings prefix   | `Libsql*`, `D1*`                                                                     |
| Lib test filters  | `drivers::sqlite_http`                                                               |
| Real-server suite | `tests/libsql.rs`, `tests/d1.rs` with `TABLER_TEST_LIBSQL_URL`, `TABLER_TEST_D1_URL` |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/sqlite_http/**`
- `src-tauri/tests/libsql.rs`
- `src-tauri/tests/d1.rs`
- `src-tauri/tests/fixtures/libsql/**`
- `src-tauri/tests/fixtures/d1/**`
- `src/features/drivers/sqlite-remote/**`
- `src/lib/ipc/mock/drivers/sqlite-remote/**`
- `src/lib/ipc/bindings/Libsql*.ts`
- `src/lib/ipc/bindings/D1*.ts`
- `docs/drivers/sqlite-remote.md`

Anything else (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead. `node dev/scripts/lane-commit.mjs sqlite-remote -m "sqlite-remote: …"` refuses other paths.

## Scope of v1

### LibSQL

- **Identity:** `libsql`, `SqlDialect::Sqlite` (core builders). Crates: none (reqwest, base64).
- **UI:** SQL grid.
- **Auth v1:** auth token (Bearer) in DbPassword, Command (`turso db tokens create`), optional basic auth.
- **Required fields:** host, or params `url` (8080; 443 with TLS, which is https).
- **Out of scope v1:** embedded replicas, WebSocket Hrana, DDL editing, ATTACH, VACUUM INTO.
- **Test:** in-process Hrana fake over rusqlite (v2/v3 pipelines, cursor, 10 s expiry, 401/404). Opt-in: Turso free tier.

### Cloudflare D1

- **Identity:** `d1`, `SqlDialect::Sqlite`. Crates: none.
- **UI:** SQL grid.
- **Auth v1:** API token (Bearer), Command.
- **Required fields:** params `accountId`, database (UUID or name), token.
- **Out of scope v1:** DDL editing, export API backup, SSH, time-travel.
- **Test:** fake REST over rusqlite with an authorizer (`hooks` dev feature): refuses BEGIN, ATTACH and non-whitelisted PRAGMAs; 100 params, 100 KB, 429. Opt-in: `CLOUDFLARE_*` account.

## Descriptor values

Seeded from the plan (§5.2). The values in force are the descriptors in `src-tauri/src/types/driver.rs` (keepalive belongs to the session's caps); a change is a request to the lead.

| Driver | Location | Port (TLS) | SSH    | TLS style | Secret label | Command TTL |
| ------ | -------- | ---------- | ------ | --------- | ------------ | ----------- |
| libsql | Host     | 8080 (443) | Tunnel | Https     | AuthToken    | 3000        |
| d1     | Cloud    | —          | None   | Https     | ApiToken     | 3000        |

| Driver | Required fields / params | Label overrides  | Keepalive | Read-only      | Sharing | Token secret |
| ------ | ------------------------ | ---------------- | --------- | -------------- | ------- | ------------ |
| libsql | host                     | database: —      | None      | PerRequest     | Indep   | yes          |
| d1     | database / `accountId`   | user: Account ID | None      | ClassifierOnly | Indep   | yes          |

## Read-only and hardening

| Engine                                                                            | When read-only                                                                                                      | When unattended (MCP)      |
| --------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- | -------------------------- |
| LibSQL                                                                            | `PRAGMA query_only = 1` in every stream prelude                                                                     | —                          |
| D1, SQL Server (ApplicationIntent=ReadOnly as routing only), Snowflake, Cassandra | ClassifierOnly, default-deny. T-SQL uses `terminator_optional`; Snowflake `EXECUTE IMMEDIATE` and `CALL` are writes | MCP only if caps grant Mcp |

Every classifier is default-deny: a statement or command it does not know is a write. The read-only mode in the caps (`Server`, `PerRequest` or `ClassifierOnly`) must say what the session really enforces.

## What to implement

- `entry.rs` (Phase B): `open`, `validate`, `util`, `util_kind` (plan §2.5). `<Prefix>Params` (`#[serde(rename_all = "camelCase", default)]`, ts-exported) parsed from `options.driver_params`.
- `DbSession`: the 18 required methods; overrides of `query_cancellable`, `execute_cancellable`, `drain_trace`, `sql_trace` and `editor_run_lock`; `dialect()`, `capabilities()` (accurate features, data, safety and lifecycle caps) and `flavor()` where it applies.
- Optional: `table_source()`, `call` / `call_kind`, `list_processes` / `kill_processes`, `table_ddl`, `explain_form`.
- Cancel through `CancelHandle::Custom(Arc<dyn CancelAction>)`; `cancel()` calls `cancel_portable` first.
- Renderer: a `KindUi` per tile, `DriverUi`, `DialectUi` and a `MockDriver`. Write-capable `sessionCall` / `driverUtil` helpers await `guardExecution` first.

## Decisions and corrections for this lane

- The module is `drivers/sqlite_http` (with `libsql/` and `d1/`), not `tunnel::sqlite_remote`, which is a SQLite file copied over SSH (critique optional 1). Each driver has its own feature: `drv-libsql`, `drv-d1`.
- Both report `SqlDialect::Sqlite` and use the core SQLite builders for browse and edits. Their `core_reuse` allow-list is empty, so DDL rebuild, backup and the other core-only features stay off: the SQLite rebuild contract cannot hold over HTTP (plan A7).
- Fetched cells are decoded by the dialect the query was built for (done in F2); rowid identity (`identity_for`) is the lane's decision (critique A6).
- The affected-rows guard must hold before commit. D1 refuses `BEGIN` and its `/raw` batch commits before `meta.changes` is visible, so D1 edits use primary-key identity only, with EditWithoutKey and all-columns edits off (critique D1). D1 allows 100 bound parameters: bound `build_where` by `caps.data.max_params` (critique optional 9).
- LibSQL: offer Turso read-only tokens as server-side read-only instead of `PRAGMA query_only` where possible (critique optional 11).
- Both secrets are tokens (`secret_is_token`), so Safe Mode confirms with the app passcode (critique A14). The D1 fake's authorizer needs rusqlite's `hooks` feature as a dev-dependency: a change request to the lead.

## Phase A status

Done (all in `src-tauri/src/drivers/sqlite_http/`, `pub` items under a temporary `#![allow(dead_code)]` in `mod.rs`; nothing touches `entry.rs`):

- **Shared by both drivers.** `transport.rs`: the `Transport` seam (`send`, `send_streaming`) the clients use, which F8a's `HttpClient` implements in Phase B; status mapping (401/403 → `Auth`, 404 → `NotFound`, 408/504 → `Timeout`, 429 → `Db` with the `Retry-After` hint, 502/503 → `Connect`) and plain-text excerpts of HTML or text bodies. `catalog.rs`: the SQLite catalog as `(sql, params)` builders plus parsers over `Vec<Vec<Value>>` (table-valued PRAGMA functions, no formatted identifiers; tables, table metadata in one round trip of 5 statements, view definition, completion in one statement, `sqlite_version()`), with each engine's hidden tables (`libsql\_%`, `\_litestream\_%`, vector-index `<index>_shadow`; `\_cf\_%`, keeping `d1_migrations`). `guard.rs`: the `changes()` guard of staged commits, whose failure names the statement and both counts and reads like `check_affected`. `value.rs`, `codes.rs`: declared-type kinds, storage class wins, base64 either way, SQLite code names ↔ `vendor_code`.
- **LibSQL.** `hrana.rs`: Hrana 2/3 JSON types (pipeline, batch with conditions, `get_autocommit`, `describe`, cursor lines), lenient where servers differ, total otherwise. `decode.rs`: values both ways (integers as strings → `BigInt` beyond 2^53, blobs base64 without padding, non-finite floats). `url.rs`: `libsql://`, `wss://`, `ws://`, `?authToken=`, `?tls=0`, basic credentials and IPv6 out of pasted URLs. `error.rs`: Hrana errors with SQLite's code in `vendor_code`; a dropped stream (`STREAM_EXPIRED`, invalid baton) says the transaction was lost. `client.rs`: version probe (`GET v3`, then `v2`), every unit of work as one Hrana batch with the prelude (when read-only `PRAGMA query_only = 1`, then `PRAGMA foreign_keys`, the startup statements, and `PRAGMA query_only = 1` again after them, so a startup statement can neither write nor turn it off) first and the work conditioned on it, worker streams closed in the same request, the editor's held stream (baton, a `base_url` followed only on the profile's scheme, host and port, `get_autocommit`, a cancelled request drops the stream, `take_lost`), catalog in one pipeline, `transaction` as `BEGIN` / chained steps / guards / `COMMIT` / `ROLLBACK` in one batch, the `v3/cursor` reader.
- **D1.** `api.rs`: endpoints (`/raw`, list, get; path segments encoded), bodies, the Cloudflare envelope, error codes (10000/9106/9109/7403 → `Auth`, 7404 → `NotFound`, 971/429 → rate limit, SQL errors with `: SQLITE_*` → `vendor_code`, `at offset N` → position). `decode.rs`: cells as D1's JavaScript sends them (whole REALs back to floats by the column kind, byte arrays to bytes, canonical integer text in an INTEGER column → exact `BigInt`), parameters native or as text, blobs refused, 100 parameters and 100 KB checked before sending. `batch.rs`: staged commits as one atomic batch with guards; once D1 has committed it, the outcome is never an error: a guarded statement reports its expected count (the guard proved `changes()` equal), the others `meta.changes`, which is the `sqlite3_total_changes()` delta and so also counts rows changed by triggers and cascades. `client.rs`: query, execute, catalog batch, transaction, database details, paged list and name → UUID resolution.
- **Fakes and tests.** `libsql/testing.rs`: a Hrana server on rusqlite (one connection per stream on a WAL file, batons, conditions, cursor NDJSON, expiry, `base_url`, canned 401/404/HTML failures, delayed replies). `d1/testing.rs`: the D1 REST API on rusqlite (paged list, atomic batches, D1's refusals and limits, JavaScript-style numbers, `meta.changes` as the `total_changes` delta). `libsql/tests.rs` and `d1/tests.rs` run the clients against them, and through reqwest against `testkit::fake_http` for the wire format (bearer only when a token is set, JSON bodies, a cursor split across chunks).

Left for Phase B: `entry.rs` (`open`, `validate`, `util`, `LibsqlParams` / `D1Params`), the two `DbSession`s on these clients (worker vs editor stream, `TxState` from `get_autocommit` or `tx_effect`, the lost-transaction notice from `take_lost`, cancel through `CancelHandle::Custom` dropping the request, `max_rows` over the cursor), the transport on `HttpClient` (SSH `resolve` for LibSQL), caps (LibSQL `PerRequest`, D1 `ClassifierOnly`, D1 primary-key identity only), the renderer and mock, the real-server suites, removing `#![allow(dead_code)]`.

## Research notes

> Background from the driver research (October 2026), with file references and no line numbers. Verify against the code; where it differs from the scope above or from README.md, those win.

### LibSQL (Turso / sqld)

**Approach.**

Hand-roll a Hrana-over-HTTP client on the existing reqwest 0.13 (rustls + aws-lc-rs, tokio, `stream` feature already enabled in src-tauri/Cargo.toml) plus serde_json and base64 0.23 (both already dependencies). Do NOT use the `libsql` crate. Its latest is 0.10.0 (2026-10-13), with 0.9.30 (2026-03-19) before it, and even with only `remote` it pulls hyper ^0.14, hyper-rustls ^0.25 (rustls 0.22), tower ^0.4, http ^0.2, base64 ^0.21 and libsql-sqlite3-parser. Its default `core` feature compiles libsql-ffi, a second C copy of SQLite (build deps bindgen 0.66 + cmake), next to rusqlite's bundled SQLCipher. `libsql-hrana` 0.10.0 holds only the protocol types and depends on prost 0.12 and base64 0.21, so it is not worth it either.

Protocol (HRANA_3_SPEC.md / HTTP_V2_SPEC.md, Turso docs.turso.tech/sdk/http/reference):

- URL: `libsql://` or `wss://` becomes `https://`, `ws://` becomes `http://`. Strip `?authToken=…` and `?tls=0` from pasted URLs.
- Version: probe `GET {base}/v3`; a 2xx means use `POST v3/pipeline`, plus `POST v3/cursor` for NDJSON streaming and `get_autocommit`. Otherwise use `GET /v2` and `POST /v2/pipeline`. Turso only documents /v2/pipeline.
- Request: `{"baton":null|"…","requests":[{"type":"execute","stmt":{"sql":"…","args":[Value],"named_args":[{"name","value"}],"want_rows":true}},{"type":"close"}]}`. Other request types: `batch` (steps with conditions ok/error/not/and/or/is_autocommit), `sequence` (a SQL script, no results), `describe` (params, cols, is_explain, is_readonly), `store_sql`, `close_sql`.
- Response: `{"baton","base_url","results":[{"type":"ok","response":{"type":"execute","result":{"cols":[{"name","decltype"}],"rows":[[Value]],"affected_row_count","last_insert_rowid":"<string>","rows_read","rows_written","query_duration_ms"}}}|{"type":"error","error":{"message","code"}}]}`.
- Values: `{"type":"null"}`, `{"type":"integer","value":"<i64 as string>"}`, `{"type":"float","value":f64}`, `{"type":"text","value":"…"}`, `{"type":"blob","base64":"…"}`.
- HTTP 4xx/5xx means the stream is invalid.

New module `src-tauri/src/drivers/libsql/` with `hrana.rs` (serde types and client), `session.rs` (`impl DbSession`, Driver::Libsql, `dialect()` = Dialect::Sqlite) and `decode.rs` (a Hrana value becomes `Value` using `kind_from_decltype`, drivers/sqlite/decode.rs, and the same storage-class-wins rule as `cell_value`, decode.rs).

Sessions:

- Worker: short-lived streams (baton null, then `close` in the same pipeline).
- Editor: keeps the baton across `run_statement` calls so BEGIN and PRAGMAs persist, and follows `base_url`.
- Every new stream gets a prelude in the same request: `PRAGMA foreign_keys` per `options.sqlite_foreign_keys`, `startup_sql` as a `sequence` request, and `PRAGMA query_only = 1` when read-only.
- `transaction()` is one Hrana `batch`: BEGIN; each step conditioned on `{"type":"ok","step":i-1}`; COMMIT on ok(last); ROLLBACK on not ok(commit). `expect_affected` is checked with guard steps or interactively on a baton stream.
- In-transaction state: `get_autocommit` (v3) or `tx_effect` (drivers/mod.rs).
- SSH (TablePlus added "LibSQL over SSH" in Mac 27.0.2, Oct 2026): point reqwest `ClientBuilder::resolve(host, tunnel_local_addr)` at `ConnectTarget.host_override` (drivers/mod.rs), so SNI and certificate checks still use the real host.
- Cancel: new `CancelHandle::Http(tokio_util::sync::CancellationToken)` variant (drivers/mod.rs) that aborts the request future.

Shared refactor, done once for LibSQL and D1: make drivers/sqlite/introspect.rs independent of the transport. Today every function takes `&mut rusqlite::Connection` and maps `rusqlite::Row` (rows(), row_opt). Turn it into query builders (`Vec<(sql, Vec<Value>)>`) plus parsers over `Vec<Vec<Value>>`. The local actor then runs them one by one, and remote drivers send them in one pipeline or batch, so a table costs one round trip. Today it costs 1 + table_xinfo + index_list + one `PRAGMA index_xinfo` per index + foreign_key_list + triggers.

**Windows build.**

Pure Rust on crates already in Cargo.lock (reqwest 0.13.5, rustls 0.23.45 with aws-lc-rs, hyper 1.12, tokio, serde_json, base64). No C or C++, no OpenSSL, no bindgen or libclang, no new TLS stack. Build time and binary size grow only marginally.

Avoid the `libsql` crate on Windows:

- `core`/`libsql-sys` compiles libsql-ffi, a C SQLite fork whose build deps are bindgen 0.66 (needs libclang/LLVM installed) and cmake.
- It exports `sqlite3_*` symbols next to rusqlite's `bundled-sqlcipher-vendored-openssl` build (src-tauri/Cargo.toml), which risks LNK2005 duplicate-symbol link errors.
- Its `remote`/`tls` features add hyper 0.14 + hyper-rustls 0.25 (rustls 0.22, a second TLS stack), tower 0.4, http 0.2, base64 0.21, and tonic 0.11 if `replication` stays on.

reqwest has no `json` feature in Cargo.toml. Serialize with serde_json and set the body and content-type by hand, or add the cheap `json` feature. `system-proxy` already covers corporate proxies.

**Authentication.**

TablePlus: URL plus auth token. Its docs list `libsql` URLs as accepted when importing from the welcome window, and issue TablePlus/TablePlus#3270 reports that 5.9.6 dropped the `libsql://` prefix on URL import, after which the token test failed.

Tabler should:

- Use the auth token as the profile's password. It is stored in the OS keyring via secrets.rs with the usual storage menu (`PasswordMode` keychain/ask/none/command, types/profile.rs). Command mode suits `turso db tokens create <db>`, which is run on every connect and never stored.
- Send `Authorization: Bearer <token>` only when the token is non-empty. A sqld started without auth needs no header, and some clients report that sending an empty token gives a 400.
- Accept Turso database or group JWTs (`turso db tokens create <db> [--read-only] [--expiration]`).
- Optionally offer HTTP Basic auth (user + password) for self-hosted sqld configured with basic auth.
- Map 401/403 to "Authentication failed: check the token", 404 to "database not found", and 429 to "rate limited".

**Data model.**

Same tree as SQLite. One database per URL, since sqld namespaces are selected by subdomain, so `list_databases` returns one entry (like LiteSession::list_databases, drivers/sqlite/mod.rs) and there is no database switching. Schemas: `main`. Try `SELECT name FROM pragma_database_list` and fall back to `[main]`, because sqld refuses ATTACH unless configured. Tables and views come from sqlite_schema. Add `libsql\_%` (e.g. `libsql_wasm_func_table`) and `_litestream\_%` to the `NOT LIKE 'sqlite_%'` filter (introspect.rs), and consider hiding the `<index>_shadow` tables of libSQL vector indexes. Indexes, foreign keys, triggers and checks come from the same PRAGMAs and sqlite_schema SQL as SQLite. Row identity is PK or rowid as for SQLite (`identity_for`, commands/browse.rs, which must treat Libsql as SQLite-family).

TablePlus calls it "[LibSQL] Added SQLite over https" (Windows 5.9.6 build 270 and Mac build 542, March 2024) and presents it as an SQLite database. Later Mac releases added the bootstrap command (7.0.0, May 2026) and SSH (27.0.2, Oct 2026).

**Query language.**

SQLite SQL in the libSQL dialect. Extras: `ALTER TABLE t ALTER COLUMN c TO c <new definition>`, vector types and functions (F32_BLOB(n), vector32(), vector_distance_cos, vector_top_k, libsql_vector_idx indexes), and `WITH RANDOM ROWID` tables.

The editor sends one statement per Hrana `execute`, since the spec says each Stmt should hold one statement. Tabler already splits with sqlgen::split for Dialect::Sqlite. Parameters bind positionally via `args`; Tabler's `?1…` placeholders (Dialect::Sqlite.placeholder, sqlgen/quote.rs) map directly, and named `:a @a $a` go through `named_args`. EXPLAIN uses the SQLite path: `EXPLAIN QUERY PLAN` (sqlgen/explain.rs), with no analyze wrapper (explain.rs).

**Editing.**

Full SQLite-family editing through the existing sqlgen Dialect::Sqlite code (select/mutate/ddl):

- Rows: staged commits go through `DbSession::transaction` as one Hrana batch (BEGIN … COMMIT, ROLLBACK on failure).
- DDL: plain ALTER TABLE ADD/RENAME/DROP COLUMN, CREATE/DROP INDEX and TRIGGER. Other changes use the SQLite rebuild planner (sqlgen/ddl.rs `plan_ddl`). Its BEGIN block runs statement by statement in `run_plan` (commands/ddl.rs), so on LibSQL it must run on one baton-held stream and finish within Turso's 5-second transaction limit.
- Optional later: use libSQL's `ALTER TABLE … ALTER COLUMN` instead of rebuilds for type, NOT NULL, default or check changes.
- Inline editing of SQL-editor result sets is not available. Hrana `cols` carry only name and decltype, not the base table and column that `columns_of` takes from sqlite3_column_table_name (drivers/sqlite/decode.rs), so those results are read-only. Table tabs stay editable.

**Introspection.**

Reuse the SQLite catalog SQL in table-valued form with bound arguments (one pipeline per call; drizzle-kit's SQLite serializer uses the same functions over HTTP):

- Schemas: `SELECT name FROM pragma_database_list`, fallback `main`.
- Tables: `SELECT name, type FROM sqlite_schema WHERE type IN ('table','view') AND name NOT LIKE 'sqlite\_%' ESCAPE '\' AND name NOT LIKE 'libsql\_%' ESCAPE '\' ORDER BY name`.
- Table DDL: `SELECT type, sql FROM sqlite_schema WHERE name = ?1`.
- Columns: `SELECT cid, name, type, "notnull", dflt_value, pk, hidden FROM pragma_table_xinfo(?1, ?2)`.
- Indexes in one query: `SELECT il.seq, il.name, il."unique", il.origin, il.partial, ix.seqno, ix.name, ix."desc", ix.key FROM pragma_index_list(?1, ?2) il JOIN pragma_index_xinfo(il.name, ?2) ix ORDER BY il.seq, ix.seqno`, plus `SELECT name, sql FROM sqlite_schema WHERE type = 'index' AND tbl_name = ?1` for definitions and partial predicates (`index_predicate`).
- Foreign keys: `SELECT id, seq, "table", "from", "to", on_update, on_delete FROM pragma_foreign_key_list(?1, ?2)`.
- Triggers: `SELECT name, sql FROM sqlite_schema WHERE type = 'trigger' AND tbl_name = ?1`.
- View definition: `SELECT sql FROM sqlite_schema WHERE type = 'view' AND name = ?1`.
- Completion in one statement instead of one PRAGMA per table (introspect.rs): `SELECT m.name, p.name FROM sqlite_schema m JOIN pragma_table_info(m.name) p WHERE m.type IN ('table','view') … ORDER BY m.name, p.cid`.
- Server info: `SELECT sqlite_version()`, and optionally sqld `GET /version`.

The parsers (autoincrement detection, rowid alias, generated hidden=2/3, `sqlite_checks`, `parse_trigger_head`) carry over unchanged from introspect.rs.

**Testing without a server.**

1. Serde unit tests of the Hrana types against JSON fixtures from the spec (values, ok/error results, batch conditions, cursor NDJSON lines).

2. An in-process fake Hrana server in Rust tests: a tokio TcpListener (tokio `net` is already on) speaking minimal HTTP/1.1, or a hyper 1.x server as a dev-dependency (hyper 1.12 is already in Cargo.lock). Back it with a rusqlite in-memory connection per baton, implementing /v2/pipeline, /v3/pipeline, /v3/cursor, GET /v2 and /v3. This exercises the client, the shared introspection, transaction batches, the 10 s stream expiry and 401/404 handling with no network.

3. The reworked drivers/sqlite/introspect.rs keeps its local rusqlite tests (introspect.rs) running the same queries, which checks local/remote parity.

4. Optional real server for `pnpm test:rs:int`: add `ghcr.io/tursodatabase/libsql-server` (sqld, unauthenticated, http) to dev/docker-compose.yml. sqld has no native Windows build, so Docker is needed there.

5. Renderer: libsql profiles and fixtures in src/lib/ipc/mock (fixtures.ts, handlers.ts), as CLAUDE.md requires a mock for every new command.

**What it shares with an existing driver.**

Not wire-compatible with anything Tabler has: it is HTTP/JSON, not rusqlite. It shares a lot, though:

- Dialect::Sqlite in all of sqlgen: quoting, `?N` placeholders, rowid identity, blob-hex fetch (select.rs), the GLOB/instr filters (select.rs), EXPLAIN QUERY PLAN and the rebuild DDL.
- The SQLite catalog queries and parsers in drivers/sqlite/introspect.rs, after the transport refactor.
- `kind_from_decltype` (decode.rs).

Gaps compared with local SQLite:

- No column origin metadata, so `keep_trusted_origins` (decode.rs) and VIEW_NAMES_SQL do not apply and editor results are read-only.
- No sqlite3_interrupt (`CancelHandle::Sqlite` and the actor cannot be used).
- No `total_changes`/`is_autocommit`; use affected_row_count and get_autocommit (v3) or tx_effect.
- No SQLCipher key, file header checks, file backup or "SQLite over SSH" download (tunnel/sqlite_remote.rs).
- ATTACH and PRAGMA database_list may be refused.
- Session state lives only as long as a stream.

In code: add `Driver::Libsql` (types/profile.rs) with helpers `Driver::dialect()` and `is_sqlite_family()`. Change the `Driver::Sqlite` checks that mean "SQLite family" to include it: commands/browse.rs, commands/ddl.rs, commands/export.rs, assistant/context.rs. Keep the checks that mean "local file" away from it: commands/connections.rs (which must ALLOW an SSH tunnel for LibSQL), commands/profiles.rs, commands/backup.rs, commands/import.rs, commands/conn_extras.rs, mcp/pool.rs, mcp/tools.rs, conn_options.rs and .

**Connection fields.**

- Name / color / group / tag (common)
- URL: libsql://\<db>-\<org>.\<region>.turso.io, https://…, or http://127.0.0.1:8080 for sqld. Store it either in `host` + `port` + `ssl` (Disable means http, otherwise https) or in a new `ProfileOptions.http_url`. The host/port form keeps the SSH tunnel model working.
- Auth token, in the password field with the storage menu (Keychain / Ask / None / Command line)
- Over SSH (optional, as in TablePlus Mac 27.0.2): host / port / user / auth, the existing SshConfig
- Foreign keys on/off (options.sqlite_foreign_keys, sent as PRAGMA at the start of each stream)
- Bootstrap commands (options.startup_sql, run as a Hrana `sequence` on every new stream; TablePlus 7.0.0 added "[LibSQL] Support for bootstrap command")
- Read only (options.read_only: PRAGMA query_only per stream, plus the client-side guard)
- Connect timeout (options.connect_timeout_secs), Safe mode, Lazy load (common)
- URL import: libsql://host?authToken=… moves the token into the secret, and ?tls=0 means http

**Effort:** L.

**Risks.**

- Turso documents that transactions must finish within 5 s and that streams close after 10 s idle. An editor BEGIN…COMMIT left open, or a large table rebuild (commands/ddl.rs run_plan), gets rolled back by the server. Detect a dead baton or HTTP 4xx, reset TxState to Some(false) and add a notice that the transaction was lost.
- There is no server-side cancel. Aborting the HTTP request leaves the statement running on sqld or Turso, so report it like LATE_CANCEL_NOTICE (drivers/mod.rs).
- /v2/pipeline returns the whole result in one body, so the max_rows cap only saves memory after the download. Real streaming needs /v3/cursor, which Turso does not document. Probe GET /v3 and fall back.
- The `libsql` crate is on an old HTTP stack (hyper 0.14, hyper-rustls 0.25 with rustls 0.22, tower 0.4), and its `core` feature brings a second bundled SQLite (libsql-ffi with bindgen/libclang) that can clash with rusqlite's bundled SQLCipher symbols at link time. This is why the client is hand-rolled.
- Without column origins in Hrana results, editor result grids are read-only (decode.rs has no equivalent). Users may expect TablePlus-style inline edits of query results.
- Read-only cannot be enforced on the server unless the user brings a read-only token. `PRAGMA query_only` per stream is unverified on Turso, so the renderer and MCP guards have to do it.
- sqld may refuse ATTACH and `PRAGMA database_list`, so list_schemas needs a fallback to `main`.
- Internal tables (`libsql_wasm_func_table`, vector-index `<name>_shadow` tables, `_litestream_*`) clutter the sidebar and completion unless filtered.
- Hrana error `code` values are server-specific. The parser must also handle plain-text or HTML bodies on 4xx/5xx, as the spec warns.
- A `libsql://` URL pasted with `?authToken=` must not keep the token in `host` or a deep link. TablePlus issue #3270 shows URL import mangling the scheme.
- Turso is moving to its new Rust engine; the `turso` crate (0.9.0-pre.1, 2026-10-15) is a separate local engine, not an HTTP client. Keep to the documented /v2/pipeline contract and re-check Turso's docs before release.
- Every DbSession method is one HTTPS round trip, so latency is high. Introspection, completion and the grid's count and page queries should go out in one pipeline request.

### Cloudflare D1

**Approach.**

Hand-roll a REST client on reqwest 0.13 (already a dependency; rustls + aws-lc-rs) with serde_json. No crate needed. There is no official Rust D1 REST client, and `cloudflare` SDK crates are not needed for three endpoints.

Base URL: `https://api.cloudflare.com/client/v4`.

- `POST /accounts/{account_id}/d1/database/{database_id}/raw`, body `{"sql":"…","params":[…]}` or `{"batch":[{"sql","params"},…]}`. Response: `{"success","errors":[{"code","message"}],"messages","result":[{"results":{"columns":["…"],"rows":[[…]]},"meta":{"changed_db","changes","duration","last_row_id","rows_read","rows_written","size_after","served_by_colo","served_by_primary","served_by_region","timings":{"sql_duration_ms"}},"success"}]}`. Prefer /raw over /query: /query returns rows as objects keyed by column name, so `SELECT a.id, b.id` loses a column.
- `GET /accounts/{account_id}/d1/database?name=&page=&per_page=` returns uuid, name, created_at, version and jurisdiction, for `list_databases` and name-to-UUID resolution.
- `GET …/d1/database/{database_id}` returns name, version, file_size, read_replication, for `server_info`.
- `POST …/d1/database/{database_id}/export` with `{output_format:"polling", dump_options:{no_data,no_schema,tables}, current_bookmark}`, polled until `result.result.signed_url` (valid one hour), gives a Backup… equivalent.

New `src-tauri/src/drivers/d1/` with Driver::D1 and dialect Dialect::Sqlite. It reuses the transport-agnostic SQLite catalog layer built for LibSQL (see that entry), sending all catalog statements of a call in one `batch` request.

- `transaction()`: one REST `batch`, which is atomic and rolls back if any statement fails. For `expect_affected`, add a sqlgen-generated guard after each guarded statement, e.g. `SELECT CASE WHEN changes() = ?1 THEN 1 ELSE json('affected-row mismatch') END`. The malformed json() raises an error and rolls the batch back; map it to the `check_affected` message (drivers/mod.rs).
- `run_statement`: one `/raw` call, then cap rows client-side. Tx state is always Some(false).
- `switch_database` (commands/connections.rs) reconnects with another database id, which costs nothing because there is no real connection.
- Cancel: `CancelHandle::Http` aborts the request client-side only.

**Windows build.**

Pure Rust over reqwest 0.13.5 and rustls 0.23 with aws-lc-rs, all already in Cargo.lock. No C or C++, no OpenSSL, no new crates. Negligible build-time and binary-size impact. `system-proxy` (Cargo.toml) gives corporate proxy support. No TLS customisation is needed because the endpoint is always api.cloudflare.com over HTTPS.

**Authentication.**

API token sent as `Authorization: Bearer <token>`. Create a custom token with Account › D1 › Edit, or Read for browsing only. The API reference names the permissions "D1 Read" / "D1 Write". The legacy `X-Auth-Email` + `X-Auth-Key` (Global API key) is supported by the API but should not be offered.

Store the token as the profile password in the OS keyring, with the storage menu including Command (e.g. a secrets-manager CLI). A Cloudflare community thread about TablePlus reports that a token with only D1 Read could still write, so Tabler must enforce read-only and safe mode on the client and must not rely on the token. One token usually reaches every D1 database in the account. Map error codes and HTTP statuses: 401/403 means authentication, 404 means the database is not found, 429 means the 1,200-requests-per-5-minutes global API limit with a retry delay.

**Data model.**

Databases: every D1 database in the account from GET /accounts/{id}/d1/database, with `is_current` for the profile's database. Switching reconnects through the existing switch_database flow (commands/connections.rs).

Schemas: `main` only. `PRAGMA database_list` is not on D1's PRAGMA whitelist and there is no ATTACH, so hard-code it.

Tables and views come from sqlite_schema with `_cf\_%` filtered out (D1 keeps an internal `_cf_KV` table, listed by `PRAGMA table_list`, that cannot be read) as well as `sqlite\_%`. Keep `d1_migrations`, wrangler's migration table, because it is a user table. Indexes, foreign keys, triggers and checks come from the same SQLite catalog. Row identity is PK or rowid (commands/browse.rs must treat D1 as SQLite-family).

TablePlus added Cloudflare D1 in Windows 5.9.7 (build 272, April 2024) and Mac build 550 and presents it like SQLite. TablePro, a similar client, lists the account's databases in the toolbar switcher, which matches Tabler's switch_database model.

**Query language.**

SQLite SQL as implemented by D1, with FTS5 (and fts5vocab), the JSON extension and math functions. Parameters: only `?NNN` (ordered) and `?` (anonymous); named parameters are not supported. Tabler's `?1…` placeholders work.

Multiple statements separated by `;` may go in one /raw or /query call and run as a batch. Tabler's editor still splits and runs them one by one.

Not available:

- BEGIN, COMMIT, SAVEPOINT. D1 answers BEGIN with an error telling you to use `state.storage.transaction()`.
- ATTACH.
- PRAGMAs outside the whitelist: table_list, table_info, table_xinfo, index_list, index_info, index_xinfo, quick_check, foreign_key_check, foreign_key_list, case_sensitive_like, ignore_check_constraints, legacy_alter_table, recursive_triggers, reverse_unordered_selects, foreign_keys, defer_foreign_keys, optimize. D1 PRAGMAs only apply to the current transaction.

EXPLAIN QUERY PLAN goes through the SQLite path in sqlgen/explain.rs. TablePro reports that it works on D1.

**Editing.**

Row inserts, updates and deletes use sqlgen Dialect::Sqlite with PK or rowid identity. Staged commits (⌘S) go out as one atomic REST `batch` with the changes()-guard statements for `expect_affected`. Simple DDL (CREATE/DROP/RENAME TABLE, ADD/RENAME/DROP COLUMN, CREATE/DROP INDEX and TRIGGER) runs as is.

Table rebuilds from the SQLite planner (sqlgen/ddl.rs `plan_ddl`, wrapped as `PRAGMA foreign_keys=OFF; BEGIN; …; PRAGMA foreign_key_check; COMMIT; PRAGMA foreign_keys=ON` and run statement by statement in commands/ddl.rs) cannot run as they are, because D1 rejects BEGIN and foreign keys cannot be switched off across requests. For D1, `adapt_to_profile` (commands/ddl.rs) should:

- drop the BEGIN/COMMIT and foreign_keys toggles;
- send the whole block as one REST batch, with `PRAGMA defer_foreign_keys = on` as its first statement;
- keep `foreign_key_check`.

The batch must finish within 30 s, since D1's query-duration limit covers a whole batch. With that, Tabler can rename and retype columns on D1, which TablePro does not support.

The SQL editor's result sets are read-only: /raw returns column names only, with no origins and no decltypes.

Imports and paste must chunk to D1's 100 bound parameters per statement. `max_params(Dialect::Sqlite)` is 32,000 today (sqlgen/transfer.rs), so it must take the driver into account. Statements are limited to 100 KB and rows to 2 MB.

**Introspection.**

The same table-valued catalog queries as LibSQL (all functions on D1's whitelist; drizzle-kit's D1 HTTP introspection uses them), sent as one `{"batch":[…]}` /raw request per call:

- Tables: `SELECT name, type FROM sqlite_schema WHERE type IN ('table','view') AND name NOT LIKE 'sqlite\_%' ESCAPE '\' AND name NOT LIKE '\_cf\_%' ESCAPE '\' ORDER BY name`.
- Columns: `SELECT cid, name, type, "notnull", dflt_value, pk, hidden FROM pragma_table_xinfo(?1)`.
- Indexes: `SELECT il.seq, il.name, il."unique", il.origin, il.partial, ix.seqno, ix.name, ix."desc", ix.key FROM pragma_index_list(?1) il JOIN pragma_index_xinfo(il.name) ix ORDER BY il.seq, ix.seqno`.
- Index and trigger SQL: `SELECT type, name, sql FROM sqlite_schema WHERE tbl_name = ?1`.
- Foreign keys: `SELECT * FROM pragma_foreign_key_list(?1)`.
- Completion in one statement: `SELECT m.name, p.name FROM sqlite_schema m JOIN pragma_table_info(m.name) p WHERE m.type IN ('table','view') AND m.name NOT LIKE '\_cf\_%' ESCAPE '\' …`.
- Schemas: hard-coded `[main]`.
- Version: `SELECT sqlite_version()` plus GET …/d1/database/{id} (version, file_size).

Do not use schema-qualified `PRAGMA "main".x(...)`, which introspect.rs build with format!: whether D1 allows that form is unverified, and the table-valued form also keeps identifiers out of string formatting.

**Testing without a server.**

`wrangler d1 execute --local` (Miniflare) does not emulate the REST API, so build a fake D1 REST server in Rust tests:

- tokio TcpListener or a hyper 1.x dev-dependency, serving `/client/v4/accounts/{a}/d1/database` (list), `/{d}` (get) and `/{d}/raw|query`, backed by a rusqlite in-memory database.
- A rusqlite authorizer (needs the rusqlite `hooks` feature, dev-only) that rejects BEGIN, SAVEPOINT, ATTACH, PRAGMAs off the whitelist and reads of a seeded `_cf_KV` table.
- Enforce 100 params and 100 KB statements, make batches atomic, return integers as JSON numbers and blobs as arrays of bytes, and return Cloudflare-style `errors` arrays and 429s.
- The client takes an overridable API base URL so the tests point it at 127.0.0.1.

Together with the shared introspection tests this covers the driver offline. Real-account checks go in `pnpm test:rs:int`, gated on CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_API_TOKEN and D1_DATABASE_ID in `.env` (the free plan allows 10 databases of 500 MB each); document them in docs/verify-servers.md. Renderer: D1 fixtures in src/lib/ipc/mock.

**What it shares with an existing driver.**

Not wire-compatible: it is the Cloudflare REST API over HTTPS, JSON in and out. It shares the same Dialect::Sqlite parts of sqlgen and the SQLite catalog layer as LibSQL.

Differences from SQLite:

- No SQL transactions; a REST batch is the only unit of atomicity.
- PRAGMA whitelist (database_list and query_only are not on it); PRAGMAs and temp tables do not survive between requests.
- `_cf_KV` is hidden but cannot be read.
- No decltypes or column origins in results.
- Values arrive as JSON: integers above 2^53 are lossy, REAL and INTEGER cannot be told apart for whole numbers, and blobs come back as arrays of numbers.
- `params` are documented as an array of strings.
- Limits: 100 params, 100 KB per statement, 50-byte LIKE/GLOB patterns, 100 columns per table, 2 MB per row, 30 s per query, 10 GB per database.
- No cancel, no streaming, no SSH, no file backup.

Use the same `Driver::is_sqlite_family()` and "is local file" split of the `Driver::Sqlite` call sites listed for LibSQL, except that D1 excludes SSH in `tunnel_config` (commands/connections.rs). Also handle D1 in conn_options.rs, the UTF-8 encoding check, which does not apply.

**Connection fields.**

- Name / color / group / tag (common)
- Account ID (32 hex characters; Workers & Pages › Overview). Store it in a new `ProfileOptions.cloudflare_account_id`, or reuse `user` labelled "Account ID".
- Database: D1 database UUID or name, stored in `profile.database`; a name is resolved through GET /d1/database?name=. The toolbar database switcher lists every D1 database of the account.
- API token, in the password field with the storage menu (Keychain / Ask / Command)
- Read only (enforced client-side only)
- Safe mode, Connect timeout (applied as the request timeout), Lazy load (common)
- Hidden or advanced API base URL override (default https://api.cloudflare.com/client/v4), used by tests that point it at a local fake server
- No host, port, SSL, SSH or bootstrap commands: each request is its own transaction, so per-session PRAGMAs and temp tables cannot persist
- URL import / deep link: TablePlus schemes `d1://` and `cloudflared1://` (tableplus.com/docs/gui-tools/manage-connections); TablePro uses `d1://<account-id>/<database>` with the token kept out of the URL

**Effort:** M.

**Risks.**

- The `params` array is documented only as strings. Whether JSON numbers and null are accepted is unverified, and blobs cannot be bound at all. Fall back to strings, which INTEGER affinity converts, and inline NULL and `X'…'` blob literals through sqlgen (`literal`, sqlgen/select.rs). Columns with no affinity then compare '5' ≠ 5.
- Integers outside ±2^53 come back rounded, because D1's API has no BigInt. For INTEGER-kind columns, have sqlgen select `CASE WHEN typeof(c)='integer' AND abs(c) > 9007199254740991 THEN CAST(c AS TEXT) ELSE c END` and decode that as Value::BigInt.
- No BEGIN, COMMIT or SAVEPOINT. Anything that relies on connection state across `execute` calls breaks: the DDL rebuild blocks, the editor's transactions, the Explain Analyze wrapper and the SQLite plan lock in commands/ddl.rs. Every such path needs a D1 branch that sends one batch.
- Foreign keys are always enforced and PRAGMAs last only for the current transaction. Rebuilds must use `PRAGMA defer_foreign_keys = on` inside the same batch, and a batch is capped at 30 s.
- The 100-bound-parameter limit breaks today's import and transfer batching (`max_params(Dialect::Sqlite)` = 32,000, sqlgen/transfer.rs), large multi-row INSERTs and big IN lists in copy/paste or delete-many. The 50-byte LIKE/GLOB pattern limit breaks long Contains or prefix filters (sqlgen/select.rs).
- The Cloudflare API allows 1,200 requests per 5 minutes per user, after which it answers 429 and blocks for the rest of the window. A sidebar expanding 50 tables one request at a time, polling or auto-refresh can lock the user out. Batch, cache and respect lazy loading.
- A D1 Read token reportedly did not stop writes (Cloudflare community report involving TablePlus). Read-only and safe mode must be enforced by Tabler's statement classification.
- `_cf_KV` appears in sqlite_schema and `PRAGMA table_list`, but reading it fails as not authorized. It must be filtered everywhere: sidebar, completion, export-all and the diagram.
- Latency: each call is a round trip through api.cloudflare.com to the database's primary region. Counting plus paging plus table metadata can take seconds without batching.
- Cancel only stops the client: a running statement continues up to D1's 30 s limit. Large editor results are fully materialized, since there is no cursor.
- Whether a REST `batch` is atomic in the same way as the Workers `db.batch()` is assumed from D1's batch semantics. Verify it against a real account before relying on it for staged commits.
- Database names are not unique identifiers across renames, so store the UUID after resolving a name. A token may expose every database in the account through the switcher.

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._

Known so far:

- Only the `main` schema: `sqld` and D1 refuse `ATTACH`, D1 refuses `PRAGMA database_list`.
- SQL editor results are read-only: neither engine reports the base column of a result column.
- LibSQL: no cancel on the server (the request is dropped; the statement runs on). A `base_url` on another scheme, host or port is not followed (it would get the token, and the SSH tunnel maps only the profile's host): the stream stays at the URL it used. Streams expire after about 10 s idle and transactions after about 5 s: an editor transaction left open is rolled back by the server, and the session says so.
- D1: no transactions across statements (a staged commit is one batch); blobs cannot be bound (they go in the statement as literals); at most 100 parameters and 100 KB per statement; integers beyond 2^53 are exact only where the select reads them as text (otherwise D1's JavaScript has rounded them); the account's API rate limit (1,200 requests per 5 minutes) answers 429. Outside a staged commit an affected count is D1's `meta.changes`, which includes rows changed by triggers and cascades.
