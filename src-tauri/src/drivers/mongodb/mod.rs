//! MongoDB (the mongodb lane). Built with its cargo feature only.
//!
//! - `entry`: what the registry calls (open, validate, the form's calls);
//! - `params`: the profile's MongoDB settings (`MongoParams`) and a light reading of a URL;
//! - `connect`: a profile as the crate's client options (hosts, SSH, TLS, sign-in, timeouts);
//! - `session`: the `NativeSession` and its `TableSource` (catalog, grid, counts, edits, cancel);
//! - `console`: one console statement run and shown; `calls`: the session's own calls and
//!   Running Queries;
//! - `literal`: JavaScript object literals and Extended JSON read into BSON (the console's and
//!   the raw filter's values, Json cells written back);
//! - `values`: BSON values as `Value`s and back, keeping each value's BSON type;
//! - `shell`: the console's mongosh subset (the splitter, statements as commands);
//! - `classify`: whether a statement reads or writes (an allow-list, default-deny);
//! - `schema`: a collection's columns inferred from a sample of its documents;
//! - `catalog`: the replies of the catalog commands as databases, tables, indexes, `TableMeta`;
//! - `errors`: driver and server errors as `AppError`s;
//! - `testing`: an in-memory MongoDB for the tests, also served over the wire protocol.
//!
//! The commands themselves (catalog, browse, filters, edits, the console's methods and their
//! preview text) are built in `sqlgen::native::mongo`. The crate is `::mongodb` here: this
//! module shadows its name.

pub mod calls;
pub mod catalog;
pub mod classify;
pub mod connect;
pub mod console;
pub mod entry;
pub mod errors;
pub mod literal;
pub mod params;
pub mod schema;
pub mod session;
#[cfg(test)]
mod session_tests;
pub mod shell;
#[cfg(test)]
pub mod testing;
#[cfg(test)]
mod tests;
pub mod values;
