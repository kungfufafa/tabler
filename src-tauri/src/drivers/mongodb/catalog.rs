//! The replies of the catalog commands (`sqlgen::native::mongo` builds them) as Tabler's types:
//! the server (`hello`, `buildInfo`, `connectionStatus`), databases, collections, indexes,
//! storage numbers, and a collection's `TableMeta` with its definition. Every reader takes what a
//! server sends without failing: a field of an unexpected type is skipped.

use ::mongodb::bson::{Bson, Document};

use super::literal::int_of;
use crate::sqlgen::native::mongo::{
    ID, JsonMode, create_collection_text, create_index_text, create_view_text, doc_json,
};
use crate::types::admin::ProcessRow;
use crate::types::schema::{
    CheckMeta, ColumnMeta, DatabaseInfo, IndexMeta, TableKind, TableMeta, TableRef, TableSummary,
};

/// The deployment a `hello` reply describes: transactions need a replica set or mongos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Topology {
    Standalone,
    ReplicaSet { name: String, primary: bool },
    Sharded,
}

impl Topology {
    pub fn from_hello(hello: &Document) -> Topology {
        if hello.get_str("msg") == Ok("isdbgrid") {
            return Topology::Sharded;
        }
        match hello.get_str("setName") {
            Ok(name) => Topology::ReplicaSet {
                name: name.to_string(),
                primary: flag(hello, "isWritablePrimary") || flag(hello, "ismaster"),
            },
            Err(_) => Topology::Standalone,
        }
    }

    /// Whether a commit can be one transaction.
    pub fn transactions(&self) -> bool {
        !matches!(self, Topology::Standalone)
    }
}

/// A boolean the server may send as a bool or a number.
fn flag(d: &Document, key: &str) -> bool {
    match d.get(key) {
        Some(Bson::Boolean(b)) => *b,
        Some(other) => int_of(other).is_some_and(|n| n != 0),
        None => false,
    }
}

fn int(d: &Document, key: &str) -> Option<i64> {
    d.get(key).and_then(int_of)
}

/// A number the server may send as any numeric type (sizes are doubles on some versions).
fn number(d: &Document, key: &str) -> Option<i64> {
    match d.get(key) {
        Some(Bson::Double(f)) if f.is_finite() => Some(f.round() as i64),
        Some(other) => int_of(other),
        None => None,
    }
}

/// `buildInfo.version` (`8.0.4`), and the product when it is not MongoDB itself (FerretDB,
/// Amazon DocumentDB answer `buildInfo` as MongoDB but say so elsewhere).
pub fn server_version(build_info: &Document) -> String {
    let version = build_info
        .get_str("version")
        .unwrap_or("unknown")
        .to_string();
    if build_info.contains_key("ferretdb") {
        format!("{version} (FerretDB)")
    } else {
        version
    }
}

/// The first authenticated user of a `connectionStatus` reply, as `user@db`.
pub fn current_user(connection_status: &Document) -> Option<String> {
    let users = connection_status
        .get_document("authInfo")
        .ok()?
        .get_array("authenticatedUsers")
        .ok()?;
    users.iter().find_map(|u| match u {
        Bson::Document(u) => {
            let user = u.get_str("user").ok()?;
            Some(match u.get_str("db") {
                Ok(db) => format!("{user}@{db}"),
                Err(_) => user.to_string(),
            })
        }
        _ => None,
    })
}

/// The databases of a `listDatabases` reply, by name.
pub fn databases(reply: &Document, current: Option<&str>) -> Vec<DatabaseInfo> {
    let mut out: Vec<DatabaseInfo> = reply
        .get_array("databases")
        .map(|list| {
            list.iter()
                .filter_map(|d| match d {
                    Bson::Document(d) => d.get_str("name").ok(),
                    _ => None,
                })
                .map(|name| DatabaseInfo {
                    name: name.to_string(),
                    owner: None,
                    encoding: None,
                    is_current: Some(name) == current,
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// One `listCollections` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionInfo {
    pub name: String,
    /// `collection`, `view` or `timeseries`.
    pub kind: String,
    pub options: Document,
    pub read_only: bool,
}

impl CollectionInfo {
    pub fn from_entry(entry: &Document) -> Option<CollectionInfo> {
        Some(CollectionInfo {
            name: entry.get_str("name").ok()?.to_string(),
            kind: entry.get_str("type").unwrap_or("collection").to_string(),
            options: entry.get_document("options").cloned().unwrap_or_default(),
            read_only: entry
                .get_document("info")
                .map(|i| flag(i, "readOnly"))
                .unwrap_or(false),
        })
    }

    pub fn is_view(&self) -> bool {
        self.kind == "view"
    }

    /// `system.*` collections and the buckets of time series belong to the server.
    pub fn is_system(&self) -> bool {
        self.name.starts_with("system.")
    }

    pub fn table_kind(&self) -> TableKind {
        if self.is_system() {
            TableKind::SystemView
        } else if self.is_view() {
            TableKind::View
        } else {
            TableKind::Table
        }
    }

    /// What the sidebar says about it: a view's source, a time series, a capped collection.
    pub fn comment(&self) -> Option<String> {
        if self.is_view() {
            return self
                .options
                .get_str("viewOn")
                .ok()
                .map(|on| format!("view on {on}"));
        }
        if self.kind == "timeseries" {
            return Some("time series".into());
        }
        flag(&self.options, "capped").then(|| "capped".into())
    }

    /// Whether the grid may edit it: not a view, a system collection or one the server reports
    /// read-only.
    pub fn editable(&self) -> bool {
        !self.is_view() && !self.is_system() && !self.read_only
    }

    /// `db.createCollection(…)` / `db.createView(…)` with the options it was created with.
    pub fn definition(&self) -> String {
        if self.is_view() {
            let on = self.options.get_str("viewOn").unwrap_or_default();
            let pipeline = self
                .options
                .get_array("pipeline")
                .cloned()
                .unwrap_or_default();
            let text = create_view_text(&self.name, on, &pipeline);
            return match self.options.get_document("collation") {
                Ok(c) => format!(
                    "{}, {{\"collation\": {}}})",
                    text.trim_end_matches(')'),
                    doc_json(c, JsonMode::Relaxed)
                ),
                Err(_) => text,
            };
        }
        create_collection_text(&self.name, &self.options)
    }
}

/// The collections of a `listCollections` cursor as the sidebar's tables, by name.
pub fn collections(entries: &[Document], db: &str) -> Vec<TableSummary> {
    let mut out: Vec<TableSummary> = entries
        .iter()
        .filter_map(CollectionInfo::from_entry)
        .map(|c| TableSummary {
            schema: Some(db.to_string()),
            kind: c.table_kind(),
            comment: c.comment(),
            name: c.name,
            row_estimate: None,
            size_bytes: None,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The indexes of a `listIndexes` cursor; `_id_` is the primary one.
pub fn indexes(collection: &str, entries: &[Document]) -> Vec<IndexMeta> {
    entries
        .iter()
        .filter_map(|spec| {
            let name = spec.get_str("name").ok()?.to_string();
            let keys = spec.get_document("key").ok()?;
            let columns = keys.keys().cloned().collect();
            let mut methods: Vec<String> = Vec::new();
            for (k, v) in keys {
                let m = match v {
                    _ if k.ends_with("$**") => "wildcard".to_string(),
                    Bson::String(s) => s.clone(),
                    other if int_of(other).is_some_and(|n| n < 0) => "desc".to_string(),
                    _ => continue,
                };
                if !methods.contains(&m) {
                    methods.push(m);
                }
            }
            let mut notes = Vec::new();
            if let Some(ttl) = int(spec, "expireAfterSeconds") {
                notes.push(format!("TTL {ttl} s"));
            }
            for f in ["sparse", "hidden"] {
                if flag(spec, f) {
                    notes.push(f.to_string());
                }
            }
            Some(IndexMeta {
                primary: name == "_id_",
                unique: flag(spec, "unique") || name == "_id_",
                method: (!methods.is_empty()).then(|| methods.join(", ")),
                predicate: spec
                    .get_document("partialFilterExpression")
                    .ok()
                    .map(|p| doc_json(p, JsonMode::Relaxed)),
                definition: Some(create_index_text(collection, spec)),
                comment: (!notes.is_empty()).then(|| notes.join(", ")),
                include: Vec::new(),
                columns,
                name,
            })
        })
        .collect()
}

/// `count` and storage size of the first `$collStats` document.
pub fn coll_stats(stats: &Document) -> (Option<i64>, Option<i64>) {
    match stats.get_document("storageStats") {
        Ok(s) => (number(s, "count"), number(s, "size")),
        Err(_) => (number(stats, "count"), number(stats, "size")),
    }
}

/// The fields a `$jsonSchema` validator requires.
fn required_fields(validator: &Document) -> Vec<String> {
    validator
        .get_document("$jsonSchema")
        .ok()
        .and_then(|s| s.get_array("required").ok())
        .map(|r| {
            r.iter()
                .filter_map(|f| match f {
                    Bson::String(s) => Some(s.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What defines a collection beyond its documents: its create text (options, validator), then
/// each index but `_id_`.
pub fn ddl(info: &CollectionInfo, indexes: &[IndexMeta]) -> String {
    let mut ddl = format!("{};", info.definition());
    for i in indexes.iter().filter(|i| !i.primary) {
        if let Some(d) = &i.definition {
            ddl.push('\n');
            ddl.push_str(d);
            ddl.push(';');
        }
    }
    ddl
}

/// The operations of a `$currentOp` listing as Running Queries rows. `listing` is the comment of
/// the listing itself (`current`); `app` the application name Tabler's sessions give (`own`).
pub fn processes(ops: &[Document], listing: &str, app: &str) -> Vec<ProcessRow> {
    ops.iter()
        .filter_map(|op| {
            let id = match op.get("opid")? {
                Bson::String(s) => s.clone(),
                other => int_of(other)?.to_string(),
            };
            let command = op.get_document("command").ok();
            let comment = command.and_then(|c| c.get_str("comment").ok());
            let user = op
                .get_array("effectiveUsers")
                .ok()
                .and_then(|u| u.first())
                .and_then(Bson::as_document)
                .and_then(|u| {
                    let name = u.get_str("user").ok()?;
                    Some(match u.get_str("db") {
                        Ok(db) => format!("{name}@{db}"),
                        Err(_) => name.to_string(),
                    })
                });
            let ns = op.get_str("ns").ok();
            let application = op.get_str("appName").ok().map(str::to_string);
            let micros = int(op, "microsecs_running");
            Some(ProcessRow {
                id,
                user,
                database: ns
                    .map(|ns| ns.split_once('.').map_or(ns, |(db, _)| db))
                    .filter(|db| !db.is_empty())
                    .map(str::to_string),
                client: op
                    .get_str("client")
                    .or_else(|_| op.get_str("client_s"))
                    .ok()
                    .map(str::to_string),
                state: Some(if flag(op, "active") { "active" } else { "idle" }.to_string()),
                command: op.get_str("op").ok().map(str::to_string),
                started_at: op.get_str("currentOpTime").ok().map(str::to_string),
                duration_ms: micros
                    .map(|us| us / 1000)
                    .or_else(|| int(op, "secs_running").map(|s| s.saturating_mul(1000))),
                wait_event: flag(op, "waitingForLock").then(|| "waiting for a lock".to_string()),
                query: command.map(|c| {
                    let text = doc_json(c, JsonMode::Relaxed);
                    match text.char_indices().nth(QUERY_TEXT_CHARS) {
                        Some((at, _)) => format!("{}…", &text[..at]),
                        None => text,
                    }
                }),
                current: comment == Some(listing),
                own: application.as_deref() == Some(app),
                application,
            })
        })
        .collect()
}

/// How much of an operation's command the process list shows.
const QUERY_TEXT_CHARS: usize = 2000;

/// A collection's structure: the inferred `columns`, its indexes, its validator as a check, and
/// its definition (the create text, then each index but `_id_`).
pub fn table_meta(
    db: &str,
    info: &CollectionInfo,
    columns: Vec<ColumnMeta>,
    indexes: Vec<IndexMeta>,
    row_estimate: Option<i64>,
) -> TableMeta {
    let has_id = columns.iter().any(|c| c.name == ID && c.is_pk);
    let ddl = ddl(info, &indexes);
    let checks = match info.options.get_document("validator") {
        Ok(v) if !v.is_empty() => vec![CheckMeta {
            name: "validator".into(),
            expression: doc_json(v, JsonMode::Relaxed),
            columns: required_fields(v),
        }],
        _ => Vec::new(),
    };
    TableMeta {
        table: TableRef::new(Some(db), &info.name),
        kind: info.table_kind(),
        primary_key: if has_id && info.editable() {
            vec![ID.to_string()]
        } else {
            Vec::new()
        },
        columns,
        indexes,
        foreign_keys: Vec::new(),
        triggers: Vec::new(),
        comment: info.comment(),
        row_estimate,
        ddl: Some(ddl),
        checks,
        options: None,
    }
}

/// The entries of a cursor reply (`cursor.firstBatch` or `cursor.nextBatch`) and its id (0 when
/// the server closed it).
pub fn cursor_batch(reply: &Document) -> (Vec<Document>, i64) {
    let Ok(cursor) = reply.get_document("cursor") else {
        return (Vec::new(), 0);
    };
    let batch = cursor
        .get_array("firstBatch")
        .or_else(|_| cursor.get_array("nextBatch"))
        .map(|items| {
            items
                .iter()
                .filter_map(|d| match d {
                    Bson::Document(d) => Some(d.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    (batch, int(cursor, "id").unwrap_or(0))
}

/// The `n` a write reply reports (inserted, matched or deleted documents).
pub fn reply_n(reply: &Document) -> Option<i64> {
    int(reply, "n")
}

/// The count of a `count` reply, or of the `countDocuments` aggregation's first document.
pub fn count_of(reply: &Document) -> Option<i64> {
    if let Some(n) = int(reply, "n") {
        return Some(n);
    }
    let (batch, _) = cursor_batch(reply);
    match batch.first() {
        Some(d) => int(d, "n"),
        None if reply.contains_key("cursor") => Some(0),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use ::mongodb::bson::doc;

    use super::*;
    use crate::drivers::mongodb::schema;

    #[test]
    fn topology_from_hello() {
        assert_eq!(
            Topology::from_hello(&doc! { "isWritablePrimary": true, "maxWireVersion": 25 }),
            Topology::Standalone
        );
        let rs = Topology::from_hello(&doc! { "setName": "rs0", "isWritablePrimary": true });
        assert_eq!(
            rs,
            Topology::ReplicaSet {
                name: "rs0".into(),
                primary: true
            }
        );
        assert!(rs.transactions());
        assert_eq!(
            Topology::from_hello(&doc! { "msg": "isdbgrid", "ismaster": 1 }),
            Topology::Sharded
        );
        assert!(!Topology::Standalone.transactions());
    }

    #[test]
    fn server_version_and_user() {
        assert_eq!(server_version(&doc! { "version": "8.0.4" }), "8.0.4");
        assert_eq!(
            server_version(&doc! { "version": "7.0.77", "ferretdb": { "version": "v2.0.0" } }),
            "7.0.77 (FerretDB)"
        );
        assert_eq!(server_version(&doc! {}), "unknown");
        let status =
            doc! { "authInfo": { "authenticatedUsers": [{ "user": "app", "db": "admin" }] } };
        assert_eq!(current_user(&status).as_deref(), Some("app@admin"));
        assert_eq!(
            current_user(&doc! { "authInfo": { "authenticatedUsers": [] } }),
            None
        );
        assert_eq!(current_user(&doc! { "authInfo": 5 }), None);
    }

    #[test]
    fn databases_sorted_with_the_current_one() {
        let reply = doc! { "databases": [
            { "name": "shop", "sizeOnDisk": 8192.0, "empty": false },
            { "name": "admin", "sizeOnDisk": 4096_i64 },
            5,
            { "nameless": true },
        ], "ok": 1 };
        let dbs = databases(&reply, Some("shop"));
        let names: Vec<&str> = dbs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["admin", "shop"]);
        assert!(dbs[1].is_current && !dbs[0].is_current);
        assert!(databases(&doc! { "ok": 1 }, None).is_empty());
    }

    #[test]
    fn collections_views_and_system_collections() {
        let entries = vec![
            doc! { "name": "orders", "type": "collection", "options": {}, "info": { "readOnly": false } },
            doc! { "name": "big", "type": "view", "options": { "viewOn": "orders", "pipeline": [{ "$match": { "n": { "$gt": 5 } } }] } },
            doc! { "name": "system.views", "type": "collection", "options": {} },
            doc! { "name": "metrics", "type": "timeseries", "options": { "timeseries": { "timeField": "t" } } },
            doc! { "name": "logs", "options": { "capped": true, "size": 4096 } },
            doc! { "type": "collection" },
        ];
        let tables = collections(&entries, "shop");
        let rows: Vec<(&str, TableKind, Option<&str>)> = tables
            .iter()
            .map(|t| (t.name.as_str(), t.kind, t.comment.as_deref()))
            .collect();
        assert_eq!(
            rows,
            [
                ("big", TableKind::View, Some("view on orders")),
                ("logs", TableKind::Table, Some("capped")),
                ("metrics", TableKind::Table, Some("time series")),
                ("orders", TableKind::Table, None),
                ("system.views", TableKind::SystemView, None),
            ]
        );
        assert!(tables.iter().all(|t| t.schema.as_deref() == Some("shop")));
        let view = CollectionInfo::from_entry(&entries[1]).unwrap();
        assert!(!view.editable());
        assert_eq!(
            view.definition(),
            "db.createView(\"big\", \"orders\", [{\"$match\": {\"n\": {\"$gt\": 5}}}])"
        );
        let logs = CollectionInfo::from_entry(&entries[4]).unwrap();
        assert_eq!(
            logs.definition(),
            "db.createCollection(\"logs\", {\"capped\": true, \"size\": 4096})"
        );
    }

    #[test]
    fn index_specs() {
        let entries = vec![
            doc! { "v": 2, "key": { "_id": 1 }, "name": "_id_" },
            doc! { "v": 2, "key": { "a": 1, "b": -1 }, "name": "a_1_b_-1", "unique": true,
            "partialFilterExpression": { "a": { "$exists": true } } },
            doc! { "v": 2, "key": { "_fts": "text", "_ftsx": 1 }, "name": "body_text", "weights": { "body": 1 } },
            doc! { "v": 2, "key": { "created": 1 }, "name": "ttl", "expireAfterSeconds": 3600, "hidden": true },
            doc! { "v": 2, "key": { "$**": 1 }, "name": "all" },
            doc! { "v": 2, "name": "no key" },
        ];
        let idx = indexes("c", &entries);
        assert_eq!(idx.len(), 5);
        assert!(idx[0].primary && idx[0].unique && idx[0].method.is_none());
        assert_eq!(idx[1].columns, ["a", "b"]);
        assert!(idx[1].unique && !idx[1].primary);
        assert_eq!(idx[1].method.as_deref(), Some("desc"));
        assert_eq!(
            idx[1].predicate.as_deref(),
            Some("{\"a\": {\"$exists\": true}}")
        );
        assert_eq!(
            idx[1].definition.as_deref(),
            Some(
                "db.getCollection(\"c\").createIndex({\"a\": 1, \"b\": -1}, {\"name\": \"a_1_b_-1\", \"unique\": true, \"partialFilterExpression\": {\"a\": {\"$exists\": true}}})"
            )
        );
        assert_eq!(idx[2].method.as_deref(), Some("text"));
        assert_eq!(idx[3].comment.as_deref(), Some("TTL 3600 s, hidden"));
        assert_eq!(idx[4].method.as_deref(), Some("wildcard"));
    }

    #[test]
    fn storage_numbers_take_any_numeric_type() {
        let stats =
            doc! { "storageStats": { "count": 12, "size": 4096.0, "storageSize": 8192_i64 } };
        assert_eq!(coll_stats(&stats), (Some(12), Some(4096)));
        assert_eq!(coll_stats(&doc! { "count": 3_i64 }), (Some(3), None));
    }

    #[test]
    fn table_meta_of_a_collection() {
        let info = CollectionInfo::from_entry(&doc! {
            "name": "people",
            "type": "collection",
            "options": { "validator": { "$jsonSchema": { "required": ["name", 5] } } },
        })
        .unwrap();
        let sample = vec![doc! { "_id": 1, "name": "a" }];
        let cols = schema::columns(&schema::infer(&sample), 1);
        let idx = indexes(
            "people",
            &[
                doc! { "key": { "_id": 1 }, "name": "_id_" },
                doc! { "key": { "name": 1 }, "name": "name_1" },
            ],
        );
        let meta = table_meta("hr", &info, cols, idx, Some(1));
        assert_eq!(meta.table, TableRef::new(Some("hr"), "people"));
        assert_eq!(meta.primary_key, ["_id"]);
        assert_eq!(meta.checks.len(), 1);
        assert_eq!(meta.checks[0].columns, ["name"]);
        assert_eq!(
            meta.ddl.as_deref(),
            Some(
                "db.createCollection(\"people\", {\"validator\": {\"$jsonSchema\": {\"required\": [\"name\", 5]}}});\n\
                 db.getCollection(\"people\").createIndex({\"name\": 1}, {\"name\": \"name_1\"});"
            )
        );
        assert_eq!(meta.row_estimate, Some(1));
    }

    #[test]
    fn cursor_replies_and_counts() {
        let reply = doc! { "cursor": { "id": 77_i64, "ns": "a.b", "firstBatch": [{ "x": 1 }, 5] }, "ok": 1 };
        let (batch, id) = cursor_batch(&reply);
        assert_eq!((batch.len(), id), (1, 77));
        let next = doc! { "cursor": { "id": 0_i64, "nextBatch": [] }, "ok": 1 };
        assert_eq!(cursor_batch(&next), (Vec::new(), 0));
        assert_eq!(cursor_batch(&doc! { "ok": 1 }), (Vec::new(), 0));
        assert_eq!(count_of(&doc! { "n": 5, "ok": 1 }), Some(5));
        assert_eq!(
            count_of(&doc! { "cursor": { "id": 0_i64, "firstBatch": [{ "_id": 1, "n": 9 }] } }),
            Some(9)
        );
        assert_eq!(
            count_of(&doc! { "cursor": { "id": 0_i64, "firstBatch": [] } }),
            Some(0)
        );
        assert_eq!(count_of(&doc! { "ok": 1 }), None);
        assert_eq!(reply_n(&doc! { "n": 1_i64, "nModified": 1 }), Some(1));
    }
}
