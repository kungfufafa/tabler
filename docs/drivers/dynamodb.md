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

## Phase A status

Built in the lane's own files, `pub` and unused until `entry.rs` is handed over (F10); `drivers/dynamodb/mod.rs` carries a temporary `#![allow(dead_code)]`.

| Module                      | What it does                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `drivers/dynamodb/sigv4.rs` | SigV4 on aws-lc-rs, checked against AWS's signing test suite. Moved to `drivers/common/aws` (F8a, F8d); removed in Phase B.                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `credentials.rs`            | Access keys, the environment, `~/.aws` profiles, `credential_process`, the Command mode and Local's keys. Moved to `drivers/common/aws` (F8a, F8d); removed in Phase B, where `auth.rs` keeps what only DynamoDB needs.                                                                                                                                                                                                                                                                                                                                                                           |
| `params.rs`                 | `DynamoParams` (`auth`, `profileName`, `endpoint`, `consistentReads`, `sampleSize` 100, `pageRequests` 5, `consumedCapacity`) and `DynamoAuth`, ts-exported; region names; `https://dynamodb.<region>.amazonaws.com/` (`.com.cn` in China), `http://localhost:8000/` for Local, overrides normalized (http/https, a host, no credentials, query or fragment).                                                                                                                                                                                                                                     |
| `transport.rs`              | The HTTP seam (one POST, the whole reply, request and response headers); in Phase B `HttpTransport` sends it through the shared `HttpClient`.                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `client.rs`                 | Signed JSON 1.0 calls (`X-Amz-Target: DynamoDB_20120810.<Action>`, every sent header signed). Throttling (`ProvisionedThroughputExceededException`, `ThrottlingException`, `RequestLimitExceeded`, `TransactionInProgressException`, 429) retried with capped exponential backoff and full jitter; server errors and lost connections only for reads and token-carrying `TransactWriteItems`; a stale or early signature retried once with the clock corrected from the reply's `Date`; cancel drops the request or the pause.                                                                    |
| `errors.rs`                 | Error replies (`__type`, `x-amzn-ErrorType`, `message`/`Message`, `CancellationReasons`) as `AppError`s: refused credentials `Auth`, a gateway's 502–504 `Connect`, the rest `Db` with the exception name in `detail` (DynamoDB has no numeric codes); a failed condition reads like `check_affected`; a cancelled transaction names each failed item.                                                                                                                                                                                                                                            |
| `attr.rs`                   | AttributeValues ↔ `Value`, each attribute keeping its type: `S` Text, `N` Int/BigInt/Decimal by its exact text (never a float), `B` Bytes, `BOOL`, `NULL`, `M`/`L` Json of their DynamoDB JSON content (cut past 64 KiB in the grid, whole in `fetch_cell`), `SS`/`NS`/`BS` Array, a missing attribute `Other{missing}`. Writing takes the loaded (or column) type as a hint, so `'02134'` stays a string; Json cells read DynamoDB JSON when every value is typed, else plain JSON with numbers' digits kept (an own reader: serde_json would round them). Decoders are total and depth-bounded. |
| `catalog.rs`                | `ListTables` and `DescribeTable` replies; columns (keys first, then sampled attributes in first-seen order typed `S\|N\|NULL`, then index keys, at most 500); indexes (`PRIMARY`, `GSI`, `LSI` with projections); the item count as the estimate; a comment (status, billing, TTL, stream, class, protection); the table as its `CreateTable` request (definition and DDL).                                                                                                                                                                                                                       |
| `browse.rs`                 | Every `ListTables` page; describe, sample and TTL into `TableMeta`; grid pages (Scan, or Query when the filter fixes the partition key) asking for the rows still missing, up to `pageRequests` requests, the `LastEvaluatedKey` as a base64url cursor; sort only by the sort key of a Query; counts (estimate, or `Select: COUNT` over every page); cells by `GetItem`; console commands (PartiQL paged by `NextToken` up to the row cap, API requests once); edits applied one by one (stop at the first failure) or as transactions (cancellation reasons per change).                         |
| `testing.rs`, `tests.rs`    | `FakeDynamo`: tables in key order, the actions above, the expression subset the builders write, scripted errors, SigV4 checked against a known secret and a server clock, expired tokens, idempotency tokens; served in-process and over a socket (`testkit::fake_http`) with a plain reqwest transport.                                                                                                                                                                                                                                                                                          |
| `sqlgen/native/dynamo.rs`   | Every request and its text (`Action {json}`, which the console reads back to the same request); the read-action allow-list and the console's splitter and classifier; `#n`/`:v` placeholders; filters as `KeyConditionExpression` and `FilterExpression`; cursors; edits as `PutItem`/`UpdateItem`/`DeleteItem` guarded by the key and every changed attribute's loaded value, a changed key as delete + put in one `TransactWriteItems`, a commit as transactions of at most 100 items with `ClientRequestToken`s.                                                                               |

### Classifier sweep (API reference, October 2026: 58 actions)

The console takes PartiQL statements (sent with `ExecuteStatement`) and API commands (`Action {json}`). A command reads only when its action is on the read list, or (the statement actions) when every statement it carries is a `SELECT`; anything else, unknown or misspelled actions included, is a write.

- **Reads (27):** BatchGetItem, DescribeBackup, DescribeContinuousBackups, DescribeContributorInsights, DescribeEndpoints, DescribeExport, DescribeGlobalTable, DescribeGlobalTableSettings, DescribeImport, DescribeKinesisStreamingDestination, DescribeLimits, DescribeTable, DescribeTableReplicaAutoScaling, DescribeTimeToLive, GetItem, GetResourcePolicy, ListBackups, ListContributorInsights, ListExports, ListGlobalTables, ListImports, ListTables, ListTagsOfResource, Query, Scan, SearchVectors, TransactGetItems.
- **By their statements (3):** ExecuteStatement, BatchExecuteStatement, ExecuteTransaction (a read transaction only when all its statements are `SELECT`). The body is sent whole, so it may hold only the members the reference names, spelled exactly (ExecuteStatement: `Statement`, `Parameters`, `ConsistentRead`, `Limit`, `NextToken`, `ReturnConsumedCapacity`, `ReturnValuesOnConditionCheckFailure`; the batch and transaction members and their statement objects likewise); any other member (`statement` beside `Statement`, in case DynamoDB matched names regardless of case) makes it a write. Repeated keys are refused when the console parses the JSON.
- **Writes (28):** items (BatchWriteItem, PutItem, UpdateItem, DeleteItem, TransactWriteItems); tables, indexes and replicas (CreateTable, UpdateTable, DeleteTable, UpdateTimeToLive, UpdateTableReplicaAutoScaling, CreateGlobalTable, UpdateGlobalTable, UpdateGlobalTableSettings); backups and recovery (CreateBackup, DeleteBackup, RestoreTableFromBackup, RestoreTableToPointInTime, UpdateContinuousBackups); other services reached (ExportTableToPointInTime writes to S3, ImportTable reads S3 and creates a table, Enable/Disable/UpdateKinesisStreamingDestination); settings, policies and tags (UpdateContributorInsights, PutResourcePolicy, DeleteResourcePolicy, TagResource, UntagResource).
- **PartiQL:** a statement reads only when its first word (after whitespace and `--` / `/* */` comments, ASCII letters only, not glued to `_`, `$` or a digit) is `SELECT`. `INSERT`, `UPDATE`, `DELETE`, `EXISTS` (valid only inside a transaction), a leading `(`, a BOM, `ſelect` and anything unknown are writes. DynamoDB's PartiQL functions (`EXISTS`, `BEGINS_WITH`, `ATTRIBUTE_TYPE`, `CONTAINS`, `MISSING`, `SIZE`, and `LIST_APPEND`, `SET_ADD`, `SET_DELETE` in `UPDATE`) change nothing by themselves: the statement word decides. One statement runs per `ExecuteStatement`, so the splitter's statements are exactly what is sent; a statement inside an API request is sent as written and classified on that text, always lexed as PartiQL (never as JSON, even after `SELECT {`), and must be one statement (only comments and `;` after it).
- **Lexing:** as PartiQL's lexer (PartiQLTokens.g4): a `--` comment ends at CR or LF, `/* */` does not nest, `'…'` and `"…"` double their quote. Text that lexers read differently is never a read, because the server's lexer is not documented: non-blank text after a lone CR or another line break (VT, FF, NEL, U+2028, U+2029) inside a `--` comment, a `/*` inside a block comment (SQL nests them), an Ion literal in backticks (PartiQL's own lexers disagree on a backtick inside its strings and comments), a backslash before a closing quote (an escape to some lexers), anything unterminated. A differential fuzz test checks that a statement read as a read is one `SELECT` to every combination of those lexer choices.
- Not part of this API and never sent by the console: DynamoDB Streams (`DynamoDBStreams_20120810`), DAX, STS.

## Phase B status

Part B1 (the backend driver) and part B2 (the renderer module, the mock and the URL handling, below) are built and tested without a server. `#![allow(dead_code)]` is gone from `mod.rs`.

| Module                         | What Phase B added                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| ------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `entry.rs`                     | `open` (refused in the MCP process: native stores are not offered to MCP clients in v1), `validate` (`DynamoParams`, the region's spelling, the Command Line mode for the Command source; a missing key or secret is the connect's `Auth` error, as a profile is saved before its secret), `util` (`regions`, `awsProfiles`: the profile names of this machine's shared files) and `util_kind` (both read; anything unknown writes).                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `auth.rs`                      | The credentials by `DynamoAuth` over `drivers::common::aws`: static keys with the `SessionToken` secret, a named profile (keys or `credential_process` through `credential_process_runner`, its `endpoint_url` used when the profile names none), the environment, Local's dummy keys, and the Command mode (below). `Credentials::local` and `from_command_output` stay the lane's.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `session.rs`                   | `DynamoSession`: `NativeSession` + `TableSource`, the caps, read-only, cancel, the trace, `tableInfo` and `browseIndex` calls.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `client.rs`, `transport.rs`    | Signing by `common::aws::sigv4::sign_parts` with the lane's clock-skew correction (a reply's `Date` more than a day off is ignored, and the corrected clock never overflows), the shared `Credentials` / `CredentialProvider`, the Command mode's command run again after `ExpiredTokenException`, every call but the console's own recorded in the session's `SqlTrace`. `HttpTransport`: `HttpClient::request_exact` (the URL as signed), `Credential::None`, `RetryPolicy::NONE` under the client's own loop, the body read chunk by chunk up to `max_reply_bytes(action)` (8 MiB, 16 MiB for `TransactGetItems`, 32 MiB for `BatchGetItem`: DynamoDB's own are 1, 4 and 16 MB of items, more as JSON). Before a reply is parsed, `parsed_size` estimates from one pass over its text what its `serde_json::Value` would take (a tree node per object), and one past `MAX_REPLY_MEMORY` (128 MB) is refused. |
| `browse.rs`                    | Pages of a secondary index (`IndexRead`, strongly consistent only on a local index), the page's items handed to the session (which remembers a table's, never an index's), result columns typed `S\|N` (at most 500, and the rows that fit `Budget::cells`), the affected count only where the reply proves it (`PartiqlWrite`), an exact count that refuses a `Count` that would overflow, `Budget::bytes` for grid pages and PartiQL statements, `ListTables` bounded to a region's 10,000 tables.                                                                                                                                                                                                                                                                                                                                                                                                            |
| `sqlgen/native/dynamo.rs`      | `comment_at` linear (a comment ended by a lone `\r` looks ahead only to the first text or `\n`), `split_console` counting lines on from the last command, `partiql_write` (`PartiqlWrite`: `INSERT` and `UPDATE` change one item, `DELETE` at most one), `LoadedItems::forget` and its byte budget (`BYTE_CAP`, `trim_loaded` over a session's tables), `ConsistentRead` as the caller asks (Phase A left it out of every index read, local ones too).                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `session_tests.rs`, `tests.rs` | The session against the fake (connect over a socket through `entry::open`, TLS against `testkit::tls`); `tests/dynamodb.rs` against a real server.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |

**Caps** (`session::capabilities`, proven by `caps_grant_what_is_implemented` and the test named with each):

- Features: `StructureView` (sidebar and structure: `the_catalog_in_the_sidebar_s_terms`), `Editor` (the console: `console_runs_through_the_editor`), `EditRows`, `InsertRows`, `DeleteRows` (`a_commit_with_one_failing_change`, `tests::edits_keep_types_and_stop_at_a_changed_item`), `ExactCount` (`grid_pages_counts_and_cells`), `ViewDefinition` (the table's `CreateTable` request as `definition()`). Not granted: `CancelOnServer` (a request DynamoDB received runs to its end; a cancel stops the client's side and every later page), `Mcp`, `EditWithoutKey`, `ServerExpressions`, `SwitchDatabase`, `TableDdl` and every structure, admin, import and export feature.
- Namespaces: the region is the one database and schema (`SchemaMode::Single`, default schema the region, labels Region / Table / Tables).
- Data: `Paging::Cursor` (the `LastEvaluatedKey`), `CountMode::Costly` (the item count DynamoDB refreshes about every six hours, or every page read with `Select: COUNT`), `sort` (only by the sort key of a Query; any other is refused with the reason), the 17 filter operators of `FILTER_OPS` (every other one is refused by the builders), no raw filter, `TxMode::Batch` (one change is one conditional request; more go as transactions of at most 100 items, each all or nothing, with a result per change), `Affected::Exact` (each write's condition proves it hit the one item it names).
- Safety: `ClassifierOnly` (DynamoDB has no read-only session), not hardened, Safe Mode confirms with the passcode (the secret is a token). Lifecycle: no keep-alive, ping free.

**How it behaves.**

- **Connect** proves the endpoint and the credentials with a ping: `DescribeEndpoints` (free), or `ListTables {Limit: 1}` once the server says it has no such action (DynamoDB Local), remembered for the session. Server info: `DynamoDB API 2012-08-10` (with the endpoint when the profile names its own), the region, the access key ID.
- **TLS** (`session::endpoint`, `aws_endpoints_are_always_verified`): the form's SSL mode applies only to an endpoint the form names (or DynamoDB Local's) whose host is not AWS's: an emulator or a private host. Everything else is verified in full whatever the form's mode (its plain HTTPS modes skip the check), with the profile's CA file and client certificate: the region's endpoint, any host under `amazonaws.com`, `amazonaws.com.cn` or `api.aws` named in the form (FIPS, VPC `*.vpce.amazonaws.com`, dual-stack), and an AWS profile's `endpoint_url` (the AWS SDKs always verify it). B2's form should start an https endpoint at verify-full. SSH: none (the descriptor's). Requests give up after 60 s (`Timeout`); the console's and the grid's time outs cancel sooner.
- **Hostile replies**: a body past its action's cap (8 MiB for a page) is refused as it streams (`a_reply_past_the_size_cap_is_refused`); a reply of many small objects, which parsed takes about 100 times its text (`[{"":0},…]`, deep one-entry maps), is refused before it is parsed once its estimate passes 128 MB (`a_reply_too_big_once_parsed_is_refused`, `parsed_sizes_count_values_outside_strings`; a full grid page of 1,000 items of 50 attributes estimates about 87 MB); a `Date` more than a day off does not move the signing clock (`an_implausible_server_date_is_ignored`: a year-262142 `Date` once made chrono's `+` panic, which aborts a release build), counts that would overflow are an error (`a_count_that_overflows_is_refused`).
- **Bounded memory and time** (a sparse real table reaches some of these, a hostile server all): attributes are gathered by name in hash maps, so a page naming a million attributes stays linear (`many_attributes_are_sampled_in_linear_time`; a scan of every name for every name took minutes, uncancellable). The console's result shows at most 500 attributes, like the grid, and the rows that fit 5,000,000 cells (10,000 rows of 500), each with a notice: every item has a cell for every attribute, so 10,000 items of one attribute each would have made 10^8 missing cells and aborted on memory (`sparse_console_results_are_capped`). Item budgets count `attr::item_size`, an upper bound of an item's memory: a tree node (632 bytes) for every map however small, a value's own 32 bytes in its list's or set's room (twice, as collected), every name and text with the allocator's header and rounding (`sizes_count_every_nested_value`). Measured against a process's private bytes, items take 0.4 to 0.9 times what is counted (flat texts 0.54, lists of NULLs 0.62, one-entry maps 0.78, 30-deep one-entry maps 0.86, one-attribute items 0.88), where the earlier count of texts and 16 bytes per value undercounted nested maps 20 to 40 times. A grid page and a PartiQL statement stop reading pages once 128 MB of items are in (`browse::Budget`; DynamoDB's pages are 1 MB, so only a hostile server or items of many tiny maps reach it), the statement with a notice (`reads_stop_at_their_byte_budget`). `ListTables` refuses a page longer than the `Limit` of 100 or a name longer than 255 characters and stops after a full region's 10,000 tables (`table_listings_stay_within_a_region`). The items remembered for edits are bounded by count (20,000 per table) and size (128 MB per table, 256 MB per session, counted by `item_size` with the key's text twice and the map's entry, other tables' forgotten first; each item is trimmed as it is recorded, so a page's copy never passes the table's cap: `loaded_items_are_bounded_by_size`). Sets are checked for repeated members by hashing.
- **Console counts**: a PartiQL `INSERT` or `UPDATE` that succeeds changed one item (DynamoDB refuses a duplicate key with `DuplicateItemException` and a missing one with `ConditionalCheckFailedException`): "1 row affected". A `DELETE` of a missing key succeeds and deletes nothing, so a `DELETE` reports "OK" with no count, unless it returned the item (`RETURNING ALL OLD *`): then its row and "1 row affected" (`partiql_writes_count_only_what_their_reply_proves`). "OK" (`StatementKind::Empty`) rather than an `Affected` result without a count, which the editor shows as "0 rows affected".
- **Command mode**: the profile's command runs through `secrets::run_command_output` (credential_process JSON spans lines) inside a `secrets::CommandPassword` with the descriptor's 600 s TTL, seeded by the connect's output when its last line is the whole of it (JSON on one line, or a bare secret). Its credentials are kept until five minutes before their `Expiration` (output about to expire runs the command again at once), and the command runs again after an `ExpiredTokenException`. Output without `Expiration` is kept for the session, as AWS's credential_process contract says.
- **Read-only**: the session refuses `plan_changes` and `apply_changes`, every console command the classifier calls a write (also a statement action carrying one, a member the reference does not name, an unknown action, a statement lexers could read differently) and every call that is not a read, before anything is sent (`read_only_refuses_every_write_path`); the commands layer refuses them first as well.
- **Cancel and time out**: each operation's `CancelHandle::Custom` cancels a token its requests race against (the request is dropped, or the pause between retries); `close()` cancels every one still running at once. The query time out cancels through the session and reports a time out (`cancel_and_time_out`).
- **Commits**: after a commit the items it applied are forgotten, so their next edit is guarded by the key until the grid loads them again; a failed transaction keeps what was loaded.
- **Trace**: every request the app sends for itself (sidebar, structure, pages, counts, cells, commits, calls) goes to the console log as `Action {json}` with its item count or error; console commands are logged by the editor.

**Tests**: 104 lib tests in the lane's filters (`lane.mjs dynamodb test`; 84 in `drivers::dynamodb`, 20 in `sqlgen::native::dynamo`; `test-all`: 1,337 passed), among them value mapping (`attr`: 2^53 + 1 a BigInt, decimals and timestamps as text, maps and lists JSON; end to end in `grid_pages_counts_and_cells`), placeholders and injection (`names_and_values_are_never_inlined`, the builders' tests), the classifier and console (`sqlgen::native::dynamo`, with `long_texts_stay_linear` and `insert_and_update_change_one_item_and_delete_at_most_one` for the hardening rows), hostile replies (`an_implausible_server_date_is_ignored`, `a_reply_past_the_size_cap_is_refused`, `a_reply_too_big_once_parsed_is_refused`, `a_count_that_overflows_is_refused`), bounded memory and time (`sparse_console_results_are_capped`, `reads_stop_at_their_byte_budget`, `table_listings_stay_within_a_region`, `many_attributes_are_sampled_in_linear_time`, `loaded_items_are_bounded_by_size`), TLS (`tls_follows_the_profile_for_its_own_endpoint`, `aws_endpoints_are_always_verified`), and the session's (connect, list, browse with paging, count, cell, a commit with one failing change, a console run, a cancel, a time out, read-only refusals).

**What only a real server can confirm** (`tests/dynamodb.rs`, `TABLER_TEST_DYNAMODB_URL`): that DynamoDB Local and AWS accept the signature through `HttpClient` (the fake checks it with the shared signer), the exact error codes of DynamoDB Local for `DescribeEndpoints` and `DescribeContinuousBackups`, PartiQL statements beyond the fake's `SELECT * FROM t` (the suite runs an `UPDATE` for "1 row affected" and a `DELETE` of a missing key for "OK"), `RETURNING ALL OLD *` on a `DELETE`, the cancellation reasons of a real `TransactWriteItems`, index reads (the fake records the request, with `ConsistentRead` only for a local index, but answers none), a cancel that stops a count in flight, a read-only session's refusals with the read-only message leaving the table and its items as they were (`read_only_refuses_writes`, a writable session's plan included), and how long a table takes to become active.

**Deliberate differences from TablePlus** (its DynamoDB support is a beta: an IAM access key and a region):

- Credential sources beyond the access key: a named AWS profile (keys or `credential_process`, its `endpoint_url`), Tabler's environment, the password's Command mode, and DynamoDB Local with dummy keys and an endpoint field, which TablePlus lacks (asked for in #3793).
- Every `ListTables` page is followed (TablePlus showed only the first until a fix in May 2026, #3793).
- The region is the profile's: no region switch in the sidebar (`SwitchDatabase` is not granted); a second region is a second connection.
- Sorting is served only by the sort key, in a Query; any other sort is refused with the reason instead of sorting one page locally, which would misorder a table read page by page.
- The console takes PartiQL and every DynamoDB API action as `Action {json}`, classified default-deny for Safe Mode and read-only.

The public sources say nothing more of TablePlus's DynamoDB (its count, sort or console behaviour); where Tabler differs elsewhere it is for the reasons in Limits.

### Part B2: the renderer and the mock

| File (`src/features/drivers/dynamodb/`)                | What it does                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| ------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `index.ts`                                             | The `LaneModule`: the `dynamodb` `KindUi`, and a `DriverUi` with the console, the inspector and a table's menu. No `TableView` or `StructureView` (the shared grid and Structure tab show the sampled attributes, cursor pages, `Other{missing}` cells and `definition()`), no `DialectUi` (no SQL dialect).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `form.ts`, `params.ts`, `DynamoFields.tsx`             | The form. Every row is the lane's (`rows: []`), shown by the credential source: Access key (Access key ID as the user, Secret Access Key as the secret with its Keychain, ask, none and Command Line modes), AWS profile (the name, with this machine's profiles from the `awsProfiles` call), Environment, Command (the secret's Command Line, chosen with the source) and DynamoDB Local; the region with the `regions` call's names; an endpoint of its own; the SSL mode only for an https endpoint off AWS's domains (`formTlsApplies`, as `session::endpoint`), starting at verify-full; consistent reads, consumed capacity, sample size and requests per page. `validate` says what `entry::validate` and `DynamoParams::check` refuse (the region's spelling, the endpoint as `normalize_endpoint` reads it, the Command source without Command Line, 1–1000 sampled items, 1–50 requests) and asks for an access key's ID. `toProfile` saves every `DynamoParams` field, no host, port or SSH, the user only for the sources that sign with it, and password mode `none` for those that read no secret. The form's calls are reads, sent without a secret and with password mode `none` (no Command Line runs: the backend resolves a Command Line secret before any util call, and opening the form must not start an SSO login or MFA push). |
| `url.ts`                                               | `dynamodb://[keyId[:secret]@]region[?auth=&profile=&endpoint=&consistentReads=&sampleSize=&pageRequests=&consumedCapacity=&sslmode=]`: a secret goes to the password, never into the profile; the format writes no secret and only what differs from the defaults, and round-trips for every source; `sslmode` is read as one of the form's two (`disable` and `prefer` as `require`, `verify-ca` as `verify-full`).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `classify.ts`, `console.ts`                            | The console's splitter and classifier, a mirror of `split_console`, `classify_console`, `parse_command`, `partiql_class`, `api_class` and `attr::parse_json` (code points, Unicode `White_Space`, repeated keys, nesting past 64, numbers beyond a double), on the case file both run: `src-tauri/tests/fixtures/dynamodb/console.cases.json` (231 classify and 15 split cases; `sqlgen::native::dynamo::tests::shared_console_cases` and `classify.test.ts`). Offsets go to the editor as UTF-16. The language is a small tokenizer (PartiQL's comments, strings, quoted names, Ion literals, the actions, JSON names); files are `.partiql`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `DynamoInspector.tsx`                                  | The item in `JsonDocumentEditor` with the DynamoDB JSON ↔ plain JSON toggle: changed attributes staged as one change, a removed one as missing (`REMOVE`), the key attributes read-only (a key changes in the grid).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `menu.ts`, `info.ts`, `TableDialogs.tsx`, `dialogs.ts` | A table's menu: Table Info… (`tableInfo`: status, billing and capacity, item count and size with their six-hour caveat, key, indexes, stream, TTL, point-in-time recovery, deletion protection, class, encryption, creation, ARN; "not reported by this server" where an emulator has none) and Browse Index… (`browseIndex`: pages of a global or local index with Next and Previous over its cursors, read only). Both calls read, so they are sent as they are (no Safe Mode prompt) and work on a read-only connection.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |

The lane makes no write-capable `sessionCall` or `driverUtil` (its two session calls and two form calls all read), so it has no `guardExecution` of its own; console runs and commits go through the shared editor and grid, which await it.

**The mock** (`src/lib/ipc/mock/drivers/dynamodb/`, `?mockDrivers=dynamodb`): three profiles (an access key in us-east-1, DynamoDB Local, a read-only AWS profile) over one region's tables (`data.ts`: Music with a GSI and an LSI, 2^53 + 1 as a big integer, sets and maps; sparse Users with a string zip and NULLs; Events of 660 items). `session.ts` has the backend's caps exactly (`dynamoCaps`), the region as the one database and schema, columns as `catalog::columns` gives them (`model.ts`), Scan or Query pages by cursor (an index's cursor holds the table's key too), sort only by the sort key of a Query with the backend's refusals, the 17 filter operators, a stale estimate or an exact filtered count, whole cells, Preview as `UpdateItem` / `PutItem` / `DeleteItem` / `TransactWriteItems` text, one change as one conditional request and more as one transaction (all or nothing: `CONDITION_FAILED` on the failing change and "not applied: the transaction was cancelled because item N failed" on the others; a key change as a delete and a put), the `tableInfo` and `browseIndex` calls, `regions` and `awsProfiles`, and every refusal of a read-only session (Preview, apply, console writes as the classifier calls them, unknown calls). `console.ts` runs PartiQL `SELECT` (`=`, `begins_with`, `AND`, an index), `INSERT … VALUE`, `UPDATE … SET/REMOVE … WHERE` the key ("1 row affected", `DuplicateItemException`, `ConditionalCheckFailedException`), `DELETE` ("OK", or its row and "1 row affected" with `RETURNING ALL OLD *`), and `ListTables`, the `Describe…` reads, `Scan`, `Query`, `GetItem`, `PutItem` and `DeleteItem`, answered as `run_command` answers (items as a grid, any other reply as one JSON cell).

**Tests** (`lane.mjs dynamodb ts` with the shared smoke set): `classify.test.ts` (the shared cases, the 58 actions, UTF-16 offsets and lines, linear splitting, commands and JSON as the backend reads them, a fixed-seed fuzz), `form.test.ts` (form ↔ profile for every source, validate's messages, endpoints, the summary, URL round trips for every source, a secret only as the password, refusals), `views.test.tsx` (the module, the console, the inspector and its toggle, the info panel's facts, the menu's calls and the index browser's paging, the form's rows by source with the `regions` and `awsProfiles` calls), and the mock's `index.test.ts` (caps, catalog, cursor pages to the end, a Query sorted by its sort key and the refused sorts, counts, a cell, one change and a transaction with a failing change, the console, the calls, the form's calls, the read-only refusals). Backend: `shared_console_cases` (`lane.mjs dynamodb test --snapshot sqlgen::native::dynamo`).

### Left after B2

- A session token for access keys: the shared form has no field for a driver's `SessionToken` secret (`formToProfileInput` never sets `ProfileInput.sessionToken`), so the form says to use an AWS profile or the Command source for temporary keys (a change request).
- The mock reads a native console a line at a time (the shared mock's `commandLines`), so a command written over several lines (a pretty-printed request) runs only in the app; one per line runs in the mock.
- Consumed capacity in the status bar needs a place in `BrowsePage` (the client reads it into `browse::Page::consumed`); not asked for yet.
- `ObjectMenuContext.call` treats every call as a write (`laneMenu.ts` guards it with `write: true`), which would refuse the read-only `tableInfo` and `browseIndex` on a read-only connection; `menu.ts` and `TableDialogs.tsx` send those two reads with `ipc.sessionCall` instead (both `CallKind::Read` in `session.rs`). A change request: a read form of `ctx.call` (or `write` taken from the driver's `call_kind`), after which they move onto it.

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

Known so far (Phase A and B):

- Credentials: no assume-role, SSO or web identity of its own; such profiles name `credential_process` with `aws configure export-credentials` as the way.
- Sorting: only by the sort key, in a Query (the filter fixes the partition key with `=`).
- A Query's `FilterExpression` cannot name a key attribute (DynamoDB refuses it), so a filter is a Query only when, besides the partition key equality and at most one sort key condition at the top level of an AND, it names no key attribute: a second key condition, a key condition inside a group or with another operator, an empty string as a key condition's value (DynamoDB refuses that too), Any column (the key columns are columns) or a raw expression naming a key make a Scan of the whole filter (and refuse a sort). For an index read (`browseIndex`) the rule is applied to the index's keys.
- Consistent reads (the profile's `consistentReads`) cover the table and its local indexes; a global index is always read eventually consistent (DynamoDB refuses `ConsistentRead` on one, even one that shares the table's partition key).
- A console `DELETE` reports no count unless it returns the item (`RETURNING ALL OLD *`): DynamoDB answers a missing key with success.
- Filters match case-sensitively (`contains`, `begins_with`); no suffix, case-insensitive or ILIKE matching; LIKE only as a prefix (`abc%`) or an exact value. A text value in a column seen with several types matches each of them.
- A filtered page can come back short: `Limit` counts evaluated items, and a page stops after `pageRequests` requests with a cursor.
- An exact count (or any filtered count) reads the whole table or partition with `Select: COUNT`.
- Columns come from a sample of `sampleSize` items plus each page's items, at most 500 (index keys the sample lacked come after them). The console's results show at most 500 attributes too, and at most the rows that fit 5,000,000 cells; a notice names what was left out (name the attributes in the SELECT list or a `ProjectionExpression`).
- A PartiQL statement reads at most 100 pages and stops once 128 MB of items are in (a notice says so); a grid page stops there too, with its cursor.
- A reply that would take more than 128 MB once parsed (about 170,000 small objects: a page of many tiny attributes or maps) is refused with an error asking for fewer items; a grid page of such items needs a smaller page size. DynamoDB's own pages of ordinary items stay far below it.
- A changed key attribute needs the item loaded in the grid (delete + put guarded by every loaded attribute); a commit of more than 100 items is split into several transactions, which together are not atomic. The guard of a changed key names every attribute, so an item with very many can exceed DynamoDB's 4 KB expression limit (the server refuses it; nothing is applied).
- Edits are guarded by the loaded values of the changed attributes; an item the grid did not load is guarded by its key only, and so is one forgotten past the 20,000 latest of its table, 128 MB per table or 256 MB over the session's tables.
- Map and list cells hold DynamoDB JSON of their content; plain JSON is read too, but an object whose every value looks like a typed AttributeValue is read as DynamoDB JSON.
- No cancel on the server: a request DynamoDB received runs to its end (a Scan page, a transaction); cancel stops the client's side and every later request.
- No raw filter: a FilterExpression typed by hand could name no values (they must be `:placeholders`), and the shared `RawFilter` kinds are SQL, JSON and query strings.
- Sorting is offered on every column but only the sort key of a Query is served; any other sort is refused with the reason.
- No region switch: the region is the profile's (`SwitchDatabase` is not granted).
- Completions name every table and the attributes the session has seen of it; no table is read for them.
- An item of a secondary index is never remembered for edits (an index may not project every attribute): edits of a row shown by `browseIndex` are guarded by its key alone, and its key cannot change.
- MCP clients get no DynamoDB session in v1 (`open` refuses in the MCP process).
- The connection form has no Session Token field yet (the shared form sends no `sessionToken`): temporary keys go through an AWS profile or the Command source, whose `credential_process` JSON carries the token.
- The Table Info panel and Browse Index read only; an index's items are not edited (the grid edits the table's).
