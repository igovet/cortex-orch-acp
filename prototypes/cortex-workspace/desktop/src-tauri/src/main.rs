#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Mock-only shell: no commands, plugins, services or ACP.
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("mock prototype shell failed");
}
