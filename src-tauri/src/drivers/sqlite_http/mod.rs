//! SQLite engines reached over HTTP: LibSQL (Turso, `sqld`) and Cloudflare D1 (the sqlite-remote
//! lane). Not to be confused with `tunnel::sqlite_remote`, a SQLite file copied over SSH. Each
//! driver is built with its own cargo feature; the modules here are shared by both.

// Phase A: the clients, codecs and catalog are complete, but no session calls them until the
// lane takes `entry.rs` over (F10). Remove this once the sessions use them.
#![allow(dead_code)]

pub mod catalog;
pub mod codes;
pub mod guard;
pub mod transport;
pub mod value;

#[cfg(test)]
pub(crate) mod testing;

#[cfg(feature = "drv-d1")]
pub mod d1;
#[cfg(feature = "drv-libsql")]
pub mod libsql;
