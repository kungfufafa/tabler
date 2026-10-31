//! LibSQL over HTTP (the sqlite-remote lane). Built with its cargo feature only.

pub mod client;
pub mod decode;
pub mod entry;
pub mod error;
pub mod hrana;
pub mod session;
pub mod url;

#[cfg(test)]
mod session_tests;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
