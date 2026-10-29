//! Snowflake (the snowflake lane), over snowflake-connector-rs (the session API the official
//! connectors use). Built with its cargo feature only.
//!
//! - `params`: the profile's Snowflake settings (`SnowflakeParams`) and the account identifier.
//! - `auth`: key pair (private key file and passphrase), PAT, OAuth and password sign-in.
//! - `conn`: one session: sign-in, statements with binds, rows up to a cap, cancel.
//! - `decode`: result cells as `Value`s, column types as the editor names them.
//! - `catalog`: the parsers of the `SHOW` introspection.
//! - `errors`: the connector's errors as `AppError`s.
//!
//! The SQL text the driver sends comes from `sqlgen::dialect::snowflake`.

// Phase A: nothing outside the lane calls these until `entry.rs` is the lane's (F10).
#![allow(dead_code)]

pub mod auth;
pub mod catalog;
pub mod conn;
pub mod decode;
pub mod entry;
pub mod errors;
pub mod params;
#[cfg(test)]
pub mod testing;
