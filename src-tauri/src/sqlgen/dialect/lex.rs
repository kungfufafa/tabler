//! How the text of an extension dialect reads: its `LexProfile` (quotes, comments, batch
//! separators), the `Reading`s of backslashes its server may be in, the `ClassifyRules` its Safe
//! Mode classifier decides with, and the scanner the portable builders share. The core dialects
//! keep their own lexers (`split.rs`, `classify.rs`, `drivers/redact.rs`); these serve the
//! dialects of `SqlDialect` beyond them.
//!
//! The profile and the rules are exported to the renderer (ts-rs types, plus
//! `bindings/lex/<d>.json` written by each dialect's `export_bindings_lex_<d>` test), whose
//! portable scanner reads the same data, so "Run current statement" and Safe Mode split and judge
//! a statement the same way on both sides.
//!
//! Offsets are **character** offsets into the text, like the core splitter's.

use serde::Serialize;
use ts_rs::TS;

/// How the server reads backslashes, which a setting the scanner cannot see may change. A
/// statement is a read only under every reading of its dialect; splitting uses the first one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Reading {
    /// String literals take backslash escapes (`'it\'s'`).
    pub string_backslash: bool,
    /// Quoted identifiers take backslash escapes (ClickHouse, BigQuery backticks).
    pub ident_backslash: bool,
}

pub const fn reading(string_backslash: bool, ident_backslash: bool) -> Reading {
    Reading {
        string_backslash,
        ident_backslash,
    }
}

/// Backslashes are plain characters everywhere.
pub const PLAIN: Reading = reading(false, false);

/// The quoting and comment rules of a dialect's text.
///
/// Safe Mode does not trust the comment rules where servers differ, since the text after a
/// comment that ends elsewhere for the server could be a string to the scanner and code to the
/// server. A block comment holding a `/*`, or a line comment holding a line break other than LF
/// or CRLF (a lone CR, VT, FF, NEL, U+2028, U+2029), makes the text a write and more than one
/// statement whatever the profile says (`Scan::ambiguous`); the renderer's mirror does the same.
/// So does a literal opener that lexers may read differently because they class the characters
/// around it differently: with `dollar_quotes`, a `$` of code that may open a body (a run of tag
/// characters, ASCII letters, digits, `_` or any non-ASCII ones, then another `$` follow it) after
/// a non-ASCII character, with a non-ASCII character in that run, or in a word that starts with a
/// digit (`a\u{A0}$$`, `$\u{A0}$`, `$a\u{A0}b$`, `1$$`); with `e_strings`, an `E'…'` string after
/// a non-ASCII character, and a number glued to `E'` (`1E'…'`).
#[derive(Debug, Clone, Copy, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LexProfile {
    /// Identifier quotes as (open, close): `"…"`, `[…]`, `` `…` ``. Inside, the closing quote
    /// doubled stands for itself.
    #[ts(as = "Vec<(char, char)>")]
    pub ident_quotes: &'static [(char, char)],
    /// String quote characters; inside, the quote doubled stands for itself.
    #[ts(as = "Vec<char>")]
    pub string_quotes: &'static [char],
    /// Every way the server may read backslashes. At least one: with none, nothing is a read.
    #[ts(as = "Vec<Reading>")]
    pub readings: &'static [Reading],
    /// `E'…'` strings take backslash escapes (the PostgreSQL family).
    pub e_strings: bool,
    /// `$$…$$` bodies are strings.
    pub dollar_quotes: bool,
    /// With `dollar_quotes`, `$tag$…$tag$` bodies are strings too (the PostgreSQL family). Off
    /// where the server has only `$$` and `$a$` means something else (Snowflake's session variable
    /// `a$`). Where `$a$` is otherwise a syntax error, on is the safe guess: a scanner that misses
    /// a tag the server takes could see code where the server sees a string.
    pub dollar_tags: bool,
    /// `'''…'''` and `"""…"""` strings, which may hold newlines and single quotes (BigQuery).
    pub triple_quotes: bool,
    /// `r'…'`, `rb'…'` and `br'…'` strings take no escapes (BigQuery).
    pub raw_prefixes: bool,
    /// `b'…'` byte strings (BigQuery); they read like the other strings.
    pub byte_prefixes: bool,
    /// `#` starts a line comment.
    pub hash_comments: bool,
    /// `//` starts a line comment (Snowflake, CQL).
    pub slash_slash_comments: bool,
    /// Block comments nest (a `/*` inside one opens another), as in the lexers of PostgreSQL and
    /// ClickHouse. Splitting follows it; the classifier does not rely on it (a block comment
    /// holding a `/*` makes the text a write either way).
    pub nested_block_comments: bool,
    /// `--` starts a comment only when whitespace follows (MySQL).
    pub dash_comment_needs_space: bool,
    /// A line holding only this word (and an optional repeat count) ends a batch, and statements
    /// are the batches: T-SQL's `GO`, where a `;` ends no batch.
    #[ts(as = "Option<String>")]
    pub batch_separator: Option<&'static str>,
    /// Statements need no `;` between them (T-SQL: `SELECT 1 DELETE FROM t` is two), so a write
    /// keyword anywhere makes the text a write.
    pub terminator_optional: bool,
    /// `BEGIN … END` blocks (procedural bodies, scripts) keep their inner `;`.
    pub begin_end_bodies: bool,
}

/// What the portable classifier (`sqlgen::portable::classify`) decides with. Words are upper
/// case; an entry ending in `*` matches every name that starts with the rest (`XP_*`).
/// Everything not listed as a read is a write, and so is text with a comment a server may end
/// elsewhere (see `LexProfile`), whatever the rules say.
#[derive(Debug, Clone, Copy, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClassifyRules {
    /// First keywords of queries (`SELECT`, `WITH`…): reads unless a word of `writes` is in them.
    #[ts(as = "Vec<String>")]
    pub queries: &'static [&'static str],
    /// First keywords of statements that read whatever follows (`SHOW`, `DESCRIBE`, `USE`).
    /// `EXPLAIN` here plans only; with `ANALYZE` it is judged by the query it runs.
    #[ts(as = "Vec<String>")]
    pub inspections: &'static [&'static str],
    /// Words that make a query a write wherever they are in it (`INTO`, `DELETE`…), and any
    /// statement one when the terminator is optional.
    #[ts(as = "Vec<String>")]
    pub writes: &'static [&'static str],
    /// Functions a call of which (`name(`) changes the server, reads its files or reaches other
    /// servers: T-SQL `OPENQUERY`, `XP_*`, Snowflake `SYSTEM$*`, DuckDB `READ_*`.
    #[ts(as = "Vec<String>")]
    pub write_calls: &'static [&'static str],
    /// The table functions a read may use where a table goes (after `FROM`, `JOIN`, `TABLE`,
    /// `DESCRIBE` or a comma of a `FROM` list); any other table function makes it a write.
    /// None: the dialect has none to police. A file named where a table goes (DuckDB
    /// `FROM 'data.csv'`) is not a table function: the engine settings of the session refuse it.
    #[ts(as = "Option<Vec<String>>")]
    pub table_functions: Option<&'static [&'static str]>,
}

/// What `bindings/lex/<d>.json` holds for the renderer: a dialect's lexer profile and the rules
/// of its classifier.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DialectLex {
    #[ts(as = "LexProfile")]
    pub lex: &'static LexProfile,
    #[ts(as = "ClassifyRules")]
    pub classify: &'static ClassifyRules,
}

/// Writes `lex/<name>.json` next to the ts-rs bindings (the `export_bindings_lex_<d>` tests).
#[cfg(test)]
pub fn export_lex(name: &str, lex: &'static LexProfile, classify: &'static ClassifyRules) {
    let dir = std::env::var("TS_RS_EXPORT_DIR").unwrap_or_else(|_| "./bindings".into());
    let dir = std::path::Path::new(&dir).join("lex");
    std::fs::create_dir_all(&dir).expect("create the lex bindings folder");
    let mut json = serde_json::to_string_pretty(&DialectLex { lex, classify })
        .expect("a lex profile serializes");
    json.push('\n');
    std::fs::write(dir.join(format!("{name}.json")), json).expect("write the lex binding");
}

/// Whether `name` (upper case) is in `list`, whose `PREFIX*` entries match by prefix.
pub fn listed(list: &[&str], name: &str) -> bool {
    list.iter().any(|entry| match entry.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => *entry == name,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// A bare word or number: keywords, names, the digits of `$1`. A number runs on into the
    /// letters after it (`1DELETE` is one word); the classifier reads every keyword such a word
    /// may hide.
    Word,
    /// A quoted identifier.
    Ident,
    /// A string literal (with its `E`, `r` or `b` prefix) or a dollar-quoted body.
    Str,
    Comment,
    Space,
    /// Any other single character.
    Punct,
}

/// A token as a span of characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
}

/// The tokens of a text under one reading. Every character is in exactly one token.
pub struct Scan {
    pub chars: Vec<char>,
    pub tokens: Vec<Token>,
    /// A server may end one of the comments elsewhere than this scan did: a block comment holds
    /// a `/*` (it ends at its first `*/` where comments do not nest, later where they do), or a
    /// line comment holds a line break other than LF or CRLF (PostgreSQL ends one at a CR,
    /// ClickHouse only at an LF). What follows such a comment may be a string to one reader and
    /// code to the other, so the classifier calls the text a write and `is_single_statement`
    /// says no.
    ///
    /// The same holds where a lexer may open a literal this scan did not, or the other way
    /// round, because lexers class the characters around the opener differently: PostgreSQL and
    /// the lexers built on it read every non-ASCII character as part of a name or of a
    /// dollar-quote tag, where others read a no-break space as a space, and they end a number
    /// before a `$` or an `E` this scan keeps in the word. So a `$` that may open a body is
    /// ambiguous after a non-ASCII character, with one in its tag, or in a word that starts with
    /// a digit (`a\u{A0}$$`, `$\u{A0}$`, `$a\u{A0}b$`, `1$$`), and so is an `E'…'` string after a
    /// non-ASCII character or glued to a number (`1E'…'`: `1` then an E-string to those lexers).
    pub ambiguous: bool,
}

impl Scan {
    pub fn text(&self, t: &Token) -> String {
        self.chars[t.start..t.end].iter().collect()
    }

    pub fn upper(&self, t: &Token) -> String {
        self.text(t).to_uppercase()
    }

    /// Whether `t` is the punctuation character `c`.
    pub fn is_punct(&self, t: &Token, c: char) -> bool {
        t.kind == TokenKind::Punct && self.chars[t.start] == c
    }

    /// The name a quoted identifier stands for (quotes removed, doubled quotes undone).
    pub fn ident_name(&self, t: &Token) -> String {
        let inner = &self.chars[t.start + 1..t.end.saturating_sub(1).max(t.start + 1)];
        let close = self.chars[t.end - 1];
        let mut out = String::with_capacity(inner.len());
        let mut i = 0;
        while i < inner.len() {
            out.push(inner[i]);
            i += if inner[i] == close && inner.get(i + 1) == Some(&close) {
                2
            } else {
                1
            };
        }
        out
    }
}

pub fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Splits `sql` into tokens as `lex` reads it under `reading`. An unterminated string, quoted
/// name or comment runs to the end of the text.
pub fn scan(lex: &LexProfile, reading: Reading, sql: &str) -> Scan {
    let chars: Vec<char> = sql.chars().collect();
    let mut tokens = Vec::new();
    let s = Scanner {
        lex,
        reading,
        chars: &chars,
    };
    let mut ambiguous = false;
    let mut i = 0;
    while i < chars.len() {
        let (kind, end) = s.token(i);
        let end = end.max(i + 1).min(chars.len());
        ambiguous |= match kind {
            TokenKind::Comment => s.comment_may_end_elsewhere(i, end),
            _ => s.literal_may_open_elsewhere(kind, i, end),
        };
        tokens.push(Token {
            kind,
            start: i,
            end,
        });
        i = end;
    }
    Scan {
        chars,
        tokens,
        ambiguous,
    }
}

/// A character a dollar-quote tag may hold for some lexer: PostgreSQL's take ASCII letters,
/// digits and `_`, and every non-ASCII character.
fn is_tag_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii()
}

/// Line breaks some lexers end a line comment at besides LF.
fn is_other_line_break(c: char) -> bool {
    matches!(
        c,
        '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

struct Scanner<'a> {
    lex: &'a LexProfile,
    reading: Reading,
    chars: &'a [char],
}

impl Scanner<'_> {
    fn at(&self, i: usize) -> Option<char> {
        self.chars.get(i).copied()
    }

    fn token(&self, i: usize) -> (TokenKind, usize) {
        let c = self.chars[i];
        let next = self.at(i + 1);
        let lex = self.lex;
        if c.is_whitespace() {
            let mut j = i;
            while self.at(j).is_some_and(char::is_whitespace) {
                j += 1;
            }
            return (TokenKind::Space, j);
        }
        let line_comment = match (c, next) {
            ('-', Some('-')) => {
                !lex.dash_comment_needs_space
                    || self
                        .at(i + 2)
                        .is_none_or(|x| x.is_whitespace() || x.is_control())
            }
            ('#', _) => lex.hash_comments,
            ('/', Some('/')) => lex.slash_slash_comments,
            _ => false,
        };
        if line_comment {
            return (TokenKind::Comment, self.line_end(i));
        }
        if c == '/' && next == Some('*') {
            return (TokenKind::Comment, self.block_comment_end(i));
        }
        if c == '$'
            && lex.dollar_quotes
            && let Some(len) = self.dollar_tag(i)
        {
            return (TokenKind::Str, self.dollar_end(i, len));
        }
        if lex.string_quotes.contains(&c) {
            return (
                TokenKind::Str,
                self.string_end(i, self.reading.string_backslash),
            );
        }
        if let Some(&(_, close)) = lex.ident_quotes.iter().find(|(open, _)| *open == c) {
            return (
                TokenKind::Ident,
                self.quoted_end(i, close, self.reading.ident_backslash),
            );
        }
        if is_ident_start(c) || c.is_ascii_digit() {
            let mut j = i;
            while self.at(j).is_some_and(is_word_char) {
                j += 1;
            }
            if let Some(end) = self.prefixed_string(i, j) {
                return (TokenKind::Str, end);
            }
            return (TokenKind::Word, j);
        }
        (TokenKind::Punct, i + 1)
    }

    /// A string whose prefix is the word `i..j` (`E'…'`, `r'…'`, `b'…'`), when the prefix changes
    /// how it reads; its end.
    fn prefixed_string(&self, i: usize, j: usize) -> Option<usize> {
        let quote = self.at(j)?;
        if !self.lex.string_quotes.contains(&quote) {
            return None;
        }
        let prefix: String = self.chars[i..j].iter().collect::<String>().to_uppercase();
        let lex = self.lex;
        match prefix.as_str() {
            "E" if lex.e_strings && quote == '\'' => Some(self.quoted_end(j, quote, true)),
            "R" | "RB" | "BR" if lex.raw_prefixes => Some(self.string_end(j, false)),
            "B" if lex.byte_prefixes => Some(self.string_end(j, self.reading.string_backslash)),
            _ => None,
        }
    }

    /// End of a line comment for splitting: before the LF, or before a lone CR, where the editor
    /// breaks the line too. Servers differ there (PostgreSQL ends a comment at a CR, ClickHouse
    /// only at an LF), so the classifier does not rely on it (`comment_may_end_elsewhere`).
    fn line_end(&self, mut i: usize) -> usize {
        while self.at(i).is_some_and(|c| c != '\n' && c != '\r') {
            i += 1;
        }
        i
    }

    /// Whether a server may end the comment `i..end` elsewhere (`Scan::ambiguous`).
    fn comment_may_end_elsewhere(&self, i: usize, end: usize) -> bool {
        if self.chars[i] == '/' && self.at(i + 1) == Some('*') {
            return self.chars[i + 2..end].windows(2).any(|w| w == ['/', '*']);
        }
        // A line comment, up to the LF: a CR right before the LF ends the same line either way.
        let mut j = i;
        while let Some(c) = self.at(j).filter(|&c| c != '\n') {
            if is_other_line_break(c) && !(c == '\r' && self.at(j + 1) == Some('\n')) {
                return true;
            }
            j += 1;
        }
        false
    }

    /// Whether a lexer may open a literal elsewhere than this scan did around the token `i..end`
    /// of `kind` (`Scan::ambiguous`): a `$` that may open a body next to a character lexers
    /// class differently, or an `E'…'` string whose prefix a lexer may read otherwise.
    fn literal_may_open_elsewhere(&self, kind: TokenKind, i: usize, end: usize) -> bool {
        let chars = self.chars;
        let after_non_ascii = |j: usize| j > 0 && !chars[j - 1].is_ascii();
        let number = kind == TokenKind::Word && chars[i].is_ascii_digit();
        if self.lex.dollar_quotes {
            // The `$`s of code: in words and punctuation, and the opener of a body.
            let dollars = match kind {
                TokenKind::Word | TokenKind::Punct => i..end,
                TokenKind::Str if chars[i] == '$' => i..i + 1,
                _ => i..i,
            };
            let unsure = dollars.filter(|&j| chars[j] == '$').any(|j| {
                let tag = || chars[j + 1..].iter().take_while(|&&c| is_tag_char(c));
                let opens = self.at(j + 1 + tag().count()) == Some('$');
                opens && (number || after_non_ascii(j) || tag().any(|c| !c.is_ascii()))
            });
            if unsure {
                return true;
            }
        }
        if self.lex.e_strings {
            let e_string = kind == TokenKind::Str && matches!(chars[i], 'E' | 'e');
            let glued_e =
                number && matches!(chars[end - 1], 'E' | 'e') && self.at(end) == Some('\'');
            if (e_string && after_non_ascii(i)) || glued_e {
                return true;
            }
        }
        false
    }

    fn block_comment_end(&self, i: usize) -> usize {
        let mut depth = 1;
        let mut j = i + 2;
        while j < self.chars.len() {
            let next = self.at(j + 1);
            if self.lex.nested_block_comments && self.chars[j] == '/' && next == Some('*') {
                depth += 1;
                j += 2;
            } else if self.chars[j] == '*' && next == Some('/') {
                depth -= 1;
                j += 2;
                if depth == 0 {
                    return j;
                }
            } else {
                j += 1;
            }
        }
        self.chars.len()
    }

    /// A string at `i`, triple-quoted where the dialect has those.
    fn string_end(&self, i: usize, backslash: bool) -> usize {
        let q = self.chars[i];
        if self.lex.triple_quotes && self.at(i + 1) == Some(q) && self.at(i + 2) == Some(q) {
            let mut j = i + 3;
            while j < self.chars.len() {
                if backslash && self.chars[j] == '\\' {
                    j += 2;
                } else if self.chars[j] == q
                    && self.at(j + 1) == Some(q)
                    && self.at(j + 2) == Some(q)
                {
                    return j + 3;
                } else {
                    j += 1;
                }
            }
            return self.chars.len();
        }
        self.quoted_end(i, q, backslash)
    }

    /// End of the run quoted from `i` up to `close` (doubled: escaped; and after a backslash
    /// when `backslash` is set).
    fn quoted_end(&self, i: usize, close: char, backslash: bool) -> usize {
        let mut j = i + 1;
        while j < self.chars.len() {
            let c = self.chars[j];
            if backslash && c == '\\' {
                j += 2;
            } else if c == close {
                if self.at(j + 1) == Some(close) {
                    j += 2;
                } else {
                    return j + 1;
                }
            } else {
                j += 1;
            }
        }
        self.chars.len()
    }

    /// Length of the `$$` / `$tag$` opener at `i`. A `$` after a word character (`a$b`) or before
    /// a digit (`$1`) opens nothing, and a tag opens nothing without `dollar_tags`.
    fn dollar_tag(&self, i: usize) -> Option<usize> {
        if i > 0 && is_word_char(self.chars[i - 1]) {
            return None;
        }
        let mut j = i + 1;
        match self.at(j) {
            Some('$') => return Some(2),
            Some(c) if self.lex.dollar_tags && is_ident_start(c) => j += 1,
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

    fn dollar_end(&self, i: usize, len: usize) -> usize {
        let tag = &self.chars[i..i + len];
        let mut j = i + len;
        while j + len <= self.chars.len() {
            if &self.chars[j..j + len] == tag {
                return j + len;
            }
            j += 1;
        }
        self.chars.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEX: LexProfile = LexProfile {
        ident_quotes: &[('"', '"'), ('[', ']')],
        string_quotes: &['\''],
        readings: &[PLAIN],
        e_strings: true,
        dollar_quotes: true,
        dollar_tags: true,
        triple_quotes: false,
        raw_prefixes: false,
        byte_prefixes: false,
        hash_comments: false,
        slash_slash_comments: true,
        nested_block_comments: true,
        dash_comment_needs_space: false,
        batch_separator: None,
        terminator_optional: false,
        begin_end_bodies: false,
    };

    fn kinds(lex: &LexProfile, reading: Reading, sql: &str) -> Vec<(TokenKind, String)> {
        let s = scan(lex, reading, sql);
        s.tokens
            .iter()
            .filter(|t| t.kind != TokenKind::Space)
            .map(|t| (t.kind, s.text(t)))
            .collect()
    }

    #[test]
    fn scans_quotes_comments_and_bodies() {
        use TokenKind::*;
        assert_eq!(
            kinds(
                &LEX,
                PLAIN,
                "SELECT [a]]b], \"x\"\"y\", 'it''s', E'a\\'b' -- c;\n/* /* n */ ; */ $f$;$f$ $1 // z"
            ),
            [
                (Word, "SELECT".into()),
                (Ident, "[a]]b]".into()),
                (Punct, ",".into()),
                (Ident, "\"x\"\"y\"".into()),
                (Punct, ",".into()),
                (Str, "'it''s'".into()),
                (Punct, ",".into()),
                (Str, "E'a\\'b'".into()),
                (Comment, "-- c;".into()),
                (Comment, "/* /* n */ ; */".into()),
                (Str, "$f$;$f$".into()),
                (Punct, "$".into()),
                (Word, "1".into()),
                (Comment, "// z".into()),
            ]
        );
        let s = scan(&LEX, PLAIN, "[a]]b]");
        assert_eq!(s.ident_name(&s.tokens[0]), "a]b");
    }

    #[test]
    fn tags_open_bodies_only_with_dollar_tags() {
        use TokenKind::*;
        let untagged = LexProfile {
            dollar_tags: false,
            ..LEX
        };
        // Snowflake: `$a$` is the session variable `a$`, and only `$$` opens a body.
        assert_eq!(
            kinds(&untagged, PLAIN, "$a$, x, $a$ $$;$$"),
            [
                (Punct, "$".into()),
                (Word, "a$".into()),
                (Punct, ",".into()),
                (Word, "x".into()),
                (Punct, ",".into()),
                (Punct, "$".into()),
                (Word, "a$".into()),
                (Str, "$$;$$".into()),
            ]
        );
        assert_eq!(
            kinds(&LEX, PLAIN, "$a$, x, $a$"),
            [(Str, "$a$, x, $a$".into())]
        );
    }

    #[test]
    fn backslashes_follow_the_reading() {
        let lex = LexProfile {
            raw_prefixes: true,
            triple_quotes: true,
            string_quotes: &['\'', '"'],
            ..LEX
        };
        let texts = |r: Reading, sql: &str| -> Vec<String> {
            kinds(&lex, r, sql).into_iter().map(|t| t.1).collect()
        };
        let escaping = reading(true, false);
        assert_eq!(
            texts(PLAIN, r"'a\' ; DELETE'"),
            [r"'a\'", ";", "DELETE", "'"]
        );
        assert_eq!(texts(escaping, r"'a\' ; DELETE'"), [r"'a\' ; DELETE'"]);
        // A raw string takes no escapes; a triple-quoted one holds single quotes.
        assert_eq!(texts(escaping, r"r'\' x"), [r"r'\'", "x"]);
        assert_eq!(texts(escaping, "'''it's''' x"), ["'''it's'''", "x"]);
    }

    #[test]
    fn comments_a_server_may_end_elsewhere_are_ambiguous() {
        let flat = LexProfile {
            nested_block_comments: false,
            hash_comments: true,
            ..LEX
        };
        for lex in [&LEX, &flat] {
            for sql in [
                "/* /* */ SELECT ' */ DELETE FROM t --'",
                "SELECT 1 /* a /*/ x",
                "SELECT 1 -- x\r' \nDELETE FROM t --'",
                "SELECT 1 // x\r'",
                "SELECT 1 -- x\u{0C}y\n",
                "SELECT 1 -- x\r\r\n",
            ] {
                assert!(scan(lex, PLAIN, sql).ambiguous, "{sql:?}");
            }
            for sql in [
                "/* a */ SELECT 1 /* b */",
                "/*/ a */ SELECT 1",
                "-- a\r\nSELECT 1 -- b\r\n",
                // In a string, neither counts.
                "SELECT '/* x\r' -- y",
                "SELECT 1 -- /* x\n",
            ] {
                assert!(!scan(lex, PLAIN, sql).ambiguous, "{sql:?}");
            }
        }
        assert!(scan(&flat, PLAIN, "SELECT 1 # x\r'").ambiguous);
    }

    #[test]
    fn openers_lexers_class_differently_are_ambiguous() {
        let untagged = LexProfile {
            dollar_tags: false,
            ..LEX
        };
        for lex in [&LEX, &untagged] {
            for sql in [
                // A non-ASCII character before the `$`: part of a name to PostgreSQL, a space or
                // a symbol to others.
                "SELECT 1 AS a\u{A0}$$; DELETE FROM t; --$$",
                "SELECT 1 AS a\u{2003}$$; DELETE FROM t; --$$",
                "SELECT 1 AS a\u{3000}$$; DELETE FROM t; --$$",
                "SELECT 1 AS a\u{B7}$$; DELETE FROM t; --$$",
                "SELECT 1 AS é$$ x $$",
                // A non-ASCII character in the tag.
                "SELECT $\u{A0}$ $$ $\u{A0}$; DELETE FROM t; --$$",
                "SELECT $\u{1F600}$ $$ $\u{1F600}$; DELETE FROM t; --$$",
                "SELECT $a\u{A0}b$ $$ $a\u{A0}b$; DELETE FROM t; --$$",
                "SELECT $é$ x $é$",
                // A number ends before the `$` for PostgreSQL.
                "SELECT 1$$ ; SELECT $$; DELETE FROM t; --$$",
                "SELECT $1$$ ; SELECT $$; DELETE FROM t; --$$",
                "SELECT 1.5$$ x $$",
                "SELECT 1e5$a$ x $a$",
                // An E-string after a non-ASCII character, or glued to a number.
                "SELECT x\u{A0}E'a\\' ; DELETE FROM t; --'",
                "SELECT 1E'a\\' ; DELETE FROM t; --'",
                "SELECT 1e'a'",
            ] {
                assert!(scan(lex, PLAIN, sql).ambiguous, "{sql:?}");
            }
            for sql in [
                "SELECT $$a$$, $q$b$q$, a$$, a$b, $1, $1 + $2, x$1$",
                "SELECT 1 AS a\u{A0}b, '\u{A0}$$', $$\u{A0}$$, $$é$$",
                "SELECT \u{A0}$1, é$1, 1$, E'a', e'b', xE'c', 1 + E'd'",
                "SELECT \"\u{A0}$$\" -- \u{A0}$$\n",
            ] {
                assert!(!scan(lex, PLAIN, sql).ambiguous, "{sql:?}");
            }
        }
        // Without dollar bodies and E-strings, none of it opens anything.
        let plain = LexProfile {
            dollar_quotes: false,
            dollar_tags: false,
            e_strings: false,
            ..LEX
        };
        for sql in [
            "SELECT a\u{A0}$$ x $$",
            "SELECT 1$$ x $$",
            "SELECT x\u{A0}E'a'",
        ] {
            assert!(!scan(&plain, PLAIN, sql).ambiguous, "{sql:?}");
        }
    }

    #[test]
    fn listed_names_match_exactly_or_by_prefix() {
        let list = ["OPENQUERY", "XP_*"];
        assert!(listed(&list, "OPENQUERY") && listed(&list, "XP_CMDSHELL"));
        assert!(!listed(&list, "OPENQUERYX") && !listed(&list, "SP_WHO"));
    }
}
