//! Serde types shared with the renderer. Every type here derives `ts_rs::TS` and is exported to
//! `src/lib/ipc/bindings/` when `cargo test` runs (see `.cargo/config.toml`).

pub mod app;
pub mod browse;
pub mod ddl;
pub mod export;
pub mod history;
pub mod mutate;
pub mod profile;
pub mod query;
pub mod schema;
pub mod value;
