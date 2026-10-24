//! Helpers shared by the unit tests of the drivers and the shared transport: the process-wide
//! crypto provider and an in-process HTTP server. Test-only (`#[cfg(test)]` in `lib.rs`), so the
//! integration tests under `tests/` use `tests/common/mod.rs` instead.

pub mod crypto;
pub mod fake_http;

pub use crypto::install_crypto;
pub use fake_http::{Body, FakeServer, Outcome, Piece, Reply, Request, Route};
