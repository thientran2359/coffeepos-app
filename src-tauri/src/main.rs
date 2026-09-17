#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
#[cfg_attr(not(debug_assertions), allow(dead_code))]
mod provisioning;
#[cfg_attr(not(debug_assertions), allow(dead_code))]
mod runtime;
mod secret;

use config::{AppConfig, Store};
#[cfg(debug_assertions)]
use provisioning::Provisioner;
use provisioning::ProvisioningInfo;
use runtime::{RuntimeInfo, RuntimeManager};
use serde::Serialize;
#[cfg(debug_assertions)]
use std::path::PathBuf;
use std::sync::Mutex;
#[cfg(debug_assertions)]
use std::sync::TryLockError;
use tauri::{Manager, State};

#[derive(Default)]
struct ShellState {
    store: Mutex<Option<Store>>,
    runtime: Mutex<Option<RuntimeManager>>,
    #[cfg(debug_assertions)]
    provisioning: Mutex<()>,
}

#[derive(Serialize)]
struct ShellInfo {
    version: &'static str,
    data_dir: String,
    config: AppConfig,
}

fn with_store(
    app: &tauri::AppHandle,
    state: &ShellState,
    operation: impl FnOnce(&mut Store) -> Result<(), String>,
) -> Result<ShellInfo, String> {
    let mut guard = state
        .store
        .lock()
        .map_err(|_| "Application state unavailable. Restart CoffeePOS Desktop.")?;
    if guard.is_none() {
        let root = app.path().app_local_data_dir().map_err(|e| {
            format!("Cannot locate application data: {e}. Check your OS user profile.")
        })?;
        *guard = Some(Store::open(root)?);
    }
    let store = guard
        .as_mut()
        .ok_or("Configuration unavailable. Retry startup.")?;
    operation(store)?;
    Ok(ShellInfo {
        version: env!("CARGO_PKG_VERSION"),
        data_dir: store.root.to_string_lossy().into_owned(),
        config: store.config.clone(),
    })
}

#[cfg(debug_assertions)]
fn data_root(app: &tauri::AppHandle, state: &ShellState) -> Result<PathBuf, String> {
    let mut guard = state
        .store
        .lock()
        .map_err(|_| "Application state unavailable. Restart CoffeePOS Desktop.")?;
    if guard.is_none() {
        let root = app.path().app_local_data_dir().map_err(|e| {
            format!("Cannot locate application data: {e}. Check your OS user profile.")
        })?;
        *guard = Some(Store::open(root)?);
    }
    guard
        .as_ref()
        .map(|store| store.root.clone())
        .ok_or_else(|| "Configuration unavailable. Retry startup.".to_string())
}

#[cfg(debug_assertions)]
fn development_runtime_paths() -> Result<(PathBuf, PathBuf), String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let project_root = manifest_dir
        .parent()
        .ok_or("Cannot resolve CoffeePOS Desktop project root for the development runtime.")?
        .to_path_buf();
    let target = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else {
        return Err("No development runtime target is configured for this platform.".into());
    };
    let manifest = project_root
        .join("runtime")
        .join("development")
        .join(target)
        .join("manifest.json");
    Ok((project_root, manifest))
}

fn with_runtime(
    _app: &tauri::AppHandle,
    state: &ShellState,
    operation: impl FnOnce(&mut RuntimeManager) -> Result<RuntimeInfo, runtime::RuntimeErrorInfo>,
) -> Result<RuntimeInfo, String> {
    #[cfg(debug_assertions)]
    let root = data_root(_app, state)?;
    let mut guard = state
        .runtime
        .lock()
        .map_err(|_| "Runtime state unavailable. Restart CoffeePOS Desktop.".to_string())?;
    if guard.is_none() {
        #[cfg(debug_assertions)]
        {
            let (project_root, manifest) = development_runtime_paths()?;
            *guard = Some(
                RuntimeManager::from_development(&project_root, &manifest, root)
                    .map_err(|error| error.to_string())?,
            );
        }
        #[cfg(not(debug_assertions))]
        {
            return Err("Bundled Phase 2 runtime resources are not packaged yet. Use a development build until runtime packaging is implemented.".into());
        }
    }
    let manager = guard
        .as_mut()
        .ok_or_else(|| "Runtime manager unavailable. Retry startup.".to_string())?;
    operation(manager).map_err(|error| error.to_string())
}

#[tauri::command]
fn get_shell_info(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<ShellInfo, String> {
    with_store(&app, &state, |_| Ok(()))
}

#[tauri::command]
fn save_store_name(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
    store_name: String,
) -> Result<ShellInfo, String> {
    with_store(&app, &state, |store| store.save_name(&store_name))
}

#[tauri::command]
fn get_runtime_info(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<RuntimeInfo, String> {
    with_runtime(&app, &state, |runtime| Ok(runtime.refresh()))
}

#[tauri::command]
fn start_runtime(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<RuntimeInfo, String> {
    with_runtime(&app, &state, RuntimeManager::start)
}

#[tauri::command]
fn stop_runtime(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<RuntimeInfo, String> {
    with_runtime(&app, &state, RuntimeManager::stop)
}

#[tauri::command]
fn restart_runtime(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<RuntimeInfo, String> {
    with_runtime(&app, &state, RuntimeManager::restart)
}

#[tauri::command]
fn get_provisioning_info(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<ProvisioningInfo, String> {
    #[cfg(debug_assertions)]
    {
        let provisioning_guard = match state.provisioning.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(_)) => {
                return Err(
                    "Provisioning state unavailable. Restart CoffeePOS Desktop before retrying."
                        .into(),
                );
            }
        };
        let root = data_root(&app, &state)?;
        let (project_root, manifest) = development_runtime_paths()?;
        let resolved = runtime::resolve_development_manifest(&project_root, &manifest)
            .map_err(|error| error.to_string())?;
        let provisioner = Provisioner::from_development(&project_root, &manifest, resolved, root)
            .map_err(|error| error.to_string())?;
        if provisioning_guard.is_some() {
            Ok(provisioner.inspect())
        } else {
            Ok(provisioner.installing_info())
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = state;
        Err("Bundled Phase 3 WordPress resources are not packaged yet. Use a development build until runtime packaging is implemented.".into())
    }
}

#[tauri::command]
fn provision_wordpress(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<ProvisioningInfo, String> {
    #[cfg(debug_assertions)]
    {
        let _provisioning_guard = state.provisioning.lock().map_err(|_| {
            "Provisioning state unavailable. Restart CoffeePOS Desktop.".to_string()
        })?;
        let root = data_root(&app, &state)?;
        let store_name = {
            let guard = state.store.lock().map_err(|_| {
                "Application state unavailable. Restart CoffeePOS Desktop.".to_string()
            })?;
            guard
                .as_ref()
                .map(|store| store.config.store_name.clone())
                .ok_or_else(|| "Configuration unavailable. Retry startup.".to_string())?
        };
        let (project_root, manifest) = development_runtime_paths()?;
        let mut runtime_guard = state
            .runtime
            .lock()
            .map_err(|_| "Runtime state unavailable. Restart CoffeePOS Desktop.".to_string())?;
        if runtime_guard.is_none() {
            *runtime_guard = Some(
                RuntimeManager::from_development(&project_root, &manifest, root.clone())
                    .map_err(|error| error.to_string())?,
            );
        }
        let runtime = runtime_guard
            .as_mut()
            .ok_or_else(|| "Runtime manager unavailable. Retry startup.".to_string())?;
        runtime.stop().map_err(|error| error.to_string())?;
        let (resolved, runtime_root) = runtime.provisioning_context();
        let mut provisioner =
            Provisioner::from_development(&project_root, &manifest, resolved, runtime_root)
                .map_err(|error| error.to_string())?;
        provisioner.prepare().map_err(|error| error.to_string())?;
        let runtime_info = runtime.start().map_err(|error| error.to_string())?;
        match provisioner.install_wordpress(&store_name, &runtime_info) {
            Ok(info) => Ok(info),
            Err(error) => {
                let _ = runtime.stop();
                Err(error.to_string())
            }
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = state;
        Err("Bundled Phase 3 WordPress resources are not packaged yet. Use a development build until runtime packaging is implemented.".into())
    }
}

fn main() {
    tauri::Builder::default()
        .manage(ShellState::default())
        .invoke_handler(tauri::generate_handler![
            get_shell_info,
            save_store_name,
            get_runtime_info,
            start_runtime,
            stop_runtime,
            restart_runtime,
            get_provisioning_info,
            provision_wordpress
        ])
        .run(tauri::generate_context!())
        .expect("CoffeePOS Desktop could not start the native shell");
}
