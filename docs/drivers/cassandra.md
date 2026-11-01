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
- **`sqlgen/portable/gen/cql.rs`**: the complete generated suite (`check()` and `fuzz(3000)`), with the engine's read corpus, write forms (batches, LWTs, `USING TTL` / `TIMESTAMP`, DDL, roles, Cassandra 5 identities, ScyllaDB `PRUNE`, cqlsh file commands), CQL-only contexts and fuzz atoms. Its fixture `tests/fixtures/classify-generated/cql.json` is blessed. The red-team review passed at landing (Phase B status).
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

## Phase B status

Part B1 (the backend driver) and part B2 (the renderer and the mock) are done.

The lane landed in the default build (`release-drivers`) on 2026-11-01, with no real-server run (none was available).

Red-team review of the classifier (reviewers other than the lane, trying to get a write read as a read): passed with no finding. Part B1's safety review (default-deny with CR-ended comments, several statements and empty text; raw filters with `;` or write words refused; the strict cqlsh parser), part B2's (50 classifier cases through the renderer's mirror over `lex/cql.json` and 24 crafted writes against the read-only mock), and the lead at landing: 70 writes through `dialect::cql::SPEC.classify`, each a write (batches of every kind, LWTs, `USING TTL` / `TIMESTAMP`, roles, Cassandra 5 identities, ScyllaDB `PRUNE`, cqlsh file commands and `LOGIN`, a BOM or NBSP first, `$$` bodies empty, glued and inside comments, strings and quoted names, flat block comments, line comments ended by CR, LF or U+2028, blob and exponent literals, bind markers, `WITH`, `EXPLAIN` and `CALL` as unknown statements). Read by design: an inspection or cqlsh command followed by other text in the same statement (`USE ks` / `TRACING ON` / `DESC t` then a write on the next line), since a node parses exactly one statement per request and refuses the rest as a syntax error; the session's own `USE` passes only a bare or quoted name, which the scylla driver checks again (`[A-Za-z0-9_]`, at most 48) before sending.

**What works.**

- **`entry.rs`**: `open` builds a `Dial` from the profile (`dial`): the contact point (the tunnel's local end through SSH), TLS by the SSL mode with the CA file and client certificate, checked for the profile's host (`ConnectTarget::tls_host`), PasswordAuthenticator when the profile has a user, the keyspace (the profile's database, matched exactly as the keyspace picker lists it), and `CassandraParams`. `validate` parses `CassandraParams` and wants both files of a client certificate. `util` knows no call; `util_kind` is `Write`.
- **`params.rs`**: `CassandraParams` (ts-exported with `CassandraConsistency`, `CassandraSerialConsistency`, `CassandraCompression`): consistency (default `LOCAL_ONE`), serial consistency (`LOCAL_SERIAL`), local datacenter, request timeout (30 s), editor page size (100, as cqlsh), compression (none, LZ4, Snappy), single node.
- **`client.rs`**: the scylla side. The contact point is resolved here and given to the driver as an address, because the driver names a node to rustls by its IP address: `tls_client::rustls_config_through` checks that address's certificate for the profile's host. Single node (always over SSH): a host filter keeps only the contact point and an address translator sends any peer back to it. A Command password is read by an own `AuthenticatorProvider` for each new connection (`CommandPassword`, TTL 600 s). Schema metadata is not fetched by the driver (the catalog reads `system_schema` when asked). Values are bound through a prepared statement whose markers give their types (`encode.rs`).
- **`session.rs`** (`DbSession` + `TableSource`) over the `wire.rs` seam:
  - introspection: keyspaces (system ones hidden), the current keyspace as the only schema (`SchemaMode::DatabaseIsSchema`), tables and materialized views, structure in key order with indexes, triggers, options and the size estimate, Show DDL (the server's `DESCRIBE` on Cassandra 4.0+ / ScyllaDB 5.2+, else rebuilt from `system_schema`), view definitions, completion;
  - the grid through `table_source()`: pages with the driver's paging state as the cursor (`cursor.rs`, bound to the statement and values), AND-only filters with `ALLOW FILTERING`, the size estimate as the count until an exact `count(*)` is asked for, whole cells by full primary key;
  - edits as lightweight transactions by the full primary key (`UPDATE … IF EXISTS`, `INSERT … IF NOT EXISTS`, `DELETE … IF EXISTS`), sent one by one; `[applied]` = false fails the change with a message that says why, and the changes before it stay applied (`TxMode::Sequential`);
  - the editor (`editor.rs`): cqlsh's `CONSISTENCY`, `SERIAL CONSISTENCY`, `TRACING`, `PAGING`, `EXPAND` are the session's own and never sent; `USE` switches every connection; anything else runs at the editor's consistency, page by page up to the row cap; DDL waits for schema agreement; server warnings and a traced statement's trace come back as notices;
  - cancel: a `CancelHandle::Custom` that drops the run on this side (CQL has no server cancel; the node finishes within its own timeout); the query time out goes through it;
  - read-only (`ClassifierOnly`, from `conn_options::effective_read_only`): every statement the CQL classifier calls a write, or does not know, is refused before it is sent on every path (query, execute, transaction, editor, stream, startup commands); a read-only profile whose startup commands write does not open;
  - startup commands: cqlsh commands, `USE` and CQL statements, at connect;
  - keep-alive: `SELECT key FROM system.local WHERE key = 'local'` every 30 s, answered by the node itself.
- **`sqlgen/dialect/cql.rs`**: the grid's statements (`where_clause`, `browse_select`, `count_select`, `fetch_cell_select`) and the spec's hooks (select with `LIMIT` before `ALLOW FILTERING`, count, fetch, change).
- **`errors.rs`**: bad credentials → `Auth`, a lost connection → `NotConnected`, the driver's request timeout → `Timeout`, server errors → `Db` with the CQL error code in `vendor_code`.

**Caps granted, and the test that proves each** (`drivers::cassandra::tests` unless named):

| Feature / cap                                                        | Test                                                                                                                                                     |
| -------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Editor`                                                             | `the_editor_pages_to_its_cap_and_runs_cqlsh_commands_itself`, `traced_runs_bring_their_trace_back_as_notices`                                            |
| `StructureView`                                                      | `lists_keyspaces_tables_and_their_structure`, `browses_page_after_page_with_the_paging_state`, `counts_by_estimate_unless_asked_and_fetches_whole_cells` |
| `EditRows`, `InsertRows`, `DeleteRows`                               | `a_commit_stops_at_the_change_that_did_not_apply`; `sqlgen::dialect::cql::tests::edits_are_lightweight_transactions_by_full_key`                         |
| `ServerExpressions` (`now()`, `toTimestamp(now())`, `toDate(now())`) | `edits_are_lightweight_transactions_by_full_key`, `server_expressions_are_the_cql_clocks`                                                                |
| `ExactCount`, `CountMode::Costly`                                    | `counts_by_estimate_unless_asked_and_fetches_whole_cells`                                                                                                |
| `TableDdl`, `ViewDefinition`                                         | `lists_keyspaces_tables_and_their_structure`                                                                                                             |
| `SwitchDatabase` (a new session in the keyspace)                     | `connects_with_the_profile_and_reads_who_the_node_is` (the dial's keyspace), `entry::tests::dial_reads_the_profile`                                      |
| `StartupCommands`                                                    | `startup_commands_run_at_connect`, `read_only_refuses_every_write_before_sending_it`                                                                     |
| `Paging::Cursor`, no sort, AND-only filter ops, raw WHERE            | `browses_page_after_page_with_the_paging_state`, `filters_with_and_and_allow_filtering_only`, `the_grid_selects_with_limit_before_allow_filtering`       |
| `TxMode::Sequential`, `Affected::Exact` (the LWT's `[applied]`)      | `a_commit_stops_at_the_change_that_did_not_apply`                                                                                                        |
| `ReadOnlyMode::ClassifierOnly`                                       | `read_only_refuses_every_write_before_sending_it`                                                                                                        |
| keep-alive 30 s                                                      | `caps_grant_what_the_session_does`                                                                                                                       |

Not granted: `CancelOnServer` (CQL has none), `Explain` (none either), `EditWithoutKey` (every row has a key, and edits need all of it), `Returning`, `StructureEdit` and the DDL features, `Routines`, `Users`, `ProcessList`, `SessionDetails`, exports and imports (core SQL), and `Mcp` (not in v1).

**Tests.** `node dev/scripts/lane.mjs cassandra test --snapshot`: value mapping both ways (2^53+1 → `BigInt`, decimals and varints exact, timestamps as text, collections, tuples, UDTs and vectors as JSON and back from it), quoting and injection (names `"` doubled, values bound), the classifier suite, and the session over a scripted node (`fake.rs`): connect, list, browse with paging, count, fetch cell, a commit with one failing change, the editor, a cancel and the query time out, and read-only refusals on every write path. The real-server suite `tests/cassandra.rs` covers the same against a server and skips without `TABLER_TEST_CASSANDRA_URL`.

**Only a real server can confirm** (`lane.mjs cassandra it`): the scylla driver's own behaviour (paging states, prepared markers' types, LWT `[applied]` rows, schema agreement, request timeouts), TLS against a node's certificate through the IP mapping, single-node pinning over SSH, PasswordAuthenticator and a Command password on reconnect, ScyllaDB's version, and the server's `DESCRIBE`.

### The renderer and the mock (part B2)

**Renderer** (`src/features/drivers/cassandra/`; no views, session calls or form calls of its own, since the backend has none, so nothing the lane sends needs `guardExecution`):

- **`form.ts`** (`KindUi`): host and port (9042), user and password (no user: no authentication), the keyspace as the database, and TLS as `entry::dial` maps it: Off, On with any certificate, verify-ca, verify-full. A node takes TLS on its port or not at all, so there is no Prefer: a profile with it is saved as Disable, without client files. `validate` checks the request timeout (1–3600 s) and the editor's page size (1–10,000 rows) as `CassandraParams::check` does; the host, a Command password's command and the client certificate pair are the shared checks. The summary is `host : port : user : keyspace`, with the consistency when it is not LOCAL_ONE.
- **`params.ts`**: `CassandraParams` read leniently (a wrong type counts as unset) and saved with only what differs from the backend's defaults, numbers as numbers.
- **`CassandraFields.tsx`**: consistency, serial consistency, local datacenter, request timeout, page size, compression, and single node, shown on and locked over SSH (the backend always pins one node there).
- **`url.ts`**: `cassandra://user:password@host:port/keyspace?…` (TablePlus's scheme, the descriptor's only one): the password goes to the secret; `sslmode` (or `ssl` / `tls` as a flag, on being Require, or a mode's name; `prefer` reads as Disable) and the settings under their `CassandraParams` names, also as `localDC`, `fetchSize`, `requestTimeout`, `connectTimeout`. Values the backend would refuse are left out; an unknown `sslmode`, an `ssl` / `tls` value that is neither a flag nor a mode (rather than plaintext), a host with a space, `@`, `/`, `?` or `#`, or a malformed %-escape makes it no URL (`format` writes none for such a host either). `format` writes the SSL mode and the settings that differ from the defaults, never the password, and reads back to the same profile.
- **`dialect.ts`** (`DialectUi`): CodeMirror's Cassandra dialect with cqlsh's and newer CQL's words, the CQL types, `$` bodies and `//` comments; names double-quoted with `"` doubled; CQL's functions and types for completion; no formatter (sql-formatter has no CQL). Safe Mode and splitting use the backend's lexer profile (`bindings/lex/cql.json`). `splitCql` is `split_cql` (a batch kept whole), and the editor splits with it through `cqlUi.split` (`DialectUi.split`, added by the lead at landing).

**Mock** (`src/lib/ipc/mock/drivers/cassandra/`, `?mockDrivers=cassandra`): a Cassandra 4.1.7 node (`shop`: users with a set and a map, orders by customer clustered newest first with a `bigint` past 2^53 and a decimal, page views as counters, a materialized view of the orders by status; `iot`: readings by a two-column partition key, with blobs) and a ScyllaDB 6.2.1 profile opened read-only at LOCAL_QUORUM. The session mirrors `session.rs`:

- connecting checks what `validate` and `open` check, in the same words (the settings, the certificate pair, a missing keyspace, a read-only profile whose startup commands write, failing startup commands); the caps are the backend's, compared with `src-tauri/tests/fixtures/cassandra/caps.json`, which `drivers::cassandra::tests::caps_match_the_recorded_fixture` writes from a real session (password and Command profiles); no method the backend lacks;
- keyspaces as databases (system ones hidden), the current one as the only schema, columns in key order with their roles, indexes, the server's DDL;
- pages by a `cql1.` cursor bound to the statement, refused for an offset, a sort, or another query's cursor; AND-only filters with ALLOW FILTERING and the backend's refusals (OR, NULL, `Ne`, any column, a raw WHERE that writes or adds a statement); the size estimate as the count until filtered (a view, which has none, is counted);
- edits as lightweight transactions by the full key, deletes first, sent one by one: a change that does not apply (`[applied]` false, or the node's refusal of a materialized view) fails with the backend's message, those before it stay applied and the rest are not sent; key changes, DEFAULT, counter tables and incomplete keys are refused as `change_hook` refuses them; a read-only profile's edits are refused outright;
- the editor: cqlsh's commands with the backend's notices, `USE`, `SELECT` (with the ALLOW FILTERING rule and `count(*)`), `INSERT` / `UPDATE` / `DELETE` (lightweight transactions answer `[applied]`, counters move by deltas), batches (all or nothing: a failing statement puts the tables back, as a node refuses the whole batch), `TRUNCATE` and `DESCRIBE`; anything else is refused as the mock's limit; a read-only session refuses what the classifier calls a write before it runs.

**Tests** (`node dev/scripts/lane.mjs cassandra ts`): `form.test.ts` (defaults, save and read back, Prefer, lenient reading, every check's message), `url.test.ts` (parse, other spellings, refusals, format round trips), `fields.test.tsx`, `dialect.test.ts` (quoting, the editor's dialect, the lexer profile, every case of `classify-generated/cql.json`, the brief's hard cases, `split_cql`'s cases and UTF-16 offsets), and the mock story in `mock/drivers/cassandra/index.test.ts` (caps against the fixture, connect checks, catalog, cursor pages, filters, counts, cells, an edit with one failing change, a view's refusal, the editor, read-only refusals).

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

What v1 does not do, and where it differs from TablePlus on purpose:

- **Protocol v4 only** (the scylla driver): Cassandra 3.0+ and ScyllaDB. No DataStax Astra (Secure Connect Bundle), no AWS Keyspaces SigV4 (its service-specific credentials work as a password), no protocol v5.
- **No sort**: CQL orders by clustering columns within one partition only, so the grid keeps the table's order.
- **Filters**: AND only, `=`, `<`, `>`, `<=`, `>=`, `IN`, `LIKE` (SASI/SAI indexes only, the server refuses it otherwise) and BETWEEN (as `>=` and `<=`); no OR, NULL tests, negations or Any column. A filtered page adds `ALLOW FILTERING`, which can scan the cluster.
- **Paging** is Previous / Next only (the paging state); there is no jump to page N.
- **Counts**: the size estimate counts partitions, not rows; an exact count reads every partition and may hit the request timeout.
- **Edits**: by the whole primary key only, as lightweight transactions (Paxos, about four round trips each); key columns cannot be changed (delete and insert instead); counter tables cannot be edited in the grid (`UPDATE … SET c = c + n` in the editor); no DEFAULT. Changes apply one by one: a failed one leaves those before it applied.
- **Cancel** stops the run on this side only; the node finishes the statement within its own timeout.
- **Collections in edits** are read back from the grid's JSON; a number inside one keeps the precision `serde_json` gives it (exact for 64-bit integers, an f64 otherwise).
- **TLS with verify-full** checks the contact point's certificate for the profile's host; other nodes of a multi-node session are checked for their IP addresses (turn on single node, or give their certificates IP SANs).
- **Keyspace names**: the profile's keyspace is matched exactly (as the keyspace picker lists it); the editor's `USE` folds a bare name to lower case, as CQL does.
- **No Explain, process list, users, routines, structure editing, exports or imports** in v1; `EXPAND` is accepted and changes nothing in the grid.
