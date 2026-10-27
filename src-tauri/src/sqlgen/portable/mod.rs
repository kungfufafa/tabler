//! The SQL builders of the extension dialects, driven by their `DialectSpec`: browse SELECT and
//! COUNT, fetch of one cell, staged edits, literals for Preview SQL, EXPLAIN, splitting, Safe
//! Mode's classifier and the trace's redaction. Shared code calls them only through the
//! `sqlgen::sql` facade, which sends the core dialects to their own (unchanged) builders.
//!
//! These builders are deliberately narrow: rows are addressed by key only (a row without a
//! primary key cannot be edited), and whatever a spec does not support is refused, never
//! approximated.

pub mod classify;
pub mod explain;
// The generated classifier and splitter tests of every dialect (`gen` is a reserved word).
#[cfg(test)]
#[path = "gen/mod.rs"]
mod generated;
#[cfg(test)]
mod golden;
pub mod literal;
pub mod mutate;
pub mod redact;
pub mod select;
pub mod split;
#[cfg(test)]
pub mod testing;
