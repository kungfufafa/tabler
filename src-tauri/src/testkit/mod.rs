//! Helpers shared by the unit tests of the drivers and the shared transport: the process-wide
//! crypto provider, an in-process HTTP server and a SQL session that reaches no server.
//! Test-only (`#[cfg(test)]` in `lib.rs`), so the integration tests under `tests/` use
//! `tests/common/mod.rs` instead.

pub mod crypto;
pub mod fake_http;
pub mod fake_session;

pub use crypto::install_crypto;
pub use fake_http::{Body, FakeServer, Outcome, Piece, Reply, Request, Route};
pub use fake_session::FakeSession;
