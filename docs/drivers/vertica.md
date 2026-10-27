# Vertica: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                                                          |
| ----------------- | ---------------------------------------------------------------------------------------- |
| Lane              | `vertica`                                                                                |
| Databases         | Vertica                                                                                  |
| Drivers           | `vertica`                                                                                |
| Picker kinds      | `vertica`                                                                                |
| SQL dialect       | `vertica`                                                                                |
| Cargo features    | `drv-vertica`                                                                            |
| Home slot         | `s2`                                                                                     |
| Bindings prefix   | `Vertica*`                                                                               |
| Lib test filters  | `drivers::vertica`, `sqlgen::dialect::vertica`, `sqlgen::portable::generated::vertica::` |
| Real-server suite | `tests/vertica.rs` with `TABLER_TEST_VERTICA_URL`                                        |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/vertica/**`
- `src-tauri/src/sqlgen/dialect/vertica.rs`
- `src-tauri/src/sqlgen/portable/gen/vertica.rs`
- `src-tauri/tests/vertica.rs`
- `src-tauri/tests/fixtures/vertica/**`
- `src-tauri/tests/fixtures/classify-generated/vertica.json`
- `src/features/drivers/vertica/**`
- `src/lib/ipc/mock/drivers/vertica/**`
- `src/lib/ipc/bindings/Vertica*.ts`
- `src/lib/ipc/bindings/lex/vertica.json`
- `docs/drivers/vertica.md`

Anything else (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead. `node dev/scripts/lane-commit.mjs vertica -m "vertica: …"` refuses other paths.

## Scope of v1

### Vertica

- **Identity:** `vertica`, `SqlDialect::Vertica`. Crates: tokio-rustls 0.26 (in lock), md-5 0.11; SHA-512 through aws-lc-rs.
- **UI:** SQL grid.
- **Auth v1:** password (SHA-512 or MD5 hash; cleartext only over TLS), TLS client certificate, OAuth token via password or Command.
- **Required fields:** host, user, database (5433).
- **Out of scope v1:** Kerberos, TOTP, COPY LOCAL, load balancing / backup nodes, complex types beyond Json.
- **Test:** scripted byte-stream fake (`fake_pg` pattern); hash vectors cross-checked with vertica-python. No Windows server (Docker only).

## Descriptor values

Seeded from the plan (§5.2). The values in force are the descriptors in `src-tauri/src/types/driver.rs` (keepalive belongs to the session's caps); a change is a request to the lead.

| Driver  | Location | Port (TLS) | SSH    | TLS style | Secret label | Command TTL |
| ------- | -------- | ---------- | ------ | --------- | ------------ | ----------- |
| vertica | Host     | 5433       | Tunnel | Libpq     | Password     | 600         |

| Driver  | Required fields / params | Label overrides | Keepalive | Read-only | Sharing | Token secret |
| ------- | ------------------------ | --------------- | --------- | --------- | ------- | ------------ |
| vertica | host, user, database     | —               | 30        | Server    | Indep   | no           |

## Read-only and hardening

| Engine  | When read-only                                         | When unattended (MCP) |
| ------- | ------------------------------------------------------ | --------------------- |
| Vertica | `SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY` | —                     |

Every classifier is default-deny: a statement or command it does not know is a write. The read-only mode in the caps (`Server`, `PerRequest` or `ClassifierOnly`) must say what the session really enforces.

## What to implement

- `entry.rs` (Phase B): `open`, `validate`, `util`, `util_kind` (plan §2.5). `<Prefix>Params` (`#[serde(rename_all = "camelCase", default)]`, ts-exported) parsed from `options.driver_params`.
- `DbSession`: the 18 required methods; overrides of `query_cancellable`, `execute_cancellable`, `drain_trace`, `sql_trace` and `editor_run_lock`; `dialect()`, `capabilities()` (accurate features, data, safety and lifecycle caps) and `flavor()` where it applies.
- Optional: `table_source()`, `call` / `call_kind`, `list_processes` / `kill_processes`, `table_ddl`, `explain_form`.
- Cancel through `CancelHandle::Custom(Arc<dyn CancelAction>)`; `cancel()` calls `cancel_portable` first.
- Renderer: a `KindUi` per tile, `DriverUi`, `DialectUi` and a `MockDriver`. Write-capable `sessionCall` / `driverUtil` helpers await `guardExecution` first.

## Decisions and corrections for this lane

- A driver of its own (not a PostgreSQL flavor): the wire protocol looks like PostgreSQL's but the session differs (plan §1.5 Q1). TLS through tokio-rustls, MD5 through md-5 and SHA-512 through aws-lc-rs.
- Read-only: `SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY` (plan §5.3).
- No Vertica server runs on Windows: the tests use a scripted byte-stream fake (the `fake_pg` pattern) and hash vectors checked against vertica-python.

## Definition of done (this lane)

Beyond the shared list in [README.md](README.md#definition-of-done-v1):

- **Classifier hardening (blocks landing).** `src-tauri/src/sqlgen/portable/gen/vertica.rs` is complete and `src-tauri/tests/fixtures/classify-generated/vertica.json` blessed, as [Classifier hardening](README.md#classifier-hardening) requires: every write keyword, write call and read starter of `dialect::vertica::SPEC` in every shared context and in the contexts only this grammar has, an engine-realistic read corpus, its write forms and fuzz atoms, `suite().check()` and `suite().fuzz(…)` in place of the placeholder. Then a red-team review passes, each finding fixed and kept as a case. Known hard cases: dollar-quoted strings (`$$…$$` and `$tag$…$tag$`) as inert contexts and glued to a write keyword; the meta-functions that change state: the deny-list of `write_calls` becomes an allow-list of the safe ones, so an unlisted meta-function is a write (a new rules field is a request to the lead); `COPY`, `EXPORT TO …`, `SELECT … INTO`.

## Research notes

> Background from the driver research (October 2026), with file references and no line numbers. Verify against the code; where it differs from the scope above or from README.md, those win.

### Vertica

**Approach.**

A separate `Driver::Vertica` with its own `drivers/vertica/` module and a new `Dialect::Vertica`, because tokio-postgres cannot talk to Vertica:

- The startup message differs. vertica-python sends protocol version 3.5 with Vertica-only parameters: client_label, client_type, client_version, client_os, client_os_user_name, client_os_hostname, client_pid, autocommit, binary_data_protocol, protocol_features, protocol_compat, workload, auth_category, plus oauth_access_token and totp when used.
- The default password authentication is Vertica's HASH / HASH_SHA512 (codes 65536 / 66048, carrying a 4-byte salt and a 16-byte user salt). OAUTH is code 12 and TOTP is code 14. postgres-protocol's Message::parse rejects all of these.
- RowDescription is different: a pool of type names comes before the fields, the table OID is 8 bytes followed by schema and table names, and fields carry a user-type flag, nullable and identity flags, and an optional parent attribute for complex types (protocol 3.12 and later).

A Vertica engineer has said on the Vertica forum that the protocol has diverged from PostgreSQL and is not publicly documented.

Rust options: vertica-rs 0.0.1 (2025-09, hosted on gitee, 362 downloads) is too immature. ODBC through odbc-api 29.2.0 (2026-10-04) requires the user to install the Vertica ODBC driver, plus unixODBC on macOS. Recommended: hand-roll the client on tokio + tokio-rustls, about 1.5–2.5k lines porting vertica-python's message layer. It needs:

- SSLRequest (80877103) and the startup message;
- authentication: cleartext, MD5, the SHA-512 hash, and an OAuth token;
- simple Query with text-format DataRow (same layout as PostgreSQL: i16 count, then an i32 length or -1 for NULL per column);
- extended Parse / Bind / Describe / Execute with `?` placeholders;
- CommandDescription, EndOfBatch, ErrorResponse and NoticeResponse fields;
- refusing CopyIn / LoadFile for COPY LOCAL;
- CancelRequest on a new socket with the backend pid and key;
- optionally LoadBalanceRequest.

Implement DbSession with a small pool plus one dedicated editor connection, as PgSession does.

**Windows build.**

The hand-rolled client is pure Rust with no C dependencies. It needs tokio, tokio-rustls 0.26.6 and rustls 0.23 (aws-lc-rs is already built), and md-5 0.11.0 and sha2 0.11.0, which are already in Cargo.lock (aws_lc_rs::digest is an alternative). MD5 response: 'md5' + md5hex(md5hex(password + user) + salt). SHA-512 response: 'sha512' + sha512hex(sha512hex(password + usersalt) + salt). It adds roughly 200–400 KB. The ODBC alternative: odbc-api links odbc32.dll, which ships with Windows, so the build needs nothing extra. But the Vertica client driver package must be installed separately, and the macOS build would need unixODBC. That does not fit a self-contained app.

**Authentication.**

TablePlus has offered Vertica on macOS since build 102 (2018). Build 520 updated the driver and build 532 started sending client information. The Windows changelog has no Vertica entry. Tabler should offer:

- password: the server-chosen SHA-512 hash or legacy MD5, or cleartext for LDAP users, only over TLS;
- TLS modes disable / prefer / require / verify-ca / verify-full, and a TLS client certificate for Vertica's `tls` auth method;
- an OAuth access token, through the password field or the Command Line mode;
- an optional TOTP prompt for MFA.

Kerberos/GSS is out of scope, because there is no pure-Rust GSSAPI.

**Data model.**

One database per cluster, so the database list has a single entry from v_catalog.databases. Below it are schemas: user schemas, plus the system schemas v_catalog, v_monitor, v_internal, v_func and v_txtindex, shown with 'Show System Schemas' (TablePlus build 532 made v_internal items visible). Objects are tables (regular, temporary, flex, external), views, projections, sequences / identity columns, user functions (UDx or SQL) and stored procedures. Projections are physical sorted and segmented copies; show them in Structure where PostgreSQL shows indexes. PK / UNIQUE / CHECK constraints can be enabled or disabled; FKs are informational.

**Query language.**

Vertica SQL is ANSI SQL and largely PostgreSQL-like: double-quoted identifiers, `::` casts, LIMIT/OFFSET, ILIKE. Vertica extensions include: CREATE PROJECTION … ORDER BY … SEGMENTED BY HASH(…) ALL NODES; COPY … FROM LOCAL or S3; MERGE; EXPORT TO PARQUET; flex tables (MAPLOOKUP); time series / GAPFILL; PROFILE \<stmt>; EXPLAIN [VERBOSE|JSON|ANNOTATED|LOCAL]; EXPORT_OBJECTS / EXPORT_TABLES for DDL; and meta-functions such as CLOSE_SESSION, INTERRUPT_STATEMENT and ANALYZE_STATISTICS. Server-side bind placeholders are `?`, hence Dialect::Vertica. There is no RETURNING, no ON CONFLICT and no ctid.

**Editing.**

INSERT / UPDATE / DELETE / MERGE work, but they are expensive (they create delete vectors). They also need a COMMIT unless the session is in autocommit: send `autocommit=on` at startup, or `SET SESSION AUTOCOMMIT TO ON`, on pool connections. The editor can stay transactional when the user asks for it. Generated SQL must differ from Dialect::Postgres:

- `?` placeholders (sqlgen/quote.rs);
- no RETURNING, so re-select by key;
- no ctid / rowid locator, so edit only tables with a PK. A WHERE on all columns is unsafe, because UPDATE/DELETE have no LIMIT;
- null-safe comparison with `<=>` (verify whether IS NOT DISTINCT FROM is accepted).

DDL: CREATE TABLE (with ORDER BY, SEGMENTED BY and PARTITION BY), ALTER TABLE ADD / DROP / RENAME COLUMN, ALTER COLUMN SET DATA TYPE (limited), SET DEFAULT, ADD / DROP CONSTRAINT, ALTER CONSTRAINT ENABLED / DISABLED, COMMENT ON, CREATE PROJECTION, TRUNCATE (commits automatically) and CREATE SEQUENCE. Read the existing DDL text with EXPORT_OBJECTS. Savepoints are supported.

**Introspection.**

All metadata comes from v_catalog / v_monitor; there is no pg_catalog.

- Databases: `SELECT database_name FROM v_catalog.databases`.
- Schemas: `SELECT schema_name, is_system_schema FROM v_catalog.schemata ORDER BY 1`.
- Tables: `SELECT table_name, is_temp_table, is_flextable, owner_name, create_time FROM v_catalog.tables WHERE table_schema = ?`, plus views from `SELECT table_name FROM v_catalog.views WHERE table_schema = ?`.
- Comments: `SELECT object_type, object_schema, object_name, comment FROM v_catalog.comments`.
- Columns: `SELECT column_name, data_type, data_type_id, character_maximum_length, numeric_precision, numeric_scale, is_nullable, column_default, is_identity, ordinal_position FROM v_catalog.columns WHERE table_schema = ? AND table_name = ? ORDER BY ordinal_position`. For views, use v_catalog.view_columns.
- Primary key: `SELECT constraint_name, column_name, ordinal_position FROM v_catalog.primary_keys WHERE table_schema = ? AND table_name = ?`.
- Foreign keys: `SELECT constraint_name, column_name, reference_table_schema, reference_table_name, reference_column_name, ordinal_position FROM v_catalog.foreign_keys WHERE table_schema = ? AND table_name = ?`.
- Unique and check constraints: v_catalog.table_constraints (constraint_type, predicate, is_enabled) with v_catalog.constraint_columns.
- Projections, shown like indexes: `SELECT projection_name, is_super_projection, is_segmented, segment_expression FROM v_catalog.projections WHERE projection_schema = ? AND anchor_table_name = ?`, plus v_catalog.projection_columns (sort_position).
- View text: `SELECT view_definition FROM v_catalog.views WHERE table_schema = ? AND table_name = ?`.
- DDL: `SELECT EXPORT_OBJECTS('', 'schema.table', false)`.
- Sizes and rows: `SELECT anchor_table_schema, anchor_table_name, SUM(used_bytes), SUM(row_count) FROM v_monitor.projection_storage GROUP BY 1, 2`. Count rows over super projections only.
- Sequences: v_catalog.sequences. Routines: v_catalog.user_functions and v_catalog.user_procedures.
- Sessions: `SELECT session_id, user_name, client_hostname, login_timestamp, statement_start, current_statement, statement_id FROM v_monitor.sessions`. Kill with `SELECT INTERRUPT_STATEMENT(?, ?)` or `SELECT CLOSE_SESSION(?)`.
- Users and grants: v_catalog.users and v_catalog.grants.
- Completion: one query over v_catalog.columns.

**Testing without a server.**

Docker image vertica/vertica-ce: Community Edition, single node, VMart sample database, user dbadmin, port 5433, free up to 1 TB and 3 nodes. CE images expire after one year, and the product is now named OpenText Analytics Database. Protocol unit tests can use a fake server modelled on connect.rs's fake_pg (910-993), with scripted byte streams for:

- startup → AuthenticationHashSHA512 (salt + 16-byte user salt) → ParameterStatus(protocol_version) → BackendKeyData → ReadyForQuery;
- RowDescription with a type pool and 8-byte table OIDs;
- DataRow with -1 NULLs;
- ErrorResponse and CancelRequest.

Cross-check the password hashing against vertica-python's password.py.

**What it shares with an existing driver.**

Not wire-compatible with tokio-postgres. The differences: a protocol 3.5 startup; auth codes 65536, 65541, 66048, 12 and 14; a different RowDescription; and extra backend messages (CommandDescription, EndOfBatch, LoadBalanceResponse, LoadFile, VerifyFiles, WriteFile, CopyDoneResponse). Reusable pieces:

- the same framing (1-byte tag + i32 length);
- postgres_protocol::message::frontend encoders for Query / Parse / Bind / Execute / Sync / Terminate / CancelRequest / SslRequest (the startup message must be custom);
- the SSH tunnel and the rustls verifiers in postgres/connect.rs;
- RowSink / RowChunker / CancelSlot / SqlTrace from drivers/mod.rs;
- much of decode.rs's text_to_value, because Vertica's text output is close to PostgreSQL's ('t'/'f' booleans, ISO dates and timestamps).

None of the catalog SQL in introspect.rs, objects.rs, admin.rs or sqlgen/ddl.rs applies.

**Connection fields.**

- Name / Color / Tag / Group
- Host
- Port (default 5433)
- User (e.g. dbadmin)
- Password + storage mode (Keychain / Ask / None / Command Line for OAuth tokens)
- Database (required; one per cluster, e.g. VMart)
- SSL mode + Key / Cert / CA Cert
- Backup server nodes (host:port list for failover)
- Connection load balance (on/off)
- Client label / Workload
- Autocommit (Tabler should turn it on for pool connections)
- Read only, Timezone, Bootstrap commands, Connect timeout
- Over SSH
- Safe mode

**Effort:** XL.

**Risks.**

- The protocol is undocumented and still evolving; vertica-python is the only reference, and protocol_features / protocol_compat negotiation may change between server versions.
- A new Dialect touches every exhaustive `match dialect` in sqlgen (select, mutate, ddl, transfer, objects, explain, admin, classify, split, quote) and the renderer's per-dialect code.
- Sessions are not autocommit by default; pool connections must be switched to autocommit, or reads hold snapshots and locks.
- Row editing without a PK is unsafe: there is no ctid, and UPDATE/DELETE have no LIMIT.
- Complex types (ARRAY / SET / ROW) need the parent-attribute RowDescription handling (protocol 3.12 and later) and nested value decoding.
- Product ownership keeps moving (Micro Focus → OpenText, renamed OpenText Analytics Database; docs.vertica.com 25.x pages now redirect to docs.rocketsoftware.com), so CE image availability and licensing for testing may change.

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._
