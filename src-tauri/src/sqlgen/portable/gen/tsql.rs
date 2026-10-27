//! Generated cases of Transact-SQL (`dialect::tsql`, fixture
//! `tests/fixtures/classify-generated/tsql.json`). `TSQL_LIKE_TEST`, the always-built spec the
//! renderer's T-SQL cases run against, has its suite in `tsql_like.rs`.
//!
//! A skeleton: the sqlserver lane adds the contexts only T-SQL has (`GO` batches, statements
//! with no terminator, …), a corpus of ordinary reads of the engine, its write forms and fuzz
//! atoms (see `ansi.rs`), then replaces the placeholder with `suite().check()` and
//! `suite().fuzz(…)`.

use super::Suite;
use crate::sqlgen::dialect::tsql::SPEC;

fn suite() -> Suite {
    Suite::new("tsql", &SPEC)
}

/// Placeholder: the shared contexts and the fuzzer over the rules' words only.
#[test]
fn placeholder() {
    let suite = suite();
    suite.check_contexts();
    suite.fuzz(500);
}
