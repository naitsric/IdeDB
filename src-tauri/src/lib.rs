mod data_sources;
mod error;
mod export;
mod menu;
mod sessions;

use idedb_store::{SecretStore, Store};
use tauri::Manager;

use crate::data_sources::Secrets;

/// Passwords go to the macOS Keychain.
#[cfg(target_os = "macos")]
fn secret_store(app: &tauri::App) -> Box<dyn SecretStore> {
    Box::new(idedb_store::Keychain::new(app.config().identifier.clone()))
}

/// IdeDB ships for macOS only. Other platforms build (for CI) with
/// in-memory secrets: saved passwords are forgotten when the app quits.
#[cfg(not(target_os = "macos"))]
fn secret_store(_app: &tauri::App) -> Box<dyn SecretStore> {
    Box::new(idedb_store::MemorySecrets::default())
}

/// Ends the app. The UI's Quit (menu and ⌘Q) calls it only after asking
/// about open transactions; see src/db/transactions.ts.
#[tauri::command]
fn app_quit(app: tauri::AppHandle) {
    app.exit(0);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Reopens the window at its last size and position.
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .on_menu_event(menu::on_menu_event)
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            app.manage(Store::open(&dir.join("idedb.db"))?);
            app.manage(Secrets(secret_store(app)));
            Ok(())
        })
        .manage(sessions::Sessions::default())
        .manage(export::Exports::default())
        .invoke_handler(tauri::generate_handler![
            data_sources::data_sources_list,
            data_sources::data_source_save,
            data_sources::data_source_delete,
            data_sources::data_source_test,
            sessions::session_open,
            sessions::session_close,
            sessions::session_execute,
            sessions::session_fetch_more,
            sessions::session_close_result,
            sessions::session_cancel,
            sessions::session_schemas,
            sessions::session_introspect,
            sessions::history_list,
            sessions::session_apply,
            sessions::session_check,
            sessions::session_set_schema,
            export::export_begin,
            export::export_write,
            export::export_finish,
            menu::menu_set,
            app_quit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
