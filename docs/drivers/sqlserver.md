# SQL Server: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                                                      |
| ----------------- | ------------------------------------------------------------------------------------ |
| Lane              | `sqlserver`                                                                          |
| Databases         | SQL Server                                                                           |
| Drivers           | `sqlserver`                                                                          |
| Picker kinds      | `sqlserver`                                                                          |
| SQL dialect       | `tsql`                                                                               |
| Cargo features    | `drv-sqlserver`                                                                      |
| Home slot         | `s2`                                                                                 |
| Bindings prefix   | `Mssql*`                                                                             |
| Lib test filters  | `drivers::sqlserver`, `sqlgen::dialect::tsql`, `sqlgen::portable::generated::tsql::` |
| Real-server suite | `tests/sqlserver.rs` with `TABLER_TEST_SQLSERVER_URL`                                |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/sqlserver/**`
- `src-tauri/src/sqlgen/dialect/tsql.rs`
- `src-tauri/src/sqlgen/portable/gen/tsql.rs`
- `src-tauri/tests/sqlserver.rs`
- `src-tauri/tests/fixtures/sqlserver/**`
- `src-tauri/tests/fixtures/classify-generated/tsql.json`
- `src/features/drivers/sqlserver/**`
- `src/lib/ipc/mock/drivers/sqlserver/**`
- `src/lib/ipc/bindings/Mssql*.ts`
- `src/lib/ipc/bindings/lex/tsql.json`
- `docs/drivers/sqlserver.md`

Anything else (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead. `node dev/scripts/lane-commit.mjs sqlserver -m "sqlserver: …"` refuses other paths.

## Scope of v1

### SQL Server

- **Identity:** `sqlserver`, `SqlDialect::Tsql`. Crates: tiberius 0.13 (no default features; tds80, rustls, winauth, sql-browser-tokio, chrono), tokio-util `compat`.
- **UI:** SQL grid.
- **Auth v1:** SQL login; Windows NTLM (DOMAIN\user); Integrated (Windows only); Entra token via Command.
- **Required fields:** host, plus user unless integrated (1433, `master`).
- **Out of scope v1:** Kerberos on macOS, interactive Entra/MFA, LocalDB / named pipes, Always Encrypted, PRINT capture, per-statement counts inside a batch.
- **Test:** spec goldens (brackets, `N''`, `@P`, OFFSET/FETCH, TOP, OUTPUT, GO); `ColumnData` decoder tests. Opt-in: SQL Server 2022 Express/Developer native with TCP on (approval).

## Descriptor values

Seeded from the plan (§5.2). The values in force are the descriptors in `src-tauri/src/types/driver.rs` (keepalive belongs to the session's caps); a change is a request to the lead.

| Driver    | Location | Port (TLS) | SSH    | TLS style | Secret label | Command TTL |
| --------- | -------- | ---------- | ------ | --------- | ------------ | ----------- |
| sqlserver | Host     | 1433       | Tunnel | Tds       | Password     | 3000        |

| Driver    | Required fields / params | Label overrides | Keepalive | Read-only      | Sharing | Token secret |
| --------- | ------------------------ | --------------- | --------- | -------------- | ------- | ------------ |
| sqlserver | host                     | —               | 30        | ClassifierOnly | Indep   | no           |

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

- T-SQL statements need no terminator, so its lexer is `terminator_optional`: any write keyword anywhere makes the batch a write (plan A8).
- Read-only is ClassifierOnly and default-deny; `ApplicationIntent=ReadOnly` only routes to a replica. `OPENQUERY(srv, 'DELETE …')`, `OPENROWSET` and `OPENDATASOURCE` hide DML in a string, and `EXEC` and `SELECT … INTO` are writes too: list them with the `xp_` / `sp_` prefixes as side-effect functions (critique D4). MCP only when the caps grant `Mcp` (plan §5.3).
- The text projection is per column: `CONVERT(NVARCHAR(MAX), e)` mangles varbinary, which it reads as UTF-16 (critique D3).
- Integrated (Windows) authentication works on Windows only; Entra tokens come through Command mode.
- tiberius is built without native-tls or vendored OpenSSL (`tests/manifest_features.rs` guards it).
- Columns reach the grid in their own type; only money (tiberius reads it as a float: `CONVERT(NVARCHAR(40), e, 2)`), hierarchyid and the spatial types (`e.ToString()`, spatial lossy since WKT drops the SRID) and the 2025 `json` / `vector` types are projected as text. Pattern filters compare `CONVERT(NVARCHAR(MAX), e) COLLATE Latin1_General_100_BIN2` (datetime with style 121, money with style 2), so the plain variants are case-sensitive and the `…Case` ones are not, as the portable builders mean them.
- Encryption follows the SSL mode: Disable encrypts the login only, Prefer if possible, Require and the Verify modes always; Prefer and Require take any certificate (a default SQL Server certificate is self-signed), the Verify modes check it against the system roots plus the CA file, under the server's name. `strict` (driver param) is TDS 8.0. Never a fallback to plain text.
- Transactions use the TDS transaction manager, and each statement's count comes from `@@ROWCOUNT` sent after it (`tsql::counted`), since the DONE counts tiberius sums include those of triggers.

## Phase A status

Done (2026-10-28, after the verifier's round 1; `lane.mjs sqlserver clippy --snapshot` clean, `test --snapshot` 56 passed and 1 ignored, `test-all --snapshot` 1119 passed and 1 ignored):

- `sqlgen/dialect/tsql.rs`: the spec as above, T-SQL clocks (`SYSDATETIME()`, `SYSDATETIMEOFFSET()` for datetimeoffset), `switches_database` (a `USE` anywhere in a batch, not `USE HINT` / `USE PLAN`), redaction of `PASSWORD`, `OLD_PASSWORD`, `MEDIAPASSWORD`, `RMTPASSWORD`, `PROVSTR`, `SECRET`, `KEY_SOURCE` and the passphrase functions, `side_effect_functions`, and the driver's own statements (`SESSION_SETUP`, `TRANCOUNT`, `ROLLBACK_IF_OPEN`, `counted`, `use_database`, `kill`, `showplan`, `object_name`), with goldens of the builders (brackets, `N''`, `@P`, OFFSET/FETCH, TOP (1), OUTPUT INSERTED, GO batches).
- `sqlgen/portable/gen/tsql.rs`: the complete suite (82 reads, 110 write forms, 43 contexts of T-SQL's own beside the shared ones, 14 inert ones, fuzz atoms, 3,000 fuzz runs) and its blessed fixture. Forms that only write at the start of a batch (a procedure without EXEC, a cursor FETCH, a label) are written after a `GO` line, since later in a batch they are a column alias or a syntax error.
- `drivers/sqlserver`: `params` (`MssqlParams`, `MssqlAuth`), `connect` (config from the profile, `Endpoint` for tunnels, SQL Browser, one redirect, session setup), `conn` (one actor per connection, `Stop` per statement, Attention cancel, streaming with back-pressure, transactions), `decode` (every `ColumnData`: integers past 2^53, money, decimals, datetime rounding, datetime2 and time by scale, datetimeoffset in its offset, GUIDs, XML, sql_variant; impossible values become `Other`; typed bind parameters), `introspect` (`sys.*` queries and parsers: server, databases, schemas, tables with sizes, columns with alias / identity / computed / temporal, indexes with INCLUDE and filters, foreign keys, checks, triggers, definitions, completion, processes), `errors`, and `testing`: a scripted TDS server over an in-memory pipe that the actor's tests run against (login, parameters, cancel before the answer and between rows, a caller that stops listening, server errors with their line, counts, commit, rollback on a count or an error, a lost connection, a broken connection that drops its socket at once, a close behind a full queue, nvarchar with an unpaired surrogate read as U+FFFD through `connect::session_options`). Row counts come from `sys.partitions` alone in TABLES and TABLE (one shared subquery), sizes from `sys.allocation_units` joined per unit type, so LOB and row-overflow data do not multiply the count.

Left for Phase B:

- `entry.rs`: `open` (an `Endpoint` from `ConnectTarget`, the Command password as the Entra token, a pool of `Conn`s plus the editor's, server info from `SERVER_INFO`), `validate` (`MssqlParams::from_driver_params`, the user unless integrated / Entra), `util`, `util_kind`.
- `DbSession`: introspection through `introspect`, traced; `query` / `stream_query` with `Mode::Rpc`; the editor splits with `portable::split` over `LEX` (the classifier's own splitter) and sends each GO batch whole with `Mode::Batch`, repeating it for `GO n`; at the row cap a single read is stopped (`Stop`), anything else drained (drop the receiver); `in_transaction` from `TRANCOUNT`; cancel as `CancelHandle::Custom` over `Stop`; `switch_database` with `use_database` (a new connection on Azure SQL Database); Explain through `showplan` batches; caps (ClassifierOnly, `Mcp` only after the red-team review), `list_processes` / `kill_processes` (`PROCESSES`, `kill`), routines and sequences.
- The DDL planner and CREATE TABLE text from the catalog (`table_ddl`), which `ddl: None` leaves off for now.
- Renderer (form, `sqlserver://` / `mssql://` and ADO / JDBC strings), mock driver, bindings (`MssqlParams`, `MssqlAuth`, `lex/tsql.json`), the real-server suite.
- To verify on a real server: `@@ROWCOUNT` after a statement whose AFTER or INSTEAD OF trigger writes; the TEXTSIZE tiberius' login leaves; `SESSION_SETUP` on Azure Synapse; whether a batch takes an ODBC `{call p}` escape (then `CALL` must be a write word).

Requests to the lead (shared files):

- The scanner folds case with Unicode rules (`Scan::upper`), so `ſelect 1`, `wıth 1` and `uſe x` classify as reads while SQL Server runs them as procedure calls. Read keywords should match in ASCII only (`gen/tsql.rs` keeps the case, ignored until then).
- `SELECT NEXT VALUE FOR s` advances a sequence and classifies as a read; no single word of `RULES` can catch it (`NEXT` and `VALUE` occur in reads). A two-word rule, or a knob for it.
- Locking hints (`WITH (UPDLOCK)`, `XLOCK`, `HOLDLOCK`, `TABLOCKX`) take locks a read-only session should not; PostgreSQL's `FOR UPDATE` is a write for the same reason.
- A NULL bind parameter of unknown type goes as `nvarchar`, which no binary column takes: the portable builders could write `NULL` in SET and VALUES, or pass the column's kind with each parameter (`decode::to_param` takes it).
- `lane.mjs <lane> test <filters>` ignores the filters (`parseArgs` returns `opts.rest`, always empty, over the positional ones).

## Definition of done (this lane)

Beyond the shared list in [README.md](README.md#definition-of-done-v1):

- **Classifier hardening (blocks landing).** `src-tauri/src/sqlgen/portable/gen/tsql.rs` is complete and `src-tauri/tests/fixtures/classify-generated/tsql.json` blessed, as [Classifier hardening](README.md#classifier-hardening) requires: every write keyword, write call and read starter of `dialect::tsql::SPEC` in every shared context and in the contexts only this grammar has, an engine-realistic read corpus, its write forms and fuzz atoms, `suite().check()` and `suite().fuzz(…)` in place of the placeholder. Then a red-team review passes, each finding fixed and kept as a case. `RULES` must stay equal to `portable::testing::TSQL_RULES` (the renderer's T-SQL cases run against `TSQL_LIKE_TEST`, whose suite `gen/tsql_like.rs` is the foundation's), so a rule change is a request to the lead. Known hard cases: batches without terminators (a write keyword anywhere makes the batch a write, and `SELECT 1 DELETE FROM t` is two statements); `GO` (alone on its line, with a count, and inside strings, `[names]` and comments, where it separates nothing); `OPENQUERY`, `OPENROWSET` and `OPENDATASOURCE` with the DML inside a string; `EXEC` / `EXECUTE` of a procedure or of a string (`EXEC('DELETE …')`, `sp_executesql`) and the `xp_` / `sp_` procedures; `SELECT … INTO`.

## Research notes

> Background from the driver research (October 2026), with file references and no line numbers. Verify against the code; where it differs from the scope above or from README.md, those win.

### Microsoft SQL Server

**Approach.**

New Driver::Sqlserver + Dialect::Tsql (src-tauri/src/types/profile.rs Driver, default_port 1433; src-tauri/src/sqlgen/quote.rs Dialect; regenerated src/lib/ipc/bindings/Driver.ts + Dialect.ts). Native TDS through tiberius 0.13.0 (prisma/tiberius, released 2026-09-25 after 0.12.3 in 2024-07; the only mature pure-Rust TDS client), declared as `tiberius = { version = "0.13", default-features = false, features = ["tds80", "rustls", "winauth", "sql-browser-tokio", "chrono"] }`. 0.13 moved to rustls 0.23 / tokio-rustls 0.26 and builds its config with `ClientConfig::builder_with_provider(CryptoProvider::get_default() or aws_lc_rs::default_provider())`, the same fallback as drivers/postgres/connect.rs. APIs used: Config::{host, port, database, instance_name, application_name("Tabler"), readonly, authentication, encryption(EncryptionLevel::{NotSupported, Off, On, Required, Strict}), trust_cert, trust_cert_ca, trust_cert_ca_bundle, hostname_in_certificate, client_certificate, handshake_timeout, command_timeout, multi_subnet_failover, from_ado_string/from_jdbc_string for URL import}; Client::{connect(config, stream), query(sql, &[&dyn ToSql]) (sp_executesql with @P1..), execute -> ExecuteResult::rows_affected() per statement, simple_query (whole batch, several result sets separated by QueryItem::Metadata), cancel_query() (TDS Attention, drains the ack), begin/commit/rollback_transaction, bulk_insert, column_metadata}. Layout like drivers/mysql: drivers/sqlserver/{mod,connect,decode,introspect,editor}.rs plugged into drivers::connect (drivers/mod.rs). Each connection lives in its own tokio task (actor) because cancel_query takes &mut self while the QueryStream borrows the client: the actor select!s between the next stream item and a cancel channel, drops the stream, then calls cancel_query(); add CancelHandle::Sqlserver(mpsc::Sender\<>) to drivers/mod.rs and keep settle_cancels semantics. TCP: tokio::net::TcpStream::connect((target.host(), target.port())) so SSH tunnels work through ConnectTarget.host_override (drivers/mod.rs), set_nodelay(true), then tokio_util::compat::TokioAsyncWriteCompatExt::compat_write() (Cargo.toml tokio-util needs features = ["compat"]); TLS name = hostname_in_certificate(target.tls_host()). Named instances: tiberius::SqlBrowser::connect_named (UDP 1434) only when not tunnelled. Handle Error::Routing{host, port} (Azure SQL redirect) with one reconnect. Pool: a small hand-rolled pool of actors (bb8-tiberius 0.16 exists but is unnecessary). Cross-cutting edits shared by all four new drivers: Driver/Dialect enums and every exhaustive match (sqlgen/ddl.rs 337 Dialect:: sites, transfer.rs 106, select.rs 85, objects.rs 71, definition.rs 65, metrics.rs 64, classify.rs 58, export/writer.rs 58, split.rs 57, redact.rs 48, mutate.rs 47), mcp/tools.rs dialect_of, commands/export.rs listed_schema, conn_options.rs, commands/conn_extras.rs encoding, commands/browse.rs identity_for/decode_rows, backup/args.rs (return Unsupported), renderer src/features/connections/colors.ts (DRIVER_LABEL, DRIVERS, DATABASE_KINDS, KIND_\*), ConnectionDialog.tsx driver-specific sections and an IPC mock per new command (src/lib/ipc/mock/). Rejected: mssql-tds 0.2.0 (microsoft/mssql-rs, first published 2026-09-10; native-tls + OpenSSL on Linux, no rustls, no docs URL) - revisit when stable; tiberius-ng 0.13.1 (community fork); ODBC (needs msodbcsql18 installed).

**Windows build.**

Pure Rust with the `rustls` feature: no OpenSSL, no system libraries. `winauth` is cfg(windows) only and depends on winapi 0.3 (features sspi, winerror; winapi 0.3.9 already in Cargo.lock) plus md5/rand 0.7/base64 0.10, linking Secur32 from the Windows SDK that MSVC builds already have. default-features MUST be off: the default set (tds80, winauth, native-tls) adds async-native-tls (SChannel on Windows, Security Framework on macOS, which the tiberius README says does not work with SQL Server TLS settings), and the README warns against enabling two TLS backends. Do not add `vendored-openssl` even though OpenSSL is already vendored for SQLCipher (Cargo.toml). tokio-rustls 0.26.6, rustls-native-certs 0.8.4 (reads the Windows cert store) and rustls-webpki 0.103.15 are already locked, so new crates are only tiberius, tiberius-macros, asynchronous-codec 0.7, connection-string, enumflags2, futures-timer, pretty-hex, secrecy 0.10 and winauth: well under a minute of extra build, small binary growth. `sspi-rs` (NTLM on macOS/Linux) and `integrated-auth-gssapi` (Kerberos via macOS GSS.framework / libgssapi) are cfg(unix) only; sspi 0.18 pulls picky, rsa rc, hickory, so leave it for a later macOS pass.

**Authentication.**

TablePlus offers SQL Server authentication (user/password), Windows auth with DOMAIN\user + password (issues #210, #244, #896), and Auth Type "Microsoft Entra ID (Token Only)" with "Get from CLI" for short-lived tokens; its Command Line password option is not offered for SQL Server (tableplus.com/docs/gui-tools/manage-connections). Tabler should offer an Auth Type select: (1) SQL Server login -> AuthMethod::sql_server(user, password); (2) Windows / NTLMv2 with DOMAIN\user + password -> AuthMethod::windows (on Windows via winauth NtlmV2ClientBuilder; macOS later via sspi-rs); (3) Integrated (current Windows user) -> AuthMethod::Integrated, on Windows via SSPI winauth NtlmSspiBuilder with the SPN derived from host:port; Windows-only in v1 (macOS would need integrated-auth-gssapi + kinit); (4) Microsoft Entra ID access token -> AuthMethod::aad_token(token) (FedAuth in prelogin/login), the token produced by the existing PasswordMode::Command (types/profile.rs, options.password_command), e.g. `az account get-access-token --resource https://database.windows.net/ --query accessToken -o tsv`, re-run on every connect/reconnect. tiberius has no interactive Entra/MFA, Entra password or managed-identity flow; those go through the command. TLS client certificates via Config::client_certificate(cert, key) (PEM/DER, rustls) mapped to the existing ssl_cert_path / ssl_key_path (types/profile.rs).

**Data model.**

Server -> databases -> schemas -> tables / views / procedures / functions / sequences / synonyms / triggers: the PostgreSQL three-level shape. list_databases = sys.databases, list_schemas = sys.schemas of the current database, TableRef.schema = the SQL Server schema (types/schema.rs). Catalog views are per database, so switching database = `USE [db]` on each pooled connection, or a reconnect with Config::database (mandatory on Azure SQL Database, EngineEdition 5, where USE and cross-database queries are not supported). TablePlus presents SQL Server like PostgreSQL (database chosen with Open Database, schemas in the sidebar); its open issues show paging done with TOP N that breaks on views (#3234), no column comments (#3676, #3233), lost datetimeoffset offsets (#1590), rowversion shown as garbage (#1982). Value mapping from tiberius ColumnData: U8/I16/I32/I64 -> Value::from_i64; F32/F64 -> Float; Bit -> Bool; Numeric (i128 + scale) -> Decimal(String); money/smallmoney arrive as F64 (lossy) so browse SELECTs cast them to decimal(19,4); String -> Text (varchar decoded with the collation code page); Guid -> Uuid; Binary -> Bytes (binary, varbinary, image, rowversion; CLR UDTs hierarchyid/geometry/geography also arrive as raw bytes, so select col.ToString() / col.STAsBinary() for ValueKind::Geometry via src-tauri/src/geometry.rs); Xml -> Text/Other; Date -> Date; Time -> Time; DateTime/SmallDateTime/DateTime2 -> Timestamp; DateTimeOffset -> TimestampTz keeping the offset; sql_variant decodes as its inner type. Row identity (commands/browse.rs): primary key, else a unique index, else none (no rowid; %%physloc%% is undocumented).

**Query language.**

T-SQL. Identifiers `[name]` with `]` doubled (new arm in Dialect::quote_ident, quote.rs; qualify quote.rs gives [schema].[table]); string literals N'...' with ' doubled and no backslash escapes (quote_literal / quote_literal_with, quote.rs; MysqlLiteralMode ignored); bytes 0x...; booleans 1/0 (select.rs literal()). Placeholders @P1..@Pn (Dialect::placeholder, quote.rs; tiberius sends them through sp_executesql); a request takes at most 2100 parameters, so transfer::max_params (transfer.rs) about 2098. Paging `ORDER BY ... OFFSET n ROWS FETCH NEXT m ROWS ONLY` (SQL Server 2012+; ORDER BY required: PK, else `ORDER BY (SELECT NULL)`) replacing the hard-coded LIMIT/OFFSET at select.rs; fetch-cell `SELECT TOP (1)` instead of LIMIT 1 (select.rs); count `SELECT COUNT_BIG(*)` (select.rs). No NULLS FIRST/LAST: emulate with a CASE WHEN col IS NULL sort key like the MySQL branch (select.rs). No ILIKE: case-sensitive filter variants use `COLLATE Latin1_General_100_BIN2` (pattern_sql select.rs, check_dialect 270-282); Any-column text form `CAST(col AS nvarchar(max))` (select.rs). Null-safe equality: `IS NOT DISTINCT FROM` only on 2022+, else `(c = @P OR (c IS NULL AND @P IS NULL))` (select.rs, mutate.rs). Editor: batches separated by `GO` lines (client-side, `GO n` repeats), semicolons optional; split.rs needs a T-SQL mode (bracket identifiers like the SQLite-only arms at split.rs, GO separators) and each GO batch is sent whole to simple_query because CREATE VIEW/PROCEDURE/FUNCTION/TRIGGER/SCHEMA must be first in their batch. EXPLAIN: `SET SHOWPLAN_XML ON` (estimated) / `SET STATISTICS XML ON` (actual) in their own batch, new arms in explain.rs. Processes and kill: sys.dm_exec_sessions + sys.dm_exec_requests, `KILL <spid>` (sqlgen/admin.rs).

**Editing.**

Full read-write. INSERT/UPDATE/DELETE with bound @P parameters typed per column (bind typed ToSql, not text: TablePlus #3163 fails bigint updates with 'text and varchar are incompatible'). Without a PK use `UPDATE TOP (1) ... WHERE <all columns>` / `DELETE TOP (1) FROM ... WHERE ...` in place of ctid/rowid/LIMIT 1 (mutate.rs); text/ntext/image/xml/UDT columns cannot be compared with = and must be left out of an all-columns key (TablePlus #1624). RETURNING becomes `OUTPUT INSERTED.*`, which sits before VALUES / before WHERE rather than at the end, so returning_clause (mutate.rs) needs a structural change; OUTPUT without INTO fails on tables with enabled triggers, then fall back to SCOPE_IDENTITY() + re-select. IDENTITY columns (sys.columns.is_identity) are omitted on insert; explicit values need `SET IDENTITY_INSERT t ON` (one table per session; also needed by SQL export, TablePlus #2078). Read-only columns: computed, rowversion/timestamp, GENERATED ALWAYS (temporal). `INSERT ... DEFAULT VALUES` and `SET c = DEFAULT` work. transaction(): BEGIN TRANSACTION ... COMMIT with ExecuteResult::rows_affected() for expect_affected; editor transaction state exact via `SELECT @@TRANCOUNT` instead of the tx_effect heuristics (drivers/mod.rs; a bare T-SQL BEGIN opens a block, not a transaction). DDL (sqlgen/ddl.rs): ALTER TABLE ADD / ALTER COLUMN / DROP COLUMN; renames via `EXEC sp_rename 'schema.table.col', 'new', 'COLUMN'`; defaults are named constraints (DROP CONSTRAINT DF_... before altering or dropping the column); CREATE [UNIQUE] [CLUSTERED|NONCLUSTERED] INDEX ... INCLUDE (...) WHERE ...; FKs with NO ACTION/CASCADE/SET NULL/SET DEFAULT; CHECK; comments through sp_addextendedproperty / sp_updateextendedproperty 'MS_Description'; DROP ... IF EXISTS (2016+); TRUNCATE (refused when FKs reference the table). DDL is transactional, so Preview/Apply can run inside one transaction. CREATE TABLE text must be generated from the catalog (no SHOW CREATE TABLE), as for PostgreSQL in sqlgen/definition.rs. Backup/restore: Unsupported (backup/args.rs/650 pattern), as in TablePlus (#1588).

**Introspection.**

Run in the current database (or with [db].sys.\* three-part names). server_info: `SELECT CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128)), @@VERSION, CAST(SERVERPROPERTY('EngineEdition') AS int), DB_NAME(), SUSER_SNAME(), SCHEMA_NAME(), @@SPID` (EngineEdition 5 = Azure SQL Database, 8 = Managed Instance). Databases: `SELECT name, SUSER_SNAME(owner_sid) AS owner, collation_name, state_desc, CASE WHEN name = DB_NAME() THEN 1 ELSE 0 END AS is_current FROM sys.databases WHERE HAS_DBACCESS(name) = 1 ORDER BY name`. Schemas: `SELECT name, CASE WHEN name = SCHEMA_NAME() THEN 1 ELSE 0 END FROM sys.schemas WHERE schema_id < 16384 AND name NOT IN ('sys','INFORMATION_SCHEMA','guest') ORDER BY name` (ids 16384-16399 are the fixed-role schemas). Tables/views: `SELECT o.name, o.type, CAST(ep.value AS nvarchar(max)) AS comment, ps.rows, ps.bytes FROM sys.objects o LEFT JOIN (SELECT object_id, SUM(CASE WHEN index_id IN (0,1) THEN row_count END) AS rows, SUM(reserved_page_count) * 8192 AS bytes FROM sys.dm_db_partition_stats GROUP BY object_id) ps ON ps.object_id = o.object_id LEFT JOIN sys.extended_properties ep ON ep.class = 1 AND ep.major_id = o.object_id AND ep.minor_id = 0 AND ep.name = 'MS_Description' WHERE o.schema_id = SCHEMA_ID(@P1) AND o.type IN ('U','V') AND o.is_ms_shipped = 0 ORDER BY o.name` (dm_db_partition_stats needs VIEW DATABASE STATE: fall back to sys.partitions.rows). Columns: `SELECT c.column_id, c.name, TYPE_NAME(c.user_type_id) AS type_name, TYPE_NAME(c.system_type_id) AS base_type, c.max_length, c.precision, c.scale, c.is_nullable, c.is_identity, c.is_computed, c.is_rowguidcol, c.generated_always_type, c.collation_name, dc.name AS default_name, dc.definition AS default_def, cc.definition AS computed_def, cc.is_persisted, CAST(ic.seed_value AS nvarchar(40)), CAST(ic.increment_value AS nvarchar(40)), CAST(ep.value AS nvarchar(max)) AS comment FROM sys.columns c LEFT JOIN sys.default_constraints dc ON dc.object_id = c.default_object_id LEFT JOIN sys.computed_columns cc ON cc.object_id = c.object_id AND cc.column_id = c.column_id LEFT JOIN sys.identity_columns ic ON ic.object_id = c.object_id AND ic.column_id = c.column_id LEFT JOIN sys.extended_properties ep ON ep.class = 1 AND ep.major_id = c.object_id AND ep.minor_id = c.column_id AND ep.name = 'MS_Description' WHERE c.object_id = OBJECT_ID(@P1) ORDER BY c.column_id` with @P1 = QUOTENAME(schema) + '.' + QUOTENAME(table) (max_length in bytes, -1 = MAX, halve for n-types). Indexes/PK/unique: `SELECT i.index_id, i.name, i.type_desc, i.is_primary_key, i.is_unique, i.is_unique_constraint, i.filter_definition, ic.key_ordinal, ic.is_descending_key, ic.is_included_column, COL_NAME(ic.object_id, ic.column_id) FROM sys.indexes i JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id WHERE i.object_id = OBJECT_ID(@P1) AND i.type > 0 ORDER BY i.index_id, ic.key_ordinal` (included columns -> IndexMeta.include, filter -> predicate). FKs: `SELECT fk.name, OBJECT_SCHEMA_NAME(fk.referenced_object_id), OBJECT_NAME(fk.referenced_object_id), COL_NAME(fkc.parent_object_id, fkc.parent_column_id), COL_NAME(fkc.referenced_object_id, fkc.referenced_column_id), fk.delete_referential_action_desc, fk.update_referential_action_desc, fk.is_disabled, fk.is_not_trusted FROM sys.foreign_keys fk JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id WHERE fk.parent_object_id = OBJECT_ID(@P1) ORDER BY fk.name, fkc.constraint_column_id`. Checks: `SELECT name, definition, COL_NAME(parent_object_id, parent_column_id), is_disabled FROM sys.check_constraints WHERE parent_object_id = OBJECT_ID(@P1)`. Triggers: `SELECT t.name, t.is_instead_of_trigger, t.is_disabled, te.type_desc, OBJECT_DEFINITION(t.object_id) FROM sys.triggers t JOIN sys.trigger_events te ON te.object_id = t.object_id WHERE t.parent_id = OBJECT_ID(@P1)`. View/routine text: `SELECT OBJECT_DEFINITION(OBJECT_ID(@P1))` (NULL WITH ENCRYPTION). Other objects: sys.objects type IN ('P','FN','IF','TF','X','SO','SN'), sys.sql_modules, sys.parameters, sys.sequences, sys.synonyms. Completion: `SELECT s.name, o.name, c.name FROM sys.columns c JOIN sys.objects o ON o.object_id = c.object_id JOIN sys.schemas s ON s.schema_id = o.schema_id WHERE o.type IN ('U','V') AND o.is_ms_shipped = 0 ORDER BY s.name, o.name, c.column_id`. Transaction state: `SELECT @@TRANCOUNT`. Users: sys.server_principals, sys.database_principals, sys.database_role_members. Add SYS. to CATALOG_WORDS (drivers/mod.rs) so the trace classifies catalog reads.

**Testing without a server.**

Offline: sqlgen unit tests in the existing style (quote.rs tests from line 125, select.rs tests around line 930, mutate.rs) for brackets, N'' literals, @P placeholders, OFFSET/FETCH, TOP (1), OUTPUT INSERTED; split.rs tests for GO batches; decoder tests that build tiberius ColumnData values directly; the fake DbSession pattern (commands/query.rs, commands/ddl.rs) and the renderer IPC mock (pnpm dev:mock) for UI work. Real server on this Windows box: SQL Server 2022 Developer or Express installed natively with TCP/IP enabled on 1433 (off by default in Developer/Express per the tiberius README); LocalDB is not usable (named pipes only, tiberius is TCP-only). Or Docker image mcr.microsoft.com/mssql/server:2022-latest added to dev/docker-compose.yml next to pg / mysql (Azure SQL Edge on ARM Macs). Azure SQL Database free offer for Entra tokens and redirect routing. microsoft/mssql-rs also contains an mssql-mock-tds mock server crate (not verified as published).

**What it shares with an existing driver.**

No. SQL Server speaks TDS, not the PostgreSQL, MySQL or SQLite protocols, so none of drivers/postgres, drivers/mysql or drivers/sqlite is reusable beyond the shared infrastructure (RowChunker, CancelSlot, SqlTrace, ConnectTarget, the SSH tunnel). Azure SQL Database / Managed Instance / Synapse / Fabric warehouses use the same TDS driver with catalog gaps: no USE or cross-database queries on Azure SQL Database, sys.dm_db_partition_stats restricted on some tiers, Synapse dedicated pools without enforced FKs.

**Connection fields.**

- Name, color, tag, group, safe mode, read-only (existing profile fields)
- Host
- Port (default 1433)
- Instance name (optional, resolved through SQL Browser UDP 1434; ignored when a port is set or over SSH)
- Auth type: SQL Server / Windows (NTLM) / Integrated (Windows only) / Microsoft Entra ID token
- User (DOMAIN\user for Windows auth)
- Password with storage mode keychain / ask / none / command (command = Entra token CLI)
- Database (default master)
- Encrypt: Not supported / Off (login only) / On (if available) / Required / Strict (TDS 8.0, SQL Server 2022+), mapped from SslMode plus a Strict option
- Trust server certificate (Config::trust_cert)
- CA certificate file (trust_cert_ca) and Hostname in certificate (hostname_in_certificate)
- Client key + client certificate files (mutual TLS)
- Application intent read-only (Config::readonly)
- Application name (default Tabler)
- Multi-subnet failover
- Packet size
- Connect timeout (handshake_timeout) and command timeout (None by default; the app enforces its own query timeout)
- Startup SQL / bootstrap commands (existing startup_sql)
- SSH tunnel (existing)

**Effort:** XL.

**Risks.**

- tiberius QueryStream yields only rows and result metadata: PRINT / RAISERROR info messages are only logged through `tracing`, and per-statement DONE row counts, return status and output parameters of a mixed batch are not exposed (only execute() returns rows_affected), so the editor cannot show 'N rows affected' per statement or PRINT output without a tracing layer or a vendored patch (the repo already vendors mysql_async via [patch.crates-io], Cargo.toml).
- tiberius 0.13 changed defaults: command_timeout 30 s per server round-trip (long queries fail with TimedOut unless set to None), handshake_timeout 15 s, encrypt now Required, trust_cert and trust_cert_ca panic when combined; repeated trust_cert_ca calls accumulate.
- Cancel needs the actor design: cancel_query takes &mut self after the stream is dropped, and dropping a query future does not stop server work.
- rustls speaks TLS 1.2/1.3 only: SQL Server 2008-2014 without TLS 1.2 updates fail the handshake (TablePlus #3720 'TLS version mismatch'); a native-tls fallback cannot be built alongside rustls.
- Default SQL Server certificates are self-signed, so verification fails unless 'Trust server certificate' is set; never silently fall back to an unencrypted connection (TablePlus #3032).
- Integrated auth on Windows uses SSPI NTLM (winauth NtlmSspiBuilder): domains that disable NTLM and require Kerberos will fail; macOS has no Windows/Integrated auth without sspi-rs or GSSAPI.
- Named instances need SQL Browser on UDP 1434, which does not pass through the SSH tunnel; named pipes, shared memory and LocalDB are unsupported (TCP only).
- Azure SQL: Error::Routing redirects to another host:port that bypasses an SSH tunnel; Entra tokens expire in about an hour (reconnect must re-run the token command).
- money/smallmoney decode to f64 and UDTs (geometry, geography, hierarchyid) to raw bytes: browse SQL must cast them.
- OUTPUT INSERTED.\* fails on tables with enabled triggers; IDENTITY_INSERT is per session and one table at a time.
- 2100-parameter request limit for bulk inserts and big IN filters.
- GO separators and optional semicolons break the ';'-based statement detection used by 'Run current'; CREATE PROCEDURE/VIEW/FUNCTION/TRIGGER must start a batch.
- tiberius sets the ODBC login flag (ANSI_DEFAULTS ON, so QUOTED_IDENTIFIER ON) but not ARITHABORT; send `SET ARITHABORT ON` after login (as SSMS does) or DML on indexed views / some filtered-index tables fails with SET-option errors (TablePlus #441).
- Adding Dialect/Driver variants breaks every exhaustive match (about 1,300 Dialect:: sites across sqlgen, export, metrics, commands): a large mechanical change; consider capability helpers on Dialect.
- tiberius had no release between 2024-07 and 2026-09; prisma's maintenance cadence is uneven (a community fork tiberius-ng exists), and SQL Server 2025 json/vector columns need verification.

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._ So far (Phase A):

- TCP only: no named pipes, shared memory or LocalDB. A named instance is found through SQL Browser (UDP 1434), so not over SSH; give the port there. A redirect to another host (Azure SQL's gateway) is refused over SSH.
- Windows and integrated authentication in the Windows build only (NTLM through SSPI; no Kerberos); no interactive Entra ID or MFA: an Entra token comes from the Command password.
- rustls speaks TLS 1.2 and 1.3 only: a server without TLS 1.2 (SQL Server 2014 and older without the update) cannot be reached.
- No PRINT or RAISERROR output and no row counts per statement inside a batch (tiberius does not report them); a batch's errors arrive after its result sets.
- Money values above about 900 billion lose digits where they are not projected as text (results of the editor); an alias type of money is not projected.
- A query that calls a user function with side effects (a CLR function with EXTERNAL_ACCESS, an extended procedure called from a function) is a read to the classifier, which cannot see into functions.
