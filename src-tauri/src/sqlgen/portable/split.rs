//! The statement splitter of the extension dialects, over the tokens of `dialect::lex::scan`.
//!
//! A `;` outside strings, quoted names, comments and dollar-quoted bodies ends a statement,
//! except inside a `BEGIN … END` block where the dialect has those (`begin_end_bodies`). A dialect
//! with a batch separator (T-SQL's `GO` on a line of its own) splits at the separator only: the
//! batch is what the server runs, and a `CREATE PROCEDURE` body ends with its batch, not at a
//! `;`. Offsets are character offsets; leading comments are not part of a statement, and empty or
//! comment-only statements are skipped, as in the core splitter.

use std::ops::Range;

use crate::sqlgen::dialect::lex::{
    ClassifyRules, LexProfile, PLAIN, Reading, Scan, TokenKind, scan,
};
use crate::sqlgen::portable::classify::one_statement;
use crate::sqlgen::split::SplitStatement;

/// Words after `BEGIN` that start a transaction rather than a block.
const TRANSACTION_WORDS: &[&str] = &[
    "TRANSACTION",
    "TRAN",
    "WORK",
    "DISTRIBUTED",
    "DEFERRED",
    "IMMEDIATE",
    "EXCLUSIVE",
    "NAME",
    "ISOLATION",
    "READ",
];

/// Words after `END` that close a construct whose opener the block depth does not count.
const END_OF_CONSTRUCT: &[&str] = &["IF", "LOOP", "WHILE", "REPEAT", "FOR"];

/// The reading splitting uses: the dialect's first.
pub fn first_reading(lex: &LexProfile) -> Reading {
    lex.readings.first().copied().unwrap_or(PLAIN)
}

/// Splits editor text into statements.
pub fn split(lex: &LexProfile, sql: &str) -> Vec<SplitStatement> {
    let scan = scan(lex, first_reading(lex), sql);
    // Statements come in order, so each line number goes on from the previous one's: counting
    // from the start every time made a long script quadratic.
    let mut lines = Lines { at: 0, line: 1 };
    statements(lex, &scan)
        .into_iter()
        .map(|r| statement(&scan, r, &mut lines))
        .collect()
}

/// The line a character offset is on, for offsets that only grow.
struct Lines {
    at: usize,
    line: u32,
}

impl Lines {
    fn of(&mut self, chars: &[char], offset: usize) -> u32 {
        let newlines = chars[self.at..offset]
            .iter()
            .filter(|&&c| c == '\n')
            .count();
        self.line += newlines as u32;
        self.at = offset;
        self.line
    }
}

/// Whether `sql` holds at most one statement under every reading of the dialect. Callers guard
/// with it (EXPLAIN wraps one statement, only a single read is cancelled, a board object runs
/// one), so an unsure answer is no: a batch or block counts its `;`-separated statements and
/// the ones a flow operator chains (`SHOW TABLES ->> SELECT …`), where terminators are optional a
/// word of `rules` that may start a second statement counts as one, and a comment a server may
/// end elsewhere (`Scan::ambiguous`) may hide one.
pub fn is_single_statement(lex: &LexProfile, rules: &ClassifyRules, sql: &str) -> bool {
    let readings = if lex.readings.is_empty() {
        &[PLAIN][..]
    } else {
        lex.readings
    };
    readings.iter().all(|&r| {
        let scan = scan(lex, r, sql);
        if scan.ambiguous {
            return false;
        }
        match statements(lex, &scan).as_slice() {
            [] => true,
            [one] => one_statement(lex, rules, &scan, one.clone()),
            _ => false,
        }
    })
}

/// The token ranges of the statements of `scan`: from the first token that is neither space nor
/// a comment to the last one before the terminator, trailing space left out.
pub fn statements(lex: &LexProfile, scan: &Scan) -> Vec<Range<usize>> {
    let toks = &scan.tokens;
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut depth = 0u32;
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        match t.kind {
            TokenKind::Space | TokenKind::Comment => {
                i += 1;
                continue;
            }
            TokenKind::Punct
                if scan.is_punct(&t, ';') && lex.batch_separator.is_none() && depth == 0 =>
            {
                finish(&mut out, scan, start.take(), i);
                i += 1;
                continue;
            }
            TokenKind::Word => {
                if let Some(sep) = lex.batch_separator
                    && let Some(next) = separator_line(scan, i, sep)
                {
                    finish(&mut out, scan, start.take(), i);
                    depth = 0;
                    i = next;
                    continue;
                }
                if lex.begin_end_bodies {
                    depth = block_depth(scan, i, depth);
                }
            }
            _ => {}
        }
        // A `;` with nothing before it starts no statement.
        if start.is_none() && !scan.is_punct(&t, ';') {
            start = Some(i);
        }
        i += 1;
    }
    finish(&mut out, scan, start, toks.len());
    out
}

fn finish(out: &mut Vec<Range<usize>>, scan: &Scan, start: Option<usize>, end: usize) {
    let Some(start) = start else {
        return;
    };
    let mut end = end;
    while end > start && scan.tokens[end - 1].kind == TokenKind::Space {
        end -= 1;
    }
    if end > start {
        out.push(start..end);
    }
}

/// The statement the token range `r` spans (ranges in order, as `statements` gives them).
fn statement(scan: &Scan, r: Range<usize>, lines: &mut Lines) -> SplitStatement {
    let start = scan.tokens[r.start].start;
    let end = scan.tokens[r.end - 1].end;
    let line = lines.of(&scan.chars, start);
    SplitStatement {
        sql: scan.chars[start..end].iter().collect(),
        start,
        end,
        line,
    }
}

/// When the word at `i` is the batch separator on a line of its own (blanks, an optional repeat
/// count and an optional `--` comment around it), the index of the first token after that line.
fn separator_line(scan: &Scan, i: usize, separator: &str) -> Option<usize> {
    let t = scan.tokens[i];
    if !scan.text(&t).eq_ignore_ascii_case(separator) {
        return None;
    }
    let chars = &scan.chars;
    let mut j = t.start;
    while j > 0 {
        match chars[j - 1] {
            '\n' => break,
            ' ' | '\t' => j -= 1,
            _ => return None,
        }
    }
    let blanks = |mut k: usize| {
        while chars.get(k).is_some_and(|&c| c == ' ' || c == '\t') {
            k += 1;
        }
        k
    };
    let mut k = blanks(t.end);
    while chars.get(k).is_some_and(char::is_ascii_digit) {
        k += 1;
    }
    k = blanks(k);
    if chars.get(k) == Some(&'-') && chars.get(k + 1) == Some(&'-') {
        while chars.get(k).is_some_and(|&c| c != '\n' && c != '\r') {
            k += 1;
        }
    }
    if chars.get(k) == Some(&'\r') {
        k += 1;
    }
    match chars.get(k) {
        None => {}
        Some('\n') => k += 1,
        Some(_) => return None,
    }
    Some(scan.tokens.partition_point(|t| t.start < k))
}

/// The `BEGIN … END` depth after the word at `i`.
fn block_depth(scan: &Scan, i: usize, depth: u32) -> u32 {
    if previous(scan, i).is_some_and(|p| scan.is_punct(&scan.tokens[p], '.')) {
        return depth; // a qualified name: `t.end`
    }
    match scan.upper(&scan.tokens[i]).as_str() {
        "BEGIN" => {
            let opens = match next(scan, i).map(|n| scan.tokens[n]) {
                None => false,
                Some(n) if scan.is_punct(&n, ';') => false,
                Some(n) => {
                    !(n.kind == TokenKind::Word
                        && TRANSACTION_WORDS.contains(&scan.upper(&n).as_str()))
                }
            };
            depth + u32::from(opens)
        }
        "CASE" if depth > 0 => depth + 1,
        "END" if depth > 0 => {
            let closes_construct = next(scan, i).is_some_and(|n| {
                scan.tokens[n].kind == TokenKind::Word
                    && END_OF_CONSTRUCT.contains(&scan.upper(&scan.tokens[n]).as_str())
            });
            if closes_construct { depth } else { depth - 1 }
        }
        _ => depth,
    }
}

/// The next token after `i` that is neither space nor a comment.
fn next(scan: &Scan, i: usize) -> Option<usize> {
    (i + 1..scan.tokens.len())
        .find(|&j| !matches!(scan.tokens[j].kind, TokenKind::Space | TokenKind::Comment))
}

fn previous(scan: &Scan, i: usize) -> Option<usize> {
    (0..i)
        .rev()
        .find(|&j| !matches!(scan.tokens[j].kind, TokenKind::Space | TokenKind::Comment))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlgen::portable::testing::ANSI_LEX;

    #[test]
    fn long_scripts_split_in_linear_time_with_their_lines() {
        let script = "SELECT 1;\n".repeat(40_000);
        let started = std::time::Instant::now();
        let parts = split(&ANSI_LEX, &script);
        // Counting each line from the start took about a minute here in a debug build.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(parts.len(), 40_000);
        assert_eq!((parts[0].line, parts[39_999].line), (1, 40_000));
        let lines: Vec<u32> = split(
            &ANSI_LEX,
            "SELECT 1;\n\n  SELECT 2; SELECT 3;\n-- c\nSELECT 4",
        )
        .iter()
        .map(|s| s.line)
        .collect();
        assert_eq!(lines, [1, 3, 3, 5]);
    }
}
