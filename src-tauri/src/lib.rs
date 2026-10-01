mod data_sources;
mod error;
mod export;
mod mcp;
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
            // After the store and secrets: the MCP server reads both.
            mcp::setup(app.handle());
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
            mcp::mcp_status,
            mcp::mcp_settings_get,
            mcp::mcp_settings_save,
            mcp::mcp_clients_list,
            mcp::mcp_client_create,
            mcp::mcp_client_rename,
            mcp::mcp_client_rotate,
            mcp::mcp_client_revoke,
            mcp::mcp_client_delete,
            mcp::mcp_grants_set,
            mcp::mcp_never_write_list,
            mcp::mcp_never_write_set,
            mcp::mcp_audit_list,
            mcp::mcp_approval_answer,
            mcp::mcp_approvals_pending,
            mcp::mcp_endpoint,
            menu::menu_set,
            app_quit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
