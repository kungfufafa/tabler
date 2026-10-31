//! Running one console statement (`shell::parse` reads it) and showing what it returns: a
//! cursor's documents as rows whose columns are the fields of its first documents, one value or
//! one reply as a row, a write's counts as affected documents.

use std::collections::BTreeSet;

use ::mongodb::Client;
use ::mongodb::bson::{Bson, Document};

use super::catalog;
use super::classify;
use super::errors;
use super::schema;
use super::session::{Docs, MongoSession, OpTag, Run};
use super::shell::{self, Statement};
use super::values;
use crate::commands::safety::READ_ONLY_MESSAGE;
use crate::drivers::{CancelSlot, RowChunker, RowSink, StatementOutcome};
use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::native::mongo::{self, ConsoleCommand, JsonMode, Output};
use crate::types::query::{ResultColumn, StatementKind};
use crate::types::schema::{ColumnMeta, ValueKind};

/// A cursor's columns are the fields of up to this many of its first documents.
const COLUMN_SAMPLE: usize = 200;
/// The fields named in the notice about fields outside the columns.
const LATE_FIELDS_SHOWN: usize = 20;

pub(super) async fn run(
    s: &MongoSession,
    text: &str,
    max_rows: usize,
    sink: &mut dyn RowSink,
    cancel: &CancelSlot,
) -> Result<StatementOutcome, AppError> {
    let statement = shell::parse(text, &s.console_db()).map_err(|e| e.into_app(text))?;
    let class = classify::classify_statement(&statement);
    if s.read_only() && class == StatementClass::Write {
        return Err(AppError::InvalidInput(READ_ONLY_MESSAGE.into()));
    }
    match statement {
        Statement::Use(db) => {
            s.set_console_db(&db);
            Ok(StatementOutcome {
                kind: StatementKind::Empty,
                notices: vec![format!("switched to db {db}")],
                ..StatementOutcome::default()
            })
        }
        Statement::Run(command) => {
            let (client, generation) = s.live_client();
            match run_command(s, &client, &command, max_rows, sink, cancel).await {
                // Signing in happens before a command is sent: a read runs again with a fresh
                // password, a write is left for the user to run again.
                Err(AppError::Auth(m)) => {
                    if s.renew(generation).await && class == StatementClass::Read {
                        run_command(s, &s.client(), &command, max_rows, sink, cancel).await
                    } else {
                        Err(AppError::Auth(m))
                    }
                }
                other => other,
            }
        }
        Statement::Batch { commands, ordered } => {
            let (client, generation) = s.live_client();
            let outcome = run_batch(s, &client, &commands, ordered, cancel).await;
            if matches!(outcome, Err(AppError::Auth(_))) {
                s.renew(generation).await;
            }
            outcome
        }
    }
}

/// Whether a command is sent to the members a read may use, or to the primary.
fn run_kind(command: &Document) -> Run {
    match classify::classify_command(command) {
        StatementClass::Read => Run::Read,
        _ => Run::Write,
    }
}

async fn run_command(
    s: &MongoSession,
    client: &Client,
    c: &ConsoleCommand,
    max_rows: usize,
    sink: &mut dyn RowSink,
    cancel: &CancelSlot,
) -> Result<StatementOutcome, AppError> {
    let tag = OpTag::new(s.start_session(client).await?);
    let run = run_kind(&c.command);
    match &c.output {
        Output::Cursor | Output::Names => {
            let names = c.output == Output::Names;
            let read = async {
                let batch = u32::try_from(max_rows.saturating_add(1)).unwrap_or(u32::MAX);
                let mut docs = s
                    .open_cursor(client, &c.db, c.command.clone(), run, &tag, Some(batch))
                    .await?;
                if names {
                    stream_names(&mut docs, max_rows, sink).await
                } else {
                    stream_docs(&mut docs, max_rows, sink).await
                }
            };
            s.cancellable(cancel, client, &tag, read).await
        }
        output => {
            let command = mongo::with_comment(c.command.clone(), &tag.comment);
            let op = async {
                let mut session = tag.session().await;
                s.run(client, &c.db, command, run, session.as_deref_mut())
                    .await
            };
            let reply = s.cancellable(cancel, client, &tag, op).await?;
            show_reply(c, output, reply, max_rows, sink)
        }
    }
}

/// `bulkWrite`: its commands in order in one session; ordered, the first that fails stops it.
async fn run_batch(
    s: &MongoSession,
    client: &Client,
    commands: &[ConsoleCommand],
    ordered: bool,
    cancel: &CancelSlot,
) -> Result<StatementOutcome, AppError> {
    let tag = OpTag::new(s.start_session(client).await?);
    let mut affected = 0_u64;
    let mut notices = Vec::new();
    let mut first_error: Option<(usize, AppError)> = None;
    for (i, c) in commands.iter().enumerate() {
        let command = mongo::with_comment(c.command.clone(), &tag.comment);
        let op = async {
            let mut session = tag.session().await;
            s.run(client, &c.db, command, Run::Write, session.as_deref_mut())
                .await
        };
        let reply = s.cancellable(cancel, client, &tag, op).await;
        let counted = reply.and_then(|reply| write_counts(&c.output, &reply));
        match counted {
            Ok((n, notice)) => {
                // The counts are the server's: a hostile reply must not overflow the sum.
                affected = affected.saturating_add(n);
                notices.extend(notice);
            }
            Err(AppError::Cancelled) => return Err(AppError::Cancelled),
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some((i, e));
                }
                if ordered {
                    break;
                }
            }
        }
    }
    match first_error {
        None => Ok(StatementOutcome {
            kind: StatementKind::Affected,
            affected: Some(affected),
            notices,
            ..StatementOutcome::default()
        }),
        Some((at, e)) => Err(prefixed(
            e,
            &format!(
                "bulkWrite: command {} of {} failed ({affected} documents written before)",
                at + 1,
                commands.len()
            ),
        )),
    }
}

/// `e` with `what` before its message (a server error keeps its code).
fn prefixed(e: AppError, what: &str) -> AppError {
    match e {
        AppError::Db {
            message,
            detail,
            hint,
            sqlstate,
            vendor_code,
            position,
            statement_index,
        } => AppError::Db {
            message: format!("{what}: {message}"),
            detail,
            hint,
            sqlstate,
            vendor_code,
            position,
            statement_index,
        },
        other => AppError::db(format!("{what}: {other}")),
    }
}

/// The count a write's reply reports, and what to say about it; a write error is an error.
fn write_counts(output: &Output, reply: &Document) -> Result<(u64, Option<String>), AppError> {
    if let Some(e) = errors::from_reply(reply) {
        return Err(e);
    }
    let n = catalog::reply_n(reply).unwrap_or(0).max(0) as u64;
    Ok(match output {
        Output::Updated => {
            let modified = reply
                .get("nModified")
                .and_then(super::literal::int_of)
                .unwrap_or(0)
                .max(0) as u64;
            let upserted = reply.get_array("upserted").map_or(0, |u| u.len() as u64);
            let matched = n.saturating_sub(upserted);
            (
                modified.saturating_add(upserted),
                Some(format!(
                    "matched {matched}, modified {modified}, upserted {upserted}"
                )),
            )
        }
        _ => (n, None),
    })
}

fn result_column(name: &str, data_type: &str, kind: ValueKind) -> ResultColumn {
    ResultColumn {
        name: name.to_string(),
        data_type: data_type.to_string(),
        kind,
        nullable: None,
        table: None,
        origin: None,
    }
}

fn result_columns(columns: &[ColumnMeta]) -> Vec<ResultColumn> {
    columns
        .iter()
        .map(|c| ResultColumn {
            nullable: Some(c.nullable),
            ..result_column(&c.name, &c.data_type, c.kind)
        })
        .collect()
}

fn row_of(columns: &[ColumnMeta], d: &Document) -> Vec<crate::types::value::Value> {
    columns
        .iter()
        .map(|c| values::field_value(d.get(&c.name)))
        .collect()
}

fn rows_outcome(row_count: u64, truncated: bool, notices: Vec<String>) -> StatementOutcome {
    StatementOutcome {
        kind: StatementKind::Rows,
        result_sets: 1,
        row_count,
        truncated,
        notices,
        ..StatementOutcome::default()
    }
}

/// A cursor's documents, up to `max_rows`. The columns are the fields of its first documents
/// (read before any row is shown); a field that turns up only later is named in a notice.
async fn stream_docs(
    docs: &mut Docs,
    max_rows: usize,
    sink: &mut dyn RowSink,
) -> Result<StatementOutcome, AppError> {
    let mut head = Vec::new();
    let mut ended = false;
    while head.len() < max_rows.min(COLUMN_SAMPLE) {
        match docs.next().await {
            Some(d) => head.push(d?),
            None => {
                ended = true;
                break;
            }
        }
    }
    let columns = schema::columns(&schema::infer(&head), head.len());
    let mut chunker = RowChunker::new(sink);
    let mut stopped = chunker.begin_set(result_columns(&columns)).is_break();
    let mut rows = 0_u64;
    for d in &head {
        if stopped {
            break;
        }
        stopped = chunker.push(row_of(&columns, d)).is_break();
        rows += 1;
    }
    let mut late = BTreeSet::new();
    let mut truncated = false;
    while !ended && !stopped {
        let Some(d) = docs.next().await else { break };
        let d = d?;
        if rows as usize >= max_rows {
            truncated = true;
            break;
        }
        for k in d.keys() {
            if !columns.iter().any(|c| &c.name == k) {
                late.insert(k.clone());
            }
        }
        stopped = chunker.push(row_of(&columns, &d)).is_break();
        rows += 1;
    }
    if !ended && !stopped && !truncated && rows as usize >= max_rows {
        truncated = docs.next().await.is_some();
    }
    let _ = chunker.flush();
    let mut notices = Vec::new();
    if !late.is_empty() {
        let shown: Vec<String> = late.iter().take(LATE_FIELDS_SHOWN).cloned().collect();
        let more = late.len().saturating_sub(LATE_FIELDS_SHOWN);
        notices.push(format!(
            "Fields that are not in the first {} documents are not shown: {}{}",
            head.len(),
            shown.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        ));
    }
    Ok(rows_outcome(rows, truncated, notices))
}

/// The `name`s of a cursor's documents (`show collections`, `getCollectionNames()`).
async fn stream_names(
    docs: &mut Docs,
    max_rows: usize,
    sink: &mut dyn RowSink,
) -> Result<StatementOutcome, AppError> {
    let mut chunker = RowChunker::new(sink);
    let mut stopped = chunker
        .begin_set(vec![result_column("name", "string", ValueKind::Text)])
        .is_break();
    let mut rows = 0_u64;
    let mut truncated = false;
    while !stopped {
        let Some(d) = docs.next().await else { break };
        let d = d?;
        if rows as usize >= max_rows {
            truncated = true;
            break;
        }
        let name = d.get("name").map_or_else(values::missing, values::to_value);
        stopped = chunker.push(vec![name]).is_break();
        rows += 1;
    }
    let _ = chunker.flush();
    Ok(rows_outcome(rows, truncated, Vec::new()))
}

/// Documents already in hand as one result set.
fn show_documents(
    docs: &[Document],
    max_rows: usize,
    sink: &mut dyn RowSink,
    notices: Vec<String>,
) -> StatementOutcome {
    let shown = &docs[..docs.len().min(max_rows)];
    let columns = schema::columns(&schema::infer(shown), shown.len());
    let mut chunker = RowChunker::new(sink);
    let mut rows = 0_u64;
    if chunker.begin_set(result_columns(&columns)).is_continue() {
        for d in shown {
            rows += 1;
            if chunker.push(row_of(&columns, d)).is_break() {
                break;
            }
        }
    }
    let _ = chunker.flush();
    rows_outcome(rows, docs.len() > max_rows, notices)
}

/// Values in one column `name`, a row each.
fn show_values(
    name: &str,
    items: &[Bson],
    max_rows: usize,
    sink: &mut dyn RowSink,
) -> StatementOutcome {
    let label = items.first().map_or("null", values::type_label);
    let mut chunker = RowChunker::new(sink);
    let mut rows = 0_u64;
    if chunker
        .begin_set(vec![result_column(name, label, values::kind_of(label))])
        .is_continue()
    {
        for item in items.iter().take(max_rows) {
            rows += 1;
            if chunker.push(vec![values::to_value(item)]).is_break() {
                break;
            }
        }
    }
    let _ = chunker.flush();
    rows_outcome(rows, items.len() > max_rows, Vec::new())
}

/// A value of a reply: documents as rows, other values in one column.
fn show_bson(
    name: &str,
    value: Option<&Bson>,
    max_rows: usize,
    sink: &mut dyn RowSink,
) -> StatementOutcome {
    match value {
        Some(Bson::Document(d)) => show_documents(std::slice::from_ref(d), max_rows, sink, vec![]),
        Some(Bson::Array(items)) if items.iter().all(|i| matches!(i, Bson::Document(_))) => {
            let docs: Vec<Document> = items
                .iter()
                .filter_map(|i| i.as_document().cloned())
                .collect();
            show_documents(&docs, max_rows, sink, vec![])
        }
        Some(Bson::Array(items)) => show_values(name, items, max_rows, sink),
        Some(other) => show_values(name, std::slice::from_ref(other), max_rows, sink),
        None => show_values(name, &[Bson::Null], max_rows, sink),
    }
}

/// What a command's reply shows, by how the statement reads it (`Output`).
fn show_reply(
    c: &ConsoleCommand,
    output: &Output,
    reply: Document,
    max_rows: usize,
    sink: &mut dyn RowSink,
) -> Result<StatementOutcome, AppError> {
    let affected = |n: u64, notices: Vec<String>, last: Option<String>| StatementOutcome {
        kind: StatementKind::Affected,
        affected: Some(n),
        last_insert_id: last,
        notices,
        ..StatementOutcome::default()
    };
    Ok(match output {
        Output::Reply => {
            // The driver's own fields (cluster time, operation time) are not the command's.
            let shown: Document = reply
                .into_iter()
                .filter(|(k, _)| !k.starts_with('$') && k != "operationTime")
                .collect();
            show_documents(&[shown], max_rows, sink, vec![])
        }
        Output::First => {
            let (docs, _) = catalog::cursor_batch(&reply);
            show_documents(&docs[..docs.len().min(1)], max_rows, sink, vec![])
        }
        Output::Count | Output::CountDocuments => {
            let n = catalog::count_of(&reply)
                .ok_or_else(|| AppError::db("the server's count reply has no count"))?;
            show_values("count", &[Bson::Int64(n)], max_rows, sink)
        }
        Output::Distinct => {
            let key = c.command.get_str("key").unwrap_or("value");
            let items = reply.get_array("values").cloned().unwrap_or_default();
            show_values(key, &items, max_rows, sink)
        }
        Output::Modified => show_bson("value", reply.get("value"), max_rows, sink),
        Output::Inserted(ids) => {
            let (n, _) = write_counts(output, &reply)?;
            let last = match ids.as_slice() {
                [one] => Some(mongo::bson_json(one, JsonMode::Relaxed)),
                _ => None,
            };
            affected(n, Vec::new(), last)
        }
        Output::Updated | Output::Deleted => {
            let (n, notice) = write_counts(output, &reply)?;
            affected(n, notice.into_iter().collect(), None)
        }
        Output::Field(name) => show_bson(name, reply.get(*name), max_rows, sink),
        Output::Value(v) => show_values("result", std::slice::from_ref(v), max_rows, sink),
        // Read through the cursor in `run_command`.
        Output::Cursor | Output::Names => {
            let (docs, _) = catalog::cursor_batch(&reply);
            show_documents(&docs, max_rows, sink, vec![])
        }
    })
}

#[cfg(test)]
mod tests {
    use ::mongodb::bson::doc;

    use super::*;
    use crate::drivers::CollectSink;
    use crate::types::value::Value;

    fn command(text: &str) -> ConsoleCommand {
        match shell::parse(text, "hr").unwrap() {
            Statement::Run(c) => c,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn replies_show_as_rows_or_counts() {
        let mut sink = CollectSink::default();
        let c = command("db.people.distinct('name')");
        let out = show_reply(
            &c,
            &c.output,
            doc! { "values": ["Ann", "Bob"], "ok": 1.0 },
            1,
            &mut sink,
        )
        .unwrap();
        assert_eq!((out.row_count, out.truncated), (1, true));
        assert_eq!(sink.sets[0].columns[0].name, "name");
        assert_eq!(sink.sets[0].rows, [[Value::Text("Ann".into())]]);

        let mut sink = CollectSink::default();
        let c = command("db.runCommand({ping: 1})");
        show_reply(
            &c,
            &c.output,
            doc! { "ok": 1.0, "$clusterTime": { "x": 1 }, "operationTime": 5_i64 },
            10,
            &mut sink,
        )
        .unwrap();
        let names: Vec<&str> = sink.sets[0]
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["ok"]);

        let c = command("db.people.updateMany({}, {$set: {a: 1}})");
        let mut sink = CollectSink::default();
        let out = show_reply(
            &c,
            &c.output,
            doc! { "n": 3, "nModified": 2, "ok": 1.0 },
            10,
            &mut sink,
        )
        .unwrap();
        assert_eq!(out.kind, StatementKind::Affected);
        assert_eq!(out.affected, Some(2));
        assert_eq!(out.notices, ["matched 3, modified 2, upserted 0"]);
        let err = show_reply(
            &c,
            &c.output,
            doc! { "n": 0, "ok": 1.0, "writeErrors": [{ "index": 0, "code": 11000, "errmsg": "dup" }] },
            10,
            &mut sink,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            AppError::Db {
                vendor_code: Some(11000),
                ..
            }
        ));

        let c = command("db.people.insertOne({_id: 7})");
        let out = show_reply(&c, &c.output, doc! { "n": 1, "ok": 1.0 }, 10, &mut sink).unwrap();
        assert_eq!(
            (out.affected, out.last_insert_id.as_deref()),
            (Some(1), Some("7"))
        );
        let c = command("db.people.countDocuments({})");
        let mut sink = CollectSink::default();
        show_reply(
            &c,
            &c.output,
            doc! { "cursor": { "id": 0_i64, "firstBatch": [] }, "ok": 1.0 },
            10,
            &mut sink,
        )
        .unwrap();
        assert_eq!(sink.sets[0].rows, [[Value::Int(0)]]);
    }

    #[test]
    fn a_failed_command_of_a_batch_says_where() {
        let e = prefixed(AppError::db("boom"), "bulkWrite: command 2 of 3 failed");
        assert_eq!(e.to_string(), "bulkWrite: command 2 of 3 failed: boom");
    }
}
