//! Read or write: the console's classifier (read-only connections, Safe Mode). An allow-list,
//! default-deny, over the commands a statement runs rather than its text: a command is a read
//! only when its name (the document's first key, as the server reads it) is in `READS`, an
//! `aggregate` has no `$out` or `$merge` anywhere in it, and an `explain` explains such a read.
//! `use` reads; a statement the parser cannot read, and an empty script, are writes.

use ::mongodb::bson::{Bson, Document};

use super::shell::{self, Statement};
use crate::sqlgen::classify::StatementClass;

/// Commands that never change data or server state (aggregate and explain are checked apart).
pub const READS: &[&str] = &[
    "find",
    "count",
    "distinct",
    "listCollections",
    "listIndexes",
    "listDatabases",
    "collStats",
    "dbStats",
    "hello",
    "isMaster",
    "ismaster",
    "buildInfo",
    "buildinfo",
    "connectionStatus",
    "ping",
    "serverStatus",
    "hostInfo",
    "usersInfo",
    "rolesInfo",
    "listCommands",
    "currentOp",
    "replSetGetStatus",
    "replSetGetConfig",
];

/// The commands `explain` may wrap and still read.
const EXPLAINABLE_READS: &[&str] = &["find", "aggregate", "count", "distinct"];

/// Aggregation stages that write.
const WRITE_STAGES: &[&str] = &["$out", "$merge"];

/// One command document.
pub fn classify_command(command: &Document) -> StatementClass {
    let Some((name, body)) = command.iter().next() else {
        return StatementClass::Write;
    };
    let read = match name.as_str() {
        "aggregate" => !has_key(command, WRITE_STAGES, 0),
        "explain" => match body {
            Bson::Document(inner) => {
                inner
                    .keys()
                    .next()
                    .is_some_and(|n| EXPLAINABLE_READS.contains(&n.as_str()))
                    && classify_command(inner) == StatementClass::Read
            }
            _ => false,
        },
        n => READS.contains(&n),
    };
    if read {
        StatementClass::Read
    } else {
        StatementClass::Write
    }
}

/// Whether any document inside `v` (at any depth) has one of `keys`. Too deep to look through
/// counts as having one.
fn has_key(d: &Document, keys: &[&str], depth: usize) -> bool {
    if depth > 200 {
        return true;
    }
    d.iter()
        .any(|(k, v)| keys.contains(&k.as_str()) || bson_has_key(v, keys, depth + 1))
}

fn bson_has_key(v: &Bson, keys: &[&str], depth: usize) -> bool {
    match v {
        Bson::Document(d) => has_key(d, keys, depth),
        Bson::Array(items) => depth > 200 || items.iter().any(|i| bson_has_key(i, keys, depth + 1)),
        Bson::JavaScriptCodeWithScope(c) => has_key(&c.scope, keys, depth),
        _ => false,
    }
}

pub fn classify_statement(st: &Statement) -> StatementClass {
    match st {
        Statement::Use(_) => StatementClass::Read,
        // bulkWrite only writes.
        Statement::Batch { .. } => StatementClass::Write,
        Statement::Run(c) => classify_command(&c.command),
    }
}

/// One statement as the splitter returned it.
pub fn classify_text(text: &str) -> StatementClass {
    // The database name changes no command's kind.
    match shell::parse(text, "test") {
        Ok(st) => classify_statement(&st),
        Err(_) => StatementClass::Write,
    }
}

/// A whole console text: a read only when it has statements and every one reads.
pub fn classify_script(text: &str) -> StatementClass {
    let statements = shell::split(text);
    if statements.is_empty() {
        return StatementClass::Write;
    }
    if statements
        .iter()
        .all(|s| classify_text(&s.sql) == StatementClass::Read)
    {
        StatementClass::Read
    } else {
        StatementClass::Write
    }
}
