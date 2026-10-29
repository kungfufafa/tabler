//! DynamoDB (the dynamodb lane). Built with its cargo feature only.
//!
//! A hand-rolled client of the JSON 1.0 protocol (`POST /`, `X-Amz-Target:
//! DynamoDB_20120810.<Action>`), signed with SigV4:
//!
//! - `sigv4`: AWS Signature Version 4, checked against AWS's test suite;
//! - `credentials`: access keys, the environment, `~/.aws` profiles, `credential_process`, the
//!   Command mode and DynamoDB Local's dummy keys, refreshed before they expire;
//! - `params`: the profile's `DynamoParams` and the endpoint of a region or an override;
//! - `transport`: the HTTP seam (the shared client of step F8a in Phase B);
//! - `client`: signed calls with retries on throttling, clock-skew correction, one refresh of
//!   expired credentials and cancel;
//! - `errors`: error replies as `AppError`s, cancelled transactions item by item;
//! - `attr`: AttributeValues as `Value`s and back, keeping each attribute's type;
//! - `catalog`: ListTables and DescribeTable as tables, columns (keys, then sampled attributes)
//!   and indexes, and the table's CreateTable definition;
//! - `browse`: grid pages from Scan or Query with the LastEvaluatedKey as the cursor, counts and
//!   cells;
//! - `testing`: an in-memory DynamoDB the tests talk to, also served over HTTP.
//!
//! The requests themselves (reads, filters, edits with their guards, the console's PartiQL and
//! API commands and their classifier) are built in `sqlgen::native::dynamo`.

// Phase A: nothing calls the client before entry.rs is handed over (F10).
#![allow(dead_code)]

pub mod attr;
pub mod browse;
pub mod catalog;
pub mod client;
pub mod credentials;
pub mod entry;
pub mod errors;
pub mod params;
pub mod sigv4;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
pub mod transport;
