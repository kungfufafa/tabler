# DuckDB: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                                                       |
| ----------------- | ------------------------------------------------------------------------------------- |
| Lane              | `duckdb`                                                                              |
| Databases         | DuckDB                                                                                |
| Drivers           | `duckdb`                                                                              |
| Picker kinds      | `duckdb`                                                                              |
| SQL dialect       | `duckdb`                                                                              |
| Cargo features    | `drv-duckdb`                                                                          |
| Home slot         | `s1`                                                                                  |
| Bindings prefix   | `Duckdb*`                                                                             |
| Lib test filters  | `drivers::duckdb`, `sqlgen::dialect::duckdb`, `sqlgen::portable::generated::duckdb::` |
| Real-server suite | `tests/duckdb.rs` with `TABLER_TEST_DUCKDB_PATH`                                      |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/duckdb/**`
- `src-tauri/src/sqlgen/dialect/duckdb.rs`
- `src-tauri/src/sqlgen/portable/gen/duckdb.rs`
- `src-tauri/tests/duckdb.rs`
- `src-tauri/tests/fixtures/duckdb/**`
- `src-tauri/tests/fixtures/classify-generated/duckdb.json`
- `src/features/drivers/duckdb/**`
- `src/lib/ipc/mock/drivers/duckdb/**`
- `src/lib/ipc/bindings/Duckdb*.ts`
- `src/lib/ipc/bindings/lex/duckdb.json`
- `docs/drivers/duckdb.md`

Anything else (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead. `node dev/scripts/lane-commit.mjs duckdb -m "duckdb: …"` refuses other paths.

## Scope of v1

### DuckDB

- **Identity:** `duckdb`, `SqlDialect::Duckdb`. Crates: duckdb `~1.10506.1` (bundled, json, parquet).
- **UI:** SQL grid.
- **Auth v1:** none; optional encryption key (DbPassword).
- **Required fields:** file (`sqlite_path`); "New…" through `driver_util`.
- **Out of scope v1:** MotherDuck, Quack, httpfs/remote files, SFTP, extension auto-install, triggers, ADD/DROP CONSTRAINT rebuild.
- **Test:** fully local, in-memory and tempfile (`close()` before the TempDir drops); `dev/fixtures/sample.duckdb` script.

## Descriptor values

Seeded from the plan (§5.2). The values in force are the descriptors in `src-tauri/src/types/driver.rs` (keepalive belongs to the session's caps); a change is a request to the lead.

| Driver | Location | Port (TLS) | SSH  | TLS style | Secret label  | Command TTL |
| ------ | -------- | ---------- | ---- | --------- | ------------- | ----------- |
| duckdb | File     | —          | None | None      | EncryptionKey | —           |

| Driver | Required fields / params | Label overrides | Keepalive | Read-only | Sharing       | Token secret |
| ------ | ------------------------ | --------------- | --------- | --------- | ------------- | ------------ |
| duckdb | file                     | —               | 30        | Server    | SingleProcess | no           |

## Read-only and hardening

| Engine | When read-only                                  | When unattended (MCP)                                                                                                                                               |
| ------ | ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| DuckDB | `access_mode=READ_ONLY` plus the unattended set | `enable_external_access=false`, `lock_configuration=true`, autoload and autoinstall off; the file-lock error is mapped to "close it in Tabler or open it read-only" |

Every classifier is default-deny: a statement or command it does not know is a write. The read-only mode in the caps (`Server`, `PerRequest` or `ClassifierOnly`) must say what the session really enforces.

**The engine, not the classifier, stops file reads named as tables.** The portable classifier (`sqlgen::portable::classify` with the rules in `sqlgen/dialect/duckdb.rs`) only knows the plain spellings: calls of `read_*()`, `glob()` and the other scanners in `write_calls`, a table function outside `table_functions`, and `COPY`, `ATTACH`, `INSTALL`, `LOAD`, `EXPORT` / `IMPORT DATABASE`. A replacement scan (`FROM 'x.csv'`, `FROM "C:/x.csv"`, `FROM x."/path"`, or `FROM data.csv`, whose parts the binder joins into a path when no table has that name) classifies as a read. So the lane must:

- open read-only sessions, unattended ones (`registry::unattended`, the MCP process) and plugin-origin ones with `SET enable_external_access = false` followed by `SET lock_configuration = true`, so no later `SET` / `RESET` can turn access back on, with extension autoload and autoinstall off. External access off still leaves DuckDB's own paths open to SQL: its temp directory (`<db>.tmp/`, or `.tmp/` under the process's working directory in memory) and the attached database files with the WAL paths beside them (`<db>.wal`, `.wal.checkpoint`, `.wal.recovery`, which COPY can create). So the open configuration sets no temp directory (`temp_directory = ''`), and an instance without a database file that takes writes turns DuckDB's local file system off as well (`SET disabled_filesystems = 'LocalFileSystem'`, before the lock): SQL then reaches no file at all, and DuckDB reads its databases through the handles it opened before. The lock covers `SET` and `RESET` only: the table and pragma functions that switch the logger, the profiler, the optimizer, verification, the progress bar and checkpoint-on-shutdown (`enable_logging`, `enable_profiling`, `PRAGMA enable_profiling`, `PRAGMA disable_optimizer`, …) still run. None of them reaches a file outside those paths, but two of them break the instance (see Limits);
- report `ReadOnlyMode::Server` with `safety.hardened` for those sessions, and never for a session without these settings;
- test it: in a hardened session `SELECT * FROM 'x.csv'`, `SELECT * FROM data.csv`, reading a file planted in DuckDB's default temp folder, COPY / EXPORT / ATTACH into it and `SET enable_external_access = true` must fail; in a read-only one COPY beside the database file too.

## What to implement

- `entry.rs` (Phase B): `open`, `validate`, `util`, `util_kind` (plan §2.5). `<Prefix>Params` (`#[serde(rename_all = "camelCase", default)]`, ts-exported) parsed from `options.driver_params`.
- `DbSession`: the 18 required methods; overrides of `query_cancellable`, `execute_cancellable`, `drain_trace`, `sql_trace` and `editor_run_lock`; `dialect()`, `capabilities()` (accurate features, data, safety and lifecycle caps) and `flavor()` where it applies.
- Optional: `table_source()`, `call` / `call_kind`, `list_processes` / `kill_processes`, `table_ddl`, `explain_form`.
- Cancel through `CancelHandle::Custom(Arc<dyn CancelAction>)`; `cancel()` calls `cancel_portable` first.
- Renderer: a `KindUi` per tile, `DriverUi`, `DialectUi` and a `MockDriver`. Write-capable `sessionCall` / `driverUtil` helpers await `guardExecution` first.

## Decisions and corrections for this lane

- The first build compiles DuckDB's C++ (minutes): run it with `--heavy`, which holds every slot. Dev builds compile libduckdb-sys without debug info (critique C4). DuckDB lands last and becomes default-on then (plan A28, §6.1 step 13), when default builds on this machine move to a short target folder (critique C5).
- The module `drivers::duckdb` shadows the `duckdb` crate in parent scopes: refer to the crate as `::duckdb::…` (critique optional 2).
- Local files and remote sources: classify `read_*`, `glob`, `COPY`, `ATTACH`, `INSTALL`, `LOAD` and `EXPORT` / `IMPORT DATABASE` as writes (critique D5). Files named where a table goes are left to the engine (lead decision, F4a): the hardened settings above, not the classifier, keep a plugin's `context.execute` and MCP from reading them.
- One engine per file: the lane keeps one process-wide instance per canonical path and refuses a different configuration; the MCP pool keys SingleProcess drivers by file (critique D9, F5).
- Unattended (MCP), plugin-origin and read-only sessions are hardened: `enable_external_access=false`, then `lock_configuration=true`, extension autoload and autoinstall off; the file-lock error says to close the file in Tabler or open it read-only (plan §5.3).
- `dev/fixtures/sample.duckdb` and its script live in the shared `dev/fixtures`: a change request to the lead. The content is the lane's `src-tauri/tests/fixtures/duckdb/sample.sql`.
- No `INTO` write keyword (Phase A): DuckDB has no `SELECT … INTO`, and `UNPIVOT … INTO NAME … VALUE …` reads.
- The crate is pinned `=1.10506.0` in Cargo.toml (DuckDB v1.5.6, source 069cc9f9b5).

## Definition of done (this lane)

Beyond the shared list in [README.md](README.md#definition-of-done-v1):

- **Classifier hardening (blocks landing).** `src-tauri/src/sqlgen/portable/gen/duckdb.rs` is complete and `src-tauri/tests/fixtures/classify-generated/duckdb.json` blessed, as [Classifier hardening](README.md#classifier-hardening) requires: every write keyword, write call and read starter of `dialect::duckdb::SPEC` in every shared context and in the contexts only this grammar has, an engine-realistic read corpus, its write forms and fuzz atoms, `suite().check()` and `suite().fuzz(…)` in place of the placeholder. Then a red-team review passes, each finding fixed and kept as a case. Known hard cases: the engine hardening, which the classifier cannot replace (a test that a replacement scan such as `FROM 'x.csv'` fails in a read-only, MCP and plugin session: `SET enable_external_access = false` then `SET lock_configuration = true`, and that `SET` cannot undo it); `read_*`, `glob` and the other scanners as calls anywhere; table functions outside `table_functions`; `COPY`, `ATTACH` / `DETACH`, `INSTALL`, `LOAD`, `EXPORT` / `IMPORT DATABASE`; `PIVOT_WIDER` / `PIVOT_LONGER`, `PIVOT` / `UNPIVOT`, `FROM`-first queries and `SUMMARIZE` as reads.

## Phase A status

Done (in the lane's files; nothing outside them changed):

- **Engine** (`drivers/duckdb/engine.rs`): one instance per canonical file path in a process-wide registry; the same settings share it (`Arc`), other settings are refused while it is open; in-memory databases are their own instance. Opened with `access_mode=READ_ONLY` for a read-only profile; read-only, unattended and plugin sessions are hardened: autoload and autoinstall off, community extensions and persistent secrets off and no temp directory in the open configuration; after the startup SQL `SET enable_external_access = false`, then `SET disabled_filesystems = 'LocalFileSystem'` when no attached database file takes writes and the startup SQL set no temp directory or allowed folder or file (`Instance::local_files_off`), then `SET lock_configuration = true`. Encrypted files open through `ATTACH … (ENCRYPTION_KEY …, READ_ONLY)` into an in-memory instance under their file name, which every connection `USE`s: the file is always read-only, since DuckDB 1.5's built-in crypto only decrypts (writing needs httpfs's OpenSSL), but the instance is not unless the profile is read-only (other files ATTACH and take writes, so does `memory`). `Instance::is_read_only` (a read-only profile, hardened with the local file system off: what `ReadOnlyMode::Server` may claim) and `is_file_read_only` (the file itself, also an encrypted one) tell the two apart. Startup SQL runs once per instance except `USE` / `SET schema` / `SET search_path`, which every connection repeats. `check_database_file` (the `DUCK` magic at byte 8) and `create_database_file` for the form's Browse / New…. Names and literals in the text the driver writes (the encrypted ATTACH and USE, macro DDL, STRUCT member names) are quoted in `sqlgen/dialect/duckdb.rs`.
- **Connections** (`conn.rs`): a `Worker` thread per connection (try_clone of the instance's), streaming execution from Arrow chunks (`stream_arrow` only for the schema, then `step`, so no panicking iterator), row cap, cancel through `CancelHandle::Custom` (the connection's `InterruptHandle`), transaction state by `tx_effect` (duckdb-rs's `is_autocommit` always answers true), DuckDB's status columns (`Count`, `Success`) turned into Affected / Empty, `fetch_whole` for whole blobs. `close()` ends the thread within 2 s; the file closes with its last connection.
- **Values** (`types.rs`, `decode.rs`): a `DuckType` tree read once per column from the logical type (Arrow alone cannot tell HUGEINT / UHUGEINT / DECIMAL(38,0), UUID / JSON from text, ARRAY sizes), then total decoders straight from the Arrow arrays (never `Row::get_ref`, which panics on string views, list views, Float16 and unknown shapes). Every type: integers past 2^53 and HUGEINT / UHUGEINT / UBIGINT / BIGNUM as `BigInt` text, DECIMAL text, FLOAT in its shortest form, NaN / ±inf as `Other`, BLOB preview or whole, BIT as `Other{bit}` `0101…`, dates, times, TIMESTAMP_S / MS / NS, TIMESTAMPTZ (UTC, `+00`), INTERVAL and infinities as DuckDB prints them (BC dates and DuckDB's ±5.8-million-year range included), UUID, JSON, ENUM, GEOMETRY as WKT, LIST / ARRAY / STRUCT / MAP / UNION as JSON text. Parameters go as typed values or text, which DuckDB casts.
- **Catalog** (`catalog.rs`): queries over `duckdb_databases / schemas / tables / views / columns / constraints / indexes / sequences / functions / types` of `current_database()` with `$1` / `$2` names, and parsers: server info, databases, schemas, tables and views with estimates and comments, columns (ENUM labels inline or from `duckdb_types`, `nextval` defaults as auto-increment, list element types), PK / UNIQUE as indexes, FKs (schema from the constraint text), CHECKs, CREATE INDEX columns parsed from `sql`, table and view DDL, sequences with the column that owns them, macros and table macros with their `CREATE MACRO` text, completion.
- **Params** (`params.rs`): `DuckdbParams { autoloadExtensions }` and `engine_config(profile, key, hardened)`.
- **Dialect** (`sqlgen/dialect/duckdb.rs`): the swept `RULES` (below), `from_hex` blobs, ILIKE, `json_contains`, RETURNING, `EXPLAIN` / `EXPLAIN ANALYZE` with `BEGIN TRANSACTION … ROLLBACK` around an executing analyze, text projection of VARIANT (which the duckdb crate refuses to decode) and TIMETZ (whose offset Arrow drops), side-effect functions, redaction of secrets, tokens, passwords and encryption keys.
- **Classifier suite** (`sqlgen/portable/gen/duckdb.rs`) complete: DuckDB's own active contexts (FROM-first, PIVOT forms, lambdas, list / struct / map literals, EXCLUDE / REPLACE / COLUMNS, QUALIFY, ASOF / POSITIONAL joins, samples, parameters, standard and E-strings, dollar bodies), inert ones, a read corpus starting with every read keyword, the write forms, fuzz atoms; fixture `tests/fixtures/classify-generated/duckdb.json` blessed.
- **Sample**: `src-tauri/tests/fixtures/duckdb/sample.sql` (every object kind and a row of every type) builds the tests' database; the ignored test `drivers::duckdb::testing::write_sample_file` writes it to `TABLER_DUCKDB_SAMPLE_OUT` for `dev/fixtures/sample.duckdb` (a change request).
- **Tests** (49 in the lane filters, all green): unit tests per module (also the redaction of every secret option and credential setting, and type aliases that are no UTF-8), and engine tests (`testing.rs`) on temp files and in-memory databases: sharing and closing (the file reopens with other settings, the temp dir is removable), a second open of a held file (refused with the hint on Windows), read-only and hardened sessions (file, in-memory, encrypted) refusing writes, replacement scans, `read_*`, `glob`, `json_serialize_plan` of a scanner, COPY, ATTACH, EXPORT, INSTALL / LOAD, a file planted in DuckDB's default temp folder and COPY / EXPORT / ATTACH into it, COPY beside a read-only database file (its WAL paths) and every attempt to undo the settings; startup SQL that allows a folder or a temp directory keeping the file system on (and the session out of `is_read_only`); `enable_profiling` with a save location and `enable_logging` with file storage writing nothing (refused with the local file system off, wedging the instance where it is on); SELECT … INTO and inspections never writing; encrypted files (the file read-only, the instance only with a read-only profile); streaming, caps, counts, cancel; every value of the sample and of `test_all_types()` under every Arrow export setting, compared with DuckDB's own text; the whole catalog of the sample; result type names that parse back whatever their STRUCT members are called (the reserved-word list kept equal to `duckdb_keywords()`).

**Classifier sweep** (DuckDB 1.5.6 `duckdb_functions()` with built-in JSON and Parquet, plus the documented core extensions and well-known community ones). Writes: every statement but the queries and inspections (COPY, ATTACH / DETACH, INSTALL / LOAD, EXPORT / IMPORT, CHECKPOINT, VACUUM, CALL, PRAGMA, SET / RESET, DML, DDL, transactions, PREPARE / EXECUTE by default-deny); calls of `read_*` (CSV, JSON, NDJSON, Parquet, text, blob, DuckDB files, Excel, Avro, Vortex), `glob`, `sniff_csv`, `parquet_*`, `st_read*`, `arrow_scan*` (process memory), `postgres_*`, `pg_clear_cache`, `mysql_*`, `sqlite_*`, `odbc_*`, `adbc_*`, `iceberg_*`, `delta_*`, `ducklake_*`, `lance_*`, `vortex_*`, `md_*`, `bigquery_*`, `snowflake_*`, `mongo_*`, `redis_*`, `airport_*`, `http_*`, `open_prompt`, `load_aws_credentials`; dynamic SQL: `query`, `query_table`, `json_execute_serialized_sql` and `json_serialize_plan` (it binds the SQL in its string, so the binders of `read_*`, `glob` and replacement scans open their files and a binder error lists another database's tables); `nextval`, `setseed`, `write_log`, `enable_*` / `disable_*` (logging, profiling), `truncate_duckdb_logs`, `force_checkpoint`, `dbgen`, `dsdgen`, `start_ui*`, `stop_ui_server`, `getenv`, `duckdb_extensions` (lists the extension folder), `duckdb_secrets` and `which_secret` (load the persistent secrets from `~/.duckdb/stored_secrets` and show their key ids and scopes), `duckdb_table_sample`; and any table function outside `range`, `generate_series`, `unnest`, `duckdb_*`, `pragma_*`, `json_each`, `json_tree`, `repeat`, `repeat_row`, `regexp_split_to_table`, `test_all_types`, `test_vector_types` (so `checkpoint()`, `summary()`, `seq_scan()`, `histogram()`, `st_drivers()`, autocomplete's `sql_auto_complete()` (lists folders), TPC-H / TPC-DS `*_queries()`, table macros… are writes). Every PRAGMA is a write, also the ones the configuration lock does not stop (`enable_profiling`, `enable_verification`, `disable_optimizer`, `enable_checkpoint_on_shutdown`, `enable_object_cache`, `enable_progress_bar`, `verify_*`). Reads: `currval`, `current_setting`, `getvariable`, `random`, `uuid`, `sleep_ms` (refused by the Metrics Board as a side effect), `json_serialize_sql` and `json_deserialize_sql` (they parse only, nothing is bound), the catalog functions. The extension entries DuckDB 1.5.6 lists for autoloading (`extension_entries.hpp`: functions, settings, secret types and providers of the core extensions, quack, fts, vss, odbc, tpch / tpcds and ui included) were checked too: their table functions and table macros are writes by default-deny (`quack_serve`, `quack_query`, `quack_stop`, `whoami`, `hnsw_index_scan`…), their pragmas are PRAGMA statements, and their scalar functions are pure or under a write prefix (`odbc_*`, `iceberg_*`, `postgres_*`). The dynamic-SQL class was checked against every function of `duckdb_functions()` taking SQL text: `query`, `query_table`, `json_execute_serialized_sql` and `json_serialize_plan` are all of them in 1.5.6. `INTO` is not a write word: DuckDB has no `SELECT … INTO` (a parser error, tested), and `UNPIVOT … INTO NAME … VALUE …` is a read.

Left for Phase B: `entry.rs` (`open` building the `EngineConfig` from the profile and `registry::unattended()`, `validate`, `util` / `util_kind` with "New…" and the file check), the `DbSession` on two workers (editor and worker connections of one instance), refusing a statement that calls `enable_logging` before it reaches the engine in every session (see Limits: file storage that cannot be written aborts the process when a connection closes), caps (`ReadOnlyMode::Server` only when `Instance::is_read_only`, `safety.hardened` only for hardened instances, no edits when `is_file_read_only`; `SwitchDatabase`, imports and exports only as second connections of the same instance), `USE` for database switching, the DDL planner (no ADD / DROP CONSTRAINT: rebuild), edits of nested values (JSON text to STRUCT / MAP literals), the renderer (`KindUi`, `DriverUi`, `DialectUi`, mock), `tests/duckdb.rs`, the red-team review.

## Research notes

> Background from the driver research (October 2026), with file references and no line numbers. Verify against the code; where it differs from the scope above or from README.md, those win.

### DuckDB

**Approach.**

Embedded engine through the `duckdb` crate (duckdb-rs), an API modelled on rusqlite. Suggested dependency: `duckdb = { version = "~1.10506.1", features = ["bundled", "json", "parquet"] }`, which is DuckDB 1.5.6. The crate version format is `1.MAJOR_MINOR_PATCH.x`, and the tilde keeps the bundled engine fixed while taking crate patches.

Do not take 1.20000.0 (DuckDB 2.0.0 LTS) yet: the engine was released 2026-10-21, the crate was published 2026-10-22, and DuckDB's Rust page still said a 2.0-based Rust client was "not yet available". Plan a move to `~1.20000` after it settles.

The API matches what the SQLite actor uses: `Connection::open_with_flags(path, Config::default().access_mode(AccessMode::ReadOnly)?.enable_autoload_extension(..)?.threads(..)?)`, `try_clone()`, `interrupt_handle() -> Arc<InterruptHandle>`, `is_autocommit()`, `prepare`/`query`/`Row::get_ref -> ValueRef`, `Appender` for fast imports. `Connection` is Send but not Sync, so it fits the actor model.

New `src-tauri/src/drivers/duckdb/` with Driver::Duckdb:

- Make `drivers/sqlite/actor.rs` generic (its `type Job` is tied to `rusqlite::Connection`), and port the streaming in drivers/sqlite/exec.rs and the cancel slot pattern (drivers/sqlite/mod.rs).
- Open ONE database instance and create the second (editor/worker) connection with `try_clone()`. Two `Connection::open` calls on the same file in one process conflict on the file lock.
- Run `ATTACH` bootstrap statements once per instance, but `USE`/`SET` per connection.
- Cancel: new `CancelHandle::Duckdb(Arc<duckdb::InterruptHandle>)` (drivers/mod.rs).
- Values: map the primitive ValueRef variants directly. Render STRUCT, MAP, UNION, LIST and ARRAY (and VARIANT if needed) through arrow's display formatter (arrow ^58 is a mandatory duckdb dependency) as Json/Text. GEOMETRY is WKB, so decode it with geometry::parse_wkb (src-tauri/src/geometry.rs).
- Add a new `Dialect::Duckdb` (sqlgen/quote.rs). It starts from the Postgres branches for quoting `"…"`, `$n` placeholders, ILIKE, `::` casts, RETURNING and ON CONFLICT, with overrides: hex is `hex(x)` (Postgres `encode()` means something else in DuckDB), blob literals are `'\xAA'::BLOB`, there is no ctid, no SAVEPOINT, and its own EXPLAIN and DDL.

**Windows build.**

`bundled` compiles DuckDB's C++ from the crate's embedded source tarball with the cc crate under MSVC. No system DuckDB or OpenSSL is needed, and no Perl/NASM beyond what SQLCipher already needs (Cargo.toml).

- Build cost: the first build is long (tens of minutes is typical; upstream gives no figures and only warns that build time for libduckdb-sys "increased dramatically"), with large unity translation units that need a lot of RAM per cc job. Enable only `bundled` + `json` + `parquet`.
- Paths: keep CARGO_TARGET_DIR short (e.g. `C:/t`). The README warns of MSVC C1083 errors on deep paths past 260 characters, and this repo sits under C:\Users\Developer\Documents\Code\tabler\src-tauri\target\…. `DUCKDB_DISABLE_UNITY=1` is the alternative.
- Linked system libraries: ws2_32, rstrtmgr and bcrypt.
- Toolchain: DuckDB 2.0 needs C++17 and deprecates amalgamation builds (relevant at the upgrade).
- Rust dependencies added: arrow ^58 (mandatory; arrow-array, buffer, data, schema and related crates), comfy-table, hashlink 0.10, strum 0.27, fallible-iterator, cast, num-integer. Build-time-only: ureq 3, zip 9, tar and flate2 for libduckdb-sys.
- Binary size grows substantially, typically tens of MB with LTO. Measure it with `pnpm tauri build`.
- Bundled builds leave out ICU (timezones and collations); it can be loaded at runtime with `INSTALL icu; LOAD icu;`. httpfs is not supported on Windows `bundled-cmake` builds.
- rustls is irrelevant: the engine makes no TLS connections unless runtime extensions are downloaded from extensions.duckdb.org.
- Option: put it behind a cargo feature (`duckdb = ["dep:duckdb"]`, on by default for release) so `cargo test` and clippy loops for other work can skip the C++ build.
- No symbol clash is expected with rusqlite/SQLCipher or aws-lc, since DuckDB's vendored libraries are namespaced, but verify with a full release link.

**Authentication.**

No authentication for local files. TablePlus shows only a file. Optional extras for Tabler:

- Encryption key: DuckDB ≥1.4 supports `ATTACH 'f.duckdb' (ENCRYPTION_KEY '…')`. The profile password would act as the key, as with SQLCipher (drivers/sqlite/mod.rs). Verify that encryption works in the plain `bundled` build without httpfs.
- Later and out of scope: MotherDuck (`md:` path + `motherduck_token`, which needs a runtime extension download) and DuckDB 2.0's Quack protocol (`ATTACH 'quack:host' (TOKEN …)`).

**Data model.**

Databases are the attached catalogs: `SELECT database_name, path, type, readonly, internal FROM duckdb_databases() WHERE NOT internal`, which leaves out `system` and `temp`. The current one is `current_database()`. Switching runs `USE <catalog>` on both connections, or reconnects through switch_database (commands/connections.rs) with profile.database set to the catalog and `USE` after the bootstrap.

Schemas: `duckdb_schemas()` for the current catalog, not internal (information_schema and pg_catalog are internal). The default is `main` (`current_schema()`). Tables come from duckdb_tables() and views from duckdb_views(), with estimated_size as the row estimate and comments. Possible extra objects: sequences (duckdb_sequences()), macros (duckdb_functions() with function_type 'macro'/'table_macro', not internal), ENUM and user types (duckdb_types()), secrets (duckdb_secrets()).

`TableRef` (types/schema.rs) has schema + name only, so cross-catalog objects need `USE` first; adding a catalog field to TableRef would be a larger change.

Row identity: PK when present. Otherwise RowIdentity::None, matching all columns (types/browse.rs). Do NOT use rowid: DuckDB says rowids follow physical storage, are only stable within a transaction, and should not be used as identifiers.

TablePlus history (Mac / Windows):

- Added DuckDB in Mac build 562 and Windows 6.1.0 build 276 (June 2024).
- Schema support in build 568 and Windows build 290; `main` as the default schema in build 578.
- Fixed display of UUID, Struct and HugeInt values (578) and decimals (576).
- Geometry support in 6.9.6 (May 2026).
- Ships DuckDB 1.5.5 in Mac 27.0.2 (Oct 2026).

**Query language.**

DuckDB SQL, PostgreSQL-flavoured with "friendly SQL":

- FROM-first queries, SELECT \* EXCLUDE/REPLACE, GROUP BY ALL, QUALIFY, PIVOT/UNPIVOT, list/struct/map literals, lambdas.
- File table functions (read_csv, read_parquet, read_json, or a file name used as a table).
- ATTACH/DETACH/USE, COPY, EXPORT/IMPORT DATABASE, SUMMARIZE, DESCRIBE, SHOW, PRAGMA/SET.
- Parameters `?`, `$1` and `$name`.
- Dollar-quoted strings, so split with the Postgres rules in sqlgen::split.

EXPLAIN gives a text tree in explain_key/explain_value columns. `EXPLAIN ANALYZE` executes the statement. There is no SAVEPOINT, so the in-transaction branch of `analyze_wrapper` (sqlgen/explain.rs) must refuse rather than use SAVEPOINT. DuckDB 2.0 switches to a PEG parser by default (`SET enable_legacy_parser = true` reverts) and disables single-arrow lambdas.

**Editing.**

Rows: UPDATE, DELETE and INSERT … RETURNING keyed by PK, otherwise by an all-column match guarded by `expect_affected(1)` inside `transaction()`. Upserts use ON CONFLICT on PK or UNIQUE columns. Transactions are BEGIN/COMMIT/ROLLBACK, and DDL is transactional.

DDL that DuckDB supports in place: ADD COLUMN (with default), DROP COLUMN, ALTER COLUMN TYPE … USING, SET/DROP DEFAULT, SET/DROP NOT NULL, RENAME COLUMN, RENAME TO, ADD PRIMARY KEY, CREATE/DROP INDEX, COMMENT ON, CREATE/DROP SEQUENCE, VIEW and MACRO, and SET/RESET table options (1.5).

Not supported: `ADD CONSTRAINT` and `DROP CONSTRAINT` ("not yet supported"), meaning UNIQUE, CHECK and FOREIGN KEY added after creation. Dropping or retyping a column that an index, PK, UNIQUE or multi-column CHECK depends on raises a Dependency Error. The planner must drop the index, alter and recreate it, or rebuild the table (CREATE new; INSERT … SELECT; DROP; RENAME), which reuses the SQLite rebuild idea in sqlgen/ddl.rs with DuckDB syntax. Foreign keys only support NO ACTION/RESTRICT (no CASCADE).

Triggers exist only from 2.0 (`CREATE TRIGGER`, with limits). Imports can use `Connection::appender` for bulk loads and `read_csv` directly.

**Introspection.**

Every query is filtered with `WHERE database_name = current_database()`. Turn lists into JSON with `to_json(list)` to parse them safely.

- Databases: `SELECT database_name, path, type, readonly, comment FROM duckdb_databases() WHERE NOT internal ORDER BY database_name`, plus `SELECT current_database(), current_schema()`.
- Schemas: `SELECT schema_name, comment FROM duckdb_schemas() WHERE database_name = current_database() AND NOT internal ORDER BY schema_name`.
- Tables and views: `SELECT table_name, 'table', estimated_size, comment, temporary FROM duckdb_tables() WHERE database_name = current_database() AND schema_name = $1 AND NOT internal UNION ALL SELECT view_name, 'view', NULL, comment, temporary FROM duckdb_views() WHERE database_name = current_database() AND schema_name = $1 AND NOT internal ORDER BY 1`.
- Columns: `SELECT column_name, column_index, data_type, is_nullable, column_default, comment, character_maximum_length, numeric_precision, numeric_scale FROM duckdb_columns() WHERE database_name = current_database() AND schema_name = $1 AND table_name = $2 ORDER BY column_index`. In 2.0 docs this also has is_generated and generation_expression; 1.5 does not. Treat `column_default` starting with `nextval(` as auto-increment.
- Constraints: `SELECT constraint_type, constraint_name, to_json(constraint_column_names), referenced_table, to_json(referenced_column_names), expression, constraint_text FROM duckdb_constraints() WHERE … AND table_name = $2`. Types are PRIMARY KEY, UNIQUE, FOREIGN KEY, CHECK and NOT NULL (skip NOT NULL). These supply the PK, unique "indexes", FKs (on_update/on_delete "NO ACTION") and checks.
- Indexes: `SELECT index_name, is_unique, sql, comment FROM duckdb_indexes() WHERE …`. `expressions` is always NULL and `is_primary` always false, so parse the columns from `sql`.
- DDL and view definition: `duckdb_tables().sql` and `duckdb_views().sql`.
- ENUM labels: `duckdb_types()` (labels) or `enum_range(NULL::t)`.
- Completion in one query: `SELECT schema_name, table_name, column_name FROM duckdb_columns() WHERE database_name = current_database() AND NOT internal ORDER BY schema_name, table_name, column_index`.
- Version: `SELECT version()` or `PRAGMA version` (library_version, source_id).
- Map types with a new `kind_from_duckdb_type`: BOOLEAN; TINYINT…BIGINT; UBIGINT; HUGEINT/UHUGEINT as BigInt; DECIMAL(p,s); DOUBLE/FLOAT; VARCHAR; BLOB; DATE; TIME; TIMESTAMP[\_S/_MS/_NS]; TIMESTAMPTZ; INTERVAL; UUID; JSON; ENUM; LIST/ARRAY; STRUCT/MAP/UNION/VARIANT as Json or Other; BIT; GEOMETRY.
- Do NOT reuse drivers/postgres/introspect.rs: DuckDB's pg_catalog is only a partial emulation.

**Testing without a server.**

Entirely local:

- Unit tests on `Connection::open_in_memory()` and tempfile `.duckdb` files, mirroring the SQLite tests in drivers/sqlite/mod.rs and introspect.rs: introspection of tables, views, PK/UNIQUE/FK/CHECK, indexes, sequences, ENUMs and nested types; value decoding of HUGEINT, DECIMAL, UUID, STRUCT, LIST, MAP, INTERVAL, TIMESTAMPTZ and GEOMETRY; streaming with the row cap; interrupt-based cancel; `is_autocommit` tx state; read-only access mode; the file-lock error when a second instance opens the file; ATTACH in the bootstrap running once per instance.
- A fixture `dev/fixtures/sample.duckdb` built by a script next to `pnpm dev:sqlite:build`.
- Renderer: duckdb fixtures in src/lib/ipc/mock (handlers.ts, fixtures.ts, query.ts DATA_TYPES).
- Cost: every `cargo test` that compiles the crate pays the C++ build once per target dir (cached afterwards).

**What it shares with an existing driver.**

No wire compatibility: it is an in-process engine. DuckDB 2.0's Quack client-server protocol is DuckDB-specific, not PostgreSQL wire, so drivers/postgres cannot connect to it.

Reusable parts:

- The SQLite driver's architecture (actor thread per connection, InterruptHandle cancel, `is_autocommit` tx state, channel streaming in drivers/sqlite/exec.rs), because duckdb-rs mirrors rusqlite's API.
- Postgres-like sqlgen behaviour as the starting point for a new Dialect::Duckdb.
- geometry::parse_wkb for GEOMETRY.

New code: catalog queries (duckdb_\* table functions), DDL rules, value decoding and EXPLAIN.

Touch points for the new Driver and Dialect: exhaustive `Driver` matches at types/profile.rs, conn_options.rs, mcp/tools.rs, assistant/context.rs, backup/args.rs, backup/discover.rs, commands/conn_extras.rs, commands/export.rs, drivers/mod.rs. Exhaustive `Dialect` matches in about 26 files (sqlgen/ddl.rs alone has 27 Mysql arms; sqlgen/transfer.rs 8; select.rs 6; objects.rs, admin.rs 5 each). Renderer `Record<Driver,…>` maps: src/app/connectionLabels.ts, src/features/admin/ProcessList.tsx, src/features/connections/BootstrapCommandsDialog.tsx, ConnectionDialog.tsx, src/features/plugins/snapshot.ts, src/lib/ipc/mock/query.ts; the zod enum at src/features/connections/schema.ts; url.ts and .

File-based `Driver::Sqlite` checks that should extend to DuckDB: commands/connections.rs, commands/profiles.rs, the backup copy in commands/backup.rs (use `EXPORT DATABASE` or `ATTACH` + `COPY FROM DATABASE`), and mcp/tools.rs.

**Connection fields.**

- Name / color / group / tag (common)
- Database file: path, stored in the existing `sqlite_path` field or a renamed or generic `file_path`, with Browse…, New… (creates a .duckdb like create_database_file, drivers/sqlite/mod.rs) and optionally `:memory:`. Refuse missing files the way check_database_file (drivers/sqlite/mod.rs) does; DuckDB files carry the magic bytes "DUCK" after an 8-byte checksum (verify).
- Read only (Config.access_mode = READ_ONLY; lets other read-only processes open the file at the same time)
- Encryption key (optional, password field: ENCRYPTION_KEY)
- Bootstrap commands (options.startup_sql), e.g. `ATTACH 'other.duckdb' AS other (READ_ONLY)`, `ATTACH 'x.sqlite' (TYPE sqlite)`, `INSTALL/LOAD`, `SET threads=4`, `SET memory_limit='4GB'`
- Autoload/auto-install extensions toggle (Config.enable_autoload_extension; turning it off avoids network downloads)
- Safe mode, Lazy load (common); no host, port, SSL or SSH (a remote file would need a TablePlus-style download like SQLite over SSH)
- URL import / deep link: TablePlus scheme `duckdb://<path>` (tableplus.com/docs/gui-tools/manage-connections)

**Effort:** XL.

**Risks.**

- Version timing: DuckDB 2.0.0 LTS shipped 2026-10-21, and files it creates cannot be opened by DuckDB 1.5.x. A Tabler on `~1.10506` will fail on files from an updated DuckDB CLI or Python, so it needs a clear error. Upgrading to `~1.20000` (crate published 2026-10-22) brings a new default parser, C++17, and deprecated amalgamation builds.
- Windows build: long MSVC compile of the DuckDB C++ and high RAM per unity translation unit; C1083 on deep target paths (this repo's target dir is deep); a much larger installer; the mandatory arrow ^58 dependency tree. This affects every `pnpm check` and `cargo test` unless DuckDB sits behind a cargo feature.
- Concurrency: one read-write process per file. A file open in DBeaver, Python or the DuckDB CLI fails with a lock error, and read-only mode only helps when no one writes. Within Tabler, open one instance and use `try_clone()`, never two `Connection::open` calls.
- ATTACH is per database instance while `USE` and `SET` are per connection. Running startup_sql on both connections, as drivers/sqlite/mod.rs does, would ATTACH twice and fail, so use `ATTACH IF NOT EXISTS` or split the bootstrap.
- `Statement::column_count`, `column_names`, `column_name` and `column_type` panic before the statement is executed, unlike rusqlite where exec.rs reads columns first. Port the streaming so it executes first, and never call these on unexecuted statements. Release builds use panic=abort (Cargo.toml).
- Exotic types: STRUCT, MAP, UNION, LIST, ARRAY, ENUM, UUID, HUGEINT/UHUGEINT, DECIMAL, INTERVAL, TIMESTAMP_NS, TIMESTAMPTZ (bundled builds lack ICU, so it shows UTC), GEOMETRY (WKB) and VARIANT. TablePlus needed several fixes for exactly these (builds 576 and 578). Grid reads should cast complex columns to VARCHAR server-side, and editor results use the arrow formatter.
- No rowid identity: tables without a PK are edited by all-column match, so duplicate rows make an edit roll back on the `expect_affected` mismatch.
- DDL limits: no ADD/DROP CONSTRAINT, Dependency Errors when altering indexed columns, no SAVEPOINT (sqlgen/explain.rs analyze_wrapper uses SAVEPOINT inside a transaction), no FK CASCADE. The planner needs DuckDB-specific rebuild and index-recreate sequences.
- Optimistic concurrency: write-write conflicts between the editor and worker connections (or against an ALTER) fail with "Transaction conflict". Retry or report clearly.
- Extensions: by default DuckDB may auto-install and auto-load extensions from extensions.duckdb.org (network access, writes under ~/.duckdb, must match the exact build). Whether C++ extensions (spatial, httpfs, icu, sqlite) load into a statically linked MSVC exe needs verification. Expose a toggle and default to built-in json and parquet.
- Encryption (`ENCRYPTION_KEY`) support in the plain `bundled` build without httpfs or OpenSSL is unverified.
- A new Dialect variant touches about 26 Rust files with exhaustive matches and the renderer's per-dialect code (editor completion, the PlanView for EXPLAIN text, structure editors). This is the main reason for the XL estimate.

## Limits

What v1 does not do (as of Phase A; the lane completes the list before it lands):

- **One writer per file.** DuckDB opens a file read-write in one process at a time: a file another program holds (the DuckDB CLI, Python, DBeaver) fails with the lock error and the hint to close it there or open the connection read-only. In Tabler one instance serves every connection of a file, so a second connection of an open file needs the same settings.
- **Files named where a table goes** (`FROM 'x.csv'`, `FROM data.csv`), and views or macros defined over a scanner, are read when queried: only hardened sessions (read-only, MCP, plugin) refuse that; the classifier catches the spelled-out scanners only.
- **Hardened sessions do not spill to disk**: they have no temp directory (DuckDB would open it to SQL), so a query that needs more than `memory_limit` (80% of RAM by default) fails with DuckDB's out-of-memory error. A profile whose startup SQL sets `temp_directory` (or `allowed_directories` / `allowed_paths`) gets that, and its hardened sessions may then read and write those folders and files: the local file system stays on, and a read-only profile then reports no `ReadOnlyMode::Server` (`Instance::is_read_only`).
- **A hardened session that may write keeps DuckDB's own paths.** Its database file (or a file its startup SQL attached read-write) takes writes, so DuckDB's local file system stays on, and SQL may create or overwrite the files DuckDB allows beside each attached database (`COPY … TO '<db>.wal'`, `.wal.checkpoint`, `.wal.recovery`; on macOS and Linux `COPY … (USE_TMP_FILE false)` onto the database file itself): such a session (MCP or a plugin on a read-write profile) can damage the databases it may write anyway, but reaches no other file. Read-only hardened sessions, and in-memory ones whose startup SQL attached no writable file, reach no file at all (unless their startup SQL allowed some, above).
- **Encrypted files are read-only**: the bundled DuckDB decrypts with its built-in crypto but needs httpfs (OpenSSL) to write encrypted data. Only the file is: with a read-write profile the session still ATTACHes and writes other files and the in-memory `memory` database, so it reports no read-only mode.
- **Read-only sessions keep in-memory scratch**: TEMP tables take writes, and so do an encrypted file's `memory` database and `ATTACH ':memory:'`; nothing of it outlives the session, and no file is written.
- **Logging and profiling can break an instance.** The configuration lock does not cover `enable_logging` and `enable_profiling` (table functions). `CALL enable_logging(storage := 'file', storage_path := …)` is refused where the local file system is off (`Instance::local_files_off`: hardened sessions with no writable database file whose startup SQL allowed no files); elsewhere, with a path the instance cannot write (outside the allowed paths in a hardened session, where external access is off; an unwritable one in any session), it makes every later statement of the instance fail, `disable_logging` included, and closing a connection of it then aborts the process (DuckDB 1.5.6 throws from a destructor). `CALL enable_profiling(save_location := …)` in a hardened session saves nothing and fails that connection's statements until `CALL disable_profiling()`. No file is written; the classifier calls both writes, so only a session that lets writes through (MCP or a plugin on a read-write profile, the editor after Safe Mode's prompt) reaches them, until Phase B refuses `enable_logging` itself.
- **Trace redaction** masks the string after a secret option of CREATE SECRET or ATTACH (`SECRET`, `SESSION_TOKEN`, `TOKEN`, `BEARER_TOKEN`, `ACCESS_TOKEN`, `PASSWORD`, `CLIENT_SECRET`, `CONNECTION_STRING`, `PROXY_PASSWORD`, `HTTP_PROXY_PASSWORD`), ATTACH's `ENCRYPTION_KEY` and the credential settings (`s3_secret_access_key`, `s3_session_token`, `azure_storage_connection_string`, `azure_proxy_password`, `http_proxy_password`, `pg_oauth_token`, `motherduck_token`, `password`), as whole words; swept from `duckdb_settings()` and from the extension settings, secret types and secret providers DuckDB 1.5.6 lists in `extension_entries.hpp`. A secret option written as a named argument (`quack_serve('quack:…', token := '…')`, `=>`) and the key of `PRAGMA add_parquet_key('name', '<key>')` (by position, `redact_calls`) are masked too (F8c). It cannot reach a password inside a connection string (`ATTACH 'host=… password=…' (TYPE postgres)`, `ATTACH 'md:?motherduck_token=…'`) or the values of a map option (`EXTRA_HTTP_HEADERS MAP {'Authorization': …}`, DuckLake's `METADATA_PARAMETERS`); key and client ids (`KEY_ID`, `CLIENT_ID`, `s3_access_key_id`) stay readable.
- **Values.** TIMESTAMPTZ shows in UTC (no ICU in the bundled build). TIMETZ in editor results shows its local time without the offset (Arrow drops it; the grid reads TIMETZ columns as text). A result holding a VARIANT fails with the duckdb crate's error (the grid reads VARIANT columns as text). ENUM result columns are typed `ENUM`, not by the user type's name (the catalog still has the type and its labels). Generated columns are not flagged (DuckDB 1.5's catalog does not tell them).
- **Schema changes** follow DuckDB: no ADD / DROP CONSTRAINT (a rebuild), foreign keys with NO ACTION only, no triggers, no SAVEPOINT (an executing Explain Analyze of a write inside a transaction is refused).
- **Extensions** beyond the built-in JSON and Parquet load through the profile's autoload setting (network; off in hardened sessions) or `INSTALL` / `LOAD` in the startup SQL. The startup SQL runs before the hardening, so a hardened session has the core extensions its startup SQL loaded (community extensions stay refused, `allow_community_extensions = false`), and their functions run there: the classifier calls their scanners and table functions writes, and external access off refuses what goes through DuckDB's file systems, but an extension that reaches files or servers on its own is the profile's choice. Statements cannot load more once the session is hardened. MotherDuck, Quack, httpfs and remote files are out of scope.
- **Macro definitions** (`catalog::parse_macro_definition`) leave out parameter defaults (DuckDB 1.5's `duckdb_functions()` does not list them) and parameter types.
- **Classifier false positives** (safe): a column named like a write keyword (`load`, `copy`) unquoted, `x GLOB (…)`, the SQL-standard `t PIVOT (sum(x) FOR …)` clause (a shared-classifier change request), a name after `->>` (`j->>col`: the shared classifier judges what follows Snowflake's flow operator as a statement, F8c; a path string or number there reads), and `duckdb_secrets()` / `which_secret()` (they load the persistent secrets from disk; a hardened session turns persistent secrets off) classify as writes.
