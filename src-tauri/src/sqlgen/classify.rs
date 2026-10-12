//! Safe-mode statement classifier: is a statement a READ or a WRITE? The renderer's copy
//! (`src/features/safety/classify.ts`) follows the same rules; both run the shared cases in
//! `src/features/safety/classify.cases.json`.
//!
//! READ: `SELECT` / `VALUES` / `TABLE` (unless they hold an `INTO`), `SHOW`, `USE <db>`,
//! `EXPLAIN` / `DESCRIBE` / `DESC` (an EXPLAIN ANALYZE is classified by the statement it runs),
//! `WITH …` without a data-modifying keyword, and SQLite PRAGMAs that only read. Everything else,
//! including empty and unknown statements, is a WRITE, and so is any statement that calls
//! `set_config(…)` (it changes settings, `default_transaction_read_only` too). Comments, strings
//! and quoted identifiers are ignored; a `;` outside them starts another statement (all of them
//! must be reads). The text is read under every quoting / comment mode the server may be in, and
//! is a READ only under all of them. The check is deliberately coarse in the safe direction: a
//! doubtful statement is a WRITE.

use crate::sqlgen::Dialect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementClass {
    Read,
    Write,
}

/// Words that make a `WITH …` statement a write wherever they appear in its code.
const WITH_WRITE_WORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "DROP", "ALTER", "TRUNCATE", "INTO",
];
/// EXPLAIN / DESCRIBE options that come before the explained statement.
const EXPLAIN_OPTION_WORDS: &[&str] = &["VERBOSE", "EXTENDED", "PARTITIONS", "QUERY", "PLAN"];
/// PRAGMAs that change the database although they take no `= value`.
const ACTION_PRAGMAS: &[&str] = &["OPTIMIZE", "WAL_CHECKPOINT", "INCREMENTAL_VACUUM"];
/// PRAGMAs whose `(argument)` names what to read rather than a value to set.
const READ_ARG_PRAGMAS: &[&str] = &[
    "TABLE_INFO",
    "TABLE_XINFO",
    "TABLE_LIST",
    "INDEX_LIST",
    "INDEX_INFO",
    "INDEX_XINFO",
    "FOREIGN_KEY_LIST",
    "FOREIGN_KEY_CHECK",
    "INTEGRITY_CHECK",
    "QUICK_CHECK",
];

/// How the server may read quotes and comments (it depends on server settings the classifier
/// cannot see); a statement is a READ only under every reading of its dialect.
#[derive(Debug, Clone, Copy)]
struct Reading {
    /// `'…'` strings take backslash escapes: MySQL unless `NO_BACKSLASH_ESCAPES`, PostgreSQL with
    /// `standard_conforming_strings = off`.
    single_backslash: bool,
    /// `"…"` strings take backslash escapes: MySQL unless `ANSI_QUOTES` (an identifier then) or
    /// `NO_BACKSLASH_ESCAPES`.
    double_backslash: bool,
    /// MySQL / MariaDB conditional comments run: versioned `/*!NNNNN … */`, MariaDB's
    /// `/*M! … */` and optimizer hints `/*+ … */` are code. Otherwise they are plain comments, as
    /// on a server older than the version, on MySQL for `/*M!`, or for a hint out of place. An
    /// unversioned `/*! … */` is code under every reading.
    exec_comments: bool,
}

const fn reading(single_backslash: bool, double_backslash: bool, exec_comments: bool) -> Reading {
    Reading {
        single_backslash,
        double_backslash,
        exec_comments,
    }
}

const POSTGRES_READINGS: &[Reading] = &[reading(false, false, false), reading(true, false, false)];
/// Default, `NO_BACKSLASH_ESCAPES`, `ANSI_QUOTES` (with backslash escapes in `'…'` only); each
/// with and without the conditional comments.
const MYSQL_READINGS: &[Reading] = &[
    reading(true, true, true),
    reading(false, false, true),
    reading(true, false, true),
    reading(true, true, false),
    reading(false, false, false),
    reading(true, false, false),
];
const SQLITE_READINGS: &[Reading] = &[reading(false, false, false)];

fn readings(dialect: Dialect) -> &'static [Reading] {
    match dialect {
        Dialect::Postgres => POSTGRES_READINGS,
        Dialect::Mysql => MYSQL_READINGS,
        Dialect::Sqlite => SQLITE_READINGS,
    }
}

fn is_ident_start(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphabetic() || c == '_')
}

fn is_word_char(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// Index just past the quote that closes the one at `i` (doubled = escaped).
fn skip_quoted(chars: &[char], i: usize, quote: char, backslash: bool) -> usize {
    let mut j = i + 1;
    while j < chars.len() {
        let c = chars[j];
        if backslash && c == '\\' {
            j += 2;
        } else if c == quote {
            if chars.get(j + 1) == Some(&quote) {
                j += 2;
            } else {
                return j + 1;
            }
        } else {
            j += 1;
        }
    }
    chars.len()
}

fn skip_line(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && chars[i] != '\n' {
        i += 1;
    }
    i
}

fn skip_block_comment(chars: &[char], i: usize, nested: bool) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while j < chars.len() {
        if chars[j] == '/' && chars.get(j + 1) == Some(&'*') {
            depth = if nested { depth + 1 } else { 1 };
            j += 2;
        } else if chars[j] == '*' && chars.get(j + 1) == Some(&'/') {
            depth = depth.saturating_sub(1);
            j += 2;
            if depth == 0 {
                return j;
            }
        } else {
            j += 1;
        }
    }
    chars.len()
}

/// Length of the PostgreSQL dollar-quote tag (`$$`, `$body$`) at `i`, or 0.
fn dollar_tag(chars: &[char], i: usize) -> usize {
    if i > 0 && is_word_char(chars.get(i - 1).copied()) {
        return 0;
    }
    let mut j = i + 1;
    if is_ident_start(chars.get(j).copied()) {
        while j < chars.len() && is_word_char(Some(chars[j])) && chars[j] != '$' {
            j += 1;
        }
    }
    if chars.get(j) == Some(&'$') {
        j - i + 1
    } else {
        0
    }
}

fn skip_dollar(chars: &[char], i: usize, len: usize) -> usize {
    let tag = &chars[i..i + len];
    let mut j = i + len;
    while j + len <= chars.len() {
        if &chars[j..j + len] == tag {
            return j + len;
        }
        j += 1;
    }
    chars.len()
}

/// Length of the opening of the MySQL / MariaDB comment at `i` when its body is code under
/// `reading` (`/*!`, `/*M!`, `/*+`; version digits stay in the code as a number), else None.
fn code_comment_open(chars: &[char], i: usize, reading: Reading) -> Option<usize> {
    let exec = reading.exec_comments;
    match (chars.get(i + 2), chars.get(i + 3)) {
        (Some('!'), next) => (exec || !next.is_some_and(char::is_ascii_digit)).then_some(3),
        (Some('M'), Some('!')) => exec.then_some(4),
        (Some('+'), _) => exec.then_some(3),
        _ => None,
    }
}

/// The PostgreSQL function that changes settings from a query.
const SET_CONFIG: &str = "set_config";

/// `sql` with every comment, string literal, quoted identifier and dollar-quoted body replaced
/// by one space, so keyword checks see only code. MySQL executable comments and optimizer hints
/// are code as `reading` says (see [`Reading`]). A PostgreSQL `"set_config"` stays a name.
fn code_only(sql: &str, dialect: Dialect, reading: Reading) -> Vec<char> {
    let chars: Vec<char> = sql.chars().collect();
    let mysql = dialect == Dialect::Mysql;
    let pg = dialect == Dialect::Postgres;
    let at = |k: usize| chars.get(k).copied();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = at(i + 1);
        let end = match c {
            '\'' => Some(skip_quoted(&chars, i, c, reading.single_backslash)),
            '"' => {
                let end = skip_quoted(&chars, i, c, reading.double_backslash);
                let name = chars.get(i + 1..end - 1).unwrap_or_default();
                if pg && name.iter().copied().eq(SET_CONFIG.chars()) {
                    out.extend(SET_CONFIG.chars());
                    i = end;
                    continue;
                }
                Some(end)
            }
            '`' if !pg => Some(skip_quoted(&chars, i, '`', false)),
            '[' if dialect == Dialect::Sqlite => Some(
                chars[i + 1..]
                    .iter()
                    .position(|&x| x == ']')
                    .map_or(chars.len(), |p| i + 1 + p + 1),
            ),
            '-' if next == Some('-')
                && (!mysql || at(i + 2).is_none_or(|x| x.is_whitespace() || x.is_control())) =>
            {
                Some(skip_line(&chars, i))
            }
            '#' if mysql => Some(skip_line(&chars, i)),
            '/' if next == Some('*') => {
                if mysql && let Some(len) = code_comment_open(&chars, i, reading) {
                    out.extend(std::iter::repeat_n(' ', len));
                    i += len;
                    continue;
                }
                Some(skip_block_comment(&chars, i, pg))
            }
            '$' if pg => match dollar_tag(&chars, i) {
                0 => None,
                len => Some(skip_dollar(&chars, i, len)),
            },
            c if is_ident_start(Some(c)) => {
                let mut j = i;
                while j < chars.len() && is_word_char(Some(chars[j])) {
                    j += 1;
                }
                out.extend_from_slice(&chars[i..j]);
                // PostgreSQL escape strings: E'…' with backslash escapes.
                if pg && j == i + 1 && (c == 'e' || c == 'E') && at(j) == Some('\'') {
                    out.push(' ');
                    i = skip_quoted(&chars, j, '\'', true);
                } else {
                    i = j;
                }
                continue;
            }
            _ => None,
        };
        match end {
            Some(end) => {
                out.push(' ');
                i = end.max(i + 1);
            }
            None => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// A word (upper-cased; numbers count as words) or a single punctuation character.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    word: bool,
    text: String,
}

impl Token {
    fn is(&self, text: &str) -> bool {
        self.text == text
    }

    fn is_word_in(&self, words: &[&str]) -> bool {
        self.word && words.contains(&self.text.as_str())
    }
}

/// Splits code-only text at `;` into the tokens of each statement (empty ones dropped).
fn statement_tokens(code: &[char]) -> Vec<Vec<Token>> {
    let mut statements = Vec::new();
    let mut current = Vec::new();
    let mut i = 0;
    while i < code.len() {
        let c = code[i];
        if c == ';' {
            if !current.is_empty() {
                statements.push(std::mem::take(&mut current));
            }
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if is_word_char(Some(c)) {
            let mut j = i;
            while j < code.len() && is_word_char(Some(code[j])) {
                j += 1;
            }
            current.push(Token {
                word: true,
                text: code[i..j].iter().collect::<String>().to_uppercase(),
            });
            i = j;
        } else {
            current.push(Token {
                word: false,
                text: c.to_string(),
            });
            i += 1;
        }
    }
    if !current.is_empty() {
        statements.push(current);
    }
    statements
}

fn has_word(tokens: &[Token], words: &[&str]) -> bool {
    tokens.iter().any(|t| t.is_word_in(words))
}

fn read_unless(write: bool) -> StatementClass {
    if write {
        StatementClass::Write
    } else {
        StatementClass::Read
    }
}

fn classify_explain(tokens: &[Token]) -> StatementClass {
    let analyze_words = ["ANALYZE", "ANALYSE"];
    let mut analyze = false;
    let mut i = 0;
    if tokens.first().is_some_and(|t| t.is("(")) {
        // PostgreSQL option list: EXPLAIN (ANALYZE, BUFFERS) … — any ANALYZE counts.
        let mut depth = 0i32;
        while i < tokens.len() {
            let t = &tokens[i];
            if t.is("(") {
                depth += 1;
            } else if t.is(")") {
                depth -= 1;
            } else if t.is_word_in(&analyze_words) {
                analyze = true;
            }
            i += 1;
            if depth == 0 {
                break;
            }
        }
    }
    while let Some(t) = tokens.get(i).filter(|t| t.word) {
        if t.is_word_in(&analyze_words) {
            analyze = true;
            i += 1;
        } else if t.is_word_in(EXPLAIN_OPTION_WORDS) {
            i += 1;
        } else if t.is("FORMAT") {
            i += 1;
            if tokens.get(i).is_some_and(|t| t.is("=")) {
                i += 1;
            }
            if tokens.get(i).is_some_and(|t| t.word) {
                i += 1;
            }
        } else {
            break;
        }
    }
    // A plain EXPLAIN only plans; EXPLAIN ANALYZE runs the statement.
    if analyze {
        classify_tokens(&tokens[i..])
    } else {
        StatementClass::Read
    }
}

fn classify_pragma(tokens: &[Token]) -> StatementClass {
    if tokens.iter().any(|t| t.is("=")) {
        return StatementClass::Write;
    }
    // PRAGMA [schema.]name[(argument)]
    let i = if tokens.get(1).is_some_and(|t| t.is(".")) {
        2
    } else {
        0
    };
    let Some(name) = tokens.get(i).filter(|t| t.word) else {
        return StatementClass::Write;
    };
    let with_arg = tokens.get(i + 1).is_some_and(|t| t.is("("));
    read_unless(name.is_word_in(ACTION_PRAGMAS) || (with_arg && !name.is_word_in(READ_ARG_PRAGMAS)))
}

/// `set_config(…)`, `pg_catalog.set_config(…)`: changes settings (even
/// `default_transaction_read_only`) although it is called from a SELECT.
fn calls_set_config(tokens: &[Token]) -> bool {
    tokens
        .windows(2)
        .any(|w| w[0].word && w[0].is("SET_CONFIG") && w[1].is("("))
}

fn classify_tokens(tokens: &[Token]) -> StatementClass {
    if calls_set_config(tokens) {
        return StatementClass::Write;
    }
    let start = tokens.iter().take_while(|t| t.is("(")).count();
    let Some(first) = tokens.get(start).filter(|t| t.word) else {
        return StatementClass::Write;
    };
    let rest = &tokens[start + 1..];
    match first.text.as_str() {
        "SELECT" | "VALUES" | "TABLE" => read_unless(has_word(rest, &["INTO"])),
        "WITH" => read_unless(has_word(rest, WITH_WRITE_WORDS)),
        "SHOW" | "USE" => StatementClass::Read,
        "EXPLAIN" | "DESCRIBE" | "DESC" => classify_explain(rest),
        "PRAGMA" => classify_pragma(rest),
        _ => StatementClass::Write,
    }
}

fn reads_under(sql: &str, dialect: Dialect, reading: Reading) -> bool {
    let statements = statement_tokens(&code_only(sql, dialect, reading));
    !statements.is_empty()
        && statements
            .iter()
            .all(|tokens| classify_tokens(tokens) == StatementClass::Read)
}

/// Classifies one statement (several separated by `;` are a READ only when each one is). The
/// text is read under every quoting / comment mode of the dialect (see [`Reading`]: MySQL with
/// and without backslash escapes, `ANSI_QUOTES`, conditional comments; PostgreSQL with and
/// without `standard_conforming_strings`) and is a READ only under all of them.
pub fn classify(sql: &str, dialect: Dialect) -> StatementClass {
    let all_read = readings(dialect)
        .iter()
        .all(|&reading| reads_under(sql, dialect, reading));
    read_unless(!all_read)
}

/// `true` unless `sql` is a READ.
pub fn writes(sql: &str, dialect: Dialect) -> bool {
    classify(sql, dialect) == StatementClass::Write
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Case {
        sql: String,
        dialect: Option<String>,
        class: String,
    }

    fn dialect(name: &str) -> Dialect {
        match name {
            "postgres" => Dialect::Postgres,
            "mysql" => Dialect::Mysql,
            "sqlite" => Dialect::Sqlite,
            other => panic!("unknown dialect {other}"),
        }
    }

    #[test]
    fn shared_cases_match_the_renderer() {
        let cases: Vec<Case> = serde_json::from_str(include_str!(
            "../../../src/features/safety/classify.cases.json"
        ))
        .unwrap();
        assert!(cases.len() > 50);
        for case in cases {
            let dialects = match &case.dialect {
                Some(d) => vec![dialect(d)],
                None => vec![Dialect::Postgres, Dialect::Mysql, Dialect::Sqlite],
            };
            let expected = match case.class.as_str() {
                "read" => StatementClass::Read,
                "write" => StatementClass::Write,
                other => panic!("unknown class {other}"),
            };
            for d in dialects {
                assert_eq!(classify(&case.sql, d), expected, "{d:?}: {:?}", case.sql);
            }
        }
    }

    #[test]
    fn writes_is_the_negation_of_read() {
        assert!(!writes("SELECT 1", Dialect::Postgres));
        assert!(writes("DELETE FROM t", Dialect::Postgres));
        assert!(writes("", Dialect::Sqlite));
    }

    #[test]
    fn unterminated_quotes_and_comments_hide_the_rest() {
        // The server rejects these anyway; nothing after the opening quote is read as code.
        assert_eq!(
            classify("SELECT 'abc", Dialect::Postgres),
            StatementClass::Read
        );
        assert_eq!(
            classify("SELECT 1 /* DELETE", Dialect::Mysql),
            StatementClass::Read
        );
        assert_eq!(
            classify("SELECT $x$ ; DROP", Dialect::Postgres),
            StatementClass::Read
        );
    }

    #[test]
    fn non_ascii_text_is_handled() {
        assert_eq!(
            classify("SELECT 'héllo' AS ünïcode FROM tàble", Dialect::Postgres),
            StatementClass::Read
        );
        assert_eq!(
            classify("UPDATE tàble SET ü = 'é'", Dialect::Mysql),
            StatementClass::Write
        );
    }
}
