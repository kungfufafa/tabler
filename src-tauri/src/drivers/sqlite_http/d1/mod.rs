//! Cloudflare D1 over its REST API (the sqlite-remote lane). Built with its cargo feature only.

pub mod api;
pub mod batch;
pub mod client;
pub mod decode;
pub mod entry;
pub mod session;

#[cfg(test)]
mod session_tests;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
