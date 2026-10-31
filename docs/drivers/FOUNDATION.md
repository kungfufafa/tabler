# Driver foundation

The foundation (steps F0–F10) is the shared code the 13 driver lanes build on: identity, capabilities, the session traits and their hooks, the SQL dialect layer, profiles and secrets, the shared transport, the renderer's lane slots, the mock, the test kit and the tooling. Since F10 every lane owns its whole module, `entry.rs` included, and works in **Phase B**: the driver itself, its renderer module and mock, and its real-server suite.

This page is the map: what exists, which hook a lane fills, and the exact commands a lane runs. [README.md](README.md) holds the contract (who owns what, the rules every driver follows, the read-only contract, classifier hardening, features, landing and the change-request history); each lane's brief (`docs/drivers/<lane>.md`) holds its engine's plan and limits. Symbols are cited by path and name; read the code for the details.

## What a lane owns and what it asks for

`dev/lanes.json` lists each lane's globs: `src-tauri/src/drivers/<id>/**` (pg-flavors: `drivers/postgres/flavor/imp/**`; sqlite-remote: `drivers/sqlite_http/**`), its `sqlgen/dialect/<d>.rs` and generated suite `sqlgen/portable/gen/<d>.rs` with its fixture, or its `sqlgen/native/<x>.rs`, its `src-tauri/tests/<id>.rs` and fixtures, `src/features/drivers/<lane>/**`, `src/lib/ipc/mock/drivers/<lane>/**`, its bindings (`<Prefix>*.ts`, `lex/<d>.json`) and its brief. Everything below is shared: a lane uses it as it is, and anything missing (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead (README, Shared files and their owners).

## Identity and the registry

| What                                                                                                                                              | Where                                                                                                                                                                                                                           |
| ------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Driver` (one per driver) and `ConnectionProfile`, whose `options.driver_params` is the lane's opaque JSON and `options.pg_flavor` a flavor       | `src-tauri/src/types/profile.rs`                                                                                                                                                                                                |
| `DriverDescriptor` per driver (`descriptor(driver)`, `Driver::descriptor`) and `KindDescriptor` per picker tile (`KINDS`, `KindId`)               | `src-tauri/src/types/driver.rs`: ports, TLS style, SSH, secret label and `extra_secrets`, `command_ttl_secs`, `required` / `required_params`, `url_schemes`, `system_schemas`, `read_only`, `sharing`, `cargo_feature`, `wire`… |
| Opening, validating and the form's calls: `registry::open`, `connect` (SQL only), `validate`, `util`, `util_kind`; `built_drivers`, `built_kinds` | `src-tauri/src/drivers/registry.rs`; a lane is wired through its `lane!` shim, which answers `not_built` when the lane's feature is off                                                                                         |
| The MCP process: `registry::set_unattended` / `unattended()`                                                                                      | a driver applies its engine's hardening when `unattended()` is true                                                                                                                                                             |
| Cargo features `drv-*` (`all-drivers`, `release-drivers`) and the shared `common-tls`, `common-http`, `common-aws`, `common-jwt`                  | `src-tauri/Cargo.toml`; `src-tauri/tests/manifest_features.rs` guards them                                                                                                                                                      |

**The lane's entry points** (`drivers/<id>/entry.rs`; sqlite-remote has `sqlite_http/libsql/entry.rs` and `sqlite_http/d1/entry.rs`; pg-flavors has none, the PostgreSQL driver opens its databases):

```rust
pub async fn open(target: ConnectTarget<'_>) -> Result<Session, AppError>;
pub fn validate(profile: &ConnectionProfile) -> Result<(), AppError>; // after the registry's common checks
pub async fn util(input: UtilInput<'_>, method: &str, args: serde_json::Value) -> Result<serde_json::Value, AppError>;
pub fn util_kind(method: &str) -> CallKind; // anything unknown is Write
```

`open` returns `Session::Sql(Arc<dyn DbSession>)` for a SQL engine or `Session::Native(Arc<dyn NativeSession>)` for a store that speaks no SQL. `validate` parses the lane's `<Prefix>Params` (`#[serde(rename_all = "camelCase", default)]`, ts-exported) from `driver_params`. `ConnectTarget` gives `profile`, `password`, and `host()`, `port()` and `tls_host()` (the SSH tunnel's address and the name TLS checks). The Tauri commands `session_call` and `driver_util` (`commands/native.rs`) reach `call` and `util`; a read-only profile allows only what `call_kind` / `util_kind` call reads.

## Capabilities

`SessionCaps` (`src-tauri/src/types/caps.rs`) says what a session can do, and the backend refuses what it lacks before anything is sent; the renderer hides it.

- `features: Vec<Feature>` (README, Features and the commands that require them), `namespaces` (`NamespaceCaps`: `SchemaMode`, `NamespaceLabels`), `data` (`DataCaps`: `Paging`, `CountMode`, `sort`, `filter_ops`, `RawFilter`, `TxMode`, `Affected`, `default_keyword`, `upsert`, `server_exprs`, `max_params`), `safety` (`SafetyCaps`: `ReadOnlyMode`, `hardened`, `SafeConfirm`), `session` (`LifecycleCaps`: `keepalive_secs`, `ping_billed`).
- A SQL lane starts from `SessionCaps::base(driver, dialect, flavor)`: `minimal` for a non-core session (the structure view and the editor, offset paging, nothing that writes, `ClassifierOnly`), or `sqlite_family` for LibSQL and D1 (SQLite's namespaces and data caps); a native store from `SessionCaps::minimal(driver)` (the structure view). It grants what it implements and proves with `with(&[Feature::…])` and sets the other fields; `without` takes features away.
- Enforcement: `SessionCaps::require`, `<dyn DbSession>::require` and `core_dialect(f)` (core SQL only for core dialects, or a driver's `core_reuse`), `facade_dialect(f)` (the same for a core dialect, or a built extension dialect, for code that writes only through the `sqlgen::sql` facade: the table export; a query export and a SQL script stay core-only, `export::require_query_export` and `script_dialect`), `Connection::session_for(f)` / `any_session_for(f)` (`src-tauri/src/state.rs`), `require_upsert`. `src-tauri/src/commands/caps_tests.rs` proves every command checks its feature first.

## Sessions and their hooks

`Session` (`src-tauri/src/drivers/session.rs`) is what a connection holds. SQL-only commands take `Session::sql()`, which refuses a native store (`not_sql`); what both kinds serve goes through `Session` itself (ping, close, cancel, introspection, `view_definition` / `definition`, `call`) or `Session::table_source()`.

**`DbSession`** (`src-tauri/src/drivers/mod.rs`), for SQL engines. The 18 methods without a default: `driver`, `dialect`, `server_info`, `ping`, `close`, `list_databases`, `list_schemas`, `list_tables`, `table_meta`, `view_definition`, `completion_metadata`, `query`, `execute`, `transaction` (guarantees each `TxStatement::expect_affected` or fails), `run_statement`, `stream_query`, `cancel`, `in_transaction`. The hooks, each with a default:

| Hook                                                   | Default                 | Who asks                                                                                             |
| ------------------------------------------------------ | ----------------------- | ---------------------------------------------------------------------------------------------------- |
| `query_cancellable`, `execute_cancellable`             | not cancellable         | table loads, counts and EXPLAIN under the time out; Metrics Board writes                             |
| `sql_trace`, `drain_trace`                             | no trace                | the console log (`drain_trace` drains the `SqlTrace` of `sql_trace`)                                 |
| `run_statement_traced`, `editor_run_lock`              | traced, no lock         | the app's own statements (EXPLAIN buttons, dump import); editor runs                                 |
| `capabilities`                                         | `SessionCaps::base`     | every command                                                                                        |
| `flavor`, `is_mariadb`                                 | none                    | caps, EXPLAIN                                                                                        |
| `explain_form(mode)`                                   | the dialect's EXPLAIN   | Explain, Explain Analyze                                                                             |
| `table_source()`                                       | rows read with SQL      | `browse_table`, `count_table_rows`, `fetch_cell` (BigQuery `tabledata.list`, Cassandra paging state) |
| `call(method, args)`, `call_kind(method)`              | unknown call, `Write`   | `session_call`                                                                                       |
| `list_processes()`, `kill_processes(ids, scope)`       | `None`: the core SQL    | `admin::list_processes`, `admin::kill` (Running Queries)                                             |
| `table_ddl(table)`                                     | `None`: built from meta | `get_table_ddl` (Show DDL)                                                                           |
| `list_routines(schema)`, `routine_definition(routine)` | `None`: the core SQL    | `objects::list_routines`, `objects::routine_definition` (F10)                                        |
| `list_users(include_nologin)`, `load_user(user)`       | `None`: the core SQL    | `admin::list_users`, `admin::load_user` (F10)                                                        |
| `session_details()`                                    | `None`: the core SQL    | `commands::lock::session_details_of` (F10)                                                           |

A hook answering `Some` is used as it is; `None` leaves the work to the core SQL of a core dialect, which a non-core dialect refuses (`core_dialect`). The callers check the feature first, so a hook runs only where the caps grant it.

**`NativeSession` and `TableSource`** (`src-tauri/src/drivers/native.rs`), for Redis, MongoDB, Elasticsearch and DynamoDB. `TableSource`: `browse` (offset or `BrowseRequest.cursor` / `BrowsePage.next_cursor`), `count`, `fetch_cell`. `NativeSession` adds `driver`, `capabilities`, `server_info`, `ping` (free), `close` (about 2 s), `cancel`, the introspection, `definition` (a mapping, validator or key schema), `plan_changes` → `NativePlan` (the preview and the operations from one plan) and `apply_changes` (a `RowResult` per change), the console (`split_console`, `classify_console`, default-deny, `run_console`, `console_lock`), `call` / `call_kind`, and since F10 `list_processes` / `kill_processes` (Elasticsearch `_tasks`; `admin::list_processes_any` and `kill_any` serve both kinds, behind `ProcessList` / `KillProcess`).

**Cancel.** A driver arms its `CancelSlot` with `arm_cancel(slot, CancelHandle::Custom(Arc<dyn CancelAction>))` and starts its `cancel` with `cancel_portable(handle)`, which handles the handles every driver shares (`Requested`, `Pending`). Cancel ends in `AppError::Cancelled`.

**Second sessions of a file engine** (DuckDB): `drivers::sharing` (`single_process`, `file_key`, `may_share`, `ensure_second_session`, `file_in_use`) and the descriptor's `sharing`.

## The PostgreSQL flavor seam (pg-flavors)

`PgFlavorHooks` (`src-tauri/src/drivers/postgres/flavor/mod.rs`): every hook defaults to plain PostgreSQL (`VANILLA`), and `hooks(flavor)` picks the lane's (`flavor/imp::hooks`) when `drv-pg-flavors` is built.

- Connect: `flavor`, `sql_dialect`, `default_database`, `init_sql(profile, read_only, base)` over `InitStatement { kind: InitKind, sql }` (pick by kind, never by text), `init_one_by_one`.
- Catalog: `list_databases`, `list_schemas`, `list_tables`, `table_meta`, `view_definition`, `completion_metadata`, each with a `Traced` connection (`introspect::Traced`, which records the SQL and knows `server_version_num`).
- Admin and objects: `list_processes`, `kill`, `table_ddl`, and since F10 `list_routines`, `routine_definition`, `list_users`, `load_user`, `session_details` (`PgSession` forwards the matching `DbSession` hooks; plain PostgreSQL takes no connection for them).
- Behaviour: `explain_form`, `adjust_caps(caps, server_version_num)` (grants back what `flavor::adjust_caps` took: `UNPROVEN_ON_FLAVORS`), `retry_serialization` (40001), `row_cap_savepoint`.
- Reuse: `introspect::columns_sql` / `indexes_sql` by version, `table_meta_of`, and the `pub(crate)` helpers there; `server_version_num`, `detect`, `session_flavor`. Tests use `FakePg` (`drivers/postgres/testing.rs`: `Col`, `Cell`, `PgType`, `params`, `error`, `rows`, `queries`, `startups`, `cancels`).

## The SQL dialect layer

- `SqlDialect` (`sqlgen/dialect/mod.rs`): the core three plus each lane's; `resolve()` gives the core generators, the lane's `DialectSpec`, or the `unavailable` spec that refuses every builder in a build without the lane.
- `DialectSpec` (`sqlgen/dialect/spec.rs`) in the lane's `sqlgen/dialect/<d>.rs` as `pub static SPEC`: quoting, placeholders (`PlaceholderStyle`, `max_params`), `lex: &LexProfile`, `rules: &ClassifyRules`, `classify`, `knobs: SqlKnobs` (paging, `Returning`, `RowLocator`, `ExplainForm`, `redact_commands`, `redact_calls`…), `hooks: SqlHooks`, `ddl: Option<&dyn DdlPlanner>`; and its `export_bindings_lex_<d>` test, which writes `bindings/lex/<d>.json` for the renderer.
- Shared code reaches any dialect only through the facade `sqlgen::sql` (`select` / `select_on`, `export_select`, `count_on`, `fetch_cell_on`, `change_on`, `literal`, `inline_params`, `wrap_explain_form`, `split_statements`, `is_single_statement`, `classify`, `writes`, `calls_side_effect_function`, `redact`, `max_params`, `column_form_on`, `fetch_form`).
- `sqlgen::portable` (`select`, `mutate`, `split`, `classify::with`, `redact`, `literal`, `explain`) builds and reads an extension dialect's SQL; `sqlgen::narrow::Narrowing` narrows it by caps (D1's 100 parameters and big integers).
- The generated classifier suite: `Suite` of `sqlgen/portable/gen/mod.rs` (the test module `sqlgen::portable::generated`: `new`, `active`, `inert`, `reads`, `writes`, `vocab`, `check`, `check_contexts`, `fuzz`) in the lane's `gen/<d>.rs`, with its blessed fixture `src-tauri/tests/fixtures/classify-generated/<d>.json` (`TABLER_BLESS=1`). README, Classifier hardening, says what a lane's suite must hold before it lands.
- Native stores' command and document text: the lane's `sqlgen/native/<x>.rs` (`redis`, `mongo`, `elastic`, `dynamo`).

## Profiles, secrets and read-only

- `conn_options::effective_read_only(profile)`: whether the session must refuse writes; the lane maps it to its engine's mechanism (README, Read-only contract) and reports it in `SafetyCaps`.
- `secrets::CommandPassword::for_profile_ttl` (Command Line passwords and tokens, TTL from `command_ttl_secs`); `secrets::extra_secret(ExtraSecret::KeyPassphrase | SessionToken)` inside `open` and `util` (the connect runs them in `ExtraSecrets::scope`).
- Safe Mode asks for the app passcode (or the connection's name when none is set) where nobody types the secret: a token (`secret_is_token`), or a Command Line password of a driver beyond the core three (`confirmsWithPasscode` in `src/features/drivers/catalog.ts`, `verify_safe_mode` in the backend); everyone else keeps the password prompt.

## Shared transport

Built only with the features of the lanes that use it (README, The shared transport (F8a) and What F8d added to it).

- `tls_client::rustls_config(mode, ca, identity, alpn, pin)`, `server_name`, `parse_pin` (`common-tls`).
- `drivers::common::http`: `HttpClient::for_target(target, base, HttpOptions::for_profile(p)?, credential)`, requests (`get`, `post`, `request`, `request_exact`, `request_url`, `with_form`), `send` / `send_repeating` with a `CancelSlot`, `HttpReply` (`json`, `error_for_status`, `chunk`, `into_stream`, `into_lines`, `into_response`, `header`, `retry_after`), `RetryPolicy` and `Repeat`, `HttpTls`, `Credential` (`Bearer`, `Basic`, `Header`, `Scheme`, `Sigv4`, `None`), `status_error`, `map_reqwest_error` (`common-http`).
- `drivers::common::token`: `TokenSource` (`for_profile`, `from_secret`, `issued` with `IssuedToken`).
- `drivers::common::aws`: `Signer`, `Credentials`, `CredentialProvider`, `credential_process_runner` (`common-aws`); `drivers::common::jwt`: `Rs256Key`, `verify_rs256`, `token_request_claims`, `JWT_BEARER_GRANT` (`common-jwt`).

## Test kit

- `src-tauri/src/testkit`: `FakeSession` (a SQL session that records what would reach the server), `FakeStore` (a native document store: cursor pages, edits one at a time, a console, calls, processes), `fake_http` (scripted HTTP server with `Route::handler`), `tls::TlsServer` (test CA, client certificates), `sigv4_vectors`, `jwt_key`, `install_crypto`.
- `src-tauri/tests/common/mod.rs` for the real-server suites; `sqlgen::portable::testing` and `sqlgen::dialect::testing` for dialect tests; `FakePg` above. A lane keeps its own fakes in `drivers/<id>/testing.rs`.

## The renderer

- `src/features/drivers/types.ts`: the lane's `index.ts` exports a `LaneModule` (`kinds` → `KindUi`, `drivers` → `DriverUi`, `dialects` → `DialectUi`). `KindUi`: `rows` (`SharedRow`), `labels`, `sslModes`, `defaults`, `Fields`, `validate` (`FormIssue`), `toProfile` / `fromProfile`, `url { schemes, parse, format }`, `summary`. `DriverUi`: `console` (`ConsoleUi`: `language`, `split`, `classify`, `fileExtension`), `TableView`, `Inspector` (`onStage`), `StructureView`, `objectMenu` (`ObjectMenuContext.call` goes through Safe Mode). `DialectUi`: `codemirror`, `formatter`, `functions`, `columnTypes`, `quoteIdent`.
- `registry.ts` loads lanes lazily (`loadKindUi` / `loadDriverUi` / `loadDialectUi`: null for a core or unbuilt database, undefined while loading; `await…` and `use…` forms; `registerLaneOverride` for tests); `lanes.ts` (`LANES`, `laneOfKind`, `laneOfDriver`, `laneOfDialect`); `catalog.ts` (the descriptors from `bindings/driverCatalog.ts`, `CORE_KINDS`, `kindOf`, `visibleKinds`).
- `portable/`: the TS mirror of the portable scanner and classifier (`classifyWith`, `classifyDialect`, `loadDialectLex`, `split`, `caseFailures` for a lane's `classify.cases.json` or generated fixture).
- `src/lib/caps.ts` (`capsOf`, `has`, `useCaps`, `isCoreSession`, `sessionDialect`, `connectionDialect`, `nounsOf`, `appliesPartially`, `allowFeature`), `src/features/connections/rows/` (the shared form rows), `src/features/documents/JsonDocumentEditor.tsx` with `documents/formats.ts`, `src/features/safety/guard.ts` (`guardExecution`, awaited before any write-capable `sessionCall` / `driverUtil`).
- Tests: `__fixtures__/fakeLane.tsx` (`fakeLane`, `fakeSqlLane`) and `wiring.test.tsx` show every slot working on a fake.

## The mock

`src/lib/ipc/mock/drivers/types.ts`: the lane's mock `index.ts` exports a `MockLane` with a `MockDriver` per driver (`serverVersion`, `seedProfiles`, `connect` → `MockSession`, `util`). `MockSession` mirrors the backend: `listDatabases`, `listSchemas`, `listTables`, `tableMeta`, `browse`, `run`, and the optional `tableDdl`, `count`, `fetchCell`, `preview`, `apply`, `call`, and since F10 `listProcesses`, `killProcesses`, `listRoutines`, `routineDefinition`, `listUsers`, `loadUser`, `sessionDetails`; the mock's handlers refuse what a session leaves out. `vite --mode mock` with `?mockDrivers=<id>` turns a lane's mock on (`buildMockDrivers`, `enableMockDrivers`); tests use `registerMockDriver` and `mock/drivers/fake` (`fakeMockDriver(driver, { dialect })`).

## Guards

- `src-tauri/tests/driver_guard.rs`: lane drivers are named only in the descriptors, the registry and their lane's files; shared code adds no `Driver::…` compares (baseline `tests/fixtures/guard_baseline.json`).
- `src-tauri/tests/dialect_guard.rs`, `src-tauri/tests/manifest_features.rs` (no ring, OpenSSL or native-tls; real-server suites have `required-features`).
- `src/features/drivers/guard.test.ts` (with `guardBaseline.json`): shared renderer code does not name lane drivers.
- `dev/scripts/lane-lib.test.mjs` (`node --test`): ownership, slots, tsc attribution (an error in one lane's files fails only that lane), snapshots.

## Commands per lane

Run from the repository root with Node only; each command takes a build slot (`C:/t/s1`–`s3`, `CARGO_BUILD_JOBS=4`, line-table debug info) or a TS slot, and waits while all are busy.

```bash
node dev/scripts/lane.mjs <lane> check               # cargo check --all-targets with the lane's features only
node dev/scripts/lane.mjs <lane> clippy              # cargo clippy --all-targets -- -D warnings
node dev/scripts/lane.mjs <lane> test                # lib tests of the lane's filters (below)
node dev/scripts/lane.mjs <lane> test <filter>… [-- --nocapture]
node dev/scripts/lane.mjs <lane> test-all            # every lib test with the lane's features (before each commit)
node dev/scripts/lane.mjs <lane> it                  # the real-server suites with .env loaded
node dev/scripts/lane.mjs <lane> bindings            # writes only <Prefix>*.ts and lex/<d>.json
node dev/scripts/lane.mjs <lane> ts                  # tsc-lane, eslint + prettier on its files, vitest (own folders + smoke)
node dev/scripts/lane.mjs <lane> fmt                 # rustfmt and prettier --write on its files only
node dev/scripts/lane-commit.mjs <lane> -m "<lane>: …"
```

Options: `--snapshot` (check, clippy, test, test-all, bindings) builds HEAD with only the lane's changed files over it; `--slot sN`; `--heavy` (a cold DuckDB build holds every slot); `--jobs N`. `node dev/scripts/tsc-lane.mjs <lane>` runs the type check alone.

Underneath, every cargo command is `cargo <cmd> --manifest-path src-tauri/Cargo.toml --locked --no-default-features --features <the lane's>` with `CARGO_TARGET_DIR=C:/t/<slot>` and `TS_RS_EXPORT_DIR=C:/t/<slot>/bindings-<lane>`; `test` and `test-all` add `-- --skip export_bindings_`, and `it` adds `--test <suite>… -- --include-ignored --test-threads=1`.

| Lane          | Home slot | Cargo features                 | `test` filters                                                                                    | `it` suites                 | Real-server variables                                                         |
| ------------- | --------- | ------------------------------ | ------------------------------------------------------------------------------------------------- | --------------------------- | ----------------------------------------------------------------------------- |
| pg-flavors    | s1        | `--features drv-pg-flavors`    | `drivers::postgres::flavor` `sqlgen::dialect::redshift` `sqlgen::portable::generated::redshift::` | `--test pg_flavors`         | `TABLER_TEST_CRDB_URL` `TABLER_TEST_GREENPLUM_URL` `TABLER_TEST_REDSHIFT_URL` |
| sqlite-remote | s1        | `--features drv-libsql,drv-d1` | `drivers::sqlite_http`                                                                            | `--test libsql` `--test d1` | `TABLER_TEST_LIBSQL_URL` `TABLER_TEST_D1_URL`                                 |
| duckdb        | s1        | `--features drv-duckdb`        | `drivers::duckdb` `sqlgen::dialect::duckdb` `sqlgen::portable::generated::duckdb::`               | `--test duckdb`             | `TABLER_TEST_DUCKDB_PATH`                                                     |
| clickhouse    | s1        | `--features drv-clickhouse`    | `drivers::clickhouse` `sqlgen::dialect::clickhouse` `sqlgen::portable::generated::clickhouse::`   | `--test clickhouse`         | `TABLER_TEST_CLICKHOUSE_URL`                                                  |
| sqlserver     | s2        | `--features drv-sqlserver`     | `drivers::sqlserver` `sqlgen::dialect::tsql` `sqlgen::portable::generated::tsql::`                | `--test sqlserver`          | `TABLER_TEST_SQLSERVER_URL`                                                   |
| snowflake     | s2        | `--features drv-snowflake`     | `drivers::snowflake` `sqlgen::dialect::snowflake` `sqlgen::portable::generated::snowflake::`      | `--test snowflake`          | `TABLER_TEST_SNOWFLAKE_URL`                                                   |
| bigquery      | s2        | `--features drv-bigquery`      | `drivers::bigquery` `sqlgen::dialect::bigquery` `sqlgen::portable::generated::bigquery::`         | `--test bigquery`           | `TABLER_TEST_BIGQUERY_URL`                                                    |
| vertica       | s2        | `--features drv-vertica`       | `drivers::vertica` `sqlgen::dialect::vertica` `sqlgen::portable::generated::vertica::`            | `--test vertica`            | `TABLER_TEST_VERTICA_URL`                                                     |
| dynamodb      | s2        | `--features drv-dynamodb`      | `drivers::dynamodb` `sqlgen::native::dynamo`                                                      | `--test dynamodb`           | `TABLER_TEST_DYNAMODB_URL`                                                    |
| cassandra     | s3        | `--features drv-cassandra`     | `drivers::cassandra` `sqlgen::dialect::cql` `sqlgen::portable::generated::cql::`                  | `--test cassandra`          | `TABLER_TEST_CASSANDRA_URL`                                                   |
| redis         | s3        | `--features drv-redis`         | `drivers::redis` `sqlgen::native::redis`                                                          | `--test redis`              | `TABLER_TEST_REDIS_URL`                                                       |
| mongodb       | s3        | `--features drv-mongodb`       | `drivers::mongodb` `sqlgen::native::mongo`                                                        | `--test mongodb`            | `TABLER_TEST_MONGODB_URL`                                                     |
| elasticsearch | s3        | `--features drv-elasticsearch` | `drivers::elasticsearch` `sqlgen::native::elastic`                                                | `--test elasticsearch`      | `TABLER_TEST_ELASTICSEARCH_URL`                                               |

**Before each commit:** `clippy`, `test-all` and `ts` clean; `bindings` run and its files committed with the code that exports them (CI's `features` job exports every driver's bindings and fails on a committed one that differs or is no longer exported, and warns about one not committed yet); `fmt`. Then `lane-commit.mjs`, which takes only the lane's files and refuses while shared build inputs are changed and uncommitted (`--snapshot` checks the lane on HEAD instead).

**The foundation's own gates while lanes work** (the lead and shared steps): `node dev/scripts/gates.mjs rs` (rustfmt on the changed shared files, clippy default and `--no-default-features`, `test --lib`, each shared suite by name), `gates.mjs ts`, `gates.mjs feat [--clippy]` (all features on a HEAD snapshot). README, The foundation's gates while lanes work.

## Hand-off (F10)

- The `entry.rs` files are the lanes' now; they still hold the F2/F6 stubs (`open` answers "not implemented yet", `validate` accepts, `util` knows no call, `util_kind` is `Write`).
- Acceptance, run on the stubs before the hand-off: `lane.mjs <lane> test-all` and `lane.mjs <lane> ts` passed for every lane, and a type error put into `src/features/drivers/mongodb/index.ts` failed `lane.mjs mongodb ts` while `lane.mjs redis ts` passed (kept as a test in `dev/scripts/lane-lib.test.mjs`).
- Done on the shared side since (L0a, README, Change requests): `TableOptions::distribution` and `sort_key` (filled in the clickhouse and pg-flavors lanes' files), `PgTableContext::mpp` for the clauses an MPP table adds after its column list, and the table export through the dialect facade (`sql::export_select`), with `ExportTables` granted to Redshift (query exports and SQL scripts stay core-only).
