//! BigQuery over its REST API (the bigquery lane). Built with its cargo feature only.
//!
//! A hand-rolled client of REST v2: `transport` is the HTTP seam (the shared client of step F8a
//! in Phase B), `auth` gets bearer tokens (a service account's RS256 assertion signed in `jwt`,
//! an Application Default Credentials refresh token, or a fixed token), `client` runs the calls
//! (jobs with polling and cancel, the metadata listings, `tabledata.list`), `jobs` holds their
//! bodies and replies, `params` the profile's settings and the typed `@pN` parameters,
//! `decode` turns schemas and rows into Tabler's columns and `Value`s, `catalog` makes the
//! sidebar, structure and process list of the metadata, and `errors` maps BigQuery's errors.
//! `testing` is a scripted BigQuery the tests talk to.

// Phase A: nothing calls the client before entry.rs is handed over (F10).
#![allow(dead_code)]

pub mod auth;
pub mod catalog;
pub mod client;
pub mod decode;
pub mod entry;
pub mod errors;
pub mod jobs;
pub mod jwt;
pub mod params;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
pub mod transport;
