//! Guards of the SQL dialects (G1, G2). `sqlgen::Dialect` is the closed set of the core
//! generators; `SqlDialect` adds the dialects of the other drivers, and shared code must never
//! send one of those into a core generator's "else" branch:
//!
//! - G1: a `SqlDialect` variant (`SqlDialect::Tsql`…) and `SqlDialect::core()` appear only where
//!   naming them is the point: the dialect layer (`sqlgen/dialect/**`, the `sqlgen::sql`
//!   facade), the descriptor tables (`types/driver.rs`, `types/caps.rs`), the code of the drivers
//!   that speak one dialect (the core three and the lanes) and `DbSession::core_dialect`. The
//!   portable builders may ask for `core()` but name no variant (a spec carries its dialect).
//!   Elsewhere shared code goes through the facade, `core_dialect(Feature)` or `resolve()`.
//! - G2: a `match` on `….resolve()` names its arms: a catch-all (`_ =>`, or a bare binding) needs
//!   the comment `// generic: safe for every dialect` on its line or the line above.
//!
//! Only production code counts (see `guard::production_sources`); tests may name any dialect.

mod guard;

use regex::Regex;

use guard::{code_only, group_end, lane_globs, line_of, matches_glob, production_sources, root};

/// Where variants and `core()` may appear (besides the lanes' own files).
const DIALECT_PLACES: [&str; 7] = [
    "src/sqlgen/dialect/**",
    "src/sqlgen/sql.rs",
    "src/types/driver.rs",
    "src/types/caps.rs",
    "src/drivers/postgres/**",
    "src/drivers/mysql/**",
    "src/drivers/sqlite/**",
];

/// The portable builders: `core()` yes, variants no.
const PORTABLE: &str = "src/sqlgen/portable/**";

/// The file of `DbSession::core_dialect`, whose `fn core_dialect…` items may call `core()`.
const CORE_DIALECT_FILE: &str = "src/drivers/mod.rs";

/// The comment that lets a catch-all arm of a `resolve()` match stand.
const GENERIC: &str = "// generic: safe for every dialect";

fn variant_re() -> Regex {
    Regex::new(r"\bSqlDialect\s*::\s*[A-Z]").unwrap()
}

fn core_re() -> Regex {
    Regex::new(r"\.\s*core\s*\(\s*\)").unwrap()
}

/// Char offset of a byte offset of `text`.
fn char_at(text: &str, byte: usize) -> usize {
    text[..byte].chars().count()
}

/// The spans (char offsets) of the bodies of the `fn core_dialect…` items of `code`.
fn core_dialect_bodies(code: &[char]) -> Vec<(usize, usize)> {
    let text: String = code.iter().collect();
    let item = Regex::new(r"\bfn\s+core_dialect\w*").unwrap();
    item.find_iter(&text)
        .filter_map(|m| {
            let from = char_at(&text, m.end());
            let open = (from..code.len()).find(|&i| code[i] == '{')?;
            Some((open, group_end(code, open)))
        })
        .collect()
}

/// G1's findings in one production file (`code` as `production_sources` returns it).
fn misplaced_dialects(path: &str, code: &[char], lanes: &[String]) -> Vec<String> {
    if DIALECT_PLACES.iter().any(|g| matches_glob(path, g))
        || lanes.iter().any(|g| matches_glob(path, g))
    {
        return Vec::new();
    }
    let text: String = code.iter().collect();
    let mut found = Vec::new();
    for m in variant_re().find_iter(&text) {
        let at = char_at(&text, m.start());
        found.push(format!("{path}:{} {}", line_of(code, at), m.as_str()));
    }
    if !matches_glob(path, PORTABLE) {
        let allowed = if path == CORE_DIALECT_FILE {
            core_dialect_bodies(code)
        } else {
            Vec::new()
        };
        for m in core_re().find_iter(&text) {
            let at = char_at(&text, m.start());
            if !allowed.iter().any(|&(from, to)| from <= at && at < to) {
                found.push(format!("{path}:{} .core()", line_of(code, at)));
            }
        }
    }
    found
}

/// The patterns (text, char offset of their start) of the top-level arms of the match body
/// `code[open..close]` (`open` is its `{`).
fn arm_patterns(code: &[char], open: usize, close: usize) -> Vec<(String, usize)> {
    let mut arms = Vec::new();
    let mut depth = 0;
    let mut start = open + 1;
    let mut i = open + 1;
    while i < close - 1 {
        match code[i] {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' => depth -= 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    start = i + 1; // the end of a block arm
                }
            }
            ',' if depth == 0 => start = i + 1,
            '=' if depth == 0 && code.get(i + 1) == Some(&'>') => {
                let pattern: String = code[start..i].iter().collect();
                let skipped = pattern.len() - pattern.trim_start().len();
                arms.push((pattern.trim().to_string(), start + skipped));
                start = i + 2;
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    arms
}

/// G2's findings in one file: catch-all arms of `match ….resolve()` without the comment.
/// `code` is the production code, `raw` the file as written (for the comments).
fn unnamed_arms(path: &str, code: &[char], raw: &str) -> Vec<String> {
    let text: String = code.iter().collect();
    let raw_lines: Vec<&str> = raw.lines().collect();
    let keyword = Regex::new(r"\bmatch\b").unwrap();
    let resolve = Regex::new(r"\.\s*resolve\s*\(\s*\)").unwrap();
    let catch_all = Regex::new(r"^(_|[a-z_][a-z0-9_]*)$").unwrap();
    let mut found = Vec::new();
    for m in keyword.find_iter(&text) {
        // The scrutinee runs to the `{` that opens the body.
        let mut i = char_at(&text, m.end());
        while i < code.len() && code[i] != '{' && code[i] != ';' {
            i = if matches!(code[i], '(' | '[') {
                group_end(code, i)
            } else {
                i + 1
            };
        }
        if i >= code.len() || code[i] != '{' {
            continue;
        }
        let scrutinee: String = code[char_at(&text, m.end())..i].iter().collect();
        if !resolve.is_match(&scrutinee) {
            continue;
        }
        for (pattern, at) in arm_patterns(code, i, group_end(code, i)) {
            if !catch_all.is_match(&pattern) {
                continue;
            }
            let line = line_of(code, at);
            let commented = [line, line.saturating_sub(1)]
                .iter()
                .filter_map(|&l| raw_lines.get(l.wrapping_sub(1)))
                .any(|l| l.contains(GENERIC));
            if !commented {
                found.push(format!("{path}:{line} `{pattern} =>`"));
            }
        }
    }
    found
}

#[test]
fn sql_dialects_are_named_only_in_their_places() {
    let lanes = lane_globs();
    let found: Vec<String> = production_sources()
        .iter()
        .flat_map(|(path, code)| misplaced_dialects(path, code, &lanes))
        .collect();
    assert!(
        found.is_empty(),
        "SqlDialect variants or core() in shared code (use the sqlgen::sql facade, \
         session.core_dialect(Feature) or resolve() instead):\n{}",
        found.join("\n")
    );
}

#[test]
fn resolve_matches_name_their_arms() {
    let mut found = Vec::new();
    for (path, code) in production_sources() {
        let raw = std::fs::read_to_string(root().join(&path)).expect("readable source");
        found.extend(unnamed_arms(&path, &code, &raw));
    }
    assert!(
        found.is_empty(),
        "catch-all arms of a match on resolve() (name the Core and Ext arms, or mark an arm \
         that fits every dialect with `{GENERIC}`):\n{}",
        found.join("\n")
    );
}

#[test]
fn the_guards_read_the_code_as_the_compiler_does() {
    let lanes = lane_globs();
    let source = r#"
use crate::sqlgen::SqlDialect;
fn shared(d: SqlDialect) -> bool { d == SqlDialect::Tsql } // SqlDialect::Cql in a comment
const S: &str = "SqlDialect::Duckdb and .core() in a string";
fn core_dialect_for(d: SqlDialect) -> Option<Dialect> { d.core() }
fn other(d: SqlDialect) -> Option<Dialect> { d . core ( ) }
fn named(d: SqlDialect) -> &'static str {
    match d.into().resolve() {
        DialectRef::Core(Dialect::Mysql) => "m",
        DialectRef::Core(_) => "c",
        DialectRef::Ext(spec) => spec.label,
    }
}
fn wild(d: SqlDialect) -> u8 {
    match (d.resolve()) {
        DialectRef::Core(Dialect::Mysql) => { if true { 1 } else { 2 } }
        _ => 0,
    }
}
fn bound(d: SqlDialect) -> u8 {
    match d.resolve() {
        DialectRef::Ext(_) => 1,
        // generic: safe for every dialect
        other => 2,
    }
}
fn guarded(d: SqlDialect, b: bool) -> u8 {
    match d.resolve() {
        _ if b => 1,
        DialectRef::Core(_) | DialectRef::Ext(_) => 2,
    }
}
fn not_resolve(n: u8) -> u8 { match n { _ => 0 } }
#[cfg(test)]
mod tests {
    fn t(d: SqlDialect) { let _ = SqlDialect::Vertica; d.core(); match d.resolve() { _ => {} } }
}
"#;
    let mut code = code_only(source);
    guard::strip_tests(&mut code);
    let lines = |found: Vec<String>| -> Vec<String> {
        found
            .into_iter()
            .map(|f| f.split_once(':').unwrap().1.to_string())
            .collect()
    };
    // In shared code: the variant and both core() calls.
    assert_eq!(
        lines(misplaced_dialects("src/commands/x.rs", &code, &lanes)),
        ["3 SqlDialect::T", "5 .core()", "6 .core()"]
    );
    // drivers/mod.rs: core() only inside `fn core_dialect…`.
    assert_eq!(
        lines(misplaced_dialects(CORE_DIALECT_FILE, &code, &lanes)),
        ["3 SqlDialect::T", "6 .core()"]
    );
    // The portable builders: core() yes, variants no.
    assert_eq!(
        lines(misplaced_dialects(
            "src/sqlgen/portable/x.rs",
            &code,
            &lanes
        )),
        ["3 SqlDialect::T"]
    );
    for path in [
        "src/sqlgen/dialect/tsql.rs",
        "src/sqlgen/sql.rs",
        "src/types/caps.rs",
        "src/drivers/sqlite/mod.rs",
        "src/drivers/sqlserver/mod.rs",
    ] {
        assert!(misplaced_dialects(path, &code, &lanes).is_empty(), "{path}");
    }
    // G2: the unmarked `_ =>` only (not the guarded one, a commented binding or another match).
    assert_eq!(
        lines(unnamed_arms("src/commands/x.rs", &code, source)),
        ["17 `_ =>`"]
    );
}
