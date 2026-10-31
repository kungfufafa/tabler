//! SQLite engines reached over HTTP: LibSQL (Turso, `sqld`) and Cloudflare D1 (the sqlite-remote
//! lane). Not to be confused with `tunnel::sqlite_remote`, a SQLite file copied over SSH. Each
//! driver is built with its own cargo feature; the modules here are shared by both.

pub mod catalog;
pub mod codes;
pub mod guard;
pub mod session;
pub mod transport;
pub mod value;

#[cfg(test)]
pub(crate) mod testing;

#[cfg(feature = "drv-d1")]
pub mod d1;
#[cfg(feature = "drv-libsql")]
pub mod libsql;
