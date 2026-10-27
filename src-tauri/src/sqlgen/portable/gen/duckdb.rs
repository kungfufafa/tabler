//! Generated cases of DuckDB SQL (`dialect::duckdb`, fixture
//! `tests/fixtures/classify-generated/duckdb.json`).
//!
//! A skeleton: the duckdb lane adds the contexts only its grammar has, a corpus of ordinary
//! reads of the engine, its write forms and fuzz atoms (see `ansi.rs`), then replaces the
//! placeholder with `suite().check()` and `suite().fuzz(…)`.

use super::Suite;
use crate::sqlgen::dialect::duckdb::SPEC;

fn suite() -> Suite {
    Suite::new("duckdb", &SPEC)
}

/// Placeholder: the shared contexts and the fuzzer over the rules' words only.
#[test]
fn placeholder() {
    let suite = suite();
    suite.check_contexts();
    suite.fuzz(500);
}
