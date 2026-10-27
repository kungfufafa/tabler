//! Generated cases of GoogleSQL (`dialect::bigquery`, fixture
//! `tests/fixtures/classify-generated/bigquery.json`).
//!
//! A skeleton: the bigquery lane adds the contexts only its grammar has, a corpus of ordinary
//! reads of the engine, its write forms and fuzz atoms (see `ansi.rs`), then replaces the
//! placeholder with `suite().check()` and `suite().fuzz(…)`.

use super::Suite;
use crate::sqlgen::dialect::bigquery::SPEC;

fn suite() -> Suite {
    Suite::new("bigquery", &SPEC)
}

/// Placeholder: the shared contexts and the fuzzer over the rules' words only.
#[test]
fn placeholder() {
    let suite = suite();
    suite.check_contexts();
    suite.fuzz(500);
}
