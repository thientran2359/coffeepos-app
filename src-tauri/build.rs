fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_shell_info",
            "save_store_name",
            "get_runtime_info",
            "start_runtime",
            "stop_runtime",
            "restart_runtime",
            "get_provisioning_info",
            "provision_wordpress",
        ]),
    ))
    .expect("failed to build CoffeePOS Desktop resources");
}
