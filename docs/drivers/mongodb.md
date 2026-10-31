# MongoDB: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                   |
| ----------------- | ------------------------------------------------- |
| Lane              | `mongodb`                                         |
| Databases         | MongoDB                                           |
| Drivers           | `mongodb`                                         |
| Picker kinds      | `mongodb`                                         |
| SQL dialect       | none (native store)                               |
| Native builders   | `sqlgen/native/mongo.rs`                          |
| Cargo features    | `drv-mongodb`                                     |
| Home slot         | `s3`                                              |
| Bindings prefix   | `Mongo*`                                          |
| Lib test filters  | `drivers::mongodb`, `sqlgen::native::mongo`       |
| Real-server suite | `tests/mongodb.rs` with `TABLER_TEST_MONGODB_URL` |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/mongodb/**`
- `src-tauri/src/sqlgen/native/mongo.rs`
- `src-tauri/tests/mongodb.rs`
- `src-tauri/tests/fixtures/mongodb/**`
- `src/features/drivers/mongodb/**`
- `src/lib/ipc/mock/drivers/mongodb/**`
- `src/lib/ipc/bindings/Mongo*.ts`
- `docs/drivers/mongodb.md`

Anything else (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead. `node dev/scripts/lane-commit.mjs mongodb -m "mongodb: …"` refuses other paths.

## Scope of v1

### MongoDB

- **Identity:** `mongodb`, no dialect (MongoShell subset). Crates: mongodb 3.9.1 (no default features; compat-3-0-0, rustls-tls-aws-lc, dns-resolver).
- **UI:** document grid (sampled schema) plus `JsonDocumentEditor` inspector.
- **Auth v1:** SCRAM, X.509, PLAIN; AWS and OIDC tokens via Command. (Phase B: OIDC only; MONGODB-AWS is refused, see Phase B status.)
- **Required fields:** host or params `uri` (27017). Phase B: with neither the session connects to 127.0.0.1, as mongosh and `ConnectTarget::host` do (the shared `validate_refuses_secrets_in_params` test validates a MongoDB profile without either).
- **Out of scope v1:** replica set / SRV over SSH (directConnection only), GSSAPI, in-use encryption, full JavaScript shell, change streams.
- **Test:** BSON round trips, mongosh-subset parser, FilterGroup→query. Opt-in: `mongod.exe` zip (approval).

## Descriptor values

Seeded from the plan (§5.2). The values in force are the descriptors in `src-tauri/src/types/driver.rs` (keepalive belongs to the session's caps); a change is a request to the lead.

| Driver  | Location | Port (TLS) | SSH                  | TLS style | Secret label | Command TTL |
| ------- | -------- | ---------- | -------------------- | --------- | ------------ | ----------- |
| mongodb | Host     | 27017      | Tunnel (single node) | OnOff     | Password     | 600         |

| Driver  | Required fields / params | Label overrides | Keepalive | Read-only      | Sharing | Token secret |
| ------- | ------------------------ | --------------- | --------- | -------------- | ------- | ------------ |
| mongodb | host or `uri`            | —               | 30        | ClassifierOnly | Indep   | no           |

## Read-only and hardening

| Engine  | When read-only                                                                                                                             | When unattended (MCP) |
| ------- | ------------------------------------------------------------------------------------------------------------------------------------------ | --------------------- |
| MongoDB | Allow-list: find, aggregate without `$out` / `$merge`, count, distinct, listCollections, listIndexes, collStats, dbStats, explain of reads | refused               |

Every classifier is default-deny: a statement or command it does not know is a write. The read-only mode in the caps (`Server`, `PerRequest` or `ClassifierOnly`) must say what the session really enforces.

## What to implement

- `entry.rs` (Phase B): `open`, `validate`, `util`, `util_kind` (plan §2.5). `<Prefix>Params` (`#[serde(rename_all = "camelCase", default)]`, ts-exported) parsed from `options.driver_params`.
- `NativeSession` plus `TableSource` (plan §2.5): browse with cursors, count, fetch cell, `plan_changes` / `apply_changes` (preview and operations from one plan, text through `sqlgen/native/<x>.rs`), the console (`split_console`, `classify_console` default-deny, `run_console`), `definition()`, `call` / `call_kind` (unknown calls are writes).
- `ping` is free, `close` returns within about 2 s.
- Cancel through `CancelHandle::Custom(Arc<dyn CancelAction>)`.
- Renderer: a `KindUi` per tile, `DriverUi` with `ConsoleUi`, `TableView` and `Inspector` where the grid does not fit, and a `MockDriver`. Write-capable `sessionCall` / `driverUtil` helpers await `guardExecution` first.

## Decisions and corrections for this lane

- A native store (document grid with a sampled schema); MCP refuses it in v1 (plan A30). Phase A starts after F6 or is limited to codecs and the query parser (critique optional 16).
- The module `drivers::mongodb` shadows the `mongodb` crate in parent scopes: refer to the crate as `::mongodb::…` (critique optional 2).
- Over SSH: one node with `directConnection` (plan A27); replica sets and SRV are out of scope then.
- `src/features/documents/JsonDocumentEditor.tsx` (F9a) is shared with elasticsearch and dynamodb: this lane specifies its changes as change requests, the others configure it through props (critique B4).
- Writes prove their count from the server's `n` (plan A13). Tokens from Command mode (AWS, OIDC) may exceed the Windows Credential Manager limit (critique D8).

## Phase A status

Phase A (codecs and builders; the native session comes with F6) is done. Everything is pure code with unit tests, and an in-memory server that the `mongodb` crate itself connects to.

| File                          | What it does                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| ----------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `drivers/mongodb/values.rs`   | BSON ↔ `Value`, total and type-keeping: double → Float (NaN/±Infinity `Other{double}`), int → Int, long → Int or BigInt beyond 2^53, decimal → Decimal (exact text), date → TimestampTz `2024-05-01T10:00:00.000Z` (years 0–9999, else `Other{date}`), ObjectId `Other{objectId}`, missing `Other{missing}`, binary 0 → Bytes, 4 → Uuid, others `Other{binData}`, documents and arrays → Json (relaxed Extended JSON), rare types `Other` with canonical Extended JSON. Writes take a type hint; `text_as` reads a staged value's text (`value_text`) as a field's own type.                                                                                                                                                                                                                                                                                                                                      |
| `drivers/mongodb/literal.rs`  | JavaScript object literals and Extended JSON v2 (canonical, relaxed, legacy `$binary`) into BSON: unquoted keys, single quotes, comments, trailing commas, regex literals and the shell helpers (`ObjectId`, `ISODate`, `NumberLong`, `NumberDecimal`, `UUID`, `BinData`, `Timestamp`…). Errors carry line and column.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `drivers/mongodb/shell.rs`    | The console's mongosh subset: the splitter (`;`, or a line break after a complete expression; `.` on the next line continues) and the parser of `db.c.<method>(…)` with find's cursor chain, `explain()`, the database methods, `rs.status()`, `use`, `show`. Each statement becomes the command(s) it runs plus how to read the reply (`Output`).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `drivers/mongodb/classify.rs` | Read/write by the command a statement runs (its first key), an allow-list, default-deny: find, count, distinct, aggregate without `$out` / `$merge` at any depth, explain of those, the catalog and status commands, `use`, `show`. Parse errors, empty scripts, bulkWrite and unknown commands are writes.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `drivers/mongodb/schema.rs`   | Columns from a sample: `_id` first as the key, then fields by frequency and name; type lists (`int\|string\|null`), kind from the dominant type, nullable when null or missing.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `drivers/mongodb/catalog.rs`  | Replies as Tabler types: topology from `hello` (transactions on replica sets and mongos), version, current user, databases, collections (views, `system.*`, time series, capped), indexes (method, TTL, partial filter, definition), `$collStats` numbers, `TableMeta` with the `db.createCollection(…)` definition and the validator as a check, cursor batches and counts.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `drivers/mongodb/errors.rs`   | Driver and reply errors: `Interrupted` → Cancelled, `MaxTimeMSExpired` → Timeout, auth → Auth, transport → NotConnected, selection/DNS/TLS → Connect, server errors → Db with the code in `vendor_code` and the code name in `detail`; write errors of `ok: 1` replies.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `drivers/mongodb/testing.rs`  | `FakeMongo`: in-memory databases answering the commands above (queries, `$expr` with `$eq`/`$literal`/`$$ROOT`, cursors and `getMore` in their own session, batches cut at 16 MiB or a test's lower cap, updates, unique indexes, views, `$currentOp`/`killOp`, injected failures and delays), served over OP_MSG on a localhost port.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `sqlgen/native/mongo.rs`      | Extended JSON writer (relaxed text reads back to the same types), catalog and cursor commands, the grid's `find` (Phase A read it whole with `PageRead`; Phase B reads it through the crate's cursor, which pins each `getMore` to the member that opened it, and dropped `PageRead`), sorts and projections, the filter bar as a query document (values matched as every type the column holds; ranges by number or date; LIKE and contains as escaped regexes; raw filters in shell syntax), grid edits by `_id` expecting `n: 1` that keep each field's loaded type and binary subtype (`LoadedTypes`; a value staged as another type, by text or by its column's kind, is written in the field's type when it is exactly one and refused otherwise), whole-document replacements matched by `$$ROOT` against the original as a `$literal`, collection and index DDL text, and the console's command builders. |

Tests: 104 lane tests (`drivers::mongodb`, `sqlgen::native::mongo`), among them a test where the real driver connects to `FakeMongo` (ping, a page, a cursor over several batches, `insert_many` document sequences, `count_documents`, a cancel through `$currentOp` + `killOp`, a `maxTimeMS` timeout), a classifier list of reads, writes and red-team cases, and a fixed-seed fuzz of the splitter and classifier. `test-all` passes.

Left for Phase B (done, see Phase B status): `entry.rs` (`MongoParams`, `open` with `ClientOptions`, `validate`, `util`), the `NativeSession` and `TableSource` over `Database::run_command` (browse pages through `find` read on through the crate's cursor, whose `getMore`s stay in the session and on the member that opened it, counts, `fetch_cell`, `plan_changes` / `apply_changes` with a transaction on replica sets and mongos and per-change results on standalone servers, the console with `max_rows` and `getMore`, cancel through `comment` + `killOp`), the renderer (`KindUi`, form, URL import, console and document view), the mock and `tests/mongodb.rs`.

## Phase B status

Part B1, the backend driver, is done: `entry.rs` opens a `NativeSession` with its `TableSource`, and every method talks to the server through the `mongodb` crate. The renderer, the mock and URL import are part B2.

| File                               | What it does                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| ---------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `drivers/mongodb/entry.rs`         | `open` (refused in the MCP process), `validate` (the params against the profile), `util` (no form calls: every one unknown), `util_kind` (`Write`).                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `drivers/mongodb/params.rs`        | `MongoParams` (ts-exported with `MongoAuth`, `MongoReadPreference`): `uri`, `authMechanism`, `authSource`, `replicaSet`, `directConnection`, `readPreference`, `appName`, `serverSelectionTimeoutSecs`, `sampleSize`; the checks before connecting (ranges, X.509 needs TLS and a certificate, PLAIN a user, no MONGODB-AWS or GSSAPI, SSL Off refused when the URL asks for TLS, no secret among the URL's options, over SSH one node which is the profile's host and port); `UriParts`, a light reading of a URL for those checks. Without a host or a URL the session connects to 127.0.0.1, as mongosh does. |
| `drivers/mongodb/connect.rs`       | The crate's `ClientOptions`: the URL (`ClientOptions::parse`, SRV included) or the form's host; over SSH the tunnel's address with `directConnection`; one host without a replica set name connects directly (as mongosh); the connect timeout for connecting and server selection; retryable writes off (the commit decides). TLS by SSL mode; the sign-in (profile user over the URL's, `$external` for X.509, PLAIN and OIDC, else `admin`); `Login` (signs in anew with a Command Line password's fresh output) and `TokenFeed` (the OIDC callback's tokens).                                                |
| `drivers/mongodb/session.rs`       | `MongoSession`: caps, `server_info` (`buildInfo`, `connectionStatus`), `ping` (`{ping: 1}`, untraced like the core drivers' keep-alive), `close` (immediate shutdown within 2 s), introspection, `definition`, browse, count, `fetch_cell`, `plan_changes` / `apply_changes`, the cancel (`KillOp`), the console hooks, calls and processes. Commands the app sends for itself are traced as console text.                                                                                                                                                                                                       |
| `drivers/mongodb/console.rs`       | One console statement: a cursor streamed in chunks with the row cap (columns from its first 200 documents, a notice naming fields that come later), one reply, value or count as a row, a write's counts as affected documents (`matched, modified, upserted` for updates), `use`, and `bulkWrite` (stops at the failing command when ordered and says which).                                                                                                                                                                                                                                                   |
| `drivers/mongodb/calls.rs`         | `session_call`: reads `collectionStats`, `databaseStats`, `preview` (the command a write call would run, for the menus' confirmations); writes `createCollection`, `createView`, `dropCollection`, `truncateCollection`, `renameCollection`, `createIndex`, `dropIndex`, `setValidator`, `dropDatabase` (each answers its command text and reply; unknown calls are writes). Running Queries: `$currentOp` (every user's, or the user's own without `inprog`), `killOp`.                                                                                                                                         |
| `drivers/mongodb/session_tests.rs` | The session against `FakeMongo` served over the wire (below).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `drivers/mongodb/testing.rs`       | `FakeMongo` gained transactions (`startTransaction` snapshots, `abortTransaction` restores), access control (SASL PLAIN and MONGODB-OIDC, `ReauthenticationRequired` on demand), dropped connections (a restart), `lsid`, `appName` and `microsecs_running` in `$currentOp`, and failures with `errorLabels` (`fail_next_labelled`, also on `commitTransaction`).                                                                                                                                                                                                                                                |
| `sqlgen/native/mongo.rs`           | Added `with_comment`, `current_op_matching` (by comment and session), `current_ops`, `drop_database`, `set_validator` with level and action; removed `PageRead`, `get_more`, `kill_cursors` and `FindSpec::max_time_ms` (the crate's cursors do that work).                                                                                                                                                                                                                                                                                                                                                      |
| `tests/mongodb.rs`                 | The real-server suite (`TABLER_TEST_MONGODB_URL`, a connection string whose password goes to the password field): connect, list, browse with paging, count, fetch, a commit with a failing change (rolled back on a replica set, partial on a standalone server), the console, a cancel and the query time out on a slow `$where`, and a read-only session's refusals. It works in a scratch database it drops.                                                                                                                                                                                                  |

**Caps granted** (`session::capabilities`, starting from `SessionCaps::minimal`), each proven by `session_tests` against `FakeMongo` served over the wire, which the real crate connects to:

| Grant                                                              | Why it holds                                                                                                                        | Test                                                                                                                      |
| ------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `StructureView`, `ViewDefinition`                                  | `table_meta` (sampled columns, indexes, validator, estimate) and `definition` (create and index text)                               | `connects_lists_and_says_what_it_offers`, `values_keep_their_types_in_the_grid`                                           |
| `Editor`                                                           | the console runs the mongosh subset                                                                                                 | `the_console_runs_statements_with_a_row_cap`                                                                              |
| `CancelOnServer`                                                   | every run carries Tabler's comment and session; the cancel stops waiting and `killOp`s it                                           | `a_cancel_kills_the_operation_and_a_time_out_stops_the_run`, `a_server_without_sessions_is_read_and_cancelled_by_comment` |
| `EditRows`, `InsertRows`, `DeleteRows`                             | commands by `_id`, each checked for `n: 1`; views and system collections refused                                                    | `a_standalone_server_applies_changes_one_after_the_other`, `a_replica_set_commits_in_one_transaction`                     |
| `ServerExpressions` (`now`, `currentTimestamp`)                    | `$currentDate` in updates (inserts take values only)                                                                                | `a_server_expression_sets_the_servers_date`                                                                               |
| `ExactCount`, `CountMode::Estimate`                                | the collection's own count unless asked, `countDocuments` for exact and filtered counts                                             | `browses_by_page_counts_and_fetches_cells`                                                                                |
| `SwitchDatabase`                                                   | a new session on the chosen database (the profile's `database`)                                                                     | `connects_lists_and_says_what_it_offers` (`list_schemas` is that database)                                                |
| `ProcessList`, `KillProcess`                                       | `$currentOp` and `killOp`; Kill Connection is refused per id                                                                        | `running_queries_are_listed_and_killed`                                                                                   |
| `Paging::Offset`, `sort`, 23 filter operators, `RawFilter::Json`   | `find` with `skip` / `limit`, `_id` breaking ties; the filter bar as a query document, values never read as operators               | `browses_by_page_counts_and_fetches_cells`, `pages_of_large_documents_come_whole`                                         |
| `TxMode::Atomic` (replica set, mongos) / `Sequential` (standalone) | one transaction where the server has them; otherwise in order, stopping at the first failure, with a result per change              | the two commit tests                                                                                                      |
| `Affected::Exact`                                                  | every write checks the reply's `n`                                                                                                  | the commit tests                                                                                                          |
| `ReadOnlyMode::ClassifierOnly`, `hardened: false`                  | MongoDB has no read-only session; the session refuses edits, console writes (default-deny classifier), write calls and kills itself | `read_only_refuses_every_write_path`                                                                                      |
| `SafeConfirm::Password` / `Passcode`                               | Safe Mode asks for the password, or the app passcode when a Command Line prints it (`commands::safety::confirms_with_passcode`)     | `connects_lists_and_says_what_it_offers`, `a_rotated_command_password_signs_in_again`                                     |
| `keepalive_secs: 30`, `ping_billed: false`                         | `{ping: 1}` costs nothing                                                                                                           | `connects_lists_and_says_what_it_offers`                                                                                  |

Not granted: `EditWithoutKey` (documents are addressed by `_id`), `Returning`, `StructureEdit`, `CreateTable`, `DropTable`, `TruncateTable`, `RenameTable`, `TableDdl` and the database dialogs (their commands build SQL; the lane's calls do these for the renderer's menus), `Explain` (the console's `explain()` does it), exports, imports, backups, `Users`, `SessionDetails`, `Metrics`, `Assistant`, `Mcp`.

Tests: 140 lane tests (`drivers::mongodb`, `sqlgen::native::mongo`): value mapping (`9007199254740993` → `BigInt`, Decimal128 → `Decimal` text, dates → `TimestampTz` text, nested → `Json`, missing fields, ObjectId), the filter bar's values never read as operators, the console's classifier and red-team cases, and the session over the wire: connect, list, structure, browse with paging (pages cut at the batch size come whole), sort, filter, count, fetch, commits with a failing change (standalone and replica set) and a failed `commitTransaction` (outcome unknown, or aborted), the console (row cap, `use`, `show`, `bulkWrite`, late fields), a cancel and the query time out, a stale cancel that leaves the next operations alone, `maxTimeMS`, read-only refusals on every write path, processes, calls, a rotated Command Line password, an OIDC token fetched again, bad credentials and MCP.

**Deliberate differences from TablePlus, and limits of B1:**

- Paging is by offset (`skip` / `limit`), as TablePlus pages, not by cursor: a sort on any field over values of mixed types cannot be resumed from a key (MongoDB compares across types by type order, and a range query brackets types), and a server cursor kept between pages dies after ten idle minutes. A deep page costs the server the skipped documents.
- A standalone server has no transactions: a commit applies its changes in order and stops at the first that fails; the ones before it stay applied (the renderer unstages them).
- A `commitTransaction` that fails without the server saying it aborted the transaction (`TransientTransactionError`), such as a lost reply or a write concern error, is reported as of unknown outcome: the changes may have been applied, so the message says to refresh before committing again (an insert without an `_id` would be added twice).
- Sign-in: SCRAM, X.509, PLAIN and MONGODB-OIDC (machine flow, the token in the password). MONGODB-AWS needs the crate's heavy `aws-auth` feature and GSSAPI its `gssapi-auth`; both are refused. A Command Line password is run again only when the server refuses the old one. A command Tabler sends for itself (reads, counts, a commit, collection calls, `killOp`) is then retried once on the new sign-in: sign-in happens before the command is sent, so nothing runs twice. A console statement is retried only when it reads; a console write reports the refusal.
- TLS: `verify-ca` checks the host name too (rustls cannot skip it); `prefer` leaves TLS to the URL (off without one). Off (the form's first value) with a URL that asks for TLS (`tls=true`, `ssl=true`, `mongodb+srv://`) is refused rather than sent in plain text. Secrets are refused among the URL's options (`tlsCertificateKeyFilePassword`, `proxyPassword`, an `AWS_SESSION_TOKEN`): the URL is saved in plain text with the profile. Option names are matched as the crate reads them (Unicode lower case, so U+212A KELVIN SIGN counts as `k`), percent-decoded too; and `connect` refuses a key password the crate itself read from the URL, whatever the spelling. The client certificate and its key are one PEM file, as MongoDB's `tlsCertificateKeyFile`. The key's password is read from the profile's second secret, which the descriptor does not list yet (change request). Over SSH only `require` (no verification) works: the crate would check the certificate against the tunnel's address (the open TLS exemption request).
- Over SSH one node only (`directConnection`): no SRV, no seed list, and a URL's host and port must be the profile's (the tunnel goes to those, as the SSH server sees them).
- The cancel finds an operation by its comment (or the user's own `.comment(…)` by its session) on the primary, and keeps that session out of the crate's pool until it is done (the pool hands the session returned last to the next operation, which a stale match would kill): with a non-primary read preference on a replica set the server-side kill can miss a read on a secondary; the run still stops at once. The cancel lists `$currentOp` again for up to 2 s, as the operation may reach the server after the cancel; one that arrives later still runs to its end (or its `maxTimeMS`). The whole kill is capped at 5 s (`a_server_that_stalls_the_kill_does_not_hold_a_timed_out_run`): a server that stalls `$currentOp` or `killOp` does not hold the cancelled run or the query time out, which then stop here only.
- The console's `use` is the connection's, shared by its tabs. Result columns come from a cursor's first 200 documents; later fields are named in a notice, not shown.
- Running Queries kills operations (`killOp`); there is no Kill Connection.
- Counts without a filter are the collection's own count, reported as not exact (it can drift after an unclean shutdown or on a sharded cluster); the exact count runs on request.
- Only `NOW()` and `CURRENT_TIMESTAMP` are offered as cell expressions (`$currentDate`); BSON dates have no date-only or time-only form.

**What only a real server can confirm** (`tests/mongodb.rs`, and by hand): TLS handshakes with a CA file, a client certificate and an encrypted key; SRV resolution; SCRAM, X.509 and OIDC against real identity setups; transactions on a replica set and mongos (and transient transaction errors); `killOp` and `$currentOp` privileges (Atlas, `inprog`); mongos operation ids (`shard:opid`); `$sample` on views and on DocumentDB, Cosmos DB and FerretDB; a secondary read preference with a cursor's `getMore`.

## Research notes

> Background from the driver research (October 2026), with file references and no line numbers. Verify against the code; where it differs from the scope above or from README.md, those win.

### MongoDB

**Approach.**

Use the official `mongodb` crate 3.9.1 (MSRV 1.88; supports servers 4.4+). Configure it as `mongodb = { version = "3.9.1", default-features = false, features = ["compat-3-0-0", "rustls-tls-aws-lc", "dns-resolver"] }`. Optional features: `"socks5-proxy"` for SSH, and `"snappy-compression"`/`"zlib-compression"` (pure Rust). Use BSON through the re-exported `mongodb::bson`: bson 2.x via compat-3-0-0, or add `bson-3` for bson 3.1.0.

Connecting:

- For URL profiles, `ClientOptions::parse(uri)` resolves mongodb+srv through hickory DNS.
- For form profiles, build ClientOptions directly (hosts, credential, repl_set_name, direct_connection, tls, app_name, connect_timeout, server_selection_timeout).
- Then `Client::with_options`.

APIs per job:

- Introspection: `Database::run_command(doc!{..})`.
- Grid: `Collection::<Document>::find(filter).sort().skip().limit().projection().max_time()` and `aggregate`.
- Editor: the text is parsed into a command document and run with `Database::run_command` or `run_cursor_command` (cursor-returning commands such as find/aggregate/listIndexes).
- Transactions: `ClientSession::start_transaction` on replica sets and mongos.
- Cancel: tag every operation with `comment: 'tabler-<uuid>'`, and on cancel find it with `$currentOp {ownOps:true}` and run `killOp`. Also cap reads with maxTimeMS.

Why: it is the official driver, async on tokio, can select rustls with aws-lc-rs, has SCRAM/X.509/OIDC/PLAIN built in, and handles SRV/replica-set discovery.

**Windows build.**

Never keep the default features. Default `rustls-tls` enables `rustls/ring` and `tokio-rustls/ring`. With both ring and aws-lc-rs enabled on rustls, `CryptoProvider::get_default_or_install_from_crate_features()` can no longer pick a provider. That breaks every crate that calls `ClientConfig::builder()` without an explicit provider, such as redis-rs TLS and possibly reqwest, unless the app installs a default at startup. Today that is done only ad hoc (commands/conn_extras.rs, drivers/mysql/mod.rs).

With the chosen features everything is pure Rust: hickory-resolver 0.26 (reads the Windows DNS config itself), rustls 0.23 + webpki-roots, sha1/sha2/hmac/pbkdf2/md-5 for SCRAM, stringprep (already in the lock), typed-builder, derive-where. It is heavy: roughly 1-2 min for a clean debug build at the repo's opt-level 1 for dependencies, and 6-10 MB added to the release binary.

Avoid:

- `zstd-compression`: zstd-sys is C; it builds with cc on MSVC but slowly.
- `in-use-encryption`: needs the libmongocrypt C library.
- `openssl-tls`: no system OpenSSL on Windows.
- `aws-auth`: pulls aws-config and aws-sigv4, which is heavy.
- `gssapi-auth`: cross-krb5 uses SSPI on Windows, which works but should be deferred.

TLS roots are webpki-roots (Mozilla) unless `ca_file_path` is set; the Windows/corporate certificate store is not consulted. TlsOptions only accepts file paths (ca_file_path, cert_key_file_path). `allow_invalid_hostnames` exists only with openssl-tls, so with rustls a TLS connection through the SSH tunnel's 127.0.0.1 fails the hostname check. Use SOCKS5 (`socks5-proxy`, pure-Rust fast-socks5) through SSH so the driver keeps the real host name.

**Authentication.**

TablePlus offers user/password (SCRAM), URL import (b200) whose authSource/authMechanism params are the only way to reach advanced auth (#3647 asks for form fields), and SSL + PEM certificates (b428). Its MONGODB-AWS only tries the EC2 metadata endpoint (#3816). SSH works for a single host only, not replica sets (#1390).

Tabler should offer:

- Default mechanism negotiation (SCRAM-SHA-256, SCRAM-SHA-1), with the existing password modes.
- X.509: client PEM cert+key, authSource $external.
- MONGODB-OIDC: built into the crate.
- PLAIN (LDAP): built in.
- GSSAPI/Kerberos: optional feature.
- MONGODB-AWS: needs the heavy `aws-auth` feature. Alternatively, supply the credential through the existing 'command' password mode.
- An authentication-database field.
- TLS: CA file, client cert/key with key password (`cert-key-password` feature: pem/pkcs8, pure Rust), allow invalid certificates.
- SSH tunnel, using directConnection for one host or SOCKS5 for replica sets and SRV.

**Data model.**

How TablePlus shows MongoDB:

- The sidebar lists databases and their collections.
- A collection opens a spreadsheet grid whose columns are the top-level fields; keys are sorted since b222, and users complain that `_id` comes last (#928). Nested documents and arrays show as raw JSON cells, edited in the JSON editor or Quick Look (#928, #1103).
- The right sidebar shows row details.
- The filter bar (Cmd+F) has a 'Raw SQL' row that takes a JSON filter fragment such as `"field": {"$regex": "x", "$options": "i"}` (#1516).
- There is no document or tree view (requested in #2763 and #2247).
- Truncate collection (b388) and drop collection (b376) are supported; create table is 'not available for this driver' (#2667).
- An 'Old' dialect for MongoDB 3.6/4.0 arrived in 27.0.2.

How MongoDB maps onto DbSession:

- list_databases → listDatabases (authorizedDatabases: true).
- list_schemas → the current database only (MySQL pattern, drivers/mysql/introspect.rs). switch_database (commands/connections.rs) could take a fast path, since a new Database handle on the same Client is enough.
- list_tables → listCollections. Collections and timeseries become TableKind::Table (comment 'time series'), views become View, and system.\* become SystemView. row_estimate comes from estimatedDocumentCount, loaded lazily.
- table_meta → inferred columns:
  - `_id` first, as the PK: RowIdentity::Pk, pk_columns ['\_id'].
  - Then the top-level fields from a `$sample` of N documents, ordered by frequency then name.
  - data_type lists the BSON type(s) seen ('objectId', 'string|null'); kind comes from the dominant type.
  - Embedded documents and arrays get one Json column each.
  - indexes come from listIndexes: IndexMeta.columns = the key fields; method = 1/-1/text/2dsphere/hashed/wildcard; predicate = the partialFilterExpression JSON; unique.
  - comment and checks show the $jsonSchema validator.
  - ddl = the `db.createCollection(…)` text.
- view_definition → `db.createView(name, viewOn, pipeline)`.
- completion_metadata → collection → sampled field names.

BSON → Value mapping (types/value.rs):

- Double → Float; Rust's shortest round-trip Display avoids TablePlus's '1847.2699999999999818' (#3919).
- String → Text; Boolean → Bool; Null → Null.
- Int32 → Int; Int64 → Int, or BigInt beyond 2^53.
- Decimal128 → Decimal.
- DateTime → TimestampTz (ISO 8601 with milliseconds, Z).
- ObjectId → `Other{ty:'objectId', text:hex}`. It must round-trip exactly: string vs ObjectId `_id` was TablePlus bug #1183.
- Binary subtype 4 → Uuid; other subtypes → Bytes.
- Embedded document / Array → Json (relaxed Extended JSON).
- Regex, Timestamp, JavaScript, MinKey/MaxKey, Symbol, DbPointer, Undefined → Other carrying the canonical Extended JSON.
- A missing field is not NULL. Value has no variant for it: add `Missing` or use a dedicated Other marker.

New UI beyond the grid:

- Document view: a JSON tree per document, toggled against the grid (TablePlus's most-asked Mongo feature).
- Insert/Edit Document in Extended JSON, reusing src/features/datagrid/editors/JsonEditorDialog.tsx and QuickLook.
- A Structure tab with inferred fields (type distribution %), indexes (create/drop, TTL, unique, partial), the validator viewer/editor and $collStats numbers.
- Query results as a grid or a JSON tree; aggregation results are often nested.
- Users/roles (usersInfo/rolesInfo) and currentOp/killOp as admin panes later.

**Query language.**

Two input forms, detected automatically.

(1) TablePlus-compatible database-command documents, for example `{"find": "orders", "filter": {"status": "A"}, "limit": 50}` or `{"count": "c", "query": {...}}`. TablePlus requires strict JSON and so chokes on `ISODate` (#3074); Tabler should parse relaxed/Extended JSON. These run through `Database::run_command`, or `run_cursor_command` for find/aggregate/list\*.

(2) A mongosh subset. TablePlus added 'basic MongoShell syntax' in b512 (Oct 2023) and a JS 'Mongo Shell' in 26.10 (Sep 2026):

- `db.coll.find(filter, projection)` with `.sort().skip().limit().project()`.
- findOne, countDocuments, estimatedDocumentCount, distinct, `aggregate([...])`, explain().
- insertOne/insertMany, updateOne/updateMany, replaceOne, deleteOne/deleteMany, findOneAndUpdate/Replace/Delete, bulkWrite.
- createIndex/dropIndex/getIndexes, drop, renameCollection.
- `db.getCollection('a.b')`, `db.runCommand({...})`, `db.adminCommand`, `db.getSiblingDB('x')`.
- `show dbs|collections`, `use x`.

Arguments are JS object literals: unquoted keys, single quotes, trailing commas, comments. Shell helpers are supported: ObjectId(), ISODate()/new Date(), NumberLong/NumberInt/NumberDecimal, Long/Int32/Double/Decimal128, UUID(), BinData(), Timestamp(), /re/flags, MinKey/MaxKey. Parse them with a hand-written tokenizer into bson::Document; the json5 crate cannot handle function calls. Generated text goes in a new module such as src-tauri/src/sqlgen/mongo.rs.

Full JS (variables, loops, functions) needs an embedded engine: boa_engine (pure Rust, large compile) or rquickjs (QuickJS C via cc). Defer it.

Splitting: top-level `;` or a newline after a balanced call, in a new splitter beside sqlgen/split.rs.

Safe mode: classify methods as read (find/aggregate without $out/$merge/count/distinct) or write.

Editor: CodeMirror with @codemirror/lang-javascript (new dependency) instead of lang-sql (cmExtensions.ts).

**Editing.**

The grid's StagedChange values are planned into commands. Preview shows them in TablePlus's runCommand style (#1183): `{"update": coll, "updates": [{"q": {"_id": <exact original _id>}, "u": {"$set": {...}, "$unset": {...}}, "multi": false}]}`. expect_affected becomes a check that the reply's `n == 1`, mirroring check_affected (drivers/mod.rs).

- Insert → `insert` with documents; the driver adds an ObjectId `_id` when one is missing.
- Delete → `delete` with `[{q: {_id}, limit: 1}]`.
- Edit Document (whole document) → `findAndModify`/replaceOne with the original document or its fields as the filter, for an optimistic check.

Types are preserved per cell: keep the original BSON type of each loaded field, so an Int32 stays Int32 and a string `_id` stays a string. Json cells are parsed as relaxed Extended JSON (`Bson::try_from(serde_json::Value)`). Nested edits use dotted `$set` paths.

Commit: a ClientSession transaction when hello reports setName or msg 'isdbgrid'. On standalone servers, an ordered bulk with per-row errors that is not atomic; tell the user.

Collection DDL:

- create (capped, timeseries, validator, collation, clusteredIndex). TablePlus cannot.
- drop (`drop`), rename (`renameCollection` on admin), truncate (`delete` with `q: {}`), as TablePlus has.
- createIndexes/dropIndexes, `collMod` (validator, TTL, hidden index), createView.

Views and system.\* collections are read-only.

**Introspection.**

Server:

- `{hello: 1}` → isWritablePrimary, setName, msg 'isdbgrid' for mongos, maxWireVersion.
- `{buildInfo: 1}` → version.
- `{connectionStatus: 1}` → authenticatedUsers, for ServerInfo.current_user.

Databases: `{listDatabases: 1, nameOnly: false, authorizedDatabases: true}` → name, sizeOnDisk, empty. When not authorized, fall back to profile.database.

Collections: `{listCollections: 1, nameOnly: false, authorizedCollections: true}` → name, type (collection/view/timeseries), options (viewOn, pipeline, validator, validationLevel, capped, size, max, timeseries, clusteredIndex, collation), info.readOnly/uuid, idIndex.

Indexes: `{listIndexes: '<coll>'}` → key, name, unique, sparse, partialFilterExpression, expireAfterSeconds (TTL), hidden, collation, weights/default_language (text indexes), 2dsphereIndexVersion, wildcardProjection.

Stats: `{aggregate: '<coll>', pipeline: [{$collStats: {storageStats: {}, count: {}}}], cursor: {}}` → count, size, storageSize, totalIndexSize, avgObjSize. The collStats command is deprecated since 6.2.

Counts:

- Fast: `{count: '<coll>'}` without a query (= estimatedDocumentCount, from metadata).
- Exact or filtered: `countDocuments`, which aggregates `[$match, $group {n: $sum 1}]`.

Schema sample: `{aggregate: '<coll>', pipeline: [{$sample: {size: 100}}], cursor: {}}`, falling back to `find` with limit 100 for views and DocumentDB/Cosmos quirks.

Other: `{dbStats: 1}`. Admin later: `{usersInfo: 1}`, `{rolesInfo: 1}`, `$currentOp {ownOps: true}` + `{killOp: 1, op}`, `{serverStatus: 1}` for metrics.

Rust calls: `Client::list_databases().authorized_databases(true)`, `Database::list_collections()` (CollectionSpecification), `Collection::list_indexes()` (IndexModel), `Database::run_command`, `Database::run_cursor_command`.

**Testing without a server.**

Rust has no embedded MongoDB, so unit-test the pure parts:

- BSON ↔ Value round trips for every BSON type: Int64 above 2^53, Decimal128, UUID subtypes 3 and 4, DateTime before 1970, ObjectId.
- Extended JSON relaxed vs canonical.
- Golden tests for the mongosh-subset parser → command Document.
- FilterGroup (types/browse.rs) → query document: Eq → {f: v}, Ne → $ne, ranges, Like/Contains → $regex with escaping, In → $in, IsNull → {f: null} or $exists, Raw → a JSON fragment.
- Schema inference over fixture documents.
- Command plans for StagedChange.

Integration:

- `mongo:8` in a new `mongo` profile of dev/docker-compose.yml, started with `--replSet rs0` + rs.initiate so transactions work.
- No Docker needed on Windows: the MongoDB Community zip (`mongod.exe --dbpath`).
- FerretDB on the already-running local PostgreSQL gives a wire-compatible target.
- Atlas free tier for SRV and TLS.

**What it shares with an existing driver.**

Not wire-compatible with PG, MySQL or SQLite (OP_MSG over BSON), and no sqlgen SQL applies. The same driver serves:

- Amazon DocumentDB: TLS with the global-bundle.pem CA, retryWrites=false, limited aggregation stages, no transactions on elastic clusters.
- Azure Cosmos DB for MongoDB: RU throttling, subset of commands.
- FerretDB 2.x: partial command set.
- Percona Server for MongoDB.

Servers older than 4.4 are not supported by crate 3.x; TablePlus needed an 'Old' dialect for 3.6/4.0.

**Connection fields.**

- Name / color / tag / group (existing)
- Connection URL (mongodb:// or mongodb+srv://) — source of truth for advanced URI options; add both schemes to src/features/connections/url.ts
- Host(s) and port (default 27017); several host:port pairs for a replica-set seed list
- Use SRV record (mongodb+srv)
- Username
- Password + storage mode (keychain/ask/none/command)
- Authentication database (authSource; default admin, $external for X.509/AWS/OIDC)
- Auth mechanism (Default, SCRAM-SHA-256, SCRAM-SHA-1, X.509, MONGODB-OIDC, PLAIN, GSSAPI, MONGODB-AWS)
- Default database (empty = list all databases)
- Replica set name
- Read preference (primary, primaryPreferred, secondary, secondaryPreferred, nearest)
- Direct connection (forced on for a single-host SSH tunnel)
- TLS on/off
- TLS CA file (PEM)
- TLS client certificate+key file (PEM) and key password
- Allow invalid certificates
- App name
- Connect timeout / server selection timeout
- Schema sample size (documents sampled to infer columns, default 100)
- SSH tunnel (existing SshConfig); replica sets/SRV over SSH need SOCKS5
- Read only / safe mode / startup commands (existing ProfileOptions)

**Effort:** XL.

**Risks.**

- Crypto-provider clash: the crate's default `rustls-tls` enables ring next to the app's aws-lc-rs. Use `default-features = false` with `rustls-tls-aws-lc`, or install a process default provider at startup.
- SSH plus TLS plus replica sets: rustls cannot disable hostname checks (allow_invalid_hostnames is openssl-only), and a tunnel to 127.0.0.1 breaks SNI. Replica sets and SRV need several forwards (TablePlus #1390). Plan a SOCKS5 listener over russh direct-tcpip channels (tunnel/mod.rs) with the `socks5-proxy` feature.
- Shell expectations: users expect full mongosh JavaScript, which TablePlus added in Sep 2026. A subset parser will reject variables and loops, and an embedded JS engine is a large dependency.
- Column inference from a sample misses rare fields and mixed types per field. The grid needs 'missing' vs null semantics, and Value has no Missing variant.
- Exact type round-trips: string vs ObjectId `_id` (TablePlus #1183), Int32 vs Int64 vs Double (an edit must not change the type), Decimal128, UUID subtype. Loss here corrupts application data silently.
- Transactions exist only on replica sets and mongos. A multi-row commit on standalone servers is not atomic.
- Users without the listDatabases or listCollections privilege (common on Atlas) need the authorizedDatabases/authorizedCollections options plus a fallback to the profile database.
- Compatibility gaps on DocumentDB and Cosmos DB ($sample, $collStats, killOp, sessions), and no support for servers before 4.4.
- MONGODB-AWS needs the heavy aws-auth feature (TablePlus #3816). CSFLE/queryable encryption needs the libmongocrypt C library; out of scope.
- The Mozilla webpki roots are compiled in: corporate MITM proxies or private CAs need an explicit CA file, because the Windows certificate store is not used.
- Very large (16 MB) or deeply nested documents in grid cells. Cap Json previews and load the full value on demand (fetch_cell).

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._

Known so far (Phase A):

- The console runs a mongosh subset without JavaScript: no variables, functions, loops, `forEach` / `map` callbacks, `watch` or `mapReduce`.
- Columns come from a sample (100 documents by default): rare fields can be missing from the grid. Filter-bar `is null` matches null and missing fields, as MongoDB's `{f: null}` does.
- Top-level field names with a `.` or a leading `$` cannot be edited cell by cell (`$set` would read a path or an operator); edit the whole document instead. Filters and sorts on them follow MongoDB's path reading.
- Nested values over 64 KiB show as a cut preview in the grid and are loaded whole on demand; a preview is never written back.
- A grid edit keeps the type each field had in the loaded document, and a binary its subtype; a cell cannot change a field's type (edit the whole document), only set it to NULL. The shared editor stages a cell by its column's dominant kind, so in a column that mixes types a value can come as another type than the field's: it is written in the field's type when it is exactly a value of it (7.0 over an Int32 writes the Int32 7, a decimal 7 over a long the long 7, 7 over a string the string "7", text over an ObjectId or a date that type), and refused otherwise (7.5 or 3000000000 over an Int32, a number over an ObjectId, JSON `5` over an object, a cleared cell or text without a digit over a decimal, which is not 0). Text typed into a missing or null cell takes the column's type, and is refused when it is not one.
- Binaries: in a binary column a cell is edited as hex, which keeps the field's subtype (a legacy subtype-3 UUID stays subtype 3). In a text or mixed column the text spellings `BinData(n, '…')` and `UUID('…')` are read; they must have the field's subtype, so `UUID('…')` over a generic binary is refused. A UUID field (subtype 4) takes only a UUID. A cut binary preview is never written back.
- Relaxed Extended JSON cannot tell a nested document whose only keys look like a type wrapper (`{"$date": 5}`, `{"$oid": "…"}`, `{"$numberLong": "…"}`) from that type: a Json cell holding one reads back as that type, or fails to parse. Such documents are kept only when the cell is not edited; when the cell's whole value is one, the edit is refused (it would change the field's type).
- `CURRENT_DATE` and `LOCALTIMESTAMP` as cell values write `$currentDate`, the server's full UTC date and time (BSON dates have no date-only or local form); `CURRENT_TIME` and `DEFAULT` are refused.
- A whole-document replacement matches only while the document is still equal to the one loaded (`$$ROOT` against it as a `$literal`); that equality compares numbers by value, so a change of a number's type alone by someone else is not noticed.
