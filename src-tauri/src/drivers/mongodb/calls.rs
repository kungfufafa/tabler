//! The session's own calls (`session_call`), for what the sidebar's menus do with a collection,
//! an index or a database, and Running Queries (`$currentOp`, `killOp`).
//!
//! Reads: `collectionStats` `{database, collection}`, `databaseStats` `{database}`, and
//! `preview` `{method, args}`, the command a write call would run, as text (what the menus'
//! confirmations show). Writes, each `{database, …}`: `createCollection` (`name`, `options`
//! text), `createView` (`name`, `viewOn`, `pipeline` text), `dropCollection`,
//! `truncateCollection`, `renameCollection` (`to`, `dropTarget`), `createIndex` (`keys` and
//! `options` text), `dropIndex` (`name`), `setValidator` (`validator` text, `level`, `action`)
//! and `dropDatabase`; each answers `{command, reply}`. Texts are shell syntax (`literal`).

use ::mongodb::bson::{Bson, Document};
use serde_json::{Value as Json, json};

use super::catalog;
use super::errors;
use super::literal;
use super::session::{MongoSession, Run};
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::native::{CallKind, unknown_call};
use crate::error::AppError;
use crate::sqlgen::native::mongo;
use crate::types::admin::{KillOutcome, KillScope, ProcessRow};
use crate::types::browse::TraceCategory;
use crate::types::profile::Driver;

/// The calls that only read; every other one, and any call not listed, may change something.
const READS: [&str; 3] = ["collectionStats", "databaseStats", "preview"];

pub(super) fn call_kind(method: &str) -> CallKind {
    if READS.contains(&method) {
        CallKind::Read
    } else {
        CallKind::Write
    }
}

/// A call's arguments, with the errors a caller gets for a missing or mistyped one.
struct Args<'a>(&'a Json);

impl Args<'_> {
    fn text(&self, key: &str) -> Result<String, AppError> {
        self.0
            .get(key)
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                AppError::InvalidInput(format!("MongoDB: \u{201c}{key}\u{201d} is required"))
            })
    }

    fn optional(&self, key: &str) -> Option<String> {
        self.text(key).ok()
    }

    fn flag(&self, key: &str) -> bool {
        self.0.get(key).and_then(Json::as_bool).unwrap_or(false)
    }

    /// A document in shell syntax; an empty or missing one is `{}`.
    fn document(&self, key: &str) -> Result<Document, AppError> {
        match self.optional(key) {
            Some(text) => literal::parse_document(&text).map_err(|e| {
                let at = e.position(&text);
                AppError::InvalidInput(format!(
                    "MongoDB {key}: {} (line {}, column {})",
                    e.message, at.line, at.column
                ))
            }),
            None => Ok(Document::new()),
        }
    }

    fn one_of(&self, key: &str, allowed: &[&str]) -> Result<Option<String>, AppError> {
        match self.optional(key) {
            Some(v) if !allowed.contains(&v.as_str()) => Err(AppError::InvalidInput(format!(
                "MongoDB {key}: one of {}",
                allowed.join(", ")
            ))),
            other => Ok(other),
        }
    }
}

/// The database a write call runs on and its command; `None` for a call that is not a write.
fn write_command(method: &str, a: &Args<'_>) -> Result<Option<(String, Document)>, AppError> {
    let db = || a.text("database");
    let coll = || a.text("collection");
    Ok(Some(match method {
        "createCollection" => (
            db()?,
            mongo::create_collection(&a.text("name")?, &a.document("options")?),
        ),
        "createView" => {
            let text = a.text("pipeline")?;
            let pipeline = match literal::parse_value(&text) {
                Ok(Bson::Array(stages)) => stages,
                _ => {
                    return Err(AppError::InvalidInput(
                        "MongoDB pipeline: an array of stages ([{$match: …}, …])".into(),
                    ));
                }
            };
            (
                db()?,
                mongo::create_view(&a.text("name")?, &a.text("viewOn")?, pipeline),
            )
        }
        "dropCollection" => (db()?, mongo::drop_collection(&coll()?)),
        "truncateCollection" => (db()?, mongo::truncate(&coll()?)),
        // On `admin`, with the full names.
        "renameCollection" => (
            "admin".to_string(),
            mongo::rename_collection(&db()?, &coll()?, &a.text("to")?, a.flag("dropTarget")),
        ),
        "createIndex" => {
            let keys = a.document("keys")?;
            if keys.is_empty() {
                return Err(AppError::InvalidInput(
                    "MongoDB index: its keys are required ({field: 1})".into(),
                ));
            }
            (
                db()?,
                mongo::create_index(&coll()?, &keys, &a.document("options")?),
            )
        }
        "dropIndex" => (db()?, mongo::drop_index(&coll()?, &a.text("name")?)),
        "setValidator" => (
            db()?,
            mongo::set_validator(
                &coll()?,
                &a.document("validator")?,
                a.one_of("level", &["strict", "moderate", "off"])?
                    .as_deref(),
                a.one_of("action", &["error", "warn"])?.as_deref(),
            ),
        ),
        "dropDatabase" => (db()?, mongo::drop_database()),
        _ => return Ok(None),
    }))
}

/// A reply as relaxed Extended JSON.
fn json_of(d: Document) -> Json {
    Bson::Document(d).into_relaxed_extjson()
}

pub(super) async fn call(s: &MongoSession, method: &str, args: Json) -> Result<Json, AppError> {
    let a = Args(&args);
    match method {
        "collectionStats" => {
            let (db, coll) = (a.text("database")?, a.text("collection")?);
            let stats = s
                .read_docs(TraceCategory::Structure, &db, mongo::coll_stats(&coll), 1)
                .await?;
            Ok(stats.into_iter().next().map_or(Json::Null, json_of))
        }
        "databaseStats" => {
            let db = a.text("database")?;
            let reply = s
                .traced(TraceCategory::Structure, &db, mongo::db_stats(), Run::Read)
                .await?;
            Ok(json_of(reply))
        }
        "preview" => {
            let inner = a.text("method")?;
            let inner_args = args.get("args").cloned().unwrap_or(Json::Null);
            match write_command(&inner, &Args(&inner_args))? {
                Some((db, command)) => Ok(json!({
                    "database": db,
                    "command": mongo::command_text(Some(&db), &command),
                })),
                None => Err(unknown_call(Driver::Mongodb, &inner)),
            }
        }
        _ => {
            let Some((db, command)) = write_command(method, &a)? else {
                return Err(unknown_call(Driver::Mongodb, method));
            };
            if s.read_only() {
                return Err(AppError::InvalidInput(READ_ONLY_MESSAGE.into()));
            }
            let text = mongo::command_text(Some(&db), &command);
            let reply = s
                .traced(TraceCategory::Structure, &db, command, Run::Write)
                .await?;
            if let Some(e) = errors::from_reply(&reply) {
                return Err(e);
            }
            Ok(json!({ "command": text, "reply": json_of(reply) }))
        }
    }
}

/// The operations running now: every user's where the user may see them (`inprog`), else their
/// own. Listed on the primary, which `killOp` reaches.
pub(super) async fn list_processes(s: &MongoSession) -> Result<Vec<ProcessRow>, AppError> {
    let listing = mongo::op_comment(&uuid::Uuid::new_v4().simple().to_string());
    let read = |all: bool| {
        s.read_docs_on(
            TraceCategory::Structure,
            "admin",
            mongo::current_ops(all, &listing),
            0,
            Run::Write,
        )
    };
    let ops = match read(true).await {
        Err(AppError::Db {
            vendor_code: Some(13),
            ..
        }) => read(false).await?,
        other => other?,
    };
    Ok(catalog::processes(&ops, &listing, s.app_name()))
}

/// `killOp` for each id as the list gave it (a number, or `shard:number` through mongos).
pub(super) async fn kill_processes(
    s: &MongoSession,
    ids: &[String],
    scope: KillScope,
) -> Vec<KillOutcome> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let refused = if s.read_only() {
            Some(READ_ONLY_MESSAGE.to_string())
        } else if scope == KillScope::Connection {
            Some("MongoDB stops operations, not connections: use Kill Query".to_string())
        } else {
            None
        };
        if let Some(message) = refused {
            out.push(KillOutcome {
                id: id.clone(),
                ok: false,
                message: Some(message),
            });
            continue;
        }
        let op = match id.trim().parse::<i64>() {
            Ok(n) => i32::try_from(n).map_or(Bson::Int64(n), Bson::Int32),
            Err(_) => Bson::String(id.trim().to_string()),
        };
        let result = s
            .traced(
                TraceCategory::Structure,
                "admin",
                mongo::kill_op(op),
                Run::Write,
            )
            .await;
        out.push(KillOutcome {
            id: id.clone(),
            ok: result.is_ok(),
            message: result.err().map(|e| e.to_string()),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use ::mongodb::bson::doc;

    use super::*;

    fn command(method: &str, args: Json) -> Result<Option<(String, Document)>, AppError> {
        write_command(method, &Args(&args))
    }

    #[test]
    fn calls_read_or_write_by_name_and_unknown_ones_write() {
        for m in READS {
            assert_eq!(call_kind(m), CallKind::Read, "{m}");
        }
        for m in ["dropCollection", "createIndex", "frobnicate", "", "Preview"] {
            assert_eq!(call_kind(m), CallKind::Write, "{m}");
        }
    }

    #[test]
    fn write_calls_build_their_commands() {
        assert_eq!(
            command(
                "dropCollection",
                json!({ "database": "shop", "collection": "orders" })
            )
            .unwrap(),
            Some(("shop".into(), doc! { "drop": "orders" }))
        );
        assert_eq!(
            command(
                "renameCollection",
                json!({ "database": "shop", "collection": "a", "to": "b", "dropTarget": true })
            )
            .unwrap(),
            Some((
                "admin".into(),
                doc! { "renameCollection": "shop.a", "to": "shop.b", "dropTarget": true }
            ))
        );
        assert_eq!(
            command(
                "createIndex",
                json!({ "database": "d", "collection": "c", "keys": "{a: 1, b: -1}", "options": "{unique: true}" })
            )
            .unwrap(),
            Some((
                "d".into(),
                doc! { "createIndexes": "c", "indexes": [{ "key": { "a": 1, "b": -1 }, "name": "a_1_b_-1", "unique": true }] }
            ))
        );
        assert_eq!(
            command(
                "setValidator",
                json!({ "database": "d", "collection": "c", "validator": "{$jsonSchema: {required: ['a']}}", "level": "moderate" })
            )
            .unwrap(),
            Some((
                "d".into(),
                doc! { "collMod": "c", "validator": { "$jsonSchema": { "required": ["a"] } }, "validationLevel": "moderate" }
            ))
        );
        assert_eq!(
            command(
                "createView",
                json!({ "database": "d", "name": "v", "viewOn": "c", "pipeline": "[{$match: {a: 1}}]" })
            )
            .unwrap(),
            Some((
                "d".into(),
                doc! { "create": "v", "viewOn": "c", "pipeline": [{ "$match": { "a": 1 } }] }
            ))
        );
        assert_eq!(
            command("dropDatabase", json!({ "database": "old" })).unwrap(),
            Some(("old".into(), doc! { "dropDatabase": 1 }))
        );
        assert_eq!(command("collectionStats", json!({})).unwrap(), None);
        for (method, args, msg) in [
            (
                "dropCollection",
                json!({ "database": "d" }),
                "MongoDB: \u{201c}collection\u{201d} is required",
            ),
            (
                "createIndex",
                json!({ "database": "d", "collection": "c", "keys": "{}" }),
                "MongoDB index: its keys are required ({field: 1})",
            ),
            (
                "setValidator",
                json!({ "database": "d", "collection": "c", "level": "loose" }),
                "MongoDB level: one of strict, moderate, off",
            ),
            (
                "createView",
                json!({ "database": "d", "name": "v", "viewOn": "c", "pipeline": "{}" }),
                "MongoDB pipeline: an array of stages ([{$match: …}, …])",
            ),
        ] {
            assert_eq!(
                command(method, args).unwrap_err().to_string(),
                msg,
                "{method}"
            );
        }
        let err = command(
            "createCollection",
            json!({ "database": "d", "name": "c", "options": "{capped: }" }),
        )
        .unwrap_err();
        assert!(err.to_string().starts_with("MongoDB options: "), "{err}");
    }
}
