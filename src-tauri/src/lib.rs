//! Tabler backend: Tauri commands are thin wrappers; logic lives in the modules below so it can be
//! unit-tested without a running app.

// `AppError::Db` carries the full server diagnostic; boxing it everywhere is not worth the noise.
#![allow(clippy::result_large_err)]

pub mod appdb;
pub mod commands;
pub mod drivers;
pub mod error;
pub mod export;
pub mod secrets;
pub mod sqlgen;
pub mod state;
pub mod tunnel;
pub mod types;

use tauri::Manager;

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
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let db_path = dir.join("tabler.sqlite");
            let appdb = appdb::AppDb::open(&db_path)?;
            log::info!("app database at {}", db_path.display());
            let secrets = secrets::Secrets::from_env(appdb.clone());
            app.manage(state::AppState::new(appdb, secrets));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::app_info,
            commands::profiles::list_profiles,
            commands::profiles::get_profile,
            commands::profiles::save_profile,
            commands::profiles::delete_profile,
            commands::profiles::duplicate_profile,
            commands::profiles::reorder_profiles,
            commands::profiles::get_secret_status,
            commands::profiles::test_connection,
            commands::connections::connect,
            commands::connections::disconnect,
            commands::connections::list_connections,
            commands::connections::ping,
            commands::connections::switch_database,
            commands::schema::list_databases,
            commands::schema::list_schemas,
            commands::schema::list_tables,
            commands::schema::get_table,
            commands::schema::get_view_definition,
            commands::schema::get_completion_metadata,
            commands::schema::invalidate_schema_cache,
            commands::browse::browse_table,
            commands::browse::count_table_rows,
            commands::browse::fetch_cell,
            commands::mutate::preview_changes,
            commands::mutate::apply_changes,
            commands::ddl::preview_ddl,
            commands::ddl::apply_ddl,
            commands::ddl::get_table_ddl,
            commands::query::run_sql,
            commands::query::cancel_query,
            commands::query::explain,
            commands::history::list_history,
            commands::history::delete_history,
            commands::history::clear_history,
            commands::files::read_text_file,
            commands::files::write_text_file,
            commands::settings::get_settings,
            commands::settings::set_setting,
            commands::export::export_data,
            commands::export::cancel_export,
            commands::connections::accept_host_key,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
