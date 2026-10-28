//! Vertica (the vertica lane). Built with its cargo feature only.
//!
//! A hand-rolled client of Vertica's protocol 3.x (no Rust crate speaks it): `wire` frames and
//! parses the messages, `auth` answers the password requests, `conn` runs a session over any
//! byte stream, `types` and `decode` turn the described columns and their cells into `Value`s,
//! and `catalog` reads `v_catalog` / `v_monitor`. `testing` is a scripted server the tests talk
//! to over a loopback socket.

// Phase A: nothing calls the client before entry.rs is handed over (F10).
#![allow(dead_code)]

pub mod auth;
pub mod catalog;
pub mod conn;
pub mod decode;
pub mod entry;
#[cfg(test)]
pub(crate) mod testing;
pub mod types;
pub mod wire;
