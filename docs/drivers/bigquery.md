# BigQuery: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                     |
| ----------------- | --------------------------------------------------- |
| Lane              | `bigquery`                                          |
| Databases         | BigQuery                                            |
| Drivers           | `bigquery`                                          |
| Picker kinds      | `bigquery`                                          |
| SQL dialect       | `bigquery`                                          |
| Cargo features    | `drv-bigquery`                                      |
| Home slot         | `s2`                                                |
| Bindings prefix   | `Bigquery*`                                         |
| Lib test filters  | `drivers::bigquery`, `sqlgen::dialect::bigquery`    |
| Real-server suite | `tests/bigquery.rs` with `TABLER_TEST_BIGQUERY_URL` |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/bigquery/**`
- `src-tauri/src/sqlgen/dialect/bigquery.rs`
- `src-tauri/tests/bigquery.rs`
- `src-tauri/tests/fixtures/bigquery/**`
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

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._
