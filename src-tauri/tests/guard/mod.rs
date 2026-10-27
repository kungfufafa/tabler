//! What the source guards (`driver_guard.rs`, `dialect_guard.rs`) share: the crate's source
//! files, their production code (comments, string literals and test-only items blanked) and the
//! lane globs of `dev/lanes.json`.

// Each guard uses only some of these.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::Regex;

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The `rust` globs of every lane, relative to `src-tauri`.
pub fn lane_globs() -> Vec<String> {
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

pub fn matches_glob(path: &str, glob: &str) -> bool {
    match glob.strip_suffix("/**") {
        Some(dir) => path.starts_with(&format!("{dir}/")),
        None => path == glob,
    }
}

/// Every `.rs` file under `src`, as `src/…` paths with `/`.
pub fn source_files() -> Vec<String> {
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
pub fn code_only(text: &str) -> Vec<char> {
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
pub fn group_end(code: &[char], open: usize) -> usize {
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
pub fn strip_tests(code: &mut [char]) -> Vec<String> {
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

/// The files of the test-only modules `text` declares with a `#[path]` (`#[cfg(test)]
/// #[path = "gen/mod.rs"] mod generated;`), relative to its directory: `code_only` blanks the
/// path, so `strip_tests` only sees the module's name.
fn test_module_paths(text: &str) -> Vec<String> {
    let declaration =
        Regex::new(r"((?:#\[[^\]]*\]\s*)+)(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*;").unwrap();
    let test = Regex::new(r"#\[cfg\((test|all\(\s*test\b)").unwrap();
    let path = Regex::new(r#"#\[path\s*=\s*"([^"]+)"\]"#).unwrap();
    declaration
        .captures_iter(text)
        .filter(|c| test.is_match(&c[1]))
        .filter_map(|c| path.captures(&c[1]).map(|p| p[1].to_string()))
        .map(|p| match p.strip_suffix("/mod.rs") {
            Some(dir) => format!("{dir}/**"),
            None => p,
        })
        .collect()
}

/// Production code of every source file (test files and test items removed), by path.
pub fn production_sources() -> BTreeMap<String, Vec<char>> {
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
        // A `#[path]` is relative to the declaring file's own directory.
        let own_dir = path.rsplit_once('/').map_or("", |(d, _)| d);
        for file in test_module_paths(&text) {
            test_only.push(format!("{own_dir}/{file}"));
        }
        code.insert(path.clone(), c);
    }
    code.retain(|path, _| !test_only.iter().any(|t| matches_glob(path, t)));
    code
}

pub fn line_of(code: &[char], at: usize) -> usize {
    code[..at].iter().filter(|c| **c == '\n').count() + 1
}

/// Whether `path` is shared code: in no lane's globs.
pub fn shared(path: &str, lanes: &[String]) -> bool {
    !lanes.iter().any(|g| matches_glob(path, g))
}
