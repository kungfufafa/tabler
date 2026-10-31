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

LibSQL's descriptor still says `PerRequest`; its sessions report `ClassifierOnly` unless a read-only session's server took `PRAGMA query_only = 1` at connect, which `sqld` and Turso refuse (a change request to the lead: the descriptor's `ClassifierOnly`).

## Read-only and hardening

| Engine                                                                            | When read-only                                                                                                                                                                                          | When unattended (MCP)      |
| --------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------- |
| LibSQL                                                                            | The session's classifier, default-deny, before sending (`sqld` and Turso refuse no write for a session); `PRAGMA query_only = 1` in every batch as well where the server takes it (not `sqld` or Turso) | MCP only if caps grant Mcp |
| D1, SQL Server (ApplicationIntent=ReadOnly as routing only), Snowflake, Cassandra | ClassifierOnly, default-deny. T-SQL uses `terminator_optional`; Snowflake `EXECUTE IMMEDIATE` and `CALL` are writes                                                                                     | MCP only if caps grant Mcp |

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

Left for Phase B (the backend part is done, see Phase B status; the renderer, mock and URL handling are part B2): `entry.rs` (`open`, `validate`, `util`, `LibsqlParams` / `D1Params`), the two `DbSession`s on these clients (worker vs editor stream, `TxState` from `get_autocommit` or `tx_effect`, the lost-transaction notice from `take_lost`, cancel through `CancelHandle::Custom` dropping the request, `max_rows` over the cursor), the transport on `HttpClient` (SSH `resolve` for LibSQL), caps (LibSQL `PerRequest`, D1 `ClassifierOnly`, D1 primary-key identity only), the renderer and mock, the real-server suites, removing `#![allow(dead_code)]`.

## Phase B status

Part B1 (the backend drivers) is built and tested without a server; part B2 (the renderer module, the mock and the URL handling) is below. `#![allow(dead_code)]` is gone from `mod.rs`.

| Module                                                 | What Phase B added                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| ------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `transport.rs`                                         | `HttpTransport`: the `Transport` seam over the shared `HttpClient` (the credential on the endpoint's own requests only, SSH through `resolve`, TLS by `rustls_config`, a 429 waited out, the body streamed for the cursor; a reply read whole stops at 256 MB, an error body at 64 KB). The lane's reqwest test transport and its copy of `body_excerpt` are gone.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `session.rs`                                           | What both sessions share: `refuse_write` (a read-only session's refusal before sending: the classifier, and `hides_code_in_a_variable`, both moved here from D1's session), `cancellable` (a `CancelHandle::Custom` whose action drops the request; a run cancelled before it starts sends nothing), the console log of a catalog round trip (one entry, its statements one per line) and of a staged commit (each statement up to the failed one), `Closed` (calls after `close` fail with `NotConnected`).                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `catalog.rs`, `value.rs`                               | `table_ddl_queries` / `parse_table_ddl` (Show DDL: the table's `CREATE` and its indexes' and triggers'), `first_set`; `kind_from_decltype` and `parse_trigger_head` are the core driver's (F6e), the copies gone.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `libsql/entry.rs`                                      | `LibsqlParams` (`url` in place of host and port, for a server under a path, its scheme saying HTTP or HTTPS; `basicAuth`), `open` (the base from the profile's host and port, never the tunnel's; HTTPS unless SSL is off, its certificate checked unless SSL is plain "HTTPS" (`require`): `prefer`, a new profile's mode, and a URL with SSL off check it fully (`certificate_check`), so the token goes only to the server the name says; `TokenSource::for_profile`: the auth token as a bearer, Command mode with the descriptor's 3,000 s TTL and one refresh after a 401; or HTTP Basic with the user and password), `validate` (a host or a URL; no secret in the URL: `url::parse_url`'s `has_secret` is any user or password, a password alone too, and a token key in any casing, even empty or repeated, so neither a later empty `authToken=` nor `AuthToken=` keeps a token in the stored params; no URL with SSH; Basic needs a user), no `util` calls. |
| `libsql/client.rs`                                     | `probe_query_only`: a read-only session asks once at connect whether the server takes `PRAGMA query_only = 1`, alone on a stream of its own, and keeps it only then (`sqld` and Turso refuse the whole batch that holds it). With it, batches on a held stream start with it too (the README's change request), and a new stream's prelude repeats it after each startup command; a held stream the server forgot while idle outside a transaction (`STREAM_EXPIRED`: the request never ran) is replaced and the work runs on the new one; `ping` (`GET v3` / `v2`: no SQL); a cursor line beyond 256 MB fails the call; `describe`, `base` and the protocol items only they used (`execute`, `sequence`, `describe` requests, named arguments) went.                                                                                                                                                                                                                  |
| `libsql/session.rs`                                    | `LibsqlSession`: the app's statements on a stream each (closed in the same request), the editor on the stream it holds. A read-only session refuses before sending what the classifier does not call a read, on every path (`refuse_write`, as on D1), and at connect startup commands that write. On Hrana 3 the editor reads through `v3/cursor` with a probe step (`SELECT 1` on condition `is_autocommit`) that gives the transaction state in the same request; on Hrana 2 one pipeline and `tx_effect`. Rows beyond the cap: outside a transaction the request is dropped (and the stream, quietly); inside one the rest is read and dropped, so the transaction stays. `table_ddl`, `session_details`.                                                                                                                                                                                                                                                          |
| `d1/entry.rs`                                          | `D1Params` (`accountId`; `apiBase`, HTTPS or HTTP to this machine only, no user, password, query or fragment, and never echoed in an error), `open` (the account checked, a database name resolved to its UUID once, the API's certificate always verified whatever the profile's SSL mode), `validate`, `util` `listDatabases` (the picker; a read).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `d1/session.rs`                                        | `D1Session`: one request per call, the staged commit as one guarded batch, the account's databases for the switcher, `in_transaction` always `Some(false)`; every call sends one statement (`api::one_statement`: D1 runs each statement of a `;`-joined text, and the app's SQL can end in a raw filter; a text with a variable whose suffix hides a `;` from the shared lexer, `SELECT $a(--) ; DELETE FROM t`, counts as several, on a writable session too); a read-only session refuses before sending what the classifier does not call a read, and text with a variable SQLite reads differently from the classifier (`hides_code_in_a_variable`); `ping` sends nothing. `table_ddl`, `session_details`.                                                                                                                                                                                                                                                        |
| `testing.rs`, `*/testing.rs`                           | `serve`: a fake (`Answers`) behind a socket of `testkit::fake_http`, so `entry::open` is tested through the registry and the real `HttpClient`; `app_connection` (the command bodies on a session); D1's fake delays replies too, and refuses as workerd does: an authorizer (`rusqlite` `hooks`) and SQLite's own statement loop, where only the last statement may have parameters and it gets exactly as many values as it has. LibSQL's fake answers only what the client sends; `FakeHrana::sqld` also refuses what `sqld`'s statement analysis refuses (libsql-server `query_analysis.rs`: a PRAGMA it does not know, and one of its settings given a value, `query_only = 1` and `soft_heap_limit = …` among them), the whole batch as `SQL_PARSE_ERROR`.                                                                                                                                                                                                       |
| `*/session_tests.rs`, `tests/libsql.rs`, `tests/d1.rs` | The sessions end to end against the fakes; the real-server suites (skipped without their variable).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |

**Caps** (`libsql::session::capabilities`, `d1::session::capabilities`, compared with what each session reports in `connects_over_http_and_reads_the_catalog` / `connects_by_name_or_uuid_and_reads_the_catalog`):

- Both start from `SessionCaps::base` (`sqlite_family`: SQLite's namespaces and data caps, `Editor`, `StructureView`) and grant:
  - `Explain` (EXPLAIN QUERY PLAN on the editor: `the_editor_streams_caps_and_keeps_its_transaction`, `the_editor_runs_one_request_per_statement`);
  - `EditRows`, `InsertRows`, `DeleteRows`, `ServerExpressions` (`the_grid_reads_counts_fetches_and_commits_through_the_commands`: `apply_changes_on` with a failing change, a server expression, D1's blob written into the statement);
  - `ExactCount`, `TableDdl`, `ViewDefinition`, `SessionDetails` (the same test and the catalog tests);
  - `Mcp` (MCP opens a connection it may not write to read-only, which both sessions enforce: `read_only_sessions_refuse_writes_on_every_path`, `read_only_sessions_refuse_writes_before_sending_them`);
  - `Format`, `CopyAsInsert`: the renderer's, on the core SQLite dialect; nothing in the backend.
- LibSQL also: `EditWithoutKey` (rowid, under the batch's `changes()` guard, which fails the batch before `COMMIT`), `Returning` (rows read back from the batch), `StartupCommands` (`startup_commands_and_foreign_keys_run_on_every_new_stream`).
- D1 also: `SwitchDatabase` (`switching_databases_reconnects_by_name`; `namespaces.databases` is on). Never `EditWithoutKey` (its descriptor has no rowid) nor `Returning` (the rows a `RETURNING` sends back are not narrowed).
- Data: `CountMode::Costly` for both (Turso and D1 bill the rows a `COUNT(*)` reads, and D1's API is rate limited), `TxMode::Atomic` (each commit is one batch), `Affected::Exact`, D1's 100 parameters from its descriptor.
- Safety: LibSQL `ClassifierOnly`, and `PerRequest` on a read-only session whose server took `PRAGMA query_only = 1` (`read_only_sessions_refuse_writes_on_every_path`, `on_sqld_the_classifier_alone_keeps_a_session_read_only`); D1 `ClassifierOnly`; not hardened (nothing to harden: the SQL runs on a server with no access to this machine); Safe Mode confirms with the passcode (the secret is a token). Lifecycle: no keep-alive (Turso streams expire after 10 s anyway), ping free (LibSQL `GET v3`; D1 sends nothing: there is no connection, and every API call counts against the account's rate limit).
- Not granted: `StructureEdit` and the table operations, `Diagram`, exports, imports, `Backup` / `Restore`, `Metrics`, `Assistant`, `WorkspaceCommit`, routines, sequences, triggers, processes, users and database admin: their commands build SQL with the core SQLite generators (`core_dialect`), which a driver beyond the core three reaches only through its descriptor's `core_reuse` (empty). `ExplainAnalyze` would be the same EXPLAIN QUERY PLAN; `CancelOnServer`: neither server has a cancel; `Encoding`, `Timezone`: none.

**How it behaves.**

- **Connect** probes the protocol (LibSQL `GET v3`, then `v2`) and reads `sqlite_version()` (D1 also the database's details), so a refused token, a missing database or a failing startup command fails it. Server info: `LibSQL (SQLite 3.x)` with the host as the database; `Cloudflare D1 (SQLite 3.x)` with the database's name.
- **SSH and TLS** (LibSQL): `HttpClient::for_target` maps the profile's host to the tunnel and keeps the name in the Host header, SNI and the certificate check (`an_ssh_tunnel_keeps_the_servers_name`, `tls_checks_the_certificate_the_profile_trusts` against `testkit::tls`). The certificate is checked unless the SSL mode is plain "HTTPS" (`require`): `prefer` and a URL whatever the mode check it fully, so a token is never sent to a server that only claims the name (`https_checks_the_certificate_unless_told_not_to`). D1 has no SSH (its descriptor) and always verifies `api.cloudflare.com`.
- **Cancel and time out**: the request is dropped; the server finishes a statement it has. On LibSQL the editor's stream goes with it, and the next statement says that a transaction it held was lost (`a_cancel_drops_the_request_and_the_time_out_reports_it`, both drivers).
- **Read-only**: both sessions' `query`, `execute`, their cancellable forms, `transaction`, `run_statement` and `stream_query` refuse what the classifier does not call a read, before sending; `run_statement` is also where an explain plan arrives, which the commands layer does not classify although SQLite applies a PRAGMA while preparing it (`EXPLAIN QUERY PLAN PRAGMA soft_heap_limit = …` would set a limit for the whole server process: `refuses_every_write`, D1's `read_only_sessions_refuse_writes_before_sending_them`). LibSQL: startup commands that write fail the connect before anything is sent (`read_only_startup_commands_cannot_write`); where the server takes `PRAGMA query_only = 1` every batch and cursor also starts with it, on a held stream too, and a new stream repeats it after each startup command; `sqld` and Turso refuse it, so there the classifier alone keeps the session read-only (`on_sqld_the_classifier_alone_keeps_a_session_read_only`). Both also refuse text where SQLite reads a TCL-style variable (`$a(--)`, one token to SQLite) whose suffix opens a comment or a quote for the classifier, which would miss the code after it (`variables_that_hide_code_never_write`; D1 itself refuses a variable it is given no value for, which the fake models, and `sqld` a text of several statements); the commands layer refuses first as well, except explain plans.
- **One statement per D1 request**: D1 runs every statement of a `;`-joined text, so each call checks that it sends one (`a_raw_filter_cannot_add_a_statement`: a browse or a count with a raw filter of `1) ; DELETE FROM t; SELECT * FROM t WHERE (1` is refused before its select is sent, on a writable session too). The editor and MCP split their scripts first.
- **Transactions on Hrana 2**: without `get_autocommit` the session follows the statements: `BEGIN` / `COMMIT` / `ROLLBACK` (`tx_effect`), and SQLite's savepoints: `SAVEPOINT` opens a transaction when none is, `ROLLBACK TO` keeps it, and after `RELEASE` the state is unknown, so a stream the server forgot is reported rather than replaced quietly (`hrana_2_savepoints_count_as_a_transaction`).
- **Trace**: every statement the app runs (catalog round trips as one entry, queries, commits per statement, exports) goes to the console log; editor runs are logged by the renderer.

**Tests**: the lane's filter (`lane.mjs sqlite-remote test`) runs the lane's lib tests, among them value mapping (`libsql::decode`, `d1::decode`: 2^53 + 1 a BigInt, decimals and timestamps as text, JSON; end to end in the grid tests), quoting and injection (`hostile_names_and_values_are_quoted_or_bound`, per driver: a table `we"ird'; --` with columns `c"ol` and `x'y`, an index `i"dx'` and a view `v'iew` through the catalog, Show DDL, browse, count, fetch cell and a commit that writes `z'; DROP TABLE t; --`; catalog statements carry names as bound values; D1's 100 parameters and 100 KB; a raw filter that adds a statement), the read-only classifier refusals of D1 and the variables it cannot read (`HIDDEN_WRITES`, kept as regression cases), Command mode (`a_command_token_runs_again_after_the_server_refuses_it`: the token printed at connect, then the command again after a 401), and per driver connect, list, browse with paging, count, fetch cell, a commit with one failing change, an editor run, a cancel and a time out, read-only refusals on every write path, explain plans among them, and `FakeHrana::sqld`, which refuses `PRAGMA query_only = 1` as `sqld` does (118 tests; `lane.mjs sqlite-remote test-all --snapshot`: 1,327 passed). `lane.mjs sqlite-remote bindings` writes `LibsqlParams.ts` and `D1Params.ts`; there is no `lex/*.json`, as both use the core SQLite dialect.

**What only a real server can confirm** (`tests/libsql.rs` with `TABLER_TEST_LIBSQL_URL`, `tests/d1.rs` with `TABLER_TEST_D1_URL`): Turso's `v3/cursor` and the `is_autocommit` condition (Turso documents only `v2/pipeline`), that `sqld` and Turso refuse `PRAGMA query_only = 1` as the fake's `sqld` mode does (so a read-only session reports `ClassifierOnly` there) and accept `EXPLAIN QUERY PLAN` of anything, the code a real `sqld` answers for an expired stream (the fake answers `STREAM_EXPIRED`), that `sqld` refuses a statement text of several (the fake does, as rusqlite's `prepare`), Basic authentication on `sqld`; D1's refusals as the fake models them from workerd's source (its authorizer, and the parameter rules of a text of several statements); D1's acceptance of JSON numbers as parameters (`ParamEncoding::Native`; the API documents strings, and `Strings` stays ready), the atomicity of a `/raw` batch, `meta.last_row_id`, EXPLAIN QUERY PLAN, and the `Retry-After` of a 429.

### Part B2: the renderer and the mock

Both sessions speak the core SQLite dialect, so the grid, the structure view and the SQL editor (splitter, Safe Mode classifier, highlighting, formatter) are the app's own: the lane module (`src/features/drivers/sqlite-remote/`) has the two connection forms and their URLs, no `DriverUi` and no `DialectUi`. Neither lane calls a write-capable `sessionCall` or `driverUtil` (D1's `listDatabases` is a read; the sessions have no calls), so there is nothing for `guardExecution` to confirm in the lane's code.

| File                                                 | What it does                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| ---------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `index.ts`                                           | The `LaneModule`: `kinds.libsql`, `kinds.d1`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `libsql.ts`, `LibsqlFields.tsx`                      | LibSQL's `KindUi`: rows Host/Port, Auth Token, SSL (the shared HTTPS menu), then the lane's URL field and Basic authentication (its User row shows once it is on); a new profile starts on "HTTPS, verify the certificate" with an empty port (443, or 8080 over HTTP). `validate` mirrors `libsql/entry.rs::validate` (a host or a URL, no secret in the URL (`hasSecret`, as `has_secret`: any user or password, any token key in any casing, empty or repeated), no URL with SSH, Basic needs a user, a host with `/ ? # @` or a space is a pasted URL). A URL with a secret pasted in the URL field is spread over the form on blur (an empty token key is dropped with it): the token into Auth Token, credentials into User and Password, host, port and SSL from it. `toProfile` keeps `url` and `basicAuth` only when set, the user only with Basic, no database. `summary` shows a stored URL as its scheme, host, port and path only (no user, password, query or fragment, so not a token the checks do not read as one, after `;` or in a fragment), and one that does not parse as "Invalid URL" when it holds `@`, `?` or `#`.                                                                                                                                       |
| `d1.ts`, `D1Fields.tsx`                              | D1's `KindUi`: Account ID (32 hexadecimal characters, the backend's `validate_account_id`), API Token, Database with **List databases** (`driverUtil("listDatabases")`, a read; the names fill the field's suggestions, an account with one database fills it in), and API base (HTTPS, or HTTP to this machine only, and no user, password, query or fragment, as `D1Params::api_base`: it is stored as plain text and a URL's user and password would go out as Basic auth). No SSL menu: the API's certificate is always checked. `toProfile` clears host, port, user and the TLS files.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `url.ts`, `params.ts`                                | `libsql://` (TablePlus's scheme) read as `libsql/url.rs` reads it and written back without the token (`?tls=0` for HTTP, the `tls` key read in any casing as the backend reads it, the port when it is not the scheme's, `user@` with Basic, a server's path from the `url` param); `d1://<account>/<database>` and `cloudflared1://` (`?apiBase=`, a token as the URL's password or `?apiToken=` goes to the secret; a link never carries an API base's user, password, query or fragment, even of a profile stored before the check). `+ssh` URLs of LibSQL are the shared code's. The params read leniently from a profile or the form.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `src/lib/ipc/mock/drivers/sqlite-remote/` `index.ts` | `libsqlMock` (a Turso database and a local `sqld`) and `d1Mock` (an account with `shop` and `analytics`, the latter opened read-only), `?mockDrivers=sqlite-remote` (or `libsql`, `d1`). Connecting checks what the backends' `validate` and `open` check, in their words (the account ID, the API base with its credential check, a LibSQL startup command that writes on a read-only profile, a database name or UUID that does not exist, a URL holding a secret as `has_secret` reads it, a URL with SSH, Basic without a user); `listDatabases` answers the picker.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `session.ts`                                         | The `MockSession` of both: `remoteCaps` (the backend's `FEATURES` on SQLite's base caps, `CountMode::Costly`, `ClassifierOnly` (LibSQL's `PerRequest` with `queryOnly`, which the mock's `sqld` and Turso never take), D1's `databases` namespace), the catalog under `main`, offset pages with filters and sorts, LibSQL's rowid identity for a table without a key (D1: none, and a rowid edit refused), counts, Show DDL, session details, a staged commit as one batch (deletes, updates, inserts; a constraint failure or a missing row rolls back the rest; `RETURNING` rows on LibSQL only), the checks of the backend's `worker_query` on a browse's or a count's SELECT with every raw filter in it, nested groups included (refused on a read-only session when it is not a read, and on D1 when it is more than one statement or hides one behind a variable), D1's one statement per editor run (`$a(--) ;` included; LibSQL answers a text of several with the server's `SQL string contains more than one statement`), and the read-only refusal (`This connection is read-only`) of the SQLite classifier and of `hidesCodeInAVariable` (a port of `hides_code_in_a_variable`: only `)` and ASCII whitespace end a variable, never a Unicode space) before running. |
| `data.ts`, `sql.ts`                                  | The databases (customers with a UNIQUE email, orders with a big integer beyond 2^53, a foreign key, an index and a trigger, `events` without a key, the view `big_spenders`, D1's `d1_migrations`), kept for the page. The editor runs SELECT (columns, `*`, `count(*)`, WHERE of AND-joined comparisons, ORDER BY, LIMIT / OFFSET, literals), INSERT, UPDATE, DELETE, EXPLAIN QUERY PLAN, `PRAGMA table_info`, `sqlite_version()`; D1 refuses BEGIN and SAVEPOINT in workerd's words; result columns name no table (editor results stay read-only).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |

**Tests** (`lane.mjs sqlite-remote ts`): `url.test.ts` (the backend's URL cases, tokens and credentials out, a parse / format round trip of every form for both schemes, through `parseConnectionUrl` / `profileToUrl` with the real lane module, `libsql+ssh://`, D1 refused over SSH), `forms.test.ts` (form → profile → form for both, the validate messages next to the schema's, the connection list's summaries), `fields.test.tsx` (a pasted URL's token moved to the secret, Basic's User row, D1's picker through `driver_util` and its failures), and the mock's `index.test.ts` (caps equal to `remoteCaps`, the connect refusals, catalog, pages, sort, filter, count, big integers, fetch cell, rowid edits on LibSQL and their refusal on D1, a commit with one failing change rolled back, the guard, `RETURNING`, the editor's statements, D1's BEGIN, a read-only session's refusals on the editor and the commit with reads still served, the shared classifier cases and `HIDDEN_WRITES`, the picker; the URL secrets `has_secret` refuses, raw filters in nested groups and a read-only LibSQL browse refused as the backend refuses them). 34 tests.

**Left or limited in the mock** (shared mock behaviour; change requests in the lane's report): the mock splits a lane session's editor text by line when its dialect is a core one (`laneStatements` loads no lexer for `sqlite`), so two statements on one line are refused as one text of several; switching D1's database changes the connection's name only (the mock's `switch_database` does not reach the lane session); `driver_util`'s mock gets no profile, so the picker's account is not checked there; a lane connection's `currentDatabase` is the profile's database (none for LibSQL; the backend says the host).

### Deliberate differences from TablePlus

- **No structure editing, import or export** on either engine (nor backup, the diagram, Metrics or the assistant): TablePlus treats both as SQLite and offers its SQLite structure editor, import and export on them. Here those commands build SQL with the core SQLite generators, which a driver beyond the core three reaches only through its descriptor's `core_reuse`, empty for both: the SQLite table rebuild cannot hold over HTTP (no `BEGIN` on D1, Turso's 5 s transactions), and the export is not narrowed for D1 yet (README, D1's narrowed SQL).
- **SQL editor results are read-only**: neither Hrana nor D1's API says which table and column a result column comes from, so a result cannot be edited in place; table tabs are.
- **LibSQL by host, port and SSL as well as by URL**: TablePlus takes a URL and the auth token; Tabler keeps the host form so an SSH tunnel works (TablePlus added SSH for LibSQL too), and takes a URL (`url` param) for a server under a path. A pasted `libsql://…?authToken=` URL (New connection from URL, a deep link, or the URL field) moves the token into the secret; Copy as URL never writes it. A written URL cannot say plain "HTTPS" (`require`): read back it checks the certificate.
- **D1's `d1://` and `cloudflared1://` URLs** (TablePlus's import schemes) are read and written as `d1://<account>/<database>`, with `?apiBase=` for another API base.
- **Read-only and Safe Mode on D1 are Tabler's**: a D1 token with only Read permission reportedly still wrote through TablePlus, so a read-only D1 connection refuses writes in the session itself (`ClassifierOnly`), and so does LibSQL's (`sqld` refuses no write for a session, and refuses `PRAGMA query_only = 1`; where a server takes it, it goes with every request as well).
- **One statement per D1 request**: D1's API accepts a `;`-joined text and runs all of it; Tabler sends each statement on its own (the editor splits), and refuses a text of several.
- **Row counts on request** (`CountMode::Costly`) and no D1 ping: both services bill the rows a count reads, and D1's API is rate limited.

### Left after B1

- Done in B2 (above): the renderer module (`KindUi` for both tiles: LibSQL host or URL, token, Basic, SSH, bootstrap commands; D1 account and the database picker through `listDatabases`; the dialect is the core SQLite one), the mock, URL import and format (`libsql://…?authToken=` moving the token into the secret, `d1://<account>/<database>`). The form should start LibSQL on "HTTPS, verify the certificate" (`KindUi.defaults`): the shared `https` SSL menu starts on plain "HTTPS" (`require`), which checks nothing; the backend already checks the certificate for `prefer`. With a URL the form keeps the SSL menu (plain "HTTPS" for a server whose certificate cannot be checked) and the CA file.

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
- LibSQL HTTPS checks the server's certificate unless the SSL mode is plain "HTTPS" (`require`), which sends the token to whatever answers at the address: choose it only for a server whose certificate cannot be checked.
- A reply read whole (a Hrana 2 result, any D1 result: both protocols send a result at once, so the editor's row cap applies only after it has arrived) or one cursor line larger than 256 MB fails the call rather than exhausting memory: add a `LIMIT`. Below that the reply is decoded whole, so at its peak it takes a multiple of its size: about 2 to 3 times for Hrana, and up to about 16 times for D1, whose cells each become a JSON value (a 256 MB reply of `0,` cells, which only Cloudflare or a chosen `apiBase` could send, would take several GB).
- LibSQL read-only: a startup command the classifier does not call a read (also a setting such as `PRAGMA cache_size = 100`) fails the connect, rather than being skipped. On `sqld` and Turso read-only is the classifier's alone (they refuse `PRAGMA query_only = 1`), and, as on D1, text with a TCL-style variable whose suffix holds a comment or quote opener is refused too.
- LibSQL on Hrana 2: after a `RELEASE` whether a transaction is open is unknown until the next `BEGIN`, `COMMIT` or `ROLLBACK`.
- D1: a statement text of several is refused (each request sends one); a read-only session also refuses text with a TCL-style SQLite variable whose suffix holds a comment or quote opener (`$a(--)`, `:a('`), even inside a string, since the shared classifier reads it differently from SQLite. Ping sends nothing, so a revoked token shows on the next call.
- LibSQL: a `base_url` on another scheme, host or port is not followed (it would get the token, and the SSH tunnel maps only the profile's host): the stream stays at the URL it used. Streams expire after about 10 s idle and transactions after about 5 s: an editor transaction left open is rolled back by the server, and the session says so.
- D1: no transactions across statements (a staged commit is one batch); blobs cannot be bound (they go in the statement as literals); at most 100 parameters and 100 KB per statement; integers beyond 2^53 are exact only where the select reads them as text (otherwise D1's JavaScript has rounded them); the account's API rate limit (1,200 requests per 5 minutes) answers 429. Outside a staged commit an affected count is D1's `meta.changes`, which includes rows changed by triggers and cascades.
- D1 reports no column types: in the grid and the editor a whole REAL reads as an integer (2.0 as 2, D1's JavaScript drops the fraction), and dates and JSON as text. Written back they keep their column's affinity.
- No structure editing, export, import, backup, diagram, Metrics Board or assistant: their commands use the core SQLite generators, which only the local SQLite driver may use (the descriptors' `core_reuse` is empty).
- Row counts on request only (`CountMode::Costly`): both services bill the rows a count reads.
- LibSQL: the editor's capped read outside a transaction drops its stream (the PRAGMAs and temporary tables set on it go too); inside a transaction the rest of the result is read and dropped, so the transaction stays. A Turso read-only token is not offered as server-side read-only: the session's classifier does it.
- Neither server cancels a statement: a cancel or a time out drops the request, the server finishes it (D1 within its 30 s limit).
