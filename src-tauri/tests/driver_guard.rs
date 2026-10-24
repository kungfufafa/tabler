//! Guard of the driver lanes (G3). The drivers beyond PostgreSQL, MySQL and SQLite are named only
//! where naming them is the point: their descriptors (`types/driver.rs`), the `Driver` enum
//! (`types/profile.rs`), the registry, their own lane code (the `rust` globs of `dev/lanes.json`)
//! and tests. Elsewhere code reads a descriptor value instead, so a new driver needs no edit
//! there. Shared code also must not grow new `== Driver::…` / `!= Driver::…` /
//! `matches!(…, Driver::…)` compares: their count per file stays at or below
//! `tests/fixtures/guard_baseline.json` (`TABLER_BLESS=1` rewrites it after a reviewed change).
//!
//! Only production code counts: comments, string literals and `#[cfg(test)]` items (test modules,
//! inline or in their own files, and single test-only items) are left out.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::Regex;

/// The `Driver` variants of the lane drivers.
const LANE_DRIVERS: [&str; 13] = [
    "Sqlserver",
    "Libsql",
    "D1",
    "Duckdb",
    "Clickhouse",
    "Snowflake",
    "Bigquery",
    "Vertica",
    "Cassandra",
    "Redis",
    "Mongodb",
    "Elasticsearch",
    "Dynamodb",
];

/// Shared files that may name every driver.
const NAMES_DRIVERS: [&str; 3] = [
    "src/drivers/registry.rs",
    "src/types/driver.rs",
    "src/types/profile.rs",
];

const BASELINE: &str = "tests/fixtures/guard_baseline.json";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The `rust` globs of every lane, relative to `src-tauri`.
fn lane_globs() -> Vec<String> {
    let text = std::fs::read_to_string(root().join("../dev/lanes.json")).expect("dev/lanes.json");
    let json: serde_json::Value = serde_json::from_str(&text).expect("lanes.json is JSON");
    let lanes = json["lanes"].as_object().expect("a lanes object");
    assert_eq!(lanes.len(), 13, "13 lanes");
    lanes
        .values()
        .flat_map(|lane| lane["rust"].as_array().expect("rust globs").iter())
        .map(|g| {
            let g = g.as_str().expect("a glob");
            g.strip_prefix("src-tauri/")
                .unwrap_or_else(|| panic!("{g} is under src-tauri"))
                .to_string()
        })
        .collect()
}

fn matches_glob(path: &str, glob: &str) -> bool {
    match glob.strip_suffix("/**") {
        Some(dir) => path.starts_with(&format!("{dir}/")),
        None => path == glob,
    }
}

/// Every `.rs` file under `src`, as `src/…` paths with `/`.
fn source_files() -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("readable source dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&root().join("src"), &mut files);
    let mut out: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(root())
                .expect("under the crate")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    out.sort();
    out
}

/// `text` with comments and the contents of string and char literals replaced by spaces
/// (newlines kept, so line numbers stay right).
fn code_only(text: &str) -> Vec<char> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = chars.clone();
    let blank = |out: &mut Vec<char>, from: usize, to: usize| {
        for c in &mut out[from..to] {
            if *c != '\n' {
                *c = ' ';
            }
        }
    };
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            let end = chars[i..]
                .iter()
                .position(|c| *c == '\n')
                .map_or(chars.len(), |n| i + n);
            blank(&mut out, i, end);
            i = end;
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            let mut j = i;
            while j < chars.len() {
                if chars[j] == '/' && chars.get(j + 1) == Some(&'*') {
                    depth += 1;
                    j += 2;
                } else if chars[j] == '*' && chars.get(j + 1) == Some(&'/') {
                    depth -= 1;
                    j += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            blank(&mut out, i, j);
            i = j;
        } else if (c == 'r' || (c == 'b' && next == Some('r')))
            && (i == 0 || !ident(chars[i - 1]))
            && {
                let start = if c == 'b' { i + 2 } else { i + 1 };
                let hashes = chars[start..].iter().take_while(|c| **c == '#').count();
                chars.get(start + hashes) == Some(&'"')
            }
        {
            // A raw string: r"…", r#"…"#, br"…".
            let start = if c == 'b' { i + 2 } else { i + 1 };
            let hashes = chars[start..].iter().take_while(|c| **c == '#').count();
            let body = start + hashes + 1;
            let mut j = body;
            while j < chars.len() {
                if chars[j] == '"' && chars[j + 1..].iter().take(hashes).all(|c| *c == '#') {
                    break;
                }
                j += 1;
            }
            let end = (j + 1 + hashes).min(chars.len());
            blank(&mut out, body, j.min(chars.len()));
            i = end;
        } else if c == '"' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '"' {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            blank(&mut out, i + 1, j.min(chars.len()));
            i = j + 1;
        } else if c == '\'' {
            // A char literal ('x', '\n', '\u{201c}'), else a lifetime or a label.
            let end = if next == Some('\\') {
                // Past the escaped character, which may be a quote itself ('\'').
                chars
                    .get(i + 3..)
                    .and_then(|rest| rest.iter().position(|c| *c == '\''))
                    .map(|n| i + 3 + n)
            } else if chars.get(i + 2) == Some(&'\'') {
                Some(i + 2)
            } else {
                None
            };
            match end {
                Some(end) => {
                    blank(&mut out, i + 1, end);
                    i = end + 1;
                }
                None => i += 1,
            }
        } else {
            i += 1;
        }
    }
    out
}

/// The index after the bracketed group that opens at `open` (`(`, `[` or `{`).
fn group_end(code: &[char], open: usize) -> usize {
    let (o, c) = match code[open] {
        '(' => ('(', ')'),
        '[' => ('[', ']'),
        _ => ('{', '}'),
    };
    let mut depth = 0;
    for (j, ch) in code.iter().enumerate().skip(open) {
        if *ch == o {
            depth += 1;
        } else if *ch == c {
            depth -= 1;
            if depth == 0 {
                return j + 1;
            }
        }
    }
    code.len()
}

/// Test-only code removed from `code` (as `code_only` returns it): each item under
/// `#[cfg(test)]` or `#[cfg(all(test, …))]` (a module, a function, a `use`…) up to its closing
/// brace or semicolon. Returns the names of out-of-line test modules (`#[cfg(test)] mod x;`).
fn strip_tests(code: &mut [char]) -> Vec<String> {
    let text: String = code.iter().collect();
    let attr = Regex::new(r"#\[cfg\((test|all\(\s*test\b[^\]]*)\)\]").unwrap();
    // Byte offsets to char offsets.
    let starts: Vec<usize> = attr
        .find_iter(&text)
        .map(|m| text[..m.start()].chars().count())
        .collect();
    let module = Regex::new(r"^(pub(\([^)]*\))?\s+)?mod\s+(\w+)\s*;$").unwrap();
    let mut out_of_line = Vec::new();
    for start in starts.into_iter().rev() {
        if code[start] != '#' {
            continue; // inside an item removed already
        }
        // Past this attribute and any others on the item.
        let mut j = start;
        while j < code.len() {
            if code[j].is_whitespace() {
                j += 1;
            } else if code[j] == '#' && code.get(j + 1) == Some(&'[') {
                j = group_end(code, j + 1);
            } else {
                break;
            }
        }
        let item_start = j;
        let mut end = code.len();
        while j < code.len() {
            match code[j] {
                '(' | '[' => j = group_end(code, j),
                ';' => {
                    end = j + 1;
                    break;
                }
                '{' => {
                    end = group_end(code, j);
                    break;
                }
                _ => j += 1,
            }
        }
        let item: String = code[item_start..end].iter().collect();
        if let Some(m) = module.captures(item.trim()) {
            out_of_line.push(m[3].to_string());
        }
        for c in &mut code[start..end] {
            if *c != '\n' {
                *c = ' ';
            }
        }
    }
    out_of_line
}

/// Production code of every source file (test files and test items removed), by path.
fn production_sources() -> BTreeMap<String, Vec<char>> {
    let files = source_files();
    let mut code = BTreeMap::new();
    let mut test_only: Vec<String> = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(root().join(path)).expect("readable source");
        let mut c = code_only(&text);
        // `x/mod.rs` and `lib.rs` declare modules in `x/`; `x/y.rs` in `x/y/`.
        let dir = match path.rsplit_once('/') {
            Some((dir, "mod.rs" | "lib.rs")) => dir.to_string(),
            _ => path.trim_end_matches(".rs").to_string(),
        };
        for module in strip_tests(&mut c) {
            test_only.push(format!("{dir}/{module}.rs"));
            test_only.push(format!("{dir}/{module}/**"));
        }
        code.insert(path.clone(), c);
    }
    code.retain(|path, _| !test_only.iter().any(|t| matches_glob(path, t)));
    code
}

fn line_of(code: &[char], at: usize) -> usize {
    code[..at].iter().filter(|c| **c == '\n').count() + 1
}

/// The `Driver::…` compares of `code`: `==` / `!=` on either side, and `matches!` holding one.
fn driver_compares(text: &str) -> usize {
    let compare =
        Regex::new(r"[=!]=\s*(\w+\s*::\s*)*Driver\s*::\s*\w+|\bDriver\s*::\s*\w+\s*[=!]=").unwrap();
    let mut n = compare.find_iter(text).count();
    let chars: Vec<char> = text.chars().collect();
    let macro_ = Regex::new(r"\bmatches!\s*\(").unwrap();
    let driver = Regex::new(r"\bDriver\s*::").unwrap();
    for m in macro_.find_iter(text) {
        let open = text[..m.end()].chars().count() - 1;
        let inner: String = chars[open..group_end(&chars, open)].iter().collect();
        if driver.is_match(&inner) {
            n += 1;
        }
    }
    n
}

fn shared(path: &str, lanes: &[String]) -> bool {
    !lanes.iter().any(|g| matches_glob(path, g))
}

#[test]
fn lane_drivers_are_named_only_in_their_places() {
    let lanes = lane_globs();
    let named = Regex::new(&format!(r"\bDriver\s*::\s*({})\b", LANE_DRIVERS.join("|"))).unwrap();
    let mut found = Vec::new();
    for (path, code) in production_sources() {
        if NAMES_DRIVERS.contains(&path.as_str()) || !shared(&path, &lanes) {
            continue;
        }
        let text: String = code.iter().collect();
        for m in named.find_iter(&text) {
            let at = text[..m.start()].chars().count();
            found.push(format!("{path}:{} {}", line_of(&code, at), m.as_str()));
        }
    }
    assert!(
        found.is_empty(),
        "lane drivers named in shared code (read a descriptor value instead):\n{}",
        found.join("\n")
    );
}

#[test]
fn shared_code_grows_no_driver_compares() {
    let lanes = lane_globs();
    let mut counts = BTreeMap::new();
    for (path, code) in production_sources() {
        if !shared(&path, &lanes) {
            continue;
        }
        let n = driver_compares(&code.iter().collect::<String>());
        if n > 0 {
            counts.insert(path, n);
        }
    }
    let path = root().join(BASELINE);
    if std::env::var_os("TABLER_BLESS").is_some() {
        let json = serde_json::json!({ "driverCompares": counts });
        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap() + "\n").unwrap();
        return;
    }
    let baseline: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("the baseline")).unwrap();
    let allowed = |file: &str| baseline["driverCompares"][file].as_u64().unwrap_or(0) as usize;
    let grown: Vec<String> = counts
        .iter()
        .filter(|(file, n)| **n > allowed(file))
        .map(|(file, n)| format!("{file}: {n} (baseline {})", allowed(file)))
        .collect();
    assert!(
        grown.is_empty(),
        "new Driver compares in shared code (read a descriptor value instead, or bless a reviewed \
         change with TABLER_BLESS=1):\n{}",
        grown.join("\n")
    );
}

#[test]
fn every_lane_driver_has_its_globs() {
    let lanes = lane_globs();
    // Each lane module of `drivers/` belongs to a lane.
    for module in [
        "sqlserver",
        "sqlite_http",
        "duckdb",
        "clickhouse",
        "snowflake",
        "bigquery",
        "vertica",
        "cassandra",
        "redis",
        "mongodb",
        "elasticsearch",
        "dynamodb",
    ] {
        let path = format!("src/drivers/{module}/entry.rs");
        let path = if module == "sqlite_http" {
            "src/drivers/sqlite_http/d1/entry.rs".to_string()
        } else {
            path
        };
        assert!(root().join(&path).is_file(), "{path}");
        assert!(!shared(&path, &lanes), "{path} is in a lane");
    }
    assert!(!shared("src/drivers/postgres/flavor/imp/mod.rs", &lanes));
    // The core drivers and the shared seam are not.
    for path in [
        "src/drivers/postgres/mod.rs",
        "src/drivers/postgres/flavor/mod.rs",
        "src/drivers/registry.rs",
        "src/sqlgen/select.rs",
    ] {
        assert!(shared(path, &lanes), "{path}");
    }
}

#[test]
fn test_code_is_left_out() {
    let source = r#"
pub fn real() -> Driver { Driver::Postgres } // Driver::Redis in a comment
const S: &str = "Driver::Redis in a string";
const C: char = '{';
impl Thing {
    #[cfg(test)]
    pub fn only_in_tests(&self, a: [u8; 2]) -> bool {
        self.driver == Driver::Redis
    }

    pub fn register(&self) -> bool { self.driver == Driver::Mysql }
}
#[cfg(all(test, windows))]
use crate::Driver::Mongodb;
#[cfg(test)]
mod tests {
    fn t() { let x = Driver::Duckdb; if x == Driver::D1 {} }
}
#[cfg(test)]
mod golden;
fn after<'a>(s: &'a str) -> bool { matches!(d, Driver::Sqlite | Driver::Mysql) }
"#;
    let mut code = code_only(source);
    let modules = strip_tests(&mut code);
    let text: String = code.iter().collect();
    assert_eq!(modules, ["golden"]);
    assert_eq!(text.lines().count(), source.lines().count());
    for name in ["Redis", "Mongodb", "Duckdb", "D1"] {
        assert!(!text.contains(&format!("Driver::{name}")), "{name}: {text}");
    }
    assert!(text.contains("pub fn register"));
    assert!(text.contains("fn after"));
    assert_eq!(driver_compares(&text), 2);
}
