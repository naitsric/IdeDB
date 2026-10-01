// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `idedb mcp-bridge`: the stdio MCP server that clients such as Claude
    // Desktop launch, relaying to the app's own. Decided before Tauri is
    // built, so it opens no window and shows no Dock icon.
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "mcp-bridge") {
        std::process::exit(idedb_lib::mcp_bridge_main());
    }
    idedb_lib::run()
}
