//! Elasticsearch and OpenSearch requests (the elasticsearch lane): every request Tabler sends
//! and the console text Preview shows for it come from here, as `EsRequest`s
//! (`drivers::elasticsearch::console`), so the preview is what is sent.
//!
//! - Catalog: `GET /`, `_cat/indices`, `_cat/aliases`, `_data_stream`, `_mapping`, `_settings`,
//!   aliases, tasks; an index's definition as the `PUT /<index>` that recreates it.
//! - The grid: the filter bar as Query DSL, sorts (text through its keyword multi-field), the
//!   page plan (`from`/`size` within `index.max_result_window`, then a point in time with
//!   `search_after` on Elasticsearch 7.12+, otherwise a scroll), its cursor as a `String`,
//!   counts and single documents.
//! - Edits: one `_bulk` action per staged change, guarded by `if_seq_no` / `if_primary_term`
//!   (and `routing`), applied together with `refresh=wait_for`; each change's preview is the
//!   same action as a one-item `_bulk`, and the reply is read back per change.
//! - Index operations and SQL (`_sql`, `_xpack/sql`, `_plugins/_sql`) with cursors.

use std::collections::BTreeMap;

use crate::drivers::elasticsearch::catalog::{self, Field};
use crate::drivers::elasticsearch::classify;
use crate::drivers::elasticsearch::console::{EsRequest, Method};
use crate::drivers::elasticsearch::errors;
use crate::drivers::elasticsearch::info::{Features, PitApi, SqlApi};
use crate::drivers::elasticsearch::json::{self, Json};
use crate::drivers::elasticsearch::values;
use crate::error::AppError;
use crate::types::browse::{
    Conjunction, FilterGroup, FilterNode, FilterOp, NullsOrder, RowKey, SortDir, SortSpec,
};
use crate::types::mutate::StagedChange;
use crate::types::schema::ValueKind;
use crate::types::value::Value;

/// Engine name in messages.
pub const ENGINE: &str = "Elasticsearch";
/// How long a point in time or scroll stays open between two pages.
pub const KEEP_ALIVE: &str = "5m";
/// The largest page Elasticsearch serves (`index.max_result_window` bounds `from + size`; a
/// point-in-time or scroll page is bounded by the same default).
pub const MAX_PAGE: u64 = 10_000;

// ---------------------------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------------------------

pub fn info() -> EsRequest {
    EsRequest::new(Method::Get, &[])
}

pub fn cluster_health() -> EsRequest {
    EsRequest::new(Method::Get, &["_cluster", "health"])
}

fn wildcards(show_system: bool) -> &'static str {
    if show_system { "all" } else { "open" }
}

pub fn cat_indices(show_system: bool) -> EsRequest {
    EsRequest::new(Method::Get, &["_cat", "indices"])
        .param("format", "json")
        .param("bytes", "b")
        .param("h", "index,health,status,pri,rep,docs.count,store.size")
        .param("s", "index")
        .param(
            "expand_wildcards",
            if show_system { "all" } else { "open,closed" },
        )
}

/// The document counts of a target's indices (an index, alias, stream or pattern).
pub fn cat_indices_of(target: &str) -> EsRequest {
    EsRequest::new(Method::Get, &["_cat", "indices", target])
        .param("format", "json")
        .param("bytes", "b")
        .param("h", "index,health,status,pri,rep,docs.count,store.size")
}

pub fn cat_aliases() -> EsRequest {
    EsRequest::new(Method::Get, &["_cat", "aliases"])
        .param("format", "json")
        .param("h", "alias,index,filter")
        .param("s", "alias")
}

pub fn data_streams(show_system: bool) -> EsRequest {
    EsRequest::new(Method::Get, &["_data_stream"]).param("expand_wildcards", wildcards(show_system))
}

pub fn mapping(target: &str) -> EsRequest {
    EsRequest::new(Method::Get, &[target, "_mapping"])
}

pub fn settings(target: &str) -> EsRequest {
    EsRequest::new(Method::Get, &[target, "_settings"]).param("flat_settings", "true")
}

pub fn alias(name: &str) -> EsRequest {
    EsRequest::new(Method::Get, &["_alias", name])
}

/// One data stream (its backing indices, template and status).
pub fn data_stream(name: &str) -> EsRequest {
    EsRequest::new(Method::Get, &["_data_stream", name])
}

/// Running search and write tasks (the process list).
pub fn tasks() -> EsRequest {
    EsRequest::new(Method::Get, &["_tasks"])
        .param("detailed", "true")
        .param("group_by", "none")
}

/// Cancels the task `id`, whether the user picked it or the server listed it.
pub fn cancel_task(id: &str) -> Result<EsRequest, AppError> {
    Ok(EsRequest::new(
        Method::Post,
        &["_tasks", task_id(id)?, "_cancel"],
    ))
}

/// A task id as `_tasks` lists it (`<node id>:<number>`), checked before it goes into a path.
fn task_id(id: &str) -> Result<&str, AppError> {
    let ok = id.split_once(':').is_some_and(|(node, n)| {
        !node.is_empty()
            && node
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            && !node.starts_with('_')
            && !n.is_empty()
            && n.chars().all(|c| c.is_ascii_digit())
    });
    if ok {
        Ok(id)
    } else {
        Err(AppError::InvalidInput(format!(
            "\u{201c}{id}\u{201d} is not a task id (node:number)"
        )))
    }
}

/// An index as the request that creates it again: its settings (without the ones the cluster
/// sets itself) and its mappings.
pub fn index_definition(index: &str, settings: &[(String, Json)], mappings: &Json) -> String {
    let settings: Vec<(String, Json)> = settings
        .iter()
        .filter(|(k, _)| !catalog::is_internal_setting(k))
        .cloned()
        .collect();
    let mut body = Vec::new();
    if !settings.is_empty() {
        body.push(("settings".to_string(), Json::Obj(settings)));
    }
    if mappings.as_object().is_some_and(|m| !m.is_empty()) {
        body.push(("mappings".to_string(), mappings.clone()));
    }
    let req = EsRequest::new(Method::Put, &[index]);
    format!(
        "{} {}\n{}",
        req.method.as_str(),
        req.target(),
        Json::Obj(body).to_pretty()
    )
}

/// An alias (reply of `GET /_alias/<name>`) as the `_aliases` request that creates it.
pub fn alias_definition(name: &str, reply: &Json) -> String {
    let mut actions = Vec::new();
    for (index, body) in reply.as_object().unwrap_or_default() {
        let mut add = vec![
            ("index".to_string(), Json::str(index.clone())),
            ("alias".to_string(), Json::str(name)),
        ];
        if let Some(def) = body.at(&["aliases", name]).and_then(Json::as_object) {
            add.extend(def.iter().cloned());
        }
        actions.push(Json::obj([("add", Json::Obj(add))]));
    }
    let req = EsRequest::new(Method::Post, &["_aliases"]);
    format!(
        "{} {}\n{}",
        req.method.as_str(),
        req.target(),
        Json::obj([("actions", Json::Arr(actions))]).to_pretty()
    )
}

/// A data stream (reply of `GET /_data_stream/<name>`) as that request, commented, and what it
/// says: a stream is made by its index template, so there is no request that creates it alone.
pub fn data_stream_definition(name: &str, reply: &Json) -> String {
    let req = data_stream(name);
    format!(
        "# {} {}\n{}",
        req.method.as_str(),
        req.target(),
        reply.to_pretty()
    )
}

// ---------------------------------------------------------------------------------------------
// Index operations
// ---------------------------------------------------------------------------------------------

pub fn create_index(name: &str, body: Option<Json>) -> EsRequest {
    let req = EsRequest::new(Method::Put, &[name]);
    match body {
        Some(b) => req.json(b),
        None => req,
    }
}

pub fn delete_index(name: &str) -> EsRequest {
    EsRequest::new(Method::Delete, &[name])
}

/// Deletes every document and keeps the index (Elasticsearch has no truncate).
pub fn truncate(name: &str) -> EsRequest {
    EsRequest::new(Method::Post, &[name, "_delete_by_query"])
        .param("conflicts", "proceed")
        .param("refresh", "true")
        .json(Json::obj([("query", match_all())]))
}

/// `_open`, `_close`, `_refresh`, `_flush`, `_forcemerge`, `_cache/clear`.
pub fn index_action(name: &str, action: &str) -> Result<EsRequest, AppError> {
    let segs: &[&str] = match action {
        "open" => &["_open"],
        "close" => &["_close"],
        "refresh" => &["_refresh"],
        "flush" => &["_flush"],
        "forcemerge" => &["_forcemerge"],
        "clear_cache" => &["_cache", "clear"],
        other => {
            return Err(AppError::InvalidInput(format!(
                "unknown index action \u{201c}{other}\u{201d}"
            )));
        }
    };
    let mut all = vec![name];
    all.extend_from_slice(segs);
    Ok(EsRequest::new(Method::Post, &all))
}

// ---------------------------------------------------------------------------------------------
// The filter bar as Query DSL
// ---------------------------------------------------------------------------------------------

pub fn match_all() -> Json {
    Json::obj([("match_all", Json::Obj(vec![]))])
}

fn bool_query(clause: &str, items: Vec<Json>) -> Json {
    Json::obj([("bool", Json::obj([(clause, Json::Arr(items))]))])
}

fn not(q: Json) -> Json {
    bool_query("must_not", vec![q])
}

fn leaf(kind: &str, field: &str, body: Json) -> Json {
    Json::obj([(kind, Json::obj([(field, body)]))])
}

/// The query of a filter group (`match_all` without one).
pub fn query(
    fields: &[Field],
    filter: Option<&FilterGroup>,
    features: Features,
) -> Result<Json, AppError> {
    match filter {
        Some(g) if !g.items.is_empty() => group(fields, g, features, 0),
        _ => Ok(match_all()),
    }
}

fn group(fields: &[Field], g: &FilterGroup, f: Features, depth: usize) -> Result<Json, AppError> {
    if depth > 32 {
        return Err(AppError::InvalidInput("filter nested too deeply".into()));
    }
    let mut items = Vec::with_capacity(g.items.len());
    for node in &g.items {
        items.push(match node {
            FilterNode::Condition { column, op, values } => {
                condition(fields, column, *op, values, f)?
            }
            FilterNode::Group(inner) => group(fields, inner, f, depth + 1)?,
            FilterNode::Raw { sql } => raw_filter(fields, sql)?,
            FilterNode::AnyColumn { op, values } => any_column(*op, values)?,
        });
    }
    Ok(match g.conjunction {
        Conjunction::And => bool_query("filter", items),
        Conjunction::Or => Json::obj([(
            "bool",
            Json::obj([
                ("should", Json::Arr(items)),
                ("minimum_should_match", Json::int(1)),
            ]),
        )]),
    })
}

/// A raw filter: Query DSL when it starts with `{`, otherwise a Lucene `query_string`. DSL that
/// calls a model or another service is refused here (the grid only reads), and so is DSL that
/// sends text to a `semantic_text` field of `fields`; the console runs them.
pub fn raw_filter(fields: &[Field], text: &str) -> Result<Json, AppError> {
    let t = text.trim();
    if t.starts_with('{') {
        let q = json::parse(t)
            .map_err(|e| AppError::InvalidInput(format!("raw filter: {}", e.describe(t))))?;
        let probe = EsRequest::new(Method::Get, &["_search"]).json(q.clone());
        if classify::classify(&probe) != crate::sqlgen::classify::StatementClass::Read
            || infers_on_semantic_text(&q, fields, 0)
        {
            return Err(AppError::InvalidInput(
                "raw filter: this query calls a model or another service; run it in the console"
                    .into(),
            ));
        }
        Ok(q)
    } else if t.is_empty() {
        Ok(match_all())
    } else {
        Ok(leaf("query_string", "query", Json::str(t)))
    }
}

/// Whether DSL sends text to the inference endpoint of a `semantic_text` field, which the
/// request does not name: a `match` on such a field (Elasticsearch 8.18+ runs it as a semantic
/// query), or a `multi_match` whose `fields` name one (also by a `*` pattern or with a boost).
fn infers_on_semantic_text(q: &Json, fields: &[Field], depth: usize) -> bool {
    let semantic = |name: &str| {
        let name = name.split('^').next().unwrap_or(name);
        fields.iter().any(|f| {
            f.es_type.split('|').any(|t| t == "semantic_text") && glob_matches(name, &f.path)
        })
    };
    match q {
        // The parser bounds nesting; this only keeps the walk cheap.
        _ if depth > json::MAX_DEPTH => true,
        Json::Obj(pairs) => pairs.iter().any(|(k, v)| {
            let named = match k.as_str() {
                "match" => v
                    .as_object()
                    .is_some_and(|o| o.iter().any(|(f, _)| semantic(f.as_str()))),
                "multi_match" => v
                    .get("fields")
                    .and_then(Json::as_array)
                    .is_some_and(|fs| fs.iter().filter_map(Json::as_str).any(semantic)),
                _ => false,
            };
            named || infers_on_semantic_text(v, fields, depth + 1)
        }),
        Json::Arr(items) => items
            .iter()
            .any(|v| infers_on_semantic_text(v, fields, depth + 1)),
        _ => false,
    }
}

/// A field pattern with `*` wildcards against a field path.
fn glob_matches(pattern: &str, path: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = path.strip_prefix(first) else {
        return false;
    };
    let middle: Vec<&str> = parts.collect();
    let Some((last, middle)) = middle.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(i) => rest = rest.get(i + part.len()..).unwrap_or_default(),
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// Where a column's conditions go.
struct Target<'a> {
    /// The field matched exactly (a keyword multi-field of a text field).
    exact: String,
    path: &'a str,
    es_type: Option<&'a str>,
    analyzed: bool,
    textual: bool,
}

fn target<'a>(fields: &'a [Field], column: &'a str) -> Target<'a> {
    match fields.iter().find(|f| f.path == column) {
        Some(f) => {
            let analyzed = catalog::is_analyzed(&f.es_type);
            Target {
                exact: f
                    .keyword
                    .clone()
                    .filter(|_| analyzed)
                    .unwrap_or_else(|| f.path.clone()),
                path: &f.path,
                es_type: Some(f.es_type.as_str()).filter(|t| !t.contains('|')),
                analyzed: analyzed && f.keyword.is_none(),
                textual: matches!(
                    catalog::kind_of(&f.es_type),
                    ValueKind::Text | ValueKind::Other | ValueKind::Json
                ),
            }
        }
        // Metadata and unmapped fields.
        None => Target {
            exact: column.to_string(),
            path: column,
            es_type: None,
            analyzed: false,
            textual: true,
        },
    }
}

fn need(column: &str, op: FilterOp, values: &[Value], n: usize) -> Result<(), AppError> {
    if values.len() < n {
        return Err(AppError::InvalidInput(format!(
            "{column}: {op:?} needs {n} value(s)"
        )));
    }
    Ok(())
}

fn query_value(t: &Target, v: &Value) -> Result<Json, AppError> {
    values::to_json(v, t.path, t.es_type)
}

fn condition(
    fields: &[Field],
    column: &str,
    op: FilterOp,
    vals: &[Value],
    f: Features,
) -> Result<Json, AppError> {
    let t = target(fields, column);
    let negate = op.is_negated();
    let q = match op {
        FilterOp::IsNull => not(leaf("exists", "field", Json::str(t.path))),
        FilterOp::IsNotNull => leaf("exists", "field", Json::str(t.path)),
        FilterOp::Eq | FilterOp::Ne => {
            need(column, op, vals, 1)?;
            if vals[0].is_null() {
                not(leaf("exists", "field", Json::str(t.path)))
            } else {
                equals(&t, std::slice::from_ref(&vals[0]))?
            }
        }
        FilterOp::In | FilterOp::NotIn => {
            need(column, op, vals, 1)?;
            equals(&t, vals)?
        }
        FilterOp::Gt | FilterOp::Lt | FilterOp::Ge | FilterOp::Le => {
            need(column, op, vals, 1)?;
            let bound = match op {
                FilterOp::Gt => "gt",
                FilterOp::Lt => "lt",
                FilterOp::Ge => "gte",
                _ => "lte",
            };
            leaf(
                "range",
                &t.exact,
                Json::obj([(bound, query_value(&t, &vals[0])?)]),
            )
        }
        FilterOp::Between | FilterOp::NotBetween => {
            need(column, op, vals, 2)?;
            leaf(
                "range",
                &t.exact,
                Json::obj([
                    ("gte", query_value(&t, &vals[0])?),
                    ("lte", query_value(&t, &vals[1])?),
                ]),
            )
        }
        FilterOp::Like | FilterOp::NotLike | FilterOp::Ilike => {
            need(column, op, vals, 1)?;
            let pattern = like_to_wildcard(&text_of(column, &vals[0])?);
            pattern_query(&t, column, "wildcard", pattern, op == FilterOp::Ilike, f)?
        }
        FilterOp::Contains
        | FilterOp::NotContains
        | FilterOp::ContainsCase
        | FilterOp::NotContainsCase => {
            need(column, op, vals, 1)?;
            let s = escape_wildcard(&text_of(column, &vals[0])?);
            let ci = matches!(op, FilterOp::ContainsCase | FilterOp::NotContainsCase);
            pattern_query(&t, column, "wildcard", format!("*{s}*"), ci, f)?
        }
        FilterOp::HasPrefix | FilterOp::HasPrefixCase => {
            need(column, op, vals, 1)?;
            let s = text_of(column, &vals[0])?;
            pattern_query(&t, column, "prefix", s, op == FilterOp::HasPrefixCase, f)?
        }
        FilterOp::HasSuffix | FilterOp::HasSuffixCase => {
            need(column, op, vals, 1)?;
            let s = escape_wildcard(&text_of(column, &vals[0])?);
            pattern_query(
                &t,
                column,
                "wildcard",
                format!("*{s}"),
                op == FilterOp::HasSuffixCase,
                f,
            )?
        }
        FilterOp::JsonContains | FilterOp::JsonNotContains => {
            return Err(AppError::InvalidInput(format!(
                "{column}: JSON containment is not available for Elasticsearch; use a raw filter"
            )));
        }
    };
    Ok(if negate { not(q) } else { q })
}

fn equals(t: &Target, vals: &[Value]) -> Result<Json, AppError> {
    let items: Vec<Json> = vals
        .iter()
        .map(|v| query_value(t, v))
        .collect::<Result<_, _>>()?;
    if t.path == catalog::ID {
        let ids = items
            .into_iter()
            .map(|i| match i {
                Json::Num(n) => Json::Str(n),
                other => other,
            })
            .collect();
        return Ok(leaf("ids", "values", Json::Arr(ids)));
    }
    if t.analyzed {
        // A text field without a keyword multi-field: phrase matches.
        let phrases: Vec<Json> = items
            .into_iter()
            .map(|v| leaf("match_phrase", t.path, v))
            .collect();
        return Ok(match phrases.len() {
            1 => phrases.into_iter().next().unwrap_or_else(match_all),
            _ => Json::obj([(
                "bool",
                Json::obj([
                    ("should", Json::Arr(phrases)),
                    ("minimum_should_match", Json::int(1)),
                ]),
            )]),
        });
    }
    Ok(match items.len() {
        1 => leaf(
            "term",
            &t.exact,
            items.into_iter().next().unwrap_or(Json::Null),
        ),
        _ => leaf("terms", &t.exact, Json::Arr(items)),
    })
}

fn text_of(column: &str, v: &Value) -> Result<String, AppError> {
    v.as_text()
        .ok_or_else(|| AppError::InvalidInput(format!("{column}: a pattern needs a value")))
}

fn pattern_query(
    t: &Target,
    column: &str,
    kind: &str,
    value: String,
    case_insensitive: bool,
    f: Features,
) -> Result<Json, AppError> {
    if !t.textual {
        return Err(AppError::InvalidInput(format!(
            "{column}: pattern filters work on keyword and text fields only"
        )));
    }
    let mut body = vec![("value".to_string(), Json::str(value))];
    if case_insensitive {
        if !f.case_insensitive {
            return Err(AppError::Unsupported(
                "case-insensitive patterns need Elasticsearch 7.10 or later".into(),
            ));
        }
        body.push(("case_insensitive".to_string(), Json::Bool(true)));
    }
    Ok(leaf(kind, &t.exact, Json::Obj(body)))
}

/// Escapes `*`, `?` and `\` for a `wildcard` pattern.
pub fn escape_wildcard(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '*' | '?' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A SQL LIKE pattern as a `wildcard` pattern: `%` → `*`, `_` → `?`, `\` escapes the next
/// character, everything else literal.
pub fn like_to_wildcard(like: &str) -> String {
    let mut out = String::with_capacity(like.len());
    let mut chars = like.chars();
    while let Some(c) = chars.next() {
        match c {
            '%' => out.push('*'),
            '_' => out.push('?'),
            '\\' => match chars.next() {
                Some(n) => out.push_str(&escape_wildcard(&n.to_string())),
                None => out.push_str("\\\\"),
            },
            '*' | '?' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// Escapes the characters a Lucene `query_string` reserves.
pub fn escape_query_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "+-=&|><!(){}[]^\"~*?:\\/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn any_column(op: FilterOp, vals: &[Value]) -> Result<Json, AppError> {
    need("any column", op, vals, 1)?;
    let s = text_of("any column", &vals[0])?;
    let qs = |q: String| {
        Json::obj([(
            "query_string",
            Json::obj([
                ("query", Json::str(q)),
                ("lenient", Json::Bool(true)),
                ("analyze_wildcard", Json::Bool(true)),
            ]),
        )])
    };
    let q = match op {
        FilterOp::Eq | FilterOp::Ne => Json::obj([(
            "multi_match",
            Json::obj([
                ("query", Json::str(s)),
                ("type", Json::str("phrase")),
                ("lenient", Json::Bool(true)),
            ]),
        )]),
        FilterOp::Contains
        | FilterOp::NotContains
        | FilterOp::ContainsCase
        | FilterOp::NotContainsCase => qs(format!("*{}*", escape_query_string(&s))),
        FilterOp::HasPrefix | FilterOp::HasPrefixCase => {
            qs(format!("{}*", escape_query_string(&s)))
        }
        FilterOp::HasSuffix | FilterOp::HasSuffixCase => {
            qs(format!("*{}", escape_query_string(&s)))
        }
        FilterOp::Like | FilterOp::NotLike | FilterOp::Ilike => {
            // Wildcards stay, everything else is literal.
            let mut q = String::new();
            for c in s.chars() {
                match c {
                    '%' => q.push('*'),
                    '_' => q.push('?'),
                    c => q.push_str(&escape_query_string(&c.to_string())),
                }
            }
            qs(q)
        }
        other => {
            return Err(AppError::InvalidInput(format!(
                "{other:?} is not available across every column"
            )));
        }
    };
    Ok(if op.is_negated() { not(q) } else { q })
}

// ---------------------------------------------------------------------------------------------
// Sorts and the grid's searches
// ---------------------------------------------------------------------------------------------

/// The `sort` of a browse (`_doc` order without one: the cheapest, and stable on a point in
/// time or a scroll).
pub fn sort(fields: &[Field], specs: &[SortSpec], tiebreak: &str) -> Result<Vec<Json>, AppError> {
    let mut out = Vec::with_capacity(specs.len() + 1);
    for s in specs {
        let (field, unmapped) = match fields.iter().find(|f| f.path == s.column) {
            Some(f) => (
                f.sort_field().map(str::to_string).ok_or_else(|| {
                    AppError::InvalidInput(format!(
                        "{}: a {} field without a keyword multi-field cannot be sorted",
                        s.column, f.es_type
                    ))
                })?,
                Some(if f.sort_field() == Some(f.path.as_str()) {
                    f.es_type.clone()
                } else {
                    "keyword".to_string()
                }),
            ),
            None => (s.column.clone(), None),
        };
        let mut body = vec![(
            "order".to_string(),
            Json::str(match s.dir {
                SortDir::Asc => "asc",
                SortDir::Desc => "desc",
            }),
        )];
        if let Some(n) = s.nulls {
            body.push((
                "missing".to_string(),
                Json::str(match n {
                    NullsOrder::First => "_first",
                    NullsOrder::Last => "_last",
                }),
            ));
        }
        if let Some(t) = unmapped.filter(|_| !catalog::META_COLUMNS.contains(&field.as_str())) {
            body.push(("unmapped_type".to_string(), Json::str(t)));
        }
        out.push(Json::obj([(field, Json::Obj(body))]));
    }
    if out.is_empty() {
        out.push(Json::str(tiebreak));
    }
    Ok(out)
}

/// What a browse reads.
#[derive(Debug, Clone)]
pub struct BrowseSpec {
    /// Index, alias, data stream or pattern.
    pub target: String,
    pub query: Json,
    pub sort: Vec<Json>,
    /// `_source` fields to read (`None`: all).
    pub source: Option<Vec<String>>,
    pub seq_no: bool,
}

impl BrowseSpec {
    /// Identifies the query a cursor belongs to (FNV-1a over the target, query and sort).
    pub fn fingerprint(&self) -> String {
        let text = format!(
            "{}\u{0}{}\u{0}{}",
            self.target,
            self.query.to_text(),
            Json::Arr(self.sort.clone()).to_text()
        );
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in text.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{h:016x}")
    }

    fn body(&self, size: u64, with_source: bool) -> Vec<(String, Json)> {
        let mut body = vec![
            ("size".to_string(), Json::uint(size)),
            ("query".to_string(), self.query.clone()),
            ("sort".to_string(), Json::Arr(self.sort.clone())),
            ("track_total_hits".to_string(), Json::Bool(false)),
        ];
        if self.seq_no {
            body.push(("seq_no_primary_term".to_string(), Json::Bool(true)));
        }
        body.push((
            "_source".to_string(),
            match (&self.source, with_source) {
                (_, false) => Json::Bool(false),
                (None, true) => Json::Bool(true),
                (Some(fields), true) => {
                    Json::Arr(fields.iter().map(|f| Json::str(f.clone())).collect())
                }
            },
        ));
        body
    }
}

/// A shallow page: `from` + `size` within the result window.
pub fn search_page(spec: &BrowseSpec, from: u64, size: u64, preference: &str) -> EsRequest {
    let mut body = spec.body(size, true);
    body.insert(0, ("from".to_string(), Json::uint(from)));
    EsRequest::new(Method::Post, &[&spec.target, "_search"])
        .param("preference", preference)
        .json(Json::Obj(body))
}

pub fn open_pit(api: PitApi, target: &str) -> EsRequest {
    match api {
        PitApi::Elastic => EsRequest::new(Method::Post, &[target, "_pit"]),
        PitApi::OpenSearch => EsRequest::new(Method::Post, &[target, "_search", "point_in_time"]),
    }
    .param("keep_alive", KEEP_ALIVE)
}

/// The id in an open-PIT reply (`id` on Elasticsearch, `pit_id` on OpenSearch).
pub fn pit_id(reply: &Json) -> Option<String> {
    reply
        .get("id")
        .or_else(|| reply.get("pit_id"))
        .and_then(Json::as_str)
        .map(str::to_string)
}

pub fn close_pit(api: PitApi, id: &str) -> EsRequest {
    match api {
        PitApi::Elastic => {
            EsRequest::new(Method::Delete, &["_pit"]).json(Json::obj([("id", Json::str(id))]))
        }
        PitApi::OpenSearch => EsRequest::new(Method::Delete, &["_search", "point_in_time"])
            .json(Json::obj([("pit_id", Json::Arr(vec![Json::str(id)]))])),
    }
}

/// A page on a point in time, after `after` (the last hit's `sort` values). Without `_source`
/// while skipping to a far page.
pub fn pit_page(
    spec: &BrowseSpec,
    pit: &str,
    after: Option<&Json>,
    size: u64,
    with_source: bool,
) -> EsRequest {
    let mut body = spec.body(size, with_source);
    body.push((
        "pit".to_string(),
        Json::obj([
            ("id", Json::str(pit)),
            ("keep_alive", Json::str(KEEP_ALIVE)),
        ]),
    ));
    if let Some(a) = after {
        body.push(("search_after".to_string(), a.clone()));
    }
    // A point-in-time search names no index: the point in time does.
    EsRequest::new(Method::Post, &["_search"]).json(Json::Obj(body))
}

pub fn scroll_start(spec: &BrowseSpec, size: u64) -> EsRequest {
    EsRequest::new(Method::Post, &[&spec.target, "_search"])
        .param("scroll", KEEP_ALIVE)
        .json(Json::Obj(spec.body(size, true)))
}

pub fn scroll_next(id: &str) -> EsRequest {
    EsRequest::new(Method::Post, &["_search", "scroll"]).json(Json::obj([
        ("scroll", Json::str(KEEP_ALIVE)),
        ("scroll_id", Json::str(id)),
    ]))
}

pub fn clear_scroll(id: &str) -> EsRequest {
    clear_scrolls(&[id.to_string()])
}

pub fn clear_scrolls(ids: &[String]) -> EsRequest {
    EsRequest::new(Method::Delete, &["_search", "scroll"]).json(Json::obj([(
        "scroll_id",
        Json::Arr(ids.iter().map(|id| Json::str(id.clone())).collect()),
    )]))
}

pub fn count(target: &str, query: &Json) -> EsRequest {
    EsRequest::new(Method::Post, &[target, "_count"]).json(Json::obj([("query", query.clone())]))
}

/// The count of a `_count` reply.
pub fn count_of(reply: &Json) -> Option<i64> {
    reply.get("count").and_then(Json::as_i64)
}

/// One document by id, `_source` limited to `field` when given (`fetch_cell`). Read with
/// `_mget`, so the id travels in the body: any id works, where a path segment `.` or `..`
/// (also as `%2e`) would be folded into another path by URL parsers.
pub fn get_doc(
    index: &str,
    type_name: Option<&str>,
    id: &str,
    routing: Option<&str>,
    field: Option<&str>,
) -> EsRequest {
    let mut doc = vec![("_id".to_string(), Json::str(id))];
    if let Some(t) = type_name {
        doc.push(("_type".to_string(), Json::str(t)));
    }
    if let Some(r) = routing {
        doc.push(("routing".to_string(), Json::str(r)));
    }
    if let Some(f) = field {
        doc.push(("_source".to_string(), Json::Arr(vec![Json::str(f)])));
    }
    EsRequest::new(Method::Post, &[index, "_mget"])
        .json(Json::obj([("docs", Json::Arr(vec![Json::Obj(doc)]))]))
}

/// The document of a `get_doc` reply, `None` when it was not found.
pub fn doc_of(reply: &Json) -> Option<Json> {
    reply
        .get("docs")
        .and_then(Json::as_array)
        .and_then(|d| d.first())
        .filter(|d| d.get("found").and_then(Json::as_bool) == Some(true))
        .cloned()
}

/// How the next page is read.
#[derive(Debug, Clone, PartialEq)]
pub enum PagePlan {
    /// `from` + `size` (within the result window).
    FromSize { from: u64, size: u64 },
    /// Continue the walk of a cursor from the previous page.
    Continue(PageCursor),
    /// Open a point in time (or a scroll) and skip `skip` rows first.
    Walk { skip: u64 },
}

/// Where the next page starts, handed to the renderer as `BrowsePage.next_cursor`: a position
/// within the result window, or where a deep walk stands.
#[derive(Debug, Clone, PartialEq)]
pub struct PageCursor {
    pub mode: WalkMode,
    /// The point-in-time or scroll id (empty for `From`).
    pub id: String,
    /// `search_after` values (point in time).
    pub after: Option<Json>,
    /// The offset of the row the cursor reads next.
    pub position: u64,
    /// `BrowseSpec::fingerprint` of the query it belongs to.
    pub fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkMode {
    /// `from` / `size` at `position` (within the result window).
    From,
    Pit,
    Scroll,
}

impl PageCursor {
    /// The cursor of a page read with `from` / `size` that ends at `position`.
    pub fn from_position(position: u64, fingerprint: &str) -> PageCursor {
        PageCursor {
            mode: WalkMode::From,
            id: String::new(),
            after: None,
            position,
            fingerprint: fingerprint.to_string(),
        }
    }

    pub fn encode(&self) -> String {
        let mut pairs = vec![
            ("v".to_string(), Json::int(1)),
            (
                "mode".to_string(),
                Json::str(match self.mode {
                    WalkMode::From => "from",
                    WalkMode::Pit => "pit",
                    WalkMode::Scroll => "scroll",
                }),
            ),
            ("id".to_string(), Json::str(self.id.clone())),
            ("pos".to_string(), Json::uint(self.position)),
            ("q".to_string(), Json::str(self.fingerprint.clone())),
        ];
        if let Some(a) = &self.after {
            pairs.push(("after".to_string(), a.clone()));
        }
        Json::Obj(pairs).to_text()
    }

    pub fn decode(text: &str) -> Result<PageCursor, AppError> {
        let bad = || AppError::InvalidInput("malformed page cursor".into());
        let v = json::parse(text).map_err(|_| bad())?;
        if v.get("v").and_then(Json::as_i64) != Some(1) {
            return Err(bad());
        }
        Ok(PageCursor {
            mode: match v.get("mode").and_then(Json::as_str) {
                Some("from") => WalkMode::From,
                Some("pit") => WalkMode::Pit,
                Some("scroll") => WalkMode::Scroll,
                _ => return Err(bad()),
            },
            id: v
                .get("id")
                .and_then(Json::as_str)
                .ok_or_else(bad)?
                .to_string(),
            after: v.get("after").cloned(),
            position: v.get("pos").and_then(Json::as_u64).ok_or_else(bad)?,
            fingerprint: v
                .get("q")
                .and_then(Json::as_str)
                .ok_or_else(bad)?
                .to_string(),
        })
    }
}

/// Plans a page of `limit` rows: where a cursor of this query says (a walk it continues, or a
/// position), else at `offset`; `from`/`size` while the page ends within `window`
/// (`index.max_result_window`), otherwise a walk from the start. A cursor of another query (the
/// filter or sort changed) is ignored.
pub fn plan_page(
    offset: u64,
    limit: u64,
    window: u64,
    cursor: Option<&str>,
    fingerprint: &str,
) -> Result<PagePlan, AppError> {
    let mut start = offset;
    if let Some(c) = cursor {
        let c = PageCursor::decode(c)?;
        if c.fingerprint == fingerprint {
            match c.mode {
                WalkMode::From => start = c.position,
                WalkMode::Pit | WalkMode::Scroll => return Ok(PagePlan::Continue(c)),
            }
        }
    }
    if start.saturating_add(limit) <= window {
        Ok(PagePlan::FromSize {
            from: start,
            size: limit,
        })
    } else {
        Ok(PagePlan::Walk { skip: start })
    }
}

/// The hits of a search reply.
pub fn hits_of(reply: &Json) -> &[Json] {
    reply
        .at(&["hits", "hits"])
        .and_then(Json::as_array)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Update,
    Insert,
    Delete,
}

/// One staged change as a `_bulk` action (and its document line).
#[derive(Debug, Clone, PartialEq)]
pub struct BulkOp {
    /// Index of the change in the request.
    pub change: usize,
    pub kind: OpKind,
    pub action: Json,
    pub source: Option<Json>,
}

impl BulkOp {
    fn lines(&self) -> Vec<Json> {
        let mut lines = vec![self.action.clone()];
        lines.extend(self.source.clone());
        lines
    }
}

/// Staged changes planned once: the preview and the bulk request come from the same ops.
#[derive(Debug, Clone, PartialEq)]
pub struct EditPlan {
    pub ops: Vec<BulkOp>,
    pub preview: Vec<(usize, String)>,
}

impl EditPlan {
    /// The one `_bulk` request that applies every op, in order.
    pub fn request(&self) -> EsRequest {
        bulk(self.ops.iter().flat_map(BulkOp::lines).collect())
    }
}

/// A `_bulk` request that waits for the changes to be searchable.
pub fn bulk(lines: Vec<Json>) -> EsRequest {
    EsRequest::new(Method::Post, &["_bulk"])
        .param("refresh", "wait_for")
        .ndjson(lines)
}

/// What the edit planner needs to know about the target.
#[derive(Debug, Clone)]
pub struct EditTarget<'a> {
    /// The browsed index, alias or data stream (used when a row does not name its index).
    pub target: &'a str,
    /// The mapped fields (types for the written values).
    pub fields: &'a [Field],
    /// The 6.x mapping type.
    pub type_name: Option<&'a str>,
    pub features: Features,
}

fn key_text(key: &BTreeMap<String, Value>, name: &str) -> Option<String> {
    match key.get(name) {
        None | Some(Value::Null) => None,
        Some(v) => v.as_text(),
    }
}

fn key_number(key: &BTreeMap<String, Value>, name: &str) -> Option<Json> {
    match key.get(name) {
        Some(Value::Int(n)) => Some(Json::int(*n)),
        Some(Value::BigInt(s)) if json::is_integer_text(s) => Some(Json::Num(s.clone())),
        Some(Value::Text(s)) if json::is_integer_text(s.trim()) => Some(Json::Num(s.trim().into())),
        _ => None,
    }
}

/// Builds `{"user": {"name": v}}` from `user.name`, merging paths that share a prefix.
fn insert_path(doc: &mut Vec<(String, Json)>, path: &str, value: Json) -> Result<(), AppError> {
    let (head, rest) = match path.split_once('.') {
        Some((h, r)) if !h.is_empty() && !r.is_empty() => (h, Some(r)),
        _ => (path, None),
    };
    match rest {
        None => {
            if doc.iter().any(|(k, _)| k == head) {
                return Err(AppError::InvalidInput(format!(
                    "{path} is written twice (as a field and as an object)"
                )));
            }
            doc.push((head.to_string(), value));
        }
        Some(rest) => {
            let slot = match doc.iter().position(|(k, _)| k == head) {
                Some(i) => i,
                None => {
                    doc.push((head.to_string(), Json::Obj(vec![])));
                    doc.len() - 1
                }
            };
            match &mut doc[slot].1 {
                Json::Obj(inner) => insert_path(inner, rest, value)?,
                _ => {
                    return Err(AppError::InvalidInput(format!(
                        "{path}: {head} is written as a value and as an object"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn refuse(column: &str, why: &str) -> AppError {
    AppError::InvalidInput(format!("{column}: {why}"))
}

/// Plans staged changes as `_bulk` actions. Updates and deletes name the row's index, `_id`,
/// routing and its `if_seq_no` / `if_primary_term` from the row key, so a document changed
/// since it was read is refused by the server (409) instead of overwritten.
pub fn plan_edits(t: &EditTarget<'_>, changes: &[StagedChange]) -> Result<EditPlan, AppError> {
    let mut ops = Vec::with_capacity(changes.len());
    for (i, change) in changes.iter().enumerate() {
        let op = match change {
            StagedChange::Update {
                key,
                changes,
                exprs,
                ..
            } => {
                if exprs.as_ref().is_some_and(|e| !e.is_empty()) {
                    return Err(AppError::Unsupported(
                        "Elasticsearch has no server expressions".into(),
                    ));
                }
                let action = guarded_action(t, key)?;
                let mut doc = Vec::new();
                for (column, v) in changes {
                    if catalog::META_COLUMNS.contains(&column.as_str()) {
                        return Err(refuse(
                            column,
                            "document metadata cannot be edited (duplicate the row to give it another id)",
                        ));
                    }
                    if values::is_missing(v) {
                        return Err(refuse(
                            column,
                            "removing a field needs a script; use the console (`ctx._source.remove`)",
                        ));
                    }
                    let es_type = values::es_type_of(t.fields, column);
                    insert_path(&mut doc, column, values::to_json(v, column, es_type)?)?;
                }
                if doc.is_empty() {
                    return Err(AppError::InvalidInput("an update with no changes".into()));
                }
                BulkOp {
                    change: i,
                    kind: OpKind::Update,
                    action: Json::obj([("update", Json::Obj(action))]),
                    source: Some(Json::obj([("doc", Json::Obj(doc))])),
                }
            }
            StagedChange::Delete { key, .. } => {
                let action = guarded_action(t, key)?;
                BulkOp {
                    change: i,
                    kind: OpKind::Delete,
                    action: Json::obj([("delete", Json::Obj(action))]),
                    source: None,
                }
            }
            StagedChange::Insert {
                values: vals,
                exprs,
                ..
            } => {
                if exprs.as_ref().is_some_and(|e| !e.is_empty()) {
                    return Err(AppError::Unsupported(
                        "Elasticsearch has no server expressions".into(),
                    ));
                }
                let mut meta = Vec::new();
                let index = vals
                    .get(catalog::INDEX)
                    .and_then(Value::as_text)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| t.target.to_string());
                meta.push(("_index".to_string(), Json::str(index)));
                if let Some(ty) = t.type_name.filter(|_| t.features.typed_paths) {
                    meta.push(("_type".to_string(), Json::str(ty)));
                }
                let id = vals
                    .get(catalog::ID)
                    .filter(|v| !v.is_null() && !values::is_missing(v) && **v != Value::Default)
                    .and_then(Value::as_text)
                    .filter(|s| !s.is_empty());
                let has_id = id.is_some();
                if let Some(id) = id {
                    meta.push(("_id".to_string(), Json::str(id)));
                }
                if let Some(r) = vals
                    .get(catalog::ROUTING)
                    .filter(|v| !v.is_null())
                    .and_then(Value::as_text)
                    .filter(|s| !s.is_empty())
                {
                    meta.push(("routing".to_string(), Json::str(r)));
                }
                let mut doc = Vec::new();
                for (column, v) in vals {
                    if catalog::META_COLUMNS.contains(&column.as_str())
                        || values::is_missing(v)
                        || *v == Value::Default
                    {
                        continue;
                    }
                    let es_type = values::es_type_of(t.fields, column);
                    insert_path(&mut doc, column, values::to_json(v, column, es_type)?)?;
                }
                // `create` never overwrites; without an id, older servers need `index`.
                let verb = if has_id || t.features.create_without_id {
                    "create"
                } else {
                    "index"
                };
                BulkOp {
                    change: i,
                    kind: OpKind::Insert,
                    action: Json::obj([(verb, Json::Obj(meta))]),
                    source: Some(Json::Obj(doc)),
                }
            }
        };
        ops.push(op);
    }
    let preview = ops
        .iter()
        .map(|op| (op.change, bulk(op.lines()).to_console_text()))
        .collect();
    Ok(EditPlan { ops, preview })
}

/// `_index`, `_id`, `routing`, `if_seq_no`, `if_primary_term` of an update or delete.
fn guarded_action(t: &EditTarget<'_>, key: &RowKey) -> Result<Vec<(String, Json)>, AppError> {
    let RowKey::Pk(key) = key else {
        return Err(AppError::Unsupported(
            "Elasticsearch edits need the document id (_id)".into(),
        ));
    };
    if !t.features.seq_no_guards {
        return Err(AppError::Unsupported(
            "editing needs Elasticsearch 6.7 or later (if_seq_no guards)".into(),
        ));
    }
    let id = key_text(key, catalog::ID)
        .ok_or_else(|| AppError::InvalidInput("the row has no _id".into()))?;
    let seq_no = key_number(key, catalog::SEQ_NO);
    let term = key_number(key, catalog::PRIMARY_TERM);
    let (Some(seq_no), Some(term)) = (seq_no, term) else {
        return Err(AppError::InvalidInput(
            "the row was read without _seq_no / _primary_term; reload it before editing".into(),
        ));
    };
    let mut fields = vec![
        (
            "_index".to_string(),
            Json::str(key_text(key, catalog::INDEX).unwrap_or_else(|| t.target.to_string())),
        ),
        ("_id".to_string(), Json::str(id)),
    ];
    if let Some(ty) = t.type_name.filter(|_| t.features.typed_paths) {
        fields.push(("_type".to_string(), Json::str(ty)));
    }
    if let Some(r) = key_text(key, catalog::ROUTING) {
        fields.push(("routing".to_string(), Json::str(r)));
    }
    fields.push(("if_seq_no".to_string(), seq_no));
    fields.push(("if_primary_term".to_string(), term));
    Ok(fields)
}

/// What one applied change reports.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub affected: u64,
    /// `_index`, `_id`, `_seq_no`, `_primary_term` after the write (new row key values).
    pub returned: BTreeMap<String, Value>,
}

/// Reads a `_bulk` reply back per op, in order. An item that failed (a version conflict, a
/// missing document, a mapping error) is that change's error; an op the reply has no item for
/// is reported as not applied.
pub fn bulk_outcomes(reply: &Json, ops: &[BulkOp]) -> Vec<Result<Applied, AppError>> {
    let items = reply
        .get("items")
        .and_then(Json::as_array)
        .unwrap_or_default();
    ops.iter()
        .enumerate()
        .map(|(i, op)| {
            let Some((_, body)) = items
                .get(i)
                .and_then(Json::as_object)
                .and_then(|p| p.first())
            else {
                return Err(AppError::db(
                    "the server sent no result for this change; it was not applied",
                ));
            };
            let status = body.get("status").and_then(Json::as_u64).unwrap_or(0) as u16;
            if let Some(e) = body.get("error") {
                return Err(errors::item_error(ENGINE, status, e));
            }
            let result = body.get("result").and_then(Json::as_str).unwrap_or("");
            if !(200..300).contains(&status) || result == "not_found" {
                return Err(AppError::Db {
                    message: match (op.kind, result) {
                        (OpKind::Delete | OpKind::Update, "not_found") | (_, "")
                            if status == 404 =>
                        {
                            "the document no longer exists".to_string()
                        }
                        _ => format!("the server answered {status} ({result}) for this change"),
                    },
                    detail: None,
                    hint: None,
                    sqlstate: None,
                    vendor_code: Some(i64::from(status)),
                    position: None,
                    statement_index: None,
                });
            }
            let mut returned = BTreeMap::new();
            for k in [catalog::INDEX, catalog::ID] {
                if let Some(v) = body.get(k).and_then(Json::as_str) {
                    returned.insert(k.to_string(), Value::Text(v.to_string()));
                }
            }
            for k in [catalog::SEQ_NO, catalog::PRIMARY_TERM] {
                if let Some(Json::Num(n)) = body.get(k) {
                    returned.insert(k.to_string(), values::number_value(n));
                }
            }
            Ok(Applied {
                affected: 1,
                returned,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// SQL
// ---------------------------------------------------------------------------------------------

/// A SQL query with JSON results; OpenSearch takes `fetch_size` for a cursor too.
pub fn sql_query(api: SqlApi, query: &str, fetch_size: u64) -> EsRequest {
    EsRequest::new(Method::Post, api.segments())
        .param(
            "format",
            if api == SqlApi::OpenSearch {
                "jdbc"
            } else {
                "json"
            },
        )
        .json(Json::obj([
            ("query", Json::str(query)),
            ("fetch_size", Json::uint(fetch_size)),
        ]))
}

/// The next page of a SQL cursor, on the endpoint and in the `format` of the query that opened
/// it (`opened`: a console request, or [`sql_query`]).
pub fn sql_next(opened: &EsRequest, cursor: &str) -> EsRequest {
    let segs: Vec<&str> = opened.segments.iter().map(String::as_str).collect();
    let mut next = EsRequest::new(Method::Post, &segs);
    if let Some(format) = opened.query_value("format") {
        next = next.param("format", format);
    }
    next.json(Json::obj([("cursor", Json::str(cursor))]))
}

/// Frees a SQL cursor that was not read to its end: `close` on the endpoint that opened it.
pub fn sql_close(opened: &EsRequest, cursor: &str) -> EsRequest {
    let mut segs: Vec<&str> = opened.segments.iter().map(String::as_str).collect();
    segs.push("close");
    EsRequest::new(Method::Post, &segs).json(Json::obj([("cursor", Json::str(cursor))]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::elasticsearch::catalog::flatten_mapping;
    use crate::drivers::elasticsearch::info::ClusterInfo;
    use crate::drivers::elasticsearch::json::parse;

    fn features(version: &str) -> Features {
        ClusterInfo::parse(
            &parse(&format!(
                r#"{{"cluster_name":"c","version":{{"number":"{version}","build_flavor":"default"}}}}"#
            ))
            .unwrap(),
        )
        .unwrap()
        .features()
    }

    fn fields() -> Vec<Field> {
        flatten_mapping(
            &parse(
                r#"{"properties":{"title":{"type":"text","fields":{"keyword":{"type":"keyword"}}},
                "body":{"type":"text"},"n":{"type":"long"},"tag":{"type":"keyword"},
                "at":{"type":"date"},"user":{"properties":{"name":{"type":"keyword"}}}}}"#,
            )
            .unwrap(),
        )
        .fields
    }

    fn cond(column: &str, op: FilterOp, values: Vec<Value>) -> FilterGroup {
        FilterGroup {
            conjunction: Conjunction::And,
            items: vec![FilterNode::Condition {
                column: column.into(),
                op,
                values,
            }],
        }
    }

    fn q(column: &str, op: FilterOp, values: Vec<Value>) -> String {
        query(
            &fields(),
            Some(&cond(column, op, values)),
            features("8.15.0"),
        )
        .unwrap()
        .to_text()
    }

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn conditions_become_query_dsl() {
        assert_eq!(
            q("tag", FilterOp::Eq, vec![text("a")]),
            r#"{"bool":{"filter":[{"term":{"tag":"a"}}]}}"#
        );
        assert_eq!(
            q("title", FilterOp::Eq, vec![text("A b")]),
            r#"{"bool":{"filter":[{"term":{"title.keyword":"A b"}}]}}"#
        );
        assert_eq!(
            q("body", FilterOp::Eq, vec![text("x y")]),
            r#"{"bool":{"filter":[{"match_phrase":{"body":"x y"}}]}}"#
        );
        assert_eq!(
            q("n", FilterOp::Ne, vec![text("5")]),
            r#"{"bool":{"filter":[{"bool":{"must_not":[{"term":{"n":5}}]}}]}}"#
        );
        assert_eq!(
            q("n", FilterOp::Between, vec![Value::Int(1), Value::Int(9)]),
            r#"{"bool":{"filter":[{"range":{"n":{"gte":1,"lte":9}}}]}}"#
        );
        assert_eq!(
            q("at", FilterOp::Gt, vec![text("2024-01-01")]),
            r#"{"bool":{"filter":[{"range":{"at":{"gt":"2024-01-01"}}}]}}"#
        );
        assert_eq!(
            q("tag", FilterOp::In, vec![text("a"), text("b")]),
            r#"{"bool":{"filter":[{"terms":{"tag":["a","b"]}}]}}"#
        );
        assert_eq!(
            q("_id", FilterOp::In, vec![text("1"), Value::Int(2)]),
            r#"{"bool":{"filter":[{"ids":{"values":["1","2"]}}]}}"#
        );
        assert_eq!(
            q("user.name", FilterOp::IsNull, vec![]),
            r#"{"bool":{"filter":[{"bool":{"must_not":[{"exists":{"field":"user.name"}}]}}]}}"#
        );
        assert_eq!(
            q("tag", FilterOp::ContainsCase, vec![text("a*b")]),
            r#"{"bool":{"filter":[{"wildcard":{"tag":{"value":"*a\\*b*","case_insensitive":true}}}]}}"#
        );
        assert_eq!(
            q("title", FilterOp::Like, vec![text("a%b\\_c_")]),
            r#"{"bool":{"filter":[{"wildcard":{"title.keyword":{"value":"a*b_c?"}}}]}}"#
        );
        assert_eq!(
            q("tag", FilterOp::HasPrefix, vec![text("pre*")]),
            r#"{"bool":{"filter":[{"prefix":{"tag":{"value":"pre*"}}}]}}"#
        );
        // Unmapped fields are matched as they are named.
        assert_eq!(
            q("extra", FilterOp::Eq, vec![text("v")]),
            r#"{"bool":{"filter":[{"term":{"extra":"v"}}]}}"#
        );
    }

    #[test]
    fn groups_raw_and_any_column() {
        let g = FilterGroup {
            conjunction: Conjunction::Or,
            items: vec![
                FilterNode::Raw {
                    sql: "status:ok AND n:>3".into(),
                },
                FilterNode::Raw {
                    sql: r#"{"term":{"tag":"x"}}"#.into(),
                },
                FilterNode::AnyColumn {
                    op: FilterOp::Contains,
                    values: vec![text("a:b")],
                },
            ],
        };
        assert_eq!(
            query(&fields(), Some(&g), features("8.15.0"))
                .unwrap()
                .to_text(),
            r#"{"bool":{"should":[{"query_string":{"query":"status:ok AND n:>3"}},{"term":{"tag":"x"}},{"query_string":{"query":"*a\\:b*","lenient":true,"analyze_wildcard":true}}],"minimum_should_match":1}}"#
        );
        assert_eq!(
            query(&fields(), None, features("8.15.0")).unwrap(),
            match_all()
        );
    }

    #[test]
    fn filters_that_cannot_be_built_are_refused() {
        let f = features("8.15.0");
        for (c, op, v) in [
            ("n", FilterOp::Contains, vec![text("1")]),
            ("tag", FilterOp::Eq, vec![]),
            ("n", FilterOp::Between, vec![Value::Int(1)]),
            ("tag", FilterOp::JsonContains, vec![text("{}")]),
        ] {
            assert!(
                query(&fields(), Some(&cond(c, op, v)), f).is_err(),
                "{c} {op:?}"
            );
        }
        assert!(
            raw_filter(
                &fields(),
                r#"{"neural":{"v":{"query_text":"x","model_id":"m"}}}"#
            )
            .is_err()
        );
        assert!(raw_filter(&fields(), "{ bad").is_err());
        // A query in base64 is not decoded here: `{"semantic":…}`, `{"sparse_vector":…}`, `{}`.
        for inner in [
            "eyJzZW1hbnRpYyI6eyJmaWVsZCI6ImJvZHkiLCJxdWVyeSI6ImhlbGxvIn19",
            "eyJzcGFyc2VfdmVjdG9yIjp7ImZpZWxkIjoiYm9keSIsInF1ZXJ5IjoiaGVsbG8ifX0=",
            "e30=",
        ] {
            let dsl = format!(r#"{{"bool":{{"must":[{{"wrapper":{{"query":"{inner}"}}}}]}}}}"#);
            assert!(raw_filter(&fields(), &dsl).is_err(), "{dsl}");
        }
        // Text for a `semantic_text` field goes to its inference endpoint: not in the grid.
        let mut semantic = fields();
        semantic.push(Field {
            path: "body.semantic".into(),
            es_type: "semantic_text".into(),
            keyword: None,
            doc_values: false,
        });
        for dsl in [
            r#"{"match":{"body.semantic":"what is x"}}"#,
            r#"{"bool":{"must":[{"match":{"body.semantic":{"query":"x"}}}]}}"#,
            r#"{"multi_match":{"query":"x","fields":["title","body.semantic^2"]}}"#,
            r#"{"multi_match":{"query":"x","fields":["body.*"]}}"#,
        ] {
            assert!(raw_filter(&semantic, dsl).is_err(), "{dsl}");
            assert!(raw_filter(&fields(), dsl).is_ok(), "{dsl}");
        }
        for dsl in [
            r#"{"match":{"body":"x"}}"#,
            r#"{"multi_match":{"query":"x","fields":["title","b*y"]}}"#,
            r#"{"term":{"body.semantic":"x"}}"#,
        ] {
            assert!(raw_filter(&semantic, dsl).is_ok(), "{dsl}");
        }
        let old = features("7.9.0");
        assert!(matches!(
            query(
                &fields(),
                Some(&cond("tag", FilterOp::ContainsCase, vec![text("a")])),
                old
            ),
            Err(AppError::Unsupported(_))
        ));
    }

    #[test]
    fn sorts_use_keyword_fields() {
        let s = sort(
            &fields(),
            &[
                SortSpec {
                    column: "title".into(),
                    dir: SortDir::Desc,
                    nulls: Some(NullsOrder::Last),
                },
                SortSpec {
                    column: "n".into(),
                    dir: SortDir::Asc,
                    nulls: None,
                },
                SortSpec {
                    column: "_id".into(),
                    dir: SortDir::Asc,
                    nulls: None,
                },
            ],
            "_doc",
        )
        .unwrap();
        assert_eq!(
            Json::Arr(s).to_text(),
            r#"[{"title.keyword":{"order":"desc","missing":"_last","unmapped_type":"keyword"}},{"n":{"order":"asc","unmapped_type":"long"}},{"_id":{"order":"asc"}}]"#
        );
        assert_eq!(
            sort(&fields(), &[], "_shard_doc").unwrap(),
            vec![Json::str("_shard_doc")]
        );
        assert!(
            sort(
                &fields(),
                &[SortSpec {
                    column: "body".into(),
                    dir: SortDir::Asc,
                    nulls: None
                }],
                "_doc"
            )
            .is_err()
        );
    }

    fn spec() -> BrowseSpec {
        BrowseSpec {
            target: "logs".into(),
            query: match_all(),
            sort: vec![Json::str("_doc")],
            source: Some(vec!["a".into(), "b.c".into()]),
            seq_no: true,
        }
    }

    #[test]
    fn page_requests() {
        let s = spec();
        assert_eq!(
            search_page(&s, 20, 10, "tabler-1").to_console_text(),
            "POST /logs/_search?preference=tabler-1\n{\"from\":20,\"size\":10,\"query\":{\"match_all\":{}},\"sort\":[\"_doc\"],\"track_total_hits\":false,\"seq_no_primary_term\":true,\"_source\":[\"a\",\"b.c\"]}"
        );
        let after = parse(r#"[5,42]"#).unwrap();
        let p = pit_page(&s, "PITID", Some(&after), 10_000, false);
        assert_eq!(p.target(), "/_search");
        let body = p.body.as_ref().unwrap().values()[0].clone();
        assert_eq!(body.get("_source"), Some(&Json::Bool(false)));
        assert_eq!(
            body.at(&["pit", "id"]).and_then(Json::as_str),
            Some("PITID")
        );
        assert_eq!(body.get("search_after"), Some(&after));
        assert_eq!(
            open_pit(PitApi::Elastic, "logs").target(),
            "/logs/_pit?keep_alive=5m"
        );
        assert_eq!(
            open_pit(PitApi::OpenSearch, "logs").target(),
            "/logs/_search/point_in_time?keep_alive=5m"
        );
        assert_eq!(
            close_pit(PitApi::OpenSearch, "x").to_console_text(),
            "DELETE /_search/point_in_time\n{\"pit_id\":[\"x\"]}"
        );
        assert_eq!(
            pit_id(&parse(r#"{"pit_id":"o"}"#).unwrap()).as_deref(),
            Some("o")
        );
        assert_eq!(scroll_start(&s, 50).target(), "/logs/_search?scroll=5m");
        assert_eq!(
            count("logs", &match_all()).to_console_text(),
            "POST /logs/_count\n{\"query\":{\"match_all\":{}}}"
        );
        assert_eq!(
            get_doc("logs", None, "..", Some("r"), Some("x.y")).to_console_text(),
            "POST /logs/_mget\n{\"docs\":[{\"_id\":\"..\",\"routing\":\"r\",\"_source\":[\"x.y\"]}]}"
        );
        assert_eq!(
            get_doc("logs", Some("doc"), "1", None, None).to_console_text(),
            "POST /logs/_mget\n{\"docs\":[{\"_id\":\"1\",\"_type\":\"doc\"}]}"
        );
        assert_eq!(
            doc_of(&parse(r#"{"docs":[{"_id":"1","found":true,"_source":{"a":1}}]}"#).unwrap())
                .and_then(|d| d.get("_source").cloned()),
            Some(parse(r#"{"a":1}"#).unwrap())
        );
        assert_eq!(
            doc_of(&parse(r#"{"docs":[{"_id":"1","found":false}]}"#).unwrap()),
            None
        );
        // Every request the grid sends for itself classifies as a read.
        for r in [
            search_page(&s, 0, 1, "p"),
            pit_page(&s, "x", None, 1, true),
            open_pit(PitApi::Elastic, "logs"),
            open_pit(PitApi::OpenSearch, "logs"),
            scroll_start(&s, 1),
            scroll_next("x"),
            count("logs", &match_all()),
            get_doc("logs", None, "1", None, None),
            cat_indices(true),
            cat_aliases(),
            data_streams(false),
            mapping("logs"),
            settings("logs"),
            alias("a"),
            tasks(),
            info(),
            cluster_health(),
        ] {
            assert_eq!(
                classify::classify(&r),
                crate::sqlgen::classify::StatementClass::Read,
                "{}",
                r.to_console_text()
            );
        }
    }

    #[test]
    fn page_plans_and_cursors() {
        let fp = spec().fingerprint();
        assert_eq!(
            plan_page(100, 50, 10_000, None, &fp).unwrap(),
            PagePlan::FromSize {
                from: 100,
                size: 50
            }
        );
        assert_eq!(
            plan_page(9_990, 50, 10_000, None, &fp).unwrap(),
            PagePlan::Walk { skip: 9_990 }
        );
        let c = PageCursor {
            mode: WalkMode::Pit,
            id: "pit==".into(),
            after: Some(parse("[1,\"x\",7]").unwrap()),
            position: 10_050,
            fingerprint: fp.clone(),
        };
        let text = c.encode();
        assert_eq!(PageCursor::decode(&text).unwrap(), c);
        // The renderer pages by cursor (offset 0): the cursor decides.
        assert_eq!(
            plan_page(0, 50, 10_000, Some(&text), &fp).unwrap(),
            PagePlan::Continue(c.clone())
        );
        // A cursor of another query is ignored.
        assert_eq!(
            plan_page(0, 50, 10_000, Some(&text), "other").unwrap(),
            PagePlan::FromSize { from: 0, size: 50 }
        );
        // A position cursor pages with from/size inside the window and walks past it.
        let at = PageCursor::from_position(200, &fp);
        assert_eq!(PageCursor::decode(&at.encode()).unwrap(), at);
        assert_eq!(
            plan_page(0, 50, 10_000, Some(&at.encode()), &fp).unwrap(),
            PagePlan::FromSize {
                from: 200,
                size: 50
            }
        );
        let deep = PageCursor::from_position(9_980, &fp).encode();
        assert_eq!(
            plan_page(0, 50, 10_000, Some(&deep), &fp).unwrap(),
            PagePlan::Walk { skip: 9_980 }
        );
        for bad in [
            "",
            "{}",
            "[1]",
            r#"{"v":1,"mode":"x","id":"a","pos":1,"q":"f"}"#,
        ] {
            assert!(PageCursor::decode(bad).is_err(), "{bad}");
        }
        let mut other = spec();
        other.sort = vec![Json::str("_shard_doc")];
        assert_ne!(other.fingerprint(), fp);
    }

    fn pk(pairs: &[(&str, Value)]) -> RowKey {
        RowKey::Pk(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    fn edit_target<'a>(fields: &'a [Field], f: Features) -> EditTarget<'a> {
        EditTarget {
            target: "logs-alias",
            fields,
            type_name: None,
            features: f,
        }
    }

    #[test]
    fn edits_are_guarded_bulk_actions() {
        let fields = fields();
        let t = edit_target(&fields, features("8.15.0"));
        let changes = vec![
            StagedChange::Update {
                row_id: "r1".into(),
                key: pk(&[
                    ("_index", text("logs-1")),
                    ("_id", text("a")),
                    ("_seq_no", Value::Int(7)),
                    ("_primary_term", Value::Int(2)),
                ]),
                changes: [
                    ("n".to_string(), text("42")),
                    ("user.name".to_string(), text("bo")),
                    ("title".to_string(), Value::Null),
                ]
                .into_iter()
                .collect(),
                exprs: None,
            },
            StagedChange::Insert {
                row_id: "r2".into(),
                values: [
                    ("_id".to_string(), Value::Null),
                    ("n".to_string(), Value::Int(1)),
                    ("tag".to_string(), Value::Default),
                    ("body".to_string(), values::missing()),
                ]
                .into_iter()
                .collect(),
                exprs: None,
            },
            StagedChange::Delete {
                row_id: "r3".into(),
                key: pk(&[
                    ("_id", text("b")),
                    ("_routing", text("u1")),
                    ("_seq_no", Value::Int(3)),
                    ("_primary_term", Value::Int(1)),
                ]),
            },
        ];
        let plan = plan_edits(&t, &changes).unwrap();
        assert_eq!(plan.preview.len(), 3);
        assert_eq!(
            plan.preview[0].1,
            "POST /_bulk?refresh=wait_for\n{\"update\":{\"_index\":\"logs-1\",\"_id\":\"a\",\"if_seq_no\":7,\"if_primary_term\":2}}\n{\"doc\":{\"n\":42,\"title\":null,\"user\":{\"name\":\"bo\"}}}"
        );
        assert_eq!(
            plan.preview[1].1,
            "POST /_bulk?refresh=wait_for\n{\"create\":{\"_index\":\"logs-alias\"}}\n{\"n\":1}"
        );
        assert_eq!(
            plan.preview[2].1,
            "POST /_bulk?refresh=wait_for\n{\"delete\":{\"_index\":\"logs-alias\",\"_id\":\"b\",\"routing\":\"u1\",\"if_seq_no\":3,\"if_primary_term\":1}}"
        );
        let req = plan.request();
        let body = String::from_utf8(req.body.as_ref().unwrap().to_bytes()).unwrap();
        assert_eq!(body.lines().count(), 5);
        assert!(body.ends_with('\n'));
        // The preview of each change runs back as the same request.
        let again =
            crate::drivers::elasticsearch::console::parse_request(&plan.preview[2].1).unwrap();
        assert_eq!(again, bulk(plan.ops[2].lines()));
    }

    #[test]
    fn edits_that_cannot_be_guarded_are_refused() {
        let fields = fields();
        let t = edit_target(&fields, features("8.15.0"));
        let update = |key: RowKey, col: &str, v: Value| StagedChange::Update {
            row_id: "r".into(),
            key,
            changes: [(col.to_string(), v)].into_iter().collect(),
            exprs: None,
        };
        let good = pk(&[
            ("_id", text("a")),
            ("_seq_no", Value::Int(1)),
            ("_primary_term", Value::Int(1)),
        ]);
        for change in [
            update(pk(&[("_id", text("a"))]), "n", Value::Int(1)),
            update(good.clone(), "_id", text("b")),
            update(good.clone(), "n", values::missing()),
            update(good.clone(), "n", Value::Default),
            update(RowKey::Rowid(1), "n", Value::Int(1)),
            update(good.clone(), "user", Value::Json("{".into())),
        ] {
            assert!(
                plan_edits(&t, std::slice::from_ref(&change)).is_err(),
                "{change:?}"
            );
        }
        let six_five = edit_target(&fields, features("6.5.0"));
        assert!(plan_edits(&six_five, &[update(good.clone(), "n", Value::Int(1))]).is_err());
        // Conflicting paths in one document.
        let both = StagedChange::Update {
            row_id: "r".into(),
            key: good,
            changes: [
                ("user".to_string(), Value::Int(1)),
                ("user.name".to_string(), text("x")),
            ]
            .into_iter()
            .collect(),
            exprs: None,
        };
        assert!(plan_edits(&t, &[both]).is_err());
    }

    #[test]
    fn six_x_actions_name_the_type_and_old_servers_index_without_id() {
        let fields = fields();
        let mut t = edit_target(&fields, features("6.8.0"));
        t.type_name = Some("doc");
        let insert = StagedChange::Insert {
            row_id: "r".into(),
            values: [("n".to_string(), Value::Int(1))].into_iter().collect(),
            exprs: None,
        };
        let plan = plan_edits(&t, &[insert]).unwrap();
        assert_eq!(
            plan.ops[0].action.to_text(),
            r#"{"index":{"_index":"logs-alias","_type":"doc"}}"#
        );
    }

    #[test]
    fn bulk_replies_map_back_per_change() {
        let fields = fields();
        let t = edit_target(&fields, features("8.15.0"));
        let key = |id: &str| {
            pk(&[
                ("_id", text(id)),
                ("_seq_no", Value::Int(1)),
                ("_primary_term", Value::Int(1)),
            ])
        };
        let changes: Vec<StagedChange> = ["a", "b", "c", "d"]
            .iter()
            .map(|id| StagedChange::Delete {
                row_id: id.to_string(),
                key: key(id),
            })
            .collect();
        let plan = plan_edits(&t, &changes).unwrap();
        let reply = parse(
            r#"{"took":5,"errors":true,"items":[
            {"delete":{"_index":"logs-1","_id":"a","_version":2,"result":"deleted","_seq_no":9,"_primary_term":1,"status":200}},
            {"delete":{"_index":"logs-1","_id":"b","status":409,"error":{"type":"version_conflict_engine_exception","reason":"[b]: version conflict, required seqNo [1], primary term [1]. current document has seqNo [4] and primary term [1]"}}},
            {"delete":{"_index":"logs-1","_id":"c","_version":1,"result":"not_found","status":404}}]}"#,
        )
        .unwrap();
        let out = bulk_outcomes(&reply, &plan.ops);
        assert_eq!(out.len(), 4);
        let ok = out[0].as_ref().unwrap();
        assert_eq!(ok.affected, 1);
        assert_eq!(ok.returned.get("_seq_no"), Some(&Value::Int(9)));
        assert_eq!(ok.returned.get("_index"), Some(&text("logs-1")));
        match &out[1] {
            Err(AppError::Db { hint, detail, .. }) => {
                assert!(hint.as_deref().unwrap().contains("changed"));
                assert_eq!(detail.as_deref(), Some("version_conflict_engine_exception"));
            }
            other => panic!("{other:?}"),
        }
        assert!(
            matches!(&out[2], Err(AppError::Db { message, .. }) if message.contains("no longer exists"))
        );
        assert!(
            matches!(&out[3], Err(AppError::Db { message, .. }) if message.contains("not applied"))
        );
    }

    #[test]
    fn definitions_and_index_operations() {
        let text = index_definition(
            "logs",
            &[
                ("index.number_of_shards".into(), Json::str("1")),
                ("index.uuid".into(), Json::str("u")),
                ("index.creation_date".into(), Json::str("1")),
            ],
            &parse(r#"{"properties":{"a":{"type":"long"}}}"#).unwrap(),
        );
        assert_eq!(
            text,
            "PUT /logs\n{\n  \"settings\": {\n    \"index.number_of_shards\": \"1\"\n  },\n  \"mappings\": {\n    \"properties\": {\n      \"a\": {\n        \"type\": \"long\"\n      }\n    }\n  }\n}"
        );
        let a = alias_definition(
            "cur",
            &parse(r#"{"logs-1":{"aliases":{"cur":{"is_write_index":true,"filter":{"term":{"a":1}}}}}}"#)
                .unwrap(),
        );
        assert!(a.starts_with("POST /_aliases\n"));
        assert!(a.contains("\"is_write_index\": true"));
        assert_eq!(
            data_stream_definition("logs-app", &parse(r#"{"data_streams":[]}"#).unwrap()),
            "# GET /_data_stream/logs-app\n{\n  \"data_streams\": []\n}"
        );
        assert_eq!(
            truncate("logs").to_console_text(),
            "POST /logs/_delete_by_query?conflicts=proceed&refresh=true\n{\"query\":{\"match_all\":{}}}"
        );
        assert_eq!(
            index_action("logs", "clear_cache").unwrap().target(),
            "/logs/_cache/clear"
        );
        assert!(index_action("logs", "explode").is_err());
        assert_eq!(
            cancel_task("oTUltX4IQMOUUVeiohTt8A:12345")
                .unwrap()
                .target(),
            "/_tasks/oTUltX4IQMOUUVeiohTt8A:12345/_cancel"
        );
        for bad in ["../_bulk:1", "_all:1", "n:x", "n", "n:1/_cancel"] {
            assert!(cancel_task(bad).is_err(), "{bad}");
        }
        // Every one of them is a write to the classifier.
        for r in [
            truncate("logs"),
            create_index("x", None),
            delete_index("x"),
            index_action("x", "open").unwrap(),
            index_action("x", "refresh").unwrap(),
            cancel_task("n:1").unwrap(),
            bulk(vec![]),
            sql_close(&sql_query(SqlApi::Elastic, "SELECT 1", 1), "c"),
            close_pit(PitApi::Elastic, "p"),
            clear_scroll("s"),
            clear_scrolls(&["a".into(), "b".into()]),
        ] {
            assert_eq!(
                classify::classify(&r),
                crate::sqlgen::classify::StatementClass::Write,
                "{}",
                r.to_console_text()
            );
        }
    }

    #[test]
    fn sql_requests() {
        assert_eq!(
            sql_query(SqlApi::Elastic, "SELECT 1", 100).to_console_text(),
            "POST /_sql?format=json\n{\"query\":\"SELECT 1\",\"fetch_size\":100}"
        );
        let os = sql_query(SqlApi::OpenSearch, "SELECT 1", 5);
        assert_eq!(
            sql_next(&os, "c").to_console_text(),
            "POST /_plugins/_sql?format=jdbc\n{\"cursor\":\"c\"}"
        );
        assert_eq!(
            sql_close(&os, "c").to_console_text(),
            "POST /_plugins/_sql/close\n{\"cursor\":\"c\"}"
        );
        let xpack = sql_query(SqlApi::XPack, "SELECT 1", 5);
        assert_eq!(sql_close(&xpack, "c").target(), "/_xpack/sql/close");
        // A console request keeps its endpoint and format; its other parameters stay behind.
        let typed = EsRequest::new(Method::Post, &["_opendistro", "_sql"])
            .param("format", "json")
            .param("pretty", "");
        assert_eq!(
            sql_next(&typed, "c").target(),
            "/_opendistro/_sql?format=json"
        );
        for r in [
            sql_query(SqlApi::Elastic, "SELECT 1", 1),
            sql_next(&os, "c"),
        ] {
            assert_eq!(
                classify::classify(&r),
                crate::sqlgen::classify::StatementClass::Read
            );
        }
    }
}
