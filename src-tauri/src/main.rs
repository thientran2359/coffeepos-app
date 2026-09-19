#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg_attr(not(debug_assertions), allow(dead_code))]
mod backup;
#[allow(dead_code)]
mod backup_database;
// Phase 7.1 defines the format writer before Phase 7.3 starts feeding it the
// complete encrypted database/uploads/config snapshot.
#[allow(dead_code)]
mod backup_format;
mod config;
mod logs;
#[cfg_attr(not(debug_assertions), allow(dead_code))]
mod provisioning;
#[cfg_attr(not(debug_assertions), allow(dead_code))]
mod runtime;
mod secret;

use backup::{BackupOperationState, BackupResult, BackupStatus};
use backup_format::{
    BackupCompatibilityTarget, BackupErrorInfo, BackupInspection, BackupValidation,
};
use config::{AppConfig, StartupView, Store};
use logs::{LogCatalog, LogErrorInfo, LogPage, SupportBundleResult};
#[cfg(debug_assertions)]
use provisioning::Provisioner;
use provisioning::{ProvisioningInfo, RepairApplyResult, RepairPlan};
#[cfg(debug_assertions)]
use provisioning::{
    ProvisioningState, RepairItemStatus, RepairResultStatus, WORDPRESS_ADMIN_EMAIL,
    WORDPRESS_ADMIN_SECRET, WORDPRESS_ADMIN_USER,
};
use runtime::{HealthDiagnosticsInfo, RuntimeInfo, RuntimeManager, RuntimeState};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, TryLockError};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, State};

const TRAY_OPEN_ID: &str = "tray-open";
const TRAY_EXIT_ID: &str = "tray-exit";

#[derive(Default)]
struct ShellState {
    store: Mutex<Option<Store>>,
    runtime: Mutex<Option<RuntimeManager>>,
    lifecycle: Mutex<()>,
    lifecycle_requested: AtomicBool,
    exit_authorized: AtomicBool,
    shutdown_in_progress: AtomicBool,
    support_export_in_progress: AtomicBool,
    backup: Mutex<BackupOperationState>,
    #[cfg(debug_assertions)]
    provisioning: Mutex<()>,
}

#[derive(Serialize)]
struct ShellInfo {
    version: &'static str,
    data_dir: String,
    config: AppConfig,
}

fn application_data_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(value) = std::env::var_os("COFFEEPOS_DESKTOP_DATA_ROOT") {
        let root = PathBuf::from(value);
        if !root.is_absolute() {
            return Err(
                "COFFEEPOS_DESKTOP_DATA_ROOT must be an absolute path in development builds."
                    .into(),
            );
        }
        return Ok(root);
    }
    app.path()
        .app_local_data_dir()
        .map_err(|e| format!("Cannot locate application data: {e}. Check your OS user profile."))
}

fn managed_data_root(app: &tauri::AppHandle, state: &ShellState) -> Result<PathBuf, String> {
    match state.store.try_lock() {
        Ok(guard) => Ok(guard
            .as_ref()
            .map(|store| store.root.clone())
            .unwrap_or(application_data_root(app)?)),
        Err(TryLockError::WouldBlock) => application_data_root(app),
        Err(TryLockError::Poisoned(_)) => Err(
            "Application state unavailable. Restart CoffeePOS Desktop before reading logs.".into(),
        ),
    }
}

#[derive(Serialize)]
struct SetupInfo {
    store_name: String,
    admin_username: String,
    admin_email: String,
    password_configured: bool,
    editable: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(debug_assertions), allow(dead_code))]
struct RepairInputs {
    admin_password: Option<String>,
}

#[cfg(debug_assertions)]
const WORDPRESS_ADMIN_PENDING_SECRET: &str = "config/wordpress-admin.pending.secret";

#[cfg(debug_assertions)]
fn persist_setup_profile_transaction(
    store: &mut Store,
    store_name: &str,
    admin_username: &str,
    admin_email: &str,
    admin_password: Option<&str>,
) -> Result<(), String> {
    let secret_path = store.root.join(WORDPRESS_ADMIN_SECRET);
    let pending_secret_path = store.root.join(WORDPRESS_ADMIN_PENDING_SECRET);
    if pending_secret_path.exists() && admin_password.is_none() {
        return Err("A previous administrator-password save did not finish. Re-enter the password to replace the incomplete staged credential before continuing.".into());
    }
    let already_configured = secret::load(&secret_path)
        .map(|value| !value.is_empty())
        .unwrap_or(false);
    if admin_password.is_none() && !already_configured {
        return Err("Set an administrator password before starting installation.".into());
    }

    if let Some(password) = admin_password {
        // Keep a durable pending marker through every metadata failure/crash boundary. The marker
        // is removed only by `promote_staged_password` after config commit and active-secret
        // verification, so relaunch can never mistake an older active password for the new profile.
        secret::store_password(&pending_secret_path, password)?;
        store.save_setup_profile(store_name, admin_username, admin_email)?;
        secret::promote_staged_password(&pending_secret_path, &secret_path)?;
    } else {
        store.save_setup_profile(store_name, admin_username, admin_email)?;
    }
    Ok(())
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
        let root = application_data_root(app)?;
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
        let root = application_data_root(app)?;
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

fn with_runtime<T>(
    _app: &tauri::AppHandle,
    state: &ShellState,
    operation: impl FnOnce(&mut RuntimeManager) -> Result<T, runtime::RuntimeErrorInfo>,
) -> Result<T, String> {
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

#[cfg(debug_assertions)]
fn inspect_provisioning(
    app: &tauri::AppHandle,
    state: &ShellState,
) -> Result<ProvisioningInfo, String> {
    let backup_active = state
        .backup
        .lock()
        .map_err(|_| "Backup state unavailable. Restart CoffeePOS Desktop.".to_string())?
        .active();
    let provisioning_guard = match state.provisioning.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::WouldBlock) => None,
        Err(TryLockError::Poisoned(_)) => {
            return Err(
                "Provisioning state unavailable. Restart CoffeePOS Desktop before retrying.".into(),
            );
        }
    };
    let root = data_root(app, state)?;
    let (project_root, manifest) = development_runtime_paths()?;
    let resolved = runtime::resolve_development_manifest(&project_root, &manifest)
        .map_err(|error| error.to_string())?;
    let provisioner = Provisioner::from_development(&project_root, &manifest, resolved, root)
        .map_err(|error| error.to_string())?;
    if provisioning_guard.is_some() || backup_active {
        Ok(provisioner.inspect())
    } else {
        Ok(provisioner.installing_info())
    }
}

#[cfg(all(debug_assertions, windows))]
fn open_system_browser(url: &str) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation: Vec<u16> = OsStr::new("open").encode_wide().chain(once(0)).collect();
    let target: Vec<u16> = OsStr::new(url).encode_wide().chain(once(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        Err(format!(
            "Windows could not open the managed local URL in the system browser (ShellExecuteW code {}). Check the default browser association and retry.",
            result as isize
        ))
    } else {
        Ok(())
    }
}

#[cfg(all(debug_assertions, not(windows)))]
fn open_system_browser(_url: &str) -> Result<(), String> {
    Err("Opening managed local URLs in the system browser is currently qualified only for Windows x64 development builds.".into())
}

fn try_lifecycle<'a>(state: &'a ShellState, operation: &str) -> Result<MutexGuard<'a, ()>, String> {
    match state.lifecycle.try_lock() {
        Ok(guard) => Ok(guard),
        Err(TryLockError::WouldBlock) => Err(format!(
            "Runtime lifecycle is busy while trying to {operation}. Wait for the current install/start/stop/restart operation to finish, then retry."
        )),
        Err(TryLockError::Poisoned(_)) => Err(
            "Runtime lifecycle state unavailable. Restart CoffeePOS Desktop before retrying."
                .into(),
        ),
    }
}

#[cfg(debug_assertions)]
fn try_provisioning<'a>(
    state: &'a ShellState,
    operation: &str,
) -> Result<MutexGuard<'a, ()>, String> {
    match state.provisioning.try_lock() {
        Ok(guard) => Ok(guard),
        Err(TryLockError::WouldBlock) => Err(format!(
            "Provisioning is busy while trying to {operation}. Wait for the current install/repair operation to finish, then retry."
        )),
        Err(TryLockError::Poisoned(_)) => Err(
            "Provisioning state unavailable. Restart CoffeePOS Desktop before retrying.".into(),
        ),
    }
}

async fn run_runtime_blocking<T, F>(
    app: tauri::AppHandle,
    lifecycle_operation: &'static str,
    operation: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&mut RuntimeManager) -> Result<T, runtime::RuntimeErrorInfo> + Send + 'static,
{
    {
        let state = app.state::<ShellState>();
        if state.lifecycle_requested.swap(true, Ordering::AcqRel) {
            return Err(format!(
                "Runtime lifecycle is busy while trying to {lifecycle_operation}. Wait for the current operation to finish, then retry."
            ));
        }
    }

    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = worker_app.state::<ShellState>();
        let _lifecycle_guard = try_lifecycle(&state, lifecycle_operation)?;
        with_runtime(&worker_app, &state, operation)
    })
    .await;
    app.state::<ShellState>()
        .lifecycle_requested
        .store(false, Ordering::Release);
    result.map_err(|error| format!("Runtime worker failed: {error}."))?
}

async fn read_runtime_blocking(
    app: tauri::AppHandle,
    operation: fn(&mut RuntimeManager) -> RuntimeInfo,
) -> Result<RuntimeInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<ShellState>();
        with_runtime(&app, &state, |runtime| Ok(operation(runtime)))
    })
    .await
    .map_err(|error| format!("Runtime status worker failed: {error}."))?
}

#[cfg(windows)]
fn shutdown_message_box(text: &str, title: &str, flags: u32) -> i32 {
    use std::ptr;
    use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;

    let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { MessageBoxW(ptr::null_mut(), text.as_ptr(), title.as_ptr(), flags) }
}

#[cfg(windows)]
fn confirm_runtime_exit() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO,
    };

    shutdown_message_box(
        "Dừng và thoát sẽ ngắt kết nối POS và các thiết bị đang dùng cửa hàng này. Thao tác này không chốt ca và không tự thay đổi trạng thái thanh toán.\n\nChọn Có để dừng cửa hàng và thoát, hoặc Không để ở lại.",
        "Dừng cửa hàng và thoát CoffeePOS?",
        MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
    ) == IDYES
}

#[cfg(not(windows))]
fn confirm_runtime_exit() -> bool {
    false
}

#[cfg(windows)]
fn show_shutdown_notice(title: &str, message: &str, error: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONWARNING, MB_OK};
    let icon = if error { MB_ICONERROR } else { MB_ICONWARNING };
    let _ = shutdown_message_box(message, title, MB_OK | icon);
}

#[cfg(not(windows))]
fn show_shutdown_notice(_title: &str, _message: &str, _error: bool) {}

fn show_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

fn request_full_exit(app: tauri::AppHandle) {
    let state = app.state::<ShellState>();
    if state.exit_authorized.load(Ordering::Acquire) {
        return;
    }
    if state.shutdown_in_progress.swap(true, Ordering::AcqRel) {
        return;
    }
    if state.lifecycle_requested.swap(true, Ordering::AcqRel) {
        state.shutdown_in_progress.store(false, Ordering::Release);
        show_main_window(&app);
        show_shutdown_notice(
            "CoffeePOS đang bận",
            "CoffeePOS đang hoàn tất một thao tác hệ thống. Hãy thử thoát lại sau khi thao tác hiện tại kết thúc.",
            false,
        );
        return;
    }

    let lifecycle_guard = match state.lifecycle.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::WouldBlock) => {
            state.lifecycle_requested.store(false, Ordering::Release);
            state.shutdown_in_progress.store(false, Ordering::Release);
            show_main_window(&app);
            show_shutdown_notice(
                "CoffeePOS đang bận",
                "CoffeePOS đang hoàn tất cài đặt hoặc thay đổi trạng thái hệ thống. Hãy chờ thao tác hiện tại kết thúc rồi thử thoát lại.",
                false,
            );
            return;
        }
        Err(TryLockError::Poisoned(_)) => {
            state.lifecycle_requested.store(false, Ordering::Release);
            state.shutdown_in_progress.store(false, Ordering::Release);
            show_main_window(&app);
            show_shutdown_notice(
                "Không thể thoát an toàn",
                "Trạng thái vòng đời runtime không còn khả dụng. Hãy giữ ứng dụng mở và kiểm tra Chẩn đoán trước khi thử lại.",
                true,
            );
            return;
        }
    };

    let runtime_guard = match state.runtime.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::WouldBlock) => {
            state.lifecycle_requested.store(false, Ordering::Release);
            state.shutdown_in_progress.store(false, Ordering::Release);
            show_main_window(&app);
            show_shutdown_notice(
                "CoffeePOS đang bận",
                "CoffeePOS đang cập nhật trạng thái hệ thống. Hãy thử thoát lại sau khi thao tác hiện tại kết thúc.",
                false,
            );
            return;
        }
        Err(TryLockError::Poisoned(_)) => {
            state.lifecycle_requested.store(false, Ordering::Release);
            state.shutdown_in_progress.store(false, Ordering::Release);
            show_main_window(&app);
            show_shutdown_notice(
                "Không thể thoát an toàn",
                "Không thể đọc trạng thái runtime để dừng cửa hàng an toàn. Hãy giữ ứng dụng mở và thử lại.",
                true,
            );
            return;
        }
    };

    if let Some(runtime) = runtime_guard.as_ref() {
        if runtime.requires_exit_confirmation() && !confirm_runtime_exit() {
            drop(runtime_guard);
            drop(lifecycle_guard);
            state.lifecycle_requested.store(false, Ordering::Release);
            state.shutdown_in_progress.store(false, Ordering::Release);
            return;
        }
    }
    drop(runtime_guard);
    drop(lifecycle_guard);

    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<ShellState>();
        let result = (|| -> Result<(), String> {
            let _lifecycle_guard = try_lifecycle(&state, "stop the runtime for exit")?;
            let mut runtime_guard = state
                .runtime
                .lock()
                .map_err(|_| "Runtime state unavailable. Restart CoffeePOS Desktop.".to_string())?;
            if let Some(runtime) = runtime_guard.as_mut() {
                if let Err(error) = runtime.stop() {
                    if runtime.requires_exit_confirmation() {
                        return Err(format!(
                            "CoffeePOS chưa dừng hoàn toàn nên ứng dụng vẫn mở để tránh bỏ lại tiến trình.\n\n{error}"
                        ));
                    }
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                state.exit_authorized.store(true, Ordering::Release);
                app.exit(0);
            }
            Err(message) => {
                state.lifecycle_requested.store(false, Ordering::Release);
                state.shutdown_in_progress.store(false, Ordering::Release);
                show_main_window(&app);
                show_shutdown_notice("Chưa thể dừng cửa hàng", &message, true);
            }
        }
    });
}

fn logs_command_error(action: &str, code: &str, message: &str, recovery: &str) -> LogErrorInfo {
    LogErrorInfo {
        component: "logs".into(),
        action: action.into(),
        code: code.into(),
        message: message.into(),
        recovery: recovery.into(),
    }
}

fn backup_command_error(
    action: &str,
    code: &str,
    message: &str,
    recovery: &str,
) -> BackupErrorInfo {
    BackupErrorInfo {
        component: "backup".into(),
        action: action.into(),
        code: code.into(),
        message: message.into(),
        recovery: recovery.into(),
    }
}

async fn run_backup_dialog_operation<T: Send + 'static>(
    app: tauri::AppHandle,
    action: &'static str,
    backup_password: String,
    operation: fn(&Path, &str, &BackupCompatibilityTarget) -> Result<T, BackupErrorInfo>,
) -> Result<T, BackupErrorInfo> {
    #[cfg(debug_assertions)]
    {
        let backup_password = zeroize::Zeroizing::new(backup_password);
        let restore_root = application_data_root(&app).map_err(|_| {
            backup_command_error(
                action,
                "disk_capacity_unavailable",
                "CoffeePOS cannot resolve the managed restore data volume.",
                "Repair the CoffeePOS data path and retry backup inspection.",
            )
        })?;
        tauri::async_runtime::spawn_blocking(move || {
            let selected = backup_format::choose_backup_file(action)?;
            let path = selected.ok_or_else(|| {
                backup_command_error(
                    action,
                    "selection_cancelled",
                    "No CoffeePOS backup file was selected.",
                    "Choose a .coffeepos-backup file when you are ready to continue.",
                )
            })?;
            let (project_root, runtime_manifest) = development_runtime_paths().map_err(|_| {
                backup_command_error(
                    action,
                    "target_compatibility_unavailable",
                    "CoffeePOS cannot resolve the pinned target versions needed to inspect this backup.",
                    "Repair the managed runtime artifacts before retrying backup inspection.",
                )
            })?;
            let target = backup_format::load_development_compatibility_target(
                &project_root,
                &runtime_manifest,
                &restore_root,
                action,
            )?;
            operation(&path, backup_password.as_str(), &target)
        })
        .await
        .map_err(|_| {
            backup_command_error(
                action,
                "worker_failed",
                "CoffeePOS could not complete the backup operation.",
                "Retry the operation or restart CoffeePOS Desktop.",
            )
        })?
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = backup_password;
        let _ = operation;
        Err(backup_command_error(
            action,
            "runtime_artifacts_unavailable",
            "Backup inspection is not available until managed runtime artifacts are packaged for this build.",
            "Use the qualified Windows development build until runtime packaging is completed.",
        ))
    }
}

#[tauri::command]
async fn inspect_backup(
    app: tauri::AppHandle,
    backup_password: String,
) -> Result<BackupInspection, BackupErrorInfo> {
    run_backup_dialog_operation(
        app,
        action_inspect(),
        backup_password,
        backup_format::inspect_backup,
    )
    .await
}

#[tauri::command]
async fn validate_backup(
    app: tauri::AppHandle,
    backup_password: String,
) -> Result<BackupValidation, BackupErrorInfo> {
    run_backup_dialog_operation(
        app,
        action_validate(),
        backup_password,
        backup_format::validate_backup,
    )
    .await
}

const fn action_inspect() -> &'static str {
    "inspect"
}

const fn action_validate() -> &'static str {
    "validate"
}

#[tauri::command]
fn get_backup_status(state: State<'_, ShellState>) -> Result<BackupStatus, BackupErrorInfo> {
    state
        .backup
        .lock()
        .map(|guard| guard.status.clone())
        .map_err(|_| {
            backup_command_error(
                "status",
                "backup_state_unavailable",
                "CoffeePOS cannot read the current backup operation state.",
                "Restart CoffeePOS Desktop before retrying backup.",
            )
        })
}

#[tauri::command]
fn cancel_backup(
    state: State<'_, ShellState>,
    operation_id: String,
) -> Result<BackupStatus, BackupErrorInfo> {
    let guard = state.backup.lock().map_err(|_| {
        backup_command_error(
            "cleanup",
            "backup_state_unavailable",
            "CoffeePOS cannot signal the current backup operation.",
            "Keep CoffeePOS open and retry after the current operation settles.",
        )
    })?;
    guard.request_cancel(&operation_id)?;
    Ok(guard.status.clone())
}

#[tauri::command]
fn open_backup_folder(
    state: State<'_, ShellState>,
    operation_id: String,
) -> Result<(), BackupErrorInfo> {
    let destination = {
        let guard = state.backup.lock().map_err(|_| {
            backup_command_error(
                "finalize",
                "backup_state_unavailable",
                "CoffeePOS cannot read the completed backup destination.",
                "Restart CoffeePOS Desktop and open the destination manually.",
            )
        })?;
        guard
            .last_destination
            .as_ref()
            .filter(|(id, _)| id == &operation_id)
            .map(|(_, path)| path.clone())
            .ok_or_else(|| {
                backup_command_error(
                    "finalize",
                    "backup_destination_unavailable",
                    "The requested completed backup destination is no longer available in this app session.",
                    "Open the destination manually or create another backup.",
                )
            })?
    };
    backup::open_backup_folder(&destination)
}

#[tauri::command]
async fn create_backup(
    app: tauri::AppHandle,
    backup_password: String,
) -> Result<BackupResult, BackupErrorInfo> {
    #[cfg(debug_assertions)]
    {
        let backup_password = zeroize::Zeroizing::new(backup_password);
        let state = app.state::<ShellState>();
        let shell = with_store(&app, &state, |_| Ok(())).map_err(|_| {
            backup_command_error(
                "preflight",
                "store_config_unavailable",
                "CoffeePOS cannot read the managed store configuration for backup.",
                "Repair the application configuration and retry backup.",
            )
        })?;
        let operation_id = backup::new_operation_id()?;
        let cancelled = {
            let mut guard = state.backup.lock().map_err(|_| {
                backup_command_error(
                    "preflight",
                    "backup_state_unavailable",
                    "CoffeePOS cannot reserve a backup operation.",
                    "Restart CoffeePOS Desktop before retrying backup.",
                )
            })?;
            guard.begin(operation_id.clone())?
        };

        let suggested = backup_format::default_backup_file_name(&shell.config.store_name);
        let selected = tauri::async_runtime::spawn_blocking(move || {
            backup_format::choose_backup_destination(&suggested)
        })
        .await
        .map_err(|_| {
            backup_command_error(
                "preflight",
                "save_dialog_worker_failed",
                "CoffeePOS could not complete the backup destination picker.",
                "Retry backup creation or restart CoffeePOS Desktop.",
            )
        })?;
        let destination = match selected {
            Ok(Some(path)) => path,
            Ok(None) => {
                if let Ok(mut guard) = state.backup.lock() {
                    guard.finish_cancelled(&operation_id);
                }
                return Ok(BackupResult {
                    operation_id,
                    status: "cancelled".into(),
                });
            }
            Err(error) => {
                if let Ok(mut guard) = state.backup.lock() {
                    guard.finish_failed(&operation_id, error.clone());
                }
                return Err(error);
            }
        };
        if cancelled.load(Ordering::Acquire) {
            if let Ok(mut guard) = state.backup.lock() {
                guard.finish_cancelled(&operation_id);
            }
            return Ok(BackupResult {
                operation_id,
                status: "cancelled".into(),
            });
        }

        if state.lifecycle_requested.swap(true, Ordering::AcqRel) {
            let error = backup_command_error(
                "preflight",
                "lifecycle_busy",
                "CoffeePOS is already completing another managed lifecycle operation.",
                "Wait for the current operation to finish, then retry backup.",
            );
            if let Ok(mut guard) = state.backup.lock() {
                guard.finish_failed(&operation_id, error.clone());
            }
            return Err(error);
        }

        let worker_app = app.clone();
        let worker_operation_id = operation_id.clone();
        let worker_cancelled = cancelled.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let state = worker_app.state::<ShellState>();
            if let Ok(mut guard) = state.backup.lock() {
                guard.update(backup::BackupProgress {
                    operation_id: worker_operation_id.clone(),
                    stage: backup::BackupStage::Preflight,
                    processed_files: 0,
                    estimated_files: 0,
                    processed_bytes: 0,
                    estimated_bytes: 0,
                    warnings: Vec::new(),
                });
            }
            let _lifecycle_guard = try_lifecycle(&state, "create a portable store backup")
                .map_err(|error| {
                    backup_command_error(
                        "preflight",
                        "lifecycle_busy",
                        &error,
                        "Wait for the current managed operation to finish, then retry backup.",
                    )
                })?;
            let provisioning = inspect_provisioning(&worker_app, &state).map_err(|error| {
                backup_command_error(
                    "preflight",
                    "provisioning_state_unavailable",
                    &error,
                    "Repair provisioning state before creating a portable backup.",
                )
            })?;
            if provisioning.state != ProvisioningState::Ready {
                return Err(backup_command_error(
                    "preflight",
                    "store_not_ready",
                    "CoffeePOS can create a portable backup only from a fully provisioned ready store.",
                    "Finish or repair store provisioning, then retry backup.",
                ));
            }
            let admin_username = provisioning.admin_username.ok_or_else(|| {
                backup_command_error(
                    "preflight",
                    "administrator_identity_unavailable",
                    "The ready store does not expose its managed administrator identity.",
                    "Repair provisioning metadata before retrying backup.",
                )
            })?;
            let _provisioning_guard = try_provisioning(&state, "create a portable store backup")
                .map_err(|error| {
                    backup_command_error(
                        "preflight",
                        "provisioning_busy",
                        &error,
                        "Wait for provisioning or repair to finish, then retry backup.",
                    )
                })?;
            let root = data_root(&worker_app, &state).map_err(|_| {
                backup_command_error(
                    "preflight",
                    "data_root_unavailable",
                    "CoffeePOS cannot resolve the managed data root for backup.",
                    "Repair the application data path before retrying backup.",
                )
            })?;
            let (project_root, manifest) = development_runtime_paths().map_err(|_| {
                backup_command_error(
                    "preflight",
                    "runtime_artifacts_unavailable",
                    "CoffeePOS cannot resolve the pinned managed runtime needed for backup.",
                    "Restage the managed runtime artifacts before retrying backup.",
                )
            })?;
            let target = backup_format::load_development_compatibility_target(
                &project_root,
                &manifest,
                &root,
                "create",
            )?;
            let context = backup::BackupCreateContext {
                data_root: root.clone(),
                admin_username,
                source: target.source.clone(),
            };

            let mut runtime_guard = state.runtime.lock().map_err(|_| {
                backup_command_error(
                    "preflight",
                    "runtime_state_unavailable",
                    "CoffeePOS cannot access the managed runtime for backup.",
                    "Restart CoffeePOS Desktop before retrying backup.",
                )
            })?;
            if runtime_guard.is_none() {
                *runtime_guard = Some(
                    RuntimeManager::from_development(&project_root, &manifest, root.clone())
                        .map_err(|error| {
                            backup_command_error(
                                "preflight",
                                "runtime_state_unavailable",
                                &error.message,
                                &error.recovery,
                            )
                        })?,
                );
            }
            let runtime = runtime_guard.as_mut().ok_or_else(|| {
                backup_command_error(
                    "preflight",
                    "runtime_state_unavailable",
                    "CoffeePOS runtime manager is unavailable for backup.",
                    "Restart CoffeePOS Desktop before retrying backup.",
                )
            })?;
            if runtime.backup_maintenance_active() {
                backup_database::retry_database_backup_cleanup(runtime).map_err(|error| {
                    backup_command_error(
                        "cleanup",
                        &error.code,
                        &error.message,
                        &error.recovery,
                    )
                })?;
                backup::recover_interrupted_backup(&root)?;
            }

            let (destination, upload_files, upload_bytes, warnings) =
                backup::preflight_before_quiesce(&context, &destination)?;
            if let Ok(mut guard) = state.backup.lock() {
                guard.update(backup::BackupProgress {
                    operation_id: worker_operation_id.clone(),
                    stage: backup::BackupStage::Preflight,
                    processed_files: 0,
                    estimated_files: upload_files.saturating_add(3),
                    processed_bytes: 0,
                    estimated_bytes: upload_bytes,
                    warnings: backup::warning_codes(&warnings),
                });
            }
            let progress_state = &state;
            backup::run_backup(
                runtime,
                backup::BackupRunRequest {
                    context: &context,
                    destination: &destination,
                    operation_id: &worker_operation_id,
                    backup_password: backup_password.as_str(),
                    cancelled: worker_cancelled.as_ref(),
                    warnings,
                },
                |progress| {
                    if let Ok(mut guard) = progress_state.backup.lock() {
                        guard.update(progress);
                    }
                },
            )?;
            Ok(destination.path.clone())
        })
        .await;
        state.lifecycle_requested.store(false, Ordering::Release);

        match result {
            Ok(Ok(destination)) => {
                let warnings = state
                    .backup
                    .lock()
                    .ok()
                    .map(|guard| guard.status.warnings.clone())
                    .unwrap_or_default();
                if let Ok(mut guard) = state.backup.lock() {
                    guard.finish_success(&operation_id, destination, warnings);
                }
                Ok(BackupResult {
                    operation_id,
                    status: "succeeded".into(),
                })
            }
            Ok(Err(error)) if error.code == "cancelled" => {
                if let Ok(mut guard) = state.backup.lock() {
                    guard.finish_cancelled(&operation_id);
                }
                Ok(BackupResult {
                    operation_id,
                    status: "cancelled".into(),
                })
            }
            Ok(Err(error)) => {
                if let Ok(mut guard) = state.backup.lock() {
                    guard.finish_failed(&operation_id, error.clone());
                }
                Err(error)
            }
            Err(_) => {
                let error = backup_command_error(
                    "cleanup",
                    "backup_worker_failed",
                    "CoffeePOS backup worker terminated unexpectedly.",
                    "Keep CoffeePOS open, reload the application so interrupted-backup recovery can run, then inspect runtime health.",
                );
                if let Ok(mut guard) = state.backup.lock() {
                    guard.finish_failed(&operation_id, error.clone());
                }
                Err(error)
            }
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = backup_password;
        Err(backup_command_error(
            "preflight",
            "runtime_artifacts_unavailable",
            "Portable backup creation is not available until managed runtime artifacts are packaged for this build.",
            "Use the qualified Windows development build until runtime packaging is completed.",
        ))
    }
}

#[tauri::command]
async fn get_log_catalog(app: tauri::AppHandle) -> Result<LogCatalog, LogErrorInfo> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<ShellState>();
        let root = managed_data_root(&app, &state).map_err(|_| {
            logs_command_error(
                "catalog",
                "data_root_unavailable",
                "CoffeePOS cannot resolve the managed log directory.",
                "Restart CoffeePOS Desktop and retry.",
            )
        })?;
        logs::get_log_catalog(&root)
    })
    .await
    .map_err(|_| {
        logs_command_error(
            "catalog",
            "worker_failed",
            "CoffeePOS could not complete the log catalog operation.",
            "Retry the log view or restart CoffeePOS Desktop.",
        )
    })?
}

#[tauri::command]
async fn read_log_page(
    app: tauri::AppHandle,
    log_id: String,
    cursor: Option<String>,
    direction: Option<String>,
    max_lines: Option<usize>,
) -> Result<LogPage, LogErrorInfo> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<ShellState>();
        let root = managed_data_root(&app, &state).map_err(|_| {
            logs_command_error(
                "read",
                "data_root_unavailable",
                "CoffeePOS cannot resolve the managed log directory.",
                "Restart CoffeePOS Desktop and retry.",
            )
        })?;
        logs::read_log_page(
            &root,
            &log_id,
            cursor.as_deref(),
            direction.as_deref(),
            max_lines,
        )
    })
    .await
    .map_err(|_| {
        logs_command_error(
            "read",
            "worker_failed",
            "CoffeePOS could not complete the log read operation.",
            "Refresh the selected log or restart CoffeePOS Desktop.",
        )
    })?
}

#[tauri::command]
async fn export_support_bundle(app: tauri::AppHandle) -> Result<SupportBundleResult, LogErrorInfo> {
    {
        let state = app.state::<ShellState>();
        if state
            .support_export_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(logs_command_error(
                "export",
                "export_in_progress",
                "CoffeePOS is already creating a support bundle.",
                "Wait for the current export to finish before trying again.",
            ));
        }
    }

    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        struct ExportFlagReset<'a>(&'a AtomicBool);
        impl Drop for ExportFlagReset<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }

        let state = worker_app.state::<ShellState>();
        let _reset = ExportFlagReset(&state.support_export_in_progress);
        let root = managed_data_root(&worker_app, &state).map_err(|_| {
            logs_command_error(
                "export",
                "data_root_unavailable",
                "CoffeePOS cannot resolve the managed support data directory.",
                "Restart CoffeePOS Desktop and retry support-bundle export.",
            )
        })?;
        let default_name = logs::default_support_bundle_name();
        let Some(destination) = logs::choose_support_bundle_destination(&default_name)? else {
            return Ok(SupportBundleResult::cancelled());
        };
        let runtime_snapshot = match state.runtime.try_lock() {
            Ok(guard) => guard.as_ref().map(RuntimeManager::info),
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(_)) => {
                return Err(logs_command_error(
                    "export",
                    "runtime_snapshot_unavailable",
                    "CoffeePOS cannot read the cached runtime state for the support bundle.",
                    "Restart CoffeePOS Desktop and retry export.",
                ));
            }
        };
        logs::export_support_bundle(&root, &destination, runtime_snapshot.as_ref())
    })
    .await;

    if result.is_err() {
        app.state::<ShellState>()
            .support_export_in_progress
            .store(false, Ordering::Release);
    }
    result.map_err(|_| {
        logs_command_error(
            "export",
            "worker_failed",
            "CoffeePOS could not complete support-bundle export.",
            "Retry export or restart CoffeePOS Desktop.",
        )
    })?
}

#[tauri::command]
fn get_shell_info(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<ShellInfo, String> {
    let backup_active = state
        .backup
        .lock()
        .map_err(|_| "Backup state unavailable. Restart CoffeePOS Desktop.".to_string())?
        .active();
    // An active backup owns the runtime mutex for its full maintenance window. WebView reloads
    // still need shell metadata so they can reconnect to get_backup_status/cancel_backup instead
    // of blocking behind that mutex until the backup is already over.
    let fresh_runtime = if backup_active {
        false
    } else {
        state
            .runtime
            .lock()
            .map_err(|_| "Runtime state unavailable. Restart CoffeePOS Desktop.".to_string())?
            .is_none()
    };
    if fresh_runtime && !backup_active {
        let root = application_data_root(&app)?;
        backup::recover_interrupted_backup(&root)
            .map_err(|error| format!("{} {}", error.message, error.recovery))?;
    }
    with_store(&app, &state, |_| Ok(()))
}

#[tauri::command]
fn save_app_settings(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
    startup_view: StartupView,
) -> Result<ShellInfo, String> {
    let _lifecycle_guard = try_lifecycle(&state, "save application settings")?;
    with_store(&app, &state, |store| store.save_startup_view(startup_view))
}

#[cfg(debug_assertions)]
fn read_setup_info(app: &tauri::AppHandle, state: &ShellState) -> Result<SetupInfo, String> {
    let shell = with_store(app, state, |_| Ok(()))?;
    let provisioning = inspect_provisioning(app, state)?;
    let root = PathBuf::from(&shell.data_dir);
    let pending_password = root.join(WORDPRESS_ADMIN_PENDING_SECRET).exists();
    let password_configured = !pending_password
        && secret::load(&root.join(WORDPRESS_ADMIN_SECRET))
            .map(|value| !value.is_empty())
            .unwrap_or(false);
    let admin_username = provisioning
        .admin_username
        .or(shell.config.setup_admin_username.clone())
        .unwrap_or_else(|| WORDPRESS_ADMIN_USER.into());
    let admin_email = shell
        .config
        .setup_admin_email
        .clone()
        .unwrap_or_else(|| WORDPRESS_ADMIN_EMAIL.into());
    Ok(SetupInfo {
        store_name: shell.config.store_name,
        admin_username,
        admin_email,
        password_configured,
        editable: provisioning.state == ProvisioningState::NotInstalled,
    })
}

#[tauri::command]
fn get_setup_info(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<SetupInfo, String> {
    #[cfg(debug_assertions)]
    {
        read_setup_info(&app, &state)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = state;
        Err("Setup onboarding is currently qualified only for the Windows development build until runtime resources are packaged.".into())
    }
}

#[tauri::command]
fn save_setup_profile(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
    store_name: String,
    admin_username: String,
    admin_email: String,
    admin_password: Option<String>,
) -> Result<SetupInfo, String> {
    #[cfg(debug_assertions)]
    {
        let _lifecycle_guard = try_lifecycle(&state, "save the initial store profile")?;
        let provisioning = inspect_provisioning(&app, &state)?;
        if provisioning.state != ProvisioningState::NotInstalled {
            return Err("Initial store/account details are locked because provisioning has already started. Continue setup with the existing account; CoffeePOS Desktop will not reset its credentials.".into());
        }
        if let Some(password) = admin_password.as_deref() {
            let count = password.chars().count();
            if !(12..=128).contains(&count) || password.chars().any(char::is_control) {
                return Err("Administrator password must contain 12–128 characters without control characters.".into());
            }
        }

        with_store(&app, &state, |store| {
            persist_setup_profile_transaction(
                store,
                &store_name,
                &admin_username,
                &admin_email,
                admin_password.as_deref(),
            )
        })?;
        read_setup_info(&app, &state)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = state;
        let _ = store_name;
        let _ = admin_username;
        let _ = admin_email;
        let _ = admin_password;
        Err("Setup onboarding is currently qualified only for the Windows development build until runtime resources are packaged.".into())
    }
}

#[cfg(all(debug_assertions, windows))]
fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    use std::ptr;
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * std::mem::size_of::<u16>();
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
    if memory.is_null() {
        return Err("Windows could not allocate clipboard memory.".into());
    }
    let pointer = unsafe { GlobalLock(memory) } as *mut u16;
    if pointer.is_null() {
        unsafe {
            let _ = GlobalFree(memory);
        }
        return Err("Windows could not lock clipboard memory.".into());
    }
    unsafe {
        ptr::copy_nonoverlapping(wide.as_ptr(), pointer, wide.len());
        let _ = GlobalUnlock(memory);
    }
    if unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 {
        unsafe {
            let _ = GlobalFree(memory);
        }
        return Err(
            "Windows clipboard is busy. Close the application currently using it and retry.".into(),
        );
    }
    let result = unsafe {
        let _ = EmptyClipboard();
        SetClipboardData(CF_UNICODETEXT.into(), memory)
    };
    unsafe {
        let _ = CloseClipboard();
    }
    if result.is_null() {
        unsafe {
            let _ = GlobalFree(memory);
        }
        return Err("Windows could not copy the administrator password to the clipboard.".into());
    }
    Ok(())
}

#[cfg(all(debug_assertions, not(windows)))]
fn copy_text_to_clipboard(_text: &str) -> Result<(), String> {
    Err(
        "Copying the protected administrator password is currently qualified only on Windows."
            .into(),
    )
}

#[tauri::command]
fn copy_admin_password(app: tauri::AppHandle, state: State<'_, ShellState>) -> Result<(), String> {
    #[cfg(debug_assertions)]
    {
        let provisioning = inspect_provisioning(&app, &state)?;
        if provisioning.admin_username.is_none() {
            return Err(
                "Administrator credentials are not available yet. Finish the account step first."
                    .into(),
            );
        }
        let root = data_root(&app, &state)?;
        if root.join(WORDPRESS_ADMIN_PENDING_SECRET).exists() {
            return Err("Administrator credential update is incomplete. Return to setup and save the administrator password again before copying or provisioning.".into());
        }
        let password = secret::load(&root.join(WORDPRESS_ADMIN_SECRET))?;
        copy_text_to_clipboard(&password)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = state;
        Err("Administrator credential copy is currently qualified only for the Windows development build.".into())
    }
}

#[tauri::command]
async fn get_runtime_info(app: tauri::AppHandle) -> Result<RuntimeInfo, String> {
    read_runtime_blocking(app, RuntimeManager::refresh).await
}

#[tauri::command]
async fn start_runtime(app: tauri::AppHandle) -> Result<RuntimeInfo, String> {
    run_runtime_blocking(app, "start the runtime", RuntimeManager::start).await
}

#[tauri::command]
async fn stop_runtime(app: tauri::AppHandle) -> Result<RuntimeInfo, String> {
    run_runtime_blocking(app, "stop the runtime", RuntimeManager::stop).await
}

#[tauri::command]
async fn restart_runtime(app: tauri::AppHandle) -> Result<RuntimeInfo, String> {
    run_runtime_blocking(app, "restart the runtime", RuntimeManager::restart).await
}

#[tauri::command]
async fn retry_runtime_health(app: tauri::AppHandle) -> Result<RuntimeInfo, String> {
    run_runtime_blocking(app, "recheck runtime health", |runtime| {
        let info = runtime.refresh();
        if info.state != RuntimeState::Running {
            return Err(runtime::RuntimeErrorInfo {
                component: "runtime".into(),
                operation: "health retry".into(),
                message: "The local store is not running.".into(),
                recovery: "Start the store before retrying its health check.".into(),
            });
        }
        Ok(runtime.refresh_wordpress_health())
    })
    .await
}

#[tauri::command]
async fn get_health_diagnostics(app: tauri::AppHandle) -> Result<HealthDiagnosticsInfo, String> {
    run_runtime_blocking(app, "check health diagnostics", |runtime| {
        Ok(runtime.health_diagnostics())
    })
    .await
}

#[cfg(debug_assertions)]
fn validate_repair_admin_password_input(password: &str) -> Result<(), String> {
    let count = password.chars().count();
    if !(12..=128).contains(&count) || password.chars().any(char::is_control) {
        return Err(
            "Administrator password must contain 12–128 characters without control characters."
                .into(),
        );
    }
    Ok(())
}

#[cfg(debug_assertions)]
fn diagnostics_all_healthy(info: &HealthDiagnosticsInfo) -> bool {
    use runtime::ComponentHealthState;
    [
        &info.database,
        &info.php,
        &info.wordpress,
        &info.woocommerce,
        &info.coffeepos,
    ]
    .into_iter()
    .all(|component| component.state == ComponentHealthState::Healthy)
}

#[tauri::command]
async fn get_repair_plan(app: tauri::AppHandle) -> Result<RepairPlan, String> {
    #[cfg(debug_assertions)]
    {
        tauri::async_runtime::spawn_blocking(move || {
            let state = app.state::<ShellState>();
            let _lifecycle_guard = try_lifecycle(&state, "inspect the repair plan")?;
            let _provisioning_guard = try_provisioning(&state, "inspect the repair plan")?;
            let root = data_root(&app, &state)?;
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
            let runtime_info = runtime.refresh();
            let runtime_was_running = runtime_info.state == RuntimeState::Running;
            let machine_auth_failed = matches!(
                runtime_info.coffeepos_health.failure_kind,
                Some(runtime::CoffeePosHealthFailureKind::Authentication)
            );
            let (resolved, runtime_root) = runtime.provisioning_context();
            drop(runtime_guard);
            let provisioner =
                Provisioner::from_development(&project_root, &manifest, resolved, runtime_root)
                    .map_err(|error| error.to_string())?;
            Ok(provisioner.repair_plan(runtime_was_running, machine_auth_failed))
        })
        .await
        .map_err(|error| format!("Repair-plan worker failed: {error}."))?
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        Err("Repair is currently qualified only for the Windows development build until runtime resources are packaged.".into())
    }
}

#[tauri::command]
async fn apply_repair(
    app: tauri::AppHandle,
    plan_id: String,
    inputs: Option<RepairInputs>,
) -> Result<RepairApplyResult, String> {
    #[cfg(debug_assertions)]
    {
        {
            let state = app.state::<ShellState>();
            if state.lifecycle_requested.swap(true, Ordering::AcqRel) {
                return Err(
                    "Runtime lifecycle is busy. Wait for the current operation to finish, then retry repair."
                        .into(),
                );
            }
        }
        let admin_password = inputs.and_then(|value| value.admin_password);
        let worker_app = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let state = worker_app.state::<ShellState>();
            let _lifecycle_guard = try_lifecycle(&state, "apply the repair plan")?;
            let _provisioning_guard = try_provisioning(&state, "apply the repair plan")?;
            let root = data_root(&worker_app, &state)?;
            let shell = with_store(&worker_app, &state, |_| Ok(()))?;
            let store_name = shell.config.store_name.clone();
            let admin_username = shell
                .config
                .setup_admin_username
                .clone()
                .unwrap_or_else(|| WORDPRESS_ADMIN_USER.into());
            let admin_email = shell
                .config
                .setup_admin_email
                .clone()
                .unwrap_or_else(|| WORDPRESS_ADMIN_EMAIL.into());
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
            if runtime.backup_maintenance_active() {
                return Err(
                    "Database backup maintenance is active. Finish or cancel the current backup cleanup before applying repair."
                        .into(),
                );
            }
            let before_runtime = runtime.refresh();
            let runtime_was_running = before_runtime.state == RuntimeState::Running;
            let machine_auth_failed = matches!(
                before_runtime.coffeepos_health.failure_kind,
                Some(runtime::CoffeePosHealthFailureKind::Authentication)
            );
            let (resolved, runtime_root) = runtime.provisioning_context();
            let mut provisioner =
                Provisioner::from_development(&project_root, &manifest, resolved, runtime_root)
                    .map_err(|error| error.to_string())?;
            provisioner
                .configure_initial_admin(&admin_username, &admin_email)
                .map_err(|error| error.to_string())?;
            let current_plan = provisioner.repair_plan(runtime_was_running, machine_auth_failed);
            if current_plan.plan_id != plan_id {
                let error = runtime::RuntimeErrorInfo {
                    component: "repair".into(),
                    operation: "apply plan".into(),
                    message: "The store changed after this repair plan was inspected.".into(),
                    recovery: "Inspect the repair plan again before applying any changes.".into(),
                };
                return Ok(RepairApplyResult {
                    plan_id,
                    status: RepairResultStatus::Stale,
                    items: Vec::new(),
                    provisioning_info: provisioner.inspect(),
                    health_diagnostics: None,
                    last_error: Some(error),
                });
            }
            if !current_plan.can_apply {
                return Err(
                    "This repair plan contains no safe automatic action. Review the blocked items instead of applying it."
                        .into(),
                );
            }
            if current_plan.items.len() == 1
                && current_plan.items[0].id == "repair_transaction"
            {
                if runtime.refresh().state == RuntimeState::Running {
                    runtime.stop().map_err(|error| error.to_string())?;
                }
                let recovered = provisioner
                    .recover_interrupted_repair(&current_plan.plan_id)
                    .map_err(|error| error.to_string())?;
                return Ok(RepairApplyResult {
                    plan_id: current_plan.plan_id,
                    status: RepairResultStatus::Partial,
                    items: vec![recovered],
                    provisioning_info: provisioner.inspect(),
                    health_diagnostics: None,
                    last_error: None,
                });
            }
            let requires_admin_password = current_plan
                .items
                .iter()
                .any(|item| item.input_kind.as_deref() == Some("admin_password"));
            if requires_admin_password {
                let password = admin_password.as_deref().ok_or_else(|| {
                    "Enter a replacement administrator password before applying this repair."
                        .to_string()
                })?;
                validate_repair_admin_password_input(password)?;
            }

            let live_database_port = if runtime_was_running {
                match before_runtime.database_port {
                    Some(port) => Some(port),
                    None => {
                        let error = runtime::RuntimeErrorInfo {
                            component: "database".into(),
                            operation: "verify repair database credentials".into(),
                            message: "The running runtime has no managed MariaDB port for credential verification.".into(),
                            recovery: "Restart the runtime and inspect the repair plan again. Repair mutation has not started.".into(),
                        };
                        return Ok(RepairApplyResult {
                            plan_id: current_plan.plan_id,
                            status: RepairResultStatus::Partial,
                            items: vec![provisioning::RepairItemResult {
                                id: "database_credentials".into(),
                                status: RepairItemStatus::Blocked,
                                message: error.message.clone(),
                            }],
                            provisioning_info: provisioner.inspect(),
                            health_diagnostics: None,
                            last_error: Some(error),
                        });
                    }
                }
            } else {
                None
            };
            if let Err(error) =
                provisioner.verify_database_credentials_for_repair(live_database_port)
            {
                return Ok(RepairApplyResult {
                    plan_id: current_plan.plan_id,
                    status: RepairResultStatus::Partial,
                    items: vec![provisioning::RepairItemResult {
                        id: "database_credentials".into(),
                        status: RepairItemStatus::Blocked,
                        message: error.message.clone(),
                    }],
                    provisioning_info: provisioner.inspect(),
                    health_diagnostics: None,
                    last_error: Some(error),
                });
            }

            let needs_offline = current_plan.items.iter().any(|item| {
                item.requires_runtime_stop
                    && matches!(
                        item.classification,
                        provisioning::RepairClassification::Repairable
                            | provisioning::RepairClassification::RequiresInput
                    )
            });
            if needs_offline {
                runtime.stop().map_err(|error| error.to_string())?;
            }
            if let Err(error) = provisioner.begin_repair(&current_plan) {
                if runtime_was_running && runtime.refresh().state != RuntimeState::Running {
                    let _ = runtime.start();
                }
                return Err(error.to_string());
            }
            if needs_offline {
                if let Err(error) =
                    provisioner.mark_repair_runtime_stopped(&current_plan.plan_id)
                {
                    if runtime_was_running && runtime.refresh().state != RuntimeState::Running {
                        let _ = runtime.start();
                    }
                    return Ok(RepairApplyResult {
                        plan_id: current_plan.plan_id,
                        status: RepairResultStatus::Partial,
                        items: Vec::new(),
                        provisioning_info: provisioner.inspect(),
                        health_diagnostics: None,
                        last_error: Some(error),
                    });
                }
            }

            let mut item_results = match provisioner.apply_offline_repairs(&current_plan) {
                Ok(results) => results,
                Err(error) => {
                    let failure = match provisioner.rollback_pending_plugin_repairs() {
                        Ok(()) => error,
                        Err(rollback_error) => runtime::RuntimeErrorInfo {
                            component: "repair".into(),
                            operation: "rollback partial offline repair".into(),
                            message: format!(
                                "Offline repair failed ({}), and a pending plugin rollback also failed: {}",
                                error.message, rollback_error.message
                            ),
                            recovery: "The runtime remains stopped. Preserve config/repair.json plus all repair staging/backup evidence and reconcile the managed plugin trees explicitly before starting the store.".into(),
                        },
                    };
                    return Ok(RepairApplyResult {
                        plan_id: current_plan.plan_id,
                        status: RepairResultStatus::Partial,
                        items: Vec::new(),
                        provisioning_info: provisioner.inspect(),
                        health_diagnostics: None,
                        last_error: Some(runtime::RuntimeErrorInfo {
                            recovery: format!(
                                "{} The runtime remains stopped because repair did not reach online verification.",
                                failure.recovery
                            ),
                            ..failure
                        }),
                    });
                }
            };

            let repaired_woocommerce = item_results.iter().any(|item| {
                item.id == "woocommerce_plugin" && item.status == RepairItemStatus::Repaired
            });
            let repaired_coffeepos = item_results.iter().any(|item| {
                item.id == "coffeepos_plugin" && item.status == RepairItemStatus::Repaired
            });
            let has_online_item = current_plan.items.iter().any(|item| {
                matches!(
                    item.id.as_str(),
                    "wordpress_admin_password" | "machine_token_pending"
                ) && item.classification != provisioning::RepairClassification::Blocked
            });
            let has_repaired_offline = item_results
                .iter()
                .any(|item| item.status == RepairItemStatus::Repaired);
            let has_blocked = current_plan.items.iter().any(|item| {
                item.classification == provisioning::RepairClassification::Blocked
            });
            let needs_online_verification = has_online_item || has_repaired_offline;

            let mut health_diagnostics = None;
            let mut failure: Option<runtime::RuntimeErrorInfo> = None;
            if needs_online_verification {
                let runtime_info = match runtime.start() {
                    Ok(info) => Some(info),
                    Err(error) => {
                        failure = Some(error);
                        None
                    }
                };
                if let Some(runtime_info) = runtime_info.as_ref() {
                    if failure.is_none() && (repaired_woocommerce || repaired_coffeepos) {
                        match provisioner.verify_repaired_plugins(
                            runtime_info,
                            &store_name,
                            repaired_woocommerce,
                            repaired_coffeepos,
                        ) {
                            Ok(()) => {}
                            Err(verifier_error) => {
                                match runtime.stop() {
                                    Ok(_) => {
                                        if let Err(rollback_error) = provisioner.rollback_repaired_plugins(
                                            repaired_woocommerce,
                                            repaired_coffeepos,
                                        ) {
                                            failure = Some(runtime::RuntimeErrorInfo {
                                                component: "repair".into(),
                                                operation: "rollback repaired plugins".into(),
                                                message: format!(
                                                    "Plugin verification failed ({}), and rollback also failed: {}",
                                                    verifier_error.message, rollback_error.message
                                                ),
                                                recovery: "Preserve config/repair.json plus all repair staging/backup directories. Do not start another repair until the plugin trees are reconciled explicitly.".into(),
                                            });
                                        } else {
                                            failure = Some(verifier_error);
                                        }
                                    }
                                    Err(stop_error) => {
                                        failure = Some(runtime::RuntimeErrorInfo {
                                            component: "repair".into(),
                                            operation: "prepare plugin rollback".into(),
                                            message: format!(
                                                "Plugin verification failed ({}), but the runtime could not be stopped safely for rollback: {}",
                                                verifier_error.message, stop_error.message
                                            ),
                                            recovery: "The pre-repair plugin backup is preserved. Stop the managed runtime cleanly before attempting explicit recovery; do not mutate live plugin files while Caddy/PHP are serving requests.".into(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    if failure.is_none()
                        && current_plan
                            .items
                            .iter()
                            .any(|item| item.id == "wordpress_admin_password")
                    {
                        match provisioner
                            .repair_admin_password(runtime_info, admin_password.as_deref())
                        {
                            Ok(Some(result)) => {
                                item_results.retain(|item| item.id != result.id);
                                item_results.push(result);
                            }
                            Ok(None) => {}
                            Err(error) => failure = Some(error),
                        }
                    }
                    if failure.is_none()
                        && current_plan
                            .items
                            .iter()
                            .any(|item| item.id == "machine_token_pending")
                    {
                        match provisioner.recover_pending_machine_token(runtime_info) {
                            Ok(Some(result)) => {
                                item_results.retain(|item| item.id != result.id);
                                item_results.push(result);
                            }
                            Ok(None) => {}
                            Err(error) => failure = Some(error),
                        }
                    }
                    if failure.is_none() {
                        let diagnostics = runtime.health_diagnostics();
                        if !has_blocked && !diagnostics_all_healthy(&diagnostics) {
                            failure = Some(runtime::RuntimeErrorInfo {
                                component: "repair".into(),
                                operation: "verify repaired store".into(),
                                message: "Repair mutations completed, but the final component health snapshot is not fully healthy.".into(),
                                recovery: "Keep the repaired files and inspect Hệ thống → Chẩn đoán before retrying the remaining repair.".into(),
                            });
                        }
                        health_diagnostics = Some(diagnostics);
                    }
                }
            }

            if failure.is_none() {
                if let Err(error) = provisioner.mark_repair_verified(&current_plan.plan_id) {
                    failure = Some(error);
                }
            }
            if failure.is_none() {
                if let Err(error) = provisioner
                    .commit_repaired_plugins(repaired_woocommerce, repaired_coffeepos)
                {
                    failure = Some(error);
                }
            }
            if failure.is_none() {
                if let Err(error) = provisioner.mark_repair_committed(&current_plan.plan_id) {
                    failure = Some(error);
                }
            }

            if failure.is_some() {
                if runtime.refresh().state == RuntimeState::Running {
                    if let Err(stop_error) = runtime.stop() {
                        let previous = failure.take().expect("failure checked above");
                        failure = Some(runtime::RuntimeErrorInfo {
                            component: "repair".into(),
                            operation: "stop runtime after repair failure".into(),
                            message: format!(
                                "Repair failed ({}), and the runtime could not be stopped afterward: {}",
                                previous.message, stop_error.message
                            ),
                            recovery: format!(
                                "{} {} Preserve config/repair.json and repair backup evidence; do not continue selling until the interrupted transaction is recovered.",
                                previous.recovery, stop_error.recovery
                            ),
                        });
                    }
                }
            } else if !runtime_was_running && runtime.refresh().state == RuntimeState::Running {
                if let Err(error) = runtime.stop() {
                    failure = Some(error);
                }
            } else if runtime_was_running && runtime.refresh().state != RuntimeState::Running {
                if let Err(error) = runtime.start() {
                    failure = Some(error);
                }
            }

            if failure.is_none() {
                if let Err(error) = provisioner.finish_repair(&current_plan.plan_id) {
                    failure = Some(error);
                }
            }
            let provisioning_info = provisioner.inspect();
            let fully_ready = provisioning_info.state == ProvisioningState::Ready
                && !has_blocked
                && failure.is_none();
            Ok(RepairApplyResult {
                plan_id: current_plan.plan_id,
                status: if fully_ready {
                    RepairResultStatus::Repaired
                } else {
                    RepairResultStatus::Partial
                },
                items: item_results,
                provisioning_info,
                health_diagnostics,
                last_error: failure,
            })
        })
        .await;
        app.state::<ShellState>()
            .lifecycle_requested
            .store(false, Ordering::Release);
        result.map_err(|error| format!("Repair worker failed: {error}."))?
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = plan_id;
        let _ = inputs;
        Err("Repair is currently qualified only for the Windows development build until runtime resources are packaged.".into())
    }
}

#[tauri::command]
async fn refresh_runtime_maintenance(app: tauri::AppHandle) -> Result<RuntimeInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<ShellState>();
        if state.shutdown_in_progress.load(Ordering::Acquire)
            || state.lifecycle_requested.load(Ordering::Acquire)
        {
            return Err("Runtime maintenance skipped while lifecycle work is pending.".into());
        }
        match state.lifecycle.try_lock() {
            Ok(guard) => drop(guard),
            Err(TryLockError::WouldBlock) => {
                return Err("Runtime maintenance skipped while lifecycle work is active.".into());
            }
            Err(TryLockError::Poisoned(_)) => {
                return Err(
                    "Runtime lifecycle state unavailable. Restart CoffeePOS Desktop before retrying."
                        .into(),
                );
            }
        }

        let (snapshot, probe) = {
            let mut guard = match state.runtime.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::WouldBlock) => {
                    return Err("Runtime maintenance skipped while runtime state is busy.".into());
                }
                Err(TryLockError::Poisoned(_)) => {
                    return Err(
                        "Runtime state unavailable. Restart CoffeePOS Desktop before retrying."
                            .into(),
                    );
                }
            };
            let runtime = guard.as_mut().ok_or_else(|| {
                "Runtime maintenance skipped until runtime state is initialized.".to_string()
            })?;
            runtime.prepare_background_maintenance()
        };

        let Some(probe) = probe else {
            return Ok(snapshot);
        };
        if state.shutdown_in_progress.load(Ordering::Acquire)
            || state.lifecycle_requested.load(Ordering::Acquire)
        {
            return Ok(snapshot);
        }
        let health = RuntimeManager::run_background_health_probe(&probe);
        if state.shutdown_in_progress.load(Ordering::Acquire)
            || state.lifecycle_requested.load(Ordering::Acquire)
        {
            return Ok(snapshot);
        }
        match state.lifecycle.try_lock() {
            Ok(guard) => drop(guard),
            Err(TryLockError::WouldBlock) => return Ok(snapshot),
            Err(TryLockError::Poisoned(_)) => {
                return Err(
                    "Runtime lifecycle state unavailable. Restart CoffeePOS Desktop before retrying."
                        .into(),
                );
            }
        }
        let mut guard = match state.runtime.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return Ok(snapshot),
            Err(TryLockError::Poisoned(_)) => {
                return Err(
                    "Runtime state unavailable. Restart CoffeePOS Desktop before retrying.".into(),
                );
            }
        };
        let runtime = guard.as_mut().ok_or_else(|| {
            "Runtime maintenance skipped because runtime state is unavailable.".to_string()
        })?;
        Ok(runtime.commit_background_health(&probe, health))
    })
    .await
    .map_err(|error| format!("Runtime maintenance worker failed: {error}."))?
}

#[tauri::command]
fn get_provisioning_info(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<ProvisioningInfo, String> {
    #[cfg(debug_assertions)]
    {
        inspect_provisioning(&app, &state)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = state;
        Err("Bundled Phase 3 WordPress resources are not packaged yet. Use a development build until runtime packaging is implemented.".into())
    }
}

#[tauri::command]
fn open_wordpress(app: tauri::AppHandle, state: State<'_, ShellState>) -> Result<String, String> {
    #[cfg(debug_assertions)]
    {
        let _lifecycle_guard = try_lifecycle(&state, "open WordPress")?;
        let provisioning = inspect_provisioning(&app, &state)?;
        if provisioning.state != ProvisioningState::Ready {
            return Err(
                "WordPress can only be opened after provisioning is ready. Finish or repair provisioning first."
                    .into(),
            );
        }
        let url = with_runtime(&app, &state, |runtime| {
            runtime.refresh();
            runtime.wordpress_url()
        })?;
        open_system_browser(&url)?;
        Ok(url)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = state;
        Err("Bundled runtime resources are not packaged yet. Open WordPress is available only in the qualified development build until Phase 9.".into())
    }
}

#[tauri::command]
fn open_pos(app: tauri::AppHandle, state: State<'_, ShellState>) -> Result<String, String> {
    #[cfg(debug_assertions)]
    {
        let _lifecycle_guard = try_lifecycle(&state, "open the POS")?;
        let provisioning = inspect_provisioning(&app, &state)?;
        if provisioning.state != ProvisioningState::Ready {
            return Err(
                "CoffeePOS can only be opened after provisioning is ready. Finish or recover setup first."
                    .into(),
            );
        }
        let url = with_runtime(&app, &state, |runtime| {
            runtime.refresh();
            runtime.pos_url()
        })?;
        open_system_browser(&url)?;
        Ok(url)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = state;
        Err("Bundled runtime resources are not packaged yet. Open POS is available only in the qualified development build until Phase 9.".into())
    }
}

#[tauri::command]
fn provision_wordpress(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<ProvisioningInfo, String> {
    #[cfg(debug_assertions)]
    {
        let _lifecycle_guard = try_lifecycle(&state, "provision WordPress")?;
        let root = data_root(&app, &state)?;
        let (store_name, admin_username, admin_email, setup_profile_configured) = {
            let guard = state.store.lock().map_err(|_| {
                "Application state unavailable. Restart CoffeePOS Desktop.".to_string()
            })?;
            let store = guard
                .as_ref()
                .ok_or_else(|| "Configuration unavailable. Retry startup.".to_string())?;
            (
                store.config.store_name.clone(),
                store
                    .config
                    .setup_admin_username
                    .clone()
                    .unwrap_or_else(|| WORDPRESS_ADMIN_USER.into()),
                store
                    .config
                    .setup_admin_email
                    .clone()
                    .unwrap_or_else(|| WORDPRESS_ADMIN_EMAIL.into()),
                store.config.setup_admin_username.is_some()
                    && store.config.setup_admin_email.is_some(),
            )
        };
        let provisioning_before = inspect_provisioning(&app, &state)?;
        if provisioning_before.state == ProvisioningState::NotInstalled {
            if root.join(WORDPRESS_ADMIN_PENDING_SECRET).exists() {
                return Err("Administrator credential update is incomplete. Return to setup and save the administrator password again before installing CoffeePOS.".into());
            }
            let password_ready = secret::load(&root.join(WORDPRESS_ADMIN_SECRET))
                .map(|value| !value.is_empty())
                .unwrap_or(false);
            if !setup_profile_configured || !password_ready {
                return Err("Complete the store and administrator account step before installing CoffeePOS.".into());
            }
        }
        let _provisioning_guard = match state.provisioning.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => {
                return Err(
                    "WordPress provisioning is already running. Wait for it to finish before retrying."
                        .into(),
                );
            }
            Err(TryLockError::Poisoned(_)) => {
                return Err(
                    "Provisioning state unavailable. Restart CoffeePOS Desktop before retrying."
                        .into(),
                );
            }
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
        if runtime.backup_maintenance_active() {
            return Err(
                "Database backup maintenance is active. Finish or cancel the current backup cleanup before provisioning CoffeePOS."
                    .into(),
            );
        }
        runtime.stop().map_err(|error| error.to_string())?;
        let (resolved, runtime_root) = runtime.provisioning_context();
        let mut provisioner =
            Provisioner::from_development(&project_root, &manifest, resolved, runtime_root)
                .map_err(|error| error.to_string())?;
        provisioner
            .configure_initial_admin(&admin_username, &admin_email)
            .map_err(|error| error.to_string())?;
        provisioner.prepare().map_err(|error| error.to_string())?;
        let runtime_info = runtime
            .start_for_provisioning()
            .map_err(|error| error.to_string())?;
        match provisioner.install_wordpress(&store_name, &runtime_info) {
            Ok(info) => {
                runtime.refresh_wordpress_health();
                Ok(info)
            }
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
        .setup(|app| {
            let open_item =
                MenuItem::with_id(app, TRAY_OPEN_ID, "Mở CoffeePOS", true, None::<&str>)?;
            let exit_item =
                MenuItem::with_id(app, TRAY_EXIT_ID, "Thoát hoàn toàn", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_item, &exit_item])?;
            let mut tray = TrayIconBuilder::with_id("coffeepos-main")
                .menu(&menu)
                .tooltip("CoffeePOS")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id() == TRAY_OPEN_ID {
                        show_main_window(app);
                    } else if event.id() == TRAY_EXIT_ID {
                        request_full_exit(app.clone());
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::DoubleClick {
                            button: MouseButton::Left,
                            ..
                        }
                    ) {
                        show_main_window(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            match event {
                tauri::WindowEvent::Resized(_) => {
                    if window.is_minimized().unwrap_or(false) {
                        let _ = window.hide();
                        // Reset the native minimized state while the window stays hidden. This
                        // prevents a stale minimize resize from immediately hiding a tray restore.
                        let _ = window.unminimize();
                    }
                }
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    let state = window.state::<ShellState>();
                    if state.exit_authorized.load(Ordering::Acquire) {
                        return;
                    }
                    api.prevent_close();
                    request_full_exit(window.app_handle().clone());
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_backup_status,
            create_backup,
            cancel_backup,
            open_backup_folder,
            inspect_backup,
            validate_backup,
            get_log_catalog,
            read_log_page,
            export_support_bundle,
            get_shell_info,
            save_app_settings,
            get_setup_info,
            save_setup_profile,
            copy_admin_password,
            get_runtime_info,
            start_runtime,
            stop_runtime,
            restart_runtime,
            retry_runtime_health,
            refresh_runtime_maintenance,
            get_health_diagnostics,
            get_repair_plan,
            apply_repair,
            open_wordpress,
            open_pos,
            get_provisioning_info,
            provision_wordpress
        ])
        .run(tauri::generate_context!())
        .expect("CoffeePOS Desktop could not start the native shell");
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn setup_profile_failure_preserves_preexisting_pending_secret_blocker() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path().to_path_buf()).unwrap();
        store
            .save_setup_profile("Committed Store", "owner_52", "owner52@example.com")
            .unwrap();
        let active = store.root.join(WORDPRESS_ADMIN_SECRET);
        let pending = store.root.join(WORDPRESS_ADMIN_PENDING_SECRET);
        secret::store_password(&active, "OldActivePassword2026").unwrap();
        secret::store_password(&pending, "PendingPassword2026").unwrap();

        let error = persist_setup_profile_transaction(
            &mut store,
            "<invalid>",
            "owner_52",
            "owner52@example.com",
            Some("ReplacementPending2026"),
        )
        .unwrap_err();
        assert!(error.contains("Store name"));
        assert!(pending.exists());
        assert_eq!(secret::load(&pending).unwrap(), "ReplacementPending2026");
        assert_eq!(secret::load(&active).unwrap(), "OldActivePassword2026");
        assert_eq!(store.config.store_name, "Committed Store");
    }
}
