//! SQL Server (the sqlserver lane), over TDS with tiberius. Built with its cargo feature only.
//!
//! - `params`: the profile's SQL Server settings (`MssqlParams`).
//! - `connect`: the tiberius configuration, the TCP connection and the login.
//! - `conn`: one connection as an actor, which runs statements and cancels them on the server.
//! - `decode`: tiberius values as `Value`s, and `Value`s as bind parameters.
//! - `introspect`: the `sys.*` catalog queries and their parsers.
//! - `errors`: tiberius errors as `AppError`s.
//!
//! The SQL text the driver sends comes from `sqlgen::dialect::tsql`.

pub mod conn;
pub mod connect;
pub mod decode;
pub mod entry;
pub mod errors;
pub mod introspect;
pub mod params;
#[cfg(test)]
pub mod testing;
