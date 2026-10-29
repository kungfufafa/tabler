//! Generated tests of the portable classifier and splitter, per extension dialect.
//!
//! A dialect's file (`gen/<d>.rs`) builds a `Suite` from its spec, adds what only its engine has
//! (contexts, a corpus of ordinary reads, write forms, fuzz vocabulary) and runs:
//!
//! - `Suite::check`: every write its rules list (a `writes` keyword, a `write_calls` call) is a
//!   write in every active context (`active_contexts` plus the suite's own) and in no inert one
//!   (`inert_contexts`: strings, quoted names, comments, plus the suite's own), where the text
//!   stays one statement (and, without terminators, a write keyword after the first word makes
//!   it several); unknown text is a write; every read of the corpus stays a read, also
//!   re-spaced, re-cased and next to another statement, and the corpus starts with every read
//!   keyword of the rules; every write form is a write. The cases go to
//!   `tests/fixtures/classify-generated/<name>.json`, which the renderer's portable classifier
//!   can run too; `TABLER_BLESS=1` rewrites it (review the diff).
//! - `Suite::fuzz`: random scripts over the dialect's vocabulary (fixed seed, no crates) must not
//!   panic or hang; a write keyword or call the scanner reads as code makes a SELECT a write; a
//!   statement that writes makes its script a write (and where every reading scans the script
//!   alike, the script is exactly as severe as its worst statement); splitting neither loses nor
//!   duplicates text.
//!
//! `Suite::check_contexts` runs the shared and the suite's contexts only (the skeletons of the
//! lanes that have not filled in their corpus yet).
//!
//! Classes are asked of both the spec's `classify` and `portable::classify::with` over its lexer
//! profile and rules, which must agree: the renderer has only the latter.
//!
//! The module is `generated` (`gen` is a reserved word): `cargo test --lib
//! sqlgen::portable::generated::<d>` runs one dialect's.

use std::fmt::Write as _;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::sqlgen::classify::StatementClass::{self, Read, Write};
use crate::sqlgen::dialect::lex::{ClassifyRules, LexProfile, Reading, TokenKind, listed, scan};
use crate::sqlgen::dialect::spec::DialectSpec;
use crate::sqlgen::portable::classify::{self, calls_listed_function, code_only};
use crate::sqlgen::portable::split::{first_reading, is_single_statement, split};

mod ansi;
#[cfg(feature = "drv-bigquery")]
mod bigquery;
#[cfg(feature = "drv-clickhouse")]
mod clickhouse;
#[cfg(feature = "drv-cassandra")]
mod cql;
#[cfg(feature = "drv-duckdb")]
mod duckdb;
#[cfg(feature = "drv-pg-flavors")]
mod redshift;
#[cfg(feature = "drv-snowflake")]
mod snowflake;
#[cfg(feature = "drv-sqlserver")]
mod tsql;
mod tsql_like;
#[cfg(feature = "drv-vertica")]
mod vertica;

/// Where a token is embedded: `before`, the token, `after`.
#[derive(Debug, Clone)]
pub struct Context {
    pub name: String,
    pub before: String,
    pub after: String,
}

impl Context {
    pub fn embed(&self, token: &str) -> String {
        format!("{}{token}{}", self.before, self.after)
    }
}

pub fn ctx(
    name: impl Into<String>,
    before: impl Into<String>,
    after: impl Into<String>,
) -> Context {
    Context {
        name: name.into(),
        before: before.into(),
        after: after.into(),
    }
}

/// A write the rules list: a keyword of `writes`, or a function of `write_calls` (spelt as a
/// call, `name(1)`). A `PREFIX*` entry gets a name it matches.
#[derive(Debug, Clone)]
pub struct WriteToken {
    pub name: String,
    pub call: bool,
}

impl WriteToken {
    /// The token as a statement spells it, in one of `SPELLINGS`.
    pub fn text(&self, spelling: usize) -> String {
        let name = spell(&self.name, spelling);
        if self.call {
            format!("{name}(1)")
        } else {
            name
        }
    }
}

pub fn write_tokens(rules: &ClassifyRules) -> Vec<WriteToken> {
    let name = |entry: &str| match entry.strip_suffix('*') {
        // `SHUTDOWN*` matches `SHUTDOWN` itself; `XP_*` needs something after the `_`.
        Some(prefix) if prefix.ends_with(|c: char| c.is_alphanumeric()) => prefix.to_string(),
        Some(prefix) => format!("{prefix}X"),
        None => entry.to_string(),
    };
    let keywords = rules.writes.iter().map(|w| WriteToken {
        name: name(w),
        call: false,
    });
    let calls = rules.write_calls.iter().map(|w| WriteToken {
        name: name(w),
        call: true,
    });
    keywords.chain(calls).collect()
}

/// Upper case (as listed), lower case, and alternating.
pub const SPELLINGS: usize = 3;

fn spell(s: &str, spelling: usize) -> String {
    match spelling % SPELLINGS {
        0 => s.to_uppercase(),
        1 => s.to_lowercase(),
        _ => s
            .chars()
            .enumerate()
            .flat_map(|(i, c)| {
                if i.is_multiple_of(2) {
                    c.to_lowercase().collect::<Vec<_>>()
                } else {
                    c.to_uppercase().collect::<Vec<_>>()
                }
            })
            .collect(),
    }
}

/// Numbers a keyword may be glued to: a server may end the number before it (T-SQL and the
/// PostgreSQL lexers before 15 read `1DELETE` as `1` then `DELETE`), wherever that is.
const NUMBERS: &[&str] = &[
    "1", "12", "1.5", ".5", "1.", "1e3", "1e", "1.5e-3", "1E+3", "1.e5", "0x1F", "0x", "0b101",
    "X'1F'", "1_000", "$1",
];

/// Whitespace a scanner must skip like a space; some servers take only some of it.
const WHITESPACE: &[(&str, &str)] = &[
    ("tab", "\t"),
    ("CR", "\r"),
    ("CRLF", "\r\n"),
    ("vertical tab", "\u{0B}"),
    ("form feed", "\u{0C}"),
    ("no-break space", "\u{A0}"),
    ("em space", "\u{2003}"),
    ("ideographic space", "\u{3000}"),
    ("next line", "\u{85}"),
    ("line separator", "\u{2028}"),
];

/// The identifier quotes of the dialect, then the usual ones it lacks (where they are plain
/// punctuation, a keyword after them is code all the same).
fn ident_forms(lex: &LexProfile) -> Vec<(char, char)> {
    let mut forms = lex.ident_quotes.to_vec();
    for f in [('"', '"'), ('[', ']'), ('`', '`')] {
        if !forms.contains(&f) && !lex.string_quotes.contains(&f.0) {
            forms.push(f);
        }
    }
    forms
}

/// Contexts in which a write keyword or call must make the text a write, for any dialect: the
/// start of a statement, after another one, in a query, glued to numbers, strings, quoted names,
/// parentheses, dollar bodies and comments, past a quote a reading of backslashes ends the string
/// at, in subqueries and CTEs, in qualified names, and between unusual whitespace.
pub fn active_contexts(lex: &LexProfile, rules: &ClassifyRules) -> Vec<Context> {
    let mut out = vec![
        ctx("at the start", "", " t"),
        ctx("at the start after a line comment", "-- c\n", " t"),
        ctx("at the start after a block comment", "/* c */", " t"),
        ctx("at the start in parentheses", "(", " t)"),
        ctx("after a statement and ;", "SELECT 1;", " t"),
        ctx("after a statement, ; and a space", "SELECT 1; ", " t"),
        ctx("after a statement, ; and a line break", "SELECT 1;\n", " t"),
        ctx("in a query after a space", "SELECT 1 ", " t"),
        ctx("in a query after a line break", "SELECT 1\n", " t"),
    ];
    for q in rules.queries.iter().filter(|q| !q.ends_with('*')) {
        out.push(ctx(
            format!("after the query keyword {q}"),
            format!("{q} "),
            " t",
        ));
    }
    if lex.terminator_optional {
        // Without terminators, any statement may run on into the next one.
        for i in rules.inspections.iter().filter(|i| !i.ends_with('*')) {
            out.push(ctx(
                format!("after the inspection {i}"),
                format!("{i} x "),
                " t",
            ));
        }
    }
    if listed(rules.inspections, "EXPLAIN") {
        out.push(ctx("after EXPLAIN ANALYZE", "EXPLAIN ANALYZE ", " t"));
    }
    for n in NUMBERS {
        out.push(ctx(
            format!("glued to the number {n}"),
            format!("SELECT {n}"),
            " t",
        ));
    }
    for &q in lex.string_quotes {
        out.push(ctx(
            format!("glued to a {q}string{q}"),
            format!("SELECT {q}a{q}"),
            " t",
        ));
        if lex.triple_quotes {
            out.push(ctx(
                format!("glued to a triple-quoted {q}string{q}"),
                format!("SELECT {q}{q}{q}a{q}{q}{q}"),
                " t",
            ));
        }
        if lex.raw_prefixes {
            // A backslash keeps the quote after it in a raw string too.
            out.push(ctx(
                format!("glued to a raw {q}string{q} past \\{q}"),
                format!("SELECT r{q}a\\{q}{q}"),
                " t",
            ));
        }
        if lex.byte_prefixes {
            out.push(ctx(
                format!("glued to a byte {q}string{q}"),
                format!("SELECT b{q}a{q}"),
                " t",
            ));
        }
    }
    if lex.e_strings {
        out.push(ctx("glued to an E-string", "SELECT E'a'", " t"));
    }
    for (o, c) in ident_forms(lex) {
        out.push(ctx(
            format!("glued to a {o}name{c}"),
            format!("SELECT {o}a{c}"),
            " t",
        ));
    }
    out.push(ctx("glued to a parenthesis", "SELECT (1)", " t"));
    if lex.dollar_quotes {
        out.push(ctx("glued to a $$ body", "SELECT $$a$$", " t"));
        out.push(ctx(
            "after a word ending in $$, which opens no body",
            "SELECT a$$ ",
            " t $$",
        ));
        if lex.dollar_tags {
            out.push(ctx("glued to a $tag$ body", "SELECT $q$a$q$", " t"));
        } else {
            out.push(ctx(
                "after $q$, which opens no body here",
                "SELECT $q$ ",
                " t $q$",
            ));
        }
    } else {
        out.push(ctx(
            "after $$, which opens no body here",
            "SELECT $$ ",
            " t $$",
        ));
    }
    out.extend([
        ctx("after a line comment", "SELECT 1 -- c\n", " t"),
        ctx("after a line comment and CRLF", "SELECT 1 -- c\r\n", " t"),
        ctx(
            "after a line comment a lone CR may end",
            "SELECT 1 -- c\r",
            " t",
        ),
        ctx("glued to a block comment", "SELECT 1 /* c */", " t"),
        ctx("glued to an empty block comment", "SELECT 1/**/", " t"),
        // Ends at the first `*/` where comments do not nest, at the last where they do.
        ctx(
            "after a nested block comment",
            "SELECT 1 /* a /* b */ c */ ",
            " t",
        ),
    ]);
    if lex.hash_comments {
        out.push(ctx("after a # comment", "SELECT 1 # c\n", " t"));
    } else {
        out.push(ctx(
            "after a #, which starts no comment here",
            "SELECT 1 # ",
            " t",
        ));
    }
    if lex.slash_slash_comments {
        out.push(ctx("after a // comment", "SELECT 1 // c\n", " t"));
    } else {
        out.push(ctx(
            "after a //, which starts no comment here",
            "SELECT 1 // ",
            " t",
        ));
    }
    if lex.dash_comment_needs_space {
        out.push(ctx(
            "after a --, which needs a space here",
            "SELECT 1 --",
            " t",
        ));
    }
    // A string ends at `\'` under a reading without backslash escapes, and goes on past it under
    // one with them: the text after it is code to one of them.
    let plain_strings = lex.readings.iter().any(|r| !r.string_backslash);
    let escaping_strings = lex.readings.iter().any(|r| r.string_backslash);
    for &q in lex.string_quotes {
        if plain_strings {
            out.push(ctx(
                format!("after {q}a\\{q}, a string a reading ends at the quote"),
                format!("SELECT {q}a\\{q} "),
                format!(" t --{q}"),
            ));
        }
        if escaping_strings {
            out.push(ctx(
                format!("after {q}a\\{q}{q}, a string a reading escapes the quote in"),
                format!("SELECT {q}a\\{q}{q} "),
                " t",
            ));
        }
    }
    if lex.e_strings {
        out.push(ctx(
            "after E'a\\'', whose quote is escaped",
            "SELECT E'a\\'' ",
            " t",
        ));
    }
    let plain_idents = lex.readings.iter().any(|r| !r.ident_backslash);
    let escaping_idents = lex.readings.iter().any(|r| r.ident_backslash);
    for &(o, c) in lex.ident_quotes {
        if plain_idents {
            out.push(ctx(
                format!("after {o}a\\{c}, a name a reading ends at the quote"),
                format!("SELECT {o}a\\{c} "),
                format!(" t --{c}"),
            ));
        }
        if escaping_idents {
            out.push(ctx(
                format!("after {o}a\\{c}{c}, a name a reading escapes the quote in"),
                format!("SELECT {o}a\\{c}{c} "),
                " t",
            ));
        }
    }
    out.extend([
        ctx("in a derived table", "SELECT * FROM (SELECT 1 ", " t) s"),
        ctx(
            "in an IN subquery",
            "SELECT * FROM t WHERE a IN (SELECT ",
            " t)",
        ),
        ctx(
            "in an EXISTS subquery",
            "SELECT 1 WHERE EXISTS (SELECT ",
            " t)",
        ),
        ctx("in a scalar subquery", "SELECT (SELECT ", " t) FROM u"),
        ctx(
            "in a function's arguments",
            "SELECT coalesce(",
            ", 1) FROM t",
        ),
        ctx("in a CTE", "WITH c AS (SELECT 1 ", " t) SELECT * FROM c"),
        ctx("after a CTE", "WITH c AS (SELECT 1) SELECT * FROM c ", " t"),
        ctx("after UNION ALL", "SELECT 1 UNION ALL SELECT ", " t"),
        ctx(
            "as the last part of a qualified name",
            "SELECT s.",
            " FROM t",
        ),
        ctx(
            "as the first part of a qualified name",
            "SELECT ",
            ".a FROM t",
        ),
    ]);
    if let Some(&(o, c)) = lex.ident_quotes.first() {
        out.push(ctx(
            format!("after a {o}quoted{c} qualifier"),
            format!("SELECT {o}s{c}."),
            " FROM t",
        ));
    }
    for (label, ws) in WHITESPACE {
        out.push(ctx(
            format!("between {label}s"),
            format!("SELECT 1{ws}"),
            format!("{ws}t"),
        ));
        out.push(ctx(
            format!("at the start after a {label}"),
            *ws,
            format!("{ws}t"),
        ));
    }
    out.extend(ambiguous_comment_contexts(lex));
    if lex.begin_end_bodies {
        // A bare `begin` opens a block depth for the splitter, so the `;` stays inside one
        // statement: the classifier must still judge each `;`-separated part.
        for i in rules.inspections.iter().filter(|i| !i.ends_with('*')) {
            out.push(ctx(
                format!("after {i} begin x and ;"),
                format!("{i} begin x; "),
                " t",
            ));
        }
    }
    out
}

/// Contexts where the token is a string or a comment to the scanner but code to a server that
/// ends a comment elsewhere (`Scan::ambiguous`): only the ambiguity rule makes these writes.
fn ambiguous_comment_contexts(lex: &LexProfile) -> Vec<Context> {
    let mut openers = vec!["--"];
    if lex.hash_comments {
        openers.push("#");
    }
    if lex.slash_slash_comments {
        openers.push("//");
    }
    let mut out = Vec::new();
    for o in openers {
        // The scanner ends the comment at a lone CR and reads a string after it; a server that
        // ends it only at LF reads the token as code.
        if let Some(&q) = lex.string_quotes.first() {
            out.push(ctx(
                format!("after a {o} comment a lone CR may end, inside a {q}"),
                format!("SELECT 1 {o} c\r{q}\n"),
                format!(" t {o}{q}"),
            ));
        }
        // The scanner's comment runs on past the break; a server that ends it there does not.
        for (name, b) in [
            ("VT", "\u{0B}"),
            ("FF", "\u{0C}"),
            ("NEL", "\u{85}"),
            ("LS", "\u{2028}"),
            ("PS", "\u{2029}"),
        ] {
            out.push(ctx(
                format!("in a {o} comment after a {name} a server may end it at"),
                format!("SELECT 1 {o} c{b}"),
                " t\n",
            ));
        }
    }
    // A block comment holding `/*`: nested for the scanner, flat for a server, or the other way.
    out.push(ctx(
        "in a block comment holding /*, which a flat reader has ended",
        "SELECT 1 /* /* */ ",
        " t */",
    ));
    if let Some(&q) = lex.string_quotes.first() {
        out.push(ctx(
            format!("after a block comment holding /*, inside a {q}"),
            format!("/* /* */ SELECT {q} */ "),
            format!(" t --{q}"),
        ));
    }
    out
}

/// Ways to call a function the classifier must see through, as contexts of its bare name: a
/// quoted name, and space, a comment or a line break before the `(`.
fn call_contexts(lex: &LexProfile) -> Vec<Context> {
    let mut out = vec![
        ctx("called after a space", "SELECT ", " (1) FROM t"),
        ctx("called after a comment", "SELECT ", "/* c */(1) FROM t"),
        ctx("called after a line break", "SELECT ", "\n(1) FROM t"),
        ctx("called by a qualified name", "SELECT s.", "(1) FROM t"),
        ctx("called glued to a number", "SELECT 1", "(1) FROM t"),
    ];
    for &(o, c) in lex.ident_quotes {
        out.push(ctx(
            format!("called by a {o}quoted{c} name"),
            format!("SELECT {o}"),
            format!("{c}(1) FROM t"),
        ));
    }
    out
}

/// Contexts in which no write keyword or call may make a read a write, for any dialect: every
/// string, quoted-name, dollar-body and comment form the profile has, also past a backslash the
/// dialect's readings all read alike.
pub fn inert_contexts(lex: &LexProfile) -> Vec<Context> {
    let mut out = Vec::new();
    for &q in lex.string_quotes {
        out.push(ctx(
            format!("in a {q}string{q}"),
            format!("SELECT {q}"),
            format!(" t{q} FROM x"),
        ));
        out.push(ctx(
            format!("in a {q}string{q} over lines"),
            format!("SELECT {q}a\n"),
            format!("\nt{q} FROM x"),
        ));
        out.push(ctx(
            format!("in a {q}string{q} past a doubled quote"),
            format!("SELECT {q}a{q}{q} "),
            format!(" t{q} FROM x"),
        ));
        if lex.triple_quotes {
            out.push(ctx(
                format!("in a triple-quoted {q}string{q}"),
                format!("SELECT {q}{q}{q}{q} "),
                format!("\n t{q}{q}{q} FROM x"),
            ));
        }
        if lex.raw_prefixes {
            for p in ["r", "R", "rb", "br"] {
                out.push(ctx(
                    format!("in a {p}{q}string{q}"),
                    format!("SELECT {p}{q}"),
                    format!(" t{q} FROM x"),
                ));
            }
            out.push(ctx(
                format!("in a raw {q}string{q} past \\{q}"),
                format!("SELECT r{q}a\\{q} "),
                format!(" t{q} FROM x"),
            ));
        }
        if lex.byte_prefixes {
            out.push(ctx(
                format!("in a b{q}string{q}"),
                format!("SELECT b{q}"),
                format!(" t{q} FROM x"),
            ));
        }
    }
    if lex.e_strings {
        out.push(ctx("in an E-string", "SELECT E'", " t' FROM x"));
        out.push(ctx(
            "in an E-string past \\'",
            "SELECT E'a\\' ",
            " t' FROM x",
        ));
    }
    for &(o, c) in lex.ident_quotes {
        out.push(ctx(
            format!("in a {o}name{c}"),
            format!("SELECT {o}"),
            format!(" t{c} FROM x"),
        ));
        out.push(ctx(
            format!("in a {o}name{c} past a doubled quote"),
            format!("SELECT {o}a{c}{c} "),
            format!(" t{c} FROM x"),
        ));
    }
    if lex.dollar_quotes {
        out.push(ctx("in a $$ body", "SELECT $$", " t$$ FROM x"));
        out.push(ctx(
            "in a $$ body over lines",
            "SELECT $$\n",
            "\n;t$$ FROM x",
        ));
        if lex.dollar_tags {
            out.push(ctx("in a $tag$ body", "SELECT $q$", " t$q$ FROM x"));
            out.push(ctx(
                "in a $tag$ body holding $$",
                "SELECT $q$ $$ ",
                " $$ $q$ FROM x",
            ));
        }
    }
    out.extend([
        ctx("in a line comment", "SELECT 1 -- ", " t\nFROM x"),
        ctx(
            "in a line comment ended by CRLF",
            "SELECT 1 -- ",
            " t\r\nFROM x",
        ),
        ctx("in a line comment at the end", "SELECT 1 FROM x -- ", " t"),
        ctx("in a leading line comment", "-- ", " t\nSELECT 1"),
        ctx("in a block comment", "SELECT 1 /* ", " t */ FROM x"),
        ctx(
            "in a block comment over lines",
            "SELECT 1 /*\n",
            "\nt */ FROM x",
        ),
        ctx("in a leading block comment", "/* ", " t */ SELECT 1"),
    ]);
    if lex.hash_comments {
        out.push(ctx("in a # comment", "SELECT 1 # ", " t\nFROM x"));
    }
    if lex.slash_slash_comments {
        out.push(ctx("in a // comment", "SELECT 1 // ", " t\nFROM x"));
    }
    // Past a backslash, where every reading of the dialect reads it alike.
    let all = |f: fn(&Reading) -> bool| lex.readings.iter().all(f);
    for &q in lex.string_quotes {
        if all(|r| r.string_backslash) {
            out.push(ctx(
                format!("in a {q}string{q} past \\{q}"),
                format!("SELECT {q}a\\{q} "),
                format!(" t{q} FROM x"),
            ));
        }
        if all(|r| !r.string_backslash) {
            out.push(ctx(
                format!("in a {q}string{q} past \\{q}{q}"),
                format!("SELECT {q}a\\{q}{q} "),
                format!(" t{q} FROM x"),
            ));
        }
    }
    for &(o, c) in lex.ident_quotes {
        if all(|r| r.ident_backslash) {
            out.push(ctx(
                format!("in a {o}name{c} past \\{c}"),
                format!("SELECT {o}a\\{c} "),
                format!(" t{c} FROM x"),
            ));
        }
        if all(|r| !r.ident_backslash) {
            out.push(ctx(
                format!("in a {o}name{c} past \\{c}{c}"),
                format!("SELECT {o}a\\{c}{c} "),
                format!(" t{c} FROM x"),
            ));
        }
    }
    out
}

/// Text that is no statement the rules know: a write by default.
const UNKNOWN: &[&str] = &[
    "", " ", "\n", ";", ";;", "-- c", "/* c */", "x", "1", "'a'", "\"a\"", "(", ")", "FOO BAR",
];

/// A generated case: the text and the class it must get.
#[derive(Debug, Clone)]
pub struct Case {
    pub why: String,
    pub sql: String,
    pub class: StatementClass,
    /// What `is_single_statement` must say, where the case decides it.
    pub single: Option<bool>,
    /// Recorded in the fixture (a rotating subset of the contexts' cases, the corpus as
    /// written, unknown text).
    pub record: bool,
}

fn case(why: impl Into<String>, sql: String, class: StatementClass, record: bool) -> Case {
    Case {
        why: why.into(),
        sql,
        class,
        single: None,
        record,
    }
}

/// A dialect's generated tests: its spec and what only its engine has.
#[derive(Debug, Clone)]
pub struct Suite {
    /// The fixture's name: `tests/fixtures/classify-generated/<name>.json`.
    pub name: &'static str,
    pub spec: &'static DialectSpec,
    /// The dialect's own contexts in which every write must count.
    pub active: Vec<Context>,
    /// The dialect's own contexts in which no write may count.
    pub inert: Vec<Context>,
    /// Ordinary reads of the engine.
    pub reads: Vec<&'static str>,
    /// Whole statements that must be writes, beyond the rules' words.
    pub writes: Vec<&'static str>,
    /// Atoms the fuzzer draws from, beside the shared ones and the rules' words.
    pub vocab: Vec<&'static str>,
}

impl Suite {
    pub fn new(name: &'static str, spec: &'static DialectSpec) -> Suite {
        Suite {
            name,
            spec,
            active: Vec::new(),
            inert: Vec::new(),
            reads: Vec::new(),
            writes: Vec::new(),
            vocab: Vec::new(),
        }
    }

    pub fn active(mut self, name: &str, before: &str, after: &str) -> Suite {
        self.active.push(ctx(name, before, after));
        self
    }

    pub fn inert(mut self, name: &str, before: &str, after: &str) -> Suite {
        self.inert.push(ctx(name, before, after));
        self
    }

    pub fn reads(mut self, reads: &[&'static str]) -> Suite {
        self.reads.extend_from_slice(reads);
        self
    }

    pub fn writes(mut self, writes: &[&'static str]) -> Suite {
        self.writes.extend_from_slice(writes);
        self
    }

    pub fn vocab(mut self, atoms: &[&'static str]) -> Suite {
        self.vocab.extend_from_slice(atoms);
        self
    }

    fn lex(&self) -> &'static LexProfile {
        self.spec.lex
    }

    fn rules(&self) -> &'static ClassifyRules {
        self.spec.rules
    }

    /// The cases of the contexts: each write token in each active context (and each call in
    /// each call context), in every spelling; each in each inert context; unknown text.
    ///
    /// An inert case is one statement. Where terminators are optional, a write keyword after
    /// the first word may start a second one, so such a case is not.
    pub fn context_cases(&self) -> Vec<Case> {
        let (lex, rules) = (self.lex(), self.rules());
        let tokens = write_tokens(rules);
        let mut active = active_contexts(lex, rules);
        active.extend(self.active.iter().cloned());
        let mut inert = inert_contexts(lex);
        inert.extend(self.inert.iter().cloned());
        let calls = call_contexts(lex);
        // The fixture keeps about `FIXTURE_TOKENS` tokens per context, rotating, in one
        // spelling: every context and token is in it, and it stays small.
        let stride = tokens.len().div_ceil(FIXTURE_TOKENS).max(1);
        let record = |ti: usize, ci: usize, s: usize| {
            (ti + ci).is_multiple_of(stride) && (ti + ci) / stride % SPELLINGS == s
        };
        let mut out = Vec::new();
        for (ti, t) in tokens.iter().enumerate() {
            for (ci, c) in active.iter().enumerate() {
                for s in 0..SPELLINGS {
                    let mut k = case(
                        format!("{} {}", t.name, c.name),
                        c.embed(&t.text(s)),
                        Write,
                        record(ti, ci, s),
                    );
                    let first = first_word(lex, &k.sql);
                    if lex.terminator_optional && !t.call && first.as_deref() != Some(&t.name) {
                        k.single = Some(false);
                    }
                    out.push(k);
                }
            }
            if t.call {
                for (ci, c) in calls.iter().enumerate() {
                    let s = (ti + ci) % SPELLINGS;
                    out.push(case(
                        format!("{} {}", t.name, c.name),
                        c.embed(&spell(&t.name, s)),
                        Write,
                        (ti + ci).is_multiple_of(stride),
                    ));
                }
            }
            for (ci, c) in inert.iter().enumerate() {
                for s in 0..SPELLINGS {
                    let mut k = case(
                        format!("{} {}", t.name, c.name),
                        c.embed(&t.text(s)),
                        Read,
                        record(ti, ci, s),
                    );
                    k.single = Some(true);
                    out.push(k);
                }
            }
        }
        for u in UNKNOWN {
            out.push(case("unknown text", (*u).to_string(), Write, true));
        }
        out
    }

    /// The cases of the corpus: each read as written, re-spaced, re-cased, after another
    /// statement and with a terminator; each write form as written, re-spaced, re-cased and
    /// after another statement (also with no terminator before it, where none is needed).
    pub fn corpus_cases(&self) -> Vec<Case> {
        let lex = self.lex();
        let mut out = Vec::new();
        let mut variants = |sql: &str, class: StatementClass, what: &str| {
            out.push(case(what, sql.to_string(), class, true));
            for (label, ws) in [
                ("tabs", "\t"),
                ("CRLF", "\r\n"),
                ("no-break spaces", "\u{A0}"),
                ("ideographic spaces", "\u{3000}"),
            ] {
                // A non-ASCII space next to a `$` or an E-string is part of a name to some
                // lexers (`Scan::ambiguous`), so such a read rightly becomes a write.
                let sql = respaced(lex, sql, ws);
                let ambiguous = lex.readings.iter().any(|&r| scan(lex, r, &sql).ambiguous);
                let class = if ambiguous { Write } else { class };
                out.push(case(format!("{what} with {label}"), sql, class, false));
            }
            for s in 0..SPELLINGS {
                out.push(case(
                    format!("{what} re-cased"),
                    recased(lex, sql, s),
                    class,
                    false,
                ));
            }
            out.push(case(
                format!("{what} after another statement"),
                format!("SELECT 1;\n{sql}"),
                class,
                false,
            ));
            if lex.terminator_optional {
                out.push(case(
                    format!("{what} after another statement with no ;"),
                    format!("SELECT 1\n{sql}"),
                    class,
                    false,
                ));
            }
            if class == Read {
                out.push(case(
                    format!("{what} with ;"),
                    format!("{sql};"),
                    class,
                    false,
                ));
            }
        };
        for r in &self.reads {
            variants(r, Read, "read");
        }
        for w in &self.writes {
            variants(w, Write, "write form");
        }
        out
    }

    /// The shared and the suite's contexts hold (no corpus, no fixture): what the lanes' skeleton
    /// suites run until they have a corpus, so unused in builds without a lane.
    #[allow(dead_code)]
    pub fn check_contexts(&self) {
        let failures = self.failures(&self.context_cases());
        report(self.name, "context", failures);
    }

    /// Every generated case holds, the corpus starts with every read keyword of the rules, and
    /// the fixture matches (`TABLER_BLESS=1` rewrites it).
    pub fn check(&self) {
        let contexts = self.context_cases();
        let corpus = self.corpus_cases();
        let mut failures = self.failures(&contexts);
        failures.extend(self.failures(&corpus));
        failures.extend(self.missing_starters());
        failures.extend(self.check_fixture(&contexts, &corpus));
        report(self.name, "generated", failures);
    }

    fn failures(&self, cases: &[Case]) -> Vec<String> {
        let mut out = Vec::new();
        for c in cases {
            let (spec, portable) = classes(self.spec, &c.sql);
            if spec != c.class || portable != c.class {
                out.push(format!(
                    "{}: {:?} is {spec:?} (portable {portable:?}), want {:?}",
                    c.why, c.sql, c.class
                ));
            }
            if let Some(want) = c.single
                && is_single_statement(self.lex(), self.rules(), &c.sql) != want
            {
                let what = if want { "one statement" } else { "several" };
                out.push(format!("{}: {:?} should count as {what}", c.why, c.sql));
            }
        }
        out
    }

    /// The read keywords of the rules (`queries`, `inspections`) no read of the corpus starts
    /// with.
    fn missing_starters(&self) -> Vec<String> {
        let lex = self.lex();
        let rules = self.rules();
        let firsts: Vec<String> = self
            .reads
            .iter()
            .filter_map(|r| first_word(lex, r))
            .collect();
        let starters = rules.queries.iter().chain(rules.inspections).copied();
        starters
            .filter(|&k| !firsts.iter().any(|f| listed(&[k], f)))
            .map(|k| format!("no read of the corpus starts with {k}"))
            .collect()
    }

    fn check_fixture(&self, contexts: &[Case], corpus: &[Case]) -> Vec<String> {
        let recorded = contexts.iter().chain(corpus).filter(|c| c.record);
        let splits = fuzz_samples(self, FIXTURE_SPLITS);
        let text = fixture_text(self.name, recorded, &splits, self.spec);
        let path = fixture_dir().join(format!("{}.json", self.name));
        if std::env::var("TABLER_BLESS").as_deref() == Ok("1") {
            std::fs::create_dir_all(fixture_dir()).expect("create the fixture folder");
            std::fs::write(&path, &text).expect("write the fixture");
            return Vec::new();
        }
        let Ok(expected) = std::fs::read_to_string(&path) else {
            return vec![format!(
                "{} is missing (TABLER_BLESS=1 writes it)",
                path.display()
            )];
        };
        let expected = expected.replace("\r\n", "\n");
        if expected == text {
            return Vec::new();
        }
        let at = expected
            .lines()
            .zip(text.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(expected.lines().count().min(text.lines().count()));
        let line = |s: &str| s.lines().nth(at).unwrap_or("<end of file>").to_string();
        vec![format!(
            "{} differs at line {}:\n  fixture: {}\n  now:     {}\nIf the change is intended: \
             TABLER_BLESS=1 cargo test --lib sqlgen::portable::generated, then review the diff.",
            path.display(),
            at + 1,
            line(&expected),
            line(&text)
        )]
    }

    /// Runs `runs` random scripts of each kind (see the module doc). Panics, with the inputs,
    /// on the first failures, a panic of the scanner, or a run that takes too long.
    pub fn fuzz(&self, runs: usize) {
        let suite = self.clone();
        let current = Arc::new(Mutex::new(String::new()));
        let seen = Arc::clone(&current);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let failures = fuzz_runs(&suite, runs, &seen);
            let _ = tx.send(failures);
        });
        match rx.recv_timeout(FUZZ_TIMEOUT) {
            Ok(failures) => report(self.name, "fuzz", failures),
            Err(_) => {
                let input = current.lock().map(|s| s.clone()).unwrap_or_default();
                panic!("{}: the fuzz run hung or died on {input:?}", self.name);
            }
        }
    }
}

const FUZZ_TIMEOUT: Duration = Duration::from_secs(120);
/// Atoms of a random script at most.
const MAX_ATOMS: usize = 28;
/// Fuzz scripts the fixture records with their statements.
const FIXTURE_SPLITS: usize = 24;
/// Write tokens per context the fixture records.
const FIXTURE_TOKENS: usize = 4;

/// The class `spec` gives `sql`, and the one the portable classifier gives it.
fn classes(spec: &DialectSpec, sql: &str) -> (StatementClass, StatementClass) {
    (
        (spec.classify)(sql),
        classify::with(spec.lex, spec.rules, sql),
    )
}

fn report(name: &str, what: &str, failures: Vec<String>) {
    if failures.is_empty() {
        return;
    }
    let shown: Vec<&str> = failures.iter().take(40).map(String::as_str).collect();
    panic!(
        "{name}: {} {what} failure(s):\n{}{}",
        failures.len(),
        shown.join("\n"),
        if failures.len() > shown.len() {
            "\n…"
        } else {
            ""
        }
    );
}

/// Per character of `sql`: whether every reading scans it into a token of `kind`.
fn common(lex: &LexProfile, sql: &str, kind: TokenKind) -> Vec<bool> {
    let n = sql.chars().count();
    let mut out = vec![true; n];
    for &r in lex.readings {
        let s = scan(lex, r, sql);
        let mut mine = vec![false; n];
        for t in s.tokens.iter().filter(|t| t.kind == kind) {
            mine[t.start..t.end].iter_mut().for_each(|m| *m = true);
        }
        out.iter_mut().zip(mine).for_each(|(o, m)| *o &= m);
    }
    out
}

/// `sql` with each plain space that every reading scans as space replaced by `ws`.
pub fn respaced(lex: &LexProfile, sql: &str, ws: &str) -> String {
    let spaces = common(lex, sql, TokenKind::Space);
    sql.chars()
        .zip(spaces)
        .map(|(c, s)| {
            if s && c == ' ' {
                ws.to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// `sql` with the words every reading scans as words in one of `SPELLINGS`.
pub fn recased(lex: &LexProfile, sql: &str, spelling: usize) -> String {
    let words = common(lex, sql, TokenKind::Word);
    let mut out = String::with_capacity(sql.len());
    let mut run = String::new();
    for (c, w) in sql.chars().zip(words) {
        if w {
            run.push(c);
        } else {
            out.push_str(&spell(&run, spelling));
            run.clear();
            out.push(c);
        }
    }
    out.push_str(&spell(&run, spelling));
    out
}

/// The first word of `sql` under the first reading, past comments and `(` (upper case).
fn first_word(lex: &LexProfile, sql: &str) -> Option<String> {
    let s = scan(lex, first_reading(lex), sql);
    s.tokens
        .iter()
        .find(|t| match t.kind {
            TokenKind::Space | TokenKind::Comment => false,
            TokenKind::Punct => !s.is_punct(t, '('),
            _ => true,
        })
        .filter(|t| t.kind == TokenKind::Word)
        .map(|t| s.upper(t))
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/classify-generated")
}

/// A JSON string, ASCII only: every other character, and every control character, escaped,
/// so the whitespace the cases hinge on stays visible in a diff.
fn json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{u:04x}");
                }
            }
        }
    }
    out.push('"');
    out
}

fn class_name(c: StatementClass) -> &'static str {
    match c {
        Read => "read",
        Write => "write",
    }
}

/// The fixture: the recorded cases (`{sql, class}`), then fuzz scripts with their class and
/// statements, in the shape of `tests/fixtures/portable/*.cases.json`.
fn fixture_text<'a>(
    name: &str,
    cases: impl Iterator<Item = &'a Case>,
    splits: &[(String, StatementClass, Vec<String>)],
    spec: &DialectSpec,
) -> String {
    let mut out = String::from("{\n");
    let comment = format!(
        "Generated by src-tauri/src/sqlgen/portable/gen ({name}, {}): the classes and \
         statements its lexer profile and rules must give (src/lib/ipc/bindings/lex). Never \
         edited by hand: TABLER_BLESS=1 cargo test --lib sqlgen::portable::generated rewrites it.",
        spec.label
    );
    let _ = writeln!(out, "  \"$comment\": {},", json(&comment));
    let mut lines: Vec<String> = cases
        .map(|c| {
            format!(
                "    {{ \"sql\": {}, \"class\": \"{}\" }}",
                json(&c.sql),
                class_name(c.class)
            )
        })
        .collect();
    lines.extend(splits.iter().map(|(sql, class, _)| {
        format!(
            "    {{ \"sql\": {}, \"class\": \"{}\" }}",
            json(sql),
            class_name(*class)
        )
    }));
    let _ = writeln!(out, "  \"classify\": [\n{}\n  ],", lines.join(",\n"));
    let splits: Vec<String> = splits
        .iter()
        .map(|(sql, _, statements)| {
            let statements: Vec<String> = statements.iter().map(|s| json(s)).collect();
            format!(
                "    {{ \"sql\": {}, \"statements\": [{}] }}",
                json(sql),
                statements.join(", ")
            )
        })
        .collect();
    let _ = writeln!(out, "  \"split\": [\n{}\n  ]", splits.join(",\n"));
    out.push_str("}\n");
    out
}

/// A small linear congruential generator: the fuzz runs are the same on every machine.
struct Lcg(u64);

impl Lcg {
    fn new(name: &str) -> Lcg {
        // FNV-1a of the suite's name: each dialect draws its own scripts.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in name.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        Lcg(h)
    }

    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    fn below(&mut self, n: usize) -> usize {
        self.next() as usize % n.max(1)
    }

    fn pick<'a>(&mut self, items: &'a [String]) -> &'a str {
        items
            .get(self.below(items.len()))
            .map_or("", String::as_str)
    }
}

/// What goes between two atoms: mostly a space, sometimes nothing (the atoms run together).
const SEPARATORS: &[&str] = &["", " ", " ", " ", " ", "\n", "\t", "\r\n"];

/// Atoms every dialect's fuzzer draws from: words, numbers, punctuation (also the openers and
/// closers of literals and comments on their own), whole literals and comments.
const SHARED_ATOMS: [&[&str]; 4] = [
    &[
        "SELECT", "WITH", "FROM", "WHERE", "AS", "JOIN", "ON", "UNION", "ALL", "DISTINCT", "IS",
        "NOT", "NULL", "ORDER", "BY", "GROUP", "LIMIT", "CASE", "WHEN", "THEN", "ELSE", "END",
        "BEGIN", "TABLE", "FUNCTION", "LATERAL", "IF", "LOOP", "WORK", "GO", "t", "a", "x1",
    ],
    &[
        "1", "1.5", ".5", "1.", "1e3", "1.5e-3", "1E+3", "0x1F", "0b1", "X'1F'", "$1", "?", "@p1",
        ":a", "{p1:Int}",
    ],
    &[
        "(", ")", "(", ")", ",", ".", ";", ";", "*", "=", "+", "-", "/", "\\", "$", "@", "#", ":",
        "'", "\"", "[", "]", "`", "{", "}", "/*", "*/", "--", "//", "$$", "$q$", "\n", "\r", "\t",
        "\u{A0}", "\u{2028}",
    ],
    &[
        "'a'",
        "'it''s'",
        "'a\\'b'",
        "'a\\\\'",
        "E'a\\'b'",
        "\"q\"",
        "\"a\"\"b\"",
        "[a]",
        "[a]]b]",
        "`a`",
        "`a\\`b`",
        "$$a;b$$",
        "$q$a;b$q$",
        "'''a'b'''",
        "r'a\\'",
        "b'a'",
        "-- c\n",
        "/* c */",
        "# c\n",
        "// c\n",
        "/* /* */",
        "\"\"\"a\"b\"\"\"",
    ],
];

/// The fuzz vocabulary: the shared atoms, the rules' words (calls with their `(`), and the
/// suite's atoms. `restricted` leaves out every atom holding an inspection keyword, which would
/// let a write keyword after it be a read (`SHOW … DELETE` is one statement, and a bad one).
fn vocabulary(suite: &Suite, restricted: bool) -> Vec<String> {
    let rules = suite.rules();
    let name = |entry: &str| entry.replace('*', "X");
    let mut out: Vec<String> = SHARED_ATOMS
        .concat()
        .iter()
        .map(|a| a.to_string())
        .collect();
    out.extend(rules.queries.iter().map(|w| name(w)));
    out.extend(rules.inspections.iter().map(|w| name(w)));
    out.extend(rules.writes.iter().map(|w| name(w)));
    out.extend(rules.write_calls.iter().map(|w| format!("{}(", name(w))));
    if let Some(fns) = rules.table_functions {
        out.extend(fns.iter().map(|f| format!("{}(", name(f))));
    }
    out.extend(suite.vocab.iter().map(|a| a.to_string()));
    if restricted {
        let lex = suite.lex();
        out.retain(|atom| {
            let s = scan(lex, first_reading(lex), atom);
            let words = s.tokens.iter().filter(|t| t.kind == TokenKind::Word);
            !words
                .map(|t| s.upper(t))
                .any(|w| listed(rules.inspections, &w))
                && !rules
                    .inspections
                    .iter()
                    .any(|i| atom.to_uppercase().contains(i.trim_end_matches('*')))
        });
    }
    out
}

fn random_script(rng: &mut Lcg, vocab: &[String], max: usize) -> String {
    let mut out = String::new();
    for _ in 0..rng.below(max + 1) {
        out.push_str(SEPARATORS[rng.below(SEPARATORS.len())]);
        out.push_str(rng.pick(vocab));
    }
    out
}

/// The first `n` free scripts of the suite's fuzzer that split into statements, short enough
/// to read, with their class and statements.
fn fuzz_samples(suite: &Suite, n: usize) -> Vec<(String, StatementClass, Vec<String>)> {
    let mut rng = Lcg::new(suite.name);
    let vocab = vocabulary(suite, false);
    let mut out = Vec::new();
    for _ in 0..n * 50 {
        if out.len() == n {
            break;
        }
        let sql = random_script(&mut rng, &vocab, 12);
        let statements: Vec<String> = split(suite.lex(), &sql)
            .into_iter()
            .map(|s| s.sql)
            .collect();
        if !statements.is_empty() && sql.chars().count() <= 120 {
            let class = classify::with(suite.lex(), suite.rules(), &sql);
            out.push((sql, class, statements));
        }
    }
    out
}

fn fuzz_runs(suite: &Suite, runs: usize, current: &Mutex<String>) -> Vec<String> {
    let mut rng = Lcg::new(suite.name);
    let free = vocabulary(suite, false);
    let restricted = vocabulary(suite, true);
    let tokens = write_tokens(suite.rules());
    let mut failures = Vec::new();
    let mut run = |sql: String, write: Option<(Range<usize>, bool)>| {
        if let Ok(mut c) = current.lock() {
            c.clone_from(&sql);
        }
        let checked = catch_unwind(AssertUnwindSafe(|| properties(suite, &sql, write.clone())));
        match checked {
            Ok(found) => failures.extend(found.into_iter().map(|f| format!("{f}: {sql:?}"))),
            Err(_) => failures.push(format!("panicked on {sql:?}")),
        }
    };
    for _ in 0..runs {
        // A free script.
        run(random_script(&mut rng, &free, MAX_ATOMS), None);
        // A query with a write keyword or call somewhere in it, the rest without inspections.
        if tokens.is_empty() {
            continue;
        }
        let token = &tokens[rng.below(tokens.len())];
        let spelling = rng.below(SPELLINGS);
        let before = format!(
            "SELECT {} ",
            random_script(&mut rng, &restricted, MAX_ATOMS / 2)
        );
        let after = random_script(&mut rng, &restricted, MAX_ATOMS / 2);
        let start = before.chars().count();
        let at = start..start + token.name.chars().count();
        run(
            format!("{before}{} {after}", token.text(spelling)),
            Some((at, token.call)),
        );
    }
    failures
}

/// The fuzz properties of one script; `write`: the char range of a write keyword or callee put
/// in it, and whether it is a call.
fn properties(suite: &Suite, sql: &str, write: Option<(Range<usize>, bool)>) -> Vec<String> {
    let (lex, rules, spec) = (suite.lex(), suite.rules(), suite.spec);
    let mut out = Vec::new();
    let (by_spec, script) = classes(spec, sql);
    if by_spec != script {
        out.push(format!(
            "the spec's classifier says {by_spec:?}, the portable one {script:?}"
        ));
    }
    // Total: none of these may panic.
    let _ = code_only(lex, sql);
    let _ = calls_listed_function(lex, &["X"], sql);
    let single = is_single_statement(lex, rules, sql);

    if let Some((at, call)) = write
        && is_code_word(lex, sql, &at, call)
        && script == Read
    {
        out.push(format!(
            "a write the scanner reads as code at {at:?} is a read"
        ));
    }

    let statements = split(lex, sql);
    if single && statements.len() > 1 {
        out.push(format!(
            "one statement to is_single_statement, {} to split",
            statements.len()
        ));
    }
    let worst = if statements.is_empty()
        || statements
            .iter()
            .any(|s| classify::with(lex, rules, &s.sql) == Write)
    {
        Write
    } else {
        Read
    };
    if worst == Write && script == Read {
        out.push("a statement of it writes, the script reads".into());
    } else if worst != script && scans_alike(lex, sql) {
        out.push(format!(
            "the script is a {script:?}, its worst statement a {worst:?}"
        ));
    }
    out.extend(split_keeps_text(lex, sql, &statements));
    out
}

/// Whether some reading scans `at` as a word of its own (and with `call`, a `(` right after
/// it): the write is code to that reading.
fn is_code_word(lex: &LexProfile, sql: &str, at: &Range<usize>, call: bool) -> bool {
    lex.readings.iter().any(|&r| {
        let s = scan(lex, r, sql);
        s.tokens.iter().enumerate().any(|(k, t)| {
            t.kind == TokenKind::Word
                && t.start == at.start
                && t.end == at.end
                && (!call || s.tokens.get(k + 1).is_some_and(|n| s.is_punct(n, '(')))
        })
    })
}

/// Whether every reading scans `sql` into the same tokens, with no comment a server may end
/// elsewhere: then classifying it is classifying its statements.
fn scans_alike(lex: &LexProfile, sql: &str) -> bool {
    let mut scans = lex.readings.iter().map(|&r| scan(lex, r, sql));
    let Some(first) = scans.next() else {
        return false;
    };
    !first.ambiguous && scans.all(|s| !s.ambiguous && s.tokens == first.tokens)
}

/// Splitting keeps the text: the statements are in order, do not overlap, are the text at
/// their offsets, and every token of code is in one, but `;` terminators and batch separator
/// lines.
fn split_keeps_text(
    lex: &LexProfile,
    sql: &str,
    statements: &[crate::sqlgen::split::SplitStatement],
) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut covered = vec![false; chars.len()];
    let mut end = 0;
    for s in statements {
        if s.start < end || s.end < s.start || s.end > chars.len() {
            out.push(format!(
                "statement {}..{} overlaps or runs out",
                s.start, s.end
            ));
            continue;
        }
        if chars[s.start..s.end].iter().collect::<String>() != s.sql {
            out.push(format!(
                "statement {}..{} is not the text there",
                s.start, s.end
            ));
        }
        covered[s.start..s.end].iter_mut().for_each(|c| *c = true);
        end = s.end;
    }
    let scan = scan(lex, first_reading(lex), sql);
    for t in &scan.tokens {
        if matches!(t.kind, TokenKind::Space | TokenKind::Comment) {
            continue;
        }
        let inside = covered[t.start..t.end].iter().filter(|&&c| c).count();
        if inside == t.end - t.start {
            continue;
        }
        let terminator = scan.is_punct(t, ';');
        let separator = lex
            .batch_separator
            .is_some_and(|sep| on_separator_line(&chars, t.start, sep));
        if inside > 0 || !(terminator || separator) {
            out.push(format!("split lost {:?} at {}", scan.text(t), t.start));
        }
    }
    out
}

/// Whether the character at `i` is on a line holding only the batch separator, an optional
/// repeat count and an optional `--` comment.
fn on_separator_line(chars: &[char], i: usize, separator: &str) -> bool {
    let start = chars[..i]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |p| p + 1);
    let end = chars[i..]
        .iter()
        .position(|&c| c == '\n')
        .map_or(chars.len(), |p| i + p);
    let line: String = chars[start..end].iter().collect();
    let code = line.split("--").next().unwrap_or_default();
    let mut words = code.split([' ', '\t', '\r']).filter(|w| !w.is_empty());
    words
        .next()
        .is_some_and(|w| w.eq_ignore_ascii_case(separator))
        && words
            .next()
            .is_none_or(|n| n.chars().all(|c| c.is_ascii_digit()))
        && words.next().is_none()
}
