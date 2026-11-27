#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

fn main() -> tauri::Result<()> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::history_list,
            commands::connections_list,
            commands::settings_get,
            commands::settings_set
        ])
        .run(tauri::generate_context!())
}
