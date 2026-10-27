# ClickHouse: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                                                                   |
| ----------------- | ------------------------------------------------------------------------------------------------- |
| Lane              | `clickhouse`                                                                                      |
| Databases         | ClickHouse                                                                                        |
| Drivers           | `clickhouse`                                                                                      |
| Picker kinds      | `clickhouse`                                                                                      |
| SQL dialect       | `clickhouse`                                                                                      |
| Cargo features    | `drv-clickhouse`                                                                                  |
| Home slot         | `s1`                                                                                              |
| Bindings prefix   | `Clickhouse*`                                                                                     |
| Lib test filters  | `drivers::clickhouse`, `sqlgen::dialect::clickhouse`, `sqlgen::portable::generated::clickhouse::` |
| Real-server suite | `tests/clickhouse.rs` with `TABLER_TEST_CLICKHOUSE_URL`                                           |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/clickhouse/**`
- `src-tauri/src/sqlgen/dialect/clickhouse.rs`
- `src-tauri/src/sqlgen/portable/gen/clickhouse.rs`
- `src-tauri/tests/clickhouse.rs`
- `src-tauri/tests/fixtures/clickhouse/**`
- `src-tauri/tests/fixtures/classify-generated/clickhouse.json`
- `src/features/drivers/clickhouse/**`
- `src/lib/ipc/mock/drivers/clickhouse/**`
- `src/lib/ipc/bindings/Clickhouse*.ts`
- `src/lib/ipc/bindings/lex/clickhouse.json`
- `docs/drivers/clickhouse.md`

Anything else (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead. `node dev/scripts/lane-commit.mjs clickhouse -m "clickhouse: …"` refuses other paths.

## Scope of v1

### ClickHouse

- **Identity:** `clickhouse`, `SqlDialect::Clickhouse`. Crates: none.
- **UI:** SQL grid.
- **Auth v1:** user and password headers, mTLS / X.509, Command.
- **Required fields:** host (8123; 8443 with TLS; user `default`).
- **Out of scope v1:** native protocol, transactions, ON CLUSTER DDL editing, edits where the key is not unique (pre-count refuses).
- **Test:** fake HTTP (JSONCompactEachRowWithNamesAndTypes, mid-stream `__exception__`, summary headers); type-parser tests. No Windows server (Cloud trial opt-in).

## Descriptor values

Seeded from the plan (§5.2). The values in force are the descriptors in `src-tauri/src/types/driver.rs` (keepalive belongs to the session's caps); a change is a request to the lead.

| Driver     | Location | Port (TLS)  | SSH    | TLS style | Secret label | Command TTL |
| ---------- | -------- | ----------- | ------ | --------- | ------------ | ----------- |
| clickhouse | Host     | 8123 (8443) | Tunnel | OnOff     | Password     | 600         |

| Driver     | Required fields / params | Label overrides | Keepalive | Read-only  | Sharing | Token secret |
| ---------- | ------------------------ | --------------- | --------- | ---------- | ------- | ------------ |
| clickhouse | host                     | —               | 30        | PerRequest | Indep   | no           |

## Read-only and hardening

| Engine     | When read-only                                                                                                        | When unattended (MCP) |
| ---------- | --------------------------------------------------------------------------------------------------------------------- | --------------------- |
| ClickHouse | `readonly=2` per request; the classifier allows only the table functions of its allow-list and calls `file()` a write | same                  |

Every classifier is default-deny: a statement or command it does not know is a write. The read-only mode in the caps (`Server`, `PerRequest` or `ClassifierOnly`) must say what the session really enforces.

**The engine, not the classifier, has the last word on reads.** The portable classifier (`sqlgen::portable::classify` with the rules in `sqlgen/dialect/clickhouse.rs`) only knows the plain spellings: a table function outside the allow-list where a table goes, a call of `file()` anywhere, and `INTO` (`INTO OUTFILE`). What else a SELECT may reach (the server's files, other servers through dictionaries or engines of existing tables) is the server's to refuse. So the lane must:

- send `readonly=2` with every request of a read-only or unattended (`registry::unattended`, the MCP process) session, and report `ReadOnlyMode::PerRequest` with `safety.hardened` only when it does;
- test it: under `readonly=2` an `INSERT`, a `CREATE` and an `ALTER … UPDATE` must fail, and the brief's limits must say that the server's grants on sources (`FILE`, `URL`, `REMOTE`, `S3`…) decide what a read may reach.

## What to implement

- `entry.rs` (Phase B): `open`, `validate`, `util`, `util_kind` (plan §2.5). `<Prefix>Params` (`#[serde(rename_all = "camelCase", default)]`, ts-exported) parsed from `options.driver_params`.
- `DbSession`: the 18 required methods; overrides of `query_cancellable`, `execute_cancellable`, `drain_trace`, `sql_trace` and `editor_run_lock`; `dialect()`, `capabilities()` (accurate features, data, safety and lifecycle caps) and `flavor()` where it applies.
- Optional: `table_source()`, `call` / `call_kind`, `list_processes` / `kill_processes`, `table_ddl`, `explain_form`.
- Cancel through `CancelHandle::Custom(Arc<dyn CancelAction>)`; `cancel()` calls `cancel_portable` first.
- Renderer: a `KindUi` per tile, `DriverUi`, `DialectUi` and a `MockDriver`. Write-capable `sessionCall` / `driverUtil` helpers await `guardExecution` first.

## Decisions and corrections for this lane

- HTTP interface through `drivers/common/http` (`common-http`); no native protocol in v1.
- Read-only is `readonly=2` on every request (plan §5.3). The classifier allows only the table functions `numbers`, `zeros`, `values`, `generateRandom`, `view`, `merge` and `null`; every other one (`url`, `s3`, `file`, `remote`, `mysql`, `iceberg*`, `deltaLake*`, `cluster`…) is a write (critique D4, D5).
- The affected-rows guard: the pre-count before an edit is best-effort (check-then-act), so edits use primary-key identity only, with EditWithoutKey and all-columns edits off; a key that is not unique refuses the edit (critique D1).
- ClickHouse 23.3 or later (lightweight DELETE); UPDATE from 25.7, otherwise an `ALTER UPDATE` mutation behind a warning (plan §1.5 Q18).

## Definition of done (this lane)

Beyond the shared list in [README.md](README.md#definition-of-done-v1):

- **Classifier hardening (blocks landing).** `src-tauri/src/sqlgen/portable/gen/clickhouse.rs` is complete and `src-tauri/tests/fixtures/classify-generated/clickhouse.json` blessed, as [Classifier hardening](README.md#classifier-hardening) requires: every write keyword, write call and read starter of `dialect::clickhouse::SPEC` in every shared context and in the contexts only this grammar has, an engine-realistic read corpus, its write forms and fuzz atoms, `suite().check()` and `suite().fuzz(…)` in place of the placeholder. Then a red-team review passes, each finding fixed and kept as a case. Known hard cases: `file()` as a scalar call anywhere, not only where a table goes; the table-function allow-list (`numbers`, `zeros`, `values`, `generateRandom`, `view`, `merge`, `null`), with every other function in a table position (`url`, `s3`, `remote`, `cluster`, `iceberg*`, `deltaLake*`, the `*Cluster` forms), also qualified, quoted, in a join or a subquery, a write; `INTO OUTFILE`; `SYSTEM …`, `KILL …`, `OPTIMIZE …`; mutations (`ALTER TABLE … UPDATE` / `DELETE`, lightweight `DELETE FROM`). What a SELECT reaches through dictionaries or table engines is `readonly=2`'s to refuse.

## Research notes

> Background from the driver research (October 2026), with file references and no line numbers. Verify against the code; where it differs from the scope above or from README.md, those win.

### ClickHouse

**Approach.**

New Driver::Clickhouse + Dialect::Clickhouse over the HTTP(S) interface, hand-rolled on the reqwest 0.13 client the app already has (Cargo.toml: rustls, system proxy; client pattern in src-tauri/src/assistant/mod.rs). Per request: POST http(s)://host:8123|8443/?database=\<db>&query_id=\<uuid>&session_id=\<id> (editor connection only)&default_format=JSONCompactEachRowWithNamesAndTypes plus explicit settings as URL parameters (output_format_json_quote_64bit_integers=1, output_format_json_quote_decimals=1, output_format_json_quote_denormals=1, output_format_json_validate_utf8=1, cancel_http_readonly_queries_on_client_close=1, optionally send_progress_in_http_headers=1); credentials in X-ClickHouse-User / X-ClickHouse-Key headers (never URL params, which proxies log); SQL in the body. Stream the body line by line: line 1 column names, line 2 ClickHouse types, then one JSON array per row into RowChunker (drivers/mod.rs). HTTP 200 does not mean success: detect the mid-stream `__exception__` block carrying the X-ClickHouse-Exception-Tag value; errors before output arrive as HTTP 500 with 'Code: N. DB::Exception: ...' text and X-ClickHouse-Exception-Code -> AppError::Db{vendor_code}. Read X-ClickHouse-Summary (read_rows, written_rows, result_rows, elapsed_ns) for affected counts and X-ClickHouse-Timezone. Bind values as server-side query parameters `{p1:<ClickHouse type>}` + `param_p1=` (type taken from the column meta, i.e. the ParamSink cast slot, select.rs); inserts as `INSERT INTO db.t (cols) FORMAT JSONEachRow` with JSON rows in the body (typed parsing on the server, no literal escaping). Editor session: one session_id per editor connection (SET persists; sessions expire after 60 s idle by default, so send session_timeout and keep the editor_run_lock since a session runs one query at a time). Cancel: CancelHandle::Clickhouse{query_id} -> `KILL QUERY WHERE query_id = {id:String} ASYNC` on a second request (users can always kill their own queries by id) plus aborting the reqwest future. SSH tunnel: ClientBuilder::resolve(tls_host, 127.0.0.1:\<tunnel port>) so SNI and certificate checks still use the real host. Rejected: the official `clickhouse` crate 0.15.3 (2026-10-13; hyper 1 + hyper-rustls 0.27): built around typed serde rows over RowBinary (fetch::\<T: Row>), dynamic results only through fetch_bytes(format), no query_id or cancel API on Query, and a second HTTP stack without reqwest's system proxy; native-protocol crates (klickhouse, clickhouse-arrow) are less maintained. A native TCP driver (9000/9440, which TablePlus also offers next to HTTP, #2876) can come later.

**Windows build.**

Zero new crates for the hand-rolled HTTP driver: reqwest 0.13.5 (rustls with the aws-lc-rs provider), serde_json, uuid, tokio, futures are already in Cargo.toml/Cargo.lock; nothing native to compile. If the clickhouse crate were chosen: default-features = false (the default enables lz4 via lz4_flex + cityhash-rs) and features rustls-tls-aws-lc + rustls-tls-native-roots (not rustls-tls, which forces webpki-roots); hyper-rustls 0.27.10 and hyper-util 0.1.21 are already locked; it adds bnum, half, hashbrown 0.17, clickhouse-types, clickhouse-macros (proc-macro): pure Rust, fine on MSVC. ClickHouse server has no native Windows build (local testing needs WSL, Docker or ClickHouse Cloud).

**Authentication.**

TablePlus: user/password on its native or HTTP driver (scheme clickhouse://), TLS with and without SSL validation (#3091), SSH. Tabler: user (default `default`) + password with the existing PasswordMode (keychain/ask/none/command) sent in X-ClickHouse-User / X-ClickHouse-Key (or HTTP Basic); HTTPS through the existing SslMode (disable = http; require = https without verification; verify-ca / verify-full = https with rustls verification), CA file (ssl_ca_path) and client certificate/key (ssl_cert_path / ssl_key_path) for mTLS, including X.509 user authentication (users IDENTIFIED WITH ssl_certificate, header X-ClickHouse-SSL-Certificate-Auth: on); optional role(s) via the `role` URL parameter and a settings profile; SSH tunnel. ClickHouse Cloud: HTTPS 8443, user default.

**Data model.**

Two levels, databases -> tables (no schemas): map it like the MySQL driver, list_schemas returns the databases and TableRef.schema = database (types/schema.rs), list_databases a single server entry. Kinds from system.tables.engine: \*MergeTree, Log, Memory, Distributed, Kafka, Buffer -> Table; View -> View; MaterializedView -> MaterializedView; MySQL/PostgreSQL/S3/URL/File engines -> ForeignTable; Dictionary -> its own object kind; system, INFORMATION_SCHEMA and information_schema hidden by default. No foreign keys, no triggers (materialized views are insert triggers, see system.tables.dependencies_table); the 'indexes' are the primary/sorting/partition key expressions plus data-skipping indices and projections. TablePlus shows it MySQL-style; most of its ClickHouse issues are type decoding: LowCardinality(Nullable(String)) (#3772, #3608), SimpleAggregateFunction (#3284), AggregateFunction (#3417), UInt256 (#3164), Map crash (#3811), Nested + Enum8 crash (#3700), JSON type (#3667), DateTime64 timezones (#3604), DateTime milliseconds (#3921), NULL copied as '\<null>' on HTTP (#3833). Mapping: strip Nullable(), LowCardinality() and SimpleAggregateFunction(f, T) wrappers; (U)Int8-32 -> Int; (U)Int64 (quoted) -> Value::from_i64 or BigInt; (U)Int128/256 -> BigInt; Float32/64 -> Float (nan/inf quoted -> Other); Decimal(P,S) (quoted) -> Decimal; Bool -> Bool; String/FixedString -> Text (invalid UTF-8 replaced, full bytes via hex()); UUID -> Uuid; Date/Date32 -> Date; DateTime / DateTime64(p, 'tz') -> Timestamp text as sent, column timezone kept in ResultColumn.data_type; Enum8/16 -> Enum (values parsed from the type string); Array -> Array; Tuple/Map/Nested/JSON/Dynamic/Variant -> Json; IPv4/IPv6/Interval/Point/Ring/Polygon -> Other; AggregateFunction -> finalizeAggregation(col) in browse SELECTs or shown as unsupported.

**Query language.**

ClickHouse SQL. Identifiers quoted with backticks or double quotes, and the string-literal escape rules apply inside them (escape backslash and the quote character with a backslash); string literals single-quoted only, with backslash escapes and '' doubling, so literals follow the MySQL BackslashEscapes branch (quote.rs); heredoc $tag$...$tag$; comments --, #, /\* \*/. SQL-standard keywords are case-insensitive, function names case-sensitive. Placeholders are typed `{pN:Type}` rather than ?; parameters travel in the URL, limited to 1 MiB by default (http_max_uri_size), which bounds transfer::max_params (transfer.rs). Paging `LIMIT m OFFSET n` works as written today (select.rs); without a user sort fall back to system.tables.sorting_key; NULLS FIRST/LAST and ILIKE exist, so check_dialect (select.rs) can allow ILIKE. The HTTP interface rejects multi-statement requests ('Multi-statements are not allowed'), so the editor must split (split.rs needs a ClickHouse mode: backslash escapes, # comments, heredocs) and send each statement separately on the same session. A FORMAT clause in user SQL overrides default_format (X-ClickHouse-Format header overrides it since 26.8): strip or reject it for grid display. EXPLAIN PLAN indexes = 1 / EXPLAIN PIPELINE / EXPLAIN ESTIMATE (new arms in explain.rs). Process list / kill from system.processes (admin.rs).

**Editing.**

Possible but different, and there are no transactions. INSERT: `INSERT INTO db.t (cols) FORMAT JSONEachRow` with the rows in the body. UPDATE: `ALTER TABLE db.t UPDATE c = {p1:T} WHERE ...` (heavyweight asynchronous mutation; add mutations_sync=2 to wait; progress and failures in system.mutations is_done / latest_fail_reason; KILL MUTATION to stop) or, on ClickHouse 25.7+ MergeTree tables with enable_block_number_column / enable_block_offset_column, the lightweight `UPDATE db.t SET ... WHERE ...` (patch parts, meant for small changes). TablePlus sent a plain UPDATE and failed (#3017). DELETE: lightweight `DELETE FROM db.t WHERE ...` (MergeTree family) else `ALTER TABLE db.t DELETE WHERE ...`. Columns used in the primary, sorting or partition key cannot be updated (mark read-only from system.columns.is_in_\*_key). The ORDER BY key is not unique, so row edits match all columns (bind DateTime values as typed parameters: TablePlus #3202 / #3344 failed deletes on DateTime; Map/Array/JSON/Float columns need care) and cannot be limited to one row: pre-check `SELECT count() ... WHERE` and refuse when it is not 1. transaction() cannot roll back: apply one by one and say that a failure leaves earlier statements applied; mutations report no row counts (X-ClickHouse-Summary written_rows covers inserts). DDL: CREATE TABLE ... ENGINE = MergeTree ORDER BY ... [PARTITION BY] [TTL] [SETTINGS]; ALTER TABLE ADD / DROP / MODIFY / RENAME / COMMENT COLUMN; ADD / DROP INDEX (skipping); MODIFY ORDER BY / TTL; RENAME TABLE; EXCHANGE TABLES; TRUNCATE; DROP; ON CLUSTER for replicated clusters. Structure editor scope: columns (type, DEFAULT/MATERIALIZED/ALIAS, codec, comment) and skipping indexes; engine and keys read-only. Clone/duplicate must emit valid quoted names (TablePlus #3203).

**Introspection.**

server_info: `SELECT version(), currentDatabase(), currentUser(), timezone()`. Databases: `SELECT name, engine, comment, name = currentDatabase() AS is_current FROM system.databases ORDER BY name`. Tables: `SELECT name, engine, total_rows, total_bytes, comment, is_temporary FROM system.tables WHERE database = {db:String} ORDER BY name` (total_rows / total_bytes are NULL when not cheaply known). Columns: `SELECT name, type, position, default_kind, default_expression, comment, is_in_primary_key, is_in_sorting_key, is_in_partition_key, is_in_sampling_key, compression_codec, numeric_precision, numeric_scale, datetime_precision FROM system.columns WHERE database = {db:String} AND table = {t:String} ORDER BY position` (default_kind MATERIALIZED/ALIAS -> is_generated; nullable = type starts with Nullable(). Keys: `SELECT engine, engine_full, partition_key, sorting_key, primary_key, sampling_key, create_table_query, as_select FROM system.tables WHERE database = {db:String} AND name = {t:String}` (primary_key shown, not used as row identity). Skipping indexes: `SELECT name, type, expr, granularity FROM system.data_skipping_indices WHERE database = {db:String} AND table = {t:String}`; projections: system.projections. DDL text: `SHOW CREATE TABLE {db:Identifier}.{t:Identifier}` or system.tables.create_table_query; view definition: as_select. Dictionaries: system.dictionaries; user functions: `system.functions WHERE origin = 'SQLUserDefined'`. Completion: `SELECT database, table, name FROM system.columns WHERE database NOT IN ('system','INFORMATION_SCHEMA','information_schema') ORDER BY database, table, position`. Processes: `SELECT query_id, user, elapsed, read_rows, memory_usage, query FROM system.processes`; mutations: `SELECT mutation_id, command, is_done, latest_fail_reason FROM system.mutations WHERE database = {db:String} AND table = {t:String} AND is_done = 0`. Users/roles: system.users, system.roles, system.grants. Parameters go as param_db= / param_t=; add SYSTEM. to CATALOG_WORDS (drivers/mod.rs). An information_schema emulation also exists but system.\* is richer.

**Testing without a server.**

Mostly offline: a tiny tokio HTTP/1.1 responder in unit tests (same idea as the minimal PostgreSQL server in drivers/postgres/connect.rs) serving canned JSONCompactEachRowWithNamesAndTypes bodies, X-ClickHouse-Summary / Exception-Code / Exception-Tag headers and a mid-stream \__exception__ block; type-string parser and decoder tests built from the TablePlus issue cases (LowCardinality(Nullable(String)), Map, Nested, Decimal, DateTime64('Asia/Jakarta'), UInt256); sqlgen tests for mutations and {p:Type} parameters. Real server: Docker clickhouse/clickhouse-server (add to dev/docker-compose.yml with 8123/8443) or the single binary inside WSL; no native Windows server; ClickHouse Cloud trial for HTTPS 8443.

**What it shares with an existing driver.**

Not for the main path. ClickHouse can also listen on a MySQL wire port (mysql_port, 9004 in the docs example; 3306 on Cloud once enabled) and a PostgreSQL wire port, but its docs say prepared queries are not supported, some types come back as strings, the SSL implementation 'might not be fully compatible', and it is not guaranteed to be a drop-in replacement. The app's mysql_async driver binds parameters through prepared statements and runs MySQL bootstrap/introspection (SET NAMES, @@sql_mode, information_schema), so reusing Driver::Mysql would only be a lossy read-mostly stopgap (UInt64, Int128, Array, Map).

**Connection fields.**

- Host
- Port (8123 HTTP / 8443 HTTPS, default chosen by SSL mode)
- SSL mode (HTTP, HTTPS, HTTPS with CA / full verification)
- User (default `default`)
- Password with storage mode
- Database (default `default`)
- CA file, client key, client certificate
- Role(s) (optional)
- Extra settings key=value sent as URL settings (like pg_params, types/profile.rs)
- Session timeout
- Cluster name (optional, for ON CLUSTER DDL)
- Read-only (existing read_only; readonly=2 on the server side keeps settings changeable)
- Compression (Accept-Encoding gzip/zstd)
- Connect / query timeout
- SSH tunnel (existing)

**Effort:** L.

**Risks.**

- HTTP 200 can still end in an error: the mid-stream \__exception__ block must be detected or a partial result looks complete.
- Multi-statement requests are rejected; a FORMAT clause in user SQL conflicts with the grid format.
- No transactions: staged changes apply one by one; mutations are asynchronous, return no row counts, cannot touch key columns, and cannot be limited to one row, so duplicate rows are all updated or deleted.
- readonly=1 users may not change settings, so sending output_format_\* or session settings fails for them ('Cannot modify setting in readonly mode'): detect and fall back to server defaults (64-bit integers are quoted by default, decimals are not).
- Sessions (session_id) live on one server node and expire after 60 s idle by default; behind a load balancer or ClickHouse Cloud with several replicas a session or a KILL QUERY can land on another node.
- Large type zoo (AggregateFunction binary states, Dynamic/Variant/JSON, Nested, Int256, geo types): the source of most TablePlus ClickHouse crashes, so the type parser must never panic.
- DateTime values are rendered in the column or server time zone; copy/paste round-trips need the exact text (TablePlus #3604, #3921).
- Query parameters travel in the URL (1 MiB default limit), so huge IN filters fail.
- Lightweight UPDATE needs 25.7+ and table settings; older servers only have ALTER TABLE ... UPDATE mutations, which rewrite whole parts and are slow on big tables.
- No native Windows ClickHouse server for local development.

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._
