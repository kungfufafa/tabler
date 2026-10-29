//! Helpers shared by the unit tests of the drivers and the shared transport: the process-wide
//! crypto provider, an in-process HTTP server, and a SQL session and a native store that reach
//! no server.
//! Test-only (`#[cfg(test)]` in `lib.rs`), so the integration tests under `tests/` use
//! `tests/common/mod.rs` instead.

pub mod crypto;
pub mod fake_http;
pub mod fake_native;
pub mod fake_session;
// The keys, certificates and vectors of the shared transport's tests (`tls_client`,
// `drivers::common`), built with the features that build what they test.
#[cfg(feature = "common-jwt")]
pub mod jwt_key;
#[cfg(feature = "common-aws")]
pub mod sigv4_vectors;
#[cfg(feature = "common-tls")]
pub mod tls;

pub use crypto::install_crypto;
pub use fake_http::{Body, FakeServer, Outcome, Piece, Reply, Request, Route};
pub use fake_native::FakeStore;
pub use fake_session::FakeSession;
