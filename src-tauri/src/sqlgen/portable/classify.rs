//! Safe Mode's classifier of the extension dialects, driven by a `LexProfile` and the dialect's
//! `ClassifyRules`. It is an allow-list: a statement is a READ only when its first keyword is a
//! query or an inspection of the rules and nothing in it writes; empty and unknown statements are
//! WRITEs. On top of the statement kind:
//!
//! - a query holding a word of `writes` anywhere (`INTO`, `DELETE`…) is a write, and with an
//!   optional terminator (T-SQL) any statement is (`SELECT 1 DELETE FROM t` is two statements);
//! - a call of a function of `write_calls` is a write (T-SQL `OPENQUERY(srv, 'DELETE …')` hides
//!   its statement in a string);
//! - where a table goes, a table function outside `table_functions` is a write (ClickHouse
//!   `url()`, `s3()`…), also as the first table of a parenthesized join
//!   (`FROM (url(…) CROSS JOIN t)`). A file named where a table goes (DuckDB `FROM 'x.csv'`,
//!   `FROM data.csv`) is not looked for: the session's engine settings refuse those reads (see
//!   `dialect::duckdb`);
//! - a word that starts with a digit may be a number and then a keyword to the server: T-SQL,
//!   and PostgreSQL before 15 with the servers built on its lexer, read `1DELETE` as `1` then
//!   `DELETE`, and T-SQL `0xINTO` as an empty binary then `INTO`. Servers end a number at
//!   different places (`1EXEC` may be `1E` then `XEC`, or `1` then `EXEC`), so each suffix of such
//!   a word that starts like a name counts as a keyword or callee too (`hidden_words`). So does
//!   each suffix of a word right after a number and a `.`: the scanner reads `1.e5DELETE` as `1`,
//!   `.`, `E5DELETE`, where those servers read the number `1.e5` and then `DELETE`;
//! - keywords match in ASCII only, as servers read them: `ſelect 1` (long s) starts with a name,
//!   not SELECT, so it is unknown and a write. The words of `writes` and `write_calls` also match
//!   with every letter upper-cased by Unicode rules, since a server may look a function up that
//!   way (`spellings`);
//! - the text is read under every `Reading` of the dialect and is a read only under all of them;
//! - a comment a server may end elsewhere than the scanner (`Scan::ambiguous`: a block comment
//!   holding `/*`, a line comment holding a lone CR or another line break than LF) makes the
//!   text a write whatever the profile says: what follows it may be code to the server and a
//!   string to the scanner.
//!
//! Comments, strings and quoted names are never keywords; a quoted name followed by `(` is still
//! a call of that function. A Unicode-escaped name (`U&"\0072ead_csv"`, with an optional
//! `UESCAPE 'c'`) may spell any function: as a callee it is on every deny-list and on no
//! allow-list.

use std::borrow::Cow;
use std::ops::Range;

use crate::sqlgen::classify::StatementClass;
use crate::sqlgen::dialect::lex::{
    ClassifyRules, LexProfile, Scan, Token, TokenKind, is_ident_start, listed, scan,
};
use crate::sqlgen::portable::split::{first_reading, statements};

/// A token of code: a word (upper case), a quoted name (upper case), a name whose spelling the
/// scanner cannot know (`U&"…"`), a string or a punctuation character.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Code {
    Word(String),
    Name(String),
    AnyName,
    Str,
    Punct(char),
}

/// What `Code::AnyName` answers as a callee: no allow-list holds it (`names_listed` puts it on
/// every deny-list).
const ANY_NAME: &str = "\u{FFFD}";

impl Code {
    fn is(&self, c: char) -> bool {
        matches!(self, Code::Punct(x) if *x == c)
    }

    fn word(&self) -> Option<&str> {
        match self {
            Code::Word(w) => Some(w),
            _ => None,
        }
    }

    /// What a call could name: a word or a quoted name.
    fn callee(&self) -> Option<&str> {
        match self {
            Code::Word(w) | Code::Name(w) => Some(w),
            Code::AnyName => Some(ANY_NAME),
            Code::Str | Code::Punct(_) => None,
        }
    }
}

/// Whether the word at `i` may be read as part of a number: it starts with a digit, or it follows
/// one and a `.`, which the scanner keeps apart from both (`1.e5DELETE` is `1`, `.`, `E5DELETE`;
/// T-SQL reads `1.e5` and `1.e` as numbers, the lexers before PostgreSQL 15 read `1.e5`). Spaces
/// are not code here, so `1. e5x` counts too, which only adds writes.
fn in_number(code: &[Code], i: usize) -> bool {
    let digits = |c: &Code| {
        c.word()
            .is_some_and(|w| w.starts_with(|c: char| c.is_ascii_digit()))
    };
    digits(&code[i]) || (i >= 2 && code[i - 1].is('.') && digits(&code[i - 2]))
}

/// The words the word at `i` may hide from the scanner: where it may be part of a number
/// (`in_number`), every suffix after its first character that starts like a name, since the
/// server may end the number before any of them (`1e0DELETE`, `0xINTO`, `1.e5DELETE`).
fn hidden_words(code: &[Code], i: usize) -> impl Iterator<Item = &str> {
    suffixes(code[i].word().unwrap_or_default(), in_number(code, i))
}

/// The suffixes of `w` after its first character that start like a name, when `number`.
fn suffixes(w: &str, number: bool) -> impl Iterator<Item = &str> {
    w.char_indices()
        .skip(1)
        .filter(move |&(_, c)| number && is_ident_start(c))
        .map(move |(i, _)| &w[i..])
}

/// The word at `i` and the words it may hide, for the deny-lists (`writes`, `write_calls`, the
/// side-effect functions). Keywords match in ASCII only (`Scan::upper`), but a server may look a
/// function's name up with Unicode case rules (`ſystem$x` as `SYSTEM$X`), so a word with other
/// letters also counts with them upper-cased that way: no spelling hides a listed write.
fn spellings(code: &[Code], i: usize) -> impl Iterator<Item = Cow<'_, str>> {
    let folded = code[i].word().filter(|w| !w.is_ascii()).map(|w| {
        let w = w.to_uppercase();
        let mut all: Vec<String> = suffixes(&w, in_number(code, i))
            .map(str::to_owned)
            .collect();
        all.push(w);
        all
    });
    code[i]
        .word()
        .into_iter()
        .chain(hidden_words(code, i))
        .map(Cow::Borrowed)
        .chain(folded.into_iter().flatten().map(Cow::Owned))
}

/// Whether the token at `i` names an entry of `list`: a quoted name, or a word that is one or may
/// hide one.
fn names_listed(code: &[Code], i: usize, list: &[&str]) -> bool {
    match &code[i] {
        Code::Name(n) => listed(list, n),
        Code::AnyName => !list.is_empty(),
        _ => spellings(code, i).any(|s| listed(list, &s)),
    }
}

/// Functions whose arguments may hold a `FROM` (`EXTRACT(YEAR FROM ts)`).
const FROM_IN_ARGUMENTS: &[&str] = &[
    "EXTRACT",
    "SUBSTRING",
    "SUBSTR",
    "TRIM",
    "OVERLAY",
    "POSITION",
];
/// What may follow a set operator that ends a `FROM` list (DuckDB `UNION BY NAME`), besides `(`.
const AFTER_SET_OPERATOR: &[&str] = &[
    "ALL", "DISTINCT", "SELECT", "VALUES", "TABLE", "FROM", "WITH", "BY",
];
/// Words a name or an operand follows, besides `TABLE_WORDS` and `TABLE_PREFIXES`: a clause
/// keyword right after one is a name (`FROM limit`, ClickHouse `ON limit = 1`).
const NAME_AFTER: &[&str] = &[
    "AS", "FROM", "USING", "ON", "AND", "OR", "NOT", "IS", "IN", "LIKE", "ILIKE", "REGEXP",
    "BETWEEN", "CASE", "WHEN", "THEN", "ELSE", "INTERVAL", "DIV", "MOD", "COLLATE",
];
/// The options of EXPLAIN that may stand before the statement it explains.
const EXPLAIN_OPTIONS: &[&str] = &[
    "ANALYZE", "ANALYSE", "VERBOSE", "EXTENDED", "COSTS", "BUFFERS", "TIMING", "SUMMARY",
    "SETTINGS", "FORMAT", "TEXT", "JSON", "XML", "YAML", "TREE", "USING", "TABULAR",
];
/// Words between a table position and the table itself.
const TABLE_PREFIXES: &[&str] = &["LATERAL", "ONLY"];
/// Words a table follows, besides `FROM`.
const TABLE_WORDS: &[&str] = &[
    "JOIN",
    "TABLE",
    "DESCRIBE",
    "DESC",
    "SUMMARIZE",
    "SHOW",
    "PIVOT",
    "UNPIVOT",
    // DuckDB's spellings of PIVOT and UNPIVOT.
    "PIVOT_WIDER",
    "PIVOT_LONGER",
];
/// Words after which a query keyword goes on the same statement (`UNION ALL SELECT`).
const SET_OPERATORS: &[&str] = &["UNION", "EXCEPT", "INTERSECT", "MINUS", "ALL", "DISTINCT"];

/// Classifies one statement or several (a READ only when each one is).
pub fn with(lex: &LexProfile, rules: &ClassifyRules, sql: &str) -> StatementClass {
    let reads = !lex.readings.is_empty()
        && lex.readings.iter().all(|&r| {
            let scan = scan(lex, r, sql);
            !scan.ambiguous && reads_under(lex, rules, &scan)
        });
    if reads {
        StatementClass::Read
    } else {
        StatementClass::Write
    }
}

fn reads_under(lex: &LexProfile, rules: &ClassifyRules, scan: &Scan) -> bool {
    let ranges = statements(lex, scan);
    !ranges.is_empty()
        && ranges.into_iter().all(|r| {
            let code = code_of(scan, r);
            // A batch may hold several `;`-separated statements: each one must read.
            let parts: Vec<&[Code]> = code
                .split(|c| c.is(';'))
                .filter(|p| !p.is_empty())
                .collect();
            !parts.is_empty() && parts.iter().all(|p| reads(lex, rules, p))
        })
}

fn code_of(scan: &Scan, r: Range<usize>) -> Vec<Code> {
    let toks = &scan.tokens[r];
    let mut out = Vec::with_capacity(toks.len());
    let mut i = 0;
    while i < toks.len() {
        if let Some(next) = unicode_name(scan, toks, i) {
            out.push(Code::AnyName);
            i = next;
            continue;
        }
        let t = &toks[i];
        out.extend(match t.kind {
            TokenKind::Word => Some(Code::Word(scan.upper(t))),
            TokenKind::Ident => Some(Code::Name(scan.ident_name(t).to_uppercase())),
            TokenKind::Str => Some(Code::Str),
            TokenKind::Punct => Some(Code::Punct(scan.chars[t.start])),
            TokenKind::Space | TokenKind::Comment => None,
        });
        i += 1;
    }
    out
}

/// When `toks[i..]` starts with a Unicode-escaped name (`U&"…"`: the word `U`, `&` and a quoted
/// name with nothing between them, then an optional `UESCAPE 'c'`), the index after it. Its
/// escapes may spell any name (`U&"\0072ead_csv"` is `read_csv`), so the scanner cannot tell
/// which.
fn unicode_name(scan: &Scan, toks: &[Token], i: usize) -> Option<usize> {
    let (u, amp, name) = (toks.get(i)?, toks.get(i + 1)?, toks.get(i + 2)?);
    let glued = u.end == amp.start && amp.end == name.start;
    if !(u.kind == TokenKind::Word
        && scan.upper(u) == "U"
        && scan.is_punct(amp, '&')
        && name.kind == TokenKind::Ident
        && glued)
    {
        return None;
    }
    // `UESCAPE 'c'` after it, past any space or comment.
    let code = |from: usize| {
        (from..toks.len()).find(|&j| !matches!(toks[j].kind, TokenKind::Space | TokenKind::Comment))
    };
    if let Some(e) = code(i + 3)
        && toks[e].kind == TokenKind::Word
        && scan.upper(&toks[e]) == "UESCAPE"
        && let Some(s) = code(e + 1)
        && toks[s].kind == TokenKind::Str
    {
        return Some(s + 1);
    }
    Some(i + 3)
}

fn has_word(code: &[Code], words: &[&str]) -> bool {
    (0..code.len()).any(|i| spellings(code, i).any(|s| listed(words, &s)))
}

fn reads(lex: &LexProfile, rules: &ClassifyRules, code: &[Code]) -> bool {
    !(lex.terminator_optional && has_word(code, rules.writes))
        && !calls(code, rules.write_calls)
        && tables_read_only(rules, code)
        && statement_reads(rules, code)
}

fn statement_reads(rules: &ClassifyRules, code: &[Code]) -> bool {
    let start = code.iter().take_while(|c| c.is('(')).count();
    let Some(first) = code.get(start).and_then(Code::word) else {
        return false;
    };
    let rest = &code[start + 1..];
    if listed(rules.queries, first) {
        !has_word(rest, rules.writes) && !locks_rows(rest)
    } else if first == "EXPLAIN" && listed(rules.inspections, first) {
        // With ANALYZE it runs the statement it explains, which must then be a query.
        !explain_analyzes(rest) || explained_query_reads(rules, rest)
    } else {
        listed(rules.inspections, first)
    }
}

/// Whether the EXPLAIN whose options are `rest` runs its statement: ANALYZE (or ANALYSE) among
/// them, or a quoted name or a string in its option list, which may spell ANALYZE (PostgreSQL
/// takes `EXPLAIN ("analyze") DELETE …`).
fn explain_analyzes(rest: &[Code]) -> bool {
    if has_word(rest, &["ANALYZE", "ANALYSE"]) {
        return true;
    }
    if !rest.first().is_some_and(|c| c.is('(')) {
        return false;
    }
    let mut depth = 0usize;
    for c in rest {
        match c {
            Code::Punct('(') => depth += 1,
            Code::Punct(')') => {
                depth -= 1;
                if depth == 0 {
                    return false;
                }
            }
            Code::Name(_) | Code::AnyName | Code::Str => return true,
            _ => {}
        }
    }
    false
}

/// Whether a query locks the rows it reads (`FOR UPDATE`, `FOR NO KEY UPDATE`, `FOR SHARE`,
/// `FOR KEY SHARE`), which a read-only transaction refuses like a write.
fn locks_rows(code: &[Code]) -> bool {
    let word = |i: usize| code.get(i).and_then(Code::word);
    (0..code.len()).any(|i| {
        word(i) == Some("FOR")
            && match word(i + 1) {
                Some("UPDATE" | "SHARE") => true,
                Some("KEY") => word(i + 2) == Some("SHARE"),
                Some("NO") => word(i + 2) == Some("KEY") && word(i + 3) == Some("UPDATE"),
                _ => false,
            }
    })
}

/// Whether the statement an `EXPLAIN ANALYZE` runs (`rest`: what follows EXPLAIN) is a query
/// that reads: the first word after the known options must start a query. An option this does
/// not know stands where the query should, so the statement counts as a write.
fn explained_query_reads(rules: &ClassifyRules, rest: &[Code]) -> bool {
    let mut i = 0;
    loop {
        match rest.get(i) {
            Some(c) if c.is('(') => {
                // A parenthesized option list: `(ANALYZE, BUFFERS)`.
                let mut depth = 0usize;
                while let Some(c) = rest.get(i) {
                    if c.is('(') {
                        depth += 1;
                    } else if c.is(')') {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    i += 1;
                }
                i += 1;
            }
            Some(c) if c.is('=') => i += 1,
            Some(Code::Word(w)) if EXPLAIN_OPTIONS.contains(&w.as_str()) => i += 1,
            _ => break,
        }
    }
    rest.get(i)
        .and_then(Code::word)
        .is_some_and(|w| listed(rules.queries, w))
        && statement_reads(rules, &rest[i..])
}

/// Whether `code` calls a function of `names`.
fn calls(code: &[Code], names: &[&str]) -> bool {
    (1..code.len()).any(|i| code[i].is('(') && names_listed(code, i - 1, names))
}

/// Whether every table function `code` reads from is allowed.
fn tables_read_only(rules: &ClassifyRules, code: &[Code]) -> bool {
    let Some(allowed) = rules.table_functions else {
        return true;
    };
    table_positions(code).into_iter().all(|mut p| {
        // The first table of a parenthesized join goes here too (`FROM (url(…) CROSS JOIN t)`,
        // which DuckDB's PostgreSQL grammar takes); a subquery's own FROM has its own positions.
        let mut nested = false;
        loop {
            match code.get(p) {
                Some(c) if c.is('(') => nested = true,
                Some(Code::Word(w)) if TABLE_PREFIXES.contains(&w.as_str()) => {}
                _ => break,
            }
            p += 1;
        }
        match code.get(p) {
            Some(Code::Word(w)) if nested && listed(rules.queries, w) => true,
            Some(c) if c.callee().is_some() => {
                // A qualified name: the function is its last part.
                let mut q = p;
                while code.get(q + 1).is_some_and(|c| c.is('.'))
                    && code.get(q + 2).is_some_and(|c| c.callee().is_some())
                {
                    q += 2;
                }
                let called = code.get(q + 1).is_some_and(|c| c.is('('));
                match code[q].callee() {
                    Some(name) if called => listed(allowed, name),
                    _ => true,
                }
            }
            _ => true,
        }
    })
}

/// Whether the `FROM` at `i` ends the comparison `IS [NOT] DISTINCT FROM`, which no table
/// follows. After any other `DISTINCT` it starts a query: DuckDB takes `SELECT 1 UNION DISTINCT
/// FROM st_read('x.shp')`.
fn compares_distinct(code: &[Code], i: usize) -> bool {
    let word = |back: usize| i.checked_sub(back).and_then(|p| code[p].word());
    word(1) == Some("DISTINCT")
        && (word(2) == Some("IS") || (word(2) == Some("NOT") && word(3) == Some("IS")))
}

/// Indexes of the tokens where a table goes: after `FROM` (but `IS DISTINCT FROM`), `JOIN`,
/// `TABLE` (`DESCRIBE TABLE`, `INSERT INTO TABLE FUNCTION`), `DESCRIBE` / `DESC` / `SUMMARIZE` /
/// `SHOW` and `PIVOT` / `UNPIVOT` with their DuckDB spellings (DuckDB takes a table function
/// there), after a word that may hide one of them (`1e0FROM`), and after a comma of a `FROM`
/// list until a clause ends it (`ends_from_list`).
fn table_positions(code: &[Code]) -> Vec<usize> {
    let mut out = Vec::new();
    // Per open parenthesis: whether it holds a function's arguments that may say FROM.
    let mut parens: Vec<bool> = Vec::new();
    // The parenthesis depths of the open FROM lists, innermost last: a subquery in a FROM list
    // has its own, and when it closes the outer list goes on (`FROM (…) b, url(…)`).
    let mut froms: Vec<usize> = Vec::new();
    for (i, c) in code.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|p| code[p].word());
        let in_from = froms.last() == Some(&parens.len());
        match c {
            Code::Punct('(') => parens.push(prev.is_some_and(|w| FROM_IN_ARGUMENTS.contains(&w))),
            Code::Punct(')') => {
                parens.pop();
                while froms.last().is_some_and(|&d| d > parens.len()) {
                    froms.pop();
                }
            }
            Code::Punct(',') if in_from => out.push(i + 1),
            Code::Word(w) => {
                // After a number, a hidden word has the number before it (`SELECT 1e0FROM t`).
                let hides = |words: &[&str]| hidden_words(code, i).any(|h| words.contains(&h));
                let w = w.as_str();
                if (w == "FROM" && !compares_distinct(code, i)) || hides(&["FROM"]) {
                    let in_arguments = parens.last().copied().unwrap_or(false);
                    if !in_arguments {
                        // A second FROM at the same depth (T-SQL `DELETE FROM t FROM …`)
                        // continues the list rather than nesting one.
                        if !in_from {
                            froms.push(parens.len());
                        }
                        out.push(i + 1);
                    }
                } else if TABLE_WORDS.contains(&w)
                    || hides(TABLE_WORDS)
                    || (w == "FUNCTION" && prev == Some("TABLE"))
                {
                    out.push(i + 1);
                } else if in_from && ends_from_list(code, i, w) {
                    froms.pop();
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether the word `w` at `i` ends the `FROM` list it stands in. Only the clauses whose own
/// comma lists may hold calls need to (`GROUP BY a, lower(b)`, ClickHouse `LIMIT 1 BY a, f(b)`,
/// the select list after `UNION`); after any other clause no table follows a comma in a valid
/// statement, so the list may as well stay open. An ended list's commas are no longer checked,
/// so the keyword counts only where a clause can start and followed by what the clause takes.
/// Anywhere else it is a name: an alias (`FROM t AS where, …`), a part of a qualified name
/// (`ON t.a = u.order, …`), a table, or an operand (ClickHouse reads `ON a = limit` as a column).
fn ends_from_list(code: &[Code], i: usize, w: &str) -> bool {
    let next = code.get(i + 1);
    let next_word = next.and_then(Code::word);
    let paren = next.is_some_and(|c| c.is('('));
    let takes_next = match w {
        "GROUP" | "ORDER" => next_word == Some("BY"),
        "UNION" | "EXCEPT" | "INTERSECT" => {
            paren || next_word.is_some_and(|n| AFTER_SET_OPERATOR.contains(&n))
        }
        "LIMIT" => paren || next_word.is_some(),
        _ => false,
    };
    let clause_can_start = match i.checked_sub(1).map(|p| &code[p]) {
        Some(Code::Punct(c)) => *c == ')',
        Some(Code::Word(p)) => {
            let p = p.as_str();
            !(NAME_AFTER.contains(&p) || TABLE_WORDS.contains(&p) || TABLE_PREFIXES.contains(&p))
        }
        Some(Code::Name(_) | Code::AnyName | Code::Str) | None => true,
    };
    takes_next && clause_can_start
}

/// Whether `sql` calls a function of `names` (upper case, `PREFIX*` allowed) under any reading:
/// the Metrics Board's check of `SqlKnobs::side_effect_functions`. An entry of several words
/// (T-SQL's `NEXT VALUE FOR`) is those words in a row, not a call. A comment a server may end
/// elsewhere could hide a call, so it counts as one.
pub fn calls_listed_function(lex: &LexProfile, names: &[&str], sql: &str) -> bool {
    !names.is_empty()
        && lex.readings.iter().any(|&r| {
            let scan = scan(lex, r, sql);
            let code = code_of(&scan, 0..scan.tokens.len());
            scan.ambiguous || calls(&code, names) || names.iter().any(|n| says(&code, n))
        })
}

/// Whether `code` holds the words of `phrase` (several, upper case) in a row; the first may be
/// hidden after a number like any word.
fn says(code: &[Code], phrase: &str) -> bool {
    let words: Vec<&str> = phrase.split(' ').collect();
    words.len() > 1
        && (0..code.len()).any(|i| {
            spellings(code, i).any(|s| s == words[0])
                && words[1..]
                    .iter()
                    .enumerate()
                    .all(|(k, w)| code.get(i + 1 + k).and_then(Code::word) == Some(*w))
        })
}

/// Whether the statement `r` of `scan` (a batch, where the dialect has them) is one statement
/// for the server: it holds no `;` between two statements, and where terminators are optional no
/// word that may start a second one (`starts_another`).
pub(super) fn one_statement(
    lex: &LexProfile,
    rules: &ClassifyRules,
    scan: &Scan,
    r: Range<usize>,
) -> bool {
    let code = code_of(scan, r);
    let mut parts = code.split(|c| c.is(';')).filter(|p| !p.is_empty());
    match (parts.next(), parts.next()) {
        (Some(_), Some(_)) => false,
        (Some(part), None) => !(lex.terminator_optional && starts_another(rules, part)),
        (None, _) => true,
    }
}

/// Whether a word after the first one of `code` may start a second statement where none needs
/// a terminator: a word of `writes` anywhere, or a query or inspection keyword outside
/// parentheses, also hidden after a number (`hidden_words`). A query keyword still belongs to
/// the statement after a set operator (`UNION SELECT`) or as the query a leading `WITH` names its
/// tables for, and so does a `WITH` that defines no table (`WITH (NOLOCK)`, `WITH TIES`,
/// `WITH ROLLUP`). A word of `writes` in a write statement (`INSERT INTO`) cannot be told from a
/// second statement's, so such a statement counts as several.
fn starts_another(rules: &ClassifyRules, code: &[Code]) -> bool {
    let mut depth = 0usize;
    let mut cte_query = code.first().and_then(Code::word) == Some("WITH");
    for (i, c) in code.iter().enumerate() {
        match c {
            Code::Punct('(') => depth += 1,
            Code::Punct(')') => depth = depth.saturating_sub(1),
            Code::Word(w) if i > 0 => {
                let starts = |s: &str| listed(rules.queries, s) || listed(rules.inspections, s);
                if listed(rules.writes, w) {
                    return true;
                }
                // A keyword hidden after a number (`SELECT 1SELECT 2`) follows the number, not
                // a set operator or a `WITH`.
                if hidden_words(code, i)
                    .any(|h| listed(rules.writes, h) || (depth == 0 && starts(h)))
                {
                    return true;
                }
                if depth > 0 || !starts(w) {
                    continue;
                }
                if code[i - 1]
                    .word()
                    .is_some_and(|p| SET_OPERATORS.contains(&p))
                {
                    continue;
                }
                if w == "WITH" && !defines_table(&code[i + 1..]) {
                    continue;
                }
                if cte_query {
                    cte_query = false;
                    continue;
                }
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Whether the code after a `WITH` defines a table: `name AS (` or `name (columns) AS (`.
fn defines_table(rest: &[Code]) -> bool {
    rest.first().and_then(Code::callee).is_some()
        && rest
            .get(1)
            .is_some_and(|c| c.is('(') || c.word() == Some("AS"))
}

/// `sql` with strings, quoted names and comments replaced by a space, so keyword checks see only
/// code (first reading).
pub fn code_only(lex: &LexProfile, sql: &str) -> String {
    let scan = scan(lex, first_reading(lex), sql);
    let mut out = String::with_capacity(sql.len());
    for t in &scan.tokens {
        match t.kind {
            TokenKind::Str | TokenKind::Ident | TokenKind::Comment => out.push(' '),
            TokenKind::Word | TokenKind::Space | TokenKind::Punct => {
                out.extend(&scan.chars[t.start..t.end]);
            }
        }
    }
    out
}
