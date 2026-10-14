//! Client-side statement splitter for the SQL editor.
//!
//! A single pass over the characters of the editor text with these states: normal text,
//! `'…'` strings (`''` escapes; backslash escapes in MySQL and in PostgreSQL `E'…'`), `"…"`
//! (identifiers; strings with backslash escapes in MySQL), `` `…` `` (MySQL / SQLite), `[…]`
//! (SQLite), `--` line comments (MySQL: only when followed by whitespace), `#` line comments
//! (MySQL), `/* … */` block comments (nested in PostgreSQL), `$tag$ … $tag$` dollar quoting
//! (PostgreSQL), a `BEGIN … END` depth counter inside `CREATE TRIGGER / FUNCTION / PROCEDURE /
//! EVENT` statements (so routine bodies keep their inner `;`), the MySQL client directive
//! `DELIMITER xx` (changes the terminator; the directive line itself is dropped), psql
//! meta-command lines (`\d users`), which become statements of their own so the caller can
//! reject them, and (PostgreSQL, like psql) parenthesis depth: a `;` inside parentheses does not
//! end the statement (`CREATE RULE … DO ALSO (INSERT …; INSERT …)`).
//!
//! Offsets are **character** offsets (not bytes) into the submitted text, which is what the
//! editor works with. Empty and comment-only statements are skipped; leading comments are not
//! part of a statement (except MySQL `/*! … */` executable comments, which are code).

use super::Dialect;

/// One statement of the editor text. `start..end` are char offsets of `sql` in the input
/// (terminator excluded); `line` is the 1-based line `start` is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitStatement {
    pub sql: String,
    pub start: usize,
    pub end: usize,
    pub line: u32,
}

/// Splits editor text into statements.
pub fn split_statements(sql: &str, dialect: Dialect) -> Vec<SplitStatement> {
    let mut s = Splitter::new(sql, dialect);
    s.run();
    s.out
}

/// Whether `sql` (one statement as the splitter returned it) holds at most one statement when
/// split again with the default `;` terminator, under either reading of backslashes in MySQL
/// strings (`NO_BACKSLASH_ESCAPES` / `ANSI_QUOTES` end a string at `\'`). A custom-`DELIMITER`
/// block such as `SELECT …; INSERT …` is one splitter statement but several server statements.
pub fn is_single_statement(sql: &str, dialect: Dialect) -> bool {
    let count = |backslash: bool| {
        let mut s = Splitter::new(sql, dialect);
        s.backslash_escapes = backslash;
        s.run();
        s.out.len()
    };
    count(dialect == Dialect::Mysql) <= 1 && (dialect != Dialect::Mysql || count(false) <= 1)
}

/// `sql` with string literals, quoted identifiers, dollar-quoted bodies and comments replaced by
/// a space, so keyword checks only see code (`… where action = 'UPDATE'` holds no `UPDATE`).
/// MySQL executable comments (`/*! … */`) and optimizer hints are code and are kept.
pub fn code_only(sql: &str, dialect: Dialect) -> String {
    let s = Splitter::new(sql, dialect);
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < s.n() {
        let c = s.chars[i];
        let next = s.at(i + 1);
        let skipped = match c {
            '\'' => Some(s.skip_quoted(i, '\'', s.backslash_escapes)),
            '"' => Some(s.skip_quoted(i, '"', s.backslash_escapes)),
            '`' if dialect != Dialect::Postgres => Some(s.skip_quoted(i, '`', false)),
            '[' if dialect == Dialect::Sqlite => Some(s.skip_until(i + 1, ']')),
            '-' if next == Some('-') && s.dash_comment_ok(i) => Some(s.skip_comment_line(i)),
            '#' if s.mysql() => Some(s.skip_comment_line(i)),
            '/' if next == Some('*') => {
                if s.mysql() && matches!(s.at(i + 2), Some('!') | Some('+')) {
                    // Code: keep scanning inside it.
                    out.push_str("   ");
                    i += 3;
                    continue;
                }
                Some(s.skip_block_comment(i))
            }
            '$' if s.pg() => s.dollar_tag(i).map(|len| s.skip_dollar(i, len)),
            c if is_ident_start(c) => {
                let mut j = i;
                while j < s.n() && is_word_char(s.chars[j]) {
                    j += 1;
                }
                out.extend(&s.chars[i..j]);
                // PostgreSQL escape strings: E'…' with backslash escapes.
                if s.pg() && j == i + 1 && c.eq_ignore_ascii_case(&'e') && s.at(j) == Some('\'') {
                    out.push(' ');
                    i = s.skip_quoted(j, '\'', true);
                } else {
                    i = j;
                }
                continue;
            }
            _ => None,
        };
        match skipped {
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

/// The first `n` keywords of a statement, upper-cased, skipping leading whitespace, comments and
/// opening parentheses (`(SELECT …)`). Stops at the first token that is not a plain word.
pub fn leading_keywords(sql: &str, n: usize) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() && out.len() < n {
        let c = chars[i];
        if c.is_whitespace() || (c == '(' && out.is_empty()) {
            i += 1;
        } else if (c == '-' && chars.get(i + 1) == Some(&'-')) || (c == '#' && out.is_empty()) {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
        } else if is_ident_start(c) {
            let start = i;
            while i < chars.len() && is_word_char(chars[i]) {
                i += 1;
            }
            out.push(
                chars[start..i]
                    .iter()
                    .collect::<String>()
                    .to_ascii_uppercase(),
            );
        } else {
            break;
        }
    }
    out
}

/// `true` for a psql meta-command such as `\d users` (the splitter emits them as statements).
pub fn is_meta_command(sql: &str) -> bool {
    sql.trim_start().starts_with('\\')
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

struct Splitter {
    chars: Vec<char>,
    dialect: Dialect,
    out: Vec<SplitStatement>,
    /// First char of real (non-comment) content in the current statement.
    content_start: Option<usize>,
    delimiter: Vec<char>,
    /// First words of the current statement (upper-cased) plus `(`, for routine detection.
    head: Vec<String>,
    /// `Some(true)` once the statement is known to be `CREATE … TRIGGER|FUNCTION|PROCEDURE|EVENT`.
    routine: Option<bool>,
    depth: u32,
    /// Open parentheses in the current statement (PostgreSQL only).
    parens: u32,
    /// Line counting cache: `line_count` newlines lie strictly before `line_pos`.
    line_pos: usize,
    line_count: u32,
    /// Backslash escapes the next character in `'…'` / `"…"` (MySQL's default `sql_mode`).
    backslash_escapes: bool,
}

const HEAD_MAX: usize = 16;

impl Splitter {
    fn new(sql: &str, dialect: Dialect) -> Splitter {
        Splitter {
            chars: sql.chars().collect(),
            dialect,
            out: Vec::new(),
            content_start: None,
            delimiter: vec![';'],
            head: Vec::new(),
            routine: None,
            depth: 0,
            parens: 0,
            line_pos: 0,
            line_count: 0,
            backslash_escapes: dialect == Dialect::Mysql,
        }
    }

    fn n(&self) -> usize {
        self.chars.len()
    }

    fn at(&self, i: usize) -> Option<char> {
        self.chars.get(i).copied()
    }

    fn mysql(&self) -> bool {
        self.dialect == Dialect::Mysql
    }

    fn pg(&self) -> bool {
        self.dialect == Dialect::Postgres
    }

    fn default_delimiter(&self) -> bool {
        self.delimiter.len() == 1 && self.delimiter[0] == ';'
    }

    fn mark_content(&mut self, i: usize) {
        if self.content_start.is_none() {
            self.content_start = Some(i);
        }
    }

    fn at_delimiter(&self, i: usize) -> bool {
        let d = &self.delimiter;
        if i + d.len() > self.n() {
            return false;
        }
        if self.chars[i..i + d.len()] != d[..] {
            return false;
        }
        // psql does not end a statement inside parentheses either.
        if self.pg() && self.parens > 0 {
            return false;
        }
        // Inside a routine body `;` separates the body's statements.
        !(self.default_delimiter() && self.routine == Some(true) && self.depth > 0)
    }

    fn line_of(&mut self, pos: usize) -> u32 {
        if pos < self.line_pos {
            self.line_pos = 0;
            self.line_count = 0;
        }
        for i in self.line_pos..pos {
            if self.chars[i] == '\n' {
                self.line_count += 1;
            }
        }
        self.line_pos = pos;
        self.line_count + 1
    }

    /// Emits `[content_start, to)` (trimmed) as a statement and starts a new one.
    fn finish(&mut self, to: usize) {
        if let Some(from) = self.content_start {
            let mut end = to;
            while end > from && self.chars[end - 1].is_whitespace() {
                end -= 1;
            }
            if end > from {
                let line = self.line_of(from);
                self.out.push(SplitStatement {
                    sql: self.chars[from..end].iter().collect(),
                    start: from,
                    end,
                    line,
                });
            }
        }
        self.content_start = None;
        self.head.clear();
        self.routine = None;
        self.depth = 0;
        self.parens = 0;
    }

    fn run(&mut self) {
        let mut i = 0;
        while i < self.n() {
            if self.at_delimiter(i) {
                self.finish(i);
                i += self.delimiter.len();
                continue;
            }
            let c = self.chars[i];
            let next = self.at(i + 1);
            i = match c {
                '\'' => {
                    self.mark_content(i);
                    self.skip_quoted(i, '\'', self.backslash_escapes)
                }
                '"' => {
                    self.mark_content(i);
                    self.skip_quoted(i, '"', self.backslash_escapes)
                }
                '`' if self.dialect != Dialect::Postgres => {
                    self.mark_content(i);
                    self.skip_quoted(i, '`', false)
                }
                '[' if self.dialect == Dialect::Sqlite => {
                    self.mark_content(i);
                    self.skip_until(i + 1, ']')
                }
                '-' if next == Some('-') && self.dash_comment_ok(i) => self.skip_comment_line(i),
                '#' if self.mysql() => self.skip_comment_line(i),
                '/' if next == Some('*') => {
                    // MySQL executable comments (`/*! … */`) and optimizer hints are code.
                    if self.mysql() && matches!(self.at(i + 2), Some('!') | Some('+')) {
                        self.mark_content(i);
                    }
                    self.skip_block_comment(i)
                }
                '$' if self.pg() => match self.dollar_tag(i) {
                    Some(tag_len) => {
                        self.mark_content(i);
                        self.skip_dollar(i, tag_len)
                    }
                    None => {
                        self.mark_content(i);
                        i + 1
                    }
                },
                '\\' if self.pg() && self.at_line_start(i) => self.meta_command(i),
                c if is_ident_start(c) => self.word(i),
                c if c.is_whitespace() => i + 1,
                '(' => {
                    self.mark_content(i);
                    if self.pg() {
                        self.parens += 1;
                    }
                    if self.head.len() < HEAD_MAX {
                        self.head.push("(".into());
                    }
                    self.decide_routine();
                    i + 1
                }
                ')' => {
                    self.mark_content(i);
                    self.parens = self.parens.saturating_sub(1);
                    i + 1
                }
                _ => {
                    self.mark_content(i);
                    i + 1
                }
            };
        }
        self.finish(self.n());
    }

    /// MySQL only treats `--` as a comment when a space or control character follows.
    fn dash_comment_ok(&self, i: usize) -> bool {
        if !self.mysql() {
            return true;
        }
        match self.at(i + 2) {
            None => true,
            Some(c) => c.is_whitespace() || c.is_control(),
        }
    }

    fn skip_line(&self, mut i: usize) -> usize {
        while i < self.n() && self.chars[i] != '\n' {
            i += 1;
        }
        i
    }

    /// End of the line comment that starts at `i`. PostgreSQL ends one at a lone CR as well as at
    /// LF (`SELECT 1 --x<CR>; DELETE …` runs the DELETE); MySQL and SQLite read to the LF.
    fn skip_comment_line(&self, mut i: usize) -> usize {
        let cr_ends = self.dialect == Dialect::Postgres;
        while i < self.n() && self.chars[i] != '\n' && !(cr_ends && self.chars[i] == '\r') {
            i += 1;
        }
        i
    }

    fn skip_until(&self, mut i: usize, close: char) -> usize {
        while i < self.n() {
            if self.chars[i] == close {
                return i + 1;
            }
            i += 1;
        }
        self.n()
    }

    /// Skips a quoted run starting at `i` (the opening quote); doubled quotes escape, and so do
    /// backslashes when `backslash` is set. Returns the index after the closing quote.
    fn skip_quoted(&self, i: usize, quote: char, backslash: bool) -> usize {
        let mut j = i + 1;
        while j < self.n() {
            let c = self.chars[j];
            if backslash && c == '\\' {
                j += 2;
                continue;
            }
            if c == quote {
                if self.at(j + 1) == Some(quote) {
                    j += 2;
                    continue;
                }
                return j + 1;
            }
            j += 1;
        }
        self.n()
    }

    fn skip_block_comment(&self, i: usize) -> usize {
        let nested = self.pg();
        let mut depth = 1;
        let mut j = i + 2;
        while j < self.n() {
            let c = self.chars[j];
            let next = self.at(j + 1);
            if nested && c == '/' && next == Some('*') {
                depth += 1;
                j += 2;
            } else if c == '*' && next == Some('/') {
                depth -= 1;
                j += 2;
                if depth == 0 {
                    return j;
                }
            } else {
                j += 1;
            }
        }
        self.n()
    }

    /// Length of a `$tag$` opener at `i` (including both `$`), if there is one.
    fn dollar_tag(&self, i: usize) -> Option<usize> {
        // `$` inside an identifier (`a$b`) or a positional parameter (`$1`) is not a quote.
        if i > 0 && is_word_char(self.chars[i - 1]) {
            return None;
        }
        let mut j = i + 1;
        match self.at(j) {
            Some('$') => return Some(2),
            Some(c) if is_ident_start(c) => j += 1,
            _ => return None,
        }
        while let Some(c) = self.at(j) {
            if c == '$' {
                return Some(j + 1 - i);
            }
            if !(c.is_alphanumeric() || c == '_') {
                return None;
            }
            j += 1;
        }
        None
    }

    fn skip_dollar(&self, i: usize, tag_len: usize) -> usize {
        let tag = &self.chars[i..i + tag_len];
        let mut j = i + tag_len;
        while j + tag_len <= self.n() {
            if self.chars[j..j + tag_len] == *tag {
                return j + tag_len;
            }
            j += 1;
        }
        self.n()
    }

    fn at_line_start(&self, i: usize) -> bool {
        let mut j = i;
        while j > 0 {
            match self.chars[j - 1] {
                '\n' => return true,
                ' ' | '\t' | '\r' => j -= 1,
                _ => return false,
            }
        }
        true
    }

    /// A psql meta-command: ends the current statement and becomes a statement of its own.
    fn meta_command(&mut self, i: usize) -> usize {
        self.finish(i);
        let eol = self.skip_line(i);
        self.mark_content(i);
        self.finish(eol);
        eol
    }

    /// Upper-cased word starting at the first non-whitespace char at or after `i`, plus the
    /// index after it.
    fn peek_word(&self, mut i: usize) -> Option<(String, usize)> {
        while i < self.n() && self.chars[i].is_whitespace() {
            i += 1;
        }
        if i >= self.n() || !is_ident_start(self.chars[i]) {
            return None;
        }
        let start = i;
        while i < self.n() && is_word_char(self.chars[i]) {
            i += 1;
        }
        Some((
            self.chars[start..i]
                .iter()
                .collect::<String>()
                .to_ascii_uppercase(),
            i,
        ))
    }

    fn word(&mut self, i: usize) -> usize {
        let custom = !self.default_delimiter();
        let mut j = i;
        while j < self.n() && is_word_char(self.chars[j]) {
            // `END$$` with `DELIMITER $$`: the delimiter ends the word.
            if custom && self.chars[j..].starts_with(&self.delimiter) {
                break;
            }
            j += 1;
        }
        let word: String = self.chars[i..j]
            .iter()
            .collect::<String>()
            .to_ascii_uppercase();

        // PostgreSQL escape strings: E'…' with backslash escapes.
        if self.pg() && word == "E" && self.at(j) == Some('\'') {
            self.mark_content(i);
            return self.skip_quoted(j, '\'', true);
        }

        let first = self.content_start.is_none();
        if first
            && self.mysql()
            && word == "DELIMITER"
            && let Some(end) = self.delimiter_directive(j)
        {
            return end;
        }

        self.mark_content(i);
        let qualified = i > 0 && self.chars[i - 1] == '.';
        if self.head.len() < HEAD_MAX {
            self.head.push(word.clone());
        }
        if self.routine.is_none() {
            // MariaDB anonymous compound statement: `BEGIN NOT ATOMIC … END`.
            if first
                && self.mysql()
                && word == "BEGIN"
                && let Some((w1, k)) = self.peek_word(j)
                && w1 == "NOT"
                && let Some((w2, _)) = self.peek_word(k)
                && w2 == "ATOMIC"
            {
                self.routine = Some(true);
            }
            self.decide_routine();
        }
        if qualified || self.routine != Some(true) || !self.default_delimiter() {
            return j;
        }
        match word.as_str() {
            "BEGIN" => {
                let next = self.peek_word(j).map(|(w, _)| w);
                let opens = if self.pg() {
                    next.as_deref() == Some("ATOMIC")
                } else {
                    !matches!(
                        next.as_deref(),
                        Some("TRANSACTION" | "WORK" | "DEFERRED" | "IMMEDIATE" | "EXCLUSIVE")
                    ) && !self.next_is_delimiter(j)
                };
                if opens {
                    self.depth += 1;
                }
                j
            }
            "CASE" => {
                self.depth += 1;
                j
            }
            "END" => match self.peek_word(j) {
                Some((w, k))
                    if matches!(w.as_str(), "IF" | "LOOP" | "WHILE" | "REPEAT" | "FOR") =>
                {
                    k
                }
                Some((w, k)) if w == "CASE" => {
                    self.depth = self.depth.saturating_sub(1);
                    k
                }
                _ => {
                    self.depth = self.depth.saturating_sub(1);
                    j
                }
            },
            _ => j,
        }
    }

    fn next_is_delimiter(&self, mut i: usize) -> bool {
        while i < self.n() && self.chars[i].is_whitespace() {
            i += 1;
        }
        i >= self.n() || self.chars[i..].starts_with(&self.delimiter)
    }

    /// Handles `DELIMITER xx` (the word ends at `j`). Returns the index after the directive line,
    /// or `None` when this is not a directive.
    fn delimiter_directive(&mut self, j: usize) -> Option<usize> {
        match self.at(j) {
            Some(' ') | Some('\t') => {}
            _ => return None,
        }
        let eol = self.skip_line(j);
        let rest: String = self.chars[j..eol].iter().collect();
        if let Some(tok) = rest.split_whitespace().next() {
            self.delimiter = tok.chars().collect();
        }
        // The directive is a client command: it never becomes part of a statement.
        Some(eol)
    }

    fn decide_routine(&mut self) {
        if self.routine.is_some() || self.head.is_empty() {
            return;
        }
        if self.head[0] != "CREATE" {
            self.routine = Some(false);
            return;
        }
        for tok in &self.head[1..] {
            match tok.as_str() {
                "TRIGGER" | "FUNCTION" | "PROCEDURE" | "EVENT" => {
                    self.routine = Some(true);
                    return;
                }
                "TABLE" | "VIEW" | "INDEX" | "(" | "SCHEMA" | "DATABASE" | "SEQUENCE" | "TYPE"
                | "DOMAIN" | "MATERIALIZED" | "ROLE" | "USER" | "EXTENSION" | "AS" => {
                    self.routine = Some(false);
                    return;
                }
                _ => {}
            }
        }
        if self.head.len() >= HEAD_MAX {
            self.routine = Some(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(sql: &str, dialect: Dialect) -> Vec<String> {
        let out = split_statements(sql, dialect);
        // Invariant: every span maps back onto the input text.
        for st in &out {
            let slice: String = sql.chars().skip(st.start).take(st.end - st.start).collect();
            assert_eq!(slice, st.sql, "span mismatch for {:?}", st);
            let line = sql.chars().take(st.start).filter(|c| *c == '\n').count() as u32 + 1;
            assert_eq!(line, st.line, "line mismatch for {:?}", st);
        }
        out.into_iter().map(|s| s.sql).collect()
    }

    const ALL: [Dialect; 3] = [Dialect::Postgres, Dialect::Mysql, Dialect::Sqlite];

    #[test]
    fn postgres_keeps_semicolons_inside_parentheses() {
        let sql = "create rule r as on insert to t do also (insert into a values (1); insert into b values (2)); select 1";
        assert_eq!(
            texts(sql, Dialect::Postgres),
            [
                "create rule r as on insert to t do also (insert into a values (1); insert into b values (2))",
                "select 1"
            ]
        );
        // Parentheses inside strings, comments and dollar quotes do not count; a stray `)`
        // does not swallow the next statement.
        let sql = "select '(' as a; select $$($$ /* ( */; select 1); select 2 -- (\n;select 3";
        assert_eq!(
            texts(sql, Dialect::Postgres),
            [
                "select '(' as a",
                "select $$($$ /* ( */",
                "select 1)",
                "select 2 -- (",
                "select 3"
            ]
        );
        // Other dialects keep splitting at every `;` (MySQL / SQLite have no such statements).
        assert_eq!(texts("select (1; select 2)", Dialect::Mysql).len(), 2);
    }

    #[test]
    fn splits_simple_statements_with_offsets() {
        let sql = "select 1;\nselect 2 ;  \n\n  update users set name = name where false;";
        let out = split_statements(sql, Dialect::Postgres);
        assert_eq!(out.len(), 3);
        assert_eq!(
            out[0],
            SplitStatement {
                sql: "select 1".into(),
                start: 0,
                end: 8,
                line: 1
            }
        );
        assert_eq!(
            out[1],
            SplitStatement {
                sql: "select 2".into(),
                start: 10,
                end: 18,
                line: 2
            }
        );
        assert_eq!(out[2].sql, "update users set name = name where false");
        assert_eq!(out[2].line, 4);
        assert_eq!(out[2].start, 26);
        for d in ALL {
            assert_eq!(texts(sql, d).len(), 3);
        }
    }

    #[test]
    fn skips_empty_and_comment_only_statements() {
        for d in ALL {
            assert!(texts("", d).is_empty());
            assert!(texts(" ;; ;\n", d).is_empty());
            assert!(texts("-- just a comment\n", d).is_empty());
            assert!(texts("/* block */ ; -- trailing", d).is_empty());
            assert_eq!(texts("select 1; -- done", d), ["select 1"]);
            assert_eq!(texts("select 1 -- note\n;", d), ["select 1 -- note"]);
        }
    }

    #[test]
    fn leading_comments_are_not_part_of_the_statement() {
        let sql = "-- first\n/* second */\nselect 1;";
        let out = split_statements(sql, Dialect::Postgres);
        assert_eq!(out[0].sql, "select 1");
        assert_eq!(out[0].line, 3);
        assert_eq!(out[0].start, 22);
        // …but inner comments are.
        assert_eq!(
            texts("select /* ; */ 1 -- x;\n + 2;", Dialect::Sqlite),
            ["select /* ; */ 1 -- x;\n + 2"]
        );
    }

    #[test]
    fn respects_quotes() {
        for d in ALL {
            assert_eq!(
                texts("select 'a;b', 'it''s;'; select 2", d),
                ["select 'a;b', 'it''s;'", "select 2"]
            );
            assert_eq!(
                texts(r#"select "x;y" from t; select 3"#, d),
                [r#"select "x;y" from t"#, "select 3"]
            );
        }
        assert_eq!(
            texts("select `a;b` from t; select 1", Dialect::Mysql),
            ["select `a;b` from t", "select 1"]
        );
        assert_eq!(
            texts("select [a;b] from t; select 1", Dialect::Sqlite),
            ["select [a;b] from t", "select 1"]
        );
        assert_eq!(
            texts("select `a;b` from t; select 1", Dialect::Sqlite),
            ["select `a;b` from t", "select 1"]
        );
    }

    #[test]
    fn backslash_escapes_by_dialect() {
        // MySQL: backslash escapes the quote.
        assert_eq!(
            texts(r"select 'a\';b'; select 2", Dialect::Mysql),
            [r"select 'a\';b'", "select 2"]
        );
        assert_eq!(
            texts(r#"select "a\";b"; select 2"#, Dialect::Mysql),
            [r#"select "a\";b""#, "select 2"]
        );
        // PostgreSQL standard strings: backslash is literal, so the string ends after `\`.
        assert_eq!(
            texts(r"select 'a\'; select 2", Dialect::Postgres),
            [r"select 'a\'", "select 2"]
        );
        // …but E'…' strings use backslash escapes.
        assert_eq!(
            texts(r"select E'a\';b'; select 2", Dialect::Postgres),
            [r"select E'a\';b'", "select 2"]
        );
        assert_eq!(
            texts(r"select e'\\'; select 2", Dialect::Postgres),
            [r"select e'\\'", "select 2"]
        );
        // `type'…'` is not an E-string.
        assert_eq!(
            texts(r"select date'2020-01-01'; select 2", Dialect::Postgres),
            [r"select date'2020-01-01'", "select 2"]
        );
    }

    #[test]
    fn comments() {
        for d in ALL {
            assert_eq!(
                texts("select 1 /* ; */; select 2 -- ;\n; select 3", d),
                ["select 1 /* ; */", "select 2 -- ;", "select 3"]
            );
        }
        // Nested block comments in PostgreSQL only.
        assert_eq!(
            texts("/* a /* b */ ; */ select 1; select 2", Dialect::Postgres),
            ["select 1", "select 2"]
        );
        assert_eq!(
            texts("/* a /* b */ select 1; select 2", Dialect::Mysql),
            ["select 1", "select 2"]
        );
        // MySQL `#` comments, and `--` needs a following space.
        assert_eq!(
            texts("select 1 # ; x\n; select 2", Dialect::Mysql),
            ["select 1 # ; x", "select 2"]
        );
        assert_eq!(
            texts("select 1--1; select 2", Dialect::Mysql),
            ["select 1--1", "select 2"]
        );
        assert_eq!(
            texts("select 1--1; select 2", Dialect::Postgres),
            ["select 1--1; select 2"]
        );
        // MySQL executable comments are code.
        assert_eq!(
            texts("/*!40101 SET NAMES utf8 */;\nselect 1;", Dialect::Mysql),
            ["/*!40101 SET NAMES utf8 */", "select 1"]
        );
    }

    #[test]
    fn dollar_quoting() {
        let sql = "create function f() returns int as $$ begin; return 1; end; $$ language plpgsql;\nselect f();";
        assert_eq!(
            texts(sql, Dialect::Postgres),
            [
                "create function f() returns int as $$ begin; return 1; end; $$ language plpgsql",
                "select f()"
            ]
        );
        let sql =
            "do $body$ declare x text := '$$;'; begin end $body$; select $1, a$b from t; select 2";
        assert_eq!(
            texts(sql, Dialect::Postgres),
            [
                "do $body$ declare x text := '$$;'; begin end $body$",
                "select $1, a$b from t",
                "select 2"
            ]
        );
        // Unterminated dollar quote swallows the rest.
        assert_eq!(
            texts("select $x$ ; abc", Dialect::Postgres),
            ["select $x$ ; abc"]
        );
    }

    #[test]
    fn begin_end_blocks_in_routines() {
        let trigger = "CREATE TRIGGER trg AFTER INSERT ON t BEGIN\n  UPDATE t SET a = CASE WHEN new.a > 0 THEN 1 ELSE 0 END;\n  INSERT INTO log VALUES (1);\nEND;\nselect 1;";
        assert_eq!(
            texts(trigger, Dialect::Sqlite),
            [
                "CREATE TRIGGER trg AFTER INSERT ON t BEGIN\n  UPDATE t SET a = CASE WHEN new.a > 0 THEN 1 ELSE 0 END;\n  INSERT INTO log VALUES (1);\nEND",
                "select 1"
            ]
        );
        assert_eq!(
            texts(
                "create temp trigger if not exists x before delete on t begin select raise(abort, 'no'); end; select 2",
                Dialect::Sqlite
            ),
            [
                "create temp trigger if not exists x before delete on t begin select raise(abort, 'no'); end",
                "select 2"
            ]
        );
        let proc = "CREATE DEFINER=`root`@`localhost` PROCEDURE p(IN n INT)\nBEGIN\n  DECLARE i INT DEFAULT 0;\n  IF n > 0 THEN SELECT 1; END IF;\n  WHILE i < n DO SET i = i + 1; END WHILE;\n  CASE n WHEN 1 THEN SELECT 'one'; ELSE SELECT 'many'; END CASE;\n  lbl: BEGIN SELECT 2; END lbl;\n  REPEAT SET i = i - 1; UNTIL i = 0 END REPEAT;\nEND;\nCALL p(3);";
        let out = texts(proc, Dialect::Mysql);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out[0].ends_with("END REPEAT;\nEND"));
        assert_eq!(out[1], "CALL p(3)");

        // Transactions and CASE outside routines are not blocks.
        assert_eq!(
            texts(
                "BEGIN; select case when 1 then 2 end; COMMIT;",
                Dialect::Sqlite
            ),
            ["BEGIN", "select case when 1 then 2 end", "COMMIT"]
        );
        assert_eq!(
            texts("BEGIN TRANSACTION; select 1; END;", Dialect::Sqlite),
            ["BEGIN TRANSACTION", "select 1", "END"]
        );
        // A table whose name looks like a keyword does not start routine mode.
        assert_eq!(
            texts(
                "create table t (\"begin\" int); select 1",
                Dialect::Postgres
            ),
            ["create table t (\"begin\" int)", "select 1"]
        );
        // PostgreSQL SQL-standard function bodies.
        let pg = "CREATE FUNCTION add(a int, b int) RETURNS int LANGUAGE sql\nBEGIN ATOMIC\n  SELECT a + b;\nEND;\nselect add(1, 2);";
        assert_eq!(
            texts(pg, Dialect::Postgres),
            [
                "CREATE FUNCTION add(a int, b int) RETURNS int LANGUAGE sql\nBEGIN ATOMIC\n  SELECT a + b;\nEND",
                "select add(1, 2)"
            ]
        );
        assert_eq!(
            texts(
                "create function f(x int) returns int language sql return case when x > 0 then 1 else 0 end; select 1",
                Dialect::Postgres
            ),
            [
                "create function f(x int) returns int language sql return case when x > 0 then 1 else 0 end",
                "select 1"
            ]
        );
        // MariaDB anonymous blocks.
        assert_eq!(
            texts(
                "BEGIN NOT ATOMIC SELECT 1; SELECT 2; END; SELECT 3",
                Dialect::Mysql
            ),
            ["BEGIN NOT ATOMIC SELECT 1; SELECT 2; END", "SELECT 3"]
        );
    }

    #[test]
    fn mysql_delimiter_directive() {
        let sql = "DELIMITER $$\nCREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\n  SELECT 2;\nEND$$\nDELIMITER ;\nCALL p();\nselect 'x';";
        let out = split_statements(sql, Dialect::Mysql);
        let got: Vec<_> = out.iter().map(|s| s.sql.as_str()).collect();
        assert_eq!(
            got,
            [
                "CREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\n  SELECT 2;\nEND",
                "CALL p()",
                "select 'x'"
            ]
        );
        assert_eq!(out[0].line, 2);
        assert_eq!(out[1].line, 8);
        assert_eq!(out[2].line, 9);

        assert_eq!(
            texts(
                "delimiter //\nselect 1; select 2//\nselect 3 //",
                Dialect::Mysql
            ),
            ["select 1; select 2", "select 3"]
        );
        // Not a directive in other dialects or mid-statement.
        assert_eq!(
            texts("select delimiter from t; select 1", Dialect::Mysql),
            ["select delimiter from t", "select 1"]
        );
    }

    #[test]
    fn psql_meta_commands_are_their_own_statements() {
        let sql = "select 1;\n\\d users\nselect 2;\n  \\timing on\n";
        let out = split_statements(sql, Dialect::Postgres);
        let got: Vec<_> = out.iter().map(|s| s.sql.as_str()).collect();
        assert_eq!(got, ["select 1", "\\d users", "select 2", "\\timing on"]);
        assert_eq!(out[1].line, 2);
        assert!(is_meta_command(&out[1].sql));
        assert!(!is_meta_command(&out[0].sql));
        // A meta-command line ends the statement before it.
        assert_eq!(
            texts("select 1\n\\g\nselect 2", Dialect::Postgres),
            ["select 1", "\\g", "select 2"]
        );
        // Mid-line backslashes are not meta-commands.
        assert_eq!(
            texts("select 1 \\ 2; select 3", Dialect::Postgres),
            ["select 1 \\ 2", "select 3"]
        );
    }

    #[test]
    fn crlf_and_unicode_offsets() {
        let sql = "select 'héllo';\r\nselect 'wörld' ;\r\n\r\nselect 3";
        let out = split_statements(sql, Dialect::Sqlite);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].sql, "select 'héllo'");
        assert_eq!((out[0].start, out[0].end), (0, 14));
        assert_eq!(out[1].sql, "select 'wörld'");
        assert_eq!((out[1].start, out[1].line), (17, 2));
        assert_eq!(out[2].sql, "select 3");
        assert_eq!(out[2].line, 4);
        texts(sql, Dialect::Sqlite);
        // CRLF inside a line comment.
        assert_eq!(
            texts("select 1 -- c;\r\n; select 2", Dialect::Postgres),
            ["select 1 -- c;", "select 2"]
        );
    }

    #[test]
    fn unterminated_constructs_do_not_panic() {
        for d in ALL {
            for sql in [
                "select 'abc",
                "select \"abc",
                "/* abc",
                "select $$abc",
                "select `x",
                "select [x",
                "--",
                "-",
                "$",
                "e'",
                "DELIMITER",
                "DELIMITER ",
                "\\",
            ] {
                let _ = texts(sql, d);
            }
        }
    }

    #[test]
    fn leading_keyword_detection() {
        assert_eq!(
            leading_keywords("  -- c\n/* x */ (SELECT 1)", 2),
            ["SELECT"]
        );
        assert_eq!(
            leading_keywords("start transaction read only", 2),
            ["START", "TRANSACTION"]
        );
        assert_eq!(
            leading_keywords("rollback to savepoint a", 3),
            ["ROLLBACK", "TO", "SAVEPOINT"]
        );
        assert!(leading_keywords("  ", 1).is_empty());
        assert_eq!(leading_keywords("# c\nshow tables", 1), ["SHOW"]);
    }

    #[test]
    fn single_statement_checks() {
        let my = Dialect::Mysql;
        // The body of a custom-DELIMITER block is several statements for the server.
        let block = &split_statements(
            "DELIMITER //\nSELECT * FROM t; INSERT INTO t VALUES (42) //",
            my,
        )[0];
        assert_eq!(block.sql, "SELECT * FROM t; INSERT INTO t VALUES (42)");
        assert!(!is_single_statement(&block.sql, my));
        assert!(is_single_statement("select 1;", my));
        assert!(is_single_statement("select 'a;b'", my));
        // A routine body keeps its `;`.
        assert!(is_single_statement(
            "CREATE PROCEDURE p() BEGIN SELECT 1; SELECT 2; END",
            my
        ));
        // With NO_BACKSLASH_ESCAPES the string ends at `\'`, and the INSERT is a statement.
        assert!(!is_single_statement(
            r"select 'a\'; insert into t values (1); select '",
            my
        ));
        for d in ALL {
            assert!(is_single_statement("select 1", d));
            assert!(is_single_statement("", d));
            assert!(!is_single_statement("select 1; select 2", d));
        }
        assert!(is_single_statement(r"select 'a\'", Dialect::Postgres));
    }

    #[test]
    fn code_only_masks_literals_identifiers_and_comments() {
        let pg = Dialect::Postgres;
        let my = Dialect::Mysql;
        let words = |sql: &str, d: Dialect| -> Vec<String> {
            code_only(sql, d)
                .split(|c: char| !is_word_char(c))
                .filter(|w| !w.is_empty())
                .map(str::to_ascii_uppercase)
                .collect()
        };
        assert_eq!(
            words("select * from log where op = 'UPDATE' -- delete\n", pg),
            ["SELECT", "FROM", "LOG", "WHERE", "OP"]
        );
        assert_eq!(
            words(
                r#"select "update", $x$delete$x$, E'it\'s insert' /* call */ from t"#,
                pg
            ),
            ["SELECT", "E", "FROM", "T"]
        );
        assert_eq!(
            words(r"select `delete`, 'a\'update', x # insert", my),
            ["SELECT", "X"]
        );
        // Executable comments are code.
        assert_eq!(
            words("select /*!50000 sql_no_cache */ 1", my),
            ["SELECT", "50000", "SQL_NO_CACHE", "1"]
        );
        assert_eq!(
            words("select [delete] from t", Dialect::Sqlite),
            ["SELECT", "FROM", "T"]
        );
        // Unterminated constructs do not panic.
        for d in ALL {
            let _ = code_only("select 'abc", d);
            let _ = code_only("select /* x", d);
            let _ = code_only("select $a$ x", d);
        }
    }
}
