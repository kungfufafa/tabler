//! SQL Server (the sqlserver lane), over TDS with tiberius. Built with its cargo feature only.
//!
//! - `entry`: what `drivers::registry` calls (open, validate, the form's calls).
//! - `session`: the `DbSession`: the editor's connection and a pool of workers, caps, read-only.
//! - `editor`: the editor's batches, the row cap and Explain.
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
mod editor;
pub mod entry;
pub mod errors;
pub mod introspect;
pub mod params;
pub mod session;
#[cfg(test)]
pub mod testing;
#[cfg(test)]
mod tests;
