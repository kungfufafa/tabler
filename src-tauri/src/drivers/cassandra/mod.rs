//! Cassandra and ScyllaDB (the cassandra lane), over the scylla driver. Built with its cargo
//! feature only.
//!
//! - `values`: CQL values to Tabler's `Value`, result columns;
//! - `types`: CQL type texts and result types, as `ValueKind`s;
//! - `catalog`: the `system` / `system_schema` queries and the parsers of their rows;
//! - `cursor`: the paging state as the grid's page cursor;
//! - `cqlsh`: the session commands cqlsh runs itself (`CONSISTENCY`, `TRACING`, …).
//!
//! The generated CQL (quoting, the editor's splitter, Safe Mode's classifier, rebuilt DDL) is in
//! `sqlgen::dialect::cql`.

pub mod catalog;
pub mod cqlsh;
pub mod cursor;
pub mod entry;
#[cfg(test)]
pub mod testing;
pub mod types;
pub mod values;
