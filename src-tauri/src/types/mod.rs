//! Serde types shared with the renderer. Every type here derives `ts_rs::TS` and is exported to
//! `src/lib/ipc/bindings/` when `cargo test` runs (see `.cargo/config.toml`).

pub mod admin;
pub mod app;
pub mod assistant;
pub mod backup;
pub mod browse;
pub mod ddl;
pub mod diagram;
pub mod export;
pub mod history;
pub mod import;
pub mod locations;
pub mod mcp;
pub mod metrics;
pub mod mutate;
pub mod objects;
pub mod plugins;
pub mod profile;
pub mod query;
pub mod saved_query;
pub mod schema;
pub mod security;
pub mod sql_file;
pub mod value;
pub mod window;
