//! Cassandra and ScyllaDB (the cassandra lane), over the scylla driver. Built with its cargo
//! feature only.
//!
//! - `entry`: what the registry calls (open, validate, util), and the `Dial` of a profile;
//! - `params`: `CassandraParams`, the profile's Cassandra settings;
//! - `session`: the `DbSession` and its `TableSource` (the grid, paged by the driver's state);
//! - `editor`: the editor's runs and cqlsh's session commands;
//! - `wire`: the seam between the session and the driver, `client`: the scylla side of it;
//! - `encode` / `values`: Tabler's values to bound CQL values and back; `errors`: the driver's
//!   errors as `AppError`s;
//! - `types`: CQL type texts and result types, as `ValueKind`s;
//! - `catalog`: the `system` / `system_schema` queries and the parsers of their rows;
//! - `cursor`: the paging state as the grid's page cursor;
//! - `cqlsh`: the session commands cqlsh runs itself (`CONSISTENCY`, `TRACING`, …).
//!
//! The generated CQL (quoting, the grid's statements and edits, the editor's splitter, Safe
//! Mode's classifier, rebuilt DDL) is in `sqlgen::dialect::cql`.

pub mod catalog;
pub mod client;
pub mod cqlsh;
pub mod cursor;
mod editor;
pub mod encode;
pub mod entry;
pub mod errors;
#[cfg(test)]
mod fake;
pub mod params;
pub mod session;
#[cfg(test)]
pub mod testing;
#[cfg(test)]
mod tests;
pub mod types;
pub mod values;
pub mod wire;
