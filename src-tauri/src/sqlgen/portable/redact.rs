//! Keeps passwords out of the SQL trace for the extension dialects: the string literal after a
//! password keyword of the dialect (`SqlKnobs::redact`, also as a named argument: `token := '…'`,
//! `password => '…'`), after the first argument of a command that starts a statement
//! (`SqlKnobs::redact_commands`), or in a function's secret argument (`SqlKnobs::redact_calls`)
//! becomes the mask the core redaction uses (`sqlgen::admin::PASSWORD_MASK`). A `‘…’` quoted
//! name counts as a string: ClickHouse reads it as one, and the profile lists it with the name
//! quotes only because its closing quote differs. Statements are read the way the dialect's scanner reads them,
//! under every reading of backslashes; where the readings end a literal in different places, the
//! mask covers the longest of them (`'x\'y'` is masked whole, not only up to `\'`).

use crate::drivers::redact::Redaction;
use crate::sqlgen::admin::PASSWORD_MASK;
use crate::sqlgen::dialect::lex::{LexProfile, Scan, Token, TokenKind, scan};
use crate::sqlgen::dialect::spec::{DialectSpec, RedactCall, RedactRule};

/// Words that may stand between a password keyword and its literal.
const LINKS: &[&str] = &["BY", "AS", "WITH", "USING"];

/// `sql` with the literals after `rules`' keywords masked. Fast when no keyword is mentioned.
pub fn redact(lex: &LexProfile, rules: &[RedactRule], sql: &str) -> Redaction {
    redact_with(lex, rules, &[], &[], sql)
}

/// `sql` with the passwords a statement of `spec`'s dialect carries masked: after its password
/// keywords, in its commands and in its functions' secret arguments (`SqlKnobs::redact`,
/// `SqlKnobs::redact_commands`, `SqlKnobs::redact_calls`).
pub fn redact_spec(spec: &DialectSpec, sql: &str) -> Redaction {
    let k = &spec.knobs;
    redact_with(spec.lex, k.redact, k.redact_commands, k.redact_calls, sql)
}

fn redact_with(
    lex: &LexProfile,
    rules: &[RedactRule],
    commands: &[&str],
    calls: &[RedactCall],
    sql: &str,
) -> Redaction {
    let mut redaction = Redaction {
        sql: sql.to_string(),
        secrets: Vec::new(),
    };
    let mut keywords = rules
        .iter()
        .map(|r| r.keyword)
        .chain(commands.iter().copied())
        .chain(calls.iter().map(|c| c.function));
    if !keywords.any(|k| mentions(sql, k)) {
        return redaction;
    }
    let chars: Vec<char> = sql.chars().collect();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for &reading in lex.readings {
        let scan = scan(lex, reading, sql);
        for span in secret_literals(&scan, rules, commands, calls) {
            let raw: String = chars[span.0..span.1].iter().collect();
            for form in [unquoted(&raw, reading.string_backslash), raw] {
                if !form.is_empty() && !redaction.secrets.contains(&form) {
                    redaction.secrets.push(form);
                }
            }
            spans.push(span);
        }
    }
    if spans.is_empty() {
        return redaction;
    }
    // One mask over the literals the readings found, overlapping ones merged.
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if start < last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    let mut text = String::with_capacity(sql.len());
    let mut at = 0;
    for (start, end) in merged {
        text.extend(&chars[at..start]);
        text.push_str(PASSWORD_MASK);
        at = end;
    }
    text.extend(&chars[at..]);
    redaction.sql = text;
    redaction
}

/// Whether `sql` mentions `keyword` in any case (cheap; no allocation).
fn mentions(sql: &str, keyword: &str) -> bool {
    sql.as_bytes()
        .windows(keyword.len())
        .any(|w| w.eq_ignore_ascii_case(keyword.as_bytes()))
}

/// The spans of the string literals that follow a keyword of `rules`, the first argument of a
/// command of `commands` that starts a statement, or stand in the secret argument of a call of
/// `calls`. A literal that is already the mask is left alone.
fn secret_literals(
    scan: &Scan,
    rules: &[RedactRule],
    commands: &[&str],
    calls: &[RedactCall],
) -> Vec<(usize, usize)> {
    let code: Vec<usize> = (0..scan.tokens.len())
        .filter(|&i| !matches!(scan.tokens[i].kind, TokenKind::Space | TokenKind::Comment))
        .collect();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (k, &i) in code.iter().enumerate() {
        let t = scan.tokens[i];
        if t.kind != TokenKind::Word {
            continue;
        }
        let word = scan.upper(&t);
        let starts = k == 0 || scan.is_punct(&scan.tokens[code[k - 1]], ';');
        if starts && commands.contains(&word.as_str()) {
            // `LOGIN user 'password'`: the user is a word, a quoted name or a string.
            let at = |n: usize| code.get(k + n).map(|&j| scan.tokens[j]);
            if let (Some(user), Some(secret)) = (at(1), at(2))
                && matches!(
                    user.kind,
                    TokenKind::Word | TokenKind::Ident | TokenKind::Str
                )
                && is_literal(scan, &secret)
            {
                keep(scan, secret, &mut out);
            }
            continue;
        }
        if let Some(call) = calls.iter().find(|c| c.function == word)
            && code
                .get(k + 1)
                .is_some_and(|&j| scan.is_punct(&scan.tokens[j], '('))
        {
            argument_literals(scan, &code[k + 2..], call.argument, &mut out);
        }
        if !rules.iter().any(|r| r.keyword == word) {
            continue;
        }
        let mut other_words = 0;
        for &j in &code[k + 1..] {
            let n = scan.tokens[j];
            match n.kind {
                _ if is_literal(scan, &n) => {
                    keep(scan, n, &mut out);
                    break;
                }
                // `=`, `(`, and the named-argument arrows `:=` and `=>`.
                TokenKind::Punct if ['=', '(', ':', '>'].iter().any(|&c| scan.is_punct(&n, c)) => {}
                TokenKind::Word if LINKS.contains(&scan.upper(&n).as_str()) => {}
                TokenKind::Word | TokenKind::Ident if other_words == 0 => other_words += 1,
                _ => break,
            }
        }
    }
    out.sort_unstable();
    out
}

/// Whether `t` is a string literal, or a `‘…’` quoted name (ClickHouse's other string).
fn is_literal(scan: &Scan, t: &Token) -> bool {
    t.kind == TokenKind::Str || (t.kind == TokenKind::Ident && scan.chars[t.start] == '\u{2018}')
}

/// Keeps the literals of the argument number `argument` (1-based) of the call whose arguments
/// `args` (indexes of code tokens) follow its `(`.
fn argument_literals(scan: &Scan, args: &[usize], argument: usize, out: &mut Vec<(usize, usize)>) {
    let mut depth = 0usize;
    let mut at = 1;
    for &j in args {
        let t = scan.tokens[j];
        if scan.is_punct(&t, '(') {
            depth += 1;
        } else if scan.is_punct(&t, ')') {
            if depth == 0 {
                return;
            }
            depth -= 1;
        } else if depth == 0 && scan.is_punct(&t, ',') {
            at += 1;
        } else if at == argument && is_literal(scan, &t) {
            keep(scan, t, out);
        }
    }
}

/// Adds the span of the literal `n` unless it is the mask already or in `out`.
fn keep(scan: &Scan, n: Token, out: &mut Vec<(usize, usize)>) {
    let span = (n.start, n.end);
    if scan.text(&n) != PASSWORD_MASK && !out.contains(&span) {
        out.push(span);
    }
}

/// The text of a literal as written (`N'p''w'`, `'p\'w'`, `$$p'w$$`): prefix and quotes removed,
/// doubled quotes undone, and backslash escapes undone when the reading has them. A dollar-quoted
/// body is the text between its tags, which takes no escapes and may hold quotes, and so is a
/// `‘…’` string.
fn unquoted(raw: &str, backslash: bool) -> String {
    if let Some(body) = dollar_body(raw) {
        return body.to_string();
    }
    if let Some(body) = raw.strip_prefix('\u{2018}') {
        return body.strip_suffix('\u{2019}').unwrap_or(body).to_string();
    }
    let chars: Vec<char> = raw.chars().collect();
    let Some(open) = chars.iter().position(|c| matches!(c, '\'' | '"')) else {
        return String::new();
    };
    let quote = chars[open];
    let inner = &chars[open + 1..chars.len().saturating_sub(1).max(open + 1)];
    let mut out = String::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let c = inner[i];
        if backslash && c == '\\' && i + 1 < inner.len() {
            out.push(inner[i + 1]);
            i += 2;
        } else if c == quote && inner.get(i + 1) == Some(&quote) {
            out.push(quote);
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// The body of a dollar-quoted literal (`$$pw$$`, `$a$pw$a$`; an unterminated one runs to the
/// end of the text).
fn dollar_body(raw: &str) -> Option<&str> {
    let tag_end = raw.strip_prefix('$')?.find('$')? + 2;
    let (tag, body) = raw.split_at(tag_end);
    Some(body.strip_suffix(tag).unwrap_or(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlgen::portable::testing::{ANSI_LEX, ANSI_TEST, ansi_with};

    #[test]
    fn masks_what_any_reading_finds() {
        let rules = ANSI_TEST.knobs.redact;
        // Read with backslash escapes the literal runs to the last quote; without, to `\'`.
        let r = redact(
            &ANSI_LEX,
            rules,
            r"ALTER USER u IDENTIFIED WITH sha2 BY 'x\'y' -- c",
        );
        assert_eq!(
            r.sql,
            "ALTER USER u IDENTIFIED WITH sha2 BY '********' -- c"
        );
        assert!(r.secrets.contains(&"x'y".to_string()));
        let r = redact(
            &ANSI_LEX,
            rules,
            "CREATE USER u PASSWORD = 'p''w', x 'keep'",
        );
        assert_eq!(r.sql, "CREATE USER u PASSWORD = '********', x 'keep'");
        assert_eq!(r.secrets, ["p'w", "'p''w'"]);
        let plain = "SELECT 'password' FROM t";
        assert_eq!(redact(&ANSI_LEX, rules, plain).sql, plain);
    }

    /// A server error may quote the password itself: a dollar-quoted one is recorded as the text
    /// between its tags, quotes and all.
    #[test]
    fn a_dollar_quoted_secret_is_its_body() {
        let rules = ANSI_TEST.knobs.redact;
        let r = redact(&ANSI_LEX, rules, "CREATE USER u PASSWORD = $$pw$$");
        assert_eq!(r.sql, "CREATE USER u PASSWORD = '********'");
        assert_eq!(r.secrets, ["pw", "$$pw$$"]);
        let r = redact(&ANSI_LEX, rules, "CREATE USER u PASSWORD = $$p'w$$");
        assert_eq!(r.secrets, ["p'w", "$$p'w$$"]);
        let r = redact(&ANSI_LEX, rules, "CREATE USER u PASSWORD = $a$x$$y\\$a$");
        assert_eq!(r.secrets, ["x$$y\\", "$a$x$$y\\$a$"]);
        // Unterminated, it runs to the end of the text.
        let r = redact(&ANSI_LEX, rules, "CREATE USER u PASSWORD = $$pw");
        assert_eq!(r.secrets, ["pw", "$$pw"]);
    }

    /// A command's password follows its first argument, and only where the command starts a
    /// statement: elsewhere its word is a name like any other.
    #[test]
    fn a_command_masks_its_argument_only_where_it_starts_a_statement() {
        let spec = ansi_with(|k| k.redact_commands = &["LOGIN"]);
        let r = redact_spec(&spec, "LOGIN alice 'se''cret'");
        assert_eq!(r.sql, "LOGIN alice '********'");
        assert_eq!(r.secrets, ["se'cret", "'se''cret'"]);
        assert_eq!(
            redact_spec(&spec, "SELECT 1; login 'alice' 'pw'").sql,
            "SELECT 1; login 'alice' '********'"
        );
        assert_eq!(
            redact_spec(&spec, "/* c */ LOGIN \"alice\" 'pw'").sql,
            "/* c */ LOGIN \"alice\" '********'"
        );
        for sql in [
            "SELECT * FROM t WHERE login = 'bob'",
            "SELECT login 'x' FROM t",
            "LOGIN alice",
            "LOGIN 'alice'",
            "LOGIN = 'x'",
        ] {
            assert_eq!(redact_spec(&spec, sql).sql, sql, "{sql}");
        }
        // The password keywords still count anywhere.
        assert_eq!(
            redact_spec(&spec, "SELECT 1; CREATE USER u PASSWORD 'x'").sql,
            "SELECT 1; CREATE USER u PASSWORD '********'"
        );
    }

    /// A named argument (`token := '…'`, `password => '…'`) keeps its password out too: the
    /// redactor used to stop at the `:` or the `>`.
    #[test]
    fn a_named_argument_is_masked() {
        let rules = ANSI_TEST.knobs.redact;
        for (sql, masked) in [
            (
                "SELECT serve('srv:1', password := 'pw1')",
                "SELECT serve('srv:1', password := '********')",
            ),
            (
                "CALL f(x => 1, password => 'pw2')",
                "CALL f(x => 1, password => '********')",
            ),
        ] {
            let r = redact(&ANSI_LEX, rules, sql);
            assert_eq!(r.sql, masked);
        }
    }

    /// ClickHouse reads `‘…’` as a string, which the profile can only list with the name quotes
    /// (its closing quote differs): after a password keyword it is the secret.
    #[test]
    fn a_unicode_quoted_string_is_masked() {
        let lex = LexProfile {
            ident_quotes: &[('"', '"'), ('\u{2018}', '\u{2019}')],
            ..ANSI_LEX
        };
        let r = redact(
            &lex,
            ANSI_TEST.knobs.redact,
            "CREATE USER u IDENTIFIED BY \u{2018}s'cret\u{2019}",
        );
        assert_eq!(r.sql, "CREATE USER u IDENTIFIED BY '********'");
        assert!(r.secrets.contains(&"s'cret".to_string()), "{:?}", r.secrets);
        // A double-quoted name is a name.
        let sql = "CREATE USER u IDENTIFIED WITH \"plugin\"";
        assert_eq!(redact(&lex, ANSI_TEST.knobs.redact, sql).sql, sql);
    }

    /// A function's secret argument is masked by position, whatever the arguments before it
    /// hold; other arguments and other functions keep their text.
    #[test]
    fn a_secret_argument_is_masked_by_position() {
        use crate::sqlgen::dialect::spec::RedactCall;
        let spec = ansi_with(|k| {
            k.redact_calls = &[RedactCall {
                function: "ENCRYPT",
                argument: 2,
            }]
        });
        for (sql, masked) in [
            (
                "SELECT encrypt('value', 'pass''phrase')",
                "SELECT encrypt('value', '********')",
            ),
            (
                "SELECT s.ENCRYPT(f('a', 'b'), 'k' || 'ey', 'aad') FROM t",
                "SELECT s.ENCRYPT(f('a', 'b'), '********' || '********', 'aad') FROM t",
            ),
            (
                "SELECT ENCRYPT (v, /* c */ 'k'), DECRYPT(v, 'x')",
                "SELECT ENCRYPT (v, /* c */ '********'), DECRYPT(v, 'x')",
            ),
        ] {
            assert_eq!(redact_spec(&spec, sql).sql, masked, "{sql}");
        }
        for sql in [
            "SELECT encrypt('value')",
            "SELECT encrypt",
            "SELECT 'encrypt(1, 2)'",
            "SELECT encrypt(v, k)",
        ] {
            assert_eq!(redact_spec(&spec, sql).sql, sql, "{sql}");
        }
        assert_eq!(
            redact_spec(&spec, "SELECT encrypt(v, 'k')").secrets,
            ["k", "'k'"]
        );
    }
}
