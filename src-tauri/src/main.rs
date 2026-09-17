#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;

use config::{AppConfig, Store};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{Manager, State};

#[derive(Default)]
struct ShellState(Mutex<Option<Store>>);

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
        .0
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

fn main() {
    tauri::Builder::default()
        .manage(ShellState::default())
        .invoke_handler(tauri::generate_handler![get_shell_info, save_store_name])
        .run(tauri::generate_context!())
        .expect("CoffeePOS Desktop could not start the native shell");
}
