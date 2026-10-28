//! Safe-mode statement classifier: is a statement a READ or a WRITE? The renderer's copy
//! (`src/features/safety/classify.ts`) follows the same rules; both run the shared cases in
//! `src/features/safety/classify.cases.json`.
//!
//! READ: `SELECT` / `VALUES` / `TABLE` (unless they hold an `INTO`), `SHOW`, `USE <db>`,
//! `EXPLAIN` / `DESCRIBE` / `DESC` (an EXPLAIN ANALYZE is classified by the statement it runs,
//! an EXPLAIN of a PRAGMA as the PRAGMA, which SQLite applies while preparing it),
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

/// Whether PostgreSQL reads `c` as part of an identifier: every non-ASCII character is one (so
/// `€$$` is a single name, not a name and a dollar quote), as are the letters, digits, `_` and `$`.
fn is_pg_word_char(c: Option<char>) -> bool {
    c.is_some_and(|c| !c.is_ascii() || is_word_char(Some(c)))
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

/// End of the line comment that starts at `i`. `cr_ends`: PostgreSQL ends one at a lone CR as well
/// as at LF (`SELECT 1 --x<CR>, pg_sleep(100)` calls `pg_sleep`); MySQL and SQLite read to the LF.
fn skip_line(chars: &[char], mut i: usize, cr_ends: bool) -> usize {
    while i < chars.len() && chars[i] != '\n' && !(cr_ends && chars[i] == '\r') {
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
    if i > 0 && is_pg_word_char(chars.get(i - 1).copied()) {
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

/// Number of ASCII digits in a row at `i`.
fn digit_run(chars: &[char], i: usize) -> usize {
    chars
        .get(i..)
        .unwrap_or_default()
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count()
}

/// Length of the opening of the MySQL / MariaDB comment at `i` when its body is code under
/// `reading` (`/*!`, `/*M!`, `/*+`), else None. The opening of a versioned comment
/// (`/*!50000`, `/*M!100100`) includes its version number: the server reads the digits as the
/// version, not as code, so the body may start right after them (`/*!50000SLEEP*/(1)` calls
/// `SLEEP`) and must not be glued to them as one word. Fewer than five digits are code.
fn code_comment_open(chars: &[char], i: usize, reading: Reading) -> Option<usize> {
    let exec = reading.exec_comments;
    let version = |from: usize| match digit_run(chars, from) {
        0 => None,
        n if n >= 5 => Some(n),
        _ => Some(0),
    };
    match (chars.get(i + 2), chars.get(i + 3)) {
        // Unversioned: code under every reading. Versioned (a number follows): only when the
        // server is new enough, which `exec` stands for.
        (Some('!'), _) => match version(i + 3) {
            None => Some(3),
            Some(n) => exec.then_some(3 + n),
        },
        (Some('M'), Some('!')) => exec.then_some(4 + version(i + 4).unwrap_or(0)),
        (Some('+'), _) => exec.then_some(3),
        _ => None,
    }
}

/// End of the whitespace and comments at `i` (PostgreSQL: block comments nest).
fn skip_blank(chars: &[char], mut i: usize) -> usize {
    loop {
        match (chars.get(i), chars.get(i + 1)) {
            (Some(c), _) if c.is_whitespace() => i += 1,
            (Some('-'), Some('-')) => i = skip_line(chars, i, true),
            (Some('/'), Some('*')) => i = skip_block_comment(chars, i, true),
            _ => return i,
        }
    }
}

/// Whether the word `word` (upper case) is at `i`, in any case, and ends there.
fn word_at(chars: &[char], i: usize, word: &str) -> bool {
    let len = word.chars().count();
    chars
        .get(i..i + len)
        .is_some_and(|got| got.iter().flat_map(|c| c.to_uppercase()).eq(word.chars()))
        && !is_word_char(chars.get(i + len).copied())
}

/// The text of a PostgreSQL `U&"…"` identifier written with the default escape character `\`:
/// `\XXXX` and `\+XXXXXX` are code points (a high and a low surrogate together make one), `\\` is
/// a backslash. None when the server would refuse the text (a stray backslash, bad digits, a
/// lone surrogate).
fn unicode_escaped_name(raw: &[char]) -> Option<Vec<char>> {
    // The code point and the length of the escape that starts at the backslash at `i`.
    let escape = |i: usize| -> Option<(u32, usize)> {
        let (digits, len) = match raw.get(i + 1)? {
            '+' => (raw.get(i + 2..i + 8)?, 8),
            _ => (raw.get(i + 1..i + 5)?, 5),
        };
        let digits: String = digits.iter().collect();
        if digits.chars().all(|c| c.is_ascii_hexdigit()) {
            u32::from_str_radix(&digits, 16)
                .ok()
                .map(|code| (code, len))
        } else {
            None
        }
    };
    let mut name = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] != '\\' {
            name.push(raw[i]);
            i += 1;
        } else if raw.get(i + 1) == Some(&'\\') {
            name.push('\\');
            i += 2;
        } else {
            let (mut code, mut len) = escape(i)?;
            if (0xD800..0xDC00).contains(&code) {
                if raw.get(i + len) != Some(&'\\') {
                    return None;
                }
                let (low, low_len) = escape(i + len)?;
                if !(0xDC00..0xE000).contains(&low) {
                    return None;
                }
                code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                len += low_len;
            }
            name.push(char::from_u32(code)?);
            i += len;
        }
    }
    Some(name)
}

/// End of the `UESCAPE 'c'` clause that follows a `U&"…"` identifier ending at `i` (the escape
/// character is any single-character string constant: `'!'`, `E'!'`, `$$!$$`, even continued over
/// lines), or None when there is no such clause.
fn uescape_clause(chars: &[char], i: usize) -> Option<usize> {
    let keyword = skip_blank(chars, i);
    if !word_at(chars, keyword, "UESCAPE") {
        return None;
    }
    let operand = skip_blank(chars, keyword + "UESCAPE".len());
    let mut end = match (chars.get(operand), chars.get(operand + 1)) {
        (Some('\''), _) => skip_quoted(chars, operand, '\'', false),
        (Some('e' | 'E'), Some('\'')) => skip_quoted(chars, operand + 1, '\'', true),
        (Some('$'), _) => match dollar_tag(chars, operand) {
            0 => return Some(operand),
            len => skip_dollar(chars, operand, len),
        },
        // Not a string constant: the server refuses the statement.
        _ => return Some(operand),
    };
    loop {
        let more = skip_blank(chars, end);
        if chars.get(more) != Some(&'\'') {
            return Some(end);
        }
        end = skip_quoted(chars, more, '\'', false);
    }
}

/// The quoted function names `code_only_keeping` keeps in the code as bare names, and how it
/// treats a name it cannot read.
#[derive(Clone, Copy)]
struct Names {
    /// Whether a call of the (unquoted, decoded) name matters.
    keep: fn(&[char]) -> bool,
    /// The word that stands in for a PostgreSQL `U&"…"` identifier whose name cannot be worked
    /// out (an escape character set by `UESCAPE`, or an escape the server refuses), so that a
    /// call of it counts as a call of a name that matters. None: such an identifier is blanked
    /// like any other quoted name.
    unreadable: Option<&'static str>,
}

fn no_name(_: &[char]) -> bool {
    false
}

/// The PostgreSQL function that changes settings from a query.
const SET_CONFIG: &str = "set_config";

fn is_set_config_name(name: &[char]) -> bool {
    name.iter().copied().eq(SET_CONFIG.chars())
}

/// No quoted name matters (MySQL and SQLite statements for [`classify`]).
const NO_NAMES: Names = Names {
    keep: no_name,
    unreadable: None,
};
/// A quoted `"set_config"` is the function: [`classify`] for PostgreSQL.
const SET_CONFIG_NAME: Names = Names {
    keep: is_set_config_name,
    unreadable: Some(SET_CONFIG),
};
/// A quoted name of [`SIDE_EFFECT_FUNCTIONS`] is that function: [`calls_side_effect_function`].
const SIDE_EFFECT_NAMES: Names = Names {
    keep: is_side_effect_name,
    unreadable: Some(UNREADABLE_NAME),
};

/// `sql` with every comment, string literal, quoted identifier and dollar-quoted body replaced
/// by one space, so keyword checks see only code. MySQL executable comments and optimizer hints
/// are code as `reading` says (see [`Reading`]). A PostgreSQL `"set_config"` stays a name.
fn code_only(sql: &str, dialect: Dialect, reading: Reading) -> Vec<char> {
    let names = if dialect == Dialect::Postgres {
        SET_CONFIG_NAME
    } else {
        NO_NAMES
    };
    code_only_keeping(sql, dialect, reading, names)
}

/// [`code_only`] where a quoted identifier (`"name"`, `` `name` `` outside PostgreSQL, SQLite's
/// `[name]`) that `names` accepts stays in the code as a bare name, padded with spaces: the
/// server reads `"set_config"(…)` or MySQL's `` `SLEEP`(…) `` as a call of that function. A
/// PostgreSQL `U&"…"` identifier is decoded first (see [`unicode_escaped_name`]).
///
/// MySQL executable comments and optimizer hints are code under the readings that run them, so
/// the `*/` that ends one is not code: it is read as a space, as the server does
/// (`SLEEP/*!*/(1)` calls `SLEEP`), and not as the two tokens `*` and `/`.
fn code_only_keeping(sql: &str, dialect: Dialect, reading: Reading, names: Names) -> Vec<char> {
    let chars: Vec<char> = sql.chars().collect();
    let mysql = dialect == Dialect::Mysql;
    let pg = dialect == Dialect::Postgres;
    let sqlite = dialect == Dialect::Sqlite;
    let at = |k: usize| chars.get(k).copied();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    // Inside an executable comment: its closing `*/` is still to come.
    let mut in_code_comment = false;
    while i < chars.len() {
        let c = chars[i];
        let next = at(i + 1);
        let end = match c {
            '\'' => Some(skip_quoted(&chars, i, c, reading.single_backslash)),
            '"' | '`' | '[' if c == '"' || (c == '`' && !pg) || (c == '[' && sqlite) => {
                let end = match c {
                    '"' => skip_quoted(&chars, i, c, reading.double_backslash),
                    '`' => skip_quoted(&chars, i, c, false),
                    _ => chars[i + 1..]
                        .iter()
                        .position(|&x| x == ']')
                        .map_or(chars.len(), |p| i + 1 + p + 1),
                };
                let name = chars.get(i + 1..end.saturating_sub(1)).unwrap_or_default();
                if (names.keep)(name) {
                    out.push(' ');
                    out.extend_from_slice(name);
                    out.push(' ');
                    i = end;
                    continue;
                }
                Some(end)
            }
            '-' if next == Some('-')
                && (!mysql || at(i + 2).is_none_or(|x| x.is_whitespace() || x.is_control())) =>
            {
                Some(skip_line(&chars, i, pg))
            }
            '#' if mysql => Some(skip_line(&chars, i, false)),
            '/' if next == Some('*') => {
                if mysql && let Some(len) = code_comment_open(&chars, i, reading) {
                    out.extend(std::iter::repeat_n(' ', len));
                    i += len;
                    in_code_comment = true;
                    continue;
                }
                Some(skip_block_comment(&chars, i, pg))
            }
            '*' if in_code_comment && next == Some('/') => {
                in_code_comment = false;
                Some(i + 2)
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
                // `€e'…'` is the name `€e` and a plain string, not an escape string: PostgreSQL
                // reads every non-ASCII character as part of a name.
                let after_pg_word = i > 0 && is_pg_word_char(at(i - 1));
                // PostgreSQL Unicode-escaped identifiers: U&"d\0061ta" [UESCAPE '!'].
                if pg
                    && j == i + 1
                    && !after_pg_word
                    && (c == 'u' || c == 'U')
                    && at(j) == Some('&')
                    && at(j + 1) == Some('"')
                {
                    let quoted_end = skip_quoted(&chars, j + 1, '"', false);
                    let raw = chars
                        .get(j + 2..quoted_end.saturating_sub(1))
                        .unwrap_or_default();
                    let clause_end = uescape_clause(&chars, quoted_end);
                    let name = match clause_end {
                        Some(_) => None,
                        None => unicode_escaped_name(raw),
                    };
                    match (name, names.unreadable) {
                        (Some(name), _) if (names.keep)(&name) => {
                            out.push(' ');
                            out.extend(name);
                            out.push(' ');
                        }
                        (None, Some(stand_in)) => {
                            out.push(' ');
                            out.extend(stand_in.chars());
                            out.push(' ');
                        }
                        _ => out.push(' '),
                    }
                    i = clause_end.unwrap_or(quoted_end);
                    continue;
                }
                out.extend_from_slice(&chars[i..j]);
                // PostgreSQL escape strings: E'…' with backslash escapes.
                if pg
                    && j == i + 1
                    && !after_pg_word
                    && (c == 'e' || c == 'E')
                    && at(j) == Some('\'')
                {
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
    // A plain EXPLAIN only plans; EXPLAIN ANALYZE runs the statement. SQLite applies a setting
    // PRAGMA while it prepares it, so `EXPLAIN [QUERY PLAN] PRAGMA query_only = 0` changes the
    // setting: it is the PRAGMA. A lone `pragma` is a table to MySQL's EXPLAIN / DESCRIBE.
    let pragma = tokens.get(i).is_some_and(|t| t.is("PRAGMA")) && tokens.len() > i + 1;
    if analyze || pragma {
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

/// Functions a SELECT can call that change the server or hold a lock (the PostgreSQL, MySQL and
/// MariaDB ones; a name no other dialect has costs nothing there). [`classify`] calls such a
/// SELECT a READ (Safe Mode must not stop `SELECT nextval('s')` in the editor), but the Metrics
/// Board runs reads on open and on a timer under "Off: statements that change data are
/// refused", so it refuses them too ([`calls_side_effect_function`]). Upper case.
const SIDE_EFFECT_FUNCTIONS: &[&str] = &[
    // Sequences (PostgreSQL, MariaDB).
    "SETVAL",
    "NEXTVAL",
    // PostgreSQL server control.
    "PG_TERMINATE_BACKEND",
    "PG_CANCEL_BACKEND",
    "PG_RELOAD_CONF",
    "PG_ROTATE_LOGFILE",
    "PG_SWITCH_WAL",
    "PG_CREATE_RESTORE_POINT",
    "PG_PROMOTE",
    "PG_WAL_REPLAY_PAUSE",
    "PG_WAL_REPLAY_RESUME",
    "PG_BACKUP_START",
    "PG_BACKUP_STOP",
    "PG_START_BACKUP",
    "PG_STOP_BACKUP",
    "PG_NOTIFY",
    "PG_LOGICAL_EMIT_MESSAGE",
    "PG_STAT_RESET",
    "PG_STAT_RESET_SHARED",
    "PG_STAT_RESET_SINGLE_TABLE_COUNTERS",
    "PG_STAT_RESET_SINGLE_FUNCTION_COUNTERS",
    "PG_CREATE_PHYSICAL_REPLICATION_SLOT",
    "PG_CREATE_LOGICAL_REPLICATION_SLOT",
    "PG_DROP_REPLICATION_SLOT",
    "PG_REPLICATION_SLOT_ADVANCE",
    "PG_LOGICAL_SLOT_GET_CHANGES",
    "PG_LOGICAL_SLOT_GET_BINARY_CHANGES",
    "PG_COPY_LOGICAL_REPLICATION_SLOT",
    "PG_COPY_PHYSICAL_REPLICATION_SLOT",
    "PG_REPLICATION_ORIGIN_CREATE",
    "PG_REPLICATION_ORIGIN_DROP",
    "PG_REPLICATION_ORIGIN_ADVANCE",
    "PG_STAT_STATEMENTS_RESET",
    "PG_LOG_BACKEND_MEMORY_CONTEXTS",
    // PostgreSQL advisory locks: all eleven functions of the family (session and transaction
    // level, exclusive and shared, blocking and `try`, and the unlocks).
    "PG_ADVISORY_LOCK",
    "PG_ADVISORY_LOCK_SHARED",
    "PG_ADVISORY_XACT_LOCK",
    "PG_ADVISORY_XACT_LOCK_SHARED",
    "PG_TRY_ADVISORY_LOCK",
    "PG_TRY_ADVISORY_LOCK_SHARED",
    "PG_TRY_ADVISORY_XACT_LOCK",
    "PG_TRY_ADVISORY_XACT_LOCK_SHARED",
    "PG_ADVISORY_UNLOCK",
    "PG_ADVISORY_UNLOCK_SHARED",
    "PG_ADVISORY_UNLOCK_ALL",
    // PostgreSQL large objects (server-side files too).
    "LO_IMPORT",
    "LO_EXPORT",
    "LO_UNLINK",
    "LO_CREAT",
    "LO_CREATE",
    "LO_PUT",
    "LO_FROM_BYTEA",
    "LO_OPEN",
    "LO_TRUNCATE",
    "LO_TRUNCATE64",
    "LOWRITE",
    // dblink: runs any statement, on any database.
    "DBLINK",
    "DBLINK_EXEC",
    "DBLINK_CONNECT",
    "DBLINK_CONNECT_U",
    "DBLINK_SEND_QUERY",
    // Sleeping holds the worker connection.
    "PG_SLEEP",
    "PG_SLEEP_FOR",
    "PG_SLEEP_UNTIL",
    "SLEEP",
    "BENCHMARK",
    // MySQL / MariaDB locks and waits.
    "GET_LOCK",
    "RELEASE_LOCK",
    "RELEASE_ALL_LOCKS",
    "MASTER_POS_WAIT",
    "SOURCE_POS_WAIT",
    "WAIT_FOR_EXECUTED_GTID_SET",
    "WAIT_UNTIL_SQL_THREAD_AFTER_GTIDS",
    // SQLite loads code from a file.
    "LOAD_EXTENSION",
    // Not a function: what `code_only_keeping` writes for a `U&"…"` name it cannot read.
    "$UNREADABLE_NAME$",
];

/// The word that stands in for a PostgreSQL Unicode-escaped function name that cannot be read
/// (see [`Names::unreadable`]). No identifier of the user can spell it: it starts with `$`.
const UNREADABLE_NAME: &str = "$unreadable_name$";

fn is_side_effect_name(name: &[char]) -> bool {
    let name: String = name.iter().collect();
    SIDE_EFFECT_FUNCTIONS.contains(&name.to_uppercase().as_str())
}

/// Whether `sql` calls a function that changes the server although the statement is a SELECT
/// ([`SIDE_EFFECT_FUNCTIONS`], or MariaDB's `NEXT VALUE FOR`): a word of the list directly
/// followed by `(`, as in `setval(…)` or `pg_catalog.setval(…)`. Strings, comments and a column
/// or alias of that name are not calls; a quoted name (`"setval"(…)`, MySQL `` `sleep`(…) ``,
/// SQLite `[load_extension](…)`) is, as the server calls it. The ways to write a name that the
/// server reads as the same function are read as it reads them:
///
/// - MySQL / MariaDB executable comments and hints are code, version number and closing `*/`
///   apart: `SLEEP/*!*/(1)`, `/*!50000 GET_LOCK*/('a', 10)`, `/*!50000SLEEP*/(1)`,
///   `/*M!100100 SETVAL*/(s, 1)` and `NEXT /*!*/ VALUE FOR s` all call the function.
/// - A PostgreSQL `U&"set\0076al"(…)` is decoded (`\XXXX`, `\+XXXXXX`, `\\`). One whose name cannot
///   be worked out (a `UESCAPE` clause sets another escape character, or the server would refuse
///   the escape) counts as a call of a function of the list when it is called.
///
/// Read under every quoting / comment mode of the dialect like [`classify`]; any of them calling
/// one counts. Unlike [`classify`] this is not about the statement kind: it only tells the
/// Metrics Board which reads it must refuse.
pub fn calls_side_effect_function(sql: &str, dialect: Dialect) -> bool {
    readings(dialect).iter().any(|&reading| {
        let code = code_only_keeping(sql, dialect, reading, SIDE_EFFECT_NAMES);
        statement_tokens(&code).iter().any(|tokens| {
            tokens
                .windows(2)
                .any(|w| w[0].is_word_in(SIDE_EFFECT_FUNCTIONS) && w[1].is("("))
                || (dialect == Dialect::Mysql
                    && tokens
                        .windows(3)
                        .any(|w| w[0].is("NEXT") && w[1].is("VALUE") && w[2].is("FOR")))
        })
    })
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

    /// SQLite applies a setting PRAGMA while it prepares the statement, under EXPLAIN too: the
    /// EXPLAIN turns `query_only` off and the INSERT after it goes through. So it is a write.
    #[test]
    fn sqlite_applies_a_pragma_it_only_explains() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE t (a); PRAGMA query_only = 1")
            .unwrap();
        assert!(db.execute("INSERT INTO t VALUES (1)", []).is_err());
        for sql in [
            "EXPLAIN PRAGMA query_only = 0",
            "EXPLAIN QUERY PLAN PRAGMA query_only = 0",
        ] {
            db.execute_batch("PRAGMA query_only = 1").unwrap();
            let mut stmt = db.prepare(sql).unwrap();
            let mut rows = stmt.query([]).unwrap();
            while rows.next().unwrap().is_some() {}
            drop(rows);
            drop(stmt);
            assert!(db.execute("INSERT INTO t VALUES (1)", []).is_ok(), "{sql}");
            assert_eq!(
                classify(sql, Dialect::Sqlite),
                StatementClass::Write,
                "{sql}"
            );
        }
    }

    #[test]
    fn side_effect_functions_are_found_for_every_family() {
        let flagged = |dialect: Dialect, list: &[&str]| {
            for sql in list {
                assert!(
                    calls_side_effect_function(sql, dialect),
                    "{dialect:?}: {sql}"
                );
            }
        };
        flagged(
            Dialect::Postgres,
            &[
                "SELECT setval('s', 1)",
                "SELECT nextval('s')",
                "SELECT pg_catalog.setval('s', 1)",
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE pid <> pg_backend_pid()",
                "SELECT pg_cancel_backend(42)",
                "SELECT pg_reload_conf()",
                "SELECT pg_switch_wal()",
                "SELECT pg_create_restore_point('x')",
                "SELECT pg_advisory_lock(1)",
                "SELECT pg_advisory_xact_lock_shared(1, 2)",
                "SELECT pg_try_advisory_lock(1)",
                "SELECT pg_try_advisory_lock_shared(1)",
                "SELECT pg_try_advisory_xact_lock_shared(1, 2)",
                "SELECT pg_advisory_unlock_shared(1)",
                "SELECT pg_advisory_unlock_all()",
                "SELECT lo_unlink(1234)",
                "SELECT lo_import('/etc/passwd')",
                "SELECT lo_from_bytea(0, 'x'::bytea)",
                "SELECT dblink_exec('dbname=x', 'DELETE FROM t')",
                "SELECT dblink_connect('c', 'dbname=x')",
                "SELECT pg_sleep(100)",
                "SELECT * FROM pg_sleep_for('5 minutes')",
                // Case, spacing, comments and a column list before it do not hide the call.
                "select a, NextVal  ('s') from t",
                "SELECT setval/* x */('s', 1)",
                "SELECT 1; SELECT nextval('s')",
                // A quoted name is the function too.
                r#"SELECT "setval"('s', 1)"#,
                r#"SELECT pg_catalog."nextval"('s')"#,
            ],
        );
        flagged(
            Dialect::Mysql,
            &[
                "SELECT GET_LOCK('a', 10)",
                "SELECT get_lock('a', 10)",
                "SELECT RELEASE_LOCK('a')",
                "SELECT RELEASE_ALL_LOCKS()",
                "SELECT SLEEP(100000)",
                "SELECT BENCHMARK(100000000, MD5('x'))",
                "SELECT /*!50000 SLEEP(5) */ 1",
                "SELECT NEXTVAL(s)",
                "SELECT SETVAL(s, 10)",
                "SELECT NEXT VALUE FOR s",
                "SELECT `sleep`(1)",
                "SELECT 1 FROM t WHERE SLEEP(1) = 0",
            ],
        );
        flagged(Dialect::Sqlite, &["SELECT load_extension('x')"]);
    }

    /// Every function of PostgreSQL's advisory-lock family (the documented list of "Advisory Lock
    /// Functions"), with the one-bigint and the two-int forms, taking a session-level or
    /// transaction-level lock on the pooled worker connection or releasing one.
    #[test]
    fn every_advisory_lock_function_is_found() {
        let family = [
            "pg_advisory_lock",
            "pg_advisory_lock_shared",
            "pg_advisory_xact_lock",
            "pg_advisory_xact_lock_shared",
            "pg_try_advisory_lock",
            "pg_try_advisory_lock_shared",
            "pg_try_advisory_xact_lock",
            "pg_try_advisory_xact_lock_shared",
            "pg_advisory_unlock",
            "pg_advisory_unlock_shared",
        ];
        for name in family {
            for args in ["1", "1, 2"] {
                for sql in [
                    format!("SELECT {name}({args})"),
                    format!("select {}({args})", name.to_uppercase()),
                    format!("SELECT pg_catalog.{name} ({args})"),
                    format!("SELECT 1 WHERE {name}({args})"),
                ] {
                    assert!(calls_side_effect_function(&sql, Dialect::Postgres), "{sql}");
                    // A read for Safe Mode: only the Metrics Board refuses it.
                    assert_eq!(classify(&sql, Dialect::Postgres), StatementClass::Read);
                }
            }
        }
        assert!(calls_side_effect_function(
            "SELECT pg_advisory_unlock_all()",
            Dialect::Postgres
        ));
        // A column or an alias with such a name is not a call.
        assert!(!calls_side_effect_function(
            "SELECT 1 AS pg_try_advisory_lock_shared, pg_advisory_unlock_shared FROM t",
            Dialect::Postgres
        ));
    }

    #[test]
    fn side_effect_functions_are_not_confused_with_text_or_names() {
        let quiet = |dialect: Dialect, list: &[&str]| {
            for sql in list {
                assert!(
                    !calls_side_effect_function(sql, dialect),
                    "{dialect:?}: {sql}"
                );
            }
        };
        quiet(
            Dialect::Postgres,
            &[
                "SELECT 'setval(1)'",
                "SELECT 'it''s nextval(1)' AS t",
                "SELECT 1 -- nextval(1)",
                "/* pg_sleep(5) */ SELECT 1",
                "SELECT 1 AS nextval",
                "SELECT nextval FROM t",
                r#"SELECT "nextval" FROM t"#,
                r#"SELECT 'x' AS "setval""#,
                "SELECT $$setval(1)$$",
                "SELECT $f$ pg_terminate_backend(1) $f$",
                r"SELECT E'nextval(\'s\')'",
                // Other words that merely contain a name.
                "SELECT my_nextval(1), setvalue(2), sleeping(3)",
                "SELECT currval('s'), lastval(), count(*) FROM t",
                "SELECT * FROM t WHERE next value = 1",
                "SELECT now(), pg_backend_pid(), pg_is_in_recovery()",
            ],
        );
        quiet(
            Dialect::Mysql,
            &[
                "SELECT 'GET_LOCK(1)'",
                "SELECT 1 # SLEEP(5)",
                "SELECT 1 -- SLEEP(5)\n",
                "SELECT 1 /* BENCHMARK(1, 1) */",
                "SELECT sleep FROM t",
                "SELECT `sleep` FROM t",
                "SELECT IS_FREE_LOCK('a'), NOW()",
            ],
        );
        quiet(
            Dialect::Sqlite,
            &["SELECT 'load_extension(1)'", "SELECT 1 AS sleep"],
        );
        // Only a MySQL / MariaDB statement can say NEXT VALUE FOR.
        assert!(!calls_side_effect_function(
            "SELECT next value for FROM t",
            Dialect::Postgres
        ));
    }

    #[test]
    fn side_effect_functions_do_not_change_the_statement_class() {
        // `classify` (Safe Mode, the editor) still calls these reads; only the Metrics Board
        // refuses them through `calls_side_effect_function`.
        for (sql, dialect) in [
            ("SELECT setval('s', 1)", Dialect::Postgres),
            ("SELECT pg_terminate_backend(1)", Dialect::Postgres),
            ("SELECT GET_LOCK('a', 10)", Dialect::Mysql),
            ("SELECT SLEEP(1)", Dialect::Mysql),
        ] {
            assert_eq!(classify(sql, dialect), StatementClass::Read, "{sql}");
            assert!(calls_side_effect_function(sql, dialect), "{sql}");
        }
        // Keeping a quoted `"set_config"` a name still makes it the write it always was.
        assert_eq!(
            classify(r#"SELECT"set_config"('a', 'b', false)"#, Dialect::Postgres),
            StatementClass::Write
        );
    }

    fn flagged(dialect: Dialect, list: &[&str]) {
        for sql in list {
            assert!(
                calls_side_effect_function(sql, dialect),
                "{dialect:?}: {sql}"
            );
        }
    }

    fn quiet(dialect: Dialect, list: &[&str]) {
        for sql in list {
            assert!(
                !calls_side_effect_function(sql, dialect),
                "{dialect:?}: {sql}"
            );
        }
    }

    /// The server strips `/*!`, its version number and the closing `*/` of an executable
    /// comment, so the name and the `(` around them are one call. (Checked on MySQL 9.6:
    /// `SELECT LENGTH/*!*/('abc')`, `SELECT /*!50000 LENGTH*/('abc')` and
    /// `SELECT /*!50000LENGTH*/('abc')` all return 3.)
    #[test]
    fn a_call_hidden_by_an_executable_comment_is_found() {
        flagged(
            Dialect::Mysql,
            &[
                "SELECT SLEEP/*!*/(100000)",
                "SELECT /*!50000 GET_LOCK*/('a', 10)",
                "SELECT /*!SLEEP*/(1)",
                "SELECT /*!50000SLEEP*/(1)",
                "SELECT /*!50000 SETVAL*/(s, 10)",
                "SELECT /*!50000 NEXTVAL*/(s)",
                "SELECT /*!50000 BENCHMARK*/ (1000, 1)",
                "SELECT SLEEP/*!50000*/(1)",
                "SELECT SLEEP /*!*/ /*!*/ (1)",
                "SELECT /*!SLEEP*//**/(1)",
                "SELECT /*!50000 SLEEP/**/(1) */ 1",
                "SELECT /*!50000 `sleep`*/(1)",
                "SELECT /*!SLEEP(1)*/ 1",
                // MariaDB's own comment and the optimizer hint.
                "SELECT /*M!100000 SLEEP*/(1)",
                "SELECT /*M!100100SLEEP*/(1)",
                "SELECT /*M!SLEEP*/(1)",
                "SELECT /*M!100000 SETVAL*/(s, 10)",
                "SELECT /*+ SLEEP*/(1)",
                // MariaDB's `NEXT VALUE FOR`, with a comment end in it.
                "SELECT NEXT /*!*/ VALUE FOR s",
                "SELECT /*!50000NEXT*/ VALUE FOR s",
                "SELECT NEXT VALUE/*!*/ FOR s",
            ],
        );
        // Not calls: the closing `*/` is only special inside an executable comment.
        quiet(
            Dialect::Mysql,
            &[
                "SELECT a */*SLEEP(1)*/ b FROM t",
                "SELECT 2 */* x */ 3, sleep FROM t",
                "SELECT sleep /*!*/ FROM t",
                "SELECT /*!50000 sleep*/ FROM t",
                "SELECT SLEEP_TIME/*!*/(1)",
                "SELECT /*!50000 1*/, sleeping(1)",
                "SELECT 1 /* GET_LOCK(1, 1) */",
            ],
        );
        // PostgreSQL and SQLite have no executable comments: `/*! … */` is a plain comment, so
        // the name in it is not called (the `(` that follows belongs to the value).
        for dialect in [Dialect::Postgres, Dialect::Sqlite] {
            quiet(
                dialect,
                &[
                    "SELECT /*!50000 SLEEP*/(1)",
                    "SELECT /*!SLEEP*/(1)",
                    "SELECT /*M!100000 SLEEP*/(1)",
                    "SELECT /*+ SLEEP*/(1)",
                ],
            );
        }
    }

    /// `Unicode-escaped identifiers` of PostgreSQL are function names too (checked on 17:
    /// `SELECT U&"\006cength"('abc')` and `SELECT U&"l!0065ngth" UESCAPE $$!$$('abc')` return 3).
    #[test]
    fn a_call_hidden_by_a_unicode_escaped_name_is_found() {
        flagged(
            Dialect::Postgres,
            &[
                r#"SELECT U&"set\0076al"('s', 1)"#,
                r#"SELECT u&"SET\0056AL"('s', 1)"#,
                r#"SELECT U&"pg_\+000074erminate_backend"(1)"#,
                r#"SELECT U&"pg_terminate_backend"(1)"#,
                r#"SELECT pg_catalog.U&"set\0076al"('s', 1)"#,
                r#"SELECT U&"nextval" ('s')"#,
                r#"SELECT * FROM U&"pg_s\006ceep"(100)"#,
                r#"SELECT U&"pg_advisory_l\006fck"(1)"#,
                // A name that is not well written is refused by the server: it counts as a call.
                r#"SELECT U&"set\zval"('s', 1)"#,
                r#"SELECT U&"set\00"('s', 1)"#,
                r#"SELECT U&"\D800"('s', 1)"#,
                // Another escape character: the name cannot be worked out here.
                r#"SELECT U&"set!0076al" UESCAPE '!'('s', 1)"#,
                r#"SELECT U&"set!0076al" UESCAPE $$!$$('s', 1)"#,
                r#"SELECT U&"set!0076al" UESCAPE $e$!$e$('s', 1)"#,
                r#"SELECT U&"set!0076al" UESCAPE E'!'('s', 1)"#,
                r#"SELECT U&"set!0076al" uescape '!' ('s', 1)"#,
                "SELECT U&\"set!0076al\" /* c */ UESCAPE -- d\n '!'\n''('s', 1)",
                r#"SELECT U&"set\0076al" UESCAPE '!'('s', 1)"#,
            ],
        );
        quiet(
            Dialect::Postgres,
            &[
                r#"SELECT U&"d\0061ta" FROM t"#,
                r#"SELECT U&"abs"(1), U&"l\0065ngth"('abc'), U&"\D83D\DE00"(1)"#,
                r#"SELECT U&'set\0076al(1)'"#,
                r#"SELECT U&"x" UESCAPE '!' FROM t"#,
                r#"SELECT U&"d\0061ta", count(U&"\0061")  FROM t"#,
                // An identifier that only ends in a `u`, or a `U` that is not followed by `&"`.
                r#"SELECT menu&"x"(1)"#,
                r#"SELECT U & "x" FROM t"#,
            ],
        );
    }

    #[test]
    fn unicode_escapes_are_decoded_like_the_server_does() {
        let decode = |text: &str| {
            let raw: Vec<char> = text.chars().collect();
            unicode_escaped_name(&raw).map(|name| name.into_iter().collect::<String>())
        };
        assert_eq!(decode("plain").as_deref(), Some("plain"));
        assert_eq!(decode(r"set\0076al").as_deref(), Some("setval"));
        assert_eq!(decode(r"\+000073etval").as_deref(), Some("setval"));
        assert_eq!(decode(r"a\\b").as_deref(), Some(r"a\b"));
        assert_eq!(decode(r"\0041\+000042").as_deref(), Some("AB"));
        assert_eq!(decode(r"\00e9").as_deref(), Some("é"));
        // A surrogate pair is one character.
        assert_eq!(decode(r"\D83D\DE00").as_deref(), Some("\u{1F600}"));
        assert_eq!(decode(r"\+01F600").as_deref(), Some("\u{1F600}"));
        // What the server refuses.
        for bad in [
            r"\",
            r"\z",
            r"\12",
            r"\00zz",
            r"\+0000",
            r"\+110000",
            r"\D83D",
            r"\D83Dx",
            r"\DE00",
            r"\D83D\0041",
        ] {
            assert_eq!(decode(bad), None, "{bad}");
        }
    }

    /// Where a name is quoted, SQLite's `[name]` is a name as well as `"name"` and `` `name` ``
    /// (checked in SQLite: `SELECT [length]('abc')` returns 3).
    #[test]
    fn a_sqlite_bracketed_function_name_is_found() {
        flagged(
            Dialect::Sqlite,
            &[
                "SELECT [load_extension]('x')",
                r#"SELECT "load_extension"('x')"#,
                "SELECT `load_extension`('x')",
                "SELECT [load_extension] ('x', 'y')",
                "SELECT 1 WHERE [LOAD_EXTENSION](1)",
            ],
        );
        quiet(
            Dialect::Sqlite,
            &[
                "SELECT [load_extension] FROM t",
                "SELECT [a b](1), [length]('x')",
                "SELECT 1 AS [load_extension]",
            ],
        );
    }

    /// The classifier reads the same text, so what it calls a read stays a read only when the
    /// server runs a read. A version number glued to the body of an executable comment used to
    /// make `50000INTO` one word: `SELECT 1 /*!50000INTO*/ OUTFILE '…'` was a READ although the
    /// server runs it as `SELECT … INTO OUTFILE` (MySQL 9.6 answers it with the
    /// `--secure-file-priv` error, not a syntax error).
    #[test]
    fn a_version_number_does_not_hide_the_words_of_an_executable_comment() {
        for sql in [
            "SELECT 1 /*!50000INTO*/ OUTFILE '/tmp/x'",
            "SELECT 1 /*!50000 INTO*/ OUTFILE '/tmp/x'",
            "SELECT 1 /*M!100100INTO*/ DUMPFILE '/tmp/x'",
            "WITH x AS (SELECT 1) /*!50000DELETE*/ FROM t",
            "WITH x AS (SELECT 1) /*!50000UPDATE*/ t SET a = 1",
            "/*!50000DELETE*/ FROM t",
            "/*!40101 SET NAMES utf8 */",
        ] {
            assert_eq!(
                classify(sql, Dialect::Mysql),
                StatementClass::Write,
                "{sql}"
            );
        }
        // The end of an executable comment is not code: the server runs these as a plain
        // `SELECT 1` (they used to be a WRITE, read as `* / SELECT 1`).
        for sql in [
            "/*!*/ SELECT 1",
            "/*!*/SELECT 1",
            "SELECT/*!*/ 1",
            "SELECT 1 /*!50000 FROM t*/",
            "SELECT 1 /*+ MAX_EXECUTION_TIME(1000) */",
        ] {
            assert_eq!(classify(sql, Dialect::Mysql), StatementClass::Read, "{sql}");
        }
    }

    /// `set_config` is found under a Unicode-escaped name too: a statement that calls it is a
    /// WRITE, as is a call of a name that cannot be worked out.
    #[test]
    fn a_unicode_escaped_set_config_is_a_write() {
        for sql in [
            r#"SELECT U&"set\005fconfig"('default_transaction_read_only', 'off', false)"#,
            r#"SELECT U&"set!005fconfig" UESCAPE '!'('a', 'b', false)"#,
            r#"SELECT pg_catalog.U&"\+000073et_config"('a', 'b', false)"#,
            r#"SELECT U&"set\zconfig"('a', 'b', false)"#,
        ] {
            assert_eq!(
                classify(sql, Dialect::Postgres),
                StatementClass::Write,
                "{sql}"
            );
        }
        for sql in [
            r#"SELECT U&"d\0061ta" FROM t"#,
            r#"SELECT U&"l\0065ngth"('abc')"#,
            r#"SELECT U&"x" UESCAPE '!' FROM t"#,
        ] {
            assert_eq!(
                classify(sql, Dialect::Postgres),
                StatementClass::Read,
                "{sql}"
            );
        }
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

    /// PostgreSQL ends a `--` comment at a lone CR as well as at LF; MySQL and SQLite read to the
    /// LF. The server runs what follows the CR, so the classifier must see it as code.
    #[test]
    fn a_postgres_line_comment_ends_at_a_carriage_return() {
        flagged(
            Dialect::Postgres,
            &[
                "SELECT 1 --x\r, pg_sleep(100)",
                "SELECT 1 --x\r; SELECT pg_sleep(100)",
                "SELECT U&\"setval\" --UESCAPE\r ('s', 1)(s, 1)",
            ],
        );
        assert_eq!(
            classify("SELECT 1 --x\r; DELETE FROM t", Dialect::Postgres),
            StatementClass::Write
        );
        assert_eq!(
            classify(
                "WITH x AS (-- c\r DELETE FROM t RETURNING 1) SELECT count(*) FROM x",
                Dialect::Postgres
            ),
            StatementClass::Write
        );
        // MySQL and SQLite do not end the comment at a CR: the rest is still the comment.
        for dialect in [Dialect::Mysql, Dialect::Sqlite] {
            quiet(
                dialect,
                &[
                    "SELECT 1 -- x\r, pg_sleep(100)",
                    "SELECT 1 -- x\r, sleep(100)",
                ],
            );
            assert_eq!(
                classify("SELECT 1 -- x\r; DELETE FROM t", dialect),
                StatementClass::Read,
                "{dialect:?}"
            );
        }
        // A plain LF still ends it, and a comment with no line end hides the rest.
        quiet(Dialect::Postgres, &["SELECT 1 --x, pg_sleep(100)"]);
    }

    /// PostgreSQL reads every non-ASCII character as part of a name, so `€$$` is one identifier and
    /// opens no dollar quote: the code between that and the next `$$` is code.
    #[test]
    fn a_non_ascii_name_does_not_open_a_dollar_quote() {
        flagged(
            Dialect::Postgres,
            &[
                "SELECT 1 AS €$$, pg_sleep(100) AS a$$",
                "SELECT 1 AS —$$, setval('s', 1) AS a$$",
                "SELECT 1 AS 😀$$, pg_sleep(100) AS a$$",
            ],
        );
        assert_eq!(
            classify(
                "SELECT 1 AS €$$; DELETE FROM users; SELECT 3 AS a$$",
                Dialect::Postgres
            ),
            StatementClass::Write
        );
        // A real dollar-quoted string still hides what it quotes.
        quiet(
            Dialect::Postgres,
            &["SELECT $$setval(1)$$", "SELECT 'x' AS é, $$pg_sleep(100)$$"],
        );
        assert_eq!(
            classify("SELECT $$; DELETE FROM t; $$", Dialect::Postgres),
            StatementClass::Read
        );
        // `€e'…'` is a name and a plain string, not an escape string: the backslash does not
        // escape the quote, so what follows the string is code.
        flagged(Dialect::Postgres, &["SELECT €e'\\', pg_sleep(100), '"]);
    }

    #[test]
    fn more_postgres_server_side_effect_functions_are_found() {
        flagged(
            Dialect::Postgres,
            &[
                "SELECT pg_logical_slot_get_changes('s', NULL, NULL)",
                "SELECT pg_logical_slot_get_binary_changes('s', NULL, NULL)",
                "SELECT pg_copy_logical_replication_slot('a', 'b')",
                "SELECT pg_copy_physical_replication_slot('a', 'b')",
                "SELECT pg_replication_origin_create('o')",
                "SELECT pg_replication_origin_drop('o')",
                "SELECT pg_replication_origin_advance('o', '0/1')",
                "SELECT pg_stat_statements_reset()",
                "SELECT pg_log_backend_memory_contexts(1)",
                "SELECT dblink_connect_u('c', 'dbname=x')",
            ],
        );
    }
}
