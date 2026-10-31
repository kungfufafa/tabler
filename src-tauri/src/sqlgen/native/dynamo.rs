//! DynamoDB request builders (the dynamodb lane): every request Tabler sends and its text. A
//! request is an action and its JSON body; its text is `Action {json}`, which the console reads
//! back to the same request (`parse_command`), so Preview shows exactly what runs.
//!
//! - The actions that only read ([`READ_ACTIONS`], swept from the API reference's complete
//!   list of 58 actions): anything else changes something, or is unknown, and is a write.
//! - The console: PartiQL statements (run with `ExecuteStatement`) and API commands
//!   (`Scan {"TableName": "t"}`), split on `;` outside strings, quoted names, comments and JSON;
//!   a statement is a read only when it is one `SELECT` that lexers cannot read differently
//!   (comments end at `\r` or `\n` as in PartiQL's lexer; text after a lone `\r` in a comment,
//!   nested `/*`, Ion literals and the like are writes), a command only when its action reads
//!   (`ExecuteStatement` and the batches by their statements, with only the reference's
//!   members). Default-deny: `INSERT`, `UPDATE`, `DELETE`, `EXISTS` (only valid inside a
//!   transaction) and anything unknown write.
//! - Placeholders: attribute names always go through `#n…` and values through `:v…`, never
//!   inline (reserved words and special characters break expressions).
//! - The catalog and the grid: ListTables, DescribeTable, the sample Scan, Scan or Query pages
//!   from the filter bar (an equality on the partition key, and one condition on the sort key,
//!   make a Query when the rest of the filter names no key attribute), counts, single cells,
//!   and the LastEvaluatedKey as the grid's cursor.
//! - Grid edits as `PutItem` / `UpdateItem` / `DeleteItem` guarded by `ConditionExpression`:
//!   the item must exist (or must not, for an insert) and each changed attribute must still
//!   hold what was loaded; a change of a key attribute is a delete and a put in one
//!   `TransactWriteItems`; a whole commit can be one transaction of at most 100 items.

use std::collections::{BTreeMap, HashMap};

use base64::Engine as _;
use serde_json::{Map, Value as J, json};

use crate::drivers::dynamodb::attr::{
    self, Attr, AttrType, Item, Json, from_value, item_from_json, map_json, parse_json,
};
use crate::error::AppError;
use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::split::SplitStatement;
use crate::types::browse::{Conjunction, FilterGroup, FilterNode, FilterOp, RowKey};
use crate::types::mutate::StagedChange;
use crate::types::schema::ColumnMeta;
use crate::types::value::Value;

// ---------------------------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------------------------

/// The actions that only read. Swept from the API reference's list (October 2026): every other
/// action writes items or tables, changes settings, tags, policies, backups, streams or
/// replicas, or reaches another service (`ExportTableToPointInTime` and `ImportTable` use S3,
/// the Kinesis destinations Kinesis).
pub const READ_ACTIONS: &[&str] = &[
    "BatchGetItem",
    "DescribeBackup",
    "DescribeContinuousBackups",
    "DescribeContributorInsights",
    "DescribeEndpoints",
    "DescribeExport",
    "DescribeGlobalTable",
    "DescribeGlobalTableSettings",
    "DescribeImport",
    "DescribeKinesisStreamingDestination",
    "DescribeLimits",
    "DescribeTable",
    "DescribeTableReplicaAutoScaling",
    "DescribeTimeToLive",
    "GetItem",
    "GetResourcePolicy",
    "ListBackups",
    "ListContributorInsights",
    "ListExports",
    "ListGlobalTables",
    "ListImports",
    "ListTables",
    "ListTagsOfResource",
    "Query",
    "Scan",
    "SearchVectors",
    "TransactGetItems",
];

/// The other actions of the reference: writes, or (the statement actions) as their statements.
pub const OTHER_ACTIONS: &[&str] = &[
    "BatchExecuteStatement",
    "BatchWriteItem",
    "CreateBackup",
    "CreateGlobalTable",
    "CreateTable",
    "DeleteBackup",
    "DeleteItem",
    "DeleteResourcePolicy",
    "DeleteTable",
    "DisableKinesisStreamingDestination",
    "EnableKinesisStreamingDestination",
    "ExecuteStatement",
    "ExecuteTransaction",
    "ExportTableToPointInTime",
    "ImportTable",
    "PutItem",
    "PutResourcePolicy",
    "RestoreTableFromBackup",
    "RestoreTableToPointInTime",
    "TagResource",
    "TransactWriteItems",
    "UntagResource",
    "UpdateContinuousBackups",
    "UpdateContributorInsights",
    "UpdateGlobalTable",
    "UpdateGlobalTableSettings",
    "UpdateItem",
    "UpdateKinesisStreamingDestination",
    "UpdateTable",
    "UpdateTableReplicaAutoScaling",
    "UpdateTimeToLive",
];

/// Whether `action` (exactly as DynamoDB spells it) only reads.
pub fn is_read_action(action: &str) -> bool {
    READ_ACTIONS.contains(&action)
}

pub fn is_known_action(action: &str) -> bool {
    is_read_action(action) || OTHER_ACTIONS.contains(&action)
}

/// The members of the statement actions' requests and of their statements (API reference,
/// October 2026). The body is sent whole, so a member the reference does not name (`statement`
/// beside `Statement`, if DynamoDB matched names regardless of case) makes the request a write.
const EXECUTE_STATEMENT_MEMBERS: &[&str] = &[
    "ConsistentRead",
    "Limit",
    "NextToken",
    "Parameters",
    "ReturnConsumedCapacity",
    "ReturnValuesOnConditionCheckFailure",
    "Statement",
];
const BATCH_EXECUTE_STATEMENT_MEMBERS: &[&str] = &["ReturnConsumedCapacity", "Statements"];
const BATCH_STATEMENT_MEMBERS: &[&str] = &[
    "ConsistentRead",
    "Parameters",
    "ReturnValuesOnConditionCheckFailure",
    "Statement",
];
const EXECUTE_TRANSACTION_MEMBERS: &[&str] = &[
    "ClientRequestToken",
    "ReturnConsumedCapacity",
    "TransactStatements",
];
const TRANSACT_STATEMENT_MEMBERS: &[&str] = &[
    "Parameters",
    "ReturnValuesOnConditionCheckFailure",
    "Statement",
];

/// The `Statement` of an object holding only `members`.
fn statement_of<'a>(o: &'a J, members: &[&str]) -> Option<&'a str> {
    let map = o.as_object()?;
    if !map.keys().all(|k| members.contains(&k.as_str())) {
        return None;
    }
    map.get("Statement")?.as_str()
}

/// Whether an API request only reads: a read action, or a statement action whose every
/// statement is a `SELECT` (and whose request holds only the reference's members).
pub fn api_class(action: &str, body: &J) -> StatementClass {
    let reads = |text: Option<&str>| text.is_some_and(|t| partiql_class(t) == StatementClass::Read);
    let list = |members: &[&str], key: &str, item_members: &[&str]| -> bool {
        let Some(items) = body
            .as_object()
            .filter(|m| m.keys().all(|k| members.contains(&k.as_str())))
            .and_then(|m| m.get(key))
            .and_then(J::as_array)
        else {
            return false;
        };
        !items.is_empty() && items.iter().all(|s| reads(statement_of(s, item_members)))
    };
    let read = match action {
        a if is_read_action(a) => true,
        "ExecuteStatement" => reads(statement_of(body, EXECUTE_STATEMENT_MEMBERS)),
        "BatchExecuteStatement" => list(
            BATCH_EXECUTE_STATEMENT_MEMBERS,
            "Statements",
            BATCH_STATEMENT_MEMBERS,
        ),
        "ExecuteTransaction" => list(
            EXECUTE_TRANSACTION_MEMBERS,
            "TransactStatements",
            TRANSACT_STATEMENT_MEMBERS,
        ),
        _ => false,
    };
    if read {
        StatementClass::Read
    } else {
        StatementClass::Write
    }
}

/// A request on one line: `Action` and its body as compact JSON (the console log's entry).
pub fn request_line(action: &str, body: &J) -> String {
    format!("{action} {body}")
}

/// The text of a request: `Action` and its body as indented JSON (what the console runs).
pub fn request_text(action: &str, body: &J) -> String {
    let json = serde_json::to_string_pretty(body).unwrap_or_else(|_| body.to_string());
    format!("{action} {json}")
}

// ---------------------------------------------------------------------------------------------
// The console: PartiQL statements and API commands
// ---------------------------------------------------------------------------------------------

/// DynamoDB refuses longer PartiQL statements.
pub const MAX_STATEMENT_CHARS: usize = 8192;

/// One console command, as it is sent.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsoleCommand {
    /// Run with `ExecuteStatement`.
    Partiql(String),
    /// An action and its request.
    Api { action: String, body: J },
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Line breaks PartiQL keeps inside a `--` comment that other lexers may end it at (Unicode's,
/// and the file, group and record separators that Python's `splitlines` ends lines at too).
const OTHER_BREAKS: [char; 8] = [
    '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}', '\u{2029}',
];

/// The comment starting at `i`, as PartiQL's lexer reads it (PartiQLTokens.g4: `--` up to `\r`
/// or `\n`, `/* … */` without nesting): where it ends, and whether another lexer could end it
/// elsewhere. Statements in API requests are sent as written, so text a lexer reads
/// differently cannot be a read: a `--` comment with text after a lone `\r` or another line
/// break before the next `\n` (one lexer's comment, another's statement), a `/*` inside a block
/// comment (SQL nests them), an unterminated block comment.
fn comment_at(chars: &[char], i: usize) -> Option<(usize, bool)> {
    match (chars.get(i), chars.get(i + 1)) {
        (Some('-'), Some('-')) => {
            let mut j = i + 2;
            let mut other_break = false;
            let mut unclear = false;
            while j < chars.len() && !matches!(chars[j], '\n' | '\r') {
                if other_break && !chars[j].is_whitespace() {
                    unclear = true;
                }
                other_break |= OTHER_BREAKS.contains(&chars[j]);
                j += 1;
            }
            // Text after a lone `\r` before the next `\n`. The look ahead stops at the first
            // text (or the `\n`), so comments ended by lone `\r`s stay linear.
            if !unclear && (other_break || chars.get(j) == Some(&'\r')) {
                unclear = chars[j..]
                    .iter()
                    .take_while(|c| **c != '\n')
                    .any(|c| !c.is_whitespace());
            }
            Some((j, unclear))
        }
        (Some('/'), Some('*')) => {
            let mut j = i + 2;
            let mut unclear = false;
            loop {
                match (chars.get(j), chars.get(j + 1)) {
                    (Some('*'), Some('/')) => return Some((j + 2, unclear)),
                    (Some('/'), Some('*')) => unclear = true,
                    (None, _) => return Some((chars.len(), true)),
                    _ => {}
                }
                j += 1;
            }
        }
        _ => None,
    }
}

/// Where whitespace and comments starting at `i` end, and whether a lexer could read those
/// comments differently (`comment_at`).
fn trivia(chars: &[char], mut i: usize) -> (usize, bool) {
    let mut unclear = false;
    loop {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        match comment_at(chars, i) {
            Some((end, u)) => {
                unclear |= u;
                i = end;
            }
            None => return (i, unclear),
        }
    }
}

fn skip_trivia(chars: &[char], i: usize) -> usize {
    trivia(chars, i).0
}

/// Where the PartiQL statement starting at `i` ends (the index of its `;`, or the end), lexed
/// like PartiQL: `'…'` strings and `"…"` names with doubled quotes inside, comments
/// (`comment_at`), `` `…` `` Ion literals. Also whether a lexer could read it differently:
/// an unclear comment, a backslash before a quote (an escape elsewhere), an Ion literal
/// (PartiQL's lexers disagree on a backtick inside its strings and comments), anything
/// unterminated.
fn partiql_end(chars: &[char], i: usize) -> (usize, bool) {
    let mut unclear = false;
    let mut j = i;
    while j < chars.len() {
        match chars[j] {
            ';' => return (j, unclear),
            q @ ('\'' | '"') => {
                j += 1;
                loop {
                    match chars.get(j) {
                        None => return (chars.len(), true),
                        Some(c) if *c == q => {
                            if chars.get(j + 1) == Some(&q) {
                                j += 2;
                                continue;
                            }
                            break;
                        }
                        Some('\\') if chars.get(j + 1) == Some(&q) => {
                            unclear = true;
                            j += 1;
                        }
                        _ => j += 1,
                    }
                }
            }
            '`' => {
                unclear = true;
                j += 1;
                while j < chars.len() && chars[j] != '`' {
                    j += 1;
                }
                if j >= chars.len() {
                    return (chars.len(), true);
                }
            }
            '-' | '/' => {
                if let Some((end, u)) = comment_at(chars, j) {
                    unclear |= u;
                    j = end;
                    continue;
                }
            }
            _ => {}
        }
        j += 1;
    }
    (chars.len(), unclear)
}

/// The action of an API command starting at `i`: a word followed by `{`, or a known action
/// alone.
fn api_action(chars: &[char], i: usize) -> Option<String> {
    if !chars.get(i).is_some_and(char::is_ascii_alphabetic) {
        return None;
    }
    let mut j = i;
    while j < chars.len() && is_word_char(chars[j]) {
        j += 1;
    }
    let word: String = chars[i..j].iter().collect();
    let mut k = j;
    while k < chars.len() && chars[k].is_whitespace() {
        k += 1;
    }
    match chars.get(k) {
        Some('{') => Some(word),
        None | Some(';') if is_known_action(&word) => Some(word),
        _ => None,
    }
}

/// Where the command starting at `i` ends (the index of its `;`, or the end): an API command's
/// JSON with its backslash escapes, else a PartiQL statement (`partiql_end`).
fn command_end(chars: &[char], i: usize) -> usize {
    if api_action(chars, i).is_none() {
        return partiql_end(chars, i).0;
    }
    let mut j = i;
    while j < chars.len() {
        match chars[j] {
            ';' => return j,
            '"' => {
                j += 1;
                while j < chars.len() && chars[j] != '"' {
                    if chars[j] == '\\' {
                        j += 1;
                    }
                    j += 1;
                }
            }
            _ => {}
        }
        j += 1;
    }
    chars.len()
}

/// Splits console text into commands (char offsets, like `sqlgen::split`): on `;` outside
/// strings, quoted names, comments and an API command's JSON. Leading comments are not part of
/// a command; empty and comment-only ones are skipped.
pub fn split_console(text: &str) -> Vec<SplitStatement> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    // Lines are counted on from the last command's start, not from the top each time.
    let (mut counted, mut line) = (0, 1u32);
    loop {
        i = skip_trivia(&chars, i);
        while chars.get(i) == Some(&';') {
            i = skip_trivia(&chars, i + 1);
        }
        if i >= chars.len() {
            return out;
        }
        let end = command_end(&chars, i);
        let mut e = end.min(chars.len());
        while e > i && chars[e - 1].is_whitespace() {
            e -= 1;
        }
        if e > i {
            line += chars[counted..i].iter().filter(|&&c| c == '\n').count() as u32;
            counted = i;
            out.push(SplitStatement {
                sql: chars[i..e].iter().collect(),
                start: i,
                end: e,
                line,
            });
        }
        i = end + 1;
    }
}

/// A PartiQL statement reads only when it is one `SELECT` (after whitespace and comments) that
/// every lexer reads the same way (`partiql_end`), always lexed as PartiQL: the text is sent as
/// it is, from the console or inside an API request.
pub fn partiql_class(statement: &str) -> StatementClass {
    let chars: Vec<char> = statement.chars().collect();
    let (i, lead_unclear) = trivia(&chars, 0);
    let mut j = i;
    while j < chars.len() && chars[j].is_ascii_alphabetic() {
        j += 1;
    }
    let word: String = chars[i..j].iter().collect();
    let ends = chars.get(j).is_none_or(|c| !is_word_char(*c) && *c != '$');
    if word.eq_ignore_ascii_case("SELECT") && ends && !lead_unclear && one_statement(&chars, i) {
        StatementClass::Read
    } else {
        StatementClass::Write
    }
}

/// What a PartiQL statement that succeeded proves it changed. Not a safety check: a `SELECT` the
/// classifier calls a write still reads, and changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartiqlWrite {
    Nothing,
    /// One item: DynamoDB refuses an `INSERT` of a key that exists (`DuplicateItemException`)
    /// and an `UPDATE` of one that does not (`ConditionalCheckFailedException`).
    One,
    /// One item or none: a `DELETE` of a missing key succeeds and deletes nothing.
    AtMostOne,
}

impl PartiqlWrite {
    /// The items it changed, given how many it returned (`RETURNING ALL OLD *`); unknown for a
    /// `DELETE` that returned none.
    pub fn affected(self, returned: usize) -> Option<u64> {
        match self {
            PartiqlWrite::Nothing => None,
            PartiqlWrite::One => Some(1),
            PartiqlWrite::AtMostOne => (returned > 0).then_some(1),
        }
    }
}

/// The write a PartiQL statement is, by its first word.
pub fn partiql_write(statement: &str) -> PartiqlWrite {
    let chars: Vec<char> = statement.chars().collect();
    let i = skip_trivia(&chars, 0);
    let mut j = i;
    while j < chars.len() && chars[j].is_ascii_alphabetic() {
        j += 1;
    }
    let word: String = chars[i..j].iter().collect();
    if chars.get(j).is_some_and(|c| is_word_char(*c) || *c == '$') {
        return PartiqlWrite::Nothing;
    }
    if word.eq_ignore_ascii_case("INSERT") || word.eq_ignore_ascii_case("UPDATE") {
        PartiqlWrite::One
    } else if word.eq_ignore_ascii_case("DELETE") {
        PartiqlWrite::AtMostOne
    } else {
        PartiqlWrite::Nothing
    }
}

/// Whether the statement starting at `i` reads clearly and nothing but whitespace, comments
/// and `;` follows it (an `ExecuteStatement` request's text could otherwise carry a second).
fn one_statement(chars: &[char], i: usize) -> bool {
    let (mut k, unclear) = partiql_end(chars, i);
    if unclear {
        return false;
    }
    while k < chars.len() {
        let (next, unclear) = trivia(chars, k + 1);
        if unclear || chars.get(next).is_some_and(|c| *c != ';') {
            return false;
        }
        k = next;
    }
    true
}

/// One console command as it will be sent; an API command's body must be a JSON object (no
/// repeated keys, so what is sent is what was classified).
pub fn parse_command(text: &str) -> Result<ConsoleCommand, AppError> {
    let chars: Vec<char> = text.chars().collect();
    let i = skip_trivia(&chars, 0);
    let rest: String = chars[i..].iter().collect();
    let rest = rest.trim_end().trim_end_matches(';').trim_end();
    if let Some(action) = api_action(&chars, i) {
        let body = rest[action.len()..].trim();
        let body = if body.is_empty() { "{}" } else { body };
        if !matches!(parse_json(body)?, Json::Obj(_)) {
            return Err(AppError::InvalidInput(format!(
                "{action}: the request is a JSON object"
            )));
        }
        return Ok(ConsoleCommand::Api {
            action,
            body: serde_json::from_str(body)?,
        });
    }
    if rest.is_empty() {
        return Err(AppError::InvalidInput("an empty statement".into()));
    }
    if rest.chars().count() > MAX_STATEMENT_CHARS {
        return Err(AppError::InvalidInput(format!(
            "DynamoDB runs PartiQL statements of at most {MAX_STATEMENT_CHARS} characters"
        )));
    }
    Ok(ConsoleCommand::Partiql(rest.to_string()))
}

/// Whether console text only reads: every command it holds must (a command that does not parse
/// is a write).
pub fn classify_console(text: &str) -> StatementClass {
    let commands = split_console(text);
    if commands.is_empty() {
        return StatementClass::Write;
    }
    let all_read = commands.iter().all(|c| match parse_command(&c.sql) {
        Ok(ConsoleCommand::Partiql(s)) => partiql_class(&s) == StatementClass::Read,
        Ok(ConsoleCommand::Api { action, body }) => {
            api_class(&action, &body) == StatementClass::Read
        }
        Err(_) => false,
    });
    if all_read {
        StatementClass::Read
    } else {
        StatementClass::Write
    }
}

/// The request a console command sends: `ExecuteStatement` for PartiQL.
pub fn command_request(cmd: &ConsoleCommand, next_token: Option<&str>) -> (String, J) {
    match cmd {
        ConsoleCommand::Partiql(statement) => {
            let mut body = json!({ "Statement": statement });
            if let Some(t) = next_token {
                body["NextToken"] = J::String(t.to_string());
            }
            ("ExecuteStatement".to_string(), body)
        }
        ConsoleCommand::Api { action, body } => (action.clone(), body.clone()),
    }
}

// ---------------------------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------------------------

/// The placeholders of one request's expressions: `#n0` for attribute names, `:v0` for values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Expr {
    names: Vec<String>,
    values: Vec<Attr>,
}

impl Expr {
    /// The placeholder of attribute `name` (the same one each time).
    pub fn name(&mut self, name: &str) -> String {
        let i = match self.names.iter().position(|n| n == name) {
            Some(i) => i,
            None => {
                self.names.push(name.to_string());
                self.names.len() - 1
            }
        };
        format!("#n{i}")
    }

    /// The placeholder of a value.
    pub fn value(&mut self, a: Attr) -> String {
        self.values.push(a);
        format!(":v{}", self.values.len() - 1)
    }

    /// Adds `ExpressionAttributeNames` and `ExpressionAttributeValues` (when there are any).
    pub fn apply(&self, body: &mut Map<String, J>) {
        if !self.names.is_empty() {
            body.insert(
                "ExpressionAttributeNames".into(),
                J::Object(
                    self.names
                        .iter()
                        .enumerate()
                        .map(|(i, n)| (format!("#n{i}"), J::String(n.clone())))
                        .collect(),
                ),
            );
        }
        if !self.values.is_empty() {
            body.insert(
                "ExpressionAttributeValues".into(),
                J::Object(
                    self.values
                        .iter()
                        .enumerate()
                        .map(|(i, v)| (format!(":v{i}"), v.to_json()))
                        .collect(),
                ),
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Keys, column types and the catalog requests
// ---------------------------------------------------------------------------------------------

/// The key attributes of a table or index, with their types (`S`, `N` or `B`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySchema {
    pub hash: (String, AttrType),
    pub range: Option<(String, AttrType)>,
}

impl KeySchema {
    pub fn names(&self) -> Vec<&str> {
        let mut n = vec![self.hash.0.as_str()];
        if let Some((r, _)) = &self.range {
            n.push(r);
        }
        n
    }

    pub fn type_of(&self, name: &str) -> Option<AttrType> {
        if self.hash.0 == name {
            return Some(self.hash.1);
        }
        self.range
            .as_ref()
            .filter(|(r, _)| r == name)
            .map(|(_, t)| *t)
    }

    /// The key of an item; `None` when it lacks a key attribute.
    pub fn key_of(&self, item: &Item) -> Option<Item> {
        self.names()
            .into_iter()
            .map(|n| Some((n.to_string(), item.get(n)?.clone())))
            .collect()
    }
}

/// The types of a column (`ColumnMeta.data_type` is `S|N|NULL`).
pub fn column_types(data_type: &str) -> Vec<AttrType> {
    data_type
        .split('|')
        .filter_map(|t| AttrType::parse(t.trim()))
        .collect()
}

/// The one type of a column besides NULL, when it has exactly one.
pub fn column_hint(columns: &[ColumnMeta], name: &str) -> Option<AttrType> {
    let col = columns.iter().find(|c| c.name == name)?;
    let types: Vec<AttrType> = column_types(&col.data_type)
        .into_iter()
        .filter(|t| *t != AttrType::Null)
        .collect();
    match types.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// `ListTables` from `start` (the previous page's `LastEvaluatedTableName`).
pub fn list_tables(start: Option<&str>, limit: u32) -> J {
    let mut body = json!({ "Limit": limit.clamp(1, 100) });
    if let Some(s) = start {
        body["ExclusiveStartTableName"] = J::String(s.to_string());
    }
    body
}

pub fn describe_table(table: &str) -> J {
    json!({ "TableName": table })
}

pub fn describe_time_to_live(table: &str) -> J {
    json!({ "TableName": table })
}

pub fn describe_continuous_backups(table: &str) -> J {
    json!({ "TableName": table })
}

/// What the session pings with: free, and answered by DynamoDB itself without a table.
pub fn describe_endpoints() -> J {
    json!({})
}

// ---------------------------------------------------------------------------------------------
// Reads: the grid's Scan or Query, counts, cells, cursors
// ---------------------------------------------------------------------------------------------

/// The grid's cursor: the `LastEvaluatedKey` as base64url of its JSON.
pub fn encode_cursor(key: &Item) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(map_json(key).to_string())
}

pub fn decode_cursor(cursor: &str) -> Result<Item, AppError> {
    let bad = || AppError::InvalidInput("the grid's page cursor is not one Tabler made".into());
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor.trim())
        .map_err(|_| bad())?;
    let json: J = serde_json::from_slice(&bytes).map_err(|_| bad())?;
    let item = item_from_json(&json).map_err(|_| bad())?;
    if item.is_empty() {
        return Err(bad());
    }
    Ok(item)
}

/// What a grid read is.
#[derive(Debug, Clone)]
pub struct ReadSpec<'a> {
    pub table: &'a str,
    /// A secondary index to read instead of the table.
    pub index: Option<&'a str>,
    /// The keys of what is read (the index's for an index).
    pub keys: &'a KeySchema,
    /// The columns, for the types filter values are read as.
    pub columns: &'a [ColumnMeta],
    pub filter: Option<&'a FilterGroup>,
    /// Newest first (a Query only: `ScanIndexForward: false`).
    pub descending: bool,
    /// `ConsistentRead`: the caller asks only where DynamoDB has it, the table and its local
    /// indexes (only the description tells a global index that shares the table's keys apart).
    pub consistent: bool,
    pub consumed_capacity: bool,
}

/// A planned Scan or Query: the action and its body without `Limit` or `ExclusiveStartKey`.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadRequest {
    pub action: &'static str,
    pub body: Map<String, J>,
}

impl ReadRequest {
    /// The body of one page: at most `limit` evaluated items, after `start`.
    pub fn page(&self, limit: Option<u32>, start: Option<&Item>) -> J {
        let mut body = self.body.clone();
        if let Some(l) = limit {
            body.insert("Limit".into(), json!(l.max(1)));
        }
        if let Some(s) = start {
            body.insert("ExclusiveStartKey".into(), map_json(s));
        }
        J::Object(body)
    }

    /// The same read counting items only (`Select: COUNT`).
    pub fn count(&self) -> ReadRequest {
        let mut body = self.body.clone();
        body.insert("Select".into(), json!("COUNT"));
        body.remove("ProjectionExpression");
        ReadRequest {
            action: self.action,
            body,
        }
    }

    /// The text the grid shows for this read.
    pub fn text(&self) -> String {
        request_text(self.action, &J::Object(self.body.clone()))
    }
}

/// The grid's read: a Query when the filter fixes the partition key (and maybe one sort key
/// condition) and the rest of it leaves the key attributes alone (a Query's
/// `FilterExpression` cannot name them: a second key condition, Any column or a raw
/// expression naming a key make a Scan), else a Scan; the rest is a `FilterExpression`.
pub fn read_request(spec: &ReadSpec<'_>) -> Result<ReadRequest, AppError> {
    if let Some(f) = spec.filter {
        let mut expr = Expr::default();
        if let (Some(k), rest) = split_key_condition(f, spec.keys, &mut expr)? {
            let filter = match &rest {
                Some(g) => group_expr(g, spec.columns, &mut expr)?,
                None => None,
            };
            if !filter
                .as_deref()
                .is_some_and(|t| names_a_key(t, &expr, spec.keys))
            {
                return Ok(read_body(spec, Some(k), filter, &expr));
            }
        }
    }
    let mut expr = Expr::default();
    let filter = match spec.filter {
        Some(g) => group_expr(g, spec.columns, &mut expr)?,
        None => None,
    };
    Ok(read_body(spec, None, filter, &expr))
}

/// Whether an expression names a key attribute: through a placeholder of `expr`, or by name
/// (a raw expression's words; a nested member of the same name counts too).
fn names_a_key(text: &str, expr: &Expr, keys: &KeySchema) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !(c == '#' || c == ':' || is_word_char(c)) {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < chars.len() && is_word_char(chars[i]) {
            i += 1;
        }
        let word: String = chars[start + 1..i].iter().collect();
        let name = match c {
            '#' => word
                .strip_prefix('n')
                .and_then(|n| n.parse::<usize>().ok())
                .and_then(|n| expr.names.get(n))
                .cloned(),
            ':' => None,
            _ => Some(format!("{c}{word}")),
        };
        if name.is_some_and(|n| keys.type_of(&n).is_some()) {
            return true;
        }
    }
    false
}

fn read_body(
    spec: &ReadSpec<'_>,
    key_condition: Option<String>,
    filter: Option<String>,
    expr: &Expr,
) -> ReadRequest {
    let mut body = Map::new();
    body.insert("TableName".into(), json!(spec.table));
    if let Some(i) = spec.index {
        body.insert("IndexName".into(), json!(i));
    }
    let action = if let Some(k) = key_condition {
        body.insert("KeyConditionExpression".into(), json!(k));
        if spec.descending {
            body.insert("ScanIndexForward".into(), json!(false));
        }
        "Query"
    } else {
        "Scan"
    };
    if let Some(f) = filter {
        body.insert("FilterExpression".into(), json!(f));
    }
    if spec.consistent {
        body.insert("ConsistentRead".into(), json!(true));
    }
    if spec.consumed_capacity {
        body.insert("ReturnConsumedCapacity".into(), json!("TOTAL"));
    }
    expr.apply(&mut body);
    ReadRequest { action, body }
}

/// The Scan that samples a table's attributes for its columns.
pub fn sample_scan(table: &str, limit: u32) -> J {
    json!({ "TableName": table, "Limit": limit.max(1) })
}

/// `GetItem` of one attribute of one item (`fetch_cell`).
pub fn get_attribute(table: &str, key: &Item, attribute: &str, consistent: bool) -> J {
    let mut expr = Expr::default();
    let mut body = Map::new();
    body.insert("TableName".into(), json!(table));
    body.insert("Key".into(), map_json(key));
    body.insert("ProjectionExpression".into(), json!(expr.name(attribute)));
    if consistent {
        body.insert("ConsistentRead".into(), json!(true));
    }
    expr.apply(&mut body);
    J::Object(body)
}

/// Takes the key conditions out of a filter: `#hash = :v` and at most one sort key condition,
/// both at the top level of an AND (or of a group with one item). The rest stays a filter.
fn split_key_condition(
    filter: &FilterGroup,
    keys: &KeySchema,
    expr: &mut Expr,
) -> Result<(Option<String>, Option<FilterGroup>), AppError> {
    let and = filter.conjunction == Conjunction::And || filter.items.len() <= 1;
    if !and {
        return Ok((None, Some(filter.clone())));
    }
    // DynamoDB refuses an empty string as a key condition's value (the condition stays a
    // filter, and the read a Scan).
    let is_cond = |n: &FilterNode, name: &str, ops: &[FilterOp]| match n {
        FilterNode::Condition { column, op, values } => {
            column == name
                && ops.contains(op)
                && !values
                    .iter()
                    .any(|v| v.is_null() || v.as_text().is_some_and(|t| t.is_empty()))
        }
        _ => false,
    };
    let Some(h) = filter
        .items
        .iter()
        .position(|n| is_cond(n, &keys.hash.0, &[FilterOp::Eq]))
    else {
        return Ok((None, Some(filter.clone())));
    };
    let key_value = |v: &Value, ty: AttrType| -> Result<Attr, AppError> {
        match from_value(v, Some(ty))? {
            Some(a) if a.ty() == ty => Ok(a),
            _ => Err(AppError::InvalidInput(format!(
                "the key filter value {} is not of the key's type {}",
                v.as_text().unwrap_or_default(),
                ty.as_str()
            ))),
        }
    };
    let mut rest: Vec<FilterNode> = Vec::new();
    let mut cond = String::new();
    let mut range_done = false;
    let range_ops = [
        FilterOp::Eq,
        FilterOp::Lt,
        FilterOp::Le,
        FilterOp::Gt,
        FilterOp::Ge,
        FilterOp::Between,
        FilterOp::HasPrefix,
    ];
    for (i, node) in filter.items.iter().enumerate() {
        if i == h {
            let FilterNode::Condition { values, .. } = node else {
                continue;
            };
            let [v] = values.as_slice() else {
                return Err(AppError::InvalidInput(
                    "an equality filter takes one value".into(),
                ));
            };
            let n = expr.name(&keys.hash.0);
            let p = expr.value(key_value(v, keys.hash.1)?);
            cond = format!("{n} = {p}");
            continue;
        }
        if let (Some((range, ty)), false) = (&keys.range, range_done)
            && is_cond(node, range, &range_ops)
            && let FilterNode::Condition { op, values, .. } = node
        {
            let prefix_ok = *op != FilterOp::HasPrefix || *ty == AttrType::S;
            let count_ok = if *op == FilterOp::Between {
                values.len() == 2
            } else {
                values.len() == 1
            };
            if prefix_ok && count_ok {
                let n = expr.name(range);
                let mut vals = Vec::new();
                for v in values {
                    vals.push(expr.value(key_value(v, *ty)?));
                }
                let part = match op {
                    FilterOp::Between => format!("{n} BETWEEN {} AND {}", vals[0], vals[1]),
                    FilterOp::HasPrefix => format!("begins_with({n}, {})", vals[0]),
                    other => format!("{n} {} {}", comparison(*other), vals[0]),
                };
                cond.push_str(&format!(" AND {part}"));
                range_done = true;
                continue;
            }
        }
        rest.push(node.clone());
    }
    let rest = (!rest.is_empty()).then_some(FilterGroup {
        conjunction: Conjunction::And,
        items: rest,
    });
    Ok((Some(cond), rest))
}

fn comparison(op: FilterOp) -> &'static str {
    match op {
        FilterOp::Eq => "=",
        FilterOp::Ne => "<>",
        FilterOp::Gt => ">",
        FilterOp::Lt => "<",
        FilterOp::Ge => ">=",
        _ => "<=",
    }
}

fn unsupported(what: &str) -> AppError {
    AppError::Unsupported(format!("DynamoDB filters cannot {what}"))
}

/// The AttributeValues a filter value may mean in a column: one per type the column was seen
/// with that the value can be read as (text `5` in a column of strings and numbers is both).
pub fn candidates(v: &Value, columns: &[ColumnMeta], column: &str) -> Result<Vec<Attr>, AppError> {
    let types: Vec<AttrType> = columns
        .iter()
        .find(|c| c.name == column)
        .map(|c| column_types(&c.data_type))
        .unwrap_or_default()
        .into_iter()
        .filter(|t| !matches!(t, AttrType::Null))
        .collect();
    let mut out: Vec<Attr> = Vec::new();
    if types.is_empty() {
        if let Some(a) = from_value(v, None)? {
            out.push(a);
        }
    } else {
        for t in types {
            if let Ok(Some(a)) = from_value(v, Some(t))
                && a.ty() == t
                && !out.contains(&a)
            {
                out.push(a);
            }
        }
    }
    if out.is_empty() {
        return Err(AppError::InvalidInput(format!(
            "the filter value {} does not fit the column {column}",
            v.as_text().unwrap_or_default()
        )));
    }
    Ok(out)
}

fn group_expr(
    g: &FilterGroup,
    columns: &[ColumnMeta],
    expr: &mut Expr,
) -> Result<Option<String>, AppError> {
    let mut parts = Vec::new();
    for item in &g.items {
        if let Some(p) = node_expr(item, columns, expr)? {
            parts.push(p);
        }
    }
    Ok(match parts.len() {
        0 => None,
        1 => parts.pop(),
        _ => {
            let joiner = match g.conjunction {
                Conjunction::And => " AND ",
                Conjunction::Or => " OR ",
            };
            Some(format!("({})", parts.join(joiner)))
        }
    })
}

fn node_expr(
    node: &FilterNode,
    columns: &[ColumnMeta],
    expr: &mut Expr,
) -> Result<Option<String>, AppError> {
    match node {
        FilterNode::Condition { column, op, values } => {
            condition(column, *op, values, columns, expr).map(Some)
        }
        FilterNode::Group(g) => group_expr(g, columns, expr),
        FilterNode::Raw { sql } => {
            let text = sql.trim();
            Ok((!text.is_empty()).then(|| format!("({text})")))
        }
        FilterNode::AnyColumn { op, values } => {
            let mut parts = Vec::new();
            for c in columns {
                // Columns a value cannot fit are left out.
                let mut trial = expr.clone();
                if let Ok(p) = condition(&c.name, *op, values, columns, &mut trial) {
                    *expr = trial;
                    parts.push(p);
                }
            }
            Ok(match (parts.is_empty(), op.is_negated()) {
                // No column can hold the value: none matches it, so all match its negation.
                (true, true) => None,
                (true, false) => Some(false_expr(expr)),
                (false, negated) => Some(format!(
                    "({})",
                    parts.join(if negated { " AND " } else { " OR " })
                )),
            })
        }
    }
}

/// An expression no item satisfies.
fn false_expr(expr: &mut Expr) -> String {
    let n = expr.name("_");
    format!("(attribute_exists({n}) AND attribute_not_exists({n}))")
}

fn condition(
    column: &str,
    op: FilterOp,
    values: &[Value],
    columns: &[ColumnMeta],
    expr: &mut Expr,
) -> Result<String, AppError> {
    let one = || match values {
        [v] => Ok(v),
        _ => Err(AppError::InvalidInput(format!(
            "the filter on {column} takes one value"
        ))),
    };
    let n = expr.name(column);
    let any =
        |expr: &mut Expr, v: &Value, f: &dyn Fn(&str) -> String| -> Result<String, AppError> {
            let parts: Vec<String> = candidates(v, columns, column)?
                .into_iter()
                .map(|a| f(&expr.value(a)))
                .collect();
            Ok(if parts.len() == 1 {
                parts.into_iter().next().unwrap_or_default()
            } else {
                format!("({})", parts.join(" OR "))
            })
        };
    let text_value =
        |expr: &mut Expr, v: &Value| expr.value(Attr::S(v.as_text().unwrap_or_default()));
    Ok(match op {
        FilterOp::IsNull | FilterOp::IsNotNull => {
            let t = expr.value(Attr::S("NULL".into()));
            if op == FilterOp::IsNull {
                format!("(attribute_not_exists({n}) OR attribute_type({n}, {t}))")
            } else {
                format!("(attribute_exists({n}) AND NOT attribute_type({n}, {t}))")
            }
        }
        FilterOp::Eq | FilterOp::Gt | FilterOp::Lt | FilterOp::Ge | FilterOp::Le => {
            let v = one()?;
            any(expr, v, &|p| format!("{n} {} {p}", comparison(op)))?
        }
        FilterOp::Ne => {
            let v = one()?;
            let eq = any(expr, v, &|p| format!("{n} = {p}"))?;
            format!("NOT {eq}")
        }
        FilterOp::In | FilterOp::NotIn => {
            if values.is_empty() || values.len() > 100 {
                return Err(AppError::InvalidInput(
                    "an IN filter takes 1 to 100 values".into(),
                ));
            }
            let mut ps = Vec::new();
            for v in values {
                for a in candidates(v, columns, column)? {
                    ps.push(expr.value(a));
                }
            }
            if ps.len() > 100 {
                return Err(AppError::InvalidInput(
                    "an IN filter takes at most 100 values".into(),
                ));
            }
            let inn = format!("{n} IN ({})", ps.join(", "));
            if op == FilterOp::In {
                inn
            } else {
                format!("NOT ({inn})")
            }
        }
        FilterOp::Between | FilterOp::NotBetween => {
            let [lo, hi] = values else {
                return Err(AppError::InvalidInput(format!(
                    "the BETWEEN filter on {column} takes two values"
                )));
            };
            let los = candidates(lo, columns, column)?;
            let his = candidates(hi, columns, column)?;
            let mut parts = Vec::new();
            for l in los {
                if let Some(h) = his.iter().find(|h| h.ty() == l.ty()) {
                    let (pl, ph) = (expr.value(l), expr.value(h.clone()));
                    parts.push(format!("{n} BETWEEN {pl} AND {ph}"));
                }
            }
            if parts.is_empty() {
                return Err(AppError::InvalidInput(format!(
                    "the BETWEEN values on {column} are of different types"
                )));
            }
            let b = if parts.len() == 1 {
                parts.remove(0)
            } else {
                format!("({})", parts.join(" OR "))
            };
            if op == FilterOp::Between {
                b
            } else {
                format!("NOT {b}")
            }
        }
        FilterOp::Contains | FilterOp::NotContains => {
            let p = text_value(expr, one()?);
            if op == FilterOp::Contains {
                format!("contains({n}, {p})")
            } else {
                format!("NOT contains({n}, {p})")
            }
        }
        FilterOp::HasPrefix => {
            let p = text_value(expr, one()?);
            format!("begins_with({n}, {p})")
        }
        FilterOp::Like | FilterOp::NotLike => {
            let pattern = one()?.as_text().unwrap_or_default();
            let body = pattern.strip_suffix('%').unwrap_or(&pattern);
            if body.contains(['%', '_']) {
                return Err(unsupported(
                    "match LIKE patterns other than a prefix (abc%) or an exact value",
                ));
            }
            let p = expr.value(Attr::S(body.to_string()));
            let positive = if pattern.ends_with('%') {
                format!("begins_with({n}, {p})")
            } else {
                format!("{n} = {p}")
            };
            if op == FilterOp::Like {
                positive
            } else {
                format!("NOT {positive}")
            }
        }
        FilterOp::ContainsCase
        | FilterOp::NotContainsCase
        | FilterOp::HasPrefixCase
        | FilterOp::HasSuffixCase
        | FilterOp::Ilike => return Err(unsupported("match regardless of case")),
        FilterOp::HasSuffix => return Err(unsupported("match a suffix")),
        FilterOp::JsonContains | FilterOp::JsonNotContains => {
            return Err(unsupported("match JSON containment"));
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------------------------

/// Items the grid loaded, by key, so an edit can prove each changed attribute still holds what
/// was shown, keep each attribute's type, and copy an item whose key changes. The session
/// records each page it returns; the oldest are forgotten past `CAP` items or `BYTE_CAP` bytes
/// (an item no longer loaded is still edited, guarded by its key alone; its key cannot change).
#[derive(Debug, Clone, Default)]
pub struct LoadedItems {
    /// Each item with its size ([`LoadedItems::size`]).
    by_key: HashMap<String, (Item, usize)>,
    order: std::collections::VecDeque<String>,
    bytes: usize,
}

impl LoadedItems {
    pub const CAP: usize = 20_000;
    /// 20,000 items of DynamoDB's largest (400 KB) would be 8 GB.
    pub const BYTE_CAP: usize = 128 * 1024 * 1024;

    fn key_text(key: &Item) -> String {
        map_json(key).to_string()
    }

    /// At most the memory an item kept takes: the item ([`attr::item_size`]), its key's text
    /// twice (the map's and the order's) and the map's entry with its spare room.
    fn size(key: &str, item: &Item) -> usize {
        2 * (key.len() + key.len() / 8 + 32) + 128 + attr::item_size(item)
    }

    /// Remembers a loaded item (one without its key attributes is not editable).
    pub fn record(&mut self, keys: &KeySchema, item: &Item) {
        let Some(key) = keys.key_of(item) else {
            return;
        };
        let k = LoadedItems::key_text(&key);
        let size = LoadedItems::size(&k, item);
        match self.by_key.insert(k.clone(), (item.clone(), size)) {
            Some((_, old)) => self.bytes = self.bytes.saturating_sub(old),
            None => self.order.push_back(k),
        }
        self.bytes += size;
        self.trim(LoadedItems::CAP, LoadedItems::BYTE_CAP);
    }

    /// Forgets the oldest items until at most `items` of at most `bytes` are left.
    pub fn trim(&mut self, items: usize, bytes: usize) {
        while self.order.len() > items || self.bytes > bytes {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some((_, size)) = self.by_key.remove(&old) {
                self.bytes = self.bytes.saturating_sub(size);
            }
        }
    }

    pub fn get(&self, key: &Item) -> Option<&Item> {
        self.by_key
            .get(&LoadedItems::key_text(key))
            .map(|(item, _)| item)
    }

    /// Forgets an item a commit wrote: what was loaded no longer guards it.
    pub fn forget(&mut self, key: &Item) {
        let k = LoadedItems::key_text(key);
        if let Some((_, size)) = self.by_key.remove(&k) {
            self.bytes = self.bytes.saturating_sub(size);
            self.order.retain(|o| *o != k);
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// About the bytes the items take.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub fn clear(&mut self) {
        self.by_key.clear();
        self.order.clear();
        self.bytes = 0;
    }
}

/// Keeps the loaded items of a session's tables within `bytes` in all: the other tables' go
/// first, the largest first, then the oldest of `current`'s.
pub fn trim_loaded(tables: &mut HashMap<String, LoadedItems>, current: &str, bytes: usize) {
    let mut total: usize = tables.values().map(LoadedItems::bytes).sum();
    while total > bytes {
        let largest = tables
            .iter()
            .filter(|(t, _)| t.as_str() != current)
            .max_by_key(|(_, l)| l.bytes())
            .map(|(t, _)| t.clone());
        let Some(other) = largest else {
            break;
        };
        let freed = tables.remove(&other).map_or(0, |l| l.bytes());
        total = total.saturating_sub(freed);
    }
    if total > bytes
        && let Some(own) = tables.get_mut(current)
    {
        own.trim(LoadedItems::CAP, bytes);
    }
}

/// One write of a transaction (`Put`, `Update`, `Delete`) or of its own request.
#[derive(Debug, Clone, PartialEq)]
pub struct TxItem {
    /// `Put`, `Update` or `Delete`.
    pub kind: &'static str,
    /// The request: `TableName`, `Item` or `Key`, the expressions.
    pub body: J,
    /// The key of the item it writes (no two items of a transaction may write the same one).
    pub key: String,
}

impl TxItem {
    /// The action that sends it alone.
    pub fn action(&self) -> &'static str {
        match self.kind {
            "Put" => "PutItem",
            "Update" => "UpdateItem",
            _ => "DeleteItem",
        }
    }
}

/// The writes of one staged change: one request, or (a changed key) a delete and a put that
/// must apply together.
#[derive(Debug, Clone, PartialEq)]
pub struct WriteOp {
    /// Index of the change in the request.
    pub change: usize,
    pub row_id: String,
    pub op: &'static str,
    pub items: Vec<TxItem>,
    /// The `ClientRequestToken` of a transaction of its own (a changed key).
    pub token: String,
}

impl WriteOp {
    /// The request that applies this change alone.
    pub fn request(&self) -> (&'static str, J) {
        match self.items.as_slice() {
            [one] => (one.action(), one.body.clone()),
            items => (
                "TransactWriteItems",
                json!({
                    "TransactItems": items.iter().map(|i| json!({ i.kind: i.body })).collect::<Vec<_>>(),
                    "ClientRequestToken": self.token,
                }),
            ),
        }
    }

    pub fn text(&self) -> String {
        let (action, body) = self.request();
        request_text(action, &body)
    }
}

/// A transaction of a whole commit (or one part of it): the request, and the change of each of
/// its items in order (for `CancellationReasons`).
#[derive(Debug, Clone, PartialEq)]
pub struct Transaction {
    pub body: J,
    pub changes: Vec<usize>,
}

/// DynamoDB takes at most this many items per transaction.
pub const MAX_TRANSACTION_ITEMS: usize = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct EditPlan {
    pub ops: Vec<WriteOp>,
    /// Prefix of the `ClientRequestToken`s: at most 28 characters, so with a suffix they stay
    /// within the 36 DynamoDB takes.
    pub token: String,
}

impl EditPlan {
    /// The preview text of each change, from the same requests that run.
    pub fn preview(&self) -> Vec<(usize, String)> {
        self.ops.iter().map(|op| (op.change, op.text())).collect()
    }

    /// The whole commit as transactions of at most 100 items, each change whole in one of them.
    /// Two changes of the same item cannot share a transaction.
    pub fn transactions(&self) -> Result<Vec<Transaction>, AppError> {
        let mut out: Vec<Transaction> = Vec::new();
        let mut items: Vec<J> = Vec::new();
        let mut changes: Vec<usize> = Vec::new();
        let mut keys: Vec<&str> = Vec::new();
        let flush = |out: &mut Vec<Transaction>, items: &mut Vec<J>, changes: &mut Vec<usize>| {
            if items.is_empty() {
                return;
            }
            out.push(Transaction {
                body: json!({
                    "TransactItems": std::mem::take(items),
                    "ClientRequestToken": format!("{}-t{}", self.token, out.len()),
                }),
                changes: std::mem::take(changes),
            });
        };
        for op in &self.ops {
            if items.len() + op.items.len() > MAX_TRANSACTION_ITEMS {
                flush(&mut out, &mut items, &mut changes);
                keys.clear();
            }
            for i in &op.items {
                if keys.contains(&i.key.as_str()) {
                    return Err(AppError::InvalidInput(
                        "two changes of the same item cannot be applied in one transaction; \
                         commit them one at a time"
                            .into(),
                    ));
                }
                keys.push(&i.key);
                items.push(json!({ i.kind: i.body }));
                changes.push(op.change);
            }
        }
        flush(&mut out, &mut items, &mut changes);
        Ok(out)
    }
}

/// The type to write `name` as: the type it had in the loaded item (a null says nothing), else
/// the key's, else the column's one type.
fn hint(
    loaded: Option<&Item>,
    keys: &KeySchema,
    columns: &[ColumnMeta],
    name: &str,
) -> Option<AttrType> {
    match loaded.and_then(|i| i.get(name)).map(Attr::ty) {
        Some(t) if t != AttrType::Null => Some(t),
        _ => keys.type_of(name).or_else(|| column_hint(columns, name)),
    }
}

/// The key a row key names.
pub fn item_key(key: &RowKey, keys: &KeySchema) -> Result<Item, AppError> {
    let RowKey::Pk(map) = key else {
        return Err(AppError::Unsupported(
            "DynamoDB items are addressed by their key attributes".into(),
        ));
    };
    let mut out = Item::new();
    for name in keys.names() {
        let ty = keys.type_of(name);
        let v = map.get(name).ok_or_else(|| {
            AppError::InvalidInput(format!("the item's key attribute {name} is missing"))
        })?;
        match from_value(v, ty)? {
            Some(a) if Some(a.ty()) == ty => {
                out.insert(name.to_string(), a);
            }
            _ => {
                return Err(AppError::InvalidInput(format!(
                    "the key attribute {name} must be of type {}",
                    ty.map_or("S", AttrType::as_str)
                )));
            }
        }
    }
    Ok(out)
}

fn key_text(key: &Item) -> String {
    map_json(key).to_string()
}

/// `attribute_exists` (or `attribute_not_exists`) of the partition key: the item is there (or
/// not).
fn exists(expr: &mut Expr, keys: &KeySchema, there: bool) -> String {
    let n = expr.name(&keys.hash.0);
    if there {
        format!("attribute_exists({n})")
    } else {
        format!("attribute_not_exists({n})")
    }
}

/// `#a = :old` for each attribute, `attribute_type(#a, "NULL")` for a NULL, or
/// `attribute_not_exists(#a)` when it had none.
fn unchanged(expr: &mut Expr, loaded: &Item, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .map(|name| {
            let n = expr.name(name);
            match loaded.get(*name) {
                // A NULL is checked by its type rather than compared.
                Some(Attr::Null) => {
                    let t = expr.value(Attr::S("NULL".into()));
                    format!("attribute_type({n}, {t})")
                }
                Some(old) => {
                    let p = expr.value(old.clone());
                    format!("{n} = {p}")
                }
                None => format!("attribute_not_exists({n})"),
            }
        })
        .collect()
}

fn body_with(table: &str, fields: Vec<(&str, J)>, expr: &Expr) -> J {
    let mut body = Map::new();
    body.insert("TableName".into(), json!(table));
    for (k, v) in fields {
        body.insert(k.to_string(), v);
    }
    expr.apply(&mut body);
    J::Object(body)
}

/// `PutItem` of a new item: no item with its key may exist.
pub fn put_new(table: &str, keys: &KeySchema, item: &Item) -> TxItem {
    let mut expr = Expr::default();
    let cond = exists(&mut expr, keys, false);
    TxItem {
        kind: "Put",
        body: body_with(
            table,
            vec![
                ("Item", map_json(item)),
                ("ConditionExpression", json!(cond)),
            ],
            &expr,
        ),
        key: keys.key_of(item).map(|k| key_text(&k)).unwrap_or_default(),
    }
}

/// `DeleteItem`: the item must exist and, with `loaded`, still hold what was loaded of
/// `guarded`.
pub fn delete(
    table: &str,
    keys: &KeySchema,
    key: &Item,
    loaded: Option<&Item>,
    guarded: &[&str],
) -> TxItem {
    let mut expr = Expr::default();
    let mut conds = vec![exists(&mut expr, keys, true)];
    if let Some(l) = loaded {
        conds.extend(unchanged(&mut expr, l, guarded));
    }
    TxItem {
        kind: "Delete",
        body: body_with(
            table,
            vec![
                ("Key", map_json(key)),
                ("ConditionExpression", json!(conds.join(" AND "))),
            ],
            &expr,
        ),
        key: key_text(key),
    }
}

/// `UpdateItem` setting (`Some`) or removing (`None`) attributes: the item must exist and, with
/// `loaded`, each changed attribute must still hold what was loaded.
pub fn update(
    table: &str,
    keys: &KeySchema,
    key: &Item,
    sets: &[(String, Option<Attr>)],
    loaded: Option<&Item>,
) -> TxItem {
    let mut expr = Expr::default();
    let mut set = Vec::new();
    let mut remove = Vec::new();
    for (name, value) in sets {
        let n = expr.name(name);
        match value {
            Some(a) => {
                let p = expr.value(a.clone());
                set.push(format!("{n} = {p}"));
            }
            None => remove.push(n),
        }
    }
    let mut update_expr = Vec::new();
    if !set.is_empty() {
        update_expr.push(format!("SET {}", set.join(", ")));
    }
    if !remove.is_empty() {
        update_expr.push(format!("REMOVE {}", remove.join(", ")));
    }
    let mut conds = vec![exists(&mut expr, keys, true)];
    if let Some(l) = loaded {
        let names: Vec<&str> = sets.iter().map(|(n, _)| n.as_str()).collect();
        conds.extend(unchanged(&mut expr, l, &names));
    }
    TxItem {
        kind: "Update",
        body: body_with(
            table,
            vec![
                ("Key", map_json(key)),
                ("UpdateExpression", json!(update_expr.join(" "))),
                ("ConditionExpression", json!(conds.join(" AND "))),
            ],
            &expr,
        ),
        key: key_text(key),
    }
}

/// Plans the grid's staged changes on `table`. `loaded` holds the items the grid showed: the
/// types they had decide how values are written, and their values guard the writes.
pub fn plan_changes(
    table: &str,
    keys: &KeySchema,
    columns: &[ColumnMeta],
    loaded: &LoadedItems,
    changes: &[StagedChange],
    token: &str,
) -> Result<EditPlan, AppError> {
    let token: String = token.chars().take(28).collect();
    let mut ops = Vec::with_capacity(changes.len());
    for (i, change) in changes.iter().enumerate() {
        let items = match change {
            StagedChange::Update {
                key,
                changes,
                exprs,
                ..
            } => {
                if exprs.as_ref().is_some_and(|e| !e.is_empty()) {
                    return Err(AppError::Unsupported(
                        "DynamoDB has no server expressions".into(),
                    ));
                }
                let key = item_key(key, keys)?;
                plan_update(table, keys, columns, loaded.get(&key), &key, changes)?
            }
            StagedChange::Insert { values, exprs, .. } => {
                if exprs.as_ref().is_some_and(|e| !e.is_empty()) {
                    return Err(AppError::Unsupported(
                        "DynamoDB has no server expressions".into(),
                    ));
                }
                let mut item = Item::new();
                for (name, v) in values {
                    if let Some(a) = from_value(v, hint(None, keys, columns, name))? {
                        item.insert(name.clone(), a);
                    }
                }
                check_key(keys, &item)?;
                vec![put_new(table, keys, &item)]
            }
            StagedChange::Delete { key, .. } => {
                let key = item_key(key, keys)?;
                vec![delete(table, keys, &key, None, &[])]
            }
        };
        ops.push(WriteOp {
            change: i,
            row_id: change.row_id().to_string(),
            op: change.op_name(),
            items,
            token: format!("{token}-c{i}"),
        });
    }
    Ok(EditPlan { ops, token })
}

/// An item's key attributes are there, of the key's types.
fn check_key(keys: &KeySchema, item: &Item) -> Result<(), AppError> {
    for name in keys.names() {
        match (item.get(name), keys.type_of(name)) {
            (Some(a), Some(t)) if a.ty() == t => {}
            (Some(a), Some(t)) => {
                return Err(AppError::InvalidInput(format!(
                    "the key attribute {name} must be of type {}, not {}",
                    t.as_str(),
                    a.ty().as_str()
                )));
            }
            _ => {
                return Err(AppError::InvalidInput(format!(
                    "a new item needs its key attribute {name}"
                )));
            }
        }
    }
    Ok(())
}

fn plan_update(
    table: &str,
    keys: &KeySchema,
    columns: &[ColumnMeta],
    loaded: Option<&Item>,
    key: &Item,
    changes: &BTreeMap<String, Value>,
) -> Result<Vec<TxItem>, AppError> {
    let mut sets: Vec<(String, Option<Attr>)> = Vec::new();
    let mut new_key = key.clone();
    for (name, v) in changes {
        let a = from_value(v, hint(loaded, keys, columns, name))?;
        if keys.type_of(name).is_some() {
            match a {
                Some(a) if Some(a.ty()) == keys.type_of(name) => {
                    new_key.insert(name.clone(), a);
                }
                _ => {
                    return Err(AppError::InvalidInput(format!(
                        "the key attribute {name} must keep a {} value",
                        keys.type_of(name).map_or("S", AttrType::as_str)
                    )));
                }
            }
            continue;
        }
        sets.push((name.clone(), a));
    }
    if new_key == *key {
        if sets.is_empty() {
            return Err(AppError::InvalidInput(
                "the change sets no attribute".into(),
            ));
        }
        return Ok(vec![update(table, keys, key, &sets, loaded)]);
    }
    // A new key: the item is deleted and put again under it, in one transaction, guarded by
    // every attribute it was loaded with.
    let Some(old) = loaded else {
        return Err(AppError::InvalidInput(
            "a key attribute can only change on an item loaded in the grid: reload the page".into(),
        ));
    };
    let mut item = old.clone();
    item.extend(new_key.clone());
    for (name, a) in sets {
        match a {
            Some(a) => {
                item.insert(name, a);
            }
            None => {
                item.remove(&name);
            }
        }
    }
    let guarded: Vec<&str> = old
        .keys()
        .map(String::as_str)
        .filter(|n| keys.type_of(n).is_none())
        .collect();
    Ok(vec![
        delete(table, keys, key, Some(old), &guarded),
        put_new(table, keys, &item),
    ])
}

/// Two attribute maps' JSON, for messages and tests.
pub fn item_text(item: &Item) -> String {
    map_json(item).to_string()
}

/// The item of a reply's `Item` field, if any.
pub fn reply_item(reply: &J) -> Result<Option<Item>, AppError> {
    match reply.get("Item") {
        Some(i) => item_from_json(i).map(Some),
        None => Ok(None),
    }
}

/// The items of a reply's `Items` field (Scan, Query, ExecuteStatement).
pub fn reply_items(reply: &J) -> Result<Vec<Item>, AppError> {
    match reply.get("Items") {
        Some(J::Array(items)) => items.iter().map(item_from_json).collect(),
        Some(other) => Err(AppError::db(format!(
            "unexpected Items in a DynamoDB reply: {other}"
        ))),
        None => Ok(Vec::new()),
    }
}

/// The `LastEvaluatedKey` of a reply, if any.
pub fn reply_last_key(reply: &J) -> Result<Option<Item>, AppError> {
    match reply.get("LastEvaluatedKey") {
        Some(J::Null) | None => Ok(None),
        Some(k) => {
            let item = item_from_json(k)?;
            Ok((!item.is_empty()).then_some(item))
        }
    }
}

/// `true` when two attributes are equal as DynamoDB compares them (sets in any order): the fake
/// server's conditions.
#[cfg(test)]
pub fn same_attr(a: &Attr, b: &Attr) -> bool {
    fn sorted<T: Ord + Clone>(v: &[T]) -> Vec<T> {
        let mut s = v.to_vec();
        s.sort();
        s
    }
    match (a, b) {
        (Attr::Ss(x), Attr::Ss(y)) => sorted(x) == sorted(y),
        (Attr::Ns(x), Attr::Ns(y)) => sorted(x) == sorted(y),
        (Attr::Bs(x), Attr::Bs(y)) => sorted(x) == sorted(y),
        (Attr::M(x), Attr::M(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_attr(v, w)))
        }
        (Attr::L(x), Attr::L(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same_attr(v, w))
        }
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::schema::ValueKind;

    fn col(name: &str, data_type: &str) -> ColumnMeta {
        serde_json::from_value(json!({
            "name": name, "ordinal": 1, "dataType": data_type, "kind": "text",
            "nullable": true, "isPk": false, "isAutoIncrement": false, "isGenerated": false,
            "isUnsigned": false
        }))
        .unwrap()
    }

    fn keys() -> KeySchema {
        KeySchema {
            hash: ("pk".into(), AttrType::S),
            range: Some(("sk".into(), AttrType::N)),
        }
    }

    fn columns() -> Vec<ColumnMeta> {
        vec![
            col("pk", "S"),
            col("sk", "N"),
            col("name", "S"),
            col("age", "N|S"),
            col("tags", "SS"),
        ]
    }

    fn cond(column: &str, op: FilterOp, values: Vec<Value>) -> FilterNode {
        FilterNode::Condition {
            column: column.into(),
            op,
            values,
        }
    }

    fn and(items: Vec<FilterNode>) -> FilterGroup {
        FilterGroup {
            conjunction: Conjunction::And,
            items,
        }
    }

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    // --- console -------------------------------------------------------------------------

    #[test]
    fn every_action_of_the_reference_is_classified() {
        assert_eq!(READ_ACTIONS.len() + OTHER_ACTIONS.len(), 58);
        for a in READ_ACTIONS {
            assert!(!OTHER_ACTIONS.contains(a), "{a}");
            assert_eq!(api_class(a, &json!({})), StatementClass::Read, "{a}");
        }
        for a in OTHER_ACTIONS {
            assert_eq!(api_class(a, &json!({})), StatementClass::Write, "{a}");
        }
        for unknown in ["scan", "SCAN", "FlushTable", "", "Scan2", "GetRecords"] {
            assert_eq!(
                api_class(unknown, &json!({})),
                StatementClass::Write,
                "{unknown}"
            );
        }
    }

    #[test]
    fn partiql_reads_only_select() {
        for read in [
            "SELECT * FROM \"t\"",
            "select a, b from t where pk = 'x'",
            "  \n-- note\n/* c */ SeLeCt * FROM t",
            "SELECT\t* FROM \"t\".\"idx\"",
            "SELECT * FROM t WHERE a = 'DELETE FROM t'",
            "SELECT(1) FROM t",
            "SELECT * FROM t;",
            "SELECT * FROM t ; -- end\n;",
            "SELECT * FROM t WHERE a = 'x;y'",
            // Line ends every lexer agrees on.
            "-- note\r\nSELECT * FROM t -- x\r\n",
            "SELECT * FROM t -- x\r",
            "SELECT * FROM t -- x\r  \t\n;",
            "SELECT * FROM t -- a\u{2028}  \n",
            "SELECT * FROM t WHERE a = 'C:\\path' AND b = 'it''s' AND \"c\\d\" = 1",
            "SELECT * FROM t /* a -- b ' \" ` */",
        ] {
            assert_eq!(partiql_class(read), StatementClass::Read, "{read:?}");
        }
        for write in [
            "INSERT INTO t VALUE {'pk': 'a'}",
            "UPDATE t SET a = 1 WHERE pk = 'x'",
            "DELETE FROM t WHERE pk = 'x'",
            "EXISTS(SELECT * FROM t WHERE pk = 'x')",
            "/* SELECT */ DELETE FROM t WHERE pk = 'x'",
            "-- SELECT\nUPDATE t SET a = 1 WHERE pk = 'x'",
            "SELECTX * FROM t",
            "SELECT_ * FROM t",
            "SELECT$ * FROM t",
            "\u{17f}elect * from t",
            "(SELECT * FROM t)",
            "",
            "   ",
            "-- only a comment",
            "REPLACE INTO t VALUE {}",
            "\u{feff}SELECT * FROM t",
            "SELECT * FROM t; DELETE FROM t WHERE pk = 'a'",
            "SELECT * FROM t WHERE a = 'x';;\nUPDATE t SET b = 1 WHERE pk = 'a'",
            "SELECT * FROM t -- x\n; INSERT INTO t VALUE {'pk': 'b'}",
            // PartiQL ends a `--` comment at `\r` (PartiQLTokens.g4); text after a lone `\r`
            // or another line break is a statement to one lexer and a comment to another.
            "-- x\rDELETE FROM \"t\" WHERE \"pk\" = 'a' /*\nSELECT * FROM \"t\" */",
            "-- x\rSELECT * FROM \"t\" /*\nDELETE FROM \"t\" WHERE \"pk\" = 'a' --*/",
            "SELECT * FROM t --x\r; DELETE FROM t WHERE pk = 'a'",
            "SELECT * FROM t --x\r DELETE FROM t WHERE pk = 'a'",
            "-- x\u{2028}SELECT * FROM t /*\nDELETE FROM t WHERE pk = 'a' --*/",
            "-- x\u{85}SELECT * FROM t /*\nDELETE FROM t WHERE pk = 'a' --*/",
            "-- x\u{b}SELECT * FROM t /*\nDELETE FROM t WHERE pk = 'a' --*/",
            "-- x\u{c}SELECT * FROM t /*\nDELETE FROM t WHERE pk = 'a' --*/",
            "-- x\u{2029}SELECT * FROM t /*\nDELETE FROM t WHERE pk = 'a' --*/",
            "SELECT * FROM t -- x\u{2028}; DELETE FROM t WHERE pk = 'a'",
            "-- x\u{1c}DELETE FROM t WHERE pk = 'a' --\nSELECT * FROM t",
            "SELECT * FROM t -- x\u{1d}; DELETE FROM t WHERE pk = 'a'",
            "SELECT * FROM t -- x\u{1e}; DELETE FROM t WHERE pk = 'a'",
            // Nested block comments (SQL nests them, PartiQL does not).
            "/* /* */ SELECT 1 */ DELETE FROM t WHERE pk = 'a'",
            "SELECT * FROM t /* /* */ ; DELETE FROM t WHERE pk = 'a' -- */",
            // Ion literals: PartiQL's lexers disagree on a backtick inside their text.
            "SELECT * FROM t WHERE a = `1 // '\n`; DELETE FROM t WHERE pk = 'a' --'",
            "SELECT * FROM t WHERE a = `{b: 1}`",
            // A backslash before a quote is an escape to some lexers.
            "SELECT * FROM t WHERE a = '\\' /* '; DELETE FROM t WHERE pk = 'a' -- */",
            "SELECT * FROM \"t\\\" /* \"; DELETE FROM t WHERE pk = 'a' -- */",
            // Unterminated text.
            "SELECT * FROM t /* ; DELETE FROM t WHERE pk = 'a'",
            "SELECT * FROM t WHERE a = '; DELETE FROM t WHERE pk = 'a'",
            "SELECT * FROM \"t; DELETE FROM t WHERE pk = 'a'",
            // Always lexed as PartiQL, never as an API command's JSON (`SELECT {` looks like one).
            "SELECT {'a': '\"'} FROM t; DELETE FROM t WHERE pk = 'a' --\"}",
            "SELECT {'a': \"\\\"} FROM t; DELETE FROM t WHERE pk = 'a' --\"}",
        ] {
            assert_eq!(partiql_class(write), StatementClass::Write, "{write:?}");
        }
    }

    /// A PartiQL lexer with the choices real lexers differ on: what ends a `--` comment,
    /// whether block comments nest, whether a backslash escapes a quote. The first word of
    /// each statement, or `None` when the text does not lex (the server runs nothing).
    fn variant_words(
        text: &str,
        breaks: &[char],
        nest: bool,
        backslash: bool,
    ) -> Option<Vec<String>> {
        let c: Vec<char> = text.chars().collect();
        let mut words = Vec::new();
        let mut first: Option<String> = None;
        let mut i = 0;
        while i < c.len() {
            let ch = c[i];
            if ch == '-' && c.get(i + 1) == Some(&'-') {
                i += 2;
                while i < c.len() && !breaks.contains(&c[i]) {
                    i += 1;
                }
                continue;
            }
            if ch == '/' && c.get(i + 1) == Some(&'*') {
                let mut depth = 1;
                i += 2;
                while depth > 0 {
                    match (c.get(i), c.get(i + 1)) {
                        (None, _) => return None,
                        (Some('*'), Some('/')) => {
                            depth -= 1;
                            i += 2;
                        }
                        (Some('/'), Some('*')) if nest => {
                            depth += 1;
                            i += 2;
                        }
                        _ => i += 1,
                    }
                }
                continue;
            }
            // A lexer that ends a comment at a break skips the break like a space.
            if ch.is_whitespace() || breaks.contains(&ch) {
                i += 1;
                continue;
            }
            if ch == ';' {
                words.extend(first.take());
                i += 1;
                continue;
            }
            let start = i;
            match ch {
                '\'' | '"' | '`' => {
                    i += 1;
                    loop {
                        match c.get(i) {
                            None => return None,
                            Some('\\') if backslash && ch != '`' => i += 2,
                            Some(q) if *q == ch => {
                                if ch != '`' && c.get(i + 1) == Some(&ch) {
                                    i += 2;
                                } else {
                                    i += 1;
                                    break;
                                }
                            }
                            _ => i += 1,
                        }
                    }
                }
                w if is_word_char(w) || w == '$' => {
                    while i < c.len() && (is_word_char(c[i]) || c[i] == '$') {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
            if first.is_none() {
                first = Some(c[start..i].iter().collect());
            }
        }
        words.extend(first);
        Some(words)
    }

    /// Random statements over the lexers' disputed text (fixed seed): one read as a read is a
    /// single `SELECT` to every lexer variant (or does not lex there).
    #[test]
    fn partiql_reads_are_reads_to_every_lexer() {
        let atoms = [
            "SELECT", "DELETE", " ", "\n", "\r", "\r\n", "\u{2028}", "\u{85}", "\u{1e}", ";", "'",
            "\"", "''", "--", "/*", "*/", "`", "\\", "x", "*", "FROM t", "{", "}",
        ];
        let all_breaks = [
            '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}',
            '\u{2029}',
        ];
        let breaks: [&[char]; 3] = [&['\n'], &['\n', '\r'], &all_breaks];
        let mut seed: u64 = 0xd1a0;
        let mut next = || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as usize
        };
        let mut reads = 0;
        for n in 0..40_000 {
            let len = next() % 10 + 1;
            let mut text: String = if n % 2 == 0 {
                "SELECT * ".into()
            } else {
                String::new()
            };
            text.extend((0..len).map(|_| atoms[next() % atoms.len()]));
            if partiql_class(&text) != StatementClass::Read {
                continue;
            }
            reads += 1;
            for b in breaks {
                for nest in [false, true] {
                    for backslash in [false, true] {
                        if let Some(words) = variant_words(&text, b, nest, backslash) {
                            assert!(
                                words.len() == 1 && words[0].eq_ignore_ascii_case("SELECT"),
                                "{text:?} reads as {words:?} (breaks {b:?}, nest {nest}, backslash {backslash})"
                            );
                        }
                    }
                }
            }
        }
        assert!(reads > 1000, "{reads}");
    }

    #[test]
    fn console_splitting() {
        let text = "SELECT * FROM t WHERE a = 'x;y' AND \"we;ird\" = 1;\n\
                    -- comment; with a semicolon\n\
                    Scan {\"TableName\": \"t;1\", \"FilterExpression\": \"a = :v\", \"x\": \"q\\\";\"};\
                    ;;  /* only; a comment */ ;\n\
                    DescribeLimits;\n\
                    UPDATE \"t\" SET a = 'it''s;' WHERE pk = 'k'";
        let parts: Vec<String> = split_console(text).into_iter().map(|s| s.sql).collect();
        assert_eq!(
            parts,
            [
                "SELECT * FROM t WHERE a = 'x;y' AND \"we;ird\" = 1",
                "Scan {\"TableName\": \"t;1\", \"FilterExpression\": \"a = :v\", \"x\": \"q\\\";\"}",
                "DescribeLimits",
                "UPDATE \"t\" SET a = 'it''s;' WHERE pk = 'k'",
            ]
        );
        let s = split_console("\n\n  SELECT 1;\nSELECT ü");
        assert_eq!((s[0].start, s[0].end, s[0].line), (4, 12, 3));
        assert_eq!((s[1].start, s[1].end, s[1].line), (14, 22, 4));
        assert!(split_console("  -- x\n /* y */ ;; ").is_empty());
        // A comment inside JSON is text, not a comment.
        assert_eq!(split_console("Scan {\"TableName\": \"a--b\"}").len(), 1);
        // Splitting loses nothing between the commands.
        for t in [
            text,
            "a;b",
            "'unclosed;",
            "\"unclosed;",
            "Scan {\"x\": \"un;closed",
        ] {
            for st in split_console(t) {
                let chars: Vec<char> = t.chars().collect();
                assert_eq!(st.sql, chars[st.start..st.end].iter().collect::<String>());
            }
        }
    }

    /// Long texts lex and split in linear time: comments ended by lone `\r`s (each once looked
    /// ahead to the next `\n`) and many commands (each once counted its line from the top).
    #[test]
    fn long_texts_stay_linear() {
        let started = std::time::Instant::now();
        let crs = "--a\r".repeat(50_000);
        assert_eq!(classify_console(&crs), StatementClass::Write);
        assert!(split_console(&crs).is_empty());
        let statement = format!("SELECT * FROM t {crs}");
        assert_eq!(partiql_class(&statement), StatementClass::Write);
        let many = "SELECT * FROM t;\n".repeat(50_000);
        let parts = split_console(&many);
        assert_eq!(parts.len(), 50_000);
        assert_eq!(parts[49_999].line, 50_000);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
        // A comment that a lone `\r` (or another break) ends early, before blanks only.
        for clear in [
            "-- x\r  \t\nSELECT 1",
            "-- x\u{2028} \nSELECT 1",
            "SELECT 1 -- x\r",
        ] {
            assert_eq!(partiql_class(clear), StatementClass::Read, "{clear:?}");
        }
        for unclear in [
            "-- x\r y\nSELECT 1",
            "-- x\u{1e}y\nSELECT 1",
            "-- x\u{85}\ry\nSELECT 1",
        ] {
            assert_eq!(partiql_class(unclear), StatementClass::Write, "{unclear:?}");
        }
    }

    #[test]
    fn insert_and_update_change_one_item_and_delete_at_most_one() {
        for w in [
            "INSERT INTO \"t\" VALUE {'pk': 'a'}",
            " -- c\n update \"t\" SET a = 1 WHERE pk = 'a'",
        ] {
            assert_eq!(partiql_write(w), PartiqlWrite::One, "{w}");
        }
        assert_eq!(
            partiql_write("/* x */DELETE FROM \"t\" WHERE pk = 'a'"),
            PartiqlWrite::AtMostOne
        );
        // A DELETE of a missing key succeeds: only the item it returned proves one was deleted.
        assert_eq!(PartiqlWrite::AtMostOne.affected(0), None);
        assert_eq!(PartiqlWrite::AtMostOne.affected(1), Some(1));
        assert_eq!(PartiqlWrite::One.affected(0), Some(1));
        assert_eq!(PartiqlWrite::Nothing.affected(3), None);
        // A SELECT the classifier calls a write (lexers could read it differently) still reads.
        for r in [
            "SELECT * FROM t -- x\r DELETE",
            "SELECT * FROM \"t\" WHERE a = `1`",
            "EXISTS(SELECT * FROM t)",
            "INSERTX INTO t",
            "insert_ INTO t",
            "delete$ FROM t",
            "",
        ] {
            assert_eq!(partiql_write(r), PartiqlWrite::Nothing, "{r}");
        }
        assert_eq!(
            partiql_class("SELECT * FROM t -- x\r DELETE"),
            StatementClass::Write
        );
    }

    /// Random texts over the console's vocabulary (fixed seed): nothing panics, splitting keeps
    /// each command's text, and a text read as a read holds only reads.
    #[test]
    fn console_fuzz() {
        let atoms = [
            "SELECT",
            "select",
            "DELETE",
            "UPDATE",
            "EXISTS",
            " ",
            "\n",
            "\r",
            "\u{2028}",
            "`",
            ";",
            "'",
            "\"",
            "--",
            "/*",
            "*/",
            "{",
            "}",
            "[",
            "]",
            "\\",
            ":",
            ",",
            "x",
            "\u{e9}",
            "\u{17f}",
            "Scan",
            "PutItem",
            "ExecuteStatement",
            "\"Statement\"",
            "\"\\u0053tatement\"",
            "*",
            "FROM t",
        ];
        let mut seed: u64 = 0x5eed;
        let mut next = || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as usize
        };
        for _ in 0..5000 {
            let n = next() % 14 + 1;
            let text: String = (0..n).map(|_| atoms[next() % atoms.len()]).collect();
            let chars: Vec<char> = text.chars().collect();
            let parts = split_console(&text);
            for p in &parts {
                assert_eq!(p.sql, chars[p.start..p.end].iter().collect::<String>());
                assert!(!p.sql.trim().is_empty(), "{text:?}");
            }
            if classify_console(&text) == StatementClass::Read {
                for p in &parts {
                    match parse_command(&p.sql).unwrap() {
                        ConsoleCommand::Partiql(s) => {
                            let first = s.trim_start().get(..6).unwrap_or_default();
                            assert!(first.eq_ignore_ascii_case("SELECT"), "{text:?}");
                        }
                        ConsoleCommand::Api { action, body } => {
                            assert_eq!(api_class(&action, &body), StatementClass::Read, "{text:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn console_commands_and_their_class() {
        assert_eq!(
            parse_command("Scan {\"TableName\": \"t\"};").unwrap(),
            ConsoleCommand::Api {
                action: "Scan".into(),
                body: json!({"TableName": "t"})
            }
        );
        assert_eq!(
            parse_command("DescribeLimits").unwrap(),
            ConsoleCommand::Api {
                action: "DescribeLimits".into(),
                body: json!({})
            }
        );
        assert_eq!(
            parse_command("-- x\nSELECT * FROM t;").unwrap(),
            ConsoleCommand::Partiql("SELECT * FROM t".into())
        );
        assert!(parse_command("Scan {\"TableName\": \"a\", \"TableName\": \"b\"}").is_err());
        // Not an API command (no object follows): a PartiQL statement, and a write.
        assert_eq!(
            parse_command("Scan [1]").unwrap(),
            ConsoleCommand::Partiql("Scan [1]".into())
        );
        assert_eq!(classify_console("Scan [1]"), StatementClass::Write);
        assert!(parse_command("Scan {\"a\": 1} trailing").is_err());
        assert!(
            parse_command(&format!(
                "SELECT '{}' FROM t",
                "x".repeat(MAX_STATEMENT_CHARS)
            ))
            .is_err()
        );

        for read in [
            "SELECT * FROM t",
            "Scan {\"TableName\": \"t\"}",
            "Query {\"TableName\": \"t\", \"KeyConditionExpression\": \"#k = :v\"}",
            "ExecuteStatement {\"Statement\": \"SELECT * FROM t\"}",
            "BatchExecuteStatement {\"Statements\": [{\"Statement\": \"SELECT * FROM t WHERE pk='a'\"}]}",
            "ExecuteTransaction {\"TransactStatements\": [{\"Statement\": \"SELECT * FROM t WHERE pk='a'\"}]}",
            "SELECT * FROM t; Scan {\"TableName\": \"t\"}",
            "ListTables",
        ] {
            assert_eq!(classify_console(read), StatementClass::Read, "{read}");
        }
        for write in [
            "SELECT * FROM t; DELETE FROM t WHERE pk = 'a'",
            "PutItem {\"TableName\": \"t\", \"Item\": {}}",
            "ExecuteStatement {\"Statement\": \"DELETE FROM t WHERE pk = 'a'\"}",
            "ExecuteStatement {}",
            "ExecuteStatement {\"Statement\": 1}",
            "BatchExecuteStatement {\"Statements\": []}",
            "BatchExecuteStatement {\"Statements\": [{\"Statement\": \"SELECT * FROM t\"}, {\"Statement\": \"UPDATE t SET a=1 WHERE pk='a'\"}]}",
            "ExecuteTransaction {\"TransactStatements\": [{\"Statement\": \"EXISTS(SELECT * FROM t WHERE pk='a')\"}]}",
            "ExecuteStatement {\"Statement\": \"SELECT * FROM t\", \"\\u0053tatement\": \"DELETE FROM t\"}",
            "Scan {\"TableName\": ",
            "scan {\"TableName\": \"t\"}",
            "Unknown",
            "",
            "-- nothing",
            "DeleteTable {\"TableName\": \"t\"}",
            "UpdateTimeToLive {}",
            "ExportTableToPointInTime {}",
            "ExecuteStatement {\"Statement\": \"SELECT * FROM t; DELETE FROM t WHERE pk = 'a'\"}",
            "ExecuteStatement {\"Statement\": \"SELECT * FROM t\", \"Statement\": \"DELETE FROM t\"}",
            // Statements in a request are sent as written: a `\r` (a JSON escape is enough)
            // ends a PartiQL `--` comment.
            "ExecuteStatement {\"Statement\": \"-- x\\rDELETE FROM \\\"t\\\" WHERE \\\"pk\\\" = 'a' /*\\nSELECT * FROM \\\"t\\\" */\"}",
            "BatchExecuteStatement {\"Statements\": [{\"Statement\": \"-- x\\rDELETE FROM \\\"t\\\" WHERE \\\"pk\\\" = 'a' /*\\nSELECT * FROM \\\"t\\\" */\"}]}",
            "ExecuteTransaction {\"TransactStatements\": [{\"Statement\": \"-- x\\rDELETE FROM \\\"t\\\" WHERE \\\"pk\\\" = 'a' /*\\nSELECT * FROM \\\"t\\\" */\"}]}",
            "ExecuteStatement {\"Statement\": \"SELECT * FROM t --x\\r; DELETE FROM t WHERE pk = 'a'\"}",
            "BatchExecuteStatement {\"Statements\": [{\"Statement\": \"SELECT * FROM t --x\\r; DELETE FROM t WHERE pk = 'a'\"}]}",
            "SELECT * FROM t --x\r; DELETE FROM t WHERE pk = 'a'",
            "-- x\rDELETE FROM \"t\" WHERE \"pk\" = 'a' /*\nSELECT * FROM \"t\" */",
            // Members the reference does not name, in any case.
            "ExecuteStatement {\"Statement\": \"SELECT * FROM t\", \"statement\": \"DELETE FROM t WHERE pk='a'\"}",
            "ExecuteStatement {\"Statement\": \"SELECT * FROM t\", \"STATEMENT\": \"DELETE FROM t WHERE pk='a'\"}",
            "ExecuteStatement {\"Statement\": \"SELECT * FROM t\", \"Statements\": [{\"Statement\": \"DELETE FROM t\"}]}",
            "BatchExecuteStatement {\"Statements\": [{\"Statement\": \"SELECT * FROM t\", \"statement\": \"DELETE FROM t\"}]}",
            "BatchExecuteStatement {\"Statements\": [{\"Statement\": \"SELECT * FROM t\"}], \"statements\": [{\"Statement\": \"DELETE FROM t\"}]}",
            "ExecuteTransaction {\"TransactStatements\": [{\"Statement\": \"SELECT * FROM t\"}], \"transactStatements\": [{\"Statement\": \"DELETE FROM t\"}]}",
            "ExecuteTransaction {\"TransactStatements\": [{\"Statement\": \"SELECT * FROM t\", \"ConsistentRead\": true}]}",
            "ExecuteStatement {\"Statement\": \"SELECT * FROM t\", \"Unknown\": 1}",
            "ExecuteStatement [\"SELECT * FROM t\"]",
            "BatchExecuteStatement {\"Statements\": [\"SELECT * FROM t\"]}",
        ] {
            assert_eq!(classify_console(write), StatementClass::Write, "{write:?}");
        }
        // Every member the reference names, with a read.
        for read in [
            "ExecuteStatement {\"Statement\": \"SELECT * FROM t WHERE pk = ?\", \"ConsistentRead\": true, \"Limit\": 5, \"NextToken\": \"n\", \"Parameters\": [{\"S\": \"a\"}], \"ReturnConsumedCapacity\": \"TOTAL\", \"ReturnValuesOnConditionCheckFailure\": \"NONE\"}",
            "BatchExecuteStatement {\"ReturnConsumedCapacity\": \"TOTAL\", \"Statements\": [{\"Statement\": \"SELECT * FROM t WHERE pk = ?\", \"ConsistentRead\": true, \"Parameters\": [{\"S\": \"a\"}], \"ReturnValuesOnConditionCheckFailure\": \"NONE\"}]}",
            "ExecuteTransaction {\"ClientRequestToken\": \"c\", \"ReturnConsumedCapacity\": \"TOTAL\", \"TransactStatements\": [{\"Statement\": \"SELECT * FROM t WHERE pk = ?\", \"Parameters\": [{\"S\": \"a\"}], \"ReturnValuesOnConditionCheckFailure\": \"NONE\"}]}",
            "ExecuteStatement {\"Statement\": \"-- note\\r\\nSELECT * FROM t\"}",
        ] {
            assert_eq!(classify_console(read), StatementClass::Read, "{read:?}");
        }
        // The path Phase B's calls take: the request itself.
        let hidden = "-- x\rDELETE FROM \"t\" WHERE \"pk\" = 'a' /*\nSELECT * FROM \"t\" */";
        let second = "SELECT * FROM t --x\r; DELETE FROM t WHERE pk = 'a'";
        for s in [hidden, second] {
            for (action, body) in [
                ("ExecuteStatement", json!({ "Statement": s })),
                (
                    "BatchExecuteStatement",
                    json!({ "Statements": [{ "Statement": s }] }),
                ),
                (
                    "ExecuteTransaction",
                    json!({ "TransactStatements": [{ "Statement": s }] }),
                ),
            ] {
                assert_eq!(
                    api_class(action, &body),
                    StatementClass::Write,
                    "{action} {s:?}"
                );
            }
        }
        assert_eq!(
            api_class(
                "ExecuteStatement",
                &json!({"Statement": "SELECT * FROM t", "statement": "DELETE FROM t WHERE pk='a'"})
            ),
            StatementClass::Write
        );
        assert_eq!(
            command_request(&ConsoleCommand::Partiql("SELECT 1".into()), Some("tok")),
            (
                "ExecuteStatement".to_string(),
                json!({"Statement": "SELECT 1", "NextToken": "tok"})
            )
        );
    }

    /// The cases the renderer's mirror of the splitter and classifier runs too
    /// (`src/features/drivers/dynamodb/classify.test.ts`): both must agree on every one.
    #[test]
    fn shared_console_cases() {
        let cases: J = serde_json::from_str(include_str!(
            "../../../tests/fixtures/dynamodb/console.cases.json"
        ))
        .expect("the case file is JSON");
        let classify = cases["classify"].as_array().expect("classify cases");
        assert!(classify.len() > 200);
        let mut wrong = Vec::new();
        for c in classify {
            let text = c["sql"].as_str().expect("sql");
            let want = match c["class"].as_str() {
                Some("read") => StatementClass::Read,
                _ => StatementClass::Write,
            };
            if classify_console(text) != want {
                wrong.push(format!("{text:?}: want {want:?}"));
            }
        }
        for c in cases["split"].as_array().expect("split cases") {
            let text = c["sql"].as_str().expect("sql");
            let want: Vec<&str> = c["statements"]
                .as_array()
                .expect("statements")
                .iter()
                .filter_map(J::as_str)
                .collect();
            let got: Vec<String> = split_console(text).into_iter().map(|s| s.sql).collect();
            if got != want {
                wrong.push(format!("split {text:?}: {got:?}, want {want:?}"));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    // --- reads -----------------------------------------------------------------------------

    #[test]
    fn an_equality_on_the_partition_key_makes_a_query() {
        let cols = columns();
        let k = keys();
        let filter = and(vec![
            cond("name", FilterOp::Eq, vec![text("Ann")]),
            cond("pk", FilterOp::Eq, vec![text("user#1")]),
            cond("sk", FilterOp::Between, vec![text("1"), Value::Int(9)]),
        ]);
        let r = read_request(&ReadSpec {
            table: "t",
            index: None,
            keys: &k,
            columns: &cols,
            filter: Some(&filter),
            descending: true,
            consistent: true,
            consumed_capacity: true,
        })
        .unwrap();
        assert_eq!(r.action, "Query");
        assert_eq!(
            J::Object(r.body.clone()),
            json!({
                "TableName": "t",
                "KeyConditionExpression": "#n0 = :v0 AND #n1 BETWEEN :v1 AND :v2",
                "ScanIndexForward": false,
                "FilterExpression": "#n2 = :v3",
                "ConsistentRead": true,
                "ReturnConsumedCapacity": "TOTAL",
                "ExpressionAttributeNames": {"#n0": "pk", "#n1": "sk", "#n2": "name"},
                "ExpressionAttributeValues": {
                    ":v0": {"S": "user#1"}, ":v1": {"N": "1"}, ":v2": {"N": "9"}, ":v3": {"S": "Ann"}
                }
            })
        );
        let start = Item::from([("pk".to_string(), Attr::S("a".into()))]);
        let page = r.page(Some(25), Some(&start));
        assert_eq!(page["Limit"], json!(25));
        assert_eq!(page["ExclusiveStartKey"], json!({"pk": {"S": "a"}}));
        let count = r.count();
        assert_eq!(count.body["Select"], json!("COUNT"));
        assert!(r.text().starts_with("Query {\n"));
        // One sort key condition and a raw filter of other attributes, on an index.
        let f = and(vec![
            cond("pk", FilterOp::Eq, vec![text("a")]),
            cond("sk", FilterOp::Gt, vec![text("5")]),
            FilterNode::Raw {
                sql: "size(tags) > :x".into(),
            },
        ]);
        let r = read_request(&ReadSpec {
            table: "t",
            index: Some("byName"),
            keys: &k,
            columns: &cols,
            filter: Some(&f),
            descending: true,
            consistent: true,
            consumed_capacity: false,
        })
        .unwrap();
        assert_eq!(r.action, "Query");
        assert_eq!(
            r.body["KeyConditionExpression"],
            json!("#n0 = :v0 AND #n1 > :v1")
        );
        assert_eq!(r.body["FilterExpression"], json!("(size(tags) > :x)"));
        assert_eq!(r.body["ScanIndexForward"], json!(false));
        assert_eq!(r.body["IndexName"], json!("byName"));
        // The caller asked (a local index): the builder cannot tell a global one apart.
        assert_eq!(r.body["ConsistentRead"], json!(true));
    }

    /// A Query's FilterExpression cannot name a key attribute (DynamoDB refuses it): a filter
    /// whose rest would is read with a Scan of the whole filter.
    #[test]
    fn a_filter_left_naming_a_key_makes_a_scan() {
        let cols = columns();
        let k = keys();
        let spec = |f: &FilterGroup| {
            read_request(&ReadSpec {
                table: "t",
                index: None,
                keys: &k,
                columns: &cols,
                filter: Some(f),
                descending: true,
                consistent: false,
                consumed_capacity: false,
            })
            .unwrap()
        };
        let pk = || cond("pk", FilterOp::Eq, vec![text("a")]);
        // A second sort key condition.
        let r = spec(&and(vec![
            pk(),
            cond("sk", FilterOp::Gt, vec![text("5")]),
            cond("sk", FilterOp::Lt, vec![text("9")]),
        ]));
        assert_eq!(
            J::Object(r.body),
            json!({
                "TableName": "t",
                "FilterExpression": "(#n0 = :v0 AND #n1 > :v1 AND #n1 < :v2)",
                "ExpressionAttributeNames": {"#n0": "pk", "#n1": "sk"},
                "ExpressionAttributeValues": {":v0": {"S": "a"}, ":v1": {"N": "5"}, ":v2": {"N": "9"}}
            })
        );
        assert_eq!(r.action, "Scan");
        let scans = [
            // A second partition key condition, of any operator.
            and(vec![pk(), cond("pk", FilterOp::Eq, vec![text("b")])]),
            and(vec![pk(), cond("pk", FilterOp::HasPrefix, vec![text("a")])]),
            // An empty string, which DynamoDB refuses as a key condition's value.
            and(vec![cond("pk", FilterOp::Eq, vec![text("")])]),
            // A key condition DynamoDB cannot take: inside a group, or negated.
            and(vec![
                pk(),
                FilterNode::Group(FilterGroup {
                    conjunction: Conjunction::Or,
                    items: vec![cond("sk", FilterOp::Eq, vec![Value::Int(1)])],
                }),
            ]),
            and(vec![pk(), cond("sk", FilterOp::Ne, vec![Value::Int(1)])]),
            and(vec![pk(), cond("sk", FilterOp::IsNull, vec![])]),
            // Any column holds the key columns.
            and(vec![
                pk(),
                FilterNode::AnyColumn {
                    op: FilterOp::Eq,
                    values: vec![text("7")],
                },
            ]),
            // A raw expression naming a key, by name or by one of the request's placeholders.
            and(vec![
                pk(),
                FilterNode::Raw {
                    sql: "sk > :x".into(),
                },
            ]),
            and(vec![
                pk(),
                FilterNode::Raw {
                    sql: "begins_with(#n0, :x)".into(),
                },
            ]),
        ];
        for f in &scans {
            let r = spec(f);
            assert_eq!(r.action, "Scan", "{f:?}");
            assert!(!r.body.contains_key("KeyConditionExpression"), "{f:?}");
            assert!(!r.body.contains_key("ScanIndexForward"), "{f:?}");
        }
        // Words that are not keys keep the Query.
        for raw in [
            "size(#x) > :y",
            "attribute_exists(pkx)",
            "#n9 = :v9",
            "skate = :q",
        ] {
            let r = spec(&and(vec![pk(), FilterNode::Raw { sql: raw.into() }]));
            assert_eq!(r.action, "Query", "{raw}");
        }
    }

    #[test]
    fn other_filters_make_a_scan() {
        let cols = columns();
        let k = keys();
        let spec = |f: &FilterGroup| {
            read_request(&ReadSpec {
                table: "t",
                index: None,
                keys: &k,
                columns: &cols,
                filter: Some(f),
                descending: false,
                consistent: false,
                consumed_capacity: false,
            })
        };
        // OR at the top: no key condition.
        let or = FilterGroup {
            conjunction: Conjunction::Or,
            items: vec![
                cond("pk", FilterOp::Eq, vec![text("a")]),
                cond("age", FilterOp::Eq, vec![text("5")]),
            ],
        };
        let r = spec(&or).unwrap();
        assert_eq!(r.action, "Scan");
        assert_eq!(
            r.body["FilterExpression"],
            json!("(#n0 = :v0 OR (#n1 = :v1 OR #n1 = :v2))")
        );
        assert_eq!(
            r.body["ExpressionAttributeValues"],
            json!({":v0": {"S": "a"}, ":v1": {"N": "5"}, ":v2": {"S": "5"}})
        );
        let many = and(vec![
            cond("name", FilterOp::IsNull, vec![]),
            cond("name", FilterOp::IsNotNull, vec![]),
            cond("name", FilterOp::Ne, vec![text("x")]),
            cond("name", FilterOp::In, vec![text("a"), text("b")]),
            cond("name", FilterOp::NotIn, vec![text("c")]),
            cond("tags", FilterOp::Contains, vec![text("red")]),
            cond("name", FilterOp::HasPrefix, vec![text("An")]),
            cond("name", FilterOp::Like, vec![text("Bo%")]),
            cond("name", FilterOp::NotLike, vec![text("Cy")]),
            cond(
                "sk",
                FilterOp::NotBetween,
                vec![Value::Int(1), Value::Int(2)],
            ),
            FilterNode::Raw {
                sql: "size(#x) > 3".into(),
            },
            FilterNode::Group(FilterGroup {
                conjunction: Conjunction::Or,
                items: vec![cond("sk", FilterOp::Ge, vec![Value::Int(3)])],
            }),
        ]);
        let r = spec(&many).unwrap();
        assert_eq!(
            r.body["FilterExpression"],
            json!(
                "((attribute_not_exists(#n0) OR attribute_type(#n0, :v0)) AND \
                 (attribute_exists(#n0) AND NOT attribute_type(#n0, :v1)) AND \
                 NOT #n0 = :v2 AND #n0 IN (:v3, :v4) AND NOT (#n0 IN (:v5)) AND \
                 contains(#n1, :v6) AND begins_with(#n0, :v7) AND begins_with(#n0, :v8) AND \
                 NOT #n0 = :v9 AND NOT #n2 BETWEEN :v10 AND :v11 AND (size(#x) > 3) AND #n2 >= :v12)"
            )
        );
        for bad in [
            cond("name", FilterOp::HasSuffix, vec![text("x")]),
            cond("name", FilterOp::ContainsCase, vec![text("x")]),
            cond("name", FilterOp::Ilike, vec![text("x")]),
            cond("name", FilterOp::Like, vec![text("a%b")]),
            cond("name", FilterOp::Eq, vec![text("a"), text("b")]),
            cond("sk", FilterOp::Eq, vec![text("not a number")]),
            cond("name", FilterOp::Between, vec![text("a")]),
            cond("name", FilterOp::In, vec![]),
        ] {
            assert!(spec(&and(vec![bad.clone()])).is_err(), "{bad:?}");
        }
        // A partition key value of the wrong type is an error, not a scan.
        let k_n = KeySchema {
            hash: ("pk".into(), AttrType::N),
            range: None,
        };
        assert!(
            read_request(&ReadSpec {
                table: "t",
                index: None,
                keys: &k_n,
                columns: &cols,
                filter: Some(&and(vec![cond("pk", FilterOp::Eq, vec![text("x")])])),
                descending: false,
                consistent: false,
                consumed_capacity: false,
            })
            .is_err()
        );
        // Any column: the columns a value fits.
        let any = and(vec![FilterNode::AnyColumn {
            op: FilterOp::Eq,
            values: vec![text("7")],
        }]);
        let r = spec(&any).unwrap();
        assert_eq!(r.action, "Scan");
        let f = r.body["FilterExpression"].as_str().unwrap();
        assert!(f.starts_with('(') && f.contains(" OR "), "{f}");
        let none = and(vec![FilterNode::AnyColumn {
            op: FilterOp::Eq,
            values: vec![Value::Bool(true)],
        }]);
        let r = spec(&none).unwrap();
        assert!(
            r.body["FilterExpression"]
                .as_str()
                .unwrap()
                .contains("attribute_not_exists")
        );
        // No filter: a plain scan.
        let plain = read_request(&ReadSpec {
            table: "t",
            index: None,
            keys: &k,
            columns: &cols,
            filter: None,
            descending: true,
            consistent: false,
            consumed_capacity: false,
        })
        .unwrap();
        assert_eq!(J::Object(plain.body), json!({"TableName": "t"}));
    }

    #[test]
    fn cursors_round_trip() {
        let key = Item::from([
            ("pk".to_string(), Attr::S("a/b+c".into())),
            ("sk".to_string(), Attr::N("12345678901234567890".into())),
        ]);
        let c = encode_cursor(&key);
        assert!(!c.contains(['+', '/', '=']));
        assert_eq!(decode_cursor(&c).unwrap(), key);
        for bad in [
            "",
            "!!",
            "e30",
            "W10",
            &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("{\"a\":{\"X\":1}}"),
        ] {
            assert!(decode_cursor(bad).is_err(), "{bad}");
        }
        assert_eq!(
            reply_last_key(&json!({"LastEvaluatedKey": {"pk": {"S": "a"}}})).unwrap(),
            Some(Item::from([("pk".to_string(), Attr::S("a".into()))]))
        );
        assert_eq!(reply_last_key(&json!({})).unwrap(), None);
        assert_eq!(
            reply_last_key(&json!({"LastEvaluatedKey": {}})).unwrap(),
            None
        );
        assert!(reply_items(&json!({"Items": {}})).is_err());
    }

    #[test]
    fn cells_and_catalog_requests() {
        let key = Item::from([("pk".to_string(), Attr::S("a".into()))]);
        assert_eq!(
            get_attribute("t", &key, "my attr", true),
            json!({
                "TableName": "t", "Key": {"pk": {"S": "a"}}, "ProjectionExpression": "#n0",
                "ConsistentRead": true, "ExpressionAttributeNames": {"#n0": "my attr"}
            })
        );
        assert_eq!(
            list_tables(Some("b"), 500),
            json!({"Limit": 100, "ExclusiveStartTableName": "b"})
        );
        assert_eq!(list_tables(None, 0), json!({"Limit": 1}));
        assert_eq!(sample_scan("t", 0), json!({"TableName": "t", "Limit": 1}));
        assert_eq!(
            column_types("S|N| NULL |X"),
            [AttrType::S, AttrType::N, AttrType::Null]
        );
        assert_eq!(column_hint(&columns(), "age"), None);
        assert_eq!(column_hint(&[col("x", "N|NULL")], "x"), Some(AttrType::N));
        assert_eq!(AttrType::N.kind(), ValueKind::Decimal);
    }

    // --- edits -----------------------------------------------------------------------------

    fn pk(pk: &str, sk: i64) -> RowKey {
        RowKey::Pk(BTreeMap::from([
            ("pk".to_string(), text(pk)),
            ("sk".to_string(), Value::Int(sk)),
        ]))
    }

    fn loaded() -> LoadedItems {
        let mut l = LoadedItems::default();
        l.record(
            &keys(),
            &Item::from([
                ("pk".to_string(), Attr::S("a".into())),
                ("sk".to_string(), Attr::N("1".into())),
                ("zip".to_string(), Attr::S("02134".into())),
                ("n".to_string(), Attr::N("5".into())),
                ("gone".to_string(), Attr::Null),
            ]),
        );
        l.record(
            &keys(),
            &Item::from([("x".to_string(), Attr::S("no key".into()))]),
        );
        l
    }

    #[test]
    fn updates_guard_the_key_and_the_old_values() {
        let changes = vec![StagedChange::Update {
            row_id: "r1".into(),
            key: pk("a", 1),
            changes: BTreeMap::from([
                ("zip".to_string(), text("02135")),
                ("n".to_string(), text("6")),
                ("gone".to_string(), attr::missing()),
                ("new".to_string(), text("x")),
            ]),
            exprs: None,
        }];
        let plan = plan_changes("t", &keys(), &columns(), &loaded(), &changes, "tok").unwrap();
        let (action, body) = plan.ops[0].request();
        assert_eq!(action, "UpdateItem");
        assert_eq!(
            body,
            json!({
                "TableName": "t",
                "Key": {"pk": {"S": "a"}, "sk": {"N": "1"}},
                "UpdateExpression": "SET #n1 = :v0, #n2 = :v1, #n3 = :v2 REMOVE #n0",
                "ConditionExpression": "attribute_exists(#n4) AND attribute_type(#n0, :v3) AND #n1 = :v4 AND attribute_not_exists(#n2) AND #n3 = :v5",
                "ExpressionAttributeNames": {"#n0": "gone", "#n1": "n", "#n2": "new", "#n3": "zip", "#n4": "pk"},
                "ExpressionAttributeValues": {
                    ":v0": {"N": "6"}, ":v1": {"S": "x"}, ":v2": {"S": "02135"},
                    ":v3": {"S": "NULL"}, ":v4": {"N": "5"}, ":v5": {"S": "02134"}
                }
            })
        );
        let preview = plan.preview();
        assert_eq!(preview[0].0, 0);
        assert!(preview[0].1.starts_with("UpdateItem {"));
        // The preview is a console command that sends the same request.
        match parse_command(&preview[0].1).unwrap() {
            ConsoleCommand::Api { action, body: b } => {
                assert_eq!(action, "UpdateItem");
                assert_eq!(b, body);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(classify_console(&preview[0].1), StatementClass::Write);
        // An item the grid did not load is guarded by its key only.
        let plan = plan_changes(
            "t",
            &keys(),
            &columns(),
            &LoadedItems::default(),
            &changes,
            "tok",
        )
        .unwrap();
        assert_eq!(
            plan.ops[0].request().1["ConditionExpression"],
            json!("attribute_exists(#n4)")
        );
    }

    #[test]
    fn inserts_and_deletes() {
        let changes = vec![
            StagedChange::Insert {
                row_id: "new1".into(),
                values: BTreeMap::from([
                    ("pk".to_string(), text("b")),
                    ("sk".to_string(), text("2")),
                    ("tags".to_string(), Value::Json("[\"x\",\"y\"]".into())),
                    ("skip".to_string(), attr::missing()),
                ]),
                exprs: None,
            },
            StagedChange::Delete {
                row_id: "r2".into(),
                key: pk("a", 1),
            },
        ];
        let plan = plan_changes("t", &keys(), &columns(), &loaded(), &changes, "tok").unwrap();
        assert_eq!(
            plan.ops[0].request(),
            (
                "PutItem",
                json!({
                    "TableName": "t",
                    "Item": {"pk": {"S": "b"}, "sk": {"N": "2"}, "tags": {"SS": ["x", "y"]}},
                    "ConditionExpression": "attribute_not_exists(#n0)",
                    "ExpressionAttributeNames": {"#n0": "pk"}
                })
            )
        );
        assert_eq!(
            plan.ops[1].request(),
            (
                "DeleteItem",
                json!({
                    "TableName": "t",
                    "Key": {"pk": {"S": "a"}, "sk": {"N": "1"}},
                    "ConditionExpression": "attribute_exists(#n0)",
                    "ExpressionAttributeNames": {"#n0": "pk"}
                })
            )
        );
        let txs = plan.transactions().unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].changes, [0, 1]);
        assert_eq!(txs[0].body["ClientRequestToken"], json!("tok-t0"));
        assert_eq!(
            txs[0].body["TransactItems"][0]["Put"]["TableName"],
            json!("t")
        );
        assert_eq!(
            txs[0].body["TransactItems"][1]["Delete"]["Key"]["pk"],
            json!({"S": "a"})
        );
        // A new item needs its key, of the key's type.
        for values in [
            BTreeMap::from([("pk".to_string(), text("b"))]),
            BTreeMap::from([("pk".to_string(), text("b")), ("sk".to_string(), text("x"))]),
            BTreeMap::from([
                ("pk".to_string(), Value::Null),
                ("sk".to_string(), Value::Int(1)),
            ]),
        ] {
            let bad = [StagedChange::Insert {
                row_id: "n".into(),
                values,
                exprs: None,
            }];
            assert!(plan_changes("t", &keys(), &columns(), &loaded(), &bad, "t").is_err());
        }
        assert!(
            plan_changes(
                "t",
                &keys(),
                &columns(),
                &loaded(),
                &[StagedChange::Delete {
                    row_id: "x".into(),
                    key: RowKey::Rowid(1)
                }],
                "t"
            )
            .is_err()
        );
    }

    #[test]
    fn a_changed_key_is_a_delete_and_a_put_in_one_transaction() {
        let changes = vec![StagedChange::Update {
            row_id: "r1".into(),
            key: pk("a", 1),
            changes: BTreeMap::from([
                ("sk".to_string(), Value::Int(2)),
                ("n".to_string(), Value::Int(7)),
            ]),
            exprs: None,
        }];
        let plan = plan_changes("t", &keys(), &columns(), &loaded(), &changes, "tok").unwrap();
        let (action, body) = plan.ops[0].request();
        assert_eq!(action, "TransactWriteItems");
        assert_eq!(body["ClientRequestToken"], json!("tok-c0"));
        let items = body["TransactItems"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0]["Delete"]["ConditionExpression"],
            json!("attribute_exists(#n0) AND attribute_type(#n1, :v0) AND #n2 = :v1 AND #n3 = :v2")
        );
        assert_eq!(
            items[1]["Put"]["Item"],
            json!({"pk": {"S": "a"}, "sk": {"N": "2"}, "zip": {"S": "02134"}, "n": {"N": "7"}, "gone": {"NULL": true}})
        );
        // Not loaded: refused rather than guessed.
        assert!(
            plan_changes(
                "t",
                &keys(),
                &columns(),
                &LoadedItems::default(),
                &changes,
                "tok"
            )
            .is_err()
        );
        // The same key again is no change of the key.
        let same = vec![StagedChange::Update {
            row_id: "r1".into(),
            key: pk("a", 1),
            changes: BTreeMap::from([
                ("sk".to_string(), text("1")),
                ("n".to_string(), Value::Int(7)),
            ]),
            exprs: None,
        }];
        let plan = plan_changes("t", &keys(), &columns(), &loaded(), &same, "tok").unwrap();
        assert_eq!(plan.ops[0].request().0, "UpdateItem");
        let only_key = vec![StagedChange::Update {
            row_id: "r1".into(),
            key: pk("a", 1),
            changes: BTreeMap::from([("sk".to_string(), Value::Int(1))]),
            exprs: None,
        }];
        assert!(plan_changes("t", &keys(), &columns(), &loaded(), &only_key, "tok").is_err());
        let null_key = vec![StagedChange::Update {
            row_id: "r1".into(),
            key: pk("a", 1),
            changes: BTreeMap::from([("pk".to_string(), Value::Null)]),
            exprs: None,
        }];
        assert!(plan_changes("t", &keys(), &columns(), &loaded(), &null_key, "tok").is_err());
    }

    #[test]
    fn transactions_hold_at_most_100_items_and_each_item_once() {
        let changes: Vec<StagedChange> = (0..150)
            .map(|i| StagedChange::Delete {
                row_id: format!("r{i}"),
                key: pk("a", i),
            })
            .collect();
        let plan = plan_changes(
            "t",
            &keys(),
            &columns(),
            &loaded(),
            &changes,
            "0123456789abcdef0123456789abcdef-extra",
        )
        .unwrap();
        assert_eq!(plan.token.len(), 28);
        let txs = plan.transactions().unwrap();
        assert_eq!(txs.len(), 2);
        assert_eq!(txs[0].changes.len(), 100);
        assert_eq!(txs[1].changes[0], 100);
        assert!(txs[1].body["ClientRequestToken"].as_str().unwrap().len() <= 36);
        let twice = vec![
            StagedChange::Delete {
                row_id: "a".into(),
                key: pk("a", 1),
            },
            StagedChange::Delete {
                row_id: "b".into(),
                key: pk("a", 1),
            },
        ];
        let plan = plan_changes("t", &keys(), &columns(), &loaded(), &twice, "tok").unwrap();
        assert!(plan.transactions().is_err());
    }

    #[test]
    fn loaded_items_are_bounded() {
        let k = KeySchema {
            hash: ("id".into(), AttrType::N),
            range: None,
        };
        let mut l = LoadedItems::default();
        for i in 0..(LoadedItems::CAP + 10) {
            l.record(
                &k,
                &Item::from([("id".to_string(), Attr::N(i.to_string()))]),
            );
        }
        assert_eq!(l.len(), LoadedItems::CAP);
        assert!(
            l.get(&Item::from([("id".to_string(), Attr::N("0".into()))]))
                .is_none()
        );
        assert!(
            l.get(&Item::from([("id".to_string(), Attr::N("10".into()))]))
                .is_some()
        );
        l.clear();
        assert!(l.is_empty());
        assert_eq!(l.bytes(), 0);
    }

    /// Items are also forgotten by size, per table and over a session's tables, so pages of large
    /// items cannot pile up.
    #[test]
    fn loaded_items_are_bounded_by_size() {
        let k = KeySchema {
            hash: ("id".into(), AttrType::N),
            range: None,
        };
        let item = |id: usize, len: usize| {
            Item::from([
                ("id".to_string(), Attr::N(id.to_string())),
                ("blob".to_string(), Attr::S("x".repeat(len))),
            ])
        };
        let mut l = LoadedItems::default();
        for i in 0..10 {
            l.record(&k, &item(i, 1000));
        }
        let each = l.bytes() / 10;
        assert!(each > 1000, "{each}");
        // Recording an item again replaces its size, it is not counted twice.
        l.record(&k, &item(9, 3000));
        let more = attr::item_size(&item(9, 3000)) - attr::item_size(&item(9, 1000));
        assert!(more >= 2000, "{more}");
        assert_eq!(l.bytes(), 10 * each + more);
        l.forget(&Item::from([("id".to_string(), Attr::N("9".into()))]));
        assert_eq!((l.len(), l.bytes()), (9, 9 * each));
        l.trim(LoadedItems::CAP, 4 * each);
        assert_eq!(l.len(), 4);
        assert!(
            l.get(&Item::from([("id".to_string(), Attr::N("4".into()))]))
                .is_none()
        );
        assert!(
            l.get(&Item::from([("id".to_string(), Attr::N("5".into()))]))
                .is_some()
        );

        // Over a session: the other tables go first, the largest first, then the current one's
        // oldest items.
        let mut tables: HashMap<String, LoadedItems> = HashMap::new();
        for (t, n) in [("a", 3), ("b", 5), ("c", 4)] {
            let entry = tables.entry(t.to_string()).or_default();
            for i in 0..n {
                entry.record(&k, &item(i, 1000));
            }
        }
        trim_loaded(&mut tables, "a", 9 * each);
        let mut left: Vec<&str> = tables.keys().map(String::as_str).collect();
        left.sort_unstable();
        assert_eq!(left, ["a", "c"]);
        trim_loaded(&mut tables, "a", 2 * each);
        assert_eq!(tables.keys().collect::<Vec<_>>(), ["a"]);
        assert_eq!(tables["a"].len(), 2);
    }

    #[test]
    fn sets_compare_in_any_order() {
        assert!(same_attr(
            &Attr::Ss(vec!["a".into(), "b".into()]),
            &Attr::Ss(vec!["b".into(), "a".into()])
        ));
        assert!(!same_attr(&Attr::L(vec![Attr::Null]), &Attr::L(vec![])));
        assert!(same_attr(
            &Attr::M(BTreeMap::from([(
                "k".to_string(),
                Attr::Ns(vec!["1".into(), "2".into()])
            )])),
            &Attr::M(BTreeMap::from([(
                "k".to_string(),
                Attr::Ns(vec!["2".into(), "1".into()])
            )]))
        ));
    }
}
