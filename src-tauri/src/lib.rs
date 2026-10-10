//! Tabler backend: Tauri commands are thin wrappers; logic lives in the modules below so it can be
//! unit-tested without a running app.

pub mod commands;
pub mod sqlgen;
pub mod types;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // One process-wide TLS crypto provider for russh, mysql_async and tokio-postgres-rustls.
    // Mixing providers panics at runtime, so install it before any connection is made.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    let log_level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_log::Builder::new().level(log_level).build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![commands::app::app_info])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
