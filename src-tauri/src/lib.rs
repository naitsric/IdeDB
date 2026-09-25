mod data_sources;
mod drivers;
mod error;
mod export;
mod sessions;

use idedb_store::{Keychain, Store};
use tauri::Manager;

use crate::data_sources::Secrets;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            app.manage(Store::open(&dir.join("idedb.db"))?);
            app.manage(Secrets(Box::new(Keychain::new(app.config().identifier.clone()))));
            Ok(())
        })
        .manage(sessions::Sessions::default())
        .invoke_handler(tauri::generate_handler![
            data_sources::data_sources_list,
            data_sources::data_source_save,
            data_sources::data_source_delete,
            data_sources::data_source_test,
            sessions::session_open,
            sessions::session_close,
            sessions::session_execute,
            sessions::session_cancel,
            sessions::session_schemas,
            sessions::session_introspect,
            sessions::history_list,
            sessions::session_apply,
            export::export_write,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
