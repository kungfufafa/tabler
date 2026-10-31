//! Whether a console request only reads: an allow-list, default-deny. It judges the parsed
//! [`EsRequest`] the client sends, never the typed text.
//!
//! - A path with a `_` word the engines do not define at that place is a write whatever the
//!   method: a plugin can register any path and act on `GET` (see [`known_endpoint`]).
//! - `GET` and `HEAD` read (Elasticsearch serves `HEAD` with the `GET` handler), except the
//!   endpoints that act although they accept `GET`: `_flush`, `_refresh`, the old `_optimize`
//!   and `GET _cache/clear`, ingest and painless `_simulate` / `_execute`, the transform and ML
//!   `_preview` / `_explain` calls (they run a pipeline, a stored query or an ML process),
//!   security enrollment, the k-NN `warmup` and the notification `test` of OpenSearch, templated
//!   searches and search applications (a stored template is not in the request), and every other
//!   segment of [`ACTION_SEGMENTS`].
//! - `POST` reads only on the search endpoints of [`post_reads`] (`_search`, `_count`,
//!   `_msearch`, `_mget`, `_field_caps`, `_validate/query`, opening a point in time,
//!   `_search/scroll`, …) and `_sql` with a `SELECT`, `SHOW` or `DESCRIBE` (every query the
//!   request carries: the `sql` parameters and the body's `query` and `sql`).
//! - `PUT`, `DELETE`, `PATCH`, `OPTIONS` write.
//! - A segment holding a decoded `/`, `\` or `?` (`%2F`, `%5C`, `%3F`) is one segment to the
//!   cluster, but a reverse proxy in front of it may decode it: the path such a proxy passes on
//!   must read too (see [`proxied`]).
//!
//! On top of that, whatever the endpoint: a `refresh` parameter (a realtime `GET` with
//! `refresh=true` refreshes the shard), a body passed as the `source` parameter, a
//! `search_pipeline`, a stored async result (`keep_on_completion`, `wait_for_completion_timeout`,
//! and `keep_alive` outside opening a point in time: it extends a stored result), and a body
//! that calls an inference or ML model (`semantic`, `neural`, `query_vector_builder`,
//! `text_similarity_reranker`, `inference_id`, `model_id`, …, see [`REMOTE_KEYS`], and query text
//! the field's mapping turns into a vector), runs stored queries (`sltr`) or stores the query
//! (`ubi`), which can reach another service, all make the request a write. `_analyze` reads only with named analysis
//! components: an inline filter can read a file of the server (`synonyms_path`).

use crate::sqlgen::classify::StatementClass;

use super::console::{self, Body, EsRequest, Method, QueryPairs};
use super::json::Json;

/// Path segments that act on the cluster whatever the method (compared ignoring ASCII case).
pub const ACTION_SEGMENTS: &[&str] = &[
    "_flush",
    "_refresh",
    "_optimize",
    "_forcemerge",
    "_cache",
    "_clear_cache",
    "_execute",
    "_execute_retention",
    // Transform, datafeed, data frame analytics and anomaly detector previews: they run the
    // destination pipeline (processors that call models) or a stored query.
    "_preview",
    "_simulate",
    "_simulate_index",
    "_infer",
    "_predict",
    "_deploy",
    "_undeploy",
    "_load",
    "_unload",
    "_register",
    "_upload",
    "_chunk",
    "_train",
    "_provision",
    "_deprovision",
    "_shutdown",
    "_restart",
    "_start",
    "_stop",
    "_pause",
    "_resume",
    "_close",
    "_open",
    "_rollover",
    "_shrink",
    "_split",
    "_clone",
    "_freeze",
    "_unfreeze",
    "_block",
    "_reindex",
    "_update_by_query",
    "_delete_by_query",
    "_rethrottle",
    "_bulk",
    "_update",
    "_create",
    "_upgrade",
    "_restore",
    "_mount",
    "_promote",
    "_modify",
    "_migrate",
    "_verify",
    "_verify_integrity",
    "_cleanup",
    "_forecast",
    "_revert",
    "_reset",
    "_set_upgrade_mode",
    "_schedule_now",
    "_ack",
    "_acknowledge",
    "_activate",
    "_deactivate",
    "_password",
    "_enable",
    "_disable",
    "_invalidate",
    "_check_in",
    "_reload_search_analyzers",
    "_cancel",
    "_kill",
];

/// Body keys that make a search call a model or another service (inference endpoints, ML
/// connectors, search pipelines, generative answers) or store a result on the server.
pub const REMOTE_KEYS: &[&str] = &[
    "semantic",
    "neural",
    "neural_sparse",
    "query_vector_builder",
    "text_similarity_reranker",
    "inference_id",
    "inference_text",
    "model_id",
    "model_text",
    "text_expansion",
    "search_pipeline",
    "generative_qa_parameters",
    "ml_inference",
    "rerank",
    "agentic",
    "agent_id",
    "connector_id",
    // Learning to rank (OpenSearch, the ES plugin): runs the stored feature queries of a set.
    "sltr",
    // OpenSearch search extensions: user behavior insights stores the query (an index or Data
    // Prepper), Kendra and Personalize ranking call AWS.
    "ubi",
    "search_configuration",
    "personalize_request_parameters",
    "keep_on_completion",
    "wait_for_completion_timeout",
    // A query passed in base64: the cluster decodes and rewrites it like any other (a `semantic`
    // query inside calls its model), and nothing here decodes it.
    "wrapper",
];

/// The endpoint names Elasticsearch 6.8 to 9.x and OpenSearch 1 to 3 define at the start of a
/// path (their REST API specifications).
const TOP_ENDPOINTS: &[&str] = &[
    "_alias",
    "_aliases",
    "_analyze",
    "_application",
    "_async_search",
    "_autoscaling",
    "_bulk",
    "_cache",
    "_capabilities",
    "_cat",
    "_ccr",
    "_cluster",
    "_component_template",
    "_connector",
    "_count",
    "_create_from",
    "_dangling",
    "_data_frame",
    "_data_stream",
    "_delete_by_query",
    "_encryption",
    "_enrich",
    "_eql",
    "_features",
    "_field_caps",
    "_fleet",
    "_flush",
    "_forcemerge",
    "_health_report",
    "_ilm",
    "_index_template",
    "_inference",
    "_info",
    "_ingest",
    "_insights",
    "_internal",
    "_license",
    "_lifecycle",
    "_list",
    "_logstash",
    "_ltr",
    "_mapping",
    "_mappings",
    "_mget",
    "_migration",
    "_ml",
    "_monitoring",
    "_msearch",
    "_mtermvectors",
    "_nodes",
    "_opendistro",
    "_pit",
    "_plugins",
    "_profiling",
    "_query",
    "_query_rules",
    "_rank_eval",
    "_recovery",
    "_refresh",
    "_reindex",
    "_remote",
    "_remotestore",
    "_render",
    "_resolve",
    "_rollup",
    "_script_context",
    "_script_language",
    "_scripts",
    "_search",
    "_search_shards",
    "_searchable_snapshots",
    "_security",
    "_segments",
    "_settings",
    "_shard_stores",
    "_slm",
    "_snapshot",
    "_sql",
    "_ssl",
    "_stats",
    "_streams",
    "_synonyms",
    "_tasks",
    "_template",
    "_text_structure",
    "_transform",
    "_update_by_query",
    "_upgrade",
    "_validate",
    "_watcher",
    "_wlm",
    "_xpack",
];

/// The endpoint names the specifications define after an index (`/<index>/_search`, the 6.x
/// typed `/<index>/<type>/<id>/_source`).
const INDEX_ENDPOINTS: &[&str] = &[
    "_alias",
    "_aliases",
    "_analyze",
    "_async_search",
    "_block",
    "_bulk",
    "_cache",
    "_ccr",
    "_clone",
    "_close",
    "_count",
    "_create",
    "_delete_by_query",
    "_disk_usage",
    "_doc",
    "_downsample",
    "_eql",
    "_explain",
    "_field_caps",
    "_field_usage_stats",
    "_fleet",
    "_flush",
    "_forcemerge",
    "_freeze",
    "_graph",
    "_ilm",
    "_knn_search",
    "_lifecycle",
    "_mapping",
    "_mappings",
    "_mget",
    "_migration",
    "_msearch",
    "_mtermvectors",
    "_mvt",
    "_open",
    "_pause",
    "_pit",
    "_rank_eval",
    "_recovery",
    "_refresh",
    "_reload_search_analyzers",
    "_resume",
    "_rollover",
    "_rollup",
    "_rollup_search",
    "_search",
    "_search_shards",
    "_searchable_snapshots",
    "_segments",
    "_settings",
    "_shard_stores",
    "_shrink",
    "_source",
    "_split",
    "_state",
    "_stats",
    "_terms_enum",
    "_termvectors",
    "_unfreeze",
    "_update",
    "_update_by_query",
    "_upgrade",
    "_validate",
    "_xpack",
];

/// The OpenSearch plugins under `_plugins` (and the old `_opendistro`): the specification's and
/// those of the plugins the distribution ships that it leaves out. Not `_directquery` (its `GET`
/// calls are passed on to Prometheus and Alertmanager) nor `_async_query` (Spark jobs on EMR).
const PLUGINS: &[&str] = &[
    "_alerting",
    "_anomaly_detection",
    "_asynchronous_search",
    "_flow_framework",
    "_forecast",
    "_im",
    "_ism",
    "_job_scheduler",
    "_knn",
    "_ltr",
    "_ml",
    "_neural",
    "_notifications",
    "_observability",
    "_performanceanalyzer",
    "_ppl",
    "_query",
    "_refresh_search_analyzers",
    "_replication",
    "_reports",
    "_rollup",
    "_search_relevance",
    "_security",
    "_security_analytics",
    "_sm",
    "_sql",
    "_transform",
    "geospatial",
    "ubi",
];

/// The `_` words the specifications define right after a top-level word, besides
/// [`INDEX_ENDPOINTS`] and the plugins of `_plugins` / `_opendistro`.
const TOP_PAIRS: &[(&str, &str)] = &[
    ("_connector", "_secret"),
    ("_connector", "_sync_job"),
    ("_data_stream", "_migrate"),
    ("_data_stream", "_modify"),
    ("_data_stream", "_promote"),
    ("_encryption", "_reset"),
    ("_features", "_reset"),
    ("_fleet", "_fleet_msearch"),
    ("_index_template", "_simulate"),
    ("_index_template", "_simulate_index"),
    ("_inference", "_region_policy"),
    ("_ingest", "_simulate"),
    ("_ltr", "_cachestats"),
    ("_ltr", "_clearcache"),
    ("_ltr", "_feature"),
    ("_ltr", "_featureset"),
    ("_ltr", "_model"),
    ("_ml", "_delete_expired_data"),
    ("_remotestore", "_restore"),
    ("_security", "_authenticate"),
    ("_security", "_query"),
    ("_slm", "_execute_retention"),
    ("_snapshot", "_status"),
    ("_tasks", "_cancel"),
    ("_transform", "_node_stats"),
    ("_transform", "_preview"),
    ("_watcher", "_query"),
    ("_watcher", "_start"),
    ("_watcher", "_stop"),
];

/// The `_` words the specifications define deeper in a path, with the depths they sit at (the
/// first segment is depth 0: `/_transform/<id>/_stats` has `_stats` at 2), plus those of
/// anomaly detection and forecasting, which OpenSearch ships and its specification leaves out
/// (`_profile`, the top results).
const DEEP_WORDS: &[(&str, &[usize])] = &[
    ("_ack", &[3, 4]),
    ("_activate", &[2, 3, 4]),
    ("_addfeatures", &[3, 4]),
    ("_analyze", &[2]),
    ("_api_key_id", &[2]),
    ("_authenticate", &[2]),
    ("_autofollow", &[2]),
    ("_builtin", &[2]),
    ("_bulk", &[2, 3]),
    ("_bulk_update", &[2]),
    ("_cancel", &[2, 3]),
    ("_check_in", &[2, 3]),
    ("_claim", &[3]),
    ("_cleanup", &[2]),
    ("_clear", &[5]),
    ("_clear_cache", &[3, 4, 7]),
    ("_clearcache", &[2]),
    ("_clone", &[3]),
    ("_close", &[3, 4]),
    ("_configuration", &[2]),
    ("_count", &[2]),
    ("_create", &[3]),
    ("_createmodel", &[3, 4]),
    ("_data", &[3, 4]),
    ("_datasources", &[2]),
    ("_deactivate", &[3, 4]),
    ("_delete_by_query", &[2, 6]),
    ("_delete_expired_data", &[2]),
    ("_deploy", &[4]),
    ("_deprovision", &[4]),
    ("_describe", &[2]),
    ("_disable", &[2, 3, 4]),
    ("_enable", &[2, 3, 4]),
    ("_error", &[2, 3]),
    ("_estimate_model_memory", &[2]),
    ("_evaluate", &[2]),
    ("_execute", &[2, 3, 4]),
    ("_explain", &[2, 3, 4]),
    ("_explore", &[3, 4]),
    ("_feature", &[2]),
    ("_features", &[2]),
    ("_featureset", &[2]),
    ("_filtering", &[2]),
    ("_fleet_msearch", &[2]),
    ("_fleet_search", &[2]),
    ("_flush", &[3, 4]),
    ("_forecast", &[3, 4]),
    ("_graph", &[2]),
    ("_has_privileges", &[2, 3, 4]),
    ("_index_name", &[2]),
    ("_infer", &[3]),
    ("_last_sync", &[2]),
    ("_latest", &[2]),
    ("_lifecycle", &[2]),
    ("_load", &[4]),
    ("_mapping", &[2]),
    ("_mappings", &[2]),
    ("_mget", &[2]),
    ("_model", &[2]),
    ("_mount", &[3]),
    ("_msearch", &[2]),
    ("_mtermvectors", &[2]),
    ("_name", &[2]),
    ("_native", &[2]),
    ("_open", &[3, 4]),
    ("_options", &[2]),
    ("_password", &[2, 3, 4]),
    ("_pause", &[2, 3]),
    ("_pipeline", &[2]),
    ("_predict", &[2, 4]),
    ("_preview", &[2, 3, 4]),
    ("_privileges", &[2, 3]),
    ("_profile", &[4]),
    ("_provision", &[4]),
    ("_register", &[3]),
    ("_register_meta", &[3]),
    ("_render_query", &[3]),
    ("_repositories_metering", &[2]),
    ("_reset", &[2, 3]),
    ("_restart", &[2]),
    ("_restore", &[3]),
    ("_resume", &[2, 3]),
    ("_rethrottle", &[2]),
    ("_revert", &[5, 6]),
    ("_rollup_search", &[2]),
    ("_rule", &[2]),
    ("_schedule_now", &[2]),
    ("_scheduling", &[2]),
    ("_search", &[2, 3, 4, 6]),
    ("_service_type", &[2]),
    ("_settings", &[2, 5]),
    ("_simulate", &[2, 3]),
    ("_source", &[3]),
    ("_start", &[2, 3, 4]),
    ("_state", &[2]),
    ("_stats", &[2, 3, 4, 6]),
    ("_status", &[2, 3, 4]),
    ("_steps", &[3]),
    ("_stop", &[2, 3, 4]),
    ("_stream", &[3]),
    ("_suggest", &[2]),
    ("_termvectors", &[2, 3]),
    ("_test", &[2]),
    ("_topAnomalies", &[5]),
    ("_topForecasts", &[5]),
    ("_train", &[2, 3, 4]),
    ("_train_predict", &[2]),
    ("_undeploy", &[3, 4]),
    ("_unload", &[3, 4]),
    ("_update", &[2, 3, 4, 5, 6]),
    ("_update_by_query", &[2]),
    ("_upgrade", &[5]),
    ("_upgrade_check", &[3]),
    ("_upgrade_perform", &[3]),
    ("_upload", &[2, 3]),
    ("_validate", &[2, 3]),
    ("_validation", &[3]),
    ("_verify", &[2]),
    ("_verify_integrity", &[2]),
    ("_xpack", &[2]),
];

/// The values starting with `_` the engines take for a name: every index, metric, snapshot or
/// job (`_all`), this node, the elected master, the running snapshot. Also as a comma list.
const SELECTORS: &[&str] = &["_all", "_local", "_master", "_current"];

/// Query parameters that make any request a write.
pub const WRITE_PARAMS: &[&str] = &[
    "refresh",
    "source",
    "search_pipeline",
    "keep_on_completion",
    "wait_for_completion_timeout",
];

/// The SQL words a read-only `_sql` query may start with.
const SQL_READ_STARTS: &[&str] = &["SELECT", "SHOW", "DESCRIBE", "DESC"];
/// Words that never appear as code in a read-only `_sql` query (OpenSearch runs `DELETE`).
const SQL_WRITE_WORDS: &[&str] = &[
    "DELETE", "INSERT", "UPDATE", "UPSERT", "REPLACE", "MERGE", "CREATE", "DROP", "ALTER",
    "TRUNCATE", "INTO", "GRANT", "REVOKE", "SET", "RESET", "CALL", "EXEC", "EXECUTE", "LOAD",
    "COPY", "ATTACH", "KILL", "OPTIMIZE", "REFRESH", "FLUSH", "CLOSE", "OPEN",
];

/// Classifies one console statement; text that does not parse is a write. A SQL statement reads
/// when [`sql_reads`] says so.
pub fn classify_text(text: &str) -> StatementClass {
    match console::parse_command(text) {
        Ok(console::Command::Request(req)) => classify(&req),
        Ok(console::Command::Sql(sql)) if sql_reads(&sql) => StatementClass::Read,
        Ok(console::Command::Sql(_)) | Err(_) => StatementClass::Write,
    }
}

pub fn classify(req: &EsRequest) -> StatementClass {
    if reads(req) {
        StatementClass::Read
    } else {
        StatementClass::Write
    }
}

/// A request reads when its path reads as the cluster gets it and as a decoding reverse proxy
/// would pass it on (see [`proxied`]).
fn reads(req: &EsRequest) -> bool {
    let segs: Vec<&str> = req.segments.iter().map(String::as_str).collect();
    reads_as(req, &segs, &req.query)
        && proxied(req).is_none_or(|(segs, query)| {
            let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
            reads_as(req, &segs, &query)
        })
}

/// The path and query a reverse proxy in front of the cluster (`pathPrefix`) hands on when it
/// decodes the path first, as nginx's `proxy_pass http://es:9200/;` does: `%2F` and `%5C` become
/// separators (`GET /logs%2F_flush` reaches the cluster as `GET /logs/_flush`) and `%3F` starts
/// the query. `None` when no segment holds `/`, `\` or `?`. The cluster itself decodes segment
/// by segment, so a document id `a%2Fb` stays one id there.
fn proxied(req: &EsRequest) -> Option<(Vec<String>, QueryPairs)> {
    if !req.segments.iter().any(|s| s.contains(['/', '\\', '?'])) {
        return None;
    }
    let path = req.segments.join("/");
    let (path, decoded_query) = match path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path.as_str(), None),
    };
    let segs = path
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let mut query: QueryPairs = decoded_query
        .into_iter()
        .flat_map(|q| q.split(['&', '?']))
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (p.to_string(), None),
        })
        .collect();
    query.extend(req.query.iter().cloned());
    Some((segs, query))
}

/// Whether `req` reads with the path `segs` and the query `query`.
fn reads_as(req: &EsRequest, segs: &[&str], query: &QueryPairs) -> bool {
    if !known_endpoint(segs) {
        return false;
    }
    if segs.iter().any(|s| is_action_segment(s)) || acting_get(segs) {
        return false;
    }
    if query.iter().any(|(k, v)| write_param(k, v.as_deref())) {
        return false;
    }
    // `GET _async_search/<id>?keep_alive=` (and the EQL, ES|QL, SQL and OpenSearch async gets)
    // extends the stored result; only opening a point in time takes it as a plain read.
    if query
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("keep_alive"))
        && !opens_pit(segs)
    {
        return false;
    }
    if req.body.as_ref().is_some_and(body_calls_out) {
        return false;
    }
    if is_sql(segs) && !sql_body_reads(req.body.as_ref(), query) {
        return false;
    }
    if segs.contains(&"_analyze") && !(analyze_shape(segs) && analyze_body_reads(req)) {
        return false;
    }
    match req.method {
        Method::Get | Method::Head => true,
        Method::Post => post_reads(segs),
        Method::Put | Method::Delete | Method::Patch | Method::Options => false,
    }
}

/// Whether every `_` word of the path is one the engines define there. A plugin can register any
/// path, and some act on `GET`: NLPchina's SQL plugin (`_nlpcn/sql`) runs `DELETE`, zentity's
/// `_zentity/models` creates its index, codelibs' `<index>/_data?file=` writes a file of the
/// server.
///
/// - The first segment is an index when it does not start with `_` (index and alias names never
///   do) or is `_all`; then the first other `_` word is the endpoint and must be an index's.
/// - Otherwise it is a top-level endpoint, and after `_plugins` / `_opendistro` comes the
///   plugin's name.
/// - Every later `_` segment must be a word the specifications define at that depth, or a
///   selector. The server's `PathTrie` falls back to a placeholder when the explicit branch does
///   not match, so a word is not tied to the endpoint before it: `/_all/_data`, `/_cat/_data`
///   and `/_nodes/_data` reach codelibs' `/{index}/_data` (the index `_cat` is missing, and with
///   `ignore_unavailable` the file is still written), and `/logs/_doc/_x` a plugin's
///   `/{index}/{type}/_x` before the document get. A name or id starting with `_` is a write.
///
/// Matched exactly, as the server routes it.
pub fn known_endpoint(segs: &[&str]) -> bool {
    let Some(&first) = segs.first() else {
        return true;
    };
    let endpoint = if first.starts_with('_') && first != "_all" {
        TOP_ENDPOINTS.contains(&first)
            && (!matches!(first, "_plugins" | "_opendistro")
                || segs.get(1).is_some_and(|p| PLUGINS.contains(p)))
    } else {
        segs.iter()
            .find(|s| s.starts_with('_') && !is_selector(s))
            .is_none_or(|w| INDEX_ENDPOINTS.contains(w))
    };
    endpoint
        && segs
            .iter()
            .enumerate()
            .skip(1)
            .all(|(depth, w)| !w.starts_with('_') || is_selector(w) || defined_at(first, depth, w))
}

/// Whether the specifications define the `_` word `w` at this depth of a path that starts with
/// `first`.
fn defined_at(first: &str, depth: usize, w: &str) -> bool {
    if depth == 1 {
        INDEX_ENDPOINTS.contains(&w)
            || TOP_PAIRS.contains(&(first, w))
            || (matches!(first, "_plugins" | "_opendistro") && PLUGINS.contains(&w))
    } else {
        DEEP_WORDS
            .iter()
            .any(|(word, at)| *word == w && at.contains(&depth))
    }
}

fn is_selector(s: &str) -> bool {
    s.split(',').all(|p| SELECTORS.contains(&p))
}

fn is_action_segment(s: &str) -> bool {
    ACTION_SEGMENTS.iter().any(|a| a.eq_ignore_ascii_case(s))
}

/// `GET` endpoints that act or cannot be judged:
/// - the k-NN warmup (loads graphs into memory) and the notification test (sends a message to
///   the channel) of OpenSearch;
/// - security enrollment (`GET _security/enroll/kibana` mints a service-account token, `node`
///   hands out the cluster's keys);
/// - templated searches (`_search/template`, `_msearch/template`, codelibs'
///   `_search/script_template`, `_rank_eval`) and search applications
///   (`_application/search_application/<name>/_search`): a stored template can hold any query, a
///   model call included, and is not in the request;
/// - data frame analytics `_explain` (`_ml/data_frame/analytics[/<id>]/_explain`): it runs the
///   (stored) source query and starts a memory estimation process on an ML node.
fn acting_get(segs: &[&str]) -> bool {
    let has = |w: &str| segs.iter().any(|s| s.eq_ignore_ascii_case(w));
    // `template`, and a plugin's templated search under the engine's `_search`.
    let templated = segs.windows(2).any(|w| {
        (w[0].eq_ignore_ascii_case("_search") || w[0].eq_ignore_ascii_case("_msearch"))
            && w[1].to_ascii_lowercase().ends_with("template")
    });
    (has("_knn") && has("warmup"))
        || (has("_notifications") && has("test"))
        || (has("_security") && has("enroll"))
        || templated
        || has("_rank_eval")
        || (has("_application") && has("_search"))
        || (has("_ml") && has("_explain"))
}

/// Opening a point in time (`<index>/_pit`, OpenSearch `<index>/_search/point_in_time`).
fn opens_pit(segs: &[&str]) -> bool {
    matches!(segs, [i, "_pit"] | [i, "_search", "point_in_time"] if index_ok(i))
}

fn write_param(key: &str, value: Option<&str>) -> bool {
    if !WRITE_PARAMS.iter().any(|p| p.eq_ignore_ascii_case(key)) {
        return false;
    }
    // `refresh=false` is the default and refreshes nothing.
    !(key == "refresh" && value == Some("false"))
}

fn body_calls_out(body: &Body) -> bool {
    body.values().iter().any(|v| has_remote_key(v, 0))
}

fn has_remote_key(v: &Json, depth: usize) -> bool {
    match v {
        // The parser bounds nesting; this only keeps the walk cheap.
        _ if depth > super::json::MAX_DEPTH => true,
        Json::Obj(pairs) => pairs.iter().any(|(k, v)| {
            REMOTE_KEYS.contains(&k.as_str())
                || infers_from_mapping(k, v)
                || has_remote_key(v, depth + 1)
        }),
        Json::Arr(items) => items.iter().any(|v| has_remote_key(v, depth + 1)),
        _ => false,
    }
}

/// Query shapes whose text the target field's own inference endpoint turns into a vector,
/// although the request names no model:
/// - `sparse_vector` with `query` text (on a `semantic_text` field; any other field needs an
///   `inference_id`, and a precomputed vector is `query_vector`);
/// - the multi-field form of the `rrf` and `linear` retrievers (`query` with `fields`, or the
///   index's default fields; Elasticsearch 9.1+), which matches every `semantic_text` field it
///   reaches. The other forms nest `retrievers`, and the old `rank.rrf` has no `query`.
fn infers_from_mapping(key: &str, v: &Json) -> bool {
    matches!(key, "sparse_vector" | "rrf" | "linear")
        && matches!(v, Json::Obj(pairs) if pairs.iter().any(|(k, _)| k == "query"))
}

/// An index (or index pattern) segment: anything but an endpoint name, `_all` aside.
fn index_ok(s: &str) -> bool {
    !s.is_empty() && (s == "_all" || !s.starts_with('_'))
}

/// A mapping type, document id, scroll id or template id: never an endpoint name, so a path
/// cannot be read as another endpoint's.
fn name_ok(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('_')
}

/// The `POST` endpoints that only read.
pub fn post_reads(segs: &[&str]) -> bool {
    const TOP: &[&str] = &[
        "_search",
        "_count",
        "_msearch",
        "_mget",
        "_field_caps",
        "_mtermvectors",
    ];
    const PER_INDEX: &[&str] = &[
        "_search",
        "_count",
        "_msearch",
        "_mget",
        "_field_caps",
        "_mtermvectors",
        "_termvectors",
        "_terms_enum",
        "_search_shards",
        "_pit",
    ];
    const PER_TYPE: &[&str] = &["_search", "_count", "_msearch", "_mget", "_mtermvectors"];
    if is_sql(segs) {
        return true;
    }
    match segs {
        [e] => TOP.contains(e) || *e == "_analyze",
        ["_validate", "query"] | ["_search", "scroll"] | ["_render", "template"] => true,
        ["_search", "scroll", id] | ["_render", "template", id] => name_ok(id),
        [i, e] => index_ok(i) && (PER_INDEX.contains(e) || *e == "_analyze"),
        [i, "_validate", "query"] | [i, "_search", "point_in_time"] => index_ok(i),
        [i, "_explain" | "_termvectors", id] => index_ok(i) && name_ok(id),
        [i, t, e] => index_ok(i) && name_ok(t) && PER_TYPE.contains(e),
        [i, t, "_validate", "query"] => index_ok(i) && name_ok(t),
        [i, t, id, "_explain" | "_termvectors"] => index_ok(i) && name_ok(t) && name_ok(id),
        _ => false,
    }
}

/// The SQL endpoints of both engines (`_sql`, the 6.x `_xpack/sql`, OpenSearch's
/// `_plugins/_sql` and `_opendistro/_sql`), with `translate` / `_explain`.
pub fn is_sql(segs: &[&str]) -> bool {
    matches!(
        segs,
        ["_sql"]
            | ["_sql", "translate"]
            | ["_xpack", "sql"]
            | ["_xpack", "sql", "translate"]
            | ["_plugins" | "_opendistro", "_sql"]
            | ["_plugins" | "_opendistro", "_sql", "_explain"]
    )
}

/// A `_sql` request reads when every query it carries reads, or when it only pages an open
/// cursor. The queries: body `query`, every `sql` parameter (OpenSearch's legacy GET), and body
/// `sql`: NLPchina's SQL plugin, which serves `_sql` before 7.5, copies the body's string keys
/// into the parameters and runs `sql`, whatever `query` says.
fn sql_body_reads(body: Option<&Body>, query: &QueryPairs) -> bool {
    let mut queries: Vec<&str> = query
        .iter()
        .filter(|(k, _)| k == "sql")
        .map(|(_, v)| v.as_deref().unwrap_or(""))
        .collect();
    let mut cursor = false;
    match body {
        None => {}
        Some(Body::Json(Json::Obj(pairs))) => {
            for (k, v) in pairs {
                match k.as_str() {
                    "query" | "sql" => match v.as_str() {
                        Some(q) => queries.push(q),
                        None => return false,
                    },
                    "cursor" => cursor = true,
                    _ => {}
                }
            }
        }
        Some(_) => return false,
    }
    if queries.is_empty() {
        return cursor;
    }
    queries.iter().all(|q| sql_reads(q))
}

/// Whether an Elasticsearch / OpenSearch SQL query only reads: it starts with `SELECT`, `SHOW`,
/// `DESCRIBE` or `DESC`, is one statement, and no write word is code anywhere in it. Strings,
/// quoted names and comments are skipped; anything unterminated is a write.
pub fn sql_reads(query: &str) -> bool {
    let c: Vec<char> = query.chars().collect();
    let mut words: Vec<String> = Vec::new();
    let mut i = 0;
    let mut after_semicolon = false;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '-' && c.get(i + 1) == Some(&'-') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && c.get(i + 1) == Some(&'*') {
            match find(&c, i + 2, &['*', '/']) {
                Some(end) => i = end + 2,
                None => return false,
            }
        } else if matches!(ch, '\'' | '"' | '`') {
            // Doubled quotes escape; Elasticsearch SQL has no backslash escapes.
            let mut j = i + 1;
            loop {
                match c.get(j) {
                    None => return false,
                    Some(&q) if q == ch && c.get(j + 1) == Some(&ch) => j += 2,
                    Some(&q) if q == ch => break,
                    Some(_) => j += 1,
                }
            }
            i = j + 1;
            if after_semicolon {
                return false;
            }
        } else if ch == ';' {
            after_semicolon = true;
            i += 1;
        } else if ch.is_ascii_alphanumeric() || ch == '_' || !ch.is_ascii() {
            let start = i;
            while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '_' || !c[i].is_ascii()) {
                i += 1;
            }
            if after_semicolon {
                return false;
            }
            words.push(c[start..i].iter().collect::<String>().to_ascii_uppercase());
        } else {
            if after_semicolon {
                return false;
            }
            i += 1;
        }
    }
    let Some(first) = words.first() else {
        return false;
    };
    SQL_READ_STARTS.contains(&first.as_str())
        && !words.iter().any(|w| SQL_WRITE_WORDS.contains(&w.as_str()))
}

fn find(c: &[char], from: usize, pat: &[char]) -> Option<usize> {
    (from..c.len()).find(|&i| c[i..].starts_with(pat))
}

fn analyze_shape(segs: &[&str]) -> bool {
    matches!(segs, ["_analyze"]) || matches!(segs, [i, "_analyze"] if index_ok(i))
}

/// `_analyze` reads with analysis components named by string only; an inline definition (an
/// object) may load a file of the server (`synonyms_path`, `stopwords_path`, `rules_path`…).
fn analyze_body_reads(req: &EsRequest) -> bool {
    const COMPONENTS: &[&str] = &[
        "tokenizer",
        "filter",
        "char_filter",
        "token_filter",
        "token_filters",
        "char_filters",
        "filters",
        "normalizer",
        "analyzer",
    ];
    match &req.body {
        None => true,
        Some(Body::Json(Json::Obj(pairs))) => pairs.iter().all(|(k, v)| {
            !COMPONENTS.contains(&k.as_str())
                || match v {
                    Json::Str(_) => true,
                    Json::Arr(items) => items.iter().all(|i| matches!(i, Json::Str(_))),
                    _ => false,
                }
        }),
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(text: &str) -> StatementClass {
        classify_text(text)
    }

    #[track_caller]
    fn read(text: &str) {
        assert_eq!(class(text), StatementClass::Read, "{text}");
    }

    #[track_caller]
    fn write(text: &str) {
        assert_eq!(class(text), StatementClass::Write, "{text}");
    }

    #[test]
    fn reads() {
        for t in [
            "GET /",
            "HEAD /logs",
            "GET _cat/indices?format=json&bytes=b",
            "GET /logs/_mapping",
            "GET /logs/_settings?flat_settings=true",
            "GET /logs/_doc/1",
            "GET /logs/_doc/1?refresh=false",
            "GET /_cluster/health?wait_for_status=yellow",
            "GET /_cluster/health?wait_for_nodes=1000&timeout=30s",
            "GET /_tasks?detailed=true",
            "GET /logs/_search\n{\"query\":{\"match\":{\"msg\":\"x\"}}}",
            "POST /logs/_search\n{\"query\":{\"match_all\":{}},\"size\":5}",
            "POST /logs,metrics/_search?scroll=1m",
            "POST /_all/_count",
            "POST /logs/_count\n{\"query\":{\"term\":{\"a\":1}}}",
            "POST _msearch\n{\"index\":\"a\"}\n{\"query\":{\"match_all\":{}}}",
            "POST /logs/_mget\n{\"ids\":[\"1\",\"2\"]}",
            "POST /logs/_field_caps?fields=*",
            "POST /logs/_validate/query?explain\n{\"query\":{\"match_all\":{}}}",
            "POST /logs/_pit?keep_alive=1m",
            "POST /logs/_search/point_in_time?keep_alive=1m",
            "POST /_search/scroll\n{\"scroll\":\"1m\",\"scroll_id\":\"abc\"}",
            "POST /logs/_explain/1\n{\"query\":{\"match_all\":{}}}",
            "POST /logs/_termvectors/1",
            "POST /logs/_terms_enum\n{\"field\":\"tags\",\"string\":\"a\"}",
            "POST /logs/doc/_search",
            "POST /remote:logs/_search",
            "POST _render/template\n{\"source\":\"{}\"}",
            "POST _sql?format=json\n{\"query\":\"SELECT a FROM logs WHERE b = 'DELETE' LIMIT 5\"}",
            "POST _sql?format=txt\n{\"query\":\"\"\"SHOW TABLES\"\"\"}",
            "POST _sql\n{\"cursor\":\"sDXF1ZXJ5\"}",
            "POST _sql/translate\n{\"query\":\"DESCRIBE logs\"}",
            "POST _plugins/_sql\n{\"query\":\"SELECT * FROM `delete`\"}",
            "POST _plugins/_sql/_explain\n{\"query\":\"SELECT 1\"}",
            "GET _opendistro/_sql?sql=SELECT%20*%20FROM%20a",
            "POST /logs/_analyze\n{\"analyzer\":\"standard\",\"text\":\"x\"}",
            "POST _analyze\n{\"tokenizer\":\"standard\",\"filter\":[\"lowercase\"],\"text\":\"x\"}",
            "GET /_security/_authenticate",
            "GET /_snapshot/repo/_all",
            "GET /_async_search/abc",
            "GET /_transform/t/_stats",
            "GET /_ml/datafeeds/d/_stats",
            "GET /_ml/data_frame/analytics/a",
            "GET /_application/search_application/app",
            // Endpoints of the specifications, selectors, typed paths and OpenSearch plugins.
            "GET /_all/_mapping",
            "GET /_all",
            "GET /logs/doc/1/_source",
            "GET _nodes/_local/stats",
            "GET _nodes/_local,_master/stats",
            "GET /_snapshot/repo/_current",
            "GET /_snapshot/_all",
            "GET _ml/anomaly_detectors/_all/_stats",
            "GET _ml/trained_models/_all/_stats",
            "GET _security/_query/api_key",
            "GET _security/user/_privileges",
            "GET /_ltr/store/_featureset/fs",
            "GET _plugins/_anomaly_detection/detectors/d/_profile",
            "GET _plugins/_alerting/monitors/_search",
            "GET _plugins/_ism/explain/logs",
            "GET _opendistro/_security/sslinfo",
            "GET _plugins/_alerting/monitors/m",
            "GET _ltr/_cachestats",
            "GET _xpack/usage",
            // NLPchina's `_sql/_explain` (before 7.5) only translates, whatever the statement.
            "GET _sql/_explain?sql=DELETE%20FROM%20logs",
            // Vectors given in the request, and retrievers that nest their queries.
            "POST /logs/_search\n{\"query\":{\"sparse_vector\":{\"field\":\"s\",\"query_vector\":{\"a\":1.0}}}}",
            "POST /logs/_search\n{\"query\":{\"knn\":{\"field\":\"v\",\"k\":3,\"query_vector\":[0.1,0.2]}}}",
            "POST /logs/_search\n{\"retriever\":{\"rrf\":{\"retrievers\":[{\"standard\":{\"query\":{\"match_all\":{}}}}]}}}",
            "POST /logs/_search\n{\"query\":{\"match_all\":{}},\"rank\":{\"rrf\":{\"window_size\":50}}}",
        ] {
            read(t);
        }
    }

    #[test]
    fn writes() {
        for t in [
            "PUT /logs",
            "DELETE /logs",
            "PATCH /logs",
            "OPTIONS /logs",
            "POST /logs/_doc\n{\"a\":1}",
            "PUT /logs/_doc/1\n{\"a\":1}",
            "POST /logs/_update/1\n{\"doc\":{\"a\":1}}",
            "POST _bulk\n{\"delete\":{\"_index\":\"a\",\"_id\":\"1\"}}",
            "POST /logs/_update_by_query",
            "POST /logs/_delete_by_query\n{\"query\":{\"match_all\":{}}}",
            "POST /logs/_close",
            "POST /logs/_open",
            "POST _reindex\n{\"source\":{\"index\":\"a\"},\"dest\":{\"index\":\"b\"}}",
            "PUT _snapshot/repo/snap",
            "POST _snapshot/repo/snap/_restore",
            "POST _snapshot/repo/_analyze",
            // GET endpoints that act.
            "GET /logs/_flush",
            "GET /logs/_refresh",
            "GET /logs/_flush/synced",
            "GET /logs/_optimize",
            "GET /_cache/clear",
            "GET /logs/_FLUSH",
            "GET /logs/_fl%75sh",
            "HEAD /logs/_refresh",
            "GET _ingest/pipeline/p/_simulate\n{\"docs\":[]}",
            "GET _ingest/_simulate",
            "GET _scripts/painless/_execute\n{\"script\":{\"source\":\"1\"}}",
            "GET _plugins/_knn/warmup/logs",
            "GET _opendistro/_knn/warmup/logs",
            "GET _plugins/_notifications/feature/test/abc",
            "GET _security/enroll/kibana",
            "GET _security/enroll/node",
            "GET /logs/_search/template\n{\"id\":\"stored\"}",
            "GET _msearch/template\n{}\n{\"id\":\"t\"}",
            "GET /logs/_rank_eval\n{\"requests\":[]}",
            "GET _application/search_application/app/_search\n{\"params\":{\"q\":\"x\"}}",
            "GET _application/search_application/app/_search",
            // Previews run the destination pipeline or a stored query; DFA explain runs the
            // source query and an estimation process.
            "GET _transform/_preview\n{\"source\":{\"index\":\"a\"},\"dest\":{\"index\":\"b\",\"pipeline\":\"p\"},\"pivot\":{}}",
            "GET _transform/t/_preview",
            "GET _data_frame/transforms/t/_preview",
            "GET _ml/datafeeds/d/_preview",
            "GET _xpack/ml/datafeeds/d/_preview",
            "GET _ml/data_frame/analytics/_preview\n{\"config\":{}}",
            "GET _ml/data_frame/analytics/a/_preview",
            "GET _ml/data_frame/analytics/_explain\n{\"source\":{\"index\":\"a\"}}",
            "GET _ml/data_frame/analytics/a/_explain",
            // Extending a stored async result.
            "GET _async_search/abc?keep_alive=5d",
            "GET _query/async/abc?keep_alive=1d",
            "GET _sql/async/abc?keep_alive=5d",
            "GET _plugins/_asynchronous_search/abc?keep_alive=1d",
            "GET /logs/_doc/1?refresh",
            "GET /logs/_doc/1?refresh=true",
            "GET /_mget?refresh=wait_for",
            "GET /logs/_search?source=%7B%7D",
            "GET /logs/_search?search_pipeline=p",
            "POST /logs/_search?search_pipeline=p",
            // Search bodies that call a model or another service.
            "POST /logs/_search\n{\"query\":{\"semantic\":{\"field\":\"f\",\"query\":\"q\"}}}",
            "POST /logs/_search\n{\"query\":{\"neural\":{\"v\":{\"query_text\":\"q\",\"model_id\":\"m\"}}}}",
            "GET /logs/_search\n{\"knn\":{\"field\":\"v\",\"query_vector_builder\":{\"text_embedding\":{}}}}",
            "POST /logs/_search\n{\"retriever\":{\"text_similarity_reranker\":{}}}",
            "POST /logs/_search\n{\"ext\":{\"generative_qa_parameters\":{}}}",
            "POST _msearch\n{\"index\":\"a\"}\n{\"query\":{\"sparse_vector\":{\"inference_id\":\"x\"}}}",
            "POST /logs/_search\n{\"query\":{\"knn\":{\"field\":\"s\",\"k\":10,\"query_vector_builder\":{\"text_embedding\":{\"model_text\":\"q\"}}}}}",
            // Any of them (or any query) in base64: {"semantic":{"field":"body","query":"hello"}}.
            "POST /logs/_search\n{\"query\":{\"wrapper\":{\"query\":\"eyJzZW1hbnRpYyI6eyJmaWVsZCI6ImJvZHkiLCJxdWVyeSI6ImhlbGxvIn19\"}}}",
            "GET /logs/_search\n{\"query\":{\"bool\":{\"filter\":[{\"wrapper\":{\"query\":\"e30=\"}}]}}}",
            "POST _msearch\n{\"index\":\"a\"}\n{\"query\":{\"wrapper\":{\"query\":\"e30=\"}}}",
            "POST /logs/_count\n{\"query\":{\"wrapper\":{\"query\":\"e30=\"}}}",
            // Text the field's mapping infers, or stored feature queries.
            "POST /logs/_search\n{\"query\":{\"sparse_vector\":{\"field\":\"s\",\"query\":\"q\"}}}",
            "GET /logs/_search\n{\"retriever\":{\"rrf\":{\"query\":\"q\",\"fields\":[\"a\",\"s\"]}}}",
            "GET /logs/_search\n{\"retriever\":{\"linear\":{\"query\":\"q\",\"normalizer\":\"minmax\"}}}",
            "POST /logs/_search\n{\"query\":{\"sltr\":{\"featureset\":\"f\",\"params\":{\"k\":\"x\"},\"model\":\"m\"}}}",
            "POST /logs/_eql/search?keep_on_completion=true",
            // POST endpoints not on the list, or shapes that could be another endpoint's.
            "POST /logs/_async_search",
            "POST /_query\n{\"query\":\"FROM logs\"}",
            "POST _plugins/_ppl\n{\"query\":\"source=logs\"}",
            "POST /logs/_search/template\n{\"id\":\"t\"}",
            "POST /_bulk/_search",
            "POST /logs/_doc/_search",
            "POST /logs/_explain/_bulk",
            "POST /logs/_update/_search",
            "POST /_search/scroll/_bulk",
            "POST _sql/close\n{\"cursor\":\"x\"}",
            "POST _tasks/n:1/_cancel",
            "POST _inference/text_embedding/e\n{\"input\":\"x\"}",
            // SQL that writes or is not plain.
            "POST _plugins/_sql\n{\"query\":\"DELETE FROM logs\"}",
            "POST _opendistro/_sql\n{\"query\":\"SELECT 1; DELETE FROM logs\"}",
            "POST _sql\n{\"query\":\"SELECT * INTO x FROM y\"}",
            "POST _sql\n{\"query\":\"ſELECT 1\"}",
            "POST _sql\n{\"query\":\"SELECT 'unterminated\"}",
            "POST _sql\n{\"query\":\"SELECT 1 /* open\"}",
            "POST _sql\n{\"query\":1}",
            "POST _sql\n{\"fetch_size\":5}",
            "POST _sql\n{\"query\":\"SELECT 1\",\"keep_on_completion\":true}",
            "GET _plugins/_sql?sql=DELETE%20FROM%20a",
            "POST _sql\n[]",
            // NLPchina's SQL plugin before 7.5 runs the body's `sql` (or the last `sql` parameter).
            "POST _sql\n{\"query\":\"SELECT 1\",\"sql\":\"DELETE FROM logs\"}",
            "GET _sql\n{\"query\":\"SELECT 1\",\"sql\":\"DELETE FROM logs\"}",
            "GET _sql\n{\"sql\":\"DELETE FROM logs\"}",
            "POST _sql\n{\"query\":\"SELECT 1\",\"sql\":1}",
            "GET _sql?sql=SELECT%201&sql=DELETE%20FROM%20logs",
            "GET _sql?sql=DELETE%20FROM%20logs\n{\"query\":\"SELECT 1\"}",
            // Endpoints the engines do not define: plugins, which may act on GET.
            "GET /_nlpcn/sql?sql=DELETE%20FROM%20logs",
            "HEAD /_nlpcn/sql?sql=DELETE%20FROM%20logs",
            "GET /_nlpcn/sql\n{\"sql\":\"DELETE FROM logs\"}",
            "GET /_nlpcn/sql?sql=SELECT%20*%20FROM%20logs",
            "GET /_nlpcn/sql/explain?sql=SELECT%201",
            "GET /_nlp%63n/sql?sql=DELETE%20FROM%20logs",
            "GET /_zentity/models",
            "GET /logs/_data?format=csv&file=/tmp/out.csv",
            "GET /_readonlyrest/admin/config",
            "GET /_prometheus/metrics",
            "GET /logs/_search_with_clusters",
            "GET /_plugins/_directquery/_resources/prom/api/v1/labels",
            "GET /_plugins/_async_query/q1",
            "GET /_opendistro/_unknown/x",
            "GET /_plugins",
            "GET /_CAT/indices",
            "GET /_search/script_template\n{\"id\":\"t\"}",
            "GET /logs/_search/script_template",
            // A plugin word anywhere in the path: `_all` is an index, and the server's PathTrie
            // falls back to a placeholder, so `_cat` or `_search` can be the index (or the 6.x
            // type) of codelibs' `/{index}/_data` and `/{index}/{type}/_data`.
            "GET /_all/_data?format=csv&file=/tmp/out.csv",
            "HEAD /_all/_data?format=csv&file=/tmp/out.csv",
            "GET /_all/doc/_data?format=csv&file=/tmp/out.csv",
            "GET /_all/_search/_data?file=/tmp/out.csv",
            "GET /_cat/_data?format=csv&file=/tmp/out.csv&ignore_unavailable=true",
            "GET /_search/_data?format=csv&file=/tmp/out.csv&ignore_unavailable=true",
            "GET /_nodes/_data?format=csv&file=/tmp/out.csv&ignore_unavailable=true",
            "HEAD /_cat/_data?file=/tmp/out.csv",
            "GET /logs/_search/_data?file=/tmp/out.csv",
            "GET /logs/_doc/_data?file=/tmp/out.csv",
            "GET /_cat/indices/_data",
            "GET /_plugins/_ism/_data",
            "GET /_all,logs/_data",
            // A word the engines define elsewhere (`_ltr/_featureset`), and an id that starts
            // with `_`, which can be a plugin's word (`/{index}/{type}/_x`).
            "GET /_cat/_featureset",
            "GET /logs/_doc/_x",
            // Search extensions that store the query or call AWS.
            "POST /logs/_search\n{\"query\":{\"match_all\":{}},\"ext\":{\"ubi\":{\"query_id\":\"q\"}}}",
            "POST /logs/_search\n{\"ext\":{\"search_configuration\":{\"result_transformer\":{}}}}",
            "POST /logs/_search\n{\"ext\":{\"personalize_request_parameters\":{\"user_id\":\"u\"}}}",
            // Analysis that can read files.
            "POST _analyze\n{\"tokenizer\":\"standard\",\"filter\":[{\"type\":\"synonym\",\"synonyms_path\":\"s.txt\"}],\"text\":\"x\"}",
            "GET /logs/_analyze\n{\"char_filter\":[{\"type\":\"mapping\",\"mappings_path\":\"m.txt\"}]}",
            "GET /_snapshot/repo/_analyze",
            // Unparseable.
            "",
            "hello",
            "GET /a/../_bulk",
            "POST /a/_search\n{",
        ] {
            write(t);
        }
    }

    /// The cluster decodes segment by segment, but a reverse proxy in front of it (`pathPrefix`)
    /// may decode `%2F`, `%5C` and `%3F` first and resolve dot segments: what it would pass on
    /// must read too.
    #[test]
    fn paths_a_decoding_proxy_would_change() {
        for t in [
            "GET /logs%2F_flush",
            "GET /x%2F..%2F_flush",
            "GET /x%2F..%2F_security%2Fenroll%2Fkibana",
            "GET /logs/_doc/x%2F..%2F..%2F_cache%2Fclear",
            "POST /logs/_explain/x%2F..%2F..%2F_delete_by_query\n{\"query\":{\"match_all\":{}}}",
            "POST /logs/_termvectors/x%2F..%2F..%2F_update_by_query",
            "GET /logs/_doc/a%5C..%5C..%5C_flush",
            // Without dot parts the proxy still splits the segment.
            "POST /logs/_explain/x%2F_delete_by_query\n{\"query\":{\"match_all\":{}}}",
            "POST /logs/_termvectors/x%5C_update_by_query",
            "POST /logs%2F_update_by_query/_search",
            "GET /logs/_doc/a%2F_flush",
            "GET /logs/_doc/a%2F_x",
            "HEAD /logs/_doc/a%5C_refresh",
            // `%3F` starts the query behind a proxy.
            "GET /logs/_doc/a%3Frefresh=true",
            "GET /logs/_search%3Fsearch_pipeline=p",
            "GET /_async_search/abc%3Fkeep_alive=5d",
            "GET /_plugins/_sql%3Fsql=DELETE%20FROM%20logs",
        ] {
            write(t);
        }
        for t in [
            // Ids and date math with slashes stay reads.
            "GET /logs/_doc/a%2Fb",
            "GET /pages/_doc/https%3A%2F%2Fexample.com%2Fp%3Fq%3D1",
            "GET /%3Clogs-%7Bnow%2Fd%7D%3E/_search",
            "POST /%3Clogs-%7Bnow%2Fd%7D%3E/_search\n{\"query\":{\"match_all\":{}}}",
            "GET /logs/_source/a%5Cb",
        ] {
            read(t);
        }
    }

    #[test]
    fn vocabulary_tables_are_consistent() {
        for (top, w) in TOP_PAIRS {
            assert!(TOP_ENDPOINTS.contains(top), "{top}");
            assert!(w.starts_with('_') && !INDEX_ENDPOINTS.contains(w), "{w}");
        }
        for (w, at) in DEEP_WORDS {
            assert!(w.starts_with('_') && !SELECTORS.contains(w), "{w}");
            assert!(!at.is_empty() && at.iter().all(|d| *d >= 2), "{w}");
        }
        assert!(!TOP_ENDPOINTS.contains(&"_all"));
    }

    #[test]
    fn sql_check_is_conservative() {
        assert!(sql_reads(
            "SELECT a FROM \"t\" WHERE x = 'it''s' -- DELETE\n"
        ));
        assert!(sql_reads("  show tables"));
        assert!(sql_reads("SELECT 1;"));
        assert!(!sql_reads("SELECT 1; SELECT 2"));
        assert!(!sql_reads("WITH x AS (SELECT 1) SELECT * FROM x"));
        assert!(!sql_reads(""));
        assert!(!sql_reads("-- only a comment"));
        assert!(!sql_reads("SELECT `a FROM t"));
        assert!(!sql_reads("select * from t where x in (delete)"));
    }

    #[test]
    fn console_sql_statements() {
        read("SELECT a FROM logs WHERE b = 'x'");
        read("# all tables\nSHOW TABLES;");
        read("describe logs");
        write("SELECT 1; DELETE FROM logs");
        write("SELECT * FROM logs WHERE a IN (SELECT 1) INTO x");
        write("SELECT 'unterminated");
        // Not a SQL word: a request that does not parse.
        write("DELETE FROM logs");
        write("TRUNCATE logs");
    }
}
