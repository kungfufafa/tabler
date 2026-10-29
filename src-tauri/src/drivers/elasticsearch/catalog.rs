//! Catalog replies as Tabler's types: indices, aliases and data streams as tables, an index's
//! mapping flattened into grid columns, its settings.
//!
//! Columns: `_id` (the key; generated when an insert leaves it empty), `_index` when the target
//! can span indices (an alias, a data stream, a pattern), `_routing` when the mapping requires
//! it, `_seq_no` and `_primary_term` (the concurrency guard every edit sends back, part of the
//! row key so a staged change carries the values it was read with), then the mapped fields:
//! objects flatten to dotted paths, `nested`, `flattened`, vectors, ranges and shapes stay one
//! JSON column, multi-fields (`title.keyword`) are not columns but the sort and exact-match
//! field of their parent, field aliases and runtime fields are skipped (not in `_source`).

use std::collections::BTreeMap;

use crate::error::AppError;
use crate::types::schema::{ColumnMeta, TableKind, TableSummary, ValueKind};

use super::json::Json;

pub const ID: &str = "_id";
pub const INDEX: &str = "_index";
pub const ROUTING: &str = "_routing";
pub const SEQ_NO: &str = "_seq_no";
pub const PRIMARY_TERM: &str = "_primary_term";
pub const SCORE: &str = "_score";
pub const SOURCE: &str = "_source";

/// Columns that are document metadata, not `_source` fields.
pub const META_COLUMNS: &[&str] = &[ID, INDEX, ROUTING, SEQ_NO, PRIMARY_TERM, SCORE];

/// `extra` of a column with a keyword multi-field: `sort: title.keyword`.
pub const SORT_PREFIX: &str = "sort: ";

/// Elasticsearch's default `index.max_result_window`.
pub const DEFAULT_RESULT_WINDOW: u64 = 10_000;

/// The value kind of a mapping type.
pub fn kind_of(es_type: &str) -> ValueKind {
    match es_type {
        "long" | "integer" | "short" | "byte" | "token_count" => ValueKind::Int,
        "unsigned_long" => ValueKind::BigInt,
        "double" | "float" | "half_float" | "scaled_float" | "rank_feature" => ValueKind::Float,
        "keyword" | "constant_keyword" | "wildcard" | "text" | "match_only_text"
        | "search_as_you_type" | "annotated_text" | "version" | "counted_keyword" => {
            ValueKind::Text
        }
        "boolean" => ValueKind::Bool,
        "date" | "date_nanos" => ValueKind::TimestampTz,
        "binary" => ValueKind::Bytes,
        "ip" => ValueKind::Other,
        "geo_point" => ValueKind::Geometry,
        _ => ValueKind::Json,
    }
}

/// Whether a field of this type is one JSON cell in the grid even when it holds an object.
pub fn is_json_type(es_type: &str) -> bool {
    kind_of(es_type) == ValueKind::Json
}

/// Text types: exact matches and sorts go through a keyword multi-field.
pub fn is_analyzed(es_type: &str) -> bool {
    matches!(
        es_type,
        "text" | "match_only_text" | "search_as_you_type" | "annotated_text"
    )
}

/// One mapped field that is a grid column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Dotted path.
    pub path: String,
    pub es_type: String,
    /// A keyword multi-field to sort and match exactly on (`title.keyword`).
    pub keyword: Option<String>,
    /// `index: false`: not searchable.
    pub indexed: bool,
    /// `doc_values: false`: not sortable on its own.
    pub doc_values: bool,
}

impl Field {
    pub fn kind(&self) -> ValueKind {
        kind_of(&self.es_type)
    }

    /// The field a sort uses, if the field can be sorted on.
    pub fn sort_field(&self) -> Option<&str> {
        if is_analyzed(&self.es_type) || !self.doc_values || is_json_type(&self.es_type) {
            self.keyword.as_deref()
        } else {
            Some(&self.path)
        }
    }

    /// A field rebuilt from a grid column (the session gets columns back with each browse).
    pub fn from_column(c: &ColumnMeta) -> Field {
        let keyword = c
            .extra
            .as_deref()
            .and_then(|e| e.strip_prefix(SORT_PREFIX))
            .map(str::to_string);
        let sortable_raw = !c.extra.as_deref().is_some_and(|e| e == "not sortable");
        Field {
            path: c.name.clone(),
            es_type: c.data_type.clone(),
            keyword,
            indexed: true,
            doc_values: sortable_raw,
        }
    }
}

/// An index's mapping, flattened.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IndexMapping {
    pub fields: Vec<Field>,
    /// `_routing.required`.
    pub routing_required: bool,
    /// `_source.enabled` (when false, documents cannot be shown or edited).
    pub source_enabled: bool,
    /// The mapping type of a 6.x index (`_doc`, `doc`, …).
    pub type_name: Option<String>,
    /// The `mappings` object as the server sent it (without the 6.x type level).
    pub raw: Json,
}

/// `GET /<target>/_mapping`: each index's mapping, in reply order.
pub fn parse_mappings(reply: &Json) -> Result<Vec<(String, IndexMapping)>, AppError> {
    let indices = reply
        .as_object()
        .ok_or_else(|| AppError::db("unexpected _mapping reply: not an object"))?;
    let mut out = Vec::with_capacity(indices.len());
    for (index, body) in indices {
        let mappings = body.get("mappings").cloned().unwrap_or(Json::Obj(vec![]));
        out.push((index.clone(), flatten_mapping(&mappings)));
    }
    Ok(out)
}

const MAPPING_META: &[&str] = &[
    "properties",
    "dynamic",
    "_source",
    "_routing",
    "_meta",
    "runtime",
    "dynamic_templates",
    "date_detection",
    "numeric_detection",
    "dynamic_date_formats",
    "_field_names",
    "_data_stream_timestamp",
    "enabled",
    "subobjects",
];

/// One index's `mappings` object (typed or not) flattened into fields.
pub fn flatten_mapping(mappings: &Json) -> IndexMapping {
    // 6.x: `{"_doc": {"properties": …}}`: one key that is not a mapping parameter.
    let (type_name, m) = match mappings.as_object() {
        Some([(name, inner)])
            if !MAPPING_META.contains(&name.as_str()) && inner.as_object().is_some() =>
        {
            (Some(name.clone()), inner.clone())
        }
        _ => (None, mappings.clone()),
    };
    let mut fields = Vec::new();
    if let Some(props) = m.get("properties").and_then(Json::as_object) {
        walk(props, "", &mut fields, 0);
    }
    IndexMapping {
        fields,
        routing_required: m
            .at(&["_routing", "required"])
            .and_then(Json::as_bool)
            .unwrap_or(false),
        source_enabled: m
            .at(&["_source", "enabled"])
            .and_then(Json::as_bool)
            .unwrap_or(true),
        type_name,
        raw: m,
    }
}

fn walk(props: &[(String, Json)], prefix: &str, out: &mut Vec<Field>, depth: usize) {
    for (name, def) in props {
        let path = format!("{prefix}{name}");
        let sub = def.get("properties").and_then(Json::as_object);
        let es_type = def
            .get("type")
            .and_then(Json::as_str)
            .unwrap_or("object")
            .to_string();
        match es_type.as_str() {
            // A field alias points at another field and is not in `_source`.
            "alias" => continue,
            "object" | "passthrough"
                if def.get("enabled").and_then(Json::as_bool) != Some(false)
                    && sub.is_some_and(|s| !s.is_empty())
                    && depth < 64 =>
            {
                walk(sub.unwrap_or_default(), &format!("{path}."), out, depth + 1);
                continue;
            }
            _ => {}
        }
        let keyword = def.get("fields").and_then(Json::as_object).and_then(|fs| {
            let is_kw = |d: &Json| {
                matches!(
                    d.get("type").and_then(Json::as_str),
                    Some("keyword" | "constant_keyword")
                )
            };
            fs.iter()
                .find(|(n, d)| n == "keyword" && is_kw(d))
                .or_else(|| fs.iter().find(|(_, d)| is_kw(d)))
                .map(|(n, _)| format!("{path}.{n}"))
        });
        out.push(Field {
            path,
            es_type,
            keyword,
            indexed: def.get("index").and_then(Json::as_bool) != Some(false),
            doc_values: def.get("doc_values").and_then(Json::as_bool) != Some(false),
        });
    }
}

/// The fields of several indices (an alias or pattern): the union in first-seen order; a
/// field typed differently across indices keeps the types joined with `|` and is a JSON cell
/// that is neither sorted nor matched exactly.
pub fn merge(mappings: &[(String, IndexMapping)]) -> IndexMapping {
    let mut out = IndexMapping {
        source_enabled: true,
        ..IndexMapping::default()
    };
    for (_, m) in mappings {
        out.routing_required |= m.routing_required;
        out.source_enabled &= m.source_enabled;
        if out.type_name.is_none() {
            out.type_name = m.type_name.clone();
        }
        for f in &m.fields {
            match out.fields.iter_mut().find(|g| g.path == f.path) {
                None => out.fields.push(f.clone()),
                Some(g) if g.es_type == f.es_type => {
                    if g.keyword != f.keyword {
                        g.keyword = None;
                    }
                }
                Some(g) => {
                    if !g.es_type.split('|').any(|t| t == f.es_type) {
                        g.es_type = format!("{}|{}", g.es_type, f.es_type);
                    }
                    g.keyword = None;
                    g.doc_values = false;
                }
            }
        }
    }
    if mappings.len() == 1 {
        out.raw = mappings[0].1.raw.clone();
    }
    out
}

/// What a browse target is, for its columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnsFor {
    /// An alias, data stream or pattern: rows name their index.
    pub spans_indices: bool,
    /// The cluster supports `if_seq_no` guards.
    pub seq_no_guards: bool,
}

/// The grid columns of a mapping.
pub fn columns(mapping: &IndexMapping, target: ColumnsFor) -> Vec<ColumnMeta> {
    let mut out = Vec::new();
    let mut pk = 0;
    let mut push = |out: &mut Vec<ColumnMeta>, mut c: ColumnMeta, is_pk: bool| {
        c.ordinal = out.len() as u32 + 1;
        if is_pk {
            pk += 1;
            c.is_pk = true;
            c.pk_ordinal = Some(pk);
        }
        out.push(c);
    };
    if target.spans_indices {
        let mut c = column(INDEX, "keyword", false);
        c.is_generated = true;
        c.comment = Some("the index the document is in".into());
        push(&mut out, c, true);
    }
    let mut id = column(ID, "keyword", false);
    id.is_auto_increment = true;
    id.comment = Some("document id; generated when an insert leaves it empty".into());
    push(&mut out, id, true);
    if mapping.routing_required {
        let mut c = column(ROUTING, "keyword", false);
        c.comment = Some("custom routing, required by the mapping".into());
        push(&mut out, c, true);
    }
    if target.seq_no_guards {
        for name in [SEQ_NO, PRIMARY_TERM] {
            let mut c = column(name, "long", false);
            c.is_generated = true;
            c.comment = Some("concurrency guard sent back with each edit".into());
            push(&mut out, c, true);
        }
    }
    for f in &mapping.fields {
        if META_COLUMNS.contains(&f.path.as_str()) {
            continue;
        }
        let mut c = column(&f.path, &f.es_type, true);
        c.kind = if f.es_type.contains('|') {
            ValueKind::Json
        } else {
            f.kind()
        };
        c.extra = match (&f.keyword, f.sort_field()) {
            (Some(k), _) => Some(format!("{SORT_PREFIX}{k}")),
            (None, None) => Some("not sortable".into()),
            (None, Some(_)) => None,
        };
        push(&mut out, c, false);
    }
    out
}

fn column(name: &str, es_type: &str, nullable: bool) -> ColumnMeta {
    ColumnMeta {
        name: name.to_string(),
        ordinal: 0,
        data_type: es_type.to_string(),
        kind: kind_of(es_type),
        nullable,
        default_expr: None,
        is_pk: false,
        pk_ordinal: None,
        is_auto_increment: false,
        is_generated: false,
        is_unsigned: es_type == "unsigned_long",
        comment: None,
        enum_values: None,
        char_max_len: None,
        numeric_precision: None,
        numeric_scale: None,
        collation: None,
        array_elem_type: None,
        extra: None,
        base_type: None,
    }
}

/// The settings Tabler reads from `GET /<index>/_settings?flat_settings=true`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IndexSettings {
    pub shards: Option<u32>,
    pub replicas: Option<u32>,
    pub max_result_window: Option<u64>,
    pub refresh_interval: Option<String>,
    pub hidden: bool,
    /// A write block (`index.blocks.write`, `read_only`, `read_only_allow_delete`).
    pub write_blocked: bool,
    /// Every setting, flat (`index.number_of_shards` → `"1"`).
    pub flat: Vec<(String, Json)>,
}

/// `GET /<index>/_settings?flat_settings=true`: the settings of each index.
pub fn parse_settings(reply: &Json) -> Result<Vec<(String, IndexSettings)>, AppError> {
    let indices = reply
        .as_object()
        .ok_or_else(|| AppError::db("unexpected _settings reply: not an object"))?;
    Ok(indices
        .iter()
        .map(|(index, body)| {
            let flat: Vec<(String, Json)> = body
                .get("settings")
                .and_then(Json::as_object)
                .map(<[_]>::to_vec)
                .unwrap_or_default();
            let get = |k: &str| flat.iter().find(|(n, _)| n == k).map(|(_, v)| v);
            let truthy = |k: &str| {
                get(k).is_some_and(|v| v.as_bool() == Some(true) || v.as_str() == Some("true"))
            };
            let settings = IndexSettings {
                shards: get("index.number_of_shards")
                    .and_then(Json::as_u64)
                    .map(|n| n as u32),
                replicas: get("index.number_of_replicas")
                    .and_then(Json::as_u64)
                    .map(|n| n as u32),
                max_result_window: get("index.max_result_window").and_then(Json::as_u64),
                refresh_interval: get("index.refresh_interval").and_then(Json::scalar_text),
                hidden: truthy("index.hidden"),
                write_blocked: truthy("index.blocks.write")
                    || truthy("index.blocks.read_only")
                    || truthy("index.blocks.read_only_allow_delete"),
                flat,
            };
            (index.clone(), settings)
        })
        .collect())
}

/// Settings a cluster sets itself and refuses on a new index.
pub fn is_internal_setting(key: &str) -> bool {
    const INTERNAL: &[&str] = &[
        "index.uuid",
        "index.creation_date",
        "index.creation_date_string",
        "index.provided_name",
        "index.version.created",
        "index.version.created_string",
        "index.version.upgraded",
        "index.version.upgraded_string",
        "index.history.uuid",
        "index.resize.source.name",
        "index.resize.source.uuid",
        "index.routing.allocation.initial_recovery._id",
        "index.verified_before_close",
    ];
    INTERNAL.contains(&key) || key.starts_with("index.version.") || key.starts_with("archived.")
}

/// One row of `GET _cat/indices?format=json&bytes=b`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatIndex {
    pub index: String,
    pub health: Option<String>,
    pub status: Option<String>,
    pub docs: Option<i64>,
    pub size_bytes: Option<i64>,
    pub primaries: Option<i64>,
    pub replicas: Option<i64>,
}

pub fn parse_cat_indices(reply: &Json) -> Result<Vec<CatIndex>, AppError> {
    let rows = reply
        .as_array()
        .ok_or_else(|| AppError::db("unexpected _cat/indices reply: not an array"))?;
    let text = |r: &Json, k: &str| r.get(k).and_then(Json::as_str).map(str::to_string);
    let mut out: Vec<CatIndex> = rows
        .iter()
        .filter_map(|r| {
            Some(CatIndex {
                index: text(r, "index")?,
                health: text(r, "health"),
                status: text(r, "status"),
                docs: r.get("docs.count").and_then(Json::as_i64),
                size_bytes: r.get("store.size").and_then(Json::as_i64),
                primaries: r.get("pri").and_then(Json::as_i64),
                replicas: r.get("rep").and_then(Json::as_i64),
            })
        })
        .collect();
    out.sort_by(|a, b| a.index.cmp(&b.index));
    Ok(out)
}

/// One row of `GET _cat/aliases?format=json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatAlias {
    pub alias: String,
    pub index: String,
    /// `-` when the alias has no filter.
    pub filtered: bool,
    pub is_write_index: Option<bool>,
}

pub fn parse_cat_aliases(reply: &Json) -> Result<Vec<CatAlias>, AppError> {
    let rows = reply
        .as_array()
        .ok_or_else(|| AppError::db("unexpected _cat/aliases reply: not an array"))?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(CatAlias {
                alias: r.get("alias")?.as_str()?.to_string(),
                index: r.get("index")?.as_str()?.to_string(),
                filtered: r
                    .get("filter")
                    .and_then(Json::as_str)
                    .is_some_and(|f| f != "-" && f != "*"),
                is_write_index: match r.get("is_write_index").and_then(Json::as_str) {
                    Some("true") => Some(true),
                    Some("false") => Some(false),
                    _ => None,
                },
            })
        })
        .collect())
}

/// One data stream of `GET /_data_stream`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataStream {
    pub name: String,
    pub indices: Vec<String>,
    pub status: Option<String>,
    pub template: Option<String>,
    pub hidden: bool,
    pub system: bool,
}

pub fn parse_data_streams(reply: &Json) -> Result<Vec<DataStream>, AppError> {
    let streams = reply
        .get("data_streams")
        .and_then(Json::as_array)
        .ok_or_else(|| AppError::db("unexpected _data_stream reply: no data_streams"))?;
    Ok(streams
        .iter()
        .filter_map(|s| {
            Some(DataStream {
                name: s.get("name")?.as_str()?.to_string(),
                indices: s
                    .get("indices")
                    .and_then(Json::as_array)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|i| i.get("index_name").and_then(Json::as_str))
                    .map(str::to_string)
                    .collect(),
                status: s.get("status").and_then(Json::as_str).map(str::to_string),
                template: s.get("template").and_then(Json::as_str).map(str::to_string),
                hidden: s.get("hidden").and_then(Json::as_bool).unwrap_or(false),
                system: s.get("system").and_then(Json::as_bool).unwrap_or(false),
            })
        })
        .collect())
}

/// Whether an index or alias name is a system or hidden one (dot-prefixed).
pub fn is_system_name(name: &str) -> bool {
    name.starts_with('.')
}

/// The sidebar's tables: indices (`Table`), aliases (`View`), data streams (`Partitioned`),
/// dot-prefixed names as `SystemView` only when `show_system`. Backing indices of data streams
/// are left out (the stream stands for them).
pub fn tables(
    schema: &str,
    indices: &[CatIndex],
    aliases: &[CatAlias],
    streams: &[DataStream],
    show_system: bool,
) -> Vec<TableSummary> {
    let backing: std::collections::HashSet<&str> = streams
        .iter()
        .flat_map(|s| s.indices.iter().map(String::as_str))
        .collect();
    let mut out = Vec::new();
    let visible = |name: &str, hidden: bool| show_system || !(hidden || is_system_name(name));
    let kind_for = |name: &str, kind: TableKind| {
        if is_system_name(name) {
            TableKind::SystemView
        } else {
            kind
        }
    };
    for i in indices {
        if backing.contains(i.index.as_str()) || !visible(&i.index, false) {
            continue;
        }
        let mut comment = Vec::new();
        if let Some(h) = &i.health {
            comment.push(h.clone());
        }
        if let Some(s) = &i.status {
            comment.push(s.clone());
        }
        if let (Some(p), Some(r)) = (i.primaries, i.replicas) {
            comment.push(format!(
                "{p} primar{}, {r} replica{}",
                if p == 1 { "y" } else { "ies" },
                if r == 1 { "" } else { "s" }
            ));
        }
        out.push(TableSummary {
            name: i.index.clone(),
            schema: Some(schema.to_string()),
            kind: kind_for(&i.index, TableKind::Table),
            row_estimate: i.docs,
            size_bytes: i.size_bytes,
            comment: (!comment.is_empty()).then(|| comment.join(", ")),
        });
    }
    let mut by_alias: BTreeMap<&str, Vec<&CatAlias>> = BTreeMap::new();
    for a in aliases {
        by_alias.entry(a.alias.as_str()).or_default().push(a);
    }
    for (alias, rows) in by_alias {
        if !visible(alias, false) {
            continue;
        }
        let targets: Vec<&str> = rows.iter().map(|r| r.index.as_str()).collect();
        let mut comment = format!("alias of {}", targets.join(", "));
        if rows.iter().any(|r| r.filtered) {
            comment.push_str(" (filtered)");
        }
        out.push(TableSummary {
            name: alias.to_string(),
            schema: Some(schema.to_string()),
            kind: kind_for(alias, TableKind::View),
            row_estimate: None,
            size_bytes: None,
            comment: Some(comment),
        });
    }
    for s in streams {
        if !visible(&s.name, s.hidden || s.system) {
            continue;
        }
        let docs: Option<i64> = s
            .indices
            .iter()
            .map(|b| indices.iter().find(|i| &i.index == b).and_then(|i| i.docs))
            .sum();
        let mut comment = format!(
            "data stream, {} backing ind{}",
            s.indices.len(),
            if s.indices.len() == 1 { "ex" } else { "ices" }
        );
        if let Some(st) = &s.status {
            comment.push_str(&format!(", {}", st.to_ascii_lowercase()));
        }
        out.push(TableSummary {
            name: s.name.clone(),
            schema: Some(schema.to_string()),
            kind: if s.hidden || s.system {
                TableKind::SystemView
            } else {
                TableKind::Partitioned
            },
            row_estimate: docs,
            size_bytes: None,
            comment: Some(comment),
        });
    }
    out
}

/// The completion lists: each index, alias and stream with its field paths.
pub fn completion_fields(mappings: &[(String, IndexMapping)]) -> BTreeMap<String, Vec<String>> {
    mappings
        .iter()
        .map(|(index, m)| {
            let mut names: Vec<String> = vec![ID.to_string()];
            names.extend(m.fields.iter().map(|f| f.path.clone()));
            (index.clone(), names)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::elasticsearch::json::parse;

    const MAPPING: &str = r#"{"logs":{"mappings":{"_routing":{"required":true},"dynamic":"strict","properties":{
        "@timestamp":{"type":"date"},
        "message":{"type":"text","fields":{"keyword":{"type":"keyword","ignore_above":256}}},
        "status":{"type":"keyword","doc_values":false},
        "user":{"properties":{"name":{"type":"keyword"},"geo":{"properties":{"loc":{"type":"geo_point"}}}}},
        "tags":{"type":"nested","properties":{"k":{"type":"keyword"}}},
        "labels":{"type":"flattened"},
        "raw":{"type":"object","enabled":false},
        "empty":{"type":"object"},
        "v":{"type":"dense_vector","dims":3},
        "price":{"type":"scaled_float","scaling_factor":100},
        "big":{"type":"unsigned_long"},
        "ip":{"type":"ip"},
        "blob":{"type":"binary"},
        "ok":{"type":"boolean"},
        "range":{"type":"integer_range"},
        "msg_alias":{"type":"alias","path":"message"}
    }}}}"#;

    #[test]
    fn mappings_flatten_into_columns() {
        let m = parse_mappings(&parse(MAPPING).unwrap()).unwrap();
        assert_eq!(m.len(), 1);
        let (index, mapping) = &m[0];
        assert_eq!(index, "logs");
        assert!(mapping.routing_required && mapping.source_enabled);
        let paths: Vec<&str> = mapping.fields.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "@timestamp",
                "message",
                "status",
                "user.name",
                "user.geo.loc",
                "tags",
                "labels",
                "raw",
                "empty",
                "v",
                "price",
                "big",
                "ip",
                "blob",
                "ok",
                "range"
            ]
        );
        let field = |p: &str| mapping.fields.iter().find(|f| f.path == p).unwrap();
        assert_eq!(field("message").keyword.as_deref(), Some("message.keyword"));
        assert_eq!(field("message").sort_field(), Some("message.keyword"));
        assert_eq!(field("status").sort_field(), None);
        assert_eq!(field("price").sort_field(), Some("price"));
        assert_eq!(field("tags").es_type, "nested");

        let cols = columns(
            mapping,
            ColumnsFor {
                spans_indices: false,
                seq_no_guards: true,
            },
        );
        let names: Vec<&str> = cols.iter().take(4).map(|c| c.name.as_str()).collect();
        assert_eq!(names, [ID, ROUTING, SEQ_NO, PRIMARY_TERM]);
        assert!(cols.iter().take(4).all(|c| c.is_pk));
        assert!(cols[0].is_auto_increment && !cols[0].nullable);
        let kind = |n: &str| cols.iter().find(|c| c.name == n).unwrap().kind;
        assert_eq!(kind("@timestamp"), ValueKind::TimestampTz);
        assert_eq!(kind("message"), ValueKind::Text);
        assert_eq!(kind("user.geo.loc"), ValueKind::Geometry);
        assert_eq!(kind("tags"), ValueKind::Json);
        assert_eq!(kind("labels"), ValueKind::Json);
        assert_eq!(kind("raw"), ValueKind::Json);
        assert_eq!(kind("v"), ValueKind::Json);
        assert_eq!(kind("price"), ValueKind::Float);
        assert_eq!(kind("big"), ValueKind::BigInt);
        assert_eq!(kind("ip"), ValueKind::Other);
        assert_eq!(kind("blob"), ValueKind::Bytes);
        assert_eq!(kind("ok"), ValueKind::Bool);
        assert_eq!(kind("range"), ValueKind::Json);
        let message = cols.iter().find(|c| c.name == "message").unwrap();
        assert_eq!(message.extra.as_deref(), Some("sort: message.keyword"));
        // Columns round-trip to fields for the filter and sort builders.
        let f = Field::from_column(message);
        assert_eq!(f.sort_field(), Some("message.keyword"));
        let status = Field::from_column(cols.iter().find(|c| c.name == "status").unwrap());
        assert_eq!(status.sort_field(), None);
    }

    #[test]
    fn typed_six_x_mappings_and_merges() {
        let six = parse(
            r#"{"a":{"mappings":{"doc":{"properties":{"x":{"type":"long"}}}}},
                "b":{"mappings":{"properties":{"x":{"type":"keyword"},"y":{"type":"text"}}}},
                "c":{"mappings":{}}}"#,
        )
        .unwrap();
        let m = parse_mappings(&six).unwrap();
        assert_eq!(m[0].1.type_name.as_deref(), Some("doc"));
        assert_eq!(m[0].1.fields[0].path, "x");
        assert_eq!(m[1].1.type_name, None);
        assert!(m[2].1.fields.is_empty());
        let merged = merge(&m);
        assert_eq!(merged.fields[0].es_type, "long|keyword");
        assert_eq!(merged.fields[0].sort_field(), None);
        let cols = columns(
            &merged,
            ColumnsFor {
                spans_indices: true,
                seq_no_guards: false,
            },
        );
        assert_eq!(cols[0].name, INDEX);
        assert_eq!(cols[1].name, ID);
        assert_eq!(cols[2].kind, ValueKind::Json);
        assert!(!cols.iter().any(|c| c.name == SEQ_NO));
        // A mapping whose only property is called like a parameter is not typed.
        let m =
            flatten_mapping(&parse(r#"{"properties":{"dynamic":{"type":"keyword"}}}"#).unwrap());
        assert_eq!(m.type_name, None);
        assert_eq!(m.fields[0].path, "dynamic");
    }

    #[test]
    fn cat_replies_become_tables() {
        let indices = parse_cat_indices(
            &parse(
                r#"[{"health":"yellow","status":"open","index":"logs","uuid":"u1","pri":"1","rep":"1","docs.count":"1200","store.size":"52341"},
                    {"health":"green","status":"open","index":".kibana_1","pri":"1","rep":"0","docs.count":"10","store.size":"100"},
                    {"health":null,"status":"close","index":"old","pri":"1","rep":"1","docs.count":null,"store.size":null},
                    {"health":"green","status":"open","index":".ds-events-2024.01.01-000001","pri":"1","rep":"1","docs.count":"7","store.size":"9"}]"#,
            )
            .unwrap(),
        )
        .unwrap();
        let aliases = parse_cat_aliases(
            &parse(
                r#"[{"alias":"current","index":"logs","filter":"-","routing.index":"-","is_write_index":"true"},
                    {"alias":"current","index":"old","filter":"*","is_write_index":"-"},
                    {"alias":".kibana","index":".kibana_1","filter":"-"}]"#,
            )
            .unwrap(),
        )
        .unwrap();
        let streams = parse_data_streams(
            &parse(
                r#"{"data_streams":[{"name":"events","timestamp_field":{"name":"@timestamp"},"indices":[{"index_name":".ds-events-2024.01.01-000001","index_uuid":"x"}],"generation":1,"status":"GREEN","template":"events","hidden":false,"system":false}]}"#,
            )
            .unwrap(),
        )
        .unwrap();
        let t = tables("c", &indices, &aliases, &streams, false);
        let names: Vec<(&str, TableKind)> = t.iter().map(|t| (t.name.as_str(), t.kind)).collect();
        assert_eq!(
            names,
            [
                ("logs", TableKind::Table),
                ("old", TableKind::Table),
                ("current", TableKind::View),
                ("events", TableKind::Partitioned)
            ]
        );
        assert_eq!(t[0].row_estimate, Some(1200));
        assert_eq!(t[0].size_bytes, Some(52341));
        assert_eq!(
            t[0].comment.as_deref(),
            Some("yellow, open, 1 primary, 1 replica")
        );
        assert_eq!(t[1].row_estimate, None);
        assert_eq!(t[2].comment.as_deref(), Some("alias of logs, old"));
        assert_eq!(t[3].row_estimate, Some(7));
        let all = tables("c", &indices, &aliases, &streams, true);
        assert!(
            all.iter()
                .any(|t| t.name == ".kibana_1" && t.kind == TableKind::SystemView)
        );
        assert!(
            all.iter()
                .any(|t| t.name == ".kibana" && t.kind == TableKind::SystemView)
        );
        assert!(!all.iter().any(|t| t.name.starts_with(".ds-")));
    }

    #[test]
    fn settings_parse() {
        let s = parse_settings(
            &parse(
                r#"{"logs":{"settings":{"index.number_of_shards":"3","index.number_of_replicas":"1","index.max_result_window":"50000","index.refresh_interval":"30s","index.blocks.write":"true","index.uuid":"u"}}}"#,
            )
            .unwrap(),
        )
        .unwrap();
        let (_, s) = &s[0];
        assert_eq!(s.shards, Some(3));
        assert_eq!(s.max_result_window, Some(50000));
        assert_eq!(s.refresh_interval.as_deref(), Some("30s"));
        assert!(s.write_blocked && !s.hidden);
        assert!(
            is_internal_setting("index.uuid") && !is_internal_setting("index.number_of_shards")
        );
        assert!(parse_settings(&parse("[]").unwrap()).is_err());
    }

    #[test]
    fn malformed_replies_are_errors_not_panics() {
        assert!(parse_mappings(&parse("[1]").unwrap()).is_err());
        assert!(parse_cat_indices(&parse("{}").unwrap()).is_err());
        assert!(parse_data_streams(&parse("{}").unwrap()).is_err());
        // Rows without the fields Tabler needs are skipped.
        assert!(
            parse_cat_indices(&parse(r#"[{"x":1}]"#).unwrap())
                .unwrap()
                .is_empty()
        );
        let m = flatten_mapping(&parse(r#"{"properties":{"a":1,"b":{"type":7}}}"#).unwrap());
        assert_eq!(m.fields.len(), 2);
    }
}
