//! Redis and Valkey (the redis lane). Built with its cargo feature only.
//!
//! - `cli`: redis-cli command lines (the tokenizer, the console's splitter);
//! - `classify`: whether a command reads or writes (an allow-list, default-deny);
//! - `values`: RESP2 / RESP3 replies as `Value`s and JSON;
//! - `keys`: key types, the columns of a `dbN` table, value previews per type;
//! - `scan`: SCAN pages and cursors, the filter bar as globs, a page's rows;
//! - `info`: INFO, the keyspace, the product and its version gates, the sidebar's databases;
//! - `reply`: console replies as result grids;
//! - `errors`: Redis errors as `AppError`s;
//! - `testing`: an in-process RESP2 / RESP3 server for the tests.
//!
//! The commands themselves (reads, edits and their preview text) are built in
//! `sqlgen::native::redis`. The crate is `::redis` here: this module shadows its name.

pub mod classify;
pub mod cli;
pub mod entry;
pub mod errors;
pub mod info;
pub mod keys;
pub mod reply;
pub mod scan;
#[cfg(test)]
pub mod testing;
#[cfg(test)]
mod tests;
pub mod values;
