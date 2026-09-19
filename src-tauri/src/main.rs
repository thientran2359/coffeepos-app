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
mod restore;
#[cfg_attr(not(debug_assertions), allow(dead_code))]
mod runtime;
mod secret;

use backup::{BackupOperationState, BackupResult, BackupStatus};
use backup_format::{
    BackupCompatibilityTarget, BackupErrorInfo, BackupInspection, BackupValidation,
};
use config::{AppConfig, AppLanguage, StartupView, Store};
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
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, State};

const TRAY_OPEN_ID: &str = "tray-open";
const TRAY_EXIT_ID: &str = "tray-exit";
const TRAY_MAIN_ID: &str = "coffeepos-main";

struct NativeStrings {
    tray_open: &'static str,
    tray_quit: &'static str,
    shutdown_confirm_title: &'static str,
    shutdown_confirm_message: &'static str,
    restore_busy_title: &'static str,
    restore_busy_message: &'static str,
    busy_title: &'static str,
    busy_operation_message: &'static str,
    busy_lifecycle_message: &'static str,
    busy_runtime_message: &'static str,
    unsafe_exit_title: &'static str,
    lifecycle_unavailable_message: &'static str,
    runtime_unavailable_message: &'static str,
    stop_failed_title: &'static str,
    stop_failed_prefix: &'static str,
}

const NATIVE_STRINGS_VI: NativeStrings = NativeStrings {
    tray_open: "Mở CoffeePOS",
    tray_quit: "Thoát hoàn toàn",
    shutdown_confirm_title: "Dừng cửa hàng và thoát CoffeePOS?",
    shutdown_confirm_message: "Dừng và thoát sẽ ngắt kết nối POS và các thiết bị đang dùng cửa hàng này. Thao tác này không chốt ca và không tự thay đổi trạng thái thanh toán.\n\nChọn Có để dừng cửa hàng và thoát, hoặc Không để ở lại.",
    restore_busy_title: "CoffeePOS đang khôi phục dữ liệu",
    restore_busy_message: "CoffeePOS đang khôi phục hoặc đối soát một giao dịch khôi phục. Hãy để ứng dụng mở cho đến khi trạng thái khôi phục kết thúc an toàn.",
    busy_title: "CoffeePOS đang bận",
    busy_operation_message: "CoffeePOS đang hoàn tất một thao tác hệ thống. Hãy thử thoát lại sau khi thao tác hiện tại kết thúc.",
    busy_lifecycle_message: "CoffeePOS đang hoàn tất cài đặt hoặc thay đổi trạng thái hệ thống. Hãy chờ thao tác hiện tại kết thúc rồi thử thoát lại.",
    busy_runtime_message: "CoffeePOS đang cập nhật trạng thái hệ thống. Hãy thử thoát lại sau khi thao tác hiện tại kết thúc.",
    unsafe_exit_title: "Không thể thoát an toàn",
    lifecycle_unavailable_message: "Trạng thái vòng đời runtime không còn khả dụng. Hãy giữ ứng dụng mở và kiểm tra Chẩn đoán trước khi thử lại.",
    runtime_unavailable_message: "Không thể đọc trạng thái runtime để dừng cửa hàng an toàn. Hãy giữ ứng dụng mở và thử lại.",
    stop_failed_title: "Chưa thể dừng cửa hàng",
    stop_failed_prefix: "CoffeePOS chưa dừng hoàn toàn nên ứng dụng vẫn mở để tránh bỏ lại tiến trình.",
};

const NATIVE_STRINGS_EN: NativeStrings = NativeStrings {
    tray_open: "Open CoffeePOS",
    tray_quit: "Quit CoffeePOS",
    shutdown_confirm_title: "Stop the store and quit CoffeePOS?",
    shutdown_confirm_message: "Stopping and quitting will disconnect the POS and devices using this store. This does not close the current shift or change payment status automatically.\n\nChoose Yes to stop the store and quit, or No to stay.",
    restore_busy_title: "CoffeePOS is restoring data",
    restore_busy_message: "CoffeePOS is restoring or reconciling a restore transaction. Keep the application open until restore reaches a safe terminal state.",
    busy_title: "CoffeePOS is busy",
    busy_operation_message: "CoffeePOS is finishing a system operation. Try quitting again after the current operation finishes.",
    busy_lifecycle_message: "CoffeePOS is finishing setup or a system state change. Wait for the current operation to finish, then try quitting again.",
    busy_runtime_message: "CoffeePOS is updating system state. Try quitting again after the current operation finishes.",
    unsafe_exit_title: "CoffeePOS cannot quit safely",
    lifecycle_unavailable_message: "Runtime lifecycle state is unavailable. Keep the application open and check Diagnostics before trying again.",
    runtime_unavailable_message: "CoffeePOS cannot read runtime state to stop the store safely. Keep the application open and try again.",
    stop_failed_title: "The store could not be stopped",
    stop_failed_prefix: "CoffeePOS has not stopped completely, so the application will stay open to avoid leaving managed processes behind.",
};

fn native_strings(language: AppLanguage) -> &'static NativeStrings {
    match language {
        AppLanguage::Vi => &NATIVE_STRINGS_VI,
        AppLanguage::En => &NATIVE_STRINGS_EN,
    }
}

#[derive(Default)]
struct ShellState {
    store: Mutex<Option<Store>>,
    effective_language: Mutex<AppLanguage>,
    language_mutation: Mutex<()>,
    runtime: Mutex<Option<RuntimeManager>>,
    lifecycle: Mutex<()>,
    lifecycle_requested: AtomicBool,
    exit_authorized: AtomicBool,
    shutdown_in_progress: AtomicBool,
    support_export_in_progress: AtomicBool,
    backup: Mutex<BackupOperationState>,
    restore_candidates: Mutex<restore::RestoreCandidateRegistry>,
    restore_admission: Mutex<restore::RestoreAdmissionGate>,
    restore_operation: Mutex<RestoreOperationState>,
    restore_requested: AtomicBool,
    #[cfg(debug_assertions)]
    provisioning: Mutex<()>,
}

#[derive(Clone, Debug, Serialize)]
struct RestoreStatus {
    operation_id: Option<String>,
    stage: String,
    started_at: Option<u64>,
    finished_at: Option<u64>,
    succeeded: bool,
    failed: bool,
    cancelled: bool,
    rolled_back: bool,
    needs_recovery: bool,
    recovery_required: bool,
    original_state: Option<restore::RestoreOriginalState>,
    warnings: Vec<String>,
    last_error: Option<restore::RestoreErrorInfo>,
}

impl Default for RestoreStatus {
    fn default() -> Self {
        Self {
            operation_id: None,
            stage: "idle".into(),
            started_at: None,
            finished_at: None,
            succeeded: false,
            failed: false,
            cancelled: false,
            rolled_back: false,
            needs_recovery: false,
            recovery_required: false,
            original_state: None,
            warnings: Vec::new(),
            last_error: None,
        }
    }
}

#[derive(Default)]
struct RestoreOperationState {
    status: RestoreStatus,
    cancelled: Option<Arc<AtomicBool>>,
}

#[derive(Clone, Debug, Serialize)]
struct RestoreResult {
    operation_id: String,
    status: String,
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

fn effective_app_language(state: &ShellState) -> AppLanguage {
    match state.effective_language.lock() {
        Ok(language) => *language,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

fn set_effective_app_language(state: &ShellState, language: AppLanguage) {
    match state.effective_language.lock() {
        Ok(mut current) => *current = language,
        Err(poisoned) => *poisoned.into_inner() = language,
    }
}

fn native_tray_menu<R: tauri::Runtime, M: Manager<R>>(
    manager: &M,
    language: AppLanguage,
) -> tauri::Result<Menu<R>> {
    let strings = native_strings(language);
    let open_item =
        MenuItem::<R>::with_id(manager, TRAY_OPEN_ID, strings.tray_open, true, None::<&str>)?;
    let exit_item =
        MenuItem::<R>::with_id(manager, TRAY_EXIT_ID, strings.tray_quit, true, None::<&str>)?;
    Menu::with_items(manager, &[&open_item, &exit_item])
}

fn refresh_native_tray(app: &tauri::AppHandle, language: AppLanguage) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(TRAY_MAIN_ID) else {
        return Ok(());
    };
    tray.set_menu(Some(native_tray_menu(app, language)?))
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

fn ipc_error_info(
    code: &'static str,
    component: &'static str,
    operation: &'static str,
    message: impl Into<String>,
    recovery: &'static str,
) -> runtime::RuntimeErrorInfo {
    runtime::RuntimeErrorInfo {
        component: component.into(),
        operation: operation.into(),
        code: code.into(),
        message: message.into(),
        recovery: recovery.into(),
    }
}

fn runtime_ipc_error(
    operation: &'static str,
    message: impl Into<String>,
) -> runtime::RuntimeErrorInfo {
    ipc_error_info(
        "runtime_error",
        "runtime",
        operation,
        message,
        "Retry the runtime action. If the problem continues, open System diagnostics before making further changes.",
    )
}

fn provisioning_ipc_error(
    operation: &'static str,
    message: impl Into<String>,
) -> runtime::RuntimeErrorInfo {
    ipc_error_info(
        "provisioning_error",
        "provisioning",
        operation,
        message,
        "Retry from the current provisioning checkpoint. If setup remains blocked, open System diagnostics or repair.",
    )
}

fn repair_ipc_error(
    operation: &'static str,
    message: impl Into<String>,
) -> runtime::RuntimeErrorInfo {
    ipc_error_info(
        "repair_error",
        "repair",
        operation,
        message,
        "Inspect the current repair plan and system diagnostics before retrying repair.",
    )
}

fn with_runtime_structured<T>(
    _app: &tauri::AppHandle,
    state: &ShellState,
    operation: impl FnOnce(&mut RuntimeManager) -> Result<T, runtime::RuntimeErrorInfo>,
) -> Result<T, runtime::RuntimeErrorInfo> {
    #[cfg(debug_assertions)]
    let root = data_root(_app, state)
        .map_err(|message| runtime_ipc_error("resolve data root", message))?;
    let mut guard = state
        .runtime
        .lock()
        .map_err(|_| runtime_ipc_error("access runtime state", "Runtime state is unavailable."))?;
    if guard.is_none() {
        #[cfg(debug_assertions)]
        {
            let (project_root, manifest) = development_runtime_paths()
                .map_err(|message| runtime_ipc_error("resolve runtime manifest", message))?;
            *guard = Some(RuntimeManager::from_development(
                &project_root,
                &manifest,
                root,
            )?);
        }
        #[cfg(not(debug_assertions))]
        {
            return Err(runtime_ipc_error(
                "initialize runtime",
                "Bundled runtime resources are not packaged yet.",
            ));
        }
    }
    let manager = guard.as_mut().ok_or_else(|| {
        runtime_ipc_error("access runtime manager", "Runtime manager is unavailable.")
    })?;
    operation(manager)
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
) -> Result<T, runtime::RuntimeErrorInfo>
where
    T: Send + 'static,
    F: FnOnce(&mut RuntimeManager) -> Result<T, runtime::RuntimeErrorInfo> + Send + 'static,
{
    {
        let state = app.state::<ShellState>();
        ensure_restore_allows_managed_operation(&state, lifecycle_operation)
            .map_err(|message| runtime_ipc_error(lifecycle_operation, message))?;
        if state.lifecycle_requested.swap(true, Ordering::AcqRel) {
            return Err(runtime_ipc_error(
                lifecycle_operation,
                "Runtime lifecycle is busy with another operation.",
            ));
        }
    }

    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = worker_app.state::<ShellState>();
        let _lifecycle_guard = try_lifecycle(&state, lifecycle_operation)
            .map_err(|message| runtime_ipc_error(lifecycle_operation, message))?;
        with_runtime_structured(&worker_app, &state, operation)
    })
    .await;
    app.state::<ShellState>()
        .lifecycle_requested
        .store(false, Ordering::Release);
    result.map_err(|error| {
        runtime_ipc_error(
            lifecycle_operation,
            format!("Runtime worker failed: {error}."),
        )
    })?
}

async fn read_runtime_blocking(
    app: tauri::AppHandle,
    operation: fn(&mut RuntimeManager) -> RuntimeInfo,
) -> Result<RuntimeInfo, runtime::RuntimeErrorInfo> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<ShellState>();
        with_runtime_structured(&app, &state, |runtime| Ok(operation(runtime)))
    })
    .await
    .map_err(|error| {
        runtime_ipc_error(
            "read runtime status",
            format!("Runtime status worker failed: {error}."),
        )
    })?
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
fn confirm_runtime_exit(language: AppLanguage) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO,
    };

    let strings = native_strings(language);
    shutdown_message_box(
        strings.shutdown_confirm_message,
        strings.shutdown_confirm_title,
        MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
    ) == IDYES
}

#[cfg(not(windows))]
fn confirm_runtime_exit(_language: AppLanguage) -> bool {
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
    let language = effective_app_language(&state);
    let strings = native_strings(language);
    if state.exit_authorized.load(Ordering::Acquire) {
        return;
    }
    if restore_gate_blocks(&state).unwrap_or(true)
        || state.restore_requested.load(Ordering::Acquire)
    {
        show_main_window(&app);
        show_shutdown_notice(
            strings.restore_busy_title,
            strings.restore_busy_message,
            false,
        );
        return;
    }
    if state.shutdown_in_progress.swap(true, Ordering::AcqRel) {
        return;
    }
    if state.lifecycle_requested.swap(true, Ordering::AcqRel) {
        state.shutdown_in_progress.store(false, Ordering::Release);
        show_main_window(&app);
        show_shutdown_notice(strings.busy_title, strings.busy_operation_message, false);
        return;
    }

    let lifecycle_guard = match state.lifecycle.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::WouldBlock) => {
            state.lifecycle_requested.store(false, Ordering::Release);
            state.shutdown_in_progress.store(false, Ordering::Release);
            show_main_window(&app);
            show_shutdown_notice(strings.busy_title, strings.busy_lifecycle_message, false);
            return;
        }
        Err(TryLockError::Poisoned(_)) => {
            state.lifecycle_requested.store(false, Ordering::Release);
            state.shutdown_in_progress.store(false, Ordering::Release);
            show_main_window(&app);
            show_shutdown_notice(
                strings.unsafe_exit_title,
                strings.lifecycle_unavailable_message,
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
            show_shutdown_notice(strings.busy_title, strings.busy_runtime_message, false);
            return;
        }
        Err(TryLockError::Poisoned(_)) => {
            state.lifecycle_requested.store(false, Ordering::Release);
            state.shutdown_in_progress.store(false, Ordering::Release);
            show_main_window(&app);
            show_shutdown_notice(
                strings.unsafe_exit_title,
                strings.runtime_unavailable_message,
                true,
            );
            return;
        }
    };

    if let Some(runtime) = runtime_guard.as_ref() {
        if runtime.requires_exit_confirmation() && !confirm_runtime_exit(language) {
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
        let strings = native_strings(language);
        let result = (|| -> Result<(), String> {
            let _lifecycle_guard = match state.lifecycle.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::WouldBlock) => {
                    return Err(strings.busy_lifecycle_message.to_string())
                }
                Err(TryLockError::Poisoned(_)) => {
                    return Err(strings.lifecycle_unavailable_message.to_string())
                }
            };
            let mut runtime_guard = state
                .runtime
                .lock()
                .map_err(|_| strings.runtime_unavailable_message.to_string())?;
            if let Some(runtime) = runtime_guard.as_mut() {
                if let Err(error) = runtime.stop() {
                    if runtime.requires_exit_confirmation() {
                        return Err(format!("{}\n\n{error}", strings.stop_failed_prefix));
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
                show_shutdown_notice(strings.stop_failed_title, &message, true);
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

fn restore_command_error(
    action: &str,
    code: &str,
    message: impl Into<String>,
    recovery: impl Into<String>,
) -> restore::RestoreErrorInfo {
    restore::RestoreErrorInfo {
        component: "restore".into(),
        action: action.into(),
        code: code.into(),
        message: message.into(),
        recovery: recovery.into(),
    }
}

fn restore_error_from_backup(action: &str, error: BackupErrorInfo) -> restore::RestoreErrorInfo {
    restore::RestoreErrorInfo {
        component: "restore".into(),
        action: action.into(),
        code: error.code,
        message: error.message,
        recovery: error.recovery,
    }
}

fn restore_error_from_runtime(
    action: &str,
    error: runtime::RuntimeErrorInfo,
) -> restore::RestoreErrorInfo {
    restore::RestoreErrorInfo {
        component: "restore".into(),
        action: action.into(),
        code: "runtime_failed".into(),
        message: error.message,
        recovery: error.recovery,
    }
}

fn restore_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

fn restore_stage_name(stage: restore::RestoreStage) -> &'static str {
    use restore::RestoreStage::*;
    match stage {
        Planned => "planned",
        Validated => "validated",
        RuntimeStopped => "runtime_stopped",
        RecoveryBackupReady => "recovery_backup_ready",
        StagingPrepared => "staging_prepared",
        DatabaseImported => "database_imported",
        UploadsRestored => "uploads_restored",
        TargetSecretsBound => "target_secrets_bound",
        StagingVerified => "staging_verified",
        CutoverStarted => "cutover_started",
        ActiveSwapped => "active_swapped",
        ActiveVerified => "active_verified",
        Committed => "committed",
        Cleanup => "cleanup",
        AbortStarted => "abort_started",
        Aborted => "aborted",
        RollbackStarted => "rollback_started",
        RolledBack => "rolled_back",
    }
}

fn restore_status_from_journal(
    journal: &restore::RestoreJournal,
    last_error: Option<restore::RestoreErrorInfo>,
) -> RestoreStatus {
    let stage = restore_stage_name(journal.stage).to_string();
    let last_error =
        last_error.or_else(|| {
            journal.safe_error.as_ref().map(|error| {
                restore::RestoreErrorInfo {
            component: "restore".into(),
            action: "recover".into(),
            code: error.code.clone(),
            message: error.message.clone(),
            recovery:
                "Keep restore admission fenced until CoffeePOS completes durable reconciliation."
                    .into(),
        }
            })
        });
    let has_durable_error = journal.safe_error.is_some() || last_error.is_some();
    RestoreStatus {
        operation_id: Some(journal.transaction_id.clone()),
        stage,
        started_at: Some(journal.created_at_unix_seconds),
        finished_at: journal
            .stage
            .terminal()
            .then_some(journal.updated_at_unix_seconds),
        succeeded: matches!(
            journal.stage,
            restore::RestoreStage::Committed | restore::RestoreStage::Cleanup
        ),
        failed: has_durable_error
            && !matches!(
                journal.stage,
                restore::RestoreStage::Committed | restore::RestoreStage::Cleanup
            ),
        cancelled: journal.stage == restore::RestoreStage::Aborted && !has_durable_error,
        rolled_back: journal.stage == restore::RestoreStage::RolledBack,
        needs_recovery: false,
        recovery_required: false,
        original_state: Some(journal.original_state.clone()),
        warnings: Vec::new(),
        last_error,
    }
}

fn update_restore_status_from_journal(state: &ShellState, journal: &restore::RestoreJournal) {
    if let Ok(mut operation) = state.restore_operation.lock() {
        let last_error = operation.status.last_error.clone();
        let warnings = operation.status.warnings.clone();
        operation.status = restore_status_from_journal(journal, last_error);
        operation.status.warnings = warnings;
    }
}

fn restore_gate_blocks(state: &ShellState) -> Result<bool, String> {
    state
        .restore_admission
        .lock()
        .map(|gate| gate.blocks_managed_operations())
        .map_err(|_| {
            "Restore admission state unavailable. Restart CoffeePOS Desktop and keep the runtime stopped."
                .to_string()
        })
}

fn ensure_restore_allows_managed_operation(
    state: &ShellState,
    operation: &str,
) -> Result<(), String> {
    if restore_gate_blocks(state)? || state.restore_requested.load(Ordering::Acquire) {
        return Err(format!(
            "Restore is active or requires recovery while trying to {operation}. Finish restore recovery before starting another managed operation."
        ));
    }
    Ok(())
}

#[cfg(debug_assertions)]
fn restore_target_context(
    data_root: &Path,
) -> Result<(PathBuf, PathBuf, BackupCompatibilityTarget), restore::RestoreErrorInfo> {
    let (project_root, manifest) = development_runtime_paths().map_err(|error| {
        restore_command_error(
            "resolve target",
            "target_compatibility_unavailable",
            error,
            "Restage the pinned CoffeePOS runtime and retry restore.",
        )
    })?;
    let target = backup_format::load_development_compatibility_target(
        &project_root,
        &manifest,
        data_root,
        "restore",
    )
    .map_err(|error| restore_error_from_backup("resolve target", error))?;
    Ok((project_root, manifest, target))
}

#[cfg(debug_assertions)]
fn refresh_restore_store_state(
    state: &ShellState,
    data_root: &Path,
) -> Result<(), restore::RestoreErrorInfo> {
    let mut guard = state.store.lock().map_err(|_| {
        restore_command_error(
            "refresh store config",
            "store_state_unavailable",
            "CoffeePOS cannot refresh the active store configuration after restore cutover.",
            "Keep restore admission blocked and retry recovery after restarting CoffeePOS Desktop.",
        )
    })?;
    *guard = None;
    *guard = Some(Store::open(data_root.to_path_buf()).map_err(|error| {
        restore_command_error(
            "refresh store config",
            "store_config_unavailable",
            error,
            "Keep restore admission blocked and repair the active configuration before retrying recovery.",
        )
    })?);
    Ok(())
}

fn restore_cancel_requested(cancelled: &AtomicBool) -> bool {
    cancelled.load(Ordering::Acquire)
}

#[cfg(debug_assertions)]
fn verify_restore_runtime_health(
    data_root: &Path,
    runtime: &mut RuntimeManager,
) -> Result<RuntimeInfo, restore::RestoreErrorInfo> {
    let runtime_info = runtime
        .start_for_provisioning()
        .map_err(|error| restore_error_from_runtime("verify restored runtime", error))?;
    let http_port = runtime_info.http_port.ok_or_else(|| {
        restore_command_error(
            "verify restored runtime",
            "restore_http_port_unavailable",
            "The isolated restore runtime did not expose an HTTP port.",
            "Keep restore fenced, stop the verification runtime, and retry recovery.",
        )
    })?;
    let health = runtime::probe_coffeepos_health(data_root, http_port);
    if health.state != runtime::CoffeePosHealthState::Healthy {
        let recovery = health
            .error
            .as_ref()
            .map(|error| error.recovery.clone())
            .unwrap_or_else(|| {
                "Keep restore fenced and inspect the restored CoffeePOS health endpoint before retrying."
                    .into()
            });
        return Err(restore_command_error(
            "verify restored runtime",
            "restored_health_failed",
            "The restored CoffeePOS machine-health endpoint did not report healthy.",
            recovery,
        ));
    }
    Ok(runtime_info)
}

fn set_restore_recovery_required(
    state: &ShellState,
    journal: &restore::RestoreJournal,
    error: restore::RestoreErrorInfo,
) {
    if let Ok(mut operation) = state.restore_operation.lock() {
        operation.status = restore_status_from_journal(journal, Some(error));
        operation.status.stage = "recovery_required".into();
        operation.status.needs_recovery = true;
        operation.status.recovery_required = true;
        operation.status.failed = true;
        operation.status.finished_at = None;
        operation.cancelled = None;
    }
}

fn release_reconciled_restore(
    state: &ShellState,
    data_root: &Path,
    journal: &restore::RestoreJournal,
) -> Result<(), restore::RestoreErrorInfo> {
    restore::cleanup_terminal_restore_owned_roots(data_root, journal)?;
    restore::retire_terminal_restore_journal(data_root, journal)?;
    state
        .restore_admission
        .lock()
        .map_err(|_| {
            restore_command_error(
                "release admission",
                "restore_gate_unavailable",
                "CoffeePOS cannot reopen managed-operation admission after restore reconciliation.",
                "Keep the application open and retry restore recovery before using the store.",
            )
        })?
        .release_reconciled(journal)?;
    Ok(())
}

#[cfg(debug_assertions)]
fn ensure_active_restore_runtime<'a>(
    runtime_guard: &'a mut Option<RuntimeManager>,
    project_root: &Path,
    manifest: &Path,
    data_root: &Path,
) -> Result<&'a mut RuntimeManager, restore::RestoreErrorInfo> {
    if runtime_guard.is_none() {
        *runtime_guard = Some(
            RuntimeManager::from_development(project_root, manifest, data_root.to_path_buf())
                .map_err(|error| restore_error_from_runtime("initialize restore runtime", error))?,
        );
    }
    runtime_guard.as_mut().ok_or_else(|| {
        restore_command_error(
            "initialize restore runtime",
            "runtime_state_unavailable",
            "CoffeePOS could not initialize the managed runtime for restore.",
            "Keep restore fenced and retry after restarting CoffeePOS Desktop.",
        )
    })
}

#[cfg(debug_assertions)]
fn verify_previous_store_after_restore(
    data_root: &Path,
    journal: &restore::RestoreJournal,
    runtime: &mut RuntimeManager,
) -> Result<(), restore::RestoreErrorInfo> {
    if journal.original_state == restore::RestoreOriginalState::NoPreviousStore {
        return Ok(());
    }
    let info = verify_restore_runtime_health(data_root, runtime)?;
    if info.state != RuntimeState::Running {
        return Err(restore_command_error(
            "verify previous store",
            "previous_store_not_running",
            "CoffeePOS could not start the previous store for restore reconciliation.",
            "Keep restore admission blocked and inspect runtime health before retrying recovery.",
        ));
    }
    if !journal.runtime_was_running {
        runtime.stop().map_err(|error| {
            restore_error_from_runtime("stop previous-store verification", error)
        })?;
    }
    Ok(())
}

#[cfg(debug_assertions)]
fn finish_pre_cutover_abort(
    state: &ShellState,
    data_root: &Path,
    journal: &mut restore::RestoreJournal,
    runtime: &mut RuntimeManager,
) -> Result<(), restore::RestoreErrorInfo> {
    if journal.stage == restore::RestoreStage::Planned {
        restore::advance_restore_journal(data_root, journal, restore::RestoreStage::Validated)?;
    }
    if journal.stage != restore::RestoreStage::AbortStarted
        && journal.stage != restore::RestoreStage::Aborted
    {
        restore::begin_pre_cutover_abort(data_root, journal)?;
        update_restore_status_from_journal(state, journal);
    }
    if journal.stage == restore::RestoreStage::AbortStarted {
        restore::mark_restore_aborted(data_root, journal)?;
    }
    restore::cleanup_terminal_restore_owned_roots(data_root, journal)?;
    verify_previous_store_after_restore(data_root, journal, runtime)?;
    restore::retire_terminal_restore_journal(data_root, journal)?;
    state
        .restore_admission
        .lock()
        .map_err(|_| {
            restore_command_error(
                "release admission",
                "restore_gate_unavailable",
                "CoffeePOS cannot reopen admission after cancelling restore.",
                "Keep the application open and retry restore recovery.",
            )
        })?
        .release_reconciled(journal)?;
    update_restore_status_from_journal(state, journal);
    if let Ok(mut operation) = state.restore_operation.lock() {
        operation.status.finished_at = Some(restore_now());
        operation.cancelled = None;
    }
    Ok(())
}

#[cfg(debug_assertions)]
fn finish_restore_rollback(
    state: &ShellState,
    data_root: &Path,
    journal: &mut restore::RestoreJournal,
    runtime: &mut RuntimeManager,
) -> Result<(), restore::RestoreErrorInfo> {
    restore::rollback_restore_cutover(data_root, journal)?;
    update_restore_status_from_journal(state, journal);
    refresh_restore_store_state(state, data_root)?;
    verify_previous_store_after_restore(data_root, journal, runtime)?;
    restore::mark_restore_rolled_back(data_root, journal)?;
    restore::cleanup_terminal_restore_owned_roots(data_root, journal)?;
    restore::retire_terminal_restore_journal(data_root, journal)?;
    state
        .restore_admission
        .lock()
        .map_err(|_| {
            restore_command_error(
                "release admission",
                "restore_gate_unavailable",
                "CoffeePOS cannot reopen admission after verified restore rollback.",
                "Keep the application open and retry restore recovery.",
            )
        })?
        .release_reconciled(journal)?;
    update_restore_status_from_journal(state, journal);
    if let Ok(mut operation) = state.restore_operation.lock() {
        operation.status.rolled_back = true;
        operation.status.finished_at = Some(restore_now());
        operation.cancelled = None;
    }
    Ok(())
}

#[cfg(debug_assertions)]
fn create_restore_recovery_snapshot(
    data_root: &Path,
    journal: &restore::RestoreJournal,
    runtime: &mut RuntimeManager,
    target: &BackupCompatibilityTarget,
    admin_username: &str,
    cancelled: &AtomicBool,
) -> Result<(), restore::RestoreErrorInfo> {
    let recovery_root = restore::recovery_backup_root(data_root, journal)?;
    let destination_path = recovery_root.join("pre-restore.coffeepos-backup");
    let selection =
        backup_format::BackupDestinationSelection::capture(destination_path, "restore recovery")
            .map_err(|error| restore_error_from_backup("prepare recovery backup", error))?;
    let context = backup::BackupCreateContext {
        data_root: data_root.to_path_buf(),
        admin_username: admin_username.to_string(),
        source: target.source.clone(),
    };
    let (destination, _, _, warnings) = backup::preflight_restore_recovery(&context, &selection)
        .map_err(|error| restore_error_from_backup("prepare recovery backup", error))?;
    let recovery_password = restore::prepare_recovery_snapshot_password(data_root, journal)?;
    backup::run_backup_keep_runtime_stopped(
        runtime,
        backup::BackupRunRequest {
            context: &context,
            destination: &destination,
            operation_id: &journal.transaction_id,
            backup_password: recovery_password.as_str(),
            cancelled,
            warnings,
        },
        |_| {},
    )
    .map_err(|error| restore_error_from_backup("create recovery backup", error))?;
    let validation =
        backup_format::validate_backup(&destination.path, recovery_password.as_str(), target)
            .map_err(|error| restore_error_from_backup("validate recovery backup", error))?;
    if !validation.valid || !validation.inspection.can_restore {
        return Err(restore_command_error(
            "validate recovery backup",
            "recovery_backup_validation_failed",
            "CoffeePOS created a recovery snapshot but could not validate it for restore.",
            "Keep restore fenced and preserve the recovery snapshot evidence before retrying.",
        ));
    }
    Ok(())
}

#[cfg(debug_assertions)]
fn prepare_restore_staging(
    state: &ShellState,
    data_root: &Path,
    journal: &mut restore::RestoreJournal,
    candidate: &restore::RestoreCandidate,
    backup_password: &str,
    target: &BackupCompatibilityTarget,
    project_root: &Path,
    manifest: &Path,
    startup_view: StartupView,
    app_language: Option<AppLanguage>,
    cancelled: &AtomicBool,
) -> Result<backup_format::RestorePayload, restore::RestoreErrorInfo> {
    let staging_root = restore::staging_store_root(data_root, journal)?;
    let mut payload = backup_format::extract_restore_payload(
        candidate.source_path(),
        backup_password,
        target,
        &candidate.source_fingerprint().encrypted_sha256,
        &staging_root,
        cancelled,
    )
    .map_err(|error| restore_error_from_backup("extract restore payload", error))?;

    let staged_dump = staging_root.join("restore-store.sql");
    std::fs::rename(&payload.database_dump, &staged_dump).map_err(|error| {
        restore_command_error(
            "prepare restored database",
            "restore_dump_staging_failed",
            format!("CoffeePOS could not isolate the restored database dump from the target datadir: {error}."),
            "Keep restore fenced and rebuild the owned staging tree from the validated backup.",
        )
    })?;
    payload.database_dump = staged_dump;

    let mut staging_store = Store::open(staging_root.clone()).map_err(|error| {
        restore_command_error(
            "prepare target config",
            "restore_target_config_failed",
            error,
            "Discard the owned restore staging tree and retry from the validated backup.",
        )
    })?;
    staging_store
        .save_setup_profile(
            &payload.store_name,
            &payload.administrator_username,
            &payload.administrator_email,
        )
        .map_err(|error| {
            restore_command_error(
                "prepare target config",
                "restore_target_config_failed",
                error,
                "The active store is unchanged. Correct the portable store profile before retrying restore.",
            )
        })?;
    staging_store
        .save_desktop_preferences(startup_view, app_language)
        .map_err(|error| {
            restore_command_error(
                "prepare target config",
                "restore_target_config_failed",
                error,
                "The active store is unchanged. Retry restore after checking staging storage.",
            )
        })?;
    drop(staging_store);

    let resolved = runtime::resolve_development_manifest(project_root, manifest)
        .map_err(|error| restore_error_from_runtime("resolve staging runtime", error))?;
    let mut provisioner =
        Provisioner::from_development(project_root, manifest, resolved, staging_root.clone())
            .map_err(|error| restore_error_from_runtime("prepare restore staging", error))?;
    provisioner
        .configure_initial_admin(
            &payload.administrator_username,
            &payload.administrator_email,
        )
        .map_err(|error| restore_error_from_runtime("prepare restore staging", error))?;
    provisioner
        .prepare()
        .map_err(|error| restore_error_from_runtime("prepare restore staging", error))?;
    restore::advance_restore_journal(data_root, journal, restore::RestoreStage::StagingPrepared)?;
    update_restore_status_from_journal(state, journal);

    if restore_cancel_requested(cancelled) {
        return Err(restore_command_error(
            "restore",
            "cancelled",
            "Restore was cancelled before database import.",
            "CoffeePOS will reconcile the owned staging tree before another restore starts.",
        ));
    }

    provisioner
        .import_restored_database(&payload.database_dump)
        .map_err(|error| restore_error_from_runtime("import restored database", error))?;
    restore::advance_restore_journal(data_root, journal, restore::RestoreStage::DatabaseImported)?;
    update_restore_status_from_journal(state, journal);
    std::fs::remove_file(&payload.database_dump).map_err(|error| {
        restore_command_error(
            "cleanup restored database dump",
            "restore_dump_cleanup_failed",
            format!("CoffeePOS imported the restored database but could not remove its staged logical dump: {error}."),
            "Keep restore fenced and retry owned staging cleanup before cutover.",
        )
    })?;
    restore::advance_restore_journal(data_root, journal, restore::RestoreStage::UploadsRestored)?;
    update_restore_status_from_journal(state, journal);

    if restore_cancel_requested(cancelled) {
        return Err(restore_command_error(
            "restore",
            "cancelled",
            "Restore was cancelled before isolated identity verification.",
            "CoffeePOS will reconcile the owned staging tree before another restore starts.",
        ));
    }

    let mut staging_runtime =
        RuntimeManager::from_development(project_root, manifest, staging_root.clone())
            .map_err(|error| restore_error_from_runtime("initialize staging runtime", error))?;
    let staging_result = (|| {
        let runtime_info = staging_runtime
            .start_for_provisioning()
            .map_err(|error| restore_error_from_runtime("start staging runtime", error))?;
        provisioner
            .verify_restored_wordpress_identity(
                &runtime_info,
                &payload.store_name,
                &payload.administrator_username,
                &payload.administrator_email,
                payload.administrator_password.as_str(),
            )
            .map_err(|error| restore_error_from_runtime("verify restored administrator", error))?;
        restore::protect_verified_administrator_password(
            data_root,
            journal,
            payload.administrator_password.as_str(),
        )?;
        provisioner
            .complete_restored_provisioning(&runtime_info, &payload.store_name)
            .map_err(|error| restore_error_from_runtime("complete restored provisioning", error))?;
        restore::advance_restore_journal(
            data_root,
            journal,
            restore::RestoreStage::TargetSecretsBound,
        )?;
        update_restore_status_from_journal(state, journal);

        let inspected = provisioner.inspect();
        if inspected.state != ProvisioningState::Ready {
            return Err(restore_command_error(
                "verify restore staging",
                "staging_provisioning_not_ready",
                "The isolated restored store did not reach the ready provisioning state.",
                "Keep the active store unchanged and repair the owned restore staging tree before retrying.",
            ));
        }
        let http_port = runtime_info.http_port.ok_or_else(|| {
            restore_command_error(
                "verify restore staging",
                "restore_http_port_unavailable",
                "The isolated restored store did not expose an HTTP port.",
                "Keep the active store unchanged and retry staging verification.",
            )
        })?;
        let health = runtime::probe_coffeepos_health(&staging_root, http_port);
        if health.state != runtime::CoffeePosHealthState::Healthy {
            return Err(restore_command_error(
                "verify restore staging",
                "staging_health_failed",
                "The isolated restored CoffeePOS store did not pass machine-health verification.",
                health.error.map(|error| error.recovery).unwrap_or_else(|| {
                    "Inspect the staged store health before retrying restore.".into()
                }),
            ));
        }
        Ok(())
    })();
    let stop_result = staging_runtime.stop();
    if let Err(stop_error) = stop_result {
        return Err(restore_command_error(
            "stop staging runtime",
            "staging_cleanup_unconfirmed",
            stop_error.message,
            "Keep restore admission blocked until all staging processes are confirmed stopped. Do not remove the staging tree manually.",
        ));
    }
    staging_result?;
    restore::advance_restore_journal(data_root, journal, restore::RestoreStage::StagingVerified)?;
    update_restore_status_from_journal(state, journal);
    Ok(payload)
}

fn remember_restore_error(state: &ShellState, error: &restore::RestoreErrorInfo) {
    if let Ok(mut operation) = state.restore_operation.lock() {
        operation.status.last_error = Some(error.clone());
        operation.status.failed = true;
    }
}

fn publish_restore_recovery_failure(state: &ShellState, error: &restore::RestoreErrorInfo) {
    let _ = state.restore_admission.lock().map(|mut gate| {
        if !gate.blocks_managed_operations() {
            let _ = gate.acquire("unresolved_restore_recovery");
        }
    });
    if let Ok(mut operation) = state.restore_operation.lock() {
        operation.status.stage = "recovery_required".into();
        operation.status.failed = true;
        operation.status.needs_recovery = true;
        operation.status.recovery_required = true;
        operation.status.last_error = Some(error.clone());
        operation.status.finished_at = None;
        operation.cancelled = None;
    }
}

#[cfg(debug_assertions)]
fn run_restore_apply_worker(
    app: &tauri::AppHandle,
    candidate: restore::RestoreCandidate,
    backup_password: &str,
    cancelled: Arc<AtomicBool>,
) -> Result<RestoreResult, restore::RestoreErrorInfo> {
    let state = app.state::<ShellState>();
    let _lifecycle_guard =
        try_lifecycle(&state, "apply a restore transaction").map_err(|error| {
            restore_command_error(
                "admission",
                "lifecycle_busy",
                error,
                "Wait for the current managed operation to finish, then retry restore.",
            )
        })?;
    if state
        .backup
        .lock()
        .map_err(|_| {
            restore_command_error(
                "admission",
                "backup_state_unavailable",
                "CoffeePOS cannot inspect backup admission before restore.",
                "Restart CoffeePOS Desktop before retrying restore.",
            )
        })?
        .active()
    {
        return Err(restore_command_error(
            "admission",
            "backup_in_progress",
            "A portable backup is already active.",
            "Wait for backup cleanup to finish before applying restore.",
        ));
    }

    let data_root = application_data_root(app).map_err(|error| {
        restore_command_error(
            "prepare",
            "data_root_unavailable",
            error,
            "Repair the CoffeePOS application-data path before retrying restore.",
        )
    })?;
    let (project_root, manifest, target) = restore_target_context(&data_root)?;

    // Revalidate the exact native-owned source candidate before any runtime or store mutation.
    restore::revalidate_restore_candidate(&candidate, backup_password, &target)?;
    with_store(app, &state, |_| Ok(())).map_err(|error| {
        restore_command_error(
            "prepare",
            "store_config_unavailable",
            error,
            "Repair the CoffeePOS store configuration before applying restore.",
        )
    })?;
    let provisioning_before = inspect_provisioning(app, &state).map_err(|error| {
        restore_command_error(
            "prepare",
            "provisioning_state_unavailable",
            error,
            "Repair provisioning state before applying restore.",
        )
    })?;
    let original_state = restore::RestoreOriginalState::from_provisioning(&provisioning_before)?;
    let _provisioning_guard =
        try_provisioning(&state, "apply a restore transaction").map_err(|error| {
            restore_command_error(
                "admission",
                "provisioning_busy",
                error,
                "Wait for provisioning or repair to finish before retrying restore.",
            )
        })?;
    let (startup_view, app_language) = state
        .store
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .map(|store| (store.config.startup_view.clone(), store.config.app_language))
        })
        .unwrap_or_default();

    let mut runtime_guard = state.runtime.lock().map_err(|_| {
        restore_command_error(
            "prepare",
            "runtime_state_unavailable",
            "CoffeePOS cannot access the managed runtime for restore.",
            "Restart CoffeePOS Desktop before retrying restore.",
        )
    })?;
    let runtime =
        ensure_active_restore_runtime(&mut runtime_guard, &project_root, &manifest, &data_root)?;
    let runtime_was_running = runtime.refresh().state == RuntimeState::Running;

    let mut journal =
        restore::RestoreJournal::new(&candidate, original_state, runtime_was_running, Vec::new())?;
    restore::persist_restore_journal(&data_root, &journal)?;
    restore::advance_restore_journal(&data_root, &mut journal, restore::RestoreStage::Validated)?;
    state
        .restore_admission
        .lock()
        .map_err(|_| {
            restore_command_error(
                "admission",
                "restore_gate_unavailable",
                "CoffeePOS cannot acquire restore admission.",
                "Keep the application open and retry restore recovery.",
            )
        })?
        .acquire(&journal.transaction_id)?;
    {
        let mut operation = state.restore_operation.lock().map_err(|_| {
            restore_command_error(
                "status",
                "restore_state_unavailable",
                "CoffeePOS cannot publish restore progress.",
                "Keep the application open and retry restore recovery.",
            )
        })?;
        operation.status = restore_status_from_journal(&journal, None);
        operation.status.warnings = candidate.inspection().warnings.clone();
        operation.cancelled = Some(cancelled.clone());
    }
    if let Ok(mut registry) = state.restore_candidates.lock() {
        registry.clear();
    }
    restore::prepare_restore_owned_roots(&data_root, &journal)?;

    if restore_cancel_requested(cancelled.as_ref()) {
        finish_pre_cutover_abort(&state, &data_root, &mut journal, runtime)?;
        return Ok(RestoreResult {
            operation_id: journal.transaction_id,
            status: "cancelled".into(),
        });
    }

    if let Err(error) = runtime.stop() {
        let error = restore_error_from_runtime("stop active runtime", error);
        remember_restore_error(&state, &error);
        let _ =
            restore::record_restore_error(&data_root, &mut journal, &error.code, &error.message);
        match finish_pre_cutover_abort(&state, &data_root, &mut journal, runtime) {
            Ok(()) => return Err(error),
            Err(recovery_error) => {
                set_restore_recovery_required(&state, &journal, recovery_error.clone());
                return Err(recovery_error);
            }
        }
    }
    restore::advance_restore_journal(
        &data_root,
        &mut journal,
        restore::RestoreStage::RuntimeStopped,
    )?;
    update_restore_status_from_journal(&state, &journal);

    if journal.original_state == restore::RestoreOriginalState::ExistingStore {
        let admin_username = provisioning_before
            .admin_username
            .as_deref()
            .ok_or_else(|| {
                restore_command_error(
                    "prepare recovery backup",
                    "administrator_identity_unavailable",
                    "The installed store does not expose its managed administrator identity.",
                    "Repair provisioning metadata before retrying restore.",
                )
            })?;
        if let Err(error) = create_restore_recovery_snapshot(
            &data_root,
            &journal,
            runtime,
            &target,
            admin_username,
            cancelled.as_ref(),
        ) {
            if error.code == "cancelled" && !runtime.backup_maintenance_active() {
                finish_pre_cutover_abort(&state, &data_root, &mut journal, runtime)?;
                return Ok(RestoreResult {
                    operation_id: journal.transaction_id,
                    status: "cancelled".into(),
                });
            }
            remember_restore_error(&state, &error);
            let _ = restore::record_restore_error(
                &data_root,
                &mut journal,
                &error.code,
                &error.message,
            );
            if runtime.backup_maintenance_active() {
                set_restore_recovery_required(&state, &journal, error.clone());
                return Err(error);
            }
            match finish_pre_cutover_abort(&state, &data_root, &mut journal, runtime) {
                Ok(()) => return Err(error),
                Err(recovery_error) => {
                    set_restore_recovery_required(&state, &journal, recovery_error.clone());
                    return Err(recovery_error);
                }
            }
        }
        restore::advance_restore_journal(
            &data_root,
            &mut journal,
            restore::RestoreStage::RecoveryBackupReady,
        )?;
        update_restore_status_from_journal(&state, &journal);
    }

    if restore_cancel_requested(cancelled.as_ref()) {
        finish_pre_cutover_abort(&state, &data_root, &mut journal, runtime)?;
        return Ok(RestoreResult {
            operation_id: journal.transaction_id,
            status: "cancelled".into(),
        });
    }

    let payload = match prepare_restore_staging(
        &state,
        &data_root,
        &mut journal,
        &candidate,
        backup_password,
        &target,
        &project_root,
        &manifest,
        startup_view,
        app_language,
        cancelled.as_ref(),
    ) {
        Ok(payload) => payload,
        Err(error) => {
            if error.code == "cancelled" {
                finish_pre_cutover_abort(&state, &data_root, &mut journal, runtime)?;
                return Ok(RestoreResult {
                    operation_id: journal.transaction_id,
                    status: "cancelled".into(),
                });
            }
            remember_restore_error(&state, &error);
            let _ = restore::record_restore_error(
                &data_root,
                &mut journal,
                &error.code,
                &error.message,
            );
            if error.code == "staging_cleanup_unconfirmed" {
                set_restore_recovery_required(&state, &journal, error.clone());
                return Err(error);
            }
            match finish_pre_cutover_abort(&state, &data_root, &mut journal, runtime) {
                Ok(()) => return Err(error),
                Err(recovery_error) => {
                    set_restore_recovery_required(&state, &journal, recovery_error.clone());
                    return Err(recovery_error);
                }
            }
        }
    };

    if restore_cancel_requested(cancelled.as_ref()) {
        finish_pre_cutover_abort(&state, &data_root, &mut journal, runtime)?;
        return Ok(RestoreResult {
            operation_id: journal.transaction_id,
            status: "cancelled".into(),
        });
    }

    let language_mutation_guard = match state.language_mutation.lock() {
        Ok(guard) => guard,
        Err(_) => {
            let error = restore_command_error(
                "prepare target config",
                "language_state_unavailable",
                "CoffeePOS cannot serialize Desktop language changes with restore cutover.",
                "The active store is unchanged. Restart CoffeePOS Desktop and retry restore.",
            );
            remember_restore_error(&state, &error);
            let _ = restore::record_restore_error(
                &data_root,
                &mut journal,
                &error.code,
                &error.message,
            );
            match finish_pre_cutover_abort(&state, &data_root, &mut journal, runtime) {
                Ok(()) => return Err(error),
                Err(recovery_error) => {
                    set_restore_recovery_required(&state, &journal, recovery_error.clone());
                    return Err(recovery_error);
                }
            }
        }
    };
    let mut store_guard = match state.store.lock() {
        Ok(guard) => guard,
        Err(_) => {
            let error = restore_command_error(
                "prepare target config",
                "store_state_unavailable",
                "CoffeePOS cannot serialize target Desktop preferences with restore cutover.",
                "The active store is unchanged. Restart CoffeePOS Desktop and retry restore.",
            );
            remember_restore_error(&state, &error);
            let _ = restore::record_restore_error(
                &data_root,
                &mut journal,
                &error.code,
                &error.message,
            );
            match finish_pre_cutover_abort(&state, &data_root, &mut journal, runtime) {
                Ok(()) => return Err(error),
                Err(recovery_error) => {
                    set_restore_recovery_required(&state, &journal, recovery_error.clone());
                    return Err(recovery_error);
                }
            }
        }
    };
    let preference_sync_result = (|| {
        let store = store_guard.as_ref().ok_or_else(|| {
            restore_command_error(
                "prepare target config",
                "store_config_unavailable",
                "CoffeePOS cannot access target Desktop preferences before restore cutover.",
                "The active store is unchanged. Retry restore after reopening CoffeePOS Desktop.",
            )
        })?;
        let startup_view = store.config.startup_view.clone();
        let app_language = store.config.app_language;
        let staging_root = restore::staging_store_root(&data_root, &journal)?;
        let mut staging_store = Store::open(staging_root).map_err(|error| {
            restore_command_error(
                "prepare target config",
                "restore_target_config_failed",
                error,
                "The active store is unchanged. Retry restore after checking staging storage.",
            )
        })?;
        staging_store
            .save_desktop_preferences(startup_view, app_language)
            .map_err(|error| {
                restore_command_error(
                    "prepare target config",
                    "restore_target_config_failed",
                    error,
                    "The active store is unchanged. Retry restore after checking staging storage.",
                )
            })
    })();
    if let Err(error) = preference_sync_result {
        drop(store_guard);
        remember_restore_error(&state, &error);
        let _ =
            restore::record_restore_error(&data_root, &mut journal, &error.code, &error.message);
        match finish_pre_cutover_abort(&state, &data_root, &mut journal, runtime) {
            Ok(()) => return Err(error),
            Err(recovery_error) => {
                set_restore_recovery_required(&state, &journal, recovery_error.clone());
                return Err(recovery_error);
            }
        }
    }

    let cutover_result = (|| {
        restore::begin_restore_cutover(&data_root, &mut journal)?;
        update_restore_status_from_journal(&state, &journal);
        for component in [
            restore::RestoreComponent::Site,
            restore::RestoreComponent::Database,
            restore::RestoreComponent::Uploads,
        ] {
            restore::cutover_component(&data_root, &mut journal, component)?;
        }
        restore::apply_target_config(&data_root, &mut journal)?;
        restore::mark_active_swapped(&data_root, &mut journal)?;
        update_restore_status_from_journal(&state, &journal);
        *store_guard = None;
        *store_guard = Some(Store::open(data_root.clone()).map_err(|error| {
            restore_command_error(
                "refresh store config",
                "store_config_unavailable",
                error,
                "Keep restore admission blocked and repair the active configuration before retrying recovery.",
            )
        })?);
        Ok::<(), restore::RestoreErrorInfo>(())
    })();
    drop(store_guard);
    if let Err(error) = cutover_result {
        remember_restore_error(&state, &error);
        let _ =
            restore::record_restore_error(&data_root, &mut journal, &error.code, &error.message);
        set_restore_recovery_required(&state, &journal, error.clone());
        return Err(error);
    }

    if restore_cancel_requested(cancelled.as_ref()) {
        finish_restore_rollback(&state, &data_root, &mut journal, runtime)?;
        return Ok(RestoreResult {
            operation_id: journal.transaction_id,
            status: "rolled_back".into(),
        });
    }

    let active_verification = (|| {
        let runtime_info = runtime
            .start_for_provisioning()
            .map_err(|error| restore_error_from_runtime("start active verification", error))?;
        let (resolved, runtime_root) = runtime.provisioning_context();
        let provisioner =
            Provisioner::from_development(&project_root, &manifest, resolved, runtime_root)
                .map_err(|error| restore_error_from_runtime("verify active restore", error))?;
        provisioner
            .verify_restored_wordpress_identity(
                &runtime_info,
                &payload.store_name,
                &payload.administrator_username,
                &payload.administrator_email,
                payload.administrator_password.as_str(),
            )
            .map_err(|error| restore_error_from_runtime("verify active administrator", error))?;
        let http_port = runtime_info.http_port.ok_or_else(|| {
            restore_command_error(
                "verify active restore",
                "restore_http_port_unavailable",
                "The active restored store did not expose an HTTP port for final verification.",
                "Keep restore fenced and roll back to the previous store.",
            )
        })?;
        let health = runtime::probe_coffeepos_health(&data_root, http_port);
        if health.state != runtime::CoffeePosHealthState::Healthy {
            return Err(restore_command_error(
                "verify active restore",
                "active_health_failed",
                "The active restored CoffeePOS store did not pass final machine-health verification.",
                health
                    .error
                    .map(|value| value.recovery)
                    .unwrap_or_else(|| "Keep restore fenced and roll back the transaction.".into()),
            ));
        }
        Ok::<(), restore::RestoreErrorInfo>(())
    })();
    let active_stop = runtime.stop();
    if let Err(stop_error) = active_stop {
        let error = restore_error_from_runtime("stop active verification", stop_error);
        remember_restore_error(&state, &error);
        let _ =
            restore::record_restore_error(&data_root, &mut journal, &error.code, &error.message);
        set_restore_recovery_required(&state, &journal, error.clone());
        return Err(error);
    }
    if restore_cancel_requested(cancelled.as_ref()) {
        finish_restore_rollback(&state, &data_root, &mut journal, runtime)?;
        return Ok(RestoreResult {
            operation_id: journal.transaction_id,
            status: "rolled_back".into(),
        });
    }
    if let Err(error) = active_verification {
        remember_restore_error(&state, &error);
        let _ =
            restore::record_restore_error(&data_root, &mut journal, &error.code, &error.message);
        match finish_restore_rollback(&state, &data_root, &mut journal, runtime) {
            Ok(()) => return Err(error),
            Err(recovery_error) => {
                set_restore_recovery_required(&state, &journal, recovery_error.clone());
                return Err(recovery_error);
            }
        }
    }

    restore::mark_restore_active_verified(&data_root, &mut journal)?;
    update_restore_status_from_journal(&state, &journal);
    restore::commit_verified_restore(&data_root, &mut journal)?;
    update_restore_status_from_journal(&state, &journal);
    release_reconciled_restore(&state, &data_root, &journal)?;
    drop(language_mutation_guard);

    if runtime_was_running {
        if let Err(error) = runtime.start() {
            if let Ok(mut operation) = state.restore_operation.lock() {
                operation
                    .status
                    .warnings
                    .push(format!("runtime_resume_failed: {}", error.message));
            }
        }
    }
    if let Ok(mut operation) = state.restore_operation.lock() {
        operation.status = restore_status_from_journal(&journal, None);
        operation.status.finished_at = Some(restore_now());
        operation.cancelled = None;
    }
    Ok(RestoreResult {
        operation_id: journal.transaction_id,
        status: "succeeded".into(),
    })
}

fn recover_restore_admission(
    app: &tauri::AppHandle,
    state: &ShellState,
) -> Result<Option<restore::RestoreJournal>, restore::RestoreErrorInfo> {
    let data_root = application_data_root(app).map_err(|error| {
        restore_command_error(
            "bootstrap recovery",
            "data_root_unavailable",
            error,
            "Repair the CoffeePOS application-data path before starting managed operations.",
        )
    })?;
    let journal = match restore::load_restore_journal(&data_root) {
        Ok(journal) => journal,
        Err(error) => {
            publish_restore_recovery_failure(state, &error);
            return Err(error);
        }
    };
    if let Some(journal) = journal.as_ref() {
        state
            .restore_admission
            .lock()
            .map_err(|_| {
                restore_command_error(
                    "bootstrap recovery",
                    "restore_gate_unavailable",
                    "CoffeePOS cannot restore the durable restore admission gate.",
                    "Keep the application open and retry recovery before using the store.",
                )
            })?
            .recover_from_journal(journal);
        if let Ok(mut operation) = state.restore_operation.lock() {
            let last_error = operation.status.last_error.clone();
            operation.status = restore_status_from_journal(journal, last_error);
            operation.status.needs_recovery = true;
            operation.status.recovery_required = true;
        }
    }
    Ok(journal)
}

#[cfg(debug_assertions)]
fn verify_recovered_active_target(
    state: &ShellState,
    data_root: &Path,
    project_root: &Path,
    manifest: &Path,
    runtime: &mut RuntimeManager,
) -> Result<(), restore::RestoreErrorInfo> {
    let (store_name, admin_username, admin_email) = {
        let mut guard = state.store.lock().map_err(|_| {
            restore_command_error(
                "verify recovered target",
                "store_state_unavailable",
                "CoffeePOS cannot access the active store configuration during restore recovery.",
                "Keep restore fenced and retry recovery after restarting CoffeePOS Desktop.",
            )
        })?;
        if guard.is_none() {
            *guard = Some(Store::open(data_root.to_path_buf()).map_err(|error| {
                restore_command_error(
                    "verify recovered target",
                    "active_store_config_unavailable",
                    error,
                    "Keep restore fenced and repair the active restored configuration before retrying recovery.",
                )
            })?);
        }
        let store = guard.as_ref().ok_or_else(|| {
            restore_command_error(
                "verify recovered target",
                "active_store_config_unavailable",
                "CoffeePOS cannot access the active restored configuration.",
                "Keep restore fenced and retry recovery after restarting CoffeePOS Desktop.",
            )
        })?;
        (
            store.config.store_name.clone(),
            store.config.setup_admin_username.clone().ok_or_else(|| {
                restore_command_error(
                    "verify recovered target",
                    "administrator_identity_unavailable",
                    "The active restored configuration is missing its administrator username.",
                    "Keep restore fenced and restore the transaction configuration evidence.",
                )
            })?,
            store.config.setup_admin_email.clone().ok_or_else(|| {
                restore_command_error(
                    "verify recovered target",
                    "administrator_identity_unavailable",
                    "The active restored configuration is missing its administrator email.",
                    "Keep restore fenced and restore the transaction configuration evidence.",
                )
            })?,
        )
    };
    let admin_password = secret::load(&data_root.join(WORDPRESS_ADMIN_SECRET)).map_err(|error| {
        restore_command_error(
            "verify recovered target",
            "administrator_secret_unavailable",
            error,
            "Keep restore fenced and preserve rollback evidence; do not reset the administrator password.",
        )
    })?;
    let runtime_info = runtime.start_for_provisioning().map_err(|error| {
        restore_error_from_runtime("start recovered target verification", error)
    })?;
    let (resolved, runtime_root) = runtime.provisioning_context();
    let provisioner = Provisioner::from_development(project_root, manifest, resolved, runtime_root)
        .map_err(|error| restore_error_from_runtime("verify recovered target", error))?;
    let verification = provisioner
        .verify_restored_wordpress_identity(
            &runtime_info,
            &store_name,
            &admin_username,
            &admin_email,
            &admin_password,
        )
        .map_err(|error| restore_error_from_runtime("verify recovered target", error))
        .and_then(|_| {
            let http_port = runtime_info.http_port.ok_or_else(|| {
                restore_command_error(
                    "verify recovered target",
                    "restore_http_port_unavailable",
                    "The recovered target did not expose an HTTP port.",
                    "Keep restore fenced and roll back if target verification cannot be completed.",
                )
            })?;
            let health = runtime::probe_coffeepos_health(data_root, http_port);
            if health.state != runtime::CoffeePosHealthState::Healthy {
                return Err(restore_command_error(
                    "verify recovered target",
                    "active_health_failed",
                    "The recovered active target did not pass CoffeePOS machine-health verification.",
                    health
                        .error
                        .map(|value| value.recovery)
                        .unwrap_or_else(|| "Keep restore fenced and roll back the transaction.".into()),
                ));
            }
            Ok(())
        });
    let stop_result = runtime.stop();
    if let Err(error) = stop_result {
        return Err(restore_error_from_runtime(
            "stop recovered target verification",
            error,
        ));
    }
    verification
}

#[cfg(debug_assertions)]
fn recover_restore_transaction(
    app: &tauri::AppHandle,
    state: &ShellState,
) -> Result<(), restore::RestoreErrorInfo> {
    let Some(mut journal) = recover_restore_admission(app, state)? else {
        return Ok(());
    };
    if state.restore_requested.load(Ordering::Acquire) {
        return Ok(());
    }
    let _lifecycle_guard =
        try_lifecycle(state, "recover restore transaction").map_err(|error| {
            restore_command_error(
                "recover",
                "lifecycle_busy",
                error,
                "Wait for the current lifecycle action to finish, then retry restore recovery.",
            )
        })?;
    let _provisioning_guard =
        try_provisioning(state, "recover restore transaction").map_err(|error| {
            restore_command_error(
                "recover",
                "provisioning_busy",
                error,
                "Wait for the current provisioning action to finish, then retry restore recovery.",
            )
        })?;
    let data_root = application_data_root(app).map_err(|error| {
        restore_command_error(
            "recover",
            "data_root_unavailable",
            error,
            "Repair the CoffeePOS application-data path before retrying restore recovery.",
        )
    })?;
    let (project_root, manifest, _) = restore_target_context(&data_root)?;
    let mut runtime_guard = state.runtime.lock().map_err(|_| {
        restore_command_error(
            "recover",
            "runtime_state_unavailable",
            "CoffeePOS cannot access runtime state for restore recovery.",
            "Restart CoffeePOS Desktop and retry restore recovery.",
        )
    })?;
    let runtime =
        ensure_active_restore_runtime(&mut runtime_guard, &project_root, &manifest, &data_root)?;
    if runtime.requires_exit_confirmation() {
        if let Err(error) = runtime.stop() {
            let error = restore_error_from_runtime("stop runtime for restore recovery", error);
            set_restore_recovery_required(state, &journal, error.clone());
            return Err(error);
        }
    }
    // Restore owns recovery ordering. Reconcile an interrupted internal snapshot before touching
    // the transaction-owned recovery root.
    if let Err(error) = backup::recover_interrupted_backup(&data_root) {
        let error = restore_error_from_backup("recover restore snapshot", error);
        set_restore_recovery_required(state, &journal, error.clone());
        return Err(error);
    }
    let evidence = restore::inspect_restore_recovery_evidence(
        &data_root,
        &journal,
        !runtime.requires_exit_confirmation(),
    );
    let classification = restore::classify_restore_recovery(&journal, &evidence);
    use restore::RestoreRecoveryAction;
    let recovery_result = match classification.action {
        RestoreRecoveryAction::CleanupPreMutation
        | RestoreRecoveryAction::ResumeOrAbortPreCutover
        | RestoreRecoveryAction::CompleteAbort => {
            finish_pre_cutover_abort(state, &data_root, &mut journal, runtime)
        }
        RestoreRecoveryAction::CompleteCutoverOrRollback => {
            let complete_cutover = (|| {
                for component in [
                    restore::RestoreComponent::Site,
                    restore::RestoreComponent::Database,
                    restore::RestoreComponent::Uploads,
                ] {
                    restore::cutover_component(&data_root, &mut journal, component)?;
                }
                restore::apply_target_config(&data_root, &mut journal)?;
                restore::mark_active_swapped(&data_root, &mut journal)?;
                refresh_restore_store_state(state, &data_root)?;
                Ok::<(), restore::RestoreErrorInfo>(())
            })();
            complete_cutover.and_then(|_| {
                finish_restore_rollback(state, &data_root, &mut journal, runtime)
            })
        }
        RestoreRecoveryAction::VerifyActiveOrRollback => {
            refresh_restore_store_state(state, &data_root)?;
            match verify_recovered_active_target(
                state,
                &data_root,
                &project_root,
                &manifest,
                runtime,
            ) {
                Ok(()) => {
                    restore::mark_restore_active_verified(&data_root, &mut journal)?;
                    restore::commit_verified_restore(&data_root, &mut journal)?;
                    update_restore_status_from_journal(state, &journal);
                    release_reconciled_restore(state, &data_root, &journal)
                }
                Err(verify_error) => {
                    remember_restore_error(state, &verify_error);
                    finish_restore_rollback(state, &data_root, &mut journal, runtime)
                }
            }
        }
        RestoreRecoveryAction::CommitVerifiedActive => {
            restore::commit_verified_restore(&data_root, &mut journal)?;
            update_restore_status_from_journal(state, &journal);
            release_reconciled_restore(state, &data_root, &journal)
        }
        RestoreRecoveryAction::CompleteRollback => {
            finish_restore_rollback(state, &data_root, &mut journal, runtime)
        }
        RestoreRecoveryAction::ReconcileRolledBack
        | RestoreRecoveryAction::ReconcileAborted => {
            restore::reconcile_terminal_component_markers(&data_root, &mut journal)?;
            restore::cleanup_terminal_restore_owned_roots(&data_root, &journal)?;
            verify_previous_store_after_restore(&data_root, &journal, runtime)?;
            restore::retire_terminal_restore_journal(&data_root, &journal)?;
            state
                .restore_admission
                .lock()
                .map_err(|_| {
                    restore_command_error(
                        "recover",
                        "restore_gate_unavailable",
                        "CoffeePOS cannot release restore admission after terminal reconciliation.",
                        "Keep the application open and retry restore recovery.",
                    )
                })?
                .release_reconciled(&journal)?;
            update_restore_status_from_journal(state, &journal);
            Ok(())
        }
        RestoreRecoveryAction::ReconcileCommitted => {
            restore::reconcile_terminal_component_markers(&data_root, &mut journal)?;
            release_reconciled_restore(state, &data_root, &journal)
        }
        RestoreRecoveryAction::Blocked => Err(restore_command_error(
            "recover",
            &classification.reason_code,
            "CoffeePOS cannot safely choose an automatic restore recovery action from the durable evidence.",
            "Keep restore admission blocked and preserve config/restore.json plus transaction-owned evidence for explicit recovery.",
        )),
    };
    if let Err(error) = recovery_result {
        remember_restore_error(state, &error);
        let _ =
            restore::record_restore_error(&data_root, &mut journal, &error.code, &error.message);
        set_restore_recovery_required(state, &journal, error.clone());
        return Err(error);
    }
    if journal.runtime_was_running
        && matches!(
            journal.stage,
            restore::RestoreStage::Committed
                | restore::RestoreStage::Aborted
                | restore::RestoreStage::RolledBack
        )
        && runtime.refresh().state != RuntimeState::Running
    {
        if let Err(error) = runtime.start() {
            if let Ok(mut operation) = state.restore_operation.lock() {
                operation
                    .status
                    .warnings
                    .push(format!("runtime_resume_failed: {}", error.message));
            }
        }
    }
    Ok(())
}

#[tauri::command]
async fn get_restore_status(
    app: tauri::AppHandle,
) -> Result<RestoreStatus, restore::RestoreErrorInfo> {
    #[cfg(debug_assertions)]
    {
        if !app
            .state::<ShellState>()
            .restore_requested
            .load(Ordering::Acquire)
        {
            let worker_app = app.clone();
            let recovery = tauri::async_runtime::spawn_blocking(move || {
                let state = worker_app.state::<ShellState>();
                recover_restore_transaction(&worker_app, &state)
            })
            .await;
            match recovery {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    publish_restore_recovery_failure(&app.state::<ShellState>(), &error);
                }
                Err(_) => {
                    let error = restore_command_error(
                        "status",
                        "restore_recovery_worker_failed",
                        "CoffeePOS restore recovery worker stopped unexpectedly.",
                        "Keep the application open and retry restore status.",
                    );
                    publish_restore_recovery_failure(&app.state::<ShellState>(), &error);
                }
            }
        }
        app.state::<ShellState>()
            .restore_operation
            .lock()
            .map(|operation| operation.status.clone())
            .map_err(|_| {
                restore_command_error(
                    "status",
                    "restore_state_unavailable",
                    "CoffeePOS cannot read restore status.",
                    "Restart CoffeePOS Desktop and retry restore recovery.",
                )
            })
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        Ok(RestoreStatus::default())
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
async fn inspect_restore_backup(
    app: tauri::AppHandle,
    backup_password: String,
) -> Result<Option<restore::RestoreInspection>, restore::RestoreErrorInfo> {
    #[cfg(debug_assertions)]
    {
        let password = zeroize::Zeroizing::new(backup_password);
        {
            let state = app.state::<ShellState>();
            if restore_gate_blocks(&state).map_err(|message| {
                restore_command_error(
                    "inspect",
                    "restore_gate_unavailable",
                    message,
                    "Restart CoffeePOS Desktop and retry restore recovery.",
                )
            })? || state.restore_requested.load(Ordering::Acquire)
            {
                return Err(restore_command_error(
                    "inspect",
                    "restore_already_active",
                    "A restore transaction is already active or requires recovery.",
                    "Finish the current restore recovery before inspecting another backup.",
                ));
            }
        }
        let worker_app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let selected = backup_format::choose_backup_file("restore inspect")
                .map_err(|error| restore_error_from_backup("inspect", error))?;
            let Some(path) = selected else {
                return Ok(None);
            };
            let state = worker_app.state::<ShellState>();
            let data_root = application_data_root(&worker_app).map_err(|error| {
                restore_command_error(
                    "inspect",
                    "data_root_unavailable",
                    error,
                    "Repair the CoffeePOS application-data path and retry restore inspection.",
                )
            })?;
            let (_, _, target) = restore_target_context(&data_root)?;
            let candidate = restore::inspect_restore_candidate(&path, password.as_str(), &target)?;
            let mut registry = state.restore_candidates.lock().map_err(|_| {
                restore_command_error(
                    "inspect",
                    "restore_candidate_state_unavailable",
                    "CoffeePOS cannot retain the validated restore candidate.",
                    "Restart CoffeePOS Desktop and inspect the backup again.",
                )
            })?;
            registry.clear();
            Ok(Some(registry.insert(candidate)))
        })
        .await
        .map_err(|_| {
            restore_command_error(
                "inspect",
                "restore_worker_failed",
                "CoffeePOS could not complete restore inspection.",
                "Retry inspection or restart CoffeePOS Desktop.",
            )
        })?
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = backup_password;
        Err(restore_command_error(
            "inspect",
            "runtime_artifacts_unavailable",
            "Restore inspection is not available until managed runtime artifacts are packaged for this build.",
            "Use the qualified Windows development build until runtime packaging is completed.",
        ))
    }
}

#[tauri::command]
async fn apply_restore(
    app: tauri::AppHandle,
    candidate_id: String,
    backup_password: String,
) -> Result<RestoreResult, restore::RestoreErrorInfo> {
    #[cfg(debug_assertions)]
    {
        let candidate = {
            let state = app.state::<ShellState>();
            if restore_gate_blocks(&state).map_err(|message| {
                restore_command_error(
                    "apply",
                    "restore_gate_unavailable",
                    message,
                    "Restart CoffeePOS Desktop and recover the existing restore transaction.",
                )
            })? {
                return Err(restore_command_error(
                    "apply",
                    "restore_already_active",
                    "A restore transaction is already active or requires recovery.",
                    "Finish restore recovery before applying another backup.",
                ));
            }
            let candidate = state
                .restore_candidates
                .lock()
                .map_err(|_| {
                    restore_command_error(
                        "apply",
                        "restore_candidate_state_unavailable",
                        "CoffeePOS cannot access the validated restore candidate.",
                        "Restart CoffeePOS Desktop and inspect the backup again.",
                    )
                })?
                .get(&candidate_id)
                .cloned();
            let candidate = candidate.ok_or_else(|| {
                restore_command_error(
                    "apply",
                    "stale_restore_candidate",
                    "The selected restore candidate is no longer available.",
                    "Choose and inspect the backup again before applying restore.",
                )
            })?;
            if state
                .restore_requested
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err(restore_command_error(
                    "apply",
                    "restore_already_active",
                    "Another restore request is already starting.",
                    "Wait for the current restore request to publish its status.",
                ));
            }
            if state
                .lifecycle_requested
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                state.restore_requested.store(false, Ordering::Release);
                return Err(restore_command_error(
                    "apply",
                    "lifecycle_busy",
                    "CoffeePOS is already completing another managed lifecycle operation.",
                    "Wait for that operation to finish before applying restore.",
                ));
            }
            if restore_gate_blocks(&state).unwrap_or(true) {
                state.lifecycle_requested.store(false, Ordering::Release);
                state.restore_requested.store(false, Ordering::Release);
                return Err(restore_command_error(
                    "apply",
                    "restore_already_active",
                    "A durable restore transaction became active before this request could start.",
                    "Refresh restore status and finish recovery before applying another backup.",
                ));
            }
            candidate
        };
        let password = zeroize::Zeroizing::new(backup_password);
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let state = app.state::<ShellState>();
            if let Ok(mut operation) = state.restore_operation.lock() {
                operation.status = RestoreStatus {
                    stage: "planned".into(),
                    started_at: Some(restore_now()),
                    ..RestoreStatus::default()
                };
                operation.cancelled = Some(cancelled.clone());
            };
        }
        let worker_app = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            run_restore_apply_worker(
                &worker_app,
                candidate,
                password.as_str(),
                cancelled,
            )
        })
        .await
        .map_err(|_| {
            restore_command_error(
                "apply",
                "restore_worker_failed",
                "CoffeePOS restore worker stopped unexpectedly.",
                "Keep the application open and use restore recovery before another managed operation.",
            )
        });
        let state = app.state::<ShellState>();
        state.restore_requested.store(false, Ordering::Release);
        state.lifecycle_requested.store(false, Ordering::Release);
        match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                let gate_blocked = restore_gate_blocks(&state).unwrap_or(true);
                if !gate_blocked {
                    if let Ok(mut operation) = state.restore_operation.lock() {
                        if operation.status.stage == "planned"
                            || operation.status.operation_id.is_none()
                        {
                            operation.status.stage = "failed".into();
                            operation.status.failed = true;
                            operation.status.finished_at = Some(restore_now());
                            operation.status.last_error = Some(error.clone());
                            operation.cancelled = None;
                        }
                    }
                }
                Err(error)
            }
            Err(error) => {
                if let Ok(mut operation) = state.restore_operation.lock() {
                    operation.status.stage = "recovery_required".into();
                    operation.status.failed = true;
                    operation.status.needs_recovery = true;
                    operation.status.recovery_required = true;
                    operation.status.last_error = Some(error.clone());
                }
                Err(error)
            }
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = candidate_id;
        let _ = backup_password;
        Err(restore_command_error(
            "apply",
            "runtime_artifacts_unavailable",
            "Restore is not available until managed runtime artifacts are packaged for this build.",
            "Use the qualified Windows development build until runtime packaging is completed.",
        ))
    }
}

#[tauri::command]
fn cancel_restore(
    state: State<'_, ShellState>,
    operation_id: String,
) -> Result<(), restore::RestoreErrorInfo> {
    let operation = state.restore_operation.lock().map_err(|_| {
        restore_command_error(
            "cancel",
            "restore_state_unavailable",
            "CoffeePOS cannot access the active restore cancellation token.",
            "Keep the application open and retry restore status before cancelling again.",
        )
    })?;
    if operation.status.operation_id.as_deref() != Some(operation_id.as_str()) {
        return Err(restore_command_error(
            "cancel",
            "restore_operation_mismatch",
            "The restore operation changed before the cancellation request was applied.",
            "Refresh restore status before requesting cancellation again.",
        ));
    }
    if matches!(
        operation.status.stage.as_str(),
        "active_verified"
            | "committed"
            | "cleanup"
            | "abort_started"
            | "rollback_started"
            | "aborted"
            | "rolled_back"
    ) {
        return Err(restore_command_error(
            "cancel",
            "restore_cancellation_closed",
            "Restore can no longer accept a new cancellation request at its current stage.",
            "Allow the current reconciliation step to finish and refresh restore status.",
        ));
    }
    let cancelled = operation.cancelled.as_ref().ok_or_else(|| {
        restore_command_error(
            "cancel",
            "restore_cancellation_unavailable",
            "The restore cancellation token is no longer active.",
            "Refresh restore status before requesting cancellation again.",
        )
    })?;
    cancelled.store(true, Ordering::Release);
    Ok(())
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
        ensure_restore_allows_managed_operation(&state, "create a backup").map_err(|message| {
            backup_command_error(
                "preflight",
                "restore_in_progress",
                &message,
                "Finish restore recovery before creating another backup.",
            )
        })?;
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
    #[cfg(debug_assertions)]
    {
        if !state.restore_requested.load(Ordering::Acquire) {
            match recover_restore_admission(&app, &state) {
                Ok(Some(_)) => {
                    if let Err(error) = recover_restore_transaction(&app, &state) {
                        publish_restore_recovery_failure(&state, &error);
                    }
                }
                Ok(None) => {}
                Err(error) => publish_restore_recovery_failure(&state, &error),
            }
        }
    }
    let backup_active = state
        .backup
        .lock()
        .map_err(|_| "Backup state unavailable. Restart CoffeePOS Desktop.".to_string())?
        .active();
    // An active backup owns the runtime mutex for its full maintenance window. WebView reloads
    // still need shell metadata so they can reconnect to get_backup_status/cancel_backup instead
    // of blocking behind that mutex until the backup is already over.
    let restore_blocked = restore_gate_blocks(&state)?;
    let fresh_runtime = if backup_active || restore_blocked {
        false
    } else {
        state
            .runtime
            .lock()
            .map_err(|_| "Runtime state unavailable. Restart CoffeePOS Desktop.".to_string())?
            .is_none()
    };
    if fresh_runtime && !backup_active && !restore_blocked {
        let root = application_data_root(&app)?;
        backup::recover_interrupted_backup(&root)
            .map_err(|error| format!("{} {}", error.message, error.recovery))?;
    }
    let shell = with_store(&app, &state, |_| Ok(()))?;
    let language = shell.config.app_language.unwrap_or_default();
    set_effective_app_language(&state, language);
    let _ = refresh_native_tray(&app, language);
    Ok(shell)
}

#[tauri::command]
fn save_app_settings(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
    startup_view: StartupView,
) -> Result<ShellInfo, String> {
    ensure_restore_allows_managed_operation(&state, "save application settings")?;
    let _lifecycle_guard = try_lifecycle(&state, "save application settings")?;
    with_store(&app, &state, |store| store.save_startup_view(startup_view))
}

#[tauri::command]
fn save_app_language(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
    app_language: AppLanguage,
) -> Result<ShellInfo, String> {
    let _language_mutation_guard = state.language_mutation.lock().map_err(|_| {
        "Application language state unavailable. Restart CoffeePOS Desktop.".to_string()
    })?;
    let restore_blocked = restore_gate_blocks(&state)?;
    let restore_status = state
        .restore_operation
        .lock()
        .map_err(|_| "Restore state unavailable. Restart CoffeePOS Desktop.".to_string())?;
    let pre_cutover_restore = matches!(
        restore_status.status.stage.as_str(),
        "planned"
            | "validated"
            | "runtime_stopped"
            | "recovery_backup_ready"
            | "staging_prepared"
            | "database_imported"
            | "uploads_restored"
            | "target_secrets_bound"
            | "staging_verified"
    );
    if restore_status.status.recovery_required || (restore_blocked && !pre_cutover_restore) {
        return Err(
            "Restore recovery must finish before the application language can be saved.".into(),
        );
    }
    drop(restore_status);
    let shell = with_store(&app, &state, |store| store.save_app_language(app_language))?;
    let language = shell.config.app_language.unwrap_or_default();
    set_effective_app_language(&state, language);
    let _ = refresh_native_tray(&app, language);
    Ok(shell)
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
        ensure_restore_allows_managed_operation(&state, "save the initial store profile")?;
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
        ensure_restore_allows_managed_operation(&state, "copy the administrator password")?;
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
async fn get_runtime_info(app: tauri::AppHandle) -> Result<RuntimeInfo, runtime::RuntimeErrorInfo> {
    read_runtime_blocking(app, RuntimeManager::refresh).await
}

#[tauri::command]
async fn start_runtime(app: tauri::AppHandle) -> Result<RuntimeInfo, runtime::RuntimeErrorInfo> {
    run_runtime_blocking(app, "start the runtime", RuntimeManager::start).await
}

#[tauri::command]
async fn stop_runtime(app: tauri::AppHandle) -> Result<RuntimeInfo, runtime::RuntimeErrorInfo> {
    run_runtime_blocking(app, "stop the runtime", RuntimeManager::stop).await
}

#[tauri::command]
async fn restart_runtime(app: tauri::AppHandle) -> Result<RuntimeInfo, runtime::RuntimeErrorInfo> {
    run_runtime_blocking(app, "restart the runtime", RuntimeManager::restart).await
}

#[tauri::command]
async fn retry_runtime_health(
    app: tauri::AppHandle,
) -> Result<RuntimeInfo, runtime::RuntimeErrorInfo> {
    run_runtime_blocking(app, "recheck runtime health", |runtime| {
        let info = runtime.refresh();
        if info.state != RuntimeState::Running {
            return Err(runtime::RuntimeErrorInfo {
                component: "runtime".into(),
                operation: "health retry".into(),
                code: "runtime_health_error".into(),
                message: "The local store is not running.".into(),
                recovery: "Start the store before retrying its health check.".into(),
            });
        }
        Ok(runtime.refresh_wordpress_health())
    })
    .await
}

#[tauri::command]
async fn get_health_diagnostics(
    app: tauri::AppHandle,
) -> Result<HealthDiagnosticsInfo, runtime::RuntimeErrorInfo> {
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
async fn get_repair_plan(app: tauri::AppHandle) -> Result<RepairPlan, runtime::RuntimeErrorInfo> {
    #[cfg(debug_assertions)]
    {
        let result = tauri::async_runtime::spawn_blocking(move || -> Result<RepairPlan, String> {
            let state = app.state::<ShellState>();
            ensure_restore_allows_managed_operation(&state, "inspect the repair plan")?;
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
        .map_err(|error| {
            repair_ipc_error(
                "inspect repair plan",
                format!("Repair-plan worker failed: {error}."),
            )
        })?;
        result.map_err(|message| repair_ipc_error("inspect repair plan", message))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        Err(repair_ipc_error(
            "inspect repair plan",
            "Repair is currently qualified only for the Windows development build until runtime resources are packaged.",
        ))
    }
}

#[tauri::command]
async fn apply_repair(
    app: tauri::AppHandle,
    plan_id: String,
    inputs: Option<RepairInputs>,
) -> Result<RepairApplyResult, runtime::RuntimeErrorInfo> {
    #[cfg(debug_assertions)]
    {
        {
            let state = app.state::<ShellState>();
            ensure_restore_allows_managed_operation(&state, "apply the repair plan")
                .map_err(|message| repair_ipc_error("apply repair", message))?;
            if state.lifecycle_requested.swap(true, Ordering::AcqRel) {
                return Err(repair_ipc_error(
                    "apply repair",
                    "Runtime lifecycle is busy. Wait for the current operation to finish, then retry repair.",
                ));
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
                    code: "repair_error".into(),
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
                            code: "repair_error".into(),
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
                            code: "repair_error".into(),
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
                                                code: "repair_error".into(),
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
                                            code: "repair_error".into(),
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
                                code: "repair_error".into(),
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
                            code: "repair_error".into(),
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
        let worker_result = result.map_err(|error| {
            repair_ipc_error("apply repair", format!("Repair worker failed: {error}."))
        })?;
        worker_result.map_err(|message| repair_ipc_error("apply repair", message))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = plan_id;
        let _ = inputs;
        Err(repair_ipc_error(
            "apply repair",
            "Repair is currently qualified only for the Windows development build until runtime resources are packaged.",
        ))
    }
}

#[tauri::command]
async fn refresh_runtime_maintenance(
    app: tauri::AppHandle,
) -> Result<RuntimeInfo, runtime::RuntimeErrorInfo> {
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<RuntimeInfo, String> {
        let state = app.state::<ShellState>();
        ensure_restore_allows_managed_operation(&state, "run background maintenance")?;
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
    .map_err(|error| {
        runtime_ipc_error(
            "run background maintenance",
            format!("Runtime maintenance worker failed: {error}."),
        )
    })?;
    result.map_err(|message| runtime_ipc_error("run background maintenance", message))
}

#[tauri::command]
fn get_provisioning_info(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<ProvisioningInfo, runtime::RuntimeErrorInfo> {
    #[cfg(debug_assertions)]
    {
        inspect_provisioning(&app, &state)
            .map_err(|message| provisioning_ipc_error("inspect provisioning", message))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = state;
        Err(provisioning_ipc_error(
            "inspect provisioning",
            "Bundled WordPress resources are not packaged yet. Use a qualified development build.",
        ))
    }
}

#[tauri::command]
fn open_wordpress(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<String, runtime::RuntimeErrorInfo> {
    #[cfg(debug_assertions)]
    {
        let result = (|| -> Result<String, String> {
            ensure_restore_allows_managed_operation(&state, "open WordPress")?;
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
        })();
        result.map_err(|message| runtime_ipc_error("open WordPress", message))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = state;
        Err(runtime_ipc_error(
            "open WordPress",
            "Bundled runtime resources are not packaged yet. Open WordPress is available only in a qualified development build.",
        ))
    }
}

#[tauri::command]
fn open_pos(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<String, runtime::RuntimeErrorInfo> {
    #[cfg(debug_assertions)]
    {
        let result = (|| -> Result<String, String> {
            ensure_restore_allows_managed_operation(&state, "open the POS")?;
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
        })();
        result.map_err(|message| runtime_ipc_error("open POS", message))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = state;
        Err(runtime_ipc_error(
            "open POS",
            "Bundled runtime resources are not packaged yet. Open POS is available only in a qualified development build.",
        ))
    }
}

#[tauri::command]
fn provision_wordpress(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<ProvisioningInfo, runtime::RuntimeErrorInfo> {
    #[cfg(debug_assertions)]
    {
        let result = (|| -> Result<ProvisioningInfo, String> {
            ensure_restore_allows_managed_operation(&state, "provision WordPress")?;
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
        })();
        result.map_err(|message| provisioning_ipc_error("provision WordPress", message))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        let _ = state;
        Err(provisioning_ipc_error(
            "provision WordPress",
            "Bundled WordPress resources are not packaged yet. Use a qualified development build.",
        ))
    }
}

fn main() {
    tauri::Builder::default()
        .manage(ShellState::default())
        .setup(|app| {
            #[cfg(debug_assertions)]
            {
                let state = app.state::<ShellState>();
                if let Err(error) = recover_restore_admission(app.handle(), &state) {
                    publish_restore_recovery_failure(&state, &error);
                }
            }
            let state = app.state::<ShellState>();
            let language = application_data_root(app.handle())
                .ok()
                .and_then(|root| config::read_app_language(&root).ok().flatten())
                .unwrap_or_default();
            set_effective_app_language(&state, language);
            let menu = native_tray_menu(app, language)?;
            let mut tray = TrayIconBuilder::with_id(TRAY_MAIN_ID)
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
            get_restore_status,
            inspect_restore_backup,
            apply_restore,
            cancel_restore,
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
            save_app_language,
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
