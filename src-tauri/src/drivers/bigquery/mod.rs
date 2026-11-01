//! BigQuery over its REST API (the bigquery lane). Built with its cargo feature only.
//!
//! A hand-rolled client of REST v2 over the shared HTTP client: `transport` is the HTTP seam,
//! `auth` gets bearer tokens (a service account's RS256 assertion signed with `common::jwt`, an
//! Application Default Credentials refresh token, or the profile's token), `client` runs the
//! calls (jobs with polling and cancel, the metadata listings, `tabledata.list`), `jobs` holds
//! their bodies and replies, `params` the profile's settings and the typed `@pN` parameters,
//! `decode` turns schemas and rows into Tabler's columns and `Value`s, `catalog` makes the
//! sidebar, structure and process list of the metadata, `errors` maps BigQuery's errors,
//! `cursor` the grid's page cursors, and `session` is the `DbSession` the app talks to.
//! `testing` is a scripted BigQuery the tests talk to.

pub mod auth;
pub mod catalog;
pub mod client;
pub mod cursor;
pub mod decode;
pub mod entry;
pub mod errors;
pub mod jobs;
pub mod params;
pub mod session;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
pub mod transport;
