fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_shell_info",
            "save_app_settings",
            "get_setup_info",
            "save_setup_profile",
            "copy_admin_password",
            "get_runtime_info",
            "start_runtime",
            "stop_runtime",
            "restart_runtime",
            "retry_runtime_health",
            "refresh_runtime_maintenance",
            "get_health_diagnostics",
            "open_wordpress",
            "open_pos",
            "get_provisioning_info",
            "provision_wordpress",
        ]),
    ))
    .expect("failed to build CoffeePOS Desktop resources");
}
