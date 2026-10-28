//! MongoDB command and document builders (the mongodb lane): the database commands Tabler sends
//! and their text. A command is a BSON document run with `runCommand`; its preview is the same
//! document as `db.runCommand({…})` in Extended JSON, which the console reads back to the same
//! command (`drivers::mongodb::shell`), so Preview shows exactly what runs.
//!
//! - Extended JSON text (`bson_json`): key order kept, numbers written so they read back as the
//!   same type (see `JsonMode::Relaxed`).
//! - Catalog and server commands, the grid's `find` / `count`, the schema sample.
//! - The filter bar (`FilterGroup`) as a query document, sorts and projections.
//! - Grid edits (`StagedChange`) as `update` / `insert` / `delete` commands by `_id`, each
//!   expecting `n: 1`.
//! - Collection and index definitions as shell text (`db.createCollection(…)`).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use ::mongodb::bson::{Binary, Bson, Document, Regex, doc, oid::ObjectId, spec::BinarySubtype};

use crate::drivers::mongodb::literal;
use crate::drivers::mongodb::values::{self, is_missing, special_double};
use crate::error::AppError;
use crate::types::browse::{
    Conjunction, FilterGroup, FilterNode, FilterOp, RowKey, SortDir, SortSpec,
};
use crate::types::mutate::{Clock, ServerExpr, StagedChange};
use crate::types::schema::{ColumnMeta, TableRef};
use crate::types::value::Value;

/// The key every document has; the grid's row identity.
pub const ID: &str = "_id";

/// Operations Tabler starts carry `comment: "tabler-<id>"`, so a cancel can find them in
/// `$currentOp` and kill them.
pub const COMMENT_PREFIX: &str = "tabler-";

/// Deeper values are written as a placeholder (a server stores at most 100 levels).
const MAX_WRITE_DEPTH: usize = 256;

// ---------------------------------------------------------------------------------------------
// Extended JSON text
// ---------------------------------------------------------------------------------------------

/// How numbers and dates are spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonMode {
    /// Relaxed Extended JSON v2, kept exact on the way back: an int32 is a plain number, a
    /// double always has a `.` or an exponent (`1.0`), a long is a plain number only where it
    /// reads back as a long (outside the int32 range and within 2^53, so JavaScript keeps it
    /// exact) and `{"$numberLong": …}` otherwise; dates of the years 1970–9999 are ISO text.
    Relaxed,
    /// Canonical Extended JSON v2: every number and date typed.
    Canonical,
}

/// A value as Extended JSON text.
pub fn bson_json(b: &Bson, mode: JsonMode) -> String {
    let mut out = String::new();
    write_bson(&mut out, b, mode, 0);
    out
}

/// A document as Extended JSON text.
pub fn doc_json(d: &Document, mode: JsonMode) -> String {
    let mut out = String::new();
    write_doc(&mut out, d, mode, 0);
    out
}

/// A string as a JSON string literal.
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    write_str(&mut out, s);
    out
}

fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_doc(out: &mut String, d: &Document, mode: JsonMode, depth: usize) {
    if d.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push('{');
    for (i, (k, v)) in d.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_str(out, k);
        out.push_str(": ");
        write_bson(out, v, mode, depth + 1);
    }
    out.push('}');
}

/// One-key wrapper `{"$key": <value>}` with a string value.
fn wrap_str(out: &mut String, key: &str, value: &str) {
    out.push_str("{\"");
    out.push_str(key);
    out.push_str("\": ");
    write_str(out, value);
    out.push('}');
}

/// A double as text that reads back as a double: shortest round trip, always with a `.` or an
/// exponent (`1.0`, `1e300`).
pub fn double_text(f: f64) -> String {
    format!("{f:?}")
}

fn write_bson(out: &mut String, b: &Bson, mode: JsonMode, depth: usize) {
    if depth > MAX_WRITE_DEPTH {
        out.push_str("\"(nested too deeply)\"");
        return;
    }
    let canonical = mode == JsonMode::Canonical;
    match b {
        Bson::Double(f) if !f.is_finite() => wrap_str(out, "$numberDouble", special_double(*f)),
        Bson::Double(f) if canonical => wrap_str(out, "$numberDouble", &double_text(*f)),
        Bson::Double(f) => out.push_str(&double_text(*f)),
        Bson::String(s) => write_str(out, s),
        Bson::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_bson(out, item, mode, depth + 1);
            }
            out.push(']');
        }
        Bson::Document(d) => write_doc(out, d, mode, depth),
        Bson::Boolean(v) => out.push_str(if *v { "true" } else { "false" }),
        Bson::Null => out.push_str("null"),
        Bson::Int32(n) if canonical => wrap_str(out, "$numberInt", &n.to_string()),
        Bson::Int32(n) => {
            let _ = write!(out, "{n}");
        }
        Bson::Int64(n) => {
            let plain = !canonical
                && i32::try_from(*n).is_err()
                && n.unsigned_abs() <= crate::types::value::JS_SAFE_INT as u64;
            if plain {
                let _ = write!(out, "{n}");
            } else {
                wrap_str(out, "$numberLong", &n.to_string());
            }
        }
        Bson::Decimal128(d) => wrap_str(out, "$numberDecimal", &d.to_string()),
        Bson::DateTime(d) => {
            let ms = d.timestamp_millis();
            match values::iso_text(ms).filter(|_| !canonical && ms >= 0) {
                Some(iso) => wrap_str(out, "$date", &iso),
                None => {
                    out.push_str("{\"$date\": ");
                    wrap_str(out, "$numberLong", &ms.to_string());
                    out.push('}');
                }
            }
        }
        Bson::ObjectId(oid) => wrap_str(out, "$oid", &oid.to_hex()),
        Bson::Binary(bin) => {
            use base64::Engine as _;
            let subtype: u8 = bin.subtype.into();
            let _ = write!(
                out,
                "{{\"$binary\": {{\"base64\": \"{}\", \"subType\": \"{subtype:02x}\"}}}}",
                base64::engine::general_purpose::STANDARD.encode(&bin.bytes)
            );
        }
        Bson::RegularExpression(r) => {
            let mut options: Vec<char> = r.options.chars().collect();
            options.sort_unstable();
            let options: String = options.into_iter().collect();
            out.push_str("{\"$regularExpression\": {\"pattern\": ");
            write_str(out, &r.pattern);
            out.push_str(", \"options\": ");
            write_str(out, &options);
            out.push_str("}}");
        }
        Bson::Timestamp(t) => {
            let _ = write!(
                out,
                "{{\"$timestamp\": {{\"t\": {}, \"i\": {}}}}}",
                t.time, t.increment
            );
        }
        Bson::JavaScriptCode(code) => wrap_str(out, "$code", code),
        Bson::JavaScriptCodeWithScope(c) => {
            out.push_str("{\"$code\": ");
            write_str(out, &c.code);
            out.push_str(", \"$scope\": ");
            write_doc(out, &c.scope, mode, depth + 1);
            out.push('}');
        }
        Bson::Symbol(s) => wrap_str(out, "$symbol", s),
        Bson::Undefined => out.push_str("{\"$undefined\": true}"),
        Bson::MinKey => out.push_str("{\"$minKey\": 1}"),
        Bson::MaxKey => out.push_str("{\"$maxKey\": 1}"),
        // The crate keeps a DbPointer's parts private; its own Extended JSON has them.
        Bson::DbPointer(_) => {
            let json = b.clone().into_canonical_extjson();
            out.push_str(&serde_json::to_string(&json).unwrap_or_else(|_| "null".into()));
        }
    }
}

/// A command as the console writes it: `db.runCommand({…})`, on another database
/// `db.getSiblingDB("x").runCommand({…})`.
pub fn command_text(db: Option<&str>, command: &Document) -> String {
    let body = doc_json(command, JsonMode::Relaxed);
    match db {
        Some(db) => format!("db.getSiblingDB({}).runCommand({body})", json_string(db)),
        None => format!("db.runCommand({body})"),
    }
}

/// `db.getCollection("name")`: unambiguous for every name (`db.stats` is a method).
pub fn collection_text(name: &str) -> String {
    format!("db.getCollection({})", json_string(name))
}

// ---------------------------------------------------------------------------------------------
// Server and catalog commands
// ---------------------------------------------------------------------------------------------

pub fn hello() -> Document {
    doc! { "hello": 1 }
}

pub fn build_info() -> Document {
    doc! { "buildInfo": 1 }
}

pub fn connection_status() -> Document {
    doc! { "connectionStatus": 1 }
}

pub fn ping() -> Document {
    doc! { "ping": 1 }
}

/// Run on `admin`. `authorizedDatabases` lets a user without the listDatabases privilege see
/// the databases they may use (Atlas).
pub fn list_databases(name_only: bool) -> Document {
    doc! { "listDatabases": 1, "nameOnly": name_only, "authorizedDatabases": true }
}

pub fn list_collections(name_only: bool) -> Document {
    doc! { "listCollections": 1, "nameOnly": name_only, "authorizedCollections": true }
}

/// One collection's listCollections entry.
pub fn collection_info(name: &str) -> Document {
    doc! {
        "listCollections": 1,
        "filter": { "name": name },
        "nameOnly": false,
        "authorizedCollections": true,
    }
}

pub fn list_indexes(collection: &str) -> Document {
    doc! { "listIndexes": collection, "cursor": {} }
}

/// Storage numbers through `$collStats` (the collStats command is deprecated since 6.2).
pub fn coll_stats(collection: &str) -> Document {
    doc! {
        "aggregate": collection,
        "pipeline": [{ "$collStats": { "storageStats": {}, "count": {} } }],
        "cursor": {},
    }
}

pub fn db_stats() -> Document {
    doc! { "dbStats": 1 }
}

/// The schema sample: `size` random documents.
pub fn sample(collection: &str, size: u32) -> Document {
    doc! {
        "aggregate": collection,
        "pipeline": [{ "$sample": { "size": i64::from(size) } }],
        "cursor": {},
    }
}

/// The sample where `$sample` is missing or refused (views on some servers, DocumentDB): the
/// first documents, in one batch (one cut at 16 MiB only makes the sample smaller).
pub fn sample_fallback(collection: &str, size: u32) -> Document {
    doc! { "find": collection, "limit": i64::from(size), "singleBatch": true }
}

/// The fast count from the collection's metadata (no filter).
pub fn estimated_count(collection: &str) -> Document {
    doc! { "count": collection }
}

/// An exact count of the documents matching `filter`, as `countDocuments` runs it.
pub fn count_documents(collection: &str, filter: Document) -> Document {
    doc! {
        "aggregate": collection,
        "pipeline": [
            { "$match": filter },
            { "$group": { "_id": 1, "n": { "$sum": 1 } } },
        ],
        "cursor": {},
    }
}

/// The next batch of a cursor.
pub fn get_more(cursor_id: i64, collection: &str, batch_size: Option<i64>) -> Document {
    let mut d = doc! { "getMore": cursor_id, "collection": collection };
    if let Some(n) = batch_size {
        d.insert("batchSize", n);
    }
    d
}

pub fn kill_cursors(collection: &str, ids: &[i64]) -> Document {
    doc! { "killCursors": collection, "cursors": ids.to_vec() }
}

/// The comment of an operation Tabler starts (`tabler-<id>`).
pub fn op_comment(id: &str) -> String {
    format!("{COMMENT_PREFIX}{id}")
}

/// Run on `admin`: this connection's operations with `comment`, for a cancel.
pub fn current_op_by_comment(comment: &str) -> Document {
    doc! {
        "aggregate": 1,
        "pipeline": [
            { "$currentOp": { "allUsers": false, "idleConnections": false } },
            { "$match": { "$or": [
                { "command.comment": comment },
                { "cursor.originatingCommand.comment": comment },
            ] } },
            { "$project": { "opid": 1 } },
        ],
        "cursor": {},
    }
}

/// Run on `admin`.
pub fn kill_op(opid: Bson) -> Document {
    doc! { "killOp": 1, "op": opid }
}

// ---------------------------------------------------------------------------------------------
// Browse
// ---------------------------------------------------------------------------------------------

/// The grid's page of a collection.
#[derive(Debug, Clone, Default)]
pub struct FindSpec {
    pub filter: Document,
    pub sort: Document,
    pub projection: Option<Document>,
    pub skip: u64,
    pub limit: u32,
    pub max_time_ms: Option<u64>,
    pub comment: Option<String>,
}

pub fn find(collection: &str, spec: &FindSpec) -> Document {
    let mut d = doc! { "find": collection, "filter": spec.filter.clone() };
    if !spec.sort.is_empty() {
        d.insert("sort", spec.sort.clone());
    }
    if let Some(p) = &spec.projection {
        d.insert("projection", p.clone());
    }
    if spec.skip > 0 {
        d.insert("skip", i64::try_from(spec.skip).unwrap_or(i64::MAX));
    }
    if spec.limit > 0 {
        d.insert("limit", i64::from(spec.limit));
        // The whole page in the first batch when it fits in 16 MiB; otherwise the server cuts
        // the batch and `PageRead` fetches the rest. Never `singleBatch`: it closes the cursor
        // after a cut batch, and the page would come back short without a sign.
        d.insert("batchSize", i64::from(spec.limit));
    }
    if let Some(ms) = spec.max_time_ms.filter(|ms| *ms > 0) {
        d.insert("maxTimeMS", i64::try_from(ms).unwrap_or(i64::MAX));
    }
    if let Some(c) = &spec.comment {
        d.insert("comment", c.clone());
    }
    d
}

/// What the session runs after a batch of a grid page.
#[derive(Debug, Clone, PartialEq)]
pub enum PageStep {
    /// This `getMore`, whose batch goes to `PageRead::take` in turn.
    More(Document),
    /// The page is full while the server still holds the cursor: this `killCursors` frees it
    /// (its reply does not matter).
    Close(Document),
    /// The server closed the cursor: the page is complete.
    Done,
}

/// Reads a grid page to its end. A batch carries at most 16 MiB (the server's
/// `haveSpaceForNext`), so a page of large documents arrives in several batches even with
/// `batchSize` set to the page: the first from `find`, the rest through `getMore` on the same
/// cursor, whose `limit` the server keeps. Offset paging needs every page whole, or the next
/// page's `skip` passes over the documents a short page left out. The session runs the `find`
/// and its `getMore`s in one explicit session: a server refuses a cursor's `getMore` from
/// another one, and each `run_command` without a session takes an implicit one of its own.
#[derive(Debug, Clone)]
pub struct PageRead {
    collection: String,
    /// The page size; 0 reads to the cursor's end.
    limit: usize,
    rows: usize,
}

impl PageRead {
    pub fn new(collection: &str, spec: &FindSpec) -> PageRead {
        PageRead {
            collection: collection.to_string(),
            limit: spec.limit as usize,
            rows: 0,
        }
    }

    /// Takes a batch (`firstBatch`, then each `nextBatch`) with the reply's cursor id, keeps no
    /// more documents than the page has room for, and says what to run next.
    pub fn take<T>(&mut self, batch: &mut Vec<T>, cursor_id: i64) -> PageStep {
        if self.limit > 0 {
            batch.truncate(self.limit.saturating_sub(self.rows));
        }
        self.rows += batch.len();
        if cursor_id == 0 {
            return PageStep::Done;
        }
        let full = self.limit > 0 && self.rows >= self.limit;
        // An empty batch on an open cursor would only repeat: a grid page is never tailable.
        if full || batch.is_empty() {
            return PageStep::Close(kill_cursors(&self.collection, &[cursor_id]));
        }
        let rest = (self.limit > 0).then(|| (self.limit - self.rows) as i64);
        PageStep::More(get_more(cursor_id, &self.collection, rest))
    }

    /// The documents of the page so far.
    pub fn rows(&self) -> usize {
        self.rows
    }
}

/// One field of one document (`fetch_cell`). A name with a `.` or a leading `$` would project a
/// path or an expression, so such a field is read with its whole document.
pub fn fetch_field(collection: &str, id: Bson, field: &str) -> Document {
    let mut d = doc! { "find": collection, "filter": { ID: id } };
    if is_plain_field(field) {
        let mut projection = Document::new();
        projection.insert(field, 1);
        d.insert("projection", projection);
    }
    d.insert("limit", 1);
    d.insert("singleBatch", true);
    d
}

/// Whether a top-level field name means itself in a query, a projection or `$set`: one with a
/// `.` names a path into embedded documents, one starting with `$` an operator.
pub fn is_plain_field(name: &str) -> bool {
    !name.is_empty() && !name.contains('.') && !name.starts_with('$')
}

/// The sort of the grid; ties break on `_id` so offset pages do not repeat or skip documents.
/// MongoDB sorts null and missing first, so `nulls` is not expressible and is ignored.
pub fn sort_document(sort: &[SortSpec]) -> Document {
    let mut d = Document::new();
    for s in sort {
        if !d.contains_key(&s.column) {
            d.insert(
                s.column.clone(),
                if s.dir == SortDir::Desc { -1 } else { 1 },
            );
        }
    }
    if !d.is_empty() && !d.contains_key(ID) {
        d.insert(ID, 1);
    }
    d
}

/// The fields of a page (`_id` always, for the row identity); `None` for every field.
pub fn projection(columns: Option<&[String]>) -> Option<Document> {
    let columns = columns?;
    let mut d = doc! { ID: 1 };
    for c in columns {
        d.insert(c.clone(), 1);
    }
    Some(d)
}

// ---------------------------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------------------------

/// The type labels of a column (`ColumnMeta.data_type` is `string|null`).
pub fn column_types(data_type: &str) -> Vec<&str> {
    data_type
        .split('|')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect()
}

/// The filter bar as a query document. Values are matched as every type the column was seen
/// with (`5` typed in a column of ints and strings finds both `5` and `"5"`); the raw kind takes
/// a query document or a fragment of one (`"status": "A", qty: {$gt: 5}`) in shell syntax.
/// MongoDB semantics apply: `$ne` and `$not` also match documents without the field.
pub fn filter_document(
    group: Option<&FilterGroup>,
    columns: &[ColumnMeta],
) -> Result<Document, AppError> {
    match group {
        Some(g) => group_document(g, columns),
        None => Ok(Document::new()),
    }
}

fn group_document(g: &FilterGroup, columns: &[ColumnMeta]) -> Result<Document, AppError> {
    let parts = g
        .items
        .iter()
        .map(|item| node_document(item, columns))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(combine(g.conjunction, parts))
}

/// Several conditions joined: an AND of conditions on different fields is one document.
fn combine(conjunction: Conjunction, mut parts: Vec<Document>) -> Document {
    parts.retain(|p| conjunction == Conjunction::Or || !p.is_empty());
    match parts.len() {
        0 => Document::new(),
        1 => parts.remove(0),
        _ if conjunction == Conjunction::And => {
            let mut merged = Document::new();
            let disjoint = parts.iter().all(|p| {
                let ok = p.keys().all(|k| !merged.contains_key(k));
                merged.extend(p.clone());
                ok
            });
            if disjoint {
                merged
            } else {
                doc! { "$and": parts }
            }
        }
        _ => doc! { "$or": parts },
    }
}

fn node_document(node: &FilterNode, columns: &[ColumnMeta]) -> Result<Document, AppError> {
    match node {
        FilterNode::Condition { column, op, values } => {
            let types = columns
                .iter()
                .find(|c| &c.name == column)
                .map(|c| column_types(&c.data_type))
                .unwrap_or_default();
            condition(column, *op, values, &types)
        }
        FilterNode::Group(g) => group_document(g, columns),
        FilterNode::Raw { sql } => raw_filter(sql),
        FilterNode::AnyColumn { op, values } => {
            let parts = columns
                .iter()
                .map(|c| condition(&c.name, *op, values, &column_types(&c.data_type)))
                .collect::<Result<Vec<_>, _>>()?;
            if parts.is_empty() {
                return Ok(Document::new());
            }
            let conj = if op.is_negated() {
                Conjunction::And
            } else {
                Conjunction::Or
            };
            Ok(match conj {
                Conjunction::Or if parts.len() > 1 => doc! { "$or": parts },
                Conjunction::And if parts.len() > 1 => doc! { "$and": parts },
                _ => parts.into_iter().next().unwrap_or_default(),
            })
        }
    }
}

/// The raw filter kind: a query document, with or without its braces.
pub fn raw_filter(text: &str) -> Result<Document, AppError> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(Document::new());
    }
    let wrapped;
    let source = if t.starts_with('{') {
        t
    } else {
        wrapped = format!("{{{t}}}");
        &wrapped
    };
    literal::parse_document(source).map_err(|e| {
        let pos = e.position(source);
        AppError::InvalidInput(format!("raw filter: {} (column {})", e.message, pos.column))
    })
}

fn condition(
    field: &str,
    op: FilterOp,
    values: &[Value],
    types: &[&str],
) -> Result<Document, AppError> {
    let first = || {
        values
            .first()
            .ok_or_else(|| AppError::InvalidInput(format!("the filter on {field} needs a value")))
    };
    let field_doc = |v: Bson| {
        let mut d = Document::new();
        d.insert(field, v);
        d
    };
    let op_doc = |name: &str, v: Bson| {
        let mut inner = Document::new();
        inner.insert(name, v);
        field_doc(Bson::Document(inner))
    };
    Ok(match op {
        FilterOp::Eq => {
            let c = candidates(first()?, types)?;
            match c.as_slice() {
                [one] if plain_equality(one) => field_doc(one.clone()),
                [one] => op_doc("$eq", one.clone()),
                _ => op_doc("$in", Bson::Array(c)),
            }
        }
        FilterOp::Ne => {
            let c = candidates(first()?, types)?;
            match c.as_slice() {
                [one] => op_doc("$ne", one.clone()),
                _ => op_doc("$nin", Bson::Array(c)),
            }
        }
        FilterOp::Gt | FilterOp::Ge | FilterOp::Lt | FilterOp::Le => {
            let name = match op {
                FilterOp::Gt => "$gt",
                FilterOp::Ge => "$gte",
                FilterOp::Lt => "$lt",
                _ => "$lte",
            };
            let parts = range_candidates(first()?, types)?
                .into_iter()
                .map(|c| op_doc(name, c))
                .collect();
            combine(Conjunction::Or, parts)
        }
        FilterOp::Between | FilterOp::NotBetween => {
            let (Some(lo), Some(hi)) = (values.first(), values.get(1)) else {
                return Err(AppError::InvalidInput(format!(
                    "the filter on {field} needs two values"
                )));
            };
            let his = range_candidates(hi, types)?;
            let mut parts = Vec::new();
            for l in range_candidates(lo, types)? {
                for h in his.iter().filter(|h| same_class(&l, h)) {
                    parts.push(field_doc(Bson::Document(
                        doc! { "$gte": l.clone(), "$lte": h.clone() },
                    )));
                }
            }
            if parts.is_empty() {
                return Err(AppError::InvalidInput(format!(
                    "the bounds of the filter on {field} are of different types"
                )));
            }
            let between = combine(Conjunction::Or, parts);
            if op == FilterOp::NotBetween {
                doc! { "$nor": [between] }
            } else {
                between
            }
        }
        FilterOp::In | FilterOp::NotIn => {
            let mut all = Vec::new();
            for v in values {
                for c in candidates(v, types)? {
                    if !all.contains(&c) {
                        all.push(c);
                    }
                }
            }
            op_doc(
                if op == FilterOp::In { "$in" } else { "$nin" },
                Bson::Array(all),
            )
        }
        FilterOp::IsNull => field_doc(Bson::Null),
        FilterOp::IsNotNull => op_doc("$ne", Bson::Null),
        FilterOp::Like | FilterOp::NotLike | FilterOp::Ilike => {
            let pattern = text_of(first()?);
            let re = regex(like_pattern(&pattern), op == FilterOp::Ilike);
            if op == FilterOp::NotLike {
                op_doc("$not", re)
            } else {
                field_doc(re)
            }
        }
        FilterOp::Contains
        | FilterOp::NotContains
        | FilterOp::ContainsCase
        | FilterOp::NotContainsCase
        | FilterOp::HasPrefix
        | FilterOp::HasSuffix
        | FilterOp::HasPrefixCase
        | FilterOp::HasSuffixCase => {
            let lit = escape_regex(&text_of(first()?));
            let pattern = match op {
                FilterOp::HasPrefix | FilterOp::HasPrefixCase => format!("^{lit}"),
                FilterOp::HasSuffix | FilterOp::HasSuffixCase => format!("{lit}$"),
                _ => lit,
            };
            // The plain operators match case, the `…Case` ones ignore it (as on PostgreSQL).
            let insensitive = matches!(
                op,
                FilterOp::ContainsCase
                    | FilterOp::NotContainsCase
                    | FilterOp::HasPrefixCase
                    | FilterOp::HasSuffixCase
            );
            let re = regex(pattern, insensitive);
            if matches!(op, FilterOp::NotContains | FilterOp::NotContainsCase) {
                op_doc("$not", re)
            } else {
                field_doc(re)
            }
        }
        FilterOp::JsonContains | FilterOp::JsonNotContains => {
            return Err(AppError::Unsupported(
                "JSON containment filters are not available on MongoDB; use a raw filter".into(),
            ));
        }
    })
}

/// `{field: value}` reads as equality unless the value is a regex (a match) or a document
/// that starts with an operator.
fn plain_equality(v: &Bson) -> bool {
    match v {
        Bson::RegularExpression(_) => false,
        Bson::Document(d) => !d.keys().next().is_some_and(|k| k.starts_with('$')),
        _ => true,
    }
}

/// Values whose comparison MongoDB orders together (numbers with numbers…).
fn same_class(a: &Bson, b: &Bson) -> bool {
    let class = |v: &Bson| match v {
        Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_) | Bson::Decimal128(_) => "number",
        Bson::String(_) | Bson::Symbol(_) => "string",
        other => values::type_name(other),
    };
    class(a) == class(b)
}

fn text_of(v: &Value) -> String {
    v.as_text().unwrap_or_default()
}

fn regex(pattern: String, insensitive: bool) -> Bson {
    Bson::RegularExpression(Regex {
        pattern,
        // `s`: `%` and `.` match line breaks too, as SQL's LIKE does.
        options: if insensitive { "is" } else { "s" }.into(),
    })
}

/// A literal as a regular expression that matches it (PCRE metacharacters escaped).
pub fn escape_regex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\^$.|?*+()[]{}/-".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A SQL LIKE pattern (`%`, `_`, `\` escapes) as an anchored regular expression.
pub fn like_pattern(like: &str) -> String {
    let mut out = String::from("^");
    let mut chars = like.chars();
    while let Some(c) = chars.next() {
        match c {
            '%' => out.push_str(".*"),
            '_' => out.push('.'),
            '\\' => match chars.next() {
                Some(n) => out.push_str(&escape_regex(&n.to_string())),
                None => out.push_str("\\\\"),
            },
            c => out.push_str(&escape_regex(&c.to_string())),
        }
    }
    out.push('$');
    out
}

/// What a filter value matches as, for a column seen with `types`: the value as typed, and its
/// text read as each other type the column holds (only those it parses as).
pub fn candidates(v: &Value, types: &[&str]) -> Result<Vec<Bson>, AppError> {
    let mut out: Vec<Bson> = Vec::new();
    let push = |b: Bson, out: &mut Vec<Bson>| {
        if !out.contains(&b) {
            out.push(b);
        }
    };
    match v {
        Value::Text(_) | Value::Enum(_) => {}
        Value::Null => return Ok(vec![Bson::Null]),
        Value::Other(o) if o.ty != values::TY_OBJECT_ID => {
            push(values::to_bson(v, None)?, &mut out);
        }
        // A typed value as its own type, and its text as the column's others.
        _ => {
            let hint = types.first().copied();
            push(values::to_bson(v, hint)?, &mut out);
        }
    }
    let text = text_of(v);
    let t = text.trim();
    for ty in types {
        let b = match *ty {
            "string" => Some(Bson::String(text.clone())),
            "objectId" => ObjectId::parse_str(t).ok().map(Bson::ObjectId),
            "int" | "long" | "double" => match t.parse::<i64>() {
                Ok(n) => Some(Bson::Int64(n)),
                Err(_) => t
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite())
                    .map(Bson::Double),
            },
            "decimal" => values::parse_decimal(t).map(Bson::Decimal128),
            "bool" => match t.to_ascii_lowercase().as_str() {
                "true" => Some(Bson::Boolean(true)),
                "false" => Some(Bson::Boolean(false)),
                _ => None,
            },
            "date" => values::parse_date_millis(t)
                .map(|ms| Bson::DateTime(::mongodb::bson::DateTime::from_millis(ms))),
            "uuid" => values::to_bson(&Value::Uuid(t.to_string()), None).ok(),
            _ => None,
        };
        if let Some(b) = b {
            // Numbers compare across types: one numeric candidate per numeric reading.
            let numeric_dup = matches!(b, Bson::Int64(_) | Bson::Double(_))
                && out.iter().any(|o| same_number(o, &b));
            if !numeric_dup {
                push(b, &mut out);
            }
        }
    }
    if out.is_empty() {
        out.push(Bson::String(text));
    }
    Ok(out)
}

/// The candidates of a range bound: a number or date reading orders the values, so the text
/// reading is dropped beside one (`26` must not compare as a string with `"unknown"`).
fn range_candidates(v: &Value, types: &[&str]) -> Result<Vec<Bson>, AppError> {
    let all = candidates(v, types)?;
    if all.iter().all(|c| matches!(c, Bson::String(_))) {
        return Ok(all);
    }
    Ok(all
        .into_iter()
        .filter(|c| !matches!(c, Bson::String(_)))
        .collect())
}

fn same_number(a: &Bson, b: &Bson) -> bool {
    let n = |v: &Bson| match v {
        Bson::Int32(i) => Some(f64::from(*i)),
        Bson::Int64(i) => Some(*i as f64),
        Bson::Double(f) => Some(*f),
        _ => None,
    };
    matches!((n(a), n(b)), (Some(x), Some(y)) if x == y)
}

// ---------------------------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------------------------

/// One write of a grid commit: a single-statement command whose reply must report `n` equal to
/// `expect_n` (one document matched, inserted or deleted).
#[derive(Debug, Clone, PartialEq)]
pub struct WriteOp {
    /// Index of the change in the request.
    pub change: usize,
    pub row_id: String,
    pub op: &'static str,
    pub command: Document,
    pub expect_n: i64,
}

/// The commands of a commit, with the database they run on (the table's schema, else the
/// session's).
#[derive(Debug, Clone, PartialEq)]
pub struct EditPlan {
    pub db: Option<String>,
    pub ops: Vec<WriteOp>,
}

impl EditPlan {
    /// The preview text of each change, from the same commands that run. The database is named
    /// when it is not the session's `current` one.
    pub fn preview(&self, current: Option<&str>) -> Vec<(usize, String)> {
        let db = self.db.as_deref().filter(|db| Some(*db) != current);
        self.ops
            .iter()
            .map(|op| (op.change, command_text(db, &op.command)))
            .collect()
    }
}

/// The `$type` alias a column's values mostly have (the first of its types), for the width of
/// numbers written to it.
fn hint_for<'a>(columns: &'a [ColumnMeta], name: &str) -> Option<&'a str> {
    columns.iter().find(|c| c.name == name).and_then(|c| {
        column_types(&c.data_type)
            .into_iter()
            .find(|t| *t != "null")
    })
}

/// The BSON types of the fields of the documents the grid loaded, by `_id`, so an edit keeps the
/// type the field had even in a column that mixes types: a number keeps its width (an Int32
/// stays an Int32, a long a long), and any other value is read as that type or refused
/// (`cell_bson`); a binary keeps its subtype. The session records each page it returns.
#[derive(Debug, Clone, Default)]
pub struct LoadedTypes {
    by_id: HashMap<String, HashMap<String, FieldType>>,
}

/// A loaded field's type label (`uuid` for a subtype-4 binary) and, for a binary, its subtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldType {
    pub label: &'static str,
    pub subtype: Option<BinarySubtype>,
}

impl LoadedTypes {
    /// Remembers the field types of a loaded document (one without `_id` is not editable).
    pub fn record(&mut self, d: &Document) {
        let Some(id) = d.get(ID) else { return };
        let fields = d
            .iter()
            .map(|(k, v)| {
                let subtype = match v {
                    Bson::Binary(bin) => Some(bin.subtype),
                    _ => None,
                };
                let label = values::type_label(v);
                (k.clone(), FieldType { label, subtype })
            })
            .collect();
        self.by_id
            .insert(bson_json(id, JsonMode::Canonical), fields);
    }

    /// The type `field` had in the document `id`, when that document was loaded with it.
    pub fn field(&self, id: &Bson, field: &str) -> Option<FieldType> {
        self.by_id
            .get(&bson_json(id, JsonMode::Canonical))?
            .get(field)
            .copied()
    }

    /// The type label of `field` in the document `id` (`field`).
    pub fn get(&self, id: &Bson, field: &str) -> Option<&'static str> {
        self.field(id, field).map(|t| t.label)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn clear(&mut self) {
        self.by_id.clear();
    }
}

/// The type to write `field` of document `id` as: the one it had when loaded (null says
/// nothing), else the column's.
fn field_hint<'a>(
    loaded: &LoadedTypes,
    id: &Bson,
    columns: &'a [ColumnMeta],
    field: &str,
) -> Option<&'a str> {
    match loaded.get(id, field) {
        Some(t) if t != "null" => Some(t),
        _ => hint_for(columns, field),
    }
}

/// The `_id` a row key names.
pub fn key_id(key: &RowKey, columns: &[ColumnMeta]) -> Result<Bson, AppError> {
    match key {
        RowKey::Pk(map) => match map.get(ID) {
            Some(v) if !is_missing(v) => values::to_bson(v, hint_for(columns, ID)),
            _ => Err(AppError::InvalidInput(
                "a MongoDB edit needs the document's _id".into(),
            )),
        },
        _ => Err(AppError::Unsupported(
            "MongoDB documents are addressed by their _id".into(),
        )),
    }
}

/// Plans the grid's staged changes on `table` (a collection; its schema is the database). A
/// number keeps the width its field had (`loaded`), or the column's (`columns`).
pub fn plan_changes(
    table: &TableRef,
    columns: &[ColumnMeta],
    loaded: &LoadedTypes,
    changes: &[StagedChange],
) -> Result<EditPlan, AppError> {
    let coll = table.name.as_str();
    let mut ops = Vec::with_capacity(changes.len());
    for (i, change) in changes.iter().enumerate() {
        let command = match change {
            StagedChange::Update {
                key,
                changes,
                exprs,
                ..
            } => {
                let id = key_id(key, columns)?;
                update_command(coll, &id, changes, exprs.as_ref(), loaded, columns)?
            }
            StagedChange::Insert { values, exprs, .. } => {
                if exprs.as_ref().is_some_and(|e| !e.is_empty()) {
                    return Err(AppError::Unsupported(
                        "MongoDB inserts take values only (no server expressions)".into(),
                    ));
                }
                insert_command(coll, new_document(values, columns)?)
            }
            StagedChange::Delete { key, .. } => delete_command(coll, key_id(key, columns)?),
        };
        ops.push(WriteOp {
            change: i,
            row_id: change.row_id().to_string(),
            op: change.op_name(),
            command,
            expect_n: 1,
        });
    }
    Ok(EditPlan {
        db: table.schema.clone(),
        ops,
    })
}

fn update_command(
    coll: &str,
    id: &Bson,
    changes: &BTreeMap<String, Value>,
    exprs: Option<&BTreeMap<String, ServerExpr>>,
    loaded: &LoadedTypes,
    columns: &[ColumnMeta],
) -> Result<Document, AppError> {
    let mut set = Document::new();
    let mut unset = Document::new();
    let mut current = Document::new();
    let exprs_map = exprs.cloned().unwrap_or_default();
    for name in changes.keys().chain(exprs_map.keys()) {
        if name == ID {
            return Err(AppError::InvalidInput(
                "the _id of a document cannot be changed (insert a copy and delete the original)"
                    .into(),
            ));
        }
        if !is_plain_field(name) {
            return Err(AppError::Unsupported(format!(
                "the field \u{201c}{name}\u{201d} cannot be set by name (a `.` or a leading `$` \
                 names a path or an operator); edit the whole document instead"
            )));
        }
    }
    for (name, expr) in &exprs_map {
        let time_of_day = matches!(expr.clock(), Some((Clock::CurrentTime, _)));
        if *expr == ServerExpr::Default || time_of_day {
            return Err(AppError::Unsupported(format!(
                "{} is not available on MongoDB",
                expr.wire()
            )));
        }
        // `$currentDate`: the server's clock, as a date.
        current.insert(name.clone(), doc! { "$type": "date" });
    }
    for (name, v) in changes {
        if exprs_map.contains_key(name) {
            continue;
        }
        let hint = field_hint(loaded, id, columns, name);
        match cell_bson(name, v, loaded.field(id, name), hint)? {
            Some(b) => set.insert(name.clone(), b),
            None => unset.insert(name.clone(), ""),
        };
    }
    let mut u = Document::new();
    if !set.is_empty() {
        u.insert("$set", set);
    }
    if !unset.is_empty() {
        u.insert("$unset", unset);
    }
    if !current.is_empty() {
        u.insert("$currentDate", current);
    }
    if u.is_empty() {
        return Err(AppError::InvalidInput("the edit changes no field".into()));
    }
    Ok(doc! {
        "update": coll,
        "updates": [{ "q": { ID: id.clone() }, "u": u, "multi": false }],
        "ordered": true,
    })
}

/// What a staged cell writes to `field`; `None` unsets it (an empty missing cell). `loaded` is
/// the type the field had in the loaded document, `hint` the type to write it as (the loaded
/// one, else the column's).
///
/// The shared cell editor stages a value by its column's kind, not by the cell's own type, so in
/// a column that mixes types an edit can come as another type than the field's: an edited
/// ObjectId or date as text (TablePlus #1183), an Int32 as a float or a decimal, a string as a
/// number, a date, a boolean or JSON. A field keeps the type it was loaded with: the value is
/// written in that type when it is exactly a value of it (7.0 over an Int32 is the Int32 7, 7
/// over a string the string "7"), and refused otherwise (7.5 or 3000000000 over an Int32, a
/// number over an ObjectId), never written as another type without a word. A binary keeps its
/// subtype as well (a legacy subtype-3 UUID is not a subtype-4 one): raw bytes typed over it
/// take it, a binary of another subtype is refused. NULL is the one value every field takes; a
/// type change is a whole-document edit.
///
/// Text typed into an `Other` cell keeps that cell's type: into a missing field it comes as
/// `missing` with text, into a null or a string as `other`, and is read as the field's type
/// (or the column's, for a field the document did not have).
fn cell_bson(
    field: &str,
    v: &Value,
    loaded: Option<FieldType>,
    hint: Option<&str>,
) -> Result<Option<Bson>, AppError> {
    let label = loaded.map(|t| t.label);
    let staged = match v {
        Value::Other(o) if o.ty == values::TY_MISSING && o.text.trim().is_empty() => {
            return Ok(None);
        }
        Value::Other(o) if o.ty == values::TY_MISSING || o.ty == values::TY_UNKNOWN => {
            typed_text(field, &o.text, hint)
        }
        Value::Text(s) => match label {
            Some(ty) if ty != "string" && ty != "null" => typed_text(field, s, Some(ty)),
            _ => values::to_bson(v, hint),
        },
        _ => values::to_bson(v, hint),
    };
    // A null field (or one the document did not have) has no type to keep.
    let Some(ty) = label.filter(|t| *t != "null") else {
        return staged.map(Some);
    };
    let b = match staged {
        Ok(b) if b == Bson::Null || values::is_type(&b, ty) => b,
        staged => {
            // Another type, or not a value as staged (a date column's text over a string): the
            // same value read as the field's type.
            let text = values::value_text(v);
            let read = text
                .as_deref()
                .and_then(|t| values::text_as(t, ty))
                .filter(|b| values::is_type(b, ty));
            match (read, staged, text) {
                (Some(b), ..) => b,
                // A binary or a cut preview: why it cannot be written at all.
                (None, Err(e), None) => return Err(e),
                (None, _, text) => return Err(not_of_type(field, text.as_deref(), ty)),
            }
        }
    };
    match (b, loaded.and_then(|t| t.subtype)) {
        (Bson::Binary(bin), Some(want)) if bin.subtype != want => match v {
            Value::Bytes(_) => Ok(Some(Bson::Binary(Binary {
                subtype: want,
                bytes: bin.bytes,
            }))),
            _ => Err(not_of_type(
                field,
                values::value_text(v).as_deref(),
                &format!("binData subtype {}", u8::from(want)),
            )),
        },
        (b, _) => Ok(Some(b)),
    }
}

/// `text` as a value of type `ty`, or any value in shell syntax when the type is unknown.
fn typed_text(field: &str, text: &str, ty: Option<&str>) -> Result<Bson, AppError> {
    let Some(ty) = ty else {
        return literal::parse_value(text).map_err(|e| e.into_app(text));
    };
    values::text_as(text, ty).ok_or_else(|| not_of_type(field, Some(text), ty))
}

fn not_of_type(field: &str, text: Option<&str>, ty: &str) -> AppError {
    let shown = match text.map(str::trim) {
        Some(t) => {
            let mut shown: String = t.chars().take(60).collect();
            if shown.len() < t.len() {
                shown.push('…');
            }
            format!("\u{201c}{shown}\u{201d}")
        }
        None => "the value".to_string(),
    };
    AppError::InvalidInput(format!(
        "{shown} is not of type {ty}, the type of the field \u{201c}{field}\u{201d}; \
         edit the whole document to give the field another type"
    ))
}

/// A new document from the grid's values: `_id` first (a new ObjectId when none is given), then
/// the fields in column order; empty missing ones are left out.
pub fn new_document(
    values_map: &BTreeMap<String, Value>,
    columns: &[ColumnMeta],
) -> Result<Document, AppError> {
    let mut d = Document::new();
    let id = match values_map.get(ID) {
        Some(v) if !v.is_null() => cell_bson(ID, v, None, hint_for(columns, ID))?,
        _ => None,
    };
    d.insert(ID, id.unwrap_or_else(|| Bson::ObjectId(ObjectId::new())));
    let mut names: Vec<&String> = values_map.keys().filter(|k| *k != ID).collect();
    let ordinal = |n: &str| {
        columns
            .iter()
            .find(|c| c.name == n)
            .map_or(u32::MAX, |c| c.ordinal)
    };
    names.sort_by_key(|n| ordinal(n));
    for name in names {
        if let Some(b) = cell_bson(name, &values_map[name], None, hint_for(columns, name))? {
            d.insert(name.clone(), b);
        }
    }
    Ok(d)
}

pub fn insert_command(coll: &str, document: Document) -> Document {
    doc! { "insert": coll, "documents": [document], "ordered": true }
}

pub fn delete_command(coll: &str, id: Bson) -> Document {
    doc! {
        "delete": coll,
        "deletes": [{ "q": { ID: id }, "limit": 1 }],
        "ordered": true,
    }
}

/// Replaces a whole document (the document editor), only while it is still exactly `original`:
/// a change made by someone else since it was loaded, a field added or removed included, matches
/// nothing (`n: 0`). The comparison is `$$ROOT` against the original as a `$literal`, where
/// every field means itself; as a plain query, field names with a `.` or a leading `$` would read
/// as paths and operators, regex values as patterns, and `$`-keyed sub-documents as operators.
pub fn replace_command(coll: &str, original: &Document, replacement: Document) -> Document {
    let mut q = Document::new();
    if let Some(id) = original.get(ID) {
        // The `_id` index finds the document; `$eq` takes the value as it is.
        q.insert(ID, doc! { "$eq": id.clone() });
    }
    q.insert(
        "$expr",
        doc! { "$eq": ["$$ROOT", { "$literal": original.clone() }] },
    );
    doc! {
        "update": coll,
        "updates": [{ "q": q, "u": replacement, "multi": false }],
        "ordered": true,
    }
}

// ---------------------------------------------------------------------------------------------
// Collections and indexes
// ---------------------------------------------------------------------------------------------

/// `create` with the options a listCollections entry reports (capped, timeseries, validator,
/// collation, clusteredIndex…).
pub fn create_collection(name: &str, options: &Document) -> Document {
    let mut d = doc! { "create": name };
    d.extend(options.clone());
    d
}

pub fn create_view(name: &str, view_on: &str, pipeline: Vec<Bson>) -> Document {
    doc! { "create": name, "viewOn": view_on, "pipeline": pipeline }
}

pub fn drop_collection(name: &str) -> Document {
    doc! { "drop": name }
}

/// Run on `admin`, with full namespaces.
pub fn rename_collection(db: &str, from: &str, to: &str, drop_target: bool) -> Document {
    doc! {
        "renameCollection": format!("{db}.{from}"),
        "to": format!("{db}.{to}"),
        "dropTarget": drop_target,
    }
}

/// Removes every document but keeps the collection, its indexes and options (TablePlus's
/// Truncate).
pub fn truncate(coll: &str) -> Document {
    doc! { "delete": coll, "deletes": [{ "q": {}, "limit": 0 }] }
}

/// The server's default name of an index on `keys` (`a_1_b_-1`, `title_text`).
pub fn index_name(keys: &Document) -> String {
    let mut parts = Vec::new();
    for (k, v) in keys {
        let v = match v {
            Bson::Int32(n) => n.to_string(),
            Bson::Int64(n) => n.to_string(),
            Bson::Double(f) if f.fract() == 0.0 => format!("{}", *f as i64),
            Bson::Double(f) => f.to_string(),
            Bson::String(s) => s.clone(),
            other => bson_json(other, JsonMode::Relaxed),
        };
        parts.push(format!("{k}_{v}"));
    }
    parts.join("_")
}

/// `createIndexes` for one index; `options` may name it, else it gets the default name.
pub fn create_index(coll: &str, keys: &Document, options: &Document) -> Document {
    let mut spec = doc! { "key": keys.clone() };
    if !options.contains_key("name") {
        spec.insert("name", index_name(keys));
    }
    spec.extend(options.clone());
    doc! { "createIndexes": coll, "indexes": [spec] }
}

pub fn drop_index(coll: &str, name: &str) -> Document {
    doc! { "dropIndexes": coll, "index": name }
}

/// Sets (or, with an empty document, removes) a collection's validator.
pub fn set_validator(coll: &str, validator: &Document) -> Document {
    doc! { "collMod": coll, "validator": validator.clone() }
}

/// `db.createCollection("name", {options})`, the definition a collection's DDL shows.
pub fn create_collection_text(name: &str, options: &Document) -> String {
    if options.is_empty() {
        format!("db.createCollection({})", json_string(name))
    } else {
        format!(
            "db.createCollection({}, {})",
            json_string(name),
            doc_json(options, JsonMode::Relaxed)
        )
    }
}

/// `db.createView("name", "source", [pipeline])`.
pub fn create_view_text(name: &str, view_on: &str, pipeline: &[Bson]) -> String {
    format!(
        "db.createView({}, {}, {})",
        json_string(name),
        json_string(view_on),
        bson_json(&Bson::Array(pipeline.to_vec()), JsonMode::Relaxed)
    )
}

/// `db.getCollection("c").createIndex({keys}, {options})` from a listIndexes entry.
pub fn create_index_text(coll: &str, spec: &Document) -> String {
    let keys = match spec.get("key") {
        Some(Bson::Document(k)) => k.clone(),
        _ => Document::new(),
    };
    let mut options = Document::new();
    for (k, v) in spec {
        if k != "key" && k != "v" && k != "ns" {
            options.insert(k.clone(), v.clone());
        }
    }
    format!(
        "{}.createIndex({}, {})",
        collection_text(coll),
        doc_json(&keys, JsonMode::Relaxed),
        doc_json(&options, JsonMode::Relaxed)
    )
}

// ---------------------------------------------------------------------------------------------
// Console commands (the mongosh subset read by `drivers::mongodb::shell`)
// ---------------------------------------------------------------------------------------------

/// How the console reads the reply of a command it ran.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    /// The documents of the reply's cursor (`cursor.firstBatch`, then `getMore`).
    Cursor,
    /// The first document of the cursor, or null (`findOne`, `stats()`).
    First,
    /// The reply document itself.
    Reply,
    /// `n` of a `count` reply.
    Count,
    /// `n` of the first document of a `countDocuments` aggregation, 0 when there is none.
    CountDocuments,
    /// `values` of a `distinct` reply.
    Distinct,
    /// `value` of a `findAndModify` reply.
    Modified,
    /// `n` of an `insert` reply, with the `_id`s the documents were given.
    Inserted(Vec<Bson>),
    /// `n`, `nModified` and `upserted` of an `update` reply.
    Updated,
    /// `n` of a `delete` reply.
    Deleted,
    /// The `name`s of the documents of the cursor (`getCollectionNames`, `show collections`).
    Names,
    /// One field of the reply (`db.version()` → `version`, `show dbs` → `databases`).
    Field(&'static str),
    /// A value the console knows once the command succeeded (`createIndex` → the index name).
    Value(Bson),
}

/// A command the console runs on database `db`, and how its reply is shown.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleCommand {
    pub db: String,
    pub command: Document,
    pub output: Output,
}

impl ConsoleCommand {
    pub fn new(db: &str, command: Document, output: Output) -> ConsoleCommand {
        ConsoleCommand {
            db: db.to_string(),
            command,
            output,
        }
    }

    /// The command as the console would read it back (what the history and errors show).
    pub fn text(&self) -> String {
        command_text(Some(&self.db), &self.command)
    }
}

/// `{name: target, …fields}`: the command's name first, as the server reads it.
pub fn command_with(name: &str, target: impl Into<Bson>, fields: Document) -> Document {
    let mut d = Document::new();
    d.insert(name, target.into());
    for (k, v) in fields {
        if k != name {
            d.insert(k, v);
        }
    }
    d
}

/// `find` with a cursor's options (`sort`, `skip`, `limit`, `projection`, `hint`…).
pub fn find_with(collection: &str, filter: Document, options: Document) -> Document {
    let mut fields = doc! { "filter": filter };
    fields.extend(options);
    command_with("find", collection, fields)
}

/// `findOne`: the first document only, and no cursor left open.
pub fn find_one(collection: &str, filter: Document, options: Document) -> Document {
    let mut d = find_with(collection, filter, options);
    d.insert("limit", 1_i64);
    d.insert("singleBatch", true);
    d
}

/// `count` with a query (cursor.count(), the deprecated `count()`).
pub fn count_with(collection: &str, query: Document, options: Document) -> Document {
    let mut fields = Document::new();
    if !query.is_empty() {
        fields.insert("query", query);
    }
    fields.extend(options);
    command_with("count", collection, fields)
}

/// `countDocuments` as the drivers run it: `$match`, the skip and limit, then `$group`.
pub fn count_documents_with(
    collection: &str,
    filter: Document,
    skip: Option<i64>,
    limit: Option<i64>,
    options: Document,
) -> Document {
    let mut pipeline = vec![Bson::Document(doc! { "$match": filter })];
    if let Some(n) = skip.filter(|n| *n > 0) {
        pipeline.push(Bson::Document(doc! { "$skip": n }));
    }
    if let Some(n) = limit.filter(|n| *n > 0) {
        pipeline.push(Bson::Document(doc! { "$limit": n }));
    }
    pipeline.push(Bson::Document(
        doc! { "$group": { "_id": 1, "n": { "$sum": 1 } } },
    ));
    aggregate_with(Bson::String(collection.into()), pipeline, options)
}

pub fn distinct_with(collection: &str, key: &str, query: Document, options: Document) -> Document {
    let mut fields = doc! { "key": key };
    if !query.is_empty() {
        fields.insert("query", query);
    }
    fields.extend(options);
    command_with("distinct", collection, fields)
}

/// `aggregate` on a collection, or on the database (`target` 1: `$currentOp`, `$documents`).
/// `batchSize` goes into the cursor; with `explain: true` there is no cursor.
pub fn aggregate_with(target: Bson, pipeline: Vec<Bson>, mut options: Document) -> Document {
    let mut fields = doc! { "pipeline": pipeline };
    let batch = options.remove("batchSize");
    if options.get("explain") != Some(&Bson::Boolean(true)) {
        let mut cursor = match options.remove("cursor") {
            Some(Bson::Document(c)) => c,
            _ => Document::new(),
        };
        if let Some(n) = batch {
            cursor.insert("batchSize", n);
        }
        fields.insert("cursor", cursor);
    }
    fields.extend(options);
    command_with("aggregate", target, fields)
}

/// A document with `_id` first: its own, or a new ObjectId as the shell and the drivers add.
pub fn with_id(document: Document) -> (Document, Bson) {
    let id = document
        .get(ID)
        .cloned()
        .unwrap_or_else(|| Bson::ObjectId(ObjectId::new()));
    let mut d = doc! { ID: id.clone() };
    for (k, v) in document {
        if k != ID {
            d.insert(k, v);
        }
    }
    (d, id)
}

/// `insert` of several documents, each given an `_id` first; also the ids.
pub fn insert_many(
    collection: &str,
    documents: Vec<Document>,
    ordered: bool,
    options: Document,
) -> (Document, Vec<Bson>) {
    let (docs, ids): (Vec<Bson>, Vec<Bson>) = documents
        .into_iter()
        .map(|d| {
            let (d, id) = with_id(d);
            (Bson::Document(d), id)
        })
        .unzip();
    let mut fields = doc! { "documents": docs, "ordered": ordered };
    fields.extend(options);
    (command_with("insert", collection, fields), ids)
}

/// One statement of an `update` command. `options` carry the statement's own fields (`upsert`,
/// `arrayFilters`, `collation`, `hint`).
pub fn update_statement(q: Document, u: Bson, multi: bool, options: Document) -> Document {
    let mut d = doc! { "q": q, "u": u, "multi": multi };
    d.extend(options);
    d
}

/// One statement of a `delete` command: `limit` 1 deletes one document, 0 every match.
pub fn delete_statement(q: Document, limit: i32, options: Document) -> Document {
    let mut d = doc! { "q": q, "limit": limit };
    d.extend(options);
    d
}

/// `update` or `delete` with its statements (`updates` / `deletes`).
pub fn write_with(
    name: &str,
    collection: &str,
    statements: Vec<Document>,
    ordered: bool,
    options: Document,
) -> Document {
    let key = if name == "update" {
        "updates"
    } else {
        "deletes"
    };
    let mut fields = Document::new();
    fields.insert(key, statements);
    fields.insert("ordered", ordered);
    fields.extend(options);
    command_with(name, collection, fields)
}

/// `createIndexes`, each spec named by its keys unless it has a name.
pub fn create_indexes(collection: &str, specs: Vec<Document>, options: Document) -> Document {
    let specs: Vec<Bson> = specs
        .into_iter()
        .map(|mut spec| {
            if !spec.contains_key("name")
                && let Ok(keys) = spec.get_document("key")
            {
                let name = index_name(keys);
                spec.insert("name", name);
            }
            Bson::Document(spec)
        })
        .collect();
    let mut fields = doc! { "indexes": specs };
    fields.extend(options);
    command_with("createIndexes", collection, fields)
}

/// `explain` of a command at a verbosity (`queryPlanner`, `executionStats`, `allPlansExecution`).
pub fn explain(command: Document, verbosity: &str) -> Document {
    doc! { "explain": command, "verbosity": verbosity }
}

/// Whether `u` is an update document (every key an operator), as `updateOne` requires.
pub fn is_update_document(u: &Document) -> bool {
    !u.is_empty() && u.keys().all(|k| k.starts_with('$'))
}

/// Whether `u` is a replacement (no key an operator), as `replaceOne` requires.
pub fn is_replacement(u: &Document) -> bool {
    !u.keys().any(|k| k.starts_with('$'))
}

#[cfg(test)]
mod tests {
    use ::mongodb::bson::{Binary, DateTime, Decimal128, Timestamp, spec::BinarySubtype};

    use super::*;
    use crate::types::schema::ValueKind;

    fn rt(b: &Bson) {
        for mode in [JsonMode::Relaxed, JsonMode::Canonical] {
            let text = bson_json(b, mode);
            let back = literal::parse_value(&text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
            match (b, &back) {
                (Bson::Double(x), Bson::Double(y)) if x.is_nan() => assert!(y.is_nan()),
                _ => assert_eq!(&back, b, "{mode:?}: {text}"),
            }
        }
    }

    #[test]
    fn extended_json_round_trips_every_type() {
        let oid = ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap();
        let values = [
            Bson::Double(1.0),
            Bson::Double(-0.0),
            Bson::Double(0.1),
            Bson::Double(1e300),
            Bson::Double(5e-324),
            Bson::Double(f64::NAN),
            Bson::Double(f64::INFINITY),
            Bson::String("quote \" backslash \\ newline \n tab \t nul \0 é 😀 \u{2028}".into()),
            Bson::Boolean(false),
            Bson::Null,
            Bson::Int32(i32::MIN),
            Bson::Int64(0),
            Bson::Int64(i64::from(i32::MAX) + 1),
            Bson::Int64(9_007_199_254_740_991),
            Bson::Int64(9_007_199_254_740_992),
            Bson::Int64(i64::MIN),
            Bson::Decimal128("123.4500".parse::<Decimal128>().unwrap()),
            Bson::DateTime(DateTime::from_millis(0)),
            Bson::DateTime(DateTime::from_millis(-1)),
            Bson::DateTime(DateTime::from_millis(i64::MAX)),
            Bson::ObjectId(oid),
            Bson::Binary(Binary {
                subtype: BinarySubtype::Generic,
                bytes: vec![1, 2, 3],
            }),
            Bson::Binary(Binary {
                subtype: BinarySubtype::from(0x80),
                bytes: vec![],
            }),
            Bson::RegularExpression(Regex {
                pattern: "a\"b".into(),
                options: "ix".into(),
            }),
            Bson::Timestamp(Timestamp {
                time: u32::MAX,
                increment: 0,
            }),
            Bson::JavaScriptCode("x".into()),
            Bson::Symbol("y".into()),
            Bson::Undefined,
            Bson::MinKey,
            Bson::MaxKey,
            Bson::Document(doc! { "z": 1, "a": { "b": [1, { "c": null }] }, "": "empty key" }),
            Bson::Array(vec![]),
            Bson::Document(Document::new()),
        ];
        for b in &values {
            rt(b);
        }
    }

    #[test]
    fn relaxed_numbers_keep_their_type() {
        assert_eq!(bson_json(&Bson::Int32(5), JsonMode::Relaxed), "5");
        assert_eq!(
            bson_json(&Bson::Int64(5), JsonMode::Relaxed),
            "{\"$numberLong\": \"5\"}"
        );
        assert_eq!(
            bson_json(&Bson::Int64(1 << 40), JsonMode::Relaxed),
            "1099511627776"
        );
        assert_eq!(
            bson_json(&Bson::Int64((1 << 53) + 1), JsonMode::Relaxed),
            "{\"$numberLong\": \"9007199254740993\"}"
        );
        assert_eq!(bson_json(&Bson::Double(1.0), JsonMode::Relaxed), "1.0");
        assert_eq!(bson_json(&Bson::Double(1e21), JsonMode::Relaxed), "1e21");
        assert_eq!(
            bson_json(&Bson::Double(1847.27), JsonMode::Relaxed),
            "1847.27"
        );
        assert_eq!(
            bson_json(&Bson::Double(1.0), JsonMode::Canonical),
            "{\"$numberDouble\": \"1.0\"}"
        );
        assert_eq!(
            bson_json(&Bson::Int32(5), JsonMode::Canonical),
            "{\"$numberInt\": \"5\"}"
        );
    }

    #[test]
    fn relaxed_dates_are_iso_from_1970() {
        assert_eq!(
            bson_json(&Bson::DateTime(DateTime::from_millis(1)), JsonMode::Relaxed),
            "{\"$date\": \"1970-01-01T00:00:00.001Z\"}"
        );
        assert_eq!(
            bson_json(
                &Bson::DateTime(DateTime::from_millis(-1)),
                JsonMode::Relaxed
            ),
            "{\"$date\": {\"$numberLong\": \"-1\"}}"
        );
        assert_eq!(
            bson_json(
                &Bson::DateTime(DateTime::from_millis(1)),
                JsonMode::Canonical
            ),
            "{\"$date\": {\"$numberLong\": \"1\"}}"
        );
    }

    #[test]
    fn strings_escape_control_characters() {
        assert_eq!(json_string("a\u{1}b\u{2028}"), "\"a\\u0001b\\u2028\"");
        let parsed: serde_json::Value =
            serde_json::from_str(&doc_json(&doc! { "k\"": "v\n\u{7f}" }, JsonMode::Relaxed))
                .unwrap();
        assert_eq!(parsed["k\""], "v\n\u{7f}");
    }

    #[test]
    fn command_text_reads_back_as_the_same_command() {
        let cmd = doc! { "update": "users", "updates": [{ "q": { "_id": ObjectId::new() }, "u": { "$set": { "n": 5_i64 } }, "multi": false }] };
        let text = command_text(None, &cmd);
        assert!(
            text.starts_with("db.runCommand({\"update\": \"users\""),
            "{text}"
        );
        let inner = text
            .strip_prefix("db.runCommand(")
            .unwrap()
            .strip_suffix(')')
            .unwrap();
        assert_eq!(literal::parse_document(inner).unwrap(), cmd);
        assert_eq!(
            command_text(Some("shop"), &doc! { "ping": 1 }),
            "db.getSiblingDB(\"shop\").runCommand({\"ping\": 1})"
        );
    }

    #[test]
    fn find_builds_pages_read_to_their_end() {
        let spec = FindSpec {
            filter: doc! { "a": 1 },
            sort: sort_document(&[SortSpec {
                column: "b".into(),
                dir: SortDir::Desc,
                nulls: None,
            }]),
            projection: projection(Some(&["b".to_string()])),
            skip: 100,
            limit: 50,
            max_time_ms: Some(3000),
            comment: Some(op_comment("x")),
        };
        assert_eq!(
            find("c", &spec),
            doc! {
                "find": "c", "filter": { "a": 1 }, "sort": { "b": -1, "_id": 1 },
                "projection": { "_id": 1, "b": 1 }, "skip": 100_i64, "limit": 50_i64,
                "batchSize": 50_i64, "maxTimeMS": 3000_i64, "comment": "tabler-x",
            }
        );
        assert_eq!(
            find("c", &FindSpec::default()),
            doc! { "find": "c", "filter": {} }
        );

        // A batch cut at 16 MiB: the rest of the page through getMore, then the cursor freed.
        let mut page = PageRead::new("c", &spec);
        let mut first = vec![(); 20];
        assert_eq!(
            page.take(&mut first, 77),
            PageStep::More(doc! { "getMore": 77_i64, "collection": "c", "batchSize": 30_i64 })
        );
        let mut next = vec![(); 30];
        assert_eq!(
            page.take(&mut next, 77),
            PageStep::Close(doc! { "killCursors": "c", "cursors": [77_i64] })
        );
        assert_eq!(page.rows(), 50);
        // The server closed it: done, and never more rows than the page holds.
        let mut page = PageRead::new("c", &spec);
        let mut all = vec![(); 60];
        assert_eq!(page.take(&mut all, 0), PageStep::Done);
        assert_eq!((all.len(), page.rows()), (50, 50));
        // An empty batch on an open cursor stops instead of asking again forever.
        let mut page = PageRead::new("c", &spec);
        assert_eq!(
            page.take(&mut Vec::<()>::new(), 9),
            PageStep::Close(doc! { "killCursors": "c", "cursors": [9_i64] })
        );
        // Without a limit, every batch to the cursor's end.
        let mut page = PageRead::new("c", &FindSpec::default());
        assert_eq!(
            page.take(&mut vec![(); 101], 5),
            PageStep::More(doc! { "getMore": 5_i64, "collection": "c" })
        );
        assert_eq!(page.take(&mut vec![(); 3], 0), PageStep::Done);
        assert_eq!(page.rows(), 104);
        assert!(sort_document(&[]).is_empty());
        assert_eq!(
            sort_document(&[SortSpec {
                column: "_id".into(),
                dir: SortDir::Desc,
                nulls: None
            }]),
            doc! { "_id": -1 }
        );
    }

    fn col(name: &str, data_type: &str) -> ColumnMeta {
        ColumnMeta {
            name: name.into(),
            ordinal: 0,
            data_type: data_type.into(),
            kind: ValueKind::Text,
            nullable: true,
            default_expr: None,
            is_pk: name == ID,
            pk_ordinal: None,
            is_auto_increment: false,
            is_generated: false,
            is_unsigned: false,
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

    fn cond(column: &str, op: FilterOp, values: Vec<Value>) -> FilterNode {
        FilterNode::Condition {
            column: column.into(),
            op,
            values,
        }
    }

    fn filter(items: Vec<FilterNode>, columns: &[ColumnMeta]) -> Document {
        filter_document(
            Some(&FilterGroup {
                conjunction: Conjunction::And,
                items,
            }),
            columns,
        )
        .unwrap()
    }

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn equality_matches_every_type_the_column_holds() {
        let cols = [
            col("n", "int|string"),
            col("s", "string"),
            col(ID, "objectId|string"),
        ];
        assert_eq!(
            filter(vec![cond("n", FilterOp::Eq, vec![Value::Int(5)])], &cols),
            doc! { "n": { "$in": [5, "5"] } }
        );
        assert_eq!(
            filter(vec![cond("s", FilterOp::Eq, vec![text("x")])], &cols),
            doc! { "s": "x" }
        );
        let oid = ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap();
        assert_eq!(
            filter(
                vec![cond(
                    ID,
                    FilterOp::Eq,
                    vec![text("65f1a2b3c4d5e6f708192a3b")]
                )],
                &cols
            ),
            doc! { "_id": { "$in": [oid, "65f1a2b3c4d5e6f708192a3b"] } }
        );
        // A text that is no ObjectId only matches strings.
        assert_eq!(
            filter(vec![cond(ID, FilterOp::Eq, vec![text("abc")])], &cols),
            doc! { "_id": "abc" }
        );
        // An unknown field takes the value as typed.
        assert_eq!(
            filter(vec![cond("other", FilterOp::Eq, vec![text("x")])], &cols),
            doc! { "other": "x" }
        );
        assert_eq!(
            filter(vec![cond("n", FilterOp::Ne, vec![Value::Int(5)])], &cols),
            doc! { "n": { "$nin": [5, "5"] } }
        );
    }

    #[test]
    fn typed_columns_parse_text_values() {
        let cols = [
            col("d", "date"),
            col("b", "bool"),
            col("x", "decimal|double"),
            col("u", "uuid"),
        ];
        assert_eq!(
            filter(
                vec![cond("d", FilterOp::Ge, vec![text("2024-05-01")])],
                &cols
            ),
            doc! { "d": { "$gte": DateTime::from_millis(1_714_521_600_000) } }
        );
        assert_eq!(
            filter(vec![cond("b", FilterOp::Eq, vec![text("TRUE")])], &cols),
            doc! { "b": true }
        );
        assert_eq!(
            filter(vec![cond("x", FilterOp::Gt, vec![text("0.1")])], &cols),
            doc! { "$or": [
                { "x": { "$gt": Decimal128::from_str_lossless("0.1") } },
                { "x": { "$gt": 0.1 } },
            ] }
        );
        let f = filter(
            vec![cond(
                "u",
                FilterOp::Eq,
                vec![text("0f8fad5b-d9cb-469f-a165-70867728950e")],
            )],
            &cols,
        );
        assert!(matches!(f.get("u"), Some(Bson::Binary(b)) if b.subtype == BinarySubtype::Uuid));
    }

    trait Lossless {
        fn from_str_lossless(s: &str) -> Decimal128;
    }
    impl Lossless for Decimal128 {
        fn from_str_lossless(s: &str) -> Decimal128 {
            s.parse().unwrap()
        }
    }

    #[test]
    fn ranges_lists_and_nulls() {
        let cols = [col("n", "int")];
        assert_eq!(
            filter(
                vec![cond(
                    "n",
                    FilterOp::Between,
                    vec![Value::Int(1), Value::Int(9)]
                )],
                &cols
            ),
            doc! { "n": { "$gte": 1, "$lte": 9 } }
        );
        assert_eq!(
            filter(
                vec![cond(
                    "n",
                    FilterOp::NotBetween,
                    vec![Value::Int(1), Value::Int(9)]
                )],
                &cols
            ),
            doc! { "$nor": [{ "n": { "$gte": 1, "$lte": 9 } }] }
        );
        assert_eq!(
            filter(
                vec![cond(
                    "n",
                    FilterOp::In,
                    vec![Value::Int(1), Value::Int(2), Value::Int(1)]
                )],
                &cols
            ),
            doc! { "n": { "$in": [1, 2] } }
        );
        assert_eq!(
            filter(vec![cond("n", FilterOp::NotIn, vec![Value::Int(1)])], &cols),
            doc! { "n": { "$nin": [1] } }
        );
        assert_eq!(
            filter(vec![cond("n", FilterOp::IsNull, vec![])], &cols),
            doc! { "n": null }
        );
        assert_eq!(
            filter(vec![cond("n", FilterOp::IsNotNull, vec![])], &cols),
            doc! { "n": { "$ne": null } }
        );
        assert!(
            filter_document(
                Some(&FilterGroup {
                    conjunction: Conjunction::And,
                    items: vec![cond("n", FilterOp::Eq, vec![])]
                }),
                &cols
            )
            .is_err()
        );
    }

    #[test]
    fn text_operators_are_escaped_regexes() {
        let cols = [col("s", "string")];
        let re = |p: &str, o: &str| {
            Bson::RegularExpression(Regex {
                pattern: p.into(),
                options: o.into(),
            })
        };
        assert_eq!(
            filter(
                vec![cond("s", FilterOp::Contains, vec![text("a.b*(c)")])],
                &cols
            ),
            doc! { "s": re("a\\.b\\*\\(c\\)", "s") }
        );
        assert_eq!(
            filter(
                vec![cond("s", FilterOp::NotContainsCase, vec![text("x")])],
                &cols
            ),
            doc! { "s": { "$not": re("x", "is") } }
        );
        assert_eq!(
            filter(
                vec![cond("s", FilterOp::HasPrefix, vec![text("$1")])],
                &cols
            ),
            doc! { "s": re("^\\$1", "s") }
        );
        assert_eq!(
            filter(
                vec![cond("s", FilterOp::HasSuffixCase, vec![text("?")])],
                &cols
            ),
            doc! { "s": re("\\?$", "is") }
        );
        assert_eq!(
            filter(
                vec![cond("s", FilterOp::Like, vec![text("a%b_c\\%")])],
                &cols
            ),
            doc! { "s": re("^a.*b.c%$", "s") }
        );
        assert_eq!(
            filter(vec![cond("s", FilterOp::Ilike, vec![text("A%")])], &cols),
            doc! { "s": re("^A.*$", "is") }
        );
        assert_eq!(
            filter(vec![cond("s", FilterOp::NotLike, vec![text("%x")])], &cols),
            doc! { "s": { "$not": re("^.*x$", "s") } }
        );
        assert!(
            filter_document(
                Some(&FilterGroup {
                    conjunction: Conjunction::And,
                    items: vec![cond("s", FilterOp::JsonContains, vec![text("{}")])]
                }),
                &cols
            )
            .is_err()
        );
    }

    #[test]
    fn groups_merge_and_nest() {
        let cols = [col("a", "int"), col("b", "int")];
        assert_eq!(
            filter(
                vec![
                    cond("a", FilterOp::Eq, vec![Value::Int(1)]),
                    cond("b", FilterOp::Gt, vec![Value::Int(2)]),
                ],
                &cols
            ),
            doc! { "a": 1, "b": { "$gt": 2 } }
        );
        assert_eq!(
            filter(
                vec![
                    cond("a", FilterOp::Gt, vec![Value::Int(1)]),
                    cond("a", FilterOp::Lt, vec![Value::Int(5)]),
                ],
                &cols
            ),
            doc! { "$and": [{ "a": { "$gt": 1 } }, { "a": { "$lt": 5 } }] }
        );
        let nested = FilterNode::Group(FilterGroup {
            conjunction: Conjunction::Or,
            items: vec![
                cond("a", FilterOp::Eq, vec![Value::Int(1)]),
                cond("b", FilterOp::Eq, vec![Value::Int(2)]),
            ],
        });
        assert_eq!(
            filter(vec![nested], &cols),
            doc! { "$or": [{ "a": 1 }, { "b": 2 }] }
        );
        assert!(filter(vec![], &cols).is_empty());
        assert!(filter_document(None, &cols).unwrap().is_empty());
    }

    #[test]
    fn any_column_ors_and_negations_and() {
        let cols = [col("a", "string"), col("b", "string")];
        let re = Bson::RegularExpression(Regex {
            pattern: "x".into(),
            options: "s".into(),
        });
        assert_eq!(
            filter(
                vec![FilterNode::AnyColumn {
                    op: FilterOp::Contains,
                    values: vec![text("x")]
                }],
                &cols
            ),
            doc! { "$or": [{ "a": re.clone() }, { "b": re.clone() }] }
        );
        assert_eq!(
            filter(
                vec![FilterNode::AnyColumn {
                    op: FilterOp::NotContains,
                    values: vec![text("x")]
                }],
                &cols
            ),
            doc! { "$and": [{ "a": { "$not": re.clone() } }, { "b": { "$not": re } }] }
        );
    }

    #[test]
    fn raw_filters_take_documents_and_fragments() {
        let oid = ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap();
        assert_eq!(
            raw_filter("\"name\": {\"$regex\": \"x\", \"$options\": \"i\"}").unwrap(),
            doc! { "name": { "$regex": "x", "$options": "i" } }
        );
        assert_eq!(
            raw_filter("{_id: ObjectId('65f1a2b3c4d5e6f708192a3b'), n: {$gt: 5}}").unwrap(),
            doc! { "_id": oid, "n": { "$gt": 5 } }
        );
        assert!(raw_filter("  ").unwrap().is_empty());
        let e = raw_filter("a: ").unwrap_err().to_string();
        assert!(e.starts_with("raw filter:"), "{e}");
    }

    fn pk(v: Value) -> RowKey {
        RowKey::Pk(BTreeMap::from([(ID.to_string(), v)]))
    }

    #[test]
    fn edits_plan_commands_by_id_keeping_types() {
        let cols = [
            col(ID, "objectId"),
            col("n", "long"),
            col("s", "string"),
            col("f", "double"),
        ];
        let oid = ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap();
        let id = Value::other(values::TY_OBJECT_ID, oid.to_hex());
        let changes = vec![
            StagedChange::Update {
                row_id: "r1".into(),
                key: pk(id.clone()),
                changes: BTreeMap::from([
                    ("n".to_string(), Value::Int(7)),
                    ("s".to_string(), values::missing()),
                    ("f".to_string(), Value::Int(2)),
                ]),
                exprs: None,
            },
            StagedChange::Delete {
                row_id: "r2".into(),
                key: pk(Value::Text("plain".into())),
            },
        ];
        let table = TableRef::new(Some("shop"), "orders");
        let plan = plan_changes(&table, &cols, &LoadedTypes::default(), &changes).unwrap();
        assert_eq!(plan.ops.len(), 2);
        assert_eq!(
            plan.ops[0].command,
            doc! {
                "update": "orders",
                "updates": [{
                    "q": { "_id": oid },
                    "u": { "$set": { "f": 2.0, "n": 7_i64 }, "$unset": { "s": "" } },
                    "multi": false,
                }],
                "ordered": true,
            }
        );
        assert_eq!(plan.ops[0].expect_n, 1);
        assert_eq!(plan.ops[0].op, "update");
        assert_eq!(
            plan.ops[1].command,
            doc! { "delete": "orders", "deletes": [{ "q": { "_id": "plain" }, "limit": 1 }], "ordered": true }
        );
        let preview = plan.preview(Some("shop"));
        assert_eq!(preview[0].0, 0);
        assert!(
            preview[0]
                .1
                .starts_with("db.runCommand({\"update\": \"orders\"")
        );
        assert!(
            preview[0]
                .1
                .contains("{\"_id\": {\"$oid\": \"65f1a2b3c4d5e6f708192a3b\"}}")
        );
        assert!(preview[0].1.contains("\"n\": {\"$numberLong\": \"7\"}"));
        assert!(
            plan.preview(Some("other"))[1]
                .1
                .starts_with("db.getSiblingDB(\"shop\")")
        );
    }

    #[test]
    fn inserts_get_an_object_id_first_and_fields_in_column_order() {
        let mut a = col("a", "int");
        a.ordinal = 2;
        let mut z = col("z", "string");
        z.ordinal = 1;
        let cols = [col(ID, "objectId"), a, z];
        let change = StagedChange::Insert {
            row_id: "n1".into(),
            values: BTreeMap::from([
                ("a".to_string(), Value::Int(1)),
                ("z".to_string(), Value::Text("t".into())),
                ("gone".to_string(), values::missing()),
            ]),
            exprs: None,
        };
        let plan = plan_changes(
            &TableRef::new(None, "c"),
            &cols,
            &LoadedTypes::default(),
            &[change],
        )
        .unwrap();
        let Some(Bson::Array(docs)) = plan.ops[0].command.get("documents") else {
            panic!()
        };
        let Bson::Document(d) = &docs[0] else {
            panic!()
        };
        let keys: Vec<&String> = d.keys().collect();
        assert_eq!(keys, ["_id", "z", "a"]);
        assert!(matches!(d.get(ID), Some(Bson::ObjectId(_))));
        // The preview names the same generated id as the command.
        let preview = &plan.preview(None)[0].1;
        let oid = d.get_object_id(ID).unwrap().to_hex();
        assert!(preview.contains(&oid));

        let given = StagedChange::Insert {
            row_id: "n2".into(),
            values: BTreeMap::from([(ID.to_string(), Value::Text("mine".into()))]),
            exprs: None,
        };
        let plan = plan_changes(
            &TableRef::new(None, "c"),
            &cols,
            &LoadedTypes::default(),
            &[given],
        )
        .unwrap();
        assert_eq!(
            plan.ops[0].command,
            doc! { "insert": "c", "documents": [{ "_id": "mine" }], "ordered": true }
        );
    }

    #[test]
    fn typed_cells_keep_the_field_type() {
        let mixed = "string|objectId|date|long|binData|null";
        let cols = [
            col(ID, "int"),
            col("owner", "objectId|null"),
            col("f", "double"),
            col("oid_s", mixed),
            col("date_s", mixed),
            col("n_s", mixed),
            col("bin_s", mixed),
            col("str_s", mixed),
            col("nul_s", mixed),
            col("nul_o", "objectId|null"),
        ];
        let oid = ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap();
        let hex = oid.to_hex();
        let mut loaded = LoadedTypes::default();
        loaded.record(&doc! {
            "_id": 1, "oid_s": oid, "date_s": DateTime::from_millis(0), "n_s": 5_i64,
            "bin_s": Binary { subtype: BinarySubtype::Generic, bytes: vec![1] },
            "str_s": "x", "nul_s": null, "nul_o": null,
        });
        let table = TableRef::new(None, "c");
        let upd = |changes: Vec<(&str, Value)>| -> Result<Document, AppError> {
            let plan = plan_changes(
                &table,
                &cols,
                &loaded,
                &[StagedChange::Update {
                    row_id: "r".into(),
                    key: pk(Value::Int(1)),
                    changes: changes
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect(),
                    exprs: None,
                }],
            )?;
            let Some(Bson::Document(st)) =
                plan.ops[0].command.get_array("updates").unwrap().first()
            else {
                panic!()
            };
            Ok(st.get_document("u").unwrap().clone())
        };
        let text = |s: &str| Value::Text(s.into());

        // Text edits of a mixed text column keep each cell's own type.
        assert_eq!(
            upd(vec![
                ("oid_s", text(&hex)),
                ("date_s", text("2024-05-01T10:00:00.000Z")),
                ("n_s", text("12")),
                ("str_s", text(&hex)),
                ("nul_s", text(&hex)),
            ])
            .unwrap(),
            doc! { "$set": {
                "date_s": DateTime::from_millis(1_714_557_600_000),
                "n_s": 12_i64,
                "nul_s": hex.clone(),
                "oid_s": oid,
                "str_s": hex.clone(),
            } }
        );
        let err = upd(vec![("oid_s", text("not an id"))]).unwrap_err();
        assert!(
            matches!(&err, AppError::InvalidInput(m) if m.contains("objectId") && m.contains("oid_s")),
            "{err:?}"
        );
        assert!(upd(vec![("bin_s", text("AAEC"))]).is_err());
        assert!(upd(vec![("n_s", text("1.5"))]).is_err());

        // Text typed into a missing cell (the editor keeps `missing`) is the column's type; an
        // empty missing cell unsets the field.
        let typed = |ty: &str, s: &str| Value::other(ty, s);
        assert_eq!(
            upd(vec![("owner", typed(values::TY_MISSING, &hex))]).unwrap(),
            doc! { "$set": { "owner": oid } }
        );
        assert_eq!(
            upd(vec![("owner", typed(values::TY_MISSING, " "))]).unwrap(),
            doc! { "$unset": { "owner": "" } }
        );
        assert!(upd(vec![("owner", typed(values::TY_MISSING, "nope"))]).is_err());
        // Typed over a null of an ObjectId column (the editor stages `other`).
        assert_eq!(
            upd(vec![("nul_o", typed(values::TY_UNKNOWN, &hex))]).unwrap(),
            doc! { "$set": { "nul_o": oid } }
        );
        // A float the editor could not hold.
        assert_eq!(
            upd(vec![("f", typed("float", "-inf"))]).unwrap(),
            doc! { "$set": { "f": f64::NEG_INFINITY } }
        );

        // Inserts: a typed missing `_id` and field are written, empty missing ones left out.
        let plan = plan_changes(
            &table,
            &cols,
            &loaded,
            &[StagedChange::Insert {
                row_id: "n".into(),
                values: BTreeMap::from([
                    (ID.to_string(), typed(values::TY_MISSING, "7")),
                    ("owner".to_string(), typed(values::TY_MISSING, &hex)),
                    ("f".to_string(), values::missing()),
                ]),
                exprs: None,
            }],
        )
        .unwrap();
        assert_eq!(
            plan.ops[0].command,
            doc! { "insert": "c", "documents": [{ "_id": 7, "owner": oid }], "ordered": true }
        );
    }

    /// The editor stages a cell by its column's dominant kind: in a column that mixes types the
    /// value can come as another type than the field's, and must not change it.
    #[test]
    fn staged_values_keep_the_loaded_type_or_are_refused() {
        let oid = ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap();
        let u = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let uuid = Binary {
            subtype: BinarySubtype::Uuid,
            bytes: uuid::Uuid::parse_str(u).unwrap().as_bytes().to_vec(),
        };
        let cols = [
            col(ID, "int"),
            col("di", "double|int"),
            col("dl", "double|long"),
            col("ci", "decimal|int"),
            col("cl", "decimal|long"),
            col("si", "string|int"),
            col("is", "int|string"),
            col("io", "int|objectId"),
            col("ds", "date|string"),
            col("os", "object|string"),
            col("oo", "object"),
            col("uu", "uuid"),
            col("bb", "binData"),
            col("b3", "binData"),
            col("dd", "double"),
            col("cc", "decimal"),
            col("dn", "double|int|null"),
            col("sc", "string|decimal"),
            col("tc", "date|decimal"),
        ];
        let mut loaded = LoadedTypes::default();
        loaded.record(&doc! {
            "_id": 1, "di": 5, "dl": 5_i64, "ci": 5, "cl": 5_i64, "si": 5, "is": "x",
            "io": oid, "ds": "x", "os": "x", "oo": { "a": 1 }, "uu": uuid.clone(),
            "bb": Binary { subtype: BinarySubtype::Generic, bytes: vec![1] },
            "b3": Binary { subtype: BinarySubtype::UuidOld, bytes: vec![2; 16] },
            "dd": 1.5, "cc": "0".parse::<Decimal128>().unwrap(), "dn": null,
            "sc": "12.50".parse::<Decimal128>().unwrap(), "tc": "12.50".parse::<Decimal128>().unwrap(),
        });
        let table = TableRef::new(None, "c");
        let set = |field: &str, v: Value| -> Result<Bson, AppError> {
            let plan = plan_changes(
                &table,
                &cols,
                &loaded,
                &[StagedChange::Update {
                    row_id: "r".into(),
                    key: pk(Value::Int(1)),
                    changes: BTreeMap::from([(field.to_string(), v)]),
                    exprs: None,
                }],
            )?;
            let u = plan.ops[0].command.get_array("updates").unwrap()[0]
                .as_document()
                .unwrap()
                .get_document("u")
                .unwrap()
                .clone();
            Ok(u.get_document("$set").unwrap().get(field).unwrap().clone())
        };
        let refused = |field: &str, v: Value, ty: &str| {
            let err = set(field, v.clone()).unwrap_err();
            assert!(
                matches!(&err, AppError::InvalidInput(m) if m.contains(&format!("type {ty}")) && m.contains(field)),
                "{field} {v:?}: {err:?}"
            );
        };
        let dec = |s: &str| Value::Decimal(s.into());

        // A whole number keeps the width its field had.
        assert_eq!(set("di", Value::Float(7.0)).unwrap(), Bson::Int32(7));
        assert_eq!(set("dl", Value::Float(7.0)).unwrap(), Bson::Int64(7));
        assert_eq!(set("ci", dec("7")).unwrap(), Bson::Int32(7));
        assert_eq!(set("ci", dec("-7.00")).unwrap(), Bson::Int32(-7));
        assert_eq!(set("cl", dec("7")).unwrap(), Bson::Int64(7));
        assert_eq!(set("di", Value::Int(7)).unwrap(), Bson::Int32(7));
        // Anything else would change it.
        refused("di", Value::Float(7.5), "int");
        refused("dl", Value::Float(1e20), "long");
        refused("ci", dec("7.5"), "int");
        refused("di", Value::Int(3_000_000_000), "int");
        refused("di", Value::BigInt("9007199254740993".into()), "int");
        refused("si", Value::Text("3000000000".into()), "int");
        refused("di", Value::other("float", "nan"), "int");
        // A number over a double or a decimal is one of them.
        assert_eq!(set("dd", Value::Int(2)).unwrap(), Bson::Double(2.0));
        assert_eq!(
            set("cc", Value::Float(2.5)).unwrap(),
            Bson::Decimal128("2.5".parse().unwrap())
        );
        // A cleared cell over a decimal is not 0: what the shared editor stages for an empty
        // text, date or other cell is refused.
        assert_eq!(
            set("sc", Value::Text(" 12.5 ".into())).unwrap(),
            Bson::Decimal128("12.5".parse().unwrap())
        );
        for t in ["", " ", ".", "-", "e5"] {
            refused("sc", Value::Text(t.into()), "decimal");
            refused("tc", Value::TimestampTz(t.into()), "decimal");
            refused("cc", Value::other("other", t), "decimal");
            refused("cc", dec(t), "decimal");
        }

        // A string stays a string: the value as its text.
        let s = |v: &str| Bson::String(v.into());
        assert_eq!(set("is", Value::Int(7)).unwrap(), s("7"));
        assert_eq!(
            set("ds", Value::TimestampTz("2024-01-01T00:00:00Z".into())).unwrap(),
            s("2024-01-01T00:00:00Z")
        );
        assert_eq!(
            set("ds", Value::TimestampTz("later".into())).unwrap(),
            s("later")
        );
        assert_eq!(set("ds", Value::Bool(true)).unwrap(), s("true"));
        assert_eq!(set("ds", Value::Uuid(u.into())).unwrap(), s(u));
        assert_eq!(
            set("os", Value::Json("{\"a\":1}".into())).unwrap(),
            s("{\"a\":1}")
        );
        // Nothing else turns into an ObjectId, an object or a UUID.
        refused("io", Value::Int(7), "objectId");
        refused("io", Value::Bool(true), "objectId");
        refused("oo", Value::Json("5".into()), "object");
        refused("oo", Value::Json("[1]".into()), "object");
        refused("oo", Value::Json("{\"$date\": 5}".into()), "object");
        refused("uu", Value::from_bytes(&[1]), "uuid");
        assert_eq!(
            set("oo", Value::Json("{\"b\": 2}".into())).unwrap(),
            Bson::Document(doc! { "b": 2 })
        );
        // A cut preview is not written even when its text would read as the type.
        let cut = set("oo", Value::other(values::TY_PREVIEW, "{\"b\": 2}")).unwrap_err();
        assert!(
            matches!(&cut, AppError::InvalidInput(m) if m.contains("cut for display")),
            "{cut:?}"
        );
        // A binary keeps its subtype: hex typed over one (the editor stages bytes) takes it, a
        // UUID (subtype 4) over a generic or a legacy subtype-3 binary is refused.
        let bytes = |b: &[u8]| Value::from_bytes(b);
        assert_eq!(
            set("b3", bytes(&[9, 9])).unwrap(),
            Bson::Binary(Binary {
                subtype: BinarySubtype::UuidOld,
                bytes: vec![9, 9]
            })
        );
        assert_eq!(
            set("bb", bytes(&[9])).unwrap(),
            Bson::Binary(Binary {
                subtype: BinarySubtype::Generic,
                bytes: vec![9]
            })
        );
        assert_eq!(
            set("b3", Value::other("binData", "BinData(3, 'AQ==')")).unwrap(),
            Bson::Binary(Binary {
                subtype: BinarySubtype::UuidOld,
                bytes: vec![1]
            })
        );
        refused("bb", Value::Uuid(u.into()), "binData subtype 0");
        refused(
            "b3",
            Value::Text(format!("UUID('{u}')")),
            "binData subtype 3",
        );
        assert_eq!(
            set("uu", Value::Uuid(u.into())).unwrap(),
            Bson::Binary(uuid)
        );
        // NULL is a value of every field, and a null field takes the column's type.
        assert_eq!(set("io", Value::Null).unwrap(), Bson::Null);
        assert_eq!(set("dn", Value::Float(7.0)).unwrap(), Bson::Double(7.0));

        // An insert has no loaded types: the column's.
        let plan = plan_changes(
            &table,
            &cols,
            &loaded,
            &[StagedChange::Insert {
                row_id: "n".into(),
                values: BTreeMap::from([
                    (ID.to_string(), Value::Int(2)),
                    ("di".to_string(), Value::Float(7.0)),
                    ("is".to_string(), Value::Int(7)),
                ]),
                exprs: None,
            }],
        )
        .unwrap();
        assert_eq!(
            plan.ops[0].command,
            doc! { "insert": "c", "documents": [{ "_id": 2, "di": 7.0, "is": 7 }], "ordered": true }
        );
    }

    #[test]
    fn replacements_match_the_whole_original_literally() {
        let original = doc! {
            "_id": 1, "a.b": 1, "$x": { "$gt": 5 },
            "re": Regex { pattern: "^a".into(), options: "i".into() },
        };
        assert_eq!(
            replace_command("c", &original, doc! { "_id": 1, "n": 2 }),
            doc! {
                "update": "c",
                "updates": [{
                    "q": {
                        "_id": { "$eq": 1 },
                        "$expr": { "$eq": ["$$ROOT", { "$literal": original.clone() }] },
                    },
                    "u": { "_id": 1, "n": 2 },
                    "multi": false,
                }],
                "ordered": true,
            }
        );
    }

    #[test]
    fn edits_refuse_what_mongodb_cannot_do() {
        let cols = [col(ID, "int")];
        let table = TableRef::new(None, "c");
        let upd = |changes: BTreeMap<String, Value>,
                   exprs: Option<BTreeMap<String, ServerExpr>>| {
            plan_changes(
                &table,
                &cols,
                &LoadedTypes::default(),
                &[StagedChange::Update {
                    row_id: "r".into(),
                    key: pk(Value::Int(1)),
                    changes,
                    exprs,
                }],
            )
        };
        assert!(upd(BTreeMap::from([(ID.to_string(), Value::Int(2))]), None).is_err());
        assert!(upd(BTreeMap::new(), None).is_err());
        assert!(
            upd(
                BTreeMap::new(),
                Some(BTreeMap::from([("d".to_string(), ServerExpr::Default)]))
            )
            .is_err()
        );
        let now = upd(
            BTreeMap::new(),
            Some(BTreeMap::from([("d".to_string(), ServerExpr::Now)])),
        )
        .unwrap();
        assert_eq!(
            now.ops[0].command.get_array("updates").unwrap()[0],
            Bson::Document(
                doc! { "q": { "_id": 1 }, "u": { "$currentDate": { "d": { "$type": "date" } } }, "multi": false }
            )
        );
        let no_id = plan_changes(
            &table,
            &cols,
            &LoadedTypes::default(),
            &[StagedChange::Delete {
                row_id: "r".into(),
                key: RowKey::Rowid(1),
            }],
        );
        assert!(no_id.is_err());
        let all_cols = plan_changes(
            &table,
            &cols,
            &LoadedTypes::default(),
            &[StagedChange::Delete {
                row_id: "r".into(),
                key: RowKey::AllColumns(BTreeMap::new()),
            }],
        );
        assert!(all_cols.is_err());
        let with_expr = plan_changes(
            &table,
            &cols,
            &LoadedTypes::default(),
            &[StagedChange::Insert {
                row_id: "r".into(),
                values: BTreeMap::new(),
                exprs: Some(BTreeMap::from([("d".to_string(), ServerExpr::Now)])),
            }],
        );
        assert!(with_expr.is_err());
    }

    #[test]
    fn definitions_and_index_names() {
        assert_eq!(index_name(&doc! { "a": 1, "b": -1 }), "a_1_b_-1");
        assert_eq!(index_name(&doc! { "title": "text" }), "title_text");
        assert_eq!(index_name(&doc! { "x": 1.0 }), "x_1");
        assert_eq!(
            create_index("c", &doc! { "a": 1 }, &doc! { "unique": true }),
            doc! { "createIndexes": "c", "indexes": [{ "key": { "a": 1 }, "name": "a_1", "unique": true }] }
        );
        assert_eq!(
            create_index("c", &doc! { "a": 1 }, &doc! { "name": "mine" }),
            doc! { "createIndexes": "c", "indexes": [{ "key": { "a": 1 }, "name": "mine" }] }
        );
        assert_eq!(
            create_collection_text("logs", &Document::new()),
            "db.createCollection(\"logs\")"
        );
        assert_eq!(
            create_collection_text("logs", &doc! { "capped": true, "size": 4096 }),
            "db.createCollection(\"logs\", {\"capped\": true, \"size\": 4096})"
        );
        assert_eq!(
            create_view_text(
                "v",
                "orders",
                &[Bson::Document(doc! { "$match": { "a": 1 } })]
            ),
            "db.createView(\"v\", \"orders\", [{\"$match\": {\"a\": 1}}])"
        );
        assert_eq!(
            create_index_text(
                "stats",
                &doc! { "v": 2, "key": { "a": 1 }, "name": "a_1", "unique": true }
            ),
            "db.getCollection(\"stats\").createIndex({\"a\": 1}, {\"name\": \"a_1\", \"unique\": true})"
        );
        assert_eq!(
            rename_collection("db1", "a", "b", false),
            doc! { "renameCollection": "db1.a", "to": "db1.b", "dropTarget": false }
        );
        assert_eq!(
            truncate("c"),
            doc! { "delete": "c", "deletes": [{ "q": {}, "limit": 0 }] }
        );
        assert_eq!(
            create_collection("c", &doc! { "capped": true }),
            doc! { "create": "c", "capped": true }
        );
    }

    #[test]
    fn regex_escaping_and_like() {
        assert_eq!(escape_regex("a.b|c"), "a\\.b\\|c");
        assert_eq!(like_pattern("100\\%"), "^100%$");
        assert_eq!(like_pattern("a\\"), "^a\\\\$");
        assert_eq!(like_pattern("[x]"), "^\\[x\\]$");
        // Every escaped literal matches itself and only itself.
        for s in ["a.b", "(x)", "^$", "a+b?", "{1}", "\\d", "a|b", "-/"] {
            let re = ::regex::Regex::new(&format!("^{}$", escape_regex(s))).unwrap();
            assert!(re.is_match(s), "{s}");
        }
    }
}
