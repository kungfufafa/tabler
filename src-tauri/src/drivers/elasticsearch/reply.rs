//! Console replies as result grids: search hits (`_index`, `_id`, `_score`, then the `_source`
//! fields found, flattened), `_msearch` as one grid per search, `_mget` documents, SQL results
//! of both engines with their column types, `_cat` tables, `_bulk` items with their errors, and
//! any other reply as one JSON cell. Row caps apply; what was cut is said in a notice.

use crate::types::query::ResultColumn;
use crate::types::schema::ValueKind;
use crate::types::value::Value;

use super::catalog;
use super::console::EsRequest;
use super::json::Json;
use super::values::{field_value, number_value, to_value};

/// More `_source` columns than this in one console grid are left in the JSON reply.
pub const MAX_COLUMNS: usize = 512;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Grid {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Value>>,
    /// Rows were cut at the cap.
    pub truncated: bool,
    pub notices: Vec<String>,
    /// A cursor to read more (SQL `cursor`, a scroll id).
    pub cursor: Option<String>,
}

pub fn result_column(name: &str, data_type: &str, kind: ValueKind) -> ResultColumn {
    ResultColumn {
        name: name.to_string(),
        data_type: data_type.to_string(),
        kind,
        nullable: None,
        table: None,
        origin: None,
    }
}

/// The grids of a reply to `req`.
pub fn grids(req: &EsRequest, reply: &Json, max_rows: usize) -> Vec<Grid> {
    if let Some(responses) = reply.get("responses").and_then(Json::as_array) {
        return responses
            .iter()
            .map(|r| match r.get("error") {
                Some(e) => json_grid(&Json::obj([("error", e.clone())])),
                None => hits_or_json(r, max_rows),
            })
            .collect();
    }
    if super::classify::is_sql(&req.segments.iter().map(String::as_str).collect::<Vec<_>>())
        && req.last() != Some("translate")
        && req.last() != Some("_explain")
        && let Some(g) = sql_grid(reply, max_rows)
    {
        return vec![g];
    }
    if req.segments.first().is_some_and(|s| s == "_cat")
        && let Json::Arr(rows) = reply
    {
        return vec![table_grid(rows, max_rows)];
    }
    if let (Some(items), Some(_)) = (
        reply.get("items").and_then(Json::as_array),
        reply.get("errors"),
    ) {
        return vec![bulk_grid(items, max_rows)];
    }
    if let Some(docs) = reply.get("docs").and_then(Json::as_array) {
        return vec![docs_grid(docs, max_rows)];
    }
    vec![hits_or_json(reply, max_rows)]
}

fn hits_or_json(reply: &Json, max_rows: usize) -> Grid {
    let hits = reply.at(&["hits", "hits"]).and_then(Json::as_array);
    let has_aggs = reply.get("aggregations").is_some();
    match hits {
        Some(hits) if !hits.is_empty() || !has_aggs => {
            let mut g = docs_grid(hits, max_rows);
            g.columns
                .insert(2, result_column(catalog::SCORE, "float", ValueKind::Float));
            for (row, hit) in g.rows.iter_mut().zip(hits) {
                row.insert(
                    2,
                    hit.get("_score").map_or(Value::Null, |v| to_value(v, None)),
                );
            }
            if let Some(total) = reply.at(&["hits", "total"]) {
                let (n, rel) = match total {
                    Json::Obj(_) => (
                        total.get("value").and_then(Json::as_u64),
                        total.get("relation").and_then(Json::as_str).unwrap_or("eq"),
                    ),
                    other => (other.as_u64(), "eq"),
                };
                if let Some(n) = n {
                    let more = if rel == "gte" { " or more" } else { "" };
                    g.notices.push(format!("{n}{more} hits"));
                }
            }
            if has_aggs {
                g.notices
                    .push("the reply has aggregations: see the JSON view".to_string());
            }
            if reply.get("timed_out").and_then(Json::as_bool) == Some(true) {
                g.notices
                    .push("the search timed out: the hits are partial".to_string());
            }
            if let Some(failed) = reply.at(&["_shards", "failed"]).and_then(Json::as_u64)
                && failed > 0
            {
                g.notices
                    .push(format!("{failed} shard(s) failed: the hits are partial"));
            }
            if let Some(id) = reply.get("_scroll_id").and_then(Json::as_str) {
                g.cursor = Some(id.to_string());
            }
            g
        }
        _ => json_grid(reply),
    }
}

/// Documents (hits or `_mget` docs): `_index`, `_id`, then the `_source` leaves.
fn docs_grid(docs: &[Json], max_rows: usize) -> Grid {
    let mut paths: Vec<String> = Vec::new();
    let mut overflow = false;
    for d in docs.iter().take(max_rows) {
        if let Some(src) = d.get("_source") {
            leaf_paths(src, "", &mut paths, &mut overflow, 0);
        }
    }
    let mut columns = vec![
        result_column(catalog::INDEX, "keyword", ValueKind::Text),
        result_column(catalog::ID, "keyword", ValueKind::Text),
    ];
    columns.extend(paths.iter().map(|p| result_column(p, "", ValueKind::Json)));
    let mut rows = Vec::new();
    for d in docs.iter().take(max_rows) {
        let mut row = vec![
            d.get("_index").map_or(Value::Null, |v| to_value(v, None)),
            d.get("_id").map_or(Value::Null, |v| to_value(v, None)),
        ];
        for p in &paths {
            row.push(match d.get("_source") {
                Some(src) => field_value(src, p, None),
                None => super::values::missing(),
            });
        }
        rows.push(row);
    }
    let mut notices = Vec::new();
    if overflow {
        notices.push(format!(
            "more than {MAX_COLUMNS} fields: the rest are in the JSON view"
        ));
    }
    if docs
        .iter()
        .any(|d| d.get("found").and_then(Json::as_bool) == Some(false))
    {
        notices.push("some documents were not found".to_string());
    }
    Grid {
        columns,
        rows,
        truncated: docs.len() > max_rows,
        notices,
        cursor: None,
    }
}

/// Leaf paths of an object in first-seen order; arrays and empty objects are leaves.
fn leaf_paths(v: &Json, prefix: &str, out: &mut Vec<String>, overflow: &mut bool, depth: usize) {
    let Json::Obj(pairs) = v else { return };
    for (k, val) in pairs {
        let path = format!("{prefix}{k}");
        match val {
            Json::Obj(inner) if !inner.is_empty() && depth < 32 => {
                leaf_paths(val, &format!("{path}."), out, overflow, depth + 1);
            }
            _ => {
                if out.contains(&path) {
                    continue;
                }
                if out.len() >= MAX_COLUMNS {
                    *overflow = true;
                } else {
                    out.push(path);
                }
            }
        }
    }
}

/// Any reply as one JSON cell.
pub fn json_grid(reply: &Json) -> Grid {
    Grid {
        columns: vec![result_column("response", "json", ValueKind::Json)],
        rows: vec![vec![Value::Json(reply.to_text())]],
        ..Grid::default()
    }
}

/// A reply that is not JSON (`_cat` without `format=json`, a plain-text error).
pub fn text_grid(text: &str) -> Grid {
    Grid {
        columns: vec![result_column("response", "text", ValueKind::Text)],
        rows: vec![vec![Value::Text(text.to_string())]],
        ..Grid::default()
    }
}

/// An array of flat objects (`_cat` with `format=json`).
fn table_grid(rows: &[Json], max_rows: usize) -> Grid {
    let mut keys: Vec<String> = Vec::new();
    for r in rows.iter().take(max_rows) {
        for (k, _) in r.as_object().unwrap_or_default() {
            if !keys.contains(k) && keys.len() < MAX_COLUMNS {
                keys.push(k.clone());
            }
        }
    }
    Grid {
        columns: keys
            .iter()
            .map(|k| result_column(k, "keyword", ValueKind::Text))
            .collect(),
        rows: rows
            .iter()
            .take(max_rows)
            .map(|r| {
                keys.iter()
                    .map(|k| r.get(k).map_or(Value::Null, |v| to_value(v, None)))
                    .collect()
            })
            .collect(),
        truncated: rows.len() > max_rows,
        ..Grid::default()
    }
}

/// `_bulk` items: what each action did, or why it failed.
fn bulk_grid(items: &[Json], max_rows: usize) -> Grid {
    let columns = ["action", "_index", "_id", "status", "result", "error"]
        .iter()
        .map(|c| {
            let kind = if *c == "status" {
                ValueKind::Int
            } else {
                ValueKind::Text
            };
            result_column(c, "", kind)
        })
        .collect();
    let mut failed = 0;
    let rows = items
        .iter()
        .take(max_rows)
        .map(|item| {
            let (action, body) = item
                .as_object()
                .and_then(|p| p.first())
                .map_or(("?", &Json::Null), |(k, v)| (k.as_str(), v));
            let error = body.get("error").map(|e| {
                failed += 1;
                Value::Text(super::errors::reason_text(e))
            });
            vec![
                Value::Text(action.to_string()),
                body.get("_index")
                    .map_or(Value::Null, |v| to_value(v, None)),
                body.get("_id").map_or(Value::Null, |v| to_value(v, None)),
                body.get("status")
                    .map_or(Value::Null, |v| to_value(v, None)),
                body.get("result")
                    .map_or(Value::Null, |v| to_value(v, None)),
                error.unwrap_or(Value::Null),
            ]
        })
        .collect();
    let mut notices = Vec::new();
    if failed > 0 {
        notices.push(format!("{failed} of {} actions failed", items.len()));
    }
    Grid {
        columns,
        rows,
        truncated: items.len() > max_rows,
        notices,
        cursor: None,
    }
}

/// The value kind of an SQL column type (Elasticsearch SQL and OpenSearch).
pub fn sql_kind(ty: &str) -> ValueKind {
    match ty.to_ascii_lowercase().as_str() {
        "byte" | "short" | "integer" | "long" => ValueKind::Int,
        "unsigned_long" => ValueKind::BigInt,
        "double" | "float" | "half_float" | "scaled_float" => ValueKind::Float,
        "boolean" => ValueKind::Bool,
        "datetime" | "date" | "timestamp" | "date_nanos" => ValueKind::TimestampTz,
        "time" => ValueKind::Time,
        "ip" => ValueKind::Other,
        "binary" => ValueKind::Bytes,
        "object" | "nested" | "struct" | "array" | "geo_shape" | "shape" => ValueKind::Json,
        "geo_point" => ValueKind::Geometry,
        t if t.starts_with("interval") => ValueKind::Interval,
        _ => ValueKind::Text,
    }
}

/// A cell of an SQL result.
pub fn sql_value(v: &Json, kind: ValueKind) -> Value {
    match (v, kind) {
        (Json::Str(s), ValueKind::TimestampTz) => Value::TimestampTz(s.clone()),
        (Json::Str(s), ValueKind::Time) => Value::Time(s.clone()),
        (Json::Str(s), ValueKind::Interval) => Value::Interval(s.clone()),
        (Json::Str(s), ValueKind::Other) => Value::other("ip", s.clone()),
        (Json::Num(n), _) => number_value(n),
        _ => to_value(v, None),
    }
}

/// An SQL reply: Elasticsearch's `{columns, rows, cursor}` or OpenSearch's
/// `{schema, datarows, cursor}`. A cursor-only page of Elasticsearch has `rows` and no
/// `columns`; the caller keeps the first page's columns.
pub fn sql_grid(reply: &Json, max_rows: usize) -> Option<Grid> {
    let (cols, rows) = match (reply.get("columns"), reply.get("schema")) {
        (Some(c), _) => (c.as_array(), reply.get("rows").and_then(Json::as_array)),
        (None, Some(s)) => (s.as_array(), reply.get("datarows").and_then(Json::as_array)),
        (None, None) => (Some(&[][..]), reply.get("rows").and_then(Json::as_array)),
    };
    let (cols, rows) = (cols?, rows?);
    let kinds: Vec<ValueKind> = cols
        .iter()
        .map(|c| sql_kind(c.get("type").and_then(Json::as_str).unwrap_or("")))
        .collect();
    let columns = cols
        .iter()
        .zip(&kinds)
        .map(|(c, k)| {
            let name = c
                .get("alias")
                .or_else(|| c.get("name"))
                .and_then(Json::as_str)
                .unwrap_or("?");
            result_column(name, c.get("type").and_then(Json::as_str).unwrap_or(""), *k)
        })
        .collect();
    let rows = rows
        .iter()
        .take(max_rows)
        .map(|r| {
            r.as_array()
                .unwrap_or_default()
                .iter()
                .enumerate()
                .map(|(i, v)| sql_value(v, kinds.get(i).copied().unwrap_or(ValueKind::Text)))
                .collect()
        })
        .collect::<Vec<Vec<Value>>>();
    Some(Grid {
        columns,
        truncated: reply
            .get("rows")
            .or_else(|| reply.get("datarows"))
            .and_then(Json::as_array)
            .is_some_and(|r| r.len() > max_rows),
        rows,
        notices: Vec::new(),
        cursor: reply
            .get("cursor")
            .and_then(Json::as_str)
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::elasticsearch::console::parse_request;
    use crate::drivers::elasticsearch::json::parse;

    fn grid(req: &str, reply: &str, max: usize) -> Vec<Grid> {
        grids(&parse_request(req).unwrap(), &parse(reply).unwrap(), max)
    }

    fn names(g: &Grid) -> Vec<&str> {
        g.columns.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn search_hits_flatten_their_sources() {
        let g = grid(
            "GET /logs/_search",
            r#"{"took":3,"timed_out":false,"_shards":{"total":1,"successful":1,"skipped":0,"failed":0},
                "hits":{"total":{"value":10000,"relation":"gte"},"max_score":1.0,"hits":[
                {"_index":"logs","_id":"1","_score":1.0,"_source":{"a":1,"u":{"n":"x","t":[1,2]}}},
                {"_index":"logs","_id":"2","_score":0.5,"_source":{"b":"y"}}]}}"#,
            100,
        );
        let g = &g[0];
        assert_eq!(
            names(g),
            ["_index", "_id", "_score", "a", "u.n", "u.t", "b"]
        );
        assert_eq!(g.rows[0][3], Value::Int(1));
        assert_eq!(g.rows[0][5], Value::Json("[1,2]".into()));
        assert!(crate::drivers::elasticsearch::values::is_missing(
            &g.rows[1][3]
        ));
        assert_eq!(g.rows[1][2], Value::Float(0.5));
        assert_eq!(g.notices, ["10000 or more hits"]);
    }

    #[test]
    fn caps_and_partial_results() {
        let g = &grid(
            "POST /a/_search?scroll=1m",
            r#"{"_scroll_id":"abc","timed_out":true,"_shards":{"failed":2},"hits":{"total":3,"hits":[{"_id":"1","_source":{}},{"_id":"2","_source":{}},{"_id":"3","_source":{}}]}}"#,
            2,
        )[0];
        assert_eq!(g.rows.len(), 2);
        assert!(g.truncated);
        assert_eq!(g.cursor.as_deref(), Some("abc"));
        assert_eq!(g.notices.len(), 3);
    }

    #[test]
    fn aggregations_only_and_other_replies_are_json() {
        let g = &grid(
            "POST /a/_search",
            r#"{"hits":{"total":{"value":5,"relation":"eq"},"hits":[]},"aggregations":{"x":{"value":3}}}"#,
            10,
        )[0];
        assert_eq!(names(g), ["response"]);
        let g = &grid("GET /_cluster/health", r#"{"status":"green"}"#, 10)[0];
        assert_eq!(g.rows[0][0], Value::Json(r#"{"status":"green"}"#.into()));
        assert_eq!(text_grid("a b").rows[0][0], Value::Text("a b".into()));
    }

    #[test]
    fn msearch_gives_one_grid_per_search() {
        let gs = grid(
            "POST _msearch\n{}\n{}\n{}\n{}",
            r#"{"responses":[{"hits":{"hits":[{"_id":"1","_source":{"a":1}}]}},{"error":{"type":"x","reason":"bad"},"status":400}]}"#,
            10,
        );
        assert_eq!(gs.len(), 2);
        assert_eq!(names(&gs[1]), ["response"]);
    }

    #[test]
    fn sql_results_of_both_engines() {
        let es = &grid(
            "POST _sql?format=json\n{\"query\":\"SELECT 1\"}",
            r#"{"columns":[{"name":"a","type":"long"},{"name":"t","type":"datetime"},{"name":"ip","type":"ip"},{"name":"o","type":"object"}],
                "rows":[[9007199254740993,"2024-01-01T00:00:00.000Z","::1",{"k":1}]],"cursor":"c1"}"#,
            10,
        )[0];
        assert_eq!(names(es), ["a", "t", "ip", "o"]);
        assert_eq!(
            es.rows[0],
            vec![
                Value::BigInt("9007199254740993".into()),
                Value::TimestampTz("2024-01-01T00:00:00.000Z".into()),
                Value::other("ip", "::1"),
                Value::Json(r#"{"k":1}"#.into()),
            ]
        );
        assert_eq!(es.cursor.as_deref(), Some("c1"));
        let next = &grid("POST _sql\n{\"cursor\":\"c1\"}", r#"{"rows":[[1]]}"#, 10)[0];
        assert!(next.columns.is_empty());
        assert_eq!(next.rows[0], vec![Value::Int(1)]);
        let os = &grid(
            "POST _plugins/_sql\n{\"query\":\"SELECT 1\"}",
            r#"{"schema":[{"name":"n","alias":"cnt","type":"long"},{"name":"b","type":"boolean"}],"datarows":[[3,true]],"total":1,"size":1,"status":200}"#,
            10,
        )[0];
        assert_eq!(names(os), ["cnt", "b"]);
        assert_eq!(os.rows[0], vec![Value::Int(3), Value::Bool(true)]);
    }

    #[test]
    fn cat_and_bulk_and_mget() {
        let cat = &grid(
            "GET _cat/indices?format=json",
            r#"[{"index":"a","docs.count":"3"},{"index":"b","health":"green"}]"#,
            10,
        )[0];
        assert_eq!(names(cat), ["index", "docs.count", "health"]);
        assert_eq!(cat.rows[0][2], Value::Null);
        let bulk = &grid(
            "POST _bulk\n{\"delete\":{\"_index\":\"a\",\"_id\":\"1\"}}",
            r#"{"took":1,"errors":true,"items":[{"delete":{"_index":"a","_id":"1","status":404,"result":"not_found"}},
                {"update":{"_index":"a","_id":"2","status":409,"error":{"type":"version_conflict_engine_exception","reason":"[2]: version conflict"}}}]}"#,
            10,
        )[0];
        assert_eq!(
            names(bulk),
            ["action", "_index", "_id", "status", "result", "error"]
        );
        assert_eq!(bulk.rows[1][3], Value::Int(409));
        assert_eq!(bulk.notices, ["1 of 2 actions failed"]);
        let mget = &grid(
            "POST /a/_mget\n{\"ids\":[\"1\",\"2\"]}",
            r#"{"docs":[{"_index":"a","_id":"1","found":true,"_source":{"x":1}},{"_index":"a","_id":"2","found":false}]}"#,
            10,
        )[0];
        assert_eq!(names(mget), ["_index", "_id", "x"]);
        assert_eq!(mget.notices, ["some documents were not found"]);
    }
}
