//! Vertica (the vertica lane). Built with its cargo feature only.
//!
//! A hand-rolled client of Vertica's protocol 3.x (no Rust crate speaks it):
//! - `entry`: what `drivers::registry` calls (open, validate, the form's calls).
//! - `session`: the `DbSession`: the editor's connection and a pool of workers, caps, read-only.
//! - `editor`: the editor's statements, the row cap and the streamed reads.
//! - `params`: the profile's Vertica settings (`VerticaParams`).
//! - `connect`: the socket, TLS, the login and each connection's setup; cancel requests.
//! - `conn`: one connection over any byte stream; `wire` frames and parses the messages, `auth`
//!   answers the password requests.
//! - `types` and `decode`: the described columns and their cells as `Value`s.
//! - `catalog`: the `v_catalog` / `v_monitor` queries and their parsers.
//!
//! `testing` is a scripted server the tests talk to over a loopback socket. The SQL text the
//! driver sends comes from `sqlgen::dialect::vertica`.

pub mod auth;
pub mod catalog;
pub mod conn;
pub mod connect;
pub mod decode;
mod editor;
pub mod entry;
pub mod params;
pub mod session;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
pub(crate) mod testing;
pub mod types;
pub mod wire;
