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

mod guard;

use std::collections::BTreeMap;

use regex::Regex;

use guard::{
    code_only, group_end, lane_globs, line_of, production_sources, root, shared, strip_tests,
};

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
