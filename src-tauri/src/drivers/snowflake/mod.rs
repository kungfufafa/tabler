//! Snowflake (the snowflake lane), over snowflake-connector-rs (the session API the official
//! connectors use). Built with its cargo feature only.
//!
//! - `params`: the profile's Snowflake settings (`SnowflakeParams`) and the account identifier.
//! - `auth`: key pair (private key file and passphrase), PAT, OAuth and password sign-in.
//! - `signin`: how a session signs in, kept to sign in again.
//! - `conn`: one Snowflake session: statements with binds, rows up to a cap, cancel.
//! - `session`: the `DbSession`: the editor's session and the app's, caps, read-only, hooks.
//! - `editor`: the editor's runs.
//! - `decode`: result cells as `Value`s, column types as the editor names them.
//! - `catalog`: the parsers of the `SHOW` introspection.
//! - `errors`: the connector's errors as `AppError`s.
//!
//! The SQL text the driver sends comes from `sqlgen::dialect::snowflake`.

pub mod auth;
pub mod catalog;
pub mod conn;
pub mod decode;
mod editor;
pub mod entry;
pub mod errors;
pub mod params;
pub mod session;
#[cfg(test)]
mod session_tests;
pub mod signin;
#[cfg(test)]
pub mod testing;
