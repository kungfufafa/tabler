//! Cloudflare D1 over its REST API (the sqlite-remote lane). Built with its cargo feature only.

pub mod api;
pub mod batch;
pub mod client;
pub mod decode;
pub mod entry;

#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
