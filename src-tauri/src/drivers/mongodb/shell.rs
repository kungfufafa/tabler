//! The console's language: a subset of mongosh, read without a JavaScript engine.
//!
//! - `split`: a statement ends at a `;` or at a line break after a complete expression; a line
//!   that starts with `.` continues the statement before it (method chains), as in mongosh.
//! - `parse`: one statement as the command(s) it runs:
//!   - `db.<collection>.<method>(…)`, also `db['a-b']`, `db.getCollection('a.b')` and dotted
//!     names (`db.system.profile.find()`), with find's cursor chain (`.sort().skip().limit()…`)
//!     and `db.<collection>.explain().<method>(…)`;
//!   - the database's methods (`db.runCommand`, `db.adminCommand`, `db.getSiblingDB('x')…`,
//!     `db.getCollectionNames()`, `db.stats()`, `db.createCollection()`…) and `rs.status()`;
//!   - `use <db>` and `show dbs | collections | users | roles`.
//!
//! Arguments are literals (`literal`): no variables, functions or loops. The commands come from
//! `sqlgen::native::mongo`, so what the console runs is what Preview would show.

use ::mongodb::bson::{Bson, Document, doc};

use crate::drivers::mongodb::literal::{Lexer, ParseError, int_of};
use crate::sqlgen::native::mongo::{self, ConsoleCommand, Output};
use crate::sqlgen::split::SplitStatement;

type Result<T> = std::result::Result<T, ParseError>;

/// What one console statement does.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Run(ConsoleCommand),
    /// Several commands in order (`bulkWrite`); `ordered` stops at the first that fails.
    Batch {
        commands: Vec<ConsoleCommand>,
        ordered: bool,
    },
    /// `use <db>`: the database of the statements that follow.
    Use(String),
}

impl Statement {
    /// The commands it runs (none for `use`).
    pub fn commands(&self) -> Vec<&ConsoleCommand> {
        match self {
            Statement::Run(c) => vec![c],
            Statement::Batch { commands, .. } => commands.iter().collect(),
            Statement::Use(_) => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Splitting
// ---------------------------------------------------------------------------------------------

/// The statements of the console's text, with char offsets and 1-based lines as the SQL
/// splitter gives them. Comments before a statement are not part of it; empty statements are
/// skipped.
pub fn split(text: &str) -> Vec<SplitStatement> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    // The last code character and its index, for the regex heuristic and continuation.
    let mut last: Option<(usize, char)> = None;
    let mut depth: i64 = 0;
    let mut i = 0;
    let flush = |out: &mut Vec<SplitStatement>, start: &mut Option<usize>, end: usize| {
        if let Some(s) = start.take()
            && end > s
        {
            let sql: String = chars[s..end].iter().collect();
            let line = 1 + chars[..s].iter().filter(|c| **c == '\n').count() as u32;
            out.push(SplitStatement {
                sql,
                start: s,
                end,
                line,
            });
        }
    };
    while i < n {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '/' if next == Some('/') => {
                while i < n && chars[i] != '\n' && chars[i] != '\r' {
                    i += 1;
                }
                continue;
            }
            '/' if next == Some('*') => {
                i += 2;
                while i < n && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i = (i + 2).min(n);
                continue;
            }
            '\n' | '\r' => {
                let complete = depth <= 0
                    && !matches!(last, Some((_, '.' | ',' | ':' | '=' | '(' | '[' | '{')));
                if let (Some(_), Some((at, _)), true) = (start, last, complete)
                    && !continues(&chars, i + 1)
                {
                    flush(&mut out, &mut start, at + 1);
                    last = None;
                    depth = 0;
                }
                i += 1;
                continue;
            }
            ';' if depth <= 0 => {
                if let Some((at, _)) = last {
                    flush(&mut out, &mut start, at + 1);
                }
                start = None;
                last = None;
                depth = 0;
                i += 1;
                continue;
            }
            c if c.is_whitespace() || c == '\u{feff}' => {
                i += 1;
                continue;
            }
            _ => {}
        }
        if start.is_none() {
            start = Some(i);
        }
        match c {
            // Strings and regular expressions; one left open ends at the line break (the
            // parser reports it).
            '\'' | '"' | '`' => {
                let mut j = i + 1;
                while j < n && chars[j] != c {
                    if chars[j] == '\\' {
                        j += 1;
                    } else if c != '`' && (chars[j] == '\n' || chars[j] == '\r') {
                        break;
                    }
                    j += 1;
                }
                i = close_at(&chars, j, c, &mut last);
                continue;
            }
            '/' if regex_may_start(last.map(|(_, c)| c)) => {
                let mut j = i + 1;
                let mut class = false;
                while j < n && chars[j] != '\n' && chars[j] != '\r' {
                    match chars[j] {
                        '\\' if !matches!(chars.get(j + 1), Some('\n' | '\r')) => j += 1,
                        '[' => class = true,
                        ']' => class = false,
                        '/' if !class => break,
                        _ => {}
                    }
                    j += 1;
                }
                i = close_at(&chars, j, '/', &mut last);
                continue;
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        last = Some((i, c));
        i += 1;
    }
    if let Some((at, _)) = last {
        flush(&mut out, &mut start, at + 1);
    }
    out
}

/// After a string or regex scanned up to `j`: where scanning goes on (past the closing `quote`,
/// or at the line break that cut it), with the last code character updated.
fn close_at(chars: &[char], j: usize, quote: char, last: &mut Option<(usize, char)>) -> usize {
    let n = chars.len();
    if j < n && chars[j] == quote {
        *last = Some((j, quote));
        j + 1
    } else {
        let j = j.min(n);
        *last = Some((j.saturating_sub(1), quote));
        j
    }
}

/// Whether the next code after a line break starts with `.` (a chained call on a new line).
fn continues(chars: &[char], mut i: usize) -> bool {
    while i < chars.len() {
        match chars[i] {
            c if c.is_whitespace() => i += 1,
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            c => return c == '.' && !chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()),
        }
    }
    false
}

/// A `/` starts a regular expression where a value is expected (there is no division in the
/// subset, but `)` or a name before it reads as one).
fn regex_may_start(last: Option<char>) -> bool {
    match last {
        None => true,
        Some(c) => "(,:[{=!&|?;+-*%<>~^".contains(c),
    }
}

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

/// One statement (as `split` returns it) as what it runs, on `db` unless it names another.
pub fn parse(text: &str, db: &str) -> Result<Statement> {
    let mut p = Parser {
        lx: Lexer::new(text),
        db: db.to_string(),
    };
    let st = p.statement()?;
    p.lx.end()?;
    Ok(st)
}

struct Parser<'a> {
    lx: Lexer<'a>,
    db: String,
}

/// The arguments of one call, with the method's name and position for errors.
struct Args {
    method: String,
    at: usize,
    items: Vec<Bson>,
}

impl Args {
    fn err(&self, message: impl std::fmt::Display) -> ParseError {
        ParseError::new(self.at, format!("{}(): {message}", self.method))
    }

    fn at_most(&self, max: usize) -> Result<()> {
        if self.items.len() > max {
            return Err(self.err(format!(
                "takes at most {max} argument{}",
                if max == 1 { "" } else { "s" }
            )));
        }
        Ok(())
    }

    fn at_least(&self, min: usize) -> Result<()> {
        if self.items.len() < min {
            return Err(self.err(format!(
                "takes {min} argument{}",
                if min == 1 { "" } else { "s" }
            )));
        }
        Ok(())
    }

    /// The argument, unless it is absent, null or undefined.
    fn get(&self, i: usize) -> Option<&Bson> {
        self.items.get(i).filter(|b| !matches!(b, Bson::Null))
    }

    fn doc(&self, i: usize, what: &str) -> Result<Document> {
        match self.get(i) {
            Some(Bson::Document(d)) => Ok(d.clone()),
            _ => Err(self.err(format!("takes {what} document"))),
        }
    }

    fn opt_doc(&self, i: usize, what: &str) -> Result<Option<Document>> {
        match self.get(i) {
            None => Ok(None),
            Some(Bson::Document(d)) => Ok(Some(d.clone())),
            Some(_) => Err(self.err(format!("takes {what} document"))),
        }
    }

    /// A filter: an empty one when absent.
    fn filter(&self, i: usize) -> Result<Document> {
        Ok(self.opt_doc(i, "a filter")?.unwrap_or_default())
    }

    fn options(&self, i: usize) -> Result<Document> {
        Ok(self.opt_doc(i, "an options")?.unwrap_or_default())
    }

    fn string(&self, i: usize, what: &str) -> Result<String> {
        match self.get(i) {
            Some(Bson::String(s)) => Ok(s.clone()),
            _ => Err(self.err(format!("takes {what} string"))),
        }
    }

    fn int(&self, i: usize, what: &str) -> Result<i64> {
        self.get(i)
            .and_then(int_of)
            .ok_or_else(|| self.err(format!("takes {what}")))
    }

    fn opt_bool(&self, i: usize) -> Result<Option<bool>> {
        match self.get(i) {
            None => Ok(None),
            Some(Bson::Boolean(b)) => Ok(Some(*b)),
            Some(_) => Err(self.err("takes true or false")),
        }
    }

    fn array(&self, i: usize, what: &str) -> Result<Vec<Bson>> {
        match self.get(i) {
            Some(Bson::Array(a)) => Ok(a.clone()),
            _ => Err(self.err(format!("takes {what}"))),
        }
    }
}

/// Keeps the options a method knows, refusing the others (a typo would otherwise be ignored by
/// the shell or rejected late by the server).
fn known(args: &Args, options: Document, allowed: &[&str]) -> Result<Document> {
    if let Some(k) = options.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(args.err(format!("unknown option `{k}`")));
    }
    Ok(options)
}

/// Splits `options` into the fields named in `names` and the rest.
fn take(options: &mut Document, names: &[&str]) -> Document {
    let mut out = Document::new();
    for name in names {
        if let Some(v) = options.remove(*name) {
            out.insert(*name, v);
        }
    }
    out
}

fn truthy(b: &Bson) -> bool {
    match b {
        Bson::Boolean(v) => *v,
        Bson::Null | Bson::Undefined => false,
        other => int_of(other).is_none_or(|n| n != 0),
    }
}

/// A database name the server accepts.
fn check_db_name(name: &str, at: usize) -> Result<()> {
    let bad = name.is_empty()
        || name.len() > 64
        || name
            .chars()
            .any(|c| c.is_whitespace() || "/\\.\"$*<>:|?\0".contains(c));
    if bad {
        return Err(ParseError::new(
            at,
            format!("invalid database name \u{201c}{name}\u{201d}"),
        ));
    }
    Ok(())
}

const FIND_OPTIONS: &[&str] = &[
    "projection",
    "sort",
    "skip",
    "limit",
    "batchSize",
    "hint",
    "collation",
    "maxTimeMS",
    "comment",
    "min",
    "max",
    "allowDiskUse",
    "returnKey",
    "showRecordId",
    "noCursorTimeout",
    "let",
    "readConcern",
    "singleBatch",
];
/// The calls a `find` cursor takes.
const CURSOR_METHODS: &[&str] = &[
    "sort",
    "project",
    "projection",
    "collation",
    "min",
    "max",
    "skip",
    "batchSize",
    "maxTimeMS",
    "limit",
    "comment",
    "hint",
    "allowDiskUse",
    "noCursorTimeout",
    "returnKey",
    "showRecordId",
    "readConcern",
    "toArray",
    "pretty",
    "count",
    "size",
    "explain",
];
const COMMAND_WRITE_OPTIONS: &[&str] =
    &["writeConcern", "bypassDocumentValidation", "comment", "let"];
const UPDATE_STATEMENT_OPTIONS: &[&str] = &["upsert", "arrayFilters", "collation", "hint"];
const DELETE_STATEMENT_OPTIONS: &[&str] = &["collation", "hint"];

impl Parser<'_> {
    fn statement(&mut self) -> Result<Statement> {
        let start = self.lx.peek_pos()?;
        let Some(word) = self.lx.ident()? else {
            return Err(self.lx.unexpected("db.…, use or show"));
        };
        match word.as_str() {
            "use" => self.use_db(),
            "show" => self.show(),
            "db" => self.db_expr(),
            "rs" => self.rs(),
            _ => Err(ParseError::new(
                start,
                format!(
                    "unknown name `{word}`: the console runs db.…, use and show (a subset of \
                     mongosh, without JavaScript)"
                ),
            )),
        }
    }

    /// The word after `use` / `show`, up to white space, `;` or a comment.
    fn word(&mut self) -> Result<(usize, String)> {
        let at = self.lx.peek_pos()?;
        let rest = &self.lx.src[self.lx.pos..];
        let end = rest
            .find(|c: char| c.is_whitespace() || c == ';' || c == '/')
            .unwrap_or(rest.len());
        self.lx.pos += end;
        Ok((at, rest[..end].to_string()))
    }

    fn use_db(&mut self) -> Result<Statement> {
        let (at, name) = self.word()?;
        check_db_name(&name, at)?;
        Ok(Statement::Use(name))
    }

    fn show(&mut self) -> Result<Statement> {
        let (at, what) = self.word()?;
        let db = self.db.as_str();
        let run = |db: &str, command, output| {
            Ok(Statement::Run(ConsoleCommand::new(db, command, output)))
        };
        match what.as_str() {
            "dbs" | "databases" => run(
                "admin",
                mongo::list_databases(false),
                Output::Field("databases"),
            ),
            "collections" | "tables" => run(db, mongo::list_collections(true), Output::Names),
            "users" => run(db, doc! { "usersInfo": 1 }, Output::Field("users")),
            "roles" => run(
                db,
                doc! { "rolesInfo": 1, "showBuiltinRoles": true },
                Output::Field("roles"),
            ),
            _ => Err(ParseError::new(
                at,
                "show takes dbs, collections, users or roles",
            )),
        }
    }

    fn expect_dot(&mut self) -> Result<()> {
        if self.lx.eat('.')? {
            Ok(())
        } else {
            Err(self.lx.unexpected("`.`"))
        }
    }

    fn name(&mut self) -> Result<(usize, String)> {
        let at = self.lx.peek_pos()?;
        match self.lx.ident()? {
            Some(n) => Ok((at, n)),
            None => Err(self.lx.unexpected("a name")),
        }
    }

    fn call(&mut self, method: &str, at: usize) -> Result<Args> {
        self.lx.expect('(')?;
        let items = self.lx.args()?;
        Ok(Args {
            method: method.to_string(),
            at,
            items,
        })
    }

    /// `.pretty()` and `.toArray()` after a call change nothing here.
    fn tail(&mut self) -> Result<()> {
        loop {
            let save = self.lx.pos;
            if !self.lx.eat('.')? {
                return Ok(());
            }
            let (at, m) = self.name()?;
            if !matches!(m.as_str(), "pretty" | "toArray") {
                self.lx.pos = save;
                return Err(ParseError::new(
                    at,
                    format!("unexpected call `.{m}()` here"),
                ));
            }
            self.call(&m, at)?.at_most(0)?;
        }
    }

    fn db_expr(&mut self) -> Result<Statement> {
        let mut db = self.db.clone();
        loop {
            if self.lx.eat('[')? {
                let name = self.lx.string()?;
                self.lx.expect(']')?;
                return self.collection(&db, name);
            }
            self.expect_dot()?;
            let (at, name) = self.name()?;
            if self.lx.peek()? == Some('(') {
                let args = self.call(&name, at)?;
                match name.as_str() {
                    "getSiblingDB" => {
                        args.at_least(1)?;
                        args.at_most(1)?;
                        db = args.string(0, "a database name")?;
                        check_db_name(&db, at)?;
                        continue;
                    }
                    "getCollection" => {
                        args.at_least(1)?;
                        args.at_most(1)?;
                        let coll = args.string(0, "a collection name")?;
                        return self.collection(&db, coll);
                    }
                    _ => {
                        let st = self.db_method(&db, args)?;
                        self.tail()?;
                        return Ok(st);
                    }
                }
            }
            // A collection: its name runs up to the method (`db.system.profile.find()`).
            let mut parts = vec![name];
            loop {
                if self.lx.peek()? != Some('.') {
                    return Err(self.lx.unexpected("a method call such as `.find()`"));
                }
                self.expect_dot()?;
                let (at, part) = self.name()?;
                if self.lx.peek()? == Some('(') {
                    return self.collection_method(&db, &parts.join("."), &part, at);
                }
                parts.push(part);
            }
        }
    }

    fn collection(&mut self, db: &str, coll: String) -> Result<Statement> {
        if coll.is_empty() {
            return Err(self.lx.unexpected("a collection name"));
        }
        self.expect_dot()?;
        let (at, method) = self.name()?;
        self.collection_method(db, &coll, &method, at)
    }

    fn rs(&mut self) -> Result<Statement> {
        self.expect_dot()?;
        let (at, method) = self.name()?;
        let args = self.call(&method, at)?;
        args.at_most(0)?;
        let (command, output) = match method.as_str() {
            "status" => (doc! { "replSetGetStatus": 1 }, Output::Reply),
            "conf" | "config" => (doc! { "replSetGetConfig": 1 }, Output::Field("config")),
            "hello" | "isMaster" => (mongo::hello(), Output::Reply),
            _ => return Err(args.err("is not supported in the console")),
        };
        self.tail()?;
        Ok(Statement::Run(ConsoleCommand::new(
            "admin", command, output,
        )))
    }

    fn db_method(&mut self, db: &str, a: Args) -> Result<Statement> {
        let run = |db: &str, command, output| {
            Ok(Statement::Run(ConsoleCommand::new(db, command, output)))
        };
        match a.method.as_str() {
            "runCommand" | "adminCommand" => {
                a.at_least(1)?;
                a.at_most(2)?;
                let command = match a.get(0) {
                    Some(Bson::Document(d)) if !d.is_empty() => d.clone(),
                    Some(Bson::String(name)) if !name.is_empty() => {
                        let mut d = Document::new();
                        d.insert(name.clone(), 1);
                        d
                    }
                    _ => return Err(a.err("takes a command document")),
                };
                let target = if a.method == "adminCommand" {
                    "admin"
                } else {
                    db
                };
                run(target, command, Output::Reply)
            }
            "getCollectionNames" => {
                a.at_most(0)?;
                run(db, mongo::list_collections(true), Output::Names)
            }
            "getCollectionInfos" => {
                a.at_most(2)?;
                let filter = a.filter(0)?;
                let options = known(&a, a.options(1)?, &["nameOnly", "authorizedCollections"])?;
                let mut command = mongo::list_collections(false);
                if !filter.is_empty() {
                    command.insert("filter", filter);
                }
                command.extend(options);
                run(db, command, Output::Cursor)
            }
            "stats" => {
                a.at_most(1)?;
                let mut command = mongo::db_stats();
                match a.get(0) {
                    None => {}
                    Some(Bson::Document(o)) => {
                        command.extend(known(&a, o.clone(), &["scale", "freeStorage"])?)
                    }
                    Some(n) => {
                        let scale = int_of(n).ok_or_else(|| a.err("takes a scale"))?;
                        command.insert("scale", scale);
                    }
                }
                run(db, command, Output::Reply)
            }
            "serverStatus" => {
                a.at_most(1)?;
                let mut command = doc! { "serverStatus": 1 };
                command.extend(a.options(0)?);
                run(db, command, Output::Reply)
            }
            "hostInfo" => {
                a.at_most(0)?;
                run(db, doc! { "hostInfo": 1 }, Output::Reply)
            }
            "version" => {
                a.at_most(0)?;
                run(db, mongo::build_info(), Output::Field("version"))
            }
            "hello" | "isMaster" => {
                a.at_most(0)?;
                run(db, mongo::hello(), Output::Reply)
            }
            "listCommands" => {
                a.at_most(0)?;
                run(db, doc! { "listCommands": 1 }, Output::Field("commands"))
            }
            "currentOp" => {
                a.at_most(1)?;
                let mut command = doc! { "currentOp": 1 };
                match a.get(0) {
                    None => {}
                    Some(Bson::Boolean(all)) => {
                        command.insert("$all", *all);
                    }
                    Some(Bson::Document(f)) => command.extend(f.clone()),
                    Some(_) => return Err(a.err("takes true or a filter document")),
                }
                run("admin", command, Output::Reply)
            }
            "killOp" => {
                a.at_least(1)?;
                a.at_most(1)?;
                let op = a
                    .get(0)
                    .cloned()
                    .ok_or_else(|| a.err("takes an operation id"))?;
                run("admin", mongo::kill_op(op), Output::Reply)
            }
            "getUsers" => {
                a.at_most(1)?;
                let mut command = doc! { "usersInfo": 1 };
                command.extend(a.options(0)?);
                run(db, command, Output::Field("users"))
            }
            "getRoles" => {
                a.at_most(1)?;
                let mut command = doc! { "rolesInfo": 1 };
                command.extend(a.options(0)?);
                run(db, command, Output::Field("roles"))
            }
            "createCollection" => {
                a.at_least(1)?;
                a.at_most(2)?;
                let name = a.string(0, "a collection name")?;
                run(
                    db,
                    mongo::create_collection(&name, &a.options(1)?),
                    Output::Reply,
                )
            }
            "createView" => {
                a.at_least(3)?;
                a.at_most(4)?;
                let name = a.string(0, "a view name")?;
                let source = a.string(1, "a source collection")?;
                let pipeline = a.array(2, "a pipeline array")?;
                let mut command = mongo::create_view(&name, &source, pipeline);
                command.extend(known(&a, a.options(3)?, &["collation"])?);
                run(db, command, Output::Reply)
            }
            "dropDatabase" => {
                a.at_most(1)?;
                let mut command = doc! { "dropDatabase": 1 };
                command.extend(known(&a, a.options(0)?, &["writeConcern", "comment"])?);
                run(db, command, Output::Reply)
            }
            "aggregate" => {
                a.at_least(1)?;
                a.at_most(2)?;
                let pipeline = a.array(0, "a pipeline array")?;
                let command = mongo::aggregate_with(Bson::Int32(1), pipeline, a.options(1)?);
                let output = if command.contains_key("cursor") {
                    Output::Cursor
                } else {
                    Output::Reply
                };
                run(db, command, output)
            }
            _ => Err(a.err(format!(
                "is not a database method the console knows (db.{}() is not supported)",
                a.method
            ))),
        }
    }

    fn collection_method(
        &mut self,
        db: &str,
        coll: &str,
        method: &str,
        at: usize,
    ) -> Result<Statement> {
        if method == "explain" {
            return self.explain(db, coll, at);
        }
        let a = self.call(method, at)?;
        let st = self.collection_call(db, coll, a)?;
        if let Statement::Run(ConsoleCommand {
            output: Output::Cursor,
            ..
        }) = &st
        {
            self.tail()?;
        } else {
            self.tail_pretty()?;
        }
        Ok(st)
    }

    /// `.pretty()` only (not a cursor).
    fn tail_pretty(&mut self) -> Result<()> {
        let save = self.lx.pos;
        if self.lx.eat('.')? {
            let (at, m) = self.name()?;
            if m != "pretty" {
                self.lx.pos = save;
                return Err(ParseError::new(
                    at,
                    format!("unexpected call `.{m}()` here"),
                ));
            }
            self.call(&m, at)?.at_most(0)?;
        }
        Ok(())
    }

    /// `db.c.explain(verbosity).<method>(…)`.
    fn explain(&mut self, db: &str, coll: &str, at: usize) -> Result<Statement> {
        let a = self.call("explain", at)?;
        a.at_most(1)?;
        let verbosity = verbosity(&a, a.get(0))?;
        self.expect_dot()?;
        let (m_at, method) = self.name()?;
        if method == "explain" {
            return Err(ParseError::new(m_at, "explain() of explain()"));
        }
        let inner = self.collection_method(db, coll, &method, m_at)?;
        match inner {
            Statement::Run(c) => Ok(Statement::Run(ConsoleCommand::new(
                db,
                mongo::explain(c.command, verbosity),
                Output::Reply,
            ))),
            _ => Err(ParseError::new(
                m_at,
                format!("{method}() cannot be explained"),
            )),
        }
    }

    fn collection_call(&mut self, db: &str, coll: &str, a: Args) -> Result<Statement> {
        let run = |command, output| Ok(Statement::Run(ConsoleCommand::new(db, command, output)));
        match a.method.as_str() {
            "find" => {
                a.at_most(3)?;
                let filter = a.filter(0)?;
                let mut options = known(&a, a.options(2)?, FIND_OPTIONS)?;
                if let Some(p) = a.opt_doc(1, "a projection")?
                    && !p.is_empty()
                {
                    options.insert("projection", p);
                }
                self.find_chain(db, coll, filter, options)
            }
            "findOne" => {
                a.at_most(3)?;
                let filter = a.filter(0)?;
                let mut options = known(&a, a.options(2)?, FIND_OPTIONS)?;
                if let Some(p) = a.opt_doc(1, "a projection")?
                    && !p.is_empty()
                {
                    options.insert("projection", p);
                }
                run(mongo::find_one(coll, filter, options), Output::First)
            }
            "countDocuments" => {
                a.at_most(2)?;
                let filter = a.filter(0)?;
                let mut options = known(
                    &a,
                    a.options(1)?,
                    &["skip", "limit", "hint", "maxTimeMS", "collation", "comment"],
                )?;
                let skip = options.remove("skip").as_ref().and_then(int_of);
                let limit = options.remove("limit").as_ref().and_then(int_of);
                run(
                    mongo::count_documents_with(coll, filter, skip, limit, options),
                    Output::CountDocuments,
                )
            }
            "estimatedDocumentCount" => {
                a.at_most(1)?;
                let options = known(&a, a.options(0)?, &["maxTimeMS", "comment"])?;
                run(
                    mongo::count_with(coll, Document::new(), options),
                    Output::Count,
                )
            }
            "count" => {
                a.at_most(2)?;
                let filter = a.filter(0)?;
                let options = known(
                    &a,
                    a.options(1)?,
                    &[
                        "skip",
                        "limit",
                        "hint",
                        "maxTimeMS",
                        "collation",
                        "comment",
                        "readConcern",
                    ],
                )?;
                run(mongo::count_with(coll, filter, options), Output::Count)
            }
            "distinct" => {
                a.at_least(1)?;
                a.at_most(3)?;
                let key = a.string(0, "a field name")?;
                let filter = a.filter(1)?;
                let options = known(
                    &a,
                    a.options(2)?,
                    &["collation", "maxTimeMS", "comment", "hint"],
                )?;
                run(
                    mongo::distinct_with(coll, &key, filter, options),
                    Output::Distinct,
                )
            }
            "aggregate" => {
                // `aggregate([stages], {options})`, or the old `aggregate(stage, stage, …)`.
                let (pipeline, options) = match a.items.first() {
                    Some(Bson::Array(p)) => {
                        a.at_most(2)?;
                        (p.clone(), a.options(1)?)
                    }
                    None => (Vec::new(), Document::new()),
                    Some(_) => {
                        if !a.items.iter().all(|s| matches!(s, Bson::Document(_))) {
                            return Err(a.err("takes a pipeline array"));
                        }
                        (a.items.clone(), Document::new())
                    }
                };
                let options = known(
                    &a,
                    options,
                    &[
                        "allowDiskUse",
                        "maxTimeMS",
                        "collation",
                        "hint",
                        "comment",
                        "let",
                        "batchSize",
                        "explain",
                        "bypassDocumentValidation",
                        "readConcern",
                        "writeConcern",
                        "cursor",
                    ],
                )?;
                let command = mongo::aggregate_with(Bson::String(coll.into()), pipeline, options);
                let output = if command.contains_key("cursor") {
                    Output::Cursor
                } else {
                    Output::Reply
                };
                let st = Statement::Run(ConsoleCommand::new(db, command, output));
                self.aggregate_chain(db, st)
            }
            "insertOne" => {
                a.at_least(1)?;
                a.at_most(2)?;
                let d = a.doc(0, "a")?;
                let options = known(&a, a.options(1)?, COMMAND_WRITE_OPTIONS)?;
                let (command, ids) = mongo::insert_many(coll, vec![d], true, options);
                run(command, Output::Inserted(ids))
            }
            "insertMany" | "insert" => {
                a.at_least(1)?;
                a.at_most(2)?;
                let docs = match a.get(0) {
                    Some(Bson::Array(items)) => items
                        .iter()
                        .map(|b| match b {
                            Bson::Document(d) => Ok(d.clone()),
                            _ => Err(a.err("takes documents")),
                        })
                        .collect::<Result<Vec<_>>>()?,
                    Some(Bson::Document(d)) if a.method == "insert" => vec![d.clone()],
                    _ => return Err(a.err("takes an array of documents")),
                };
                if docs.is_empty() {
                    return Err(a.err("takes at least one document"));
                }
                let mut options = a.options(1)?;
                let ordered = options.remove("ordered").is_none_or(|b| truthy(&b));
                let options = known(&a, options, COMMAND_WRITE_OPTIONS)?;
                let (command, ids) = mongo::insert_many(coll, docs, ordered, options);
                run(command, Output::Inserted(ids))
            }
            "updateOne" | "updateMany" | "update" | "replaceOne" => {
                a.at_least(2)?;
                a.at_most(3)?;
                let q = a.filter(0)?;
                let mut options = a.options(2)?;
                let legacy_multi = options.remove("multi");
                if a.method != "update" && legacy_multi.is_some() {
                    return Err(a.err("unknown option `multi`"));
                }
                let u = match a.get(1) {
                    Some(Bson::Document(u)) => {
                        let ok = match a.method.as_str() {
                            "replaceOne" => mongo::is_replacement(u),
                            "update" => mongo::is_update_document(u) || mongo::is_replacement(u),
                            _ => mongo::is_update_document(u),
                        };
                        if !ok {
                            return Err(a.err(if a.method == "replaceOne" {
                                "a replacement document takes no update operators"
                            } else {
                                "the update document needs operators such as $set (or use replaceOne)"
                            }));
                        }
                        Bson::Document(u.clone())
                    }
                    Some(Bson::Array(p)) if a.method != "replaceOne" => Bson::Array(p.clone()),
                    _ => return Err(a.err("takes an update document or pipeline")),
                };
                let multi = match a.method.as_str() {
                    "updateMany" => true,
                    "update" => legacy_multi.is_some_and(|b| truthy(&b)),
                    _ => false,
                };
                let statement_options = take(&mut options, UPDATE_STATEMENT_OPTIONS);
                let options = known(&a, options, COMMAND_WRITE_OPTIONS)?;
                let statement = mongo::update_statement(q, u, multi, statement_options);
                run(
                    mongo::write_with("update", coll, vec![statement], true, options),
                    Output::Updated,
                )
            }
            "deleteOne" | "deleteMany" | "remove" => {
                a.at_most(2)?;
                if a.method != "remove" {
                    a.at_least(1)?;
                }
                let q = a.filter(0)?;
                let (limit, mut options) = match (a.method.as_str(), a.get(1)) {
                    ("deleteOne", _) => (1, a.options(1)?),
                    ("deleteMany", _) => (0, a.options(1)?),
                    (_, Some(Bson::Document(o))) => {
                        let mut o = o.clone();
                        let one = o.remove("justOne").is_some_and(|b| truthy(&b));
                        (i32::from(one), o)
                    }
                    (_, Some(b)) => (i32::from(truthy(b)), Document::new()),
                    (_, None) => (0, Document::new()),
                };
                let statement_options = take(&mut options, DELETE_STATEMENT_OPTIONS);
                let options = known(&a, options, COMMAND_WRITE_OPTIONS)?;
                let statement = mongo::delete_statement(q, limit, statement_options);
                run(
                    mongo::write_with("delete", coll, vec![statement], true, options),
                    Output::Deleted,
                )
            }
            "findOneAndUpdate" | "findOneAndReplace" | "findOneAndDelete" => {
                let delete = a.method == "findOneAndDelete";
                a.at_least(if delete { 1 } else { 2 })?;
                a.at_most(if delete { 2 } else { 3 })?;
                let q = a.filter(0)?;
                let mut fields = doc! { "query": q };
                if delete {
                    fields.insert("remove", true);
                } else {
                    let u = match a.get(1) {
                        Some(Bson::Document(u)) => {
                            let ok = if a.method == "findOneAndReplace" {
                                mongo::is_replacement(u)
                            } else {
                                mongo::is_update_document(u)
                            };
                            if !ok {
                                return Err(a.err("the update does not fit the method"));
                            }
                            Bson::Document(u.clone())
                        }
                        Some(Bson::Array(p)) if a.method == "findOneAndUpdate" => {
                            Bson::Array(p.clone())
                        }
                        _ => return Err(a.err("takes an update document")),
                    };
                    fields.insert("update", u);
                }
                let mut options = a.options(if delete { 1 } else { 2 })?;
                if let Some(p) = options.remove("projection") {
                    fields.insert("fields", p);
                }
                let after = match (
                    options.remove("returnDocument"),
                    options.remove("returnNewDocument"),
                ) {
                    (Some(Bson::String(s)), _) if s == "after" => true,
                    (Some(Bson::String(s)), _) if s == "before" => false,
                    (Some(_), _) => return Err(a.err("returnDocument takes 'before' or 'after'")),
                    (None, Some(b)) => truthy(&b),
                    (None, None) => false,
                };
                if !delete {
                    fields.insert("new", after);
                }
                let allowed: &[&str] = &[
                    "sort",
                    "upsert",
                    "arrayFilters",
                    "collation",
                    "hint",
                    "maxTimeMS",
                    "bypassDocumentValidation",
                    "writeConcern",
                    "comment",
                    "let",
                ];
                fields.extend(known(&a, options, allowed)?);
                run(
                    mongo::command_with("findAndModify", coll, fields),
                    Output::Modified,
                )
            }
            "findAndModify" => {
                a.at_least(1)?;
                a.at_most(1)?;
                let spec = a.doc(0, "a findAndModify")?;
                if !spec.contains_key("remove") && !spec.contains_key("update") {
                    return Err(a.err("needs `update` or `remove: true`"));
                }
                run(
                    mongo::command_with("findAndModify", coll, spec),
                    Output::Modified,
                )
            }
            "bulkWrite" => self.bulk_write(db, coll, &a),
            "createIndex" | "ensureIndex" => {
                a.at_least(1)?;
                a.at_most(2)?;
                let keys = a.doc(0, "a keys")?;
                let mut spec = doc! { "key": keys };
                spec.extend(a.options(1)?);
                let command = mongo::create_indexes(coll, vec![spec], Document::new());
                let name = index_names(&command)
                    .into_iter()
                    .next()
                    .unwrap_or(Bson::Null);
                run(command, Output::Value(name))
            }
            "createIndexes" => {
                a.at_least(1)?;
                a.at_most(2)?;
                let keys = a.array(0, "an array of keys documents")?;
                let options = a.options(1)?;
                let specs = keys
                    .into_iter()
                    .map(|k| match k {
                        Bson::Document(k) => {
                            let mut spec = doc! { "key": k };
                            spec.extend(options.clone());
                            Ok(spec)
                        }
                        _ => Err(a.err("takes keys documents")),
                    })
                    .collect::<Result<Vec<_>>>()?;
                if specs.is_empty() {
                    return Err(a.err("takes at least one keys document"));
                }
                let command = mongo::create_indexes(coll, specs, Document::new());
                let names = Bson::Array(index_names(&command));
                run(command, Output::Value(names))
            }
            "dropIndex" => {
                a.at_least(1)?;
                a.at_most(1)?;
                let index = match a.get(0) {
                    Some(b @ (Bson::String(_) | Bson::Document(_))) => b.clone(),
                    _ => return Err(a.err("takes an index name or keys document")),
                };
                run(
                    mongo::command_with("dropIndexes", coll, doc! { "index": index }),
                    Output::Reply,
                )
            }
            "dropIndexes" => {
                a.at_most(1)?;
                let index = match a.get(0) {
                    None => Bson::String("*".into()),
                    Some(b @ (Bson::String(_) | Bson::Document(_))) => b.clone(),
                    Some(Bson::Array(names))
                        if names.iter().all(|n| matches!(n, Bson::String(_))) =>
                    {
                        Bson::Array(names.clone())
                    }
                    Some(_) => return Err(a.err("takes index names")),
                };
                run(
                    mongo::command_with("dropIndexes", coll, doc! { "index": index }),
                    Output::Reply,
                )
            }
            "getIndexes" | "getIndexSpecs" => {
                a.at_most(0)?;
                run(mongo::list_indexes(coll), Output::Cursor)
            }
            "drop" => {
                a.at_most(1)?;
                let options = known(&a, a.options(0)?, &["writeConcern", "comment"])?;
                run(mongo::command_with("drop", coll, options), Output::Reply)
            }
            "renameCollection" => {
                a.at_least(1)?;
                a.at_most(2)?;
                let to = a.string(0, "a new name")?;
                let drop_target = a.opt_bool(1)?.unwrap_or(false);
                Ok(Statement::Run(ConsoleCommand::new(
                    "admin",
                    mongo::rename_collection(db, coll, &to, drop_target),
                    Output::Reply,
                )))
            }
            "stats" => {
                a.at_most(1)?;
                run(mongo::coll_stats(coll), Output::First)
            }
            "validate" => {
                a.at_most(1)?;
                let mut fields = Document::new();
                match a.get(0) {
                    None => {}
                    Some(Bson::Boolean(full)) => {
                        fields.insert("full", *full);
                    }
                    Some(Bson::Document(o)) => {
                        fields.extend(known(&a, o.clone(), &["full", "repair"])?)
                    }
                    Some(_) => return Err(a.err("takes true or an options document")),
                }
                run(mongo::command_with("validate", coll, fields), Output::Reply)
            }
            "hideIndex" | "unhideIndex" => {
                a.at_least(1)?;
                a.at_most(1)?;
                let mut index = match a.get(0) {
                    Some(Bson::String(name)) => doc! { "name": name.clone() },
                    Some(Bson::Document(keys)) => doc! { "keyPattern": keys.clone() },
                    _ => return Err(a.err("takes an index name or keys document")),
                };
                index.insert("hidden", a.method == "hideIndex");
                run(
                    mongo::command_with("collMod", coll, doc! { "index": index }),
                    Output::Reply,
                )
            }
            "watch"
            | "mapReduce"
            | "group"
            | "initializeOrderedBulkOp"
            | "initializeUnorderedBulkOp"
            | "getShardDistribution" => {
                Err(a.err("is not supported in the console (it needs the full mongosh shell)"))
            }
            _ => Err(a.err(format!(
                "is not a collection method the console knows (db.{coll}.{}() is not supported)",
                a.method
            ))),
        }
    }

    /// The cursor chain of `find`: modifiers, then the end (`count()`, `explain()`…).
    fn find_chain(
        &mut self,
        db: &str,
        coll: &str,
        filter: Document,
        mut options: Document,
    ) -> Result<Statement> {
        let run = |command, output| Ok(Statement::Run(ConsoleCommand::new(db, command, output)));
        loop {
            if !self.lx.eat('.')? {
                break;
            }
            let (at, m) = self.name()?;
            if !CURSOR_METHODS.contains(&m.as_str()) {
                return Err(ParseError::new(
                    at,
                    format!(
                        "{m}(): is not a cursor method the console knows (no JavaScript callbacks)"
                    ),
                ));
            }
            let a = self.call(&m, at)?;
            match m.as_str() {
                "sort" | "project" | "projection" | "collation" | "min" | "max" => {
                    a.at_least(1)?;
                    a.at_most(1)?;
                    let key = if m == "project" {
                        "projection"
                    } else {
                        m.as_str()
                    };
                    options.insert(key, Bson::Document(a.doc(0, "a")?));
                }
                "skip" | "batchSize" | "maxTimeMS" => {
                    a.at_least(1)?;
                    a.at_most(1)?;
                    let n = a.int(0, "a whole number")?;
                    if n < 0 {
                        return Err(a.err("takes a number of 0 or more"));
                    }
                    options.insert(m.as_str(), Bson::Int64(n));
                }
                "limit" => {
                    a.at_least(1)?;
                    a.at_most(1)?;
                    // A negative limit is one batch of that many, as mongosh reads it.
                    let n = a.int(0, "a whole number")?;
                    options.remove("singleBatch");
                    if n == 0 {
                        options.remove("limit");
                    } else {
                        options.insert("limit", Bson::Int64(n.saturating_abs()));
                        if n < 0 {
                            options.insert("singleBatch", true);
                        }
                    }
                }
                "comment" | "hint" => {
                    a.at_least(1)?;
                    a.at_most(1)?;
                    match a.get(0) {
                        Some(v @ (Bson::String(_) | Bson::Document(_))) => {
                            options.insert(m.as_str(), v.clone());
                        }
                        _ => return Err(a.err("takes a string or a document")),
                    }
                }
                "allowDiskUse" | "noCursorTimeout" | "returnKey" | "showRecordId" => {
                    a.at_most(1)?;
                    options.insert(m.as_str(), Bson::Boolean(a.opt_bool(0)?.unwrap_or(true)));
                }
                "readConcern" => {
                    a.at_least(1)?;
                    a.at_most(1)?;
                    let level = a.string(0, "a level")?;
                    options.insert("readConcern", doc! { "level": level });
                }
                "toArray" | "pretty" => a.at_most(0)?,
                "count" | "size" => {
                    a.at_most(1)?;
                    let apply = m == "size" || a.opt_bool(0)?.unwrap_or(false);
                    let mut count_opts =
                        take(&mut options, &["hint", "maxTimeMS", "collation", "comment"]);
                    if apply {
                        count_opts.extend(take(&mut options, &["skip", "limit"]));
                    }
                    let st = run(mongo::count_with(coll, filter, count_opts), Output::Count)?;
                    self.tail_pretty()?;
                    return Ok(st);
                }
                "explain" => {
                    a.at_most(1)?;
                    let verbosity = verbosity(&a, a.get(0))?;
                    let command =
                        mongo::explain(mongo::find_with(coll, filter, options), verbosity);
                    let st = run(command, Output::Reply)?;
                    self.tail_pretty()?;
                    return Ok(st);
                }
                _ => {
                    return Err(a.err(
                        "is not a cursor method the console knows (no JavaScript callbacks)",
                    ));
                }
            }
        }
        run(mongo::find_with(coll, filter, options), Output::Cursor)
    }

    /// `aggregate(…).explain()`; `.toArray()` and `.pretty()` are left to `tail`.
    fn aggregate_chain(&mut self, db: &str, st: Statement) -> Result<Statement> {
        let save = self.lx.pos;
        if !self.lx.eat('.')? {
            return Ok(st);
        }
        let (at, m) = self.name()?;
        if m != "explain" {
            self.lx.pos = save;
            return Ok(st);
        }
        let a = self.call(&m, at)?;
        a.at_most(1)?;
        let verbosity = verbosity(&a, a.get(0))?;
        let Statement::Run(c) = st else {
            return Err(a.err("cannot be explained"));
        };
        Ok(Statement::Run(ConsoleCommand::new(
            db,
            mongo::explain(c.command, verbosity),
            Output::Reply,
        )))
    }

    /// `bulkWrite([{insertOne: {document}}, {updateOne: {filter, update}}, …], {ordered})`: runs
    /// of the same kind become one command each, in order.
    fn bulk_write(&mut self, db: &str, coll: &str, a: &Args) -> Result<Statement> {
        a.at_least(1)?;
        a.at_most(2)?;
        let ops = a.array(0, "an array of operations")?;
        let mut options = a.options(1)?;
        let ordered = options.remove("ordered").is_none_or(|b| truthy(&b));
        let command_options = known(a, options, COMMAND_WRITE_OPTIONS)?;
        enum Run {
            Insert(Vec<Document>),
            Update(Vec<Document>),
            Delete(Vec<Document>),
        }
        let mut runs: Vec<Run> = Vec::new();
        for op in ops {
            let Bson::Document(op) = op else {
                return Err(a.err("takes operation documents"));
            };
            let (Some((kind, Bson::Document(spec))), 1) = (op.iter().next(), op.len()) else {
                return Err(a.err("each operation is {insertOne: {…}}, {updateOne: {…}}…"));
            };
            let mut spec = spec.clone();
            match kind.as_str() {
                "insertOne" => {
                    let Some(Bson::Document(d)) = spec.remove("document") else {
                        return Err(a.err("insertOne takes {document}"));
                    };
                    match runs.last_mut() {
                        Some(Run::Insert(docs)) => docs.push(d),
                        _ => runs.push(Run::Insert(vec![d])),
                    }
                }
                "updateOne" | "updateMany" | "replaceOne" => {
                    let q = match spec.remove("filter") {
                        Some(Bson::Document(q)) => q,
                        _ => return Err(a.err(format!("{kind} takes a filter"))),
                    };
                    let key = if kind == "replaceOne" {
                        "replacement"
                    } else {
                        "update"
                    };
                    let u = match spec.remove(key) {
                        Some(Bson::Document(u))
                            if (kind == "replaceOne" && mongo::is_replacement(&u))
                                || (kind != "replaceOne" && mongo::is_update_document(&u)) =>
                        {
                            Bson::Document(u)
                        }
                        Some(Bson::Array(p)) if kind != "replaceOne" => Bson::Array(p),
                        _ => return Err(a.err(format!("{kind} takes a valid {key}"))),
                    };
                    let statement_options = take(&mut spec, UPDATE_STATEMENT_OPTIONS);
                    if let Some(k) = spec.keys().next() {
                        return Err(a.err(format!("unknown option `{k}` of {kind}")));
                    }
                    let s = mongo::update_statement(q, u, kind == "updateMany", statement_options);
                    match runs.last_mut() {
                        Some(Run::Update(list)) => list.push(s),
                        _ => runs.push(Run::Update(vec![s])),
                    }
                }
                "deleteOne" | "deleteMany" => {
                    let q = match spec.remove("filter") {
                        Some(Bson::Document(q)) => q,
                        _ => return Err(a.err(format!("{kind} takes a filter"))),
                    };
                    let statement_options = take(&mut spec, DELETE_STATEMENT_OPTIONS);
                    if let Some(k) = spec.keys().next() {
                        return Err(a.err(format!("unknown option `{k}` of {kind}")));
                    }
                    let s = mongo::delete_statement(
                        q,
                        i32::from(kind == "deleteOne"),
                        statement_options,
                    );
                    match runs.last_mut() {
                        Some(Run::Delete(list)) => list.push(s),
                        _ => runs.push(Run::Delete(vec![s])),
                    }
                }
                _ => return Err(a.err(format!("unknown operation `{kind}`"))),
            }
        }
        if runs.is_empty() {
            return Err(a.err("takes at least one operation"));
        }
        let commands = runs
            .into_iter()
            .map(|r| match r {
                Run::Insert(docs) => {
                    let (command, ids) =
                        mongo::insert_many(coll, docs, ordered, command_options.clone());
                    ConsoleCommand::new(db, command, Output::Inserted(ids))
                }
                Run::Update(list) => ConsoleCommand::new(
                    db,
                    mongo::write_with("update", coll, list, ordered, command_options.clone()),
                    Output::Updated,
                ),
                Run::Delete(list) => ConsoleCommand::new(
                    db,
                    mongo::write_with("delete", coll, list, ordered, command_options.clone()),
                    Output::Deleted,
                ),
            })
            .collect();
        Ok(Statement::Batch { commands, ordered })
    }
}

/// The names of the indexes of a `createIndexes` command.
fn index_names(command: &Document) -> Vec<Bson> {
    match command.get("indexes") {
        Some(Bson::Array(specs)) => specs
            .iter()
            .filter_map(|s| match s {
                Bson::Document(s) => s.get("name").cloned(),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `explain()`'s verbosity: a name, or `true` for all plans.
fn verbosity(a: &Args, v: Option<&Bson>) -> Result<&'static str> {
    match v {
        None | Some(Bson::Boolean(false)) => Ok("queryPlanner"),
        Some(Bson::Boolean(true)) => Ok("allPlansExecution"),
        Some(Bson::String(s)) => match s.as_str() {
            "queryPlanner" => Ok("queryPlanner"),
            "executionStats" => Ok("executionStats"),
            "allPlansExecution" => Ok("allPlansExecution"),
            _ => Err(a.err("takes queryPlanner, executionStats or allPlansExecution")),
        },
        Some(_) => Err(a.err("takes a verbosity")),
    }
}
