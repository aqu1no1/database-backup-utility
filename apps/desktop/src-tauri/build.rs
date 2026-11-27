const COMMANDS: &[&str] = &[
    "history_list",
    "connections_list",
    "settings_get",
    "settings_set",
];

fn main() {
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("{error:#}");
    }
}
