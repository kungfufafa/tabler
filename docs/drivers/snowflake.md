# Snowflake: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                                                                |
| ----------------- | ---------------------------------------------------------------------------------------------- |
| Lane              | `snowflake`                                                                                    |
| Databases         | Snowflake                                                                                      |
| Drivers           | `snowflake`                                                                                    |
| Picker kinds      | `snowflake`                                                                                    |
| SQL dialect       | `snowflake`                                                                                    |
| Cargo features    | `drv-snowflake`                                                                                |
| Home slot         | `s2`                                                                                           |
| Bindings prefix   | `Snowflake*`                                                                                   |
| Lib test filters  | `drivers::snowflake`, `sqlgen::dialect::snowflake`, `sqlgen::portable::generated::snowflake::` |
| Real-server suite | `tests/snowflake.rs` with `TABLER_TEST_SNOWFLAKE_URL`                                          |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/snowflake/**`
- `src-tauri/src/sqlgen/dialect/snowflake.rs`
- `src-tauri/src/sqlgen/portable/gen/snowflake.rs`
- `src-tauri/tests/snowflake.rs`
- `src-tauri/tests/fixtures/snowflake/**`
- `src-tauri/tests/fixtures/classify-generated/snowflake.json`
- `src/features/drivers/snowflake/**`
- `src/lib/ipc/mock/drivers/snowflake/**`
- `src/lib/ipc/bindings/Snowflake*.ts`
- `src/lib/ipc/bindings/lex/snowflake.json`
- `docs/drivers/snowflake.md`

Anything else (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead. `node dev/scripts/lane-commit.mjs snowflake -m "snowflake: …"` refuses other paths.

## Scope of v1

### Snowflake

- **Identity:** `snowflake`, `SqlDialect::Snowflake`. Crates: snowflake-connector-rs `=1.1.0` (key-pair-auth; audit note RUSTSEC-2023-0071 for rsa 0.9).
- **UI:** SQL grid.
- **Auth v1:** key pair (`privateKeyPath` plus KeyPassphrase), PAT, OAuth via Command, password plus MFA (interactive; refused unattended).
- **Required fields:** params `account`, user.
- **Out of scope v1:** external-browser SSO, PUT/GET, more than one database per session, DDL editing.
- **Test:** mock through `custom_base_url`; JWT claims and fingerprint test. Opt-in: 30-day trial.

## Descriptor values

Seeded from the plan (§5.2). The values in force are the descriptors in `src-tauri/src/types/driver.rs` (keepalive belongs to the session's caps); a change is a request to the lead.

| Driver    | Location | Port (TLS) | SSH  | TLS style | Secret label    | Command TTL |
| --------- | -------- | ---------- | ---- | --------- | --------------- | ----------- |
| snowflake | Cloud    | —          | None | Https     | PasswordOrToken | 3000        |

| Driver    | Required fields / params | Label overrides | Keepalive | Read-only      | Sharing | Token secret |
| --------- | ------------------------ | --------------- | --------- | -------------- | ------- | ------------ |
| snowflake | user / `account`         | —               | None      | ClassifierOnly | Indep   | yes          |

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

- Read-only is ClassifierOnly and default-deny: `EXECUTE IMMEDIATE`, `CALL` and the `SYSTEM$…` functions are writes (critique D4, plan §5.3).
- The private key is a file path in params (`privateKeyPath`) with its passphrase in the new `KeyPassphrase` secret (plan A18, A19). A PAT can be longer than the 1,280 characters Windows Credential Manager keeps: store it through `set_secret` (UTF-8 bytes) or fail with a clear error (critique D8).
- Costs: `count` is Costly (estimate, exact only on click), no keep-alive, and a `ping` that never resumes a warehouse (plan §1.5 Q8). One database per session: switching database reconnects.
- snowflake-connector-rs `=1.1.0` brings rsa 0.9 (RUSTSEC-2023-0071, key-pair signing only): keep the audit note.

## Definition of done (this lane)

Beyond the shared list in [README.md](README.md#definition-of-done-v1):

- **Classifier hardening (blocks landing).** `src-tauri/src/sqlgen/portable/gen/snowflake.rs` is complete and `src-tauri/tests/fixtures/classify-generated/snowflake.json` blessed, as [Classifier hardening](README.md#classifier-hardening) requires: every write keyword, write call and read starter of `dialect::snowflake::SPEC` in every shared context and in the contexts only this grammar has, an engine-realistic read corpus, its write forms and fuzz atoms, `suite().check()` and `suite().fuzz(…)` in place of the placeholder. Then a red-team review passes, each finding fixed and kept as a case. Known hard cases: untagged `$$…$$` strings (there is no `$tag$` form, so a `$` elsewhere opens nothing) as an inert context, and a write keyword right after one; `SYSTEM$…` calls, which the scanner must read as one name (`SYSTEM$CANCEL_ALL_QUERIES(…)`, also qualified and with a space before the `(`); `EXECUTE IMMEDIATE` with the statement in a string or a `$$` body; `CALL`; `SELECT … INTO`.

## Research notes

> Background from the driver research (October 2026), with file references and no line numbers. Verify against the code; where it differs from the scope above or from README.md, those win.

### Snowflake

**Approach.**

New Driver::Snowflake + Dialect::Snowflake using snowflake-connector-rs 1.1.x (estie-inc, MIT; 1.0.0 2026-07-08, 1.1.0 2026-07-19, 1.1.1 2026-10-22), declared `default-features = false, features = ["key-pair-auth"]` (+ "external-browser-sso" later). It speaks the session API that the official connectors use (POST /session/v1/login-request, then POST /queries/v1/query-request?requestId=... with `Authorization: Snowflake Token="<session token>"`), so a Session is stateful: USE DATABASE / SCHEMA / WAREHOUSE / ROLE, ALTER SESSION, BEGIN/COMMIT, session variables and temporary tables persist across editor runs, as the DbSession editor contract needs (drivers/mod.rs). API: Client::new(ClientConfig{account, username, AuthConfig (password with optional MFA passcode, key pair from PKCS#8 PEM, OAuth access token, external_browser), SessionConfig (role, warehouse, database, schema)}) -> create_session(); session.query(stmt) -> ResultCursor (partitions fetched as you go: next_table / rows with Schema, Column, ColumnType, CellValue, DecimalValue, BinaryValue); Statement::bind / bind_named for ? and :name binds; session.query_handle(stmt) -> QueryHandle::canceller() -> QueryCanceller::cancel for a new CancelHandle::Snowflake; EndpointConfig::custom_base_url (PrivateLink, test mocks); ProxyConfig/TransportConfig. Transport is reqwest 0.13 + rustls, the app's own stack. Why not the public SQL API v2 (POST /api/v2/statements): it is documented and simple (Authorization: Bearer + X-Snowflake-Authorization-Token-Type KEYPAIR_JWT / OAUTH / PROGRAMMATIC_ACCESS_TOKEN; resultSetMetaData.partitionInfo + GET /api/v2/statements/{handle}?partition=N, gzip partitions, all values as strings, 202 = still running, POST .../{handle}/cancel), but explicit transactions, USE, ALTER SESSION, session variables and temporary tables only work inside a single multi-statement request, binds are not allowed in multi-statement requests, and there is no password login, so it cannot back an interactive editor session; keep it as the documented fallback. Rejected: snowflake-api 0.14 (Arrow dependency), adbc_snowflake (Go-based ADBC driver). Introspection through SHOW commands, which run without a warehouse.

**Windows build.**

Pure Rust, no OpenSSL. snowflake-connector-rs 1.1.1 depends on reqwest ^0.13 (default-features off; gzip, json, query, rustls - unified into the app's reqwest at Cargo.toml), serde_json raw_value, base64 0.23, flate2, chrono, uuid, indexmap, http, and with key-pair-auth: rsa 0.9, pkcs8 0.10 (encryption, pem -> pkcs5/pbkdf2/scrypt/aes/cbc), sha2 0.11 (already locked). A second rsa version (0.9) sits next to rsa 0.10.0-rc.18 from russh. external-browser-sso adds a local hyper 1 server + socket2 (nix only on unix). No Arrow, so build cost is small. Hand-rolling the SQL API instead needs no new crates for transport and RS256 (aws-lc-rs at Cargo.toml: RsaKeyPair::from_pkcs8 + RSA_PKCS1_SHA256), but encrypted PKCS#8 keys still need the pkcs8 crate and the SPKI fingerprint needs DER re-encoding.

**Authentication.**

TablePlus: user + password, an `authenticator` field for SSO / Okta (#1150, #2597, both closed), key pair added later (#2269 closed); region formats of the account identifier caused bugs (#802). Tabler: (1) Key pair (recommended for unattended use): private key file (PKCS#8 PEM, BEGIN PRIVATE KEY or BEGIN ENCRYPTED PRIVATE KEY + passphrase), JWT RS256 with iss \<ACCOUNT>.\<USER>.SHA256:\<base64 SHA-256 of the public key's SPKI DER>, sub \<ACCOUNT>.\<USER>, uppercase, periods in the account replaced by hyphens, region stripped, exp at most 1 h after iat; (2) Programmatic access token (PAT) used in place of the password; (3) OAuth access token via the existing Command password mode (types/profile.rs); (4) Password + MFA (TOTP passcode field or Duo push; the crate has no MFA token cache, so MFA may be asked on every connect); Snowflake is rolling out mandatory strong authentication (MFA for human password users, no passwords for service users; final enforcement window August-October 2026), so password must not be the default; (5) External browser SSO (crate feature external-browser-sso, experimental; IdP opened in the system browser, redirect to a localhost listener). Secrets: SecretKind (src-tauri/src/secrets.rs) only has DbPassword / SshPassword / SshPassphrase: add a key-passphrase kind or reuse DbPassword when auth = key pair.

**Data model.**

Account -> databases -> schemas -> tables (permanent, transient, temporary, external, dynamic, Iceberg, hybrid), views, materialized views, stages, pipes, streams, tasks, sequences, functions, procedures; plus the session's warehouse and role. Map to the PostgreSQL shape: list_databases = SHOW DATABASES, list_schemas = SHOW SCHEMAS IN DATABASE \<current>, TableRef.schema = schema; switching database = USE DATABASE on the session(s). TablePlus shows one database at a time with a picker at the bottom (#1399 asks for all), has autocomplete gaps for schemas not loaded yet (#3338), inserts uppercase identifiers (#1112) and can hang 'Refreshing' after choosing a database (#3670). Unquoted identifiers are stored uppercase, so always quote with the exact stored case returned by SHOW. Value mapping from the row type: FIXED / NUMBER(p,0) -> Value::from_i64 or BigInt (from strings), NUMBER(p,s>0) and DECFLOAT -> Decimal, REAL/FLOAT -> Float, TEXT/VARCHAR -> Text (always UTF-8: TablePlus #3806 garbled Japanese), BINARY -> Bytes (hex), BOOLEAN -> Bool, DATE -> Date, TIME -> Time, TIMESTAMP_NTZ -> Timestamp, TIMESTAMP_LTZ / TIMESTAMP_TZ -> TimestampTz, VARIANT / OBJECT / ARRAY -> Json, GEOGRAPHY / GEOMETRY -> Geometry or Other (GeoJSON / WKT per \*_OUTPUT_FORMAT), VECTOR -> Array.

**Query language.**

Snowflake SQL. Identifiers "name" with " doubled (same as the Postgres arm of quote_ident, quote.rs). Single-quoted string literals interpret backslash escape sequences, so literals need backslash doubling plus '' (the MySQL BackslashEscapes branch of quote_literal_with, quote.rs) or dollar-quoting $$...$$; comments --, //, /\* \*/. Binds ? or / :name with typed bindings (FIXED, REAL, TEXT, BINARY, BOOLEAN, DATE, TIME, TIMESTAMP_\*). LIMIT m OFFSET n paging (select.rs works unchanged), NULLS FIRST/LAST, ILIKE, IS [NOT] DISTINCT FROM and EQUAL_NULL() all exist, so most Postgres-dialect filter SQL applies. Editor: split client-side (split.rs needs a Snowflake mode: $$ strings, // comments, backslash escapes, Snowflake Scripting BEGIN ... END blocks) and run each statement on the same session (MULTI_STATEMENT_COUNT=0 is the alternative). EXPLAIN USING TEXT | JSON | TABULAR (explain.rs). Query history and cancel: INFORMATION_SCHEMA.QUERY_HISTORY_BY_SESSION(), SYSTEM$CANCEL_QUERY('\<query id>').

**Editing.**

Full DML: INSERT / UPDATE / DELETE / MERGE with binds. BEGIN / COMMIT / ROLLBACK on the session, but every DDL statement commits the open transaction implicitly (treat like MySQL in tx_effect, drivers/mod.rs). DML returns its counts as result columns ('number of rows updated', ...). PRIMARY KEY / UNIQUE / FOREIGN KEY are informational (enforced only on hybrid tables), so duplicate keys can exist: keep expect_affected = 1 (drivers/mod.rs) and roll back when more rows match; without a declared key fall back to all-columns matching (VARIANT/OBJECT/ARRAY via EQUAL_NULL / TO_JSON, floats risky). No RETURNING and no last-insert id: AUTOINCREMENT / IDENTITY values must be re-read or taken from a sequence NEXTVAL first. DDL: CREATE [OR REPLACE] [TRANSIENT] TABLE ... CLUSTER BY; ALTER TABLE ADD / DROP / RENAME COLUMN; ALTER COLUMN SET DATA TYPE (only widening VARCHAR length or NUMBER precision), SET / DROP NOT NULL, SET / DROP DEFAULT (sequence defaults), COMMENT; ADD / DROP PRIMARY KEY / UNIQUE / FOREIGN KEY; RENAME TO; SWAP WITH; UNDROP; CREATE ... CLONE. No indexes on standard tables (clustering keys and search optimization instead). CREATE text via GET_DDL('TABLE', '"DB"."S"."T"'). Backup/restore not applicable (Time Travel / clone).

**Introspection.**

SHOW commands need no running warehouse (they run in cloud services) but return at most 10,000 rows per object type (LIMIT cannot exceed 10000; page with LIMIT 10000 FROM '\<last name>'). server_info: `SELECT CURRENT_VERSION(), CURRENT_ACCOUNT(), CURRENT_REGION(), CURRENT_USER(), CURRENT_ROLE(), CURRENT_WAREHOUSE(), CURRENT_DATABASE(), CURRENT_SCHEMA()`. Databases: `SHOW DATABASES` (name, owner, comment, kind, is_current, is_default). Schemas: `SHOW SCHEMAS IN DATABASE "DB"`. Tables: `SHOW TABLES IN SCHEMA "DB"."S"` (name, kind TABLE/TRANSIENT/TEMPORARY, rows, bytes, comment, cluster_by, is_external, is_hybrid, is_iceberg), `SHOW VIEWS IN SCHEMA "DB"."S"` (is_materialized, text), `SHOW MATERIALIZED VIEWS`, `SHOW EXTERNAL TABLES`, `SHOW DYNAMIC TABLES`. Columns: `SHOW COLUMNS IN TABLE "DB"."S"."T"` (column_name, data_type JSON {type, precision, scale, length, nullable}, null?, default, kind, expression, comment, autoincrement). Keys: `SHOW PRIMARY KEYS IN TABLE "DB"."S"."T"` (column_name, key_sequence, constraint_name), `SHOW UNIQUE KEYS IN TABLE ...`, `SHOW IMPORTED KEYS IN TABLE ...` (pk_database_name, pk_schema_name, pk_table_name, pk_column_name, fk_column_name, key_sequence, update_rule, delete_rule, fk_name); hybrid-table indexes `SHOW INDEXES IN TABLE ...`. DDL: `SELECT GET_DDL('TABLE', '"DB"."S"."T"')` / GET_DDL('VIEW', ...). Other objects: SHOW USER FUNCTIONS / PROCEDURES / SEQUENCES / STAGES / PIPES / STREAMS / TASKS IN SCHEMA. Completion: `SHOW COLUMNS IN SCHEMA "DB"."S"` per schema, lazily. Pickers: SHOW WAREHOUSES, SHOW ROLES. "DB".INFORMATION_SCHEMA (TABLES, COLUMNS, TABLE_CONSTRAINTS, REFERENTIAL_CONSTRAINTS; no KEY_COLUMN_USAGE) needs a running warehouse and errors with 'Information schema query returned too much data' on unselective filters; SNOWFLAKE.ACCOUNT_USAGE lags by hours.

**Testing without a server.**

Offline: point snowflake-connector-rs at a local HTTP mock through EndpointConfig::custom_base_url serving canned /session/v1/login-request and /queries/v1/query-request JSON (several partitions, NULLs, every type); unit-test JWT claims and fingerprint with a generated test key; sqlgen and splitter tests. Emulators: fakesnow (Python, DuckDB-backed; `pip install fakesnow[server]` runs a standalone HTTP server for non-Python clients - check that it covers the endpoints the crate calls), LocalStack's Snowflake emulator (commercial Docker image, HTTPS, works with JDBC/Go/Python connectors by changing the host). Real: a 30-day Snowflake trial account (queries and INFORMATION_SCHEMA need a warehouse; SHOW does not).

**What it shares with an existing driver.**

No wire compatibility (HTTPS JSON API). SQL generation is closest to the Postgres dialect (double-quoted identifiers, ILIKE, NULLS FIRST/LAST, IS DISTINCT FROM) but string literals need backslash escaping, unquoted identifiers fold to UPPER, and the catalog is SHOW-based (INFORMATION_SCHEMA lacks KEY_COLUMN_USAGE).

**Connection fields.**

- Account identifier (orgname-account_name, or legacy locator[.region[.cloud]]); host \<account>.snowflakecomputing.com, underscores accepted as hyphens
- User
- Auth method: Key pair / Programmatic access token / OAuth token (command) / Password + MFA / External browser SSO
- Password or PAT with storage mode
- MFA passcode (asked at connect)
- Private key file + passphrase
- Warehouse
- Role
- Database
- Schema
- Custom host / PrivateLink URL (optional)
- Proxy (system proxy; requested in TablePlus #1311)
- Session parameters key=value (QUERY_TAG, TIMEZONE, ...)
- Statement timeout
- Read-only, safe mode, tag (existing)

**Effort:** L.

**Risks.**

- The session API (/session/v1/login-request, /queries/v1/query-request) is used by every official connector but is not publicly documented; the SQL API v2 is documented but stateless (no USE/BEGIN/temp tables across requests).
- snowflake-connector-rs is third-party (estie-inc) and only reached 1.x in July 2026; 1.1.1 is one day old: pin 1.1.0 or let it age, expect API churn.
- Single-factor password login is being phased out (strong-auth enforcement August-October 2026) and MFA may be asked on every connect.
- Session tokens expire (ErrorKind::SessionExpired): the driver must re-login transparently and re-apply USE/ALTER SESSION state.
- Large results are downloaded from cloud-storage URLs (S3 / Azure Blob / GCS), which corporate firewalls and proxies must allow (TablePlus #1311 proxy request).
- Queries and INFORMATION_SCHEMA need a running warehouse (auto-resume costs credits); introspection must stay on SHOW, which caps at 10,000 rows.
- Informational constraints: duplicate 'primary keys' make row edits ambiguous; DDL commits open transactions implicitly.
- Identifier case folding (unquoted names are uppercase) confuses completion and quoting (TablePlus #1112).
- The rsa 0.9 crate carries RUSTSEC-2023-0071 (Marvin timing side channel, no fixed release); cargo audit will flag it even though only local signing is done.
- Account identifier variants (org-account, locator.region.cloud, PrivateLink) need validation (TablePlus #802).
- No RETURNING / last insert id for AUTOINCREMENT columns.
- External browser SSO is experimental in the crate and needs a localhost listener (Windows firewall prompt).

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._
