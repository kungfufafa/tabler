//! SQL text generation. Every identifier, literal and placeholder the app emits goes through here.

pub mod mutate;
pub mod quote;
pub mod select;

pub use quote::Dialect;
pub use select::BuiltSql;
