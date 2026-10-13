//! SQL text generation. Every identifier, literal and placeholder the app emits goes through here.

pub mod admin;
pub mod backup;
pub mod classify;
pub mod ddl;
pub mod diagram;
pub mod explain;
pub mod mutate;
pub mod objects;
pub mod quote;
pub mod select;
pub mod split;
pub mod transfer;

pub use quote::Dialect;
pub use select::BuiltSql;
