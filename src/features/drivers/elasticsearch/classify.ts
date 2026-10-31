import type { StatementClass } from "@/features/safety/classify";
import { MAX_DEPTH, type Json } from "./json";
import { bodyValues, parseCommand, type Body, type EsRequest, type QueryPairs } from "./request";
import {
  asciiLower,
  asciiUpper,
  eqIgnoreAsciiCase,
  isAscii,
  isAsciiAlphanumeric,
  isWhitespace,
} from "./rust";

/**
 * Whether a console statement only reads, mirroring the backend's
 * `drivers/elasticsearch/classify.rs` rule for rule (default-deny: what it does not know is a
 * write). Both run the shared cases of `src-tauri/tests/fixtures/elasticsearch/console.cases.json`.
 * Keep the word lists in step with the backend's.
 */

/** Path segments that act on the cluster whatever the method (ignoring ASCII case). */
const ACTION_SEGMENTS = [
  "_flush",
  "_refresh",
  "_optimize",
  "_forcemerge",
  "_cache",
  "_clear_cache",
  "_execute",
  "_execute_retention",
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

/** Body keys that call a model or another service, or store a result on the server. */
export const REMOTE_KEYS = new Set([
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
  "sltr",
  "ubi",
  "search_configuration",
  "personalize_request_parameters",
  "keep_on_completion",
  "wait_for_completion_timeout",
  "wrapper",
]);

const TOP_ENDPOINTS = new Set([
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
]);

const INDEX_ENDPOINTS = new Set([
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
]);

const PLUGINS = new Set([
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
]);

const TOP_PAIRS = new Set(
  [
    ["_connector", "_secret"],
    ["_connector", "_sync_job"],
    ["_data_stream", "_migrate"],
    ["_data_stream", "_modify"],
    ["_data_stream", "_promote"],
    ["_encryption", "_reset"],
    ["_features", "_reset"],
    ["_fleet", "_fleet_msearch"],
    ["_index_template", "_simulate"],
    ["_index_template", "_simulate_index"],
    ["_inference", "_region_policy"],
    ["_ingest", "_simulate"],
    ["_ltr", "_cachestats"],
    ["_ltr", "_clearcache"],
    ["_ltr", "_feature"],
    ["_ltr", "_featureset"],
    ["_ltr", "_model"],
    ["_ml", "_delete_expired_data"],
    ["_remotestore", "_restore"],
    ["_security", "_authenticate"],
    ["_security", "_query"],
    ["_slm", "_execute_retention"],
    ["_snapshot", "_status"],
    ["_tasks", "_cancel"],
    ["_transform", "_node_stats"],
    ["_transform", "_preview"],
    ["_watcher", "_query"],
    ["_watcher", "_start"],
    ["_watcher", "_stop"],
  ].map(([a, b]) => `${a}\u0000${b}`),
);

/** The `_` words defined deeper in a path, with their depths (the first segment is 0). */
const DEEP_WORDS = new Map<string, readonly number[]>([
  ["_ack", [3, 4]],
  ["_activate", [2, 3, 4]],
  ["_addfeatures", [3, 4]],
  ["_analyze", [2]],
  ["_api_key_id", [2]],
  ["_authenticate", [2]],
  ["_autofollow", [2]],
  ["_builtin", [2]],
  ["_bulk", [2, 3]],
  ["_bulk_update", [2]],
  ["_cancel", [2, 3]],
  ["_check_in", [2, 3]],
  ["_claim", [3]],
  ["_cleanup", [2]],
  ["_clear", [5]],
  ["_clear_cache", [3, 4, 7]],
  ["_clearcache", [2]],
  ["_clone", [3]],
  ["_close", [3, 4]],
  ["_configuration", [2]],
  ["_count", [2]],
  ["_create", [3]],
  ["_createmodel", [3, 4]],
  ["_data", [3, 4]],
  ["_datasources", [2]],
  ["_deactivate", [3, 4]],
  ["_delete_by_query", [2, 6]],
  ["_delete_expired_data", [2]],
  ["_deploy", [4]],
  ["_deprovision", [4]],
  ["_describe", [2]],
  ["_disable", [2, 3, 4]],
  ["_enable", [2, 3, 4]],
  ["_error", [2, 3]],
  ["_estimate_model_memory", [2]],
  ["_evaluate", [2]],
  ["_execute", [2, 3, 4]],
  ["_explain", [2, 3, 4]],
  ["_explore", [3, 4]],
  ["_feature", [2]],
  ["_features", [2]],
  ["_featureset", [2]],
  ["_filtering", [2]],
  ["_fleet_msearch", [2]],
  ["_fleet_search", [2]],
  ["_flush", [3, 4]],
  ["_forecast", [3, 4]],
  ["_graph", [2]],
  ["_has_privileges", [2, 3, 4]],
  ["_index_name", [2]],
  ["_infer", [3]],
  ["_last_sync", [2]],
  ["_latest", [2]],
  ["_lifecycle", [2]],
  ["_load", [4]],
  ["_mapping", [2]],
  ["_mappings", [2]],
  ["_mget", [2]],
  ["_model", [2]],
  ["_mount", [3]],
  ["_msearch", [2]],
  ["_mtermvectors", [2]],
  ["_name", [2]],
  ["_native", [2]],
  ["_open", [3, 4]],
  ["_options", [2]],
  ["_password", [2, 3, 4]],
  ["_pause", [2, 3]],
  ["_pipeline", [2]],
  ["_predict", [2, 4]],
  ["_preview", [2, 3, 4]],
  ["_privileges", [2, 3]],
  ["_profile", [4]],
  ["_provision", [4]],
  ["_register", [3]],
  ["_register_meta", [3]],
  ["_render_query", [3]],
  ["_repositories_metering", [2]],
  ["_reset", [2, 3]],
  ["_restart", [2]],
  ["_restore", [3]],
  ["_resume", [2, 3]],
  ["_rethrottle", [2]],
  ["_revert", [5, 6]],
  ["_rollup_search", [2]],
  ["_rule", [2]],
  ["_schedule_now", [2]],
  ["_scheduling", [2]],
  ["_search", [2, 3, 4, 6]],
  ["_service_type", [2]],
  ["_settings", [2, 5]],
  ["_simulate", [2, 3]],
  ["_source", [3]],
  ["_start", [2, 3, 4]],
  ["_state", [2]],
  ["_stats", [2, 3, 4, 6]],
  ["_status", [2, 3, 4]],
  ["_steps", [3]],
  ["_stop", [2, 3, 4]],
  ["_stream", [3]],
  ["_suggest", [2]],
  ["_termvectors", [2, 3]],
  ["_test", [2]],
  ["_topAnomalies", [5]],
  ["_topForecasts", [5]],
  ["_train", [2, 3, 4]],
  ["_train_predict", [2]],
  ["_undeploy", [3, 4]],
  ["_unload", [3, 4]],
  ["_update", [2, 3, 4, 5, 6]],
  ["_update_by_query", [2]],
  ["_upgrade", [5]],
  ["_upgrade_check", [3]],
  ["_upgrade_perform", [3]],
  ["_upload", [2, 3]],
  ["_validate", [2, 3]],
  ["_validation", [3]],
  ["_verify", [2]],
  ["_verify_integrity", [2]],
  ["_xpack", [2]],
]);

const SELECTORS = new Set(["_all", "_local", "_master", "_current"]);

const WRITE_PARAMS = [
  "refresh",
  "source",
  "search_pipeline",
  "keep_on_completion",
  "wait_for_completion_timeout",
];

const SQL_READ_STARTS = new Set(["SELECT", "SHOW", "DESCRIBE", "DESC"]);
const SQL_WRITE_WORDS = new Set([
  "DELETE",
  "INSERT",
  "UPDATE",
  "UPSERT",
  "REPLACE",
  "MERGE",
  "CREATE",
  "DROP",
  "ALTER",
  "TRUNCATE",
  "INTO",
  "GRANT",
  "REVOKE",
  "SET",
  "RESET",
  "CALL",
  "EXEC",
  "EXECUTE",
  "LOAD",
  "COPY",
  "ATTACH",
  "KILL",
  "OPTIMIZE",
  "REFRESH",
  "FLUSH",
  "CLOSE",
  "OPEN",
]);

/** Classifies one console statement; text that does not parse is a write (`classify_text`). */
export function classifyConsole(text: string): StatementClass {
  let command;
  try {
    command = parseCommand(text);
  } catch {
    return "write";
  }
  if (command.kind === "sql") return sqlReads(command.sql) ? "read" : "write";
  return classifyRequest(command.req);
}

export function classifyRequest(req: EsRequest): StatementClass {
  if (!readsAs(req, req.segments, req.query)) return "write";
  const proxy = proxied(req);
  return proxy === null || readsAs(req, proxy.segments, proxy.query) ? "read" : "write";
}

/** What a decoding reverse proxy passes on (`%2F`, `%5C` split, `%3F` starts the query). */
function proxied(req: EsRequest): { segments: string[]; query: QueryPairs } | null {
  if (!req.segments.some((s) => /[/\\?]/.test(s))) return null;
  const joined = req.segments.join("/");
  const q = joined.indexOf("?");
  const path = q < 0 ? joined : joined.slice(0, q);
  const segments = path.split(/[/\\]/).filter((s) => s !== "");
  const query: QueryPairs = (q < 0 ? [] : joined.slice(q + 1).split(/[&?]/))
    .filter((p) => p !== "")
    .map((p) => {
      const eq = p.indexOf("=");
      return eq < 0 ? [p, null] : [p.slice(0, eq), p.slice(eq + 1)];
    });
  return { segments, query: [...query, ...req.query] };
}

function readsAs(req: EsRequest, segs: readonly string[], query: QueryPairs): boolean {
  if (!knownEndpoint(segs)) return false;
  if (segs.some(isActionSegment) || actingGet(segs)) return false;
  if (query.some(([k, v]) => writeParam(k, v))) return false;
  if (query.some(([k]) => eqIgnoreAsciiCase(k, "keep_alive")) && !opensPit(segs)) return false;
  if (req.body && bodyCallsOut(req.body)) return false;
  if (isSql(segs) && !sqlBodyReads(req.body, query)) return false;
  if (segs.includes("_analyze") && !(analyzeShape(segs) && analyzeBodyReads(req))) return false;
  switch (req.method) {
    case "GET":
    case "HEAD":
      return true;
    case "POST":
      return postReads(segs);
    default:
      return false;
  }
}

const isSelector = (s: string) => s.split(",").every((p) => SELECTORS.has(p));

/** Whether every `_` word of the path is one the engines define there (`known_endpoint`). */
export function knownEndpoint(segs: readonly string[]): boolean {
  const first = segs[0];
  if (first === undefined) return true;
  let endpoint: boolean;
  if (first.startsWith("_") && first !== "_all") {
    endpoint =
      TOP_ENDPOINTS.has(first) &&
      (!(first === "_plugins" || first === "_opendistro") ||
        (segs[1] !== undefined && PLUGINS.has(segs[1])));
  } else {
    const w = segs.find((s) => s.startsWith("_") && !isSelector(s));
    endpoint = w === undefined || INDEX_ENDPOINTS.has(w);
  }
  return (
    endpoint &&
    segs.every(
      (w, depth) =>
        depth === 0 || !w.startsWith("_") || isSelector(w) || definedAt(first, depth, w),
    )
  );
}

function definedAt(first: string, depth: number, w: string): boolean {
  if (depth === 1)
    return (
      INDEX_ENDPOINTS.has(w) ||
      TOP_PAIRS.has(`${first}\u0000${w}`) ||
      ((first === "_plugins" || first === "_opendistro") && PLUGINS.has(w))
    );
  return DEEP_WORDS.get(w)?.includes(depth) ?? false;
}

const isActionSegment = (s: string) => ACTION_SEGMENTS.some((a) => eqIgnoreAsciiCase(a, s));

function actingGet(segs: readonly string[]): boolean {
  const has = (w: string) => segs.some((s) => eqIgnoreAsciiCase(s, w));
  const templated = segs.some(
    (s, i) =>
      i + 1 < segs.length &&
      (eqIgnoreAsciiCase(s, "_search") || eqIgnoreAsciiCase(s, "_msearch")) &&
      asciiLower(segs[i + 1]!).endsWith("template"),
  );
  return (
    (has("_knn") && has("warmup")) ||
    (has("_notifications") && has("test")) ||
    (has("_security") && has("enroll")) ||
    templated ||
    has("_rank_eval") ||
    (has("_application") && has("_search")) ||
    (has("_ml") && has("_explain"))
  );
}

const indexOk = (s: string) => s !== "" && (s === "_all" || !s.startsWith("_"));
const nameOk = (s: string) => s !== "" && !s.startsWith("_");

function opensPit(segs: readonly string[]): boolean {
  const [i, a, b] = segs;
  return (
    i !== undefined &&
    indexOk(i) &&
    ((segs.length === 2 && a === "_pit") ||
      (segs.length === 3 && a === "_search" && b === "point_in_time"))
  );
}

function writeParam(key: string, value: string | null): boolean {
  if (!WRITE_PARAMS.some((p) => eqIgnoreAsciiCase(p, key))) return false;
  // `refresh=false` is the default and refreshes nothing.
  return !(key === "refresh" && value === "false");
}

function bodyCallsOut(body: Body): boolean {
  return bodyValues(body).some((v) => hasRemoteKey(v, 0));
}

function hasRemoteKey(v: Json, depth: number): boolean {
  if (depth > MAX_DEPTH) return true;
  if (v.k === "obj")
    return v.v.some(
      ([k, x]) => REMOTE_KEYS.has(k) || infersFromMapping(k, x) || hasRemoteKey(x, depth + 1),
    );
  if (v.k === "arr") return v.v.some((x) => hasRemoteKey(x, depth + 1));
  return false;
}

function infersFromMapping(key: string, v: Json): boolean {
  return (
    (key === "sparse_vector" || key === "rrf" || key === "linear") &&
    v.k === "obj" &&
    v.v.some(([k]) => k === "query")
  );
}

const POST_TOP = ["_search", "_count", "_msearch", "_mget", "_field_caps", "_mtermvectors"];
const POST_PER_INDEX = [...POST_TOP, "_termvectors", "_terms_enum", "_search_shards", "_pit"];
const POST_PER_TYPE = ["_search", "_count", "_msearch", "_mget", "_mtermvectors"];

/** The `POST` endpoints that only read (`post_reads`). */
export function postReads(segs: readonly string[]): boolean {
  if (isSql(segs)) return true;
  const [a, b, c, d] = segs;
  switch (segs.length) {
    case 1:
      return POST_TOP.includes(a!) || a === "_analyze";
    case 2:
      if (
        (a === "_validate" && b === "query") ||
        (a === "_search" && b === "scroll") ||
        (a === "_render" && b === "template")
      )
        return true;
      return indexOk(a!) && (POST_PER_INDEX.includes(b!) || b === "_analyze");
    case 3:
      if ((a === "_search" && b === "scroll") || (a === "_render" && b === "template"))
        return nameOk(c!);
      if ((b === "_validate" && c === "query") || (b === "_search" && c === "point_in_time"))
        return indexOk(a!);
      if (b === "_explain" || b === "_termvectors") return indexOk(a!) && nameOk(c!);
      return indexOk(a!) && nameOk(b!) && POST_PER_TYPE.includes(c!);
    case 4:
      if (c === "_validate" && d === "query") return indexOk(a!) && nameOk(b!);
      if (d === "_explain" || d === "_termvectors") return indexOk(a!) && nameOk(b!) && nameOk(c!);
      return false;
    default:
      return false;
  }
}

/** The SQL endpoints of both engines, with `translate` / `_explain` (`is_sql`). */
export function isSql(segs: readonly string[]): boolean {
  return SQL_SHAPES.some((s) => s.length === segs.length && s.every((w, i) => w === segs[i]));
}

const SQL_SHAPES: readonly (readonly string[])[] = [
  ["_sql"],
  ["_sql", "translate"],
  ["_xpack", "sql"],
  ["_xpack", "sql", "translate"],
  ["_plugins", "_sql"],
  ["_opendistro", "_sql"],
  ["_plugins", "_sql", "_explain"],
  ["_opendistro", "_sql", "_explain"],
];

function sqlBodyReads(body: Body | null, query: QueryPairs): boolean {
  const queries = query.filter(([k]) => k === "sql").map(([, v]) => v ?? "");
  let cursor = false;
  if (body !== null) {
    if (body.kind !== "json" || body.value.k !== "obj") return false;
    for (const [k, v] of body.value.v) {
      if (k === "query" || k === "sql") {
        if (v.k !== "str") return false;
        queries.push(v.v);
      } else if (k === "cursor") cursor = true;
    }
  }
  if (queries.length === 0) return cursor;
  return queries.every(sqlReads);
}

const isWordChar = (c: string) => isAsciiAlphanumeric(c) || c === "_" || !isAscii(c);

/**
 * Whether an Elasticsearch / OpenSearch SQL query only reads (`sql_reads`): one statement that
 * starts with `SELECT`, `SHOW`, `DESCRIBE` or `DESC`, with no write word as code anywhere.
 */
export function sqlReads(query: string): boolean {
  const c = [...query];
  const words: string[] = [];
  let i = 0;
  let afterSemicolon = false;
  while (i < c.length) {
    const ch = c[i]!;
    if (isWhitespace(ch)) i++;
    else if (ch === "-" && c[i + 1] === "-") {
      while (i < c.length && c[i] !== "\n") i++;
    } else if (ch === "/" && c[i + 1] === "*") {
      let end = -1;
      for (let j = i + 2; j + 1 < c.length; j++)
        if (c[j] === "*" && c[j + 1] === "/") {
          end = j;
          break;
        }
      if (end < 0) return false;
      i = end + 2;
    } else if (ch === "'" || ch === '"' || ch === "`") {
      // Doubled quotes escape; Elasticsearch SQL has no backslash escapes.
      let j = i + 1;
      for (;;) {
        const q = c[j];
        if (q === undefined) return false;
        if (q === ch && c[j + 1] === ch) j += 2;
        else if (q === ch) break;
        else j++;
      }
      i = j + 1;
      if (afterSemicolon) return false;
    } else if (ch === ";") {
      afterSemicolon = true;
      i++;
    } else if (isWordChar(ch)) {
      const start = i;
      while (i < c.length && isWordChar(c[i]!)) i++;
      if (afterSemicolon) return false;
      words.push(asciiUpper(c.slice(start, i).join("")));
    } else {
      if (afterSemicolon) return false;
      i++;
    }
  }
  const first = words[0];
  return (
    first !== undefined && SQL_READ_STARTS.has(first) && !words.some((w) => SQL_WRITE_WORDS.has(w))
  );
}

const analyzeShape = (segs: readonly string[]) =>
  (segs.length === 1 && segs[0] === "_analyze") ||
  (segs.length === 2 && segs[1] === "_analyze" && indexOk(segs[0]!));

const COMPONENTS = new Set([
  "tokenizer",
  "filter",
  "char_filter",
  "token_filter",
  "token_filters",
  "char_filters",
  "filters",
  "normalizer",
  "analyzer",
]);

function analyzeBodyReads(req: EsRequest): boolean {
  if (req.body === null) return true;
  if (req.body.kind !== "json" || req.body.value.k !== "obj") return false;
  return req.body.value.v.every(
    ([k, v]) =>
      !COMPONENTS.has(k) || v.k === "str" || (v.k === "arr" && v.v.every((x) => x.k === "str")),
  );
}
