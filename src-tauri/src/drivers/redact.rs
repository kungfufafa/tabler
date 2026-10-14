//! Keeps passwords out of the SQL trace.
//!
//! The trace (`SqlTrace`) records the statements the drivers send for the app's own work and the
//! console log shows them. Some of those carry a secret: the account statements of Connection ▸
//! User Management (`CREATE USER … IDENTIFIED BY '…'`, `ALTER ROLE … PASSWORD '…'`), and anything
//! that ends up in `run_statement_traced`. `redact_secrets` finds the string literal after a
//! password keyword and replaces it with the mask the previews of those statements use
//! (`sqlgen::admin::PASSWORD_MASK`), so a masked statement of the trace is the very text of the
//! masked statement the command reports (`AdminStatement::display`) and the two are one line in
//! the log.
//!
//! The statements are read the way the splitter reads them (strings, quoted names, comments,
//! `$tag$` bodies), but only for what precedes a literal:
//! - `PASSWORD 'x'`, `ENCRYPTED PASSWORD 'x'`, `MASTER_PASSWORD = 'x'`, `PASSWORD('x')`,
//!   `SET PASSWORD [FOR user] = 'x'`;
//! - after `IDENTIFIED`: `BY 'x'`, `WITH plugin BY 'x'`, `AS 'hash'`, `USING 'x'`, `REPLACE 'old'`;
//! - SQLite `PRAGMA key = 'x'` / `rekey`.
//!
//! MySQL reads backslashes in a string as escapes unless the session has `NO_BACKSLASH_ESCAPES`,
//! which the trace cannot know: a statement with a backslash is read both ways, so a literal that
//! one of the readings finds is masked.

use crate::sqlgen::Dialect;
use crate::sqlgen::admin::PASSWORD_MASK;

/// A statement with its secrets masked, and what the masked literals said.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Redaction {
    /// The statement, every password literal replaced by `PASSWORD_MASK`.
    pub sql: String,
    /// The masked literals' text in every form a server message could quote it in (as written,
    /// and unescaped), to mask in an error of the statement.
    pub secrets: Vec<String>,
}

/// The mask in a bare form (`********`), which is not a secret to look for in a message.
const MASK_BODY: &str = "********";

/// `sql` with the password literals masked. Fast for the statements that cannot hold one.
pub fn redact_secrets(sql: &str, dialect: Dialect) -> Redaction {
    if !may_carry_secret(sql) {
        return Redaction {
            sql: sql.to_string(),
            secrets: Vec::new(),
        };
    }
    let either = dialect == Dialect::Mysql && sql.contains('\\');
    let mut redaction = scan(sql, dialect, dialect == Dialect::Mysql, either);
    // MySQL: the other reading of backslashes finds what the first one misread as part of a string.
    if either {
        let second = scan(&redaction.sql, dialect, false, true);
        redaction.sql = second.sql;
        for form in second.secrets {
            if !redaction.secrets.contains(&form) {
                redaction.secrets.push(form);
            }
        }
    }
    redaction
}

/// Whether `sql` mentions a word a password can follow (cheap, case-insensitive; no allocation).
fn may_carry_secret(sql: &str) -> bool {
    ["PASSWORD", "IDENTIFIED", "PRAGMA"].iter().any(|word| {
        sql.as_bytes()
            .windows(word.len())
            .any(|w| w.eq_ignore_ascii_case(word.as_bytes()))
    })
}

/// A significant token before a literal.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// A bare word, upper-cased.
    Word(String),
    Punct(char),
    /// A string literal or a quoted name that was not a secret.
    Quoted,
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// `PASSWORD`, `MASTER_PASSWORD`…
fn is_password_word(w: &str) -> bool {
    w == "PASSWORD" || w.ends_with("_PASSWORD")
}

/// What `scan` knows of the statement so far.
#[derive(Default)]
struct Statement {
    /// The last two tokens.
    prev: [Option<Tok>; 2],
    /// The first two words (upper-cased).
    head: Vec<String>,
    /// `IDENTIFIED` was seen (MySQL / MariaDB account statements).
    identified: bool,
}

impl Statement {
    fn push(&mut self, tok: Tok) {
        if let Tok::Word(w) = &tok {
            if self.head.len() < 2 {
                self.head.push(w.clone());
            }
            if w == "IDENTIFIED" {
                self.identified = true;
            }
        }
        self.prev[0] = self.prev[1].take();
        self.prev[1] = Some(tok);
    }

    fn before_last_word(&self) -> Option<&str> {
        match &self.prev[0] {
            Some(Tok::Word(w)) => Some(w),
            _ => None,
        }
    }

    /// Whether a string literal that starts here is a password.
    fn literal_is_secret(&self) -> bool {
        let Some(last) = &self.prev[1] else {
            return false;
        };
        match last {
            // `PASSWORD 'x'`, `ENCRYPTED PASSWORD 'x'`, `IDENTIFIED BY PASSWORD 'hash'`
            Tok::Word(w) if is_password_word(w) => true,
            // `IDENTIFIED BY 'x'`, `IDENTIFIED WITH plugin BY 'x'` / `AS 'hash'`, `REPLACE 'old'`
            Tok::Word(w) => {
                self.identified && matches!(w.as_str(), "BY" | "AS" | "USING" | "REPLACE")
            }
            // `MASTER_PASSWORD = 'x'`, `SET PASSWORD [FOR user] = 'x'`, `PRAGMA key = 'x'`
            Tok::Punct('=') => {
                self.before_last_word().is_some_and(is_password_word)
                    || (self.head.first().map(String::as_str) == Some("SET")
                        && self.head.get(1).map(String::as_str) == Some("PASSWORD"))
                    || (self.head.first().map(String::as_str) == Some("PRAGMA")
                        && matches!(self.before_last_word(), Some("KEY" | "REKEY")))
            }
            // `PASSWORD('x')`, `PRAGMA key('x')`
            Tok::Punct('(') => {
                self.before_last_word().is_some_and(is_password_word)
                    || (self.head.first().map(String::as_str) == Some("PRAGMA")
                        && matches!(self.before_last_word(), Some("KEY" | "REKEY")))
            }
            _ => false,
        }
    }
}

/// The index after the string that opens at `open` (a quote character): doubled quotes escape,
/// and so do backslashes when `backslash` is set. An unterminated string runs to the end.
fn string_end(chars: &[char], open: usize, backslash: bool) -> usize {
    let quote = chars[open];
    let mut j = open + 1;
    while j < chars.len() {
        let c = chars[j];
        if backslash && c == '\\' {
            j += 2;
            continue;
        }
        if c == quote {
            if chars.get(j + 1) == Some(&quote) {
                j += 2;
                continue;
            }
            return j + 1;
        }
        j += 1;
    }
    chars.len()
}

/// The end of the `$tag$ … $tag$` body that opens at `open`, when one does (PostgreSQL).
fn dollar_end(chars: &[char], open: usize) -> Option<usize> {
    let mut j = open + 1;
    if chars.get(j).is_some_and(|c| c.is_ascii_digit()) {
        return None; // `$1`
    }
    while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
        j += 1;
    }
    if chars.get(j) != Some(&'$') {
        return None;
    }
    let tag = &chars[open..=j];
    let mut k = j + 1;
    while k + tag.len() <= chars.len() {
        if &chars[k..k + tag.len()] == tag {
            return Some(k + tag.len());
        }
        k += 1;
    }
    Some(chars.len())
}

/// Characters that make a single letter or `_charset` word the introducer of the string literal
/// that follows it: `E'…'`, `N'…'`, `X'…'`, `B'…'`, `_utf8mb4'…'`.
fn is_introducer(word: &str) -> bool {
    word.starts_with('_')
        || (word.chars().count() == 1 && "eEnNxXbB".contains(word))
        || word.eq_ignore_ascii_case("U")
}

/// The text of a masked literal in the forms a server message may quote it in: as written between
/// its quotes, and unescaped (`''` to `'`, and a backslash escape to its character).
fn forms_of(literal: &[char], backslash: bool) -> Vec<String> {
    let text: String = literal.iter().collect();
    if literal.first() == Some(&'$') {
        return dedup_forms(dollar_body(&text).map(str::to_string).into_iter().collect());
    }
    // The body: after the prefix (`E`, `_utf8mb4`) and the opening quote, before the closing one.
    let Some(at) = text.find(['\'', '"']) else {
        return Vec::new();
    };
    let quote = text[at..].chars().next().unwrap_or('\'');
    let inner = &text[at + 1..];
    let body = inner.strip_suffix(quote).unwrap_or(inner).to_string();
    let mut unescaped = body.replace(&format!("{quote}{quote}"), &quote.to_string());
    if backslash {
        let mut out = String::with_capacity(unescaped.len());
        let mut it = unescaped.chars();
        while let Some(c) = it.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match it.next() {
                Some('0') => out.push('\0'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        }
        unescaped = out;
    }
    dedup_forms(vec![body, unescaped])
}

/// The body of a `$tag$ … $tag$` literal.
fn dollar_body(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('$')?;
    let tag_len = rest.find('$')?;
    let tag = &text[..tag_len + 2];
    text.strip_prefix(tag)?.strip_suffix(tag)
}

fn dedup_forms(forms: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for form in forms {
        if !form.is_empty() && form != MASK_BODY && !out.contains(&form) {
            out.push(form);
        }
    }
    out
}

/// One reading of `sql` (`backslash`: whether `\` escapes inside `'…'` / `"…"`). A password
/// literal ends where the later of the two readings ends it when `either` is set.
fn scan(sql: &str, dialect: Dialect, backslash: bool, either: bool) -> Redaction {
    let chars: Vec<char> = sql.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(sql.len());
    let mut secrets: Vec<String> = Vec::new();
    let mut st = Statement::default();
    let mysql = dialect == Dialect::Mysql;
    let copy = |out: &mut String, from: usize, to: usize| out.extend(&chars[from..to.min(n)]);
    let mut i = 0;
    while i < n {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        // Whitespace.
        if c.is_whitespace() {
            out.push(c);
            i += 1;
            continue;
        }
        // Comments. MySQL executable comments (`/*! … */`) and hints are code: scan inside them.
        if c == '-' && next == Some('-') && (!mysql || next_is_blank(&chars, i + 2)) {
            let end = chars[i..]
                .iter()
                .position(|&x| x == '\n')
                .map_or(n, |p| i + p);
            copy(&mut out, i, end);
            i = end;
            continue;
        }
        if c == '#' && mysql {
            let end = chars[i..]
                .iter()
                .position(|&x| x == '\n')
                .map_or(n, |p| i + p);
            copy(&mut out, i, end);
            i = end;
            continue;
        }
        if c == '/' && next == Some('*') {
            if mysql && matches!(chars.get(i + 2), Some('!') | Some('+')) {
                copy(&mut out, i, i + 3);
                i += 3;
                continue;
            }
            let end = block_comment_end(&chars, i, dialect == Dialect::Postgres);
            copy(&mut out, i, end);
            i = end;
            continue;
        }
        if c == ';' {
            out.push(c);
            st = Statement::default();
            i += 1;
            continue;
        }
        // String literals (a MySQL double-quoted one too) and `$tag$` bodies.
        let literal = match c {
            '\'' => Some((i, i, true)),
            '"' if mysql => Some((i, i, true)),
            '$' if dialect == Dialect::Postgres => dollar_end(&chars, i).map(|_| (i, i, false)),
            _ => None,
        };
        if let Some((start, open, quoted)) = literal {
            i = literal_token(
                &chars,
                start,
                open,
                quoted,
                (backslash, either),
                &mut st,
                &mut out,
                &mut secrets,
            );
            continue;
        }
        // Quoted names.
        let quoted_name = match c {
            '"' => Some(string_end(&chars, i, false)),
            '`' if dialect != Dialect::Postgres => Some(string_end(&chars, i, false)),
            '[' if dialect == Dialect::Sqlite => Some(
                chars[i..]
                    .iter()
                    .position(|&x| x == ']')
                    .map_or(n, |p| i + p + 1),
            ),
            _ => None,
        };
        if let Some(end) = quoted_name {
            copy(&mut out, i, end);
            st.push(Tok::Quoted);
            i = end;
            continue;
        }
        // Words, and the introducer of a literal (`E'…'`, `_utf8mb4'…'`, `U&'…'`).
        if is_word_start(c) {
            let mut j = i;
            while j < n && is_word_char(chars[j]) {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            let upper = word.to_uppercase();
            let quote_at = if is_introducer(&word)
                && (chars.get(j) == Some(&'\'') || (mysql && chars.get(j) == Some(&'"')))
            {
                Some(j)
            } else if word.eq_ignore_ascii_case("U")
                && chars.get(j) == Some(&'&')
                && chars.get(j + 1) == Some(&'\'')
            {
                Some(j + 1)
            } else {
                None
            };
            if let Some(open) = quote_at {
                // `E'…'` reads backslashes as escapes in PostgreSQL whatever the session says.
                let escapes = backslash || (dialect == Dialect::Postgres && upper == "E");
                i = literal_token(
                    &chars,
                    i,
                    open,
                    true,
                    (escapes, either),
                    &mut st,
                    &mut out,
                    &mut secrets,
                );
                continue;
            }
            copy(&mut out, i, j);
            st.push(Tok::Word(upper));
            i = j;
            continue;
        }
        out.push(c);
        st.push(Tok::Punct(c));
        i += 1;
    }
    Redaction { sql: out, secrets }
}

/// Handles the literal whose token starts at `start` (its quote at `open`): masks it when the
/// statement says it is a password, else copies it. `escapes` is `(backslash, either)`: how this
/// reading takes backslashes, and whether a password ends where the later reading ends it.
/// Returns where scanning goes on.
#[allow(clippy::too_many_arguments)]
fn literal_token(
    chars: &[char],
    start: usize,
    open: usize,
    quoted: bool,
    escapes: (bool, bool),
    st: &mut Statement,
    out: &mut String,
    secrets: &mut Vec<String>,
) -> usize {
    let (backslash, either) = escapes;
    let mut end = if quoted {
        string_end(chars, open, backslash)
    } else {
        dollar_end(chars, open).unwrap_or(chars.len())
    };
    let secret = st.literal_is_secret();
    if secret && quoted && either {
        end = end.max(string_end(chars, open, !backslash));
    }
    if secret {
        for form in forms_of(&chars[start..end], backslash) {
            if !secrets.contains(&form) {
                secrets.push(form);
            }
        }
        out.push_str(PASSWORD_MASK);
    } else {
        out.extend(&chars[start..end]);
    }
    st.push(Tok::Quoted);
    end
}

fn next_is_blank(chars: &[char], at: usize) -> bool {
    chars
        .get(at)
        .is_none_or(|c| c.is_whitespace() || c.is_control())
}

/// The index after the block comment that opens at `open` (nested ones count in PostgreSQL).
fn block_comment_end(chars: &[char], open: usize, nested: bool) -> usize {
    let mut depth = 1;
    let mut j = open + 2;
    while j < chars.len() {
        if chars[j] == '*' && chars.get(j + 1) == Some(&'/') {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return j;
            }
        } else if nested && chars[j] == '/' && chars.get(j + 1) == Some(&'*') {
            depth += 1;
            j += 2;
        } else {
            j += 1;
        }
    }
    chars.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redact(dialect: Dialect, sql: &str) -> String {
        redact_secrets(sql, dialect).sql
    }

    #[test]
    fn masks_what_the_account_statements_send() {
        // The masked text is the text `AdminStatement::display` shows.
        assert_eq!(
            redact(
                Dialect::Mysql,
                "CREATE USER 'app'@'%' IDENTIFIED BY 'hunter2' WITH MAX_QUERIES_PER_HOUR 5"
            ),
            "CREATE USER 'app'@'%' IDENTIFIED BY '********' WITH MAX_QUERIES_PER_HOUR 5"
        );
        assert_eq!(
            redact(
                Dialect::Mysql,
                "ALTER USER 'app'@'%' IDENTIFIED BY 'hunter2'"
            ),
            "ALTER USER 'app'@'%' IDENTIFIED BY '********'"
        );
        assert_eq!(
            redact(
                Dialect::Postgres,
                r#"CREATE ROLE "app" WITH LOGIN PASSWORD 'hunter2'"#
            ),
            r#"CREATE ROLE "app" WITH LOGIN PASSWORD '********'"#
        );
        assert_eq!(
            redact(
                Dialect::Postgres,
                r#"ALTER ROLE "app" WITH PASSWORD E'it''s \\ hunter2'"#
            ),
            r#"ALTER ROLE "app" WITH PASSWORD '********'"#
        );
        assert_eq!(
            redact(
                Dialect::Postgres,
                "ALTER ROLE app PASSWORD 'SCRAM-SHA-256$4096:c2FsdA==$c3RvcmVk:c2VydmVy'"
            ),
            "ALTER ROLE app PASSWORD '********'"
        );
    }

    #[test]
    fn masks_the_other_spellings_of_a_password() {
        let cases = [
            (
                Dialect::Mysql,
                "CREATE USER u IDENTIFIED WITH caching_sha2_password BY 'pw1'",
                "CREATE USER u IDENTIFIED WITH caching_sha2_password BY '********'",
            ),
            (
                Dialect::Mysql,
                "ALTER USER u IDENTIFIED BY 'new1' REPLACE 'old1'",
                "ALTER USER u IDENTIFIED BY '********' REPLACE '********'",
            ),
            (
                Dialect::Mysql,
                "CREATE USER u IDENTIFIED VIA mysql_native_password USING PASSWORD('pw1')",
                "CREATE USER u IDENTIFIED VIA mysql_native_password USING PASSWORD('********')",
            ),
            (
                Dialect::Mysql,
                "SET PASSWORD FOR 'u'@'h' = 'pw1'",
                "SET PASSWORD FOR 'u'@'h' = '********'",
            ),
            (
                Dialect::Mysql,
                "SET PASSWORD = PASSWORD(\"pw1\")",
                "SET PASSWORD = PASSWORD('********')",
            ),
            (
                Dialect::Mysql,
                "CHANGE MASTER TO MASTER_PASSWORD='pw1', MASTER_USER='r'",
                "CHANGE MASTER TO MASTER_PASSWORD='********', MASTER_USER='r'",
            ),
            (
                Dialect::Mysql,
                "ALTER USER u IDENTIFIED BY _utf8mb4'pw1'",
                "ALTER USER u IDENTIFIED BY '********'",
            ),
            (
                Dialect::Postgres,
                "CREATE ROLE r WITH ENCRYPTED PASSWORD $$pw1$$",
                "CREATE ROLE r WITH ENCRYPTED PASSWORD '********'",
            ),
            (
                Dialect::Postgres,
                "CREATE USER MAPPING FOR u SERVER s OPTIONS (user 'u', password 'pw1')",
                "CREATE USER MAPPING FOR u SERVER s OPTIONS (user 'u', password '********')",
            ),
            (
                Dialect::Sqlite,
                "PRAGMA key = 'pw1'",
                "PRAGMA key = '********'",
            ),
            (
                Dialect::Sqlite,
                "PRAGMA rekey('pw1')",
                "PRAGMA rekey('********')",
            ),
        ];
        for (dialect, sql, masked) in cases {
            let got = redact_secrets(sql, dialect);
            assert_eq!(got.sql, masked, "{sql}");
            assert!(!got.sql.contains("pw1") && !got.sql.contains("new1"));
            assert!(!got.secrets.is_empty(), "{sql}");
        }
    }

    #[test]
    fn leaves_everything_else_alone() {
        for (dialect, sql) in [
            (
                Dialect::Mysql,
                "SELECT 'password', password FROM users WHERE name = 'x'",
            ),
            (Dialect::Mysql, "SELECT * FROM users ORDER BY 'x'"),
            (
                Dialect::Postgres,
                "ALTER ROLE r SET password_encryption = 'scram-sha-256'",
            ),
            (
                Dialect::Postgres,
                "SELECT rolpassword FROM pg_authid WHERE rolname = 'x'",
            ),
            (
                Dialect::Postgres,
                r#"UPDATE t SET "password" = 'x' WHERE id = $1"#,
            ),
            (
                Dialect::Mysql,
                "CREATE USER u IDENTIFIED BY RANDOM PASSWORD",
            ),
            (Dialect::Postgres, "ALTER ROLE r PASSWORD NULL"),
            (Dialect::Mysql, "-- IDENTIFIED BY 'x'\nSELECT 1"),
            (Dialect::Sqlite, "PRAGMA table_info('t')"),
            (Dialect::Sqlite, "SELECT * FROM t WHERE key = 'k'"),
            (Dialect::Postgres, "SELECT 1"),
        ] {
            let got = redact_secrets(sql, dialect);
            assert_eq!(got.sql, sql);
            assert!(got.secrets.is_empty(), "{sql}");
        }
    }

    #[test]
    fn a_literal_in_a_comment_or_another_string_is_not_a_password() {
        let sql = "SELECT 'a PASSWORD ''x'' b' /* PASSWORD 'y' */ -- PASSWORD 'z'";
        assert_eq!(redact(Dialect::Postgres, sql), sql);
    }

    #[test]
    fn masks_every_statement_of_a_batch() {
        assert_eq!(
            redact(
                Dialect::Mysql,
                "CREATE USER a IDENTIFIED BY 'pw1'; CREATE USER b IDENTIFIED BY 'pw2'; SELECT 'ok'"
            ),
            "CREATE USER a IDENTIFIED BY '********'; CREATE USER b IDENTIFIED BY '********'; SELECT 'ok'"
        );
    }

    #[test]
    fn masks_inside_a_mysql_executable_comment() {
        assert_eq!(
            redact(
                Dialect::Mysql,
                "/*!50000 CREATE USER u IDENTIFIED BY 'pw1' */"
            ),
            "/*!50000 CREATE USER u IDENTIFIED BY '********' */"
        );
    }

    #[test]
    fn keeps_the_secret_out_whatever_the_mysql_session_reads_a_backslash_as() {
        // Password `a\'b` as `quote_literal_with` writes it, in both session modes.
        for literal in [r"'a\\''b'", r"'a\''b'"] {
            let sql = format!("CREATE USER 'u'@'%' IDENTIFIED BY {literal}");
            let got = redact_secrets(&sql, Dialect::Mysql);
            assert!(!got.sql.contains('b'), "{literal}: {}", got.sql);
            assert!(got.sql.ends_with("IDENTIFIED BY '********'"), "{}", got.sql);
        }
        // A user name ending in a backslash (NO_BACKSLASH_ESCAPES) must not hide the password.
        let sql = r"CREATE USER 'a\'@'%' IDENTIFIED BY 'pw1'";
        let got = redact_secrets(sql, Dialect::Mysql);
        assert!(!got.sql.contains("pw1"), "{}", got.sql);
    }

    #[test]
    fn lists_the_forms_a_server_message_may_quote() {
        let got = redact_secrets(
            r"ALTER USER 'u'@'%' IDENTIFIED BY 'it''s \\ x'",
            Dialect::Mysql,
        );
        assert!(
            got.secrets.contains(&r"it''s \\ x".to_string()),
            "{:?}",
            got.secrets
        );
        assert!(
            got.secrets.contains(&r"it's \ x".to_string()),
            "{:?}",
            got.secrets
        );
        let got = redact_secrets("ALTER ROLE r PASSWORD $q$p'w$q$", Dialect::Postgres);
        assert_eq!(got.secrets, ["p'w"]);
        assert!(
            redact_secrets("SELECT 1", Dialect::Postgres)
                .secrets
                .is_empty()
        );
    }

    #[test]
    fn an_already_masked_statement_is_stable() {
        let masked = "CREATE USER 'u'@'%' IDENTIFIED BY '********'";
        let got = redact_secrets(masked, Dialect::Mysql);
        assert_eq!(got.sql, masked);
        assert!(got.secrets.is_empty());
    }

    /// What the account editor really sends, for passwords that need escaping and every way the
    /// sessions read them: the masked trace text is the text of the masked preview, and nothing
    /// of the password is left in it, in any of its spellings.
    #[test]
    fn the_masked_trace_of_an_account_statement_is_its_preview() {
        use crate::sqlgen::admin::{PgPassword, SCRAM_SALT_LEN, UserSqlContext, user_statements};
        use crate::sqlgen::quote::MysqlLiteralMode;
        use crate::types::admin::{RoleAttributes, UserChange, UserState};

        let passwords = [
            "hunter2",
            "it's",
            r"a\b",
            r"a\",
            r"a\'b",
            r#"say "hi""#,
            "$tag$x$tag$",
            "日本語pw",
            "pw;DROP USER x",
            "--pw",
            "/*pw*/",
            "p%w_d",
        ];
        for password in passwords {
            let secret_chars: Vec<String> = vec![
                password.to_string(),
                password.replace('\'', "''"),
                password.replace('\\', "\\\\").replace('\'', "''"),
            ];
            // MySQL, both readings of backslashes, with a plain and an awkward account name.
            for mode in [
                MysqlLiteralMode::BackslashEscapes,
                MysqlLiteralMode::NoBackslashEscapes,
            ] {
                for (name, host) in [("app", "%"), (r"back\", "local_host")] {
                    for original in [
                        None,
                        Some(UserState {
                            name: name.into(),
                            host: Some(host.into()),
                            ..UserState::default()
                        }),
                    ] {
                        let change = UserChange {
                            original,
                            edited: Some(UserState {
                                name: name.into(),
                                host: Some(host.into()),
                                ..UserState::default()
                            }),
                            password: Some(password.into()),
                        };
                        let ctx = UserSqlContext {
                            mysql_mode: mode,
                            ..UserSqlContext::default()
                        };
                        for st in user_statements(Dialect::Mysql, &change, &ctx).unwrap() {
                            let got = redact_secrets(&st.sql, Dialect::Mysql);
                            assert_eq!(got.sql, st.display, "{mode:?} {name} {password}");
                            assert_eq!(got.secrets.is_empty(), st.secret.is_none());
                        }
                    }
                }
            }
            // PostgreSQL: the cleartext and the SCRAM verifier.
            for how in [
                PgPassword::Plain,
                PgPassword::Scram {
                    salt: [7; SCRAM_SALT_LEN],
                },
            ] {
                for original in [
                    None,
                    Some(UserState {
                        name: "app".into(),
                        attributes: Some(RoleAttributes::default()),
                        ..UserState::default()
                    }),
                ] {
                    let state = UserState {
                        name: "app".into(),
                        attributes: Some(RoleAttributes {
                            login: true,
                            ..RoleAttributes::default()
                        }),
                        ..UserState::default()
                    };
                    let change = UserChange {
                        original,
                        edited: Some(state),
                        password: Some(password.into()),
                    };
                    let ctx = UserSqlContext {
                        pg_password: how,
                        ..UserSqlContext::default()
                    };
                    for st in user_statements(Dialect::Postgres, &change, &ctx).unwrap() {
                        let got = redact_secrets(&st.sql, Dialect::Postgres);
                        assert_eq!(got.sql, st.display, "{how:?} {password}");
                        assert_eq!(got.secrets.is_empty(), st.secret.is_none());
                        if matches!(how, PgPassword::Plain) {
                            for form in &secret_chars {
                                if form.chars().count() >= 4 {
                                    assert!(!got.sql.contains(form.as_str()), "{}", got.sql);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
