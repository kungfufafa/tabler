//! DynamoDB (the dynamodb lane). Built with its cargo feature only.
//!
//! A hand-rolled client of the JSON 1.0 protocol (`POST /`, `X-Amz-Target:
//! DynamoDB_20120810.<Action>`), signed with SigV4 by the shared `drivers::common::aws`:
//!
//! - `entry`: open, validate and the form's calls (`drivers::registry`);
//! - `session`: the `NativeSession` and `TableSource` (caps, read-only, cancel, the console);
//! - `auth`: where the credentials come from by `DynamoAuth` (the shared AWS sources, the
//!   Command mode's output, DynamoDB Local's dummy keys);
//! - `params`: the profile's `DynamoParams` and the endpoint of a region or an override;
//! - `transport`: the HTTP seam, sent through the shared `HttpClient`;
//! - `client`: signed calls with retries on throttling, clock-skew correction, one refresh of
//!   expired credentials, cancel and the session's trace;
//! - `errors`: error replies as `AppError`s, cancelled transactions item by item;
//! - `attr`: AttributeValues as `Value`s and back, keeping each attribute's type;
//! - `catalog`: ListTables and DescribeTable as tables, columns (keys, then sampled attributes)
//!   and indexes, and the table's CreateTable definition;
//! - `browse`: grid pages from Scan or Query with the LastEvaluatedKey as the cursor, counts,
//!   cells, console commands and commits;
//! - `testing`: an in-memory DynamoDB the tests talk to, also served over HTTP.
//!
//! The requests themselves (reads, filters, edits with their guards, the console's PartiQL and
//! API commands and their classifier) are built in `sqlgen::native::dynamo`.

pub mod attr;
pub mod auth;
pub mod browse;
pub mod catalog;
pub mod client;
pub mod entry;
pub mod errors;
pub mod params;
pub mod session;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
pub mod transport;
