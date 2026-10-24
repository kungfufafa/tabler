# Elasticsearch and OpenSearch: lane brief

> Seeded by the foundation (step F3b) from the driver plan and the research notes. The lane keeps it current and turns it into the driver's final doc before it lands: what works, the limits, how it was tested. Rules, tooling and the definition of done are in [README.md](README.md).

|                   |                                                               |
| ----------------- | ------------------------------------------------------------- |
| Lane              | `elasticsearch`                                               |
| Databases         | Elasticsearch, OpenSearch                                     |
| Drivers           | `elasticsearch`                                               |
| Picker kinds      | `elasticsearch`                                               |
| SQL dialect       | none (native store)                                           |
| Native builders   | `sqlgen/native/elastic.rs`                                    |
| Cargo features    | `drv-elasticsearch`                                           |
| Home slot         | `s3`                                                          |
| Bindings prefix   | `Elastic*`                                                    |
| Lib test filters  | `drivers::elasticsearch`, `sqlgen::native::elastic`           |
| Real-server suite | `tests/elasticsearch.rs` with `TABLER_TEST_ELASTICSEARCH_URL` |

## Owned files

Everything the lane may change (dev/lanes.json is authoritative):

- `src-tauri/src/drivers/elasticsearch/**`
- `src-tauri/src/sqlgen/native/elastic.rs`
- `src-tauri/tests/elasticsearch.rs`
- `src-tauri/tests/fixtures/elasticsearch/**`
- `src/features/drivers/elasticsearch/**`
- `src/lib/ipc/mock/drivers/elasticsearch/**`
- `src/lib/ipc/bindings/Elastic*.ts`
- `docs/drivers/elasticsearch.md`

Anything else (a hook, a cap, a descriptor value, a dependency, a feature) is a change request to the lead. `node dev/scripts/lane-commit.mjs elasticsearch -m "elasticsearch: …"` refuses other paths.

## Scope of v1

### Elasticsearch

- **Identity:** `elasticsearch`, no dialect (EsConsole plus `_sql`). Crates: none (common-http, common-aws).
- **UI:** hits grid (flattened mapping) plus JSON inspector.
- **Auth v1:** none, Basic, API key, Bearer, SigV4 (OpenSearch service `es` / `aoss`).
- **Required fields:** host or params `urls` / `cloudId` (9200).
- **Out of scope v1:** node sniffing, ES|QL, ILM and template editing, atomic bulk.
- **Test:** fake HTTP replay (`/`, `_cat/indices`, `_mapping`, PIT pages, `_bulk` partial failure). Opt-in: ES Windows zip (approval).

## Descriptor values

Seeded from the plan (§5.2). The values in force are the descriptors in `src-tauri/src/types/driver.rs` (keepalive belongs to the session's caps); a change is a request to the lead.

| Driver        | Location | Port (TLS) | SSH    | TLS style | Secret label    | Command TTL |
| ------------- | -------- | ---------- | ------ | --------- | --------------- | ----------- |
| elasticsearch | Host     | 9200       | Tunnel | Https     | PasswordOrToken | 3000        |

| Driver        | Required fields / params   | Label overrides | Keepalive | Read-only      | Sharing | Token secret |
| ------------- | -------------------------- | --------------- | --------- | -------------- | ------- | ------------ |
| elasticsearch | host or `urls` / `cloudId` | database: —     | 60        | ClassifierOnly | Indep   | yes          |

## Read-only and hardening

| Engine        | When read-only                                                                                                                                         | When unattended (MCP) |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------- |
| Elasticsearch | GET anything; POST only `_search`, `_count`, `_msearch`, `_mget`, `_field_caps`, `_validate/query`, `_pit`, `_search/scroll`, and `_sql` with a SELECT | refused               |

Every classifier is default-deny: a statement or command it does not know is a write. The read-only mode in the caps (`Server`, `PerRequest` or `ClassifierOnly`) must say what the session really enforces.

## What to implement

- `entry.rs` (Phase B): `open`, `validate`, `util`, `util_kind` (plan §2.5). `<Prefix>Params` (`#[serde(rename_all = "camelCase", default)]`, ts-exported) parsed from `options.driver_params`.
- `NativeSession` plus `TableSource` (plan §2.5): browse with cursors, count, fetch cell, `plan_changes` / `apply_changes` (preview and operations from one plan, text through `sqlgen/native/<x>.rs`), the console (`split_console`, `classify_console` default-deny, `run_console`), `definition()`, `call` / `call_kind` (unknown calls are writes).
- `ping` is free, `close` returns within about 2 s.
- Cancel through `CancelHandle::Custom(Arc<dyn CancelAction>)`.
- Renderer: a `KindUi` per tile, `DriverUi` with `ConsoleUi`, `TableView` and `Inspector` where the grid does not fit, and a `MockDriver`. Write-capable `sessionCall` / `driverUtil` helpers await `guardExecution` first.

## Decisions and corrections for this lane

- A native store (hits grid over the flattened mapping, JSON inspector, a Kibana-style console plus `_sql`); MCP refuses it in v1 (plan A30). HTTP through `drivers/common/http`, SigV4 through `common/aws`.
- Read-only allows GET, but not `GET /<index>/_flush` or `/_refresh`, which write (critique optional 10), and POST only to the search endpoints of plan §5.3.
- Writes prove their count with `if_seq_no` / `if_primary_term` (plan A13); a bulk apply can succeed in part, so return a result per change (critique D2).
- Costs: `count` Costly; keep-alive every 60 s (plan §5.2).

## Research notes

> Background from the driver research (October 2026), with file references and no line numbers. Verify against the code; where it differs from the scope above or from README.md, those win.

### Elasticsearch

**Approach.**

Hand-roll REST over the existing reqwest 0.13 (features rustls, stream, system-proxy; Cargo.toml) with serde_json. Build one `reqwest::Client` per session:

- `tls_certs_only`/`tls_certs_merge` for a CA PEM; `identity` for a client certificate; `tls_danger_accept_invalid_certs` for skip-verify.
- Or `tls_backend_preconfigured(rustls::ClientConfig)` to reuse Tabler's own verifiers (VERIFY-CA, and SHA-256 CA-fingerprint pinning for the self-signed certificates Elasticsearch 8 generates).
- `connect_timeout`/`timeout` from the profile.
- For the SSH tunnel, `ClientBuilder::resolve(real_host, 127.0.0.1:<tunnel port>)`, so the Host header, SNI and certificate check keep the real host name.

Detect the flavor from `GET /`: version.number, version.distribution ('opensearch'), build_flavor ('serverless'). Choose endpoints from it:

- Elasticsearch: `_sql`, `/_pit`, `_query` (ES|QL).
- OpenSearch: `_plugins/_sql`, `_plugins/_ppl`, `/_search/point_in_time`.

Do not use the `elasticsearch` crate: it only has 9.1.0-alpha.1 / 8.19.0-alpha.1 (Aug 2025), has never been stable, and brings its own reqwest 0.12 stack.

Cancel: dropping the HTTP request cancels a search on the server (Elasticsearch 7.4+). Also tag requests with `X-Opaque-Id` so `POST /_tasks/<id>/_cancel` can stop long tasks such as update_by_query.

**Windows build.**

No new crates: reqwest 0.13.5, hyper 1.12, rustls 0.23.45 (aws-lc-rs), tokio-rustls 0.26.6 and serde_json 1.0.152 are already in Cargo.lock. The only build-time or binary-size cost is Tabler's own code. Optional: reqwest's `gzip` feature for compressed responses would add async-compression on top of the existing flate2; it is not needed. SigV4 for Amazon OpenSearch reuses the same signer as DynamoDB (aws-lc-rs hmac/digest, or aws-sigv4 1.6.3).

**Authentication.**

TablePlus added the driver in 26.10.0 (macOS, Sep 2026; Windows b394) and SSH in 27.0.2 (Oct 2026). Its public docs do not describe the form.

Tabler should offer:

- None.
- Basic: username/password; the Elasticsearch 8 bootstrap `elastic` user.
- API key: an `Authorization: ApiKey <base64(id:key)>` header, accepting the encoded form or id + key.
- Bearer token: a service-account token or OAuth/JWT realm.
- AWS SigV4 for Amazon OpenSearch Service (service 'es') and Serverless ('aoss').
- Elastic Cloud ID: decode `name:base64(host$es-uuid$kibana-uuid)` into https://\<es-uuid>.\<host>:443.
- TLS: CA PEM, CA SHA-256 fingerprint, client certificate/key (PKI realm), skip verification.
- The existing password modes (keychain/ask/none/command) for the password or API key.
- SSH tunnel.

**Data model.**

How TablePlus shows Elasticsearch: the public record only covers its existence (driver in 26.10.0 and URL scheme `elasticsearch`). Request #3848 asked for HTTP/HTTPS cluster connections, index browsing and management, DSL queries with visual results, and SQL.

How Elasticsearch maps onto DbSession:

- list_databases → [cluster_name], so one database per connection.
- list_schemas → one schema.
- list_tables:
  - indices → TableKind::Table, with row_estimate = docs.count, size_bytes = store.size, comment = health/status;
  - aliases → View;
  - data streams → Partitioned;
  - dot/hidden/system indices → SystemView, behind a toggle.
- table_meta → columns:
  - `_id`: PK, keyword (RowIdentity::Pk);
  - `_index`, when browsing an alias or data stream;
  - `_routing`: hidden, but needed for edits;
  - then the mapping properties. `object` fields are flattened to dotted paths; nested/flattened/geo_shape/dense_vector each get one Json column; multi-fields such as `.keyword` are used for sort and filter but are not separate columns.
  - data_type = the Elasticsearch type.
  - kind: long/integer/short/byte → Int; unsigned_long → BigInt; double/float/half_float/scaled_float → Float; keyword/text/wildcard/constant_keyword/match_only_text → Text; boolean → Bool; date/date_nanos → TimestampTz (text exactly as stored in \_source); binary → Bytes (base64); ip → Other; geo_point → Geometry/Json; object/nested → Json.
  - Any field may hold an array, so cells may be Value::Array or Json.
  - indexes → none; shards and settings go in the comment or the structure pane.
  - ddl = `PUT /<index>` with its settings + mappings JSON.
- view_definition (alias) → the alias filter JSON.
- completion_metadata → index → field names.

Shared plumbing: browse needs the native-browse hook described under Redis (commands/browse.rs). Deep pages need BrowseRequest.cursor for PIT + search_after.

New UI beyond the grid:

- A REST console in Kibana Dev Tools style, with a JSON tree viewer for non-tabular responses.
- A mapping tree viewer with an 'add field' action.
- An index panel: health, shards, settings, open/close/refresh/forcemerge.
- Aggregation results, either flattened into bucket rows or shown as a tree.
- A `_score` column for searches.
- Tasks (`_tasks`) as the Process List.

**Query language.**

Two editor modes.

(1) REST console, Kibana Dev Tools syntax:

- A `GET|POST|PUT|DELETE|HEAD /path?query` line, then an optional JSON body.
- Several requests per tab, split at each method line by a new splitter beside sqlgen/split.rs.
- NDJSON bodies for \_bulk and \_msearch.
- `_search` hits become grid rows (flattened \_source, plus \_id/_index/_score). Other responses go to the JSON viewer.

(2) SQL:

- Elasticsearch SQL: `POST /_sql?format=json` with `{"query": …, "fetch_size": n, "params": [...]}` → `{columns: [{name, type}], rows, cursor}`. Fetch further pages with `{"cursor": …}` and close with `POST /_sql/close`. Explain is `POST /_sql/translate`.
- OpenSearch: `POST /_plugins/_sql` (jdbc format: schema/datarows/cursor) and PPL at `/_plugins/_ppl`.
- ES|QL (`POST /_query`, Elasticsearch 8.11+) is optional.

Safe mode: GET, HEAD and POST to \_search/_count/_sql/_msearch/_mget are reads; everything else is a write. guardExecution (src/features/safety/guard.ts) must classify these.

Editor: CodeMirror with @codemirror/lang-json for bodies plus a method-line highlighter, and generic lang-sql for SQL mode.

**Editing.**

Grid reads: `POST /<idx>/_search` with `{from, size, sort, query, seq_no_primary_term: true, track_total_hits: false}`. Keep \_seq_no and \_primary_term per row for optimistic concurrency. Past index.max_result_window (default 10,000), switch to PIT + search_after with a `_shard_doc` tiebreaker (Elasticsearch 7.12+).

Count: `POST /<idx>/_count`.

Filter translation (FilterOp, types/browse.rs):

- Eq → term (keyword fields) or match_phrase (text fields); Ne → bool.must_not.
- Gt/Lt/Ge/Le/Between → range; In → terms.
- IsNull / IsNotNull → must_not exists / exists.
- Like → wildcard, translating % and \_; Contains / HasPrefix → wildcard or prefix, with case_insensitive (7.10+).
- Raw → a query_string (Lucene) or a JSON DSL fragment.
- AnyColumn → multi_match or query_string over the fields.

Writes (generated in a new module such as src-tauri/src/sqlgen/elastic.rs):

- Update → `POST /<idx>/_update/<id>?if_seq_no=&if_primary_term=&refresh=wait_for[&routing=]` with `{"doc": {changed fields}}`. A 409 version_conflict becomes 'changed by someone else'.
- Insert → `PUT /<idx>/_create/<id>`, or `POST /<idx>/_doc` for an auto-generated id.
- Delete → `DELETE /<idx>/_doc/<id>?if_seq_no=…`.

Multi-row commit: `POST /_bulk` with NDJSON. It is not atomic: per-item errors map to RowResult.error and ApplyResult.success=false. DbSession::transaction can only be emulated.

DDL:

- `PUT /<idx>` (settings + mappings); `PUT /<idx>/_mapping` (adding fields only; a type change needs a reindex); `DELETE /<idx>`.
- `POST /<idx>/_close|_open|_refresh|_forcemerge`; `POST /_aliases`; `POST /_reindex`.
- Truncate → `POST /<idx>/_delete_by_query` with `{"query":{"match_all":{}}}`.

Data streams are append-only (op_type create); updates go through \_update_by_query on the backing indices.

**Introspection.**

Cluster:

- `GET /` → cluster_name, cluster_uuid, version.number, version.distribution, build_flavor.
- `GET /_cluster/health` → status, number_of_nodes, active_shards.

Indices and friends:

- `GET /_cat/indices?format=json&bytes=b&h=index,uuid,health,status,pri,rep,docs.count,store.size,creation.date.string&s=index&expand_wildcards=open`, or `expand_wildcards=all` when 'show system/hidden' is on.
- `GET /_cat/aliases?format=json&h=alias,index,filter,routing.index,is_write_index`.
- `GET /_data_stream` (Elasticsearch 7.9+ / OpenSearch 1.0+).

Per index:

- `GET /<index>/_mapping` → properties.type, fields (multi-fields), properties (object), type nested, dynamic, \_source enabled/excludes, \_routing required.
- `GET /<index>/_settings?flat_settings=true` → index.number_of_shards, number_of_replicas, max_result_window, refresh_interval, mapping.total_fields.limit.
- `POST /<index>/_count`.

Cluster-wide:

- `GET /_cat/shards?format=json` and `GET /_cat/nodes?format=json&h=name,ip,heap.percent,cpu,load_1m,node.role`.
- `GET /_index_template`, `GET /_ingest/pipeline`, `GET /_ilm/policy` (OpenSearch: `_plugins/_ism`), later as sidebar objects.
- `GET /_tasks?detailed=true&actions=*search*` for the process list; cancel with `POST /_tasks/<id>/_cancel`.
- OpenSearch PIT: `POST /<idx>/_search/point_in_time?keep_alive=1m`. Elasticsearch: `POST /<idx>/_pit?keep_alive=1m`, closed with `DELETE /_pit`.

**Testing without a server.**

Unit tests:

- Mapping flattening: object vs nested vs flattened, multi-fields, arrays.
- FilterGroup → Query DSL.
- Cloud ID decoding.
- The CA-fingerprint verifier.
- The console splitter, including NDJSON bodies.
- Bulk-error mapping.
- Flavor detection from recorded `GET /` bodies of Elasticsearch 7/8/9 and OpenSearch 2/3.

An in-process fake HTTP server (tokio TcpListener with canned HTTP/1.1 responses; no new dependency) can replay recorded JSON for `/`, `_cat/indices`, `_mapping`, `_search` (including PIT pages), `_sql` cursors and `_bulk` partial failures, exercising the reqwest code path end to end, including resolve() tunnel addressing and TLS with dev/certs.

Integration:

- `docker.elastic.co/elasticsearch/elasticsearch:9.x` single-node (discovery.type=single-node, xpack.security.enabled=false, plus a TLS + API-key variant) and `opensearchproject/opensearch:3`, as new profiles in dev/docker-compose.yml.
- Or the Elasticsearch Windows zip (bundled JDK, bin/elasticsearch.bat) without Docker.

**What it shares with an existing driver.**

Not wire-compatible: HTTP/JSON. Elasticsearch SQL is SQL-like but read-only (no DML or DDL) and uses its own function set. Grid filters must become Query DSL, so sqlgen/select.rs build_select/build_where cannot be reused. The same driver serves:

- OpenSearch 1-3: `_plugins` SQL/PPL, different PIT endpoints, `_plugins/_ism`.
- Amazon OpenSearch Service and Serverless: SigV4; Serverless lacks \_cat and parts of the index APIs.
- Elastic Cloud and Serverless: API keys; some cluster APIs are hidden.
- Elasticsearch-compatible engines such as Easysearch, with partial API coverage.

**Connection fields.**

- Name / color / tag (existing)
- URL(s): http(s)://host:9200; several nodes for failover
- Elastic Cloud ID (alternative to URL)
- Path prefix (cluster behind a reverse proxy)
- Auth method: None / Basic / API key / Bearer token / AWS SigV4
- Username
- Password + storage mode (keychain/ask/none/command)
- API key (encoded, or id + key)
- AWS region / credentials / profile (SigV4 mode; shared with DynamoDB)
- TLS verify mode (reuse SslMode)
- CA certificate file (PEM)
- CA SHA-256 fingerprint
- Client certificate + key (PKI realm)
- Show system/hidden indices (dot-prefixed, expand_wildcards=all)
- Request timeout
- Page size / track_total_hits
- SSH tunnel (existing; reqwest resolve() keeps the real host)
- Read only / safe mode (existing)
- URL import: elasticsearch:// (TablePlus scheme per its manage-connections docs) and http(s)://

**Effort:** L.

**Risks.**

- API differences across versions and forks: mapping types removed in 7.x, OpenSearch endpoints (`_plugins/_sql`, `_search/point_in_time`, ISM vs ILM), and Serverless missing `_cat` and settings APIs. Flavor detection must drive an endpoint table.
- Deep paging: from+size stops at index.max_result_window (10,000). PIT/search_after needs a stable sort and keep-alive management, and arbitrary page jumps cost a read of every page before them.
- Mappings can explode: thousands of fields (total_fields.limit 1000+), dynamic fields that are missing from the mapping but present in \_source, implicit arrays, and the object/nested/flattened distinction. Text fields cannot be sorted without a .keyword sub-field.
- Writes are near real time: an edit is invisible to the next search until a refresh. Use refresh=wait_for, which is slower, or tell the user.
- No transactions: \_bulk partially succeeds, so a commit can leave some rows written.
- Optimistic concurrency depends on seq_no_primary_term being requested on every read. Documents indexed with custom routing need \_routing on update and delete, or the update fails or lands on the wrong shard.
- Security setups vary: Elasticsearch 8's auto-generated self-signed CA (needs fingerprint pinning), API keys, PKI, Cloud ID, and SigV4 for AWS. Each is an extra code path.
- Responses can be large: aggregations, big \_source documents, ES|QL columnar output. Cap and stream them.
- Safe-mode classification of REST verbs is heuristic: a POST to \_search is a read, while a GET to some admin APIs has side effects (e.g. \_cache/clear is POST, \_flush is GET-able).

## Limits

_The lane lists here what v1 does not do, before it lands (definition of done, item 10)._
