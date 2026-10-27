//! Generated cases of Vertica SQL (`dialect::vertica`, fixture
//! `tests/fixtures/classify-generated/vertica.json`).
//!
//! A skeleton: the vertica lane adds the contexts only its grammar has, a corpus of ordinary
//! reads of the engine, its write forms and fuzz atoms (see `ansi.rs`), then replaces the
//! placeholder with `suite().check()` and `suite().fuzz(…)`.

use super::Suite;
use crate::sqlgen::dialect::vertica::SPEC;

fn suite() -> Suite {
    Suite::new("vertica", &SPEC)
}

/// Placeholder: the shared contexts and the fuzzer over the rules' words only.
#[test]
fn placeholder() {
    let suite = suite();
    suite.check_contexts();
    suite.fuzz(500);
}
