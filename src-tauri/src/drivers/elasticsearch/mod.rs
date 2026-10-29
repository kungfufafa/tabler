//! Elasticsearch and OpenSearch (the elasticsearch lane). Built with its cargo feature only.
//!
//! - `json`: JSON that keeps numbers as written and key order (replies, `_source`, bodies);
//! - `console`: the Kibana-style console (splitter, `METHOD /path` + body into an `EsRequest`);
//! - `classify`: whether a request reads or writes (an allow-list, default-deny);
//! - `info`: `GET /` (Elasticsearch or OpenSearch, version) and the features it allows;
//! - `catalog`: indices, aliases, data streams, mappings flattened into columns, settings;
//! - `values`: `_source` values as `Value`s and back, keeping each field's type;
//! - `reply`: console replies as result grids;
//! - `errors`: error replies as `AppError`s;
//! - `transport`: the HTTP seam and the cluster's base URL;
//! - `auth`, `sigv4`: Basic, API key, bearer and AWS SigV4 credentials, the Elastic Cloud ID;
//! - `client`: the REST client (catalog, paging with points in time or scrolls, counts,
//!   documents, `_bulk` commits, console runs);
//! - `testing`: an in-memory cluster behind the transport, and a reqwest transport.
//!
//! Requests, bodies and their preview text are built in `sqlgen::native::elastic`.

// Phase A: the session that calls these arrives with `entry.rs` (F10).
#![allow(dead_code)]

pub mod auth;
pub mod catalog;
pub mod classify;
pub mod client;
pub mod console;
pub mod entry;
pub mod errors;
pub mod info;
pub mod json;
pub mod reply;
pub mod sigv4;
#[cfg(test)]
pub mod testing;
#[cfg(test)]
mod tests;
pub mod transport;
pub mod values;
