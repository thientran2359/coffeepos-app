fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&["get_shell_info", "save_store_name"]),
    ))
    .expect("failed to build CoffeePOS Desktop resources");
}
