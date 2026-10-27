//! Keeps passwords out of the SQL trace for the extension dialects: the string literal after a
//! password keyword of the dialect (`SqlKnobs::redact`) becomes the mask the core redaction uses
//! (`sqlgen::admin::PASSWORD_MASK`). Statements are read the way the dialect's scanner reads them,
//! under every reading of backslashes; where the readings end a literal in different places, the
//! mask covers the longest of them (`'x\'y'` is masked whole, not only up to `\'`).

use crate::drivers::redact::Redaction;
use crate::sqlgen::admin::PASSWORD_MASK;
use crate::sqlgen::dialect::lex::{LexProfile, Scan, TokenKind, scan};
use crate::sqlgen::dialect::spec::RedactRule;

/// Words that may stand between a password keyword and its literal.
const LINKS: &[&str] = &["BY", "AS", "WITH", "USING"];

/// `sql` with the literals after `rules`' keywords masked. Fast when no keyword is mentioned.
pub fn redact(lex: &LexProfile, rules: &[RedactRule], sql: &str) -> Redaction {
    let mut redaction = Redaction {
        sql: sql.to_string(),
        secrets: Vec::new(),
    };
    if !rules.iter().any(|r| mentions(sql, r.keyword)) {
        return redaction;
    }
    let chars: Vec<char> = sql.chars().collect();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for &reading in lex.readings {
        let scan = scan(lex, reading, sql);
        for span in secret_literals(&scan, rules) {
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

/// The spans of the string literals that follow a keyword of `rules`. A literal that is already
/// the mask is left alone.
fn secret_literals(scan: &Scan, rules: &[RedactRule]) -> Vec<(usize, usize)> {
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
        if !rules.iter().any(|r| r.keyword == word) {
            continue;
        }
        let mut other_words = 0;
        for &j in &code[k + 1..] {
            let n = scan.tokens[j];
            match n.kind {
                TokenKind::Str => {
                    let span = (n.start, n.end);
                    if scan.text(&n) != PASSWORD_MASK && !out.contains(&span) {
                        out.push(span);
                    }
                    break;
                }
                TokenKind::Punct if scan.is_punct(&n, '=') || scan.is_punct(&n, '(') => {}
                TokenKind::Word if LINKS.contains(&scan.upper(&n).as_str()) => {}
                TokenKind::Word | TokenKind::Ident if other_words == 0 => other_words += 1,
                _ => break,
            }
        }
    }
    out.sort_unstable();
    out
}

/// The text of a literal as written (`N'p''w'`, `'p\'w'`): prefix and quotes removed, doubled
/// quotes undone, and backslash escapes undone when the reading has them.
fn unquoted(raw: &str, backslash: bool) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlgen::portable::testing::{ANSI_LEX, ANSI_TEST};

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
}
