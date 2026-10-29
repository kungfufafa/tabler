# BigQuery: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                                                             |
| ----------------- | ------------------------------------------------------------------------------------------- |
| Lane              | `bigquery`                                                                                  |
| Databases         | BigQuery                                                                                    |
| Drivers           | `bigquery`                                                                                  |
| Picker kinds      | `bigquery`                                                                                  |
| SQL dialect       | `bigquery`                                                                                  |
| Cargo features    | `drv-bigquery`                                                                              |
| Home slot         | `s2`                                                                                        |
| Bindings prefix   | `Bigquery*`                                                                                 |
| Lib test filters  | `drivers::bigquery`, `sqlgen::dialect::bigquery`, `sqlgen::portable::generated::bigquery::` |
| Real-server suite | `tests/bigquery.rs` with `TABLER_TEST_BIGQUERY_URL`                                         |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/bigquery/**`
- `src-tauri/src/sqlgen/dialect/bigquery.rs`
- `src-tauri/src/sqlgen/portable/gen/bigquery.rs`
- `src-tauri/tests/bigquery.rs`
- `src-tauri/tests/fixtures/bigquery/**`
- `src-tauri/tests/fixtures/classify-generated/bigquery.json`
- `src/features/drivers/bigquery/**`
- `src/lib/ipc/mock/drivers/bigquery/**`
- `src/lib/ipc/bindings/Bigquery*.ts`
- `src/lib/ipc/bindings/lex/bigquery.json`
- `docs/drivers/bigquery.md`

Anything else (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead. `node dev/scripts/lane-commit.mjs bigquery -m "bigquery: …"` refuses other paths.

## Scope of v1

### BigQuery

- **Identity:** `bigquery`, `SqlDialect::Bigquery`. Crates: none (REST plus `common-jwt`).
- **UI:** SQL grid; browse through free `tabledata.list` (cursor).
- **Auth v1:** service-account `keyFilePath` (JWT, `bigquery.readonly` scope on read-only profiles), ADC authorized_user file, access token via Command.
- **Required fields:** database (Project) plus one credential.
- **Out of scope v1:** impersonation, Drive scope, Storage Read API, row edits (EditRows off), extra projects beyond listing them as databases.
- **Test:** canned `jobs.query` / `getQueryResults` JSON; JWT test. Emulator Docker only; sandbox opt-in.

## Descriptor values

Seeded from the plan (§5.2). The values in force are the descriptors in `src-tauri/src/types/driver.rs` (keepalive belongs to the session's caps); a change is a request to the lead.

| Driver   | Location | Port (TLS) | SSH  | TLS style | Secret label | Command TTL |
| -------- | -------- | ---------- | ---- | --------- | ------------ | ----------- |
| bigquery | Cloud    | —          | None | Https     | AccessToken  | 3000        |

| Driver   | Required fields / params | Label overrides   | Keepalive | Read-only                                                 | Sharing | Token secret |
| -------- | ------------------------ | ----------------- | --------- | --------------------------------------------------------- | ------- | ------------ |
| bigquery | database                 | database: Project | None      | ClassifierOnly (Server once the readonly scope is proven) | Indep   | yes          |

## Read-only and hardening

| Engine   | When read-only                                                                                                                                                                                                                                | When unattended (MCP) |
| -------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------- |
| BigQuery | ClassifierOnly until a recorded test shows `DELETE` failing under the `bigquery.readonly` scope (critique D6); then a key-file JWT or ADC minted with that scope is Server, a Command token stays ClassifierOnly. `maximumBytesBilled` always | same                  |

Every classifier is default-deny: a statement or command it does not know is a write. The read-only mode in the caps (`Server`, `PerRequest` or `ClassifierOnly`) must say what the session really enforces.

## What to implement

- `entry.rs` (Phase B): `open`, `validate`, `util`, `util_kind` (plan §2.5). `<Prefix>Params` (`#[serde(rename_all = "camelCase", default)]`, ts-exported) parsed from `options.driver_params`.
- `DbSession`: the 18 required methods; overrides of `query_cancellable`, `execute_cancellable`, `drain_trace`, `sql_trace` and `editor_run_lock`; `dialect()`, `capabilities()` (accurate features, data, safety and lifecycle caps) and `flavor()` where it applies.
- Optional: `table_source()`, `call` / `call_kind`, `list_processes` / `kill_processes`, `table_ddl`, `explain_form`.
- Cancel through `CancelHandle::Custom(Arc<dyn CancelAction>)`; `cancel()` calls `cancel_portable` first.
- Renderer: a `KindUi` per tile, `DriverUi`, `DialectUi` and a `MockDriver`. Write-capable `sessionCall` / `driverUtil` helpers await `guardExecution` first.

## Decisions and corrections for this lane

- REST through `drivers/common/http` and `common/jwt` (`common-jwt`).
- Read-only counts as ClassifierOnly until a recorded test shows `DELETE` failing under the `bigquery.readonly` scope; only then may the caps report Server (critique D6). `EXECUTE IMMEDIATE` and `CALL` are writes (critique D4).
- Costs: browse through the free `tabledata.list` (cursor), `count` Costly, `maximumBytesBilled` 1 GiB by default (editable; empty means unlimited, with a warning), no keep-alive and a free `ping` (plan §1.5 Q8). Row edits are off in v1.
- The service-account key is a file path in params (`keyFilePath`), never stored (plan A18).

## Definition of done (this lane)

Beyond the shared list in [README.md](README.md#definition-of-done-v1):

- **Classifier hardening (blocks landing).** `src-tauri/src/sqlgen/portable/gen/bigquery.rs` is complete and `src-tauri/tests/fixtures/classify-generated/bigquery.json` blessed, as [Classifier hardening](README.md#classifier-hardening) requires: every write keyword, write call and read starter of `dialect::bigquery::SPEC` in every shared context and in the contexts only this grammar has, an engine-realistic read corpus, its write forms and fuzz atoms, `suite().check()` and `suite().fuzz(…)` in place of the placeholder. Then a red-team review passes, each finding fixed and kept as a case. Known hard cases: `EXECUTE IMMEDIATE` with the statement in a string; `CALL`; scripting (`DECLARE`, `SET`, `BEGIN … END`, `IF`, `LOOP`, `WHILE`, `FOR … IN`, `BEGIN TRANSACTION`) as writes; `EXTERNAL_QUERY`; `EXPORT DATA` and `LOAD DATA`; its string and name forms (triple-quoted, raw and bytes strings, backtick names with dots and dashes) as inert contexts.

## Research notes

> Background from the driver research (October 2026), with file references and no line numbers. Verify against the code; where it differs from the scope above or from README.md, those win.

### Google BigQuery

**Approach.**

New Driver::Bigquery + Dialect::Bigquery with a hand-rolled REST v2 client on the app's reqwest 0.13 (Cargo.toml). Endpoints (base https://bigquery.googleapis.com/bigquery/v2): jobs.query POST /projects/{p}/queries (QueryRequest: query, useLegacySql:false, parameterMode:'NAMED', queryParameters, defaultDataset{projectId,datasetId}, location, maxResults, timeoutMs, dryRun, useQueryCache, maximumBytesBilled, createSession, connectionProperties[{key:'session_id', value}], labels, jobCreationMode, requestId, jobTimeoutMs, formatOptions{useInt64Timestamp:true}; response: schema, jobReference, queryId, totalRows, pageToken, rows, jobComplete, errors, numDmlAffectedRows, dmlStats, sessionInfo, totalBytesProcessed, totalBytesBilled, cacheHit); jobs.getQueryResults GET /projects/{p}/queries/{jobId}?location=&pageToken=&startIndex=&maxResults=&timeoutMs= (poll until jobComplete: the default wait is 10 s and it returns after about 200 s even with a longer timeoutMs - exactly where TablePlus fails: #2935, #3058, #3356); jobs.cancel POST /projects/{p}/jobs/{jobId}/cancel?location= (CancelHandle::Bigquery{job_id, location}); jobs.get / jobs.list?parentJobId= for script child jobs and statistics; datasets.list GET /projects/{p}/datasets; tables.list GET /projects/{p}/datasets/{d}/tables; tables.get GET .../tables/{t}; tabledata.list GET .../tables/{t}/data (free browsing); routines.list. Rows arrive as {f:[{v:...}]} (RECORD -> {v:{f:[...]}}, REPEATED -> {v:[{v:...}]}), every scalar a string. Credentials: google-cloud-auth 1.18 (official googleapis/google-cloud-rust; reqwest ^0.13.4 with rustls-no-provider and rustls aws_lc_rs, no separate hyper stack) covering service-account JSON, Application Default Credentials (GOOGLE_APPLICATION_CREDENTIALS, gcloud authorized_user file, impersonation, external account / workload identity, metadata server) with scope https://www.googleapis.com/auth/bigquery; or hand-roll about 150 lines: service-account JWT RS256 signed with aws-lc-rs (RsaKeyPair::from_pkcs8 + RSA_PKCS1_SHA256, Cargo.toml; PEM via rustls-pki-types) exchanged at token_uri with grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer, plus the refresh_token grant for authorized_user ADC files. Rejected: gcp-bigquery-client 0.28 (yup-oauth2, whose 12.1.2 default is ring; typed models), gcloud-bigquery 1.7 (yoshidan; gRPC Storage API over tonic), google-cloud-bigquery-v2 1.1.0 (official generated client first released 2026-08/09; pulls google-cloud-gax-internal, google-cloud-lro, serde_with; heavier than five endpoints need), gcp_auth 0.12.7 (own hyper-rustls client).

**Windows build.**

The hand-rolled REST client needs no new crates (reqwest, serde_json, chrono, base64 0.23, aws-lc-rs and rustls-pki-types are already present). google-cloud-auth 1.18 adds google-cloud-gax, jiff, hmac 0.13, hex, and uses time 0.3 / sha2 0.11 / rustls 0.23 already in the lock: pure Rust, no OpenSSL, no extra C beyond aws-lc-sys (already built for rustls). Use default-features = false and add default-rustls-provider only if no provider is installed (it enables reqwest/default-tls + rustls/aws_lc_rs, matching the app); skip idtoken / jsonwebtoken. jsonwebtoken 11.1 (feature aws_lc_rs) would also work as signer but is unnecessary.

**Authentication.**

TablePlus: service-account JSON key file only; Application Default Credentials requested in #2921 (many organisations forbid service-account keys) and the Drive scope in #3740. Tabler: (1) Service-account key file (JSON with client_email, private_key in PKCS#8 PEM, private_key_id, token_uri, project_id), path stored in the profile and read at connect; (2) Application Default Credentials (`gcloud auth application-default login` -> %APPDATA%\gcloud\application_default_credentials.json on Windows, ~/.config/gcloud/application_default_credentials.json on macOS; GOOGLE_APPLICATION_CREDENTIALS; workload identity federation files); (3) Service-account impersonation (target principal) on top of ADC; (4) Access token from the existing Command password mode (`gcloud auth print-access-token`). Scopes: bigquery (or bigquery.readonly for read-only profiles) plus optional drive.readonly for Drive-backed external tables. Tokens last about an hour: refresh before expiry, keep them in memory only. Users need BigQuery Job User (to run jobs) plus Data Viewer / Data Editor.

**Data model.**

Project -> datasets -> tables (TABLE, VIEW, MATERIALIZED_VIEW, EXTERNAL, SNAPSHOT), routines, models. Map project = database, dataset = schema (TableRef.schema = dataset, qualified as `project`.`dataset`.`table`), which is what TablePlus users ask for in #3520 (TablePlus shows every dataset as a separate database, making cross-dataset queries awkward). Columns can be nested: RECORD/STRUCT with fields[] and mode REPEATED arrays; show them as Json/Array cells and use dotted field paths in completion. No indexes (only search/vector indexes); partitioning (timePartitioning / rangePartitioning) and clustering fields stand in for the index list; PRIMARY KEY / FOREIGN KEY come from tables.get tableConstraints and are NOT ENFORCED. Value mapping: INT64 -> Value::from_i64 or BigInt (strings, avoiding TablePlus #3103 exponent notation), FLOAT64 -> Float, NUMERIC / BIGNUMERIC -> Decimal, BOOL -> Bool, STRING -> Text, BYTES (base64) -> Bytes, DATE -> Date, DATETIME -> Timestamp, TIME -> Time, TIMESTAMP -> TimestampTz formatted from int64 microseconds (formatOptions.useInt64Timestamp; the default float-seconds form is the cause of TablePlus #2863; a newer timestampOutputFormat ISO8601_STRING option also exists), JSON -> Json, GEOGRAPHY (WKT) -> Geometry, INTERVAL -> Interval, RANGE -> Other, RECORD / REPEATED -> Json / Array.

**Query language.**

GoogleSQL (useLegacySql=false always). Identifiers in backticks with backtick and backslash escaped by a backslash; dashed project ids require them (`my-proj`.dataset.table or `my-proj.dataset.table`). String literals '...' or "..." with backslash escapes and no raw newlines (escape \n or use '''...'''), raw r'...', bytes b'...'; comments --, #, /\* \*/. Named parameters @p1..@pn (parameterMode NAMED, typed queryParameters STRING / INT64 / FLOAT64 / NUMERIC / BIGNUMERIC / BOOL / BYTES / DATE / DATETIME / TIME / TIMESTAMP / JSON / ARRAY / STRUCT) via Dialect::placeholder; limits 10,000 parameters, 1,024K-character query, 10 MB request, 10 GB compressed response. LIMIT/OFFSET is valid but each page re-scans and bills the selected columns; NULLS FIRST/LAST and IS [NOT] DISTINCT FROM exist; no ILIKE (use LOWER(x) LIKE or a case-insensitive collation, check_dialect select.rs). Scripts (DECLARE / SET / IF / LOOP / BEGIN ... END, BEGIN TRANSACTION ... COMMIT TRANSACTION) run natively in one jobs.query, but jobs.query returns only the last statement's result: either split client-side and run each statement in one session (createSession + session_id keeps temp tables, variables and open transactions) or list the child jobs. Splitter (split.rs) needs a BigQuery mode: backticks, triple-quoted and raw strings, # comments, no splitting inside BEGIN ... END. Explain: dryRun=true returns totalBytesProcessed (shown as a cost estimate) and the schema; the real plan is statistics.query.queryPlan from jobs.get after the run.

**Editing.**

Possible with caveats. DML through jobs.query: INSERT ... VALUES with @params, UPDATE (WHERE is mandatory), DELETE, MERGE; each DML statement scans and bills the referenced columns. Up to 2 UPDATE/DELETE/MERGE run concurrently per table and 20 more queue (beyond that they fail); INSERTs run immediately for the first 1,500 per 24 h, then 10 concurrent / 100 queued per table; rows written through the Storage Write API REST path in the last 30 minutes cannot be updated or deleted; the free BigQuery sandbox forbids DML. No LIMIT on UPDATE/DELETE and no RETURNING, so commit staged changes as one multi-statement transaction script (BEGIN TRANSACTION; UPDATE ...; IF @@row_count != 1 THEN ROLLBACK TRANSACTION; RAISE USING MESSAGE = '...'; END IF; ...; COMMIT TRANSACTION;) to keep expect_affected atomic (drivers/mod.rs); a transaction may mutate at most 100 tables and concurrent transactions on the same table fail at once instead of queueing. Row identity: declared PRIMARY KEY (NOT ENFORCED), else all columns - ARRAY / STRUCT / JSON / GEOGRAPHY cannot be compared with = (compare TO_JSON_STRING(col) or use ST_EQUALS) and FLOAT64 equality is risky; nested cells read-only in the grid. DDL: CREATE TABLE ... PARTITION BY ... CLUSTER BY ... OPTIONS(description = ...), ALTER TABLE ADD COLUMN / DROP COLUMN / RENAME COLUMN / ALTER COLUMN SET DATA TYPE (widening only) / SET DEFAULT / SET OPTIONS, RENAME TO, CREATE OR REPLACE, TRUNCATE TABLE, DROP; table metadata updates are limited to 5 per 10 s per table (batch ALTERs). No index editing.

**Introspection.**

Use the free REST metadata APIs, not INFORMATION_SCHEMA: on-demand INFORMATION_SCHEMA queries bill at least 10 MB each, are never cached, and must be qualified by dataset (`proj.ds.INFORMATION_SCHEMA.COLUMNS`) or region (`region-us.INFORMATION_SCHEMA.SCHEMATA`) and run in that location. Databases: the profile's project plus configured extra projects (optionally projects.list GET /bigquery/v2/projects). Schemas: datasets.list GET /bigquery/v2/projects/{p}/datasets?maxResults=1000&pageToken= (datasetReference.datasetId, location, labels). Tables: tables.list GET /bigquery/v2/projects/{p}/datasets/{d}/tables?maxResults=1000&pageToken= (tableReference, type, timePartitioning, clustering; no row counts). Table meta: tables.get GET .../datasets/{d}/tables/{t} -> schema.fields[] (name, type, mode NULLABLE/REQUIRED/REPEATED, fields[] for RECORD, description, maxLength, precision, scale, defaultValueExpression, collation, roundingMode), numRows, numBytes, description, view.query / materializedView.query, timePartitioning {type, field, expirationMs}, rangePartitioning, clustering.fields, requirePartitionFilter, tableConstraints {primaryKey.columns, foreignKeys[{name, referencedTable{projectId, datasetId, tableId}, columnReferences[{referencingColumn, referencedColumn}]}]}. Routines: routines.list GET .../datasets/{d}/routines. CREATE text only on demand (billed): SELECT ddl FROM `p.d`.INFORMATION_SCHEMA.TABLES WHERE table_name = @t. Completion: tables.get per table lazily, or one dataset-qualified INFORMATION_SCHEMA.COLUMNS query per dataset cached client-side (billed). Free table browse: tabledata.list GET .../tables/{t}/data?startIndex=&maxResults=&selectedFields=&formatOptions.useInt64Timestamp=true (native tables only); for sorted/filtered pages run the query once and page with getQueryResults startIndex on the same job (results cached, no re-billing). Process list: jobs.list?stateFilter=running; kill = jobs.cancel.

**Testing without a server.**

Offline: decoder tests on canned jobs.query / getQueryResults JSON (nested RECORD/REPEATED, NULLs, int64 timestamps, jobComplete=false then true, pageToken paging), JWT claim/signature tests with a generated RSA test key, ADC file parsing, sqlgen tests (backtick quoting, @params, the transaction script). Emulator: goccy/bigquery-emulator (`docker run -p 9050:9050 -p 9060:9060 ghcr.io/goccy/bigquery-emulator:latest --project=test`; REST on 9050, no auth, beta, no external tables / snapshots / IAM) via a base-URL override. Real: the BigQuery sandbox (free, but no DML and tables expire after 60 days) and bigquery-public-data for read-only checks.

**What it shares with an existing driver.**

No (HTTPS REST/JSON, no SQL wire protocol). Some generated SQL matches the Postgres dialect (IS DISTINCT FROM, NULLS FIRST/LAST) but quoting (backticks with backslash escapes) and literals (backslash escapes like MySQL, no raw newlines) differ, and there is no ILIKE, no indexes and no ctid/rowid.

**Connection fields.**

- Billing / default project ID
- Credentials: Service account key file / Application Default Credentials / Impersonate service account / Access token command
- Key file path (JSON)
- Impersonated service account email (optional)
- Location (US, EU or a region; required for jobs outside the US/EU multi-regions)
- Default dataset (optional)
- Additional projects to list (optional, e.g. bigquery-public-data)
- Maximum bytes billed per query (cost guard)
- Use query cache (default on)
- Job timeout / query timeout
- Job labels (optional)
- Include Google Drive scope
- Read-only, safe mode, tag (existing)

**Effort:** L.

**Risks.**

- Cost: every query, DML and INFORMATION_SCHEMA read is billed by bytes; LIMIT/OFFSET paging re-scans, so the grid must use tabledata.list or getQueryResults(startIndex) on one job, a maximumBytesBilled guard and a dry-run estimate; metrics boards and autocomplete must never trigger billed queries silently.
- Long jobs: getQueryResults must be polled until jobComplete with no fixed client timeout (TablePlus #2935, #3058, #3356).
- Jobs outside the US/EU multi-regions need the location on getQueryResults / cancel; INFORMATION_SCHEMA region qualifiers are not hierarchical.
- DML limits: 2 concurrent mutating statements per table + 20 queued, the 30-minute streaming-buffer restriction, no DML in the sandbox, concurrent transactions on one table fail.
- No row identity and no equality on ARRAY/STRUCT/JSON/GEOGRAPHY: editing tables without a declared key is fragile.
- jobs.query returns only the last statement of a script; per-statement results need sessions or child jobs.
- Quotas: 5 table metadata updates per 10 s per table, API rate limits on tables.get for wide autocomplete loads.
- Organisations often forbid service-account keys (#2921): ADC / impersonation must work; Drive-backed external tables need the drive scope (#3740).
- Timestamp and big-number formatting must stay exact (TablePlus #2863, #3103).
- Sessions expire after inactivity and must be recreated (temp tables and variables are lost).

## Phase A status

Done (2026-10-29), all in the lane's files and tested without a server:

- `drivers/bigquery/transport.rs`: the HTTP seam (requests carry their own headers, form and JSON bodies, percent-encoded path segments and query values, secrets kept out of `Debug`), for F8a's shared client to replace.
- `jwt.rs`: RS256 assertions signed with aws-lc-rs from the key file's PKCS#8 (or PKCS#1) PEM, `kid` from `private_key_id`, unexpanded `\n` escapes read too.
- `auth.rs`: credential files parsed (`service_account`, `authorized_user`; workload identity, impersonation and GDCH files refused with a clear message; errors never quote a value of the file), read from disk with a size cap, Application Default Credentials found (`GOOGLE_APPLICATION_CREDENTIALS`, `CLOUDSDK_CONFIG`, `%APPDATA%\gcloud`, `~/.config/gcloud`), tokens from the JWT-bearer grant (the `bigquery.readonly` scope for read-only profiles) or the refresh-token grant, cached in memory and replaced 5 minutes before they expire or after a 401, a fixed token for Command mode; a token endpoint must be HTTPS (plain HTTP only to this machine: `localhost` or a loopback address, the URL's host parsed and compared whole, so `http://localhost.example.com` is refused), and a reply that may hold a token (a success, a JSON body) is never quoted in an error.
- `client.rs`: every call with a bearer token (one refresh and retry on a 401), `x-goog-user-project` for user credentials, listings that follow page tokens (at most 100 pages; a repeated token ends them), query jobs inserted with an id Tabler picks (`tabler_<uuid>`, so a cancel can name the job at once) and polled with `jobs.getQueryResults` (10 s server waits, no client limit on how long a job runs) until they finish, result pages by token, `jobs.query` for dry runs and small reads, `jobs.cancel` (a job that is gone is no error), `jobs.list` of running and pending jobs, `tabledata.list` (free browsing: `startIndex` or page token, `maxResults`, `selectedFields`, int64 timestamps), `tables.get`, `routines.list` / `get`, `datasets.list`, `projects.list`. `JobCancel` is the `CancelAction` of `CancelHandle::Custom`: it ends the local wait and cancels the job on the server.
- `jobs.rs`: the bodies of `jobs.insert` and `jobs.query` (GoogleSQL, named parameters, `maximumBytesBilled` as an int64 string, default dataset, location, labels, `jobTimeoutMs`, sessions through `createSession` / `connectionProperties.session_id`, dry runs, `useInt64Timestamp`) and the replies (int64 strings read leniently, DML counts from `numDmlAffectedRows` or `dmlStats`, session ids, job state and errors).
- `params.rs`: `BigqueryParams` (ts-exported: `auth`, `keyFilePath`, `location`, `defaultDataset`, `extraProjects`, `maximumBytesBilled` defaulting to 1 GiB with `null` for none, `useQueryCache`, `jobTimeoutMs`, `labels`, `apiEndpoint` for an emulator, HTTPS or plain HTTP to this machine only, by the token endpoint's rule, which the client also applies before it sends a bearer) with its checks, and Tabler values as typed query parameters (`@p1…`: INT64 or BIGNUMERIC, NUMERIC or BIGNUMERIC by size, DATETIME for `Timestamp`, TIMESTAMP for `TimestampTz`, GEOGRAPHY, JSON, ARRAY; NULL as a typed parameter without a value; truncated bytes, nested arrays, NULL array elements and `DEFAULT` refused).
- `decode.rs`: schemas as GoogleSQL types (`STRING(10)`, `NUMERIC(10, 2)`, `ARRAY<STRUCT<…>>`, `RANGE<DATE>`) and kinds, and total cell decoders: INT64 beyond 2^53 → `BigInt`; NUMERIC / BIGNUMERIC → `Decimal` as sent; FLOAT64 NaN and infinities → `Other`; BYTES from base64; TIMESTAMP from int64 microseconds or (exactly, rounded to the microsecond) float seconds, written as `CAST(ts AS STRING)` writes it (`2024-01-02 03:04:05.123456+00`); DATE, TIME, DATETIME, INTERVAL as BigQuery writes them; GEOGRAPHY as WKT in `Other{geometry}`; JSON raw; RANGE `Other`; RECORD and REPEATED as JSON text in field order with exact numbers; a cell that does not fit its type is `Other` with its text, a missing one `Other{missing}`.
- `catalog.rs`: projects as databases (the profile's, the extra ones, then the listed ones), datasets as schemas, tables with their kinds (views, materialized views, external tables, snapshots), `tables.get` as the structure (columns, primary and foreign keys NOT ENFORCED, a foreign key to another project's table under `project.dataset`, partitioning and clustering as stand-in indexes, description, `numRows`), view definitions, routines with their signatures, completion from the INFORMATION_SCHEMA columns query, and the process list from `jobs.list` (ids `project:LOCATION.job_id`, parsed back for a kill; Tabler's own jobs marked).
- `errors.rs`: Google's error envelope and job `ErrorProto`s as `AppError`s (401 → `Auth`, 404 → `NotFound`, 429 and rate or quota reasons → `Db` with a retry hint, 502 / 503 → `Connect`, 408 / 504 → `Timeout`, a stopped job → `Cancelled`, otherwise `Db` with the HTTP code in `vendor_code`, the reason in `detail`, hints for the cost guard, billing and access, and the `at [line:column]` position mapped into the SQL).
- `testing.rs`, `tests.rs`: a scripted BigQuery (canned replies per method and path, delays, broken connections, every request recorded), canned `jobs.insert` / `getQueryResults` / `tabledata.list` / listing payloads, credential files with an RSA key made per run, and a reqwest transport for the wire test against `testkit::fake_http` (the token exchange as a form post, then a query with the bearer).
- `sqlgen/dialect/bigquery.rs`: the spec (backticked names and strings with backslash escapes, `FROM_HEX` bytes, filters through `CAST … AS STRING`, `TO_JSON_STRING` for nested and JSON columns and `ST_ASTEXT` for geography, LIKE with backslash escapes and no ESCAPE clause, `IS NOT DISTINCT FROM`, `NULLS FIRST / LAST`, `CURRENT_TIMESTAMP()` / `CURRENT_DATETIME()` / `CURRENT_DATE()` / `CURRENT_TIME()`, no EXPLAIN, `SET @@dataset_project_id` / `@@dataset_id` as a database switch, INFORMATION_SCHEMA as catalog words) and the driver's statements (`table_path`, Show DDL of tables and routines, completion columns). Raw strings take backslash pairs (`raw_prefixes`, since F8c).
- `sqlgen/portable/gen/bigquery.rs` complete: 81 reads covering `SELECT`, `WITH` and `FROM` (pipe queries), 143 write forms (every write call also with `SAFE.`), 26 own active and 19 own inert contexts, fuzz atoms, `check()` and `fuzz(3000)`; fixture blessed (951 cases); the holes the shared classifier closed in F8c (qualified and escaped callees) are cases of its writes.
- Bindings regenerated with `lane.mjs bigquery bindings --snapshot`: `lex/bigquery.json`, `BigqueryAuth.ts`, `BigqueryParams.ts`.
- Checks: `lane.mjs bigquery test --snapshot` 59 passed, `clippy --snapshot` clean, `test-all --snapshot` 1201 passed.

Left for Phase B: `entry.rs` (`open`, `validate`, `util` with a dataset picker), the `DbSession` (one BigQuery session per editor tab for scripts and temp tables, `TableSource` over `tabledata.list` with page tokens as cursors, sorted or filtered pages by query, counts from `numRows` (Costly), DDL from INFORMATION_SCHEMA on demand, the process list and kill, cancel through `JobCancel`, the editor's splitting of scripting blocks), the transport and token cache moved onto F8a's `HttpClient` / `TokenSource` with retries of transient 5xx, Command tokens through `CommandPassword::for_profile_ttl`, the renderer, the mock and the real-server suite. EditRows stays off in v1. To verify on a real server or the emulator: that a `DELETE` fails under the `bigquery.readonly` scope (critique D6, before the caps may say Server), `useInt64Timestamp` on `tabledata.list`, the NULL query parameter (`{"value": null}`), and whether GoogleSQL takes a separately quoted function qualifier (`` `ai`.if(…) ``, see Limits).

## Classifier sweep

Swept against the GoogleSQL references in October 2026 (the "All functions" list, the lexical page, the statement pages, pipe syntax, search functions, and the BigQuery ML and AI function lists). Every statement other than a query is a write by default; within a query:

| What                      | Functions                                                                                                                                                                                                                                                                                                                                                       | Class | Why                                                                                                                |
| ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----- | ------------------------------------------------------------------------------------------------------------------ |
| Federated queries         | `EXTERNAL_QUERY`                                                                                                                                                                                                                                                                                                                                                | write | runs SQL on Cloud SQL, Spanner or AlloyDB                                                                          |
| BigQuery ML and AI        | every `ML.*` and `AI.*` call: the words `ML` and `AI`, the quoted `` `ml.x` `` form, and the last parts `PREDICT`, `EVALUATE`, `FORECAST`, `DETECT_ANOMALIES`, `GENERATE*`, `EMBED`, `SIMILARITY`, `SCORE`, `CLASSIFY`, `COUNT_TOKENS`, `CHUNK_DOCUMENT`, `KEY_DRIVERS`, `CAUSAL_EFFECT`, `UNDERSTAND_TEXT`, `PROCESS_DOCUMENT`, `TRANSCRIBE`, `ANNOTATE_IMAGE` | write | remote models call Vertex AI endpoints, Gemini and the Cloud AI APIs; whether a model is remote is not in the text |
| Embedding search          | `VECTOR_SEARCH`                                                                                                                                                                                                                                                                                                                                                 | write | embeds a text or ObjectRef query value through the base table's connection                                         |
| Cloud KMS                 | `KEYS.KEYSET_CHAIN`, `KEYS.NEW_WRAPPED_KEYSET`, `KEYS.REWRAP_KEYSET`, `KEYS.ROTATE_WRAPPED_KEYSET`                                                                                                                                                                                                                                                              | write | unwrap or wrap through a KMS key                                                                                   |
| Sensitive Data Protection | `DLP_*`                                                                                                                                                                                                                                                                                                                                                         | write | DLP encryption with a KMS-wrapped key                                                                              |
| Cloud Storage objects     | `OBJ.FETCH_METADATA`, `OBJ.GET_ACCESS_URL`, `OBJ.GET_READ_URL`, `OBJ.LIST`, `EXTERNAL_OBJECT_TRANSFORM`                                                                                                                                                                                                                                                         | write | read object metadata and listings, and sign URLs, through a connection                                             |
| Earth Engine              | `ST_REGIONSTATS`                                                                                                                                                                                                                                                                                                                                                | write | reads Earth Engine rasters                                                                                         |
| Everything else listed    | strings, math, dates, JSON, geography (`ST_*` but `ST_REGIONSTATS`), arrays, ranges, hashing, `NET.*`, `HLL_COUNT.*`, `KLL_QUANTILES.*`, AEAD and the plain `KEYS.*`, `SEARCH`, `APPENDS`, `CHANGES`, `GAP_FILL`, graph functions, `SESSION_USER`, `ERROR`, `VECTOR_INDEX.STATISTICS`, window and aggregate functions, differential privacy                     | read  | compute on the query's data                                                                                        |

Statements: DML, DDL (`UNDROP`, models, remote functions, external tables, snapshots, search and vector indexes, row access policies, property graphs, project and capacity options), DCL, `EXPORT DATA`, `EXPORT MODEL`, `LOAD DATA`, `EXECUTE IMMEDIATE`, `CALL` (also the `BQ.*` system procedures: `ABORT_SESSION`, `JOBS.CANCEL`, `REFRESH_MATERIALIZED_VIEW`, `REFRESH_EXTERNAL_METADATA_CACHE`), the procedural language (`DECLARE`, `SET`, `BEGIN … END`, `BEGIN … EXCEPTION`, `IF`, `CASE`, `LOOP`, `WHILE`, `REPEAT`, `FOR … IN`, labels, `BREAK` / `LEAVE` / `CONTINUE` / `ITERATE`, `RETURN`, `RAISE`), transactions, `ASSERT` and GQL `GRAPH …` queries are writes (the last two only read, but no rule knows them). Pipe queries starting with `FROM` read; their `|> CALL` (a table-valued function) is a write.

Spellings of a write call, each kept as a case: the last name part (`ML.PREDICT(` calls `PREDICT`), the name quoted whole in one piece (`` `ml.predict`( ``, `` `keys.keyset_chain`( ``) or in parts (`` `ml`.`predict`( ``), and the same with `SAFE.`, GoogleSQL's only function-call prefix (`SAFE.ST_REGIONSTATS(`, `` `safe.st_regionstats`( ``, `` `safe.keys.keyset_chain`( ``, `` `safe.ml.generate_text`( ``, `` `safe`.`ml.predict`( ``): `write_calls` lists every function above whole with `SAFE.` in front (`SAFE.ML.*`, `SAFE.AI.*`, `SAFE.DLP_*`, `SAFE.KEYS.*` and `SAFE.OBJ.*` by name, `SAFE.EXTERNAL_QUERY`, `SAFE.EXTERNAL_OBJECT_TRANSFORM`, `SAFE.VECTOR_SEARCH`, `SAFE.ST_REGIONSTATS`), never a bare `SAFE.*`, since unquoted `SAFE.X(` calls `X`. Two spellings the rules cannot reach are change requests and Limits: a separately quoted qualifier over a built-in's last part (`` `ai`.if( ``) and a quoted name written with escapes (`` `st_regionstat\u0073`( ``).

Lexically: strings and backticked names always take backslash escapes, raw strings too (GoogleSQL keeps `\` and the next character together, so `r'\''` is one string), triple-quoted strings hold quotes and newlines, `#` and `--` start line comments, and block comments do not nest.

## Change requests

- **Shared lexer, raw strings (safety).** Done in F8c: raw strings keep backslash pairs and the flag is on again. `LexProfile::raw_prefixes` makes the scanner end a raw string at the first quote (`string_end(j, false)`), but GoogleSQL's raw strings keep a backslash and the next character together ("both characters are preserved"; "a raw string can't end with an odd number of backslashes"). With the flag on, `SELECT r'\', ' ; DELETE FROM t; --'` reads as a SELECT of two strings where BigQuery runs the DELETE. BigQuery is the only dialect with the flag; the lane turns it off (the prefix is then a word and the string after it reads backslashes like any other, which is where BigQuery ends it). The lead should make raw strings read backslash pairs (or drop the flag) in `dialect/lex.rs`, its TS mirror and the shared generated contexts ("glued to a raw string" assumes the opposite).
- **Classifier: qualified callees.** Done in F8c: qualified entries (`AI.*`, `ML.*`) match a qualified callee's last parts, a quoted part holding dots by the parts it spells. `write_calls` match a call's last name part, so `AI.IF(…)`, `ML.TRANSLATE(…)`, `AI.TRANSLATE(…)`, `AI.SEARCH(…)` and `AI.AGG(…)` share theirs with the plain built-ins `IF`, `TRANSLATE`, `SEARCH` and `AGG`. The lane catches them through the words `ML` and `AI`, which a separately quoted qualifier (`` `ai`.if(…) ``, `` `safe.ai`.if(…) ``) escapes. A `write_calls` entry with a qualifier (`AI.IF`) matching a qualified callee (words or quoted parts) would close it.
- **Classifier: escaped quoted names (safety).** Done in F8c: such a callee is on every deny-list and no allow-list. GoogleSQL's quoted identifiers "have the same escape sequences as string literals" (`\u0073` and `\U00000073` are valid in identifiers), but `Scan::ident_name` keeps a backslash as written, so `` `st_regionstat\u0073`(…) ``, `` `external_quer\u0079`(…) `` and `` `\u006dl.predict`(…) `` match no `write_calls` entry and read (`` `ml.predic\u0074`(…) `` is caught by `ML.*`). A quoted callee holding a backslash, under a reading whose quoted names take backslashes, may spell any function: it should answer like `U&"…"` (`Code::AnyName`), in `portable/classify.rs` and its TS mirror. ClickHouse's backticked names take backslash escapes too. The lane's `documented_holes_read_until_their_change_requests` test lists the cases.
- **F8a.** The shared `HttpClient` with header-carrying requests (BigQuery needs `x-goog-user-project`), form posts for the token endpoint, retries of 5xx / `backendError` for idempotent GETs, and a `TokenSource` with one refresh on a 401; `drivers::common::jwt` can take `jwt.rs` as it is.
- **Descriptor.** None for Phase A. `required` holds only `database`; the credential is checked by `validate` (a key file for `serviceAccount`).

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._

- Read-only is ClassifierOnly until a recorded test shows `DELETE` failing under the `bigquery.readonly` scope; a refresh-token (ADC user) login keeps the scopes of its `gcloud` login, so it is never narrowed.
- No impersonation, workload identity federation or Drive scope; no Storage Read API; no row edits (EditRows off).
- Safe Mode reads every BigQuery ML and AI call as a write (also `ML.BUCKETIZE` and `ML.PREDICT` over a local model), and so are a few reads that reuse a write word: `HLL_COUNT.MERGE(…)` (`MERGE`), pipe `|> SET` and `|> DROP`, a column named `ml`, `ai`, `set` or `begin` that is not backticked, temp functions (`CREATE TEMP FUNCTION …; SELECT …`), `ASSERT` and GQL `GRAPH` queries.
- Remote functions (`CREATE FUNCTION … REMOTE WITH CONNECTION`) are the user's own: a query calling one reaches a Cloud Run or Cloud Functions endpoint, and the classifier cannot tell it from any other function.
- A custom `apiEndpoint` must be HTTPS unless it is on this machine: an emulator on another host needs TLS or a local port forward, since every call carries the bearer token.
