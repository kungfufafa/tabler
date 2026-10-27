# Driver lanes

Tabler is adding 16 databases next to PostgreSQL, MySQL/MariaDB and SQLite. The foundation (the lead and the foundation steps F0–F10) builds what they share; 13 **lanes** each build one driver or one family, in the same checkout and at the same time. This page is the contract between them: who owns what, how a lane builds and tests on its own, the rules a driver follows, and how a lane lands. Each lane has its brief next to this file.

`dev/lanes.json` is the authoritative list of lanes, their features, slots, owned files, test filters and real-server variables. The scripts below and the Rust guards (`src-tauri/tests/driver_guard.rs`, `src-tauri/tests/manifest_features.rs`) read it; the renderer guard (`src/features/drivers/guard.test.ts`) keeps the lane folders `src/features/drivers/` and `src/lib/ipc/mock/drivers/` as a whole.

## Lanes

| Lane                              | Databases                        | Cargo features         | Home slot | Bindings prefix                         |
| --------------------------------- | -------------------------------- | ---------------------- | --------- | --------------------------------------- |
| [pg-flavors](pg-flavors.md)       | CockroachDB, Greenplum, Redshift | `drv-pg-flavors`       | s1        | `Cockroach*`, `Greenplum*`, `Redshift*` |
| [sqlite-remote](sqlite-remote.md) | LibSQL, Cloudflare D1            | `drv-libsql`, `drv-d1` | s1        | `Libsql*`, `D1*`                        |
| [duckdb](duckdb.md)               | DuckDB                           | `drv-duckdb`           | s1        | `Duckdb*`                               |
| [clickhouse](clickhouse.md)       | ClickHouse                       | `drv-clickhouse`       | s1        | `Clickhouse*`                           |
| [sqlserver](sqlserver.md)         | SQL Server                       | `drv-sqlserver`        | s2        | `Mssql*`                                |
| [snowflake](snowflake.md)         | Snowflake                        | `drv-snowflake`        | s2        | `Snowflake*`                            |
| [bigquery](bigquery.md)           | BigQuery                         | `drv-bigquery`         | s2        | `Bigquery*`                             |
| [vertica](vertica.md)             | Vertica                          | `drv-vertica`          | s2        | `Vertica*`                              |
| [dynamodb](dynamodb.md)           | DynamoDB                         | `drv-dynamodb`         | s2        | `Dynamo*`                               |
| [cassandra](cassandra.md)         | Cassandra, ScyllaDB              | `drv-cassandra`        | s3        | `Cassandra*`                            |
| [redis](redis.md)                 | Redis, Valkey                    | `drv-redis`            | s3        | `Redis*`                                |
| [mongodb](mongodb.md)             | MongoDB                          | `drv-mongodb`          | s3        | `Mongo*`                                |
| [elasticsearch](elasticsearch.md) | Elasticsearch, OpenSearch        | `drv-elasticsearch`    | s3        | `Elastic*`                              |

MariaDB and SingleStore stay `mysql` with `mysql_version`. The PostgreSQL flavors are `postgres` with `pg_flavor`; every other database is a driver of its own.

## Phases and order

- **Phase A** (protocol clients, codecs, builders, fakes and pure renderer code, in files other than `entry.rs`, with `pub` items or a temporary `#![allow(dead_code)]` in the lane's `mod.rs`):
  - Rust lanes start after F4 is committed (it creates `sqlgen/dialect/<d>.rs`), renderer lanes after F9a (it creates the lanes' `index.ts` skeletons).
  - pg-flavors starts after F8b, which adds the `PgFlavorHooks` seam.
  - redis and mongodb start after F6 (the native session), or limit Phase A to codecs and fakes until then.
  - Wave 1: pg-flavors, sqlserver, vertica, mongodb, redis, sqlite-remote, duckdb. Wave 2: clickhouse, elasticsearch, dynamodb, cassandra, snowflake, bigquery. Up to 7 lanes work at once; at most 3 cargo builds run at once.
- **Phase B** (after F10 hands `entry.rs` over): `entry.rs`, the trait implementations, renderer wiring, the mock and the real-server suite.
- **Landing** (the lead, one lane at a time): pg-flavors, sqlite-remote, redis, clickhouse, elasticsearch, dynamodb, mongodb, sqlserver, cassandra, snowflake, bigquery, vertica, and duckdb last (it makes DuckDB's C++ part of every default build).

## What a lane owns

Per lane (the exact globs are in `dev/lanes.json`):

- **Rust**, all behind the lane's features: `src-tauri/src/drivers/<id>/**` (pg-flavors: `drivers/postgres/flavor/imp/**`; sqlite-remote: `drivers/sqlite_http/**`), its `sqlgen/dialect/<d>.rs` with its generated classifier suite `sqlgen/portable/gen/<d>.rs` and fixture `src-tauri/tests/fixtures/classify-generated/<d>.json` (SQL lanes) or `sqlgen/native/<x>.rs` (native stores), its real-server suite `src-tauri/tests/<id>.rs` and `src-tauri/tests/fixtures/<id>/**`.
- **Renderer**: `src/features/drivers/<lane>/**` (an `index.ts` exporting a `LaneModule`, the form, URL handling, dialect or console, the views and their tests, `classify.cases.json`) and `src/lib/ipc/mock/drivers/<lane>/**` (a `MockDriver` with its own data model).
- **Generated**: `src/lib/ipc/bindings/<Prefix>*.ts` and `src/lib/ipc/bindings/lex/<d>.json`, written only by `lane.mjs <lane> bindings`.
- **Docs**: `docs/drivers/<lane>.md`.

The stubs a lane replaces:

| Stub                         | Created in | Replaced with                                                |
| ---------------------------- | ---------- | ------------------------------------------------------------ |
| `drivers/<id>/entry.rs`      | F2, F6     | `open` → "not implemented yet" becomes the real driver       |
| `sqlgen/dialect/<d>.rs`      | F4         | a conservative `SPEC` with `ddl: None` becomes the real spec |
| `sqlgen/portable/gen/<d>.rs` | F4a        | a placeholder becomes the complete generated suite           |
| `sqlgen/native/<x>.rs`       | F2         | an empty file becomes the builders                           |
| renderer and mock `index.ts` | F9a        | empty modules become the real ones                           |
| `src-tauri/tests/<id>.rs`    | F3b        | one ignored test becomes the real suite                      |
| `docs/drivers/<lane>.md`     | F3b        | the brief becomes the final doc                              |

`entry.rs` is the foundation's until F10; Phase A uses the lane's other files.

## Shared files and their owners

Everything outside the lanes' globs is shared and changes only through the lead, as one short serialized commit per batch of requests. Lanes batch their requests. These are the shared files lanes will need most:

| Shared file or value                                                                                                                                   | Used by                          | Owner and how it changes                                                                                                                                                                    |
| ------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Descriptor and `KINDS` values in `src-tauri/src/types/driver.rs`: ports, labels, `required_params`, `url_schemes`, `system_schemas`, `file_extensions` | every lane                       | The lead. A lane sends a change request with the value and the reason.                                                                                                                      |
| `src-tauri/Cargo.toml`, `Cargo.lock`, features                                                                                                         | every lane                       | The lead. The direct dependencies that were only transitive (`bytes`, `percent-encoding`, `hex`, scylla's `num-bigint-04` / `bigdecimal-04`) are already optional and gated per lane (F3a). |
| `drivers/common/**` and `tls_client.rs` (HTTP client, tokens, JWT, AWS SigV4 and credentials, rustls configs)                                          | 6 lanes                          | Built in F8a; in Phase B the F8a agent owns them and takes the requests.                                                                                                                    |
| `src-tauri/src/testkit/**`, `src-tauri/tests/common/mod.rs`                                                                                            | every lane's tests               | The F8a agent in Phase B. A lane keeps its own fakes in `drivers/<id>/testing.rs`.                                                                                                          |
| `fake_pg` (the scripted PostgreSQL server of the `drivers/postgres` tests)                                                                             | pg-flavors, vertica (pattern)    | F8b moves it to a `#[cfg(test)] pub(crate)` module; the lead owns it.                                                                                                                       |
| `src/features/documents/JsonDocumentEditor.tsx`                                                                                                        | mongodb, elasticsearch, dynamodb | Created in F9a. The mongodb lane specifies its changes (as requests); the others configure it through its props and slots.                                                                  |
| `src-tauri/src/mcp/pool.rs`                                                                                                                            | duckdb (SingleProcess engines)   | The lead: F5 keys SingleProcess drivers by file.                                                                                                                                            |
| `src/lib/ipc/bindings/` (the folder)                                                                                                                   | every lane                       | Shared, but `<Prefix>*.ts` and `lex/<d>.json` in it are the lane's: `tsc-lane` counts their errors against the lane, and `lane.mjs <lane> bindings` writes only them.                       |
| Registries, `lib.rs`, `src/lib/ipc/index.ts`, `mock/handlers.ts`, `types/caps.rs`, shared types, core sqlgen, core drivers                             | every lane                       | The lead.                                                                                                                                                                                   |
| `dev/fixtures/**`, `.env.example`, `docs/verify-servers.md`, `docs/tableplus-parity.md`                                                                | lanes with fixtures or servers   | The lead (updated when a lane lands).                                                                                                                                                       |

## Rules every lane follows

**Process.**

- Never edit outside the lane's globs; anything missing (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead.
- Never run repo-wide `cargo fmt`, `prettier --write .`, `cargo update`, or git commands other than `lane-commit.mjs`. Downloads and installs of local servers need the user's approval.
- If an error shows in a **shared** file, wait and retry; never patch it.

**Drivers.**

- **Read-only.** Map `conn_options::effective_read_only(profile)` to the engine's mechanism (see the read-only contract below) or report `ClassifierOnly` with a default-deny classifier. When `registry::unattended()` is true (the MCP process), apply the engine's hardening. Report `safety.hardened`.
- **Tokens.** `CommandPassword::for_profile_ttl` (TTL from the descriptor), or `TokenSource` / `HttpClient` with `Credential` (one refresh on a 401). Large credentials are file paths in params, never stored. Secrets longer than Windows Credential Manager's 1,280 characters go through `set_secret` (UTF-8 bytes) or fail with a clear error.
- **Ping** is free (no billed job, no warehouse resume, no rate-limited call). `keepalive_secs` follows the descriptor. `close()` returns within about 2 s.
- **Values.** Only the existing `Value` variants: a missing field is `Other{ty:"missing"}`, an ObjectId `Other{ty:"objectId"}`, nested values `Json`, Decimal128 `Decimal`, integers beyond 2^53 `BigInt`. Decoders are total, with no `unwrap` or `expect` on server data (release builds abort on panic). Cursors and ids are `String`.
- **Errors.** Cancel → `Cancelled`; bad credentials → `Auth`; transport loss → `NotConnected`; timeouts → `Timeout`; server errors → `Db{sqlstate, vendor_code}` with engine codes only in `vendor_code`.
- **Affected counts.** `transaction()` guarantees `expect_affected` before it commits (an LWT `IF EXISTS`, a pre-count, a conditional write, `if_seq_no`) or returns an error; never `Ok` with an unchecked guard. Where the guard cannot hold before commit (D1, ClickHouse), only primary-key identity is allowed, EditWithoutKey and all-columns edits are off, and a pre-count is documented as best-effort.
- **Partial applies.** Every `TxMode` except `Atomic` and `DataOnly` returns a result per change; the renderer unstages the applied ones, so a retry never applies a non-idempotent write twice.
- **TLS** through `tls_client::rustls_config`, or `HttpClient` for HTTP drivers. No ring, OpenSSL or native-tls (`tests/manifest_features.rs` guards the crates' features).
- **SSH** through `ConnectTarget::{host, port, tls_host}`. HTTP drivers use `resolve`. MongoDB and Cassandra pin a single node.
- **Generated text.** Preview and command text goes through `sqlgen/dialect/<d>.rs` or `sqlgen/native/<x>.rs`, never formatted elsewhere (the repository's sqlgen rule).
- **Crate names.** A lane module named like its crate (`drivers::redis`, `drivers::mongodb`, `drivers::duckdb`) shadows the crate in parent scopes: refer to the crate with a leading `::` (`::redis::cmd`).
- **Renderer.** Lane modules are loaded lazily; write-capable `sessionCall` / `driverUtil` helpers await `guardExecution` first.

## Read-only contract

Each driver declares a `ReadOnlyMode` in its caps, and the backend enforces it (`require` and the classifiers run before any SQL or native call, for the UI, plugins and MCP alike):

- **Server**: the server refuses writes for the session (PostgreSQL flavors' `default_transaction_read_only`, Vertica's session characteristics, DuckDB's `access_mode=READ_ONLY`, a BigQuery token minted with the read-only scope once a recorded test proves it).
- **PerRequest**: every request carries the restriction (ClickHouse `readonly=2`, LibSQL `PRAGMA query_only` in every stream).
- **ClassifierOnly**: only Tabler's classifier stands between the user and a write. Classifiers are allow-lists and default-deny: anything unknown is a write. T-SQL lexes as `terminator_optional` (any write keyword anywhere makes the batch a write); `OPENQUERY`, `OPENROWSET`, `OPENDATASOURCE`, `EXEC`, `SELECT … INTO`, Snowflake `SYSTEM$…`, `EXECUTE IMMEDIATE` and `CALL` are writes; ClickHouse allows only the table functions `numbers`, `zeros`, `values`, `generateRandom`, `view`, `merge`, `null`; DuckDB file and extension statements (`read_*`, `glob`, `COPY`, `ATTACH`, `INSTALL`, `LOAD`, `EXPORT` / `IMPORT DATABASE`) are writes.

MCP: native stores (Redis, MongoDB, Elasticsearch, DynamoDB) are refused in v1; an extension SQL dialect is offered only when its caps grant `Mcp`. Safe Mode confirms token or command secrets with the app passcode (or the typed connection name when no passcode is set); core profiles keep their password prompt. Each lane's brief has its engine's row of the table.

## Classifier hardening

Where the server cannot refuse a write (ClassifierOnly, Safe Mode, plugins, MCP), the portable classifier (`sqlgen::portable::classify` over the dialect's `LEX` and `RULES`) decides, so a write it calls a read is a safety bug. Its rules are lexical: files and remote sources named where a table goes (a DuckDB replacement scan, a ClickHouse dictionary) are the engine's to refuse, through the hardening in the read-only contract. The generated tests (`src-tauri/src/sqlgen/portable/gen/`, `cargo test --lib sqlgen::portable::generated::<d>::`) check per spec:

- **Active contexts.** Every write keyword (`writes`) and write call (`write_calls`), in three spellings, is a write wherever it is code: at the start, after another statement, in a query, glued to numbers, strings, quoted names, parentheses, dollar bodies and comments, past a quote that a reading of backslashes ends a string at, in subqueries, CTEs and set operations, in qualified names, after unusual whitespace; calls also through a quoted or qualified name and with a comment or line break before the `(`. Without terminators (T-SQL), a write keyword after the first word also makes the text several statements.
- **Inert contexts.** No write counts inside any string, quoted-name, dollar-body or comment form the lexer profile has, and the text stays one statement.
- **Default-deny.** Text that starts with no statement the rules know is a write.
- **Corpus.** Ordinary reads of the engine (joins, CTEs, window functions, subqueries, UNION, the allowed table functions, the rules' words inside strings and names) stay reads, also re-spaced, re-cased and next to another read, and they start with every read keyword (`queries`, `inspections`); the write forms the word lists do not spell (`SELECT … INTO`, `EXPLAIN ANALYZE` of a write, file readers, table functions outside the allow-list) are writes.
- **Fuzz** (fixed seed). Random scripts over the dialect's vocabulary never panic or hang; a write word the scanner reads as code makes a SELECT a write; a script is a write when one of its statements is; splitting neither loses nor duplicates text.
- **Agreement and fixture.** The spec's `classify` and `portable::classify::with` (the renderer's) agree on every case. A sample of the cases and fuzz splits is `src-tauri/tests/fixtures/classify-generated/<d>.json`, which the renderer's portable classifier runs too; `TABLER_BLESS=1` rewrites it (review the diff).

The foundation owns the machinery (`gen/mod.rs`, `dialect/lex.rs`, splitting, the classification engine) and the suites of its test specs, `gen/ansi.rs` (the worked example) and `gen/tsql_like.rs`, with their fixtures. Each SQL lane owns its `gen/<d>.rs`, a placeholder that runs only the shared contexts and a short fuzz, and its fixture; it fixes what its suite finds in its own `RULES`, `LEX` and `SPEC`, and sends a bug of the shared machinery to the lead.

**A SQL lane's driver lands only with its generated suite complete**: every write keyword, write call and read starter of its spec in every shared context and in the contexts only its grammar has, an engine-realistic read corpus, its write forms and fuzz atoms, `suite().check()` and `suite().fuzz(…)` in place of the placeholder, and its fixture blessed. Then a red-team review (someone other than the lane trying to get a write classified as a read) must pass, with each finding fixed and kept as a case. Each SQL lane's brief lists its known hard cases.

## Features and the commands that require them

A session's caps list the `Feature`s it offers (`src-tauri/src/types/caps.rs`). Core sessions (PostgreSQL, MySQL/MariaDB, SQLite) get every feature; a lane grants what it implements and proves. F5 adds `require(Feature)` (or `core_dialect(Feature)`) at the entry of each command below, and keeps this table exact; the renderer hides what the caps lack.

| Feature                                                                          | Backend commands and MCP tools that require it                                                                     | Renderer                                        |
| -------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------- |
| `Editor`                                                                         | `run_sql`; MCP `run_query` (with `Mcp`)                                                                            | query tabs                                      |
| `Explain`, `ExplainAnalyze`                                                      | `explain` (plan, analyze); MCP `explain_query`                                                                     | Explain buttons                                 |
| `CancelOnServer`                                                                 | `cancel_query` cancels on the server (otherwise client-side only)                                                  | cancel wording                                  |
| `Format`                                                                         |                                                                                                                    | Format SQL                                      |
| `EditRows`, `InsertRows`, `DeleteRows`                                           | `preview_changes`, `apply_changes` (updates, inserts, deletes)                                                     | grid editing, new / duplicate / delete rows     |
| `EditWithoutKey`                                                                 | `preview_changes`, `apply_changes` with all-columns or rowid keys                                                  | edits of tables without a key                   |
| `Returning`, `ServerExpressions`                                                 | `apply_changes` re-reading rows, DEFAULT and server expressions                                                    | DEFAULT / expression values                     |
| `ExactCount`                                                                     | `count_table_rows` exact                                                                                           | the count button                                |
| `CopyAsInsert`, `LaravelMigration`, `DummyData`                                  |                                                                                                                    | copy as SQL, the Laravel and Dummy Data plugins |
| `WorkspaceCommit`                                                                | `preview_workspace_changes`, `apply_workspace_changes` (otherwise per tab)                                         | workspace commit sheet                          |
| `StructureView`                                                                  | `get_table`, `browse_table`; MCP `describe_table`                                                                  | Structure tab                                   |
| `StructureEdit`, `CreateTable`, `RenameTable`                                    | `preview_ddl`, `apply_ddl`                                                                                         | structure editing, new table                    |
| `DropTable`, `TruncateTable`                                                     | `preview_object_op`, `apply_object_op` (drop, truncate, clone)                                                     | object menu                                     |
| `TableDdl`                                                                       | `get_table_ddl`                                                                                                    | Show DDL                                        |
| `ViewDefinition`                                                                 | `get_view_definition`                                                                                              | view definition                                 |
| `Routines`, `Sequences`, `Triggers`                                              | `list_routines`, `get_routine_definition`, `list_sequences`, `get_sequence_definition`, drop ops                   | sidebar sections                                |
| `Diagram`                                                                        | `schema_relations`                                                                                                 | diagram                                         |
| `SwitchDatabase`                                                                 | `switch_database`                                                                                                  | database switcher                               |
| `CreateDatabase`, `DropDatabase`, `RenameDatabase`, `CreateSchema`, `DropSchema` | `admin_database_options`, `admin_preview_ddl`, `admin_apply_ddl`                                                   | database and schema dialogs                     |
| `ProcessList`, `KillProcess`                                                     | `admin_list_processes`, `admin_kill_processes`                                                                     | process list                                    |
| `Users`                                                                          | `admin_list_users`, `admin_get_user`, `admin_preview_user`, `admin_apply_user`                                     | users                                           |
| `ExportTables`, `ExportSql`                                                      | `export_tables`, `export_database_tables`, `export_data` (SQL output with `ExportSql`)                             | export dialogs                                  |
| `ImportCsv`, `ImportJson`, `ImportSqlDump`                                       | `preview_csv_import` / `import_csv`, `preview_json_import` / `import_json`, `preview_sql_dump` / `import_sql_dump` | import dialogs                                  |
| `Backup`, `Restore`                                                              | `backup_inspect_target`, `backup_run`                                                                              | backup and restore                              |
| `Encoding`, `Timezone`, `StartupCommands`                                        | `get_connection_encoding`, `set_connection_encoding`; the profile's time zone and startup commands at connect      | connection form                                 |
| `SessionDetails`                                                                 | `session_details`                                                                                                  | session details                                 |
| `Metrics`                                                                        | `metrics_run`                                                                                                      | metrics boards                                  |
| `Assistant`                                                                      | `assistant_chat`, `assistant_context`                                                                              | AI assistant                                    |
| `Mcp`                                                                            | every MCP tool on the connection                                                                                   | MCP settings                                    |

A second session on the same target (`switch_database`, `set_connection_encoding`, the import and export sessions, `backup_inspect_target`) is refused for a `SingleProcess` driver unless its feature is granted.

## Building and testing in isolation

Run everything from the repository root (so `.cargo/config.toml` applies), with Node only (`pnpm` is not needed; where it is installed, `pnpm lane`, `pnpm lane:commit`, `pnpm gates` and `pnpm test:lanes`, the tooling's own tests, are the same commands):

```bash
node dev/scripts/lane.mjs <lane> check        # cargo check --all-targets, the lane's features only
node dev/scripts/lane.mjs <lane> clippy       # … clippy --all-targets -- -D warnings
node dev/scripts/lane.mjs <lane> test         # cargo test --lib -- --skip export_bindings_ <the lane's filters>
node dev/scripts/lane.mjs <lane> test <f>…    #   … with other filters
node dev/scripts/lane.mjs <lane> test-all     # every lib test with the lane's features (before each commit)
node dev/scripts/lane.mjs <lane> it           # the real-server suite, with .env loaded
node dev/scripts/lane.mjs <lane> bindings     # export into the slot, copy only <Prefix>*.ts and lex/<d>.json
node dev/scripts/lane.mjs <lane> ts           # tsc-lane, eslint + prettier on the lane's files, vitest
node dev/scripts/lane.mjs <lane> fmt          # rustfmt and prettier --write on the lane's files only
node dev/scripts/lane-commit.mjs <lane> -m "<lane>: …"
```

What happens underneath, for example `node dev/scripts/lane.mjs redis clippy`:

```bash
CARGO_TARGET_DIR=C:/t/s3 CARGO_BUILD_JOBS=4 TS_RS_EXPORT_DIR=C:/t/s3/bindings-redis \
CARGO_PROFILE_DEV_DEBUG=line-tables-only OPENSSL_NO_VENDOR=1 OPENSSL_STATIC=1 OPENSSL_DIR=C:/t/openssl \
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --no-default-features --features drv-redis --all-targets -- -D warnings
```

**Slots.** Cargo builds run in short target folders under `C:/t` (`TABLER_SLOT_ROOT` elsewhere; short paths keep DuckDB's C++ build under MSVC's path limit): `s1`–`s3` for the lanes, `s0` for the foundation's gates, never the folder `tauri dev` uses. A lock file in the slot (`.tabler-lane.lock`, with the holder's pid and command) lets one build use it at a time. A lane takes its home slot, or any free lane slot while the home one is busy, and waits while all are. A lock whose process is gone is stale and is taken over. `--slot sN` picks a slot; `--heavy` (a cold DuckDB build) also holds every other slot, `s0` included, so nothing else builds next to it. Lane slots build with `CARGO_BUILD_JOBS=4` (6 with `--heavy`, or `--jobs N`) and line-table debug info.

**Renderer tools** take one of two TS slots (`C:/t/locks/ts1.lock`, `ts2.lock`). `lane.mjs <lane> ts` runs `tsc-lane`, eslint and prettier on the lane's own folders and docs, and vitest with `--maxWorkers=2` on the lane's folders plus the shared smoke set (`smoke` in `dev/lanes.json`), with every other lane's folders excluded. The full shared suite is the lead's.

**Real-server suites.** `src-tauri/tests/<id>.rs` is a `[[test]]` with `required-features` (only the lane's builds compile it) whose tests are `#[ignore]`d and skip themselves while their `TABLER_TEST_*` variable is unset; `lane.mjs <lane> it` runs them with `--include-ignored --test-threads=1` and the variables from `.env` (see `.env.example`).

**Why another lane cannot break yours.** In Rust, the other lanes' modules, dialect and native files are compiled out (a `#[cfg]`'d module is not even read), their real-server suites are not built, and the registry and dialect shims answer "not built". In the renderer, the other lanes' folders are imported lazily, `tsc-lane` counts their diagnostics without failing, and vitest excludes them. `--snapshot` (check, clippy, test, test-all) builds HEAD with only the lane's changed files over it, for when shared work in progress is in the way.

### Committing

`node dev/scripts/lane-commit.mjs <lane> -m "<lane>: …" [-m <paragraph>] [--snapshot] [-- <path>…]` commits the lane's changed files (or the given paths, which must all be the lane's) with `git commit --only`, after adding new files, retrying while `index.lock` is held; nothing else that is staged or changed goes in. It refuses:

- a path the lane does not own (for example `lane-commit.mjs redis -- src-tauri/src/lib.rs`);
- while shared files a build reads (`src/`, `src-tauri/`, the build and tool configuration, `dev/lanes.json`) are changed and uncommitted, because the lane may depend on them. With `--snapshot` it instead checks the lane on HEAD plus the files being committed (`cargo check --all-targets` with the lane's features, then `tsc-lane`) and commits when that passes;
- when git has no author configured.

Messages end with the `Co-Authored-By` trailer of the step commit rule. Only the lead pushes.

### The foundation's gates while lanes work

The plain gates (`cargo fmt --check`, `--all-features`, `tsc`, `eslint .`, `prettier --check .`, `vitest run`) read every file, so a lane's unfinished work fails them from F4 on. The foundation steps and the lead's landing checks (§6.1 step 4) use these variants, which leave the lanes' files out:

```bash
node dev/scripts/gates.mjs rs-fmt [--all]   # rustfmt --check on the step's changed .rs files (all shared ones with --all), one file at a time
node dev/scripts/gates.mjs rs               # rs-fmt; in s0: clippy --all-targets (default and --no-default-features), test --lib, test --test <each shared suite>
node dev/scripts/gates.mjs feat [--clippy]  # check (clippy) --all-targets --all-features on a HEAD snapshot with the step's files, holding every slot
node dev/scripts/gates.mjs ts               # tsc without lane diagnostics, eslint and prettier without lane files, vitest without lane folders
```

- rustfmt runs file by file through stdin, because it follows `mod` declarations whatever their `cfg`: a shared `mod.rs` would pull a lane's files in.
- `--all-targets` is safe in default builds: lane suites need their features. The integration tests still run by name (every `src-tauri/tests/*.rs` no lane owns), so a landed lane's real-server suite stays the lane's.
- `feat` never builds the working tree: `git archive HEAD` goes into `C:/t/snap/foundation` (not a worktree), the step's changed shared files are copied over it, and cargo builds that with `CARGO_BUILD_JOBS=6`.
- "The step's files" are the changed files no lane owns.

## Definition of done (v1)

1. **Connect, test and disconnect.** The server version shows in the status pill; Command mode works; SSH works where the descriptor allows it; TLS works with CA, client certificate and verify modes; MCP behaves as the caps say.
2. **Sidebar.** Lists databases, schemas and tables with the caps' labels and nouns.
3. **Structure view** (read-only unless StructureEdit): columns, indexes and foreign keys where the engine has them; native stores show `definition()`.
4. **Grid.** Paging (offset or cursor), sort (when `caps.data.sort`), filter operators plus the raw filter kind; counts follow the count mode; fetch cell works.
5. **Editor or console.** Runs and streams with a row cap. Cancel works (on the server where the engine can, otherwise client-side, declared in the caps). Statements split the way the engine does.
6. **Edits** (the staged preview is what is applied) wherever the engine allows; otherwise EditRows is off. Partial applies follow the TxMode.
7. **Safety.** Read-only enforced in the backend; Safe Mode prompts come from the classifier, whose case file the TS mirror shares; the MCP gate respects the caps. SQL lanes: the generated classifier suite is complete and its red-team review passed (see Classifier hardening).
8. **Mock and URLs.** The mock story works in `vite --mode mock` with `?mockDrivers=<id>`; URL import and format round-trip for the engine's schemes.
9. **Tests.** Unit tests of value mapping (2^53+1 → BigInt, decimals as strings, timestamps as text, nested → Json), quoting and injection, classifier cases; a fake-server or embedded test covering connect, list, browse with paging, count, fetch, a commit with one failing change, a console run, a cancel and a timeout.
10. **Lead checks.** `lane.mjs <lane> clippy`, `test-all`, `ts` and `bindings` are clean; no `#![allow(dead_code)]` is left in the lane's `mod.rs`; the brief lists the limits; no file outside the lane's globs was touched.

## Landing (the lead, one lane at a time)

1. In the lane's slot: `lane.mjs <lane> clippy`, `test-all`, `ts`, and `it` where a server exists. Review the definition of done.
2. Copy the lane's bindings: `lane.mjs <lane> bindings`.
3. Append the lane's `drv-*` features to `release-drivers` in `src-tauri/Cargo.toml`.
4. Run the gates on the default features, which now include the lane (the `gates.mjs` variants while other lanes are open), and the core regression suite against the local MySQL (G-INT).
5. Update `docs/tableplus-parity.md`, `docs/verify-servers.md`, `.env.example` and the README.
6. Commit with `git commit --only -- …`; the lead pushes.

After the last lane: the `src-tauri/Cargo.toml` description, the CLAUDE.md header and its milestone line.
