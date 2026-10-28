//! DuckDB (the duckdb lane), an engine in this process through the `duckdb` crate (bundled, with
//! JSON and Parquet). Built with its cargo feature only. The crate is `::duckdb` here: this
//! module shadows its name.
//!
//! - `engine`: one instance per database file (`Sharing::SingleProcess`), the settings it is
//!   opened with, read-only and hardened instances, the file checks of the connection form.
//! - `conn`: a connection on its own thread, streaming execution, cancel by interrupt.
//! - `types`: DuckDB's logical types, their names and kinds.
//! - `decode`: Arrow chunks as `Value`s (total), `Value`s as parameters.
//! - `catalog`: the `duckdb_*()` catalog queries and the parsers of their rows.
//! - `errors`: DuckDB messages as `AppError`s.
//! - `params`: `DuckdbParams` and the engine configuration of a profile.
//!
//! Names and literals in the SQL the driver writes are quoted by `sqlgen::dialect::duckdb` (the
//! encrypted file's ATTACH and USE, macro DDL, type member names); the fixed statements (catalog
//! queries, the hardening settings) are constants of the module that sends them.

pub mod catalog;
pub mod conn;
pub mod decode;
pub mod engine;
pub mod entry;
pub mod errors;
pub mod params;
#[cfg(test)]
pub mod testing;
pub mod types;
