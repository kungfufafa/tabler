# Cassandra and ScyllaDB: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                                                    |
| ----------------- | ---------------------------------------------------------------------------------- |
| Lane              | `cassandra`                                                                        |
| Databases         | Cassandra, ScyllaDB                                                                |
| Drivers           | `cassandra`                                                                        |
| Picker kinds      | `cassandra`                                                                        |
| SQL dialect       | `cql`                                                                              |
| Cargo features    | `drv-cassandra`                                                                    |
| Home slot         | `s3`                                                                               |
| Bindings prefix   | `Cassandra*`                                                                       |
| Lib test filters  | `drivers::cassandra`, `sqlgen::dialect::cql`, `sqlgen::portable::generated::cql::` |
| Real-server suite | `tests/cassandra.rs` with `TABLER_TEST_CASSANDRA_URL`                              |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/cassandra/**`
- `src-tauri/src/sqlgen/dialect/cql.rs`
- `src-tauri/src/sqlgen/portable/gen/cql.rs`
- `src-tauri/tests/cassandra.rs`
- `src-tauri/tests/fixtures/cassandra/**`
- `src-tauri/tests/fixtures/classify-generated/cql.json`
- `src/features/drivers/cassandra/**`
- `src/lib/ipc/mock/drivers/cassandra/**`
- `src/lib/ipc/bindings/Cassandra*.ts`
- `src/lib/ipc/bindings/lex/cql.json`
- `docs/drivers/cassandra.md`

Anything else (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead. `node dev/scripts/lane-commit.mjs cassandra -m "cassandra: …"` refuses other paths.

## Scope of v1

### Cassandra

- **Identity:** `cassandra`, `SqlDialect::Cql`. Crates: scylla 1.9 (rustls-023).
- **UI:** wide-column grid through `table_source` (paging state).
- **Auth v1:** none or password.
- **Required fields:** host (contact point), 9042; database = keyspace.
- **Out of scope v1:** Astra bundle, Keyspaces SigV4, multi-node over SSH, protocol v5, counter edits.
- **Test:** CqlValue mapping, CQL goldens, `system_schema` fixtures. No Windows server.

## Descriptor values

Seeded from the plan (§5.2). The values in force are the descriptors in `src-tauri/src/types/driver.rs` (keepalive belongs to the session's caps); a change is a request to the lead.

| Driver    | Location | Port (TLS) | SSH                  | TLS style | Secret label | Command TTL |
| --------- | -------- | ---------- | -------------------- | --------- | ------------ | ----------- |
| cassandra | Host     | 9042       | Tunnel (single node) | OnOff     | Password     | 600         |

| Driver    | Required fields / params | Label overrides    | Keepalive | Read-only      | Sharing | Token secret |
| --------- | ------------------------ | ------------------ | --------- | -------------- | ------- | ------------ |
| cassandra | host                     | database: Keyspace | 30        | ClassifierOnly | Indep   | no           |

## Read-only and hardening

| Engine                                                                            | When read-only                                                                                                      | When unattended (MCP)      |
| --------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- | -------------------------- |
| D1, SQL Server (ApplicationIntent=ReadOnly as routing only), Snowflake, Cassandra | ClassifierOnly, default-deny. T-SQL uses `terminator_optional`; Snowflake `EXECUTE IMMEDIATE` and `CALL` are writes | MCP only if caps grant Mcp |

Every classifier is default-deny: a statement or command it does not know is a write. The read-only mode in the caps (`Server`, `PerRequest` or `ClassifierOnly`) must say what the session really enforces.

## What to implement

- `entry.rs` (Phase B): `open`, `validate`, `util`, `util_kind` (plan §2.5). `<Prefix>Params` (`#[serde(rename_all = "camelCase", default)]`, ts-exported) parsed from `options.driver_params`.
- `DbSession`: the 18 required methods; overrides of `query_cancellable`, `execute_cancellable`, `drain_trace`, `sql_trace` and `editor_run_lock`; `dialect()`, `capabilities()` (accurate features, data, safety and lifecycle caps) and `flavor()` where it applies.
- Optional: `table_source()`, `call` / `call_kind`, `list_processes` / `kill_processes`, `table_ddl`, `explain_form`.
- Cancel through `CancelHandle::Custom(Arc<dyn CancelAction>)`; `cancel()` calls `cancel_portable` first.
- Renderer: a `KindUi` per tile, `DriverUi`, `DialectUi` and a `MockDriver`. Write-capable `sessionCall` / `driverUtil` helpers await `guardExecution` first.

## Decisions and corrections for this lane

- scylla is built with `rustls-023` and the `num-bigint-04` / `bigdecimal-04` features, and `drv-cassandra` turns on `num-bigint` and `bigdecimal` as direct dependencies (F3a, critique B4).
- Over SSH the session pins one node: HostFilter plus AddressTranslator (plan A27).
- Writes prove their count with lightweight transactions (`IF EXISTS`); without one EditRows is off (plan A13). Counter columns are not editable.
- Read-only is ClassifierOnly and default-deny (plan §5.3). `count` is Costly.
- Browsing pages with the driver's paging state through `table_source()`.

## Phase A status

Done, in the lane's own files (nothing calls them yet; `entry.rs` is untouched):

- **`sqlgen/dialect/cql.rs`**: the spec. Strings take no backslash escapes, `$$` bodies have no tags, `//` and flat `/* */` comments. Reads are `SELECT`, `DESCRIBE` / `DESC`, `LIST`, `USE` and cqlsh's `CONSISTENCY`, `SERIAL`, `TRACING`, `PAGING`, `EXPAND`, which the session handles itself and the server refuses. Everything else writes. `hooks.split` is `split_cql`, which keeps `BEGIN [UNLOGGED | COUNTER] BATCH … APPLY BATCH` as one statement. Server expressions are `now()` for timeuuid, `toTimestamp(now())` and `toDate(now())`. Redaction covers `PASSWORD` (also `HASHED PASSWORD` and `$…$` values) and cqlsh `LOGIN user '…'`, tested in `redacts_passwords_and_cqlsh_login`. Also here: DDL text rebuilt from `system_schema` (`create_table`, `create_view`, `create_index`, `create_trigger`, `map_literal`, `describe_table`).
- **`sqlgen/portable/gen/cql.rs`**: the complete generated suite (`check()` and `fuzz(3000)`), with the engine's read corpus, write forms (batches, LWTs, `USING TTL` / `TIMESTAMP`, DDL, roles, Cassandra 5 identities, ScyllaDB `PRUNE`, cqlsh file commands), CQL-only contexts and fuzz atoms. Its fixture `tests/fixtures/classify-generated/cql.json` is blessed. The red-team review is still to come.
- **`drivers/cassandra/values.rs`**: decoders from `CqlValue` to `Value`, total over every CQL type. Integers beyond 2^53 become `BigInt`, varint is exact, and decimals are exact text (negative values and any 32-bit scale, so `1E-2147483647` stays short). Timestamps are UTC text with milliseconds over all of `i64`. Dates cover the whole 2^31-offset range. Times are nanoseconds, durations use the `1y2mo3d…` form, inet and empty values become `Other`, and collections, tuples, UDTs and vectors become JSON in `toJson()`'s shape: a map key whose JSON is already a string (text, uuid, timestamp, inet, blob `0x…`) stays as it is, and any other key (a number, a tuple) is quoted, as Cassandra's `MapType` does. `result_column` builds a `ResultColumn` from a `ColumnSpec`.
- **`drivers/cassandra/types.rs`**: a parser of `system_schema` type texts (`frozen<map<text, frozen<list<int>>>>`, quoted UDTs, `vector<float, 3>`, custom classes) that never fails, plus `ValueKind`s and type texts of result types.
- **`drivers/cassandra/catalog.rs`**: the `system` / `system_schema` queries (no aliases) and their parsers:
  - keyspaces as databases, with the system and DSE ones hidden;
  - tables and materialized views;
  - columns in key order: partition, then clustering by position, then static and regular by name, with `pk_ordinal` and `extra`;
  - indexes (secondary, SAI, SASI, custom, ScyllaDB local JSON targets) and triggers;
  - table options and the rebuilt DDL, with a server `DESCRIBE` text winning when there is one;
  - size estimates (saturating), the local node, peers, ScyllaDB's version, completion and UDTs.
- **`drivers/cassandra/cursor.rs`**: the paging state as the grid's cursor (`cql1.<base64url>`), bound by a fingerprint to the statement and values that made it. A cursor from another filter or sort is refused.
- **`drivers/cassandra/cqlsh.rs`**: a strict parser of the cqlsh session commands (`CONSISTENCY`, `SERIAL CONSISTENCY`, `TRACING`, `PAGING`, `EXPAND`), giving the driver's `Consistency` levels. A statement counts as one of them only when it is exactly that command (one trailing `;` is allowed, as in cqlsh), a command followed by other text is refused, and anything else goes to the server.
- **`drivers/cassandra/testing.rs`**: cell builders and the `system_schema` rows of a Cassandra 4.1 table.

Left for Phase B:

- `entry.rs` (open, validate, util), `CassandraParams`, the `DbSession` and `TableSource` implementations.
- The encoder from `Value` back to `CqlValue` for binds.
- The select, count, fetch and change hooks (AND-only filters, `LIMIT` before `ALLOW FILTERING`, LWT `IF EXISTS` edits, counter deltas, batches).
- Running the cqlsh commands in the session, cancel, consistency, tracing, the renderer (whose splitter must keep batches whole too), the mock, and the real-server suite.
- `lane.mjs cassandra bindings` once the tree builds: `src/lib/ipc/bindings/lex/cql.json` is still F4's, without the cqlsh inspections.

## Definition of done (this lane)

Beyond the shared list in [README.md](README.md#definition-of-done-v1):

- **Classifier hardening (blocks landing).** `src-tauri/src/sqlgen/portable/gen/cql.rs` is complete and `src-tauri/tests/fixtures/classify-generated/cql.json` blessed, as [Classifier hardening](README.md#classifier-hardening) requires: every write keyword, write call and read starter of `dialect::cql::SPEC` in every shared context and in the contexts only this grammar has, an engine-realistic read corpus, its write forms and fuzz atoms, `suite().check()` and `suite().fuzz(…)` in place of the placeholder. Then a red-team review passes, each finding fixed and kept as a case. Known hard cases: `BEGIN [UNLOGGED | COUNTER] BATCH … APPLY BATCH` as one write; lightweight transactions (`INSERT … IF NOT EXISTS`, `UPDATE … IF …`, `DELETE … IF EXISTS`); `USING TTL` / `TIMESTAMP` writes; `$$…$$` strings as an inert context; `SELECT … ALLOW FILTERING`, `SELECT JSON` and `DESCRIBE` / `LIST` as reads.

## Research notes

> Background from the driver research (October 2026), with file references and no line numbers. Verify against the code; where it differs from the scope above or from README.md, those win.

### Apache Cassandra

**Approach.**

Use `scylla` 1.9.0 (ScyllaDB's official Rust driver, published 2026-09-15). It supports Apache Cassandra 3.0+ and ScyllaDB over CQL native protocol v4. Configure it as `scylla = { version = "1.9.0", features = ["rustls-023", "num-bigint-04", "bigdecimal-04"] }`; the two numeric features are only needed for varint/decimal text, which can also be hand-converted.

Session setup: `SessionBuilder::new()` with:

- `.known_node(addr)`, `.user(u, p)` or `.authenticator_provider(..)`;
- `.tls_context(Some(..))` from `Arc<rustls::ClientConfig>`, built with Tabler's verifier so a tunnel can check the real host name;
- `.connection_timeout(..)`, `.compression(Some(Compression::Lz4))`, `.use_keyspace(ks, true)`;
- `.host_filter(..)` + `.address_translator(..)`, which pin everything to the tunnelled node;
- a default execution profile: `ExecutionProfile::builder().consistency(..).serial_consistency(..).request_timeout(..).load_balancing_policy(DefaultPolicy::builder().prefer_datacenter(dc).build())`.

Reading data:

- Grid pages: `Session::query_single_page(stmt, values, PagingState)` → `(QueryResult, PagingStateResponse)`. `into_paging_control_flow()` yields the next PagingState, carried as an opaque base64 cursor.
- Editor runs and exports stream through `query_iter` / `execute_iter`.
- Rows are read untyped as `Row` / `Vec<Option<CqlValue>>` with their column specs.
- Parameters use prepared statements (`prepare` / `execute_unpaged`).
- DDL is followed by `await_schema_agreement()`.

Cancel: CQL has no server-side cancel. Drop the future; the server stops at its own read/write timeout.

Alternative: `cdrs-tokio` 9.0.2 (May 2026; protocol v4/v5, rustls). It has a smaller community and less metadata tooling, so scylla is preferred.

**Windows build.**

Pure Rust with `rustls-023`, which declares rustls 0.23 with default features (aws_lc_rs). That matches Cargo.lock's rustls 0.23.45, which uses aws-lc-rs only, so there is no provider clash. tokio-rustls 0.26 is already in the lock. Compression (lz4_flex, snap) is pure Rust. Do not enable `openssl-010`: there is no system OpenSSL on Windows.

MSRV 1.88. Added dependencies include scylla-cql, scylla-macros (proc-macro), histogram, arc-swap, dashmap, itertools, rand, socket2 and tracing. uuid and chrono are already present. Expect about 40-70 s for a clean debug build and 3-5 MB added to the release binary.

**Authentication.**

TablePlus offers username/password, SSL with a 'VERIFY NONE' option that is reported as not respected (#2214), protocol selection (b396), a keyspace field and SSH. It has no consistency-level setting, which users have asked for repeatedly (#2342 with many +1s, #3478, Windows #557).

Tabler should offer:

- None.
- PasswordAuthenticator (`.user(u, p)`), which also covers DSE plain auth and Astra token-style 'token' / 'AstraCS:…' (Astra itself still needs its bundle).
- TLS: reuse SslMode with a CA file and client cert/key for mTLS.
- AWS Keyspaces: service-specific credentials (plain user/password), or SigV4 through a custom `AuthenticatorProvider` that answers the Keyspaces SigV4 nonce challenge, sharing the DynamoDB signer.
- Consistency and serial consistency per connection: default LOCAL_ONE, LOCAL_QUORUM for Keyspaces.
- cqlsh-style `CONSISTENCY` / `SERIAL CONSISTENCY` lines in the editor.

**Data model.**

How TablePlus shows Cassandra:

- A keyspace is the database: the form has a keyspace field and the 'Open database' dialog lists keyspaces.
- It lists tables with `SELECT keyspace_name as table_schema, table_name as table_name FROM system_schema.tables where keyspace_name='…'`; the alias broke DataStax Astra (#2338).
- The grid runs `SELECT * FROM "t" WHERE … LIMIT 300`. Next/previous page did not work (#743).
- The filter bar generates CQL (escaping fixed in #2332) and supports ALLOW FILTERING (b250).
- Row edits are committed with Cmd+S, including counter updates (b174).
- Copy as INSERT gets frozen UDT literals wrong (#1508).
- The structure view lists PK columns alphabetically instead of in partition/clustering order (Windows #49).

How Cassandra maps onto DbSession:

- list_databases → non-system keyspaces.
- list_schemas → [current keyspace] (MySQL pattern, drivers/mysql/introspect.rs). switch_database should use `use_keyspace(ks, true)` instead of the full reconnect in commands/connections.rs.
- list_tables → tables (Table) and materialized views (View).
- table_meta → columns:
  - ordered partition key (by position), then clustering (by position), then static, then regular — never alphabetical;
  - is_pk true for partition and clustering columns, with sequential pk_ordinal;
  - extra = 'partition key' / 'clustering ASC|DESC' / 'static';
  - data_type = the CQL type text (e.g. `frozen<list<text>>`).
  - indexes from system_schema.indexes; triggers from system_schema.triggers; foreign_keys and checks empty.
  - row_estimate from system.size_estimates.
  - ddl from server-side DESCRIBE.
  - comment and options carry the table options.
- completion_metadata → keyspace → table → columns, a natural fit.

CqlValue → Value:

- Ascii/Text → Text.
- Int/SmallInt/TinyInt → Int; BigInt/Counter → Int or BigInt; Varint → BigInt text.
- Decimal → Decimal, correct for negatives too (TablePlus #2191).
- Float/Double → Float; Boolean → Bool; Blob → Bytes.
- Uuid/Timeuuid → Uuid.
- Timestamp → TimestampTz with millisecond precision; dropping it broke key matching in TablePlus Windows #391.
- Date → Date (unsigned days with a 2^31 offset); Time → Time (nanoseconds).
- Duration → Interval (CQL `1mo2d3ns`).
- Inet → Other.
- List/Set/Map/Tuple/UDT/Vector → Json for display; emit CQL literals for copy-as-INSERT and writes.

Shared plumbing: browse needs the native-browse hook, or a `Dialect::Cql` in sqlgen, plus BrowseRequest.cursor/BrowsePage.next_cursor for the paging state (see Redis). Counts use the estimate path (commands/browse.rs) by default; COUNT(\*) only on 'Count exactly'.

New UI beyond the grid:

- A consistency selector in the editor footer and status bar, also showing the level each statement ran at.
- A partition-key-aware filter: it signals when a filter forces ALLOW FILTERING, and sorting is only allowed on clustering columns once the partition key is pinned.
- A query tracing view: `TRACING ON` → `Statement::set_tracing(true)` + `session.get_tracing_info(id)`.
- A keyspace replication editor.
- UDTs, functions and aggregates as sidebar objects.

**Query language.**

CQL 3 text, sent as-is. Unprepared statements go through `query_unpaged`/`query_iter`; statements with parameters are prepared first.

Client-side cqlsh commands, never sent to the server:

- `CONSISTENCY <level>` and `SERIAL CONSISTENCY <level>`.
- `USE ks`, handled through the driver's use_keyspace.
- `TRACING ON|OFF`, `PAGING ON|OFF|n`, `EXPAND`.
- `DESCRIBE …`: server-side on Cassandra 4.0+ / ScyllaDB 5.2+; otherwise rebuilt from system_schema.

Auto-limit must insert `LIMIT n` before a trailing `ALLOW FILTERING` (TablePlus Windows #429).

Splitting: `;` while respecting `'…'` strings, `$$…$$` UDF bodies and `--`, `//` and `/* */` comments. Add a Cql dialect to sqlgen/split.rs.

Editor: CodeMirror lang-sql with a CQL SQLDialect (keywords KEYSPACE, ALLOW FILTERING, USING TTL, WRITETIME, TTL(), FROZEN, COUNTER, IF NOT EXISTS, APPLY BATCH, MATERIALIZED VIEW), added next to CM_DIALECTS in src/features/editor/cmExtensions.ts.

Safe mode: guardExecution can reuse SQL keyword classification (SELECT is a read).

**Editing.**

Add `Dialect::Cql` to sqlgen/quote.rs. Identifiers are always double-quoted with `""` doubling; this fixes mixed-case keyspaces (TablePlus #2977). Literals use `'…'` with `''` doubling and no backslash escapes; blobs are written 0x…, uuids and numbers unquoted. Then teach sqlgen/mutate.rs build_change:

- Update → `UPDATE "ks"."t" SET "c" = ? WHERE <every PK column> = ? [IF EXISTS]`. CQL UPDATE is an upsert, so without IF EXISTS it can resurrect a deleted row.
- Insert → `INSERT … [IF NOT EXISTS]`.
- Delete → `DELETE FROM … WHERE <full PK> [IF EXISTS]`.
- No RETURNING, and no affected-row counts. expect_affected is only possible through LWT `[applied]`, as an opt-in 'safe edits' mode that runs Paxos at about 4× latency.
- Counters → `SET c = c + ?` with the delta. Collections are replaced whole. Timestamps keep milliseconds. PK columns are read-only; changing one means delete + insert.

Commit: `BEGIN BATCH … APPLY BATCH` through a scylla `Batch`. A logged batch eventually applies everything but is not isolated. Counters need a COUNTER BATCH and cannot be mixed in. LWT batches must stay within one partition. This maps loosely onto DbSession::transaction; document that it is not ACID.

DDL:

- CREATE/ALTER/DROP KEYSPACE with the replication map.
- CREATE TABLE with partition key, clustering columns, CLUSTERING ORDER and table options.
- ALTER TABLE ADD/DROP/RENAME (only PK columns can be renamed) / WITH options.
- CREATE INDEX, or `CREATE CUSTOM INDEX … USING 'StorageAttachedIndex'` (SAI, 5.0).
- CREATE TYPE, CREATE MATERIALIZED VIEW, TRUNCATE, DROP.
- Then `await_schema_agreement()` before re-introspecting.

**Introspection.**

Server:

- `SELECT release_version, cql_version, native_protocol_version, cluster_name, data_center, rack, partitioner FROM system.local`.
- Peers: `SELECT peer, data_center, rack, release_version, rpc_address FROM system.peers`, or system.peers_v2 on 4.0+.

Keyspaces: `SELECT keyspace_name, durable_writes, replication FROM system_schema.keyspaces`. Hide system, system_auth, system_distributed, system_schema, system_traces, system_views, system_virtual_schema, dse_\*, solr_admin and system_multiregion_info (Keyspaces).

Tables: `SELECT table_name, comment, default_time_to_live, gc_grace_seconds, compaction, compression, caching, bloom_filter_fp_chance, speculative_retry FROM system_schema.tables WHERE keyspace_name = ?`. Use no column aliases: they break Astra (#2338).

Views: `SELECT view_name, base_table_name, where_clause, include_all_columns FROM system_schema.views WHERE keyspace_name = ?`.

Columns: `SELECT table_name, column_name, kind, position, clustering_order, type FROM system_schema.columns WHERE keyspace_name = ? AND table_name = ?`. kind is partition_key, clustering, static or regular; order key columns by position.

Indexes: `SELECT index_name, kind, options FROM system_schema.indexes WHERE keyspace_name = ? AND table_name = ?`, where options.target / options.class_name identify SASI and SAI.

Triggers: `SELECT trigger_name, options FROM system_schema.triggers WHERE keyspace_name = ? AND table_name = ?`.

UDTs: `SELECT type_name, field_names, field_types FROM system_schema.types WHERE keyspace_name = ?`.

Functions and aggregates: system_schema.functions (function_name, argument_names, argument_types, return_type, language, body, called_on_null_input) and system_schema.aggregates.

Row estimate: `SELECT range_start, range_end, partitions_count, mean_partition_size FROM system.size_estimates WHERE keyspace_name = ? AND table_name = ?`, summed; 4.0+ also has system.table_estimates.

DDL text: `DESCRIBE TABLE "ks"."t"` / `DESCRIBE KEYSPACE "ks"` server-side, or rebuilt from the rows above including CLUSTERING ORDER and options.

The driver's `session.get_cluster_state()` metadata is a fallback.

**Testing without a server.**

Unit tests:

- CqlValue → Value for every type: negative decimal/varint, duration, date with the 2^31 offset, pre-1970 timestamps, timeuuid, nested frozen UDTs.
- CQL generation with Dialect::Cql: quoting of mixed-case and reserved names, LIMIT before ALLOW FILTERING, full-PK WHERE, counter deltas, batch assembly.
- system_schema fixture rows → TableMeta with the correct PK order.
- Paging-cursor encode/decode.
- The cqlsh meta-command parser and the CQL splitter ($$ bodies).

Integration:

- `cassandra:5` (about 60 s startup, 1-2 GB RAM) or `scylladb/scylla --smp 1 --developer-mode 1` (fast), as a `cassandra` profile in dev/docker-compose.yml, plus a TLS variant using dev/certs.
- Apache Cassandra dropped Windows support in 4.0, so use Docker or WSL.
- AWS Keyspaces, Cosmos DB and Astra only as manual checks.

**What it shares with an existing driver.**

Not wire-compatible with PG or MySQL: CQL has its own binary protocol. But CQL is SQL-shaped, so a `Dialect::Cql` can reuse the sqlgen/select.rs build_select/build_where and sqlgen/mutate.rs builders, with these gaps:

- no OFFSET (cursor paging instead);
- no OR; no IS NULL / IS NOT NULL;
- != only in limited forms on new versions;
- LIKE only with SASI/SAI;
- ORDER BY only on clustering columns with the partition key restricted;
- no JOIN or subquery; GROUP BY only on a partition/clustering prefix;
- no NULLS FIRST/LAST;
- non-key filters need ALLOW FILTERING;
- COUNT(\*) is a full scan;
- no RETURNING, no transactions, no affected counts.

The same driver serves ScyllaDB (shard-aware), Cassandra 3/4/5, DataStax Enterprise, AWS Keyspaces (9142 TLS; LOCAL_QUORUM writes; SigV4), Azure Cosmos DB for Apache Cassandra (10350 TLS) and YugabyteDB YCQL (TablePlus b542 added JSONB for it). It does not serve DataStax Astra, which needs a Secure Connect Bundle / SNI proxy.

**Connection fields.**

- Name / color / tag (existing)
- Contact point host(s), comma-separated
- Port (9042; AWS Keyspaces 9142; Azure Cosmos DB 10350)
- Username
- Password + storage mode (keychain/ask/none/command)
- Keyspace (the 'Database' field, as in TablePlus)
- Local datacenter (load balancing; needed for multi-DC clusters and Keyspaces)
- Consistency (ANY, ONE, TWO, THREE, QUORUM, ALL, LOCAL_QUORUM, EACH_QUORUM, LOCAL_ONE)
- Serial consistency (SERIAL / LOCAL_SERIAL)
- Request timeout (TablePlus users hit ~5 s timeouts, #883)
- Page size (fetch size)
- Compression (none / LZ4 / Snappy)
- TLS mode (reuse SslMode) + CA file + client certificate/key
- Auth provider: PasswordAuthenticator / none / AWS SigV4 (Keyspaces)
- Single-node mode (forced over SSH: HostFilter + AddressTranslator)
- SSH tunnel (existing)
- Read only / safe mode / startup CQL (existing)
- Protocol version: TablePlus shows a selector; scylla speaks v4 only, so show it read-only
- URL import: cassandra://user:pass@host:9042/keyspace (TablePlus scheme)

**Effort:** L.

**Risks.**

- Protocol v4 only: Cassandra 2.1 and older (protocol v3) are unsupported, and pre-3.0 servers lack system_schema. Support 3.0+ explicitly.
- DataStax Astra does not connect: its Secure Connect Bundle uses SNI routing that scylla does not support. AWS Keyspaces needs a custom SigV4 authenticator, the Starfield/Amazon root CA and LOCAL_QUORUM.
- SSH tunnels: the driver discovers peers from system.peers and dials their RPC addresses, which are unreachable through the tunnel. HostFilter + AddressTranslator must pin everything to the tunnelled node, and the TLS verifier must check the real host name instead of the connected IP.
- No OFFSET: Next/Previous needs cursor paging (TablePlus #743 never worked), and jumping to page N reads every page before it.
- Expensive operations: COUNT(\*) and ALLOW FILTERING scan the whole cluster and hit timeouts (TablePlus #883). Default to size_estimates and make the request timeout configurable.
- Write semantics: UPDATE/INSERT are upserts with no affected count, DELETE writes tombstones, and LWT (IF EXISTS) costs Paxos rounds and cannot be mixed into multi-partition batches. The usual 'changed by someone else' guard is opt-in and costly.
- Type fidelity: timestamp milliseconds (a key mismatch inserts a new row, Windows #391), decimal/varint encoding (many TablePlus fixes), frozen UDT and collection literals (#1508), and counters, which cannot be set directly.
- Consistency control is a must (TablePlus's long-standing gap), and wrong defaults fail on Keyspaces and multi-DC clusters.
- Schema agreement must be awaited after DDL before re-introspecting, or the sidebar shows stale tables.

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._
