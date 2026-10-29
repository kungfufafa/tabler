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

## Phase A status

Done (2026-10-28), all in the lane's files and tested without a server (`node dev/scripts/lane.mjs snowflake test --snapshot`):

- `drivers/snowflake/params.rs`: `SnowflakeParams` (`account`, `auth` = `keyPair` / `pat` / `oauth` / `password`, `privateKeyPath`, `mfa`, `warehouse`, `role`, `schema`, `baseUrl`, `sessionParameters`, `statementTimeoutSecs`; the database is the profile's). The account identifier is reduced from whatever is pasted (`https://xy12345.us-east-2.aws.snowflakecomputing.com/…`, `MYORG-ACCT`, `.snowflakecomputing.cn`) and checked (TablePlus #802); session parameter names must be bare words and their values text, numbers or booleans; `baseUrl` (PrivateLink, a proxy, the test server) is scheme and host only, over https: plain http reaches only this machine (`localhost`, `127.0.0.1`, `::1`), since the sign-in and the session token would otherwise cross the network in clear text (checked again when the client is built). An account that is not ASCII is refused, never sliced. The login always sets `MULTI_STATEMENT_COUNT = 1`, over the profile's and the account's value: Tabler splits scripts itself, so a request holds one statement, and a `;` the classifier took for part of a comment or string (see Limits) makes Snowflake refuse the request instead of running a second statement.
- `auth.rs`: the connector's `AuthConfig` per method. Key pair: the private key file (PKCS#8 PEM, `BEGIN PRIVATE KEY` or `BEGIN ENCRYPTED PRIVATE KEY` with the key passphrase secret; PKCS#1 and OpenSSH keys get a message with the `openssl pkcs8 -topk8` command), and `public_key_fingerprint`, the `SHA256:…` that `DESC USER` shows as `RSA_PUBLIC_KEY_FP` (aws-lc-rs over the SPKI DER; unencrypted keys only). PAT in place of the password, OAuth token (Command mode), password with an MFA passcode; MFA is refused unattended. Secrets never reach a `Debug` string.
- `conn.rs`: one session through snowflake-connector-rs (`custom_base_url` for `baseUrl`): sign-in with warehouse, role, database, schema and session parameters; statements with `?` binds (numbers, booleans and bytes typed, everything else text, which Snowflake casts; a partly loaded blob is refused); results read partition by partition (inline rows, then the gzipped remote chunks) into `Value` rows up to a row cap; `affected()` from the `number of rows …` columns DML answers with; `SnowflakeCancel` (the connector's abort request) implements `CancelAction` for `CancelHandle::Custom`; `ping` is `SELECT 1` (no warehouse resumes); `relogin` after an expired session.
- `decode.rs`: total decoders of the JSON result format for every type: NUMBER(p,0) as `Int` within 2^53 and `BigInt` beyond, NUMBER(p,s) and DECFLOAT as `Decimal` text, FLOAT (NaN and ±inf as `Other`), VARCHAR, BINARY (hex) as bytes, BOOLEAN, DATE (days), TIME, TIMESTAMP_NTZ / LTZ / TZ (seconds and fraction digits of the column's scale, negative epochs, the offset index and the older packed form; an instant its offset carries past chrono's range stays `Other`) as text Snowflake reads back, VARIANT / OBJECT / ARRAY / MAP / FILE as `Json` when the text is JSON, GEOGRAPHY / GEOMETRY as `Other`, VECTOR as an array. Text a type does not expect becomes `Other` with the text as sent. Column types are named as Snowflake SQL spells them (`NUMBER(38,0)`, `TIMESTAMP_TZ(3)`) with their `ValueKind`.
- `catalog.rs`: parsers of `SHOW DATABASES`, `SCHEMAS`, `TABLES` (row count and bytes from the metadata), `VIEWS` (materialized too, and the view text), `COLUMNS` (the `data_type` JSON, identity, virtual columns, VECTOR's element type and dimension), `PRIMARY KEYS`, `UNIQUE KEYS`, `IMPORTED KEYS` (foreign keys in key order), `INDEXES` (hybrid tables), `USER FUNCTIONS`, `PROCEDURES` (signatures split into arguments and return type), `SEQUENCES`, `COLUMNS IN SCHEMA` (completion), the session state of `SERVER_INFO`, and a table's row count from `SHOW TABLES LIKE`. Rows are read by column name.
- `errors.rs`: `Auth`, `Cancelled`, `NotConnected` (lost or expired session), `Timeout`, `Db` with Snowflake's code as `vendor_code`, the query id as detail and the position of a compilation error (`line L at position P`, also after words that hold `line `, such as `Pipeline`) in the statement.
- `testing.rs`: a scripted Snowflake on a loopback port reached through `custom_base_url`: sign-in checked like Snowflake does (the JWT's RS256 signature with the registered public key, `iss` = `ACCOUNT.USER.SHA256:<fp>`, `sub`, lifetime; password, PAT, passcode, OAuth), statements answered by their text with inline rows and gzipped remote partitions, errors, `333334` answers to poll, a statement that runs until an abort request naming its `requestId` arrives, an expired session. Test keys in `tests/fixtures/snowflake` (README: how they were made and the fingerprint from Snowflake's documented openssl pipeline).
- `sqlgen/dialect/snowflake.rs`: the spec (below) and the driver's statements: `SERVER_INFO`, `PING`, `ALTER SESSION SET`, `USE WAREHOUSE / ROLE / DATABASE / SCHEMA`, the `SHOW` builders with `LIMIT 10000 FROM '<last>'` paging, `GET_DDL`, `SYSTEM$CANCEL_QUERY`, the running queries of the user, `SHOW TABLES LIKE` with an escaped pattern. Knobs: `TO_BINARY('…', 'HEX')`, `TO_VARCHAR` filters (none on binary columns), ILIKE, `IS NOT DISTINCT FROM`, no `DEFAULT VALUES` or `SET … = DEFAULT`, `EXPLAIN USING TEXT`, DDL commits, binds of VARIANT / OBJECT / ARRAY through `PARSE_JSON(?)`, of GEOGRAPHY / GEOMETRY through `TO_GEOGRAPHY` / `TO_GEOMETRY` and of VECTOR through a cast rebuilt from its parts; a `build_change` hook writes an INSERT of such a value as `INSERT … SELECT` (Snowflake refuses functions in VALUES). Secrets after PASSWORD, ADMIN_PASSWORD (`CREATE ACCOUNT`, `CREATE ORGANIZATION ACCOUNT`, `CREATE MANAGED ACCOUNT`), AWS_SECRET_KEY, AWS_TOKEN, AZURE_SAS_TOKEN, MASTER_KEY, SECRET_STRING, OAUTH_CLIENT_SECRET, OAUTH_REFRESH_TOKEN, BEARER_TOKEN (catalog integrations: Iceberg REST, Delta Sharing), OIDC_CLIENT_SECRET (OIDC security integrations) and API_KEY are redacted in the SQL trace: the parameters that take a credential in the syntax of every CREATE and ALTER command (a rule matches its exact word, so PASSWORD does not cover ADMIN_PASSWORD), each masked in a unit test. A credential passed to a function by position is masked too (`redact_calls`: the token of `SYSTEM$AUTHORIZE_PRIVATELINK` and `SYSTEM$REVOKE_PRIVATELINK`, the key of `ENCRYPT`, `DECRYPT` and their `TRY_` and `_RAW` forms; F8c).
- `sqlgen/portable/gen/snowflake.rs` complete (109 reads covering every starter, 260 write forms (the holes F8c closed among them), the 31 write words (`@` among them) and 30 write calls in every shared context, 31 own active and 14 own inert contexts, fuzz atoms, `check()` and `fuzz(3000)`), fixture blessed.

### Classifier

Reads start with `SELECT`, `WITH`, `SHOW`, `DESCRIBE`, `DESC` or `EXPLAIN`, and all of them are queries for the rules: the flow operator `->>` chains any statement after any other (`SHOW TABLES ->> DELETE FROM t`): the shared classifier judges each chained statement on its own (so `… ->> GET …` and `… ->> START TRANSACTION` write) and the whole text as one, so a write word anywhere makes the text a write. Every statement word that writes is therefore in `writes`: `INSERT`, `UPDATE`, `DELETE`, `MERGE`, `INTO`, `CREATE`, `ALTER`, `DROP`, `UNDROP`, `TRUNCATE`, `COPY`, `PUT`, `REMOVE` / `RM`, `LIST` / `LS` (they read a stage), `GRANT`, `REVOKE`, `USE` (it changes the role, warehouse or database), `SET`, `UNSET`, `BEGIN`, `COMMIT`, `ROLLBACK`, `COMMENT`, `CALL`, `EXECUTE` (IMMEDIATE, TASK, ALERT, NOTEBOOK, SERVICE, DBT PROJECT), `DECLARE`, `LET`, and `NEXTVAL` (it advances a sequence). `@` is in `writes` too: in a query it only ever names a stage, and every use of one reads staged files (where a table goes, as an argument, in `EXECUTE IMMEDIATE FROM`), so a stage, or a comment or quote opener inside an unquoted stage path (`@s//x ->> DELETE FROM t`), makes the text a write. Unknown statements are writes.

The sweep of the function reference (the complete function index, the system functions, the table functions, the commands index, and the namespaces outside them: `SNOWFLAKE.CORTEX`, `SNOWFLAKE.CORE`, `SNOWFLAKE.LOCAL`, `SNOWFLAKE.ML`, `SNOWFLAKE.NOTIFICATION`, `SNOWFLAKE.DATA_PRIVACY`, Snowpark Container Services methods) puts these calls in `write_calls`:

| Calls                                                                                                                                                                                                           | Why                                                                                              |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `SYSTEM$*` (all 244: cancels and aborts, waits, settings, privatelink, replication, logging, tags, task and pipe controls, also the information ones)                                                           | the brief's rule: one name family, many change the account                                       |
| `IDENTIFIER`                                                                                                                                                                                                    | `SELECT IDENTIFIER('SYSTEM$…')()` calls a function named by a string, variable or bind           |
| `TO_QUERY`                                                                                                                                                                                                      | runs the SQL text it is given                                                                    |
| `BUILD_SCOPED_FILE_URL`, `BUILD_STAGE_FILE_URL`, `GET_PRESIGNED_URL`, `GET_ABSOLUTE_PATH`, `GET_RELATIVE_PATH`, `GET_STAGE_LOCATION`, `TO_FILE`, `TRY_TO_FILE`, `FL_*`, `INFER_SCHEMA`, `VALIDATE`, `DIRECTORY` | read staged files or their metadata, or hand out URLs that grant access to them                  |
| `GET_PYTHON_PROFILER_OUTPUT` (`SNOWFLAKE.CORE`, needs ACCOUNTADMIN)                                                                                                                                             | returns the report file the Python profiler wrote to the stage of `PYTHON_PROFILER_TARGET_STAGE` |
| `AI_*`, `COMPLETE`, `TRY_COMPLETE`, `PARSE_DOCUMENT`                                                                                                                                                            | Cortex calls that take staged files (images, documents, audio)                                   |
| `FINETUNE`, `AGENT_RUN`, `DATA_AGENT_RUN`, `EXECUTE_*` (`EXECUTE_AI_EVALUATION`)                                                                                                                                | create a model, run an agent whose tools run SQL or procedures, start an evaluation run          |
| `SET_SYS_CONTEXT`                                                                                                                                                                                               | sets a session context value                                                                     |
| `GENERATE_POSTGRES_ACCESS_TOKEN_FOR_USER`                                                                                                                                                                       | mints a credential                                                                               |
| `SPCS_CANCEL_JOB`                                                                                                                                                                                               | `svc!SPCS_CANCEL_JOB()` cancels a container job                                                  |
| `ASSOCIATE_SEMANTIC_CATEGORY_TAGS`                                                                                                                                                                              | sets tags                                                                                        |
| `GET_ANACONDA_PACKAGES_REPODATA`, `SHOW_PYTHON_PACKAGES_DEPENDENCIES`                                                                                                                                           | resolve against the Anaconda channel                                                             |

Everything else in the reference only reads: the scalar, aggregate, window, semi-structured, geospatial, H3, vector, hash, encryption, context (`CURRENT_*`, `SYS_CONTEXT`, `GETVARIABLE`, `IS_ROLE_IN_SESSION`…), data metric (`SNOWFLAKE.CORE.*`), notification message builders (`TEXT_PLAIN`, `APPLICATION_JSON`, `EMAIL_INTEGRATION_CONFIG`, `INTEGRATION`), `GET_DDL`, `EXPLAIN_JSON`, `EXTRACT_SEMANTIC_CATEGORIES`, `SNOWFLAKE.CORTEX.THREAD_MESSAGES` (a scalar that fetches an agent thread's messages), and the Cortex text functions (`SENTIMENT`, `SUMMARIZE`, `TRANSLATE`, `EXTRACT_ANSWER`, `CLASSIFY_TEXT`, `EMBED_TEXT_*`, `ENTITY_SENTIMENT`, `SPLIT_TEXT_*`, `COUNT_TOKENS`, `SEARCH_PREVIEW`). Of these, `SENTIMENT`, `SUMMARIZE`, `EXTRACT_ANSWER`, `CLASSIFY_TEXT`, `EMBED_TEXT_*`, `ENTITY_SENTIMENT`, `SEARCH_PREVIEW` and `SPLIT_TEXT_*` are `side_effect_functions` (with `SPCS_WAIT_FOR`, which waits), which the Metrics Board refuses: most bill tokens per call, and the `SPLIT_TEXT_*` helpers stay listed although their reference names no token cost. `TRANSLATE` bills tokens too and is listed by its schema (`CORTEX.TRANSLATE`), so the plain string function `TRANSLATE(s, 'abc', 'xyz')` stays a read. `COUNT_TOKENS` costs warehouse compute only, like `AI_COUNT_TOKENS` (Cortex cost page), so it is a plain read. Stored procedures, the class methods that change state (`budget!SET_SPENDING_LIMIT`), `DATA_PRIVACY.CLASSIFY`, the clean-room procedures and the information procedures `EXPLAIN_PRIVILEGES` and `EXPLAIN_GRANTABLE_PRIVILEGES` run only through `CALL`.

Where a table goes, only the table functions of `table_functions` read: `TABLE(…)` and `VALUES` themselves, `SEMANTIC_VIEW`, `FLATTEN`, `GENERATOR`, `SPLIT_TO_TABLE`, `STRTOK_SPLIT_TO_TABLE`, `RESULT_SCAN`, `BIND_VALUES`, `GET_QUERY_OPERATOR_STATS`, `GET_OBJECT_REFERENCES`, `GET_LINEAGE`, the differential-privacy, Cortex Search (`CORTEX_SEARCH_DATA_SCAN`, `CORTEX_SEARCH_BATCH`, `CORTEX_SEARCH_REFRESH_HISTORY`), model-monitor (`MODEL_MONITOR_DRIFT_METRIC`, `_PERFORMANCE_METRIC`, `_STAT_METRIC`), container-service log and `SNOWFLAKE.LOCAL` AI-observability functions (`GET_AI_EVALUATION_DATA`, `GET_AI_OBSERVABILITY_EVENTS`, `GET_AI_OBSERVABILITY_LOGS`, `GET_AI_RECORD_TRACE`), the INFORMATION_SCHEMA history functions (among them `QUERY_HISTORY` and its `_BY_SESSION`, `_BY_USER` and `_BY_WAREHOUSE` forms, `TAG_REFERENCES`, `_ALL_COLUMNS` and `_WITH_LINEAGE`, and the eight `REPLICATION_GROUP_` functions: `DANGLING_REFERENCES`, `LAG_HISTORY`, `REFRESH_HISTORY[_ALL]`, `REFRESH_PROGRESS[_ALL|_BY_JOB]`, `USAGE_HISTORY`), and PIVOT's aggregates. The list holds exact names, swept from the table-function index, never a `PREFIX*`: Snowflake resolves a name no built-in has to the user's procedure or table function in the current schema, so a pattern let `TABLE(QUERY_HISTORY_PURGE())` run one as a read (a unit test keeps patterns out). Anything else there writes: a stored procedure can be selected from (`SELECT * FROM TABLE(p())`), a user table function may reach other services, `INFER_SCHEMA`, `VALIDATE` and `DIRECTORY` read staged files, and an unlisted one (`VALIDATE_PIPE_LOAD`, `SNOWFLAKE.LOCAL.GET_PYTHON_UDF_PROFILER_OUTPUT`, the scalar `THREAD_MESSAGES` in `TABLE(…)`) is a write by default. The check takes the call's last name part past every run of dots (`TABLE(db..p())`, PUBLIC's `p`) and a method's `!` (`TABLE(m!PREDICT(…))` is checked by `PREDICT`). Entries carry their namespace (shared classifier, F8c): the 71 INFORMATION_SCHEMA functions are listed as `INFORMATION_SCHEMA.X` (with or without a database before it), the 12 of the `SNOWFLAKE` database by their path (`SNOWFLAKE.CORE.GET_LINEAGE`), the three service methods as `!SPCS_GET_LOGS` (a method only), and a bare entry (a function without a namespace, a keyword, an aggregate) allows an unqualified call only, since a qualified one names a schema's object, the user's (`TABLE(s.FLATTEN(…))` writes). So `TABLE(QUERY_HISTORY())` and `TABLE(myschema.QUERY_HISTORY())` write: a session that ran `USE SCHEMA INFORMATION_SCHEMA` gets a confirmation for an unqualified call, as Snowflake's own examples qualify them. A quoted name matches only on its exact text (`TABLE("flatten"(1))` writes: Snowflake resolves it to the user's object), and the keywords `TABLE`, `VALUES` and `SEMANTIC_VIEW` never quoted. `LIMIT` and `EXCEPT` end the `FROM` list only before their own syntax (a number, bind or variable; a query), so as a table's alias (`FROM t1 limit (a, b), my_udtf(t1.a)`) they leave it open.

Hard cases kept as cases: untagged `$$` bodies (`$a$` is the variable `a$`, never a tag; a `$$` body takes no backslash escapes, so `$$a\$$` ends there), `SYSTEM$` names read as one word, also quoted, qualified, with a space, comment or line break before the `(`, glued to a number; `EXECUTE IMMEDIATE` with a string, a `$$` body, a variable or a staged file; `CALL` (also `WITH … AS PROCEDURE … CALL`); `SELECT … INTO :v`; `FOR UPDATE`; the flow operator after `SHOW`, `DESCRIBE`, `EXPLAIN` and `SELECT`; `EXPLAIN` of a write (a write: it is judged like a query).

Left for Phase B: `entry.rs` (`open`, `validate`, `util`: the key fingerprint probe and the warehouse / role pickers), the `DbSession` (sessions per pool and editor, session state re-applied after a re-login, switching database by reconnecting, cancel through `CancelHandle::Custom(SnowflakeCancel)`, the app's query time-out cancelling on the server, `transaction()` with `expect_affected` checked from the DML counts before COMMIT, the trace), the secrets (`KeyPassphrase`; a PAT longer than Windows Credential Manager's 1,280 characters stored through `set_secret` or refused), caps (ClassifierOnly read-only, Costly counts, no keep-alive, `CancelOnServer`), the renderer and the mock, the bindings (`src/lib/ipc/bindings/lex/snowflake.json` still holds the foundation's first rules: `lane.mjs snowflake bindings` rewrites it once the working tree builds) and the real-server suite. To verify on a real account: the result format the session API sends this client (JSON is the only one the connector reads), the `data_type` JSON keys of a VECTOR column in `SHOW COLUMNS`, `SHOW … LIMIT … FROM` paging, that `PARSE_JSON(?)` binds compare in WHERE, the codes of a rejected JWT and of MFA, whether Snowflake takes a quoted `seq."NEXTVAL"` (the rules match only the bare word, so it would advance the sequence as a read: then a change request for quoted names in `writes`), and that the login's `MULTI_STATEMENT_COUNT = 1` wins over an account default of 0 (a second statement after a `;` must be refused whole).

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._

- Sign-in: no external-browser SSO and no MFA token cache (MFA may be asked on every connect); password with MFA cannot be used unattended (MCP). The fingerprint is shown only for an unencrypted key.
- No PUT / GET (the connector does not transfer files), one database per session (switching reconnects), no DDL editing in v1.
- TIMESTAMP_LTZ values show in UTC (`+00:00`): the session's time zone is not in the result. GEOGRAPHY and GEOMETRY show as text and are edited as GeoJSON / WKT.
- Constraints are informational: duplicate "primary keys" make a row edit ambiguous, so the edit checks the count and rolls back (Phase B).
- SQL trace: the literal after a secret parameter is masked (Phase A status), and so are the string literals of a credential passed by position (`redact_calls`, F8c): the token of `SYSTEM$AUTHORIZE_PRIVATELINK('<id>', '<token>')` and `SYSTEM$REVOKE_PRIVATELINK(…)`, and the passphrase or key of `ENCRYPT`, `DECRYPT`, `TRY_DECRYPT`, `ENCRYPT_RAW`, `DECRYPT_RAW` and `TRY_DECRYPT_RAW` (every literal of that argument, also inside a call such as `TO_BINARY('…', 'HEX')`). A credential held in a variable or column stays as written.
- Safe Mode and read-only (ClassifierOnly, default-deny):
  - A user's scalar function that reaches another service (an external function, a UDF with an external access integration, a service function, a model's method in the select list such as `SELECT m!PREDICT(…)`) reads as a read: its name says nothing.
  - A quoted `seq."NEXTVAL"` reads as a read until a real account shows whether Snowflake takes it (`documented_holes_read_until_their_change_requests` in `sqlgen/dialect/snowflake.rs`). The stage, stage-path, format-character, flow-operator, double-dot, method, schema, quoted-name and `LIMIT` / `EXCEPT` alias holes of Phase A were closed in the shared classifier (F8c) and are writes of the suite. Their price, false writes: any `@` in code (a stage, also `GET_PRESIGNED_URL(@s, …)`, which writes anyway), a format character such as a zero-width space or a byte-order mark inside the text (one that starts the text is a space), an unqualified call of an INFORMATION_SCHEMA or `SNOWFLAKE` table function, a quoted allowed function in another case than its name (`TABLE("flatten"(…))`), and `LIMIT` or `EXCEPT` followed by anything but their own syntax right after a table.
  - Words that are also functions or common names count as writes where they appear as code: `TRUNCATE(n, 2)` and `INSERT(s, …)` (use `TRUNC`, or quote), a column named `COMMENT` unquoted (`information_schema.tables.comment`: write `"COMMENT"`), `IDENTIFIER(…)` anywhere, user table functions, and PIVOT over an aggregate outside the list.
  - `LIST` / `LS`, `USE`, `BEGIN` / `COMMIT` / `ROLLBACK` and `EXPLAIN` of a write ask for confirmation (and are refused read-only).
