//! Serde types shared with the renderer. Every type here derives `ts_rs::TS` and is exported to
//! `src/lib/ipc/bindings/` when `cargo test` runs (see `.cargo/config.toml`).

pub mod app;
