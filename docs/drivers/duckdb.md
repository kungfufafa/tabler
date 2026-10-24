# DuckDB: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                  |
| ----------------- | ------------------------------------------------ |
| Lane              | `duckdb`                                         |
| Databases         | DuckDB                                           |
| Drivers           | `duckdb`                                         |
| Picker kinds      | `duckdb`                                         |
| SQL dialect       | `duckdb`                                         |
| Cargo features    | `drv-duckdb`                                     |
| Home slot         | `s1`                                             |
| Bindings prefix   | `Duckdb*`                                        |
| Lib test filters  | `drivers::duckdb`, `sqlgen::dialect::duckdb`     |
| Real-server suite | `tests/duckdb.rs` with `TABLER_TEST_DUCKDB_PATH` |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/duckdb/**`
- `src-tauri/src/sqlgen/dialect/duckdb.rs`
- `src-tauri/tests/duckdb.rs`
- `src-tauri/tests/fixtures/duckdb/**`
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

## What to implement

- `entry.rs` (Phase B): `open`, `validate`, `util`, `util_kind` (plan §2.5). `<Prefix>Params` (`#[serde(rename_all = "camelCase", default)]`, ts-exported) parsed from `options.driver_params`.
- `DbSession`: the 18 required methods; overrides of `query_cancellable`, `execute_cancellable`, `drain_trace`, `sql_trace` and `editor_run_lock`; `dialect()`, `capabilities()` (accurate features, data, safety and lifecycle caps) and `flavor()` where it applies.
- Optional: `table_source()`, `call` / `call_kind`, `list_processes` / `kill_processes`, `table_ddl`, `explain_form`.
- Cancel through `CancelHandle::Custom(Arc<dyn CancelAction>)`; `cancel()` calls `cancel_portable` first.
- Renderer: a `KindUi` per tile, `DriverUi`, `DialectUi` and a `MockDriver`. Write-capable `sessionCall` / `driverUtil` helpers await `guardExecution` first.

## Decisions and corrections for this lane

- The first build compiles DuckDB's C++ (minutes): run it with `--heavy`, which holds every slot. Dev builds compile libduckdb-sys without debug info (critique C4). DuckDB lands last and becomes default-on then (plan A28, §6.1 step 13), when default builds on this machine move to a short target folder (critique C5).
- The module `drivers::duckdb` shadows the `duckdb` crate in parent scopes: refer to the crate as `::duckdb::…` (critique optional 2).
- Local files and remote sources: classify `read_*`, `glob`, `COPY`, `ATTACH`, `INSTALL`, `LOAD` and `EXPORT` / `IMPORT DATABASE` as writes, so a plugin's `context.execute` cannot read files silently (critique D5).
- One engine per file: the lane keeps one process-wide instance per canonical path and refuses a different configuration; the MCP pool keys SingleProcess drivers by file (critique D9, F5).
- Unattended (MCP) and read-only sessions are hardened: `enable_external_access=false`, `lock_configuration=true`, extension autoload and autoinstall off; the file-lock error says to close the file in Tabler or open it read-only (plan §5.3).
- `dev/fixtures/sample.duckdb` and its script live in the shared `dev/fixtures`: a change request to the lead.

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

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._
