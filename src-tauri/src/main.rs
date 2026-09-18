#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
#[cfg_attr(not(debug_assertions), allow(dead_code))]
mod provisioning;
#[cfg_attr(not(debug_assertions), allow(dead_code))]
mod runtime;
mod secret;

use config::{AppConfig, StartupView, Store};
#[cfg(debug_assertions)]
use provisioning::Provisioner;
use provisioning::ProvisioningInfo;
#[cfg(debug_assertions)]
use provisioning::{
    ProvisioningState, WORDPRESS_ADMIN_EMAIL, WORDPRESS_ADMIN_SECRET, WORDPRESS_ADMIN_USER,
};
use runtime::{RuntimeInfo, RuntimeManager, RuntimeState};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, TryLockError};
use tauri::{Manager, State};

#[derive(Default)]
struct ShellState {
    store: Mutex<Option<Store>>,
    runtime: Mutex<Option<RuntimeManager>>,
    lifecycle: Mutex<()>,
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

#[derive(Serialize)]
struct SetupInfo {
    store_name: String,
    admin_username: String,
    admin_email: String,
    password_configured: bool,
    editable: bool,
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
    if provisioning_guard.is_some() {
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

#[tauri::command]
fn get_shell_info(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<ShellInfo, String> {
    with_store(&app, &state, |_| Ok(()))
}

#[tauri::command]
fn save_app_settings(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
    startup_view: StartupView,
) -> Result<ShellInfo, String> {
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
    let _lifecycle_guard = try_lifecycle(&state, "start the runtime")?;
    with_runtime(&app, &state, RuntimeManager::start)
}

#[tauri::command]
fn stop_runtime(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<RuntimeInfo, String> {
    let _lifecycle_guard = try_lifecycle(&state, "stop the runtime")?;
    with_runtime(&app, &state, RuntimeManager::stop)
}

#[tauri::command]
fn restart_runtime(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<RuntimeInfo, String> {
    let _lifecycle_guard = try_lifecycle(&state, "restart the runtime")?;
    with_runtime(&app, &state, RuntimeManager::restart)
}

#[tauri::command]
fn retry_runtime_health(
    app: tauri::AppHandle,
    state: State<'_, ShellState>,
) -> Result<RuntimeInfo, String> {
    let _lifecycle_guard = try_lifecycle(&state, "recheck runtime health")?;
    with_runtime(&app, &state, |runtime| {
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
        .invoke_handler(tauri::generate_handler![
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
