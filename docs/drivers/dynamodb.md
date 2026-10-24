# DynamoDB: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                     |
| ----------------- | --------------------------------------------------- |
| Lane              | `dynamodb`                                          |
| Databases         | DynamoDB                                            |
| Drivers           | `dynamodb`                                          |
| Picker kinds      | `dynamodb`                                          |
| SQL dialect       | none (native store)                                 |
| Native builders   | `sqlgen/native/dynamo.rs`                           |
| Cargo features    | `drv-dynamodb`                                      |
| Home slot         | `s2`                                                |
| Bindings prefix   | `Dynamo*`                                           |
| Lib test filters  | `drivers::dynamodb`, `sqlgen::native::dynamo`       |
| Real-server suite | `tests/dynamodb.rs` with `TABLER_TEST_DYNAMODB_URL` |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/dynamodb/**`
- `src-tauri/src/sqlgen/native/dynamo.rs`
- `src-tauri/tests/dynamodb.rs`
- `src-tauri/tests/fixtures/dynamodb/**`
- `src/features/drivers/dynamodb/**`
- `src/lib/ipc/mock/drivers/dynamodb/**`
- `src/lib/ipc/bindings/Dynamo*.ts`
- `docs/drivers/dynamodb.md`

Anything else (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead. `node dev/scripts/lane-commit.mjs dynamodb -m "dynamodb: …"` refuses other paths.

## Scope of v1

### DynamoDB

- **Identity:** `dynamodb`, no dialect (PartiQL plus API JSON). Crates: none (common-aws).
- **UI:** items grid (sampled attributes) plus typed JSON inspector.
- **Auth v1:** access key ID (user) plus secret (password) plus SessionToken; AWS profile / `credential_process`; environment; Command; Local (dummy keys).
- **Required fields:** database = Region, plus a credential source.
- **Out of scope v1:** assume-role, streams, GSI/table editing, PITR/backup.
- **Test:** SigV4 vectors, AttributeValue round trips, fake HTTP (paged ListTables, LastEvaluatedKey, throttling, TransactionCanceled). Opt-in: DynamoDB Local jar with Java 17+ (approval).

## Descriptor values

Seeded from the plan (§5.2). The values in force are the descriptors in `src-tauri/src/types/driver.rs` (keepalive belongs to the session's caps); a change is a request to the lead.

| Driver   | Location | Port (TLS) | SSH  | TLS style | Secret label    | Command TTL |
| -------- | -------- | ---------- | ---- | --------- | --------------- | ----------- |
| dynamodb | Cloud    | —          | None | Https     | SecretAccessKey | 600         |

| Driver   | Required fields / params | Label overrides                       | Keepalive | Read-only      | Sharing | Token secret |
| -------- | ------------------------ | ------------------------------------- | --------- | -------------- | ------- | ------------ |
| dynamodb | database                 | user: Access key ID; database: Region | None      | ClassifierOnly | Indep   | yes          |

## Read-only and hardening

| Engine   | When read-only                                                                           | When unattended (MCP) |
| -------- | ---------------------------------------------------------------------------------------- | --------------------- |
| DynamoDB | Scan, Query, GetItem, BatchGetItem, Describe*, List*, and ExecuteStatement with a SELECT | refused               |

Every classifier is default-deny: a statement or command it does not know is a write. The read-only mode in the caps (`Server`, `PerRequest` or `ClassifierOnly`) must say what the session really enforces.

## What to implement

- `entry.rs` (Phase B): `open`, `validate`, `util`, `util_kind` (plan §2.5). `<Prefix>Params` (`#[serde(rename_all = "camelCase", default)]`, ts-exported) parsed from `options.driver_params`.
- `NativeSession` plus `TableSource` (plan §2.5): browse with cursors, count, fetch cell, `plan_changes` / `apply_changes` (preview and operations from one plan, text through `sqlgen/native/<x>.rs`), the console (`split_console`, `classify_console` default-deny, `run_console`), `definition()`, `call` / `call_kind` (unknown calls are writes).
- `ping` is free, `close` returns within about 2 s.
- Cancel through `CancelHandle::Custom(Arc<dyn CancelAction>)`.
- Renderer: a `KindUi` per tile, `DriverUi` with `ConsoleUi`, `TableView` and `Inspector` where the grid does not fit, and a `MockDriver`. Write-capable `sessionCall` / `driverUtil` helpers await `guardExecution` first.

## Decisions and corrections for this lane

- A native store: no SQL dialect, a console of PartiQL plus raw API JSON, and MCP refuses it in v1 (plan A30).
- Signing and credentials through `drivers/common/aws` (`common-aws`). The secret access key is the password; a session token goes in the new `SessionToken` secret (plan A19) and can be longer than the 1,280 characters Windows Credential Manager keeps: store it through `set_secret` or fail with a clear error (critique D8).
- Writes prove their count with `ConditionExpression` (plan A13). A batch or sequential apply can succeed in part: return a result per change, and the renderer unstages the applied ones (critique D2).
- Costs: `count` Costly, no keep-alive (plan §1.5 Q8).

## Research notes

> Background from the driver research (October 2026), with file references and no line numbers. Verify against the code; where it differs from the scope above or from README.md, those win.

### Amazon DynamoDB

**Approach.**

Hand-roll the AWS JSON 1.0 protocol over the existing reqwest 0.13:

- `POST https://dynamodb.<region>.amazonaws.com/`, or the endpoint override (DynamoDB Local `http://localhost:8000`, LocalStack, VPC/FIPS endpoints, ScyllaDB Alternator).
- Headers: `Content-Type: application/x-amz-json-1.0` and `X-Amz-Target: DynamoDB_20120810.<Action>`.
- SigV4 signing with service 'dynamodb'.
- Typed serde structs for the roughly 20 actions used: ListTables, DescribeTable, DescribeTimeToLive, Scan, Query, GetItem, PutItem, UpdateItem, DeleteItem, BatchWriteItem, TransactWriteItems, ExecuteStatement, BatchExecuteStatement, ExecuteTransaction, CreateTable, UpdateTable, DeleteTable, UpdateTimeToLive, ListTagsOfResource, DescribeContinuousBackups.

The signer can be either:

- Hand-written with aws-lc-rs 1.18 `hmac::HMAC_SHA256` + `digest::SHA256`, which are already dependencies: about 200 lines, verified against AWS's published SigV4 test-suite vectors.
- Or `aws-sigv4 = "1.6.3"` (default features sign-http + http1; pure-Rust hmac/sha2) with `aws-credential-types`.

Retries: exponential backoff on ProvisionedThroughputExceededException, ThrottlingException, RequestLimitExceeded and 5xx. On 'Signature expired' / InvalidSignatureException, correct for clock skew using the response Date header.

Not recommended: `aws-sdk-dynamodb` 1.134.0 (published 2026-10-22, with new versions nearly daily) plus `aws-config` for the credential chain. It is a second HTTP/TLS stack (aws-smithy-http-client on hyper) and a large amount of generated code.

Cancel: drop the request. Every call is a single request.

**Windows build.**

Hand-rolled: zero new crates. reqwest 0.13.5, aws-lc-rs 1.18.1, chrono, serde_json and base64 are all present. No added build time, and aws-lc-sys already builds here.

aws-sigv4 1.6.3: about 10-15 small pure-Rust crates (aws-smithy-runtime-api, aws-smithy-types, aws-smithy-http, aws-credential-types, hmac, sha2, hex, time, tracing, http 0.2 + 1). Roughly 20-30 s clean build. The sigv4a feature (p256/ecdsa) is not needed.

aws-sdk-dynamodb 1.134.0 (+ aws-config 1.x): aws-smithy-runtime, aws-smithy-http-client (hyper 1 + rustls with aws-lc-rs, i.e. a second HTTP client next to reqwest), and aws-sdk-sso/ssooidc/sts through aws-config. Expect 2-4 min for a clean debug build and 8-15 MB added to the release binary. All pure Rust, so it builds on MSVC, but the cost is not justified.

**Authentication.**

TablePlus (DynamoDB Beta since 6.7.0, macOS Sep 2025; Windows b324) offers an IAM access key (access key ID + secret) and a region. It has no field for DynamoDB Local; a user asked for in #3793.

Tabler should offer:

1. Access key ID + secret access key, with an optional session token. profile.user holds the key ID; the secret goes in the keychain via the existing PasswordMode; the token is an extra secret.

2. AWS profile from ~/.aws/credentials + ~/.aws/config (on Windows, %USERPROFILE%/.aws). Supports static keys and `credential_process` (JSON {Version, AccessKeyId, SecretAccessKey, SessionToken, Expiration}). `role_arn` + `source_profile` via STS AssumeRole comes later; STS is a simple query API that can also be hand-signed. SSO profiles work through credential_process `aws configure export-credentials --profile X --format process`.

3. The existing 'command' password mode (types/profile.rs), printing the same JSON.

4. Environment variables AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_SESSION_TOKEN / AWS_REGION.

5. DynamoDB Local: no credentials (dummy keys) plus the endpoint.

Credentials are refreshed before their Expiration.

**Data model.**

How TablePlus shows DynamoDB: a 'Tables' section in the sidebar and an item-list grid. 6.9.1 fixed 'could not fetch the item list'. ListTables pagination showed only the first table until a fix in May 2026 (#3793).

How DynamoDB maps onto DbSession:

- list_databases → [region]; switch_database means changing the region. A static list of DynamoDB regions can be offered.
- list_schemas → [region].
- list_tables → ListTables, following every page.
- table_meta → columns:
  - Key attributes first: HASH with pk_ordinal 1, RANGE with 2; data_type S/N/B from AttributeDefinitions.
  - Then the union of attribute names from a sample Scan (Limit 100) plus the current page; data_type = the observed AWS type(s) (S, N, B, BOOL, NULL, M, L, SS, NS, BS).
  - primary_key = [hash, range].
  - indexes = GSIs and LSIs: IndexMeta name; columns = key attributes; method 'GSI'/'LSI'; definition = projection JSON.
  - row_estimate = ItemCount, which DynamoDB refreshes about every 6 h, so exact:false.
  - comment = status / billing mode / TTL attribute.
  - ddl = the CreateTable JSON.
  - triggers/checks empty.
- completion_metadata → table → attributes.

AttributeValue → Value:

- S → Text.
- N → Decimal string, keeping 38 digits; Int only when integral and JS-safe; never f64.
- B → Bytes; BOOL → Bool; NULL → Null.
- M / L → Json.
- SS/NS/BS → Array, or Other{ty:'SS'}.
- The per-cell type must survive the edit round trip: S '02134' must not become N. A missing attribute is not NULL.

Shared plumbing:

- Browse needs the native-browse hook and cursor paging (BrowseRequest.cursor = base64 of LastEvaluatedKey; previous pages through a cursor stack) described under Redis.
- Counts default to ItemCount (exact:false) through the estimate path in commands/browse.rs.

New UI beyond the grid:

- A key-condition query builder: an Eq on the partition key, optionally a sort-key op, maps to a cheap Query instead of a Scan. Show visibly which one will run.
- Consumed capacity in the status bar.
- A table info panel: billing mode, ItemCount/size with their update caveat, stream, TTL attribute, PITR, deletion protection, table class.
- 'Browse index' to Scan or Query a GSI/LSI.
- An item JSON editor with a DynamoDB-JSON ↔ plain-JSON toggle, reusing JsonEditorDialog.

**Query language.**

PartiQL through ExecuteStatement:

- SELECT/INSERT/UPDATE/DELETE.
- Identifiers in double quotes, strings in single quotes; `?` parameters become Parameters (AttributeValues).
- Paging with NextToken / LastEvaluatedKey. Limit counts items evaluated, not items returned.
- At most 8192 characters per statement, and 1 MB of data per call.
- Multiple write statements in one run can go through BatchExecuteStatement (≤25) or ExecuteTransaction (≤100, all-or-nothing).
- A SELECT without a key condition is a full scan: safe mode should flag it, and RCU cost should be shown.

Optional 'API mode', as in TablePro: an action name followed by a JSON request (`Query {"TableName": …}`, `Scan {…}`, `DescribeTable {…}`), restricted to an allow-list of read, write and describe actions, for features PartiQL lacks (IndexName, Select COUNT, ScanIndexForward).

Splitting: on `;`, but aware of `<<…>>` bags, `{…}` tuples and `'…'` strings; a new Partiql dialect in sqlgen/split.rs.

Editor: CodeMirror lang-sql with a PartiQL SQLDialect, plus lang-json for API mode.

Safe mode: SELECT and API actions Get\*/Query/Scan/Describe\*/List\* are reads.

**Editing.**

StagedChange planning (new module, e.g. src-tauri/src/sqlgen/dynamo.rs, or a Partiql dialect in sqlgen):

- Update → UpdateItem with Key, `UpdateExpression 'SET #n0 = :v0 REMOVE #n1'`, `ConditionExpression 'attribute_exists(#pk) [AND #n0 = :old0]'` for optimistic concurrency, and ExpressionAttributeNames/Values placeholders. Never inline names: reserved words and special characters break expressions. ConditionalCheckFailedException maps to the check_affected wording (drivers/mod.rs).
- Insert → PutItem with `attribute_not_exists(#pk)`.
- Delete → DeleteItem with Key + attribute_exists.
- Set NULL → writes {NULL: true}. Offer a separate 'Remove attribute' that sends REMOVE.
- Key attributes are immutable: delete + put in one transaction.
- Preview shows PartiQL equivalents, e.g. `UPDATE "t" SET "a" = ? WHERE "pk" = ?`, or the JSON requests.

Commit: TransactWriteItems (≤100 actions, ≤4 MB, double WCU cost). It is atomic, so it fits DbSession::transaction; larger batches are chunked non-atomically with a warning.

DDL:

- CreateTable: key schema, billing PAY_PER_REQUEST/PROVISIONED, GSIs/LSIs (LSIs only at creation), stream, table class, deletion protection.
- UpdateTable: one GSI create or delete per call, billing/throughput, stream.
- UpdateTimeToLive.
- DeleteTable, refused while DeletionProtectionEnabled.
- No column DDL; the table is schemaless.
- Truncate = Scan + BatchWriteItem DeleteRequest (25 per call, consumes WCU, slow) or drop/recreate, which loses tags, PITR and streams. Warn either way.

**Introspection.**

Tables: `ListTables` with {ExclusiveStartTableName, Limit: 100}, repeated until LastEvaluatedTableName is absent. TablePlus showed only the first table (#3793).

`DescribeTable` {TableName} → Table with:

- KeySchema [{AttributeName, KeyType HASH|RANGE}].
- AttributeDefinitions [{AttributeName, AttributeType S|N|B}].
- GlobalSecondaryIndexes [{IndexName, KeySchema, Projection{ProjectionType ALL|KEYS_ONLY|INCLUDE, NonKeyAttributes}, IndexStatus, ProvisionedThroughput, OnDemandThroughput, ItemCount, IndexSizeBytes}].
- LocalSecondaryIndexes.
- ItemCount and TableSizeBytes, refreshed about every 6 h.
- TableStatus, BillingModeSummary, ProvisionedThroughput, StreamSpecification / LatestStreamArn, TableClassSummary, DeletionProtectionEnabled, SSEDescription, CreationDateTime, TableArn.

Other calls:

- `DescribeTimeToLive` {TableName} → TimeToLiveDescription{AttributeName, TimeToLiveStatus}.
- `DescribeContinuousBackups` {TableName} → PITR status.
- `ListTagsOfResource` {ResourceArn}.
- `DescribeLimits`.

Column sampling: `Scan` {TableName, Limit: 100}, then key attributes first, then the union of names and types.

Counts: ItemCount as the estimate, or opt-in exact: `Scan` {Select: 'COUNT'} looping on LastEvaluatedKey. That is expensive: it reads the whole table.

ServerInfo: endpoint + region + caller identity (optional STS GetCallerIdentity). There is no version API.

**Testing without a server.**

Unit tests:

- SigV4 against AWS's published sigv4 test suite: get-vanilla, post-vanilla, post-x-www-form-urlencoded, header ordering and normalization, plus a known DynamoDB request vector and the session-token header.
- AttributeValue ↔ Value round trips: N precision to 38 digits, the SS/NS/BS sets, binary, nested M/L.
- FilterGroup → KeyConditionExpression vs FilterExpression with placeholder names.
- Credential file and credential_process parsing.
- A fake HTTP server (tokio TcpListener) replaying ListTables with several pages, DescribeTable, Scan pages with LastEvaluatedKey, ConditionalCheckFailedException, ThrottlingException retries and a TransactionCanceledException with CancellationReasons.

Integration without AWS:

- DynamoDB Local: `docker run amazon/dynamodb-local -jar DynamoDBLocal.jar -sharedDb -inMemory`, or the jar on Windows with Java 17+, as a `dynamodb` profile in dev/docker-compose.yml.
- LocalStack on .
- ScyllaDB Alternator for a compatibility check.

**What it shares with an existing driver.**

Not wire-compatible with anything existing. PartiQL is SQL-shaped: a `Dialect::Partiql` in sqlgen/quote.rs (identifiers in double quotes with doubling, like PG; literals in single quotes) could generate preview text and editor completions. Browsing cannot reuse sqlgen/select.rs, because PartiQL has:

- no LIMIT/OFFSET (Limit is a request parameter);
- ORDER BY only on the sort key with partition-key equality;
- no JOIN or GROUP BY;
- begins_with/contains instead of LIKE.

So the grid uses Scan/Query JSON. Compatible services: DynamoDB Local, LocalStack, and ScyllaDB Alternator (DynamoDB API; PartiQL/ExecuteStatement is not part of its documented support, so use API mode there).

**Connection fields.**

- Name / color / tag (existing)
- Auth method: Access key / AWS profile / Environment / Command (credential_process JSON) / DynamoDB Local
- Access key ID (profile.user)
- Secret access key (password; keychain/ask/command)
- Session token (optional, second secret)
- AWS profile name
- Assume-role ARN + external ID + MFA serial (optional, phase 2)
- Region (e.g. us-east-1; profile.database)
- Custom endpoint URL (DynamoDB Local http://localhost:8000, LocalStack http://localhost:4566, VPC/FIPS endpoints, Alternator)
- Consistent reads (strongly consistent Scan/Query/GetItem)
- Item sample size for column inference (default 100)
- Page size (Limit) and max requests per grid page
- Show consumed capacity (ReturnConsumedCapacity TOTAL)
- Read only / safe mode (existing)
- Proxy: OS proxy already honoured by reqwest system-proxy

**Effort:** L.

**Risks.**

- Owning SigV4 signing: canonicalization bugs show up as 403 InvalidSignatureException. Mitigate with the AWS test vectors, or use the aws-sigv4 crate. Clock skew on user machines needs Date-header correction.
- The credential chain is broad (SSO, assume-role chains, MFA, IMDS, web identity). Hand-rolling all of it is real work. Start with static keys + profile + credential_process + DynamoDB Local, and route SSO through `aws configure export-credentials`.
- Cost and throttling: Scans and exact counts consume read capacity on real tables, and PartiQL SELECTs without key conditions are full scans. Defaults must be conservative (Limit, no automatic exact count, visible capacity), with retry/backoff on throttling.
- No offset paging: Limit counts evaluated items, so a filtered page may come back empty with a LastEvaluatedKey. Keep fetching up to N requests per page, and support Previous through a cursor stack.
- Schemaless, sparse attributes and mixed types per attribute: column inference is approximate, and per-cell types must be preserved on write (S vs N, L vs SS). Value has no per-cell DynamoDB type tag beyond Other.ty.
- Service limits: 400 KB per item, 1 MB per response, 8192-character PartiQL statements, TransactWriteItems at 100 actions / 4 MB, BatchWriteItem at 25, one GSI change per UpdateTable call.
- Key attributes cannot be updated in place (delete + put). ItemCount is about 6 h stale.
- aws-sdk-dynamodb version churn (1.130 → 1.134 within three weeks) if the SDK route is chosen.

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._
