//! Elasticsearch and OpenSearch (the elasticsearch lane). Built with its cargo feature only.
//!
//! - `entry`: what the registry calls (open, validate, the form's calls);
//! - `params`: the profile's `ElasticParams`;
//! - `session`: the native session (sidebar, grid, edits, console, tasks);
//! - `client`: the REST client (catalog, paging with points in time or scrolls, counts,
//!   documents, `_bulk` commits, console runs);
//! - `transport`: the HTTP seam over the shared `HttpClient`, and the cluster's base URL;
//! - `auth`: Basic, API key, bearer and AWS SigV4 credentials, the Elastic Cloud ID;
//! - `json`: JSON that keeps numbers as written and key order (replies, `_source`, bodies);
//! - `console`: the Kibana-style console (splitter, `METHOD /path` + body into an `EsRequest`,
//!   SQL statements);
//! - `classify`: whether a statement reads or writes (an allow-list, default-deny);
//! - `info`: `GET /` (Elasticsearch or OpenSearch, version) and the features it allows;
//! - `catalog`: indices, aliases, data streams, mappings flattened into columns, settings;
//! - `values`: `_source` values as `Value`s and back, keeping each field's type;
//! - `reply`: console replies as result grids;
//! - `errors`: error replies as `AppError`s;
//! - `processes`: the task management API as Running Queries;
//! - `testing`: an in-memory cluster behind the transport, also served over HTTP.
//!
//! Requests, bodies and their preview text are built in `sqlgen::native::elastic`.

pub mod auth;
pub mod catalog;
pub mod classify;
pub mod client;
pub mod console;
pub mod entry;
pub mod errors;
pub mod info;
pub mod json;
pub mod params;
pub mod processes;
pub mod reply;
pub mod session;
#[cfg(test)]
pub mod testing;
#[cfg(test)]
mod tests;
pub mod transport;
pub mod values;
