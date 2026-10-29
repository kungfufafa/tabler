//! ClickHouse over HTTP (the clickhouse lane). Built with its cargo feature only.
//!
//! A hand-rolled client of the HTTP interface (8123, 8443 with TLS): `request` builds each POST
//! (SQL in the body; database, query id, session, roles, settings, `readonly=2` and the bound
//! values in the URL; user and key in headers), `params` writes bound values in the server's
//! text forms, `transport` is the HTTP seam (the shared client of step F8a in Phase B), `format`
//! reads the `JSONCompactEachRowWithNamesAndTypes` body as it streams and the errors written
//! into it, `types`, `json` and `decode` turn its cells into `Value`s, `client` runs statements,
//! guarded edits and `KILL QUERY`, and `catalog` reads `system.*`. `testing` is a scripted server
//! the tests talk to.

// Phase A: nothing calls the client before entry.rs is handed over (F10).
#![allow(dead_code)]

pub mod catalog;
pub mod client;
pub mod decode;
pub mod entry;
pub mod format;
pub mod json;
pub mod params;
pub mod request;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
pub mod transport;
pub mod types;
