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

| Engine        | When read-only                                                                                                                                                                                                     | When unattended (MCP) |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------- |
| Elasticsearch | GET on the engines' own endpoints but the acting ones of the sweep below; POST only `_search`, `_count`, `_msearch`, `_mget`, `_field_caps`, `_validate/query`, `_pit`, `_search/scroll`, and `_sql` with a SELECT | refused               |

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

## Phase A status

Phase A is done: pure code and a REST client behind a transport seam, with unit tests and fakes. Nothing is wired into `entry.rs`; `mod.rs` keeps a temporary `#![allow(dead_code)]`.

**Done** (`src-tauri/src/drivers/elasticsearch/`, `src-tauri/src/sqlgen/native/elastic.rs`):

- `json.rs`: a total JSON reader that keeps numbers as written and keys in order, so `_source` shows what was indexed whatever `serde_json` features other crates turn on. Nesting is bounded. Console bodies also take `#`, `//` and `/* */` comments and Kibana's `"""…"""` strings.
- `console.rs`: the Kibana Dev Tools console.
  - A request is `METHOD /path?query` plus an optional body: one JSON value, or NDJSON for `_bulk` and `_msearch`. The splitter breaks at method lines outside strings and comments, with character offsets.
  - Each request parses once into an `EsRequest`, which the classifier judges and the client sends. The path is kept as decoded segments and re-encoded strictly.
  - Refused: `.` and `..` segments (also as `%2e`, which URL parsers fold away), absolute URLs, backslashes and `#`. A path can never reach another host or another endpoint.
- `classify.rs`: the default-deny classifier, over the parsed request.
  - Only the engines' own endpoints can read: every `_` word of the path must be one the engines define at that place.
    - The first segment is an index when it does not start with `_` (index and alias names never do) or is `_all`, which names every index. After an index, the first other `_` word is the endpoint and must be an index's (`INDEX_ENDPOINTS`).
    - Otherwise the first segment is a top-level endpoint (`TOP_ENDPOINTS`), and after `_plugins` / `_opendistro` comes the plugin's name (`PLUGINS`).
    - Every later `_` segment must be a word the specifications define at that depth: right after a top-level word, an index endpoint or a word of that endpoint (`TOP_PAIRS`: `_ltr/_featureset`, `_transform/_preview`, `_data_stream/_promote`, …); deeper, a word of `DEEP_WORDS` at one of its depths (`_transform/<id>/_stats` has `_stats` at depth 2, `_ml/anomaly_detectors/<id>/_data` has `_data` at 3); or a selector (`_all`, `_local`, `_master`, `_current`, also as a comma list).
    - Words are checked by depth, not under the endpoint before them, because Elasticsearch's `PathTrie` falls back to a placeholder when the explicit branch does not match: `/_cat/_data` is `/{index}/_data` with the index `_cat`, and `/logs/_doc/_x` reaches a plugin's `/{index}/{type}/_x` before the document get.
    - The vocabulary is every such word of the REST API specifications of Elasticsearch 6.8, 7.17 and 9.x (`elastic/elasticsearch` `rest-api-spec` on 6.8, `elasticsearch-specification` on 7.17 and main) and OpenSearch (`opensearch-api-specification`), plus the OpenSearch plugins the specification leaves out (and anomaly detection's `_profile` and top results). Any other path is a plugin's, which may act on `GET`, and is a write; so is a name or id that starts with `_`, which may be a plugin's word.
  - `GET` and `HEAD` read, except the action segments (`ACTION_SEGMENTS`, `_preview` among them) and the acting `GET` endpoints of the sweep below.
  - `POST` reads only on exact search shapes: `_search`, `_count`, `_msearch`, `_mget`, `_field_caps`, `_validate/query`, `_pit` / `_search/point_in_time`, `_search/scroll`, `_explain`, `_termvectors`, `_mtermvectors`, `_terms_enum`, `_search_shards`, `_render/template`, `_analyze`. Typed 6.x forms count too. An index, type or id segment may not start with `_` (except `_all`), so `POST /idx/_update/_search` can never pass as a search.
  - `_sql`, `_xpack/sql`, `_plugins/_sql` and `_opendistro/_sql` read only when every query the request carries (body `query`, body `sql`, every `sql` parameter) is a single `SELECT`, `SHOW` or `DESCRIBE` with no write word as code, or when the body only pages a cursor.
  - `PUT`, `DELETE`, `PATCH` and `OPTIONS` write.
- `info.rs`: `GET /` names the flavor (Elasticsearch, OpenSearch, Serverless, OSS) and version, refuses Elasticsearch before 6.0, and derives the features in use:
  - point in time (Elasticsearch 7.10+ `_pit`, OpenSearch 2.4+ `_search/point_in_time`);
  - `_shard_doc` (Elasticsearch 7.12+);
  - `if_seq_no` (6.7+);
  - the SQL endpoint;
  - typed paths before 7.0, `case_insensitive` (7.10+), data streams, `create` without an id (7.5+).
- `catalog.rs`: the sidebar and the columns.
  - `_cat/indices` (counts and sizes as numbers, health and status as the comment), `_cat/aliases` (views), `_data_stream` (partitioned; its backing indices hidden), dot-prefixed names as `SystemView` only on request.
  - `_mapping` flattened into columns. Objects become dotted paths. `nested`, `flattened`, vectors, ranges, shapes and disabled objects become one JSON column. A keyword multi-field is the parent's sort and exact-match field (in `extra`, `sort: title.keyword`). Field aliases are skipped. 6.x typed mappings are read, and so are merges across indices (conflicting types become a JSON column).
  - Key columns: `_index` for aliases, streams and patterns; `_id`; `_routing` when required; `_seq_no` and `_primary_term`. These are part of the row key, so a staged change carries the guard it was read with.
  - `_settings` (`max_result_window`, write blocks, the settings a definition leaves out).
- `values.rs`: `_source` values keep their JSON type:
  - integers beyond 2^53 become `BigInt`, and more digits than a double holds become `Decimal`;
  - dates become `TimestampTz` as stored, `ip` becomes `Other{ip}`, `binary` becomes bytes;
  - arrays and objects become JSON text, and a missing field becomes `Other{missing}`.

  Dotted paths are found under nested objects and under literal dotted keys, and through arrays of objects. On the way back, a staged value is written in its field's type (text `42` for a `long` is a number); missing, DEFAULT, a cut preview and bad JSON are refused.

- `reply.rs`: console replies as grids:
  - hits with their `_source` leaves (with total-hits, timeout, shard-failure and aggregation notices, and the scroll id as a cursor);
  - `_msearch` with one grid per search, `_mget`, `_cat`, and `_bulk` items with their errors;
  - SQL results of both engines with typed columns;
  - anything else as one JSON cell, and non-JSON as text.
- `errors.rs`: 401 and AWS signature or token refusals map to `Auth`; `task_cancelled_exception` to `Cancelled`; 408 and 504 to `Timeout`; `index_not_found_exception` to `NotFound`; a 502 or 503 without an Elasticsearch body to `Connect`. Everything else is `Db`, with the HTTP status in `vendor_code`, the error type in `detail`, the `caused_by` chain in the message, and hints for conflicts, privileges, rejections and blocks. A 404 `{"found": false}` is a reply.
- `transport.rs`: the `Transport` seam (requests carry headers and body, for SigV4), and `BaseUrl` (scheme, host, port with 9200 or 443 defaults, IPv6, path prefix, no credentials in the URL).
- `auth.rs`, `sigv4.rs`: request authentication and the Cloud ID.
  - None, Basic, API key (encoded, or id + key), and Bearer.
  - SigV4 for `es` and `aoss`: double-encoded canonical path, strict canonical query, every header signed, `x-amz-content-sha256`, session token. The test vectors of the AWS signing suite pass.
  - The region and service are read from an Amazon endpoint host.
  - An Elastic Cloud ID becomes the cluster's URL.
- `client.rs`: `EsClient` covers info, sidebar tables, mappings, result window, count, documents, browse, `_bulk` apply and console runs.
  - Browse uses `from`/`size` within `max_result_window`. Deeper pages walk a point in time with `search_after` (skipping without `_source`) on Elasticsearch 7.12+, or a scroll elsewhere. Points in time and scrolls are closed at the last page.
  - The cursor is a `String`, checked against the query's fingerprint and position.
  - Every request carries `x-opaque-id`, so a task can be found again.
- `sqlgen/native/elastic.rs`: every request and its console text.
  - Catalog requests, index definitions as `PUT /<index>` with replayable settings, alias definitions, index actions, and truncate (`_delete_by_query`).
  - The filter bar as Query DSL:
    - `term` or `terms`, through the keyword field for text, `match_phrase` without one, `ids` for `_id`;
    - `range`, `exists`;
    - `wildcard` and `prefix` with escaping and `case_insensitive`;
    - raw filters as DSL or Lucene `query_string`, where DSL that calls a model is refused;
    - any-column as `multi_match` or `query_string`.
  - Sorts use the keyword field with `missing` and `unmapped_type`; an unsortable text field is refused.
  - Edits: one `_bulk` action per change with `if_seq_no`, `if_primary_term` and `routing`; `create` for inserts; nested partial documents; `refresh=wait_for`. Each change's preview is a one-item `_bulk` that runs back as the same request. `bulk_outcomes` gives a result per change (version conflicts, documents that are gone, items without a reply count as not applied), with the new `_seq_no` / `_primary_term` returned. Metadata edits, field removal and server expressions are refused.
  - SQL query, next page, close and translate/explain for both engines.
- `testing.rs`: `FakeEs`, an in-memory cluster behind the seam. It serves `from`/`size`, points in time, scrolls, `_count`, `_mget`, and `_bulk` with `if_seq_no` checks, and can inject failures. There is also a reqwest transport for the wire tests.
- `tests.rs`, fixtures in `src-tauri/tests/fixtures/elasticsearch/`: connect, list, describe, shallow and deep pages (point in time and scroll, cursor continuation, cleanup), count, fetch, a commit with one failing change, a failed bulk, and console runs.
  - Recorded replies are replayed over HTTP through `testkit::fake_http`: `/` (Elasticsearch 8 and OpenSearch 2), `_cat`, `_mapping`, point-in-time pages and a `_bulk` partial failure.
  - The wire tests check credentials, `x-opaque-id`, NDJSON bodies, SigV4 headers, bad credentials, cancel by dropping the request (the server sees it aborted) and a timeout.

Tests: 82 lane tests (`drivers::elasticsearch`, `sqlgen::native::elastic`); `test-all` passes.

**Classifier sweep.** The engine's endpoints, swept against the Elasticsearch REST API reference (core, X-Pack, ML, security, Fleet, connectors, inference) and the OpenSearch API reference and plugins (security, k-NN, ML Commons, alerting, anomaly detection and forecasting, notifications, ISM, SQL/PPL and direct query, replication, flow framework, neural search, user behavior insights, search processors). Third-party plugins, their REST routes checked in their sources: NLPchina elasticsearch-sql (6.8 to 9.3 branches), learning to rank (o19s), zentity, carrot2, Wikimedia search-extra and search-highlighter, the codelibs plugins with `GET` routes (dataformat, sstmpl, configsync, indexing-proxy), the Aiven Prometheus exporter, and the analysis plugins the Elasticsearch plugin docs list (IK, Pinyin, STConvert, Vietnamese: no REST handlers). Security plugins (ReadonlyREST, Search Guard) and any other plugin fall under the endpoint vocabulary:

| Request                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | Why it acts                                                                                                 | Class                                                   |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ------------------------------------------------------- |
| `GET`/`POST …/_flush`, `_flush/synced`, `_refresh`, `_optimize` (2.x), `_cache/clear` (`GET` before 7.0)                                                                                                                                                                                                                                                                                                                                                                                                                              | changes the index or the node's state                                                                       | write, any method                                       |
| `GET _ingest/pipeline/…/_simulate`, `GET _ingest/_simulate`                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | runs processors: `inference`, `enrich`, scripts                                                             | write                                                   |
| `GET`/`POST` `_transform/_preview`, `_transform/…/_preview` (7.x `_data_frame/transforms/…`), `_ml/datafeeds/…/_preview` (6.x `_xpack/ml`), `_ml/data_frame/analytics[/…]/_preview`, OpenSearch transform and anomaly detector `_preview`                                                                                                                                                                                                                                                                                             | runs the destination pipeline (`inference`, `enrich`, scripts) or a stored query that is not in the request | write, any method                                       |
| `GET`/`POST` `_ml/data_frame/analytics[/…]/_explain`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | runs the (stored) source query and starts a memory estimation process on an ML node                         | write                                                   |
| `GET _scripts/painless/_execute`, watcher, alerting and agent `_execute`                                                                                                                                                                                                                                                                                                                                                                                                                                                              | runs code, sends notifications, calls models                                                                | write                                                   |
| `GET _security/enroll/kibana`, `…/node`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | mints a service-account token, hands out keys                                                               | write                                                   |
| `GET _plugins/_knn/warmup/…` (and `_opendistro`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | loads graphs into memory                                                                                    | write                                                   |
| `GET _plugins/_notifications/feature/test/…`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          | sends a message to a channel                                                                                | write                                                   |
| `GET`/`POST` `_search/template`, `_msearch/template`, `_rank_eval`, `_application/search_application/…/_search`                                                                                                                                                                                                                                                                                                                                                                                                                       | a stored template is not in the request                                                                     | write                                                   |
| `_analyze` with an inline tokenizer, filter or char filter                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | `synonyms_path`, `stopwords_path`, … read files of the server                                               | write (named components read)                           |
| any request with `refresh` (a realtime `GET` / `_mget` with `refresh=true` refreshes the shard), `source` (the body as a parameter), `search_pipeline`, `keep_on_completion`, `wait_for_completion_timeout`, `keep_alive` outside opening a point in time (`GET _async_search/…?keep_alive=` and the EQL, ES\|QL, SQL and OpenSearch async gets extend the stored result)                                                                                                                                                             | refreshes, hides the body, calls a pipeline, stores a result or keeps it longer                             | write                                                   |
| bodies with `semantic`, `neural`, `neural_sparse`, `query_vector_builder`, `text_similarity_reranker`, `inference_id`, `model_id`, `text_expansion`, `generative_qa_parameters`, `ml_inference`, `rerank`, `agentic`, `agent_id`                                                                                                                                                                                                                                                                                                      | call an inference endpoint, ML model or LLM, maybe another service                                          | write, any endpoint (also refused as a grid raw filter) |
| bodies with `sparse_vector` + `query` text, or the multi-field `rrf` / `linear` retriever (`query` with `fields` or the index's default fields, 9.1+)                                                                                                                                                                                                                                                                                                                                                                                 | the field's own inference endpoint turns the text into a vector (`semantic_text`)                           | write, any endpoint                                     |
| bodies with `sltr` (learning to rank, OpenSearch and the Elasticsearch plugin)                                                                                                                                                                                                                                                                                                                                                                                                                                                        | runs the stored feature queries of a feature set                                                            | write, any endpoint                                     |
| `_inference`, `_ml/…/_infer`, `_plugins/_ml/…/_predict`, `_deploy`, `_register`, …                                                                                                                                                                                                                                                                                                                                                                                                                                                    | call or load models                                                                                         | write                                                   |
| `_sql` with anything but one `SELECT` / `SHOW` / `DESCRIBE` (OpenSearch runs `DELETE`), also through `?sql=`                                                                                                                                                                                                                                                                                                                                                                                                                          | writes                                                                                                      | write                                                   |
| `GET`/`POST _sql` with a body `sql` or several `sql` parameters: NLPchina elasticsearch-sql owns `/_sql` and `/_sql/_explain` before 7.5 (6.x, where X-Pack SQL is `_xpack/sql`, and 7.0–7.4 OSS); it copies the body's string keys into the parameters and runs `sql` (`DELETE` is a delete by query), whatever `query` says                                                                                                                                                                                                         | writes                                                                                                      | write unless every query reads                          |
| `GET _sql/_explain` (NLPchina before 7.5)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | only translates the statement (its `SELECT` subqueries run as searches)                                     | read                                                    |
| any path with a `_` word outside the specifications' vocabulary at its depth, for example NLPchina `_nlpcn/sql` and `_nlpcn/sql/explain` (7.5+, `GET` and `POST`), zentity `GET _zentity/models`, codelibs `GET <index>/_data?file=` and 6.x `<index>/<type>/_data` (`GET` and `POST`; also `_all/_data`, and through the `PathTrie` fallback `_cat/_data`, `_search/_data`, `_nodes/_data` or `logs/_search/_data`, where `_cat` or `_search` is the index or the type), OpenSearch `_plugins/_directquery`, `_plugins/_async_query` | runs `DELETE`; creates `.zentity-models`; writes a file of the server; calls Prometheus, Alertmanager, EMR  | write (default-deny), any method                        |
| … and the other plugin endpoints outside it, whether their `GET` reads (carrot2 `_search_with_clusters`, the Prometheus exporters' `_prometheus/metrics`) or was not judged (ReadonlyREST `_readonlyrest`, Search Guard `_searchguard`, codelibs `_configsync`, `_idxproxy`)                                                                                                                                                                                                                                                          | unknown to the classifier                                                                                   | write (default-deny)                                    |
| `GET _search/script_template`, `<index>/_search/script_template` (codelibs sstmpl)                                                                                                                                                                                                                                                                                                                                                                                                                                                    | a stored script template is not in the request                                                              | write                                                   |
| bodies with `ubi` (OpenSearch user behavior insights), `search_configuration` (Kendra intelligent ranking), `personalize_request_parameters` (Amazon Personalize ranking)                                                                                                                                                                                                                                                                                                                                                             | stores the query in `ubi_queries` or sends it to Data Prepper; call AWS                                     | write, any endpoint                                     |
| learning to rank `_ltr` (`GET` lists stores, features and cache stats; `_clearcache` is `POST` only), k-NN `clear_cache`, `_refresh_search_analyzers`, PPL and reporting `on_demand` (not `GET`), Wikimedia search-extra `GET _nodes/latencyStats`, anomaly detection and forecasting `GET`s (get, search, stats, top results, insights status)                                                                                                                                                                                       | read, or not routed for `GET`                                                                               | as their method says                                    |
| `POST _query` (ES\|QL), `_plugins/_ppl`, `_async_search`, `_eql/search`, `_sql/close`, `_pit` close                                                                                                                                                                                                                                                                                                                                                                                                                                   | not on the allow-list                                                                                       | write (default-deny)                                    |
| `GET _resolve/cluster`, `_remote/info`, cross-cluster `remote:index/_search`                                                                                                                                                                                                                                                                                                                                                                                                                                                          | reach only the remote clusters the admin configured                                                         | read                                                    |

**Left for Phase B:** `entry.rs` (`ElasticParams`: urls or `cloudId`, path prefix, auth method, API key id, AWS region and profile, TLS CA / fingerprint / client certificate, show system indices, page size; `open`, `validate`, `util`); the `NativeSession` and `TableSource` over `EsClient` (caps `ClassifierOnly`, no MCP, `count` Costly, keep-alive 60 s, cancel by dropping the request plus `_tasks/<id>/_cancel` by `x-opaque-id`, a search-context-missing retry of a walk); the transport on F8a's `HttpClient` (TLS, CA fingerprint pinning, SSH through `resolve`) and SigV4 credentials from `common/aws` (static, profile, environment, `credential_process`); the console's SQL mode and SQL cursor paging; aliases and data streams resolved for `_index`; the renderer (`KindUi`, form, URL import `elasticsearch://`, console, JSON inspector), the mock and `tests/elasticsearch.rs`.

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

Known so far (Phase A):

- **Search on the server's own model wiring.** Read-only cannot see a model call that the request does not spell. These run as reads: a `match` query on a `semantic_text` field (Elasticsearch 8.18+), wherever it sits (a `bool`, a `standard` or `diversify` retriever); and an index's default search pipeline (`index.search.default_pipeline`, OpenSearch) with an `ml_inference` processor. The other forms of this kind are writes: `sparse_vector` with `query` text and no `inference_id`, the multi-field `rrf` / `linear` retriever (`query` + `fields`, 9.1+), a `knn` query on a `semantic_text` field (its text needs `query_vector_builder`; with `query_vector` it calls no model), the `semantic` query, and stored search templates, search applications, learning-to-rank feature sets and transform or datafeed previews. `multi_match`, `query_string` and `combined_fields` do not infer for `semantic_text` fields (the 9.x sources rewrite only `match`, `knn` and `sparse_vector` on such fields).
- **Plugins inside the engines' paths.** The vocabulary is checked word by word at each depth, so a plugin word anywhere in the path is a write, also where the server's `PathTrie` fallback makes an engine word or `_all` the index or type of a plugin's route. A plugin route whose words are all ones the engines define at the same depths (the way codelibs sstmpl uses `_search/script_template`, which is swept), or with no `_` word at all, passes as the engine path it looks like; so does Kendra intelligent ranking turned on in an index's settings (the request names nothing). The other way round these are writes, which read-only refuses: a read-only plugin endpoint outside the vocabulary (a Prometheus exporter, carrot2), an endpoint newer than the specifications, a `_` word an OpenSearch plugin outside the specification uses deeper than its name (anomaly detection's `_profile` and top results aside), and any name or id that starts with `_` but is not `_all`, `_local`, `_master` or `_current` (a document id `_x`, a `_default_` mapping type, a node or task id that happens to start with `_`).
- **Updates write nested objects.** An update of `user.name` sends `{"user": {"name": …}}`. A document that stored the field under a literal dotted key (`"user.name": …`) then holds both forms. Removing a field (`Other{missing}`) is refused in the grid; use the console with a script.
- **Deep pages.** Pages past `index.max_result_window` read every page before them. A point in time skips 10,000 rows per request without `_source`. A scroll (OpenSearch, Elasticsearch before 7.12) reads page by page. Points in time expire after 5 minutes without a page.
- **No guard before 6.7.** Elasticsearch before 6.7 has no `if_seq_no`, so its rows cannot be updated or deleted. Inserts use `create`, which never overwrites.
- **Bulk applies.** A bulk apply is not atomic: each change reports its own result (`TxMode::Batch`).
