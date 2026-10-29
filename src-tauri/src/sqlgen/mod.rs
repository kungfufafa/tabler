//! SQL text generation. Every identifier, literal and placeholder the app emits goes through here.

pub mod admin;
pub mod backup;
pub mod classify;
pub mod conn_options;
pub mod ddl;
pub mod definition;
pub mod diagram;
pub mod dialect;
pub mod explain;
#[cfg(test)]
mod golden;
pub mod mutate;
// What a server narrows in its dialect (D1: 100 values, no blobs, integers to 2^53).
pub mod narrow;
// Command and document builders of the native stores, each with its driver's feature.
#[cfg(any(
    feature = "drv-redis",
    feature = "drv-mongodb",
    feature = "drv-elasticsearch",
    feature = "drv-dynamodb"
))]
pub mod native;
pub mod objects;
// Builders of the extension dialects, reached through `sql`.
pub mod portable;
pub mod quote;
pub mod security;
pub mod select;
pub mod split;
// The live-SQL facade over every dialect.
pub mod sql;
pub mod transfer;

pub use dialect::SqlDialect;
pub use quote::Dialect;
pub use select::BuiltSql;
