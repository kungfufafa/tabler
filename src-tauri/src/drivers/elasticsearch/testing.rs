//! Fakes for the tests: [`FakeEs`], an in-memory cluster behind the `Transport` seam (indices
//! with mappings and documents, `from`/`size`, points in time with `search_after`, scrolls,
//! counts, `_mget`, `_bulk` with `if_seq_no` checks, SQL with cursors, tasks, injected
//! failures), which [`route`] also serves over a real socket through `testkit::fake_http`.
//!
//! The fake answers what the client sends, not the whole API: queries other than `match_all`,
//! `ids` and a top-level `term` match every document, and documents come back in the order
//! they were indexed (their position is their `sort` value). Its SQL answers any query with the
//! `n` field of the first index's documents, a page of `fetch_size` rows at a time.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::error::AppError;
use crate::testkit::{Reply, Request, Route};

use super::console::{Method, parse_target};
use super::json::{self, Json};
use super::transport::{HttpRequest, HttpResponse, Transport};

#[derive(Debug, Clone)]
pub struct FakeDoc {
    pub id: String,
    pub seq_no: u64,
    pub primary_term: u64,
    pub source: Json,
}

#[derive(Debug, Clone)]
pub struct FakeIndex {
    pub name: String,
    /// The `mappings` object.
    pub mappings: Json,
    pub docs: Vec<FakeDoc>,
}

#[derive(Default)]
struct State {
    indices: Vec<FakeIndex>,
    aliases: Vec<(String, String)>,
    /// Open points in time: the index and its documents when opened.
    pits: HashMap<String, (String, Vec<FakeDoc>)>,
    /// Open scrolls: documents, next position, batch size.
    scrolls: HashMap<String, (String, Vec<FakeDoc>, usize, usize)>,
    next_id: u64,
    seq: u64,
    requests: Vec<HttpRequest>,
    /// Canned replies by `METHOD /path` prefix, used once each.
    failures: Vec<(String, HttpResponse)>,
    /// Running tasks (`_tasks` objects) and the ids cancelled.
    tasks: Vec<Json>,
    cancelled: Vec<String>,
    /// Open SQL cursors (the next row) and the ones closed.
    sql_cursors: HashMap<String, (usize, usize)>,
    sql_closed: Vec<String>,
    /// `index.max_result_window` of every index (`None`: the default).
    window: Option<u64>,
}

pub struct FakeEs {
    root: String,
    state: Mutex<State>,
}

pub const ES8_ROOT: &str = r#"{"name":"node-1","cluster_name":"fake","cluster_uuid":"u1","version":{"number":"8.15.2","build_flavor":"default","build_type":"docker","lucene_version":"9.11.1"},"tagline":"You Know, for Search"}"#;
pub const OS2_ROOT: &str = r#"{"name":"os-1","cluster_name":"fake-os","cluster_uuid":"u2","version":{"distribution":"opensearch","number":"2.17.1","build_type":"tar","lucene_version":"9.11.1"},"tagline":"The OpenSearch Project: https://opensearch.org/"}"#;

impl FakeEs {
    /// A cluster answering `GET /` with `root`.
    pub fn new(root: &str) -> FakeEs {
        FakeEs {
            root: root.to_string(),
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds an index with a mapping (`{"properties": …}`) and documents (`(id, source)`).
    pub fn index(&self, name: &str, mappings: &str, docs: &[(&str, &str)]) {
        let mut st = self.state();
        st.seq += 1;
        let docs = docs
            .iter()
            .enumerate()
            .map(|(i, (id, src))| FakeDoc {
                id: id.to_string(),
                seq_no: i as u64,
                primary_term: 1,
                source: json::parse(src).unwrap_or_else(|e| panic!("{src}: {e:?}")),
            })
            .collect();
        st.indices.push(FakeIndex {
            name: name.to_string(),
            mappings: json::parse(mappings).unwrap_or_else(|e| panic!("{mappings}: {e:?}")),
            docs,
        });
    }

    pub fn alias(&self, alias: &str, index: &str) {
        self.state()
            .aliases
            .push((alias.to_string(), index.to_string()));
    }

    /// The next request whose `METHOD /path` starts with `prefix` gets `reply`.
    pub fn fail_once(&self, prefix: &str, reply: HttpResponse) {
        self.state().failures.push((prefix.to_string(), reply));
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.state().requests.clone()
    }

    /// `METHOD /path?query` of each request so far.
    pub fn lines(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|r| {
                let q = r.query().map(|q| format!("?{q}")).unwrap_or_default();
                format!("{} {}{q}", r.method.as_str(), r.path())
            })
            .collect()
    }

    pub fn doc(&self, index: &str, id: &str) -> Option<FakeDoc> {
        self.state()
            .indices
            .iter()
            .find(|i| i.name == index)?
            .docs
            .iter()
            .find(|d| d.id == id)
            .cloned()
    }

    pub fn open_pits(&self) -> usize {
        self.state().pits.len()
    }

    pub fn open_scrolls(&self) -> usize {
        self.state().scrolls.len()
    }

    /// Sets `index.max_result_window` of every index.
    pub fn window(&self, n: u64) {
        self.state().window = Some(n);
    }

    /// Adds a running task (an object of `_tasks?group_by=none`).
    pub fn task(&self, task: &str) {
        let t = json::parse(task).unwrap_or_else(|e| panic!("{task}: {e:?}"));
        self.state().tasks.push(t);
    }

    /// The task ids cancelled so far.
    pub fn cancelled(&self) -> Vec<String> {
        self.state().cancelled.clone()
    }

    pub fn open_sql_cursors(&self) -> usize {
        self.state().sql_cursors.len()
    }

    pub fn closed_sql_cursors(&self) -> Vec<String> {
        self.state().sql_closed.clone()
    }

    /// One SQL page from row `from`, `size` rows: the `n` of the first index's documents.
    fn sql_page(st: &mut State, from: usize, size: usize, first: bool) -> HttpResponse {
        let docs: Vec<Json> = st
            .indices
            .first()
            .map(|i| {
                i.docs
                    .iter()
                    .map(|d| d.source.get("n").cloned().unwrap_or(Json::Null))
                    .collect()
            })
            .unwrap_or_default();
        let rows: Vec<Json> = docs
            .iter()
            .skip(from)
            .take(size)
            .map(|n| Json::Arr(vec![n.clone()]))
            .collect();
        let next = from + rows.len();
        let mut out = Vec::new();
        if first {
            out.push((
                "columns".to_string(),
                Json::Arr(vec![Json::obj([
                    ("name", Json::str("n")),
                    ("type", Json::str("long")),
                ])]),
            ));
        }
        out.push(("rows".to_string(), Json::Arr(rows)));
        if next < docs.len() {
            st.next_id += 1;
            let id = format!("sql-{}", st.next_id);
            st.sql_cursors.insert(id.clone(), (next, size));
            out.push(("cursor".to_string(), Json::str(id)));
        }
        HttpResponse::json(200, &Json::Obj(out).to_text())
    }

    pub fn handle(&self, req: &HttpRequest) -> HttpResponse {
        let line = format!("{} {}", req.method.as_str(), req.path());
        let mut st = self.state();
        st.requests.push(req.clone());
        if let Some(i) = st
            .failures
            .iter()
            .position(|(p, _)| line.starts_with(p.as_str()))
        {
            return st.failures.remove(i).1;
        }
        let Ok((segs, query)) = parse_target(&format!(
            "{}{}",
            req.path(),
            req.query().map(|q| format!("?{q}")).unwrap_or_default()
        )) else {
            return error(400, "bad_request", "bad path");
        };
        let param = |k: &str| {
            query
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.clone().unwrap_or_default())
        };
        let body = req
            .body
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        let body_json = json::parse(&body).unwrap_or(Json::Null);
        let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
        match (req.method, segs.as_slice()) {
            (Method::Get, []) => HttpResponse::json(200, &self.root),
            (Method::Get, ["_cat", "indices"]) => {
                let rows: Vec<Json> = st
                    .indices
                    .iter()
                    .map(|i| {
                        Json::obj([
                            ("index", Json::str(i.name.clone())),
                            ("health", Json::str("green")),
                            ("status", Json::str("open")),
                            ("pri", Json::str("1")),
                            ("rep", Json::str("0")),
                            ("docs.count", Json::str(i.docs.len().to_string())),
                            ("store.size", Json::str((i.docs.len() * 100).to_string())),
                        ])
                    })
                    .collect();
                HttpResponse::json(200, &Json::Arr(rows).to_text())
            }
            (Method::Get, ["_cat", "aliases"]) => {
                let rows: Vec<Json> = st
                    .aliases
                    .iter()
                    .map(|(a, i)| {
                        Json::obj([
                            ("alias", Json::str(a.clone())),
                            ("index", Json::str(i.clone())),
                            ("filter", Json::str("-")),
                            ("is_write_index", Json::str("-")),
                        ])
                    })
                    .collect();
                HttpResponse::json(200, &Json::Arr(rows).to_text())
            }
            (Method::Get, ["_data_stream"]) => HttpResponse::json(200, r#"{"data_streams":[]}"#),
            (Method::Get, [target, "_mapping"]) => {
                let found = resolve(&st, target);
                if found.is_empty() {
                    return not_found(target);
                }
                let out: Vec<(String, Json)> = found
                    .iter()
                    .map(|i| {
                        (
                            i.name.clone(),
                            Json::obj([("mappings", i.mappings.clone())]),
                        )
                    })
                    .collect();
                HttpResponse::json(200, &Json::Obj(out).to_text())
            }
            (Method::Get, [target, "_settings"]) => {
                let found = resolve(&st, target);
                let window = st.window;
                let out: Vec<(String, Json)> = found
                    .iter()
                    .map(|i| {
                        let mut settings = vec![
                            ("index.number_of_shards".to_string(), Json::str("1")),
                            ("index.uuid".to_string(), Json::str("x")),
                        ];
                        if let Some(w) = window {
                            settings.push((
                                "index.max_result_window".to_string(),
                                Json::str(w.to_string()),
                            ));
                        }
                        (
                            i.name.clone(),
                            Json::obj([("settings", Json::Obj(settings))]),
                        )
                    })
                    .collect();
                HttpResponse::json(200, &Json::Obj(out).to_text())
            }
            (Method::Post, [target, "_count"]) => {
                let n = resolve(&st, target)
                    .iter()
                    .flat_map(|i| &i.docs)
                    .filter(|d| matches(&body_json.get("query").cloned().unwrap_or(Json::Null), d))
                    .count();
                HttpResponse::json(200, &format!(r#"{{"count":{n}}}"#))
            }
            (Method::Post, [target, "_pit"] | [target, "_search", "point_in_time"]) => {
                let docs: Vec<FakeDoc> = resolve(&st, target)
                    .iter()
                    .flat_map(|i| i.docs.clone())
                    .collect();
                st.next_id += 1;
                let id = format!("pit-{}", st.next_id);
                st.pits.insert(id.clone(), (target.to_string(), docs));
                let key = if segs.len() == 2 { "id" } else { "pit_id" };
                HttpResponse::json(200, &Json::obj([(key, Json::str(id))]).to_text())
            }
            (Method::Delete, ["_pit"]) => {
                let id = body_json
                    .get("id")
                    .and_then(Json::as_str)
                    .unwrap_or_default();
                let found = st.pits.remove(id).is_some();
                HttpResponse::json(200, &format!(r#"{{"succeeded":{found},"num_freed":1}}"#))
            }
            (Method::Delete, ["_search", "scroll"]) => {
                for id in body_json
                    .get("scroll_id")
                    .and_then(Json::as_array)
                    .unwrap_or_default()
                {
                    if let Some(id) = id.as_str() {
                        st.scrolls.remove(id);
                    }
                }
                HttpResponse::json(200, r#"{"succeeded":true,"num_freed":1}"#)
            }
            (Method::Post, ["_search"]) => {
                let Some(pit) = body_json.at(&["pit", "id"]).and_then(Json::as_str) else {
                    return error(400, "action_request_validation_exception", "no index");
                };
                let Some((index, docs)) = st.pits.get(pit).cloned() else {
                    return error(
                        404,
                        "search_context_missing_exception",
                        "No search context found",
                    );
                };
                let start = body_json
                    .get("search_after")
                    .and_then(Json::as_array)
                    .and_then(|a| a.last())
                    .and_then(Json::as_u64)
                    .map_or(0, |p| p as usize + 1);
                let size = body_json.get("size").and_then(Json::as_u64).unwrap_or(10) as usize;
                let with_source = body_json.get("_source") != Some(&Json::Bool(false));
                let query = body_json.get("query").cloned().unwrap_or(Json::Null);
                let picked: Vec<(usize, &FakeDoc)> = docs
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| matches(&query, d))
                    .filter(|(i, _)| *i >= start)
                    .take(size)
                    .collect();
                let hits = picked
                    .iter()
                    .map(|(i, d)| hit(&index, d, Some(*i), with_source))
                    .collect();
                search_reply(hits, Some(("pit_id", pit)))
            }
            (Method::Post, ["_search", "scroll"]) => {
                let id = body_json
                    .get("scroll_id")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string();
                let Some((index, docs, pos, size)) = st.scrolls.get(&id).cloned() else {
                    return error(
                        404,
                        "search_context_missing_exception",
                        "No search context found",
                    );
                };
                let hits: Vec<Json> = docs
                    .iter()
                    .skip(pos)
                    .take(size)
                    .map(|d| hit(&index, d, None, true))
                    .collect();
                st.scrolls
                    .insert(id.clone(), (index, docs, pos + hits.len(), size));
                search_reply(hits, Some(("_scroll_id", &id)))
            }
            (Method::Get | Method::Post, [target, "_search"]) => {
                let query = body_json.get("query").cloned().unwrap_or(Json::Null);
                let docs: Vec<(String, FakeDoc)> = resolve(&st, target)
                    .iter()
                    .flat_map(|i| i.docs.iter().map(|d| (i.name.clone(), d.clone())))
                    .filter(|(_, d)| matches(&query, d))
                    .collect();
                let size = body_json.get("size").and_then(Json::as_u64).unwrap_or(10) as usize;
                if param("scroll").is_some() {
                    st.next_id += 1;
                    let id = format!("scroll-{}", st.next_id);
                    let index = docs.first().map(|(i, _)| i.clone()).unwrap_or_default();
                    let all: Vec<FakeDoc> = docs.into_iter().map(|(_, d)| d).collect();
                    let hits = all
                        .iter()
                        .take(size)
                        .map(|d| hit(&index, d, None, true))
                        .collect();
                    let first = all.len().min(size);
                    st.scrolls.insert(id.clone(), (index, all, first, size));
                    return search_reply(hits, Some(("_scroll_id", &id)));
                }
                let from = body_json.get("from").and_then(Json::as_u64).unwrap_or(0) as usize;
                if (from + size) as u64 > st.window.unwrap_or(10_000) {
                    return error(
                        400,
                        "illegal_argument_exception",
                        "Result window is too large",
                    );
                }
                let hits = docs
                    .iter()
                    .skip(from)
                    .take(size)
                    .map(|(i, d)| hit(i, d, None, true))
                    .collect();
                search_reply(hits, None)
            }
            (Method::Post, [target, "_mget"]) => {
                let found = resolve(&st, target);
                let docs: Vec<Json> = body_json
                    .get("docs")
                    .and_then(Json::as_array)
                    .unwrap_or_default()
                    .iter()
                    .map(|want| {
                        let id = want.get("_id").and_then(Json::as_str).unwrap_or_default();
                        match found.iter().find_map(|i| {
                            i.docs
                                .iter()
                                .find(|d| d.id == id)
                                .map(|d| (i.name.clone(), d))
                        }) {
                            Some((index, d)) => {
                                let mut h = hit(&index, d, None, true);
                                if let Json::Obj(p) = &mut h {
                                    p.push(("found".into(), Json::Bool(true)));
                                }
                                h
                            }
                            None => Json::obj([
                                ("_index", Json::str(target.to_string())),
                                ("_id", Json::str(id)),
                                ("found", Json::Bool(false)),
                            ]),
                        }
                    })
                    .collect();
                HttpResponse::json(200, &Json::obj([("docs", Json::Arr(docs))]).to_text())
            }
            (Method::Post, ["_bulk"]) => {
                let lines = json::parse_ndjson(&body).unwrap_or_default();
                let items = bulk(&mut st, &lines);
                let errors = items.iter().any(|i| {
                    i.as_object()
                        .and_then(|p| p.first())
                        .is_some_and(|(_, b)| b.get("error").is_some())
                });
                HttpResponse::json(
                    200,
                    &Json::obj([
                        ("took", Json::int(1)),
                        ("errors", Json::Bool(errors)),
                        ("items", Json::Arr(items)),
                    ])
                    .to_text(),
                )
            }
            (Method::Get, ["_cat", "indices", target]) => {
                let rows: Vec<Json> = resolve(&st, target)
                    .iter()
                    .map(|i| {
                        Json::obj([
                            ("index", Json::str(i.name.clone())),
                            ("docs.count", Json::str(i.docs.len().to_string())),
                        ])
                    })
                    .collect();
                if rows.is_empty() {
                    return not_found(target);
                }
                HttpResponse::json(200, &Json::Arr(rows).to_text())
            }
            (Method::Get, ["_alias", name]) => {
                let out: Vec<(String, Json)> = st
                    .aliases
                    .iter()
                    .filter(|(a, _)| a == name)
                    .map(|(a, i)| {
                        (
                            i.clone(),
                            Json::obj([("aliases", Json::obj([(a.clone(), Json::Obj(vec![]))]))]),
                        )
                    })
                    .collect();
                if out.is_empty() {
                    return error(
                        404,
                        "aliases_not_found_exception",
                        &format!("alias [{name}] missing"),
                    );
                }
                HttpResponse::json(200, &Json::Obj(out).to_text())
            }
            (Method::Get, ["_tasks"]) => HttpResponse::json(
                200,
                &Json::obj([("tasks", Json::Arr(st.tasks.clone()))]).to_text(),
            ),
            (Method::Post, ["_tasks", id, "_cancel"]) => {
                let found = st.tasks.iter().position(|t| {
                    format!(
                        "{}:{}",
                        t.get("node").and_then(Json::as_str).unwrap_or_default(),
                        t.get("id").and_then(Json::scalar_text).unwrap_or_default()
                    ) == *id
                });
                match found {
                    Some(i) => {
                        st.tasks.remove(i);
                        st.cancelled.push(id.to_string());
                        HttpResponse::json(200, r#"{"nodes":{}}"#)
                    }
                    None => error(
                        404,
                        "resource_not_found_exception",
                        &format!("task [{id}] is not found"),
                    ),
                }
            }
            (Method::Post, ["_sql"]) => {
                if let Some(c) = body_json.get("cursor").and_then(Json::as_str) {
                    let Some((from, size)) = st.sql_cursors.remove(c) else {
                        return error(404, "search_context_missing_exception", "no cursor");
                    };
                    return Self::sql_page(&mut st, from, size, false);
                }
                let size = body_json
                    .get("fetch_size")
                    .and_then(Json::as_u64)
                    .unwrap_or(1000) as usize;
                Self::sql_page(&mut st, 0, size.max(1), true)
            }
            (Method::Post, ["_sql", "close"]) => {
                let c = body_json
                    .get("cursor")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string();
                let found = st.sql_cursors.remove(&c).is_some();
                st.sql_closed.push(c);
                HttpResponse::json(200, &format!(r#"{{"succeeded":{found}}}"#))
            }
            _ => error(
                400,
                "illegal_argument_exception",
                &format!("no handler for {line}"),
            ),
        }
    }
}

fn resolve<'a>(st: &'a State, target: &str) -> Vec<&'a FakeIndex> {
    let names: Vec<String> = target
        .split(',')
        .flat_map(|t| {
            let via_alias: Vec<String> = st
                .aliases
                .iter()
                .filter(|(a, _)| a == t)
                .map(|(_, i)| i.clone())
                .collect();
            if via_alias.is_empty() {
                vec![t.to_string()]
            } else {
                via_alias
            }
        })
        .collect();
    st.indices
        .iter()
        .filter(|i| {
            names.iter().any(|n| {
                n == "_all"
                    || n == "*"
                    || *n == i.name
                    || n.strip_suffix('*').is_some_and(|p| i.name.starts_with(p))
            })
        })
        .collect()
}

/// `match_all`, `ids`, a top-level `term`, and `bool.filter` of those; anything else matches.
fn matches(query: &Json, d: &FakeDoc) -> bool {
    if let Some(ids) = query.at(&["ids", "values"]).and_then(Json::as_array) {
        return ids.iter().any(|i| i.as_str() == Some(d.id.as_str()));
    }
    if let Some(Json::Obj(t)) = query.get("term")
        && let Some((field, v)) = t.first()
    {
        return d.source.get(field) == Some(v);
    }
    if let Some(filters) = query.at(&["bool", "filter"]).and_then(Json::as_array) {
        return filters.iter().all(|f| matches(f, d));
    }
    true
}

fn hit(index: &str, d: &FakeDoc, sort: Option<usize>, with_source: bool) -> Json {
    let mut h = vec![
        ("_index".to_string(), Json::str(index)),
        ("_id".to_string(), Json::str(d.id.clone())),
        ("_seq_no".to_string(), Json::uint(d.seq_no)),
        ("_primary_term".to_string(), Json::uint(d.primary_term)),
        ("_score".to_string(), Json::Null),
    ];
    if with_source {
        h.push(("_source".to_string(), d.source.clone()));
    }
    if let Some(pos) = sort {
        h.push(("sort".to_string(), Json::Arr(vec![Json::uint(pos as u64)])));
    }
    Json::Obj(h)
}

fn search_reply(hits: Vec<Json>, id: Option<(&str, &str)>) -> HttpResponse {
    let mut out = Vec::new();
    if let Some((k, v)) = id {
        out.push((k.to_string(), Json::str(v)));
    }
    out.push(("took".to_string(), Json::int(1)));
    out.push(("timed_out".to_string(), Json::Bool(false)));
    out.push((
        "hits".to_string(),
        Json::obj([("max_score", Json::Null), ("hits", Json::Arr(hits))]),
    ));
    HttpResponse::json(200, &Json::Obj(out).to_text())
}

pub fn error(status: u16, ty: &str, reason: &str) -> HttpResponse {
    let e = Json::obj([("type", Json::str(ty)), ("reason", Json::str(reason))]);
    HttpResponse::json(
        status,
        &Json::obj([
            (
                "error",
                Json::obj([
                    ("root_cause", Json::Arr(vec![e.clone()])),
                    ("type", Json::str(ty)),
                    ("reason", Json::str(reason)),
                ]),
            ),
            ("status", Json::uint(u64::from(status))),
        ])
        .to_text(),
    )
}

fn not_found(index: &str) -> HttpResponse {
    error(
        404,
        "index_not_found_exception",
        &format!("no such index [{index}]"),
    )
}

fn bulk(st: &mut State, lines: &[Json]) -> Vec<Json> {
    let mut items = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some((verb, meta)) = lines[i].as_object().and_then(|p| p.first()).cloned() else {
            break;
        };
        i += 1;
        let source = if verb == "delete" {
            None
        } else {
            i += 1;
            lines.get(i - 1).cloned()
        };
        let index = meta
            .get("_index")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        let index = st
            .aliases
            .iter()
            .find(|(a, _)| *a == index)
            .map_or(index.clone(), |(_, i)| i.clone());
        st.seq += 1;
        let seq = st.seq + 100;
        let item = |status: u64, result: Option<&str>, id: &str, err: Option<(&str, String)>| {
            let mut b = vec![
                ("_index".to_string(), Json::str(index.clone())),
                ("_id".to_string(), Json::str(id)),
                ("status".to_string(), Json::uint(status)),
            ];
            if let Some(r) = result {
                b.push(("result".to_string(), Json::str(r)));
                b.push(("_seq_no".to_string(), Json::uint(seq)));
                b.push(("_primary_term".to_string(), Json::int(1)));
            }
            if let Some((ty, reason)) = err {
                b.push((
                    "error".to_string(),
                    Json::obj([("type", Json::str(ty)), ("reason", Json::str(reason))]),
                ));
            }
            Json::obj([(verb.clone(), Json::Obj(b))])
        };
        let Some(ix) = st.indices.iter().position(|x| x.name == index) else {
            items.push(item(
                404,
                None,
                "",
                Some((
                    "index_not_found_exception",
                    format!("no such index [{index}]"),
                )),
            ));
            continue;
        };
        let id = meta.get("_id").and_then(Json::as_str).map(str::to_string);
        let guard = (
            meta.get("if_seq_no").and_then(Json::as_u64),
            meta.get("if_primary_term").and_then(Json::as_u64),
        );
        let docs = &mut st.indices[ix].docs;
        let pos = id
            .as_ref()
            .and_then(|id| docs.iter().position(|d| &d.id == id));
        let conflict = |d: &FakeDoc| match guard {
            (Some(s), Some(t)) => s != d.seq_no || t != d.primary_term,
            _ => false,
        };
        match (verb.as_str(), pos) {
            ("create" | "index", Some(_)) if verb == "create" => {
                let id = id.unwrap_or_default();
                items.push(item(
                    409,
                    None,
                    &id,
                    Some((
                        "version_conflict_engine_exception",
                        format!("[{id}]: version conflict, document already exists"),
                    )),
                ));
            }
            ("create" | "index", existing) => {
                let id = id.unwrap_or_else(|| format!("auto-{seq}"));
                let doc = FakeDoc {
                    id: id.clone(),
                    seq_no: seq,
                    primary_term: 1,
                    source: source.unwrap_or(Json::Obj(vec![])),
                };
                match existing {
                    Some(p) => docs[p] = doc,
                    None => docs.push(doc),
                }
                items.push(item(201, Some("created"), &id, None));
            }
            ("update" | "delete", None) => {
                let id = id.unwrap_or_default();
                items.push(item(
                    404,
                    None,
                    &id,
                    Some((
                        "document_missing_exception",
                        format!("[{id}]: document missing"),
                    )),
                ));
            }
            ("update" | "delete", Some(p)) if conflict(&docs[p]) => {
                let id = id.unwrap_or_default();
                items.push(item(409, None, &id, Some(("version_conflict_engine_exception", format!("[{id}]: version conflict, required seqNo [{}], current document has seqNo [{}]", guard.0.unwrap_or(0), docs[p].seq_no)))));
            }
            ("delete", Some(p)) => {
                let id = docs.remove(p).id;
                items.push(item(200, Some("deleted"), &id, None));
            }
            ("update", Some(p)) => {
                let patch = source
                    .as_ref()
                    .and_then(|s| s.get("doc"))
                    .cloned()
                    .unwrap_or(Json::Null);
                merge(&mut docs[p].source, &patch);
                docs[p].seq_no = seq;
                let id = docs[p].id.clone();
                items.push(item(200, Some("updated"), &id, None));
            }
            _ => items.push(item(
                400,
                None,
                "",
                Some(("illegal_argument_exception", "unknown action".into())),
            )),
        }
    }
    items
}

/// A partial-document update, merged recursively as Elasticsearch does.
fn merge(into: &mut Json, patch: &Json) {
    let (Json::Obj(target), Json::Obj(p)) = (into, patch) else {
        return;
    };
    for (k, v) in p {
        match target.iter_mut().find(|(n, _)| n == k) {
            Some((_, existing)) if matches!((&*existing, v), (Json::Obj(_), Json::Obj(_))) => {
                merge(existing, v)
            }
            Some((_, existing)) => *existing = v.clone(),
            None => target.push((k.clone(), v.clone())),
        }
    }
}

#[async_trait]
impl Transport for FakeEs {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, AppError> {
        Ok(self.handle(&req))
    }
}

/// A route of `testkit::fake_http` that serves `es` over a real socket (every method and path),
/// so the session runs through the shared HTTP client end to end.
pub fn route(es: Arc<FakeEs>) -> Route {
    Route::new("*", "/*").handler(move |req: &Request| {
        let query = req
            .query
            .as_ref()
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        let http = HttpRequest {
            method: Method::parse(&req.method).unwrap_or(Method::Get),
            url: format!("http://fake{}{query}", req.path),
            headers: req.headers.clone(),
            body: (!req.body.is_empty()).then(|| req.body.clone()),
        };
        let reply = es.handle(&http);
        let mut out = Reply::new(reply.status).body(reply.body);
        if let Some(ct) = reply.content_type {
            out = out.header("content-type", &ct);
        }
        out
    })
}
