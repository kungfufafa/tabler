//! Tauri command handlers. Keep these thin: validate, look up state, delegate, map errors.

pub mod admin;
pub mod app;
pub mod assistant;
pub mod backup;
pub mod browse;
#[cfg(test)]
mod caps_tests;
pub mod conn_extras;
pub mod connections;
pub mod console;
pub mod ddl;
pub mod decoder;
pub mod deeplink;
pub mod diagram;
pub mod export;
pub mod files;
pub mod history;
pub mod import;
pub mod locations;
pub mod lock;
pub mod mcp;
pub mod menu;
pub mod metrics;
pub mod mutate;
pub mod native;
#[cfg(test)]
mod native_tests;
pub mod objects;
pub mod plugins;
pub mod profiles;
pub mod query;
pub mod recent;
pub mod safety;
pub mod saved_queries;
pub mod schema;
pub mod settings;
pub mod sql_files;
pub mod sqlite_ssh;
pub mod workspace_commit;
